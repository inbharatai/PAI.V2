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
// does not auto-detect by extension). `what` names the root in the reason
// string (e.g. "audio.cpp ASR").
//
// Returns true and fills `reason` with what was found. Returns false and
// fills `reason` with the exact failure when the root is unconfigured,
// missing, unreadable, or contains no usable weights. Never reports success
// on an empty root: readiness fails closed.
bool probe_model_root(const char *root,
                      const char *what,
                      const char *required_extension,
                      char *reason,
                      std::size_t reason_size);

// Probe every model root the compiled adapter needs at runtime: the licensed
// Qwen3-ASR weights root (the pinned loader auto-detects models by the .gguf
// extension ONLY) and the bundled Silero VAD weights root. The first failure
// is reported; on success the reason names what was verified.
bool probe_assets(char *reason, std::size_t reason_size);

} // namespace ibaudio::audio_cpp_adapter

#endif // IBAUDIO_AUDIO_CPP_PROBE_HPP