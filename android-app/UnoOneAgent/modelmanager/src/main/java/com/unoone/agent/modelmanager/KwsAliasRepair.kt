package com.unoone.agent.modelmanager

import java.io.File
import java.security.MessageDigest

/**
 * Materialises the wake-word (KWS) pack folder from an already verified English ASR folder when
 * both manifest entries declare byte-identical artifacts. Speech packs have no native brain
 * transaction; the contract is per-file: copy to `.part`, verify exact size + SHA-256, then rename.
 * A destination that already verifies is left untouched. Any mismatch deletes only the `.part`.
 *
 * This restores the standalone behaviour that the integration had replaced with a permanent
 * `false` ("pending transactional native activation"), which silently broke wake-word setup.
 */
object KwsAliasRepair {
    fun manifestsIdentical(source: ModelDescriptor, target: ModelDescriptor): Boolean {
        val sourceFiles = source.files.filterNot { it.archive }.associateBy { it.name }
        val targetFiles = target.files.filterNot { it.archive }.associateBy { it.name }
        return sourceFiles.isNotEmpty() && sourceFiles.keys == targetFiles.keys &&
            sourceFiles.all { (name, file) ->
                val other = targetFiles[name]
                other != null && file.sizeBytes == other.sizeBytes && file.sizeBytes > 0L &&
                    file.sha256.isNotBlank() && file.sha256.equals(other.sha256, ignoreCase = true)
            }
    }

    /** Returns true only when every declared target file is present and verified afterwards. */
    fun copyVerified(sourceFolder: File, targetFolder: File, files: List<ModelFile>): Boolean {
        if (!targetFolder.isDirectory && !targetFolder.mkdirs()) return false
        for (descriptor in files.filterNot { it.archive }) {
            val sourceFile = File(sourceFolder, descriptor.name)
            val destination = File(targetFolder, descriptor.name)
            if (verified(destination, descriptor)) continue
            if (!verified(sourceFile, descriptor)) return false
            val part = File(targetFolder, "${descriptor.name}.part")
            runCatching { part.delete() }
            try {
                sourceFile.copyTo(part, overwrite = true)
                if (!verified(part, descriptor)) { part.delete(); return false }
                if (destination.exists() && !destination.delete()) { part.delete(); return false }
                if (!part.renameTo(destination)) { part.delete(); return false }
            } catch (e: Exception) {
                part.delete()
                return false
            }
        }
        return files.filterNot { it.archive }.all { verified(File(targetFolder, it.name), it) }
    }

    fun verified(file: File, descriptor: ModelFile): Boolean =
        file.isFile && file.length() == descriptor.sizeBytes && sha256(file) == descriptor.sha256.lowercase()

    private fun sha256(file: File): String? = runCatching {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(65536)
            while (true) { val n = input.read(buffer); if (n < 0) break; digest.update(buffer, 0, n) }
        }
        digest.digest().joinToString("") { "%02x".format(it) }
    }.getOrNull()
}
