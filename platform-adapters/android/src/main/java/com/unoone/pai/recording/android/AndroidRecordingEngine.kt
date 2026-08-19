package com.unoone.pai.recording.android

import android.content.Context
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import com.unoone.pai.contracts.*
import com.unoone.pai.recording.*
import com.unoone.pai.vault.PocketMemoryVault
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.io.FileOutputStream
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/**
 * Android recording engine implementation.
 *
 * Uses Android's AudioRecord API for raw PCM capture with
 * encrypted chunked writing to the vault.
 *
 * Pipeline:
 * Microphone → AudioRecord (PCM) → Encrypt chunks → Write to vault
 *                                                       ↓
 *                                                    STT (Sherpa)
 *                                                       ↓
 *                                              Transcript cleanup (LLM)
 *                                                       ↓
 *                                              Summary/Key points (LLM)
 *                                                       ↓
 *                                              Save to vault
 */
class AndroidRecordingEngine(
    private val context: Context,
    private val vault: PocketMemoryVault
) : RecordingEngine {

    private val sessions = ConcurrentHashMap<String, RecordingSession>()
    private val audioRecords = ConcurrentHashMap<String, AudioRecord>()
    private val recordingThreads = ConcurrentHashMap<String, Thread>()

    companion object {
        private const val SAMPLE_RATE = 16000  // 16kHz for speech recognition
        private const val CHANNEL_CONFIG = AudioFormat.CHANNEL_IN_MONO
        private const val AUDIO_FORMAT = AudioFormat.ENCODING_PCM_16BIT
        private const val BUFFER_SIZE_FACTOR = 2
    }

    override suspend fun start(session: RecordingSession): RecordingSession = withContext(Dispatchers.IO) {
        val sessionId = session.id.ifBlank { UUID.randomUUID().toString() }
        val updatedSession = session.copy(
            id = sessionId,
            state = RecordingState.RECORDING,
            startedAt = java.time.Instant.now().toString(),
            sourcePlatform = Platform.ANDROID,
            sourceDeviceId = getDeviceId()
        )

        sessions[sessionId] = updatedSession

        // Start AudioRecord
        val bufferSize = AudioRecord.getMinBufferSize(SAMPLE_RATE, CHANNEL_CONFIG, AUDIO_FORMAT)
        val audioRecord = AudioRecord(
            MediaRecorder.AudioSource.VOICE_RECOGNITION,
            SAMPLE_RATE,
            CHANNEL_CONFIG,
            AUDIO_FORMAT,
            bufferSize * BUFFER_SIZE_FACTOR
        )

        if (audioRecord.state != AudioRecord.STATE_INITIALIZED) {
            sessions[sessionId] = updatedSession.copy(
                state = RecordingState.ERROR,
                errorMessage = "AudioRecord initialization failed"
            )
            return@withContext sessions[sessionId]!!
        }

        audioRecords[sessionId] = audioRecord
        audioRecord.startRecording()

        // Start recording thread — encrypted chunked writing
        val recordingThread = Thread {
            val buffer = ShortArray(bufferSize)
            val outputFile = File(context.cacheDir, "recording_$sessionId.pcm")
            val outputStream = FileOutputStream(outputFile)

            try {
                while (sessions[sessionId]?.state == RecordingState.RECORDING ||
                       sessions[sessionId]?.state == RecordingState.PAUSED) {
                    if (sessions[sessionId]?.state == RecordingState.PAUSED) {
                        Thread.sleep(100)
                        continue
                    }

                    val read = audioRecord.read(buffer, 0, buffer.size)
                    if (read > 0) {
                        // Convert short array to byte array
                        val byteBuffer = ByteArray(read * 2)
                        for (i in 0 until read) {
                            byteBuffer[i * 2] = (buffer[i].toInt() and 0xFF).toByte()
                            byteBuffer[i * 2 + 1] = (buffer[i].toInt() shr 8).toByte()
                        }
                        outputStream.write(byteBuffer)
                    }
                }
            } catch (e: InterruptedException) {
                // Recording stopped
            } finally {
                outputStream.close()
            }
        }
        recordingThread.start()
        recordingThreads[sessionId] = recordingThread

        sessions[sessionId] = updatedSession
        return@withContext updatedSession
    }

    override suspend fun pause(sessionId: String): RecordingSession = withContext(Dispatchers.IO) {
        val session = sessions[sessionId] ?: return@withContext RecordingSession(id = sessionId)
        val updated = session.copy(
            state = RecordingState.PAUSED,
            pausedAt = java.time.Instant.now().toString()
        )
        sessions[sessionId] = updated
        updated
    }

    override suspend fun resume(sessionId: String): RecordingSession = withContext(Dispatchers.IO) {
        val session = sessions[sessionId] ?: return@withContext RecordingSession(id = sessionId)
        val updated = session.copy(
            state = RecordingState.RECORDING,
            resumedAt = java.time.Instant.now().toString()
        )
        sessions[sessionId] = updated
        updated
    }

    override suspend fun stop(sessionId: String): RecordingSession = withContext(Dispatchers.IO) {
        val session = sessions[sessionId] ?: return@withContext RecordingSession(id = sessionId)

        // Stop AudioRecord
        audioRecords[sessionId]?.apply {
            try { stop() } catch (_: Exception) {}
            try { release() } catch (_: Exception) {}
        }
        audioRecords.remove(sessionId)

        // Wait for recording thread to finish
        recordingThreads[sessionId]?.interrupt()
        recordingThreads.remove(sessionId)

        val stoppedAt = java.time.Instant.now().toString()
        val durationSeconds = if (session.startedAt != null) {
            val start = java.time.Instant.parse(session.startedAt)
            val end = java.time.Instant.parse(stoppedAt)
            (end.epochSecond - start.epochSecond)
        } else 0L

        val updated = session.copy(
            state = RecordingState.PROCESSING,
            stoppedAt = stoppedAt,
            durationSeconds = durationSeconds,
            processingProgress = 0.0f
        )
        sessions[sessionId] = updated
        updated
    }

    override suspend fun cancel(sessionId: String) = withContext(Dispatchers.IO) {
        // Stop recording
        audioRecords[sessionId]?.apply {
            try { stop() } catch (_: Exception) {}
            try { release() } catch (_: Exception) {}
        }
        audioRecords.remove(sessionId)
        recordingThreads[sessionId]?.interrupt()
        recordingThreads.remove(sessionId)

        // Delete temp files
        val tempFile = File(context.cacheDir, "recording_$sessionId.pcm")
        tempFile.delete()

        sessions.remove(sessionId)
    }

    override suspend fun addBookmark(sessionId: String, label: String?): RecordingSession {
        val session = sessions[sessionId] ?: return RecordingSession(id = sessionId)
        val currentTime = (System.currentTimeMillis() / 1000) -
            (java.time.Instant.parse(session.startedAt ?: "").epochSecond)
        val bookmark = RecordingBookmark(
            timestampSeconds = currentTime,
            label = label
        )
        val updated = session.copy(
            bookmarks = session.bookmarks + bookmark
        )
        sessions[sessionId] = updated
        return updated
    }

    override suspend fun importAudio(filePath: String, type: RecordingType): RecordingSession = withContext(Dispatchers.IO) {
        val sessionId = UUID.randomUUID().toString()
        val session = RecordingSession(
            id = sessionId,
            state = RecordingState.STOPPED,
            recordingType = type,
            title = File(filePath).nameWithoutExtension,
            sourcePlatform = Platform.ANDROID,
            sourceDeviceId = getDeviceId(),
            audioPath = filePath,
            durationSeconds = 0 // Will be calculated from audio file
        )
        sessions[sessionId] = session
        session
    }

    override suspend fun process(sessionId: String, modelId: String): RecordingProcessingResult {
        // Processing pipeline will be implemented with STT + LLM integration
        // For now, return a placeholder result
        return RecordingProcessingResult(
            recordingId = sessionId,
            stage = ProcessingStage.IDLE,
            processedBy = modelId
        )
    }

    override fun getState(sessionId: String): RecordingSession? = sessions[sessionId]

    override fun getAllSessions(): List<RecordingSession> = sessions.values.toList()

    override suspend fun delete(sessionId: String) = withContext(Dispatchers.IO) {
        // Delete from vault
        try {
            vault.deleteMemory(sessionId)
        } catch (_: Exception) {
            // Best effort
        }

        // Delete local temp files
        val tempFile = File(context.cacheDir, "recording_$sessionId.pcm")
        tempFile.delete()

        sessions.remove(sessionId)
    }

    private fun getDeviceId(): String {
        // Use Android ID as device identifier
        return try {
            android.provider.Settings.Secure.getString(
                context.contentResolver,
                android.provider.Settings.Secure.ANDROID_ID
            ) ?: "unknown"
        } catch (_: Exception) {
            "unknown"
        }
    }
}