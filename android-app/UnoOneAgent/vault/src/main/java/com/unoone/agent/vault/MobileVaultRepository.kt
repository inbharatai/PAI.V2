package com.unoone.agent.vault

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
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
) : AutoCloseable {
    @Volatile private var closed = false
    internal fun requireOpen() { check(!closed) { "Vault session is locked" } }
    override fun close() { closed = true; masterKey.fill(0) }
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
        require(password.size in 1..4096) { "Password size is invalid" }
        val (path, header) = chooseHeader()
        val obj = header.jsonObject
        require(obj.getValue("version").jsonPrimitive.intOrNull == 1) { "Unsupported vault header version" }
        val kdf = obj.getValue("kdf_params").jsonObject
        // Untrusted headers must not control unbounded Argon2 allocation/work.
        require(kdf.getValue("memory_kib").jsonPrimitive.intOrNull == VaultCrypto.ARGON2_MEMORY_KIB &&
            kdf.getValue("iterations").jsonPrimitive.intOrNull == VaultCrypto.ARGON2_ITERATIONS &&
            kdf.getValue("parallelism").jsonPrimitive.intOrNull == VaultCrypto.ARGON2_PARALLELISM &&
            kdf.getValue("output_len").jsonPrimitive.intOrNull == VaultCrypto.KEY_LEN) {
            "Unsupported vault KDF; original files retained"
        }
        val id = obj.getValue("vault_id").jsonPrimitive.content
        require(java.util.UUID.fromString(id).toString() == id) { "Invalid vault UUID" }
        val salt = hex(obj.getValue("salt").jsonPrimitive.content)
        val wrapped = hex(obj.getValue("wrapped_master_key").jsonPrimitive.content)
        val wrapNonce = hex(obj.getValue("wrap_nonce").jsonPrimitive.content)
        require(salt.size == 32 && wrapped.size == 48 && wrapNonce.size == 24) { "Invalid header field length" }
        val kek = deriveKek(password, salt, VaultCrypto.ARGON2_MEMORY_KIB,
            VaultCrypto.ARGON2_ITERATIONS, VaultCrypto.ARGON2_PARALLELISM)
        try {
            val storedHmac = obj.getValue("header_hmac").jsonPrimitive.content
            val computed = hmacSha256Hex(kek, canonicalHeaderJson(obj, forHmac = true))
            if (!constantTimeEquals(storedHmac, computed))
                throw VaultAccessException("header authentication failed ($path)")
            val master = VaultCrypto.unwrapMasterKeyWithAad(kek, wrapped, wrapNonce)
            require(master.size == VaultCrypto.KEY_LEN)
            return VaultSession(id, master)
        } finally { kek.fill(0) }
    }

    /** Create only in an EMPTY private root. Never resets, adopts, or overwrites existing material.
     * Same Rust VaultHeader v1 format; existing spec Argon2id + wrap/HMAC, no new crypto.
     * A password may be a user-chosen long phrase. Recovery words are NOT implemented.
     */
    fun create(password: ByteArray): VaultSession {
        require(password.size in 12..4096) { "Use a password or phrase of at least 12 UTF-8 bytes" }
        require(io.list("").isEmpty() && !io.exists(HEADER_REL) && !io.exists(HEADER_B_REL)) {
            "Vault root is not empty; unlock or request recovery, never reset"
        }
        val random = java.security.SecureRandom()
        fun randomBytes(size: Int) = ByteArray(size).also(random::nextBytes)
        val id = java.util.UUID.randomUUID().toString()
        val salt = randomBytes(32)
        val nonce = randomBytes(24)
        val master = randomBytes(32)
        var transferred = false
        var kek: ByteArray? = null
        try {
            kek = deriveKek(password, salt, VaultCrypto.ARGON2_MEMORY_KIB,
                VaultCrypto.ARGON2_ITERATIONS, VaultCrypto.ARGON2_PARALLELISM)
            val wrapped = VaultCrypto.wrapMasterKeyWithAad(kek, master, nonce)
            val now = java.time.Instant.now().toString()
            val header = buildJsonObject {
                put("version", 1); put("vault_id", id)
                put("kdf_params", buildJsonObject {
                    put("memory_kib", VaultCrypto.ARGON2_MEMORY_KIB)
                    put("iterations", VaultCrypto.ARGON2_ITERATIONS)
                    put("parallelism", VaultCrypto.ARGON2_PARALLELISM)
                    put("output_len", 32)
                })
                put("salt", VaultCrypto.run { salt.toHex() })
                put("wrapped_master_key", VaultCrypto.run { wrapped.toHex() })
                put("wrap_nonce", VaultCrypto.run { nonce.toHex() })
                put("header_hmac", ""); put("recovery_enabled", false)
                put("wrapped_master_key_recovery", JsonNull)
                put("recovery_wrap_nonce", JsonNull); put("recovery_salt", JsonNull)
                put("created_at", now); put("updated_at", now)
                put("generation", 1); put("committed", true)
            }
            val signed = JsonObject(header + ("header_hmac" to
                JsonPrimitive(hmacSha256Hex(kek, canonicalHeaderJson(header, true)))))
            // One committed atomic header is enough. Never create a second copy with a new identity.
            io.write(HEADER_REL, canonicalHeaderJson(signed, false))
            transferred = true
            return VaultSession(id, master)
        } finally {
            kek?.fill(0)
            if (!transferred) master.fill(0)
        }
    }

    private fun chooseHeader(): Pair<String, kotlinx.serialization.json.JsonElement> {
        val candidates = mutableListOf<String>()
        if (io.exists(HEADER_REL)) candidates += HEADER_REL
        if (io.exists(HEADER_B_REL)) candidates += HEADER_B_REL
        if (candidates.isEmpty()) throw VaultAccessException("no vault header found")

        fun parse(path: String): kotlinx.serialization.json.JsonElement? =
            try {
                val bytes = io.read(path)
                require(bytes.size <= 64 * 1024) { "Vault header exceeds bound" }
                Json.parseToJsonElement(String(bytes, Charsets.UTF_8))
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
        password: ByteArray, salt: ByteArray, memoryKb: Int, iterations: Int, parallelism: Int,
    ): ByteArray {
        require(memoryKb == VaultCrypto.ARGON2_MEMORY_KIB &&
            iterations == VaultCrypto.ARGON2_ITERATIONS && parallelism == VaultCrypto.ARGON2_PARALLELISM)
        return VaultCrypto.deriveKek(password, salt)
    }

    // ------------------------------------------------------------------
    // Records
    // ------------------------------------------------------------------

    /**
     * One metadata map per record, read from the PLAINTEXT envelope metadata
     * (no decrypt — same fact the desktop's scan_record_metadata relies on).
     * Unparseable envelopes are skipped, never fatal. Caller must hold a
     * session only to guarantee an attached vault; metadata itself is not
     * secret (the ciphertext is).
     */
    fun listRecordMetadata(session: VaultSession): List<Map<String, Any?>> {
        session.requireOpen()
        val names = try {
            io.list(RECORDS_DIR)
        } catch (_: Exception) {
            return emptyList()
        }
        val out = ArrayList<Map<String, Any?>>(names.size)
        for (name in names) {
            if (!name.endsWith(".enc.json")) continue
            try {
                val envelope =
                    Json.parseToJsonElement(String(io.read("$RECORDS_DIR/$name"), Charsets.UTF_8))
                        .jsonObject
                val metadata = envelope.getValue("metadata").jsonObject
                out.add(metadata.entries.associate { (k, v) -> k to jsonValueToKotlin(v) })
            } catch (_: Exception) {
                // Foreign/corrupt envelope — skipped, never fatal. The vault
                // module has no logging dependency by design; the hydrator
                // counts and surfaces what it actually pulled.
            }
        }
        return out
    }

    /** Read + decrypt a record. Verifies canonical AAD before decrypting. */
    fun readRecord(session: VaultSession, recordId: String): Pair<Map<String, Any?>, ByteArray> {
        session.requireOpen()
        require(java.util.UUID.fromString(recordId).toString() == recordId)
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

        // Record envelopes use HEX for nonce/ciphertext (vault-core write_record),
        // matching associated_data. Header fields are hex too (see unlock).
        val nonce = hex(envelope.getValue("nonce").jsonPrimitive.content)
        val ciphertext = hex(envelope.getValue("encrypted_content").jsonPrimitive.content)
        val domainKey = VaultCrypto.deriveRecordDomainKey(session.masterKey)
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
        } finally { domainKey.fill(0) }
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
        session.requireOpen()
        require(content.size <= 4 * 1024 * 1024) { "Record too large" }
        val recordId = fields["record_id"] as? String
            ?: throw VaultAccessException("record_id required in fields")
        require(java.util.UUID.fromString(recordId).toString() == recordId)
        val aad = VaultCrypto.canonicalAad(fields)
        val actualNonce = nonce?.also {
            require(it.size == 12) { "injected nonce must be 12 bytes" }
        } ?: java.security.SecureRandom().let { r ->
            ByteArray(12).also { r.nextBytes(it) }
        }
        val domainKey = VaultCrypto.deriveRecordDomainKey(session.masterKey)
        val ciphertext = try { VaultCrypto.encryptRecords(domainKey, actualNonce, content, aad) } finally { domainKey.fill(0) }
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
        val (fields, oldContent) = readRecord(session, recordId)
        oldContent.fill(0)
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
        require(java.util.UUID.fromString(recordId).toString() == recordId)
        val aad = VaultCrypto.canonicalAad(fields)
        val domainKey = VaultCrypto.deriveRecordDomainKey(session.masterKey)
        val nonce = ByteArray(12).also { java.security.SecureRandom().nextBytes(it) }
        val ciphertext = try { VaultCrypto.encryptRecords(domainKey, nonce, content, aad) } finally { domainKey.fill(0) }
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
    private fun hex(s: String): ByteArray {
        require(s.length % 2 == 0 && s.length <= 32 * 1024 * 1024 && s.all { it in "0123456789abcdefABCDEF" })
        return ByteArray(s.length / 2) { index ->
            ((s[index * 2].digitToInt(16) shl 4) or s[index * 2 + 1].digitToInt(16)).toByte()
        }
    }
}

/** Every vault failure mode a caller can act on — never swallowed. */
class VaultAccessException(message: String, cause: Throwable? = null) : Exception(message, cause)
