package com.unoone.agent.envlearning

import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import com.unoone.agent.core.contracts.ContractJson
import com.unoone.agent.core.contracts.EnvObservation
import com.unoone.agent.core.contracts.EpistemicStatus
import com.unoone.agent.core.contracts.ProcedureOutcome
import com.unoone.agent.core.contracts.PromotionStatus
import com.unoone.agent.core.model.RiskLevel
import com.unoone.agent.storage.db.UnoOneDatabase
import com.unoone.agent.storage.entity.SkillEntity
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The bounded env-learning producer against a REAL in-memory Room database.
 * Pins the epistemic honesty rules the mission demands:
 *
 * - procedure outcomes record honestly (verified postconditions only from
 *   the REAL verifier verdict; a failure resets the success streak);
 * - the automatic path can produce at most SUGGESTED — never APPROVED;
 * - a suggestion is a device-local HYPOTHESIS (never mirrored);
 * - the user's enable is the ONLY promotion path, and even then only rows
 *   whose other gates hold move to APPROVED (BLOCK/unverified stay honest);
 * - the user's disable is a CORRECTION that demotes APPROVED rows to REJECTED.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class EnvLearningRecorderTest {

    private lateinit var db: UnoOneDatabase
    private val mirroredFacts = ArrayList<Long>()
    private var clockNow = 1_000L

    private fun recorder() = EnvLearningRecorder(
        memoryDao = db.memoryDao(),
        deviceIdProvider = { "test-device" },
        onEnvFactRecorded = { id -> mirroredFacts.add(id) },
        clock = { clockNow },
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

    private suspend fun recordSuccess(
        recorder: EnvLearningRecorder,
        command: String = "open my calendar please",
        tool: String = "open_calendar",
        verified: Boolean = true,
        riskLevel: RiskLevel = RiskLevel.DIRECT,
        argumentsJson: String? = """{"app":"com.google.android.calendar"}""",
    ) = recorder.recordProcedureOutcome(
        command = command,
        tool = tool,
        argumentsJson = argumentsJson,
        success = true,
        verified = verified,
        verificationEvidence = "foregroundPackage=com.google.android.calendar",
        failureReason = null,
        riskLevel = riskLevel,
    )

    private suspend fun procedureRow(tool: String, command: String = "open my calendar please"): ProcedureOutcome {
        val signature = com.unoone.agent.core.memory.OutcomeMemoryPolicy.signature(command)
        val row = db.memoryDao().getByKey("procedure:$tool:$signature")
        assertNotNull("procedure row must exist for $tool/$signature", row)
        return ContractJson.decodeFromString(ProcedureOutcome.serializer(), row!!.value)
    }

    private fun suggestionSkill(name: String) = SkillEntity(
        name = name,
        triggerPhrases = "open my calendar,show my calendar",
        stepsJson = "[\"open calendar\"]",
        riskLevel = 0,
        enabled = false,
    )

    // ---- procedure outcomes ------------------------------------------------

    @Test
    fun `first success is recorded honestly but not repeatable`() = runBlocking {
        val r = recorder()
        recordSuccess(r)

        val record = procedureRow("open_calendar")
        assertEquals(PromotionStatus.NONE, record.promotion.status)
        assertFalse(record.promotion.requirements.repeatableSuccess)
        assertTrue(record.promotion.requirements.boundedArguments)
        assertTrue(record.promotion.requirements.verifiedPostconditions)
        assertTrue(record.promotion.requirements.lowRiskClass)
        assertFalse(record.promotion.requirements.explicitApproval)
    }

    // ---- boundedArguments honesty -------------------------------------------

    @Test
    fun `captured bounded arguments set boundedArguments true with the actual args as evidence`() = runBlocking {
        val r = recorder()
        recordSuccess(r)

        val record = procedureRow("open_calendar")
        assertTrue(record.promotion.requirements.boundedArguments)
        assertTrue(
            "the evidence must carry the ACTUAL serialized arguments, not just a signature",
            record.boundedArguments.contains("\"app\":\"com.google.android.calendar\""),
        )
    }

    @Test
    fun `missing arguments leave boundedArguments honestly false`() = runBlocking {
        val r = recorder()
        recordSuccess(r, argumentsJson = null)

        val record = procedureRow("open_calendar")
        assertFalse("no captured args means boundedness is UNPROVEN", record.promotion.requirements.boundedArguments)
        assertTrue(record.boundedArguments.contains("arguments not captured"))
        assertEquals(PromotionStatus.NONE, record.promotion.status)
    }

    @Test
    fun `oversized arguments leave boundedArguments honestly false`() = runBlocking {
        val r = recorder()
        recordSuccess(r, argumentsJson = "x".repeat(EnvLearningRecorder.MAX_ARG_EVIDENCE + 1))

        val record = procedureRow("open_calendar")
        assertFalse(record.promotion.requirements.boundedArguments)
        assertTrue(record.boundedArguments.contains("exceed the"))
        assertEquals(PromotionStatus.NONE, record.promotion.status)
    }

    @Test
    fun `three verified successes reach SUGGESTED but never APPROVED`() = runBlocking {
        val r = recorder()
        repeat(3) { recordSuccess(r) }

        val record = procedureRow("open_calendar")
        assertTrue(record.promotion.requirements.repeatableSuccess)
        assertEquals(PromotionStatus.SUGGESTED, record.promotion.status)
        assertFalse(record.promotion.requirements.explicitApproval)
        // The automatic path can never approve — the promotion gate proof.
        assertTrue(record.promotable().let { it is com.unoone.agent.core.model.Result.Success && !it.data })
    }

    @Test
    fun `a failure resets the streak and records itself`() = runBlocking {
        val r = recorder()
        repeat(3) { recordSuccess(r) }
        r.recordProcedureOutcome(
            command = "open my calendar please",
            tool = "open_calendar",
            argumentsJson = """{"app":"com.google.android.calendar"}""",
            success = false,
            verified = false,
            verificationEvidence = "",
            failureReason = "calendar not installed",
            riskLevel = RiskLevel.DIRECT,
        )

        val record = procedureRow("open_calendar")
        assertEquals(0, db.memoryDao().getByKey("procedure_streak:open_calendar:open calendar")!!.value.toInt())
        assertEquals(PromotionStatus.NONE, record.promotion.status)
        assertFalse(record.promotion.requirements.repeatableSuccess)
        assertEquals("calendar not installed", record.failureReason)
    }

    @Test
    fun `unverified success never counts as verified postconditions`() = runBlocking {
        val r = recorder()
        repeat(4) { recordSuccess(r, verified = false) }

        val record = procedureRow("open_calendar")
        assertFalse(record.promotion.requirements.verifiedPostconditions)
        assertFalse(record.verification.verified)
        assertEquals(PromotionStatus.NONE, record.promotion.status)
    }

    @Test
    fun `BLOCK tier stays honest - never suggested`() = runBlocking {
        val r = recorder()
        repeat(4) { recordSuccess(r, tool = "pay_money", riskLevel = RiskLevel.BLOCK) }

        val record = procedureRow("pay_money")
        assertEquals("BLOCK", record.riskClass)
        assertEquals(PromotionStatus.NONE, record.promotion.status)
        assertTrue(record.promotable() is com.unoone.agent.core.model.Result.Error)
    }

    // ---- hypothesis / approval epistemics --------------------------------

    @Test
    fun `a suggestion is a device-local hypothesis that never mirrors`() = runBlocking {
        val r = recorder()
        r.recordSuggestionHypothesis(suggestionSkill("Suggested · Open Calendar"), "open_calendar", 3)

        val row = db.memoryDao().getByKey("envobs_hypo:open_calendar:Suggested · Open Calendar")
        assertNotNull(row)
        assertEquals("envobs_hypo", row!!.type)
        val obs = ContractJson.decodeFromString(EnvObservation.serializer(), row.value)
        assertEquals(EpistemicStatus.HYPOTHESIS, obs.epistemicStatus)
        assertFalse(obs.mayAuthorizeDeviceControl())
        // Hypotheses are excluded from the vault-mirroring surface entirely.
        assertTrue(db.memoryDao().notSynced().none { it.type == "envobs_hypo" })
        assertTrue(mirroredFacts.isEmpty())
    }

    @Test
    fun `user enable writes a verified fact, mirrors it, and promotes gated procedures`() = runBlocking {
        val r = recorder()
        // The learning path: 3 verified successes → suggestion (hypothesis)
        repeat(3) { recordSuccess(r) }
        val skill = suggestionSkill("Suggested · Open Calendar")
        r.recordSuggestionHypothesis(skill, "open_calendar", 3)

        r.approveSkill(skill)

        // The verified fact exists and was handed to the vault mirror.
        val factRow = db.memoryDao().getByKey("envfact:skill:Suggested · Open Calendar")
        assertNotNull(factRow)
        assertEquals("envobs", factRow!!.type)
        assertEquals(1, mirroredFacts.size)
        val fact = ContractJson.decodeFromString(EnvObservation.serializer(), factRow.value)
        assertEquals(EpistemicStatus.VERIFIED_FACT, fact.epistemicStatus)
        assertTrue(fact.mayAuthorizeDeviceControl())
        assertNotNull(fact.verificationRef)

        // The procedure row for the underlying tool moved to APPROVED — the
        // only code path in the product that can set it.
        val record = procedureRow("open_calendar")
        assertEquals(PromotionStatus.APPROVED, record.promotion.status)
        assertTrue(record.promotion.requirements.explicitApproval)
    }

    @Test
    fun `approval without the other gates stays honestly unpromoted`() = runBlocking {
        val r = recorder()
        // Only ONE verified success — repeatable_success is false.
        recordSuccess(r)
        val skill = suggestionSkill("Suggested · Open Calendar")
        r.recordSuggestionHypothesis(skill, "open_calendar", 1)

        r.approveSkill(skill)

        // The fact is recorded (the user DID approve the skill)…
        assertEquals(1, mirroredFacts.size)
        // …but the procedure with unmet gates stays unpromoted.
        val record = procedureRow("open_calendar")
        assertEquals(PromotionStatus.NONE, record.promotion.status)
        assertTrue(record.promotion.requirements.explicitApproval)
    }

    @Test
    fun `user disable is a correction that demotes approved procedures`() = runBlocking {
        val r = recorder()
        repeat(3) { recordSuccess(r) }
        val skill = suggestionSkill("Suggested · Open Calendar")
        r.recordSuggestionHypothesis(skill, "open_calendar", 3)
        r.approveSkill(skill)
        assertEquals(PromotionStatus.APPROVED, procedureRow("open_calendar").promotion.status)

        r.disapproveSkill(skill)

        val factRow = db.memoryDao().getByKey("envfact:skill:Suggested · Open Calendar")!!
        val fact = ContractJson.decodeFromString(EnvObservation.serializer(), factRow.value)
        assertEquals(EpistemicStatus.CORRECTION, fact.epistemicStatus)
        assertFalse(fact.mayAuthorizeDeviceControl())
        val record = procedureRow("open_calendar")
        assertEquals(PromotionStatus.REJECTED, record.promotion.status)
    }

    @Test
    fun `planner context surfaces confirmed capabilities but never hypotheses`() = runBlocking {
        val r = recorder()
        repeat(3) { recordSuccess(r) }
        val skill = suggestionSkill("Suggested · Open Calendar")
        r.recordSuggestionHypothesis(skill, "open_calendar", 3)
        r.approveSkill(skill)

        val context = com.unoone.agent.memory.MemoryModule(db.memoryDao()).getRelevantContext("calendar")
        assertTrue(context.contains("confirmed capability:"))
        assertFalse(context.contains("hypothesis"))
    }
}