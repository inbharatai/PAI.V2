package com.unoone.agent.core.modeladmission

import java.io.File
import kotlinx.serialization.json.*
import org.junit.Assert.*
import org.junit.Test

/** The Rust golden vector is the source of truth; these constants must never drift from it. */
class AdmissionOverheadsTest {
    private fun fixtureMemory(): JsonObject {
        val file = System.getProperty("model.admission.fixture")?.let(::File)
            ?: generateSequence(File(System.getProperty("user.dir")).absoluteFile) { it.parentFile }
                .map { File(it, "packages/model-admission/tests/fixtures/admission-v1.json") }.first { it.isFile }
        return Json.parseToJsonElement(file.readText()).jsonObject.getValue("candidate").jsonObject.getValue("memory").jsonObject
    }
    private fun JsonObject.n(k: String) = getValue(k).jsonPrimitive.long

    @Test fun constantsMatchSharedRustGoldenVector() {
        val m = fixtureMemory()
        assertEquals(m.n("speech_ram_bytes"), AdmissionOverheads.SPEECH_RAM_BYTES)
        assertEquals(m.n("runtime_ram_bytes"), AdmissionOverheads.RUNTIME_RAM_BYTES)
        assertEquals(m.n("per_agent_ram_bytes"), AdmissionOverheads.PER_AGENT_RAM_BYTES)
        assertEquals(m.n("heap_bytes"), AdmissionOverheads.HEAP_BYTES)
        assertEquals(m.n("os_reserve_bytes"), AdmissionOverheads.OS_RESERVE_BYTES)
        assertEquals(m.n("available_reserve_bytes"), AdmissionOverheads.AVAILABLE_RESERVE_BYTES)
        assertEquals(m.n("comfortable_headroom_bytes"), AdmissionOverheads.COMFORTABLE_HEADROOM_BYTES)
        assertEquals(m.n("disk_headroom_bytes"), AdmissionOverheads.DISK_HEADROOM_BYTES)
        assertEquals(m.n("kv_ram_bytes"), AdmissionOverheads.KV_REFERENCE_BYTES)
        assertEquals(m.n("context_tokens").toInt(), AdmissionOverheads.KV_REFERENCE_CONTEXT_TOKENS)
        // KV estimate reproduces the golden vector exactly at its own context size.
        assertEquals(m.n("kv_ram_bytes"), AdmissionOverheads.kvEstimateBytes(AdmissionOverheads.KV_REFERENCE_CONTEXT_TOKENS))
    }

    @Test fun workingSetMirrorsRustPeakRamForOneAgent() {
        val m = fixtureMemory()
        val weightsAndProjector = m.n("weights_ram_bytes") + m.n("projector_ram_bytes")
        // Rust peak_ram = weights+projector+vision+kv+speech+runtime+per_agent*agents (+vram if unified).
        val rustPeakNoVision = weightsAndProjector + m.n("kv_ram_bytes") + m.n("speech_ram_bytes") + m.n("runtime_ram_bytes") + m.n("per_agent_ram_bytes")
        assertEquals(rustPeakNoVision, AdmissionOverheads.workingSetEstimateBytes(weightsAndProjector, m.n("context_tokens").toInt()))
        assertEquals(rustPeakNoVision + m.n("os_reserve_bytes"), AdmissionOverheads.totalRequiredBytes(rustPeakNoVision))
        assertEquals(rustPeakNoVision + m.n("available_reserve_bytes"), AdmissionOverheads.availableRequiredBytes(rustPeakNoVision, 1))
        assertEquals(rustPeakNoVision + (1L shl 30), AdmissionOverheads.availableRequiredBytes(rustPeakNoVision, 1L shl 30))
    }

    @Test fun e4bOnEightGbPhoneIsNotAPermanentMisfitUnderTheSharedConstants() {
        val e4bBytes = 3_659_530_240L // models_manifest.json gemma-4-e4b
        val eightGbPhoneTotal = 7_600L * 1024 * 1024 // typical ActivityManager totalMem on an 8 GB device
        val workingSet = AdmissionOverheads.workingSetEstimateBytes(e4bBytes, 2048)
        val previousInventedEstimate = e4bBytes + 2L * 1024 * 1024 * 1024 + 384L * 1024 * 1024
        assertTrue(workingSet < previousInventedEstimate)
        assertTrue(AdmissionOverheads.totalRequiredBytes(workingSet) <= eightGbPhoneTotal)
    }

    @Test fun overflowNeverWrapsToZero() {
        try { AdmissionOverheads.workingSetEstimateBytes(Long.MAX_VALUE, 2048); fail() } catch (_: ArithmeticException) {}
        try { AdmissionOverheads.workingSetEstimateBytes(0, 2048); fail() } catch (_: IllegalArgumentException) {}
    }
}
