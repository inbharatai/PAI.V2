@file:Suppress("unused")

package com.unoone.pai.contracts

/**
 * Core contracts for UnoOne Pocket AI.
 *
 * This package defines the shared data types that both UnoOne Mobile (Android)
 * and UnoOne Power (Desktop) must use to read and write the encrypted USB vault.
 *
 * Key design principles:
 * 1. Every record includes VaultRecordMetadata for cross-platform sync and conflict detection
 * 2. Every record uses kotlinx.serialization for JSON interoperability
 * 3. No Android dependencies — pure Kotlin/JVM
 * 4. No raw model output executes tools — all actions go through ToolAction → SafetyGuard
 * 5. Deletion uses tombstones that propagate across platforms
 * 6. Platform-specific adapters translate between model output and canonical contracts
 */

// All contract types are defined in their respective files:
// VaultRecordMetadata, Platform → VaultRecord.kt
// Memory, MemoryType → Memory.kt
// Conversation, ConversationMessage, MessageRole, ToolCallRecord → Conversation.kt
// Task, TaskStatus, TaskStep, StepStatus, TaskFailure → Task.kt
// Recording, RecordingType, RecordingPrivacy, RecordingBookmark, RecordingSummary → Recording.kt
// Transcript, TranscriptSegment → Transcript.kt
// Skill, SkillStep → Skill.kt
// Document, DocumentType → Document.kt
// Preferences, SecurityLevel, Theme → Preferences.kt
// ToolAction, SafetyDecision, ToolRiskLevel, ToolResult → ToolAction.kt
// AuditRecord → AuditRecord.kt
// VaultMetadata, KdfParams, DeviceSession, ManifestEntry → VaultMetadata.kt
// PocketMemoryVault, VaultSetupResult, VaultUnlockResult, IntegrityCheckResult → PocketMemoryVault.kt