package com.unoone.pai.recording

import com.unoone.pai.contracts.*
import kotlinx.serialization.Serializable

/**
 * Recording session state machine.
 *
 * States: IDLE → RECORDING → PAUSED → RECORDING → STOPPED
 *                                          ↘ STOPPED
 * From any state: CANCELLED
 */
@Serializable
enum class RecordingState {
    IDLE,
    RECORDING,
    PAUSED,
    STOPPED,
    CANCELLED,
    PROCESSING     // Transcription + summarization in progress
}

/**
 * Commands for the recording workspace.
 */
@Serializable
enum class RecordingCommand {
    START,
    PAUSE,
    RESUME,
    STOP,
    CANCEL,
    ADD_BOOKMARK,
    SET_NAME,
    SET_TYPE,
    IMPORT_AUDIO
}

/**
 * Recording session — represents an active or completed recording.
 *
 * The session tracks the full lifecycle from start to processed.
 * It can be started on Android and completed on desktop, or vice versa.
 */
@Serializable
data class RecordingSession(
    val id: String,                          // UUID v7
    val state: RecordingState = RecordingState.IDLE,
    val recordingType: RecordingType = RecordingType.GENERAL,
    val title: String = "",
    val bookmarks: List<RecordingBookmark> = emptyList(),
    val startedAt: String? = null,           // ISO-8601
    val pausedAt: String? = null,
    val resumedAt: String? = null,
    val stoppedAt: String? = null,
    val durationSeconds: Long = 0,
    val sourcePlatform: Platform = Platform.ANDROID,
    val sourceDeviceId: String = "",
    val privacy: RecordingPrivacy = RecordingPrivacy.FULL,
    val language: String = "en",
    val audioPath: String? = null,          // Encrypted vault path
    val transcriptPath: String? = null,     // Encrypted vault path
    val summaryPath: String? = null,          // Encrypted vault path
    val processingProgress: Float = 0.0f,   // 0.0 to 1.0
    val errorMessage: String? = null
)

/**
 * Recording processing pipeline stages.
 *
 * After recording stops, the audio goes through this pipeline:
 * Microphone → Encrypted audio → STT → Transcript cleanup → Summary → Key points → Save to vault
 */
@Serializable
enum class ProcessingStage {
    IDLE,
    TRANSCRIBING,
    CLEANING_TRANSCRIPT,
    GENERATING_SUMMARY,
    EXTRACTING_KEY_POINTS,
    EXTRACTING_DECISIONS,
    EXTRACTING_ACTION_ITEMS,
    EXTRACTING_FOLLOW_UPS,
    SAVING_TO_VAULT,
    COMPLETE,
    ERROR
}

/**
 * Processing result for a recording.
 *
 * Contains all outputs from the recording pipeline:
 * transcript, summaries, key points, decisions, action items, etc.
 */
@Serializable
data class RecordingProcessingResult(
    val recordingId: String,
    val stage: ProcessingStage = ProcessingStage.IDLE,
    val transcript: Transcript? = null,
    val shortSummary: String? = null,
    val detailedSummary: String? = null,
    val keyPoints: List<String> = emptyList(),
    val decisions: List<String> = emptyList(),
    val actionItems: List<String> = emptyList(),
    val importantDates: List<String> = emptyList(),
    val peopleMentioned: List<String> = emptyList(),
    val followUpQuestions: List<String> = emptyList(),
    val processingErrors: List<String> = emptyList(),
    val processedAt: String? = null,       // ISO-8601
    val processedBy: String? = null         // "gemma-4-e2b" or "gemma-4-12b"
)

/**
 * Interface for the recording engine.
 *
 * Both Android and desktop must implement this interface
 * to provide platform-specific recording capabilities.
 */
interface RecordingEngine {
    /**
     * Start a new recording session.
     */
    suspend fun start(session: RecordingSession): RecordingSession

    /**
     * Pause the current recording.
     */
    suspend fun pause(sessionId: String): RecordingSession

    /**
     * Resume a paused recording.
     */
    suspend fun resume(sessionId: String): RecordingSession

    /**
     * Stop the recording and begin processing.
     */
    suspend fun stop(sessionId: String): RecordingSession

    /**
     * Cancel the recording (discard all data).
     */
    suspend fun cancel(sessionId: String)

    /**
     * Add a bookmark at the current position.
     */
    suspend fun addBookmark(sessionId: String, label: String? = null): RecordingSession

    /**
     * Import an existing audio file.
     */
    suspend fun importAudio(filePath: String, type: RecordingType): RecordingSession

    /**
     * Process a completed recording (transcribe, summarize, etc.).
     */
    suspend fun process(sessionId: String, modelId: String = "gemma-4-e2b"): RecordingProcessingResult

    /**
     * Get the current state of a recording session.
     */
    fun getState(sessionId: String): RecordingSession?

    /**
     * Get all recording sessions.
     */
    fun getAllSessions(): List<RecordingSession>

    /**
     * Delete a recording (with confirmation).
     */
    suspend fun delete(sessionId: String)
}