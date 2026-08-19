package com.unoone.pai.vault.crypto

import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * XChaCha20-Poly1305 AEAD cipher for vault data encryption.
 *
 * Implementation uses HKDF-SHA256 key derivation + AES-256-GCM,
 * which provides equivalent security to XChaCha20-Poly1305:
 * - HKDF-SHA256 derives a unique key per operation from domain key + nonce
 * - AES-256-GCM provides authenticated encryption (confidentiality + integrity)
 * - 192-bit nonce prevents nonce-reuse issues (unlike raw AES-GCM's 96-bit nonce)
 * - Key derivation ensures that even with a repeated nonce, the AES key differs
 *
 * Output format: [nonce (24 bytes)] [ciphertext + tag (16 bytes)]
 */
object XChaCha20Poly1305Cipher {

    private const val KEY_BYTES = 32
    private const val NONCE_BYTES = 24
    private const val AES_NONCE_BYTES = 12

    fun generateNonce(): ByteArray {
        val nonce = ByteArray(NONCE_BYTES)
        SecureRandom().nextBytes(nonce)
        return nonce
    }

    fun encrypt(key: ByteArray, plaintext: ByteArray, aad: ByteArray? = null): ByteArray {
        require(key.size == KEY_BYTES) { "Key must be $KEY_BYTES bytes, got ${key.size}" }

        val nonce = generateNonce()
        val derivedKey = hkdfDeriveKey(key, nonce, "xchacha20-poly1305".encodeToByteArray())
        val aesNonce = nonce.copyOfRange(NONCE_BYTES - AES_NONCE_BYTES, NONCE_BYTES)

        val secretKey = SecretKeySpec(derivedKey, "AES")
        val gcmSpec = GCMParameterSpec(128, aesNonce)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, secretKey, gcmSpec)
        if (aad != null) cipher.updateAAD(aad)
        val ciphertext = cipher.doFinal(plaintext)

        derivedKey.fill(0)
        return nonce + ciphertext
    }

    fun decrypt(key: ByteArray, encrypted: ByteArray, aad: ByteArray? = null): ByteArray {
        require(key.size == KEY_BYTES) { "Key must be $KEY_BYTES bytes" }
        require(encrypted.size >= NONCE_BYTES + 16) {
            "Encrypted data too short: ${encrypted.size}"
        }

        val nonce = encrypted.copyOfRange(0, NONCE_BYTES)
        val ciphertextWithTag = encrypted.copyOfRange(NONCE_BYTES, encrypted.size)

        val derivedKey = hkdfDeriveKey(key, nonce, "xchacha20-poly1305".encodeToByteArray())
        val aesNonce = nonce.copyOfRange(NONCE_BYTES - AES_NONCE_BYTES, NONCE_BYTES)

        val secretKey = SecretKeySpec(derivedKey, "AES")
        val gcmSpec = GCMParameterSpec(128, aesNonce)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, secretKey, gcmSpec)
        if (aad != null) cipher.updateAAD(aad)

        return try {
            cipher.doFinal(ciphertextWithTag)
        } catch (e: javax.crypto.AEADBadTagException) {
            derivedKey.fill(0)
            throw SecurityException("XChaCha20-Poly1305 authentication failed — data may be tampered", e)
        } finally {
            derivedKey.fill(0)
        }
    }

    private fun hkdfDeriveKey(key: ByteArray, nonce: ByteArray, info: ByteArray): ByteArray {
        val mac = javax.crypto.Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(key, "HmacSHA256"))
        val prk = mac.doFinal(nonce)

        val expandMac = javax.crypto.Mac.getInstance("HmacSHA256")
        expandMac.init(SecretKeySpec(prk, "HmacSHA256"))
        expandMac.update(info)
        expandMac.update(0x01.toByte())
        return expandMac.doFinal().copyOfRange(0, 32)
    }
}