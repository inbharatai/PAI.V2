package com.unoone.agent.core.agent

import com.unoone.agent.core.model.BrainModelId
import com.unoone.agent.core.model.BrainModelRegistry
import com.unoone.agent.core.model.ModelProfile
import com.unoone.agent.core.model.ModelProfiles

/**
 * Selects the appropriate model tier for a given command based on device capabilities
 * and command complexity.
 *
 * Key rules:
 * - The model is selected **before** a task starts and **never switches mid-task**.
 * - Simple/deterministic commands → Lite (E2B)
 * - Compound/unknown commands → Medium (E4B) if RAM allows
 * - If E4B fails mid-task: stop the task, preserve state, continue deterministic commands,
 *   offer Lite or retry.
 * - CHAT intent always uses the currently loaded model's conversation (no model switch for chat).
 */
object ModelTierSelector {

    /**
     * Minimum available RAM (MB) to attempt E4B loading. Below this, force Lite.
     * Matches [BrainModelRegistry.GEMMA_4_E4B.minimumRamMb] (8,192 MB).
     * No current phone provides 10,240 MB free after OS overhead, so the previous
     * 10,240 threshold made E4B unreachable in practice. 8,192 MB aligns with
     * the manifest's actual minimum and still excludes devices below 8 GB total.
     */
    const val E4B_MIN_RAM_MB: Int = 8_192

    /**
     * Select the appropriate model profile for a command.
     *
     * @param intent The classified intent from [CandidateToolSelector.TaskIntent].
     *   CHAT and simple deterministic intents always get Lite.
     *   MESSAGING, CALENDAR, SCREEN, WEB, UNKNOWN with compound phrasing get Medium if available.
     * @param e4bAvailable Whether E4B is loaded and ready on this device.
     * @param availableRamMb Available device RAM in MB. If below [E4B_MIN_RAM_MB], E4B is never selected.
     * @return The selected [ModelProfile].
     */
    fun select(
        intent: CandidateToolSelector.TaskIntent,
        e4bAvailable: Boolean,
        availableRamMb: Int
    ): ModelProfile {
        // CHAT never needs a model switch — use whatever is loaded (Lite by default)
        if (intent == CandidateToolSelector.TaskIntent.CHAT) {
            return ModelProfiles.LITE
        }

        // Simple deterministic intents don't need E4B's reasoning power
        val simpleIntents = setOf(
            CandidateToolSelector.TaskIntent.PHONE,
            CandidateToolSelector.TaskIntent.CAMERA,
            CandidateToolSelector.TaskIntent.ACCESSIBILITY
        )
        if (intent in simpleIntents) {
            return ModelProfiles.LITE
        }

        // Compound intents benefit from E4B's reasoning if available
        val compoundIntents = setOf(
            CandidateToolSelector.TaskIntent.MESSAGING,
            CandidateToolSelector.TaskIntent.CALENDAR,
            CandidateToolSelector.TaskIntent.NOTES,
            CandidateToolSelector.TaskIntent.SCREEN,
            CandidateToolSelector.TaskIntent.WEB,
            CandidateToolSelector.TaskIntent.DOCUMENT,
            CandidateToolSelector.TaskIntent.SKILL,
            CandidateToolSelector.TaskIntent.UNKNOWN
        )

        // availableRamMb == -1 means "unknown" (no ActivityManager data yet). Since -1 < E4B_MIN_RAM_MB,
        // E4B is automatically blocked when RAM is unknown, which is the safe default.
        if (intent in compoundIntents && e4bAvailable && availableRamMb >= E4B_MIN_RAM_MB) {
            return ModelProfiles.MEDIUM
        }

        // Default: Lite
        return ModelProfiles.LITE
    }

    /**
     * Select the appropriate model profile for a raw command string.
     * Convenience wrapper that infers the intent first.
     */
    fun selectForCommand(
        command: String,
        e4bAvailable: Boolean,
        availableRamMb: Int
    ): ModelProfile {
        val intent = CandidateToolSelector.inferIntent(command)
        return select(intent, e4bAvailable, availableRamMb)
    }

    /**
     * Result of a model switch attempt.
     * SWITCHED: successfully switched to the requested model.
     * ALREADY_ACTIVE: the requested model is already the active model.
     * INSUFFICIENT_RAM: the device doesn't have enough RAM for the requested model.
     * LOAD_FAILED: the model failed to load (engine initialization error).
     */
    sealed class SwitchResult {
        data class Switched(val profile: ModelProfile) : SwitchResult()
        data class AlreadyActive(val profile: ModelProfile) : SwitchResult()
        data class InsufficientRam(val requiredMb: Int, val availableMb: Int) : SwitchResult()
        data class LoadFailed(val error: String) : SwitchResult()
    }
}