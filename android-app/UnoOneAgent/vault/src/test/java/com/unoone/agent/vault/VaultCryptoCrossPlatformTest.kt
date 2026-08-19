package com.unoone.agent.vault

import org.bouncycastle.crypto.generators.Argon2BytesGenerator
import org.bouncycastle.crypto.params.Argon2Parameters
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.io.File

/**
 * Cross-platform crypto contract (both directions, no phone):
 * Rust's checked-in vectors are decrypted/reproduced HERE, and the Kotlin
 * ciphertext for the paired vector must equal the bytes Rust itself has
 * pinned — so Rust's decrypt check (packages/vault-core/tests) covers
 * Rust→Kotlin and this file covers Kotlin→Rust.
 */
class VaultCryptoCrossPlatformTest {

    private val vectorsText: String by lazy {
        val wanted = File("packages/vault-core/test-vectors/vault-cross-platform.json")
        var dir = File(System.getProperty("user.dir")!!)
        repeat(8) {
            val candidate = File(dir, wanted.path)
            if (candidate.exists()) return@lazy candidate.readText()
            dir = dir.parentFile ?: return@repeat
        }
        error("vault-cross-platform.json not found above ${System.getProperty("user.dir")}")
    }

    private val root: JsonObject by lazy {
        Json.parseToJsonElement(vectorsText).jsonObject
    }

    private fun hexToBytes(hex: String): ByteArray =
        hex.chunked(2).map { it.toInt(16).toByte() }.toByteArray()

    @Test
    fun `spec marker matches the Rust test`() {
        assertEquals("unoone-vault-cross-platform/1", root.getValue("spec").jsonPrimitive.content)
    }

    @Test
    fun `argon2id KEK vector matches BouncyCastle at SPEC params`() {
        for (kdf in root.getValue("kdf").jsonArray) {
            val obj = kdf.jsonObject
            assertEquals(VaultCrypto.ARGON2_MEMORY_KIB, obj.getValue("memory_kib").jsonPrimitive.int)
            assertEquals(VaultCrypto.ARGON2_ITERATIONS, obj.getValue("iterations").jsonPrimitive.int)
            assertEquals(VaultCrypto.ARGON2_PARALLELISM, obj.getValue("parallelism").jsonPrimitive.int)
            val params = Argon2Parameters.Builder(Argon2Parameters.ARGON2_id)
                .withVersion(Argon2Parameters.ARGON2_VERSION_13)
                .withIterations(VaultCrypto.ARGON2_ITERATIONS)
                .withMemoryAsKB(VaultCrypto.ARGON2_MEMORY_KIB)
                .withParallelism(VaultCrypto.ARGON2_PARALLELISM)
                .withSalt(hexToBytes(obj.getValue("salt_hex").jsonPrimitive.content))
                .build()
            val out = ByteArray(VaultCrypto.KEY_LEN)
            Argon2BytesGenerator().apply { init(params) }.generateBytes(
                obj.getValue("password_utf8").jsonPrimitive.content
                    .toByteArray(Charsets.UTF_8),
                out
            )
            assertEquals(
                "KDF drift for ${obj.getValue("name").jsonPrimitive.content}",
                obj.getValue("expected_key_hex").jsonPrimitive.content,
                VaultCrypto.run { out.toHex() }
            )
        }
    }

    @Test
    fun `record vectors decrypt and re-encrypt identically`() {
        var sawRustEncrypt = false
        var sawKotlinEncrypt = false
        for (vec in root.getValue("record").jsonArray) {
            val v = vec.jsonObject
            val name = v.getValue("name").jsonPrimitive.content
            val master = hexToBytes(v.getValue("master_key_hex").jsonPrimitive.content)
            val domainKey = VaultCrypto.deriveRecordDomainKey(
                master, v.getValue("domain").jsonPrimitive.content
            )
            val nonce = hexToBytes(v.getValue("nonce_hex").jsonPrimitive.content)
            val aad = hexToBytes(v.getValue("aad_hex").jsonPrimitive.content)
            val plaintext = v.getValue("plaintext_utf8").jsonPrimitive.content.toByteArray(Charsets.UTF_8)
            val ciphertext = hexToBytes(v.getValue("ciphertext_hex").jsonPrimitive.content)

            // 1) Kotlin MUST reconstruct the canonical AAD byte-identically.
            val recordFields = flattenRecordJson(v.getValue("record_json").jsonObject)
            assertEquals(
                "AAD construction drift for $name",
                v.getValue("aad_hex").jsonPrimitive.content,
                VaultCrypto.run { VaultCrypto.canonicalAad(recordFields).toHex() }
            )

            // 2) Both directions: decrypt the pinned ciphertext to the pinned plaintext.
            val decrypted = VaultCrypto.decryptRecords(domainKey, nonce, ciphertext, aad)
            assertArrayEquals("Rust→Kotlin decrypt drift for $name", plaintext, decrypted)

            // 3) Kotlin encrypt from the SAME inputs must equal the pinned ciphertext
            // (this is the Kotlin→Rust direction: Rust's test then decrypts it).
            val reEncrypted = VaultCrypto.encryptRecords(domainKey, nonce, plaintext, aad)
            assertArrayEquals("Kotlin encrypt drift for $name", ciphertext, reEncrypted)

            when {
                name.startsWith("rust-encrypt") -> sawRustEncrypt = true
                name.startsWith("kotlin-encrypt") -> sawKotlinEncrypt = true
            }
        }
        assertTrue("vectors must cover rust-encrypt kind", sawRustEncrypt)
        assertTrue("vectors must cover kotlin-encrypt kind", sawKotlinEncrypt)
    }


    /**
     * The canonical-AAD string escaping is a CROSS-PLATFORM CONTRACT.
     *
     * `VaultCrypto.jsonEscape` is hand-rolled, so it must reproduce
     * `serde_json::to_string` byte for byte. The Rust read path re-derives the
     * AAD from the stored metadata and rejects the record when the bytes
     * differ, so any divergence shows up on the desktop as "metadata was
     * altered after the record was written" -- a tamper error for a perfectly
     * honest record written by Android.
     *
     * The same `escaping` vectors are re-derived from real serde_json output by
     * the Rust test `json_string_escaping_matches_serde_json_for_every_pinned_case`,
     * so neither side can drift without a red build.
     *
     * Regression pinned here: backspace and form feed must serialise as the
     * two-character short escapes, not the six-character form.
     */
    @Test
    fun `json escaping matches serde_json for every pinned case`() {
        val cases = root.getValue("escaping").jsonArray
        assertTrue("escaping vectors required", cases.isNotEmpty())

        // A complete, valid 16-field record; only origin_device_id varies.
        val template = flattenRecordJson(
            root.getValue("record").jsonArray[0].jsonObject
                .getValue("record_json").jsonObject
        ).toMutableMap()

        var sawBackspaceFormfeed = false

        for (case in cases) {
            val obj = case.jsonObject
            val name = obj.getValue("name").jsonPrimitive.content
            val input = String(
                hexToBytes(obj.getValue("input_utf8_hex").jsonPrimitive.content),
                Charsets.UTF_8
            )
            val expected = obj.getValue("serde_json").jsonPrimitive.content

            // Exercise the REAL path: canonicalAad, not jsonEscape directly.
            template["origin_device_id"] = input
            val aad = String(VaultCrypto.canonicalAad(template), Charsets.UTF_8)

            // `expected` already includes the surrounding quotes, so this is an
            // exact assertion on how that field was serialised.
            assertTrue(
                "escaping drift for '$name': AAD did not contain " +
                    "\"origin_device_id\":$expected",
                aad.contains("\"origin_device_id\":$expected")
            )

            if (name == "backspace_formfeed") {
                sawBackspaceFormfeed = true
                assertTrue(
                    "backspace/form feed must use the short escapes",
                    expected.contains("\\b") && expected.contains("\\f")
                )
                assertTrue(
                    "backspace/form feed must not use the six-character form",
                    !expected.contains("\\u0008") && !expected.contains("\\u000C")
                )
            }
        }

        assertTrue(
            "the backspace/form-feed case must stay pinned -- it is the known divergence",
            sawBackspaceFormfeed
        )
    }

    private fun flattenRecordJson(obj: JsonObject): Map<String, Any?> =
        obj.entries.associate { (k, elem) -> k to jsonToKotlin(elem) }

    private fun jsonToKotlin(elem: JsonElement): Any? = when {
        elem is JsonNull -> null
        elem is JsonArray -> elem.map { jsonToKotlin(it) as String }
        elem.jsonPrimitive.isString -> elem.jsonPrimitive.content
        else -> try {
            elem.jsonPrimitive.int
        } catch (_: Exception) {
            elem.jsonPrimitive.boolean
        }
    }

    @Test
    fun `master key wrap vector unwraps and wraps identically (unlock path)`() {
        val wraps = root.getValue("wrap").jsonArray
        val w = wraps.first().jsonObject
        val kek = hexToBytes(w.getValue("kek_hex").jsonPrimitive.content)
        val nonce = hexToBytes(w.getValue("nonce_hex").jsonPrimitive.content)
        val master = hexToBytes(w.getValue("master_key_hex").jsonPrimitive.content)
        val wrapped = hexToBytes(w.getValue("wrapped_hex").jsonPrimitive.content)
        assertEquals("unoone-vault-master-key-wrap", w.getValue("aad_utf8").jsonPrimitive.content)

        // Rust→Kotlin: Kotlin must UNWRAP Rust's wrapped master key.
        val back = VaultCrypto.unwrapMasterKeyWithAad(kek, wrapped, nonce)
        assertArrayEquals("unwrap drift (unlock path)", master, back)
        // Kotlin→Rust symmetry: Kotlin's wrap must equal Rust's bytes.
        val rewrapped = VaultCrypto.wrapMasterKeyWithAad(kek, master, nonce)
        assertArrayEquals("wrap drift (unlock path)", wrapped, rewrapped)
    }
}
