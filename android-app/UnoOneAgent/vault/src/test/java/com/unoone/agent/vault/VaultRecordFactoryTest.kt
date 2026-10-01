package com.unoone.agent.vault

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the note/memory → canonical-record mapping. This is the contract the
 * desktop reads back, so every field is asserted explicitly and the produced
 * metadata is run through the REAL [VaultCrypto.canonicalAad] to prove it is
 * accepted and byte-stable.
 */
class VaultRecordFactoryTest {

    private val allFields = listOf(
        "record_id", "record_type", "schema_version", "encryption_version",
        "created_at", "updated_at", "revision", "origin_platform",
        "origin_device_id", "transaction_id", "content_hash",
        "parent_record_id", "source_record_ids", "privacy_level",
        "tombstone", "deleted_at",
    )

    @Test
    fun `note maps to a DOCUMENT record with every canonical field`() {
        val m = VaultRecordFactory.forNote(
            recordId = "11111111-1111-4111-8111-111111111111",
            transactionId = "22222222-2222-4222-8222-222222222222",
            deviceId = "test-device",
            title = "Groceries",
            content = "turmeric, cardamom",
            tags = "shopping,kitchen",
            createdAtIso = "2026-08-02T10:00:00+00:00",
            updatedAtIso = "2026-08-02T10:00:00+00:00",
        )

        assertEquals(allFields.toSet(), m.fields.keys)
        assertEquals("11111111-1111-4111-8111-111111111111", m.fields["record_id"])
        assertEquals("DOCUMENT", m.fields["record_type"])
        assertEquals(1, m.fields["schema_version"])
        assertEquals(1, m.fields["encryption_version"])
        assertEquals(1, m.fields["revision"])
        assertEquals("ANDROID", m.fields["origin_platform"])
        assertEquals("test-device", m.fields["origin_device_id"])
        assertNull(m.fields["parent_record_id"])
        assertEquals(emptyList<String>(), m.fields["source_record_ids"])
        assertEquals("PRIVATE", m.fields["privacy_level"])
        assertEquals(false, m.fields["tombstone"])
        assertNull(m.fields["deleted_at"])
        // content_hash is the SHA-256 of the exact content bytes.
        assertEquals(VaultCrypto.sha256Hex(m.content), m.fields["content_hash"])
    }

    @Test
    fun `memory maps to a MEMORY record`() {
        val m = VaultRecordFactory.forMemory(
            recordId = "33333333-3333-4333-8333-333333333333",
            transactionId = "44444444-4444-4444-8444-444444444444",
            deviceId = "test-device",
            key = "wake_word",
            value = "namaste",
            type = "preference",
            createdAtIso = "2026-08-02T10:00:00+00:00",
            updatedAtIso = "2026-08-02T10:00:00+00:00",
        )
        assertEquals(allFields.toSet(), m.fields.keys)
        assertEquals("MEMORY", m.fields["record_type"])
        assertEquals(VaultCrypto.sha256Hex(m.content), m.fields["content_hash"])
    }

    @Test
    fun `produced metadata is accepted by the real canonicalAad and is self-consistent`() {
        val m = VaultRecordFactory.forNote(
            recordId = "11111111-1111-4111-8111-111111111111",
            transactionId = "22222222-2222-4222-8222-222222222222",
            deviceId = "d",
            title = "t",
            content = "c",
            tags = "",
            createdAtIso = "2026-08-02T10:00:00+00:00",
            updatedAtIso = "2026-08-02T10:00:00+00:00",
        )
        // Must not throw (every value is a supported AAD type) and must be
        // deterministic across calls — the AAD is the authentication input.
        val aad1 = VaultCrypto.canonicalAad(m.fields)
        val aad2 = VaultCrypto.canonicalAad(m.fields)
        assertArrayEqualsMsg(aad1, aad2)
        val text = String(aad1, Charsets.UTF_8)
        assertTrue("AAD must be a JSON object", text.startsWith("{") && text.endsWith("}"))
        assertTrue("record_id first per pinned order", text.startsWith("{\"record_id\":"))
    }

    @Test
    fun `note content payload round-trips with title and tags preserved`() {
        val m = VaultRecordFactory.forNote(
            recordId = "id", transactionId = "tx", deviceId = "d",
            title = "Trip", content = "book flights", tags = "travel",
            createdAtIso = "t", updatedAtIso = "t",
        )
        val obj = Json.parseToJsonElement(String(m.content, Charsets.UTF_8)).jsonObject
        assertEquals("note", obj["kind"]!!.jsonPrimitive.content)
        assertEquals("Trip", obj["title"]!!.jsonPrimitive.content)
        assertEquals("book flights", obj["content"]!!.jsonPrimitive.content)
        assertEquals("travel", obj["tags"]!!.jsonPrimitive.content)
    }

    @Test
    fun `skill maps to a DOCUMENT record with the kind skill envelope`() {
        val m = VaultRecordFactory.forSkill(
            recordId = "55555555-5555-4555-8555-555555555555",
            transactionId = "66666666-6666-4666-8666-666666666666",
            deviceId = "test-device",
            name = "morning briefing",
            triggerPhrases = "brief me,morning update",
            stepsJson = "[\"read_screen\",\"speak_response\"]",
            riskLevel = 1,
            enabled = true,
            createdAtIso = "2026-10-01T10:00:00+00:00",
            updatedAtIso = "2026-10-01T10:00:00+00:00",
            revision = 2,
        )
        assertEquals(allFields.toSet(), m.fields.keys)
        assertEquals("DOCUMENT", m.fields["record_type"])
        assertEquals(2, m.fields["revision"])
        assertEquals(VaultCrypto.sha256Hex(m.content), m.fields["content_hash"])

        val obj = Json.parseToJsonElement(String(m.content, Charsets.UTF_8)).jsonObject
        assertEquals("skill", obj["kind"]!!.jsonPrimitive.content)
        assertEquals("morning briefing", obj["name"]!!.jsonPrimitive.content)
        assertEquals("brief me,morning update", obj["triggerPhrases"]!!.jsonPrimitive.content)
        assertEquals("[\"read_screen\",\"speak_response\"]", obj["stepsJson"]!!.jsonPrimitive.content)
        assertEquals(1, obj["riskLevel"]!!.jsonPrimitive.intOrNull)
        assertEquals(true, obj["enabled"]!!.jsonPrimitive.booleanOrNull)
    }

    @Test
    fun `turn maps to a TRANSCRIPT record with the kind transcript envelope`() {
        val m = VaultRecordFactory.forTurn(
            recordId = "77777777-7777-4777-8777-777777777777",
            transactionId = "88888888-8888-4888-8888-888888888888",
            deviceId = "test-device",
            sessionId = "sess-1",
            role = "user",
            content = "what is the weather",
            inputType = "voice",
            createdAtIso = "2026-10-01T10:00:00+00:00",
            updatedAtIso = "2026-10-01T10:00:00+00:00",
        )
        assertEquals(allFields.toSet(), m.fields.keys)
        assertEquals("TRANSCRIPT", m.fields["record_type"])
        assertEquals(1, m.fields["revision"])
        assertEquals(VaultCrypto.sha256Hex(m.content), m.fields["content_hash"])

        val obj = Json.parseToJsonElement(String(m.content, Charsets.UTF_8)).jsonObject
        assertEquals("transcript", obj["kind"]!!.jsonPrimitive.content)
        assertEquals("sess-1", obj["sessionId"]!!.jsonPrimitive.content)
        assertEquals("user", obj["role"]!!.jsonPrimitive.content)
        assertEquals("what is the weather", obj["content"]!!.jsonPrimitive.content)
        assertEquals("voice", obj["inputType"]!!.jsonPrimitive.content)
    }

    @Test
    fun `env fact maps to a DOCUMENT record with the kind envobs envelope`() {
        val observationJson = """{"schema":"inbharat.pai.envobs.v1","subject":"Suggested · Open Calendar"}"""
        val m = VaultRecordFactory.forEnvFact(
            recordId = "99999999-9999-4999-8999-999999999999",
            transactionId = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            deviceId = "test-device",
            subject = "Suggested · Open Calendar",
            observedCapability = "execute skill 'Suggested · Open Calendar'",
            epistemicStatus = "verified_fact",
            verificationRef = "user_enabled_skill:Suggested · Open Calendar@1760000000000",
            observationJson = observationJson,
            createdAtIso = "2026-10-01T10:00:00+00:00",
            updatedAtIso = "2026-10-01T10:00:00+00:00",
            revision = 2,
        )
        assertEquals(allFields.toSet(), m.fields.keys)
        assertEquals("DOCUMENT", m.fields["record_type"])
        assertEquals(2, m.fields["revision"])
        assertEquals(VaultCrypto.sha256Hex(m.content), m.fields["content_hash"])

        val obj = Json.parseToJsonElement(String(m.content, Charsets.UTF_8)).jsonObject
        assertEquals("envobs", obj["kind"]!!.jsonPrimitive.content)
        assertEquals("Suggested · Open Calendar", obj["subject"]!!.jsonPrimitive.content)
        assertEquals("verified_fact", obj["epistemicStatus"]!!.jsonPrimitive.content)
        assertEquals(
            "user_enabled_skill:Suggested · Open Calendar@1760000000000",
            obj["verificationRef"]!!.jsonPrimitive.content,
        )
        // The contract body travels verbatim — hosts index the envelope
        // without parsing, and the body must round-trip byte-identical.
        assertEquals(observationJson, obj["observationJson"]!!.jsonPrimitive.content)
    }

    private fun assertArrayEqualsMsg(a: ByteArray, b: ByteArray) {
        assertTrue("canonicalAad must be deterministic", a.contentEquals(b))
    }
}
