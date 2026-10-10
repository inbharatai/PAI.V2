package com.unoone.agent.modelmanager

import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import org.junit.After
import org.junit.Before
import org.junit.Test
import org.junit.Assert.*
import java.io.*
import java.net.ServerSocket
import java.net.Socket
import java.nio.file.Files
import java.security.MessageDigest
import java.util.concurrent.atomic.AtomicInteger
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream
import org.apache.commons.compress.archivers.tar.TarArchiveEntry
import org.apache.commons.compress.archivers.tar.TarArchiveOutputStream
import org.apache.commons.compress.compressors.bzip2.BZip2CompressorOutputStream

/** Actual filesystem + loopback HTTP tests. All positive admissions here are synthetic test grants. */
class ModelInstallerTest {
    private lateinit var base: File
    private lateinit var store: ModelBundleStore
    @Before fun setup() { base = Files.createTempDirectory("installer-bundle").toFile(); store = ModelBundleStore(base) }
    @After fun cleanup() { base.deleteRecursively() }
    private fun hash(b: ByteArray) = ModelBundleStore.sha256(b)
    private fun descriptor(vararg files: ModelFile) = ModelDescriptor("m", "legacy/m", ModelType.llm, "v1", files = files.toList())
    private fun asset(name: String, b: ByteArray) = ModelFile(name, "", hash(b), b.size.toLong(), asset = name)
    private fun partial(d: ModelDescriptor) = store.stageDirectory(d.id, hash(Json.encodeToString(ModelDescriptor.serializer(), d).toByteArray()))
    private fun run(d: ModelDescriptor, installer: ModelInstaller = ModelInstaller(base.path), cancel: () -> Boolean = { false }, guard: (() -> String?)? = { null }): ModelInstaller.InstallResult =
        runBlocking { installer.install(d, shouldCancel = cancel, admission = guard) }
    private fun staged(result: ModelInstaller.InstallResult): File {
        assertTrue(result.toString(), result is ModelInstaller.InstallResult.Staged)
        val bundle = (result as ModelInstaller.InstallResult.Staged).bundle
        assertTrue(store.verify(bundle)); assertNull(store.active("m")); return bundle.root
    }
    private class Loader : ModelBundleStore.NativeLoader {
        override val retainsPreviousInstance = true
        override fun freshAdmissionError(): String? = null
        override fun loadAndSmoke(candidateRoot: File) = ModelBundleStore.NativeResult.PASSED
        override fun commitRouting() { }
        override fun rollbackCandidate() { }
    }
    @Test fun actualOldABNewASucceedsNewBFailsPreservesExactBundleAndPointer() {
        val old = mapOf("A.bin" to "old-A".toByteArray(), "B.bin" to "old-B".toByteArray())
        val installer = ModelInstaller(base.path) { old[it]?.inputStream() }
        val before = run(descriptor(*old.map { asset(it.key, it.value) }.toTypedArray()), installer) as ModelInstaller.InstallResult.Staged
        assertTrue(store.activate(before.bundle, Loader()))
        val pointer = File(base, ".bundles-v1/m/active-v1"); val pointerBytes = pointer.readBytes()
        val fresh = mapOf("A.bin" to "new-A".toByteArray(), "B.bin" to "wrong-B".toByteArray())
        var readA = false
        val replacement = ModelInstaller(base.path) { name -> if (name == "A.bin") readA = true; fresh[name]?.inputStream() }
        val wanted = descriptor(asset("A.bin", fresh.getValue("A.bin")), asset("B.bin", "new-B".toByteArray())).copy(version = "v2")
        assertTrue(run(wanted, replacement) is ModelInstaller.InstallResult.Failure)
        assertTrue(readA)
        assertArrayEquals(fresh.getValue("A.bin"), File(partial(wanted), "A.bin").readBytes())
        assertArrayEquals(pointerBytes, pointer.readBytes())
        old.forEach { (name, bytes) -> assertArrayEquals(bytes, File(before.bundle.root, name).readBytes()) }
        assertEquals(before.bundle.bundleId, ModelBundleStore(base).active("m")!!.bundleId)
        store.cleanupPartial("m", hash(Json.encodeToString(ModelDescriptor.serializer(), wanted).toByteArray()))
        old.forEach { (name, bytes) -> assertArrayEquals(bytes, File(before.bundle.root, name).readBytes()) }
    }
    @Test fun ioFailureMidArtifactLikeEnospcKeepsOldActiveAndSealsNothing() {
        val old = mapOf("A.bin" to "old-A".toByteArray(), "B.bin" to "old-B".toByteArray())
        val before = run(descriptor(*old.map { asset(it.key, it.value) }.toTypedArray()), ModelInstaller(base.path) { old[it]?.inputStream() }) as ModelInstaller.InstallResult.Staged
        assertTrue(store.activate(before.bundle, Loader()))
        val pointer = File(base, ".bundles-v1/m/active-v1").readBytes()
        val newB = "new-B".toByteArray()
        val failing = ModelInstaller(base.path) { name ->
            if (name == "A.bin") object : InputStream() {
                var n = 0
                override fun read(): Int { if (n++ < 2) return 'x'.code; throw IOException("No space left on device") }
            } else newB.inputStream()
        }
        val wanted = descriptor(asset("A.bin", "new-A".toByteArray()), asset("B.bin", newB)).copy(version = "v2")
        assertTrue(run(wanted, failing) is ModelInstaller.InstallResult.Failure)
        assertArrayEquals(pointer, File(base, ".bundles-v1/m/active-v1").readBytes())
        old.forEach { (name, bytes) -> assertArrayEquals(bytes, File(before.bundle.root, name).readBytes()) }
        assertFalse(File(partial(wanted), "A.bin").exists())
        assertEquals(before.bundle.bundleId, ModelBundleStore(base).active("m")!!.bundleId)
        assertEquals(1, File(base, ".bundles-v1/m").listFiles()!!.count { it.name.startsWith("bundle-") })
    }
    @Test fun stalePartBesideVerifiedStagedFileStillSeals() {
        val bytes = "payload".toByteArray()
        val d = descriptor(asset("model.bin", bytes))
        File(partial(d), "model.bin").writeBytes(bytes); File(partial(d), "model.bin.part").writeText("stale")
        var reads = 0
        val root = staged(run(d, ModelInstaller(base.path) { reads++; bytes.inputStream() }))
        assertEquals(0, reads); assertFalse(File(root, "model.bin.part").exists()); assertArrayEquals(bytes, File(root, "model.bin").readBytes())
    }
    @Test fun missingOrDeniedAdmissionStartsNoAssetBytes() {
        val bytes = "payload".toByteArray(); var reads = 0
        val installer = ModelInstaller(base.path) { reads++; bytes.inputStream() }
        val d = descriptor(asset("model.bin", bytes))
        assertTrue(run(d, installer, guard = null) is ModelInstaller.InstallResult.Failure)
        for (reason in listOf("UNKNOWN_PROBE", "MEMORY_PRESSURE", "PERMANENT_MEMORY_MISFIT", "POLICY_EXPIRED", "DISABLED", "METERED", "STALE_PROBE"))
            assertTrue(run(d, installer, guard = { reason }) is ModelInstaller.InstallResult.Failure)
        assertEquals(0, reads)
    }
    @Test fun policyRevokedBetweenArtifactsNeverReadsSecond() {
        val bytes = "payload".toByteArray(); var reads = 0; var revoked = false
        val installer = ModelInstaller(base.path) { name -> reads++; if (name == "A.bin") revoked = true; bytes.inputStream() }
        val d = descriptor(asset("A.bin", bytes), asset("B.bin", bytes))
        assertTrue(run(d, installer, guard = { if (revoked) "REVOKED" else null }) is ModelInstaller.InstallResult.Failure)
        assertEquals(1, reads); assertNull(store.active("m"))
    }
    @Test fun downloadVerifyAndResumeActualHttpRange() {
        for (supports in listOf(false, true)) {
            val bytes = "the quick brown fox jumps over the lazy dog".toByteArray()
            val server = MiniHttpServer(bytes, supports).apply { start() }
            try {
                val d = descriptor(ModelFile("model.bin", server.url("model.bin"), hash(bytes), bytes.size.toLong())).copy(version = supports.toString())
                File(partial(d), "model.bin.part").writeBytes(bytes.copyOfRange(0, 10))
                val root = staged(run(d)); assertArrayEquals(bytes, File(root, "model.bin").readBytes())
                assertFalse(File(root, "model.bin.part").exists())
            } finally { server.stop() }
        }
    }
    @Test fun mismatchedRangePreservesPartial() {
        val bytes = "a long expected payload".toByteArray(); val server = MiniHttpServer(bytes, true, 1).apply { start() }
        try {
            val d = descriptor(ModelFile("model.bin", server.url("model.bin"), hash(bytes), bytes.size.toLong()))
            val part = File(partial(d), "model.bin.part").apply { writeBytes(bytes.copyOfRange(0, 5)) }
            assertTrue(run(d) is ModelInstaller.InstallResult.Failure); assertArrayEquals(bytes.copyOfRange(0, 5), part.readBytes())
        } finally { server.stop() }
    }
    @Test fun completePartCommitsWithoutNetworkAndIdempotentSealedReuse() {
        val bytes = "complete".toByteArray()
        val d = descriptor(ModelFile("model.bin", "http://invalid.invalid/model", hash(bytes), bytes.size.toLong()))
        File(partial(d), "model.bin.part").writeBytes(bytes)
        val first = run(d) as ModelInstaller.InstallResult.Staged
        assertArrayEquals(bytes, File(first.bundle.root, "model.bin").readBytes())
        val again = run(d) as ModelInstaller.InstallResult.Staged
        assertEquals(first.bundle.bundleId, again.bundle.bundleId)
    }
    @Test fun oversizedPartRejectedAndOrdinaryCancelRetained() {
        val bytes = "expected".toByteArray()
        val d = descriptor(ModelFile("model.bin", "http://invalid.invalid/model", hash(bytes), bytes.size.toLong()))
        val part = File(partial(d), "model.bin.part").apply { writeText("oversized content") }
        assertTrue(run(d) is ModelInstaller.InstallResult.Failure); assertFalse(part.exists())
        part.writeText("part")
        assertTrue(run(d, cancel = { true }) is ModelInstaller.InstallResult.Failure)
        assertEquals("part", part.readText())
    }
    @Test fun connectionFailureRetryableWrongSizeOrHashCannotSeal() {
        val port = ServerSocket(0).use { it.localPort }
        val d = descriptor(ModelFile("model.bin", "http://127.0.0.1:$port/model", "0".repeat(64), 100))
        val part = File(partial(d), "model.bin.part").apply { writeText("partial") }
        val failure = run(d) as ModelInstaller.InstallResult.Failure
        assertTrue(failure.retryable); assertEquals("partial", part.readText())
        val server = MiniHttpServer("bad".toByteArray(), false).apply { start() }
        try { assertTrue(run(d.copy(version = "bad", files = listOf(d.files.single().copy(url = server.url("bad"))))) is ModelInstaller.InstallResult.Failure) }
        finally { server.stop() }
        assertNull(store.active("m"))
    }
    @Test fun incompleteIntegrityNeverFetchesOrOverwritesStandaloneBin() {
        val old = File(base, "legacy/m/model.bin").also { it.parentFile.mkdirs(); it.writeText("standalone") }
        var reads = 0; val installer = ModelInstaller(base.path) { reads++; "new".byteInputStream() }
        assertTrue(run(descriptor(ModelFile("model.bin", "", asset = "model.bin")), installer) is ModelInstaller.InstallResult.Failure)
        assertEquals(0, reads); assertEquals("standalone", old.readText())
    }
    @Test fun assetMissingAndCorruptAssetFailWithoutActivation() {
        val d = descriptor(asset("model.bin", "right".toByteArray()))
        assertTrue(run(d, ModelInstaller(base.path) { null }) is ModelInstaller.InstallResult.Failure)
        assertTrue(run(d, ModelInstaller(base.path) { "wrong".byteInputStream() }) is ModelInstaller.InstallResult.Failure)
        assertNull(store.active("m"))
    }
    @Test fun verifiedZipExtractionAndInventoryRejectsMutationExtraMissingAndSymlink() {
        val bytes = archiveZip("pkg/model" to "correct")
        val f = ModelFile("pkg.zip", "", hash(bytes), bytes.size.toLong(), true, "pkg.zip")
        var reads = 0; val installer = ModelInstaller(base.path) { reads++; bytes.inputStream() }
        val d = descriptor(f); val root = staged(run(d, installer))
        assertFalse(File(root, "pkg.zip").exists()); assertTrue(installer.archiveAlreadyExtracted(f, root))
        assertTrue(run(d, installer) is ModelInstaller.InstallResult.Staged); assertEquals(1, reads)
        val payload = File(root, "pkg/model")
        payload.writeText("CORRUPT"); assertFalse(installer.archiveAlreadyExtracted(f, root)); payload.writeText("correct")
        File(root, "pkg/extra").writeText("extra"); assertFalse(installer.archiveAlreadyExtracted(f, root)); File(root, "pkg/extra").delete()
        payload.delete(); assertFalse(installer.archiveAlreadyExtracted(f, root))
        val outside = File(base, "outside").apply { writeText("correct") }
        Files.createSymbolicLink(payload.toPath(), outside.toPath()); assertFalse(installer.archiveAlreadyExtracted(f, root))
    }
    @Test fun badArchivePathsRootsAndUnpinnedArchivePreserveLivePayload() {
        val legacy = File(base, "legacy/m/pkg/model").also { it.parentFile.mkdirs(); it.writeText("old") }
        for (entry in listOf("../escape", "other/foreign")) {
            val bytes = archiveZip("pkg/model" to "new", entry to "bad")
            val f = ModelFile("pkg.zip", "", hash(bytes), bytes.size.toLong(), true, "pkg.zip")
            assertTrue(run(descriptor(f), ModelInstaller(base.path) { bytes.inputStream() }) is ModelInstaller.InstallResult.Failure)
            assertEquals("old", legacy.readText())
        }
        val f = ModelFile("pkg.zip", "", archive = true, asset = "pkg.zip")
        var reads = 0
        assertTrue(run(descriptor(f), ModelInstaller(base.path) { reads++; null }) is ModelInstaller.InstallResult.Failure)
        assertEquals(0, reads); assertEquals("old", legacy.readText())
    }
    @Test fun tarBz2ExtractsToExactRoot() {
        val bytes = tarBz2(mapOf("speech/encoder" to "ENC".toByteArray(), "speech/tokens" to "TOK".toByteArray()))
        val f = ModelFile("pkg.tar.bz2", "", hash(bytes), bytes.size.toLong(), true, "pkg.tar.bz2", "speech")
        val root = staged(run(descriptor(f), ModelInstaller(base.path) { bytes.inputStream() }))
        assertEquals("ENC", File(root, "speech/encoder").readText()); assertEquals("TOK", File(root, "speech/tokens").readText())
        assertFalse(File(root, "pkg.tar.bz2").exists())
    }
    private fun archiveZip(vararg entries: Pair<String, String>): ByteArray = ByteArrayOutputStream().also { out ->
        ZipOutputStream(out).use { zip -> entries.forEach { (name, value) -> zip.putNextEntry(ZipEntry(name)); zip.write(value.toByteArray()); zip.closeEntry() } }
    }.toByteArray()
    private fun tarBz2(entries: Map<String, ByteArray>): ByteArray = ByteArrayOutputStream().also { out ->
        BZip2CompressorOutputStream(out).use { bz -> TarArchiveOutputStream(bz).use { tar -> entries.forEach { (name, value) ->
            val entry = TarArchiveEntry(name); entry.size = value.size.toLong(); tar.putArchiveEntry(entry); tar.write(value); tar.closeArchiveEntry()
        } } }
    }.toByteArray()
    /** Minimal HTTP/1.1 server over a plain socket serving [body], optionally honouring Range. */
    private class MiniHttpServer(
        private val body: ByteArray,
        private val supportRange: Boolean,
        private val contentRangeStartDelta: Int = 0
    ) {
        private val server = ServerSocket(0)
        private val thread = Thread { runServer() }
        private var stopped = false

        fun start() { thread.start() }
        fun stop() {
            stopped = true
            try { server.close() } catch (_: IOException) {}
        }
        fun url(path: String): String = "http://localhost:${server.localPort}/$path"

        private fun runServer() {
            try {
                while (!stopped && !server.isClosed) {
                    val socket = try { server.accept() } catch (_: IOException) { break }
                    try { handle(socket) } catch (_: IOException) {} finally { try { socket.close() } catch (_: IOException) {} }
                }
            } catch (_: IOException) {}
        }

        private fun handle(socket: Socket) {
            val input = socket.getInputStream()
            val reader = BufferedReader(InputStreamReader(input))
            // request line
            reader.readLine() ?: return
            val headers = HashMap<String, String>()
            var line = reader.readLine()
            while (line != null && line.isNotEmpty()) {
                val idx = line.indexOf(':')
                if (idx > 0) headers[line.substring(0, idx).trim().lowercase()] = line.substring(idx + 1).trim()
                line = reader.readLine()
            }
            val range = headers["range"]
            val out: OutputStream = socket.getOutputStream()
            if (supportRange && range != null && range.startsWith("bytes=")) {
                val from = range.removePrefix("bytes=").substringBefore('-').toInt()
                if (from in 0 until body.size) {
                    val slice = body.copyOfRange(from, body.size)
                    val reportedStart = from + contentRangeStartDelta
                    writeResponse(out, "206 Partial Content", slice,
                        extra = "Content-Range: bytes $reportedStart-${body.size - 1}/${body.size}\r\n")
                } else {
                    writeResponse(out, "416 Range Not Satisfiable", ByteArray(0), extra = "")
                }
            } else {
                writeResponse(out, "200 OK", body, extra = "")
            }
            out.flush()
        }

        private fun writeResponse(out: OutputStream, status: String, payload: ByteArray, extra: String) {
            val header = "HTTP/1.1 $status\r\nContent-Length: ${payload.size}\r\nConnection: close\r\n$extra\r\n"
            out.write(header.toByteArray())
            if (payload.isNotEmpty()) out.write(payload)
        }
    }
}
