package com.unoone.agent.vault

import android.content.Context
import android.net.Uri
import androidx.documentfile.provider.DocumentFile
import java.io.ByteArrayOutputStream

/**
 * SAF DocumentFile adapter for VaultIO. All crypto/decisions live in
 * MobileVaultRepository; this class only maps relative vault paths onto a
 * user-granted document tree. It is the deliberately thin,
 * hardware-verified layer (JVM-untestable without a device → covered by the
 * physical phone journey; logic coverage sits in MobileVaultRepositoryTest).
 */
class SafVaultIO(
    private val context: Context,
    treeUri: Uri,
    private val readOnly: Boolean = true,
) : VaultIO {

    private val resolver: android.content.ContentResolver = context.contentResolver

    private fun validatePath(relativePath: String) {
        require(relativePath.length in 1..512 && !relativePath.startsWith('/') && '\\' !in relativePath)
        require(relativePath.split('/').all { it.isNotEmpty() && it != "." && it != ".." &&
            it.all { c -> c.isLetterOrDigit() || c in "._-" } })
    }

    private fun resolve(relativePath: String): DocumentFile? {
        validatePath(relativePath)
        var node = DocumentFile.fromTreeUri(context, treeUri) ?: return null
        for (segment in relativePath.split('/')) {
            if (segment.isEmpty()) continue
            val next = node.findFile(segment) ?: return null
            node = next
        }
        return node
    }

    private val treeUri: Uri = treeUri

    override fun read(relativePath: String): ByteArray {
        val file = resolve(relativePath)
            ?: throw VaultAccessException("not found on SAF tree: $relativePath")
        resolver.openInputStream(file.uri)?.use { input ->
            val out = ByteArrayOutputStream()
            val buffer = ByteArray(8192)
            while (true) {
                val count = input.read(buffer)
                if (count < 0) break
                require(out.size().toLong() + count <= 16 * 1024 * 1024) { "Legacy file exceeds size bound" }
                out.write(buffer, 0, count)
            }
            return out.toByteArray()
        } ?: throw VaultAccessException("cannot open for read: $relativePath")
    }

    override fun write(relativePath: String, bytes: ByteArray) {
        check(!readOnly) { "Legacy source is read-only; import requires review" }
        validatePath(relativePath)
        require(bytes.size <= 16 * 1024 * 1024)
        val segments = relativePath.split('/').filter { it.isNotEmpty() }
        var node = DocumentFile.fromTreeUri(context, treeUri)
            ?: throw VaultAccessException("vault tree unreachable: $treeUri")
        for (dirSegment in segments.dropLast(1)) {
            val existing = node.findFile(dirSegment)
            node = if (existing != null && existing.isDirectory) existing
                else node.createDirectory(dirSegment)
                    ?: throw VaultAccessException("cannot create directory: $dirSegment")
        }
        val name = segments.last()
        val existing = node.findFile(name)
        val file = if (existing != null && existing.isFile) existing
            else node.createFile("application/json", name)
                ?: throw VaultAccessException("cannot create file: $name")
        resolver.openOutputStream(file.uri, "wt")?.use { out ->
            out.write(bytes)
        } ?: throw VaultAccessException("cannot open for write: $relativePath")
    }

    override fun exists(relativePath: String): Boolean = resolve(relativePath) != null

    override fun list(relativePath: String): List<String> =
        (resolve(relativePath)?.listFiles()?.mapNotNull { it.name } ?: emptyList()).also {
            require(it.size <= 100_000) { "Legacy source exceeds entry bound" }
            it.forEach { name -> validatePath(name) }
        }

    override fun delete(relativePath: String): Boolean {
        check(!readOnly) { "Legacy source is read-only" }
        return resolve(relativePath)?.delete() ?: false
    }
}
