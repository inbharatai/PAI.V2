// Sarvam provider — remote STT/TTS (Saaras v3: 22 Indian languages + English,
// codemix modes, streaming; Bulbul v3: 11 languages, 35+ voices, streaming).
//
// Honest status: SPEC / BLOCKED-BY-ENVIRONMENT, and COMPILE-TIME GATED. This
// translation unit is only compiled when IBAUDIO_REMOTE_PROVIDERS=ON. The default
// build excludes it entirely, so an offline deployment (e.g. PAI) contains no remote
// code path at all. Even when compiled in, this adapter performs NO network I/O and
// fakes NO inference — every entry point returns UNAVAILABLE until a real, reviewed
// HTTP/WebSocket client with credential handling is wired and evidence exists.
//
// Remote providers are never selected by the capability router when the deployment
// disallows remote (ProviderRegistry::route checks caps.remote against remote_allowed).

#include "../provider.hpp"

#ifdef IBAUDIO_REMOTE_PROVIDERS

namespace ibaudio {
namespace {

class SarvamProvider final : public Provider {
public:
    const ProviderCapabilities &capabilities() const override {
        static const ProviderCapabilities caps = [] {
            ProviderCapabilities c;
            c.id = "sarvam";
            c.version = "0.0.0-spec";
            c.locality = "remote";
            c.privacy_class = "audio-and-transcript";  // audio leaves the device
            c.remote = true;
            c.languages = {"as-IN","bn-IN","gu-IN","hi-IN","kn-IN","ml-IN","mr-IN",
                           "od-IN","pa-IN","ta-IN","te-IN","en-IN"};
            c.supports_asr = true;   // Saaras v3
            c.supports_tts = true;   // Bulbul v3
            c.supports_vad = false;
            c.supports_kws = false;
            c.streaming_asr = true;  // Saaras v3 realtime (WebSocket)
            c.streaming_tts = true;  // Bulbul v3 streaming
            return c;
        }();
        return caps;
    }

    bool serves_family(const std::string &family) const override {
        return family == "sarvam-saaras-stt" || family == "sarvam-bulbul-tts";
    }

    ibaudio_status_t run_asr(const AudioData &, const CancellationToken *, uint64_t *, std::string &) override {
        return IBAUDIO_STATUS_UNAVAILABLE;  // no network client wired; no fake inference
    }
    ibaudio_status_t run_tts(const std::string &, const CancellationToken *, uint64_t *, AudioData &) override {
        return IBAUDIO_STATUS_UNAVAILABLE;
    }
};

SarvamProvider g_sarvam_provider;

struct SarvamRegistration {
    SarvamRegistration() {
        ProviderRegistry::instance().register_provider(&g_sarvam_provider);
    }
};
SarvamRegistration g_sarvam_registration;

} // namespace
} // namespace ibaudio

#endif // IBAUDIO_REMOTE_PROVIDERS
