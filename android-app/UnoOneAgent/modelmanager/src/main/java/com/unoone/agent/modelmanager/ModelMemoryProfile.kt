package com.unoone.agent.modelmanager

import com.unoone.agent.core.model.BrainModelRegistry
import com.unoone.agent.core.model.BrainRuntime
import com.unoone.agent.core.modeladmission.AdmissionOverheads
import com.unoone.agent.core.modeladmission.ManualAdmission
import kotlinx.serialization.json.Json

/**
 * Context-free working-set profile for one catalogue descriptor. Replaces the previous ad-hoc
 * `bytes + 2 GiB + 384 MiB` reserve with the shared Rust admission-core overheads
 * ([AdmissionOverheads], golden vector `admission-v1.json`). Still an ESTIMATE, never a measured peak.
 */
object ModelMemoryProfile {
    fun identity(d: ModelDescriptor): String =
        ModelBundleStore.sha256(Json.encodeToString(ModelDescriptor.serializer(), d).toByteArray())

    fun requiredLibraries(runtime: BrainRuntime?): Set<String> = when (runtime) {
        BrainRuntime.LITERT_LM -> setOf("liblitertlm_jni.so")
        BrainRuntime.MNN -> setOf("libunoone_qwen.so", "libMNN.so")
        BrainRuntime.LLAMA_CPP -> setOf("libunoone_owl.so", "libllama.so", "libmtmd.so", "libggml-cpu.so")
        null -> setOf("libsherpa-onnx-jni.so")
    }

    /**
     * `ActivityManager.totalMem` on an "8 GB" phone reports ~7.2–7.7 GiB (kernel/carveout reserved),
     * so a byte-exact `total < 8192 MiB` made every 8 GB device a PERMANENT_MEMORY_MISFIT for the E4B
     * profile the same device demonstrably loaded. Marketed capacity is `ceil(total / GiB)`; the rule
     * `total >= minimum - (1 GiB - 1)` is exactly `ceil(total GiB) >= minimum GiB` (Power rounds too,
     * `desktop_model_policy::fits`). Below 1 GiB minimums (speech packs) stay byte-exact.
     */
    fun minimumTotalRamBytes(minimumRamMb: Int): Long {
        val exact = minimumRamMb.toLong() * 1024 * 1024
        return if (minimumRamMb >= 1024) exact - (1024L * 1024 * 1024 - 1) else exact
    }

    fun profile(d: ModelDescriptor): ManualAdmission.Profile {
        val spec = BrainModelRegistry.byManifestId(d.id)
        val bytes = d.files.fold(0L) { sum, file -> Math.addExact(sum, file.sizeBytes) }
        val hasArchive = d.files.any { it.archive }
        // Archive expansion is unknown until a reviewed installed-size profile is supplied: no estimate.
        val installed = if (hasArchive) null else bytes
        val workingSet: Long? = when {
            hasArchive || bytes <= 0L -> null
            spec != null -> AdmissionOverheads.workingSetEstimateBytes(bytes, spec.defaultContextTokens)
            // Speech packs: no KV cache; runtime overhead only (sherpa-onnx is small, but never zero).
            else -> Math.addExact(bytes, AdmissionOverheads.RUNTIME_RAM_BYTES)
        }
        return ManualAdmission.Profile(
            identity = identity(d),
            modelId = d.id,
            contextTokens = spec?.defaultContextTokens ?: 0,
            downloadBytes = bytes,
            installedBytes = installed,
            minimumTotalRam = minimumTotalRamBytes(spec?.minimumRamMb ?: d.minRamMb),
            workingSetEstimate = workingSet,
            heapReserve = AdmissionOverheads.HEAP_BYTES,
            requiredLibraries = requiredLibraries(spec?.runtime),
            availableReserve = AdmissionOverheads.AVAILABLE_RESERVE_BYTES,
            diskHeadroom = AdmissionOverheads.DISK_HEADROOM_BYTES,
            osReserve = AdmissionOverheads.OS_RESERVE_BYTES
        )
    }
}
