#include "language.hpp"

#include <unordered_map>

namespace ibaudio {
namespace language {

uint32_t decode_utf8(const std::string &s, size_t &i) {
    if (i >= s.size()) return 0xFFFDu;
    const unsigned char c0 = static_cast<unsigned char>(s[i]);
    if (c0 < 0x80u) { ++i; return c0; }
    uint32_t cp = 0;
    uint32_t minimum = 0;
    size_t extra = 0;
    if (c0 >= 0xC2u && c0 <= 0xDFu) { cp = c0 & 0x1Fu; minimum = 0x80u; extra = 1; }
    else if (c0 >= 0xE0u && c0 <= 0xEFu) { cp = c0 & 0x0Fu; minimum = 0x800u; extra = 2; }
    else if (c0 >= 0xF0u && c0 <= 0xF4u) { cp = c0 & 0x07u; minimum = 0x10000u; extra = 3; }
    else { ++i; return 0xFFFDu; }
    if (extra >= s.size() - i) { ++i; return 0xFFFDu; }
    for (size_t k = 1; k <= extra; ++k) {
        const unsigned char ck = static_cast<unsigned char>(s[i + k]);
        if ((ck & 0xC0u) != 0x80u) { ++i; return 0xFFFDu; }
        cp = (cp << 6) | (ck & 0x3Fu);
    }
    if (cp < minimum || cp > 0x10FFFFu || (cp >= 0xD800u && cp <= 0xDFFFu)) {
        ++i;
        return 0xFFFDu;
    }
    i += extra + 1;
    return cp;
}

namespace {

bool in_range(uint32_t cp, uint32_t first, uint32_t last) {
    return cp >= first && cp <= last;
}

bool is_decimal_digit(uint32_t cp) {
    static const uint32_t starts[] = {
        0x0030u, 0x0660u, 0x06F0u, 0x0966u, 0x09E6u, 0x0A66u,
        0x0AE6u, 0x0B66u, 0x0BE6u, 0x0C66u, 0x0CE6u, 0x0D66u,
        0x1C50u, 0xABF0u
    };
    for (uint32_t start : starts) {
        if (in_range(cp, start, start + 9u)) return true;
    }
    return false;
}

bool is_text_script(Script script) {
    return script != Script::Digit && script != Script::Other;
}

} // namespace

Script script_of(uint32_t cp) {
    if (is_decimal_digit(cp)) return Script::Digit;
    if (in_range(cp, 0x0041u, 0x005Au) || in_range(cp, 0x0061u, 0x007Au) ||
        in_range(cp, 0x00C0u, 0x00D6u) || in_range(cp, 0x00D8u, 0x00F6u) ||
        in_range(cp, 0x00F8u, 0x02E4u)) return Script::Latin;
    if (in_range(cp, 0x0900u, 0x097Fu) || in_range(cp, 0xA8E0u, 0xA8FFu)) return Script::Devanagari;
    if (in_range(cp, 0x0980u, 0x09FFu)) return Script::BengaliAssamese;
    if (in_range(cp, 0x0A00u, 0x0A7Fu)) return Script::Gurmukhi;
    if (in_range(cp, 0x0A80u, 0x0AFFu)) return Script::Gujarati;
    if (in_range(cp, 0x0B00u, 0x0B7Fu)) return Script::Odia;
    if (in_range(cp, 0x0B80u, 0x0BFFu)) return Script::Tamil;
    if (in_range(cp, 0x0C00u, 0x0C7Fu)) return Script::Telugu;
    if (in_range(cp, 0x0C80u, 0x0CFFu)) return Script::Kannada;
    if (in_range(cp, 0x0D00u, 0x0D7Fu)) return Script::Malayalam;
    if (in_range(cp, 0x0600u, 0x06FFu) || in_range(cp, 0x0750u, 0x077Fu) ||
        in_range(cp, 0x08A0u, 0x08FFu) || in_range(cp, 0xFB50u, 0xFDFFu) ||
        in_range(cp, 0xFE70u, 0xFEFFu)) return Script::ArabicPersoArabic;
    if (in_range(cp, 0x1C50u, 0x1C7Fu)) return Script::OlChiki;
    if (in_range(cp, 0xAAE0u, 0xAAFFu) || in_range(cp, 0xABC0u, 0xABFFu)) return Script::MeeteiMayek;
    return Script::Other;
}

const char *script_tag(Script script) {
    switch (script) {
        case Script::Latin: return "Latn";
        case Script::Devanagari: return "Deva";
        case Script::BengaliAssamese: return "Beng";
        case Script::Gurmukhi: return "Guru";
        case Script::Gujarati: return "Gujr";
        case Script::Odia: return "Orya";
        case Script::Tamil: return "Taml";
        case Script::Telugu: return "Telu";
        case Script::Kannada: return "Knda";
        case Script::Malayalam: return "Mlym";
        case Script::ArabicPersoArabic: return "Arab";
        case Script::OlChiki: return "Olck";
        case Script::MeeteiMayek: return "Mtei";
        case Script::Digit: return "Zyyy";
        case Script::Other:
        default: return "Zzzz";
    }
}

std::vector<TokenLanguage> tag_code_mix(const std::string &text) {
    std::vector<TokenLanguage> out;
    size_t i = 0;
    std::string current;
    Script current_script = Script::Other;
    auto flush = [&]() {
        if (current.empty()) return;
        TokenLanguage t;
        t.token = current;
        t.script = current_script;
        t.script_tag = language::script_tag(current_script);
        // A script is never proof of language. Even Latin and Devanagari remain
        // undetermined: Latin may be Romanized Hindi/Assamese/Konkani/etc.; Devanagari
        // is shared by Hindi, Marathi, Nepali, Sanskrit, Bodo, Dogri, Maithili and more.
        t.language_tag = "und";
        t.language_outcome = current_script == Script::Latin
            ? LanguageOutcome::EnglishCompatible
            : current_script == Script::Devanagari
                ? LanguageOutcome::HindiCompatible
                : LanguageOutcome::Ambiguous;
        out.push_back(std::move(t));
        current.clear();
        current_script = Script::Other;
    };
    while (i < text.size()) {
        const size_t start = i;
        const uint32_t cp = decode_utf8(text, i);
        const Script sc = script_of(cp);
        const bool letter = is_text_script(sc);
        if (letter) {
            if (current_script != sc && !current.empty()) flush();
            current_script = sc;
            current.append(text, start, i - start);
        } else {
            flush();
        }
    }
    flush();
    return out;
}

LanguageScore score_code_mix(const std::string &text) {
    // Count *letters* by codepoint class. Bytes are never used, so multi-byte
    // Devanagari characters each count once.
    size_t i = 0;
    uint32_t latin = 0, dev = 0;
    while (i < text.size()) {
        const uint32_t cp = decode_utf8(text, i);
        const Script sc = script_of(cp);
        if (sc == Script::Latin) ++latin;
        else if (sc == Script::Devanagari) ++dev;
    }
    LanguageScore s;
    const float total = static_cast<float>(latin + dev);
    if (total == 0.0f) return s;
    const float en = static_cast<float>(latin) / total;
    const float hi = static_cast<float>(dev) / total;
    s.english = en;
    s.hindi = hi;
    // Hinglish when both scripts are present in meaningful proportion.
    const float mix = (en > 0.2f && hi > 0.2f) ? 2.0f * (en < hi ? en : hi) : 0.0f;
    const float renorm = 1.0f + mix;
    s.english = en / renorm;
    s.hindi = hi / renorm;
    s.hinglish = mix / renorm;
    return s;
}

// --- Indian numeric / currency normalization -------------------------------------

namespace {

std::string two_digit(int n) {
    static const char *ones[] = {"zero","one","two","three","four","five","six","seven",
        "eight","nine","ten","eleven","twelve","thirteen","fourteen","fifteen","sixteen",
        "seventeen","eighteen","nineteen"};
    static const char *tens[] = {"","","twenty","thirty","forty","fifty","sixty","seventy","eighty","ninety"};
    if (n < 20) return ones[n];
    std::string r = tens[n / 10];
    if (n % 10) r += std::string("-") + ones[n % 10];
    return r;
}

// Spell a non-negative integer using Indian grouping (thousand, lakh, crore).
std::string spell_indian(long long n) {
    if (n < 100) return two_digit(static_cast<int>(n));
    if (n < 1000) {
        std::string r = two_digit(static_cast<int>(n / 100)) + " hundred";
        if (n % 100) r += " " + two_digit(static_cast<int>(n % 100));
        return r;
    }
    if (n < 100000) {  // thousand
        std::string r = spell_indian(n / 1000) + " thousand";
        if (n % 1000) r += " " + spell_indian(n % 1000);
        return r;
    }
    if (n < 10000000) {  // lakh
        std::string r = spell_indian(n / 100000) + " lakh";
        if (n % 100000) r += " " + spell_indian(n % 100000);
        return r;
    }
    // crore
    std::string r = spell_indian(n / 10000000) + " crore";
    if (n % 10000000) r += " " + spell_indian(n % 10000000);
    return r;
}

bool is_digit_ascii(char c) { return c >= '0' && c <= '9'; }

// Expand a numeric token (with Indian commas, optional ₹/Rs., decimal) into words.
// Returns empty string when the token is not a recognizable number.
std::string expand_number(const std::string &tok) {
    std::string digits;
    std::string frac;
    bool seen_dot = false;
    bool any = false;
    for (char c : tok) {
        if (is_digit_ascii(c)) { (seen_dot ? frac : digits) += c; any = true; }
        else if (c == ',') { /* Indian grouping commas ignored */ }
        else if (c == '.' && !seen_dot) { seen_dot = true; }
        else { return std::string(); }  // not a pure number token
    }
    if (!any || digits.empty()) return std::string();
    long long whole = 0;
    for (char c : digits) {
        whole = whole * 10 + (c - '0');
        if (whole > 99999999999LL) return std::string();  // bounded
    }
    std::string out = spell_indian(whole);
    if (seen_dot && !frac.empty()) {
        out += " point";
        for (char c : frac) out += " " + two_digit(c - '0');
    }
    return out;
}

} // namespace

std::string normalize_indian_text(const std::string &text) {
    // Token-level pass: split on whitespace, expand numeric/currency tokens, keep the
    // rest. Currency markers attached to a number (₹25,000 / Rs.500 / 50%) are handled.
    std::string out;
    size_t i = 0;
    while (i < text.size()) {
        const size_t ws = text.find_first_not_of(" \t\n\r", i);
        if (ws == std::string::npos) break;
        if (ws > i) out.append(text, i, ws - i);
        const size_t end = text.find_first_of(" \t\n\r", ws);
        const std::string tok = text.substr(ws, end == std::string::npos ? end : end - ws);

        std::string replaced;
        // Currency prefix: ₹ or Rs. / Rs
        std::string core = tok;
        std::string prefix;
        if (core.rfind("\xE2\x82\xB9", 0) == 0) { prefix = "rupees "; core = core.substr(3); }  // ₹
        else if (core.rfind("Rs.", 0) == 0) { prefix = "rupees "; core = core.substr(3); }
        else if (core.rfind("Rs", 0) == 0 && core.size() > 2 && is_digit_ascii(core[2])) { prefix = "rupees "; core = core.substr(2); }
        std::string suffix;
        if (!core.empty() && core.back() == '%') { suffix = " percent"; core.pop_back(); }

        const std::string num = expand_number(core);
        if (!num.empty()) {
            replaced = prefix + num + suffix;
        }
        out += replaced.empty() ? tok : replaced;
        if (end == std::string::npos) break;
        i = end;
    }
    return out;
}

// --- Devanagari <-> Roman ----------------------------------------------------------

std::string devanagari_to_roman(const std::string &text) {
    // Deterministic grapheme mapping for the common Devanagari range. Inherent 'a'
    // (schwa) is inserted after consonants not closed by a halant; this is a
    // documented, lossy approximation — not a learned transliterator.
    static const std::unordered_map<uint32_t, const char *> map = {
        // Vowels (independent)
        {0x0905,"a"},{0x0906,"aa"},{0x0907,"i"},{0x0908,"ee"},{0x0909,"u"},{0x090A,"oo"},
        {0x090B,"ri"},{0x090F,"e"},{0x0910,"ai"},{0x0913,"o"},{0x0914,"au"},
        // Consonants
        {0x0915,"k"},{0x0916,"kh"},{0x0917,"g"},{0x0918,"gh"},{0x0919,"ng"},
        {0x091A,"ch"},{0x091B,"chh"},{0x091C,"j"},{0x091D,"jh"},{0x091E,"ny"},
        {0x091F,"t"},{0x0920,"th"},{0x0921,"d"},{0x0922,"dh"},{0x0923,"n"},
        {0x0924,"t"},{0x0925,"th"},{0x0926,"d"},{0x0927,"dh"},{0x0928,"n"},
        {0x092A,"p"},{0x092B,"ph"},{0x092C,"b"},{0x092D,"bh"},{0x092E,"m"},
        {0x092F,"y"},{0x0930,"r"},{0x0932,"l"},{0x0935,"v"},{0x0936,"sh"},
        {0x0937,"sh"},{0x0938,"s"},{0x0939,"h"},
        // Dependent vowel signs (matras)
        {0x093E,"aa"},{0x093F,"i"},{0x0940,"ee"},{0x0941,"u"},{0x0942,"oo"},
        {0x0947,"e"},{0x0948,"ai"},{0x094B,"o"},{0x094C,"au"},
        {0x094D,""}  // halant / virama: suppress inherent vowel
    };
    std::string out;
    size_t i = 0;
    while (i < text.size()) {
        const size_t start = i;
        const uint32_t cp = decode_utf8(text, i);
        if (cp >= 0x0900u && cp <= 0x097Fu) {
            const auto it = map.find(cp);
            if (it != map.end()) {
                out += it->second;
                // Inherent 'a' after a bare consonant (not closed by a halant, not
                // followed by a matra). Look ahead one codepoint.
                const uint32_t c = cp;
                const bool is_consonant = (c >= 0x0915u && c <= 0x0939u);
                if (is_consonant) {
                    size_t j = i;
                    uint32_t next = 0;
                    if (j < text.size()) next = decode_utf8(text, j);
                    const bool next_is_matra = (next >= 0x093Eu && next <= 0x094Cu);
                    const bool next_is_halant = (next == 0x094Du);
                    if (!next_is_matra && !next_is_halant) out += 'a';
                }
            }
            // Unmapped Devanagari codepoints are dropped (documented lossy behavior).
        } else {
            out.append(text, start, i - start);  // pass through unchanged
        }
    }
    return out;
}

std::string apply_script_policy(const std::string &text, ScriptPolicy policy) {
    switch (policy) {
        case ScriptPolicy::Devanagari: return text;  // keep native script as-is
        case ScriptPolicy::Roman: return devanagari_to_roman(text);
        case ScriptPolicy::Preserve:
        default: return text;
    }
}

} // namespace language
} // namespace ibaudio
