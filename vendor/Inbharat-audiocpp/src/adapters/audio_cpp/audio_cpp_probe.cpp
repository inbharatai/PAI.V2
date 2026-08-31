#include "adapters/audio_cpp/audio_cpp_probe.hpp"

// Runtime asset probe for the pinned audio.cpp adapter. Self-contained on
// purpose: no upstream audio.cpp headers are included here, so the probe
// compiles (and the status API keeps its fail-closed semantics) even in
// build configurations where the provider sources are absent.

#include <cstddef>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <string>

// The model roots are compile-time macros in the library build (see
// CMakeLists.txt). Tests and non-adapter configurations compile this file
// directly; give the macros an empty fallback so an unconfigured root reads
// as "not configured" — a fail-closed fact, not a link error.
#ifndef IBAUDIO_AUDIO_CPP_QWEN3_ASR_ROOT
#define IBAUDIO_AUDIO_CPP_QWEN3_ASR_ROOT ""
#endif
#ifndef IBAUDIO_AUDIO_CPP_SILERO_VAD_ROOT
#define IBAUDIO_AUDIO_CPP_SILERO_VAD_ROOT ""
#endif

namespace ibaudio::audio_cpp_adapter {
namespace {

void set_reason(char *reason, std::size_t reason_size, const std::string &text) {
    if (reason == nullptr || reason_size == 0u) {
        return;
    }
    std::snprintf(reason, reason_size, "%s", text.c_str());
}

bool has_extension_case_insensitive(const std::string &name, const char *extension) {
    const std::size_t name_size = name.size();
    const std::size_t extension_size = std::strlen(extension);
    if (name_size < extension_size) {
        return false;
    }
    for (std::size_t index = 0; index < extension_size; ++index) {
        char left = name[name_size - extension_size + index];
        char right = extension[index];
        if (left >= 'A' && left <= 'Z') {
            left = static_cast<char>(left - 'A' + 'a');
        }
        if (right >= 'A' && right <= 'Z') {
            right = static_cast<char>(right - 'A' + 'a');
        }
        if (left != right) {
            return false;
        }
    }
    return true;
}

} // namespace

bool probe_model_root(const char *root,
                      const char *what,
                      const char *required_extension,
                      char *reason,
                      std::size_t reason_size) {
    const std::string label = (what != nullptr ? what : "model");
    if (root == nullptr || root[0] == '\0') {
        set_reason(reason, reason_size, label + " model root is not configured");
        return false;
    }

    std::error_code error;
    const std::filesystem::path path(root);
    if (!std::filesystem::is_directory(path, error)) {
        set_reason(reason, reason_size,
                   label + " model root is missing or not a directory: " + root);
        return false;
    }

    std::size_t candidates = 0u;
    std::filesystem::path first_candidate;
    for (const auto &entry : std::filesystem::directory_iterator(path, error)) {
        if (error) {
            break;
        }
        if (!entry.is_regular_file(error)) {
            continue;
        }
        if (required_extension != nullptr &&
            !has_extension_case_insensitive(entry.path().filename().string(), required_extension)) {
            continue;
        }
        if (candidates == 0u) {
            first_candidate = entry.path();
        }
        ++candidates;
    }
    if (error) {
        set_reason(reason, reason_size, label + " model root is not readable: " + root);
        return false;
    }
    if (candidates == 0u) {
        if (required_extension != nullptr) {
            set_reason(reason, reason_size,
                       label + " model root contains no " + required_extension +
                           " weights: " + root);
        } else {
            set_reason(reason, reason_size, label + " model root contains no weights: " + root);
        }
        return false;
    }

    // A readable directory listing does not prove the weights are readable.
    // Open the first candidate so a permission error fails the probe here,
    // not inside an inference call.
    {
        std::ifstream weights(first_candidate, std::ios::binary);
        if (!weights.good()) {
            set_reason(reason, reason_size,
                       label + " weights are not readable: " + first_candidate.string());
            return false;
        }
    }

    set_reason(reason, reason_size,
               label + " model root verified (" + std::to_string(candidates) +
                   " local weight file(s)) at " + root);
    return true;
}

bool probe_assets(char *reason, std::size_t reason_size) {
    // The Qwen3-ASR loader in the pinned audio.cpp auto-detects models by the
    // .gguf extension ONLY, so a root without a .gguf file can never serve
    // ASR regardless of what else it contains.
    if (!probe_model_root(IBAUDIO_AUDIO_CPP_QWEN3_ASR_ROOT, "audio.cpp ASR", ".gguf", reason,
                          reason_size)) {
        return false;
    }
    // The bundled Silero VAD weights ship inside the pristine pinned
    // checkout; their loader picks the file itself, so any regular weights
    // file in the root counts.
    char vad_reason[192];
    if (!probe_model_root(IBAUDIO_AUDIO_CPP_SILERO_VAD_ROOT, "audio.cpp VAD", nullptr,
                          vad_reason, sizeof(vad_reason))) {
        set_reason(reason, reason_size, std::string("audio.cpp adapter assets incomplete: ") +
                                          vad_reason);
        return false;
    }
    set_reason(reason, reason_size,
               "audio.cpp adapter assets verified locally (ASR and VAD weights present)");
    return true;
}

} // namespace ibaudio::audio_cpp_adapter