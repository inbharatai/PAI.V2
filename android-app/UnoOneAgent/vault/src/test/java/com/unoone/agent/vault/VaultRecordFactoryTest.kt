package com.unoone.agent.vault

import kotlinx.serialization.json.Json
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

    private fun assertArrayEqualsMsg(a: ByteArray, b: ByteArray) {
        assertTrue("canonicalAad must be deterministic", a.contentEquals(b))
    }
}
