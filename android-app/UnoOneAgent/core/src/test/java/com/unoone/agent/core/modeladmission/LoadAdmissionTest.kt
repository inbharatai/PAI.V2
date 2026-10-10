package com.unoone.agent.core.modeladmission

import com.unoone.agent.core.modeladmission.LoadAdmission.EvidenceState
import com.unoone.agent.core.modeladmission.LoadAdmission.Purpose
import com.unoone.agent.core.modeladmission.ManualAdmission.Observation
import org.junit.Assert.*
import org.junit.Test

/** E4B-shaped numbers on an "8 GB" phone: the case the old 2.4 GiB arithmetic denied. */
class LoadAdmissionTest {
    private fun <T> d(v: T) = Observation.detected(v)
    private val gib = 1024L * 1024 * 1024
    private val e4bBytes = 3_659_530_240L
    private val workingSet = AdmissionOverheads.workingSetEstimateBytes(e4bBytes, 2048)
    private val identity = "a".repeat(64)
    private val device = "b".repeat(64)
    private val profile = ManualAdmission.Profile(identity = identity, modelId = "gemma-4-e4b", contextTokens = 2048,
        downloadBytes = e4bBytes, installedBytes = e4bBytes,
        minimumTotalRam = 8192L * 1024 * 1024 - (1024L * 1024 * 1024 - 1), // ceil(total GiB) >= 8
        workingSetEstimate = workingSet,
        heapReserve = AdmissionOverheads.HEAP_BYTES, requiredLibraries = setOf("liblitertlm_jni.so"),
        availableReserve = AdmissionOverheads.AVAILABLE_RESERVE_BYTES, diskHeadroom = AdmissionOverheads.DISK_HEADROOM_BYTES,
        osReserve = AdmissionOverheads.OS_RESERVE_BYTES)
    private val eightGbTotal = 7_400L * 1024 * 1024 // Xiaomi 14-class ActivityManager.totalMem
    private val now = 1_000_000L
    /** Typical idle 8 GB phone: ~3 GiB "available" — below the ~5.5 GB estimate, far above zero. */
    private fun probe(available: Long = 3 * gib, total: Long = eightGbTotal, low: Boolean = false, libs: Set<String> = setOf("liblitertlm_jni.so")) =
        ManualAdmission.Probe(now - 100, d(total), d(available), d(300L * 1024 * 1024), d(low), d(256L * 1024 * 1024),
            d(40 * gib), d(35), d("arm64-v8a"), d(setOf("asimd")), d(libs))
    private val policy = ManualAdmission.RiskPolicy(identity, 2048, now - 1000, now + 3_600_000, e4bBytes, Long.MAX_VALUE, false, true)
    private fun receipt(passed: Boolean, at: Long, reason: String = "") = NativeLoadReceipt(modelId = "gemma-4-e4b", identity = identity,
        deviceFingerprint = device, outcome = if (passed) NativeLoadReceipt.Outcome.PASSED else NativeLoadReceipt.Outcome.FAILED,
        reason = reason, backend = "CPU", recordedAtMs = at, lane = "EXPLICIT_LOAD")
    private val worksHere = LoadEvidence(latestReceipt = receipt(true, 500), everPassedHere = true)
    private fun decide(purpose: Purpose, evidence: LoadEvidence = LoadEvidence.NONE, p: ManualAdmission.Probe = probe(),
        po: ManualAdmission.RiskPolicy? = policy, profileOverride: ManualAdmission.Profile = profile) =
        LoadAdmission.decide(profileOverride, p, po, evidence, purpose, now, true, true, false, 0)

    @Test fun estimateAloneDeniesE4bAsMemoryPressureNotPermanentMisfit() {
        val decision = decide(Purpose.EXPLICIT_LOAD)
        assertFalse(decision.allowed)
        assertEquals("MEMORY_PRESSURE", decision.reason)
        assertEquals(EvidenceState.UNKNOWN, decision.evidence) // unknown, not refused-forever
        // With the shared constants the TOTAL-RAM budget fits an 8 GB phone (the old 2.4 GiB + threshold rule did not).
        assertTrue(AdmissionOverheads.totalRequiredBytes(workingSet) <= eightGbTotal)
    }

    @Test fun receiptGrandfathersBorderlineEstimateAsWorksHere() {
        for (purpose in listOf(Purpose.EXPLICIT_LOAD, Purpose.BOOT_LOAD, Purpose.ACTIVATION)) {
            val decision = decide(purpose, worksHere)
            assertTrue(purpose.name, decision.allowed)
            assertEquals("${LoadAdmission.EVIDENCE_BEATS_ESTIMATE}:MEMORY_PRESSURE", decision.reason)
            assertEquals(EvidenceState.WORKS_HERE, decision.evidence)
        }
        // Receipt also stands in for the 24 h manual policy on LOAD lanes (boot has no dialog).
        val noPolicy = decide(Purpose.BOOT_LOAD, worksHere, po = null)
        assertTrue(noPolicy.allowed); assertEquals("${LoadAdmission.EVIDENCE_BEATS_ESTIMATE}:LOCAL_RISK_POLICY_REQUIRED", noPolicy.reason)
    }

    @Test fun physicallyImpossibleAndLowMemoryNowAlwaysRefuseEvenWithEvidence() {
        val tooBig = decide(Purpose.BOOT_LOAD, worksHere, p = probe(total = e4bBytes - 1, available = e4bBytes - 2))
        assertFalse(tooBig.allowed); assertEquals(LoadAdmission.PHYSICALLY_IMPOSSIBLE, tooBig.reason)
        assertEquals(EvidenceState.WORKS_HERE, tooBig.evidence) // state is reported honestly, permission separately
        val lowNow = decide(Purpose.EXPLICIT_LOAD, worksHere, p = probe(low = true))
        assertFalse(lowNow.allowed); assertEquals(LoadAdmission.MEMORY_PRESSURE_NOW, lowNow.reason)
        val qualified = decide(Purpose.BOOT_LOAD, LoadEvidence(signedQualification = true), p = probe(total = e4bBytes - 1, available = 1))
        assertFalse(qualified.allowed); assertEquals(EvidenceState.QUALIFIED, qualified.evidence)
    }

    @Test fun measuredRuntimeMismatchIsNeverOverridden() {
        val decision = decide(Purpose.BOOT_LOAD, worksHere, p = probe(libs = emptySet()))
        assertFalse(decision.allowed); assertEquals("RUNTIME_MISMATCH", decision.reason)
    }

    @Test fun bootGateGrandfathersLegacyInstallUntilThisDeviceMeasuresAFailure() {
        val legacy = LoadEvidence(legacyInstalled = true)
        val boot = decide(Purpose.BOOT_LOAD, legacy, po = null)
        assertTrue(boot.allowed); assertEquals(LoadAdmission.LEGACY_INSTALL_GRANDFATHERED, boot.reason)
        assertEquals(EvidenceState.UNKNOWN, boot.evidence)
        val explicit = decide(Purpose.EXPLICIT_LOAD, legacy, po = null)
        assertTrue(explicit.allowed)
        val failedHere = decide(Purpose.BOOT_LOAD, legacy.copy(latestReceipt = receipt(false, 900, "OOM:alloc")), po = null)
        assertFalse(failedHere.allowed); assertTrue(failedHere.reason.startsWith(LoadAdmission.FAILED_HERE_PREVIOUSLY))
        // A later PASSED receipt restores evidence-backed loading.
        val recovered = decide(Purpose.BOOT_LOAD, LoadEvidence(latestReceipt = receipt(true, 950), everPassedHere = true, legacyInstalled = true), po = null)
        assertTrue(recovered.allowed); assertEquals(EvidenceState.WORKS_HERE, recovered.evidence)
        // Legacy does not grandfather physically impossible or lowMemory-now.
        assertFalse(decide(Purpose.BOOT_LOAD, legacy, p = probe(low = true), po = null).allowed)
        assertFalse(decide(Purpose.BOOT_LOAD, legacy, p = probe(total = e4bBytes - 1, available = 1), po = null).allowed)
    }

    @Test fun freshBundleWithoutEvidenceIsPausedNotRefusedAndActivationMayMeasureUnderManualRisk() {
        val bootNoEvidence = decide(Purpose.BOOT_LOAD, LoadEvidence.NONE, po = null)
        assertFalse(bootNoEvidence.allowed); assertEquals("LOCAL_RISK_POLICY_REQUIRED", bootNoEvidence.reason)
        assertEquals(EvidenceState.UNKNOWN, bootNoEvidence.evidence)
        val activation = decide(Purpose.ACTIVATION, LoadEvidence.NONE)
        assertTrue(activation.allowed); assertEquals(LoadAdmission.MANUAL_RISK_TRANSACTIONAL_SMOKE, activation.reason)
        assertFalse(decide(Purpose.ACTIVATION, LoadEvidence.NONE, po = null).allowed)
        assertFalse(decide(Purpose.ACTIVATION, LoadEvidence.NONE, p = probe(low = true)).allowed)
        // Plenty of available RAM: ordinary manual lane passes on its own.
        val roomy = decide(Purpose.EXPLICIT_LOAD, LoadEvidence.NONE, p = probe(available = 7 * gib))
        assertTrue(roomy.allowed); assertEquals("MANUAL_UNQUALIFIED_RISK_ACCEPTED_NOT_RECOMMENDED", roomy.reason)
    }

    @Test fun downloadNeverBorrowsEvidence() {
        val offline = LoadAdmission.decide(profile, probe(available = 7 * gib), policy, worksHere, Purpose.DOWNLOAD, now, true, false, null, 0)
        assertFalse(offline.allowed); assertEquals("NETWORK_POLICY", offline.reason)
        assertEquals(EvidenceState.WORKS_HERE, offline.evidence)
        assertFalse(decide(Purpose.DOWNLOAD, worksHere, po = null).allowed)
    }

    @Test fun disabledRuntimeDeniesEverything() {
        assertFalse(LoadAdmission.decide(profile, probe(), policy, worksHere, Purpose.BOOT_LOAD, now, false, true, false, 0).allowed)
    }
}
