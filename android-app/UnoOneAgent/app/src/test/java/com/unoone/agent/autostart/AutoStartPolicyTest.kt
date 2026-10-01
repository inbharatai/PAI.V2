package com.unoone.agent.autostart

import android.content.Intent
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

/**
 * P2-A: pins the auto-launch decision table. The receiver is thin glue;
 * these tests own the truth of when the agent may wake itself on boot.
 */
@RunWith(RobolectricTestRunner::class)
class AutoStartPolicyTest {

    @Test
    fun `shouldStart is false unless both the opt-in and the agent toggle are on`() {
        assertFalse(AutoStartPolicy.shouldStart(autoStartEnabled = false, agentEnabled = false))
        assertFalse(AutoStartPolicy.shouldStart(autoStartEnabled = true, agentEnabled = false))
        assertFalse(AutoStartPolicy.shouldStart(autoStartEnabled = false, agentEnabled = true))
        assertTrue(AutoStartPolicy.shouldStart(autoStartEnabled = true, agentEnabled = true))
    }

    @Test
    fun `isBootAction accepts both boot broadcasts the receiver declares`() {
        assertTrue(AutoStartPolicy.isBootAction(Intent.ACTION_BOOT_COMPLETED))
        assertTrue(AutoStartPolicy.isBootAction(AutoStartPolicy.ACTION_QUICKBOOT_POWERON))
    }

    @Test
    fun `isBootAction rejects everything else`() {
        assertFalse(AutoStartPolicy.isBootAction(null))
        assertFalse(AutoStartPolicy.isBootAction(Intent.ACTION_SCREEN_ON))
        assertFalse(AutoStartPolicy.isBootAction("com.example.CUSTOM"))
    }
}