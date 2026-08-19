package com.unoone.pai.vault.crypto

import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec
import java.security.SecureRandom

/**
 * AES-256-GCM cipher — fallback for platforms where XChaCha20-Poly1305
 * is not available (e.g., Android has hardware-accelerated AES-GCM).
 *
 * On Android, this uses the hardware-backed AES-GCM implementation
 * which is significantly faster than BouncyCastle's software XChaCha20.
 *
 * Output format: [nonce (12 bytes)] [ciphertext] [tag (16 bytes)]
 */
object Aes256GcmCipher {

    private const val KEY_BYTES = 32
    private const val NONCE_BYTES = 12  // 96 bits (GCM standard)
    private const val TAG_BITS = 128    // 16-byte authentication tag

    fun generateNonce(): ByteArray {
        val nonce = ByteArray(NONCE_BYTES)
        SecureRandom().nextBytes(nonce)
        return nonce
    }

    /**
     * Encrypt plaintext with AES-256-GCM.
     *
     * @param key 32-byte encryption key
     * @param plaintext Data to encrypt
     * @param aad Additional authenticated data
     * @return nonce (12) + ciphertext + tag (16)
     */
    fun encrypt(key: ByteArray, plaintext: ByteArray, aad: ByteArray? = null): ByteArray {
        require(key.size == KEY_BYTES) { "Key must be $KEY_BYTES bytes" }

        val nonce = generateNonce()
        val secretKey = SecretKeySpec(key, "AES")
        val gcmSpec = GCMParameterSpec(TAG_BITS, nonce)

        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, secretKey, gcmSpec)

        if (aad != null) {
            cipher.updateAAD(aad)
        }

        val ciphertext = cipher.doFinal(plaintext)

        return nonce + ciphertext // ciphertext already includes the GCM tag
    }

    /**
     * Decrypt AES-256-GCM encrypted data.
     *
     * @param key 32-byte encryption key
     * @param encrypted nonce (12) + ciphertext + tag (16)
     * @param aad Additional authenticated data (must match encryption)
     * @return Decrypted plaintext
     * @throws SecurityException if authentication fails
     */
    fun decrypt(key: ByteArray, encrypted: ByteArray, aad: ByteArray? = null): ByteArray {
        require(key.size == KEY_BYTES) { "Key must be $KEY_BYTES bytes" }
        require(encrypted.size >= NONCE_BYTES + 16) {  // 16 bytes = GCM tag
            "Encrypted data too short"
        }

        val nonce = encrypted.copyOfRange(0, NONCE_BYTES)
        val ciphertextWithTag = encrypted.copyOfRange(NONCE_BYTES, encrypted.size)

        val secretKey = SecretKeySpec(key, "AES")
        val gcmSpec = GCMParameterSpec(TAG_BITS, nonce)

        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, secretKey, gcmSpec)

        if (aad != null) {
            cipher.updateAAD(aad)
        }

        return try {
            cipher.doFinal(ciphertextWithTag)
        } catch (e: javax.crypto.AEADBadTagException) {
            throw SecurityException("AES-256-GCM authentication failed — data may be tampered", e)
        }
    }
}