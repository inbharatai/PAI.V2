package com.unoone.agent.core.agent

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for [LanguageNormalizer] and [NormalizedInput].
 *
 * Verifies language detection, confidence thresholding, transcript cleaning,
 * reply-language rules, and constrained-generation handling.
 */
class LanguageNormalizerTest {

    // ── Language detection ───────────────────────────────────────────────

    @Test
    fun detectEnglish_defaultFallback() {
        val result = LanguageNormalizer.normalize("open whatsapp", 0.9f)
        assertEquals("en", result.detectedLanguage)
    }

    @Test
    fun detectHindi_devanagari() {
        val result = LanguageNormalizer.normalize("व्हाट्सएप पर संदेश भेजो", 0.9f)
        assertEquals("hi", result.detectedLanguage)
    }

    @Test
    fun detectBengali_bengaliScript() {
        val result = LanguageNormalizer.normalize("আমার নোট দেখাও", 0.9f)
        assertEquals("bn", result.detectedLanguage)
    }

    @Test
    fun detectTamil_tamilScript() {
        val result = LanguageNormalizer.normalize("என் குறிப்பை காட்டு", 0.9f)
        assertEquals("ta", result.detectedLanguage)
    }

    @Test
    fun detectTelugu_teluguScript() {
        val result = LanguageNormalizer.normalize("నా గమనిక చూపించు", 0.9f)
        assertEquals("te", result.detectedLanguage)
    }

    @Test
    fun detectKannada_kannadaScript() {
        val result = LanguageNormalizer.normalize("ನನ್ನ ಟಿಪ್ಪಣಿ ತೋರಿಸು", 0.9f)
        assertEquals("kn", result.detectedLanguage)
    }

    @Test
    fun detectMalayalam_malayalamScript() {
        val result = LanguageNormalizer.normalize("എന്റെ കുറിപ്പ് കാണിക്കൂ", 0.9f)
        assertEquals("ml", result.detectedLanguage)
    }

    @Test
    fun detectHindi_romanized() {
        val result = LanguageNormalizer.normalize("mujhe whatsapp message bhejna hai", 0.8f)
        assertEquals("hi", result.detectedLanguage)
    }

    @Test
    fun detectBengali_romanized() {
        // Requires >=2 Bengali keywords to avoid false positives (e.g. Hindi "ki").
        // "ami" and "koro" are both Bengali keywords, satisfying the threshold.
        val result = LanguageNormalizer.normalize("ami koro note dekhte chai", 0.7f)
        assertEquals("bn", result.detectedLanguage)
    }

    // ── Reply language ──────────────────────────────────────────────────

    @Test
    fun replyLanguage_matchesDetected_byDefault() {
        val result = LanguageNormalizer.normalize("send a message", 0.9f)
        assertEquals(result.detectedLanguage, result.expectedReplyLanguage)
    }

    @Test
    fun replyLanguage_hindi_remainsHindi() {
        val result = LanguageNormalizer.normalize("whatsapp karo Rahul ko", 0.8f)
        assertEquals("hi", result.expectedReplyLanguage)
    }

    @Test
    fun replyLanguage_bengali_fallsBackToEnglish() {
        // Bengali is a constrained generation language → model should reply in English
        // Uses >=2 Bengali keywords ("ami" + "koro") to meet the minKeywordCount threshold.
        val result = LanguageNormalizer.normalize("ami koro note dekhte chai", 0.7f)
        assertEquals("bn", result.detectedLanguage)
        assertEquals("en", result.expectedReplyLanguage)
    }

    @Test
    fun replyLanguage_explicitSwitchToEnglish() {
        val result = LanguageNormalizer.normalize("hindi mein bolo, answer in english", 0.9f)
        assertEquals("en", result.expectedReplyLanguage)
    }

    @Test
    fun replyLanguage_explicitSwitchToHindi() {
        val result = LanguageNormalizer.normalize("tell me in hindi about my notes", 0.9f)
        assertEquals("hi", result.expectedReplyLanguage)
    }

    // ── Confidence thresholding ────────────────────────────────────────

    @Test
    fun highConfidence_isNotLow() {
        val result = LanguageNormalizer.normalize("open whatsapp", 0.9f)
        assertFalse("0.9 confidence should not be low confidence", result.isLowConfidence)
    }

    @Test
    fun lowConfidence_isFlagged() {
        val result = LanguageNormalizer.normalize("open whatsapp", 0.3f)
        assertTrue("0.3 confidence should be low confidence", result.isLowConfidence)
    }

    @Test
    fun exactlyAtThreshold_isNotLow() {
        val result = LanguageNormalizer.normalize("open whatsapp", 0.5f)
        assertFalse("0.5 confidence (at threshold) should not be low", result.isLowConfidence)
    }

    @Test
    fun justBelowThreshold_isLow() {
        val result = LanguageNormalizer.normalize("open whatsapp", 0.49f)
        assertTrue("0.49 confidence should be low", result.isLowConfidence)
    }

    @Test
    fun zeroConfidence_isLow() {
        val result = LanguageNormalizer.normalize("open whatsapp", 0.0f)
        assertTrue("0.0 confidence should be low", result.isLowConfidence)
    }

    // ── Transcript cleaning ────────────────────────────────────────────

    @Test
    fun cleaning_removesFillerWords() {
        val result = LanguageNormalizer.normalize("um open whatsapp uh please", 0.9f)
        assertFalse("Cleaned transcript should not contain 'um'", result.normalisedTranscript.contains("um "))
        assertFalse("Cleaned transcript should not contain 'uh'", result.normalisedTranscript.contains("uh "))
    }

    @Test
    fun cleaning_preservesOriginal() {
        val original = "  um open whatsapp  "
        val result = LanguageNormalizer.normalize(original, 0.9f)
        assertEquals(original.trim(), result.originalTranscript)
    }

    @Test
    fun cleaning_normalizesWhitespace() {
        val result = LanguageNormalizer.normalize("open   whatsapp   now", 0.9f)
        assertFalse("Should not have multiple consecutive spaces",
            result.normalisedTranscript.contains("  "))
    }

    // ── Non-English reply detection ─────────────────────────────────────

    @Test
    fun isNonEnglishReply_trueForHindi() {
        val result = LanguageNormalizer.normalize("whatsapp karo Rahul ko", 0.8f)
        assertTrue("Hindi reply should be non-English", result.isNonEnglishReply)
    }

    @Test
    fun isNonEnglishReply_falseForEnglish() {
        val result = LanguageNormalizer.normalize("open whatsapp", 0.9f)
        assertFalse("English reply should not be non-English", result.isNonEnglishReply)
    }

    @Test
    fun requiresLanguageDirective_trueForNonEnglish() {
        val result = LanguageNormalizer.normalize("whatsapp karo", 0.8f)
        assertTrue("Hindi command should require language directive", result.requiresLanguageDirective)
    }

    // ── Edge cases ──────────────────────────────────────────────────────

    @Test
    fun emptyTranscript_fallsBackToCurrentVoiceLanguage() {
        val result = LanguageNormalizer.normalize("", 0.9f, "hi")
        assertEquals("hi", result.detectedLanguage)
    }

    @Test
    fun mixedScript_devanagariWins() {
        // Devanagari takes precedence over Romanized keywords
        val result = LanguageNormalizer.normalize("send काल message", 0.9f)
        assertEquals("hi", result.detectedLanguage)
    }

    @Test
    fun confidencePreserved_inResult() {
        val result = LanguageNormalizer.normalize("open whatsapp", 0.75f)
        assertEquals(0.75f, result.confidence, 0.001f)
    }

    @Test
    fun supportedLanguages_containsCoreLanguages() {
        assertTrue("Should contain English", LanguageNormalizer.SUPPORTED_LANGUAGES.contains("en"))
        assertTrue("Should contain Hindi", LanguageNormalizer.SUPPORTED_LANGUAGES.contains("hi"))
        assertTrue("Should contain Marathi", LanguageNormalizer.SUPPORTED_LANGUAGES.contains("mr"))
        assertTrue("Should contain Bengali", LanguageNormalizer.SUPPORTED_LANGUAGES.contains("bn"))
        assertTrue("Should contain Tamil", LanguageNormalizer.SUPPORTED_LANGUAGES.contains("ta"))
        assertTrue("Should contain Telugu", LanguageNormalizer.SUPPORTED_LANGUAGES.contains("te"))
        assertTrue("Should contain Kannada", LanguageNormalizer.SUPPORTED_LANGUAGES.contains("kn"))
        assertTrue("Should contain Malayalam", LanguageNormalizer.SUPPORTED_LANGUAGES.contains("ml"))
    }

    @Test
    fun constrainedGenerationLanguages_containsBengaliAssamese() {
        assertTrue("Bengali should be constrained", LanguageNormalizer.CONSTRAINED_GENERATION_LANGUAGES.contains("bn"))
        assertTrue("Assamese should be constrained", LanguageNormalizer.CONSTRAINED_GENERATION_LANGUAGES.contains("as"))
    }

    @Test
    fun threshold_isExactlyPointFive() {
        assertEquals(0.5f, LanguageNormalizer.CONFIDENCE_THRESHOLD, 0.001f)
    }

    // ── M5: Bengali "ki" false-positive ────────────────────────────────────

    @Test
    fun hindiKi_doesNotTriggerBengali() {
        // Hindi "ki" (meaning "that") should NOT classify input as Bengali.
        // With the minKeywordCount threshold, a single ambiguous word won't trigger Bengali.
        val result = LanguageNormalizer.normalize("mujhe batao ki Rahul kahan hai", 0.8f)
        assertEquals("Hindi 'ki' should not trigger Bengali detection", "hi", result.detectedLanguage)
    }

    @Test
    fun singleBengaliWord_doesNotTriggerBengali() {
        // A single Bengali keyword ("ami") alone should not meet the >=2 threshold.
        val result = LanguageNormalizer.normalize("ami wants to go there", 0.7f)
        assertTrue("Single Bengali keyword should not trigger Bengali detection",
            result.detectedLanguage != "bn")
    }

    // ── M4: Filler words "like" and "actually" preserved ──────────────────

    @Test
    fun fillerWord_like_isPreserved() {
        // "like" carries meaning ("open something like WhatsApp") and must NOT be stripped.
        val result = LanguageNormalizer.normalize("open something like WhatsApp", 0.9f)
        assertTrue("'like' should be preserved in transcript",
            result.normalisedTranscript.contains("like"))
    }

    @Test
    fun fillerWord_actually_isPreserved() {
        // "actually" carries emphasis and must NOT be stripped.
        val result = LanguageNormalizer.normalize("actually open calendar", 0.9f)
        assertTrue("'actually' should be preserved in transcript",
            result.normalisedTranscript.contains("actually"))
    }

    @Test
    fun fillerWord_um_isStillRemoved() {
        // True filler sounds like "um" should still be removed.
        val result = LanguageNormalizer.normalize("um open whatsapp", 0.9f)
        assertFalse("'um' should be removed from transcript",
            result.normalisedTranscript.contains("um"))
    }
}