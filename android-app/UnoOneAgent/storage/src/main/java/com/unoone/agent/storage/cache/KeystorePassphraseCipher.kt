package com.unoone.agent.storage.cache

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Android Keystore adapter for [PassphraseCipher]: AES-256-GCM under a
 * non-exportable, device-bound key (hardware-backed where the device
 * supports it). Deliberately thin — every decision lives in
 * [CacheKeyManager] where it is JVM-testable; this class only performs the
 * two primitives.
 *
 * Blob layout: 12-byte GCM nonce || ciphertext+tag. GCM authenticates, so a
 * tampered blob fails to decrypt (throws), which CacheKeyManager converts
 * into a cache RESET.
 */
class KeystorePassphraseCipher(
    private val alias: String = DEFAULT_ALIAS,
) : PassphraseCipher {

    override fun encrypt(plaintext: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.ENCRYPT_MODE, obtainKey())
        return cipher.iv + cipher.doFinal(plaintext)
    }

    override fun decrypt(blob: ByteArray): ByteArray {
        require(blob.size > IV_LEN) { "wrapped blob too short" }
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(
            Cipher.DECRYPT_MODE,
            obtainKey(),
            GCMParameterSpec(TAG_BITS, blob, 0, IV_LEN),
        )
        return cipher.doFinal(blob, IV_LEN, blob.size - IV_LEN)
    }

    private fun obtainKey(): SecretKey {
        val keyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (keyStore.getKey(alias, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(
                alias,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }

    companion object {
        const val DEFAULT_ALIAS = "unoone_cache_db_key"
        private const val KEYSTORE = "AndroidKeyStore"
        private const val TRANSFORM = "AES/GCM/NoPadding"
        private const val IV_LEN = 12
        private const val TAG_BITS = 128
    }
}
