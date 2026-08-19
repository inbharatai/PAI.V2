package org.inbharat.audio

import java.io.Closeable
import java.io.File
import java.util.concurrent.Executor
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

object AudioTask {
    const val ASR = 1
    const val TTS = 2
    const val VAD = 3
    const val KWS = 4
}

data class VadSegment(val startFrame: Long, val endFrame: Long)

/**
 * Process-level native runtime owner.
 *
 * Lifecycle contract: close every Session, then Model, then Runtime. Native
 * release rejects live children with BUSY. Inference must run on [executor],
 * never on the UI, AudioRecord, or AAudio callback thread.
 */
class AudioRuntime private constructor(
    private var handle: Long,
    val executor: Executor
) : Closeable {
    private val closed = AtomicBoolean(false)

    val diagnostics: String
        get() = NativeBridge.diagnostics(requireHandle())

    fun loadModel(modelId: String): AudioModel = AudioModel(
        NativeBridge.loadModel(requireHandle(), modelId), this
    )

    internal fun requireHandle(): Long {
        check(!closed.get() && handle != 0L) { "AudioRuntime is closed" }
        return handle
    }

    override fun close() = synchronized(this) {
        if (!closed.get()) {
            NativeBridge.releaseRuntime(handle)
            handle = 0L
            closed.set(true)
        }
    }

    companion object {
        val apiVersion: Int get() = NativeBridge.apiVersion()

        fun create(
            cacheDirectory: File,
            allowedModelRoot: File? = null,
            cpuThreads: Int = 2,
            requestVulkanProbe: Boolean = false,
            executor: Executor = Executors.newSingleThreadExecutor()
        ): AudioRuntime {
            require(cpuThreads in 1..256)
            cacheDirectory.mkdirs()
            val handle = NativeBridge.createRuntime(
                cacheDirectory.absolutePath,
                allowedModelRoot?.absolutePath.orEmpty(),
                cpuThreads,
                requestVulkanProbe
            )
            return AudioRuntime(handle, executor)
        }
    }
}

class AudioModel internal constructor(
    private var handle: Long,
    private val runtime: AudioRuntime
) : Closeable {
    private val closed = AtomicBoolean(false)

    fun createSession(task: Int, streaming: Boolean = false, vadThresholdDbfs: Float = -42f): AudioSession {
        runtime.requireHandle()
        check(!closed.get() && handle != 0L) { "AudioModel is closed" }
        return AudioSession(NativeBridge.createSession(handle, task, streaming, vadThresholdDbfs), this)
    }

    override fun close() = synchronized(this) {
        if (!closed.get()) {
            NativeBridge.releaseModel(handle)
            handle = 0L
            closed.set(true)
        }
    }
}

/** Single-flight session. Concurrent calls are rejected by native code with BUSY. */
class AudioSession internal constructor(
    private var handle: Long,
    @Suppress("unused") private val model: AudioModel
) : Closeable {
    private val closed = AtomicBoolean(false)

    private fun requireHandle(): Long {
        check(!closed.get() && handle != 0L) { "AudioSession is closed" }
        return handle
    }

    fun asr(pcmInterleaved: FloatArray, sampleRate: Int, channels: Int = 1): String =
        NativeBridge.runAsr(requireHandle(), pcmInterleaved, sampleRate, channels)

    /** Returns mono 24 kHz PCM from the deterministic reference TTS engine. */
    fun tts(text: String): FloatArray = NativeBridge.runTts(requireHandle(), text)

    fun vad(pcmInterleaved: FloatArray, sampleRate: Int, channels: Int = 1): List<VadSegment> {
        val packed = NativeBridge.runVad(requireHandle(), pcmInterleaved, sampleRate, channels)
        return packed.asList().chunked(2).map { VadSegment(it[0], it[1]) }
    }

    fun setPlaybackActive(active: Boolean) = NativeBridge.setPlaybackActive(requireHandle(), active)

    /** Returns the native barge-in state. State 3 means output should be interrupted. */
    fun reportInputLevel(rmsDbfs: Float, durationMs: Int): Int {
        require(durationMs >= 0) { "durationMs must be non-negative" }
        return NativeBridge.reportInputLevel(requireHandle(), rmsDbfs, durationMs)
    }

    override fun close() = synchronized(this) {
        if (!closed.get()) {
            NativeBridge.releaseSession(handle)
            handle = 0L
            closed.set(true)
        }
    }
}
