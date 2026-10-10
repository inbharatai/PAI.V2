package com.unoone.agent.model

import com.unoone.agent.core.modeladmission.ManualAdmission
import com.unoone.agent.core.modeladmission.ManualAdmission.Observation
import com.unoone.agent.modelmanager.*
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import java.nio.file.Files

/** Executes the SAME lane called by CoroutineWorker, with real asset transfers and filesystem. */
class ModelDownloadLaneTest {
    private fun <T> d(value: T) = Observation.detected(value)
    private val profile = ManualAdmission.Profile("a".repeat(64), "m", 2048, 10, 10, 1000, 400, 10, setOf("lib.so"), availableReserve = 100, diskHeadroom = 100)
    private val probe = ManualAdmission.Probe(1000, d(2000L), d(1000L), d(100L), d(false), d(500L), d(10000L), d(35), d("arm64-v8a"), d(setOf("asimd")), d(setOf("lib.so")))
    private val policy = ManualAdmission.RiskPolicy(profile.identity, 2048, 900, 100000, 10, 10000, false, true)
    private fun decision(p: ManualAdmission.Probe = probe, consent: ManualAdmission.RiskPolicy? = policy, metered: Boolean? = false, online: Boolean = true) =
        ManualAdmission.assess(profile, p, consent, 1000, true, online, metered, 0)
    @Test fun directWorkerNegativePermissionsStartZeroActualPayloadBytes() = runBlocking {
        val root = Files.createTempDirectory("direct-worker").toFile()
        var assetReads = 0; var transferCalls = 0
        try {
            val bytes = "model-data".toByteArray()
            val descriptor = ModelDescriptor("m", "legacy/m", ModelType.llm, "v", files = listOf(ModelFile("model.bin", "", ModelBundleStore.sha256(bytes), bytes.size.toLong(), asset = "model.bin")))
            val installer = ModelInstaller(root.path) { assetReads++; bytes.inputStream() }
            val negatives = listOf(decision(consent = null), decision(consent = policy.copy(expiresAtMs = 999)),
                decision(consent = policy.copy(identity = "other")), decision(consent = policy.copy(maxStoreBytes = 1)),
                decision(metered = true), decision(metered = null), decision(online = false),
                decision(p = probe.copy(availableRam = Observation.unknown())), decision(p = probe.copy(availableRam = d(1))),
                decision(p = probe.copy(totalRam = d(500), availableRam = d(300))), decision(p = probe.copy(capturedAtMs = -60000)),
                decision(p = probe.copy(libraries = Observation.unknown())))
            for (negative in negatives) {
                val result = ModelDownloadLane.run("m", { true }, { false }, { negative }) {
                    transferCalls++; installer.install(descriptor, admission = { null })
                }
                assertTrue(result is ModelInstaller.InstallResult.Failure)
            }
            for ((id, enabled, stopped) in listOf(Triple(null, true, false), Triple("m", false, false), Triple("m", true, true))) {
                assertTrue(ModelDownloadLane.run(id, { enabled }, { stopped }, { decision() }) {
                    transferCalls++; installer.install(descriptor, admission = { null })
                } is ModelInstaller.InstallResult.Failure)
            }
            assertEquals(0, transferCalls); assertEquals(0, assetReads)
            val positive = ModelDownloadLane.run("m", { true }, { false }, { decision() }) {
                transferCalls++; installer.install(descriptor, admission = { null })
            }
            assertTrue(positive is ModelInstaller.InstallResult.Staged)
            assertEquals(1, transferCalls); assertEquals(1, assetReads)
            assertNull(ModelBundleStore(root).active("m"))
        } finally { root.deleteRecursively() }
    }
    @Test fun revokedAfterInitialAdmissionBlockedAtInstallerBoundary() = runBlocking {
        val root = Files.createTempDirectory("worker-revoke").toFile(); var reads = 0
        try {
            val bytes = "data".toByteArray()
            val installer = ModelInstaller(root.path) { reads++; bytes.inputStream() }
            val descriptor = ModelDescriptor("m", "m", ModelType.llm, "v", files = listOf(ModelFile("a.bin", "", ModelBundleStore.sha256(bytes), bytes.size.toLong(), asset = "a.bin")))
            val result = ModelDownloadLane.run("m", { true }, { false }, { decision() }) {
                installer.install(descriptor, admission = { "POLICY_REVOKED" })
            }
            assertTrue(result is ModelInstaller.InstallResult.Failure); assertEquals(0, reads)
        } finally { root.deleteRecursively() }
    }
}
