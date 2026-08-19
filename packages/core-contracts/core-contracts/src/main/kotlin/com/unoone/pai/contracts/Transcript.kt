package com.unoone.pai.contracts

import kotlinx.serialization.Serializable

/**
 * A transcript of a recording.
 *
 * Transcripts are stored separately from recordings so that
 * TRANSCRIPT_ONLY privacy mode can persist the transcript
 * without the audio file.
 */
@Serializable
data class Transcript(
    val metadata: VaultRecordMetadata,
    val recordingId: String,            // Reference to the parent Recording
    val segments: List<TranscriptSegment>,
    val language: String = "en",
    val fullText: String,               // Complete transcript text
    val cleanedText: String? = null,    // LLM-cleaned version (filler words removed, etc.)
    val wordCount: Int = 0,
    val confidence: Double = 0.0         // Average STT confidence (0.0-1.0)
)

@Serializable
data class TranscriptSegment(
    val startTimeSeconds: Double,
    val endTimeSeconds: Double,
    val speaker: String? = null,        // Speaker label if diarization available
    val text: String,
    val confidence: Double = 0.0,
    val isBookmark: Boolean = false      // True if this segment was bookmarked during recording
)