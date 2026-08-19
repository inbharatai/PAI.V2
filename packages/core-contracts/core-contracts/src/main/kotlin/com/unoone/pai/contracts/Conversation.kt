package com.unoone.pai.contracts

import kotlinx.serialization.Serializable

/**
 * A conversation in the shared vault.
 *
 * Conversations can be started on one platform and continued on another.
 * The vault stores the full history; each platform loads what it needs
 * for context reconstruction.
 */
@Serializable
data class Conversation(
    val metadata: VaultRecordMetadata,
    val title: String,
    val messages: List<ConversationMessage>,
    val summary: String? = null,       // LLM-generated summary
    val language: String = "en",        // BCP-47 language tag
    val tags: List<String> = emptyList()
)

@Serializable
data class ConversationMessage(
    val id: String,                     // UUID v7
    val role: MessageRole,
    val content: String,
    val timestamp: String,              // ISO-8601 instant
    val platform: Platform,             // Which platform sent this message
    val toolCalls: List<ToolCallRecord> = emptyList(),
    val modelUsed: String? = null       // e.g., "gemma-4-e2b" or "gemma-4-12b"
)

@Serializable
enum class MessageRole {
    USER,
    ASSISTANT,
    SYSTEM,
    TOOL_RESULT
}

@Serializable
data class ToolCallRecord(
    val toolName: String,
    val args: Map<String, String>,      // Arg keys only (values are private)
    val result: String? = null,         // "success" | "error" | "blocked"
    val safetyDecision: String? = null  // "allow" | "confirm" | "block"
)