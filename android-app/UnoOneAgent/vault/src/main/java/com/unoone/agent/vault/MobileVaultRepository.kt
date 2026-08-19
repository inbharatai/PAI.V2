package com.unoone.agent.vault

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.bouncycastle.crypto.generators.Argon2BytesGenerator
import org.bouncycastle.crypto.params.Argon2Parameters
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * Filesystem abstraction over a SAF tree. The Android DocumentFile adapter
 * implements one method each; this interface is deliberately JVM-testable.
 */
interface VaultIO {
    fun read(relativePath: String): ByteArray
    fun write(relativePath: String, bytes: ByteArray)
    fun exists(relativePath: String): Boolean
    fun list(relativePath: String): List<String>
    fun delete(relativePath: String): Boolean
}

/** Parsed vault master key — exists only in memory while the vault is open. */
class VaultSession internal constructor(
    val vaultId: String,
    val masterKey: ByteArray,
) {
    override fun toString(): String = "VaultSession(vaultId=$vaultId)" // never the key
}

/**
 * Android-facing encrypted shared vault: unlock → read → write → tombstone,
 * cryptographically identical to vault-core (pinned by the cross-platform
 * vectors: KDF params, HKDF, AAD construction, nonce/tag layout, wrap AAD).
 */
class MobileVaultRepository(private val io: VaultIO) {

    companion object {
        private const val HEADER_REL = "VAULT/header/header_a.json"
        private const val HEADER_B_REL = "VAULT/header/header_b.json"
        private const val RECORDS_DIR = "VAULT/records"
        private const val WRAP_AAD = "unoone-vault-master-key-wrap"

        /** Header JSON field order — mirrors the Rust VaultHeader struct
         * declaration order; HMAC serialisation is over this exact layout
         * with header_hmac set to an empty string. */
        private val HEADER_FIELD_ORDER = listOf(
            "version", "vault_id", "kdf_params", "salt", "wrapped_master_key",
            "wrap_nonce", "header_hmac", "recovery_enabled",
            "wrapped_master_key_recovery", "recovery_wrap_nonce",
            "recovery_salt", "created_at", "updated_at",
            // v2 slot-selection fields — included ONLY when present on disk,
            // mirroring serde's skip_serializing_if="Option::is_none".
            "generation", "committed",
        )
        private val KDF_FIELD_ORDER = listOf("memory_kib", "iterations", "parallelism", "output_len")
    }

    // ------------------------------------------------------------------
    // Unlock
    // ------------------------------------------------------------------

    /**
     * Unlock with a password: parse the newest-committed header, verify its
     * HMAC against the password-derived KEK, and unwrap the master key.
     * Returns a [VaultSession] on success — the master key lives only inside
     * the returned object.
     * @throws VaultAccessException on wrong password, tampered header, or
     *         corrupt envelope — never silently succeeds.
     */
    fun unlock(password: ByteArray): VaultSession {
        val (path, header) = chooseHeader()
        val obj = header.jsonObject

        val kdf = obj.getValue("kdf_params").jsonObject
        val memoryKb = kdf.getValue("memory_kib").jsonPrimitive.intOrNull
            ?: throw VaultAccessException("header kdf_params.memory_kib missing")
        val iterations = kdf.getValue("iterations").jsonPrimitive.intOrNull
            ?: throw VaultAccessException("header kdf_params.iterations missing")
        val parallelism = kdf.getValue("parallelism").jsonPrimitive.intOrNull
            ?: throw VaultAccessException("header kdf_params.parallelism missing")

        // The vault header stores salt/keys/nonces as HEX (see vault-core
        // header.rs save format), not base64 as the code comment once said.
        val salt = hex(obj.getValue("salt").jsonPrimitive.content)
        val kek = deriveKek(
            password,
            salt = salt,
            memoryKb = memoryKb,
            iterations = iterations,
            parallelism = parallelism,
        )

        // HMAC verification BEFORE the master key is ever unwrapped: a
        // tampered header must fail here, not after keys exist in memory.
        val storedHmac = obj.getValue("header_hmac").jsonPrimitive.content
        val computed = hmacSha256Hex(kek, canonicalHeaderJson(obj, forHmac = true))
        if (!constantTimeEquals(storedHmac, computed)) {
            kek.fill(0)
            throw VaultAccessException("header HMAC verification failed ($path) — tampered or wrong password")
        }

        val wrapped = hex(obj.getValue("wrapped_master_key").jsonPrimitive.content)
        val wrapNonce = hex(obj.getValue("wrap_nonce").jsonPrimitive.content)
        val master = try {
            VaultCrypto.unwrapMasterKeyWithAad(kek, wrapped, wrapNonce)
        } catch (e: Exception) {
            kek.fill(0)
            throw VaultAccessException("master key unwrap failed — wrong password or corrupt header", e)
        }
        kek.fill(0)
        return VaultSession(obj.getValue("vault_id").jsonPrimitive.content, master)
    }

    private fun chooseHeader(): Pair<String, kotlinx.serialization.json.JsonElement> {
        val candidates = mutableListOf<String>()
        if (io.exists(HEADER_REL)) candidates += HEADER_REL
        if (io.exists(HEADER_B_REL)) candidates += HEADER_B_REL
        if (candidates.isEmpty()) throw VaultAccessException("no vault header found")

        fun parse(path: String): kotlinx.serialization.json.JsonElement? =
            try {
                Json.parseToJsonElement(String(io.read(path), Charsets.UTF_8))
            } catch (_: Exception) {
                null
            }

        // Newest-committed-generation selection, matching vault.rs semantics:
        // a v2 slot with committed=false never wins against a committed one.
        var best: Pair<String, kotlinx.serialization.json.JsonElement>? = null
        var bestGen = -1L
        for (path in candidates) {
            val parsed = parse(path) ?: continue
            val committed = parsed.jsonObject["committed"]?.jsonPrimitive?.booleanOrNull ?: true
            val gen = parsed.jsonObject["generation"]?.jsonPrimitive?.contentOrNull?.toLongOrNull() ?: 0L
            if (!committed) continue
            if (gen >= bestGen) {
                bestGen = gen
                best = path to parsed
            }
        }
        return best ?: throw VaultAccessException("no committed vault header found")
    }

    private fun deriveKek(
        password: ByteArray,
        salt: ByteArray,
        memoryKb: Int,
        iterations: Int,
        parallelism: Int,
    ): ByteArray {
        val params = Argon2Parameters.Builder(Argon2Parameters.ARGON2_id)
            .withVersion(Argon2Parameters.ARGON2_VERSION_13)
            .withIterations(iterations)
            .withMemoryAsKB(memoryKb)
            .withParallelism(parallelism)
            .withSalt(salt)
            .build()
        val out = ByteArray(VaultCrypto.KEY_LEN)
        Argon2BytesGenerator().apply { init(params) }.generateBytes(password, out)
        return out
    }

    // ------------------------------------------------------------------
    // Records
    // ------------------------------------------------------------------

    /** Read + decrypt a record. Verifies canonical AAD before decrypting. */
    fun readRecord(session: VaultSession, recordId: String): Pair<Map<String, Any?>, ByteArray> {
        val path = "$RECORDS_DIR/$recordId.enc.json"
        val envelope = try {
            Json.parseToJsonElement(String(io.read(path), Charsets.UTF_8)).jsonObject
        } catch (e: Exception) {
            throw VaultAccessException("record $recordId unreadable", e)
        }
        val metadata = envelope.getValue("metadata").jsonObject
        val aadVersion = envelope["aad_version"]?.jsonPrimitive?.intOrNull ?: 0
        val fields = metadata.entries.associate { (k, v) -> k to jsonValueToKotlin(v) }
        val aad = VaultCrypto.canonicalAad(fields)

        val domainKey = VaultCrypto.deriveRecordDomainKey(session.masterKey)
        // Record envelopes use HEX for nonce/ciphertext (vault-core write_record),
        // matching associated_data. Header fields are hex too (see unlock).
        val nonce = hex(envelope.getValue("nonce").jsonPrimitive.content)
        val ciphertext = hex(envelope.getValue("encrypted_content").jsonPrimitive.content)
        val pts = try {
            when (nonce.size) {
                // AES-256-GCM (new records) and legacy XChaCha20 remain readable.
                12 -> VaultCrypto.decryptRecords(domainKey, nonce, ciphertext, aad)
                else -> throw VaultAccessException("record uses legacy XChaCha nonce (${nonce.size}) — supported only via vault-core today")
            }
        } catch (e: Exception) {
            when (e) {
                is VaultAccessException -> throw e
                else -> throw VaultAccessException("record decrypt failed (aad_version=$aadVersion)", e)
            }
        }
        return fields to pts
    }

    /**
     * Write a new record (random UUID v4 id supplied by caller), canonical AAD v2.
     *
     * [nonce] exists ONLY for the deterministic cross-platform vectors (the
     * committed Kotlin-authored envelope in
     * packages/vault-core/test-vectors/synthetic-vault): tests inject a pinned
     * 12-byte nonce so the produced envelope is byte-reproducible. Production
     * callers must leave it null — a fresh random nonce per write is a hard
     * AES-GCM requirement (nonce reuse under one key is catastrophic).
     */
    fun writeRecord(
        session: VaultSession,
        fields: Map<String, Any?>,
        content: ByteArray,
        nonce: ByteArray? = null,
    ): String {
        val recordId = fields["record_id"] as? String
            ?: throw VaultAccessException("record_id required in fields")
        val aad = VaultCrypto.canonicalAad(fields)
        val domainKey = VaultCrypto.deriveRecordDomainKey(session.masterKey)
        val actualNonce = nonce?.also {
            require(it.size == 12) { "injected nonce must be 12 bytes" }
        } ?: java.security.SecureRandom().let { r ->
            ByteArray(12).also { r.nextBytes(it) }
        }
        val ciphertext = VaultCrypto.encryptRecords(domainKey, actualNonce, content, aad)
        val envelopeJson = buildString {
            append("{\"metadata\":").append(String(VaultCrypto.canonicalAad(fields), Charsets.UTF_8))
            append(",\"encrypted_content\":\"").append(VaultCrypto.run { ciphertext.toHex() }).append('"')
            append(",\"nonce\":\"").append(VaultCrypto.run { actualNonce.toHex() }).append('"')
            append(",\"associated_data\":\"").append(VaultCrypto.run { aad.toHex() }).append('"')
            append(",\"aad_version\":2}")
        }
        io.write("$RECORDS_DIR/$recordId.enc.json", envelopeJson.toByteArray(Charsets.UTF_8))
        return recordId
    }

    /** Tombstone: rewrite metadata with tombstone=true + deleted_at, revision+1. */
    fun tombstoneRecord(session: VaultSession, recordId: String, deletedAtIso: String) {
        val (fields, _) = readRecord(session, recordId)
        val updated = fields.toMutableMap()
        updated["tombstone"] = true
        updated["deleted_at"] = deletedAtIso
        val revision = (fields["revision"] as? Int ?: 1) + 1
        updated["revision"] = revision
        // The tombstone content replaces the content payload with empty bytes;
        // the metadata remains plaintext-indexable, the body is gone.
        writeExistingRecord(session, updated, ByteArray(0))
    }

    private fun writeExistingRecord(session: VaultSession, fields: Map<String, Any?>, content: ByteArray) {
        val recordId = fields["record_id"] as? String
            ?: throw VaultAccessException("record_id required")
        val aad = VaultCrypto.canonicalAad(fields)
        val domainKey = VaultCrypto.deriveRecordDomainKey(session.masterKey)
        val nonce = ByteArray(12).also { java.security.SecureRandom().nextBytes(it) }
        val ciphertext = VaultCrypto.encryptRecords(domainKey, nonce, content, aad)
        val envelopeJson = buildString {
            append("{\"metadata\":").append(String(VaultCrypto.canonicalAad(fields), Charsets.UTF_8))
            append(",\"encrypted_content\":\"").append(VaultCrypto.run { ciphertext.toHex() }).append('"')
            append(",\"nonce\":\"").append(VaultCrypto.run { nonce.toHex() }).append('"')
            append(",\"associated_data\":\"").append(VaultCrypto.run { aad.toHex() }).append('"')
            append(",\"aad_version\":2}")
        }
        io.write("$RECORDS_DIR/$recordId.enc.json", envelopeJson.toByteArray(Charsets.UTF_8))
    }

    // ------------------------------------------------------------------
    // Canonical header JSON for HMAC (field order + option omission rules)
    // ------------------------------------------------------------------

    private fun canonicalHeaderJson(obj: JsonObject, forHmac: Boolean): ByteArray {
        val out = StringBuilder("{")
        var first = true
        for (name in HEADER_FIELD_ORDER) {
            if (forHmac && name == "header_hmac") {
                // HMAC is computed with the field present but EMPTY.
                if (!first) out.append(',')
                first = false
                out.append("\"header_hmac\":\"\"")
                continue
            }
            val elem = obj[name] ?: continue // absent Option fields stay absent
            if (name == "header_hmac" && elem.jsonPrimitive.content.isEmpty()) continue
            if (!first) out.append(',')
            first = false
            out.append('"').append(name).append('"').append(':')
            if (name == "kdf_params") {
                val k = elem.jsonObject
                out.append('{')
                KDF_FIELD_ORDER.forEachIndexed { i, kn ->
                    if (i > 0) out.append(',')
                    out.append('"').append(kn).append('"').append(':')
                    out.append(k.getValue(kn).jsonPrimitive.content)
                }
                out.append('}')
            } else {
                val prim = elem.jsonPrimitive
                when {
                    elem is kotlinx.serialization.json.JsonNull -> out.append("null")
                    prim.isString -> out.append('"').append(escapeJson(prim.content)).append('"')
                    else -> out.append(prim.content)
                }
            }
        }
        out.append('}')
        return out.toString().toByteArray(Charsets.UTF_8)
    }

    private fun escapeJson(s: String): String = buildString {
        for (ch in s) {
            when (ch) {
                '"' -> append("\\\"")
                '\\' -> append("\\\\")
                '\n' -> append("\\n")
                '\r' -> append("\\r")
                '\t' -> append("\\t")
                else -> if (ch < ' ') append("\\u".plus("%04x".format(ch.code))) else append(ch)
            }
        }
    }

    // ------------------------------------------------------------------
    // Small utilities
    // ------------------------------------------------------------------

    private fun jsonValueToKotlin(elem: kotlinx.serialization.json.JsonElement): Any? = when {
        elem is kotlinx.serialization.json.JsonNull -> null
        elem is kotlinx.serialization.json.JsonArray -> elem.jsonArray.map {
            it.jsonPrimitive.content
        }
        else -> when {
            elem.jsonPrimitive.content == "true" || elem.jsonPrimitive.content == "false" ->
                elem.jsonPrimitive.content.toBoolean()
            else -> elem.jsonPrimitive.content.toLongOrNull() as? Int
                ?: elem.jsonPrimitive.intOrNull
                ?: elem.jsonPrimitive.content
        }
    }

    private fun hmacSha256Hex(key: ByteArray, data: ByteArray): String {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(key, "HmacSHA256"))
        return mac.doFinal(data).joinToString("") { "%02x".format(it) }
    }

    private fun constantTimeEquals(a: String, b: String): Boolean {
        val ba = a.toByteArray(Charsets.UTF_8)
        val bb = b.toByteArray(Charsets.UTF_8)
        var diff = ba.size.xor(bb.size)
        val n = minOf(ba.size, bb.size)
        for (i in 0 until n) diff = diff or (ba[i].toInt() xor bb[i].toInt())
        return diff == 0
    }

    // (Dead base64 helpers removed — every vault field is HEX on disk; see the
    // header/record format notes above. hex() below is the only codec needed.)
    private fun hex(s: String): ByteArray = s.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
}

/** Every vault failure mode a caller can act on — never swallowed. */
class VaultAccessException(message: String, cause: Throwable? = null) : Exception(message, cause)
