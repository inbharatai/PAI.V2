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

// Minimal structurally-plausible GGUF: magic + version + tensor count +
// metadata kv count (24 bytes) plus a little padding, so the probe's
// magic/header checks exercise a file that could actually parse.
void write_gguf_file(const std::filesystem::path &root, const char *name) {
    std::ofstream out(root / name, std::ios::binary);
    out.write("GGUF", 4);
    const char zeros[28] = {};
    out.write(zeros, sizeof(zeros));
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
    write_gguf_file(root, "qwen3-asr.gguf");

    check(ibaudio::audio_cpp_adapter::probe_model_root(root.string().c_str(), "audio.cpp ASR",
                                                       ".gguf", reason, sizeof(reason)),
          "root with a real GGUF must pass");
    check(contains(reason, "verified"), "passing reason must say verified");

    // Extension detection is case-insensitive.
    std::filesystem::remove(root / "qwen3-asr.gguf");
    write_gguf_file(root, "QWEN3-ASR.GGUF");
    check(ibaudio::audio_cpp_adapter::probe_model_root(root.string().c_str(), "audio.cpp ASR",
                                                       ".gguf", reason, sizeof(reason)),
          "uppercase .GGUF must pass");

    // Any-weights probes (nullptr extension, no required name) accept any
    // non-empty regular file.
    check(ibaudio::audio_cpp_adapter::probe_model_root(root.string().c_str(), "audio.cpp VAD",
                                                        nullptr, reason, sizeof(reason)),
          "any-file probe must pass with a regular file present");

    std::filesystem::remove_all(root);
}

void test_probe_model_root_rejects_bad_or_ambiguous_weights() {
    char reason[192];

    // A zero-byte .gguf used to pass; no loader can mount one.
    {
        const std::filesystem::path root = make_temp_root("zero_gguf");
        // Scope the writer so the handle is closed before remove_all —
        // deleting a tree that still holds an open file fails on Windows and
        // remove_all throws.
        {
            std::ofstream out(root / "qwen3-asr.gguf", std::ios::binary);
            out.flush();
        }
        check(!ibaudio::audio_cpp_adapter::probe_model_root(root.string().c_str(), "audio.cpp ASR",
                                                            ".gguf", reason, sizeof(reason)),
              "zero-byte .gguf must fail");
        check(contains(reason, "not readable or empty"),
              "zero-byte reason must name the problem");

        std::filesystem::remove_all(root);
    }

    // A renamed non-GGUF file with the .gguf extension must fail the magic
    // check — a truncated or substituted download is not a usable model.
    {
        const std::filesystem::path root = make_temp_root("fake_gguf");
        write_file(root, "qwen3-asr.gguf");
        check(!ibaudio::audio_cpp_adapter::probe_model_root(root.string().c_str(), "audio.cpp ASR",
                                                            ".gguf", reason, sizeof(reason)),
              "non-GGUF bytes with .gguf extension must fail");
        check(contains(reason, "GGUF magic"),
              "magic failure reason must name the GGUF magic header");

        std::filesystem::remove_all(root);
    }

    // Several .gguf candidates is a packaging error: the loader cannot
    // disambiguate, so readiness fails closed.
    {
        const std::filesystem::path root = make_temp_root("ambiguous");
        write_gguf_file(root, "qwen3-asr.gguf");
        write_gguf_file(root, "other-asr.gguf");
        check(!ibaudio::audio_cpp_adapter::probe_model_root(root.string().c_str(), "audio.cpp ASR",
                                                            ".gguf", reason, sizeof(reason)),
              "two .gguf candidates must fail");
        check(contains(reason, "ambiguous"),
              "ambiguous reason must say one weight file is required");

        std::filesystem::remove_all(root);
    }

    // The VAD resolver requires exactly silero_vad_16k.safetensors: a root
    // with a substituted file is not ready even when it holds weights.
    {
        const std::filesystem::path root = make_temp_root("vad_wrong_name");
        write_file(root, "vad.safetensors");
        check(!ibaudio::audio_cpp_adapter::probe_model_root(root.string().c_str(), "audio.cpp VAD",
                                                            nullptr, reason, sizeof(reason),
                                                            "silero_vad_16k.safetensors"),
              "VAD root without silero_vad_16k.safetensors must fail");
        check(contains(reason, "silero_vad_16k.safetensors"),
              "VAD failure reason must name the required file");

        std::filesystem::remove(root / "vad.safetensors");
        write_file(root, "silero_vad_16k.safetensors");
        check(ibaudio::audio_cpp_adapter::probe_model_root(root.string().c_str(), "audio.cpp VAD",
                                                            nullptr, reason, sizeof(reason),
                                                            "silero_vad_16k.safetensors"),
              "VAD root with the exact silero file must pass");

        // Zero-byte VAD weights must fail even with the correct name.
        std::filesystem::remove(root / "silero_vad_16k.safetensors");
        {
            std::ofstream empty_out(root / "silero_vad_16k.safetensors", std::ios::binary);
            empty_out.flush();
        }
        check(!ibaudio::audio_cpp_adapter::probe_model_root(root.string().c_str(), "audio.cpp VAD",
                                                             nullptr, reason, sizeof(reason),
                                                             "silero_vad_16k.safetensors"),
              "zero-byte silero file must fail");

        std::filesystem::remove_all(root);
    }
}

// Content-hash gate on top of the probe (finding V6): the baked-root load
// path claimed SHA-256 verification without any hashing. The gate reuses the
// probe's candidate selection, so a digest check can only pass for the exact
// file the loader would mount.
void test_verify_model_root_sha256() {
    char reason[192];
    const std::filesystem::path root = make_temp_root("sha256_gate");
    write_gguf_file(root, "qwen3-asr.gguf");

    // Known fixture: "GGUF" + 28 zero bytes.
    const char *actual_digest = "d32718d69557e5fb1267744390d3fe5869b81db8dd9a8272ddf43ae50967eba6";

    // The exact digest passes.
    check(ibaudio::audio_cpp_adapter::verify_model_root_sha256(
              root.string().c_str(), "audio.cpp ASR", ".gguf", nullptr, actual_digest,
              reason, sizeof(reason)),
          "exact digest must pass the hash gate");
    check(contains(reason, "SHA-256 verified"), "passing reason must say verified");

    // The comparison is case-insensitive (hash tools print uppercase too).
    {
        std::string upper(actual_digest);
        for (char &c : upper) {
            c = (c >= 'a' && c <= 'f') ? static_cast<char>(c - 'a' + 'A') : c;
        }
        check(ibaudio::audio_cpp_adapter::verify_model_root_sha256(
                  root.string().c_str(), "audio.cpp ASR", ".gguf", nullptr, upper.c_str(),
                  reason, sizeof(reason)),
              "uppercase digest must pass the hash gate");
    }

    // A wrong digest fails closed and names the mismatch — a substituted
    // weights file must never pass as "ready".
    {
        std::string wrong(actual_digest);
        wrong[0] = wrong[0] == '0' ? '1' : '0';
        check(!ibaudio::audio_cpp_adapter::verify_model_root_sha256(
                  root.string().c_str(), "audio.cpp ASR", ".gguf", nullptr, wrong.c_str(),
                  reason, sizeof(reason)),
              "wrong digest must fail the hash gate");
        check(contains(reason, "mismatch"), "mismatch reason must name the problem");
    }

    // Malformed expected digests fail before any hashing.
    check(!ibaudio::audio_cpp_adapter::verify_model_root_sha256(
              root.string().c_str(), "audio.cpp ASR", ".gguf", nullptr, "not-a-digest",
              reason, sizeof(reason)),
          "non-hex digest must be rejected");
    check(contains(reason, "64 hex") || contains(reason, "non-hex"),
          "malformed-digest reason must say why");
    check(!ibaudio::audio_cpp_adapter::verify_model_root_sha256(
              root.string().c_str(), "audio.cpp ASR", ".gguf", nullptr, nullptr, reason,
              sizeof(reason)),
          "null digest must be rejected");
    check(!ibaudio::audio_cpp_adapter::verify_model_root_sha256(
              root.string().c_str(), "audio.cpp ASR", ".gguf", nullptr, "", reason,
              sizeof(reason)),
          "empty digest must be rejected");

    // The named-file variant (the VAD rule) hashes the exact silero file.
    {
        const std::filesystem::path vad_root = make_temp_root("sha256_gate_vad");
        write_file(vad_root, "silero_vad_16k.safetensors");
        // sha256("weights") — the exact content write_file produces.
        const char *digest = "9a129038d9a00aed0cf6a7ea059ca50a813449061ab87848cf1a13eafdf33b2c";
        check(ibaudio::audio_cpp_adapter::verify_model_root_sha256(
                  vad_root.string().c_str(), "audio.cpp VAD", nullptr,
                  "silero_vad_16k.safetensors", digest, reason, sizeof(reason)),
              "named-file digest must pass the hash gate");

        std::filesystem::remove_all(vad_root);
    }

    // A root that fails the probe fails the gate first, with the probe's reason.
    {
        const std::filesystem::path empty_root = make_temp_root("sha256_gate_empty");
        check(!ibaudio::audio_cpp_adapter::verify_model_root_sha256(
                  empty_root.string().c_str(), "audio.cpp ASR", ".gguf", nullptr,
                  actual_digest, reason, sizeof(reason)),
              "unready root must fail the hash gate");
        check(contains(reason, "no .gguf"),
              "unready root must keep the probe's reason");

        std::filesystem::remove_all(empty_root);
    }

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
    test_probe_model_root_rejects_bad_or_ambiguous_weights();
    test_verify_model_root_sha256();
    test_compiled_adapter_with_missing_models_is_not_ready();
    test_runtime_status_never_ready_without_adapter_or_assets();
    test_null_arguments_rejected();
    std::cout << "PASS audio_cpp_status (" << g_checks << " checks)\n";
    return 0;
}