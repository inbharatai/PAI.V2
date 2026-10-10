package com.unoone.agent.vault

import java.io.File
import java.util.concurrent.CancellationException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import kotlinx.serialization.json.*
import org.junit.Assert.*
import org.junit.Test

/** Must run with the ACTUAL host JNI artifact; never substitutes fake crypto. */
class NativeVaultKdfTest {
    private fun root(): File {
        var dir = File(requireNotNull(System.getProperty("user.dir")))
        repeat(8) {
            if (File(dir, "packages/vault-core/test-vectors/vault-cross-platform.json").isFile) return dir
            dir = dir.parentFile ?: dir
        }
        error("Shared vectors not found")
    }
    @Test fun `actual JNI agrees with unchanged shared Rust and Kotlin golden vector`() {
        assertTrue("Real JNI library required", NativeVaultKdf.nativeAvailable)
        val vectors = Json.parseToJsonElement(File(root(), "packages/vault-core/test-vectors/vault-cross-platform.json").readText()).jsonObject
        for (entry in vectors.getValue("kdf").jsonArray) {
            val v = entry.jsonObject
            val salt = v.getValue("salt_hex").jsonPrimitive.content.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
            val password = v.getValue("password_utf8").jsonPrimitive.content.toByteArray()
            val result = VaultCrypto.deriveKek(password, salt)
            try { assertEquals(v.getValue("expected_key_hex").jsonPrimitive.content, VaultCrypto.run { result.toHex() }) }
            finally { result.fill(0); password.fill(0) }
        }
    }
    @Test fun `RAM threshold low-memory length and native error checks fail closed`() {
        val n = NativeVaultKdf.REQUIRED_AVAILABLE_BYTES
        NativeVaultKdf.admit(NativeVaultKdf.Memory(n, false))
        NativeVaultKdf.admit(NativeVaultKdf.Memory(n + 100, false, 100))
        for (s in listOf(NativeVaultKdf.Memory(n - 1, false), NativeVaultKdf.Memory(Long.MAX_VALUE, true),
            NativeVaultKdf.Memory(n, false, 1), NativeVaultKdf.Memory(n, false, -1))) {
            assertThrows(IllegalStateException::class.java) { NativeVaultKdf.admit(s) }
        }
        assertThrows(IllegalArgumentException::class.java) { VaultCrypto.deriveKek(ByteArray(4097), ByteArray(32)) }
        assertThrows(IllegalArgumentException::class.java) { VaultCrypto.deriveKek(ByteArray(1), ByteArray(31)) }
        assertTrue(NativeVaultKdf.nativeAvailable)
        assertThrows(IllegalStateException::class.java) { NativeVaultKdf.deriveNative(ByteArray(0), ByteArray(32), n, false) }
        assertThrows(IllegalStateException::class.java) { NativeVaultKdf.deriveNative(ByteArray(4097), ByteArray(32), n, false) }
        assertThrows(IllegalStateException::class.java) { NativeVaultKdf.deriveNative(ByteArray(1), ByteArray(31), n, false) }
        assertThrows(IllegalStateException::class.java) { NativeVaultKdf.deriveNative(ByteArray(1), ByteArray(32), n - 1, false) }
        assertThrows(IllegalStateException::class.java) { NativeVaultKdf.deriveNative(ByteArray(1), ByteArray(32), n, true) }
        assertThrows(IllegalStateException::class.java) { NativeVaultKdf.deriveNative(ByteArray(1), ByteArray(32), -1, false) }
    }
    @Test fun `interrupted actual JNI key and malformed result buffers are wiped`() {
        assertTrue(NativeVaultKdf.nativeAvailable)
        val password = "synthetic key wiping test".toByteArray()
        val out = NativeVaultKdf.deriveNative(password, ByteArray(32), NativeVaultKdf.REQUIRED_AVAILABLE_BYTES, false)
        try {
            assertTrue(out.any { it != 0.toByte() })
            Thread.currentThread().interrupt()
            assertThrows(CancellationException::class.java) { NativeVaultKdf.finishDerivation(out) }
            assertTrue(out.all { it == 0.toByte() })
        } finally { Thread.interrupted(); out.fill(0); password.fill(0) }
        val malformed = ByteArray(31) { 1 }
        assertThrows(IllegalStateException::class.java) { NativeVaultKdf.finishDerivation(malformed) }
        assertTrue(malformed.all { it == 0.toByte() })
    }

    @Test fun `interrupt during actual native derive retains gate until return`() {
        assertTrue(NativeVaultKdf.nativeAvailable)
        val failure = AtomicReference<Throwable?>()
        val password = "synthetic active native interruption".toByteArray()
        val worker = Thread {
            try { VaultCrypto.deriveKek(password, ByteArray(32)).fill(0) }
            catch (t: Throwable) { failure.set(t) }
        }
        try {
            worker.start()
            val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(10)
            var observed = false
            while (worker.isAlive && System.nanoTime() < deadline) {
                if (worker.stackTrace.any { it.methodName == "deriveNative" && it.isNativeMethod }) {
                    observed = true; break
                }
                Thread.sleep(1)
            }
            assertTrue("Must observe actual JNI frame, not a fake/pending admission", observed)
            worker.interrupt()
            assertTrue("Interrupt must not preempt synchronous Argon2", worker.isAlive)
            assertThrows(IllegalStateException::class.java) { VaultCrypto.deriveKek(byteArrayOf(1), ByteArray(32)) }
            // Bypass Kotlin to prove the Rust gate also rejects concurrent JNI.
            assertThrows(IllegalStateException::class.java) {
                NativeVaultKdf.deriveNative(byteArrayOf(1), ByteArray(32), NativeVaultKdf.REQUIRED_AVAILABLE_BYTES, false)
            }
            worker.join(60000)
            assertFalse(worker.isAlive)
            assertTrue(failure.get() is CancellationException)
            // Gate released only after actual work finishes; valid retry succeeds.
            VaultCrypto.deriveKek(password, ByteArray(32)).fill(0)
        } finally { worker.join(60000); password.fill(0) }
    }

    @Test fun `preinterrupted calls do not enter native work`() {
        Thread.currentThread().interrupt()
        try { assertThrows(CancellationException::class.java) { VaultCrypto.deriveKek(byteArrayOf(1), ByteArray(32)) } }
        finally { Thread.interrupted() }
    }
    @Test fun `singleflight rejects another caller and interruption waits for actual KDF return`() {
        assertTrue(NativeVaultKdf.nativeAvailable)
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>()
        // Pause admission while the real dispatcher owns its gate. The first
        // call subsequently runs real JNI and its interrupted output is discarded.
        NativeVaultKdf.installAndroidMemoryProbe {
            entered.countDown()
            var interrupted = false
            while (true) {
                try { check(release.await(20, TimeUnit.SECONDS)); break }
                catch (_: InterruptedException) { interrupted = true }
            }
            if (interrupted) Thread.currentThread().interrupt()
            NativeVaultKdf.Memory(1024L * 1024 * 1024, false)
        }
        val worker = Thread {
            try { VaultCrypto.deriveKek("synthetic interrupt test".toByteArray(), ByteArray(32)).fill(0) }
            catch (t: Throwable) { failure.set(t) }
        }
        try {
            worker.start()
            check(entered.await(10, TimeUnit.SECONDS))
            assertThrows(IllegalStateException::class.java) { VaultCrypto.deriveKek(byteArrayOf(1), ByteArray(32)) }
            worker.interrupt(); release.countDown(); worker.join(60000)
            assertFalse(worker.isAlive)
            assertTrue(failure.get() is CancellationException)
        } finally {
            release.countDown(); worker.join(60000)
            // Restore actual live host memory, not a persistent generous override.
            NativeVaultKdf.installAndroidMemoryProbe(null)
        }
    }
}
