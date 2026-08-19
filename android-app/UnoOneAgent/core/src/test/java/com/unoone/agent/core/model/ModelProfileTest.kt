package com.unoone.agent.core.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for [ModelProfile] and [ModelProfiles].
 *
 * Verifies per-tier limits, temperature settings, lookup, and fallback logic.
 */
class ModelProfileTest {

    @Test
    fun liteProfile_hasCorrectLimits() {
        val lite = ModelProfiles.LITE
        assertEquals("GEMMA_4_E2B", lite.id)
        assertEquals("Lite", lite.displayName)
        assertEquals(3, lite.maxCandidateTools)
        assertEquals(2, lite.maxAgentSteps)
        assertEquals(0, lite.maxBrowserSteps)
        assertEquals(1, lite.maxRepairAttempts)
        assertEquals(0.1f, lite.actionTemperature, 0.001f)
        assertEquals(0.3f, lite.chatTemperature, 0.001f)
        assertEquals(2_048, lite.contextTokens)
        assertEquals("simple", lite.complexityThreshold)
    }

    @Test
    fun mediumProfile_hasCorrectLimits() {
        val medium = ModelProfiles.MEDIUM
        assertEquals("GEMMA_4_E4B", medium.id)
        assertEquals("Medium", medium.displayName)
        assertEquals(6, medium.maxCandidateTools)
        assertEquals(4, medium.maxAgentSteps)
        assertEquals(8, medium.maxBrowserSteps)
        assertEquals(1, medium.maxRepairAttempts)
        assertEquals(0.1f, medium.actionTemperature, 0.001f)
        assertEquals(0.7f, medium.chatTemperature, 0.001f)
        assertEquals(4_096, medium.contextTokens)
        assertEquals("compound", medium.complexityThreshold)
    }

    @Test
    fun forId_byEnum_returnsCorrectProfile() {
        assertEquals(ModelProfiles.LITE, ModelProfiles.forId(BrainModelId.GEMMA_4_E2B))
        assertEquals(ModelProfiles.MEDIUM, ModelProfiles.forId(BrainModelId.GEMMA_4_E4B))
    }

    @Test
    fun forId_byString_returnsCorrectProfile() {
        assertEquals(ModelProfiles.LITE, ModelProfiles.forId("GEMMA_4_E2B"))
        assertEquals(ModelProfiles.MEDIUM, ModelProfiles.forId("GEMMA_4_E4B"))
    }

    @Test
    fun forId_unknownString_fallsBackToLite() {
        assertEquals(ModelProfiles.LITE, ModelProfiles.forId("UNKNOWN"))
        assertEquals(ModelProfiles.LITE, ModelProfiles.forId("gemma-4-e2b"))
    }

    @Test
    fun forId_nullString_fallsBackToLite() {
        assertEquals(ModelProfiles.LITE, ModelProfiles.forId(""))
    }

    @Test
    fun medium_allowsMoreStepsThanLite() {
        assertTrue(ModelProfiles.MEDIUM.maxAgentSteps > ModelProfiles.LITE.maxAgentSteps)
    }

    @Test
    fun medium_allowsMoreCandidateToolsThanLite() {
        assertTrue(ModelProfiles.MEDIUM.maxCandidateTools > ModelProfiles.LITE.maxCandidateTools)
    }

    @Test
    fun medium_hasHigherChatTemperature() {
        assertTrue(ModelProfiles.MEDIUM.chatTemperature > ModelProfiles.LITE.chatTemperature)
    }

    @Test
    fun both_haveSameActionTemperature() {
        assertEquals(ModelProfiles.LITE.actionTemperature, ModelProfiles.MEDIUM.actionTemperature, 0.001f)
    }

    @Test
    fun both_haveSameRepairAttempts() {
        assertEquals(ModelProfiles.LITE.maxRepairAttempts, ModelProfiles.MEDIUM.maxRepairAttempts)
    }

    @Test
    fun medium_hasLargerContextWindow() {
        assertTrue(ModelProfiles.MEDIUM.contextTokens > ModelProfiles.LITE.contextTokens)
    }

    @Test
    fun medium_allowsBrowserSteps() {
        assertTrue(ModelProfiles.MEDIUM.maxBrowserSteps > 0)
    }

    @Test
    fun lite_forbidsBrowserSteps() {
        assertEquals(0, ModelProfiles.LITE.maxBrowserSteps)
    }
}