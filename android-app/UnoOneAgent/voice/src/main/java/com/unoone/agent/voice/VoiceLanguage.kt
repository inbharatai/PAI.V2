package com.unoone.agent.voice

import com.unoone.agent.core.util.Logger
import com.unoone.agent.voice.stt.SttMode

/**
 * Temporary compatibility mapping for the speech models already supported by UnoOne.
 *
 * The next migration phase replaces this hard-coded catalogue with signed downloadable language
 * packs. Until then, every path here must match `models_manifest.json` exactly so the normalized
 * model filesystem does not break the existing offline voice runtime.
 */
data class AsrSpec(val folder: String, val mode: SttMode, val language: String)

object VoiceLanguage {
    data class LanguageRequest(
        val code: String,
        val remainingCommand: String
    )

    const val PREF_NAME = "unoone_settings"
    const val PREF_KEY = "voice_language"
    const val DEFAULT = "en"

    data class Lang(val code: String, val display: String)

    val SUPPORTED: List<Lang> = listOf(
        Lang("en", "English"),
        Lang("hi", "Hindi")
    )

    // -------------------------------------------------------------------------
    // Canonical language contract — Kotlin mirror of the PAI speech table.
    //
    // This table MUST stay byte-identical to `packages/speech-contracts/
    // languages.v1.json` (the single source of truth shared with the desktop
    // Rust route). CI enforces it with `scripts/check_speech_language_sync.py`;
    // Android and desktop must never disagree on what a language tag means.
    //
    // Contract semantics (identical to the Rust canonicalize()):
    // - alias lookup is ASCII case-insensitive; the canonical form is
    //   returned verbatim (`as`, `as-in`, `as-IN` all → `as-IN`),
    // - tags absent from the table pass through with BCP-47 case normalization
    //   and are NEVER re-rooted to a region (`fr` stays `fr`, `en-US` stays
    //   `en-US`),
    // - malformed or empty tags are rejected (null), never guessed,
    // - `auto` is the reserved detect sentinel.
    // -------------------------------------------------------------------------

    // canonical-aliases-begin (sync-checked against languages.v1.json)
    val CANONICAL_ALIASES: Map<String, String> = mapOf(
        "as" to "as-IN",
        "as-in" to "as-IN",
        "as-IN" to "as-IN",
        "hi" to "hi-IN",
        "hi-in" to "hi-IN",
        "hi-IN" to "hi-IN",
        "hinglish" to "hi-en-codemix",
        "hi-en-codemix" to "hi-en-codemix",
        "en" to "en-IN",
        "en-in" to "en-IN",
        "en-IN" to "en-IN",
        "auto" to "auto"
    )
    // canonical-aliases-end

    /**
     * Canonicalizes a user- or manifest-supplied language tag. Returns null for
     * empty or malformed tags — the caller must fail closed, never guess.
     */
    fun canonicalize(code: String?): String? {
        val trimmed = code?.trim().orEmpty()
        if (trimmed.isEmpty()) return null
        CANONICAL_ALIASES[asciiLower(trimmed)]?.let { return it }
        return normalizePassthrough(trimmed)
    }

    /** True when the tag is `auto`, the reserved detect-language sentinel. */
    fun isAuto(canonical: String?): Boolean = canonical == AUTO_SENTINEL

    const val AUTO_SENTINEL = "auto"

    /**
     * BCP-47 pass-through normalization for tags outside the alias table.
     * Mirrors the Rust `normalize_passthrough` exactly: subtags are `-`
     * separated, 1–8 ASCII alphanumeric characters, first subtag 2–8 letters;
     * language lowercase, script Titlecase, region UPPERCASE. Null on any
     * violation.
     */
    private fun normalizePassthrough(tag: String): String? {
        val normalized = mutableListOf<String>()
        tag.split('-').forEachIndexed { index, subtag ->
            val ascii = subtag.all { it.isAsciiAlphanumeric() }
            when {
                subtag.isEmpty() || subtag.length > 8 || !ascii -> return null
                index == 0 -> {
                    if (subtag.length < 2 || !subtag.all { it.isAsciiLetter() }) return null
                    normalized += asciiLower(subtag)
                }
                subtag.length == 2 -> normalized +=
                    if (subtag.all { it.isAsciiLetter() }) asciiUpper(subtag) else asciiLower(subtag)
                subtag.length == 4 && subtag.all { it.isAsciiLetter() } ->
                    normalized += asciiUpper(subtag.take(1)) + asciiLower(subtag.drop(1))
                else -> normalized += asciiLower(subtag)
            }
        }
        return normalized.joinToString("-")
    }

    private fun asciiLower(text: String): String =
        text.map { if (it in 'A'..'Z') it + 32 else it }.joinToString("")

    private fun asciiUpper(text: String): String =
        text.map { if (it in 'a'..'z') it - 32 else it }.joinToString("")

    private fun Char.isAsciiLetter(): Boolean = this in 'a'..'z' || this in 'A'..'Z'

    private fun Char.isAsciiAlphanumeric(): Boolean =
        isAsciiLetter() || this in '0'..'9'

    private val ttsFolderByCode: Map<String, String> = mapOf(
        "en" to "speech/languages/en-IN/tts",
        "hi" to "speech/languages/hi-IN/tts"
    )

    fun ttsFolder(lang: String): String =
        ttsFolderByCode[lang] ?: ttsFolderByCode.getValue(DEFAULT)

    fun asrSpec(lang: String): AsrSpec =
        if (lang == "en") {
            AsrSpec("speech/shared/sherpa-asr-en", SttMode.TRANSDUCER, "en")
        } else {
            AsrSpec("speech/shared/sherpa-asr-indic", SttMode.OMNILINGUAL, lang)
        }

    const val KWS_FOLDER = "speech/shared/vad"

    /**
     * Wake-word models in priority order. The dedicated KWS download and the English streaming
     * ASR use the same transducer files, so the already-installed English model is a safe offline
     * fallback when the optional `vad` model was not downloaded.
     */
    fun kwsFolders(): List<String> = listOf(KWS_FOLDER, asrSpec(DEFAULT).folder).distinct()

    fun isSupported(code: String): Boolean = SUPPORTED.any { it.code == code }

    /**
     * Production routing gateway: maps a stored pref, pack, or command tag to one
     * of the enabled short codes ("en"/"hi").
     *
     * Finding A8: this used to be a bare `isSupported(code) ? code : DEFAULT`,
     * which silently re-rooted valid alias forms of a SUPPORTED language — a
     * pref of "hi-IN" or "hinglish" became the English voice, with no signal.
     * It now routes through [canonicalize], so every alias in the shared
     * speech table resolves to its real voice; only genuinely unknown or
     * malformed tags fall back to the default, and that decision is logged so
     * a corrupted pref is visible in diagnostics instead of silent.
     */
    fun normalize(code: String?): String {
        val canonical = canonicalize(code)
        if (canonical == null) {
            Logger.w("VoiceLanguage: malformed or empty language tag \"$code\"; using default voice")
            return DEFAULT
        }
        return when (canonical) {
            "hi-IN", "hi-en-codemix" -> "hi"
            "en-IN" -> "en"
            // Unknown-but-well-formed (e.g. "as-IN", "fr") or the auto sentinel:
            // no enabled Android voice matches, so the policy layer falls back.
            else -> {
                Logger.w("VoiceLanguage: \"$canonical\" is not an enabled voice language; using default voice")
                DEFAULT
            }
        }
    }

    fun displayName(code: String): String =
        SUPPORTED.firstOrNull { it.code == code }?.display ?: displayName(DEFAULT)

    /** Android locale tag for system STT/TTS fallbacks. Never silently falls back to en-US. */
    fun localeTag(code: String): String = when (normalize(code)) {
        "hi" -> "hi-IN"
        else -> "en-IN"
    }

    /** Short native-script phrase used by the Settings and Voice Test diagnostics. */
    fun testPhrase(code: String): String = when (normalize(code)) {
        "hi" -> "नमस्ते, यूनोवन की ऑफ़लाइन आवाज़ काम कर रही है।"
        else -> "Hello, UnoOne offline voice is working."
    }

    /** Native-language acknowledgement spoken after hands-free wake activation. */
    fun wakeCue(code: String): String = when (normalize(code)) {
        "hi" -> "हाँ, आवाज़ सुनाई दे रही है।"
        else -> "Yes, I'm listening."
    }

    /**
     * Recognizes only explicit voice-language change requests. Merely mentioning Hindi or English
     * in a message must not rebuild the speech runtime.
     */
    private val hindiRequests = listOf(
            "speak in hindi",
            "reply in hindi",
            "answer in hindi",
            "switch to hindi",
            "change language to hindi",
            "hindi mein bolo",
            "hindi me bolo",
            "hindi mein jawab do",
            "hindi me jawab do",
            "हिंदी में बोलो",
            "हिन्दी में बोलो",
            "हिंदी में जवाब दो",
            "अब हिंदी में बोलो"
        )
    private val englishRequests = listOf(
            "speak in english",
            "reply in english",
            "answer in english",
            "switch to english",
            "change language to english",
            "english mein bolo",
            "english me bolo",
            "अंग्रेज़ी में बोलो",
            "अंग्रेजी में बोलो",
            "इंग्लिश में बोलो"
        )

    /**
     * Extracts an explicit language instruction even when it follows a real action, for example
     * "blind mode start karo aur Hindi mein jawab do". Only complete imperative phrases match;
     * ordinary mentions such as "Hindi class" or "English calendar" remain untouched.
     */
    fun extractRequest(command: String): LanguageRequest? {
        val candidates = (hindiRequests.map { "hi" to it } + englishRequests.map { "en" to it })
            .sortedByDescending { it.second.length }
        val match = candidates.firstNotNullOfOrNull { (code, phrase) ->
            val regex = Regex(
                "(?iu)(?<![\\p{L}\\p{M}])${Regex.escape(phrase)}(?![\\p{L}\\p{M}])"
            )
            regex.find(command)?.let { code to it.range }
        } ?: return null

        val remaining = command.removeRange(match.second)
            .replace(Regex("(?iu)^\\s*(?:and|aur|और)\\s+"), "")
            .replace(Regex("(?iu)\\s+(?:and|aur|और)\\s*$"), "")
            .trim(' ', ',', '.', ';', ':', '-', '।')
            .replace(Regex("\\s+"), " ")
        return LanguageRequest(match.first, remaining)
    }

    fun requestedFromCommand(command: String): String? = extractRequest(command)?.code

    fun changeConfirmation(code: String): String = when (normalize(code)) {
        "hi" -> "अब जवाब हिंदी में होगा।"
        else -> "I will speak in English now."
    }

    fun changeFailure(requestedCode: String, responseCode: String): String =
        when (normalize(responseCode)) {
            "hi" -> if (normalize(requestedCode) == "hi") {
                "हिंदी की ऑफ़लाइन आवाज़ अभी उपलब्ध नहीं है।"
            } else {
                "अंग्रेज़ी की ऑफ़लाइन आवाज़ अभी उपलब्ध नहीं है।"
            }
            else -> "The offline ${displayName(requestedCode)} voice is not available yet."
        }
}
