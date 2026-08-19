package com.unoone.pai.contracts

import kotlinx.serialization.Serializable

/**
 * A user-defined skill (sequence of actions) in the shared vault.
 *
 * Skills are created by the user and can be triggered by voice or text.
 * They must work identically on both platforms.
 */
@Serializable
data class Skill(
    val metadata: VaultRecordMetadata,
    val name: String,
    val description: String,
    val triggerPhrases: List<String>,    // Voice or text triggers
    val steps: List<SkillStep>,
    val category: String? = null,
    val language: String = "en",
    val isActive: Boolean = true,
    val usageCount: Long = 0,
    val lastUsedAt: String? = null       // ISO-8601 instant
)

@Serializable
data class SkillStep(
    val id: String,                      // UUID v7
    val order: Int,                       // Execution order
    val action: String,                   // Canonical tool name
    val args: Map<String, String>,       // Action arguments
    val waitForConfirmation: Boolean = false // Require user confirmation before executing
)