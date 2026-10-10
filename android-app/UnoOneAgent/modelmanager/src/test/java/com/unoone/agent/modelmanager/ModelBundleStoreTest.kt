package com.unoone.agent.modelmanager

import org.junit.Test
import org.junit.Assert.*
import java.io.File
import java.nio.file.Files

class ModelBundleStoreTest {
    private fun withStore(test: (File, ModelBundleStore) -> Unit) {
        val dir = Files.createTempDirectory("bundle-transaction").toFile()
        try { test(dir, ModelBundleStore(dir)) } finally { dir.deleteRecursively() }
    }
    private fun stage(store: ModelBundleStore, text: String): ModelBundleStore.Staged {
        val identity = ModelBundleStore.sha256(text.toByteArray())
        val dir = store.stageDirectory("test", identity)
        File(dir, "A.bin").writeText("$text-A"); File(dir, "B.bin").writeText("$text-B")
        return store.seal("test", identity, text)
    }
    private open class Loader(val outcome: ModelBundleStore.NativeResult = ModelBundleStore.NativeResult.PASSED) : ModelBundleStore.NativeLoader {
        override val retainsPreviousInstance = true
        var rollback = false; var committed = false
        override fun freshAdmissionError(): String? = null
        override fun loadAndSmoke(candidateRoot: File) = outcome
        override fun commitRouting() { committed = true }
        override fun rollbackCandidate() { rollback = true }
    }
    @Test fun activationRequiresNativeSuccessAndPreservesOldBytesAndPointerOnBadLoadSmokeOomCancel() = withStore { dir, store ->
        val old = stage(store, "old"); assertTrue(store.activate(old, Loader()))
        val pointer = File(dir, ".bundles-v1/test/active-v1"); val bytes = pointer.readBytes()
        val replacement = stage(store, "new")
        for (result in ModelBundleStore.NativeResult.entries.filter { it != ModelBundleStore.NativeResult.PASSED }) {
            val loader = Loader(result)
            assertFalse(store.activate(replacement, loader)); assertTrue(loader.rollback)
            assertArrayEquals(bytes, pointer.readBytes())
            assertEquals("old-A", File(store.active("test")!!.root, "A.bin").readText())
            assertEquals("old-B", File(old.root, "B.bin").readText())
        }
        val oom = object : Loader() { override fun loadAndSmoke(candidateRoot: File): ModelBundleStore.NativeResult { throw OutOfMemoryError("fixture") } }
        assertFalse(store.activate(replacement, oom)); assertTrue(oom.rollback)
        assertArrayEquals(bytes, pointer.readBytes())
    }
    @Test fun freshAdmissionBeforeAndAfterSmoke() = withStore { _, store ->
        val candidate = stage(store, "new")
        var calls = 0
        val loader = object : Loader() { override fun freshAdmissionError(): String? = if (++calls > 1) "MEMORY_PRESSURE" else null }
        try { store.activate(candidate, loader); fail() } catch (_: IllegalStateException) { }
        assertFalse(store.hasPointer("test")); assertTrue(loader.rollback)
    }
    @Test fun cleanupOnlyPartialAndNoImplicitRecoveryActivation() = withStore { _, store ->
        val old = stage(store, "old"); store.activate(old, Loader())
        val pendingId = "a".repeat(64)
        File(store.stageDirectory("test", pendingId), "A.bin.part").writeText("partial")
        val staged = stage(store, "pending")
        store.cleanupPartial("test", pendingId)
        assertTrue(store.verify(old)); assertTrue(store.verify(staged))
        assertEquals(old.bundleId, store.active("test")!!.bundleId)
    }
    @Test fun pruneInactiveNeverDeletesActiveOrPartialEvenWhenActiveIsTampered() = withStore { dir, store ->
        val old = stage(store, "old"); assertTrue(store.activate(old, Loader()))
        val pointer = File(dir, ".bundles-v1/test/active-v1").readBytes()
        val staged = stage(store, "staged")
        File(store.stageDirectory("test", "b".repeat(64)), "A.bin.part").writeText("partial")
        assertEquals(listOf(staged.bundleId), store.pruneInactiveBundles("test"))
        assertFalse(staged.root.exists()); assertTrue(old.root.isDirectory)
        assertEquals("old-A", File(old.root, "A.bin").readText())
        assertTrue(File(dir, ".bundles-v1/test/partial-${"b".repeat(64)}/A.bin.part").isFile)
        File(old.root, "B.bin").writeText("tampered")
        assertEquals(emptyList<String>(), store.pruneInactiveBundles("test"))
        assertTrue(File(old.root, "A.bin").isFile); assertArrayEquals(pointer, File(dir, ".bundles-v1/test/active-v1").readBytes())
        try { store.active("test"); fail() } catch (_: IllegalStateException) { }
    }
    @Test fun explicitUninstallRemovesOnlyThatIdAndNeverLegacyOrOtherModels() = withStore { dir, store ->
        val old = stage(store, "old"); assertTrue(store.activate(old, Loader()))
        File(store.stageDirectory("test", "c".repeat(64)), "A.bin.part").writeText("partial")
        val otherId = ModelBundleStore.sha256("other".toByteArray())
        File(store.stageDirectory("other", otherId), "A.bin").writeText("other-A")
        val other = store.seal("other", otherId, "other"); assertTrue(store.activate(other, Loader()))
        val legacy = File(dir, "legacy/model.bin").also { it.parentFile.mkdirs(); it.writeText("standalone-v0") }
        store.uninstallAll("test")
        assertFalse(store.hasPointer("test")); assertNull(store.active("test")); assertFalse(old.root.exists())
        assertTrue(File(dir, ".bundles-v1/test").listFiles()!!.none { it.name.startsWith("bundle-") || it.name.startsWith("partial-") })
        assertEquals(other.bundleId, store.active("other")!!.bundleId); assertEquals("other-A", File(other.root, "A.bin").readText())
        assertEquals("standalone-v0", legacy.readText())
    }
    @Test fun staleOrTamperedBundleAndRoutingFailureCannotReplaceOld() = withStore { dir, store ->
        val old = stage(store, "old"); store.activate(old, Loader())
        val new = stage(store, "new")
        val pointer = File(dir, ".bundles-v1/test/active-v1").readBytes()
        val routing = object : Loader() { override fun commitRouting() { error("routing unavailable") } }
        try { store.activate(new, routing); fail() } catch (_: IllegalStateException) { }
        assertArrayEquals(pointer, File(dir, ".bundles-v1/test/active-v1").readBytes())
        File(new.root, "A.bin").writeText("evil")
        assertFalse(store.verify(new))
        try { store.activate(new, Loader()); fail() } catch (_: IllegalStateException) { }
        assertEquals(old.bundleId, store.active("test")!!.bundleId)
    }
    @Test fun standaloneBinLayoutIsNotMovedOrOverwritten() = withStore { dir, store ->
        val legacy = File(dir, "legacy/model.bin").also { it.parentFile.mkdirs(); it.writeText("standalone-v0") }
        stage(store, "new")
        assertEquals("standalone-v0", legacy.readText()); assertNull(store.active("test"))
    }
    @Test fun childProcessDeathPartialSealedAndLoadingKeepsExactOldPointer() = withStore { dir, store ->
        val old = stage(store, "old"); store.activate(old, Loader())
        val pointer = File(dir, ".bundles-v1/test/active-v1").readBytes()
        for (phase in listOf("partial", "sealed", "loading")) {
            val child = ProcessBuilder("${System.getProperty("java.home")}/bin/java", "-cp", System.getProperty("java.class.path"),
                "com.unoone.agent.modelmanager.BundleDeathFixture", dir.absolutePath, phase).inheritIO().start()
            assertEquals(73, child.waitFor())
            val reopened = ModelBundleStore(dir)
            assertArrayEquals(pointer, File(dir, ".bundles-v1/test/active-v1").readBytes())
            assertEquals(old.bundleId, reopened.active("test")!!.bundleId)
            assertEquals("old-A", File(old.root, "A.bin").readText())
            assertEquals("old-B", File(old.root, "B.bin").readText())
        }
    }
}

/** Real abrupt process exit; synthetic loader proves filesystem sequencing only, not JNI inference. */
object BundleDeathFixture {
    @JvmStatic fun main(args: Array<String>) {
        val store = ModelBundleStore(File(args[0])); val phase = args[1]
        val id = ModelBundleStore.sha256(phase.toByteArray())
        File(store.stageDirectory("test", id), "A.bin").writeText("new-A")
        if (phase == "partial") Runtime.getRuntime().halt(73)
        File(store.stageDirectory("test", id), "B.bin").writeText("new-B")
        val stage = store.seal("test", id, phase)
        if (phase == "sealed") Runtime.getRuntime().halt(73)
        store.activate(stage, object : ModelBundleStore.NativeLoader {
            override val retainsPreviousInstance = true
            override fun freshAdmissionError(): String? = null
            override fun loadAndSmoke(candidateRoot: File): ModelBundleStore.NativeResult { Runtime.getRuntime().halt(73); error("unreachable") }
            override fun commitRouting() = error("must not activate")
            override fun rollbackCandidate() { }
        })
    }
}
