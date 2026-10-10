package com.unoone.agent.storage.cache

import java.io.File

class DatabaseRecoveryRequired(message: String, cause: Throwable? = null) : IllegalStateException(message, cause)

/** Read-only admission, run BEFORE key creation or Room open. No conversion is claimed here. */
object DatabaseOpenPolicy {
    private val sqliteHeader = "SQLite format 3\u0000".toByteArray(Charsets.US_ASCII)

    fun requireSafeOpen(database: File, wrappedKey: File) {
        val sidecars = listOf("-wal", "-shm", "-journal").map { File(database.path + it) }
        // Reject unsafe entries (including dangling links) before key creation or any SQLite call.
        (listOf(database) + sidecars).forEach { file ->
            if (java.nio.file.Files.exists(file.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS) &&
                !java.nio.file.Files.isRegularFile(file.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS)) {
                throw DatabaseRecoveryRequired("Unsafe database entry. Preserve files for assisted recovery.")
            }
        }
        val hasSidecar = sidecars.any { java.nio.file.Files.exists(it.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS) }
        if (!database.exists() && hasSidecar) {
            throw DatabaseRecoveryRequired("Database sidecars exist without the main file. Recovery required; files and keys have been retained.")
        }
        if (!database.exists()) return
        val header = database.inputStream().use { input ->
            val bytes = ByteArray(sqliteHeader.size)
            val count = input.read(bytes)
            bytes.take(count.coerceAtLeast(0)).toByteArray()
        }
        if (header.contentEquals(sqliteHeader)) {
            throw DatabaseRecoveryRequired("Standalone plaintext database detected. Use the consent-gated backed-up encryption flow before ordinary open. Original database, sidecars and keys are unchanged.")
        }
        if (database.length() < sqliteHeader.size || !CacheKeyManager.hasPersistedKeyMaterial(wrappedKey)) {
            throw DatabaseRecoveryRequired("Existing database has missing key or incomplete header. Recovery required; refusing to generate a replacement key or reinitialize data.")
        }
    }
}
