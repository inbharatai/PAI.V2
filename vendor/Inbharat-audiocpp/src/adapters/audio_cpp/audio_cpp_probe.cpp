#include "adapters/audio_cpp/audio_cpp_probe.hpp"

// Runtime asset probe for the pinned audio.cpp adapter. Self-contained on
// purpose: no upstream audio.cpp headers are included here, so the probe
// compiles (and the status API keeps its fail-closed semantics) even in
// build configurations where the provider sources are absent.

#include "../../internal.hpp"  // sha256_file_path for the content-hash gate

#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <exception>
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

bool names_equal_case_insensitive(const std::string &left, const std::string &right) {
    if (left.size() != right.size()) {
        return false;
    }
    for (std::size_t index = 0; index < left.size(); ++index) {
        char a = left[index];
        char b = right[index];
        if (a >= 'A' && a <= 'Z') {
            a = static_cast<char>(a - 'A' + 'a');
        }
        if (b >= 'A' && b <= 'Z') {
            b = static_cast<char>(b - 'A' + 'a');
        }
        if (a != b) {
            return false;
        }
    }
    return true;
}

// Minimum structurally-valid GGUF size: magic (4) + version (4) +
// tensor_count (8) + metadata_kv_count (8). Anything smaller can never be
// parsed by the pinned loader, so a truncated download fails the probe
// here instead of failing an inference call later.
constexpr std::uintmax_t kMinimumGgufBytes = 24u;

bool file_has_gguf_magic(const std::filesystem::path &path) {
    std::ifstream file(path, std::ios::binary);
    if (!file.good()) {
        return false;
    }
    char magic[4] = {0, 0, 0, 0};
    file.read(magic, sizeof(magic));
    return file.gcount() == static_cast<std::streamsize>(sizeof(magic)) &&
           std::memcmp(magic, "GGUF", sizeof(magic)) == 0;
}

// Shared candidate scan: fills `candidate`/`candidates` with the regular
// files in `root_dir` matching the extension/filename rules. Returns false
// when the directory cannot be listed. probe_model_root turns the result
// into readiness reasons; verify_model_root_sha256 uses it to find the exact
// file it hashes, so the readiness probe and the hash gate can never
// disagree about which weights file they are talking about.
bool scan_weights_candidates(const std::filesystem::path &root_dir,
                             const char *required_extension,
                             const char *required_file_name,
                             std::filesystem::path &candidate,
                             std::size_t &candidates) {
    candidates = 0u;
    candidate.clear();
    std::error_code error;
    for (const auto &entry : std::filesystem::directory_iterator(root_dir, error)) {
        if (error) {
            return false;
        }
        if (!entry.is_regular_file(error)) {
            continue;
        }
        const std::string name = entry.path().filename().string();
        if (required_file_name != nullptr &&
            !names_equal_case_insensitive(name, required_file_name)) {
            continue;
        }
        if (required_extension != nullptr &&
            !has_extension_case_insensitive(name, required_extension)) {
            continue;
        }
        candidate = entry.path();
        ++candidates;
    }
    return !error;
}

} // namespace

bool probe_model_root(const char *root,
                      const char *what,
                      const char *required_extension,
                      char *reason,
                      std::size_t reason_size,
                      const char *required_file_name) {
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
    std::filesystem::path candidate;
    if (!scan_weights_candidates(path, required_extension, required_file_name, candidate,
                                  candidates)) {
        set_reason(reason, reason_size, label + " model root is not readable: " + root);
        return false;
    }
    if (candidates == 0u) {
        if (required_file_name != nullptr) {
            set_reason(reason, reason_size,
                       label + " model root contains no " + required_file_name +
                           " weights: " + root);
        } else if (required_extension != nullptr) {
            set_reason(reason, reason_size,
                       label + " model root contains no " + required_extension +
                           " weights: " + root);
        } else {
            set_reason(reason, reason_size, label + " model root contains no weights: " + root);
        }
        return false;
    }
    if (candidates > 1u) {
        // The pinned loader auto-detects by extension (or exact filename) and
        // cannot disambiguate between matching weights files; a root holding
        // several candidates is a packaging error, so readiness fails closed
        // instead of hoping the loader picks the right one.
        set_reason(reason, reason_size,
                   label + " model root is ambiguous (" +
                       std::to_string(candidates) +
                       " matching weight files where exactly one is required): " + root);
        return false;
    }

    // A readable directory listing does not prove the weights are readable.
    // Open the candidate so a permission error fails the probe here, not
    // inside an inference call. A zero-byte file used to pass this check —
    // no loader can ever mount one, so reject it.
    std::error_code size_error;
    const std::uintmax_t file_size = std::filesystem::file_size(candidate, size_error);
    {
        std::ifstream weights(candidate, std::ios::binary);
        if (!weights.good() || size_error || file_size == 0u) {
            set_reason(reason, reason_size,
                       label + " weights are not readable or empty: " + candidate.string());
            return false;
        }
    }

    // The pinned Qwen3-ASR loader is extension-driven and parses GGUF, so
    // the single .gguf candidate must actually carry the GGUF magic header
    // and a structurally possible header size. A renamed or truncated file
    // of any other format fails here — before any process is started.
    if (required_extension != nullptr &&
        has_extension_case_insensitive(required_extension, ".gguf")) {
        if (file_size < kMinimumGgufBytes || !file_has_gguf_magic(candidate)) {
            set_reason(reason, reason_size,
                       label + " weights do not carry the GGUF magic header (file is "
                       "truncated or not GGUF): " +
                           candidate.string());
            return false;
        }
    }

    set_reason(reason, reason_size,
               label + " model root verified (1 local weight file) at " + root);
    return true;
}

bool verify_model_root_sha256(const char *root,
                              const char *what,
                              const char *required_extension,
                              const char *required_file_name,
                              const char *expected_sha256,
                              char *reason,
                              std::size_t reason_size) {
    const std::string label = (what != nullptr ? what : "model");

    // Reuse the full readiness probe first: hashing an unreadable or
    // structurally invalid file would be meaningless, and the probe's
    // reason already names the exact packaging problem. This also means
    // the hash gate can only ever pass for a file the loader would accept.
    if (!probe_model_root(root, what, required_extension, reason, reason_size,
                          required_file_name)) {
        return false;
    }

    const std::string expected = expected_sha256 != nullptr ? expected_sha256 : "";
    if (expected.size() != 64u) {
        set_reason(reason, reason_size,
                   label + " expected SHA-256 must be exactly 64 hex characters");
        return false;
    }
    for (const char c : expected) {
        const bool hex_digit = (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') ||
                               (c >= 'A' && c <= 'F');
        if (!hex_digit) {
            set_reason(reason, reason_size,
                       label + " expected SHA-256 contains a non-hex character");
            return false;
        }
    }

    std::filesystem::path candidate;
    std::size_t candidates = 0u;
    if (!scan_weights_candidates(std::filesystem::path(root), required_extension,
                                 required_file_name, candidate, candidates) ||
        candidates != 1u) {
        // The probe just verified exactly one readable candidate, so reaching
        // here means the root changed between the two passes; refuse to hash
        // a moving file.
        set_reason(reason, reason_size,
                   label + " model root changed while hashing; refusing");
        return false;
    }

    std::string actual;
    try {
        // sha256_file_path streams the file with a heap buffer; it throws on
        // an unreadable file, which becomes a fail-closed verdict here.
        actual = ibaudio::sha256_file_path(candidate);
    } catch (const std::exception &error) {
        set_reason(reason, reason_size,
                   label + " weights could not be hashed: " + error.what());
        return false;
    }
    if (!names_equal_case_insensitive(actual, expected)) {
        set_reason(reason, reason_size,
                   label + " weights SHA-256 mismatch (expected " + expected + ", got " +
                       actual + ")");
        return false;
    }
    set_reason(reason, reason_size, label + " weights SHA-256 verified");
    return true;
}

bool probe_assets(char *reason, std::size_t reason_size) {
    // The Qwen3-ASR loader in the pinned audio.cpp auto-detects models by the
    // .gguf extension ONLY, so a root without a .gguf file can never serve
    // ASR regardless of what else it contains. The probe additionally
    // rejects ambiguity (several .gguf candidates) and non-GGUF/truncated
    // files so readiness never overstates what the loader will accept.
    if (!probe_model_root(IBAUDIO_AUDIO_CPP_QWEN3_ASR_ROOT, "audio.cpp ASR", ".gguf", reason,
                          reason_size)) {
        return false;
    }
    // The pinned Silero VAD resolver looks for exactly this filename inside
    // the configured root; a root holding a renamed or substituted weights
    // file is not ready. Extra unrelated files in the root are tolerated
    // (they cannot be selected), but the named weights must exist, be
    // readable, and be non-empty.
    char vad_reason[192];
    if (!probe_model_root(IBAUDIO_AUDIO_CPP_SILERO_VAD_ROOT, "audio.cpp VAD", nullptr,
                          vad_reason, sizeof(vad_reason), "silero_vad_16k.safetensors")) {
        set_reason(reason, reason_size, std::string("audio.cpp adapter assets incomplete: ") +
                                          vad_reason);
        return false;
    }
    set_reason(reason, reason_size,
               "audio.cpp adapter assets verified locally (ASR and VAD weights present)");
    return true;
}

} // namespace ibaudio::audio_cpp_adapter