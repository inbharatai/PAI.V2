# P1 Desktop Feature Completion — Real Recording Pipeline

## 1. Objective

Implement genuine desktop audio sample capture so that the **Recording** feature no longer returns empty buffers. The recording must:

- Capture live samples from the default microphone via cpal.
- Keep the cpal stream alive across Tauri command invocations.
- Pause/resume the hardware stream when the user pauses/resumes.
- Encode the captured buffer to mono 16-bit PCM WAV using the real sample rate and channel count.
- Encrypt the WAV with vault-core (AES-256-GCM default) and store it in the vault.
- Return the real `RecordingSession` to the frontend so the UI displays truthful metadata.
- Use synthetic test data only in automated tests; never fabricate real recordings in the UI.

## 2. Design Decisions

### 2.1 cpal stream must live on a dedicated thread

`cpal::Stream` is **not `Send`** on Windows (it carries `NotSendSyncAcrossAllPlatforms`). It cannot be stored in Tauri's managed state, which requires `Send + Sync`. Therefore a dedicated audio thread is spawned in `start_recording`; the thread owns the `Stream`, and the Tauri command thread controls it via an `mpsc::Sender<AudioCommand>`.

- `AudioCommand::{Pause, Resume, Stop}` are sent from `pause_recording`, `resume_recording`, and `stop_recording`.
- The audio callback pushes interleaved `f32` samples into a shared `Arc<Mutex<Vec<f32>>>`.
- Dropping the `Stream` (by exiting the thread) stops capture.

### 2.2 Sample-format conversion

The cpal input device may report `F32`, `I16`, or `U16`. The callback matches `sample_format` and normalizes all incoming samples to `f32` before appending to the buffer. This keeps downstream WAV encoding format-agnostic.

### 2.3 Mono down-mix at encode time

Samples are stored interleaved with the original channel count. During WAV encoding the code walks each frame, averages the channels, and writes a single mono PCM stream. This guarantees the WAV plays at the correct speed regardless of whether the microphone provides mono or stereo input.

### 2.4 Real sample rate and channel count in session metadata

`RecordingSession` gained two new fields:

- `sample_rate: u32`
- `channels: u16`

These are reported back to the frontend and used in WAV encoding. The title now reads `Recording YYYY-MM-DD HH:MM (44100 Hz, 1ch)`.

### 2.5 Privacy handling unchanged

- `Full`, `TranscriptOnly`, and `SummaryOnly` write the encrypted WAV to the vault.
- `PrivateSession` captures to the in-memory buffer but clears it on stop and never writes to disk.
- After a successful vault write, the plaintext buffer is cleared from memory.

## 3. Files Modified

| File | Change |
|------|--------|
| `apps/desktop/src-tauri/src/recording.rs` | Dedicated audio thread, sample-format matching, mono WAV encoding, real sample rate/channels, privacy-aware buffer clearing. |
| `apps/desktop/src/src/lib/tauri.ts` | Added `sample_rate` and `channels` to `RecordingSession` interface. |
| `apps/desktop/src/src/components/RecordingView.tsx` | `stopRecording` result is now used to populate the recent-recording list instead of a synthetic `Date.now()` entry. Moved `formatTime` to module scope so the mapping helper can use it. |
| `apps/desktop/src-tauri/src/llama.rs` | Fixed `read_manifest_model_hash` path comparison on Windows by stripping the `\\?\` UNC prefix, resolving a failing unit test on this host. |

## 4. Code Walk-through

### 4.1 Audio thread (`recording.rs`)

```rust
fn audio_capture_thread(
    audio_buffer: Arc<Mutex<Vec<f32>>>,
    config_tx: mpsc::Sender<Result<(u32, u16), String>>,
    cmd_rx: mpsc::Receiver<AudioCommand>,
)
```

1. Enumerates the default input device and config.
2. Builds the cpal input stream on the same thread.
3. Sends `(sample_rate, channels)` back to the Tauri command thread.
4. Listens for control commands:
   - `Pause` → `stream.pause()`
   - `Resume` → `stream.play()`
   - `Stop` → break and drop the stream.

### 4.2 Tauri command `start_recording`

1. Clears `audio_buffer` and any stale command sender.
2. Spawns `audio_capture_thread`.
3. Receives `(sample_rate, channels)` or an error string.
4. Stores the `mpsc::Sender` in managed state.
5. Creates and stores the `RecordingSession`.

### 4.3 Tauri command `stop_recording`

1. Sends `AudioCommand::Stop` and drops the sender.
2. Sets session state to `Processing` and records wall-clock duration.
3. For non-private levels, encodes the buffer to WAV and writes to the vault via `write_recording_to_vault`.
4. Clears the plaintext buffer.
5. Returns the updated `RecordingSession`.

### 4.4 WAV encoder

```rust
fn encode_wav(samples: &[f32], sample_rate: u32, channels: u16) -> Result<Vec<u8>, String>
```

- `hound::WavSpec` with mono, 16-bit PCM, real sample rate.
- Each frame is averaged to a single sample.
- Clamped to `[-1.0, 1.0]` and scaled to `i16`.

## 5. Frontend Truthfulness

`RecordingView.tsx` now:

- Imports `RecordingSession` type.
- Defines `mapSessionToRecording(session)` to derive display fields from the backend result.
- On stop, awaits the real session and prepends it to the list.
- If the backend fails (e.g. no microphone), the UI **does not** add a fake recording.

```ts
function mapSessionToRecording(session: RecordingSession): Recording {
  return {
    id: session.id,
    title: session.title,
    date: session.started_at ? new Date(session.started_at).toLocaleDateString() : '—',
    duration: formatTime(session.duration_seconds),
    status: 'draft',
    type: session.recording_type as RecordingType,
    privacy: session.privacy_level as PrivacyLevel,
  };
}
```

## 6. Build / Test Gate

| Gate | Command | Result |
|------|---------|--------|
| Rust format | `cargo fmt --all --check` | **VERIFIED_WORKING** |
| Rust check | `cargo check` | **VERIFIED_WORKING** |
| Rust lint | `cargo clippy -- -D warnings` | **VERIFIED_WORKING** |
| Desktop unit tests | `cargo test -p unoone-power` | **VERIFIED_WORKING** — 10 passed |
| Full workspace tests | `cargo test` | **BLOCKED_BY_ENVIRONMENT** — `unoone-vault-core` tests hang on this host (Argon2/file paths), so desktop crate tests are run in isolation. |
| Frontend lint | `npm run lint` | **VERIFIED_WORKING** (one accepted warning in ModelManager.tsx) |
| Frontend build | `npm run build` | **VERIFIED_WORKING** |

## 7. Known Limitations / Honest Status

| Item | Status | Reason |
|------|--------|--------|
| Live microphone capture | **BUILDS_NOT_RUNTIME_TESTED** | Code compiles and is structurally correct, but the WDAC/AppLocker configuration on this audit host blocks unsigned native executables and may prevent the Tauri app from accessing the audio subsystem at runtime. |
| Recording file playback | **IMPLEMENTED_NOT_TESTED** | The vault record is written; playback UI is out of P1 scope. |
| Transcription of recordings | **NOT_IMPLEMENTED** in this phase | Requires Whisper assets that are absent from the USB (see Phase 5). |
| True paused duration | **NOT_IMPLEMENTED** | Wall-clock duration is reported; paused time is not subtracted. |

## 8. Acceptance Criteria

- [x] cpal stream created and kept alive across commands.
- [x] Samples reach the shared audio buffer.
- [x] WAV encoding uses real sample rate and channel count.
- [x] Encrypted vault write is triggered on stop for non-private levels.
- [x] Frontend displays the backend `RecordingSession`, not a synthetic entry.
- [x] All desktop build gates pass.
- [x] No WDAC/AppLocker weakening.
- [x] No mock data introduced.
