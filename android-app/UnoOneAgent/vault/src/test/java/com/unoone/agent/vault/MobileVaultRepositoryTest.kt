package com.unoone.agent.vault

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/** In-memory VaultIO for JVM tests — no SAF, no physical drive. */
private class MemoryIO(
    initial: Map<String, ByteArray> = emptyMap()
) : VaultIO {
    val files = initial.toMutableMap()
    override fun read(relativePath: String): ByteArray =
        files[relativePath] ?: error("missing: $relativePath")
    override fun write(relativePath: String, bytes: ByteArray) {
        files[relativePath] = bytes
    }
    override fun exists(relativePath: String): Boolean = files.containsKey(relativePath)
    override fun list(relativePath: String): List<String> =
        files.keys.filter { it.startsWith("$relativePath/") }
            .map { it.removePrefix("$relativePath/").substringBefore('/') }.distinct()
    override fun delete(relativePath: String): Boolean = files.remove(relativePath) != null
}

class MobileVaultRepositoryTest {

    private val fixtureDir: File by lazy {
        val wanted = File("packages/vault-core/test-vectors/synthetic-vault")
        var dir = File(System.getProperty("user.dir")!!)
        repeat(8) {
            val candidate = File(dir, wanted.path)
            if (candidate.exists()) return@lazy candidate
            dir = dir.parentFile ?: return@repeat
        }
        error("synthetic-vault fixture not found above ${System.getProperty("user.dir")}")
    }

    private fun fixtureJson() =
        Json.parseToJsonElement(File(fixtureDir, "fixture.json").readText()).jsonObject

    /** Fixture header written by RUST — the real unlock contract. */
    private fun rustBackedIO(): MemoryIO {
        val f = fixtureJson()
        return MemoryIO(
            mapOf(
                "VAULT/header/header_a.json" to
                    File(fixtureDir, f.getValue("header_path").jsonPrimitive.content).readBytes(),
                *f.getValue("records").jsonArray.map { r ->
                    "VAULT/records/${recordIdOf(fixtureDir, r.jsonObject.getValue("path").jsonPrimitive.content)}.enc.json" to
                        File(fixtureDir, r.jsonObject.getValue("path").jsonPrimitive.content).readBytes()
                }.toTypedArray()
            )
        )
    }

    private fun recordIdOf(dir: File, name: String): String {
        val obj = Json.parseToJsonElement(File(dir, name).readText()).jsonObject
        return obj.getValue("metadata").jsonObject.getValue("record_id").jsonPrimitive.content
    }

    @Test
    fun `unlock with synthetic password against RUST header yields working decryption`() {
        val io = rustBackedIO()
        val repo = MobileVaultRepository(io)
        val password = fixtureJson().getValue("password_utf8").jsonPrimitive.content
        val session = repo.unlock(password.toByteArray(Charsets.UTF_8))
        assertTrue(session.vaultId.isNotEmpty())

        for (rec in fixtureJson().getValue("records").jsonArray) {
            val id = recordIdOf(fixtureDir, rec.jsonObject.getValue("path").jsonPrimitive.content)
            val (_, content) = repo.readRecord(session, id)
            assertEquals(
                rec.jsonObject.getValue("expected_content_utf8").jsonPrimitive.content,
                String(content, Charsets.UTF_8)
            )
        }
    }

    @Test
    fun `wrong password fails before any key exists`() {
        val repo = MobileVaultRepository(rustBackedIO())
        assertThrows(VaultAccessException::class.java) {
            repo.unlock("synthetic-but-wrong-password".toByteArray(Charsets.UTF_8))
        }
    }

    @Test
    fun `tampered header fails HMAC`() {
        val io = rustBackedIO()
        val header = String(io.read("VAULT/header/header_a.json"), Charsets.UTF_8)
        val tampered = header.replace("\"generation\": 1", "\"generation\": 2")
        io.write("VAULT/header/header_a.json", tampered.toByteArray(Charsets.UTF_8))
        val password = fixtureJson().getValue("password_utf8").jsonPrimitive.content
        assertThrows(VaultAccessException::class.java) {
            repo_unlock(io, password)
        }
    }

    private fun repo_unlock(io: VaultIO, password: String) {
        MobileVaultRepository(io).unlock(password.toByteArray(Charsets.UTF_8))
    }

    @Test
    fun `write then read then tombstone round trips with canonical AAD`() {
        // Rust fixture provides the unlock path; the write side is crypto-verified
        // by the pinned record vectors (same primitives) and re-read here.
        val io = rustBackedIO()
        val repo = MobileVaultRepository(io)
        val password = fixtureJson().getValue("password_utf8").jsonPrimitive.content
        val session = repo.unlock(password.toByteArray(Charsets.UTF_8))

        val newId = "3f4a5b6c-7d8e-4f9a-8b0c-1d2e3f4a5b01"
        val fields = mapOf<String, Any?>(
            "record_id" to newId,
            "record_type" to "MEMORY",
            "schema_version" to 1,
            "encryption_version" to 1,
            "created_at" to "2026-08-01T00:00:00+00:00",
            "updated_at" to "2026-08-01T00:00:00+00:00",
            "revision" to 1,
            "origin_platform" to "ANDROID",
            "origin_device_id" to "synthetic-android-test",
            "transaction_id" to newId,
            "content_hash" to "",
            "parent_record_id" to null,
            "source_record_ids" to emptyList<String>(),
            "privacy_level" to "PRIVATE",
            "tombstone" to false,
            "deleted_at" to null,
        )
        val body = "written by android side — पोहरिंग"
        repo.writeRecord(session, fields, body.toByteArray(Charsets.UTF_8))
        val (_, back) = repo.readRecord(session, newId)
        assertEquals(body, String(back, Charsets.UTF_8))

        repo.tombstoneRecord(session, newId, "2026-08-01T01:00:00+00:00")
        val (meta, tombBody) = repo.readRecord(session, newId)
        assertEquals(true, meta["tombstone"])
        assertEquals("2026-08-01T01:00:00+00:00", meta["deleted_at"])
        assertEquals(2, meta["revision"])
        assertEquals(0, tombBody.size)
    }

    @Test
    fun `slot selection ignores uncommitted generation`() {
        val io = rustBackedIO()
        // Inject a slot B with a HIGHER generation but committed=false.
        val headerA = Json.parseToJsonElement(String(io.read("VAULT/header/header_a.json"), Charsets.UTF_8)).jsonObject
        val fakeB = buildString {
            val obj = headerA.toMutableMap()
            append("{")
            obj.entries.forEachIndexed { i, (k, v) ->
                if (i > 0) append(',')
                append('"').append(k).append('"').append(':').append(v.toString())
            }
            append(",\"generation\":99,\"committed\":false}")
        }
        io.write("VAULT/header/header_b.json", fakeB.toByteArray(Charsets.UTF_8))
        val password = fixtureJson().getValue("password_utf8").jsonPrimitive.content
        val session = MobileVaultRepository(io).unlock(password.toByteArray(Charsets.UTF_8))
        assertTrue(session.vaultId.isNotEmpty())
    }

    // ------------------------------------------------------------------
    // The bidirectional vault seal — Kotlin half.
    // ------------------------------------------------------------------

    private fun hexToBytes(hex: String): ByteArray =
        hex.chunked(2).map { it.toInt(16).toByte() }.toByteArray()

    /** JSON fixture values → the typed map writeRecord's canonicalAad expects. */
    private fun typedFields(obj: JsonObject): Map<String, Any?> = obj.entries.associate { (k, v) ->
        k to when (v) {
            is JsonNull -> null
            is JsonArray -> v.map { it.jsonPrimitive.content }
            else -> {
                val p = v.jsonPrimitive
                when {
                    p.isString -> p.content
                    p.content == "true" -> true
                    p.content == "false" -> false
                    else -> p.content.toInt()
                }
            }
        }
    }

    @Test
    fun `writeRecord with the pinned nonce reproduces the committed Kotlin-authored envelope byte for byte`() {
        // The other half of this seal is packages/vault-core/tests/
        // kotlin_envelope_read.rs, which feeds the SAME committed envelope
        // through Rust's real Vault::open → unlock → read_record path. This
        // half proves the committed bytes are exactly what the real Android
        // writer produces — together they machine-check that a record written
        // on the phone is readable on the laptop, on every CI run.
        val fx = Json.parseToJsonElement(
            File(fixtureDir, "kotlin-envelope-fixture.json").readText()
        ).jsonObject
        val committedEnvelope =
            File(fixtureDir, fx.getValue("envelope_path").jsonPrimitive.content).readBytes()

        val io = MemoryIO(
            mapOf(
                "VAULT/header/header_a.json" to
                    File(fixtureDir, fx.getValue("header_path").jsonPrimitive.content).readBytes()
            )
        )
        val repo = MobileVaultRepository(io)
        val session = repo.unlock(
            fx.getValue("password_utf8").jsonPrimitive.content.toByteArray(Charsets.UTF_8)
        )

        val fields = typedFields(fx.getValue("fields").jsonObject)
        val content = fx.getValue("content_utf8").jsonPrimitive.content.toByteArray(Charsets.UTF_8)
        val nonce = hexToBytes(fx.getValue("nonce_hex").jsonPrimitive.content)

        val id = repo.writeRecord(session, fields, content, nonce)

        assertEquals(fx.getValue("record_id").jsonPrimitive.content, id)
        assertArrayEquals(
            "writeRecord must reproduce the committed envelope byte-for-byte",
            committedEnvelope,
            io.files.getValue("VAULT/records/$id.enc.json")
        )

        // And the writer's own read path accepts what it wrote.
        val (roundFields, roundContent) = repo.readRecord(session, id)
        assertArrayEquals(content, roundContent)
        assertEquals("ANDROID", roundFields["origin_platform"])
    }
}
