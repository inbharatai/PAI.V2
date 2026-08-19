package com.unoone.pai.contracts

import kotlinx.serialization.Serializable

/**
 * Cross-platform user preferences.
 *
 * These preferences are stored in the vault and shared across
 * UnoOne Mobile and UnoOne Power. Changes made on one platform
 * are visible on the other.
 */
@Serializable
data class Preferences(
    val metadata: VaultRecordMetadata,
    val writtenLanguage: String = "en",        // BCP-47 for written output
    val spokenLanguage: String = "en",        // BCP-47 for spoken output
    val securityLevel: SecurityLevel = SecurityLevel.STANDARD,
    val theme: Theme = Theme.SYSTEM,
    val voiceEnabled: Boolean = true,
    val voiceSpeed: Float = 1.0f,             // TTS speed multiplier
    val recordingPrivacy: RecordingPrivacy = RecordingPrivacy.FULL,
    val autoLockMinutes: Int = 5,             // Auto-lock vault after N minutes of inactivity
    val emergencyStopEnabled: Boolean = true,
    val browserTrustedDomains: List<String> = emptyList(),
    val customInstructions: String? = null     // User's custom system instructions
)

@Serializable
enum class SecurityLevel {
    STANDARD,   // Default safety guards active
    RELAXED,    // Fewer confirmation prompts (still safe)
    OFF         // No safety guards (expert mode, risky)
}

@Serializable
enum class Theme {
    SYSTEM,
    LIGHT,
    DARK
}