package com.unoone.agent.core.contracts

import com.unoone.agent.core.model.Result
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Cross-language contract parity: these fixtures are copied BYTE-FOR-BYTE from
 * `packages/capability-contracts/capability.v1.json` in the PAI.V2 repo, the
 * same examples the Rust crate's tests round-trip. If either side drifts, one
 * of the two suites fails. The Kotlin JSON is configured
 * `encodeDefaults = false` to match serde's `skip_serializing_if =
 * "Option::is_none"`, so a re-encode is byte-identical for null-optional
 * fields.
 */
class CapabilityContractsTest {

    private val json: Json = ContractJson

    private val envObsFixture = """
        {
          "schema": "inbharat.pai.envobs.v1",
          "subject": "vault-drive-<fingerprint>",
          "observed_capability": "usb-vault-attach",
          "evidence": "Windows removable-drive enumeration reports a validated UNOONE package; manifest PackageIdentity check passed",
          "confidence": "high",
          "scope": "device",
          "epistemic_status": "verified_fact",
          "provenance": { "platform": "desktop", "device_id": "unoone-power", "source": "usb-manifest-validator" },
          "timestamp_ms": 1760000000000,
          "verification_ref": "usb-validate-package:PackageIdentity"
        }
    """.trimIndent()

    private val procedureFixture = """
        {
          "schema": "inbharat.pai.procedure.v1",
          "procedure_id": "open_calendar",
          "bounded_arguments": "none",
          "preconditions": "calendar app installed",
          "postconditions": "calendar app is the foreground activity",
          "result": "success",
          "failure_reason": null,
          "verification": { "verified": true, "evidence": "ActionVerifier: foreground package == default calendar provider" },
          "risk_class": "DIRECT",
          "promotion": {
            "status": "suggested",
            "policy_version": "skill-learning-policy-v1",
            "requirements": {
              "bounded_arguments": true,
              "repeatable_success": true,
              "verified_postconditions": true,
              "low_risk_class": true,
              "no_contradictory_evidence": true,
              "explicit_approval": false
            }
          },
          "timestamp_ms": 1760000000000,
          "provenance": { "platform": "android", "device_id": "vault-device-uuid", "source": "agent-orchestrator" }
        }
    """.trimIndent()

    // ---- EnvironmentObservation ---------------------------------------------------

    @Test
    fun `envobs fixture round-trips and validates`() {
        val obs = json.decodeFromString(EnvObservation.serializer(), envObsFixture)
        assertTrue((obs.validate() as Result.Success<Unit>).data == Unit)
        assertTrue("verified_fact may authorize", obs.mayAuthorizeDeviceControl())
        assertEquals(EpistemicStatus.VERIFIED_FACT, obs.epistemicStatus)
        assertEquals(Confidence.HIGH, obs.confidence)
        assertEquals(EnvScope.DEVICE, obs.scope)
    }

    @Test
    fun `a hypothesis never authorizes device control`() {
        val obs = json.decodeFromString(EnvObservation.serializer(), envObsFixture)
        assertFalse(obs.copy(epistemicStatus = EpistemicStatus.HYPOTHESIS).mayAuthorizeDeviceControl())
        assertFalse(obs.copy(epistemicStatus = EpistemicStatus.OBSERVATION).mayAuthorizeDeviceControl())
    }

    @Test
    fun `verified_fact without a verification reference is rejected`() {
        val obs = json.decodeFromString(EnvObservation.serializer(), envObsFixture)
        val noRef = obs.copy(verificationRef = "  ")
        assertTrue(noRef.validate() is Result.Error)
        val wrongSchema = obs.copy(schema = "something.else.v1")
        assertTrue(wrongSchema.validate() is Result.Error)
        val appearanceOnly = obs.copy(evidence = "   ")
        assertTrue(appearanceOnly.validate() is Result.Error)
    }

    @Test
    fun `envobs re-encode keeps the shared field names`() {
        val obs = json.decodeFromString(EnvObservation.serializer(), envObsFixture)
        val encoded = json.encodeToString(EnvObservation.serializer(), obs)
        assertTrue(encoded.contains("\"epistemic_status\":\"verified_fact\""))
        assertTrue(encoded.contains("\"observed_capability\""))
        assertTrue(encoded.contains("\"timestamp_ms\""))
        // round-trip stability
        assertEquals(obs, json.decodeFromString(EnvObservation.serializer(), encoded))
    }

    // ---- ProcedureOutcome ---------------------------------------------------------

    @Test
    fun `procedure fixture round-trips and stays unpromoted without approval`() {
        val outcome = json.decodeFromString(ProcedureOutcome.serializer(), procedureFixture)
        assertTrue(outcome.validate() is Result.Success)
        // Suggested without explicit approval: not promotable until the user approves.
        assertTrue((outcome.promotable() as Result.Success).data == false)
        assertEquals(PromotionStatus.SUGGESTED, outcome.promotion.status)
    }

    @Test
    fun `approval alone is not enough - all six gates must hold`() {
        val base = json.decodeFromString(ProcedureOutcome.serializer(), procedureFixture)
        val approved = base.copy(
            promotion = base.promotion.copy(
                status = PromotionStatus.APPROVED,
                requirements = base.promotion.requirements.copy(explicitApproval = true),
            )
        )
        assertTrue((approved.promotable() as Result.Success).data)
        assertTrue(approved.validate() is Result.Success)

        // One missing requirement each → never promotable.
        val r = approved.promotion.requirements
        for (missing in listOf(
            r.copy(boundedArguments = false),
            r.copy(repeatableSuccess = false),
            r.copy(verifiedPostconditions = false),
            r.copy(lowRiskClass = false),
            r.copy(noContradictoryEvidence = false),
        )) {
            val broken = approved.copy(promotion = approved.promotion.copy(requirements = missing))
            assertTrue((broken.promotable() as Result.Success).data == false)
            assertTrue("APPROVED with a failed gate must not validate", broken.validate() is Result.Error)
        }
    }

    @Test
    fun `BLOCK-tier procedures are never promotable and unverified success does not promote`() {
        val base = json.decodeFromString(ProcedureOutcome.serializer(), procedureFixture)
        val approved = base.copy(
            riskClass = "BLOCK",
            promotion = base.promotion.copy(
                status = PromotionStatus.APPROVED,
                requirements = base.promotion.requirements.copy(explicitApproval = true),
            )
        )
        assertTrue(approved.promotable() is Result.Error)

        val unverified = base.copy(
            verification = Verification(verified = false, evidence = "executor said so, nothing checked"),
            promotion = base.promotion.copy(
                status = PromotionStatus.APPROVED,
                requirements = base.promotion.requirements.copy(explicitApproval = true),
            )
        )
        assertTrue((unverified.promotable() as Result.Success).data == false)
        assertTrue(unverified.validate() is Result.Error)
    }

    @Test
    fun `suggested must not carry explicit approval`() {
        val base = json.decodeFromString(ProcedureOutcome.serializer(), procedureFixture)
        val corrupt = base.copy(
            promotion = base.promotion.copy(
                requirements = base.promotion.requirements.copy(explicitApproval = true)
            )
        )
        assertTrue(corrupt.validate() is Result.Error)
    }

    @Test
    fun `BLOCK with status NONE validates - honest telemetry, parity with the Rust short-circuit`() {
        // Rust: `matches!(Approved) && !self.promotable()?` never calls
        // promotable() for a NONE record, so a BLOCK-tier attempt with status
        // NONE is VALID — it records that a blocked-tier action was attempted
        // and can never promote. The Kotlin mirror must short-circuit the
        // same way (live-caught divergence: the eager mirror rejected every
        // BLOCK record, silently dropping the telemetry).
        val base = json.decodeFromString(ProcedureOutcome.serializer(), procedureFixture)
        val blocked = base.copy(riskClass = "BLOCK")
        assertTrue("NONE + BLOCK must validate as honest unpromoted telemetry", blocked.validate() is Result.Success)
        assertTrue(blocked.promotable() is Result.Error)

        // APPROVED + BLOCK is still invalid — the Err propagates.
        val approvedBlocked = base.copy(
            riskClass = "BLOCK",
            promotion = base.promotion.copy(
                status = PromotionStatus.APPROVED,
                requirements = base.promotion.requirements.copy(explicitApproval = true),
            )
        )
        assertTrue(approvedBlocked.validate() is Result.Error)
    }

    @Test
    fun `procedure re-encode keeps the shared field names`() {
        val outcome = json.decodeFromString(ProcedureOutcome.serializer(), procedureFixture)
        val encoded = json.encodeToString(ProcedureOutcome.serializer(), outcome)
        assertTrue(encoded.contains("\"procedure_id\""))
        assertTrue(encoded.contains("\"failure_reason\":null").not()) // skip-when-null parity with serde
        assertTrue(encoded.contains("\"risk_class\":\"DIRECT\""))
        assertEquals(outcome, json.decodeFromString(ProcedureOutcome.serializer(), encoded))
    }
}