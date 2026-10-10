package com.unoone.agent.storage.cache

import java.io.File
import java.io.FileOutputStream
import java.nio.file.Files
import java.nio.file.LinkOption
import java.security.MessageDigest

/** File-only protocol. The caller holds exclusive startup admission (no open Room handles).
 * Originals are NEVER opened by SQLite, checkpointed, truncated or automatically deleted.
 * All paths are fixed children, never taken from the journal. Directory fsync is mandatory.
 */
class PlaintextUpgradeFiles(
    database: File,
    root: File,
    private val syncDirectory: (File) -> Unit,
    private val checkpoint: (String) -> Unit = {},
) {
    // Android may expose /data/user/0 through a platform symlink. Normalize trusted parents,
    // never the final DB/recovery entry itself (a symlink there is refused).
    val database = File(database.parentFile!!.canonicalFile, database.name)
    val root = File(root.parentFile!!.canonicalFile, root.name)
    enum class Phase { PREPARED, VERIFIED, PROMOTING, DONE, CLEANING, CLEANED }
    data class Journal(val phase: Phase, val originals: List<String>, val candidateHash: String = "-")
    val snapshot = File(this.root, "snapshot")
    val candidate = File(this.root, "encrypted")
    val backup = File(this.root, "original")
    private val journalFile = File(this.root, "state")
    private val suffixes = listOf("", "-wal", "-shm", "-journal")

    fun exists(): Boolean = Files.exists(root.toPath(), LinkOption.NOFOLLOW_LINKS)
    fun read(): Journal {
        require(Files.isDirectory(root.toPath(), LinkOption.NOFOLLOW_LINKS) && root.canonicalFile == root.absoluteFile) { "Unsafe recovery directory" }
        if (!Files.exists(journalFile.toPath(), LinkOption.NOFOLLOW_LINKS)) {
            // Only initial mkdir/state.next can be resumed. Any scratch/backup indicates a later
            // phase with lost state: never infer PREPARED and delete its recovery material.
            val children = requireNotNull(root.list()).toSet()
            require(children.isEmpty() || children == setOf("state.next")) { "Ambiguous initialization" }
            regular(database)
            require(isPlaintext(database)) { "Initialization requires intact plaintext source" }
            val initial = if (children.isEmpty()) Journal(Phase.PREPARED, originalHashes())
                else parse(File(root, "state.next"))
            require(initial.phase == Phase.PREPARED && initial.candidateHash == "-")
            requireOriginals(initial)
            require(suffixes.sumOf { File(database.path + it).length() } in 100..MAX_SOURCE_BYTES) { "Source exceeds safe upgrade limit" }
            return initial // inspection only; explicit consent commits it in resumeInitialization
        }
        return parse(journalFile)
    }

    fun resumeInitialization(consent: Boolean): Journal {
        require(consent) { "Plaintext retention consent required" }
        val journal = read()
        if (!Files.exists(journalFile.toPath(), LinkOption.NOFOLLOW_LINKS)) {
            requireOriginals(journal)
            save(journal)
        }
        return journal
    }

    private fun parse(file: File): Journal {
        regular(file)
        require(file.length() in 1..1024) { "Invalid upgrade journal" }
        val parts = file.readText(Charsets.US_ASCII).trimEnd('\n').split('\n')
        require(parts.size == 8 && parts[0] == "PAI-UPGRADE-1" && parts[1] == "CONSENT-RETAIN-PLAINTEXT")
        require(parts.drop(3).all { it == "-" || it.matches(Regex("[a-f0-9]{64}")) })
        return Journal(Phase.valueOf(parts[2]), parts.subList(3, 7), parts[7]).also {
            require(it.originals[0] != "-")
            require(it.phase == Phase.PREPARED || it.candidateHash != "-")
        }
    }

    /** Must only be called after the user accepts the retention explanation. */
    fun prepare(consent: Boolean): Journal {
        require(consent) { "Plaintext retention consent required" }
        require(!root.exists()) { "Upgrade state already exists; do not overwrite recovery files" }
        regular(database)
        require(isPlaintext(database)) { "Not a plaintext SQLite database" }
        val hashes = originalHashes()
        val total = suffixes.sumOf { File(database.path + it).length() }
        require(total in 100..MAX_SOURCE_BYTES) { "Source exceeds safe upgrade limit" }
        require(database.parentFile!!.usableSpace > total * 5 + RESERVE_BYTES) { "Insufficient upgrade disk space" }
        require(root.mkdir())
        syncDirectory(root.parentFile!!)
        checkpoint("directory-created")
        val journal = Journal(Phase.PREPARED, hashes)
        save(journal)
        checkpoint("prepared")
        return journal
    }

    fun copySnapshot(journal: Journal) {
        require(journal.phase == Phase.PREPARED)
        requireOriginals(journal)
        // Only generated scratch files may be removed; the sole original remains untouched.
        for (base in listOf(snapshot, candidate)) for (suffix in suffixes) {
            val file = File(base.path + suffix)
            if (Files.exists(file.toPath(), LinkOption.NOFOLLOW_LINKS)) { regular(file); require(file.delete()) }
        }
        val total = suffixes.sumOf { File(database.path + it).length() }
        require(root.usableSpace > total * 5 + RESERVE_BYTES) { "Insufficient upgrade disk space" }
        suffixes.forEachIndexed { index, suffix ->
            if (journal.originals[index] != "-") {
                val source = File(database.path + suffix)
                val target = File(snapshot.path + suffix)
                FileOutputStream(target).use { output ->
                    source.inputStream().use { input -> input.copyTo(output) }
                    output.fd.sync()
                }
                require(hash(target) == journal.originals[index]) { "Snapshot changed" }
            }
        }
        syncDirectory(root)
        requireOriginals(journal)
        checkpoint("snapshot")
    }

    fun verified(journal: Journal): Journal {
        requireOriginals(journal)
        require(!isPlaintext(candidate) && candidate.length() >= 4096)
        requireNoSidecars(candidate)
        FileOutputStream(candidate, true).use { it.fd.sync() }
        return journal.copy(phase = Phase.VERIFIED, candidateHash = hash(candidate)).also {
            save(it); checkpoint("verified")
        }
    }

    /** Reentrant for every individual rename. Never overwrites an existing destination. */
    fun promote(input: Journal) {
        var journal = input
        require(journal.phase == Phase.VERIFIED || journal.phase == Phase.PROMOTING)
        if (journal.phase == Phase.VERIFIED) {
            requireOriginals(journal)
            require(hash(candidate) == journal.candidateHash)
            requireNoSidecars(candidate)
            journal = journal.copy(phase = Phase.PROMOTING)
            save(journal)
        }
        // Once the candidate moved, original sidecars must already all be in backup.
        val promoted = !candidate.exists()
        if (promoted) require(hash(database) == journal.candidateHash)
        suffixes.forEachIndexed { index, suffix ->
            val source = File(database.path + suffix)
            val target = File(backup.path + suffix)
            val expected = journal.originals[index]
            if (expected == "-") {
                require(!target.exists())
                if (!(promoted && index == 0)) require(!source.exists())
            } else if (target.exists()) {
                require(hash(target) == expected)
                if (!(promoted && index == 0)) require(!source.exists())
            } else {
                require(!promoted && hash(source) == expected)
                rename(source, target)
                require(target.setReadOnly()) { "Cannot protect retained original" }
                checkpoint("backup$index")
            }
        }
        if (!promoted) {
            require(hash(candidate) == journal.candidateHash)
            requireNoSidecars(candidate)
            rename(candidate, database)
            checkpoint("promoted")
        }
        save(journal.copy(phase = Phase.DONE))
        checkpoint("done")
    }

    /** v1 records have no stable encrypted-store AND wrapped-key lineage binding. Authentication
     * alone could accept a substituted valid store/key pair and delete the sole old originals.
     * Keep the API for callers, but refuse every destructive cleanup (including old CLEANING).
     */
    @Suppress("UNUSED_PARAMETER")
    fun cleanup(consent: Boolean, authenticateCurrent: () -> Unit) {
        require(consent)
        val journal = read()
        if (journal.phase == Phase.CLEANED) return
        throw IllegalStateException("Cleanup refused: migration store/key lineage is not proven. Retain originals.")
    }

    fun requireOriginals(journal: Journal) {
        require(originalHashes() == journal.originals) { "Original database changed; manual recovery required" }
    }

    fun requireBackup(journal: Journal) {
        suffixes.forEachIndexed { index, suffix ->
            val file = File(backup.path + suffix)
            require((if (file.exists()) hash(file) else "-") == journal.originals[index])
        }
    }

    private fun originalHashes() = suffixes.map { suffix ->
        File(database.path + suffix).let { if (Files.exists(it.toPath(), LinkOption.NOFOLLOW_LINKS)) hash(it) else "-" }
    }

    private fun save(journal: Journal) {
        val temp = File(root, "state.next")
        if (Files.exists(temp.toPath(), LinkOption.NOFOLLOW_LINKS)) regular(temp)
        FileOutputStream(temp).use {
            it.write((listOf("PAI-UPGRADE-1", "CONSENT-RETAIN-PLAINTEXT", journal.phase.name)
                .plus(journal.originals).plus(journal.candidateHash).joinToString("\n", postfix = "\n")).toByteArray(Charsets.US_ASCII))
            it.fd.sync()
        }
        checkpoint("journal-synced:${journal.phase.name}")
        require(temp.renameTo(journalFile)) { "Journal persist failed" }
        syncDirectory(root)
    }

    private fun rename(source: File, target: File) {
        regular(source)
        require(!Files.exists(target.toPath(), LinkOption.NOFOLLOW_LINKS) && source.renameTo(target)) { "Upgrade rename failed" }
        syncDirectory(source.parentFile!!)
        if (source.parentFile != target.parentFile) syncDirectory(target.parentFile!!)
    }

    companion object {
        const val MAX_SOURCE_BYTES = 256L * 1024 * 1024
        const val RESERVE_BYTES = 32L * 1024 * 1024
        fun regular(file: File) {
            require(Files.isRegularFile(file.toPath(), LinkOption.NOFOLLOW_LINKS)) { "Unsafe upgrade file" }
            require(file.canonicalFile == file.absoluteFile) { "Unsafe upgrade path" }
            require(file.length() <= MAX_SOURCE_BYTES) { "Upgrade file exceeds limit" }
        }
        fun isPlaintext(file: File): Boolean = file.isFile && file.inputStream().use {
            val header = ByteArray(16)
            it.read(header) == 16 && header.contentEquals("SQLite format 3\u0000".toByteArray(Charsets.US_ASCII))
        }
        fun hash(file: File): String {
            regular(file)
            val digest = MessageDigest.getInstance("SHA-256")
            file.inputStream().use { input ->
                val buffer = ByteArray(65536)
                while (true) { val count = input.read(buffer); if (count < 0) break; digest.update(buffer, 0, count) }
            }
            return digest.digest().joinToString("") { "%02x".format(it) }
        }
        fun requireNoSidecars(file: File) {
            listOf("-wal", "-shm", "-journal").forEach { suffix ->
                val sidecar = File(file.path + suffix)
                if (Files.exists(sidecar.toPath(), LinkOption.NOFOLLOW_LINKS)) {
                    regular(sidecar)
                    require(sidecar.length() == 0L) { "Candidate has live SQLite sidecars" }
                }
            }
        }
    }
}
