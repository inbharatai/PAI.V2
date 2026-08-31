//! Pocket AI speech contracts — the shared speech interface.
//!
//! This crate is the single source of truth for:
//! - language alias canonicalization (`as` and `as-IN` both address Assamese;
//!   `hi` → `hi-IN`; `hinglish` → `hi-en-codemix`; global languages such as
//!   `fr` pass through and are NEVER re-rooted to a region),
//! - truthful per-provider language coverage (`languages.v1.json`),
//! - the `SpeechBackend` interface every speech implementation sits behind, so
//!   recording, UI, and agent code never care whether the active backend is
//!   InBharat Audio (Qwen3-ASR / OmniVoice / IndicConformer) or the legacy
//!   Whisper/Piper path.
//!
//! The alias/provider table is embedded from `languages.v1.json` so this crate
//! stays dependency-light and the same file remains readable by the
//! Android-side sync check (`scripts/check_speech_language_sync.py`).
//!
//! Platform policy: no speech implementation may claim a language a provider
//! does not truthfully cover. Qwen3-ASR covers Hindi but NOT Assamese;
//! Assamese routes to the IndicConformer family and fails closed when its
//! local pack is absent.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The embedded alias/provider table. Keep in sync with the Kotlin mirror
/// (`VoiceLanguage.kt`) — enforced by `scripts/check_speech_language_sync.py`.
pub const LANGUAGES_JSON: &str = include_str!("../languages.v1.json");

/// The reserved detect-language sentinel used by speech manifests.
pub const AUTO: &str = "auto";

// ---------------------------------------------------------------------------
// Language canonicalization
// ---------------------------------------------------------------------------

/// A canonical BCP-47-flavored language tag (or the reserved `auto` sentinel).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LanguageTag(String);

impl LanguageTag {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_auto(&self) -> bool {
        self.0 == AUTO
    }
}

impl std::fmt::Display for LanguageTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Failures of language canonicalization. All of them are caller errors:
/// speech never silently guesses a language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguageError {
    /// The input was empty or only whitespace.
    Empty,
    /// The tag is structurally invalid (bad subtag length or characters).
    MalformedTag(String),
}

impl std::fmt::Display for LanguageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LanguageError::Empty => f.write_str("language tag is empty"),
            LanguageError::MalformedTag(tag) => {
                write!(f, "language tag '{tag}' is not a valid BCP-47 tag")
            }
        }
    }
}

impl std::error::Error for LanguageError {}

#[derive(Debug, Deserialize)]
struct LanguageTable {
    #[serde(rename = "schema")]
    _schema: String,
    #[serde(default)]
    aliases: BTreeMap<String, String>,
    #[serde(default)]
    providers: BTreeMap<String, ProviderEntry>,
}

#[derive(Debug, Deserialize)]
struct ProviderEntry {
    kind: ProviderKind,
    #[serde(default)]
    languages: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ProviderKind {
    Asr,
    Tts,
    Both,
}

fn table() -> &'static LanguageTable {
    static TABLE: OnceLock<LanguageTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        serde_json::from_str(LANGUAGES_JSON)
            .expect("embedded languages.v1.json must parse — it is part of the crate")
    })
}

/// Canonicalize a user- or manifest-supplied language string.
///
/// - Alias lookup is ASCII case-insensitive (`as`, `as-in`, `as-IN` all →
///   `as-IN`; `hi` → `hi-IN`; `hinglish` → `hi-en-codemix`; `en` → `en-IN`).
/// - Tags not in the alias table pass through with BCP-47 case normalization
///   and are never re-rooted to a region: `fr` stays `fr`, `en-US` stays
///   `en-US`.
/// - Malformed tags are rejected, never guessed.
pub fn canonicalize(input: &str) -> Result<LanguageTag, LanguageError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(LanguageError::Empty);
    }
    if let Some(canonical) = table().aliases.get(&trimmed.to_ascii_lowercase()) {
        return Ok(LanguageTag(canonical.clone()));
    }
    Ok(LanguageTag(normalize_passthrough(trimmed)?))
}

/// Validate and case-normalize a tag that is not an alias. Subtags are
/// separated by `-`, must be 1–8 alphanumeric characters, and the first
/// subtag must be 2–8 letters. Canonical casing: language lowercase, script
/// Titlecase, region UPPERCASE, everything else lowercase.
fn normalize_passthrough(tag: &str) -> Result<String, LanguageError> {
    let mut normalized_subtags = Vec::new();
    for (index, subtag) in tag.split('-').enumerate() {
        if subtag.is_empty()
            || subtag.len() > 8
            || !subtag.chars().all(|c| c.is_ascii_alphanumeric())
        {
            return Err(LanguageError::MalformedTag(tag.to_string()));
        }
        if index == 0 {
            if subtag.len() < 2 || !subtag.chars().all(|c| c.is_ascii_alphabetic()) {
                return Err(LanguageError::MalformedTag(tag.to_string()));
            }
            normalized_subtags.push(subtag.to_ascii_lowercase());
        } else if subtag.len() == 2 {
            // Region subtag (or a single-letter extension — both uppercase fine
            // for regions; single letters are normalized lowercase instead).
            if subtag.chars().all(|c| c.is_ascii_alphabetic()) {
                normalized_subtags.push(subtag.to_ascii_uppercase());
            } else {
                normalized_subtags.push(subtag.to_ascii_lowercase());
            }
        } else if subtag.len() == 4 && subtag.chars().all(|c| c.is_ascii_alphabetic()) {
            // Script subtag.
            let mut chars = subtag.chars();
            let mut scripted = String::new();
            if let Some(first) = chars.next() {
                scripted.extend([first.to_ascii_uppercase()]);
            }
            for rest in chars {
                scripted.extend([rest.to_ascii_lowercase()]);
            }
            normalized_subtags.push(scripted);
        } else {
            normalized_subtags.push(subtag.to_ascii_lowercase());
        }
    }
    Ok(normalized_subtags.join("-"))
}

// ---------------------------------------------------------------------------
// Provider coverage (truthful capability)
// ---------------------------------------------------------------------------

/// The speech task a provider can serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechTask {
    Asr,
    Tts,
}

impl SpeechTask {
    fn matches(&self, kind: &ProviderKind) -> bool {
        matches!(
            (self, kind),
            (SpeechTask::Asr, ProviderKind::Asr)
                | (SpeechTask::Asr, ProviderKind::Both)
                | (SpeechTask::Tts, ProviderKind::Tts)
                | (SpeechTask::Tts, ProviderKind::Both)
        )
    }
}

/// Does `provider` truthfully serve `language` for `task`?
///
/// This is the ONLY place provider/language routing may be decided; changing a
/// manifest line can never add coverage (e.g. Assamese cannot join the Qwen3
/// route without changing this table AND shipping the assets).
pub fn provider_serves(provider: &str, task: SpeechTask, language: &LanguageTag) -> bool {
    let Some(entry) = table().providers.get(provider) else {
        return false;
    };
    task.matches(&entry.kind) && entry.languages.iter().any(|lang| lang == language.as_str())
}

/// The canonical languages a provider truthfully serves for `task`.
pub fn provider_languages(provider: &str, task: SpeechTask) -> Vec<LanguageTag> {
    let Some(entry) = table().providers.get(provider) else {
        return Vec::new();
    };
    if !task.matches(&entry.kind) {
        return Vec::new();
    }
    entry
        .languages
        .iter()
        .map(|lang| LanguageTag(lang.clone()))
        .collect()
}

/// Validate the embedded table's own invariants. Used by tests and the
/// cross-language sync check: every provider language must already be in
/// canonical form, and no provider may claim the `auto` sentinel.
pub fn verify_table_invariants() -> Result<(), String> {
    let parsed = table();
    for (provider, entry) in &parsed.providers {
        for language in &entry.languages {
            if language == AUTO {
                return Err(format!(
                    "provider '{provider}' claims the reserved 'auto' sentinel"
                ));
            }
            let canonical = canonicalize(language)
                .map_err(|e| format!("provider '{provider}' language '{language}': {e}"))?;
            if canonical.as_str() != language {
                return Err(format!(
                    "provider '{provider}' language '{language}' is not canonical (expected '{}')",
                    canonical.as_str()
                ));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The speech backend interface
// ---------------------------------------------------------------------------

/// What an implementation's streaming actually is. Callers must not claim
/// stateful streaming when the engine is buffered-final or segment-chunked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StreamingClass {
    /// True incremental streaming with per-chunk results.
    StatefulStreaming,
    /// Segment-level chunks with partial results at utterance boundaries.
    SegmentChunked,
    /// The engine buffers the full utterance and emits one final result.
    BufferedFinal,
}

impl StreamingClass {
    /// Stable machine-readable name for status surfaces (Tauri commands,
    /// logs, UI). `BUFFERED_FINAL` serializes as `BUFFERED_FINAL` for
    /// serde consumers; this is the human-facing spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            StreamingClass::StatefulStreaming => "stateful-streaming",
            StreamingClass::SegmentChunked => "segment-chunked",
            StreamingClass::BufferedFinal => "buffered-final",
        }
    }
}

/// Failures of a speech backend. Speech fails closed: every failure is
/// explicit, never a silent fallback or an error string smuggled into a
/// transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechError {
    /// The backend is not production ready (missing assets, failed gate…).
    NotReady(String),
    /// The requested language is not covered by the active backend.
    UnsupportedLanguage {
        requested: LanguageTag,
        reason: String,
    },
    /// Caller input is invalid (empty text, oversized audio, bad path…).
    InvalidInput(String),
    /// The backend exceeded its inference deadline and was killed.
    Timeout { seconds: u64 },
    /// The operation was cancelled through `SpeechBackend::cancel_all`.
    Cancelled,
    /// The backend itself failed (subprocess error, corrupt output…).
    Backend(String),
}

impl std::fmt::Display for SpeechError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpeechError::NotReady(reason) => write!(f, "speech backend is not ready: {reason}"),
            SpeechError::UnsupportedLanguage { requested, reason } => {
                write!(f, "language '{requested}' is not supported: {reason}")
            }
            SpeechError::InvalidInput(reason) => write!(f, "invalid speech input: {reason}"),
            SpeechError::Timeout { seconds } => {
                write!(
                    f,
                    "speech inference exceeded its {seconds}s deadline and was stopped"
                )
            }
            SpeechError::Cancelled => f.write_str("speech operation was cancelled"),
            SpeechError::Backend(reason) => write!(f, "speech backend failure: {reason}"),
        }
    }
}

impl std::error::Error for SpeechError {}

/// Point-in-time readiness of a backend. `ready == false` plus `reason` is
/// the fail-closed contract: callers render the reason, they never fall back
/// silently.
#[derive(Debug, Clone, Serialize)]
pub struct BackendStatus {
    pub backend_name: &'static str,
    pub ready: bool,
    pub streaming_class: StreamingClass,
    pub reason: String,
}

/// Speech-to-text result. `language` is the canonical tag actually used.
#[derive(Debug, Clone)]
pub struct Transcription {
    pub text: String,
    pub language: LanguageTag,
    pub processing_time_ms: u64,
}

/// Text-to-speech result.
#[derive(Debug, Clone)]
pub struct Synthesis {
    pub audio_path: PathBuf,
    pub sample_rate: u32,
    pub duration_seconds: Option<f32>,
    pub processing_time_ms: u64,
}

/// The common PAI speech backend.
///
/// Implementations: the InBharat Audio route (Qwen3-ASR / OmniVoice via the
/// audio.cpp CLI today, the shared C ABI tomorrow) and the legacy
/// Whisper/Piper route. Recording, UI, and agent code talk to this trait only.
///
/// Semantics contract:
/// - `transcribe`/`synthesize` canonicalize `language` themselves and MUST
///   return `UnsupportedLanguage` rather than silently using another language.
/// - Cancellation is cooperative: `cancel_all` best-effort aborts in-flight
///   operations, which then report `SpeechError::Cancelled`.
/// - Streaming class is declared honestly; the current InBharat CLI route is
///   `BufferedFinal`.
pub trait SpeechBackend: Send + Sync {
    fn backend_name(&self) -> &'static str;

    /// Readiness/status with a human-renderable fail-closed reason.
    fn status(&self) -> BackendStatus;

    /// Languages the backend can truthfully accept for ASR right now.
    fn asr_languages(&self) -> Vec<LanguageTag>;

    /// Languages the backend can truthfully accept for TTS right now.
    fn tts_languages(&self) -> Vec<LanguageTag>;

    /// What streaming actually is for this backend.
    fn streaming_class(&self) -> StreamingClass {
        StreamingClass::BufferedFinal
    }

    /// Speech-to-text. `language` is any alias; canonicalization happens here.
    fn transcribe(
        &self,
        audio_path: &std::path::Path,
        language: &str,
    ) -> Result<Transcription, SpeechError>;

    /// Text-to-speech. `language` is any alias; canonicalization happens here.
    fn synthesize(&self, text: &str, language: &str) -> Result<Synthesis, SpeechError>;

    /// Best-effort cancellation of in-flight speech operations.
    fn cancel_all(&self) -> Result<(), SpeechError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Requirement: Assamese addressable as both `as` and `as-IN` ----
    #[test]
    fn as_alias_maps_to_as_in() {
        for input in ["as", "as-IN", "as-in", "AS-in", "As-In"] {
            assert_eq!(
                canonicalize(input).unwrap().as_str(),
                "as-IN",
                "input '{input}' must address Assamese"
            );
        }
    }

    #[test]
    fn hi_alias_maps_to_hi_in() {
        for input in ["hi", "hi-IN", "hi-in"] {
            assert_eq!(canonicalize(input).unwrap().as_str(), "hi-IN");
        }
    }

    #[test]
    fn hinglish_maps_to_hi_en_codemix() {
        assert_eq!(canonicalize("hinglish").unwrap().as_str(), "hi-en-codemix");
        // Idempotent on the canonical form.
        assert_eq!(
            canonicalize("hi-en-codemix").unwrap().as_str(),
            "hi-en-codemix"
        );
    }

    // ---- Requirement: global languages must NOT become `xx-IN` ----
    #[test]
    fn fr_never_becomes_fr_in() {
        assert_eq!(canonicalize("fr").unwrap().as_str(), "fr");
        assert_eq!(canonicalize("FR").unwrap().as_str(), "fr");
        assert_eq!(canonicalize("fr-FR").unwrap().as_str(), "fr-FR");
    }

    #[test]
    fn en_us_stays_en_us() {
        assert_eq!(canonicalize("en-US").unwrap().as_str(), "en-US");
        assert_eq!(canonicalize("en-us").unwrap().as_str(), "en-US");
    }

    // ---- Requirement: Assamese never routes to Qwen3-ASR ----
    #[test]
    fn assamese_not_claimed_by_qwen3_route() {
        let assamese = canonicalize("as").unwrap();
        assert!(
            !provider_serves("qwen3_asr", SpeechTask::Asr, &assamese),
            "Qwen3-ASR covers Hindi but NOT Assamese; editing a manifest must never add it"
        );
        assert!(provider_serves(
            "qwen3_asr",
            SpeechTask::Asr,
            &canonicalize("hi").unwrap()
        ));
        assert!(provider_serves(
            "qwen3_asr",
            SpeechTask::Asr,
            &canonicalize("hinglish").unwrap()
        ));
    }

    #[test]
    fn assamese_routes_to_indicconformer_family() {
        let assamese = canonicalize("as-IN").unwrap();
        assert!(provider_serves(
            "indicconformer-asr",
            SpeechTask::Asr,
            &assamese
        ));
        // The 22 scheduled languages are declared for IndicConformer.
        let languages = provider_languages("indicconformer-asr", SpeechTask::Asr);
        assert_eq!(languages.len(), 22, "22 scheduled Indian-language packs");
    }

    // ---- Fail-closed language handling ----
    #[test]
    fn empty_language_is_rejected_not_bypassed() {
        assert_eq!(canonicalize("   "), Err(LanguageError::Empty));
        assert_eq!(canonicalize(""), Err(LanguageError::Empty));
    }

    #[test]
    fn malformed_tags_are_rejected() {
        for bad in ["f", "french--", "toolongsubtag1", "1n", "a-b-c-"] {
            assert!(
                matches!(canonicalize(bad), Err(LanguageError::MalformedTag(_))),
                "'{bad}' must be rejected"
            );
        }
    }

    #[test]
    fn auto_sentinel_is_reserved_and_passthrough() {
        let auto = canonicalize("auto").unwrap();
        assert!(auto.is_auto());
        assert_eq!(auto.as_str(), "auto");
    }

    #[test]
    fn canonicalization_is_idempotent() {
        for input in ["as", "hi", "hinglish", "en-US", "fr", "hi-IN", "as-IN"] {
            let once = canonicalize(input).unwrap();
            let twice = canonicalize(once.as_str()).unwrap();
            assert_eq!(
                once, twice,
                "canonicalize(canonicalize('{input}')) must be stable"
            );
        }
    }

    // ---- Table truthfulness invariants ----
    #[test]
    fn embedded_table_invariants_hold() {
        verify_table_invariants().expect("languages.v1.json must be internally consistent");
    }

    #[test]
    fn unknown_provider_serves_nothing() {
        let assamese = canonicalize("as").unwrap();
        assert!(!provider_serves("nonexistent", SpeechTask::Asr, &assamese));
        assert!(provider_languages("nonexistent", SpeechTask::Asr).is_empty());
    }

    #[test]
    fn provider_kind_respects_task() {
        let english = canonicalize("en-IN").unwrap();
        // whisper_cpp is 'both'; omnivoice is TTS-only and must not serve ASR.
        assert!(provider_serves("whisper_cpp", SpeechTask::Asr, &english));
        assert!(!provider_serves("omnivoice_tts", SpeechTask::Asr, &english));
        assert!(provider_serves("omnivoice_tts", SpeechTask::Tts, &english));
    }

    // ---- Streaming-class labels ----

    #[test]
    fn streaming_class_labels_are_stable() {
        // These strings are surfaced to UIs and logs (BharatAudioStatus
        // .streaming_class); they are a contract, not free text.
        assert_eq!(
            StreamingClass::StatefulStreaming.as_str(),
            "stateful-streaming"
        );
        assert_eq!(StreamingClass::SegmentChunked.as_str(), "segment-chunked");
        assert_eq!(StreamingClass::BufferedFinal.as_str(), "buffered-final");
    }
}
