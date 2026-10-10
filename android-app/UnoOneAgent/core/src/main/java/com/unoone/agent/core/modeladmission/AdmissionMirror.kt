package com.unoone.agent.core.modeladmission

import kotlinx.serialization.json.*
import java.math.BigInteger

/** Decision-only mirror of packages/model-admission/src/admission.rs, schema 1.
 * JSON explanations are NOT permission or evidence. Only native adapters may supply inputs.
 * In particular, no deserialized qualification is accepted by production Android: its trusted
 * qualification set is currently empty. Tests supply the exact synthetic shared vectors.
 */
object AdmissionMirror {
    const val MAX_PROBE_AGE_MS = 60_000L
    data class Decision(val status: String, val reason: String, val eligibleNow: Boolean = false,
        val peakRamBytes: BigInteger? = null, val peakVramBytes: BigInteger? = null,
        val storageReservationBytes: BigInteger? = null, val evidenceIds: List<String> = emptyList(),
        val responsiveness: JsonElement? = null)
    private val max = BigInteger("18446744073709551615")
    private fun JsonObject.s(k: String) = getValue(k).jsonPrimitive.content
    private fun JsonObject.n(k: String) = s(k).toBigInteger()
    private fun JsonObject.a(k: String) = getValue(k).jsonArray
    private fun JsonObject.o(k: String) = getValue(k).jsonObject
    private fun JsonObject.measured(k: String): JsonElement? = o(k).let {
        if (it.s("provenance") in listOf("DETECTED", "TESTED")) it["value"] else null
    }
    private fun JsonObject.mn(k: String) = measured(k)?.jsonPrimitive?.content?.toBigInteger()
    private fun sum(xs: List<BigInteger>): BigInteger = xs.fold(BigInteger.ZERO, BigInteger::add).also {
        require(it >= BigInteger.ZERO && it <= max)
    }

    /** Non-serializable native evidence; no production adapter currently creates this. */
    internal class NativePreflight private constructor(val candidate: JsonObject, val request: JsonObject,
        val probeId: String, val capturedAtMs: Long, val expiresAtMs: Long, val evidenceId: String,
        val responsiveness: JsonObject) {
        companion object {
            fun passed(candidate: JsonObject, request: JsonObject, probeId: String, capturedAtMs: Long,
                expiresAtMs: Long, evidenceId: String, responsiveness: JsonObject): NativePreflight {
                val identifier = Regex("[A-Za-z0-9][A-Za-z0-9._-]{0,159}")
                require(capturedAtMs >= 0 && capturedAtMs < expiresAtMs && identifier.matches(probeId) && identifier.matches(evidenceId))
                require(listOf("cold_load_ms", "first_token_p95_ms", "generated_tokens", "generation_ms").all {
                    responsiveness.getValue(it).jsonPrimitive.content.toBigInteger() > BigInteger.ZERO
                })
                require(identifier.matches(responsiveness.getValue("evidence_id").jsonPrimitive.content))
                require(responsiveness.getValue("thermal").jsonPrimitive.content in listOf("NOMINAL", "WARM"))
                return NativePreflight(candidate, request, probeId, capturedAtMs, expiresAtMs, evidenceId, responsiveness)
            }
        }
    }

    /** Internal because test fixtures must never become a public authority-hydration API. */
    internal fun evaluate(c: JsonObject, p: JsonObject, r: JsonObject,
        qualifications: List<JsonObject>, now: Long, preflight: NativePreflight? = null): Decision {
        var peak: BigInteger? = null; var disk: BigInteger? = null; var vram: BigInteger? = null
        var evidence = emptyList<String>(); var responsiveness: JsonElement? = null
        fun result(status: String, reason: String, eligible: Boolean = false) =
            Decision(status, reason, eligible, peak, vram, disk, evidence, responsiveness)
        fun no(reason: String) = result("NOT_YET_QUALIFIED", reason)
        fun unsupported(reason: String) = result("UNSUPPORTED", reason)
        fun limits(reason: String, eligible: Boolean = false) = result("SUPPORTED_WITH_LIMITS", reason, eligible)
        val m = c.o("memory"); val runtime = c.o("runtime")
        val agents = r.n("parallel_agents")
        if (p.n("schema_version") != BigInteger.ONE || r.n("schema_version") != BigInteger.ONE ||
            !p.s("probe_id").matches(Regex("[A-Za-z0-9][A-Za-z0-9._-]{0,159}")) ||
            agents <= BigInteger.ZERO || agents > c.n("max_parallel_agents") ||
            r.a("capabilities").isEmpty() || r.a("languages").isEmpty()) return unsupported("INVALID_INPUT")
        if (now.toBigInteger() >= c.n("expires_at_ms")) return no("EVIDENCE_EXPIRED")
        val captured = p.n("captured_at_ms")
        if (captured > now.toBigInteger() || now.toBigInteger() - captured > MAX_PROBE_AGE_MS.toBigInteger()) return no("STALE_PROBE")
        if (c.a("licences").any { it.jsonObject.s("distribution") != "APPROVED" }) return unsupported("LICENCE_NOT_APPROVED")
        if (r.n("context_tokens") != m.n("context_tokens") || r.s("kv_format") != m.s("kv_format")) return unsupported("CONTEXT_MISMATCH")
        if (r.a("capabilities").any { it !in c.a("capabilities") } || r.a("languages").any { it !in c.a("languages") }) return unsupported("CAPABILITY_MISMATCH")
        for (k in listOf("os", "os_version", "abi", "cpu_features", "device_class")) if (p.measured(k) == null) return no("UNKNOWN_PROBE")
        if (listOf("os", "os_version", "abi").any { p.measured(it) != runtime[it] } ||
            runtime.a("cpu_features").any { it !in p.measured("cpu_features")!!.jsonArray }) return unsupported("RUNTIME_MISMATCH")
        runtime["required_os_api_level"]?.takeUnless { it is JsonNull }?.let {
            val api = p.measured("os_api_level") ?: return no("UNKNOWN_PROBE")
            if (api != it) return unsupported("RUNTIME_MISMATCH")
        }
        if (p.a("backends").none { entry -> val b = entry.jsonObject
            b["runtime"] == runtime["runtime"] && b["runtime_version"] == runtime["version"] &&
                b["backend"] == runtime["backend"] && b["driver_version"] == runtime["driver_version"] &&
                b.o("health").s("provenance") == "TESTED" && b.o("health").s("value") == "LOAD_VALIDATED"
        }) return no("BACKEND_NOT_VALIDATED")
        if (m.o("provenance").s("provenance") == "UNKNOWN") return no("UNQUALIFIED")
        for (k in listOf("total_ram_bytes", "available_ram_bytes", "unified_memory", "low_memory_threshold_bytes", "native_budget_bytes", "heap_budget_bytes", "usable_storage_bytes", "thermal"))
            if (p.measured(k) == null) return no("UNKNOWN_PROBE")
        val total = p.mn("total_ram_bytes")!!; val avail = p.mn("available_ram_bytes")!!
        val low = p.mn("low_memory_threshold_bytes")!!; val nativeBudget = p.mn("native_budget_bytes")!!
        val heap = p.mn("heap_budget_bytes")!!; val storage = p.mn("usable_storage_bytes")!!
        val unified = p.measured("unified_memory")!!.jsonPrimitive.boolean
        val thermal = p.measured("thermal")!!.jsonPrimitive.content
        if (total == BigInteger.ZERO || avail > total || nativeBudget == BigInteger.ZERO || heap == BigInteger.ZERO || low > total) return unsupported("INVALID_INPUT")
        val totalNeed: BigInteger; val availNeed: BigInteger; val comfortable: BigInteger
        try {
            val checkedPeak = sum(listOf("weights_ram_bytes", "projector_ram_bytes", "vision_ram_bytes", "kv_ram_bytes", "speech_ram_bytes", "runtime_ram_bytes").map { m.n(it) } +
                listOf(m.n("per_agent_ram_bytes") * agents, if (unified) m.n("peak_vram_bytes") else BigInteger.ZERO))
            val checkedDisk = sum(c.a("artifacts").flatMap { listOf(it.jsonObject.n("download_bytes"), it.jsonObject.n("installed_bytes")) } + m.n("disk_headroom_bytes"))
            peak = checkedPeak; disk = checkedDisk; vram = m.n("peak_vram_bytes")
            totalNeed = sum(listOf(checkedPeak, m.n("os_reserve_bytes")))
            availNeed = sum(listOf(checkedPeak, m.n("available_reserve_bytes").max(low)))
            comfortable = sum(listOf(availNeed, m.n("comfortable_headroom_bytes")))
        } catch (_: IllegalArgumentException) { return unsupported("ARITHMETIC_OVERFLOW") }
        if (totalNeed > total || peak!! > nativeBudget || m.n("heap_bytes") > heap) return unsupported("PERMANENT_MEMORY_MISFIT")
        val matching = qualifications.filter { q -> val scope = q.o("scope")
            scope["candidate"] == c && scope["device_class"] == p.measured("device_class") && scope.n("parallel_agents") == agents &&
                q.n("issued_at_ms") <= now.toBigInteger() && now.toBigInteger() < q.n("expires_at_ms") &&
                r.a("capabilities").all { it in q.a("tested_capabilities") } && r.a("languages").all { it in q.a("tested_languages") }
        }.sortedBy { it.s("id") }
        val native = preflight?.takeIf { it.candidate == c && it.request == r && it.probeId == p.s("probe_id") && it.capturedAtMs <= now && now < it.expiresAtMs }
        val q = matching.firstOrNull { it.s("kind") == "PHYSICAL_DEVICE" || native != null }
            ?: return no(if (matching.isEmpty()) "UNQUALIFIED" else "PREFLIGHT_REQUIRED")
        evidence = q.a("evidence_ids").map { it.jsonPrimitive.content } + q.s("id")
        responsiveness = native?.responsiveness ?: q["responsiveness"]?.takeUnless { it is JsonNull }
        if (native != null) evidence = evidence + native.evidenceId
        if (!unified && vram!! > BigInteger.ZERO) {
            val availableVram = p.mn("available_vram_bytes") ?: return no("UNKNOWN_PROBE")
            val totalVram = p.mn("total_vram_bytes") ?: return no("UNKNOWN_PROBE")
            if (availableVram > totalVram) return unsupported("INVALID_INPUT")
            if (vram!! > totalVram) return unsupported("PERMANENT_MEMORY_MISFIT")
            if (vram!! > availableVram) return limits("MEMORY_PRESSURE")
        }
        if (availNeed > avail) return limits("MEMORY_PRESSURE")
        if (disk!! > storage) return limits("INSUFFICIENT_STORAGE")
        if (thermal in listOf("THROTTLED", "CRITICAL")) return limits("THERMAL_PRESSURE")
        if (avail < comfortable || thermal == "WARM") return limits("LIMITED_HEADROOM", true)
        if (responsiveness == null) return limits("RESPONSIVENESS_UNMEASURED", true)
        return result("RECOMMENDED", "ADMITTED", true)
    }

    /** Release catalogue has no verified qualification records. Never accepts evidence from UI. */
    fun assessUnqualified(c: JsonObject, p: JsonObject, r: JsonObject, now: Long): Decision =
        runCatching { evaluate(c, p, r, emptyList(), now) }
            .getOrElse { Decision("NOT_YET_QUALIFIED", "UNKNOWN_PROBE") }
}
