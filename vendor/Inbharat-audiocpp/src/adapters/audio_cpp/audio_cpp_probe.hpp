#ifndef IBAUDIO_AUDIO_CPP_PROBE_HPP
#define IBAUDIO_AUDIO_CPP_PROBE_HPP

// Runtime readiness probe for the pinned audio.cpp adapter.
//
// `ibaudio_runtime_get_audio_cpp_status` must never report
// `inference_ready` from compile-time truth alone ("the adapter compiled,
// therefore it is ready"). Readiness is a fact about the machine: the
// configured model roots must actually exist and contain usable local
// weights. This probe is the single authority for that fact.

#include <cstddef>

namespace ibaudio::audio_cpp_adapter {

// Probe one configured model root for usable local assets. `required_extension`
// may be nullptr, in which case any regular file counts (weights whose loader
// does not auto-detect by extension). `required_file_name` may be nullptr; when
// set, only a regular file with exactly that (case-insensitive) name counts —
// for loaders that resolve assets by exact filename. `what` names the root in
// the reason string (e.g. "audio.cpp ASR").
//
// The probe fails closed on every packaging error it can see: unconfigured or
// missing root, unreadable directory, zero matching weights, MORE THAN ONE
// matching weights file (the loader cannot disambiguate), unreadable or
// zero-byte weights, and — when `.gguf` is the required extension — a file
// that does not carry the GGUF magic header with a structurally possible
// header size. Returns true and fills `reason` with what was found.
bool probe_model_root(const char *root,
                      const char *what,
                      const char *required_extension,
                      char *reason,
                      std::size_t reason_size,
                      const char *required_file_name = nullptr);

// Probe every model root the compiled adapter needs at runtime: the licensed
// Qwen3-ASR weights root (the pinned loader auto-detects models by the .gguf
// extension ONLY) and the bundled Silero VAD weights root (the pinned resolver
// requires exactly silero_vad_16k.safetensors inside it). The first failure
// is reported; on success the reason names what was verified.
bool probe_assets(char *reason, std::size_t reason_size);

// Content-hash gate on top of the readiness probe: re-runs the probe (so an
// unreadable, ambiguous, or structurally invalid root fails first), then
// hashes the single matching weights file and compares it against
// `expected_sha256` (exactly 64 hex characters, case-insensitive). The
// candidate selection is shared with probe_model_root, so the gate always
// hashes the exact file the loader would mount. Use this when a deployment
// pins a digest for the baked-root weights (the adapter wires it to
// IBAUDIO_AUDIO_CPP_QWEN3_ASR_SHA256); a missing/invalid digest or a
// mismatch fails closed with the reason set.
bool verify_model_root_sha256(const char *root,
                              const char *what,
                              const char *required_extension,
                              const char *required_file_name,
                              const char *expected_sha256,
                              char *reason,
                              std::size_t reason_size);

} // namespace ibaudio::audio_cpp_adapter

#endif // IBAUDIO_AUDIO_CPP_PROBE_HPP