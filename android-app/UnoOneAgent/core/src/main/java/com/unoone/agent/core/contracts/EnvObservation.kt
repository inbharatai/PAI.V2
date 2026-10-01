package com.unoone.agent.core.contracts

import com.unoone.agent.core.model.Result
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/**
 * Kotlin mirror of the shared capability contracts
 * (`packages/capability-contracts/capability.v1.json` in the PAI.V2 repo —
 * the Rust types are the contract's source of truth). The environment
 * observation carries the epistemic split the mission demands: every record
 * states whether it is an observation, a hypothesis, a correction or a
 * verified fact, and ONLY a verified fact may ever authorize device control.
 *
 * Pure JVM: no Android imports, so the semantics are fully unit-testable and
 * the fixtures round-trip byte-for-byte against the same JSON the Rust side
 * pins (see [com.unoone.agent.core.contracts.CapabilityContractsTest]).
 */

/** Shared decoder/encoder matching serde's `skip_serializing_if = "Option::is_none"`. */
val ContractJson: Json = Json {
    encodeDefaults = false
    ignoreUnknownKeys = true
}

/** Schema ids — identical to `capability.v1.json`. */
object ContractSchemas {
    const val ENV_OBSERVATION = "inbharat.pai.envobs.v1"
    const val PROCEDURE = "inbharat.pai.procedure.v1"
}

@Serializable
data class Provenance(
    val platform: String,
    @SerialName("device_id") val deviceId: String,
    val source: String,
    val model: String? = null,
    @SerialName("artifact_sha256") val artifactSha256: String? = null,
)

@Serializable
enum class EpistemicStatus {
    @SerialName("observation") OBSERVATION,
    /** A hypothesis is never trusted for device control. */
    @SerialName("hypothesis") HYPOTHESIS,
    @SerialName("correction") CORRECTION,
    @SerialName("verified_fact") VERIFIED_FACT;

    /** The JSON wire name — identical to the Rust serde variant. */
    val serialName: String
        get() = when (this) {
            OBSERVATION -> "observation"
            HYPOTHESIS -> "hypothesis"
            CORRECTION -> "correction"
            VERIFIED_FACT -> "verified_fact"
        }
}

@Serializable
enum class EnvScope {
    @SerialName("device") DEVICE,
    @SerialName("user") USER,
    @SerialName("project") PROJECT,
}

@Serializable
enum class Confidence {
    @SerialName("low") LOW,
    @SerialName("medium") MEDIUM,
    @SerialName("high") HIGH,
}

@Serializable
data class EnvObservation(
    val schema: String,
    val subject: String,
    @SerialName("observed_capability") val observedCapability: String,
    val evidence: String,
    val confidence: Confidence,
    val scope: EnvScope,
    @SerialName("epistemic_status") val epistemicStatus: EpistemicStatus,
    val provenance: Provenance,
    @SerialName("timestamp_ms") val timestampMs: Long,
    @SerialName("verification_ref") val verificationRef: String? = null,
) {
    /**
     * The epistemic honesty gate: a verified fact must carry the verification
     * reference that proves it, and evidence can never be appearance-only.
     */
    fun validate(): Result<Unit> {
        if (schema != ContractSchemas.ENV_OBSERVATION) {
            return Result.Error("environment schema must be ${ContractSchemas.ENV_OBSERVATION}")
        }
        if (epistemicStatus == EpistemicStatus.VERIFIED_FACT && verificationRef.isNullOrBlank()) {
            return Result.Error("verified_fact requires a verification_ref")
        }
        if (evidence.isBlank()) {
            return Result.Error("evidence must be stated — appearance-only inference is not allowed")
        }
        return Result.Success(Unit)
    }

    /** A hypothesis can never become device-control authority. */
    fun mayAuthorizeDeviceControl(): Boolean = epistemicStatus == EpistemicStatus.VERIFIED_FACT
}