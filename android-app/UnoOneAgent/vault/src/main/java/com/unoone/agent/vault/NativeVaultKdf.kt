package com.unoone.agent.vault

import java.io.File
import java.util.concurrent.CancellationException
import java.util.concurrent.locks.ReentrantLock
import org.bouncycastle.crypto.generators.Argon2BytesGenerator
import org.bouncycastle.crypto.params.Argon2Parameters

/** Transport/admission only; native crypto is the existing public vault-core KDF.
 * No native failure retries through Java. Missing library permits only the SAME
 * Java KDF and only with adequate heap AND current system RAM. Android builds
 * require a real arm64 artifact; this fallback is not a packaging substitute.
 */
internal object NativeVaultKdf {
    internal const val REQUIRED_AVAILABLE_BYTES = 384L * 1024 * 1024
    internal data class Memory(val availableBytes: Long, val lowMemory: Boolean, val thresholdBytes: Long = 0)
    private val flight = ReentrantLock()
    @Volatile private var androidMemory: (() -> Memory)? = null
    private val isAndroid = System.getProperty("java.vm.name") == "Dalvik"
    internal val nativeAvailable: Boolean = try {
        System.loadLibrary("unoone_android_vault_jni")
        true
    } catch (_: UnsatisfiedLinkError) { false }

    internal fun installAndroidMemoryProbe(probe: (() -> Memory)?) { androidMemory = probe }

    private fun memory(): Memory {
        androidMemory?.let { return it() }
        check(!isAndroid) { "Vault memory admission is unavailable; retry after app initialization" }
        // Real Linux JVM integration tests, not an Android RAM stand-in.
        val kb = File("/proc/meminfo").useLines { lines ->
            lines.firstOrNull { it.startsWith("MemAvailable:") }
                ?.split(Regex("\\s+"))?.getOrNull(1)?.toLongOrNull()
        } ?: error("Cannot determine current available RAM")
        return Memory(Math.multiplyExact(kb, 1024), false)
    }

    internal fun admit(snapshot: Memory) {
        check(!snapshot.lowMemory && snapshot.availableBytes >= REQUIRED_AVAILABLE_BYTES &&
            snapshot.thresholdBytes >= 0 &&
            snapshot.availableBytes - REQUIRED_AVAILABLE_BYTES >= snapshot.thresholdBytes) {
            "Not enough available system RAM for the existing vault KDF; close models/apps and retry. No weaker encryption used."
        }
    }

    private fun checkInterrupted() {
        if (Thread.currentThread().isInterrupted) throw CancellationException("Vault KDF interrupted")
    }

    fun derive(password: ByteArray, salt: ByteArray): ByteArray {
        require(password.size in 1..4096 && salt.size == VaultCrypto.SALT_LEN) { "Invalid vault KDF input length" }
        checkInterrupted()
        check(flight.tryLock()) { "Vault KDF is already running; retry when it finishes" }
        try {
            checkInterrupted()
            val snapshot = memory()
            admit(snapshot)
            val out = if (nativeAvailable) deriveNative(password, salt, snapshot.availableBytes, snapshot.lowMemory)
                else deriveJava(password, salt)
            return finishDerivation(out)
        } finally { flight.unlock() }
    }

    // Synchronous native Argon2 cannot be preempted. The caller holds the gate
    // until it returns; discard an interrupted result, never start a second
    // memory-hard operation while cancellation is pending. Internal for direct
    // buffer-wipe assertions using a key produced by the actual JNI function.
    internal fun finishDerivation(out: ByteArray): ByteArray = try {
        check(out.size == VaultCrypto.KEY_LEN) { "Invalid native KDF result" }
        checkInterrupted()
        out
    } catch (failure: Throwable) { out.fill(0); throw failure }

    private fun deriveJava(password: ByteArray, salt: ByteArray): ByteArray {
        require(password.size in 1..4096 && salt.size == VaultCrypto.SALT_LEN)
        val runtime = Runtime.getRuntime()
        val heap = runtime.maxMemory() - (runtime.totalMemory() - runtime.freeMemory())
        require(heap >= VaultCrypto.ARGON2_MEMORY_KIB.toLong() * 1024 + 96L * 1024 * 1024) {
            "Native vault KDF unavailable and app heap is insufficient for the identical Java KDF; no weaker encryption used"
        }
        val params = Argon2Parameters.Builder(Argon2Parameters.ARGON2_id)
            .withVersion(Argon2Parameters.ARGON2_VERSION_13)
            .withIterations(VaultCrypto.ARGON2_ITERATIONS)
            .withMemoryAsKB(VaultCrypto.ARGON2_MEMORY_KIB)
            .withParallelism(VaultCrypto.ARGON2_PARALLELISM).withSalt(salt).build()
        val out = ByteArray(VaultCrypto.KEY_LEN)
        try {
            Argon2BytesGenerator().apply { init(params) }.generateBytes(password, out)
            return out
        } catch (failure: Throwable) { out.fill(0); throw failure }
        finally { params.clear() }
    }

    // Kept by consumer-rules.pro; byte arrays avoid String password copies.
    external fun deriveNative(password: ByteArray, salt: ByteArray, availableBytes: Long, lowMemory: Boolean): ByteArray
}
