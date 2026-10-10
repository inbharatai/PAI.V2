package com.unoone.agent.model

import com.unoone.agent.core.modeladmission.ManualAdmission
import com.unoone.agent.modelmanager.ModelInstaller

/** The direct worker lane, independently runnable on the host; no UI precondition is trusted.
 * Production supplies native probe + current local policy, never WorkManager data as evidence.
 */
object ModelDownloadLane {
    suspend fun run(
        modelId: String?,
        enabled: () -> Boolean,
        stopped: () -> Boolean,
        freshAdmission: () -> ManualAdmission.Decision,
        transfer: suspend () -> ModelInstaller.InstallResult
    ): ModelInstaller.InstallResult {
        if (modelId.isNullOrBlank()) return ModelInstaller.InstallResult.Failure("Missing model id")
        if (!enabled() || stopped()) return ModelInstaller.InstallResult.Failure("Disabled or cancelled")
        val admission = try { freshAdmission() } catch (_: Exception) {
            return ModelInstaller.InstallResult.Failure("Device Check unavailable; paused")
        }
        if (!admission.allowed) return ModelInstaller.InstallResult.Failure(admission.reason)
        if (!enabled() || stopped()) return ModelInstaller.InstallResult.Failure("Disabled or cancelled")
        return transfer()
    }
}
