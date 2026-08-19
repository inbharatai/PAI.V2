package com.unoone.pai.vault.crypto

import org.bouncycastle.crypto.generators.Argon2BytesGenerator
import org.bouncycastle.crypto.params.Argon2Parameters
import java.security.SecureRandom

/**
 * Argon2id key derivation for vault password.
 *
 * Parameters chosen for resistance against GPU/ASIC attacks:
 * - Memory: 256 MB (262144 KB)
 * - Iterations: 3
 * - Parallelism: 4
 * - Output: 32 bytes (256 bits)
 *
 * The salt is randomly generated and stored in the vault header.
 * The password is NEVER stored.
 */
object Argon2idKdf {

    private const val MEMORY_KB = 262144 // 256 MB
    private const val ITERATIONS = 3
    private const val PARALLELISM = 4
    private const val OUTPUT_BYTES = 32
    private const val SALT_BYTES = 32

    fun generateSalt(): ByteArray {
        val salt = ByteArray(SALT_BYTES)
        SecureRandom().nextBytes(salt)
        return salt
    }

    /**
     * Derive a 256-bit key from password and salt using Argon2id.
     *
     * @param password The vault password (never stored)
     * @param salt 32-byte random salt (stored in vault header)
     * @return 32-byte derived key
     */
    fun deriveKey(password: CharArray, salt: ByteArray): ByteArray {
        require(salt.size == SALT_BYTES) { "Salt must be $SALT_BYTES bytes, got ${salt.size}" }

        val params = Argon2Parameters.Builder(Argon2Parameters.ARGON2_id)
            .withMemoryAsKB(MEMORY_KB)
            .withIterations(ITERATIONS)
            .withParallelism(PARALLELISM)
            .withSalt(salt)
            .withVersion(Argon2Parameters.ARGON2_VERSION_13)
            .build()

        val generator = Argon2BytesGenerator()
        generator.init(params)

        val output = ByteArray(OUTPUT_BYTES)
        generator.generateBytes(password, output)

        return output
    }

    /**
     * Derive a domain-specific subkey from the master key.
     * Uses HKDF to derive separate keys for different vault domains
     * (memories, chats, recordings, etc.) from the same master key.
     *
     * @param masterKey 32-byte master key derived from Argon2id
     * @param domain Domain identifier (e.g., "memories", "chats", "recordings")
     * @param context Optional context bytes
     * @return 32-byte domain key
     */
    fun deriveDomainKey(masterKey: ByteArray, domain: String, context: ByteArray? = null): ByteArray {
        // HKDF-SHA256 domain key derivation — MUST match Rust vault-core exactly:
        //   salt = "unoone-vault-domain"
        //   info = domain (+ optional context bytes)
        // This guarantees that Android and Desktop derive the same per-domain key
        // from the same master key, which is required for cross-platform vault sync.
        val info = if (context != null && context.isNotEmpty()) {
            domain.encodeToByteArray() + context
        } else {
            domain.encodeToByteArray()
        }
        val hkdfParams = org.bouncycastle.crypto.params.HKDFParameters(
            masterKey,
            "unoone-vault-domain".encodeToByteArray(),
            info
        )
        val hkdf = org.bouncycastle.crypto.generators.HKDFBytesGenerator(
            org.bouncycastle.crypto.digests.SHA256Digest()
        )
        hkdf.init(hkdfParams)
        val output = ByteArray(OUTPUT_BYTES)
        hkdf.generateBytes(output, 0, OUTPUT_BYTES)
        return output
    }

    /**
     * Verify a password against a known key and salt.
     * Returns true if the password produces the same key.
     */
    fun verifyPassword(password: CharArray, salt: ByteArray, expectedKey: ByteArray): Boolean {
        val derivedKey = deriveKey(password, salt)
        return derivedKey.contentEquals(expectedKey)
    }

    /**
     * Generate a recovery key: 24 words from built-in word list.
     * The recovery key can unlock the vault if the password is forgotten.
     */
    fun generateRecoveryKey(): Pair<String, ByteArray> {
        return RecoveryKeyGenerator.generate()
    }
}