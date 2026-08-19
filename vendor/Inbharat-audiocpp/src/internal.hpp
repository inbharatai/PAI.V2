#pragma once

#include "inbharat/ibaudio.h"

#include <array>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <deque>
#include <filesystem>
#include <memory>
#include <mutex>
#include <string>
#include <stdexcept>
#include <thread>
#include <unordered_set>
#include <vector>

namespace ibaudio {

constexpr uint64_t kDefaultMaxInputFrames = 16000ull * 60ull * 60ull;
constexpr uint64_t kAbsoluteMaxInputFrames = 192000ull * 60ull * 60ull * 8ull;
constexpr uint64_t kMaxOwnedBytes = 512ull * 1024ull * 1024ull;
constexpr uint32_t kMaxChannels = 32u;
constexpr uint32_t kMinSampleRate = 1000u;
constexpr uint32_t kMaxSampleRate = 384000u;

uint64_t monotonic_ns() noexcept;
void copy_text(char *destination, size_t capacity, const std::string &source) noexcept;
std::string from_view(const ibaudio_string_view_v1 &view);
bool valid_header(uint32_t size, uint32_t expected_min, uint32_t api_version) noexcept;

ibaudio_status_t set_error(
    ibaudio_status_t status,
    ibaudio_error_domain_t domain,
    const char *function_name,
    const std::string &message,
    bool recoverable = false,
    int64_t native_code = 0) noexcept;
void clear_error_internal() noexcept;

template <typename Fn>
ibaudio_status_t guarded(const char *function_name, Fn &&fn) noexcept {
    try {
        const ibaudio_status_t status = fn();
        if (status == IBAUDIO_STATUS_OK) {
            clear_error_internal();
        }
        return status;
    } catch (const std::bad_alloc &) {
        return set_error(IBAUDIO_STATUS_OUT_OF_MEMORY, IBAUDIO_ERROR_DOMAIN_INTERNAL,
                         function_name, "allocation failed", true);
    } catch (const std::filesystem::filesystem_error &error) {
        return set_error(IBAUDIO_STATUS_IO_ERROR, IBAUDIO_ERROR_DOMAIN_IO,
                         function_name, error.what(), true, error.code().value());
    } catch (const std::invalid_argument &error) {
        return set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                         function_name, error.what(), true);
    } catch (const std::length_error &error) {
        return set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                         function_name, error.what(), true);
    } catch (const std::exception &error) {
        return set_error(IBAUDIO_STATUS_INTERNAL_ERROR, IBAUDIO_ERROR_DOMAIN_INTERNAL,
                         function_name, error.what(), false);
    } catch (...) {
        return set_error(IBAUDIO_STATUS_INTERNAL_ERROR, IBAUDIO_ERROR_DOMAIN_INTERNAL,
                         function_name, "unknown native exception", false);
    }
}

struct MetricsData {
    std::atomic<uint64_t> runtimes_created{0};
    std::atomic<uint64_t> models_loaded{0};
    std::atomic<uint64_t> model_cache_hits{0};
    std::atomic<uint64_t> model_cache_misses{0};
    std::atomic<uint64_t> sessions_created{0};
    std::atomic<uint64_t> jobs_started{0};
    std::atomic<uint64_t> jobs_cancelled{0};
    std::atomic<uint64_t> streams_started{0};
    std::atomic<uint64_t> audio_frames_in{0};
    std::atomic<uint64_t> audio_frames_out{0};
    std::atomic<uint64_t> calls_rejected_busy{0};
    std::atomic<uint64_t> backend_fallbacks{0};
    std::atomic<uint64_t> errors_reported{0};
    std::atomic<uint64_t> live_owned_buffers{0};
};

struct AcceleratorProbe {
    ibaudio_backend_availability_t availability = IBAUDIO_BACKEND_NOT_BUILT;
    bool compiled = false;
    std::string device = "none";
    std::string reason;
};

AcceleratorProbe probe_vulkan_loader();

struct ModelRecord {
    ibaudio_model_descriptor_v1 descriptor{};
};

struct AudioData {
    std::vector<float> samples;
    uint32_t sample_rate = 0;
    uint32_t channels = 0;
    ibaudio_audio_info_v1 info{};
};

struct VadConfig {
    float threshold_dbfs = -42.0f;
    uint32_t frame_ms = 20;
    uint32_t hop_ms = 10;
    uint32_t min_speech_ms = 60;
    uint32_t min_silence_ms = 100;
};

struct CancellationToken {
    std::atomic<bool> requested{false};
};

AudioData process_audio(
    const ibaudio_audio_view_v1 &input,
    const ibaudio_audio_process_options_v1 &options,
    uint64_t max_input_frames,
    const CancellationToken *cancel = nullptr);
std::vector<ibaudio_vad_segment_v1> run_energy_vad(
    const AudioData &mono_audio,
    const VadConfig &config,
    const CancellationToken *cancel = nullptr,
    uint64_t *processed_frames = nullptr);
std::string run_reference_asr(
    const AudioData &mono_audio,
    const CancellationToken *cancel = nullptr,
    uint64_t *processed_frames = nullptr);
AudioData run_reference_tts(
    const std::string &text,
    const CancellationToken *cancel = nullptr,
    uint64_t *processed_chars = nullptr);
std::vector<uint8_t> wav_encode_pcm16(const ibaudio_audio_view_v1 &audio, uint64_t max_input_frames);
AudioData wav_decode(const void *bytes, uint64_t size, uint64_t max_input_frames);
std::array<uint8_t, 32> sha256_bytes(const uint8_t *data, size_t size);
std::string sha256_hex(const uint8_t *data, size_t size);
std::string sha256_file_path(const std::filesystem::path &path);

bool is_terminal(ibaudio_job_state_t state) noexcept;

} // namespace ibaudio

struct ibaudio_buffer {
    ibaudio_buffer_kind_t kind = IBAUDIO_BUFFER_BYTES;
    std::vector<uint8_t> bytes;
    ibaudio::AudioData audio;
    std::vector<ibaudio_vad_segment_v1> segments;
    ibaudio::MetricsData *metrics = nullptr;
};

struct ibaudio_runtime {
    std::filesystem::path cache_directory;
    std::filesystem::path allowed_model_root;
    bool strict_path_policy = true;
    bool deterministic_mode = true;
    uint32_t cpu_threads = 1;
    uint32_t max_cached_models = 2;
    uint64_t max_input_frames = ibaudio::kDefaultMaxInputFrames;
    ibaudio_backend_t selected_backend = IBAUDIO_BACKEND_CPU;
    ibaudio_backend_t requested_backend = IBAUDIO_BACKEND_AUTO;
    bool fallback_used = false;
    std::vector<ibaudio_backend_info_v1> backends;
    std::vector<ibaudio::ModelRecord> models;
    ibaudio::MetricsData metrics;
    mutable std::mutex cache_mutex;
    std::deque<std::string> cache_lru;
    std::unordered_set<std::string> cache_keys;
    std::atomic<uint32_t> live_models{0};
    std::string startup_diagnostic;
};

struct ibaudio_model {
    ibaudio_runtime *runtime = nullptr;
    ibaudio::ModelRecord record;
    std::string artifact_path;
    std::string verified_hash;
    ibaudio_backend_t backend = IBAUDIO_BACKEND_CPU;
    std::atomic<uint32_t> live_sessions{0};
};

struct ibaudio_job;

struct ibaudio_session {
    ibaudio_model *model = nullptr;
    ibaudio_task_t task = IBAUDIO_TASK_ASR;
    bool streaming_enabled = false;
    ibaudio::VadConfig vad;
    float barge_threshold_dbfs = -35.0f;
    uint32_t barge_hold_ms = 120;
    std::atomic<bool> busy{false};
    std::atomic<uint32_t> live_jobs{0};
    std::atomic<uint32_t> live_streams{0};
    mutable std::mutex active_job_mutex;
    ibaudio_job *active_job = nullptr;
    mutable std::mutex barge_mutex;
    ibaudio_barge_in_state_t barge_state = IBAUDIO_BARGE_IN_IDLE;
    uint32_t barge_accumulated_ms = 0;
};

struct ibaudio_job {
    ibaudio_session *session = nullptr;
    ibaudio::CancellationToken cancellation;
    mutable std::mutex mutex;
    std::condition_variable cv;
    std::thread worker;
    ibaudio_job_state_t state = IBAUDIO_JOB_QUEUED;
    ibaudio_status_t result_status = IBAUDIO_STATUS_WOULD_BLOCK;
    uint64_t started_ns = 0;
    uint64_t finished_ns = 0;
    uint64_t processed_units = 0;
    ibaudio_buffer *result = nullptr;
    bool result_taken = false;
};

struct ibaudio_stream {
    ibaudio_session *session = nullptr;
    ibaudio_stream_options_v1 options{};
    mutable std::mutex mutex;
    std::condition_variable cv;
    std::deque<ibaudio_stream_event_v1> events;
    bool finished = false;
    bool cancelled = false;
    bool terminal_polled = false;
    bool session_busy_held = true;
    uint64_t sequence = 0;
    uint64_t coalesced_audio_events = 0;
    uint64_t expected_start_frame = 0;
    bool has_expected_start = false;
    uint32_t source_sample_rate = 0;
    uint32_t source_channels = 0;
    std::vector<float> source_mono;
    std::vector<float> canonical_mono;
    uint64_t next_resample_output = 0;
    uint64_t next_asr_partial_frame = 3200;
    uint64_t next_vad_hop_frame = 0;
    bool vad_active = false;
    uint64_t vad_candidate_start = 0;
    uint32_t vad_speech_run_ms = 0;
    uint32_t vad_silence_run_ms = 0;
    float vad_max_confidence = 0.0f;
    float vad_peak_dbfs = -120.0f;
};
