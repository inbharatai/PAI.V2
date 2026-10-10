package com.unoone.agent.storage.cache

import java.io.File
import java.io.FileOutputStream
import java.nio.file.Files
import java.nio.file.LinkOption.NOFOLLOW_LINKS
import java.util.UUID

/** Exclusive-startup snapshot for native journal recovery admission. Immutable retained bytes
 * and a separate writable probe: SQLCipher may recover/checkpoint the probe, NEVER the only copy.
 * Failure retains every copied file. No cleanup authority is implied by a successful probe.
 */
class EncryptedRecoveryCopy(
    database: File,
    recoveryParent: File,
    private val syncDirectory: (File) -> Unit,
) {
    private val database = File(database.parentFile!!.canonicalFile, database.name)
    private val parent = File(recoveryParent.parentFile!!.canonicalFile, recoveryParent.name)
    private val suffixes = listOf("", "-wal", "-shm", "-journal")
    data class Snapshot(val retained: File, val probe: File, val hashes: List<String>)

    fun create(): Snapshot {
        val hashes = sourceHashes()
        require(hashes[0] != "-") { "Missing encrypted main file" }
        val total = suffixes.sumOf { File(database.path + it).length() }
        require(total in 16..PlaintextUpgradeFiles.MAX_SOURCE_BYTES)
        if (!Files.exists(parent.toPath(), NOFOLLOW_LINKS)) {
            require(parent.mkdir()); syncDirectory(parent.parentFile!!)
        }
        require(Files.isDirectory(parent.toPath(), NOFOLLOW_LINKS) && parent.canonicalFile == parent.absoluteFile)
        require(parent.usableSpace > total * 2 + PlaintextUpgradeFiles.RESERVE_BYTES) { "Insufficient recovery space" }
        val root = File(parent, "attempt-${UUID.randomUUID()}")
        require(root.mkdir()); syncDirectory(parent)
        val retained = File(root, "original")
        val probe = File(root, "probe")
        for ((index, suffix) in suffixes.withIndex()) if (hashes[index] != "-") {
            val source = File(database.path + suffix)
            val original = File(retained.path + suffix)
            copy(source, original, hashes[index])
            require(original.setReadOnly())
            copy(original, File(probe.path + suffix), hashes[index])
        }
        syncDirectory(root)
        require(sourceHashes() == hashes) { "Live database changed during startup recovery snapshot" }
        return Snapshot(retained, probe, hashes)
    }

    fun requireUnchanged(snapshot: Snapshot) {
        require(sourceHashes() == snapshot.hashes) { "Live database changed during recovery admission" }
        suffixes.forEachIndexed { index, suffix ->
            val file = File(snapshot.retained.path + suffix)
            require((if (Files.exists(file.toPath(), NOFOLLOW_LINKS)) PlaintextUpgradeFiles.hash(file) else "-") == snapshot.hashes[index])
        }
    }

    private fun copy(source: File, target: File, hash: String) {
        require(target.createNewFile())
        FileOutputStream(target).use { output -> source.inputStream().use { it.copyTo(output) }; output.fd.sync() }
        require(PlaintextUpgradeFiles.hash(target) == hash)
    }

    private fun sourceHashes() = suffixes.map { suffix ->
        val file = File(database.path + suffix)
        if (Files.exists(file.toPath(), NOFOLLOW_LINKS)) PlaintextUpgradeFiles.hash(file) else "-"
    }
}
