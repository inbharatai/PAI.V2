package com.unoone.pai.contracts

import kotlinx.serialization.Serializable

/**
 * Canonical UnoOne tool action schema.
 *
 * Both Gemma 4 E2B (mobile) and Gemma 4 12B (desktop) produce
 * output that is parsed into this canonical format. Raw model
 * output never executes tools directly.
 *
 * Flow:
 *   Model output → Model-specific parser → Canonical ToolAction
 *   → JSON validation → SafetyGuard → Permission check → Execution
 */
@Serializable
data class ToolAction(
    val id: String,                          // UUID v7
    val toolName: String,                    // Canonical tool name from ToolRegistry
    val args: Map<String, String>,           // Tool arguments
    val confidence: Double = 1.0,            // Parser confidence (0.0-1.0)
    val sourceModel: String,                 // "gemma-4-e2b" or "gemma-4-12b"
    val sourcePlatform: Platform,
    val safetyDecision: SafetyDecision = SafetyDecision.PENDING,
    val confirmationRequired: Boolean = false // Requires user confirmation
)

@Serializable
enum class SafetyDecision {
    PENDING,     // Not yet evaluated
    ALLOW,       // Safe to execute
    CONFIRM,     // Requires user confirmation
    STRONG_CONFIRM, // Requires explicit user approval
    BLOCK        // Blocked by safety guard
}

/**
 * Risk levels for tool actions, matching the existing SafetyGuard.
 */
@Serializable
enum class ToolRiskLevel {
    DIRECT,          // Can be executed directly (e.g., set_timer)
    CONFIRM,         // Needs user confirmation (e.g., send_message)
    STRONG_CONFIRM,  // Needs explicit approval (e.g., open_url)
    BLOCK            // Cannot be executed (e.g., make_payment)
}

/**
 * Result of executing a tool action.
 */
@Serializable
data class ToolResult(
    val actionId: String,                   // Matches ToolAction.id
    val toolName: String,
    val success: Boolean,
    val result: String,                     // Human-readable result
    val data: Map<String, String> = emptyMap(), // Structured result data
    val error: String? = null,              // Error message if failed
    val durationMs: Long = 0                // Execution duration
)