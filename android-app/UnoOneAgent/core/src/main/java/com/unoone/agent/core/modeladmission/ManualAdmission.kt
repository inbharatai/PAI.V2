package com.unoone.agent.core.modeladmission

/** Explicit manual risk lane. This does not create a qualification or auto-download grant. */
object ManualAdmission {
    enum class Provenance { DETECTED, ESTIMATED, TESTED, UNKNOWN }
    data class Observation<T>(val value: T?, val provenance: Provenance) {
        fun measured(): T? = if (provenance == Provenance.DETECTED || provenance == Provenance.TESTED) value else null
        companion object {
            fun <T> detected(value: T) = Observation(value, Provenance.DETECTED)
            fun <T> unknown() = Observation<T>(null, Provenance.UNKNOWN)
        }
    }
    data class Probe(val capturedAtMs: Long, val totalRam: Observation<Long>, val availableRam: Observation<Long>,
        val lowMemoryThreshold: Observation<Long>, val lowMemory: Observation<Boolean>,
        val heapAvailable: Observation<Long>, val storage: Observation<Long>, val api: Observation<Int>,
        val abi: Observation<String>, val cpuFeatures: Observation<Set<String>>, val libraries: Observation<Set<String>>,
        val nativeProcessLimit: Observation<Long> = Observation.unknown())
    data class Profile(val identity: String, val modelId: String, val contextTokens: Int,
        val downloadBytes: Long, val installedBytes: Long?, val minimumTotalRam: Long,
        val workingSetEstimate: Long?, val heapReserve: Long, val requiredLibraries: Set<String>,
        val abi: String = "arm64-v8a", val minimumApi: Int = 28,
        val availableReserve: Long = 512L * 1024 * 1024, val diskHeadroom: Long = 512L * 1024 * 1024,
        /** Rust-core `os_reserve_bytes` for the TOTAL-RAM budget; null keeps the legacy `max(reserve, threshold)`. */
        val osReserve: Long? = null)
    data class RiskPolicy(val identity: String, val contextTokens: Int, val approvedAtMs: Long,
        val expiresAtMs: Long, val maxDownloadBytes: Long, val maxStoreBytes: Long,
        val allowMetered: Boolean, val acknowledgeUnqualified: Boolean)
    data class Decision(val allowed: Boolean, val reason: String, val peakBytes: Long? = null,
        val requiredAvailable: Long? = null, val storageReservation: Long? = null)

    fun assess(profile: Profile, probe: Probe, policy: RiskPolicy?, now: Long, enabled: Boolean,
        connected: Boolean, metered: Boolean?, storeBytes: Long?, loading: Boolean = false): Decision {
        fun deny(reason: String) = Decision(false, reason)
        if (!enabled) return deny("DISABLED")
        if (policy == null || !policy.acknowledgeUnqualified) return deny("LOCAL_RISK_POLICY_REQUIRED")
        if (policy.identity != profile.identity || policy.contextTokens != profile.contextTokens) return deny("POLICY_PROFILE_CHANGED")
        if (policy.approvedAtMs > now || now >= policy.expiresAtMs) return deny("POLICY_EXPIRED")
        if (probe.capturedAtMs > now || now - probe.capturedAtMs > AdmissionMirror.MAX_PROBE_AGE_MS) return deny("STALE_PROBE")
        if (!loading && (!connected || metered == null || metered && !policy.allowMetered)) return deny("NETWORK_POLICY")
        val total = probe.totalRam.measured() ?: return deny("UNKNOWN_TOTAL_RAM")
        val available = probe.availableRam.measured() ?: return deny("UNKNOWN_AVAILABLE_RAM")
        val threshold = probe.lowMemoryThreshold.measured() ?: return deny("UNKNOWN_LOW_MEMORY_THRESHOLD")
        val low = probe.lowMemory.measured() ?: return deny("UNKNOWN_MEMORY_PRESSURE")
        val heap = probe.heapAvailable.measured() ?: return deny("UNKNOWN_HEAP")
        val storage = probe.storage.measured() ?: return deny("UNKNOWN_STORAGE")
        val api = probe.api.measured() ?: return deny("UNKNOWN_API")
        val abi = probe.abi.measured() ?: return deny("UNKNOWN_ABI")
        val features = probe.cpuFeatures.measured() ?: return deny("UNKNOWN_CPU_FEATURES")
        val libraries = probe.libraries.measured() ?: return deny("UNKNOWN_NATIVE_LIBRARIES")
        if (total <= 0 || available < 0 || available > total || threshold < 0 || threshold > total || heap < 0 || storage < 0) return deny("INVALID_PROBE")
        if (api < profile.minimumApi || abi != profile.abi || (abi == "arm64-v8a" && "asimd" !in features) || !libraries.containsAll(profile.requiredLibraries)) return deny("RUNTIME_MISMATCH")
        val peak = profile.workingSetEstimate ?: return deny("UNKNOWN_MEMORY_PLAN")
        val installed = profile.installedBytes ?: return deny("UNKNOWN_INSTALLED_SIZE")
        val current = storeBytes ?: return deny("UNKNOWN_STORE_SIZE")
        if (peak <= 0 || installed <= 0 || profile.downloadBytes <= 0 || current < 0) return deny("INVALID_PROFILE")
        val needed: Long; val totalNeeded: Long; val disk: Long; val store: Long
        try {
            needed = Math.addExact(peak, maxOf(profile.availableReserve, threshold))
            // Rust core: total_need = peak + os_reserve (TOTAL RAM budget, like Power's `fits`).
            totalNeeded = profile.osReserve?.let { Math.addExact(peak, it) } ?: needed
            disk = Math.addExact(Math.addExact(profile.downloadBytes, installed), profile.diskHeadroom)
            store = Math.addExact(current, disk)
        } catch (_: ArithmeticException) { return deny("ARITHMETIC_OVERFLOW") }
        fun decision(reason: String, allowed: Boolean = false) = Decision(allowed, reason, peak, needed, disk)
        if (total < profile.minimumTotalRam || total < totalNeeded || heap < profile.heapReserve) return decision("PERMANENT_MEMORY_MISFIT")
        if (low || available < needed) return decision("MEMORY_PRESSURE")
        if (storage < disk) return decision("INSUFFICIENT_STORAGE")
        if (!loading && (profile.downloadBytes > policy.maxDownloadBytes || store > policy.maxStoreBytes)) return decision("STORAGE_POLICY_CAP")
        return decision("MANUAL_UNQUALIFIED_RISK_ACCEPTED_NOT_RECOMMENDED", true)
    }
}
