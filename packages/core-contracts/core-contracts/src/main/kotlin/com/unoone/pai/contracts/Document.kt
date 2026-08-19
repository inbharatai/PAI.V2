package com.unoone.pai.contracts

import kotlinx.serialization.Serializable

/**
 * Supported document types in the shared vault.
 */
@Serializable
enum class DocumentType {
    PDF,
    DOCX,
    TXT,
    MARKDOWN,
    CSV,
    XLSX,
    PPTX,
    IMAGE,
    AUDIO,
    WEB_PAGE
}

/**
 * A document in the shared vault.
 *
 * Documents can be imported on any platform and are available on all others.
 * The desktop (Gemma 4 12B) can perform deeper analysis than mobile (Gemma 4 E2B).
 */
@Serializable
data class Document(
    val metadata: VaultRecordMetadata,
    val title: String,
    val documentType: DocumentType,
    val filePath: String,               // Encrypted vault path
    val fileSizeBytes: Long,
    val language: String? = null,
    val summary: String? = null,        // LLM-generated summary
    val keyPoints: List<String> = emptyList(),
    val tags: List<String> = emptyList(),
    val extractedText: String? = null,  // Extracted/normalized text content
    val pageCount: Int? = null,         // For PDF/DOCX
    val wordCount: Int? = null,
    val importedFrom: String? = null     // Original source (e.g., URL, file path)
)