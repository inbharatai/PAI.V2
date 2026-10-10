package com.unoone.agent.vault

import java.io.File
import java.nio.ByteBuffer
import java.nio.channels.FileChannel
import java.nio.file.Files
import java.nio.file.LinkOption.NOFOLLOW_LINKS
import java.nio.file.StandardCopyOption.ATOMIC_MOVE
import java.nio.file.StandardCopyOption.REPLACE_EXISTING
import java.nio.file.StandardOpenOption.*
import java.util.UUID
import java.nio.file.attribute.PosixFilePermissions

/** App-private, single-process vault IO. Root must be under Context.noBackupFilesDir.
 * No raw vault cloning/Android backup: a fresh install must mint a distinct vault/key.
 * No symlinks, traversal, special files, in-place overwrites, or non-atomic fallback.
 * Interrupted .pending files are retained for assisted recovery, never auto-promoted/deleted.
 * App sandbox is the writer boundary; this is not a defence against a compromised same UID.
 */
class PrivateFileVaultIO(
    root: File,
    private val syncDirectory: (File) -> Unit = { dir ->
        FileChannel.open(dir.toPath(), READ).use { it.force(true) }
    },
    private val maxBytes: Int = 16 * 1024 * 1024,
    private val maxEntries: Int = 100_000,
    /** Fault injection only; production leaves this no-op. Called AFTER temp file fsync. */
    private val beforeAtomicReplace: () -> Unit = {},
) : VaultIO {
    private val root = root.absoluteFile.toPath().normalize().toFile()

    init {
        require(maxBytes > 0 && maxEntries > 0)
        verifyAncestors(this.root)
        if (!Files.exists(this.root.toPath(), NOFOLLOW_LINKS)) {
            require(checkNotNull(this.root.parentFile).isDirectory) { "Vault parent must already exist" }
            Files.createDirectory(this.root.toPath(), PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwx------")))
        }
        require(Files.isDirectory(this.root.toPath(), NOFOLLOW_LINKS)) { "Vault root is not a directory" }
        // Also retry this fsync for an existing root left by an interrupted first mkdir.
        syncDirectory(checkNotNull(this.root.parentFile))
    }

    private fun verifyAncestors(file: File) {
        var at: File? = file
        while (at != null) {
            require(!Files.isSymbolicLink(at.toPath())) { "Vault symlinks are forbidden" }
            at = at.parentFile
        }
        require(file.canonicalFile == file.absoluteFile) { "Unsafe vault path" }
    }

    private fun resolve(relative: String, allowRoot: Boolean = false): File {
        require(relative.isNotEmpty() || allowRoot) { "Empty file path" }
        require(relative.length <= 512 && !relative.startsWith('/') && '\\' !in relative) { "Unsafe vault path" }
        if (relative.isNotEmpty()) require(relative.split('/').all {
            it.isNotEmpty() && it != "." && it != ".." && it.all { c -> c.isLetterOrDigit() || c in "._-" }
        }) { "Unsafe vault segment" }
        val file = if (relative.isEmpty()) root else File(root, relative)
        verifyAncestors(file)
        return file
    }

    @Synchronized override fun read(relativePath: String): ByteArray {
        val file = resolve(relativePath)
        require(Files.isRegularFile(file.toPath(), NOFOLLOW_LINKS)) { "Vault file missing or not regular" }
        FileChannel.open(file.toPath(), READ, NOFOLLOW_LINKS).use { channel ->
            require(channel.size() <= maxBytes) { "Vault file exceeds size bound" }
            val out = java.io.ByteArrayOutputStream()
            val buf = ByteBuffer.allocate(8192)
            while (channel.read(buf) != -1) {
                buf.flip()
                require(out.size().toLong() + buf.remaining() <= maxBytes) { "Vault file grew beyond size bound" }
                out.write(buf.array(), 0, buf.remaining())
                buf.clear()
            }
            return out.toByteArray()
        }
    }

    @Synchronized override fun write(relativePath: String, bytes: ByteArray) {
        require(bytes.size <= maxBytes) { "Vault write exceeds size bound" }
        val target = resolve(relativePath)
        val parents = generateSequence(target.parentFile) { if (it == root) null else it.parentFile }.toList().asReversed()
        for (dir in parents) {
            verifyAncestors(dir)
            if (!Files.exists(dir.toPath(), NOFOLLOW_LINKS)) {
                Files.createDirectory(dir.toPath(), PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwx------")))
                syncDirectory(checkNotNull(dir.parentFile))
            }
            require(Files.isDirectory(dir.toPath(), NOFOLLOW_LINKS)) { "Vault parent is not a directory" }
        }
        if (Files.exists(target.toPath(), NOFOLLOW_LINKS)) {
            require(Files.isRegularFile(target.toPath(), NOFOLLOW_LINKS)) { "Vault target is not regular" }
        }
        val temporary = resolve(relativePath + ".pending-" + UUID.randomUUID())
        FileChannel.open(temporary.toPath(), setOf(CREATE_NEW, WRITE, NOFOLLOW_LINKS),
            PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rw-------"))).use { channel ->
            val data = ByteBuffer.wrap(bytes)
            while (data.hasRemaining()) channel.write(data)
            channel.force(true)
        }
        beforeAtomicReplace()
        verifyAncestors(target)
        Files.move(temporary.toPath(), target.toPath(), ATOMIC_MOVE, REPLACE_EXISTING)
        syncDirectory(checkNotNull(target.parentFile))
    }

    @Synchronized override fun exists(relativePath: String): Boolean =
        Files.exists(resolve(relativePath, allowRoot = true).toPath(), NOFOLLOW_LINKS)

    @Synchronized override fun list(relativePath: String): List<String> {
        val dir = resolve(relativePath, allowRoot = true)
        if (!Files.exists(dir.toPath(), NOFOLLOW_LINKS)) return emptyList()
        require(Files.isDirectory(dir.toPath(), NOFOLLOW_LINKS)) { "Not a vault directory" }
        return Files.newDirectoryStream(dir.toPath()).use { stream ->
            val names = ArrayList<String>()
            for (path in stream) {
                require(names.size < maxEntries) { "Vault directory exceeds entry bound" }
                verifyAncestors(path.toFile())
                require(Files.isRegularFile(path, NOFOLLOW_LINKS) || Files.isDirectory(path, NOFOLLOW_LINKS))
                names.add(path.fileName.toString())
            }
            names.sorted()
        }
    }

    /** Record retention is explicit: this adapter never physically deletes user vault data. */
    override fun delete(relativePath: String): Boolean =
        throw VaultAccessException("Physical vault deletion is disabled; use authenticated tombstones")
}
