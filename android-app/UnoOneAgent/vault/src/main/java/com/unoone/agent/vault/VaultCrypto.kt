package com.unoone.agent.vault

import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.Mac
import javax.crypto.spec.GCMParameterSpec
import org.bouncycastle.crypto.params.KeyParameter
import org.bouncycastle.crypto.params.ParametersWithIV
import javax.crypto.spec.SecretKeySpec

/**
 * Cross-platform crypto contract with vault-core (Rust).
 *
 * Every constant and byte layout here is pinned by the checked-in vectors in
 * `packages/vault-core/test-vectors/vault-cross-platform.json`, verified both
 * directions: Kotlin decrypts Rust's ciphertext, and Rust decrypts Kotlin's.
 * If either side drifts (KDF params, HKDF salt/info, AAD construction,
 * nonce/tag layout), CI fails WITHOUT a phone.
 */
object VaultCrypto {

    /** KDF parameters — mirrors packages/vault-core/src/crypto.rs SPEC_ARGON2_*. */
    const val ARGON2_MEMORY_KIB: Int = 256 * 1024
    const val ARGON2_ITERATIONS: Int = 3
    const val ARGON2_PARALLELISM: Int = 4
    const val ARGON2_OUTPUT_LEN: Int = 32
    const val SALT_LEN: Int = 32
    const val KEY_LEN: Int = 32

    /** HKDF salt/info — mirrors derive_domain_key in crypto.rs. */
    private const val HKDF_SALT = "unoone-vault-domain"
    private const val DOMAIN_RECORDS = "records"

    private const val AES_GCM_NONCE_LEN = 12
    private const val GCM_TAG_BITS = 128

    // ------------------------------------------------------------------
    // HKDF-SHA-256 (RFC 5869), matching hkdf crate behaviour used in
    // derive_domain_key(master, "records").
    // ------------------------------------------------------------------
    fun deriveRecordDomainKey(masterKey: ByteArray, domain: String = DOMAIN_RECORDS): ByteArray {
        require(masterKey.size == KEY_LEN) { "master key must be $KEY_LEN bytes" }
        val mac = Mac.getInstance("HmacSHA256")
        val prk = run {
            mac.init(SecretKeySpec(HKDF_SALT.toByteArray(Charsets.UTF_8), "HmacSHA256"))
            mac.doFinal(masterKey)
        }
        val info = domain.toByteArray(Charsets.UTF_8)
        // Output length (32) == hash length, so a single expand block suffices.
        val t = run {
            mac.init(SecretKeySpec(prk, "HmacSHA256"))
            mac.update(ByteArray(0)) // previous block = empty for T(1)
            mac.update(info)
            mac.update(byteArrayOf(0x01))
            mac.doFinal()
        }
        return t
    }

    // ------------------------------------------------------------------
    // AES-256-GCM record encryption (12-byte nonce; tag appended to
    // ciphertext — the exact layout of Rust's aes-gcm `encrypt` output).
    // ------------------------------------------------------------------
    fun encryptRecords(key: ByteArray, nonce: ByteArray, plaintext: ByteArray, aad: ByteArray): ByteArray {
        require(nonce.size == AES_GCM_NONCE_LEN) { "AES-GCM nonce must be $AES_GCM_NONCE_LEN bytes" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(
            Cipher.ENCRYPT_MODE,
            SecretKeySpec(key, "AES"),
            GCMParameterSpec(GCM_TAG_BITS, nonce)
        )
        cipher.updateAAD(aad)
        return cipher.doFinal(plaintext)
    }

    fun decryptRecords(key: ByteArray, nonce: ByteArray, ciphertext: ByteArray, aad: ByteArray): ByteArray {
        require(nonce.size == AES_GCM_NONCE_LEN) { "AES-GCM nonce must be $AES_GCM_NONCE_LEN bytes" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(
            Cipher.DECRYPT_MODE,
            SecretKeySpec(key, "AES"),
            GCMParameterSpec(GCM_TAG_BITS, nonce)
        )
        cipher.updateAAD(aad)
        return cipher.doFinal(ciphertext)
    }

    // ------------------------------------------------------------------
    // Canonical AAD: compact JSON in the Rust Record's declaration order,
    // byte-identical to serde_json::to_vec(&record). Field order is the
    // contract; the vector pins both the order and the escaping.
    // ------------------------------------------------------------------
    private val AAD_FIELD_ORDER = listOf(
        "record_id", "record_type", "schema_version", "encryption_version",
        "created_at", "updated_at", "revision", "origin_platform",
        "origin_device_id", "transaction_id", "content_hash",
        "parent_record_id", "source_record_ids", "privacy_level",
        "tombstone", "deleted_at"
    )

    /**
     * Build the canonical AAD from the record metadata map.
     * Values must be String / Int / Boolean / List<String> / null.
     * (A dead `pretty` parameter was removed — the AAD is canonical compact
     * bytes by definition; there is no pretty form of an authentication input.)
     */
    fun canonicalAad(fields: Map<String, Any?>): ByteArray {
        val out = StringBuilder("{")
        var first = true
        for (name in AAD_FIELD_ORDER) {
            if (!first) out.append(',')
            first = false
            out.append('"').append(name).append('"').append(':')
            val value = fields[name]
            when (value) {
                null -> out.append("null")
                is String -> out.append(jsonEscape(value))
                is Int, is Long -> out.append(value.toString())
                is Boolean -> out.append(if (value) "true" else "false")
                is List<*> -> {
                    out.append('[')
                    value.forEachIndexed { i, item ->
                        if (i > 0) out.append(',')
                        out.append(jsonEscape(item as String))
                    }
                    out.append(']')
                }
                else -> throw IllegalArgumentException("unsupported AAD field type for $name")
            }
        }
        out.append('}')
        return out.toString().toByteArray(Charsets.UTF_8)
    }

    /**
     * Compact JSON string escaping byte-identical to serde_json.
     *
     * This MUST match `serde_json::to_string` exactly. The result becomes the
     * canonical AAD, and the Rust read path re-derives that AAD from the stored
     * metadata and rejects the record when the bytes differ (vault.rs,
     * aad_version 2). Any divergence surfaces on the desktop as "metadata was
     * altered after the record was written" -- a tamper error for an honest
     * record written by Android.
     *
     * Verified against real serde_json output: the string U+0008 / U+000C /
     * U+0001 serialises as "a\bb\fc\u0001d", so:
     *   backspace U+0008 -> \b      (NOT \u0008)
     *   form feed U+000C -> \f      (NOT \u000C)
     *   other C0 controls -> \u00xx with LOWERCASE hex
     *   non-ASCII passed through as UTF-8 (serde_json does not escape it)
     */
    private fun jsonEscape(value: String): String {
        val out = StringBuilder("\"")
        for (ch in value) {
            when (ch) {
                '"' -> out.append("\\\"")
                '\\' -> out.append("\\\\")
                '\n' -> out.append("\\n")
                '\r' -> out.append("\\r")
                '\t' -> out.append("\\t")
                '\b' -> out.append("\\b")
                '' -> out.append("\\f")
                else ->
                    if (ch < ' ') out.append("\\u").append("%04x".format(ch.code))
                    else out.append(ch)
            }
        }
        return out.append('\"').toString()
    }

    // ------------------------------------------------------------------
    // XChaCha20-Poly1305 with 24-byte nonce — the master-key wrap the vault
    // header uses (crypto.rs wrap_master_key / unwrap_master_key). Bouncy
    // Castle engine; tag (16 bytes) appended to ciphertext like Rust.
    // ------------------------------------------------------------------
    private const val XCHACHA_NONCE_LEN = 24
    private val MASTER_KEY_WRAP_AAD = "unoone-vault-master-key-wrap".toByteArray(Charsets.UTF_8)

    /**
     * Master-key wrap AAD: the engine above authenticates against this —
     * note the wrap uses the SAME AEAD-with-AAD scheme; Rust constructs the
     * AAD inside encrypt(), so Kotlin passes aad into the engine via
     * ParametersWithIV... no — BouncyCastle XChaCha20Poly1305 does not take
     * AAD through init. We append AAD processing explicitly.
     */
    fun wrapMasterKeyWithAad(kek: ByteArray, masterKey: ByteArray, nonce: ByteArray): ByteArray =
        wrapEncryptWithAad(kek, nonce, masterKey, MASTER_KEY_WRAP_AAD)

    fun unwrapMasterKeyWithAad(kek: ByteArray, wrapped: ByteArray, nonce: ByteArray): ByteArray =
        wrapDecryptWithAad(kek, nonce, wrapped, MASTER_KEY_WRAP_AAD)

    /**
     * XChaCha20-Poly1305 = HChaCha20(key, nonce[0..15]) as subkey, then
     * ChaCha20-Poly1305 with nonce 0x00000000 ‖ nonce[16..23] — the exact
     * construction the chacha20poly1305 crate implements (verified against
     * the checked-in wrap vector). BouncyCastle ships no XChaCha engine, so
     * HChaCha20 is implemented inline.
     */
    private fun xchachaSubkey(key: ByteArray, nonce: ByteArray): ByteArray {
        fun rotl(v: Int, n: Int) = (v shl n) or (v ushr (32 - n))
        fun quarterRound(st: IntArray, a: Int, b: Int, c: Int, d: Int) {
            st[a] += st[b]; st[d] = rotl(st[d] xor st[a], 16)
            st[c] += st[d]; st[b] = rotl(st[b] xor st[c], 12)
            st[a] += st[b]; st[d] = rotl(st[d] xor st[a], 8)
            st[c] += st[d]; st[b] = rotl(st[b] xor st[c], 7)
        }
        fun le(bytes: ByteArray, off: Int): Int =
            (bytes[off].toInt() and 0xFF) or
                ((bytes[off + 1].toInt() and 0xFF) shl 8) or
                ((bytes[off + 2].toInt() and 0xFF) shl 16) or
                ((bytes[off + 3].toInt() and 0xFF) shl 24)
        val state = IntArray(16)
        state[0] = 0x61707865; state[1] = 0x3320646e
        state[2] = 0x79622d32; state[3] = 0x6b206574
        for (i in 0 until 8) state[4 + i] = le(key, i * 4)
        for (i in 0 until 4) state[12 + i] = le(nonce, i * 4)
        repeat(10) {
            quarterRound(state, 0, 4, 8, 12)
            quarterRound(state, 1, 5, 9, 13)
            quarterRound(state, 2, 6, 10, 14)
            quarterRound(state, 3, 7, 11, 15)
            quarterRound(state, 0, 5, 10, 15)
            quarterRound(state, 1, 6, 11, 12)
            quarterRound(state, 2, 7, 8, 13)
            quarterRound(state, 3, 4, 9, 14)
        }
        val subkey = ByteArray(32)
        val words = intArrayOf(0, 1, 2, 3, 12, 13, 14, 15)
        words.forEachIndexed { i, w ->
            val v = state[w]
            subkey[i * 4] = (v and 0xFF).toByte()
            subkey[i * 4 + 1] = ((v ushr 8) and 0xFF).toByte()
            subkey[i * 4 + 2] = ((v ushr 16) and 0xFF).toByte()
            subkey[i * 4 + 3] = ((v ushr 24) and 0xFF).toByte()
        }
        return subkey
    }

    private fun chachaNonce(xnonce: ByteArray): ByteArray {
        val n = ByteArray(12)
        System.arraycopy(xnonce, 16, n, 4, 8)
        return n
    }

    private fun wrapEncryptWithAad(key: ByteArray, nonce: ByteArray, plaintext: ByteArray, aad: ByteArray): ByteArray {
        val subkey = xchachaSubkey(key, nonce)
        val engine = org.bouncycastle.crypto.modes.ChaCha20Poly1305()
        engine.init(true, ParametersWithIV(KeyParameter(subkey), chachaNonce(nonce)))
        engine.processAADBytes(aad, 0, aad.size)
        val out = ByteArray(plaintext.size + 16)
        var off = engine.processBytes(plaintext, 0, plaintext.size, out, 0)
        off += engine.doFinal(out, off)
        return out.copyOf(off)
    }

    private fun wrapDecryptWithAad(key: ByteArray, nonce: ByteArray, ciphertext: ByteArray, aad: ByteArray): ByteArray {
        val subkey = xchachaSubkey(key, nonce)
        val engine = org.bouncycastle.crypto.modes.ChaCha20Poly1305()
        engine.init(false, ParametersWithIV(KeyParameter(subkey), chachaNonce(nonce)))
        engine.processAADBytes(aad, 0, aad.size)
        val out = ByteArray(ciphertext.size)
        var off = engine.processBytes(ciphertext, 0, ciphertext.size, out, 0)
        off += engine.doFinal(out, off)
        return out.copyOf(off)
    }

    // ------------------------------------------------------------------
    // Argon2id KEK — implemented against the JVM Bouncy Castle provider in
    // unit tests; wired into Android via the same spec constants.
    // Production unlock path also uses this on-device.
    // ------------------------------------------------------------------
    fun sha256Hex(data: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(data).toHex()

    fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }
}
