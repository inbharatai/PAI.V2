package org.inbharat.audio

/** Internal, thin JNI surface. It performs no inference scheduling on its own. */
internal object NativeBridge {
    init { System.loadLibrary("ibaudio_jni") }

    external fun apiVersion(): Int
    external fun createRuntime(cacheDirectory: String, modelRoot: String, threads: Int, vulkanProbeRequested: Boolean): Long
    external fun releaseRuntime(handle: Long)
    external fun diagnostics(handle: Long): String
    external fun loadModel(runtime: Long, modelId: String): Long
    external fun releaseModel(handle: Long)
    external fun createSession(model: Long, task: Int, streaming: Boolean, vadThresholdDbfs: Float): Long
    external fun releaseSession(handle: Long)
    external fun runAsr(session: Long, pcm: FloatArray, sampleRate: Int, channels: Int): String
    external fun runTts(session: Long, text: String): FloatArray
    external fun runVad(session: Long, pcm: FloatArray, sampleRate: Int, channels: Int): LongArray
    external fun setPlaybackActive(session: Long, active: Boolean)
    external fun reportInputLevel(session: Long, rmsDbfs: Float, durationMs: Int): Int
}
