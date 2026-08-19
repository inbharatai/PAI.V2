package com.unoone.pai.contracts

import kotlinx.serialization.Serializable

/**
 * Audit record for all tool executions and safety decisions.
 *
 * Stored in the encrypted vault for accountability.
 * Audit records are append-only and cannot be modified after creation.
 */
@Serializable
data class AuditRecord(
    val metadata: VaultRecordMetadata,
    val action: String,                      // Tool action or event description
    val inputHash: String,                   // SHA-256 hash of raw input (never store plaintext)
    val toolName: String? = null,           // Tool that was executed
    val safetyDecision: SafetyDecision? = null,
    val riskLevel: ToolRiskLevel? = null,
    val confirmationResult: Boolean? = null, // Did user confirm? (null if no confirmation needed)
    val result: String,                      // "success" | "error" | "blocked"
    val resultDetail: String? = null,        // Additional detail
    val durationMs: Long = 0,
    val modelUsed: String? = null,           // Which model generated this action
    val sessionId: String                   // Vault session ID
)