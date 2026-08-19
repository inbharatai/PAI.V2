// UnoOne Power — Desktop Recording Engine
// Cross-platform audio recording with encrypted writing to the Pocket AI vault.
// State is shared across Tauri commands via tauri::State.
//
// Pipeline: cpal captures audio → hound encodes WAV in memory → the privacy
// level's retention policy decides what may be persisted → vault-core encrypts
// (AES-256-GCM for new records; legacy XChaCha20-Poly1305 records stay
// readable) → written to VAULT/records/.
//
// RETENTION IS POLICY-DRIVEN, NOT MODE-BLIND.
//
// This module previously matched `Full | TranscriptOnly | SummaryOnly` in a
// single arm and wrote the captured WAV to the vault for all three. That made
// TRANSCRIPT_ONLY and SUMMARY_ONLY cosmetic labels on fully retained audio —
// the privacy promise shown to the user was false.
//
// The retention decision now lives in `unoone-recording-policy`, a portable
// crate that is exhaustively unit-tested on every host (including CI containers
// that cannot build this Tauri shell). This module's job is to OBEY that policy
// and then PROVE it obeyed, via `verify_retention` on the way out.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use serde::{Deserialize, Serialize};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;
use unoone_recording_policy::{
    verify_retention, zeroize_audio, AudioDisposition, PersistedArtifacts,
};

// The canonical privacy level and its retention rules are defined once, in the
// policy crate, and re-exported here so the Tauri command signatures and the
// frontend wire format are unchanged.
pub use unoone_recording_policy::PrivacyLevel;

/// Recording state machine
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RecordingState {
    Idle,
    Recording,
    Paused,
    Processing,
    Stopped,
    Error,
}

/// Recording type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RecordingType {
    VoiceMemo,
    Meeting,
    Lecture,
    Interview,
    Note,
}

/// Truthful, machine-checkable outcome of a stopped recording.
///
/// Every field is derived from what actually happened, never from what was
/// intended. The frontend renders this instead of assuming a returned session
/// means success — a recording that captured no audio, could not transcribe, or
/// could not summarise is reported as such rather than shown as a silent green
/// "Stopped".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingOutcome {
    /// Privacy level that governed this session.
    pub privacy_level: PrivacyLevel,
    /// Number of audio samples actually captured from the microphone.
    pub samples_captured: usize,
    /// What was actually committed to the encrypted vault.
    pub persisted_audio: bool,
    pub persisted_transcript: bool,
    pub persisted_summary: bool,
    /// True only when post-hoc verification confirmed the persisted set matched
    /// the retention policy for `privacy_level`.
    pub retention_verified: bool,
    /// Set when a temporary decrypted WAV was created for transcription and
    /// then deleted. `Some(true)` means deletion was confirmed by re-checking
    /// the path.
    pub temp_audio_deletion_confirmed: Option<bool>,
    /// Non-fatal conditions worth surfacing (no speech detected, summariser
    /// unavailable, and so on). Contains no user content.
    pub warnings: Vec<String>,
    /// Plain-language message for the user. Never claims more than happened.
    pub user_message: String,
}

/// Recording session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingSession {
    pub id: String,
    pub title: String,
    pub state: RecordingState,
    pub recording_type: RecordingType,
    pub privacy_level: PrivacyLevel,
    pub started_at: Option<String>,
    pub stopped_at: Option<String>,
    pub duration_seconds: u64,
    pub bookmarks: Vec<RecordingBookmark>,
    pub source_platform: String,
    pub source_device_id: String,
    pub audio_path: Option<String>,
    pub transcript_path: Option<String>,
    pub summary_path: Option<String>,
    /// Encrypted record ID in the vault (set after stop_recording writes to vault)
    pub vault_record_id: Option<String>,
    /// Sample rate used during capture (set from the cpal input config)
    pub sample_rate: u32,
    /// Channel count used during capture (set from the cpal input config)
    pub channels: u16,
    /// Truthful, verified outcome of the stop operation. `None` while the
    /// session is still recording. The UI must render this rather than infer
    /// success from the presence of a session object.
    pub outcome: Option<RecordingOutcome>,
}

/// Bookmark in a recording
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingBookmark {
    pub timestamp_seconds: u64,
    pub label: Option<String>,
}

/// Commands sent from the Tauri command thread to the dedicated audio thread.
pub(crate) enum AudioCommand {
    Pause,
    Resume,
    Stop,
}

/// Shared recording engine state
pub struct RecordingStateHolder {
    pub current_session: Mutex<Option<RecordingSession>>,
    pub start_time: Mutex<Option<Instant>>,
    /// In-memory audio buffer. Shared with the cpal callback so samples can be
    /// pushed from the audio thread. Cleared on each new recording and after stop.
    pub audio_buffer: Arc<Mutex<Vec<f32>>>,
    /// Sender to the dedicated audio thread that owns the cpal `Stream`.
    /// cpal's stream is not `Send`, so it must live on the thread that created it.
    pub command_sender: Mutex<Option<mpsc::Sender<AudioCommand>>>,
    /// Pocket AI vault root captured at `start_recording`. Needed on stop to
    /// discover the bundled Whisper assets for transcription. Held in state
    /// rather than on the session so the path is not shipped to the frontend.
    pub vault_root: Mutex<Option<String>>,
    /// Outcome of the most recently stopped session, for diagnostics export.
    pub last_outcome: Mutex<Option<RecordingOutcome>>,
}

impl RecordingStateHolder {
    pub fn new() -> Self {
        Self {
            current_session: Mutex::new(None),
            start_time: Mutex::new(None),
            audio_buffer: Arc::new(Mutex::new(Vec::new())),
            command_sender: Mutex::new(None),
            vault_root: Mutex::new(None),
            last_outcome: Mutex::new(None),
        }
    }

    /// Stop capture and discard plaintext buffers without writing to a USB
    /// that may already have been removed.
    pub fn emergency_discard(&self) {
        if let Ok(sender) = self.command_sender.lock() {
            if let Some(sender) = sender.as_ref() {
                let _ = sender.send(AudioCommand::Stop);
            }
        }
        if let Ok(mut sender) = self.command_sender.lock() {
            *sender = None;
        }
        if let Ok(mut samples) = self.audio_buffer.lock() {
            // Overwrite, empty, then release the allocation so captured audio
            // is not recoverable from a reused buffer or a core dump.
            zeroize_audio(&mut samples);
        }
        if let Ok(mut session) = self.current_session.lock() {
            *session = None;
        }
        if let Ok(mut start_time) = self.start_time.lock() {
            *start_time = None;
        }
        if let Ok(mut root) = self.vault_root.lock() {
            *root = None;
        }
    }
}

fn get_device_id() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "desktop-unknown".to_string())
}

/// Dedicated audio thread. The cpal `Stream` is created and stays on this thread
/// because it is not `Send`. The callback pushes interleaved f32 samples into
/// the shared `audio_buffer`. The thread loop handles pause/resume/stop commands.
fn audio_capture_thread(
    audio_buffer: Arc<Mutex<Vec<f32>>>,
    config_tx: mpsc::Sender<Result<(u32, u16), String>>,
    cmd_rx: mpsc::Receiver<AudioCommand>,
) {
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => {
            let _ = config_tx.send(Err("No audio input device found".to_string()));
            return;
        }
    };

    let supported_config = match device.supported_input_configs() {
        Ok(mut iter) => match iter.next() {
            Some(c) => c.with_max_sample_rate(),
            None => {
                let _ = config_tx.send(Err(
                    "No supported audio input configuration found".to_string()
                ));
                return;
            }
        },
        Err(e) => {
            let _ = config_tx.send(Err(format!("Audio config error: {}", e)));
            return;
        }
    };

    let sample_format = supported_config.sample_format();
    let config: cpal::StreamConfig = supported_config.into();
    let sample_rate = config.sample_rate.0;
    let channels = config.channels;

    let buf = Arc::clone(&audio_buffer);
    let err_fn = move |err| eprintln!("cpal input stream error: {}", err);

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if let Ok(mut b) = buf.lock() {
                    b.extend_from_slice(data);
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                if let Ok(mut b) = buf.lock() {
                    b.extend(data.iter().map(|s| s.to_sample::<f32>()));
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _: &cpal::InputCallbackInfo| {
                if let Ok(mut b) = buf.lock() {
                    b.extend(data.iter().map(|s| s.to_sample::<f32>()));
                }
            },
            err_fn,
            None,
        ),
        other => {
            let _ = config_tx.send(Err(format!(
                "Unsupported sample format {:?}. Only F32/I16/U16 are supported.",
                other
            )));
            return;
        }
    };

    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            let _ = config_tx.send(Err(format!("Failed to build audio input stream: {}", e)));
            return;
        }
    };

    if let Err(e) = stream.play() {
        let _ = config_tx.send(Err(format!("Failed to start audio stream: {}", e)));
        return;
    }

    if config_tx.send(Ok((sample_rate, channels))).is_err() {
        return; // Caller dropped; nothing to do.
    }

    loop {
        match cmd_rx.recv() {
            Ok(AudioCommand::Pause) => {
                let _ = stream.pause();
            }
            Ok(AudioCommand::Resume) => {
                let _ = stream.play();
            }
            Ok(AudioCommand::Stop) | Err(_) => break,
        }
    }
    // `stream` drops here, stopping capture.
}

/// Start recording audio from the default input device.
/// For PrivateSession: audio stays in memory only, never written to vault.
/// For all other levels: audio will be encrypted and written to vault on stop.
#[tauri::command]
pub fn start_recording(
    recording_type: RecordingType,
    privacy_level: PrivacyLevel,
    vault_root: String,
    state: tauri::State<'_, RecordingStateHolder>,
) -> Result<RecordingSession, String> {
    // Retain the vault root: `stop_recording` needs it to locate the bundled
    // Whisper assets for privacy levels that require transcription.
    *state
        .vault_root
        .lock()
        .map_err(|e| format!("State lock error: {}", e))? = Some(vault_root);

    // Clear the audio buffer and any previous command sender.
    state
        .audio_buffer
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?
        .clear();
    *state
        .command_sender
        .lock()
        .map_err(|e| format!("State lock error: {}", e))? = None;

    // Spawn a dedicated audio thread to own the cpal stream (which is not Send).
    let (config_tx, config_rx) = mpsc::channel::<Result<(u32, u16), String>>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<AudioCommand>();
    let audio_buffer = Arc::clone(&state.audio_buffer);

    std::thread::spawn(move || audio_capture_thread(audio_buffer, config_tx, cmd_rx));

    let (sample_rate, channels) = config_rx
        .recv()
        .map_err(|e| format!("Audio capture thread failed to start: {}", e))??;

    *state
        .command_sender
        .lock()
        .map_err(|e| format!("State lock error: {}", e))? = Some(cmd_tx);

    let session = RecordingSession {
        id: uuid::Uuid::new_v4().to_string(),
        title: format!(
            "Recording {} ({} Hz, {}ch)",
            chrono::Utc::now().format("%Y-%m-%d %H:%M"),
            sample_rate,
            channels
        ),
        state: RecordingState::Recording,
        recording_type,
        privacy_level,
        started_at: Some(chrono::Utc::now().to_rfc3339()),
        stopped_at: None,
        duration_seconds: 0,
        bookmarks: Vec::new(),
        source_platform: "DESKTOP".to_string(),
        source_device_id: get_device_id(),
        audio_path: None,
        transcript_path: None,
        summary_path: None,
        vault_record_id: None,
        sample_rate,
        channels,
        outcome: None,
    };

    *state
        .current_session
        .lock()
        .map_err(|e| format!("State lock error: {}", e))? = Some(session.clone());
    *state
        .start_time
        .lock()
        .map_err(|e| format!("State lock error: {}", e))? = Some(Instant::now());

    Ok(session)
}

#[tauri::command]
pub fn pause_recording(
    state: tauri::State<'_, RecordingStateHolder>,
) -> Result<RecordingSession, String> {
    let mut session = state
        .current_session
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    if let Some(ref mut s) = *session {
        s.state = RecordingState::Paused;
        if let Ok(sender_guard) = state.command_sender.lock() {
            if let Some(ref sender) = *sender_guard {
                let _ = sender.send(AudioCommand::Pause);
            }
        }
        Ok(s.clone())
    } else {
        Err("No active recording session".to_string())
    }
}

#[tauri::command]
pub fn resume_recording(
    state: tauri::State<'_, RecordingStateHolder>,
) -> Result<RecordingSession, String> {
    let mut session = state
        .current_session
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    if let Some(ref mut s) = *session {
        s.state = RecordingState::Recording;
        if let Ok(sender_guard) = state.command_sender.lock() {
            if let Some(ref sender) = *sender_guard {
                let _ = sender.send(AudioCommand::Resume);
            }
        }
        Ok(s.clone())
    } else {
        Err("No active recording session".to_string())
    }
}

/// Stop recording. For PrivateSession, the audio is discarded.
/// For all other privacy levels, the audio is encoded to WAV and
/// encrypted via vault-core's write_record (AES-256-GCM default).
#[tauri::command]
pub fn stop_recording(
    state: tauri::State<'_, RecordingStateHolder>,
    vault_state: tauri::State<'_, crate::DesktopVaultState>,
) -> Result<RecordingSession, String> {
    // Stop the cpal stream first so no more samples arrive while we encode.
    if let Ok(sender_guard) = state.command_sender.lock() {
        if let Some(ref sender) = *sender_guard {
            let _ = sender.send(AudioCommand::Stop);
        }
    }
    *state
        .command_sender
        .lock()
        .map_err(|e| format!("State lock error: {}", e))? = None;

    let mut session_lock = state
        .current_session
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let start_time_lock = state
        .start_time
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;

    let session = session_lock.as_mut().ok_or("No active recording session")?;
    session.state = RecordingState::Processing;
    session.stopped_at = Some(chrono::Utc::now().to_rfc3339());

    if let Some(start) = *start_time_lock {
        session.duration_seconds = start.elapsed().as_secs();
    }

    // ------------------------------------------------------------------
    // Retention is decided by policy, never by an ad-hoc match here.
    // ------------------------------------------------------------------
    let level = session.privacy_level;
    let policy = level.retention();
    let disposition = level.audio_disposition();

    let vault_root = state
        .vault_root
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?
        .clone();

    // Take ownership of the samples and release the buffer lock at once, so the
    // audio thread is never blocked behind vault or Whisper I/O.
    let mut samples: Vec<f32> = {
        let mut buf = state
            .audio_buffer
            .lock()
            .map_err(|e| format!("State lock error: {}", e))?;
        std::mem::take(&mut *buf)
    };

    let mut outcome = RecordingOutcome {
        privacy_level: level,
        samples_captured: samples.len(),
        persisted_audio: false,
        persisted_transcript: false,
        persisted_summary: false,
        retention_verified: false,
        temp_audio_deletion_confirmed: None,
        warnings: Vec::new(),
        user_message: String::new(),
    };

    // A session that captured nothing must not look like a successful save.
    // The previous implementation returned a `Stopped` session with no record
    // and no explanation — indistinguishable from a saved recording.
    if samples.is_empty() {
        zeroize_audio(&mut samples);
        session.state = RecordingState::Error;
        session.vault_record_id = None;
        outcome.retention_verified = true; // nothing persisted, nothing to violate
        outcome.warnings.push("ZERO_SAMPLES_CAPTURED".to_string());
        outcome.user_message = "No audio was captured, so nothing was saved. \
             Check that the microphone is connected, unmuted, and permitted for \
             UnoOne Power, then record again."
            .to_string();
        session.outcome = Some(outcome.clone());
        *state
            .last_outcome
            .lock()
            .map_err(|e| format!("State lock error: {}", e))? = Some(outcome);
        return Ok(session.clone());
    }

    // Encode once. The WAV is needed as vault payload, transcription input, or
    // both, depending on the policy.
    let wav_bytes = encode_wav(&samples, session.sample_rate, session.channels)?;
    // Raw samples are no longer needed in any mode.
    zeroize_audio(&mut samples);

    let mut audio_record_id: Option<String> = None;
    let mut transcript_text: Option<String> = None;

    match disposition {
        // PRIVATE_SESSION — destroy without transcribing or writing.
        AudioDisposition::DestroyImmediately => {}

        // FULL — audio is retained, encrypted, in the vault.
        AudioDisposition::RetainEncrypted => {
            let id = write_artifact_to_vault(
                unoone_vault_core::RecordType::Recording,
                session,
                None,
                &wav_bytes,
                &vault_state,
            )?;
            session.audio_path = Some(format!("vault://records/{}", id));
            audio_record_id = Some(id);
            outcome.persisted_audio = true;
        }

        // TRANSCRIPT_ONLY / SUMMARY_ONLY — audio is transcription input only
        // and must never reach the vault.
        AudioDisposition::TranscribeThenDestroy => match vault_root.as_deref() {
            None => {
                outcome
                    .warnings
                    .push("VAULT_ROOT_UNAVAILABLE_TRANSCRIPTION_SKIPPED".to_string());
            }
            Some(root) => {
                let (text, deletion_confirmed, warning) =
                    transcribe_transiently(&wav_bytes, root, &session.privacy_level.to_string());
                outcome.temp_audio_deletion_confirmed = Some(deletion_confirmed);
                if let Some(w) = warning {
                    outcome.warnings.push(w);
                }
                if !text.trim().is_empty() {
                    transcript_text = Some(text);
                }
            }
        },
    }

    // Persist the transcript only where policy permits it.
    if policy.transcript {
        match &transcript_text {
            Some(text) => {
                let id = write_artifact_to_vault(
                    unoone_vault_core::RecordType::Transcript,
                    session,
                    audio_record_id.clone(),
                    text.as_bytes(),
                    &vault_state,
                )?;
                session.transcript_path = Some(format!("vault://records/{}", id));
                outcome.persisted_transcript = true;
            }
            None => {
                outcome.warnings.push("NO_TRANSCRIPT_PRODUCED".to_string());
            }
        }
    }

    // Summarisation is declared by policy but not yet implemented anywhere in
    // this product. Rather than silently retain nothing while reporting
    // success, the gap is surfaced. See 61_RECORDING_PRIVACY_ACCEPTANCE.md.
    if policy.summary {
        outcome
            .warnings
            .push("SUMMARIZATION_NOT_IMPLEMENTED".to_string());
    }

    // ------------------------------------------------------------------
    // Prove the policy was obeyed instead of assuming it.
    // ------------------------------------------------------------------
    let persisted = PersistedArtifacts {
        audio: outcome.persisted_audio,
        transcript: outcome.persisted_transcript,
        summary: outcome.persisted_summary,
    };
    match verify_retention(level, persisted) {
        Ok(()) => outcome.retention_verified = true,
        Err(violations) => {
            // Defence in depth: unreachable while the code above is correct.
            // If it ever fires, fail loudly rather than report a false save.
            session.state = RecordingState::Error;
            let detail = violations
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(format!(
                "Retention policy violation for {}: {}. The recording was stopped \
                 and this has been reported as a failure.",
                level, detail
            ));
        }
    }

    session.vault_record_id = audio_record_id;
    session.state = RecordingState::Stopped;
    outcome.user_message = describe_outcome(&outcome);
    session.outcome = Some(outcome.clone());
    *state
        .last_outcome
        .lock()
        .map_err(|e| format!("State lock error: {}", e))? = Some(outcome);

    Ok(session.clone())
}

/// Build a plain-language summary of what was actually kept.
///
/// Deliberately never says "saved" unless something really was.
fn describe_outcome(outcome: &RecordingOutcome) -> String {
    let mut kept: Vec<&str> = Vec::new();
    if outcome.persisted_audio {
        kept.push("audio");
    }
    if outcome.persisted_transcript {
        kept.push("transcript");
    }
    if outcome.persisted_summary {
        kept.push("summary");
    }

    let mut msg = if kept.is_empty() {
        format!(
            "Recording stopped under {}. Nothing was retained.",
            outcome.privacy_level
        )
    } else {
        format!(
            "Recording stopped under {}. Encrypted {} saved to the Pocket AI vault.",
            outcome.privacy_level,
            kept.join(" and ")
        )
    };

    if outcome.temp_audio_deletion_confirmed == Some(false) {
        msg.push_str(
            " Warning: the temporary audio file used for transcription could not \
             be confirmed deleted.",
        );
    }
    if outcome
        .warnings
        .iter()
        .any(|w| w == "SUMMARIZATION_NOT_IMPLEMENTED")
    {
        msg.push_str(
            " This privacy level expects a summary, but no summarisation \
             capability is bundled yet, so no summary was produced.",
        );
    }
    if outcome
        .warnings
        .iter()
        .any(|w| w == "NO_TRANSCRIPT_PRODUCED")
    {
        msg.push_str(" No speech was transcribed, so no transcript was saved.");
    }
    msg
}

/// Transcribe a WAV buffer without ever committing the audio to durable
/// storage.
///
/// Whisper.cpp reads from a file path, so a temporary WAV is unavoidable. It is
/// written outside the vault, deleted immediately after transcription, and the
/// deletion is then **verified** by re-checking the path — the directive
/// requires proof of deletion, not an attempt.
///
/// Returns `(transcript_text, deletion_confirmed, optional_warning)`.
fn transcribe_transiently(
    wav_bytes: &[u8],
    vault_root: &str,
    level_label: &str,
) -> (String, bool, Option<String>) {
    let _ = level_label;

    let temp_path = std::env::temp_dir().join(format!("unoone-stt-{}.wav", uuid::Uuid::new_v4()));

    if let Err(e) = std::fs::write(&temp_path, wav_bytes) {
        return (
            String::new(),
            true, // nothing was written, so nothing is left behind
            Some(format!("TEMP_AUDIO_WRITE_FAILED:{}", e)),
        );
    }

    let config = crate::voice::discover_voice_assets(vault_root, "en");
    let module = crate::voice::VoiceModule::new(config);
    let result = module.transcribe(&temp_path.to_string_lossy());

    // Delete first, then prove it is gone.
    let _ = std::fs::remove_file(&temp_path);
    let deletion_confirmed = !temp_path.exists();

    let warning = match result.status {
        crate::voice::VoiceCapabilityStatus::Available => None,
        other => Some(format!("STT_UNAVAILABLE:{:?}", other)),
    };

    (result.text, deletion_confirmed, warning)
}

#[tauri::command]
pub fn add_bookmark(
    label: Option<String>,
    state: tauri::State<'_, RecordingStateHolder>,
) -> Result<RecordingSession, String> {
    let mut session = state
        .current_session
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let start_time = state
        .start_time
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;

    if let Some(ref mut s) = *session {
        let timestamp = if let Some(start) = *start_time {
            start.elapsed().as_secs()
        } else {
            0
        };

        s.bookmarks.push(RecordingBookmark {
            timestamp_seconds: timestamp,
            label,
        });

        Ok(s.clone())
    } else {
        Err("No active recording session".to_string())
    }
}

/// Encode interleaved audio samples as mono 16-bit PCM WAV in memory.
/// `channels` is the original channel count from the cpal input config; samples
/// are down-mixed to mono so playback speed is correct regardless of source.
fn encode_wav(samples: &[f32], sample_rate: u32, channels: u16) -> Result<Vec<u8>, String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let channel_count = channels.max(1) as usize;

    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut buf, spec)
            .map_err(|e| format!("WAV writer error: {}", e))?;

        // Down-mix each frame to mono by averaging channels.
        for frame in samples.chunks(channel_count) {
            let mono_sample = if frame.len() == 1 {
                frame[0]
            } else {
                frame.iter().sum::<f32>() / frame.len() as f32
            };
            let clamped = mono_sample.clamp(-1.0, 1.0);
            let sample_i16 = (clamped * 32767.0) as i16;
            writer
                .write_sample(sample_i16)
                .map_err(|e| format!("WAV write error: {}", e))?;
        }
        writer
            .finalize()
            .map_err(|e| format!("WAV finalize error: {}", e))?;
    }

    Ok(buf.into_inner())
}

/// Write one encrypted artifact (audio, transcript, or summary) to the vault.
///
/// Replaces the former `write_recording_to_vault`, which hardcoded
/// `RecordType::Recording` and derived the vault privacy label from the
/// recording privacy level — a mapping that implied audio was being withheld
/// (`TranscriptOnly -> SummaryOnly`) while the caller wrote the audio anyway.
/// Retention is now enforced by the caller via the policy crate, so every
/// artifact that legitimately reaches this function is private user content and
/// is labelled as such.
fn write_artifact_to_vault(
    record_type: unoone_vault_core::RecordType,
    session: &RecordingSession,
    parent_record_id: Option<String>,
    bytes: &[u8],
    vault_state: &tauri::State<'_, crate::DesktopVaultState>,
) -> Result<String, String> {
    let mut vault_opt = vault_state
        .vault
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let vault = vault_opt
        .as_mut()
        .ok_or("Vault is not unlocked — cannot write recording. Unlock the vault first.")?;

    use unoone_vault_core::{PrivacyLevel as VaultPrivacyLevel, Record};

    let mut record = Record::new(record_type, "DESKTOP", &session.source_device_id);
    record.privacy_level = VaultPrivacyLevel::Private;
    record.parent_record_id = parent_record_id;

    let record_id = record.record_id.clone();

    // Encrypted by vault-core. New records use AES-256-GCM; the cipher is
    // identified on read by nonce length, so legacy XChaCha20-Poly1305 records
    // remain readable.
    vault
        .write_record(record, bytes)
        .map_err(|e| format!("Vault write failed: {}", e))?;

    Ok(record_id)
}
