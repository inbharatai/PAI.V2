// Built-in reference provider: adapts the existing deterministic engines
// (run_reference_asr / run_reference_tts / run_energy_vad in src/audio.cpp) to the
// provider contract. This is the always-available local-native provider. It is
// honest about what it is: a signal analyzer / tone synthesizer / energy VAD used
// to make API, lifecycle, streaming, and platform work testable without importing
// unreviewed models. It is NOT trained speech recognition or a natural voice.

#include "../provider.hpp"

namespace ibaudio {
namespace {

class ReferenceProvider final : public Provider {
public:
    const ProviderCapabilities &capabilities() const override {
        static const ProviderCapabilities caps = [] {
            ProviderCapabilities c;
            c.id = "reference";
            c.version = "0.1.0-rc2";
            c.locality = "local-native";
            c.privacy_class = "ephemeral";
            c.remote = false;
            c.languages = {};  // language-agnostic deterministic signal analysis
            c.supports_asr = true;
            c.supports_tts = true;
            c.supports_vad = true;
            c.supports_kws = false;
            // Honest streaming labels: ASR is window-incremental-revisable,
            // TTS is segment-chunked. Neither is a true neural streaming model.
            c.streaming_asr = true;
            c.streaming_tts = true;
            return c;
        }();
        return caps;
    }

    bool serves_family(const std::string &family) const override {
        return family == "reference-asr" || family == "reference-tts" || family == "energy-vad";
    }

    ibaudio_status_t run_asr(const AudioData &mono_audio,
                             const CancellationToken *cancel,
                             uint64_t *processed_frames,
                             std::string &out_text) override {
        out_text = run_reference_asr(mono_audio, cancel, processed_frames);
        return IBAUDIO_STATUS_OK;
    }

    ibaudio_status_t run_tts(const std::string &text,
                             const CancellationToken *cancel,
                             uint64_t *processed_chars,
                             AudioData &out_audio) override {
        out_audio = run_reference_tts(text, cancel, processed_chars);
        return IBAUDIO_STATUS_OK;
    }

    ibaudio_status_t run_vad(const AudioData &mono_audio,
                             const VadConfig &config,
                             const CancellationToken *cancel,
                             uint64_t *processed_frames,
                             std::vector<ibaudio_vad_segment_v1> &out_segments) override {
        out_segments = run_energy_vad(mono_audio, config, cancel, processed_frames);
        return IBAUDIO_STATUS_OK;
    }
};

ReferenceProvider g_reference_provider;

struct ReferenceProviderRegistration {
    ReferenceProviderRegistration() {
        ProviderRegistry::instance().register_provider(&g_reference_provider);
    }
};
ReferenceProviderRegistration g_reference_provider_registration;

} // namespace
} // namespace ibaudio
