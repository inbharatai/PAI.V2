// sherpa-onnx provider — the NATIVE seam for IndicConformer ASR.
//
// Honest status: SEAM. This provider exists so the capability router has a
// local-native route for the `indicconformer-asr` family (Assamese and the
// other scheduled IndicConformer languages) that is distinct from the
// Python-stack local-service seam (`indicconformer-asr-nemo`, the ai4bharat
// provider). It deliberately links NO sherpa-onnx native library in this
// cycle: `run_asr` returns UNAVAILABLE — no inference is faked, and no
// readiness is claimed from the provider merely being compiled in.
//
// What the seam does establish today:
// - truthful family + language routing (Assamese reaches the IndicConformer
//   family, never the Qwen3-ASR route),
// - a fail-closed local-asset probe for the IndicConformer ONNX pack root
//   (mirroring the audio.cpp readiness probe),
// - the registration point where a future native sherpa-onnx build plugs in
//   without changing any call site.
//
// Compiled only when IBAUDIO_ENABLE_SHERPA_ONNX=ON (default OFF); an offline
// deployment contains no remote code path either way.

#include "sherpa_onnx_internal.hpp"
#include "../provider.hpp"

#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <string>

// Local IndicConformer ONNX pack root (compile-time in the library build;
// empty fallback = unconfigured = fail closed).
#ifndef IBAUDIO_SHERPA_ONNX_INDICCONFORMER_ROOT
#define IBAUDIO_SHERPA_ONNX_INDICCONFORMER_ROOT ""
#endif

namespace ibaudio {
namespace sherpa_onnx {
namespace {

void set_reason(char *reason, std::size_t reason_size, const std::string &text) {
    if (reason == nullptr || reason_size == 0u) {
        return;
    }
    std::snprintf(reason, reason_size, "%s", text.c_str());
}

bool has_onnx_extension_case_insensitive(const std::string &name) {
    static constexpr const char *kSuffix = ".onnx";
    const std::size_t suffix_size = std::strlen(kSuffix);
    if (name.size() < suffix_size) {
        return false;
    }
    for (std::size_t index = 0; index < suffix_size; ++index) {
        char left = name[name.size() - suffix_size + index];
        char right = kSuffix[index];
        if (left >= 'A' && left <= 'Z') {
            left = static_cast<char>(left - 'A' + 'a');
        }
        if (left != right) {
            return false;
        }
    }
    return true;
}

} // namespace

bool probe_indicconformer_root(const char *root,
                               char *reason,
                               std::size_t reason_size) {
    constexpr const char *kLabel = "IndicConformer";
    if (root == nullptr || root[0] == '\0') {
        set_reason(reason, reason_size,
                   std::string(kLabel) + " ONNX pack root is not configured");
        return false;
    }
    std::error_code error;
    const std::filesystem::path path(root);
    if (!std::filesystem::is_directory(path, error)) {
        set_reason(reason, reason_size,
                   std::string(kLabel) + " ONNX pack root is missing or not a directory: " + root);
        return false;
    }
    std::filesystem::path first_candidate;
    std::size_t candidates = 0u;
    for (const auto &entry : std::filesystem::directory_iterator(path, error)) {
        if (error) {
            break;
        }
        std::error_code file_error;
        if (!entry.is_regular_file(file_error)) {
            continue;
        }
        if (!has_onnx_extension_case_insensitive(entry.path().filename().string())) {
            continue;
        }
        if (candidates == 0u) {
            first_candidate = entry.path();
        }
        ++candidates;
    }
    if (error) {
        set_reason(reason, reason_size,
                   std::string(kLabel) + " ONNX pack root is not readable: " + root);
        return false;
    }
    if (candidates == 0u) {
        set_reason(reason, reason_size,
                   std::string(kLabel) + " ONNX pack root contains no .onnx weights: " + root);
        return false;
    }
    // A readable listing does not prove readable weights; open the first
    // candidate so a permission error fails here, not inside inference.
    std::ifstream weights(first_candidate, std::ios::binary);
    if (!weights.good()) {
        set_reason(reason, reason_size,
                   std::string(kLabel) + " weights are not readable: " + first_candidate.string());
        return false;
    }
    set_reason(reason, reason_size,
               std::string(kLabel) + " ONNX pack root verified (" +
                   std::to_string(candidates) + " local .onnx file(s)) at " + root);
    return true;
}

bool indicconformer_assets_usable(char *reason, std::size_t reason_size) {
    return probe_indicconformer_root(IBAUDIO_SHERPA_ONNX_INDICCONFORMER_ROOT, reason, reason_size);
}

} // namespace ibaudio::sherpa_onnx

namespace {

class SherpaOnnxProvider final : public Provider {
public:
    const ProviderCapabilities &capabilities() const override {
        static const ProviderCapabilities caps = [] {
            ProviderCapabilities c;
            c.id = "sherpa-onnx";
            c.version = "0.0.0-seam";
            c.locality = "local-native";
            c.privacy_class = "no-persistence";
            c.remote = false;  // native local inference; never a cloud route
            // Same evidence-backed IndicConformer coverage set the
            // ai4bharat local-service seam declares: Assamese and the
            // verified IndicConformer subset + English. Claims stay claims
            // until a native build exists — run_asr fails closed below.
            c.languages = {"as-IN", "bn-IN", "gu-IN", "hi-IN", "kn-IN", "ml-IN",
                           "mr-IN", "od-IN", "pa-IN", "ta-IN", "te-IN", "en-IN"};
            c.supports_asr = true;   // IndicConformer (seam)
            c.supports_tts = false;
            c.supports_vad = false;
            c.supports_kws = false;
            c.streaming_asr = false;  // not established for the native seam
            c.streaming_tts = false;
            return c;
        }();
        return caps;
    }

    bool serves_family(const std::string &family) const override {
        // The NATIVE route owns the canonical family name. The ai4bharat
        // provider claims `indicconformer-asr-nemo` instead — two providers
        // must never race on one family (cross-TU static registration order
        // is unspecified).
        return family == "indicconformer-asr";
    }

    ibaudio_status_t run_asr(const AudioData &,
                             const CancellationToken *,
                             uint64_t *,
                             std::string &out_text) override {
        // Fail closed: the native sherpa-onnx runtime is not linked in this
        // seam. Assets being present is NOT readiness — produce nothing, and
        // never a fabricated transcript (out_text stays empty).
        out_text.clear();
        char reason[192];
        if (!ibaudio::sherpa_onnx::indicconformer_assets_usable(reason, sizeof(reason))) {
            return IBAUDIO_STATUS_UNAVAILABLE;
        }
        return IBAUDIO_STATUS_UNAVAILABLE;  // assets verified, runtime not wired yet
    }
};

SherpaOnnxProvider g_sherpa_onnx_provider;

struct SherpaOnnxRegistration {
    SherpaOnnxRegistration() {
        ProviderRegistry::instance().register_provider(&g_sherpa_onnx_provider);
    }
};
SherpaOnnxRegistration g_sherpa_onnx_registration;

} // namespace
} // namespace ibaudio