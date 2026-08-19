package com.unoone.pai.contracts

import kotlinx.serialization.Serializable

/**
 * A task that can be started on one platform and continued on another.
 *
 * Tasks persist enough structured state to reconstruct context
 * across devices without transferring raw model RAM state.
 */
@Serializable
data class Task(
    val metadata: VaultRecordMetadata,
    val title: String,
    val description: String,
    val status: TaskStatus,
    val steps: List<TaskStep>,
    val originalInstruction: String,   // The user's original request
    val conversationSummary: String? = null,
    val relevantMemoryIds: List<String> = emptyList(),
    val relevantDocumentIds: List<String> = emptyList(),
    val generatedFiles: List<String> = emptyList(),  // Vault paths
    val approvalsReceived: List<String> = emptyList(), // Step IDs that were approved
    val failures: List<TaskFailure> = emptyList(),
    val language: String = "en"
)

@Serializable
enum class TaskStatus {
    PENDING,
    IN_PROGRESS,
    WAITING_APPROVAL,
    COMPLETED,
    FAILED,
    CANCELLED
}

@Serializable
data class TaskStep(
    val id: String,                      // UUID v7
    val description: String,
    val status: StepStatus,
    val toolName: String? = null,        // Tool used for this step
    val toolOutput: String? = null,      // Result summary (not raw output)
    val startedAt: String? = null,       // ISO-8601
    val completedAt: String? = null       // ISO-8601
)

@Serializable
enum class StepStatus {
    PENDING,
    IN_PROGRESS,
    COMPLETED,
    FAILED,
    SKIPPED,
    WAITING_APPROVAL
}

@Serializable
data class TaskFailure(
    val stepId: String,
    val error: String,
    val retryable: Boolean = false,
    val retried: Boolean = false
)