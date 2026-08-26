// AI4Bharat provider — boundary for IndicConformer ASR (22 scheduled languages) and
// IndicF5 TTS (11 languages incl. Assamese, Hindi).
//
// Honest status: SPEC / BLOCKED-BY-ENVIRONMENT. Both stacks are Python
// (NeMo/.nemo for IndicConformer; Hugging Face + trust_remote_code for IndicF5),
// which conflicts with the no-Python-in-core objective. This provider therefore
// models the *local-service* boundary only: a controlled local service would run the
// Python model and answer over a local socket, and this adapter is the seam for it.
// There is NO Python in libibaudio, and no inference is faked — every entry point
// returns UNAVAILABLE until a real local service is wired and evidence exists.

#include "../provider.hpp"

namespace ibaudio {
namespace {

class Ai4BharatProvider final : public Provider {
public:
    const ProviderCapabilities &capabilities() const override {
        static const ProviderCapabilities caps = [] {
            ProviderCapabilities c;
            c.id = "ai4bharat";
            c.version = "0.0.0-spec";
            c.locality = "local-service";
            c.privacy_class = "ephemeral";
            c.remote = false;
            // Evidence-backed language coverage per upstream project pages:
            // IndicConformer ASR: 22 scheduled languages. IndicF5 TTS: 11.
            // These are the upstream claims; they become InBharat claims only after
            // local-service parity evidence exists.
            c.languages = {"as-IN","bn-IN","gu-IN","hi-IN","kn-IN","ml-IN","mr-IN",
                           "od-IN","pa-IN","ta-IN","te-IN","en-IN"};
            c.supports_asr = true;   // IndicConformer
            c.supports_tts = true;   // IndicF5
            c.supports_vad = false;
            c.supports_kws = false;
            c.streaming_asr = false; // not established for the local-service path
            c.streaming_tts = false;
            return c;
        }();
        return caps;
    }

    bool serves_family(const std::string &family) const override {
        return family == "indicconformer-asr" || family == "indicf5-tts";
    }

    ibaudio_status_t run_asr(const AudioData &, const CancellationToken *, uint64_t *, std::string &) override {
        return IBAUDIO_STATUS_UNAVAILABLE;  // local service not wired; no fake inference
    }
    ibaudio_status_t run_tts(const std::string &, const CancellationToken *, uint64_t *, AudioData &) override {
        return IBAUDIO_STATUS_UNAVAILABLE;
    }
};

Ai4BharatProvider g_ai4bharat_provider;

struct Ai4BharatRegistration {
    Ai4BharatRegistration() {
        ProviderRegistry::instance().register_provider(&g_ai4bharat_provider);
    }
};
Ai4BharatRegistration g_ai4bharat_registration;

} // namespace
} // namespace ibaudio
