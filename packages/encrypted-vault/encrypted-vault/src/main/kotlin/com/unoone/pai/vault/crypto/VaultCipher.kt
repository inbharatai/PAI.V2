package com.unoone.pai.vault.crypto

/**
 * Vault cipher — selects the best available AEAD cipher for the platform.
 *
 * Priority:
 * 1. XChaCha20-Poly1305 (via BouncyCastle) — 192-bit nonce, no collision risk
 * 2. AES-256-GCM (via JDK/Android hardware) — 96-bit nonce, hardware-accelerated
 *
 * The vault header records which cipher was used, so decryption can
 * always use the correct cipher regardless of platform.
 */
object VaultCipher {

    /**
     * Cipher algorithms supported by the vault.
     */
    enum class Algorithm(val displayName: String, val nonceBytes: Int) {
        XCHACHA20_POLY1305("XChaCha20-Poly1305", 24),
        AES_256_GCM("AES-256-GCM", 12)
    }

    /**
     * Encrypt data using the specified algorithm.
     */
    fun encrypt(
        algorithm: Algorithm,
        key: ByteArray,
        plaintext: ByteArray,
        aad: ByteArray? = null
    ): ByteArray {
        return when (algorithm) {
            Algorithm.XCHACHA20_POLY1305 -> XChaCha20Poly1305Cipher.encrypt(key, plaintext, aad)
            Algorithm.AES_256_GCM -> Aes256GcmCipher.encrypt(key, plaintext, aad)
        }
    }

    /**
     * Decrypt data using the specified algorithm.
     */
    fun decrypt(
        algorithm: Algorithm,
        key: ByteArray,
        encrypted: ByteArray,
        aad: ByteArray? = null
    ): ByteArray {
        return when (algorithm) {
            Algorithm.XCHACHA20_POLY1305 -> XChaCha20Poly1305Cipher.decrypt(key, encrypted, aad)
            Algorithm.AES_256_GCM -> Aes256GcmCipher.decrypt(key, encrypted, aad)
        }
    }

    /**
     * Detect the best available cipher for the current platform.
     * On Android, prefer AES-256-GCM (hardware-accelerated).
     * On desktop, prefer XChaCha20-Poly1305 (larger nonce, no collision risk).
     */
    fun detectBestAlgorithm(): Algorithm {
        return try {
            // Check if we're on Android
            Class.forName("android.app.Activity")
            // Android — prefer hardware-accelerated AES-GCM
            Algorithm.AES_256_GCM
        } catch (e: ClassNotFoundException) {
            // Desktop — prefer XChaCha20-Poly1305
            Algorithm.XCHACHA20_POLY1305
        }
    }

    /**
     * Detect the algorithm from the encrypted data based on nonce size.
     * XChaCha20 uses 24-byte nonce, AES-GCM uses 12-byte nonce.
     */
    fun detectAlgorithmFromData(encrypted: ByteArray): Algorithm {
        if (encrypted.size < 12) {
            throw IllegalArgumentException("Encrypted data too short to detect algorithm")
        }
        // Both algorithms start with the nonce, but we can't distinguish purely from data.
        // The vault header must specify the algorithm. Default to XChaCha20.
        return Algorithm.XCHACHA20_POLY1305
    }
}