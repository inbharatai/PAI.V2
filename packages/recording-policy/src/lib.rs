//! # Pocket AI recording retention policy
//!
//! This crate is the **single source of truth** for what each recording privacy
//! mode is allowed to persist. It exists as a separate, dependency-light crate
//! for one reason: **a privacy guarantee that cannot be tested is not a
//! guarantee.**
//!
//! The Tauri desktop shell (`unoone-power`) cannot be compiled on hosts without
//! WebView2/webkit2gtk, which means its unit tests cannot run on arbitrary CI
//! containers or Linux build machines. Retention semantics are too important to
//! inherit that limitation, so the decision logic lives here where it compiles
//! and is proven anywhere.
//!
//! ## Why this crate was created
//!
//! Prior to this crate, `stop_recording` matched
//! `Full | TranscriptOnly | SummaryOnly` in a **single arm** and wrote the
//! captured WAV to the vault for all three. `TRANSCRIPT_ONLY` and
//! `SUMMARY_ONLY` were therefore labels attached to fully retained audio. The
//! user-visible promise ("no audio is kept") was false.
//!
//! ## Design rules enforced here
//!
//! 1. **No wildcard matches.** Every `match` over [`PrivacyLevel`] is
//!    exhaustive and explicit, so adding a new mode is a compile error until a
//!    retention decision is made for it. A new privacy mode can never silently
//!    inherit "retain everything".
//! 2. **Policy is separate from execution.** [`PrivacyLevel::retention`]
//!    decides; the recording pipeline obeys. They are tested independently.
//! 3. **Claims are verified, not asserted.** After the pipeline finishes,
//!    it reports what it *actually* persisted via [`PersistedArtifacts`] and
//!    [`verify_retention`] proves it matched the policy. A violation is an
//!    error, not a log line.

#![forbid(unsafe_code)]
#![deny(clippy::wildcard_enum_match_arm)]

use serde::{Deserialize, Serialize};
use std::fmt;

/// Privacy level for a recording session.
///
/// Serialisation is `SCREAMING_SNAKE_CASE` to match the existing Tauri command
/// wire format exactly. Changing this breaks the frontend contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PrivacyLevel {
    /// Retain encrypted audio, transcript and summary.
    Full,
    /// Retain encrypted transcript and summary. Audio is transcribed, then
    /// destroyed. Audio is **never** committed to the vault.
    TranscriptOnly,
    /// Retain encrypted summary only. Audio and transcript are both
    /// intermediate values that are destroyed after use.
    SummaryOnly,
    /// Retain nothing. Everything is destroyed when the session ends.
    PrivateSession,
}

/// What a given privacy level permits to be persisted to the vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// Encrypted audio may be committed to the vault.
    pub audio: bool,
    /// Encrypted transcript may be committed to the vault.
    pub transcript: bool,
    /// Encrypted summary may be committed to the vault.
    pub summary: bool,
}

impl RetentionPolicy {
    /// True when this policy permits nothing at all to be persisted.
    pub const fn retains_nothing(&self) -> bool {
        !self.audio && !self.transcript && !self.summary
    }
}

/// What must happen to the captured audio buffer once capture stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AudioDisposition {
    /// Encrypt and commit to the vault, then zeroize the plaintext buffer.
    RetainEncrypted,
    /// Use in-memory for transcription, then zeroize without ever writing the
    /// audio to durable storage.
    TranscribeThenDestroy,
    /// Zeroize immediately. Do not transcribe, do not write.
    DestroyImmediately,
}

impl PrivacyLevel {
    /// Every privacy level, for exhaustive iteration in tests and diagnostics.
    pub const ALL: [PrivacyLevel; 4] = [
        PrivacyLevel::Full,
        PrivacyLevel::TranscriptOnly,
        PrivacyLevel::SummaryOnly,
        PrivacyLevel::PrivateSession,
    ];

    /// The authoritative retention decision for this level.
    ///
    /// Exhaustive by construction — no wildcard arm.
    pub const fn retention(self) -> RetentionPolicy {
        match self {
            PrivacyLevel::Full => RetentionPolicy {
                audio: true,
                transcript: true,
                summary: true,
            },
            PrivacyLevel::TranscriptOnly => RetentionPolicy {
                audio: false,
                transcript: true,
                summary: true,
            },
            PrivacyLevel::SummaryOnly => RetentionPolicy {
                audio: false,
                transcript: false,
                summary: true,
            },
            PrivacyLevel::PrivateSession => RetentionPolicy {
                audio: false,
                transcript: false,
                summary: false,
            },
        }
    }

    /// What the pipeline must do with the captured audio buffer.
    pub const fn audio_disposition(self) -> AudioDisposition {
        match self {
            PrivacyLevel::Full => AudioDisposition::RetainEncrypted,
            // Audio is still needed as transcription input, but must never
            // reach durable storage.
            PrivacyLevel::TranscriptOnly => AudioDisposition::TranscribeThenDestroy,
            // A summary is derived from a transcript, which is derived from
            // audio, so transcription still runs — it is simply not kept.
            PrivacyLevel::SummaryOnly => AudioDisposition::TranscribeThenDestroy,
            PrivacyLevel::PrivateSession => AudioDisposition::DestroyImmediately,
        }
    }

    /// True when the pipeline must run speech-to-text to satisfy this level.
    ///
    /// This closed a real gap: `stop_recording` previously never invoked
    /// transcription at all, so `TRANSCRIPT_ONLY` could not produce the
    /// transcript it claimed to be keeping.
    pub const fn requires_transcription(self) -> bool {
        let r = self.retention();
        r.transcript || r.summary
    }

    /// True when the pipeline must run summarisation to satisfy this level.
    pub const fn requires_summarization(self) -> bool {
        self.retention().summary
    }

    /// Stable identifier for logs and evidence records. Safe to log: contains
    /// no user content.
    pub const fn as_str(self) -> &'static str {
        match self {
            PrivacyLevel::Full => "FULL",
            PrivacyLevel::TranscriptOnly => "TRANSCRIPT_ONLY",
            PrivacyLevel::SummaryOnly => "SUMMARY_ONLY",
            PrivacyLevel::PrivateSession => "PRIVATE_SESSION",
        }
    }
}

impl fmt::Display for PrivacyLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the pipeline actually committed to durable storage.
///
/// Reported by the recording pipeline *after* the fact so the claim can be
/// checked against policy rather than trusted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedArtifacts {
    pub audio: bool,
    pub transcript: bool,
    pub summary: bool,
}

impl PersistedArtifacts {
    pub const NOTHING: PersistedArtifacts = PersistedArtifacts {
        audio: false,
        transcript: false,
        summary: false,
    };
}

/// A specific way in which persisted state violated the declared policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RetentionViolation {
    /// Audio was persisted under a level that forbids retaining audio. This is
    /// the exact defect this crate was built to make impossible.
    AudioPersistedButForbidden,
    /// A transcript was persisted under a level that forbids it.
    TranscriptPersistedButForbidden,
    /// A summary was persisted under a level that forbids it.
    SummaryPersistedButForbidden,
}

impl RetentionViolation {
    pub const fn as_str(self) -> &'static str {
        match self {
            RetentionViolation::AudioPersistedButForbidden => {
                "audio was written to durable storage but this privacy level forbids retaining audio"
            }
            RetentionViolation::TranscriptPersistedButForbidden => {
                "a transcript was written to durable storage but this privacy level forbids it"
            }
            RetentionViolation::SummaryPersistedButForbidden => {
                "a summary was written to durable storage but this privacy level forbids it"
            }
        }
    }
}

impl fmt::Display for RetentionViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Prove that what was persisted is permitted by the declared privacy level.
///
/// Only over-retention is a violation. Under-retention is legitimate: a
/// microphone may capture nothing, transcription may find no speech, or a
/// summariser may be unavailable. Those cases are reported honestly elsewhere
/// rather than being treated as privacy failures.
pub fn verify_retention(
    level: PrivacyLevel,
    persisted: PersistedArtifacts,
) -> Result<(), Vec<RetentionViolation>> {
    let allowed = level.retention();
    let mut violations = Vec::new();

    if persisted.audio && !allowed.audio {
        violations.push(RetentionViolation::AudioPersistedButForbidden);
    }
    if persisted.transcript && !allowed.transcript {
        violations.push(RetentionViolation::TranscriptPersistedButForbidden);
    }
    if persisted.summary && !allowed.summary {
        violations.push(RetentionViolation::SummaryPersistedButForbidden);
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// Overwrite a plaintext audio buffer before releasing it.
///
/// `Vec::clear` only sets the length to zero; the sample bytes remain in the
/// allocation until it happens to be reused. For microphone audio we overwrite
/// first, then clear, then release capacity so the samples are not recoverable
/// from a reused allocation or a core dump.
pub fn zeroize_audio(buffer: &mut Vec<f32>) {
    buffer.iter_mut().for_each(|s| *s = 0.0);
    buffer.clear();
    buffer.shrink_to_fit();
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // The regression this crate exists to prevent
    // ---------------------------------------------------------------

    /// The original defect: `Full | TranscriptOnly | SummaryOnly` shared one
    /// arm that wrote WAV audio to the vault for all three.
    #[test]
    fn only_full_may_retain_audio() {
        assert!(PrivacyLevel::Full.retention().audio);
        assert!(!PrivacyLevel::TranscriptOnly.retention().audio);
        assert!(!PrivacyLevel::SummaryOnly.retention().audio);
        assert!(!PrivacyLevel::PrivateSession.retention().audio);
    }

    #[test]
    fn transcript_only_persisting_audio_is_a_violation() {
        let persisted = PersistedArtifacts {
            audio: true,
            transcript: true,
            summary: true,
        };
        let err = verify_retention(PrivacyLevel::TranscriptOnly, persisted)
            .expect_err("retaining audio under TRANSCRIPT_ONLY must be rejected");
        assert!(err.contains(&RetentionViolation::AudioPersistedButForbidden));
    }

    #[test]
    fn summary_only_persisting_audio_or_transcript_is_a_violation() {
        let persisted = PersistedArtifacts {
            audio: true,
            transcript: true,
            summary: true,
        };
        let err = verify_retention(PrivacyLevel::SummaryOnly, persisted).unwrap_err();
        assert!(err.contains(&RetentionViolation::AudioPersistedButForbidden));
        assert!(err.contains(&RetentionViolation::TranscriptPersistedButForbidden));
        assert_eq!(
            err.len(),
            2,
            "summary itself is permitted under SUMMARY_ONLY"
        );
    }

    // ---------------------------------------------------------------
    // Per-mode retention tables
    // ---------------------------------------------------------------

    #[test]
    fn full_retains_everything() {
        let r = PrivacyLevel::Full.retention();
        assert!(r.audio && r.transcript && r.summary);
    }

    #[test]
    fn transcript_only_retains_transcript_and_summary() {
        let r = PrivacyLevel::TranscriptOnly.retention();
        assert!(!r.audio);
        assert!(r.transcript);
        assert!(r.summary);
    }

    #[test]
    fn summary_only_retains_summary_alone() {
        let r = PrivacyLevel::SummaryOnly.retention();
        assert!(!r.audio);
        assert!(!r.transcript);
        assert!(r.summary);
    }

    #[test]
    fn private_session_retains_nothing() {
        let r = PrivacyLevel::PrivateSession.retention();
        assert!(r.retains_nothing());
        assert_eq!(
            verify_retention(PrivacyLevel::PrivateSession, PersistedArtifacts::NOTHING),
            Ok(())
        );
    }

    #[test]
    fn private_session_rejects_any_persistence() {
        for (field, persisted) in [
            (
                "audio",
                PersistedArtifacts {
                    audio: true,
                    ..Default::default()
                },
            ),
            (
                "transcript",
                PersistedArtifacts {
                    transcript: true,
                    ..Default::default()
                },
            ),
            (
                "summary",
                PersistedArtifacts {
                    summary: true,
                    ..Default::default()
                },
            ),
        ] {
            assert!(
                verify_retention(PrivacyLevel::PrivateSession, persisted).is_err(),
                "PRIVATE_SESSION must reject persisting {field}"
            );
        }
    }

    // ---------------------------------------------------------------
    // Audio disposition drives the pipeline
    // ---------------------------------------------------------------

    #[test]
    fn audio_disposition_matches_retention() {
        assert_eq!(
            PrivacyLevel::Full.audio_disposition(),
            AudioDisposition::RetainEncrypted
        );
        assert_eq!(
            PrivacyLevel::TranscriptOnly.audio_disposition(),
            AudioDisposition::TranscribeThenDestroy
        );
        assert_eq!(
            PrivacyLevel::SummaryOnly.audio_disposition(),
            AudioDisposition::TranscribeThenDestroy
        );
        assert_eq!(
            PrivacyLevel::PrivateSession.audio_disposition(),
            AudioDisposition::DestroyImmediately
        );
    }

    /// Audio may only be committed when the disposition says to retain it.
    #[test]
    fn retain_encrypted_disposition_implies_audio_allowed() {
        for level in PrivacyLevel::ALL {
            let retains = matches!(level.audio_disposition(), AudioDisposition::RetainEncrypted);
            assert_eq!(
                retains,
                level.retention().audio,
                "{level}: disposition and retention policy disagree about audio"
            );
        }
    }

    // ---------------------------------------------------------------
    // Transcription requirement (the gap the directive did not catch)
    // ---------------------------------------------------------------

    #[test]
    fn transcription_required_whenever_a_derived_artifact_is_kept() {
        assert!(PrivacyLevel::Full.requires_transcription());
        assert!(PrivacyLevel::TranscriptOnly.requires_transcription());
        // A summary is derived from a transcript, so STT still has to run.
        assert!(PrivacyLevel::SummaryOnly.requires_transcription());
        assert!(!PrivacyLevel::PrivateSession.requires_transcription());
    }

    #[test]
    fn summarization_required_only_where_summary_is_kept() {
        assert!(PrivacyLevel::Full.requires_summarization());
        assert!(PrivacyLevel::TranscriptOnly.requires_summarization());
        assert!(PrivacyLevel::SummaryOnly.requires_summarization());
        assert!(!PrivacyLevel::PrivateSession.requires_summarization());
    }

    /// If a level needs no derived artifact, it must not ask for transcription;
    /// destroying audio immediately and transcribing it are contradictory.
    #[test]
    fn destroy_immediately_never_requires_transcription() {
        for level in PrivacyLevel::ALL {
            if matches!(
                level.audio_disposition(),
                AudioDisposition::DestroyImmediately
            ) {
                assert!(
                    !level.requires_transcription(),
                    "{level}: cannot transcribe audio it destroys immediately"
                );
            }
        }
    }

    // ---------------------------------------------------------------
    // Structural invariants across all modes
    // ---------------------------------------------------------------

    /// Retention must narrow monotonically: FULL ⊇ TRANSCRIPT_ONLY ⊇
    /// SUMMARY_ONLY ⊇ PRIVATE_SESSION. Any future edit that makes a stricter
    /// mode retain more than a looser one fails here.
    #[test]
    fn retention_narrows_monotonically() {
        let order = [
            PrivacyLevel::Full,
            PrivacyLevel::TranscriptOnly,
            PrivacyLevel::SummaryOnly,
            PrivacyLevel::PrivateSession,
        ];
        for pair in order.windows(2) {
            let (looser, stricter) = (pair[0].retention(), pair[1].retention());
            for (name, l, s) in [
                ("audio", looser.audio, stricter.audio),
                ("transcript", looser.transcript, stricter.transcript),
                ("summary", looser.summary, stricter.summary),
            ] {
                assert!(
                    l || !s,
                    "{name}: stricter mode retains more than looser mode"
                );
            }
        }
    }

    /// Every mode must accept a run that persisted exactly what it allows.
    #[test]
    fn policy_is_self_consistent_for_all_modes() {
        for level in PrivacyLevel::ALL {
            let allowed = level.retention();
            let persisted = PersistedArtifacts {
                audio: allowed.audio,
                transcript: allowed.transcript,
                summary: allowed.summary,
            };
            assert_eq!(
                verify_retention(level, persisted),
                Ok(()),
                "{level}: persisting exactly what policy allows must verify"
            );
        }
    }

    /// Under-retention is never a privacy violation.
    #[test]
    fn persisting_nothing_is_always_permitted() {
        for level in PrivacyLevel::ALL {
            assert_eq!(
                verify_retention(level, PersistedArtifacts::NOTHING),
                Ok(()),
                "{level}: persisting nothing must never be a violation"
            );
        }
    }

    // ---------------------------------------------------------------
    // Buffer hygiene
    // ---------------------------------------------------------------

    #[test]
    fn zeroize_overwrites_then_empties() {
        let mut buf = vec![0.87_f32; 4096];
        zeroize_audio(&mut buf);
        assert!(buf.is_empty());
        assert_eq!(buf.capacity(), 0, "capacity must be released, not retained");
    }

    #[test]
    fn zeroize_is_safe_on_empty_buffer() {
        let mut buf: Vec<f32> = Vec::new();
        zeroize_audio(&mut buf);
        assert!(buf.is_empty());
    }

    // ---------------------------------------------------------------
    // Wire format stability
    // ---------------------------------------------------------------

    #[test]
    fn privacy_level_wire_format_is_screaming_snake_case() {
        for (level, expected) in [
            (PrivacyLevel::Full, "\"FULL\""),
            (PrivacyLevel::TranscriptOnly, "\"TRANSCRIPT_ONLY\""),
            (PrivacyLevel::SummaryOnly, "\"SUMMARY_ONLY\""),
            (PrivacyLevel::PrivateSession, "\"PRIVATE_SESSION\""),
        ] {
            let json = serde_json::to_string(&level).unwrap();
            assert_eq!(json, expected, "frontend contract must not change");
            let back: PrivacyLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(back, level);
        }
    }

    #[test]
    fn display_matches_wire_identifier() {
        for level in PrivacyLevel::ALL {
            assert_eq!(level.to_string(), level.as_str());
        }
    }
}
