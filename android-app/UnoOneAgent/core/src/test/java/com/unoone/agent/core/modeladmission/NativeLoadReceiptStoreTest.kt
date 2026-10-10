package com.unoone.agent.core.modeladmission

import java.io.File
import java.nio.file.Files
import org.junit.Assert.*
import org.junit.Test

class NativeLoadReceiptStoreTest {
    private val identity = "a".repeat(64)
    private val device = "b".repeat(64)
    private val otherDevice = "c".repeat(64)
    private fun receipt(outcome: NativeLoadReceipt.Outcome, at: Long, fingerprint: String = device, reason: String = "") =
        NativeLoadReceipt(modelId = "gemma-4-e4b", identity = identity, deviceFingerprint = fingerprint, outcome = outcome,
            reason = reason, backend = "CPU", loadMs = 1259, smokeMs = 800, smokeTokens = 3, recordedAtMs = at, lane = "EXPLICIT_LOAD")
    private fun withStore(test: (File, FileReceiptStore) -> Unit) {
        val dir = Files.createTempDirectory("receipts").toFile()
        try { test(dir, FileReceiptStore(dir)) } finally { dir.deleteRecursively() }
    }

    @Test fun recordsAndReadsBackOnlyThisDeviceAndIdentity() = withStore { dir, store ->
        store.record(receipt(NativeLoadReceipt.Outcome.PASSED, 1000))
        store.record(receipt(NativeLoadReceipt.Outcome.PASSED, 2000, fingerprint = otherDevice))
        val file = File(dir, ".receipts-v1/gemma-4-e4b/$identity.jsonl")
        assertTrue(file.isFile); assertEquals(2, file.readLines().size)
        assertEquals(1, store.receipts("gemma-4-e4b", identity, device).size)
        assertTrue(store.receipts("gemma-4-e4b", "d".repeat(64), device).isEmpty())
        val evidence = store.evidence("gemma-4-e4b", identity, device)
        assertTrue(evidence.everPassedHere); assertEquals(1000L, evidence.latestReceipt?.recordedAtMs)
        assertEquals(LoadAdmission.EvidenceState.WORKS_HERE, LoadAdmission.evidenceState(evidence))
        assertEquals(LoadAdmission.EvidenceState.UNKNOWN, LoadAdmission.evidenceState(store.evidence("gemma-4-e4b", identity, "e".repeat(64))))
    }

    @Test fun latestFailureDowngradesToUnknownButHistoryIsKept() = withStore { _, store ->
        store.record(receipt(NativeLoadReceipt.Outcome.PASSED, 1000))
        store.record(receipt(NativeLoadReceipt.Outcome.FAILED, 3000, reason = "OOM:native alloc"))
        val evidence = store.evidence("gemma-4-e4b", identity, device)
        assertTrue(evidence.everPassedHere); assertFalse(evidence.latestReceipt!!.passed)
        assertEquals(LoadAdmission.EvidenceState.UNKNOWN, LoadAdmission.evidenceState(evidence))
    }

    @Test fun corruptLinesAreSkippedNeverRepairedAndIdsAreValidated() = withStore { dir, store ->
        store.record(receipt(NativeLoadReceipt.Outcome.PASSED, 1000))
        val file = File(dir, ".receipts-v1/gemma-4-e4b/$identity.jsonl")
        file.appendText("{not json\n")
        assertEquals(1, store.receipts("gemma-4-e4b", identity, device).size)
        assertEquals(2, file.readLines().size)
        try { store.receipts("../escape", identity, device); fail() } catch (_: IllegalArgumentException) {}
        try { store.record(receipt(NativeLoadReceipt.Outcome.PASSED, 1, fingerprint = "short")); fail() } catch (_: IllegalArgumentException) {}
    }
}
