package com.unoone.pai.contracts

import kotlinx.serialization.Serializable

/**
 * Recording types supported by the recording workspace.
 */
@Serializable
enum class RecordingType {
    MEETING,
    LECTURE,
    INTERVIEW,
    PERSONAL_NOTE,
    VOICE_JOURNAL,
    GENERAL
}

/**
 * Privacy levels for recordings.
 */
@Serializable
enum class RecordingPrivacy {
    FULL,              // Save audio, transcript, and summary
    TRANSCRIPT_ONLY,   // Save transcript and summary only (delete audio after processing)
    SUMMARY_ONLY,      // Save summary only (delete audio and transcript after processing)
    PRIVATE_SESSION    // No retention — processed in memory only, nothing saved to vault
}

/**
 * A recording in the shared vault.
 *
 * Recordings made on Android must be available on desktop and vice versa.
 * The stronger Gemma 4 12B desktop model can re-analyze a mobile recording
 * and create a more detailed summary without duplicating the original.
 */
@Serializable
data class Recording(
    val metadata: VaultRecordMetadata,
    val title: String,
    val recordingType: RecordingType,
    val sourcePlatform: Platform,
    val audioPath: String?,              // Encrypted vault path (null if TRANSCRIPT_ONLY or SUMMARY_ONLY)
    val transcriptPath: String?,         // Encrypted vault path (null if SUMMARY_ONLY)
    val summaryPath: String?,            // Encrypted vault path
    val language: String = "en",         // BCP-47 language tag
    val durationSeconds: Long = 0,
    val privacy: RecordingPrivacy = RecordingPrivacy.FULL,
    val bookmarks: List<RecordingBookmark> = emptyList(),
    val summaries: List<RecordingSummary> = emptyList(), // Multiple summaries (mobile + desktop)
    val tags: List<String> = emptyList()
)

@Serializable
data class RecordingBookmark(
    val timestampSeconds: Long,
    val label: String? = null
)

/**
 * Summary of a recording. Multiple summaries can exist for the same recording:
 * one generated on mobile (Gemma 4 E2B) and a more detailed one on desktop (Gemma 4 12B).
 */
@Serializable
data class RecordingSummary(
    val id: String,                      // UUID v7
    val generatedBy: String,             // "gemma-4-e2b" or "gemma-4-12b"
    val generatedAt: String,             // ISO-8601 instant
    val platform: Platform,
    val shortSummary: String? = null,
    val detailedSummary: String? = null,
    val keyPoints: List<String> = emptyList(),
    val decisions: List<String> = emptyList(),
    val actionItems: List<String> = emptyList(),
    val importantDates: List<String> = emptyList(),
    val peopleMentioned: List<String> = emptyList(),
    val followUpQuestions: List<String> = emptyList()
)