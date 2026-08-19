package com.unoone.agent.core.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for [ModelLoadResult].
 *
 * Verifies factory methods, convenience properties (isHardwareAccelerated,
 * isCpuFallback), and all load/failure scenarios.
 */
class ModelLoadResultTest {

    // ── Factory: loaded() ──────────────────────────────────────────────

    @Test
    fun loaded_gpu_isHardwareAccelerated() {
        val result = ModelLoadResult.loaded(
            modelId = BrainModelId.GEMMA_4_E2B,
            backend = "GPU",
            loadTimeMs = 3500,
            availableRamMb = 12_288,
            profile = ModelProfiles.LITE
        )
        assertTrue(result.success)
        assertTrue(result.isHardwareAccelerated)
        assertFalse(result.isCpuFallback)
        assertEquals("GPU", result.backend)
        assertEquals(3500L, result.loadTimeMs)
        assertEquals(12_288, result.availableRamMb)
        assertEquals(ModelProfiles.LITE, result.profile)
    }

    @Test
    fun loaded_npu_isHardwareAccelerated() {
        val result = ModelLoadResult.loaded(
            modelId = BrainModelId.GEMMA_4_E4B,
            backend = "NPU",
            loadTimeMs = 2000,
            availableRamMb = 16_384,
            profile = ModelProfiles.MEDIUM
        )
        assertTrue(result.success)
        assertTrue(result.isHardwareAccelerated)
        assertFalse(result.isCpuFallback)
        assertEquals("NPU", result.backend)
    }

    @Test
    fun loaded_cpu_isCpuFallback() {
        val result = ModelLoadResult.loaded(
            modelId = BrainModelId.GEMMA_4_E2B,
            backend = "CPU",
            loadTimeMs = 8000,
            availableRamMb = 6_144,
            profile = ModelProfiles.LITE
        )
        assertTrue(result.success)
        assertFalse(result.isHardwareAccelerated)
        assertTrue(result.isCpuFallback)
        assertEquals("CPU", result.backend)
    }

    @Test
    fun loaded_e4b_onGpu() {
        val result = ModelLoadResult.loaded(
            modelId = BrainModelId.GEMMA_4_E4B,
            backend = "GPU",
            loadTimeMs = 4500,
            availableRamMb = 12_288,
            profile = ModelProfiles.MEDIUM
        )
        assertEquals(BrainModelId.GEMMA_4_E4B, result.modelId)
        assertEquals(ModelProfiles.MEDIUM, result.profile)
        assertTrue(result.isHardwareAccelerated)
    }

    // ── Factory: failed() ──────────────────────────────────────────────

    @Test
    fun failed_gpuBackendError() {
        val result = ModelLoadResult.failed(
            modelId = BrainModelId.GEMMA_4_E4B,
            errorMessage = "GPU backend failed: delegate error; fell back to CPU",
            availableRamMb = 8_192
        )
        assertFalse(result.success)
        assertFalse(result.isHardwareAccelerated)
        assertFalse(result.isCpuFallback)
        assertEquals("", result.backend)
        assertEquals(0L, result.loadTimeMs)
        assertNull(result.profile)
        assertTrue(result.errorMessage.contains("GPU backend failed"))
        assertEquals(8_192, result.availableRamMb)
    }

    @Test
    fun failed_allBackendsExhausted() {
        val result = ModelLoadResult.failed(
            modelId = BrainModelId.GEMMA_4_E2B,
            errorMessage = "Gemma 4 E2B failed to load on any backend (GPU→NPU→CPU)",
            availableRamMb = 4_096
        )
        assertFalse(result.success)
        assertEquals(BrainModelId.GEMMA_4_E2B, result.modelId)
    }

    @Test
    fun failed_defaultRamIsMinusOne() {
        // -1 is the sentinel for "unknown" — callers should check for -1 before using
        // availableRamMb to decide model tier, not treat 0 as "insufficient RAM".
        val result = ModelLoadResult.failed(
            modelId = BrainModelId.GEMMA_4_E2B,
            errorMessage = "Load error"
        )
        assertEquals(-1, result.availableRamMb)
    }

    // ── Edge cases ──────────────────────────────────────────────────────

    @Test
    fun loaded_emptyBackend_isNotHardwareAccelerated() {
        val result = ModelLoadResult.loaded(
            modelId = BrainModelId.GEMMA_4_E2B,
            backend = "",
            loadTimeMs = 0,
            availableRamMb = 0,
            profile = ModelProfiles.LITE
        )
        // Empty backend string — neither GPU/NPU nor CPU
        assertFalse(result.isHardwareAccelerated)
        assertFalse(result.isCpuFallback)
    }

    @Test
    fun failed_result_hasNoProfile() {
        val result = ModelLoadResult.failed(
            modelId = BrainModelId.GEMMA_4_E4B,
            errorMessage = "Out of memory"
        )
        assertNull(result.profile)
    }

    @Test
    fun loaded_result_e2b_matchesLiteProfile() {
        val result = ModelLoadResult.loaded(
            modelId = BrainModelId.GEMMA_4_E2B,
            backend = "GPU",
            loadTimeMs = 3000,
            availableRamMb = 8_192,
            profile = ModelProfiles.forId(BrainModelId.GEMMA_4_E2B)
        )
        assertEquals(ModelProfiles.LITE, result.profile)
        assertEquals("Lite", result.profile?.displayName)
    }

    @Test
    fun loaded_result_e4b_matchesMediumProfile() {
        val result = ModelLoadResult.loaded(
            modelId = BrainModelId.GEMMA_4_E4B,
            backend = "GPU",
            loadTimeMs = 5000,
            availableRamMb = 16_384,
            profile = ModelProfiles.forId(BrainModelId.GEMMA_4_E4B)
        )
        assertEquals(ModelProfiles.MEDIUM, result.profile)
        assertEquals("Medium", result.profile?.displayName)
    }
}