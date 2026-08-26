#ifndef INBHARAT_IBAUDIO_PROVIDER_HPP
#define INBHARAT_IBAUDIO_PROVIDER_HPP

// Internal provider contract. Applications never see this — they call the stable
// C ABI in ibaudio.h. A provider is a pluggable inference engine (the built-in
// deterministic reference engines, pinned audio.cpp, a local AI4Bharat service,
// a gated remote provider, …) behind one vtable. The capability router selects a
// provider from evidence-backed capabilities, never from filenames.
//
// Providers are internal C++ only; nothing here crosses the C ABI, so this layer
// can evolve without touching ABI v1. Every entry point is expected to honor the
// cooperative CancellationToken and to stay inside the runtime's memory/thread
// budgets. See docs/PROVIDER_API.md for the full contract.

#include "internal.hpp"

#include <string>
#include <vector>

namespace ibaudio {

// What a provider can genuinely do — evidence, not filenames. Mirrors the
// model-descriptor metadata already present in ABI v1, plus provider identity.
struct ProviderCapabilities {
    std::string id;             // stable provider id, e.g. "reference", "audiocpp", "sarvam"
    std::string version;        // provider implementation version
    std::string locality;       // "local-native" | "local-service" | "remote"
    std::string privacy_class;  // "ephemeral" | "transcript-only" | "audio-and-transcript" | "no-persistence"
    bool remote = false;        // true => requires network; excluded when remote providers are disabled
    std::vector<std::string> languages;  // BCP-47-ish codes the provider has evidence for, e.g. "en-IN", "hi-IN"
    bool supports_asr = false;
    bool supports_tts = false;
    bool supports_vad = false;
    bool supports_kws = false;
    bool streaming_asr = false; // true model streaming, honestly labeled
    bool streaming_tts = false;
    bool streaming_vad = false; // true streaming VAD (e.g. Silero), honestly labeled
};

// A provider is an engine adapter. The runtime owns registration; a model that
// resolves to a provider holds a pointer for dispatch. All methods must be
// exception-safe at the C++ boundary (callers wrap them in guarded()).
class Provider {
public:
    virtual ~Provider() = default;

    virtual const ProviderCapabilities &capabilities() const = 0;

    // Whether this provider backs a specific registered model family. Providers
    // override to claim their families; the default claims nothing. Used by the
    // capability router during production model resolution.
    virtual bool serves_family(const std::string &family) const {
        (void)family;
        return false;
    }

    // Inference entry points. The default implementations return UNSUPPORTED so a
    // provider only overrides the tasks it genuinely performs. Cancellation is
    // cooperative via the token; processed_* counters feed job/stream metrics.
    virtual ibaudio_status_t run_asr(const AudioData &mono_audio,
                                     const CancellationToken *cancel,
                                     uint64_t *processed_frames,
                                     std::string &out_text) {
        (void)mono_audio; (void)cancel; (void)processed_frames; (void)out_text;
        return IBAUDIO_STATUS_UNSUPPORTED;
    }

    virtual ibaudio_status_t run_tts(const std::string &text,
                                     const CancellationToken *cancel,
                                     uint64_t *processed_chars,
                                     AudioData &out_audio) {
        (void)text; (void)cancel; (void)processed_chars; (void)out_audio;
        return IBAUDIO_STATUS_UNSUPPORTED;
    }

    virtual ibaudio_status_t run_vad(const AudioData &mono_audio,
                                     const VadConfig &config,
                                     const CancellationToken *cancel,
                                     uint64_t *processed_frames,
                                     std::vector<ibaudio_vad_segment_v1> &out_segments) {
        (void)mono_audio; (void)config; (void)cancel; (void)processed_frames; (void)out_segments;
        return IBAUDIO_STATUS_UNSUPPORTED;
    }

    // Streaming VAD (optional). A provider with a true streaming VAD (streaming_vad=true)
    // overrides these to drive its incremental model. The default returns UNSUPPORTED, and
    // the stream layer falls back to the built-in frame-energy hysteresis path.
    // opaque state handle owned by the caller; create/destroy manage provider state.
    virtual ibaudio_status_t vad_stream_create(void **out_state) {
        (void)out_state; return IBAUDIO_STATUS_UNSUPPORTED;
    }
    // Push one audio chunk; emits zero or more VAD events via out_events.
    virtual ibaudio_status_t vad_stream_push(void *state,
                                             const float *samples,
                                             uint64_t frame_count,
                                             uint32_t sample_rate,
                                             std::vector<ibaudio_vad_segment_v1> &out_segments) {
        (void)state; (void)samples; (void)frame_count; (void)sample_rate; (void)out_segments;
        return IBAUDIO_STATUS_UNSUPPORTED;
    }
    virtual ibaudio_status_t vad_stream_finish(void *state,
                                               std::vector<ibaudio_vad_segment_v1> &out_segments) {
        (void)state; (void)out_segments; return IBAUDIO_STATUS_UNSUPPORTED;
    }
    virtual void vad_stream_destroy(void *state) { (void)state; }
};

// Registry: maps provider id -> instance, and answers "which provider can do this?"
// Registration is internal (compile-time built-ins + explicitly-enabled adapters);
// there is no runtime dynamic loading in v1, which keeps the supply chain closed.
class ProviderRegistry {
public:
    static ProviderRegistry &instance();

    void register_provider(Provider *provider);  // non-owning; providers are static
    Provider *find(const std::string &id) const;
    std::vector<Provider *> all() const;

    // Capability routing: return the highest-scoring provider that can perform the
    // task for the language under the given constraints. Returns nullptr when no
    // provider qualifies (callers surface UNAVAILABLE/DEFERRED, never a silent
    // fallback to a provider the policy excluded — e.g. a remote provider when the
    // deployment is offline).
    Provider *route(ibaudio_task_t task,
                    const std::string &language,
                    bool require_streaming,
                    bool remote_allowed) const;

    // Production model resolution: pick the provider backing a model family, subject
    // to the remote policy. Returns nullptr when the family is unbacked or when the
    // only backing provider is remote and remote is disallowed — never a silent
    // fallback to a provider the policy excluded.
    Provider *resolve_for_family(const std::string &family, bool remote_allowed) const;

private:
    std::vector<Provider *> providers_;  // registration order = priority order
};

} // namespace ibaudio

#endif // INBHARAT_IBAUDIO_PROVIDER_HPP
