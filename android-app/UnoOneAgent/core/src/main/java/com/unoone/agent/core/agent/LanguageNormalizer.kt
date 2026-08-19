package com.unoone.agent.core.agent

/**
 * Normalized representation of user speech input before it reaches the model.
 *
 * The normalizer sits between STT output and the model pipeline:
 * 1. Detects the spoken language.
 * 2. Determines the expected reply language (same as detected, unless the user explicitly requested
 *    a different language).
 * 3. Cleans up ASR artifacts (filler words, repetitions, trailing partials).
 * 4. Optionally transliterates (e.g. Hinglish → Devanagari) for the model.
 * 5. Flags low-confidence transcripts for clarification instead of execution.
 *
 * Hard output-language rule: if the user speaks Hindi, the model must reply in Hindi.
 * Bengali/Assamese generation is flagged as unsupported unless explicitly requested.
 */
data class NormalizedInput(
    /** ISO 639-1 language code detected from the transcript: "en", "hi", "bn", "ta", "te", "kn", "ml". */
    val detectedLanguage: String,

    /**
     * The language the model should reply in. Defaults to [detectedLanguage] unless the user
     * explicitly requested a switch (e.g. "answer in English").
     */
    val expectedReplyLanguage: String,

    /**
     * Cleaned, potentially transliterated transcript ready for model consumption.
     * Filler words removed, trailing partials trimmed, normalized whitespace.
     */
    val normalisedTranscript: String,

    /** The raw STT output, preserved for debugging and evidence. */
    val originalTranscript: String,

    /**
     * STT confidence score 0.0–1.0. Below [LanguageNormalizer.CONFIDENCE_THRESHOLD], the
     * orchestrator should trigger a clarification prompt instead of passing the text to the model.
     */
    val confidence: Float
) {
    /** True if the transcript confidence is too low for reliable model input. */
    val isLowConfidence: Boolean get() = confidence < LanguageNormalizer.CONFIDENCE_THRESHOLD

    /** True if the model should reply in a non-English language. */
    val isNonEnglishReply: Boolean get() = expectedReplyLanguage != "en"

    /** True if the detected language is one we support for output but not the model natively. */
    val requiresLanguageDirective: Boolean get() = expectedReplyLanguage != "en"
}

/**
 * Transforms raw STT output into a [NormalizedInput] ready for the model pipeline.
 *
 * This is pure, deterministic logic — no model involvement. The normalizer removes
 * ASR artifacts, detects language from word patterns, and enforces the hard
 * output-language rule.
 */
object LanguageNormalizer {

    /** Confidence below this threshold triggers a clarification prompt instead of execution. */
    const val CONFIDENCE_THRESHOLD: Float = 0.5f

    /** Supported language codes for detection and reply.
     *  "mr" (Marathi) shares Devanagari script with Hindi — the Hindi TTS can pronounce
     *  Marathi text acceptably, and the Devanagari OCR recognizer covers both. */
    val SUPPORTED_LANGUAGES: Set<String> = setOf("en", "hi", "mr", "bn", "ta", "te", "kn", "ml")

    /**
     * Languages where generation quality is degraded or unsupported on-device.
     * The model should fall back to English or transliteration for these.
     */
    val CONSTRAINED_GENERATION_LANGUAGES: Set<String> = setOf("bn", "as")

    // Common filler words to strip from ASR output.
    // "like" and "actually" removed — they carry meaning:
    //   "open something like WhatsApp" → corrupted if "like" stripped,
    //   "actually open calendar" → loses emphasis if "actually" stripped.
    private val FILLER_WORDS = setOf(
        "um", "uh", "hmm", "huh", "ah", "oh", "you know",
        "so yeah", "i mean", "basically"
    )

    // Language detection keyword patterns
    private val HINDI_KEYWORDS = setOf(
        "mein", "hai", "karo", "bolo", "kya", "nahi", "theek", "achha",
        "chalo", "dikha", "sunao", "likh", "dhund", "jaldi", "abhi",
        "kal", "aaj", "baad", "pehle", "yahan", "wahan", "mujhe", "tum",
        "apna", "sir", "ji", "namaste", "dhanyavaad", "shukriya"
    )

    // "ki" removed — it's a common Hindi conjunction ("that") and causes false
    // Bengali detection. Bengali detection now requires >=2 keywords (see
    // BENGALI_MIN_KEYWORD_COUNT) so a single ambiguous word won't trigger a switch.
    private val BENGALI_KEYWORDS = setOf(
        "ami", "tumi", "koro", "bolo", "na", "ache", "dao",
        "nibo", "dekho", "shono", "likho", "kothay", "kobe"
    )

    /** Minimum Bengali keyword hits required before classifying input as Bengali.
     *  Prevents false positives from overlapping words like "ki" (Hindi "that"). */
    private const val BENGALI_MIN_KEYWORD_COUNT = 2

    private val TAMIL_KEYWORDS = setOf(
        "ennai", "unggal", "seiyungal", "sol", "enna", "illai", "theriyum",
        "paaru", "kettu", "ezhuthu", "thedi", "ingge", "enge"
    )

    private val TELUGU_KEYWORDS = setOf(
        "nenu", "meeru", "cheyyi", "cheppu", "emi", "ledu", "vundhi",
        "choodu", "vinnu", "rayi", "thedi", "ikkada", "ekkada"
    )

    private val KANNADA_KEYWORDS = setOf(
        "nanu", "neenu", "maadu", "helu", "yenu", "illa", "ide",
        "nodu", "kelu", "baredhu", "huduki", "illi", "elli"
    )

    private val MALAYALAM_KEYWORDS = setOf(
        "njaan", "ningal", "cheyyu", "parayu", "enna", "illa", "undu",
        "nokku", "kelu", "ezhuthu", "thedi", "inge", "enge"
    )

    // Marathi shares Devanagari script with Hindi; common transliterated Marathi words
    // that differ from Hindi to help distinguish the two.
    private val MARATHI_KEYWORDS = setOf(
        "mee", "tumhi", "kara", "bolwa", "kaay", "nahi", "theek", "achha",
        "chala", "dakhwa", "aika", "likha", "shodha", "aata", "udya",
        "maajha", "tumcha", "houn", "naa"
    )

    // Explicit language-switch patterns
    private val LANGUAGE_SWITCH_PATTERNS = mapOf(
        "in english" to "en",
        "in hindi" to "hi",
        "in marathi" to "mr",
        "हिंदी में" to "hi",
        "मराठी में" to "mr",
        "अंग्रेजी में" to "en",
        "in tamil" to "ta",
        "in bengali" to "bn",
        "in telugu" to "te",
        "in kannada" to "kn",
        "in malayalam" to "ml"
    )

    /**
     * Normalize a raw STT transcript.
     *
     * @param rawTranscript the raw output from Sherpa offline STT.
     * @param confidence STT confidence score (0.0–1.0). Use 1.0 if unknown.
     * @param currentVoiceLanguage the current voice language setting from [VoiceLanguage] (e.g. "en").
     * @return a [NormalizedInput] ready for the model pipeline.
     */
    fun normalize(
        rawTranscript: String,
        confidence: Float,
        currentVoiceLanguage: String = "en"
    ): NormalizedInput {
        val original = rawTranscript.trim()
        val cleaned = cleanTranscript(original)
        val detectedLang = detectLanguage(cleaned, currentVoiceLanguage)
        val replyLang = detectReplyLanguage(cleaned, detectedLang)

        return NormalizedInput(
            detectedLanguage = detectedLang,
            expectedReplyLanguage = replyLang,
            normalisedTranscript = cleaned,
            originalTranscript = original,
            confidence = confidence
        )
    }

    /**
     * Clean ASR artifacts from the transcript.
     * - Strips filler words
     * - Removes trailing partial words (cut off mid-word)
     * - Normalizes whitespace
     * - Strips leading/trailing punctuation noise
     */
    private fun cleanTranscript(text: String): String {
        var cleaned = text.trim()

        // Remove trailing partial word (cut off by ASR, ending with hyphen or slash).
        // Only strips words ending with "-" or "/" — these are ASR truncation markers,
        // not valid word endings in any supported language. Indic-script words are preserved
        // because they end with their own script characters, not hyphens or slashes.
        cleaned = cleaned.replace(Regex("\\s+\\S*[-/]$"), "")

        // Strip filler words
        for (filler in FILLER_WORDS) {
            cleaned = cleaned.replace(Regex("\\b$filler\\b", RegexOption.IGNORE_CASE), "")
        }

        // Normalize whitespace
        cleaned = cleaned.replace(Regex("\\s+"), " ").trim()

        // Strip leading noise characters
        cleaned = cleaned.replace(Regex("^[^a-zA-Z0-9\\u0900-\\u097F\\u0980-\\u09FF]+"), "")

        // If cleaning removed everything (pure filler utterance like "um uh hmm"), return empty
        // so the orchestrator triggers a clarification prompt instead of passing garbage to the model.
        // However, if cleaning removed legitimate text due to aggressive regex, fall back to the
        // original trimmed input as a safety net.
        return cleaned.ifBlank { text.trim() }.ifBlank { "" }
    }

    /**
     * Detect the language from the transcript content.
     * Falls back to [currentVoiceLanguage] if detection is inconclusive.
     */
    private fun detectLanguage(text: String, currentVoiceLanguage: String): String {
        val lower = text.lowercase()

        // Check for Devanagari script (Hindi, Marathi)
        if (lower.any { it in 'ऀ'..'ॿ' }) return "hi"

        // Check for Bengali script
        if (lower.any { it in 'ঀ'..'৿' }) return "bn"

        // Check for Tamil script
        if (lower.any { it in '஀'..'௿' }) return "ta"

        // Check for Telugu script
        if (lower.any { it in 'ఀ'..'౿' }) return "te"

        // Check for Kannada script
        if (lower.any { it in 'ಀ'..'೿' }) return "kn"

        // Check for Malayalam script
        if (lower.any { it in 'ഀ'..'ൿ' }) return "ml"

        // Keyword-based detection for Romanized text
        val words = lower.split(Regex("\\s+"))

        val langScores = mutableMapOf<String, Int>(
            "hi" to words.count { it in HINDI_KEYWORDS },
            "mr" to words.count { it in MARATHI_KEYWORDS },
            "bn" to words.count { it in BENGALI_KEYWORDS },
            "ta" to words.count { it in TAMIL_KEYWORDS },
            "te" to words.count { it in TELUGU_KEYWORDS },
            "kn" to words.count { it in KANNADA_KEYWORDS },
            "ml" to words.count { it in MALAYALAM_KEYWORDS }
        )

        // Bengali requires >= BENGALI_MIN_KEYWORD_COUNT keyword hits to prevent
        // false positives from overlapping words (e.g. Hindi "ki" meaning "that").
        // If Bengali doesn't meet the threshold, zero out its score so it can't win.
        if ((langScores["bn"] ?: 0) < BENGALI_MIN_KEYWORD_COUNT) {
            langScores["bn"] = 0
        }

        val bestLang = langScores.maxByOrNull { it.value }
        if (bestLang != null && bestLang.value >= 1) {
            // Tie-breaking: if multiple languages score equally, prefer the current voice language
            // (the user's active setting) as it's the most likely intent. Otherwise, pick the first
            // highest scorer (insertion-order stable for deterministic results).
            val bestScore = bestLang.value
            val tiedWinners = langScores.entries.filter { it.value == bestScore }
            if (tiedWinners.size > 1) {
                val voiceLanguageMatch = tiedWinners.find { it.key == currentVoiceLanguage }
                if (voiceLanguageMatch != null) return voiceLanguageMatch.key
            }
            return bestLang.key
        }

        // Fall back to current voice language
        return currentVoiceLanguage
    }

    /**
     * Determine the expected reply language.
     *
     * Hard rule: user speaks Hindi → reply in Hindi.
     * If the user explicitly requested a language switch ("answer in English"), honor that.
     * Constrained generation languages (Bengali, Assamese) fall back to English unless
     * explicitly requested.
     */
    private fun detectReplyLanguage(text: String, detectedLanguage: String): String {
        val lower = text.lowercase()

        // Check for explicit language-switch requests
        for ((pattern, lang) in LANGUAGE_SWITCH_PATTERNS) {
            if (lower.contains(pattern)) {
                return lang
            }
        }

        // Constrained generation languages: fall back to English unless explicitly requested
        if (detectedLanguage in CONSTRAINED_GENERATION_LANGUAGES) {
            return "en" // Model can't generate well in Bengali/Assamese on-device
        }

        // Default: reply in the detected language
        return detectedLanguage
    }
}