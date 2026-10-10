package com.unoone.agent.storage.cache

/** Non-destructive compatibility decision table. File/header admission is DatabaseOpenPolicy. */
object EncryptedDbPolicy {
    enum class Action { OPEN, RECOVERY_REQUIRED }
    fun decide(dbFileExists: Boolean, keyOutcome: KeyOutcome): Action = when {
        keyOutcome == KeyOutcome.RESET -> Action.RECOVERY_REQUIRED
        dbFileExists && keyOutcome != KeyOutcome.UNWRAPPED -> Action.RECOVERY_REQUIRED
        else -> Action.OPEN
    }
}
