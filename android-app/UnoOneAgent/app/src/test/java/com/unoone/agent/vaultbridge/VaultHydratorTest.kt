package com.unoone.agent.vaultbridge

import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import com.unoone.agent.storage.db.UnoOneDatabase
import com.unoone.agent.storage.entity.MemoryEntity
import com.unoone.agent.storage.entity.SkillEntity
import com.unoone.agent.vault.VaultRecordReader
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The vault→Android hydration path against a REAL in-memory Room database
 * and a fake [VaultRecordReader] that plays records authored on other hosts.
 * Pins every safety/honesty rule the hydrator promises:
 *
 * - known record ids and tombstones pull nothing (no decrypt, no resurrect);
 * - only {kind:"memory"} / {kind:"skill"} envelopes hydrate — raw text and
 *   foreign payloads are skipped, never structured into fake data;
 * - local-only rows are ADOPTED (linked), synced rows update only on a
 *   strictly newer revision, and two different vault records for the same
 *   key/name never silently overwrite each other.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class VaultHydratorTest {

    private lateinit var db: UnoOneDatabase

    /** Plays the remote vault: metadata listing + per-record content reads. */
    private class FakeReader : VaultRecordReader {
        val records = LinkedHashMap<String, Pair<Map<String, Any?>, ByteArray>>()
        var metadataOverrides: MutableMap<String, Map<String, Any?>> = mutableMapOf()
        var readAttempts = 0

        private fun baseMeta(id: String, type: String, revision: Int) = mapOf<String, Any?>(
            "record_id" to id,
            "record_type" to type,
            "revision" to revision,
            "tombstone" to false,
            "created_at" to "2026-10-01T09:00:00Z",
            "updated_at" to "2026-10-01T09:00:00Z",
        )

        fun addMemory(id: String, key: String, value: String, type: String = "preference", revision: Int = 1) {
            val payload = """{"kind":"memory","key":"$key","value":"$value","type":"$type"}"""
            records[id] = baseMeta(id, "MEMORY", revision) to payload.toByteArray()
        }

        fun addSkill(id: String, name: String, revision: Int = 1) {
            val payload = """{"kind":"skill","name":"$name","triggerPhrases":"go","stepsJson":"[]","riskLevel":0,"enabled":true}"""
            records[id] = baseMeta(id, "DOCUMENT", revision) to payload.toByteArray()
        }

        fun addTurn(id: String, sessionId: String, role: String, content: String, inputType: String = "voice") {
            val payload = """{"kind":"transcript","sessionId":"$sessionId","role":"$role","content":"$content","inputType":"$inputType"}"""
            records[id] = baseMeta(id, "TRANSCRIPT", 1) to payload.toByteArray()
        }

        fun addRawText(id: String, type: String = "MEMORY") {
            records[id] = baseMeta(id, type, 1) to "just some migrated plaintext".toByteArray()
        }

        /** A user-confirmed env fact authored on ANOTHER host (kind:"envobs" envelope). */
        fun addEnvFact(
            id: String,
            subject: String,
            status: String = "verified_fact",
            revision: Int = 1,
        ) {
            val observation = (
                "{" +
                    "\"schema\":\"inbharat.pai.envobs.v1\"," +
                    "\"subject\":\"$subject\"," +
                    "\"observed_capability\":\"execute skill '$subject'\"," +
                    "\"evidence\":\"user explicitly enabled skill '$subject' in the Skills screen\"," +
                    "\"confidence\":\"high\",\"scope\":\"user\"," +
                    "\"epistemic_status\":\"$status\"," +
                    "\"provenance\":{\"platform\":\"android\",\"device_id\":\"other-phone\",\"source\":\"user-approval\"}," +
                    "\"timestamp_ms\":1760000000000," +
                    "\"verification_ref\":\"user_enabled_skill:$subject@1760000000000\"" +
                    "}"
                ).replace("\\", "\\\\").replace("\"", "\\\"")
            val payload =
                "{" +
                    "\"kind\":\"envobs\",\"subject\":\"$subject\"," +
                    "\"observedCapability\":\"execute skill '$subject'\"," +
                    "\"epistemicStatus\":\"$status\"," +
                    "\"verificationRef\":\"user_enabled_skill:$subject@1760000000000\"," +
                    "\"observationJson\":\"$observation\"" +
                    "}"
            records[id] = baseMeta(id, "DOCUMENT", revision) to payload.toByteArray()
        }

        fun tombstone(id: String) {
            val (meta, content) = records.getValue(id)
            records[id] = meta + mapOf("tombstone" to true) to content
        }

        override fun listRecordMetadata(): List<Map<String, Any?>> =
            records.values.map { it.first }

        override fun readRecord(recordId: String): Pair<Map<String, Any?>, ByteArray> {
            readAttempts++
            return records.getValue(recordId).let { it.first to it.second }
        }
    }

    private val reader = FakeReader()

    private fun hydrator() = VaultHydrator(
        memoryDao = db.memoryDao(),
        skillDao = db.skillDao(),
        turnDao = db.conversationTurnDao(),
        readerProvider = { reader },
    )

    @Before
    fun setUp() {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        db = Room.inMemoryDatabaseBuilder(context, UnoOneDatabase::class.java)
            .allowMainThreadQueries()
            .build()
    }

    @After
    fun tearDown() = db.close()

    @Test
    fun `vault memory and skill hydrate into empty cache`() = runBlocking {
        reader.addMemory("rec-m1", "wake_word", "namaste")
        reader.addSkill("rec-s1", "morning briefing")

        val result = hydrator().hydrateFromVault()

        assertEquals(1, result.memoriesAdded)
        assertEquals(1, result.skillsAdded)
        val mem = db.memoryDao().getByKey("wake_word")!!
        assertEquals("namaste", mem.value)
        assertEquals("rec-m1", mem.vaultRecordId)
        assertEquals(1, mem.vaultRevision)
        val skill = db.skillDao().allOnce().single { it.name == "morning briefing" }
        assertEquals("rec-s1", skill.vaultRecordId)
        assertTrue(db.memoryDao().notSynced().isEmpty()) // linked rows never re-mirror
    }

    @Test
    fun `known records and tombstones pull nothing`() = runBlocking {
        reader.addMemory("rec-m1", "wake_word", "namaste")
        reader.addMemory("rec-m2", "dead", "x")
        reader.tombstone("rec-m2")

        // Simulate the first pass having happened.
        hydrator().hydrateFromVault()
        val readBefore = reader.readAttempts

        // Second unlock: everything known or deleted — no decrypts at all.
        val result = hydrator().hydrateFromVault()

        assertEquals(0, result.total)
        assertEquals("no record reads for known/tombstoned records", readBefore, reader.readAttempts)
    }

    @Test
    fun `local-only row for the same key is adopted - vault wins, link set`() = runBlocking {
        db.memoryDao().insert(MemoryEntity(key = "wake_word", value = "old local value", type = "preference"))
        reader.addMemory("rec-m1", "wake_word", "vault value")

        val result = hydrator().hydrateFromVault()

        assertEquals(1, result.memoriesUpdated)
        val mem = db.memoryDao().getByKey("wake_word")!!
        assertEquals("vault value", mem.value)
        assertEquals("rec-m1", mem.vaultRecordId)
    }

    @Test
    fun `same-record update applies only on strictly newer revision`() = runBlocking {
        db.memoryDao().insert(
            MemoryEntity(key = "wake_word", value = "local", type = "preference", vaultRecordId = "rec-m1", vaultRevision = 3),
        )
        reader.addMemory("rec-m1", "wake_word", "older", revision = 2)

        val stale = hydrator().hydrateFromVault()
        assertEquals(0, stale.total)
        assertEquals("local", db.memoryDao().getByKey("wake_word")!!.value)

        reader.records["rec-m1"] = reader.records["rec-m1"]!!.let { (meta, content) ->
            (meta + mapOf("revision" to 4)) to content
        }
        // The envelope value changes too (the fake keeps old bytes; update
        // via a fresh record for clarity).
        reader.records["rec-m1"] = reader.records["rec-m1"]!!.first to
            """{"kind":"memory","key":"wake_word","value":"newer","type":"preference"}""".toByteArray()

        val result = hydrator().hydrateFromVault()
        assertEquals(1, result.memoriesUpdated)
        assertEquals("newer", db.memoryDao().getByKey("wake_word")!!.value)
        assertEquals(4, db.memoryDao().getByKey("wake_word")!!.vaultRevision)
    }

    @Test
    fun `two different vault records for one key never overwrite each other`() = runBlocking {
        db.memoryDao().insert(
            MemoryEntity(key = "wake_word", value = "mine", type = "preference", vaultRecordId = "rec-local", vaultRevision = 1),
        )
        reader.addMemory("rec-remote", "wake_word", "theirs")

        val result = hydrator().hydrateFromVault()

        assertEquals(0, result.memoriesUpdated + result.memoriesAdded)
        assertEquals("mine", db.memoryDao().getByKey("wake_word")!!.value)
        assertEquals("rec-local", db.memoryDao().getByKey("wake_word")!!.vaultRecordId)
    }

    @Test
    fun `raw migrated text never becomes a fake memory or skill`() = runBlocking {
        reader.addRawText("rec-raw", type = "MEMORY")
        reader.addRawText("rec-doc", type = "DOCUMENT")

        val result = hydrator().hydrateFromVault()

        assertEquals(0, result.total)
        assertEquals(2, result.skippedUnknown)
        assertTrue(db.memoryDao().allOnce().isEmpty())
        assertTrue(db.skillDao().allOnce().isEmpty())
    }

    @Test
    fun `vault skill update with newer revision refreshes the local row`() = runBlocking {
        reader.addSkill("rec-s1", "morning briefing")
        hydrator().hydrateFromVault()
        val local = db.skillDao().allOnce().single { it.name == "morning briefing" }

        // Remote saves revision 2 with a changed trigger set.
        reader.records["rec-s1"] = reader.records["rec-s1"]!!.let { (meta, _) ->
            (meta + mapOf("revision" to 2)) to
                """{"kind":"skill","name":"morning briefing","triggerPhrases":"good morning","stepsJson":"[]","riskLevel":0,"enabled":false}"""
                    .toByteArray()
        }
        val result = hydrator().hydrateFromVault()

        assertEquals(1, result.skillsUpdated)
        val updated = db.skillDao().allOnce().single { it.name == "morning briefing" }
        assertEquals("good morning", updated.triggerPhrases)
        assertEquals(false, updated.enabled)
        assertEquals(2, updated.vaultRevision)
        assertEquals(local.id, updated.id) // in-place update, never a duplicate row
    }

    @Test
    fun `null reader when locked means zero pulls and zero errors`() = runBlocking {
        val locked = VaultHydrator(
            memoryDao = db.memoryDao(),
            skillDao = db.skillDao(),
            turnDao = db.conversationTurnDao(),
            readerProvider = { null },
        )
        assertEquals(0, locked.hydrateFromVault().total)
        assertNull(db.memoryDao().getByKey("anything"))
    }

    @Test
    fun `transcript record authored on another host hydrates as a turn`() = runBlocking {
        reader.addTurn("rec-t1", "sess-power-1", "user", "open my notes")
        reader.addTurn("rec-t2", "sess-power-1", "assistant", "Notes opened.")

        val result = hydrator().hydrateFromVault()

        assertEquals(2, result.turnsAdded)
        val session = db.conversationTurnDao().getSession("sess-power-1")
        assertEquals(listOf("user", "assistant"), session.map { it.role })
        assertEquals("open my notes", session[0].content)
        assertEquals("rec-t1", session[0].vaultRecordId)
        assertTrue(db.conversationTurnDao().notSynced().isEmpty()) // linked rows never re-mirror
    }

    @Test
    fun `hydrated transcript record never re-pulls or re-mirrors`() = runBlocking {
        reader.addTurn("rec-t1", "sess-power-1", "user", "hello")
        hydrator().hydrateFromVault()
        val readBefore = reader.readAttempts

        val second = hydrator().hydrateFromVault()

        assertEquals(0, second.total)
        assertEquals("known transcript needs no decrypt", readBefore, reader.readAttempts)
        assertEquals(1, db.conversationTurnDao().getSession("sess-power-1").size)
    }

    @Test
    fun `foreign transcript payload without the shared envelope is skipped`() = runBlocking {
        // A desktop voice-recording transcript: raw text content, no envelope.
        reader.addRawText("rec-voice", type = "TRANSCRIPT")

        val result = hydrator().hydrateFromVault()

        assertEquals(0, result.total)
        assertEquals(1, result.skippedUnknown)
        assertTrue(db.conversationTurnDao().allOnce().isEmpty())
    }

    @Test
    fun `env fact confirmed on another host hydrates as a verified capability`() = runBlocking {
        reader.addEnvFact("rec-env1", "Suggested · Open Calendar")

        val result = hydrator().hydrateFromVault()

        assertEquals(1, result.envFactsAdded)
        val fact = db.memoryDao().getByKey("envfact:Suggested · Open Calendar")!!
        assertEquals("envobs", fact.type)
        assertEquals("rec-env1", fact.vaultRecordId)
        val obs = com.unoone.agent.core.contracts.ContractJson.decodeFromString(
            com.unoone.agent.core.contracts.EnvObservation.serializer(), fact.value,
        )
        assertTrue(obs.mayAuthorizeDeviceControl())
        assertTrue(db.memoryDao().notSynced().isEmpty()) // linked rows never re-mirror
    }

    @Test
    fun `a leaked hypothesis-status envobs payload never hydrates`() = runBlocking {
        reader.addEnvFact("rec-env-hypo", "risky guess", status = "hypothesis")

        val result = hydrator().hydrateFromVault()

        assertEquals(0, result.total)
        assertEquals(1, result.skippedUnknown)
        assertNull(db.memoryDao().getByKey("envfact:risky guess"))
    }
}