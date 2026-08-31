// audio.cpp runtime status semantics tests.
//
// Req 12/15 of the speech audit: a compiled adapter whose local model assets
// are missing must report inference_ready=0 — readiness is derived from usable
// local assets at runtime, never from compile-time truth. The probe module is
// internal (hidden visibility), so this test compiles its source directly,
// exactly like the language/transport module tests.

#include "inbharat/ibaudio.h"
#include "../src/adapters/audio_cpp/audio_cpp_probe.hpp"

#include <cassert>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>

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

std::filesystem::path make_temp_root(const char *name) {
    std::filesystem::path root = std::filesystem::temp_directory_path() /
                                 ("ibaudio_probe_test_" + std::string(name));
    std::filesystem::remove_all(root);
    std::filesystem::create_directories(root);
    return root;
}

void write_file(const std::filesystem::path &root, const char *name) {
    std::ofstream out(root / name, std::ios::binary);
    out << "weights";
}

void test_probe_model_root_fails_closed() {
    char reason[192];

    // Unconfigured root.
    check(!ibaudio::audio_cpp_adapter::probe_model_root(nullptr, "audio.cpp ASR", ".gguf",
                                                        reason, sizeof(reason)),
          "null root must fail");
    check(contains(reason, "not configured"), "null root reason must say not configured");
    check(!ibaudio::audio_cpp_adapter::probe_model_root("", "audio.cpp ASR", ".gguf", reason,
                                                        sizeof(reason)),
          "empty root must fail");
    check(contains(reason, "not configured"), "empty root reason must say not configured");

    // Missing directory.
    const std::filesystem::path missing = make_temp_root("missing");
    std::filesystem::remove_all(missing);
    check(!ibaudio::audio_cpp_adapter::probe_model_root(missing.string().c_str(), "audio.cpp ASR",
                                                        ".gguf", reason, sizeof(reason)),
          "missing root must fail");
    check(contains(reason, "missing") || contains(reason, "not a directory"),
          "missing root reason must name the problem");

    // Directory without the loader's extension-only weights.
    const std::filesystem::path empty_root = make_temp_root("empty");
    write_file(empty_root, "readme.txt");
    check(!ibaudio::audio_cpp_adapter::probe_model_root(empty_root.string().c_str(),
                                                        "audio.cpp ASR", ".gguf", reason,
                                                        sizeof(reason)),
          "root without .gguf must fail");
    check(contains(reason, "no .gguf"), "missing-weights reason must name the extension");

    std::filesystem::remove_all(empty_root);
}

void test_probe_model_root_accepts_local_weights() {
    char reason[192];
    const std::filesystem::path root = make_temp_root("weights");
    write_file(root, "qwen3-asr.gguf");

    check(ibaudio::audio_cpp_adapter::probe_model_root(root.string().c_str(), "audio.cpp ASR",
                                                       ".gguf", reason, sizeof(reason)),
          "root with a .gguf must pass");
    check(contains(reason, "verified"), "passing reason must say verified");

    // Extension detection is case-insensitive.
    std::filesystem::remove(root / "qwen3-asr.gguf");
    write_file(root, "QWEN3-ASR.GGUF");
    check(ibaudio::audio_cpp_adapter::probe_model_root(root.string().c_str(), "audio.cpp ASR",
                                                       ".gguf", reason, sizeof(reason)),
          "uppercase .GGUF must pass");

    // Any-weights probes (nullptr extension) accept any regular file.
    check(ibaudio::audio_cpp_adapter::probe_model_root(root.string().c_str(), "audio.cpp VAD",
                                                        nullptr, reason, sizeof(reason)),
          "any-file probe must pass with a regular file present");

    std::filesystem::remove_all(root);
}

void test_compiled_adapter_with_missing_models_is_not_ready() {
    // In this test translation unit the model-root macros fall back to ""
    // (unconfigured), so probe_assets — the exact function the runtime status
    // API calls in adapter builds — must fail closed: a compiled adapter with
    // no usable local assets is NOT ready.
    char reason[192];
    const bool ready =
        ibaudio::audio_cpp_adapter::probe_assets(reason, sizeof(reason));
    check(!ready, "adapter with unconfigured model roots must not be inference-ready");
    check(contains(reason, "not configured"), "unconfigured reason must say so");
}

void test_runtime_status_never_ready_without_adapter_or_assets() {
    ibaudio_runtime_options_v1 options{};
    ibaudio_runtime_options_init(&options);
    ibaudio_runtime_t *runtime = nullptr;
    assert(ibaudio_runtime_create(&options, &runtime) == IBAUDIO_STATUS_OK);
    assert(runtime != nullptr);

    ibaudio_audio_cpp_status_v1 status{};
    const ibaudio_status_t got = ibaudio_runtime_get_audio_cpp_status(runtime, &status);
    check(got == IBAUDIO_STATUS_OK, "status call must succeed");
    check(status.struct_size == sizeof(status), "struct_size must round-trip");
    check(status.reviewed_commit[0] != '\0', "reviewed commit must be reported");
    check(status.reason[0] != '\0', "reason must never be empty");
#ifndef IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER
    // Without the adapter there is no audio.cpp inference at all.
    check(status.adapter_compiled == 0u, "non-adapter build must report adapter_compiled=0");
    check(status.inference_ready == 0u, "non-adapter build must report inference_ready=0");
#else
    // With the adapter, readiness must reflect the library build's real model
    // roots: READY claims a verified-assets reason; GATED claims the exact
    // failure. Compile-time truth (adapter_compiled=1) must never imply ready.
    if (status.inference_ready == 1u) {
        check(contains(status.reason, "verified"),
              "inference_ready=1 must be justified by a verified-assets reason");
        check(!contains(status.reason, "not configured") &&
                  !contains(status.reason, "missing") &&
                  !contains(status.reason, "no .gguf"),
              "READY must not carry a failure reason");
    } else {
        check(status.reason[0] != '\0', "GATED must name the exact failure");
    }
    check(!contains(status.reason, "DEFERRED"),
          "status must never report the deleted DEFERRED stub");
#endif
    // Both manifests must never disagree with the build: a non-compiled
    // adapter can never be ready.
    if (status.adapter_compiled == 0u) {
        check(status.inference_ready == 0u,
              "inference_ready=1 with adapter_compiled=0 is compile-time truth");
    }

    assert(ibaudio_runtime_release(&runtime) == IBAUDIO_STATUS_OK);
}

void test_null_arguments_rejected() {
    ibaudio_runtime_options_v1 options{};
    ibaudio_runtime_options_init(&options);
    ibaudio_runtime_t *runtime = nullptr;
    assert(ibaudio_runtime_create(&options, &runtime) == IBAUDIO_STATUS_OK);
    ibaudio_audio_cpp_status_v1 status{};
    check(ibaudio_runtime_get_audio_cpp_status(nullptr, &status) != IBAUDIO_STATUS_OK,
          "null runtime must be rejected");
    check(ibaudio_runtime_get_audio_cpp_status(runtime, nullptr) != IBAUDIO_STATUS_OK,
          "null status must be rejected");
    assert(ibaudio_runtime_release(&runtime) == IBAUDIO_STATUS_OK);
}

} // namespace

int main() {
    test_probe_model_root_fails_closed();
    test_probe_model_root_accepts_local_weights();
    test_compiled_adapter_with_missing_models_is_not_ready();
    test_runtime_status_never_ready_without_adapter_or_assets();
    test_null_arguments_rejected();
    std::cout << "PASS audio_cpp_status (" << g_checks << " checks)\n";
    return 0;
}