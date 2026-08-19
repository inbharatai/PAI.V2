package com.unoone.agent.storage.cache

import org.junit.Assert.assertEquals
import org.junit.Test

/** Every row of the open-decision table, pinned. */
class EncryptedDbPolicyTest {

    @Test
    fun `no database file opens fresh regardless of key outcome`() {
        for (outcome in KeyOutcome.values()) {
            assertEquals(
                "no file + $outcome",
                EncryptedDbPolicy.Action.OPEN,
                EncryptedDbPolicy.decide(dbFileExists = false, keyOutcome = outcome),
            )
        }
    }

    @Test
    fun `existing file with an unwrapped key opens normally`() {
        assertEquals(
            EncryptedDbPolicy.Action.OPEN,
            EncryptedDbPolicy.decide(dbFileExists = true, keyOutcome = KeyOutcome.UNWRAPPED),
        )
    }

    @Test
    fun `existing file with a freshly created key is the pre-encryption upgrade - reset`() {
        // CREATED + file present = the DB predates encryption (plaintext).
        // It cannot be opened with the SQLCipher factory; the cache resets,
        // exactly like the routine clear-on-USB-detach.
        assertEquals(
            EncryptedDbPolicy.Action.RESET_DB_THEN_OPEN,
            EncryptedDbPolicy.decide(dbFileExists = true, keyOutcome = KeyOutcome.CREATED),
        )
    }

    @Test
    fun `existing file after a key reset is unreadable ciphertext - reset`() {
        assertEquals(
            EncryptedDbPolicy.Action.RESET_DB_THEN_OPEN,
            EncryptedDbPolicy.decide(dbFileExists = true, keyOutcome = KeyOutcome.RESET),
        )
    }
}
