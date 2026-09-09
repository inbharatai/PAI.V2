// sherpa-onnx IndicConformer seam tests (req 6/15 of the speech audit).
//
// The native seam owns the `indicconformer-asr` family: Assamese routes to
// IndicConformer, never to the Qwen3-ASR route; missing IndicConformer
// assets fail closed (UNAVAILABLE, never a fabricated transcript); the
// local-native route survives an offline (remote-disallowed) policy while
// a remote provider is excluded from it.
//
// The provider and registry are internal hidden-visibility modules, so the
// test compiles their sources directly; the registry in this test binary
// therefore contains only the seam provider plus the stubs registered below
// (the shared library's providers are separate instances behind hidden
// symbols, covered by the public-ABI roundtrips in provider_tests.cpp).

#include "../src/provider.hpp"
#include "../src/providers/sherpa_onnx_internal.hpp"

#include <algorithm>
#include <cassert>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <vector>

namespace {

int g_checks = 0;

void check(bool condition, const char *what) {
    ++g_checks;
    if (!condition) {
        std::cerr << "FAIL: " << what << '\n';
        std::exit(1);
    }
}

bool contains(const char *haystack, const char *needle) {
    return std::strstr(haystack, needle) != nullptr;
}

// A stub of the Qwen3-ASR route with its TRUTHFUL coverage: Hindi and
// English only. Assamese is not in it and must never be.
class StubQwen3Provider final : public ibaudio::Provider {
public:
    const ibaudio::ProviderCapabilities &capabilities() const override {
        static const ibaudio::ProviderCapabilities caps = [] {
            ibaudio::ProviderCapabilities c;
            c.id = "stub-qwen3";
            c.version = "0.0.0";
            c.locality = "local-native";
            c.privacy_class = "ephemeral";
            c.remote = false;
            c.languages = {"en-IN", "hi-IN", "hi-en-codemix"};
            c.supports_asr = true;
            return c;
        }();
        return caps;
    }

    bool serves_family(const std::string &family) const override {
        return family == "qwen3-asr";
    }
};

// A remote stub: cloud STT that claims the scheduled Indian languages. The
// offline policy must exclude it entirely.
class StubRemoteProvider final : public ibaudio::Provider {
public:
    const ibaudio::ProviderCapabilities &capabilities() const override {
        static const ibaudio::ProviderCapabilities caps = [] {
            ibaudio::ProviderCapabilities c;
            c.id = "stub-remote";
            c.version = "0.0.0";
            c.locality = "remote";
            c.privacy_class = "audio-and-transcript";
            c.remote = true;
            c.languages = {"as-IN", "hi-IN"};
            c.supports_asr = true;
            return c;
        }();
        return caps;
    }

    bool serves_family(const std::string &family) const override {
        return family == "stub-remote-family";
    }
};

std::filesystem::path make_temp_root(const char *name) {
    std::filesystem::path root = std::filesystem::temp_directory_path() /
                                 ("ibaudio_sherpa_test_" + std::string(name));
    std::filesystem::remove_all(root);
    std::filesystem::create_directories(root);
    return root;
}

// Assamese reaches the IndicConformer seam and never the Qwen3 route; the
// Qwen3 route's truthful coverage excludes Assamese.
void test_assamese_routes_to_indicconformer_family_not_qwen3() {
    StubQwen3Provider qwen3;
    auto &registry = ibaudio::ProviderRegistry::instance();
    registry.register_provider(&qwen3);

    // The native seam owns the canonical family.
    ibaudio::Provider *native = registry.resolve_for_family("indicconformer-asr", false);
    check(native != nullptr, "indicconformer-asr must resolve to the native seam");
    check(native->capabilities().id == std::string("sherpa-onnx"),
          "indicconformer-asr must resolve to the sherpa-onnx seam");

    // The NeMo local-service seam claims its own family — two providers must
    // never race on one family.
    ibaudio::Provider *nemo = registry.resolve_for_family("indicconformer-asr-nemo", false);
    check(nemo == nullptr,
          "the NeMo local-service family must not be claimed by the native seam");

    // The Qwen3 route exists in the same registry but its coverage excludes
    // Assamese — routing Assamese must skip it and land on the seam.
    ibaudio::Provider *qwen3_route = registry.resolve_for_family("qwen3-asr", false);
    check(qwen3_route == &qwen3, "qwen3-asr must resolve to the qwen3 stub");
    const auto &qwen3_languages = qwen3_route->capabilities().languages;
    check(std::find(qwen3_languages.begin(), qwen3_languages.end(), "as-IN") ==
              qwen3_languages.end(),
          "the Qwen3 route must not claim Assamese");

    ibaudio::Provider *routed = registry.route(IBAUDIO_TASK_ASR, "as-IN", false, false);
    check(routed != nullptr, "routing Assamese ASR must find a provider");
    check(routed->capabilities().id == std::string("sherpa-onnx"),
          "Assamese must route to the IndicConformer seam, never Qwen3");
}

// The seam never fabricates inference: run_asr is UNAVAILABLE with an empty
// transcript, and the asset probe fails closed on unconfigured / missing /
// weight-less roots. Assets PRESENT is also not readiness — the native
// runtime is not wired in this seam.
void test_missing_indicconformer_assets_fail_closed() {
    char reason[192];

    // Unconfigured root.
    check(!ibaudio::sherpa_onnx::probe_indicconformer_root(nullptr, reason, sizeof(reason)),
          "null pack root must fail closed");
    check(contains(reason, "not configured"), "null root reason must say not configured");
    check(!ibaudio::sherpa_onnx::probe_indicconformer_root("", reason, sizeof(reason)),
          "empty pack root must fail closed");
    check(contains(reason, "not configured"), "empty root reason must say not configured");

    // The configured-root wrapper in this TU compiles with the empty macro
    // fallback, so it reads as "not configured" — the same fail-closed fact
    // the runtime status API gets in a library build without a pack root.
    check(!ibaudio::sherpa_onnx::indicconformer_assets_usable(reason, sizeof(reason)),
          "unconfigured pack root must fail closed end-to-end");
    check(contains(reason, "not configured"), "unconfigured reason must say so");

    // Missing directory.
    const std::filesystem::path missing = make_temp_root("missing");
    std::filesystem::remove_all(missing);
    check(!ibaudio::sherpa_onnx::probe_indicconformer_root(missing.string().c_str(), reason,
                                                          sizeof(reason)),
          "missing pack root must fail closed");
    check(contains(reason, "missing") || contains(reason, "not a directory"),
          "missing root reason must name the problem");

    // Directory without .onnx weights.
    const std::filesystem::path weightless = make_temp_root("weightless");
    {
        std::ofstream out(weightless / "readme.txt", std::ios::binary);
        out << "not weights";
    }
    check(!ibaudio::sherpa_onnx::probe_indicconformer_root(weightless.string().c_str(), reason,
                                                          sizeof(reason)),
          "pack root without .onnx weights must fail closed");
    check(contains(reason, "no .onnx"), "missing-weights reason must name the extension");

    // A real pack root verifies — and even a VERIFIED root is still not
    // inference readiness: run_asr below must return UNAVAILABLE anyway.
    const std::filesystem::path packed = make_temp_root("packed");
    {
        std::ofstream out(packed / "indicconformer.onnx", std::ios::binary);
        out << "onnx-weights";
    }
    check(ibaudio::sherpa_onnx::probe_indicconformer_root(packed.string().c_str(), reason,
                                                          sizeof(reason)),
          "pack root with .onnx weights must verify");
    check(contains(reason, "verified"), "passing reason must say verified");
    std::filesystem::remove_all(weightless);
    std::filesystem::remove_all(packed);

    // Inference fails closed with an empty transcript — never a fabricated
    // result, never a fake readiness.
    ibaudio::Provider *seam = ibaudio::ProviderRegistry::instance().find("sherpa-onnx");
    check(seam != nullptr, "the seam provider must be registered");
    ibaudio::AudioData audio;
    audio.samples = std::vector<float>(1600, 0.2f);
    audio.sample_rate = 16000;
    audio.channels = 1;
    ibaudio::CancellationToken cancel;
    uint64_t processed = 0;
    std::string transcript = "PRE-EXISTING GARBAGE THAT MUST NOT SURVIVE";
    check(seam->run_asr(audio, &cancel, &processed, transcript) == IBAUDIO_STATUS_UNAVAILABLE,
          "seam ASR must return UNAVAILABLE (native runtime not wired)");
    check(transcript.empty(), "seam ASR must never fabricate a transcript");
}

// The seam is local-native: an offline (remote-disallowed) policy keeps it
// routable while a remote provider is excluded — no silent cloud fallback.
void test_remote_providers_rejected_when_offline() {
    StubRemoteProvider remote;
    auto &registry = ibaudio::ProviderRegistry::instance();
    registry.register_provider(&remote);

    ibaudio::Provider *seam = registry.find("sherpa-onnx");
    check(seam != nullptr, "the seam provider must be registered");
    check(!seam->capabilities().remote, "the native seam must not be a remote provider");

    // The remote family is invisible under the offline policy…
    check(registry.resolve_for_family("stub-remote-family", false) == nullptr,
          "remote provider must be excluded when remote is disallowed");
    // …and Assamese still routes to the local-native seam, never the remote
    // stub, under the same policy.
    ibaudio::Provider *routed = registry.route(IBAUDIO_TASK_ASR, "as-IN", false, false);
    check(routed != nullptr, "Assamese must still have a local route offline");
    check(routed->capabilities().remote == false,
          "the offline Assamese route must be a local provider");
    check(routed->capabilities().id == std::string("sherpa-onnx"),
          "the offline Assamese route must be the native seam");
}

} // namespace

int main() {
    test_assamese_routes_to_indicconformer_family_not_qwen3();
    test_missing_indicconformer_assets_fail_closed();
    test_remote_providers_rejected_when_offline();
    std::cout << "PASS sherpa_provider (" << g_checks << " checks)\n";
    return 0;
}