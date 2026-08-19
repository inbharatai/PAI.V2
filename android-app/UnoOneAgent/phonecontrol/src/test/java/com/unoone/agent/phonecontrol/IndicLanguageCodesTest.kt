package com.unoone.agent.phonecontrol

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM-unit test for the [INDIC_LANGUAGE_CODES] set that drives language-aware OCR selection.
 * M15: describe_scene missing Indic script OCR — the set determines which voice languages
 * trigger the Devanagari recognizer alongside Latin.
 */
class IndicLanguageCodesTest {

    @Test
    fun indicLanguagesAreRecognized() {
        assertTrue("Hindi must trigger Indic OCR", "hi" in INDIC_LANGUAGE_CODES)
        assertTrue("Marathi must trigger Indic OCR", "mr" in INDIC_LANGUAGE_CODES)
        assertTrue("Bengali must trigger Indic OCR", "bn" in INDIC_LANGUAGE_CODES)
        assertTrue("Tamil must trigger Indic OCR", "ta" in INDIC_LANGUAGE_CODES)
        assertTrue("Telugu must trigger Indic OCR", "te" in INDIC_LANGUAGE_CODES)
        assertTrue("Kannada must trigger Indic OCR", "kn" in INDIC_LANGUAGE_CODES)
        assertTrue("Malayalam must trigger Indic OCR", "ml" in INDIC_LANGUAGE_CODES)
    }

    @Test
    fun englishDoesNotTriggerIndicOcr() {
        assertFalse("English must NOT trigger Indic OCR", "en" in INDIC_LANGUAGE_CODES)
    }

    @Test
    fun setIsNotEmpty() {
        assertTrue("INDIC_LANGUAGE_CODES must not be empty", INDIC_LANGUAGE_CODES.isNotEmpty())
    }
}