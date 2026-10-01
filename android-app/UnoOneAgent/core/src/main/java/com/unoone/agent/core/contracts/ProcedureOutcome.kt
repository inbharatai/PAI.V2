package com.unoone.agent.core.contracts

import com.unoone.agent.core.model.Result
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * Kotlin mirror of the `ProcedureOutcome` record from
 * `packages/capability-contracts/capability.v1.json` (Rust is the contract's
 * source of truth). The promotion gate is the mission's safeguard: a procedure
 * may only be promoted to APPROVED when ALL six requirements hold — bounded
 * arguments, repeatable success, VERIFIED postconditions, low risk class, no
 * contradictory evidence, and EXPLICIT approval. Nothing may auto-promote:
 * the automatic path can produce at most SUGGESTED, which never executes.
 */
@Serializable
enum class ProcedureResult {
    @SerialName("success") SUCCESS,
    @SerialName("failure") FAILURE,
    @SerialName("partial") PARTIAL,
    @SerialName("cancelled") CANCELLED,
}

@Serializable
data class Verification(
    val verified: Boolean,
    val evidence: String,
)

@Serializable
enum class PromotionStatus {
    @SerialName("none") NONE,
    /** Learning suggests; a human/policy approves. Never auto-executes. */
    @SerialName("suggested") SUGGESTED,
    @SerialName("approved") APPROVED,
    @SerialName("rejected") REJECTED,
}

@Serializable
data class PromotionRequirements(
    @SerialName("bounded_arguments") val boundedArguments: Boolean,
    @SerialName("repeatable_success") val repeatableSuccess: Boolean,
    @SerialName("verified_postconditions") val verifiedPostconditions: Boolean,
    @SerialName("low_risk_class") val lowRiskClass: Boolean,
    @SerialName("no_contradictory_evidence") val noContradictoryEvidence: Boolean,
    @SerialName("explicit_approval") val explicitApproval: Boolean,
)

@Serializable
data class Promotion(
    val status: PromotionStatus,
    @SerialName("policy_version") val policyVersion: String,
    val requirements: PromotionRequirements,
)

@Serializable
data class ProcedureOutcome(
    val schema: String,
    @SerialName("procedure_id") val procedureId: String,
    @SerialName("bounded_arguments") val boundedArguments: String,
    val preconditions: String,
    val postconditions: String,
    val result: ProcedureResult,
    @SerialName("failure_reason") val failureReason: String? = null,
    val verification: Verification,
    @SerialName("risk_class") val riskClass: String,
    val promotion: Promotion,
    @SerialName("timestamp_ms") val timestampMs: Long,
    val provenance: Provenance,
) {
    /**
     * The promotion gate: every requirement must hold, including explicit
     * approval. This is what prevents a model guess or an unverified lucky
     * run from becoming a trusted procedure. BLOCK-tier procedures are never
     * promotable at all.
     */
    fun promotable(): Result<Boolean> {
        if (riskClass == "BLOCK") {
            return Result.Error("BLOCK-tier procedures are never promotable")
        }
        val r = promotion.requirements
        if (!(r.boundedArguments && r.repeatableSuccess && r.verifiedPostconditions &&
                r.lowRiskClass && r.noContradictoryEvidence && r.explicitApproval)
        ) {
            return Result.Success(false)
        }
        // Promotion evidence must itself be verified success.
        if (result != ProcedureResult.SUCCESS || !verification.verified) {
            return Result.Success(false)
        }
        return Result.Success(true)
    }

    fun validate(): Result<Unit> {
        if (schema != ContractSchemas.PROCEDURE) {
            return Result.Error("procedure schema must be ${ContractSchemas.PROCEDURE}")
        }
        // Parity with the Rust contract: promotable() is only consulted for
        // APPROVED records (`matches!(Approved) && !self.promotable()?` —
        // short-circuits otherwise). A BLOCK-tier record with status NONE is
        // VALID honest telemetry: it records that a blocked-tier attempt
        // happened and stays unpromoted forever.
        if (promotion.status == PromotionStatus.APPROVED) {
            when (val v = promotable()) {
                is Result.Error -> return v
                is Result.Success -> if (!v.data) {
                    return Result.Error("approved promotion requires all gates + verified success")
                }
            }
        }
        if (promotion.status == PromotionStatus.SUGGESTED &&
            promotion.requirements.explicitApproval
        ) {
            return Result.Error("suggested must not carry explicit_approval yet")
        }
        return Result.Success(Unit)
    }
}