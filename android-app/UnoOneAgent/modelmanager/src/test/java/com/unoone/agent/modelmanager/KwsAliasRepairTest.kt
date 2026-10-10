package com.unoone.agent.modelmanager

import java.io.File
import java.nio.file.Files
import org.junit.Assert.*
import org.junit.Test

/** Regression: the integrated tree returned `false` unconditionally and never materialised the KWS alias. */
class KwsAliasRepairTest {
    private fun file(name: String, bytes: ByteArray) = ModelFile(name, "https://example.invalid/$name", ModelBundleStore.sha256(bytes), bytes.size.toLong())
    private val encoder = "encoder-bytes".toByteArray(); private val tokens = "tokens-bytes".toByteArray()
    private val files = listOf(file("encoder.onnx", encoder), file("tokens.txt", tokens))
    private val asr = ModelDescriptor("sherpa-asr-en", "speech/shared/sherpa-asr-en", ModelType.asr, "1", files = files)
    private val kws = ModelDescriptor("sherpa-kws-en", "speech/shared/sherpa-kws-en", ModelType.kws, "1", files = files)
    private fun withRoot(test: (File) -> Unit) { val d = Files.createTempDirectory("kws").toFile(); try { test(d) } finally { d.deleteRecursively() } }

    @Test fun identicalManifestsAreRecognisedAndDifferentOnesRefused() {
        assertTrue(KwsAliasRepair.manifestsIdentical(asr, kws))
        assertFalse(KwsAliasRepair.manifestsIdentical(asr, kws.copy(files = listOf(files[0]))))
        assertFalse(KwsAliasRepair.manifestsIdentical(asr, kws.copy(files = listOf(files[0], files[1].copy(sha256 = "0".repeat(64))))))
        assertFalse(KwsAliasRepair.manifestsIdentical(asr.copy(files = emptyList()), kws.copy(files = emptyList())))
    }

    @Test fun verifiedSourceIsCopiedIntoTheKwsFolderAndVerifies() = withRoot { root ->
        val source = File(root, asr.folder).apply { mkdirs() }
        File(source, "encoder.onnx").writeBytes(encoder); File(source, "tokens.txt").writeBytes(tokens)
        val target = File(root, kws.folder)
        assertTrue(KwsAliasRepair.copyVerified(source, target, kws.files))
        assertArrayEquals(encoder, File(target, "encoder.onnx").readBytes())
        assertArrayEquals(tokens, File(target, "tokens.txt").readBytes())
        assertTrue(target.listFiles()!!.none { it.name.endsWith(".part") })
        assertTrue(kws.files.all { KwsAliasRepair.verified(File(target, it.name), it) })
        // Source untouched; idempotent second run copies nothing new.
        assertArrayEquals(encoder, File(source, "encoder.onnx").readBytes())
        val before = File(target, "encoder.onnx").lastModified()
        assertTrue(KwsAliasRepair.copyVerified(source, target, kws.files))
        assertEquals(before, File(target, "encoder.onnx").lastModified())
    }

    @Test fun tamperedSourceOrMissingFileLeavesNoPartialAndKeepsVerifiedFiles() = withRoot { root ->
        val source = File(root, asr.folder).apply { mkdirs() }
        File(source, "encoder.onnx").writeBytes(encoder); File(source, "tokens.txt").writeBytes("tampered".toByteArray())
        val target = File(root, kws.folder)
        assertFalse(KwsAliasRepair.copyVerified(source, target, kws.files))
        assertTrue(File(target, "encoder.onnx").isFile) // verified file retained
        assertFalse(File(target, "tokens.txt").exists()); assertFalse(File(target, "tokens.txt.part").exists())
        File(source, "tokens.txt").delete()
        assertFalse(KwsAliasRepair.copyVerified(source, target, kws.files))
        assertTrue(target.listFiles()!!.none { it.name.endsWith(".part") })
    }
}
