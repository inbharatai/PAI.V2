package com.unoone.agent.modelmanager

import java.io.File
import java.io.FileOutputStream
import java.io.RandomAccessFile
import java.nio.channels.FileChannel
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.StandardCopyOption
import java.nio.file.StandardOpenOption
import java.security.MessageDigest
import java.util.UUID

/** Versioned immutable bundle store. The sole commit point is active-v1's atomic rename.
 * Legacy files are read-only fallback when no pointer exists; never silently migrated or overwritten.
 * Hash-bound sealed bundles survive interruption at every phase. Recovery NEVER activates a stage.
 */
class ModelBundleStore(private val base: File) {
    class Staged internal constructor(val modelId: String, val identity: String, val bundleId: String, val root: File)
    enum class NativeResult { PASSED, BAD_LOAD, BAD_SMOKE, OOM, CANCELLED }
    /** Implement only with a native retained-old-instance transaction. UI/sync receipts are not evidence. */
    interface NativeLoader {
        val retainsPreviousInstance: Boolean
        fun freshAdmissionError(): String?
        fun loadAndSmoke(candidateRoot: File): NativeResult
        fun commitRouting()
        fun rollbackCandidate()
    }
    private fun modelRoot(id: String): File {
        require(id.matches(Regex("[A-Za-z0-9][A-Za-z0-9._-]{0,159}")))
        val root = File(base, ".bundles-v1/$id")
        check(root.isDirectory || root.mkdirs())
        require(root.canonicalPath.startsWith(base.canonicalPath + File.separator))
        require(!Files.isSymbolicLink(root.toPath()))
        return root
    }
    fun <T> locked(id: String, block: () -> T): T {
        val root = modelRoot(id)
        return RandomAccessFile(File(root, ".lock"), "rw").use { file ->
            file.channel.lock().use { block() }
        }
    }
    fun stageDirectory(id: String, identity: String): File {
        require(identity.matches(Regex("[a-f0-9]{64}")))
        return File(modelRoot(id), "partial-$identity").also {
            require(!Files.isSymbolicLink(it.toPath()))
            check(it.isDirectory || it.mkdir())
        }
    }
    fun seal(id: String, identity: String, manifest: String): Staged {
        require(identity == sha256(manifest.toByteArray(Charsets.UTF_8))) { "Manifest identity mismatch" }
        val root = modelRoot(id)
        val stage = stageDirectory(id, identity)
        val manifestFile = File(stage, MANIFEST)
        // Resume after a process died before sealing: recompute rather than trust a stale receipt.
        val contents = "unoone-bundle-v1\n$identity\n${sha256(manifest.toByteArray())}\n" + inventory(stage)
        writeSync(manifestFile, contents)
        writeSync(File(stage, DESCRIPTOR), manifest)
        syncDirectory(stage)
        val bundleId = "bundle-${UUID.randomUUID()}"
        val sealed = File(root, bundleId)
        Files.move(stage.toPath(), sealed.toPath(), StandardCopyOption.ATOMIC_MOVE)
        syncDirectory(root)
        return Staged(id, identity, bundleId, sealed).also { check(verify(it)) }
    }
    fun verify(stage: Staged): Boolean = runCatching {
        val root = modelRoot(stage.modelId)
        require(stage.bundleId.matches(Regex("bundle-[a-f0-9-]{36}")))
        require(stage.root.canonicalFile == File(root, stage.bundleId).canonicalFile)
        val expected = "unoone-bundle-v1\n${stage.identity}\n${sha256(File(stage.root, DESCRIPTOR).readBytes())}\n" + inventory(stage.root)
        !Files.isSymbolicLink(stage.root.toPath()) && File(stage.root, MANIFEST).readText() == expected
    }.getOrDefault(false)

    /** Only call with an adapter that genuinely keeps the old native model and routing alive.
     * No production adapter is supplied yet; therefore downloads remain STAGED, never Ready.
     */
    fun activate(stage: Staged, loader: NativeLoader): Boolean = locked(stage.modelId) {
        check(loader.retainsPreviousInstance) { "Transactional native loader unavailable; old model retained" }
        check(verify(stage)) { "Staged bundle changed" }
        check(loader.freshAdmissionError() == null) { "Fresh native admission denied" }
        var pointerCommitted = false
        try {
            val result = loader.loadAndSmoke(stage.root)
            if (result != NativeResult.PASSED) return@locked false
            check(loader.freshAdmissionError() == null) { "Admission expired during load" }
            check(verify(stage)) { "Bundle changed during load" }
            val root = modelRoot(stage.modelId)
            val pointer = File(root, POINTER)
            val previous = if (pointer.isFile) pointer.readBytes() else null
            val temp = File(root, "pointer-${UUID.randomUUID()}.tmp")
            writeSync(temp, "unoone-active-v1\n${stage.bundleId}\n${stage.identity}\n")
            // If routing throws, restore the previous pointer; never delete either immutable bundle.
            Files.move(temp.toPath(), pointer.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
            try {
                syncDirectory(root)
                loader.commitRouting()
                pointerCommitted = true
            } catch (failure: Throwable) {
                if (previous != null) {
                    FileOutputStream(temp).use { it.write(previous); it.fd.sync() }
                    Files.move(temp.toPath(), pointer.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
                } else Files.deleteIfExists(pointer.toPath())
                syncDirectory(root)
                throw failure
            }
            true
        } catch (_: OutOfMemoryError) { false }
        finally { if (!pointerCommitted) loader.rollbackCandidate() }
    }
    fun active(id: String): Staged? {
        val root = modelRoot(id); val pointer = File(root, POINTER)
        if (!pointer.exists()) return null
        require(!Files.isSymbolicLink(pointer.toPath()))
        val lines = pointer.readLines()
        require(lines.size == 3 && lines[0] == "unoone-active-v1") { "Unsupported/corrupt active pointer" }
        val stage = Staged(id, lines[2], lines[1], File(root, lines[1]))
        check(verify(stage)) { "Active bundle integrity failed; no legacy fallback" }
        return stage
    }
    fun hasPointer(id: String) = File(modelRoot(id), POINTER).exists()
    fun latestStaged(id: String, identity: String): Staged? = modelRoot(id).listFiles().orEmpty()
        .filter { it.name.startsWith("bundle-") }.sortedByDescending { it.lastModified() }
        .map { Staged(id, identity, it.name, it) }.firstOrNull { verify(it) }

    /** Explicit partial cleanup only. Never traverses legacy, sealed or active directories. */
    fun cleanupPartial(id: String, identity: String) = locked(id) {
        require(identity.matches(Regex("[a-f0-9]{64}")))
        val partial = File(modelRoot(id), "partial-$identity")
        require(!Files.isSymbolicLink(partial.toPath()))
        if (partial.exists()) {
            partial.walkBottomUp().forEach { require(!Files.isSymbolicLink(it.toPath())); check(it.delete()) }
            syncDirectory(modelRoot(id))
        }
    }
    /** Explicit pruning of sealed-but-never-activated bundles. The active bundle (and the pointer)
     * are never touched, even when its inventory no longer verifies; partial stages are left for
     * [cleanupPartial]. Returns the removed bundle ids.
     */
    fun pruneInactiveBundles(id: String): List<String> = locked(id) {
        val root = modelRoot(id)
        val pointer = File(root, POINTER)
        val activeId = if (pointer.isFile) pointer.readLines().getOrNull(1) else null
        root.listFiles().orEmpty()
            .filter { it.name.startsWith("bundle-") && it.name != activeId && !Files.isSymbolicLink(it.toPath()) }
            .map { dir ->
                dir.walkBottomUp().forEach { require(!Files.isSymbolicLink(it.toPath())); check(it.delete()) }
                dir.name
            }.also { if (it.isNotEmpty()) syncDirectory(root) }
    }

    /** Explicit user uninstall of EVERY version of one model id: pointer is removed first so a
     * crash mid-way leaves no pointer to a half-deleted bundle (reads fall back to legacy layout
     * only if that layout still exists). Never touches other model ids or the legacy folder.
     */
    fun uninstallAll(id: String) = locked(id) {
        val root = modelRoot(id)
        Files.deleteIfExists(File(root, POINTER).toPath())
        syncDirectory(root)
        root.listFiles().orEmpty()
            .filter { (it.name.startsWith("bundle-") || it.name.startsWith("partial-")) && !Files.isSymbolicLink(it.toPath()) }
            .forEach { dir -> dir.walkBottomUp().forEach { require(!Files.isSymbolicLink(it.toPath())); check(it.delete()) } }
        syncDirectory(root)
    }

    companion object {
        private const val MANIFEST = ".bundle-manifest-v1"
        private const val DESCRIPTOR = ".bundle-descriptor-v1.json"
        private const val POINTER = "active-v1"
        fun sha256(bytes: ByteArray): String = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
        private fun writeSync(file: File, contents: String) {
            require(!Files.isSymbolicLink(file.toPath()))
            FileOutputStream(file).use { it.write(contents.toByteArray(Charsets.UTF_8)); it.fd.sync() }
        }
        private fun syncDirectory(root: File) { FileChannel.open(root.toPath(), StandardOpenOption.READ).use { it.force(true) } }
        private fun inventory(root: File): String {
            require(Files.isDirectory(root.toPath(), LinkOption.NOFOLLOW_LINKS))
            val files = root.walkTopDown().filter { it != root }.mapNotNull { file ->
                require(!Files.isSymbolicLink(file.toPath()))
                if (file.isDirectory) return@mapNotNull null
                require(Files.isRegularFile(file.toPath(), LinkOption.NOFOLLOW_LINKS))
                if (file.parentFile == root && file.name in listOf(MANIFEST, DESCRIPTOR)) return@mapNotNull null
                val path = file.relativeTo(root).invariantSeparatorsPath
                require(!path.contains('\n') && !path.contains('\t') && !path.endsWith(".part"))
                val hash = MessageDigest.getInstance("SHA-256")
                file.inputStream().use { input -> val buffer = ByteArray(65536)
                    while (true) { val n = input.read(buffer); if (n < 0) break; hash.update(buffer, 0, n) }
                }
                "$path\t${file.length()}\t${hash.digest().joinToString("") { "%02x".format(it) }}"
            }.toList().sorted()
            require(files.isNotEmpty())
            return files.joinToString("\n")
        }
    }
}
