// Bharat adaptation layer tests — pin the actual deterministic behavior of the
// UTF-8 codepoint decoder, code-mix tagging/scoring, Indian numeric normalization,
// and Devanagari->Roman transliteration. These are the honest rules the layer
// implements; they must not silently change.

#include "../src/language/language.hpp"

#include <cassert>
#include <iostream>
#include <string>

using ibaudio::language::Script;

namespace {

void test_utf8_decode() {
    std::string s = "a\xE0\xA4\xA8z";  // 'a', Devanagari NA (U+0928), 'z'
    size_t i = 0;
    assert(ibaudio::language::decode_utf8(s, i) == 'a' && i == 1);
    assert(ibaudio::language::decode_utf8(s, i) == 0x0928u && i == 4);  // 3-byte char
    assert(ibaudio::language::decode_utf8(s, i) == 'z' && i == 5);

    // Invalid: a stray continuation byte must make progress and yield replacement.
    std::string bad = "\x80";
    size_t j = 0;
    assert(ibaudio::language::decode_utf8(bad, j) == 0xFFFDu && j == 1);
    // Overlong two-byte lead and truncated three-byte sequences are rejected while
    // always advancing one byte (no infinite loop / out-of-bounds read).
    std::string overlong = "\xC0\xAF";
    size_t k = 0;
    assert(ibaudio::language::decode_utf8(overlong, k) == 0xFFFDu && k == 1);
    std::string truncated = "\xE0\xA4";
    size_t m = 0;
    assert(ibaudio::language::decode_utf8(truncated, m) == 0xFFFDu && m == 1);
    size_t end = s.size();
    assert(ibaudio::language::decode_utf8(s, end) == 0xFFFDu && end == s.size());
    std::cout << "PASS utf8_decode\n";
}

void test_script_classification() {
    assert(ibaudio::language::script_of('k') == Script::Latin);
    assert(ibaudio::language::script_of(0x0915u) == Script::Devanagari);  // KA
    assert(ibaudio::language::script_of('7') == Script::Digit);
    assert(ibaudio::language::script_of(0x0967u) == Script::Digit);        // Devanagari digit 1
    // French accented letters are Latin, never misclassified as an Indian script.
    assert(ibaudio::language::script_of(0x00E9u) == Script::Latin);  // é
    // Representative codepoint for every Indian script class supported by the layer.
    assert(ibaudio::language::script_of(0x0995u) == Script::BengaliAssamese); // Bengali/Assamese KA
    assert(ibaudio::language::script_of(0x0A15u) == Script::Gurmukhi);        // Gurmukhi KA
    assert(ibaudio::language::script_of(0x0A95u) == Script::Gujarati);        // Gujarati KA
    assert(ibaudio::language::script_of(0x0B15u) == Script::Odia);            // Odia KA
    assert(ibaudio::language::script_of(0x0B95u) == Script::Tamil);           // Tamil KA
    assert(ibaudio::language::script_of(0x0C15u) == Script::Telugu);          // Telugu KA
    assert(ibaudio::language::script_of(0x0C95u) == Script::Kannada);         // Kannada KA
    assert(ibaudio::language::script_of(0x0D15u) == Script::Malayalam);       // Malayalam KA
    assert(ibaudio::language::script_of(0x0627u) == Script::ArabicPersoArabic);// Arabic ALEF
    assert(ibaudio::language::script_of(0x1C5Au) == Script::OlChiki);         // Ol Chiki LA
    assert(ibaudio::language::script_of(0xABC0u) == Script::MeeteiMayek);     // Meetei MAYEK KOK
    assert(std::string(ibaudio::language::script_tag(Script::BengaliAssamese)) == "Beng");
    assert(std::string(ibaudio::language::script_tag(Script::ArabicPersoArabic)) == "Arab");
    std::cout << "PASS script_classification\n";
}

void test_code_mix_score() {
    // "Hello नमस्ते" — mixed Latin + Devanagari letters.
    const auto mix = ibaudio::language::score_code_mix("Hello नमस्ते");
    assert(mix.english > 0.0f && mix.hindi > 0.0f && mix.hinglish > 0.0f);

    // Pure English.
    const auto en = ibaudio::language::score_code_mix("Hello world");
    assert(en.english == 1.0f && en.hindi == 0.0f && en.hinglish == 0.0f);

    // Pure Devanagari.
    const auto hi = ibaudio::language::score_code_mix("नमस्ते");
    assert(hi.hindi == 1.0f && hi.english == 0.0f);
    std::cout << "PASS code_mix_score\n";
}

void test_code_mix_tags() {
    using ibaudio::language::LanguageOutcome;
    const auto tags = ibaudio::language::tag_code_mix("Kal meri meeting नमस्ते বাংলা தமிழ் اردو");
    // Latin + Devanagari + Bengali/Assamese + Tamil + Perso-Arabic tokens.
    assert(tags.size() == 7);
    assert(std::string(tags[0].language_tag) == "und");
    assert(std::string(tags[3].language_tag) == "und");
    assert(std::string(tags[3].script_tag) == "Deva");
    // A script is not a language: shared-script families remain explicitly undetermined.
    assert(std::string(tags[4].language_tag) == "und");
    assert(std::string(tags[4].script_tag) == "Beng");
    assert(tags[4].language_outcome == LanguageOutcome::Ambiguous);
    assert(std::string(tags[5].script_tag) == "Taml");
    assert(std::string(tags[6].script_tag) == "Arab");
    // Romanized Indian words use Latin script and are only English-compatible, not
    // asserted to be English: downstream acoustic/history LID must resolve them.
    const auto romanized = ibaudio::language::tag_code_mix("namaste moi bhal pau");
    assert(!romanized.empty());
    assert(romanized[0].language_outcome == LanguageOutcome::EnglishCompatible);
    std::cout << "PASS code_mix_tags\n";
}

void test_indian_normalization() {
    // Indian grouping: 2,50,000 = 2.5 lakh.
    assert(ibaudio::language::normalize_indian_text("250000") == "two lakh fifty thousand");
    assert(ibaudio::language::normalize_indian_text("2,50,000") == "two lakh fifty thousand");
    // Crore.
    assert(ibaudio::language::normalize_indian_text("10000000") == "one crore");
    // Currency prefix.
    assert(ibaudio::language::normalize_indian_text("Rs.500") == "rupees five hundred");
    // Percent suffix.
    assert(ibaudio::language::normalize_indian_text("50%") == "fifty percent");
    // Non-numeric tokens pass through.
    assert(ibaudio::language::normalize_indian_text("meeting at office") == "meeting at office");
    std::cout << "PASS indian_normalization\n";
}

void test_transliteration() {
    // नमस्ते -> namaste (schwa insertion, matra handling).
    const std::string roman = ibaudio::language::devanagari_to_roman("नमस्ते");
    assert(roman == "namaste");
    // Non-Devanagari passes through unchanged.
    assert(ibaudio::language::devanagari_to_roman("hello") == "hello");
    std::cout << "PASS transliteration\n";
}

} // namespace

int main() {
    test_utf8_decode();
    test_script_classification();
    test_code_mix_score();
    test_code_mix_tags();
    test_indian_normalization();
    test_transliteration();
    std::cout << "All language-layer tests passed!\n";
    return 0;
}
