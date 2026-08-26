#ifndef INBHARAT_IBAUDIO_LANGUAGE_HPP
#define INBHARAT_IBAUDIO_LANGUAGE_HPP

// Bharat Speech Adaptation Layer — deterministic, model-free text and script
// handling for Indian-language speech output. No ML here: everything is explicit
// rules over decoded UTF-8 codepoints, which is the honest way to do
// normalization, script policy, and code-mix metadata. See docs/LANGUAGE_PACK_SPEC.md.
//
// Design rules honored throughout:
//  - UTF-8 is decoded to codepoints; bytes are never treated as characters
//    (this was the core defect in the old codeswitch byte loop).
//  - Devanagari is U+0900–U+097F only — not "any byte >= 128".
//  - Every function is bounded, allocation-light, and exception-safe.

#include <cstdint>
#include <string>
#include <vector>

namespace ibaudio {
namespace language {

// Script of a single codepoint, restricted to what the adaptation layer reasons about.
enum class Script {
    Latin,
    Devanagari,
    Digit,
    Other,
    BengaliAssamese,
    Gurmukhi,
    Gujarati,
    Odia,
    Tamil,
    Telugu,
    Kannada,
    Malayalam,
    ArabicPersoArabic,
    OlChiki,
    MeeteiMayek
};

// ISO 15924 script subtag (for example, "Deva" or "Beng"). Digits and
// unclassified codepoints return "Zyyy" and "Zzzz", respectively.
const char *script_tag(Script script);

// Decode one UTF-8 codepoint starting at s[i]. Returns the codepoint and advances i
// past it. Invalid sequences yield 0xFFFD (replacement) and advance by one byte so
// callers always make progress and never read out of bounds.
uint32_t decode_utf8(const std::string &s, size_t &i);

Script script_of(uint32_t codepoint);

// A script is not a language. This outcome explicitly distinguishes the narrow
// cases where the layer can provide a compatibility hint from cases where a
// script is shared by several languages or Latin text may be romanized.
enum class LanguageOutcome {
    EnglishCompatible,
    HindiCompatible,
    Ambiguous,
    Unknown
};

// Per-token script and language metadata. A token is a maximal run of same-script
// codepoints. language_tag is always "und" at this deterministic layer because script
// alone cannot honestly identify a language; language_outcome is only a compatibility
// hint for downstream acoustic/history/user-confirmed LID.
struct TokenLanguage {
    std::string token;
    Script script;
    const char *script_tag;
    const char *language_tag;
    LanguageOutcome language_outcome;
};

// Segment a UTF-8 string into tokens with per-token language tags. This is the
// code-mix metadata the adaptation layer emits alongside a transcript.
std::vector<TokenLanguage> tag_code_mix(const std::string &text);

// Detect the dominant language mix of a transcript by codepoint (not byte) ratio.
// Returns scores in [0,1] for en / hi / hinglish that sum to ~1 over letter content.
struct LanguageScore {
    float english = 0.0f;
    float hindi = 0.0f;
    float hinglish = 0.0f;
};
LanguageScore score_code_mix(const std::string &text);

// Normalize Indian English/Hindi numeric and currency expressions to a canonical
// written form. Handles: Indian digit grouping (lakh/crore), ₹/Rs., percent, times,
// and dates. Deterministic; does not guess at words it cannot parse.
std::string normalize_indian_text(const std::string &text);

// Apply a script policy: "devanagari" keeps Devanagari as-is, "roman" transliterates
// Devanagari to a phonemic Latin form, "preserve" returns input unchanged.
// Transliteration is a deterministic grapheme mapping, not a learned model.
enum class ScriptPolicy { Preserve, Devanagari, Roman };
std::string apply_script_policy(const std::string &text, ScriptPolicy policy);

// Devanagari -> phonemic Latin transliteration (deterministic, lossy by design for
// nukta/schwa; documented in the pack spec). Codepoints outside U+0900–U+097F pass
// through unchanged.
std::string devanagari_to_roman(const std::string &text);

} // namespace language
} // namespace ibaudio

#endif // INBHARAT_IBAUDIO_LANGUAGE_HPP
