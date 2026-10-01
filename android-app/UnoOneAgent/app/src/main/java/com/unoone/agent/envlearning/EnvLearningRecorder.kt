package com.unoone.agent.envlearning

import com.unoone.agent.core.contracts.ContractJson
import com.unoone.agent.core.contracts.ContractSchemas
import com.unoone.agent.core.contracts.EnvObservation
import com.unoone.agent.core.contracts.EpistemicStatus
import com.unoone.agent.core.contracts.Confidence
import com.unoone.agent.core.contracts.EnvScope
import com.unoone.agent.core.contracts.ProcedureOutcome
import com.unoone.agent.core.contracts.ProcedureResult
import com.unoone.agent.core.contracts.Promotion
import com.unoone.agent.core.contracts.PromotionRequirements
import com.unoone.agent.core.contracts.PromotionStatus
import com.unoone.agent.core.contracts.Provenance
import com.unoone.agent.core.contracts.Verification
import com.unoone.agent.core.memory.OutcomeMemoryPolicy
import com.unoone.agent.core.model.RiskLevel
import com.unoone.agent.core.model.Result
import com.unoone.agent.core.util.Logger
import com.unoone.agent.storage.dao.MemoryDao
import com.unoone.agent.storage.entity.MemoryEntity
import com.unoone.agent.storage.entity.SkillEntity
import kotlinx.serialization.encodeToString
import kotlinx.serialization.serializer

/**
 * P1-C — the bounded, MK-style environment-learning producer. It converts the
 * existing, real learning signals of the app into the shared capability
 * contract records (`capability.v1.json`) with the epistemic honesty the
 * mission demands:
 *
 * - Every tool execution produces a [ProcedureOutcome] row whose promotion
 *   requirements are evaluated HONESTLY: bounded arguments (signature-only,
 *   never raw content), repeatable success (a consecutive-success streak that
 *   a failure resets), verified postconditions ONLY from the real
 *   ActionVerifier verdict, low-risk class from the safety pipeline's risk
 *   level, and no contradictory evidence in the current streak. The automatic
 *   path can produce at most SUGGESTED — explicit approval starts false and
 *   only a human's skill-enable action can set it.
 * - A learned skill suggestion produces an [EnvObservation] HYPOTHESIS row —
 *   device-local, never mirrored, never executed (the suggestion itself is a
 *   disabled skill that only the user can enable).
 * - When the user explicitly enables a skill, that human approval becomes a
 *   VERIFIED_FACT observation (vault-mirrored so every host learns the
 *   confirmed capability) AND closes the promotion gate: the tool's procedure
 *   records move to APPROVED only if every other gate holds.
 * - Disabling a skill is an honest CORRECTION — the fact row is rewritten as
 *   a correction and any APPROVED procedure rows for that tool are demoted
 *   to REJECTED.
 *
 * Procedure telemetry and hypotheses are device-local (Room only, excluded
 * from vault mirroring); verified facts and corrections are user-facing and
 * mirror to the canonical vault. All writes are non-fatal: learning must
 * never break a command.
 */
class EnvLearningRecorder(
    private val memoryDao: MemoryDao,
    private val deviceIdProvider: () -> String,
    /** Invoked with the Room row id after a verified fact/correction row is written, so the app layer can mirror it to the vault. */
    private val onEnvFactRecorded: suspend (Long) -> Unit = {},
    private val clock: () -> Long = System::currentTimeMillis,
) {

    companion object {
        /** Same threshold as the skill-suggestion policy — one honest definition of "repeatable". */
        const val REPEATABLE_STREAK = com.unoone.agent.skills.SkillLearningPolicy.SUGGESTION_THRESHOLD
        private const val POLICY_VERSION = "skill-learning-policy-v1"
        private const val PROCEDURE_TYPE = "procedure_outcome"
        private const val HYPOTHESIS_TYPE = "envobs_hypo"
        private const val FACT_TYPE = "envobs"
    }

    // ---- procedure records ------------------------------------------------

    /**
     * Records the outcome of one executed tool call as a [ProcedureOutcome]
     * contract row. Upserted per (tool, command-signature) — the latest
     * procedure record per key is kept, with a consecutive-success streak
     * counter (reset by any failure) deciding [PromotionRequirements.repeatableSuccess].
     *
     * [verified] and [verificationEvidence] come from the REAL ActionVerifier
     * verdict for this execution — never assumed true because the executor
     * said so.
     */
    suspend fun recordProcedureOutcome(
        command: String,
        tool: String,
        success: Boolean,
        verified: Boolean,
        verificationEvidence: String,
        failureReason: String?,
        riskLevel: RiskLevel,
    ) {
        try {
            val signature = OutcomeMemoryPolicy.signature(command)
            if (signature.isBlank()) return
            val now = clock()
            val streakKey = "procedure_streak:$tool:$signature"
            val streak = if (success) {
                (memoryDao.getByKey(streakKey)?.value?.toIntOrNull() ?: 0) + 1
            } else 0
            upsertRow(streakKey, streak.toString(), PROCEDURE_TYPE, now)

            val requirements = PromotionRequirements(
                boundedArguments = true, // signature + tool id only — no raw content
                repeatableSuccess = success && streak >= REPEATABLE_STREAK,
                verifiedPostconditions = success && verified,
                lowRiskClass = riskLevel == RiskLevel.DIRECT,
                noContradictoryEvidence = success, // a failure resets the streak and records itself as FAILURE
                explicitApproval = false,
            )
            val status = if (requirements.let {
                    it.boundedArguments && it.repeatableSuccess && it.verifiedPostconditions &&
                        it.lowRiskClass && it.noContradictoryEvidence
                }) PromotionStatus.SUGGESTED else PromotionStatus.NONE

            val record = ProcedureOutcome(
                schema = ContractSchemas.PROCEDURE,
                procedureId = tool,
                boundedArguments = "signature: $signature",
                preconditions = "agent enabled; safety pipeline cleared at risk ${riskLevel.name}",
                postconditions = "tool $tool completed its action on the device",
                result = if (success) ProcedureResult.SUCCESS else ProcedureResult.FAILURE,
                failureReason = failureReason?.take(200),
                verification = Verification(
                    verified = verified,
                    evidence = verificationEvidence.take(200).ifBlank {
                        if (verified) "action verifier reported verified success" else "unverified"
                    },
                ),
                riskClass = riskLevel.name,
                promotion = Promotion(status, POLICY_VERSION, requirements),
                timestampMs = now,
                provenance = Provenance(platform = "android", deviceId = deviceIdProvider(), source = "agent-orchestrator"),
            )
            if (record.validate() !is Result.Success) return // never store a record the contract rejects
            upsertRow("procedure:$tool:$signature", ContractJson.encodeToString(record), PROCEDURE_TYPE, now)
        } catch (e: Exception) {
            Logger.w("EnvLearning: procedure record non-fatal: ${e.message}")
        }
    }

    // ---- hypothesis / fact observations -----------------------------------

    /**
     * A learned skill suggestion was created after [successCount] successful
     * uses. Recorded as a HYPOTHESIS — device-local only, never mirrored, and
     * the suggestion itself stays disabled until the user explicitly approves
     * it. A hypothesis is never control authority ([EnvObservation.mayAuthorizeDeviceControl]).
     */
    suspend fun recordSuggestionHypothesis(skill: SkillEntity, tool: String, successCount: Int) {
        try {
            val now = clock()
            val record = EnvObservation(
                schema = ContractSchemas.ENV_OBSERVATION,
                subject = "skill:${skill.name}",
                observedCapability = "execute skill '${skill.name}' (steps: ${skill.stepsJson.take(120)})",
                evidence = "$successCount successful uses of $tool recorded on this device; disabled suggestion created for user review",
                confidence = Confidence.MEDIUM,
                scope = EnvScope.USER,
                epistemicStatus = EpistemicStatus.HYPOTHESIS,
                provenance = Provenance(platform = "android", deviceId = deviceIdProvider(), source = "skill-learning-policy"),
                timestampMs = now,
            )
            if (record.validate() !is Result.Success) return
            upsertRow("envobs_hypo:${tool}:${skill.name}", ContractJson.encodeToString(record), HYPOTHESIS_TYPE, now)
        } catch (e: Exception) {
            Logger.w("EnvLearning: hypothesis record non-fatal: ${e.message}")
        }
    }

    /**
     * The user explicitly enabled a skill — the human approval the promotion
     * gate requires. Records a VERIFIED_FACT (vault-mirrored so every host
     * knows the user confirmed this capability) and closes the promotion gate:
     * the tool's procedure records move to APPROVED only when every other
     * requirement holds; a BLOCK-tier or unverified procedure stays honestly
     * unpromoted.
     */
    suspend fun approveSkill(skill: SkillEntity) {
        try {
            val now = clock()
            val record = EnvObservation(
                schema = ContractSchemas.ENV_OBSERVATION,
                subject = "skill:${skill.name}",
                observedCapability = "execute skill '${skill.name}'",
                evidence = "user explicitly enabled skill '${skill.name}' in the Skills screen",
                confidence = Confidence.HIGH,
                scope = EnvScope.USER,
                epistemicStatus = EpistemicStatus.VERIFIED_FACT,
                provenance = Provenance(platform = "android", deviceId = deviceIdProvider(), source = "user-approval"),
                timestampMs = now,
                verificationRef = "user_enabled_skill:${skill.name}@$now",
            )
            if (record.validate() !is Result.Success) return
            val rowId = upsertRow("envfact:skill:${skill.name}", ContractJson.encodeToString(record), FACT_TYPE, now)
            onEnvFactRecorded(rowId)
            promoteProceduresFor(skill, now)
        } catch (e: Exception) {
            Logger.w("EnvLearning: approval record non-fatal: ${e.message}")
        }
    }

    /**
     * The user explicitly disabled a skill — an honest epistemic CORRECTION of
     * the previous fact. The correction mirrors to the vault, and any APPROVED
     * procedure records for the suggestion's tool are demoted to REJECTED.
     */
    suspend fun disapproveSkill(skill: SkillEntity) {
        try {
            val now = clock()
            val record = EnvObservation(
                schema = ContractSchemas.ENV_OBSERVATION,
                subject = "skill:${skill.name}",
                observedCapability = "execute skill '${skill.name}'",
                evidence = "user explicitly disabled skill '${skill.name}' in the Skills screen",
                confidence = Confidence.HIGH,
                scope = EnvScope.USER,
                epistemicStatus = EpistemicStatus.CORRECTION,
                provenance = Provenance(platform = "android", deviceId = deviceIdProvider(), source = "user-approval"),
                timestampMs = now,
                verificationRef = "user_disabled_skill:${skill.name}@$now",
            )
            if (record.validate() !is Result.Success) return
            val rowId = upsertRow("envfact:skill:${skill.name}", ContractJson.encodeToString(record), FACT_TYPE, now)
            onEnvFactRecorded(rowId)
            demoteProceduresFor(skill, now)
        } catch (e: Exception) {
            Logger.w("EnvLearning: correction record non-fatal: ${e.message}")
        }
    }

    // ---- promotion bookkeeping -------------------------------------------

    /** The tool whose successful uses created this skill's suggestion, from the hypothesis row it wrote. */
    private suspend fun hypothesisToolFor(skill: SkillEntity): String? {
        val rows = memoryDao.getByTypeList(HYPOTHESIS_TYPE)
        val row = rows.firstOrNull { it.key == "envobs_hypo:${skill.name}" || it.key.endsWith(":${skill.name}") }
            ?: return null
        return row.key.removePrefix("envobs_hypo:").removeSuffix(":${skill.name}").ifBlank { null }
    }

    private suspend fun promoteProceduresFor(skill: SkillEntity, now: Long) {
        val tool = hypothesisToolFor(skill) ?: return
        for (row in memoryDao.getByTypeList(PROCEDURE_TYPE)) {
            if (!row.key.startsWith("procedure:$tool:")) continue
            val record = decodeProcedure(row.value) ?: continue
            val requirements = record.promotion.requirements.copy(explicitApproval = true)
            val candidate = record.copy(
                promotion = record.promotion.copy(requirements = requirements),
                timestampMs = now,
            )
            val promoted = when (val v = candidate.promotable()) {
                is Result.Error -> { Logger.i("EnvLearning: ${v.message} — stays unpromoted"); false }
                is Result.Success -> v.data
            }
            val status = if (promoted) PromotionStatus.APPROVED else PromotionStatus.NONE
            val updated = candidate.copy(promotion = candidate.promotion.copy(status = status))
            if (updated.validate() !is Result.Success) continue
            memoryDao.update(row.copy(
                value = ContractJson.encodeToString(updated),
                updatedAt = now,
            ))
            if (!promoted) {
                Logger.i("EnvLearning: approval noted for $tool but promotion gates not met — stays unpromoted")
            }
        }
    }

    private suspend fun demoteProceduresFor(skill: SkillEntity, now: Long) {
        val tool = hypothesisToolFor(skill) ?: return
        for (row in memoryDao.getByTypeList(PROCEDURE_TYPE)) {
            if (!row.key.startsWith("procedure:$tool:")) continue
            val record = decodeProcedure(row.value) ?: continue
            if (record.promotion.status != PromotionStatus.APPROVED) continue
            val demoted = record.copy(
                promotion = record.promotion.copy(status = PromotionStatus.REJECTED),
                timestampMs = now,
            )
            if (demoted.validate() !is Result.Success) continue
            memoryDao.update(row.copy(value = ContractJson.encodeToString(demoted), updatedAt = now))
        }
    }

    private fun decodeProcedure(json: String): ProcedureOutcome? =
        try {
            ContractJson.decodeFromString(ProcedureOutcome.serializer(), json)
        } catch (_: Exception) {
            null
        }

    private suspend fun upsertRow(key: String, value: String, type: String, now: Long): Long {
        val existing = memoryDao.getByKey(key)
        return if (existing != null) {
            memoryDao.update(existing.copy(value = value, updatedAt = now))
            existing.id
        } else {
            memoryDao.insert(MemoryEntity(key = key, value = value, type = type, createdAt = now, updatedAt = now))
        }
    }
}