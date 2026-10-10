package com.unoone.agent.storage.cache

import java.io.File
import java.security.SecureRandom

/**
 * Wraps/unwraps the cache-database passphrase. The production implementation
 * is backed by the Android Keystore ([KeystorePassphraseCipher]); JVM tests
 * use a fake. Implementations MUST authenticate the blob (AEAD): decrypting a
 * tampered, truncated, or foreign blob must throw, never return garbage.
 */
interface PassphraseCipher {
    fun encrypt(plaintext: ByteArray): ByteArray
    fun decrypt(blob: ByteArray): ByteArray
}

/** RESET is retained for historical API compatibility, but is never produced automatically. */
enum class KeyOutcome { CREATED, UNWRAPPED, RESET }

class PassphraseResult(val passphrase: ByteArray, val outcome: KeyOutcome)

/** Creates a key only for a new store. Unwrap failures never overwrite recovery material. */
class CacheKeyManager(
    private val cipher: PassphraseCipher,
    private val wrappedKeyFile: File,
    private val random: SecureRandom = SecureRandom(),
    private val syncDirectory: (File) -> Unit = { directory ->
        java.nio.channels.FileChannel.open(directory.toPath(), java.nio.file.StandardOpenOption.READ).use { it.force(true) }
    },
) {

    private val temporary get() = File(wrappedKeyFile.parentFile, wrappedKeyFile.name + ".tmp")
    fun hasPersistedKeyMaterial(): Boolean = hasPersistedKeyMaterial(wrappedKeyFile)

    fun getOrCreate(): PassphraseResult = synchronized(keyLock) {
        var plaintext: ByteArray? = null
        try {
            val finalExists = java.nio.file.Files.exists(wrappedKeyFile.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS)
            val tempExists = java.nio.file.Files.exists(temporary.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS)
            require(!(finalExists && tempExists)) { "Ambiguous wrapped keys; retain both" }
            if (!finalExists && !tempExists) {
                return@synchronized PassphraseResult(generateAndPersist(), KeyOutcome.CREATED)
            }
            val source = if (finalExists) wrappedKeyFile else temporary
            require(java.nio.file.Files.isRegularFile(source.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS) && source.length() in 1..4096)
            plaintext = cipher.decrypt(source.readBytes())
            require(plaintext.size == PASSPHRASE_LEN)
            if (tempExists) {
                // Authenticate BEFORE adopting the interrupted key. Never replace either blob.
                java.io.FileOutputStream(source, true).use { it.fd.sync() }
                require(!java.nio.file.Files.exists(wrappedKeyFile.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS))
                require(source.renameTo(wrappedKeyFile)) { "Cannot adopt interrupted key" }
            }
            syncDirectory(wrappedKeyFile.parentFile!!)
            PassphraseResult(plaintext, KeyOutcome.UNWRAPPED)
        } catch (_: Exception) {
            plaintext?.fill(0)
            throw DatabaseRecoveryRequired("Database key unavailable or ambiguous. Keep all wrapped key files and this installation; do not reset or uninstall. Assisted recovery may be required.")
        }
    }

    private fun generateAndPersist(): ByteArray {
        val passphrase = ByteArray(PASSPHRASE_LEN).also { random.nextBytes(it) }
        try {
            val blob = cipher.encrypt(passphrase)
            // Crash mid-write leaves recovery material, never permission to RESET the store.
            val tmp = temporary
            if (java.nio.file.Files.exists(tmp.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS))
                throw DatabaseRecoveryRequired("Interrupted key write exists; preserve it for recovery")
            require(blob.size in 1..4096)
            check(tmp.createNewFile()) { "Concurrent key persistence" }
            java.io.FileOutputStream(tmp).use { it.write(blob); it.fd.sync() }
            check(!java.nio.file.Files.exists(wrappedKeyFile.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS)) { "Refusing to replace an existing wrapped key" }
            check(tmp.renameTo(wrappedKeyFile)) { "cannot persist wrapped cache key; original data retained" }
            syncDirectory(wrappedKeyFile.parentFile!!)
            return passphrase
        } catch (error: Exception) {
            passphrase.fill(0)
            throw error
        }
    }

    companion object {
        private val keyLock = Any()
        const val PASSPHRASE_LEN: Int = 32
        /** Presence is not authentication; ambiguous/invalid material is rejected by getOrCreate. */
        fun hasPersistedKeyMaterial(file: File): Boolean =
            java.nio.file.Files.exists(file.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS) ||
                java.nio.file.Files.exists(File(file.parentFile, file.name + ".tmp").toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS)
    }
}
