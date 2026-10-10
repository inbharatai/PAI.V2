package com.unoone.agent.core.modeladmission

import com.unoone.agent.core.modeladmission.ManualAdmission.Observation
import com.unoone.agent.core.modeladmission.ManualAdmission.Profile
import com.unoone.agent.core.modeladmission.ManualAdmission.Probe
import com.unoone.agent.core.modeladmission.ManualAdmission.RiskPolicy
import com.unoone.agent.core.modeladmission.ManualAdmission.Provenance
import org.junit.Test
import org.junit.Assert.*

class ManualAdmissionTest {
    private fun <T> detected(v: T) = Observation.detected(v)
    private val p = Profile("a".repeat(64), "manual", 2048, 100, 100, 1000, 400, 10, setOf("lib.so"), availableReserve = 100, diskHeadroom = 100)
    private val probe = Probe(1000, detected(2000L), detected(1000L), detected(100L), detected(false),
        detected(500L), detected(10000L), detected(35), detected("arm64-v8a"), detected(setOf("asimd")), detected(setOf("lib.so")))
    private val policy = RiskPolicy(p.identity, 2048, 900, 100000, 100, 10000, false, true)
    private fun assess(pr: Probe = probe, po: RiskPolicy? = policy, enabled: Boolean = true,
        connected: Boolean = true, metered: Boolean? = false, now: Long = 1000, loading: Boolean = false) =
        ManualAdmission.assess(p, pr, po, now, enabled, connected, metered, 0, loading)
    @Test fun explicitManualIsNotRecommended() { assertTrue(assess().allowed); assertTrue(assess().reason.contains("NOT_RECOMMENDED")) }
    @Test fun allDirectWorkerNegativePermissionsHaveZeroTransfer() {
        val decisions = listOf(assess(po = null), assess(enabled = false), assess(connected = false), assess(metered = true),
            assess(metered = null), assess(po = policy.copy(expiresAtMs = 999)), assess(po = policy.copy(identity = "other")),
            assess(po = policy.copy(acknowledgeUnqualified = false)), assess(po = policy.copy(maxDownloadBytes = 1)),
            assess(po = policy.copy(maxStoreBytes = 1)))
        var payloadBytes = 0
        decisions.forEach { if (it.allowed) payloadBytes += 1 }
        assertEquals(0, payloadBytes)
    }
    @Test fun unknownPressurePhysicalMisfitAndStaleAlwaysPause() {
        val bad = listOf(probe.copy(availableRam = Observation.unknown()), probe.copy(cpuFeatures = Observation.unknown()),
            probe.copy(storage = Observation.unknown()), probe.copy(libraries = Observation.unknown()),
            probe.copy(availableRam = detected(300)), probe.copy(totalRam = detected(500), availableRam = detected(400)),
            probe.copy(lowMemory = detected(true)), probe.copy(heapAvailable = detected(1)), probe.copy(capturedAtMs = -60000),
            probe.copy(availableRam = Observation(1000, Provenance.ESTIMATED)))
        bad.forEach { assertFalse(assess(pr = it).allowed) }
    }
    @Test fun freshBeforeLoadStillDeniesBadHardwareButAllowsOffline() {
        assertTrue(assess(loading = true, connected = false, metered = null).allowed)
        assertFalse(assess(pr = probe.copy(availableRam = detected(1)), loading = true).allowed)
        assertFalse(assess(now = 61001, loading = true).allowed)
    }
    @Test fun missingRuntimeAndContextChangeDeny() {
        assertFalse(assess(pr = probe.copy(libraries = detected(emptySet()))).allowed)
        assertFalse(assess(po = policy.copy(contextTokens = 4096)).allowed)
    }
    @Test fun overflowAndUnknownExpansionNeverUseZero() {
        assertFalse(ManualAdmission.assess(p.copy(installedBytes = null), probe, policy, 1000, true, true, false, 0).allowed)
        assertEquals("ARITHMETIC_OVERFLOW", ManualAdmission.assess(p.copy(downloadBytes = Long.MAX_VALUE), probe, policy, 1000, true, true, false, 0).reason)
    }
}
