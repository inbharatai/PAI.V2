package com.unoone.agent.voice.tts

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import com.unoone.agent.core.model.Result
import com.unoone.agent.core.util.Logger
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeoutOrNull
import java.util.Locale
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

/**
 * Universal, highly robust TextToSpeech engine supporting English and Indian languages (Hindi, Tamil, etc.).
 * Fully offline-first.
 */
class TtsPlayer : TextToSpeech.OnInitListener {

    private var tts: TextToSpeech? = null
    private var isReady = false
    private var pendingText: String? = null
    private var activeTrack: AudioTrack? = null

    /**
     * Finding A6: per-utterance completion callbacks keyed by the utterance id
     * handed to speak(). The old single `onUtteranceDone` slot was clobbered by
     * any concurrent await, and it was compared against an id speak() never
     * used, so the fallback startsWith() match resumed the WRONG await.
     */
    private val awaitListeners = ConcurrentHashMap<String, (Boolean) -> Unit>()
    private val utteranceCounter = AtomicLong()

    private fun finishUtterance(utteranceId: String?, success: Boolean) {
        val id = utteranceId ?: return
        awaitListeners.remove(id)?.invoke(success)
    }

    fun initialize(context: Context): Result<Unit> {
        return try {
            tts = TextToSpeech(context, this)
            Result.Success(Unit)
        } catch (e: Exception) {
            Logger.e("TTS Player: Initialization failed", e)
            Result.Error("TTS failed: ${e.message}")
        }
    }

    override fun onInit(status: Int) {
        if (status == TextToSpeech.SUCCESS) {
            val result = tts?.setLanguage(Locale("en", "IN"))
            if (result == TextToSpeech.LANG_MISSING_DATA || result == TextToSpeech.LANG_NOT_SUPPORTED) {
                Logger.w("TTS Player: English (India) not supported, using default locale")
                tts?.setLanguage(Locale.getDefault())
            }

            // 0C-9: Register UtteranceProgressListener to track TTS completion.
            // Finding A6: done/error both complete the awaiting caller, but with
            // distinct outcomes so a failed playback never reads as success.
            tts?.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
                override fun onStart(utteranceId: String?) {
                    Logger.d("TTS Player: Utterance started: $utteranceId")
                }
                override fun onDone(utteranceId: String?) {
                    Logger.d("TTS Player: Utterance completed: $utteranceId")
                    finishUtterance(utteranceId, success = true)
                }
                override fun onError(utteranceId: String?) {
                    Logger.w("TTS Player: Utterance error: $utteranceId")
                    finishUtterance(utteranceId, success = false)
                }
            })

            isReady = true
            Logger.i("TTS Player: Initialized successfully")

            // Speak any pending text that was queued during init
            pendingText?.let {
                speak(it)
                pendingText = null
            }
        } else {
            Logger.e("TTS Player: Initialization failed with status $status")
        }
    }

    /**
     * Synthesize and speak text. Automatically detects Indian language context or falls back to English.
     *
     * @param utteranceId the id the UtteranceProgressListener reports for this
     * utterance. [speakAwait] passes its own unique id so it resumes on exactly
     * the utterance it started (finding A6).
     */
    fun speak(
        text: String,
        languageCode: String = "en-IN",
        utteranceId: String = DEFAULT_UTTERANCE_ID
    ): Result<Unit> {
        val t = tts
        if (!isReady || t == null) {
            pendingText = text
            return Result.Success(Unit) // Queued
        }

        return try {
            val locale = Locale.forLanguageTag(languageCode)
            val languageResult = t.setLanguage(locale)
            if (
                languageResult == TextToSpeech.LANG_MISSING_DATA ||
                languageResult == TextToSpeech.LANG_NOT_SUPPORTED
            ) {
                return Result.Error("System TTS does not support ${locale.toLanguageTag()}")
            }
            val speakResult = t.speak(
                text,
                TextToSpeech.QUEUE_FLUSH,
                null,
                utteranceId
            )
            if (speakResult == TextToSpeech.ERROR) {
                Result.Error("System TTS rejected the utterance")
            } else {
                Result.Success(Unit)
            }
        } catch (e: Exception) {
            Logger.e("TTS Player: Speak failed", e)
            Result.Error("Speak failed: ${e.message}")
        }
    }

    /**
     * 0C-8: Play raw PCM audio data from Sherpa-ONNX TTS or other offline engines.
     * Converts FloatArray samples → Int16 PCM → AudioTrack for playback.
     */
    fun playPcm(samples: FloatArray, sampleRate: Int = 22050): Result<Unit> {
        if (samples.isEmpty()) {
            Logger.w("TTS Player: playPcm called with empty samples")
            return Result.Error("Empty audio samples")
        }

        return try {
            // Stop any currently playing AudioTrack first
            stopPcmTrack()

            // Convert FloatArray [-1.0, 1.0] → Int16 PCM bytes
            val pcmBytes = ByteArray(samples.size * 2)
            for (i in samples.indices) {
                val clipped = samples[i].coerceIn(-1f, 1f)
                val intSample = (clipped * 32767f).toInt()
                val shortSample = clipped.coerceIn(-1f, 1f)
                // Little-endian encoding
                pcmBytes[i * 2] = (intSample and 0xFF).toByte()
                pcmBytes[i * 2 + 1] = ((intSample shr 8) and 0xFF).toByte()
            }

            val bufferSize = AudioTrack.getMinBufferSize(
                sampleRate,
                AudioFormat.CHANNEL_OUT_MONO,
                AudioFormat.ENCODING_PCM_16BIT
            )

            val track = AudioTrack.Builder()
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_ASSISTANCE_NAVIGATION_GUIDANCE)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                        .build()
                )
                .setAudioFormat(
                    AudioFormat.Builder()
                        .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                        .setSampleRate(sampleRate)
                        .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                        .build()
                )
                .setBufferSizeInBytes(maxOf(bufferSize, pcmBytes.size))
                .setTransferMode(AudioTrack.MODE_STATIC)
                .build()

            track.write(pcmBytes, 0, pcmBytes.size)
            track.play()
            activeTrack = track
            Logger.i("TTS Player: PCM playback started (${samples.size} samples at ${sampleRate}Hz)")
            Result.Success(Unit)
        } catch (e: Exception) {
            Logger.e("TTS Player: PCM playback failed", e)
            Result.Error("PCM playback failed: ${e.message}")
        }
    }

    /**
     * 0C-9: Suspends until TTS finishes speaking the given text.
     *
     * Finding A6: the awaited id and the id passed to speak() are now the SAME
     * string, so this resumes on exactly the utterance it started. A failed
     * playback ([UtteranceProgressListener.onError]) resumes as an Error. The
     * safety timeout remains: if the listener never fires the await resumes
     * (hands-free callers must not wedge on a silent engine), with a warning —
     * and via a cancellable [withTimeoutOrNull] instead of a leaked raw timer
     * thread per call.
     */
    suspend fun speakAwait(text: String, languageCode: String = "en-IN", timeoutMs: Long = 10_000L): Result<Unit> {
        val utteranceId = "UnoOne_TTS_Await_${utteranceCounter.incrementAndGet()}"
        val result = speak(text, languageCode, utteranceId)
        if (result is Result.Error) return result

        val done = CompletableDeferred<Boolean>()
        awaitListeners[utteranceId] = { success -> done.complete(success) }
        return try {
            val completed = withTimeoutOrNull(timeoutMs) { done.await() }
            when (completed) {
                true -> Result.Success(Unit)
                false -> Result.Error("System TTS playback failed")
                // Timeout: resume rather than wedge the hands-free flow, but say so loudly.
                null -> {
                    Logger.w("TTS Player: speakAwait timed out after ${timeoutMs}ms; resuming without a completion event")
                    Result.Success(Unit)
                }
            }
        } finally {
            awaitListeners.remove(utteranceId)
        }
    }

    fun stop() {
        try {
            tts?.stop()
        } catch (e: Exception) {
            Logger.e("TTS Player: Error stopping playback", e)
        }
        stopPcmTrack()
    }

    fun release() {
        stop()
        stopPcmTrack()
        // Finding A6: fail any pending await instead of stranding it until the
        // timeout — the engine it is waiting on is being shut down right now.
        for (listener in awaitListeners.values) runCatching { listener.invoke(false) }
        awaitListeners.clear()
        try {
            tts?.shutdown()
        } catch (e: Exception) {
            Logger.e("TTS Player: Error shutting down", e)
        }
        tts = null
        isReady = false
    }

    private fun stopPcmTrack() {
        try {
            activeTrack?.let { track ->
                if (track.state == AudioTrack.PLAYSTATE_PLAYING) {
                    track.stop()
                }
                track.release()
            }
        } catch (e: Exception) {
            Logger.e("TTS Player: Error releasing AudioTrack", e)
        }
        activeTrack = null
    }

    companion object {
        /** Utterance id for fire-and-forget [speak] calls nobody is awaiting. */
        const val DEFAULT_UTTERANCE_ID = "UnoOne_TTS_Playback"
    }
}
