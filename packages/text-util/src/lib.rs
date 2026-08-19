//! # Unicode-safe text truncation for Pocket AI
//!
//! ## The bug this crate eliminates
//!
//! Six sites across the desktop app truncated text with raw byte slicing:
//!
//! ```text
//! &content[..200]     documents.rs (memory preview, two sites)
//! &text[..8000]       documents.rs (document extraction, three sites)
//! &text[..4000]       agent.rs     (agent context window)
//! ```
//!
//! `&str[..n]` **panics** unless `n` lands on a UTF-8 character boundary.
//! Pocket AI targets Hindi, Bengali, Assamese, Tamil, Telugu, Kannada and
//! Malayalam. Devanagari and Bengali code points are 3 bytes each, so for those
//! languages a boundary landing mid-character is the *common* case, not the edge
//! case: any document long enough to truncate had roughly a 2-in-3 chance of
//! crashing the app.
//!
//! ## Graphemes, not code points
//!
//! Truncating on a code-point boundary avoids the panic but still corrupts
//! Indic text, because a user-perceived character is frequently several code
//! points — a consonant plus a vowel sign (matra), a nukta, or a virama forming
//! a conjunct. Cutting between them leaves a dangling combining mark that
//! renders as a broken glyph.
//!
//! This crate therefore truncates on **grapheme cluster** boundaries, which is
//! what "character" means to someone reading the result.
//!
//! ## Counts are reported honestly
//!
//! The original notices read `"{} total chars"` while printing `text.len()` —
//! a **byte** count, overstating Devanagari by about 3×. [`count_chars`] returns
//! grapheme clusters, so a notice built by [`truncate_with_notice`] states a
//! number that matches what the user can count on screen.

#![forbid(unsafe_code)]

use unicode_segmentation::UnicodeSegmentation;

/// Number of user-perceived characters (grapheme clusters).
///
/// This is deliberately **not** `len()` (bytes) and **not** `chars().count()`
/// (code points). For "नमस्ते" those three differ: 18 bytes, 6 code points,
/// 4 grapheme clusters.
pub fn count_chars(s: &str) -> usize {
    s.graphemes(true).count()
}

/// Truncate to at most `max_chars` grapheme clusters.
///
/// Returns a borrowed prefix of the input. Never panics, never splits a
/// grapheme cluster, and returns the whole input when it is already short
/// enough.
pub fn truncate_chars(s: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }
    match s.grapheme_indices(true).nth(max_chars) {
        // `idx` is the byte offset where cluster `max_chars` begins, so it is a
        // valid boundary and slicing there is always safe.
        Some((idx, _)) => &s[..idx],
        // Fewer than `max_chars` clusters — nothing to cut.
        None => s,
    }
}

/// Truncate and append an ellipsis when content was actually removed.
///
/// Used for short previews where a byte/char count would be noise.
pub fn preview(s: &str, max_chars: usize) -> String {
    let head = truncate_chars(s, max_chars);
    if head.len() == s.len() {
        head.to_string()
    } else {
        format!("{}...", head)
    }
}

/// Truncate and append a notice stating the **true** character count.
///
/// Replaces the previous pattern, which printed a byte count while calling it
/// "chars".
pub fn truncate_with_notice(s: &str, max_chars: usize) -> String {
    let head = truncate_chars(s, max_chars);
    if head.len() == s.len() {
        return s.to_string();
    }
    format!(
        "{}...\n\n[Truncated — {} total characters]",
        head,
        count_chars(s)
    )
}

/// Truncate to at most `max_bytes`, snapping **down** to a grapheme boundary.
///
/// Use this where the limit is a real byte budget — model context windows, log
/// lines, IPC payloads — rather than a display length. Switching those call
/// sites to a character count would silently triple the payload for Devanagari
/// or Bengali and could overflow the context window the limit exists to protect.
///
/// The result is always `<= max_bytes` and never splits a grapheme cluster.
pub fn truncate_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = 0;
    for (idx, g) in s.grapheme_indices(true) {
        if idx + g.len() > max_bytes {
            break;
        }
        end = idx + g.len();
    }
    &s[..end]
}

/// Byte-budgeted truncation with an honest character-count notice.
pub fn truncate_bytes_with_notice(s: &str, max_bytes: usize) -> String {
    let head = truncate_bytes(s, max_bytes);
    if head.len() == s.len() {
        return s.to_string();
    }
    format!(
        "{}...\n\n[Truncated — {} total characters]",
        head,
        count_chars(s)
    )
}

/// Extract a window around `byte_offset`, snapped outward to grapheme
/// boundaries.
///
/// Search hit highlighting knows a byte offset from the matched substring but
/// must not slice there directly. Bounds are widened to the nearest safe
/// boundaries rather than narrowed, so the match itself is never clipped.
pub fn snippet_around(s: &str, byte_offset: usize, radius_chars: usize) -> &str {
    if s.is_empty() {
        return s;
    }
    let clusters: Vec<(usize, &str)> = s.grapheme_indices(true).collect();
    if clusters.is_empty() {
        return s;
    }

    // Index of the cluster containing `byte_offset`.
    let hit = clusters
        .iter()
        .position(|(idx, g)| *idx <= byte_offset && byte_offset < idx + g.len())
        .unwrap_or_else(|| clusters.len().saturating_sub(1));

    let start_cluster = hit.saturating_sub(radius_chars);
    let end_cluster = (hit + radius_chars).min(clusters.len().saturating_sub(1));

    let start_byte = clusters[start_cluster].0;
    let end_byte = clusters[end_cluster].0 + clusters[end_cluster].1.len();
    &s[start_byte..end_byte]
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real strings from the languages Pocket AI claims to support.
    const HINDI: &str = "नमस्ते दुनिया यह एक परीक्षण है";
    const BENGALI: &str = "নমস্কার বিশ্ব এটি একটি পরীক্ষা";
    const ASSAMESE: &str = "নমস্কাৰ পৃথিৱী এইটো এটা পৰীক্ষা";
    const TAMIL: &str = "வணக்கம் உலகம் இது ஒரு சோதனை";
    const TELUGU: &str = "నమస్కారం ప్రపంచం ఇది ఒక పరీక్ష";

    /// The exact crash: `&s[..n]` panics when `n` is not a char boundary.
    /// This documents the old behaviour so the regression cannot return.
    #[test]
    fn raw_byte_slicing_would_panic_on_devanagari() {
        let panicked = std::panic::catch_unwind(|| {
            let _ = &HINDI[..7]; // mid-character by construction
        })
        .is_err();
        assert!(
            panicked,
            "if this stops panicking the premise changed; re-check the fix"
        );
    }

    #[test]
    fn truncate_never_panics_at_any_length_for_every_language() {
        for (name, text) in [
            ("hindi", HINDI),
            ("bengali", BENGALI),
            ("assamese", ASSAMESE),
            ("tamil", TAMIL),
            ("telugu", TELUGU),
        ] {
            // Every possible cut point, including well past the end.
            for n in 0..=(text.len() + 5) {
                let out = truncate_chars(text, n);
                assert!(
                    text.starts_with(out),
                    "{name}: truncation at {n} is not a prefix"
                );
            }
        }
    }

    #[test]
    fn truncation_lands_on_grapheme_boundaries() {
        for text in [HINDI, BENGALI, ASSAMESE, TAMIL, TELUGU] {
            for n in 0..12 {
                let out = truncate_chars(text, n);
                // A prefix that ends mid-cluster would have a different
                // cluster count than the number of clusters requested.
                assert_eq!(
                    count_chars(out),
                    n.min(count_chars(text)),
                    "cut at {n} split a grapheme cluster in {text:?}"
                );
            }
        }
    }

    #[test]
    fn combining_marks_stay_attached() {
        // Devanagari "क" + matra "ि" is one user-perceived character.
        let s = "कि";
        assert_eq!(count_chars(s), 1);
        // Asking for one character must keep the matra, not drop it.
        assert_eq!(truncate_chars(s, 1), "कि");
    }

    #[test]
    fn byte_count_and_char_count_genuinely_differ() {
        // Guards the honesty fix: reporting len() as "chars" was wrong.
        assert!(HINDI.len() > count_chars(HINDI) * 2);
        assert_ne!(HINDI.len(), count_chars(HINDI));
        assert_ne!(HINDI.chars().count(), count_chars(HINDI));
    }

    #[test]
    fn notice_reports_characters_not_bytes() {
        let out = truncate_with_notice(HINDI, 5);
        let expected = format!("{} total characters", count_chars(HINDI));
        assert!(out.contains(&expected), "got: {out}");
        assert!(
            !out.contains(&format!("{} total", HINDI.len())),
            "notice leaked a byte count"
        );
    }

    #[test]
    fn short_input_is_returned_unchanged_without_ellipsis() {
        assert_eq!(truncate_with_notice(HINDI, 10_000), HINDI);
        assert_eq!(preview(HINDI, 10_000), HINDI);
        assert!(!preview(HINDI, 10_000).ends_with("..."));
    }

    #[test]
    fn preview_appends_ellipsis_only_when_truncated() {
        assert!(preview(HINDI, 3).ends_with("..."));
        assert!(!preview("short", 100).ends_with("..."));
    }

    #[test]
    fn zero_and_empty_are_safe() {
        assert_eq!(truncate_chars(HINDI, 0), "");
        assert_eq!(truncate_chars("", 10), "");
        assert_eq!(count_chars(""), 0);
        assert_eq!(preview("", 10), "");
        assert_eq!(snippet_around("", 0, 10), "");
    }

    #[test]
    fn emoji_zwj_sequences_are_one_cluster() {
        let family = "👨‍👩‍👧‍👦";
        assert_eq!(count_chars(family), 1);
        assert_eq!(truncate_chars(family, 1), family);
        // Never return a torn half of a ZWJ sequence.
        assert_eq!(truncate_chars(family, 0), "");
    }

    #[test]
    fn snippet_around_is_boundary_safe_at_every_offset() {
        for text in [HINDI, BENGALI, ASSAMESE] {
            for off in 0..text.len() {
                let s = snippet_around(text, off, 4);
                assert!(text.contains(s), "snippet is not a substring");
            }
        }
    }

    #[test]
    fn snippet_includes_the_hit_character() {
        let hit = HINDI.find("परीक्षण").expect("substring present");
        let s = snippet_around(HINDI, hit, 3);
        assert!(
            s.contains('प'),
            "snippet dropped the matched character: {s:?}"
        );
    }

    #[test]
    fn truncate_bytes_respects_the_budget_and_never_panics() {
        for text in [HINDI, BENGALI, ASSAMESE, TAMIL, TELUGU] {
            for n in 0..=(text.len() + 5) {
                let out = truncate_bytes(text, n);
                assert!(out.len() <= n, "exceeded byte budget {n}");
                assert!(text.starts_with(out), "not a prefix at budget {n}");
                // Must still be whole clusters.
                assert_eq!(
                    out,
                    truncate_chars(out, count_chars(out)),
                    "byte truncation split a cluster at budget {n}"
                );
            }
        }
    }

    /// A byte budget must stay a byte budget. If these call sites had been
    /// switched to a character count, Devanagari payloads would have roughly
    /// tripled and could overflow the context window the limit protects.
    #[test]
    fn byte_budget_is_not_silently_a_char_budget() {
        let budget = 30;
        let out = truncate_bytes(HINDI, budget);
        assert!(out.len() <= budget);
        assert!(
            count_chars(out) < budget,
            "expected far fewer characters than bytes for Devanagari"
        );
    }

    #[test]
    fn byte_notice_also_reports_characters() {
        let out = truncate_bytes_with_notice(HINDI, 30);
        assert!(out.contains(&format!("{} total characters", count_chars(HINDI))));
    }

    #[test]
    fn mixed_script_and_ascii_behaves() {
        let mixed = "Hello नमस्ते World বিশ্ব 123";
        for n in 0..=count_chars(mixed) + 3 {
            let out = truncate_chars(mixed, n);
            assert!(mixed.starts_with(out));
        }
    }
}
