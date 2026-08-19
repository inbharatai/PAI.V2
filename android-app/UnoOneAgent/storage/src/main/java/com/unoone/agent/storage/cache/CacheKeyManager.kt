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

/** How the passphrase was obtained. Callers act on [RESET] by deleting the old DB. */
enum class KeyOutcome {
    /** No wrapped key existed; a fresh passphrase was generated and persisted. */
    CREATED,

    /** The persisted wrapped key unwrapped cleanly — the normal steady state. */
    UNWRAPPED,

    /**
     * A wrapped key existed but could not be unwrapped (Keystore key lost or
     * blob corrupted). A fresh passphrase replaced it; any database encrypted
     * under the old key is unreadable by construction and must be reset.
     */
    RESET,
}

class PassphraseResult(val passphrase: ByteArray, val outcome: KeyOutcome)

/**
 * Owns the lifecycle of the SQLCipher passphrase for the Room cache:
 * generate once, persist Keystore-wrapped, unwrap on every start.
 *
 * Decision logic only — no Android APIs — so the entire lifecycle is
 * JVM-unit-tested (the portable-logic rule). The vault on the drive is the
 * authoritative store; the cache is rebuildable, so key loss degrades to a
 * cache reset, never to data-recovery heroics.
 */
class CacheKeyManager(
    private val cipher: PassphraseCipher,
    private val wrappedKeyFile: File,
    private val random: SecureRandom = SecureRandom(),
) {

    fun getOrCreate(): PassphraseResult {
        if (!wrappedKeyFile.exists()) {
            return PassphraseResult(generateAndPersist(), KeyOutcome.CREATED)
        }
        return try {
            PassphraseResult(cipher.decrypt(wrappedKeyFile.readBytes()), KeyOutcome.UNWRAPPED)
        } catch (_: Exception) {
            // The blob is unreadable, so the DB key is gone with it. Replace
            // the key and signal RESET so the caller deletes the old database.
            wrappedKeyFile.delete()
            PassphraseResult(generateAndPersist(), KeyOutcome.RESET)
        }
    }

    private fun generateAndPersist(): ByteArray {
        val passphrase = ByteArray(PASSPHRASE_LEN).also { random.nextBytes(it) }
        val blob = cipher.encrypt(passphrase)
        // Atomic write (tmp + rename): a crash mid-write must never leave a
        // truncated blob that silently RESETs the cache on the next start.
        val tmp = File(wrappedKeyFile.parentFile, wrappedKeyFile.name + ".tmp")
        tmp.writeBytes(blob)
        if (!tmp.renameTo(wrappedKeyFile)) {
            wrappedKeyFile.delete()
            check(tmp.renameTo(wrappedKeyFile)) { "cannot persist wrapped cache key" }
        }
        return passphrase
    }

    companion object {
        const val PASSPHRASE_LEN: Int = 32
    }
}
