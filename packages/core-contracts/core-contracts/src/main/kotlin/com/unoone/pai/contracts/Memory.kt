package com.unoone.pai.contracts

import kotlinx.serialization.Serializable

/**
 * Memory types in the shared vault.
 *
 * Each type corresponds to a vault subdirectory:
 *   personal/, preferences/, conversations/, tasks/,
 *   knowledge/, accessibility/, skills/
 */
@Serializable
enum class MemoryType {
    PERSONAL,
    PREFERENCE,
    CONVERSATION,
    TASK,
    KNOWLEDGE,
    ACCESSIBILITY,
    SKILL
}

/**
 * A single memory entry in the shared vault.
 *
 * Memories are the fundamental unit of persistent user knowledge.
 * Both UnoOne Mobile and UnoOne Power read and write the same
 * memory format from the encrypted USB vault.
 */
@Serializable
data class Memory(
    val metadata: VaultRecordMetadata,
    val type: MemoryType,
    val key: String,                    // Short unique key (e.g., "preferred_language")
    val content: String,                // The memory content
    val tags: List<String> = emptyList(), // Searchable tags
    val relevanceScore: Double = 0.0,   // Computed relevance score (0.0-1.0)
    val expiresAt: String? = null        // ISO-8601 or null for permanent
)