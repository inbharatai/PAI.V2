package com.unoone.agent.model

import com.unoone.agent.core.modeladmission.NativeLoadReceipt
import com.unoone.agent.model.StagedActivation.Phase
import com.unoone.agent.modelmanager.ModelBundleStore
import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test

/** Real bundle store on a real filesystem; only the native runtime is faked. */
class StagedActivationTest {
    private class FakePort(
        var loadOutcomes: MutableList<StagedActivation.LoadOutcome> = mutableListOf(),
        var smokeOutcome: StagedActivation.SmokeOutcome = StagedActivation.SmokeOutcome.Passed(3)
    ) : StagedActivation.NativePort {
        var resident: String? = null
        val loads = mutableListOf<String>(); var unloads = 0; var smokes = 0
        override fun currentLoadedPath() = resident
        override suspend fun unloadCurrent(): Boolean { unloads++; resident = null; return true }
        override suspend fun load(path: String): StagedActivation.LoadOutcome {
            loads += path
            val outcome = if (loadOutcomes.isEmpty()) StagedActivation.LoadOutcome.Loaded("CPU") else loadOutcomes.removeAt(0)
            if (outcome is StagedActivation.LoadOutcome.Loaded) resident = path
            return outcome
        }
        override suspend fun smoke(): StagedActivation.SmokeOutcome { smokes++; return smokeOutcome }
    }
    private fun withStore(test: (File, ModelBundleStore) -> Unit) {
        val dir = Files.createTempDirectory("staged-activation").toFile()
        try { test(dir, ModelBundleStore(dir)) } finally { dir.deleteRecursively() }
    }
    private fun stage(store: ModelBundleStore, text: String): ModelBundleStore.Staged {
        val identity = ModelBundleStore.sha256(text.toByteArray())
        val dir = store.stageDirectory("gemma-4-e2b", identity)
        File(dir, "model.litertlm").writeText("$text-weights")
        return store.seal("gemma-4-e2b", identity, text)
    }
    private val resolve: (File) -> String? = { root -> File(root, "model.litertlm").takeIf { it.isFile }?.absolutePath }
    private fun run(store: ModelBundleStore, bundle: ModelBundleStore.Staged, port: FakePort, receipts: MutableList<StagedActivation.Receipt>,
        states: MutableList<StagedActivation.State> = mutableListOf(),
        activate: (ModelBundleStore.Staged, ModelBundleStore.NativeLoader) -> Boolean = { s, l -> store.activate(s, l) }) =
        runBlocking { StagedActivation.run(bundle, port, resolve, { receipts += it }, activate) { states += it } }

    @Test fun downloadStagedVerifyingActiveSwitchesPointerAfterRealLoadAndSmoke() = withStore { dir, store ->
        val bundle = stage(store, "v1")
        assertNull(store.active("gemma-4-e2b"))
        val port = FakePort(); val receipts = mutableListOf<StagedActivation.Receipt>(); val states = mutableListOf<StagedActivation.State>()
        val state = run(store, bundle, port, receipts, states)
        assertEquals(Phase.ACTIVE, state.phase)
        assertEquals(listOf(Phase.STAGED, Phase.VERIFYING), states.take(2).map { it.phase })
        assertEquals(Phase.ACTIVE, states.last().phase)
        assertEquals(bundle.bundleId, store.active("gemma-4-e2b")!!.bundleId)
        assertEquals(listOf(File(bundle.root, "model.litertlm").absolutePath), port.loads)
        assertEquals(1, port.smokes)
        assertEquals(port.resident, File(bundle.root, "model.litertlm").absolutePath) // candidate stays resident as the active model
        assertEquals(1, receipts.size); assertEquals(NativeLoadReceipt.Outcome.PASSED, receipts[0].outcome); assertEquals(3, receipts[0].smokeTokens)
        assertTrue(File(dir, ".bundles-v1/gemma-4-e2b/active-v1").readText().contains(bundle.bundleId))
    }

    @Test fun smokeFailureKeepsOldPointerRestoresPreviousModelAndRecordsFailed() = withStore { dir, store ->
        val old = stage(store, "old"); val port = FakePort(); val receipts = mutableListOf<StagedActivation.Receipt>()
        assertEquals(Phase.ACTIVE, run(store, old, port, receipts).phase)
        val oldPath = port.resident!!; val pointer = File(dir, ".bundles-v1/gemma-4-e2b/active-v1").readBytes()
        val replacement = stage(store, "new")
        port.smokeOutcome = StagedActivation.SmokeOutcome.Failed("no tokens in 45000 ms")
        val state = run(store, replacement, port, receipts)
        assertEquals(Phase.FAILED, state.phase); assertTrue(state.detail.contains("previous active model retained"))
        assertArrayEquals(pointer, File(dir, ".bundles-v1/gemma-4-e2b/active-v1").readBytes())
        assertEquals(old.bundleId, store.active("gemma-4-e2b")!!.bundleId)
        assertEquals(oldPath, port.resident) // previous model reloaded after candidate unload
        assertEquals(oldPath, port.loads.last())
        assertTrue(store.verify(replacement)) // staged bundle kept for an explicit retry, never deleted
        assertEquals(NativeLoadReceipt.Outcome.FAILED, receipts.last().outcome); assertTrue(receipts.last().reason.startsWith("BAD_SMOKE"))
    }

    @Test fun loadFailureAndOomRetainOldAndRecordDistinctReasons() = withStore { _, store ->
        val old = stage(store, "old"); val port = FakePort(); val receipts = mutableListOf<StagedActivation.Receipt>()
        run(store, old, port, receipts)
        val replacement = stage(store, "new")
        port.loadOutcomes = mutableListOf(StagedActivation.LoadOutcome.Failed("engine init failed"))
        assertEquals(Phase.FAILED, run(store, replacement, port, receipts).phase)
        assertTrue(receipts.last().reason.startsWith("BAD_LOAD"))
        port.loadOutcomes = mutableListOf(StagedActivation.LoadOutcome.Failed("OutOfMemoryError: native alloc", outOfMemory = true))
        assertEquals(Phase.FAILED, run(store, replacement, port, receipts).phase)
        assertTrue(receipts.last().reason.startsWith("OOM"))
        assertEquals(old.bundleId, store.active("gemma-4-e2b")!!.bundleId)
        assertEquals(File(old.root, "model.litertlm").absolutePath, port.resident)
        assertEquals(3, receipts.size)
    }

    @Test fun freshAdmissionRefusalIsReportedWithoutTouchingNativeOrMintingEvidence() = withStore { _, store ->
        val bundle = stage(store, "v1"); val port = FakePort(); val receipts = mutableListOf<StagedActivation.Receipt>()
        val state = run(store, bundle, port, receipts) { s, l ->
            store.activate(s, object : ModelBundleStore.NativeLoader by l { override fun freshAdmissionError() = "MEMORY_PRESSURE_NOW" })
        }
        assertEquals(Phase.FAILED, state.phase); assertTrue(state.detail.contains("Fresh native admission denied"))
        assertTrue(port.loads.isEmpty()); assertEquals(0, port.smokes); assertTrue(receipts.isEmpty())
        assertNull(store.active("gemma-4-e2b")); assertTrue(store.verify(bundle))
    }

    @Test fun missingRuntimeArtifactInBundleIsBadLoadNotActive() = withStore { _, store ->
        val identity = ModelBundleStore.sha256("odd".toByteArray())
        File(store.stageDirectory("gemma-4-e2b", identity), "README").writeText("no weights")
        val bundle = store.seal("gemma-4-e2b", identity, "odd")
        val port = FakePort(); val receipts = mutableListOf<StagedActivation.Receipt>()
        assertEquals(Phase.FAILED, run(store, bundle, port, receipts).phase)
        assertTrue(port.loads.isEmpty()); assertNull(store.active("gemma-4-e2b"))
        assertTrue(receipts.single().reason.startsWith("BAD_LOAD"))
    }
}
