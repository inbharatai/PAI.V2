package com.unoone.agent.storage.cache

/**
 * Pure decision table for opening the encrypted cache database. Separated
 * from DatabaseProvider so every row is JVM-unit-tested; the Android side
 * only executes the chosen action.
 */
object EncryptedDbPolicy {

    enum class Action {
        /** Open with the passphrase; nothing else to do. */
        OPEN,

        /** Delete the database (and siblings) first, then open fresh. */
        RESET_DB_THEN_OPEN,
    }

    /**
     * (database file exists) x (key outcome) -> action.
     *
     * [KeyOutcome.CREATED] with an existing file means the database predates
     * encryption (a plaintext SQLite file); [KeyOutcome.RESET] means the
     * wrapped key was lost, so the ciphertext on disk has no key. In both
     * cases the file cannot be opened under the current key and the cache
     * starts fresh — the same effect as the routine clear-on-USB-detach
     * (VaultCacheLifecycle), applied once. The old bytes are deliberately NOT
     * retained: keeping a plaintext file behind would defeat the at-rest
     * guarantee this policy exists to provide, and the data is bounded-life
     * cache by design (24h TTL, cleared on every detach).
     */
    fun decide(dbFileExists: Boolean, keyOutcome: KeyOutcome): Action = when {
        !dbFileExists -> Action.OPEN
        keyOutcome == KeyOutcome.UNWRAPPED -> Action.OPEN
        else -> Action.RESET_DB_THEN_OPEN // CREATED or RESET with a file present
    }
}
