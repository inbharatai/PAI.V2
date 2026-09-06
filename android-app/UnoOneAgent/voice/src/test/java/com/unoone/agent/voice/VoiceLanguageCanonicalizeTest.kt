package com.unoone.agent.voice

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Mirrors the canonical-language regression suite in
 * `packages/speech-contracts/src/lib.rs` (the desktop Rust route). The same
 * user input must canonicalize identically on Android and desktop; a drift
 * here is a cross-platform contract break, not a cosmetic difference.
 * The alias *table* itself is byte-checked against `languages.v1.json` by
 * `scripts/check_speech_language_sync.py` in CI — these tests pin behavior.
 */
class VoiceLanguageCanonicalizeTest {

    // ---- Assamese addressable as both `as` and `as-IN` ----
    @Test
    fun asAliasMapsToAsIn() {
        for (input in listOf("as", "as-IN", "as-in", "AS-in", "As-In")) {
            assertEquals("input '$input' must address Assamese", "as-IN", VoiceLanguage.canonicalize(input))
        }
    }

    @Test
    fun hiAliasMapsToHiIn() {
        for (input in listOf("hi", "hi-IN", "hi-in")) {
            assertEquals("hi-IN", VoiceLanguage.canonicalize(input))
        }
    }

    @Test
    fun hinglishMapsToHiEnCodemix() {
        assertEquals("hi-en-codemix", VoiceLanguage.canonicalize("hinglish"))
        // Idempotent on the canonical form.
        assertEquals("hi-en-codemix", VoiceLanguage.canonicalize("hi-en-codemix"))
    }

    // ---- Global languages must NOT become `xx-IN` ----
    @Test
    fun frNeverBecomesFrIn() {
        assertEquals("fr", VoiceLanguage.canonicalize("fr"))
        assertEquals("fr", VoiceLanguage.canonicalize("FR"))
        assertEquals("fr-FR", VoiceLanguage.canonicalize("fr-FR"))
    }

    @Test
    fun enUsStaysEnUs() {
        assertEquals("en-US", VoiceLanguage.canonicalize("en-US"))
        assertEquals("en-US", VoiceLanguage.canonicalize("en-us"))
    }

    // ---- Fail-closed language handling ----
    @Test
    fun emptyLanguageIsRejectedNotBypassed() {
        assertNull(VoiceLanguage.canonicalize(null))
        assertNull(VoiceLanguage.canonicalize(""))
        assertNull(VoiceLanguage.canonicalize("   "))
    }

    @Test
    fun malformedTagsAreRejected() {
        for (bad in listOf("f", "french--", "toolongsubtag1", "1n", "a-b-c-", "french café")) {
            assertNull("'$bad' must be rejected", VoiceLanguage.canonicalize(bad))
        }
    }

    @Test
    fun autoSentinelIsReservedAndPassthrough() {
        val auto = VoiceLanguage.canonicalize("auto")
        assertTrue(VoiceLanguage.isAuto(auto))
        assertEquals("auto", auto)
    }

    @Test
    fun canonicalizationIsIdempotent() {
        for (input in listOf("as", "hi", "hinglish", "en-US", "fr", "hi-IN", "as-IN")) {
            val once = VoiceLanguage.canonicalize(input)
            assertEquals(
                "canonicalize(canonicalize('$input')) must be stable",
                once,
                VoiceLanguage.canonicalize(once)
            )
        }
    }

    // ---- Table invariants (structure; values are sync-checked in CI) ----
    @Test
    fun everyAliasTargetIsIdempotentlyDefined() {
        VoiceLanguage.CANONICAL_ALIASES.forEach { (alias, target) ->
            assertEquals(
                "alias '$alias' target '$target' must itself be an idempotent table entry",
                target,
                VoiceLanguage.CANONICAL_ALIASES[target]
            )
        }
    }

    @Test
    fun caseVariantsOfOneAliasAgreeOnTheTarget() {
        // The table deliberately lists case variants (`as-in` and `as-IN`);
        // they must never disagree on where they point.
        val grouped = VoiceLanguage.CANONICAL_ALIASES.entries
            .groupBy({ it.key.lowercase() }, { it.value })
        grouped.forEach { (lowered, targets) ->
            assertEquals(
                "case variants of '$lowered' must agree on one target",
                1,
                targets.toSet().size
            )
        }
        assertEquals(12, VoiceLanguage.CANONICAL_ALIASES.size)
    }

    // ---- normalize() now routes through the canonical contract (finding A8) ----
    @Test
    fun normalizeRoutesThroughCanonicalize() {
        // `normalize` is the app-internal en/hi policy selector, but it derives
        // from `canonicalize` — it must never re-root a valid alias of a
        // supported language to the wrong voice.
        assertEquals("hi", VoiceLanguage.normalize("hi"))
        assertEquals("hi", VoiceLanguage.normalize("hi-IN"))
        assertEquals("hi", VoiceLanguage.normalize("hi-in"))
        assertEquals("hi", VoiceLanguage.normalize("hinglish"))
        assertEquals("hi", VoiceLanguage.normalize("hi-en-codemix"))
        assertEquals("en", VoiceLanguage.normalize("en"))
        assertEquals("en", VoiceLanguage.normalize("en-IN"))
        assertEquals("en", VoiceLanguage.normalize(null))
        assertEquals("hi-IN", VoiceLanguage.canonicalize("hi"))
    }

    @Test
    fun normalizeStillFallsBackForUnenabledLanguages() {
        // Well-formed but not an enabled Android voice (as-IN has no production
        // IndicConformer engine yet; fr is global) — policy falls back to the
        // default voice rather than pretending to speak it.
        assertEquals("en", VoiceLanguage.normalize("as-IN"))
        assertEquals("en", VoiceLanguage.normalize("as"))
        assertEquals("en", VoiceLanguage.normalize("fr"))
        // Malformed tags fall back too — the pref must always yield a language.
        assertEquals("en", VoiceLanguage.normalize("french café"))
        assertEquals("en", VoiceLanguage.normalize(""))
    }
}