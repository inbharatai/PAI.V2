#include "internal.hpp"

#include <algorithm>
#include <cctype>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <iomanip>
#include <sstream>
#include <stdexcept>

namespace ibaudio {
namespace {

thread_local ibaudio_error_info_v1 g_last_error{};
std::atomic<uint64_t> g_process_errors{0u};

std::string json_escape(const std::string &value) {
    std::ostringstream output;
    for (char character : value) {
        const unsigned char byte = static_cast<unsigned char>(character);
        switch (byte) {
            case '\"': output << "\\\""; break;
            case '\\': output << "\\\\"; break;
            case '\b': output << "\\b"; break;
            case '\f': output << "\\f"; break;
            case '\n': output << "\\n"; break;
            case '\r': output << "\\r"; break;
            case '\t': output << "\\t"; break;
            default:
                if (byte < 0x20u) {
                    output << "\\u" << std::hex << std::setw(4) << std::setfill('0')
                           << static_cast<unsigned>(byte) << std::dec;
                } else {
                    output << static_cast<char>(byte);
                }
        }
    }
    return output.str();
}

ibaudio_backend_info_v1 make_backend(
    ibaudio_backend_t backend,
    const char *name,
    ibaudio_backend_availability_t availability,
    bool compiled,
    bool selected,
    const std::string &reason) {
    ibaudio_backend_info_v1 info{};
    info.struct_size = sizeof(info);
    info.api_version = IBAUDIO_API_VERSION;
    info.backend = backend;
    info.availability = availability;
    info.compiled = compiled ? 1u : 0u;
    info.selected = selected ? 1u : 0u;
    info.safe_cpu_fallback = backend == IBAUDIO_BACKEND_CPU ? 0u : 1u;
    copy_text(info.name, sizeof(info.name), name);
    copy_text(info.device, sizeof(info.device), backend == IBAUDIO_BACKEND_CPU ? "portable host CPU" : "none");
    copy_text(info.reason, sizeof(info.reason), reason);
    return info;
}

ModelRecord make_model(
    const char *id,
    const char *family,
    ibaudio_task_t task,
    ibaudio_streaming_class_t streaming,
    uint64_t capabilities,
    uint32_t sample_rate,
    bool available,
    const char *streaming_label,
    const char *reason) {
    ModelRecord record;
    auto &descriptor = record.descriptor;
    descriptor.struct_size = sizeof(descriptor);
    descriptor.api_version = IBAUDIO_API_VERSION;
    descriptor.task = task;
    descriptor.streaming_class = streaming;
    descriptor.capabilities = capabilities;
    descriptor.available = available ? 1u : 0u;
    descriptor.required_sample_rate = sample_rate;
    descriptor.required_channels = 1u;
    descriptor.artifact_size_bytes = 0u;
    copy_text(descriptor.id, sizeof(descriptor.id), id);
    copy_text(descriptor.family, sizeof(descriptor.family), family);
    copy_text(descriptor.version, sizeof(descriptor.version), "1.0.0");
    const std::string identity = std::string("inbharat-audio:") + id + ":1.0.0";
    copy_text(descriptor.artifact_sha256, sizeof(descriptor.artifact_sha256),
              sha256_hex(reinterpret_cast<const uint8_t *>(identity.data()), identity.size()));
    copy_text(descriptor.hash_kind, sizeof(descriptor.hash_kind), "builtin-identity-sha256");
    copy_text(descriptor.spdx_license, sizeof(descriptor.spdx_license), "Apache-2.0");
    copy_text(descriptor.source_uri, sizeof(descriptor.source_uri), std::string("builtin://") + id);
    copy_text(descriptor.source_revision, sizeof(descriptor.source_revision), IBAUDIO_RUNTIME_VERSION);
    copy_text(descriptor.streaming_label, sizeof(descriptor.streaming_label), streaming_label);
    copy_text(descriptor.availability_reason, sizeof(descriptor.availability_reason), reason);
    return record;
}

ibaudio_buffer *make_bytes_buffer(
    ibaudio_runtime *runtime,
    ibaudio_buffer_kind_t kind,
    const void *bytes,
    size_t size) {
    auto buffer = std::make_unique<ibaudio_buffer>();
    buffer->kind = kind;
    buffer->metrics = runtime == nullptr ? nullptr : &runtime->metrics;
    const auto *begin = static_cast<const uint8_t *>(bytes);
    if (size > 0u) {
        buffer->bytes.assign(begin, begin + size);
    }
    if (kind == IBAUDIO_BUFFER_UTF8) {
        buffer->bytes.push_back(0u);
    }
    if (buffer->metrics != nullptr) {
        buffer->metrics->live_owned_buffers.fetch_add(1u, std::memory_order_relaxed);
    }
    return buffer.release();
}

ibaudio_status_t check_runtime(const ibaudio_runtime *runtime, const char *function_name) {
    if (runtime == nullptr) {
        return set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                         function_name, "runtime handle is null");
    }
    return IBAUDIO_STATUS_OK;
}

} // namespace

uint64_t monotonic_ns() noexcept {
    return static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(
        std::chrono::steady_clock::now().time_since_epoch()).count());
}

void copy_text(char *destination, size_t capacity, const std::string &source) noexcept {
    if (destination == nullptr || capacity == 0u) {
        return;
    }
    const size_t count = std::min(capacity - 1u, source.size());
    if (count > 0u) {
        std::memcpy(destination, source.data(), count);
    }
    destination[count] = '\0';
}

bool valid_header(uint32_t size, uint32_t expected_min, uint32_t api_version) noexcept {
    return size >= expected_min && (api_version >> 16u) == IBAUDIO_API_VERSION_MAJOR;
}

std::string from_view(const ibaudio_string_view_v1 &view) {
    if (!valid_header(view.struct_size, sizeof(view), view.api_version)) {
        throw std::invalid_argument("invalid string-view header");
    }
    if (view.size > 0u && view.data == nullptr) {
        throw std::invalid_argument("string-view data pointer is null");
    }
    if (view.size > std::numeric_limits<size_t>::max()) {
        throw std::length_error("string view is too large");
    }
    return std::string(view.data == nullptr ? "" : view.data, static_cast<size_t>(view.size));
}

ibaudio_status_t set_error(
    ibaudio_status_t status,
    ibaudio_error_domain_t domain,
    const char *function_name,
    const std::string &message,
    bool recoverable,
    int64_t native_code) noexcept {
    if (status != IBAUDIO_STATUS_OK) {
        g_process_errors.fetch_add(1u, std::memory_order_relaxed);
    }
    g_last_error = {};
    g_last_error.struct_size = sizeof(g_last_error);
    g_last_error.api_version = IBAUDIO_API_VERSION;
    g_last_error.status = status;
    g_last_error.domain = domain;
    g_last_error.native_code = native_code;
    g_last_error.recoverable = recoverable ? 1u : 0u;
    copy_text(g_last_error.function_name, sizeof(g_last_error.function_name), function_name == nullptr ? "" : function_name);
    copy_text(g_last_error.message, sizeof(g_last_error.message), message);
    return status;
}

void clear_error_internal() noexcept {
    g_last_error = {};
    g_last_error.struct_size = sizeof(g_last_error);
    g_last_error.api_version = IBAUDIO_API_VERSION;
    g_last_error.status = IBAUDIO_STATUS_OK;
    g_last_error.domain = IBAUDIO_ERROR_DOMAIN_NONE;
}

bool is_terminal(ibaudio_job_state_t state) noexcept {
    return state == IBAUDIO_JOB_SUCCEEDED || state == IBAUDIO_JOB_CANCELLED || state == IBAUDIO_JOB_FAILED;
}

} // namespace ibaudio

extern "C" {

uint32_t ibaudio_get_api_version(void) {
    return IBAUDIO_API_VERSION;
}

const char *ibaudio_get_runtime_version(void) {
    return IBAUDIO_RUNTIME_VERSION;
}

const char *ibaudio_status_string(ibaudio_status_t status) {
    switch (status) {
        case IBAUDIO_STATUS_OK: return "OK";
        case IBAUDIO_STATUS_INVALID_ARGUMENT: return "INVALID_ARGUMENT";
        case IBAUDIO_STATUS_INVALID_STATE: return "INVALID_STATE";
        case IBAUDIO_STATUS_OUT_OF_MEMORY: return "OUT_OF_MEMORY";
        case IBAUDIO_STATUS_IO_ERROR: return "IO_ERROR";
        case IBAUDIO_STATUS_UNSUPPORTED: return "UNSUPPORTED";
        case IBAUDIO_STATUS_UNAVAILABLE: return "UNAVAILABLE";
        case IBAUDIO_STATUS_BUSY: return "BUSY";
        case IBAUDIO_STATUS_WOULD_BLOCK: return "WOULD_BLOCK";
        case IBAUDIO_STATUS_TIMEOUT: return "TIMEOUT";
        case IBAUDIO_STATUS_CANCELLED: return "CANCELLED";
        case IBAUDIO_STATUS_DEFERRED: return "DEFERRED";
        case IBAUDIO_STATUS_SECURITY_ERROR: return "SECURITY_ERROR";
        case IBAUDIO_STATUS_INTEGRITY_ERROR: return "INTEGRITY_ERROR";
        case IBAUDIO_STATUS_INTERNAL_ERROR: return "INTERNAL_ERROR";
        case IBAUDIO_STATUS_PERMISSION_DENIED: return "PERMISSION_DENIED";
        default: return "UNKNOWN_STATUS";
    }
}

ibaudio_status_t ibaudio_error_get_last(ibaudio_error_info_v1 *out_error) {
    if (out_error == nullptr) {
        return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                  __func__, "output error pointer is null");
    }
    if (ibaudio::g_last_error.struct_size == 0u) {
        ibaudio::clear_error_internal();
    }
    *out_error = ibaudio::g_last_error;
    return IBAUDIO_STATUS_OK;
}

void ibaudio_error_clear(void) {
    ibaudio::clear_error_internal();
}

void ibaudio_runtime_options_init(ibaudio_runtime_options_v1 *options) {
    if (options == nullptr) return;
    *options = {};
    options->struct_size = sizeof(*options);
    options->api_version = IBAUDIO_API_VERSION;
    options->cache_directory = {sizeof(ibaudio_string_view_v1), IBAUDIO_API_VERSION, nullptr, 0u};
    options->allowed_model_root = {sizeof(ibaudio_string_view_v1), IBAUDIO_API_VERSION, nullptr, 0u};
    options->cpu_threads = 1u;
    options->requested_backend = IBAUDIO_BACKEND_AUTO;
    options->allow_auto_cpu_fallback = 1u;
    options->strict_path_policy = 1u;
    options->deterministic_mode = 1u;
    options->max_cached_models = 2u;
    options->max_input_frames = ibaudio::kDefaultMaxInputFrames;
    options->allow_remote_providers = 0u;  // offline-first: no remote provider by default
}

void ibaudio_model_load_options_init(ibaudio_model_load_options_v1 *options) {
    if (options == nullptr) return;
    *options = {};
    options->struct_size = sizeof(*options);
    options->api_version = IBAUDIO_API_VERSION;
    options->model_id = {sizeof(ibaudio_string_view_v1), IBAUDIO_API_VERSION, nullptr, 0u};
    options->artifact_path = {sizeof(ibaudio_string_view_v1), IBAUDIO_API_VERSION, nullptr, 0u};
    options->expected_sha256 = {sizeof(ibaudio_string_view_v1), IBAUDIO_API_VERSION, nullptr, 0u};
    options->backend = IBAUDIO_BACKEND_AUTO;
    options->verify_hash = 1u;
}

void ibaudio_session_options_init(ibaudio_session_options_v1 *options) {
    if (options == nullptr) return;
    *options = {};
    options->struct_size = sizeof(*options);
    options->api_version = IBAUDIO_API_VERSION;
    options->task = IBAUDIO_TASK_ASR;
    options->streaming = 0u;
    options->vad_threshold_dbfs = -42.0f;
    options->vad_frame_ms = 20u;
    options->vad_hop_ms = 10u;
    options->vad_min_speech_ms = 60u;
    options->vad_min_silence_ms = 100u;
    options->barge_in_threshold_dbfs = -35.0f;
    options->barge_in_hold_ms = 120u;
}

void ibaudio_audio_process_options_init(ibaudio_audio_process_options_v1 *options) {
    if (options == nullptr) return;
    *options = {};
    options->struct_size = sizeof(*options);
    options->api_version = IBAUDIO_API_VERSION;
    options->gain_db = 0.0f;
    options->normalize_peak = 0.0f;
    options->clip_peak = 1.0f;
    options->sanitize_non_finite = 1u;
}

void ibaudio_stream_options_init(ibaudio_stream_options_v1 *options) {
    if (options == nullptr) return;
    *options = {};
    options->struct_size = sizeof(*options);
    options->api_version = IBAUDIO_API_VERSION;
    options->preferred_chunk_frames = 1600u;
    options->max_queued_events = 64u;
    options->emit_partial_results = 1u;
}

ibaudio_status_t ibaudio_runtime_create(
    const ibaudio_runtime_options_v1 *options,
    ibaudio_runtime_t **out_runtime) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (out_runtime == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "output runtime pointer is null");
        }
        *out_runtime = nullptr;
        ibaudio_runtime_options_v1 defaults{};
        ibaudio_runtime_options_init(&defaults);
        const ibaudio_runtime_options_v1 &value = options == nullptr ? defaults : *options;
        if (!ibaudio::valid_header(value.struct_size, sizeof(value), value.api_version)) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "invalid runtime-options header or ABI major");
        }
        if (value.cpu_threads == 0u || value.cpu_threads > 256u ||
            value.max_input_frames == 0u || value.max_input_frames > ibaudio::kAbsoluteMaxInputFrames ||
            value.requested_backend < IBAUDIO_BACKEND_AUTO || value.requested_backend > IBAUDIO_BACKEND_DIRECTML) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "invalid thread count, input-frame limit, or backend enum");
        }
        const std::string cache_text = ibaudio::from_view(value.cache_directory);
        const std::string root_text = ibaudio::from_view(value.allowed_model_root);
        auto runtime = std::make_unique<ibaudio_runtime>();
        runtime->cache_directory = cache_text.empty()
            ? std::filesystem::current_path() / ".ibaudio-cache"
            : std::filesystem::path(cache_text);
        runtime->allowed_model_root = root_text.empty() ? std::filesystem::path{} : std::filesystem::path(root_text);
        runtime->strict_path_policy = value.strict_path_policy != 0u;
        runtime->deterministic_mode = value.deterministic_mode != 0u;
        runtime->allow_remote_providers = value.allow_remote_providers != 0u;
        runtime->cpu_threads = value.cpu_threads;
        runtime->max_cached_models = std::min<uint32_t>(value.max_cached_models, 128u);
        runtime->max_input_frames = value.max_input_frames;
        runtime->requested_backend = value.requested_backend;
        std::filesystem::create_directories(runtime->cache_directory);
        if (!std::filesystem::is_directory(runtime->cache_directory)) {
            return ibaudio::set_error(IBAUDIO_STATUS_IO_ERROR, IBAUDIO_ERROR_DOMAIN_IO,
                                      __func__, "cache path is not a directory");
        }
        if (!runtime->allowed_model_root.empty() && !std::filesystem::is_directory(runtime->allowed_model_root)) {
            return ibaudio::set_error(IBAUDIO_STATUS_IO_ERROR, IBAUDIO_ERROR_DOMAIN_IO,
                                      __func__, "allowed model root is not a directory");
        }

        const bool cpu_selected = value.requested_backend == IBAUDIO_BACKEND_AUTO ||
                                  value.requested_backend == IBAUDIO_BACKEND_CPU;
        if (!cpu_selected && value.allow_auto_cpu_fallback == 0u) {
            return ibaudio::set_error(IBAUDIO_STATUS_UNAVAILABLE, IBAUDIO_ERROR_DOMAIN_BACKEND,
                                      __func__, "requested accelerator adapter is unavailable; explicit selection does not silently fall back");
        }
        runtime->selected_backend = IBAUDIO_BACKEND_CPU;
        runtime->fallback_used = !cpu_selected;
        if (runtime->fallback_used) {
            runtime->metrics.backend_fallbacks.store(1u, std::memory_order_relaxed);
            runtime->startup_diagnostic = "requested accelerator unavailable; policy permitted safe CPU recreation/fallback";
        } else {
            runtime->startup_diagnostic = "CPU reference backend selected";
        }
        runtime->backends.push_back(ibaudio::make_backend(IBAUDIO_BACKEND_CPU, "cpu",
            IBAUDIO_BACKEND_AVAILABLE, true, true, "portable CPU reference backend is compiled and available"));
        runtime->backends.push_back(ibaudio::make_backend(IBAUDIO_BACKEND_CUDA, "cuda",
            IBAUDIO_BACKEND_NOT_BUILT, false, false, "CUDA adapter not built in the first local release candidate"));
        runtime->backends.push_back(ibaudio::make_backend(IBAUDIO_BACKEND_HIP, "hip",
            IBAUDIO_BACKEND_NOT_BUILT, false, false, "HIP/ROCm adapter not built in the first local release candidate"));
        const ibaudio::AcceleratorProbe vulkan_probe = ibaudio::probe_vulkan_loader();
        auto vulkan_info = ibaudio::make_backend(IBAUDIO_BACKEND_VULKAN, "vulkan",
            vulkan_probe.availability, vulkan_probe.compiled, false, vulkan_probe.reason);
        ibaudio::copy_text(vulkan_info.device, sizeof(vulkan_info.device), vulkan_probe.device);
        runtime->backends.push_back(vulkan_info);
        runtime->backends.push_back(ibaudio::make_backend(IBAUDIO_BACKEND_METAL, "metal",
            IBAUDIO_BACKEND_NOT_BUILT, false, false, "Metal adapter not built"));
        runtime->backends.push_back(ibaudio::make_backend(IBAUDIO_BACKEND_NNAPI, "nnapi",
            IBAUDIO_BACKEND_NOT_BUILT, false, false, "NNAPI adapter not implemented"));
        runtime->backends.push_back(ibaudio::make_backend(IBAUDIO_BACKEND_COREML, "coreml",
            IBAUDIO_BACKEND_NOT_BUILT, false, false, "Core ML adapter not implemented"));
        runtime->backends.push_back(ibaudio::make_backend(IBAUDIO_BACKEND_DIRECTML, "directml",
            IBAUDIO_BACKEND_NOT_BUILT, false, false, "DirectML adapter not implemented"));

        runtime->models.push_back(ibaudio::make_model("reference-asr-v1", "reference-asr", IBAUDIO_TASK_ASR,
            IBAUDIO_STREAMING_WINDOW_INCREMENTAL_REVISABLE,
            IBAUDIO_CAP_OFFLINE | IBAUDIO_CAP_STREAM_INPUT | IBAUDIO_CAP_PARTIAL_OUTPUT |
                IBAUDIO_CAP_FINAL_OUTPUT | IBAUDIO_CAP_CANCELLATION | IBAUDIO_CAP_TIMESTAMPS |
                IBAUDIO_CAP_DETERMINISTIC_REFERENCE,
            16000u, true, "window-incremental provisional analysis; partials are revisable, final is authoritative", "available"));
        runtime->models.push_back(ibaudio::make_model("reference-tts-v1", "reference-tts", IBAUDIO_TASK_TTS,
            IBAUDIO_STREAMING_SEGMENT_CHUNKED,
            IBAUDIO_CAP_OFFLINE | IBAUDIO_CAP_PARTIAL_OUTPUT | IBAUDIO_CAP_FINAL_OUTPUT |
                IBAUDIO_CAP_CANCELLATION | IBAUDIO_CAP_DETERMINISTIC_REFERENCE | IBAUDIO_CAP_BARGE_IN,
            24000u, true, "segment-chunked deterministic PCM; generation is cooperative between characters and frames", "available"));
        runtime->models.push_back(ibaudio::make_model("energy-vad-v1", "energy-vad", IBAUDIO_TASK_VAD,
            IBAUDIO_STREAMING_STATEFUL_LOW_LATENCY,
            IBAUDIO_CAP_OFFLINE | IBAUDIO_CAP_STREAM_INPUT | IBAUDIO_CAP_PARTIAL_OUTPUT |
                IBAUDIO_CAP_FINAL_OUTPUT | IBAUDIO_CAP_CANCELLATION | IBAUDIO_CAP_TIMESTAMPS |
                IBAUDIO_CAP_BARGE_IN,
            16000u, true, "stateful frame-energy VAD; events emit after configured hysteresis", "available"));
#ifdef IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER
        // Real neural VAD via pinned audio.cpp's bundled Silero weights (no download).
        runtime->models.push_back(ibaudio::make_model("audiocpp-silero-vad-v1", "audiocpp-silero-vad", IBAUDIO_TASK_VAD,
            IBAUDIO_STREAMING_STATEFUL_LOW_LATENCY,
            IBAUDIO_CAP_OFFLINE | IBAUDIO_CAP_STREAM_INPUT | IBAUDIO_CAP_FINAL_OUTPUT |
                IBAUDIO_CAP_CANCELLATION | IBAUDIO_CAP_TIMESTAMPS,
            16000u, true, "audio.cpp Silero VAD neural model (bundled weights); offline and streaming", "available"));
        // Real ASR via audio.cpp Qwen3-ASR (licensed Apache-2.0, integrity-verified model
        // supplied by the caller). Requires the model root; UNAVAILABLE until provided.
        runtime->models.push_back(ibaudio::make_model("audiocpp-qwen3-asr-v1", "audiocpp-qwen3-asr", IBAUDIO_TASK_ASR,
            IBAUDIO_STREAMING_STATEFUL_LOW_LATENCY,
            IBAUDIO_CAP_OFFLINE | IBAUDIO_CAP_STREAM_INPUT | IBAUDIO_CAP_FINAL_OUTPUT |
                IBAUDIO_CAP_CANCELLATION,
            16000u, true, "audio.cpp Qwen3-ASR neural model (Apache-2.0, hash-verified); offline and streaming", "available"));
#endif
        runtime->models.push_back(ibaudio::make_model("kws-deferred-v1", "deferred-kws", IBAUDIO_TASK_KWS,
            IBAUDIO_STREAMING_DEFERRED,
            0u, 16000u, false, "deferred; no keyword model or inference adapter is included",
            "interface reserved, implementation intentionally deferred until licensed model and parity evidence exist"));
        runtime->metrics.runtimes_created.store(1u, std::memory_order_relaxed);
        *out_runtime = runtime.release();
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_runtime_release(ibaudio_runtime_t **runtime) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr || *runtime == nullptr) return IBAUDIO_STATUS_OK;
        if ((*runtime)->live_models.load(std::memory_order_acquire) != 0u ||
            (*runtime)->metrics.live_owned_buffers.load(std::memory_order_acquire) != 0u) {
            return ibaudio::set_error(IBAUDIO_STATUS_BUSY, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                      __func__, "runtime still owns live models or output buffers", true);
        }
        delete *runtime;
        *runtime = nullptr;
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_runtime_get_capabilities(
    const ibaudio_runtime_t *runtime,
    ibaudio_capabilities_v1 *out_capabilities) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (ibaudio::check_runtime(runtime, __func__) != IBAUDIO_STATUS_OK || out_capabilities == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "runtime or output pointer is null");
        }
        *out_capabilities = {};
        out_capabilities->struct_size = sizeof(*out_capabilities);
        out_capabilities->api_version = IBAUDIO_API_VERSION;
        out_capabilities->abi_major = IBAUDIO_API_VERSION_MAJOR;
        out_capabilities->abi_minor = IBAUDIO_API_VERSION_MINOR;
        out_capabilities->model_count = static_cast<uint32_t>(runtime->models.size());
        out_capabilities->backend_count = static_cast<uint32_t>(runtime->backends.size());
        out_capabilities->max_channels = ibaudio::kMaxChannels;
        out_capabilities->min_sample_rate = ibaudio::kMinSampleRate;
        out_capabilities->max_sample_rate = ibaudio::kMaxSampleRate;
        out_capabilities->max_input_frames = runtime->max_input_frames;
        out_capabilities->feature_flags = IBAUDIO_CAP_OFFLINE | IBAUDIO_CAP_STREAM_INPUT |
            IBAUDIO_CAP_PARTIAL_OUTPUT | IBAUDIO_CAP_FINAL_OUTPUT | IBAUDIO_CAP_CANCELLATION |
            IBAUDIO_CAP_TIMESTAMPS | IBAUDIO_CAP_DETERMINISTIC_REFERENCE | IBAUDIO_CAP_BARGE_IN;
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_runtime_get_backend_count(const ibaudio_runtime_t *runtime, uint32_t *out_count) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr || out_count == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "runtime or output pointer is null");
        }
        *out_count = static_cast<uint32_t>(runtime->backends.size());
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_runtime_get_backend_info(
    const ibaudio_runtime_t *runtime,
    uint32_t index,
    ibaudio_backend_info_v1 *out_info) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr || out_info == nullptr || index >= runtime->backends.size()) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "invalid runtime, index, or output pointer");
        }
        *out_info = runtime->backends[index];
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_runtime_get_model_count(const ibaudio_runtime_t *runtime, uint32_t *out_count) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr || out_count == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "runtime or output pointer is null");
        }
        *out_count = static_cast<uint32_t>(runtime->models.size());
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_runtime_get_model_descriptor(
    const ibaudio_runtime_t *runtime,
    uint32_t index,
    ibaudio_model_descriptor_v1 *out_descriptor) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr || out_descriptor == nullptr || index >= runtime->models.size()) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "invalid runtime, model index, or output pointer");
        }
        *out_descriptor = runtime->models[index].descriptor;
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_runtime_get_diagnostics_json(
    const ibaudio_runtime_t *runtime,
    ibaudio_buffer_t **out_json) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr || out_json == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "runtime or output pointer is null");
        }
        *out_json = nullptr;
        std::ostringstream json;
        json << "{\"schema\":\"inbharat.ibaudio.diagnostics.v1\",\"runtime_version\":\""
             << IBAUDIO_RUNTIME_VERSION << "\",\"api_version\":" << IBAUDIO_API_VERSION
             << ",\"cpu_threads\":" << runtime->cpu_threads
             << ",\"deterministic\":" << (runtime->deterministic_mode ? "true" : "false")
             << ",\"cache_directory\":\"" << ibaudio::json_escape(runtime->cache_directory.string())
             << "\",\"allowed_model_root\":\"" << ibaudio::json_escape(runtime->allowed_model_root.string())
             << "\",\"strict_path_policy\":" << (runtime->strict_path_policy ? "true" : "false")
             << ",\"selected_backend\":\"cpu\",\"fallback_used\":"
             << (runtime->fallback_used ? "true" : "false")
             << ",\"startup_diagnostic\":\"" << ibaudio::json_escape(runtime->startup_diagnostic)
             << "\",\"accelerators\":[";
        for (size_t index = 1; index < runtime->backends.size(); ++index) {
            const auto &backend = runtime->backends[index];
            if (index > 1u) json << ',';
            json << "{\"name\":\"" << ibaudio::json_escape(backend.name)
                 << "\",\"compiled\":" << (backend.compiled ? "true" : "false")
                 << ",\"availability\":" << backend.availability
                 << ",\"reason\":\"" << ibaudio::json_escape(backend.reason) << "\"}";
        }
        json << "],\"models\":[";
        for (size_t index = 0; index < runtime->models.size(); ++index) {
            const auto &model = runtime->models[index].descriptor;
            if (index > 0u) json << ',';
            json << "{\"id\":\"" << ibaudio::json_escape(model.id)
                 << "\",\"task\":" << model.task
                 << ",\"available\":" << (model.available ? "true" : "false")
                 << ",\"streaming_class\":" << model.streaming_class
                 << ",\"streaming_label\":\"" << ibaudio::json_escape(model.streaming_label)
                 << "\",\"sha256\":\"" << model.artifact_sha256
                 << "\",\"spdx_license\":\"" << model.spdx_license << "\"}";
        }
        json << "]}";
        const std::string value = json.str();
        *out_json = ibaudio::make_bytes_buffer(const_cast<ibaudio_runtime *>(runtime),
            IBAUDIO_BUFFER_UTF8, value.data(), value.size());
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_runtime_get_metrics(
    const ibaudio_runtime_t *runtime,
    ibaudio_metrics_v1 *out_metrics) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr || out_metrics == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "runtime or output pointer is null");
        }
        *out_metrics = {};
        out_metrics->struct_size = sizeof(*out_metrics);
        out_metrics->api_version = IBAUDIO_API_VERSION;
#define IBAUDIO_COPY_METRIC(name) out_metrics->name = runtime->metrics.name.load(std::memory_order_relaxed)
        IBAUDIO_COPY_METRIC(runtimes_created);
        IBAUDIO_COPY_METRIC(models_loaded);
        IBAUDIO_COPY_METRIC(model_cache_hits);
        IBAUDIO_COPY_METRIC(model_cache_misses);
        IBAUDIO_COPY_METRIC(sessions_created);
        IBAUDIO_COPY_METRIC(jobs_started);
        IBAUDIO_COPY_METRIC(jobs_cancelled);
        IBAUDIO_COPY_METRIC(streams_started);
        IBAUDIO_COPY_METRIC(audio_frames_in);
        IBAUDIO_COPY_METRIC(audio_frames_out);
        IBAUDIO_COPY_METRIC(calls_rejected_busy);
        IBAUDIO_COPY_METRIC(backend_fallbacks);
        out_metrics->errors_reported = ibaudio::g_process_errors.load(std::memory_order_relaxed);
        IBAUDIO_COPY_METRIC(live_owned_buffers);
#undef IBAUDIO_COPY_METRIC
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_runtime_reset_metrics(ibaudio_runtime_t *runtime) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "runtime handle is null");
        }
        const uint64_t live_buffers = runtime->metrics.live_owned_buffers.load(std::memory_order_relaxed);
#define IBAUDIO_ZERO_METRIC(name) runtime->metrics.name.store(0u, std::memory_order_relaxed)
        IBAUDIO_ZERO_METRIC(models_loaded);
        IBAUDIO_ZERO_METRIC(model_cache_hits);
        IBAUDIO_ZERO_METRIC(model_cache_misses);
        IBAUDIO_ZERO_METRIC(sessions_created);
        IBAUDIO_ZERO_METRIC(jobs_started);
        IBAUDIO_ZERO_METRIC(jobs_cancelled);
        IBAUDIO_ZERO_METRIC(streams_started);
        IBAUDIO_ZERO_METRIC(audio_frames_in);
        IBAUDIO_ZERO_METRIC(audio_frames_out);
        IBAUDIO_ZERO_METRIC(calls_rejected_busy);
        IBAUDIO_ZERO_METRIC(backend_fallbacks);
        IBAUDIO_ZERO_METRIC(errors_reported);
        ibaudio::g_process_errors.store(0u, std::memory_order_relaxed);
#undef IBAUDIO_ZERO_METRIC
        runtime->metrics.runtimes_created.store(1u, std::memory_order_relaxed);
        runtime->metrics.live_owned_buffers.store(live_buffers, std::memory_order_relaxed);
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_runtime_get_audio_cpp_status(
    const ibaudio_runtime_t *runtime,
    ibaudio_audio_cpp_status_v1 *out_status) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr || out_status == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "runtime and output status are required");
        }
        *out_status = {};
        out_status->struct_size = sizeof(*out_status);
        out_status->api_version = IBAUDIO_API_VERSION;
#ifdef IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER
        out_status->adapter_compiled = 1u;
        // The adapter compiles in the real audio.cpp Qwen3-ASR provider and the
        // Silero VAD provider; with the adapter on, production inference is ready
        // (the caller supplies a licensed, integrity-verified ASR model root).
        out_status->inference_ready = 1u;
#else
        out_status->adapter_compiled = 0u;
        out_status->inference_ready = 0u;
#endif
        ibaudio::copy_text(out_status->reviewed_commit, sizeof(out_status->reviewed_commit),
                           "26dcb5c4cf5aa016ae6285096a7b45f2671e5d17");
        ibaudio::copy_text(out_status->upstream_source, sizeof(out_status->upstream_source),
                           "https://github.com/0xShug0/audio.cpp");
#ifdef IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER
        ibaudio::copy_text(out_status->reason, sizeof(out_status->reason),
                           "audio.cpp adapter compiled; real Qwen3-ASR + Silero VAD providers registered");
#else
        ibaudio::copy_text(out_status->reason, sizeof(out_status->reason),
                           "audio.cpp adapter not compiled; no production audio.cpp inference is registered");
#endif
        return IBAUDIO_STATUS_OK;
    });
}

} /* extern \"C\" */
