package com.unoone.agent.core.modeladmission

/**
 * Single decision function for every Android model entry point (download, explicit load, boot
 * auto-load, staged activation). It wraps the manual-risk ESTIMATE lane ([ManualAdmission]) with
 * device-local EVIDENCE ([NativeLoadReceipt]) under one rule:
 *
 *   **evidence beats estimate** — a model that has a recorded successful native load+smoke on THIS
 *   device is allowed as "Works here" even when the working-set estimate is borderline;
 *   **physically impossible is always refused** — artifact bytes larger than total RAM, or a measured
 *   low-memory condition right now, never load no matter what the history says.
 *
 * Evidence state and permission are reported separately so the UI can never conflate
 * "unsigned/unknown" with "refused".
 */
object LoadAdmission {
    enum class Purpose { DOWNLOAD, EXPLICIT_LOAD, BOOT_LOAD, ACTIVATION }

    /** Three honest states. QUALIFIED = signed physical-device record (production set is empty today). */
    enum class EvidenceState { QUALIFIED, WORKS_HERE, UNKNOWN }

    data class Decision(
        val allowed: Boolean,
        val reason: String,
        val evidence: EvidenceState,
        val estimate: ManualAdmission.Decision,
        val purpose: Purpose,
        /** Human-readable provenance of the decision, e.g. "estimate MEMORY_PRESSURE overridden by receipt 2026-…". */
        val note: String = ""
    ) {
        val peakBytes: Long? get() = estimate.peakBytes
        val requiredAvailable: Long? get() = estimate.requiredAvailable
    }

    const val PHYSICALLY_IMPOSSIBLE = "PHYSICALLY_IMPOSSIBLE"
    const val MEMORY_PRESSURE_NOW = "MEMORY_PRESSURE_NOW"
    const val EVIDENCE_BEATS_ESTIMATE = "EVIDENCE_BEATS_ESTIMATE"
    const val LEGACY_INSTALL_GRANDFATHERED = "LEGACY_INSTALL_GRANDFATHERED"
    const val MANUAL_RISK_TRANSACTIONAL_SMOKE = "MANUAL_RISK_TRANSACTIONAL_SMOKE"
    const val FAILED_HERE_PREVIOUSLY = "FAILED_HERE_PREVIOUSLY"

    /** Estimate/policy reasons that recorded evidence may override for a LOAD purpose. */
    val OVERRIDABLE_BY_EVIDENCE: Set<String> = setOf(
        "MEMORY_PRESSURE", "PERMANENT_MEMORY_MISFIT", "LOCAL_RISK_POLICY_REQUIRED", "POLICY_EXPIRED",
        "POLICY_PROFILE_CHANGED", "STALE_PROBE", "NETWORK_POLICY", "INSUFFICIENT_STORAGE", "STORAGE_POLICY_CAP",
        "UNKNOWN_MEMORY_PLAN", "UNKNOWN_INSTALLED_SIZE", "UNKNOWN_STORE_SIZE", "UNKNOWN_TOTAL_RAM",
        "UNKNOWN_AVAILABLE_RAM", "UNKNOWN_LOW_MEMORY_THRESHOLD", "UNKNOWN_MEMORY_PRESSURE", "UNKNOWN_HEAP",
        "UNKNOWN_STORAGE", "UNKNOWN_CPU_FEATURES", "UNKNOWN_NATIVE_LIBRARIES", "UNKNOWN_API", "UNKNOWN_ABI"
    )

    /** Measured facts that no evidence overrides: missing libraries / wrong ABI / corrupt probe or profile. */
    val NEVER_OVERRIDDEN: Set<String> = setOf("RUNTIME_MISMATCH", "INVALID_PROBE", "INVALID_PROFILE", "ARITHMETIC_OVERFLOW", "DISABLED")

    fun evidenceState(evidence: LoadEvidence): EvidenceState = when {
        evidence.signedQualification -> EvidenceState.QUALIFIED
        evidence.latestReceipt?.passed == true -> EvidenceState.WORKS_HERE
        else -> EvidenceState.UNKNOWN
    }

    fun decide(
        profile: ManualAdmission.Profile,
        probe: ManualAdmission.Probe,
        policy: ManualAdmission.RiskPolicy?,
        evidence: LoadEvidence,
        purpose: Purpose,
        now: Long,
        enabled: Boolean,
        connected: Boolean,
        metered: Boolean?,
        storeBytes: Long?
    ): Decision {
        val state = evidenceState(evidence)
        val loading = purpose != Purpose.DOWNLOAD
        val estimate = ManualAdmission.assess(profile, probe, policy, now, enabled, connected, metered, storeBytes, loading)
        fun deny(reason: String, note: String = "") = Decision(false, reason, state, estimate, purpose, note)
        fun allow(reason: String, note: String = "") = Decision(true, reason, state, estimate, purpose, note)

        if (!enabled) return deny("DISABLED")

        // Hard refusals first; they are measured facts, not estimates.
        val total = probe.totalRam.measured()
        val footprint = profile.installedBytes ?: profile.downloadBytes
        if (total != null && total > 0 && footprint > total) {
            return deny(PHYSICALLY_IMPOSSIBLE, "artifact bytes $footprint exceed total RAM $total")
        }
        if (loading && probe.lowMemory.measured() == true) {
            return deny(MEMORY_PRESSURE_NOW, "system reports lowMemory right now; retry later")
        }

        // Download never borrows evidence: network/storage/consent policy applies in full.
        if (!loading) return Decision(estimate.allowed, estimate.reason, state, estimate, purpose)

        if (estimate.allowed) return allow(estimate.reason)
        if (estimate.reason in NEVER_OVERRIDDEN) return deny(estimate.reason, "measured mismatch; evidence cannot override")

        val latest = evidence.latestReceipt
        if (state == EvidenceState.WORKS_HERE || state == EvidenceState.QUALIFIED) {
            if (estimate.reason in OVERRIDABLE_BY_EVIDENCE) {
                return allow("$EVIDENCE_BEATS_ESTIMATE:${estimate.reason}",
                    "estimate ${estimate.reason} overridden by ${if (state == EvidenceState.QUALIFIED) "signed qualification" else "receipt recorded at ${latest?.recordedAtMs} on backend ${latest?.backend}"}")
            }
            return deny(estimate.reason)
        }

        // Pre-receipt legacy installs keep loading (boot and explicit) unless this device already
        // measured a failure for this exact identity; the first successful load mints the receipt.
        if (evidence.legacyInstalled && (purpose == Purpose.BOOT_LOAD || purpose == Purpose.EXPLICIT_LOAD)) {
            if (latest != null && !latest.passed) {
                return deny("$FAILED_HERE_PREVIOUSLY:${latest.reason}", "last native attempt on this device failed (${latest.reason}); explicit retry required")
            }
            if (estimate.reason in OVERRIDABLE_BY_EVIDENCE) {
                return allow(LEGACY_INSTALL_GRANDFATHERED, "installed before receipts existed; estimate ${estimate.reason} not treated as refusal")
            }
        }

        // Staged activation IS the measurement: with a valid manual-risk policy, an available-RAM
        // ESTIMATE shortfall does not block the transactional retained-old load+smoke attempt.
        if (purpose == Purpose.ACTIVATION && estimate.reason == "MEMORY_PRESSURE") {
            return allow(MANUAL_RISK_TRANSACTIONAL_SMOKE, "available-RAM estimate borderline; outcome is measured and the previous active bundle is retained on failure")
        }

        val failedNote = if (latest != null && !latest.passed) "; last native attempt here failed (${latest.reason})" else ""
        return deny(estimate.reason, "no receipt for this device$failedNote")
    }
}
