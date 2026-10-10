package com.unoone.agent.modelmanager

import com.unoone.agent.core.modeladmission.AdmissionOverheads
import com.unoone.agent.core.modeladmission.ManualAdmission
import com.unoone.agent.core.modeladmission.ManualAdmission.Observation
import org.junit.Assert.*
import org.junit.Test

/** The manual-risk lane arithmetic now derives from the shared Rust-core overheads, not 2 GiB + 384 MiB. */
class ModelMemoryProfileTest {
    private fun <T> d(v: T) = Observation.detected(v)
    private val mib = 1024L * 1024
    private val e4b = ModelDescriptor("gemma-4-e4b", "brain/gemma-4-e4b", ModelType.llm, "1", minRamMb = 8192, files = listOf(
        ModelFile("gemma-4-E4B-it.litertlm", "https://example.invalid/e4b", "f".repeat(64), 3_659_530_240L)))
    private val kws = ModelDescriptor("sherpa-kws-en", "speech/shared/sherpa-kws-en", ModelType.kws, "1", minRamMb = 256, files = listOf(
        ModelFile("encoder.onnx", "https://example.invalid/k", "e".repeat(64), 72_654_782L)))
    private val archive = ModelDescriptor("sherpa-asr-indic", "speech/shared/x", ModelType.asr, "1", minRamMb = 2048, files = listOf(
        ModelFile("pack.tar.bz2", "https://example.invalid/a", "e".repeat(64), 292_571_207L, archive = true)))

    @Test fun brainProfileUsesSharedOverheadsAndRegistryContext() {
        val p = ModelMemoryProfile.profile(e4b)
        assertEquals(2048, p.contextTokens)
        assertEquals(AdmissionOverheads.workingSetEstimateBytes(3_659_530_240L, 2048), p.workingSetEstimate)
        assertEquals(AdmissionOverheads.OS_RESERVE_BYTES, p.osReserve)
        assertEquals(AdmissionOverheads.AVAILABLE_RESERVE_BYTES, p.availableReserve)
        assertEquals(AdmissionOverheads.DISK_HEADROOM_BYTES, p.diskHeadroom)
        assertEquals(AdmissionOverheads.HEAP_BYTES, p.heapReserve)
        assertEquals(setOf("liblitertlm_jni.so"), p.requiredLibraries)
        val invented = 3_659_530_240L + 2L * 1024 * mib + 384L * mib
        assertTrue(p.workingSetEstimate!! < invented)
        assertEquals(ModelBundleStore.sha256(kotlinx.serialization.json.Json.encodeToString(ModelDescriptor.serializer(), e4b).toByteArray()), p.identity)
    }

    @Test fun eightGbPhoneIsMemoryPressureNotPermanentMisfitForE4b() {
        val p = ModelMemoryProfile.profile(e4b)
        val now = 10_000L
        val policy = ManualAdmission.RiskPolicy(p.identity, 2048, now - 1, now + 1000, p.downloadBytes, Long.MAX_VALUE, false, true)
        fun probe(total: Long, available: Long) = ManualAdmission.Probe(now, d(total), d(available), d(300 * mib), d(false), d(256 * mib),
            d(40L * 1024 * mib), d(35), d("arm64-v8a"), d(setOf("asimd")), d(setOf("liblitertlm_jni.so")))
        // Xiaomi 14-class 8 GB device: ActivityManager reports ~7.4 GiB total.
        val idle = ManualAdmission.assess(p, probe(7_400 * mib, 3_000 * mib), policy, now, true, true, false, 0, loading = true)
        assertEquals("MEMORY_PRESSURE", idle.reason) // soft, retryable; evidence may override (LoadAdmission)
        val roomy = ManualAdmission.assess(p, probe(7_400 * mib, 6_500 * mib), policy, now, true, true, false, 0, loading = true)
        assertTrue(roomy.allowed)
        // A 6 GB device really is a permanent misfit for an 8 GB-minimum profile.
        val small = ManualAdmission.assess(p, probe(5_600 * mib, 5_000 * mib), policy, now, true, true, false, 0, loading = true)
        assertEquals("PERMANENT_MEMORY_MISFIT", small.reason)
    }

    @Test fun minimumTotalRamUsesCeilGibForBrainsAndExactBytesForSpeech() {
        assertEquals(8192 * mib - (1024 * mib - 1), ModelMemoryProfile.minimumTotalRamBytes(8192))
        assertEquals(256 * mib, ModelMemoryProfile.minimumTotalRamBytes(256))
        assertEquals(256 * mib, ModelMemoryProfile.profile(kws).minimumTotalRam)
    }

    @Test fun speechPackAndArchiveProfilesStayHonest() {
        val k = ModelMemoryProfile.profile(kws)
        assertEquals(0, k.contextTokens)
        assertEquals(72_654_782L + AdmissionOverheads.RUNTIME_RAM_BYTES, k.workingSetEstimate)
        assertEquals(setOf("libsherpa-onnx-jni.so"), k.requiredLibraries)
        val a = ModelMemoryProfile.profile(archive)
        assertNull(a.installedBytes); assertNull(a.workingSetEstimate) // unknown expansion is never zero
    }
}
