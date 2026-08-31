// UnoOne Power — Speech router
// The single product-facing entry point for STT/TTS. Recording, UI, and agent
// code talk to `SpeechRouter`, never to a backend module directly.
//
// Two backends implement `unoone_speech_contracts::SpeechBackend`:
// - `InbharatSpeechBackend` — the InBharat Audio route (Qwen3-ASR /
//   OmniVoice via the audio.cpp CLI subprocess today; the shared C ABI
//   loaded from ibaudio.dll is the documented follow-up).
// - `LegacyVoiceBackend` — the legacy Whisper.cpp/Piper route, wrapped and
//   fixed (deadline, package-manifest hash verification, real errors).
//
// Routing policy is EXPLICIT. The default is InBharat-only: a failed
// InBharat gate surfaces an error; it never silently reroutes to the legacy
// Whisper/Piper plane. The legacy route is selected only when product
// policy (`SpeechRoutePolicy::InbharatAudioThenLegacy`) says so.

use std::path::{Path, PathBuf};

use unoone_speech_contracts::{
    provider_languages, BackendStatus, LanguageTag, SpeechBackend, SpeechError, SpeechTask,
    StreamingClass, Synthesis, Transcription,
};

use crate::bharat_audio;
use crate::voice::{self, VoiceConfig, VoiceModule};

/// Which backends a speech request may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechRoutePolicy {
    /// Default. InBharat Audio only; a failed gate is an error, not a reroute.
    InbharatAudioOnly,
    /// InBharat Audio first; the legacy Whisper/Piper plane may serve a
    /// request only if the InBharat gate fails. This must be deliberately
    /// selected by product configuration — it is never the default.
    InbharatAudioThenLegacy,
}

/// The product-facing speech router.
pub struct SpeechRouter {
    vault_root: String,
    policy: SpeechRoutePolicy,
}

impl SpeechRouter {
    pub fn new(vault_root: impl Into<String>, policy: SpeechRoutePolicy) -> Self {
        Self {
            vault_root: vault_root.into(),
            policy,
        }
    }

    pub fn policy(&self) -> SpeechRoutePolicy {
        self.policy
    }

    pub fn vault_root(&self) -> &str {
        &self.vault_root
    }

    /// The InBharat Audio backend for this root.
    pub fn inbharat_backend(&self) -> InbharatSpeechBackend {
        InbharatSpeechBackend::new(self.vault_root.clone())
    }

    /// The legacy Whisper/Piper backend for this root and language.
    pub fn legacy_backend(&self, language: &str) -> LegacyVoiceBackend {
        LegacyVoiceBackend::new(&self.vault_root, language)
    }

    /// Speech-to-text through the routed backend.
    pub fn transcribe(
        &self,
        audio_path: &Path,
        language: &str,
    ) -> Result<Transcription, SpeechError> {
        match self.inbharat_backend().transcribe(audio_path, language) {
            Ok(result) => Ok(result),
            Err(inbharat_error) => match self.policy {
                SpeechRoutePolicy::InbharatAudioOnly => Err(inbharat_error),
                SpeechRoutePolicy::InbharatAudioThenLegacy => {
                    self.legacy_backend(language)
                        .transcribe(audio_path, language)
                        .map_err(|legacy_error| {
                            SpeechError::Backend(format!(
                                "InBharat Audio failed ({inbharat_error}) and the legacy voice plane also failed ({legacy_error})"
                            ))
                        })
                }
            },
        }
    }

    /// Text-to-speech through the routed backend.
    pub fn synthesize(&self, text: &str, language: &str) -> Result<Synthesis, SpeechError> {
        match self.inbharat_backend().synthesize(text, language) {
            Ok(result) => Ok(result),
            Err(inbharat_error) => match self.policy {
                SpeechRoutePolicy::InbharatAudioOnly => Err(inbharat_error),
                SpeechRoutePolicy::InbharatAudioThenLegacy => {
                    self.legacy_backend(language)
                        .synthesize(text, language)
                        .map_err(|legacy_error| {
                            SpeechError::Backend(format!(
                                "InBharat Audio failed ({inbharat_error}) and the legacy voice plane also failed ({legacy_error})"
                            ))
                        })
                }
            },
        }
    }
}

/// The InBharat Audio route: Qwen3-ASR / OmniVoice behind the audio.cpp CLI
/// subprocess (via the hardened `bharat_audio` module). Buffered-final: the
/// CLI emits one final transcript; no stateful streaming is claimed.
pub struct InbharatSpeechBackend {
    vault_root: String,
}

impl InbharatSpeechBackend {
    pub fn new(vault_root: impl Into<String>) -> Self {
        Self {
            vault_root: vault_root.into(),
        }
    }

    /// Map the manifest's family name (e.g. `qwen3-asr`) to the provider key
    /// in `languages.v1.json` (e.g. `qwen3_asr`) for truthful coverage.
    fn provider_key(family: &str) -> String {
        family.replace('-', "_")
    }
}

impl SpeechBackend for InbharatSpeechBackend {
    fn backend_name(&self) -> &'static str {
        "inbharat-audio"
    }

    fn status(&self) -> BackendStatus {
        let status = bharat_audio::status(&self.vault_root);
        BackendStatus {
            backend_name: "inbharat-audio",
            ready: status.configured && status.enabled && status.production_ready,
            streaming_class: StreamingClass::BufferedFinal,
            reason: status.reason,
        }
    }

    fn asr_languages(&self) -> Vec<LanguageTag> {
        let status = bharat_audio::status(&self.vault_root);
        status
            .asr_family
            .as_deref()
            .map(|family| provider_languages(&Self::provider_key(family), SpeechTask::Asr))
            .unwrap_or_default()
    }

    fn tts_languages(&self) -> Vec<LanguageTag> {
        let status = bharat_audio::status(&self.vault_root);
        status
            .tts_family
            .as_deref()
            .map(|family| provider_languages(&Self::provider_key(family), SpeechTask::Tts))
            .unwrap_or_default()
    }

    fn streaming_class(&self) -> StreamingClass {
        StreamingClass::BufferedFinal
    }

    fn transcribe(&self, audio_path: &Path, language: &str) -> Result<Transcription, SpeechError> {
        let status = self.status();
        if !status.ready {
            return Err(SpeechError::NotReady(status.reason));
        }
        let result =
            bharat_audio::transcribe(&self.vault_root, &audio_path.to_string_lossy(), language)
                .map_err(SpeechError::Backend)?;
        // `bharat_audio::transcribe` already returns the canonical tag; if it
        // ever stops doing so, this is a hard error, never a silent passthrough.
        let language = unoone_speech_contracts::canonicalize(&result.language).map_err(|e| {
            SpeechError::Backend(format!(
                "InBharat Audio returned a non-canonical language tag: {e}"
            ))
        })?;
        Ok(Transcription {
            text: result.transcript,
            language,
            processing_time_ms: result.processing_time_ms,
        })
    }

    fn synthesize(&self, text: &str, language: &str) -> Result<Synthesis, SpeechError> {
        let status = self.status();
        if !status.ready {
            return Err(SpeechError::NotReady(status.reason));
        }
        let result = bharat_audio::synthesize(&self.vault_root, text, language)
            .map_err(SpeechError::Backend)?;
        Ok(Synthesis {
            audio_path: PathBuf::from(result.audio_path),
            sample_rate: result.sample_rate,
            duration_seconds: result.duration_seconds,
            processing_time_ms: result.processing_time_ms,
        })
    }

    fn cancel_all(&self) -> Result<(), SpeechError> {
        // The CLI-subprocess route has no in-flight handle registry yet; the
        // inference deadline (bharat_audio::run_command_timeout) is what
        // actually kills a runaway process. Real cooperative cancellation
        // arrives with the C-ABI migration (ibaudio_stream_* + CancellationToken).
        Ok(())
    }
}

/// The legacy Whisper.cpp/Piper route, wrapped behind the same contract.
/// Selected ONLY by explicit `SpeechRoutePolicy::InbharatAudioThenLegacy`
/// when the InBharat gate fails.
pub struct LegacyVoiceBackend {
    module: VoiceModule,
}

impl LegacyVoiceBackend {
    pub fn new(vault_root: &str, language: &str) -> Self {
        let config: VoiceConfig = voice::discover_voice_assets(vault_root, language);
        Self {
            module: VoiceModule::new(config),
        }
    }
}

impl SpeechBackend for LegacyVoiceBackend {
    fn backend_name(&self) -> &'static str {
        "legacy-whisper-piper"
    }

    fn status(&self) -> BackendStatus {
        let stt = self.module.check_stt_availability();
        let tts = self.module.check_tts_availability();
        // The plane is "ready" only if both halves exist; partial readiness
        // is reported with the missing half named.
        let ready = stt == voice::VoiceCapabilityStatus::Available
            && tts == voice::VoiceCapabilityStatus::Available;
        let reason = if ready {
            "legacy Whisper/Piper assets present and package-manifest verified".to_string()
        } else {
            format!(
                "legacy voice plane incomplete (STT: {:?}, TTS: {:?})",
                stt, tts
            )
        };
        BackendStatus {
            backend_name: "legacy-whisper-piper",
            ready,
            streaming_class: StreamingClass::BufferedFinal,
            reason,
        }
    }

    fn asr_languages(&self) -> Vec<LanguageTag> {
        // The shipped whisper model is English-only (whisper-base.en); the
        // provider table is the truthful source of that coverage.
        provider_languages("whisper_cpp", SpeechTask::Asr)
    }

    fn tts_languages(&self) -> Vec<LanguageTag> {
        provider_languages("piper_tts", SpeechTask::Tts)
    }

    fn streaming_class(&self) -> StreamingClass {
        StreamingClass::BufferedFinal
    }

    fn transcribe(
        &self,
        audio_path: &Path,
        // The module was constructed with this session language already
        // mapped to the legacy CLI vocabulary (voice::legacy_cli_language).
        _language: &str,
    ) -> Result<Transcription, SpeechError> {
        let result = self.module.transcribe(&audio_path.to_string_lossy());
        // Errors are surfaced as errors, never as transcript text (the old
        // bug); an Error status with no error string is still a failure.
        match result.status {
            voice::VoiceCapabilityStatus::Available => {
                // The module echoes back its own session language; canonicalize
                // it rather than trusting the string blindly.
                let language =
                    unoone_speech_contracts::canonicalize(&result.language).map_err(|e| {
                        SpeechError::Backend(format!(
                            "legacy Whisper STT returned an unusable language tag: {e}"
                        ))
                    })?;
                Ok(Transcription {
                    text: result.text,
                    language,
                    processing_time_ms: result.processing_time_ms,
                })
            }
            other => Err(SpeechError::Backend(format!(
                "legacy Whisper STT failed ({:?}): {}",
                other,
                result.error.unwrap_or_else(|| "no detail".to_string())
            ))),
        }
    }

    fn synthesize(&self, text: &str, _language: &str) -> Result<Synthesis, SpeechError> {
        if text.trim().is_empty() {
            return Err(SpeechError::InvalidInput("TTS text is empty".to_string()));
        }
        let result = self.module.synthesize(text);
        match (&result.audio_path, result.status) {
            (Some(path), voice::VoiceCapabilityStatus::Available) => Ok(Synthesis {
                audio_path: PathBuf::from(path),
                sample_rate: result.sample_rate,
                duration_seconds: result.duration_seconds,
                processing_time_ms: result.processing_time_ms,
            }),
            (_, other) => Err(SpeechError::Backend(format!(
                "legacy Piper TTS failed ({:?}): {}",
                other,
                result.error.unwrap_or_else(|| "no detail".to_string())
            ))),
        }
    }

    fn cancel_all(&self) -> Result<(), SpeechError> {
        // Same as the InBharat route: the deadline runner
        // (voice::run_with_deadline) kills runaway processes; cooperative
        // cancellation arrives with the C-ABI migration.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A root with no SPEECH config and no RUNTIMES: both backends fail closed.
    fn empty_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn cli_backend_reports_buffered_final_streaming_class() {
        let root = empty_root();
        let backend = SpeechRouter::new(
            root.path().to_string_lossy().to_string(),
            SpeechRoutePolicy::InbharatAudioOnly,
        )
        .inbharat_backend();
        assert_eq!(backend.streaming_class(), StreamingClass::BufferedFinal);
        let legacy = LegacyVoiceBackend::new(&root.path().to_string_lossy(), "en-IN");
        assert_eq!(legacy.streaming_class(), StreamingClass::BufferedFinal);
    }

    #[test]
    fn router_never_falls_back_to_legacy_without_policy() {
        let root = empty_root();
        let root_str = root.path().to_string_lossy().to_string();
        let router = SpeechRouter::new(root_str.clone(), SpeechRoutePolicy::InbharatAudioOnly);

        // The InBharat gate fails (no SPEECH config). With the default
        // policy this must surface an error — NOT run the legacy plane.
        let audio = root.path().join("capture.wav");
        std::fs::write(&audio, b"RIFF").unwrap();
        let err = router.transcribe(&audio, "hi").unwrap_err();
        assert!(
            matches!(err, SpeechError::NotReady(_)),
            "default policy must fail closed, got: {err}"
        );

        // Synthesis likewise.
        let err = router.synthesize("hello", "hi").unwrap_err();
        assert!(
            matches!(err, SpeechError::NotReady(_)),
            "default policy must fail closed, got: {err}"
        );
    }

    #[test]
    fn router_falls_back_to_legacy_only_when_policy_permits() {
        let root = empty_root();
        let root_str = root.path().to_string_lossy().to_string();
        let router =
            SpeechRouter::new(root_str.clone(), SpeechRoutePolicy::InbharatAudioThenLegacy);

        // The InBharat gate fails AND the legacy plane is absent — the
        // combined error names BOTH planes, proving the fallback path was
        // taken (and failed closed) rather than silently dropping.
        let audio = root.path().join("capture.wav");
        std::fs::write(&audio, b"RIFF").unwrap();
        let err = router.transcribe(&audio, "en-IN").unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("legacy voice plane also failed"),
            "fallback policy must surface the legacy failure too, got: {message}"
        );
    }

    #[test]
    fn inbharat_backend_not_ready_without_speech_config() {
        let root = empty_root();
        let backend = InbharatSpeechBackend::new(root.path().to_string_lossy().to_string());
        let status = backend.status();
        assert!(!status.ready, "missing SPEECH config must fail closed");
        assert!(!status.reason.is_empty());
        assert_eq!(status.backend_name, "inbharat-audio");
        assert!(backend.asr_languages().is_empty());
        assert!(backend.tts_languages().is_empty());
    }

    #[test]
    fn legacy_backend_languages_are_english_only() {
        let root = empty_root();
        let backend = LegacyVoiceBackend::new(&root.path().to_string_lossy(), "en-IN");
        // The shipped whisper-base.en model is English-only; the provider
        // table must never claim Hindi for it.
        let asr = backend.asr_languages();
        assert!(asr.iter().all(|lang| lang.as_str().starts_with("en")));
        assert!(backend.status().streaming_class == StreamingClass::BufferedFinal);
    }
}
