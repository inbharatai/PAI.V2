package com.unoone.agent.storage.cache

import org.junit.Assert.assertEquals
import org.junit.Test

class EncryptedDbPolicyTest {
    @Test fun existingDataRequiresOriginalUnwrappedKey() {
        for (outcome in KeyOutcome.values()) assertEquals(
            if (outcome == KeyOutcome.UNWRAPPED) EncryptedDbPolicy.Action.OPEN else EncryptedDbPolicy.Action.RECOVERY_REQUIRED,
            EncryptedDbPolicy.decide(true, outcome))
    }
    @Test fun historicalResetNeverAuthorizesReinitialization() {
        assertEquals(EncryptedDbPolicy.Action.RECOVERY_REQUIRED, EncryptedDbPolicy.decide(false, KeyOutcome.RESET))
        assertEquals(EncryptedDbPolicy.Action.OPEN, EncryptedDbPolicy.decide(false, KeyOutcome.CREATED))
        assertEquals(EncryptedDbPolicy.Action.OPEN, EncryptedDbPolicy.decide(false, KeyOutcome.UNWRAPPED))
    }
}
