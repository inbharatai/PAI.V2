#ifndef INBHARAT_IBAUDIO_H
#define INBHARAT_IBAUDIO_H

/*
 * InBharat Audio public C99 ABI.
 *
 * Inputs are borrowed for the duration of the call unless a function explicitly
 * says otherwise. Every output ibaudio_buffer_t is immutable and owned by the
 * caller until ibaudio_buffer_release(). No C++ type or exception crosses this
 * boundary.
 */

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32) && defined(IBAUDIO_SHARED)
#  if defined(IBAUDIO_BUILDING_LIBRARY)
#    define IBAUDIO_API __declspec(dllexport)
#  else
#    define IBAUDIO_API __declspec(dllimport)
#  endif
#elif defined(__GNUC__) || defined(__clang__)
#  define IBAUDIO_API __attribute__((visibility("default")))
#else
#  define IBAUDIO_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

#define IBAUDIO_API_VERSION_MAJOR 1u
#define IBAUDIO_API_VERSION_MINOR 0u
#define IBAUDIO_API_VERSION ((IBAUDIO_API_VERSION_MAJOR << 16u) | IBAUDIO_API_VERSION_MINOR)
#define IBAUDIO_RUNTIME_VERSION "0.2.0-dev.1"
#define IBAUDIO_SHA256_HEX_LENGTH 64u

typedef struct ibaudio_runtime ibaudio_runtime_t;
typedef struct ibaudio_model ibaudio_model_t;
typedef struct ibaudio_session ibaudio_session_t;
typedef struct ibaudio_buffer ibaudio_buffer_t;
typedef struct ibaudio_job ibaudio_job_t;
typedef struct ibaudio_stream ibaudio_stream_t;

typedef int32_t ibaudio_status_t;
enum {
    IBAUDIO_STATUS_OK = 0,
    IBAUDIO_STATUS_INVALID_ARGUMENT = 1,
    IBAUDIO_STATUS_INVALID_STATE = 2,
    IBAUDIO_STATUS_OUT_OF_MEMORY = 3,
    IBAUDIO_STATUS_IO_ERROR = 4,
    IBAUDIO_STATUS_UNSUPPORTED = 5,
    IBAUDIO_STATUS_UNAVAILABLE = 6,
    IBAUDIO_STATUS_BUSY = 7,
    IBAUDIO_STATUS_WOULD_BLOCK = 8,
    IBAUDIO_STATUS_TIMEOUT = 9,
    IBAUDIO_STATUS_CANCELLED = 10,
    IBAUDIO_STATUS_DEFERRED = 11,
    IBAUDIO_STATUS_SECURITY_ERROR = 12,
    IBAUDIO_STATUS_INTEGRITY_ERROR = 13,
    IBAUDIO_STATUS_INTERNAL_ERROR = 14
};

typedef int32_t ibaudio_error_domain_t;
enum {
    IBAUDIO_ERROR_DOMAIN_NONE = 0,
    IBAUDIO_ERROR_DOMAIN_ARGUMENT = 1,
    IBAUDIO_ERROR_DOMAIN_LIFECYCLE = 2,
    IBAUDIO_ERROR_DOMAIN_IO = 3,
    IBAUDIO_ERROR_DOMAIN_MODEL = 4,
    IBAUDIO_ERROR_DOMAIN_BACKEND = 5,
    IBAUDIO_ERROR_DOMAIN_AUDIO = 6,
    IBAUDIO_ERROR_DOMAIN_SECURITY = 7,
    IBAUDIO_ERROR_DOMAIN_INTERNAL = 8
};

typedef int32_t ibaudio_task_t;
enum {
    IBAUDIO_TASK_ASR = 1,
    IBAUDIO_TASK_TTS = 2,
    IBAUDIO_TASK_VAD = 3,
    IBAUDIO_TASK_KWS = 4
};

typedef int32_t ibaudio_backend_t;
enum {
    IBAUDIO_BACKEND_AUTO = 0,
    IBAUDIO_BACKEND_CPU = 1,
    IBAUDIO_BACKEND_CUDA = 2,
    IBAUDIO_BACKEND_HIP = 3,
    IBAUDIO_BACKEND_VULKAN = 4,
    IBAUDIO_BACKEND_METAL = 5,
    IBAUDIO_BACKEND_NNAPI = 6,
    IBAUDIO_BACKEND_COREML = 7,
    IBAUDIO_BACKEND_DIRECTML = 8
};

typedef int32_t ibaudio_backend_availability_t;
enum {
    IBAUDIO_BACKEND_AVAILABLE = 1,
    IBAUDIO_BACKEND_NOT_BUILT = 2,
    IBAUDIO_BACKEND_NO_DEVICE = 3,
    IBAUDIO_BACKEND_ADAPTER_UNAVAILABLE = 4,
    IBAUDIO_BACKEND_PROBE_FAILED = 5
};

typedef int32_t ibaudio_streaming_class_t;
enum {
    IBAUDIO_STREAMING_OFFLINE_ONLY = 0,
    IBAUDIO_STREAMING_BUFFERED_FINAL = 1,
    IBAUDIO_STREAMING_WINDOW_INCREMENTAL_REVISABLE = 2,
    IBAUDIO_STREAMING_STATEFUL_LOW_LATENCY = 3,
    IBAUDIO_STREAMING_SEGMENT_CHUNKED = 4,
    IBAUDIO_STREAMING_DEFERRED = 5
};

typedef int32_t ibaudio_buffer_kind_t;
enum {
    IBAUDIO_BUFFER_BYTES = 1,
    IBAUDIO_BUFFER_UTF8 = 2,
    IBAUDIO_BUFFER_AUDIO_F32 = 3,
    IBAUDIO_BUFFER_VAD_SEGMENTS = 4
};

typedef int32_t ibaudio_job_state_t;
enum {
    IBAUDIO_JOB_QUEUED = 1,
    IBAUDIO_JOB_RUNNING = 2,
    IBAUDIO_JOB_SUCCEEDED = 3,
    IBAUDIO_JOB_CANCELLED = 4,
    IBAUDIO_JOB_FAILED = 5
};

typedef int32_t ibaudio_event_type_t;
enum {
    IBAUDIO_EVENT_PARTIAL_TEXT = 1,
    IBAUDIO_EVENT_FINAL_TEXT = 2,
    IBAUDIO_EVENT_AUDIO_CHUNK = 3,
    IBAUDIO_EVENT_VAD_SPEECH_START = 4,
    IBAUDIO_EVENT_VAD_SPEECH_END = 5,
    IBAUDIO_EVENT_VAD_SEGMENT = 6,
    IBAUDIO_EVENT_FINAL = 7,
    IBAUDIO_EVENT_CANCELLED = 8,
    IBAUDIO_EVENT_DIAGNOSTIC = 9
};

typedef int32_t ibaudio_barge_in_state_t;
enum {
    IBAUDIO_BARGE_IN_IDLE = 0,
    IBAUDIO_BARGE_IN_OUTPUT_ACTIVE = 1,
    IBAUDIO_BARGE_IN_SPEECH_CANDIDATE = 2,
    IBAUDIO_BARGE_IN_INTERRUPTED = 3
};

enum {
    IBAUDIO_CAP_OFFLINE = 1ull << 0u,
    IBAUDIO_CAP_STREAM_INPUT = 1ull << 1u,
    IBAUDIO_CAP_PARTIAL_OUTPUT = 1ull << 2u,
    IBAUDIO_CAP_FINAL_OUTPUT = 1ull << 3u,
    IBAUDIO_CAP_CANCELLATION = 1ull << 4u,
    IBAUDIO_CAP_TIMESTAMPS = 1ull << 5u,
    IBAUDIO_CAP_DETERMINISTIC_REFERENCE = 1ull << 6u,
    IBAUDIO_CAP_BARGE_IN = 1ull << 7u
};

enum {
    IBAUDIO_AUDIO_FLAG_NONE = 0u,
    IBAUDIO_AUDIO_FLAG_DISCONTINUITY = 1u << 0u,
    IBAUDIO_AUDIO_FLAG_END_OF_INPUT = 1u << 1u
};

typedef struct ibaudio_string_view_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    const char *data;
    uint64_t size;
} ibaudio_string_view_v1;

typedef struct ibaudio_error_info_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    ibaudio_status_t status;
    ibaudio_error_domain_t domain;
    int64_t native_code;
    uint32_t recoverable;
    char function_name[64];
    char message[256];
} ibaudio_error_info_v1;

typedef struct ibaudio_runtime_options_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    ibaudio_string_view_v1 cache_directory;
    ibaudio_string_view_v1 allowed_model_root;
    uint32_t cpu_threads;
    ibaudio_backend_t requested_backend;
    uint32_t allow_auto_cpu_fallback;
    uint32_t strict_path_policy;
    uint32_t deterministic_mode;
    uint32_t max_cached_models;
    uint64_t max_input_frames;
} ibaudio_runtime_options_v1;

typedef struct ibaudio_capabilities_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    uint32_t abi_major;
    uint32_t abi_minor;
    uint32_t model_count;
    uint32_t backend_count;
    uint32_t max_channels;
    uint32_t min_sample_rate;
    uint32_t max_sample_rate;
    uint64_t max_input_frames;
    uint64_t feature_flags;
} ibaudio_capabilities_v1;

typedef struct ibaudio_backend_info_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    ibaudio_backend_t backend;
    ibaudio_backend_availability_t availability;
    uint32_t compiled;
    uint32_t selected;
    uint32_t safe_cpu_fallback;
    char name[32];
    char device[96];
    char reason[192];
} ibaudio_backend_info_v1;

typedef struct ibaudio_model_descriptor_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    ibaudio_task_t task;
    ibaudio_streaming_class_t streaming_class;
    uint64_t capabilities;
    uint32_t available;
    uint32_t required_sample_rate;
    uint32_t required_channels;
    uint64_t artifact_size_bytes;
    char id[64];
    char family[64];
    char version[32];
    char artifact_sha256[65];
    char hash_kind[32];
    char spdx_license[32];
    char source_uri[128];
    char source_revision[80];
    char streaming_label[128];
    char availability_reason[160];
} ibaudio_model_descriptor_v1;

typedef struct ibaudio_model_load_options_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    ibaudio_string_view_v1 model_id;
    ibaudio_string_view_v1 artifact_path;
    ibaudio_string_view_v1 expected_sha256;
    ibaudio_backend_t backend;
    uint32_t verify_hash;
} ibaudio_model_load_options_v1;

typedef struct ibaudio_session_options_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    ibaudio_task_t task;
    uint32_t streaming;
    float vad_threshold_dbfs;
    uint32_t vad_frame_ms;
    uint32_t vad_hop_ms;
    uint32_t vad_min_speech_ms;
    uint32_t vad_min_silence_ms;
    float barge_in_threshold_dbfs;
    uint32_t barge_in_hold_ms;
} ibaudio_session_options_v1;

typedef struct ibaudio_audio_view_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    const float *interleaved_f32;
    uint64_t frame_count;
    uint32_t sample_rate;
    uint32_t channels;
    uint64_t start_frame;
    uint32_t flags;
} ibaudio_audio_view_v1;

typedef struct ibaudio_audio_process_options_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    uint32_t target_sample_rate;
    uint32_t target_channels;
    float gain_db;
    float normalize_peak;
    float clip_peak;
    uint32_t sanitize_non_finite;
} ibaudio_audio_process_options_v1;

typedef struct ibaudio_audio_info_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    uint32_t sample_rate;
    uint32_t channels;
    uint64_t frame_count;
    uint64_t clipped_samples;
    uint64_t sanitized_samples;
    float input_peak;
    float output_peak;
    float applied_gain;
} ibaudio_audio_info_v1;

typedef struct ibaudio_vad_segment_v1 {
    uint64_t start_frame;
    uint64_t end_frame;
    float confidence;
    float peak_dbfs;
} ibaudio_vad_segment_v1;

typedef struct ibaudio_stream_options_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    uint32_t preferred_chunk_frames;
    uint32_t max_queued_events;
    uint32_t emit_partial_results;
} ibaudio_stream_options_v1;

typedef struct ibaudio_stream_event_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    ibaudio_event_type_t type;
    uint32_t is_final;
    uint64_t sequence;
    uint64_t start_frame;
    uint64_t end_frame;
    float confidence;
    ibaudio_buffer_t *payload;
} ibaudio_stream_event_v1;

typedef struct ibaudio_job_info_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    ibaudio_job_state_t state;
    ibaudio_status_t result_status;
    uint64_t started_monotonic_ns;
    uint64_t finished_monotonic_ns;
    uint64_t processed_units;
    uint32_t cancellation_requested;
} ibaudio_job_info_v1;

typedef struct ibaudio_audio_cpp_status_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    uint32_t adapter_compiled;
    uint32_t inference_ready;
    char reviewed_commit[48];
    char upstream_source[128];
    char reason[192];
} ibaudio_audio_cpp_status_v1;

typedef struct ibaudio_metrics_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    uint64_t runtimes_created;
    uint64_t models_loaded;
    uint64_t model_cache_hits;
    uint64_t model_cache_misses;
    uint64_t sessions_created;
    uint64_t jobs_started;
    uint64_t jobs_cancelled;
    uint64_t streams_started;
    uint64_t audio_frames_in;
    uint64_t audio_frames_out;
    uint64_t calls_rejected_busy;
    uint64_t backend_fallbacks;
    uint64_t errors_reported;
    uint64_t live_owned_buffers;
} ibaudio_metrics_v1;

/* Version and error inspection. ibaudio_error_get_last() is thread-local. */
IBAUDIO_API uint32_t ibaudio_get_api_version(void);
IBAUDIO_API const char *ibaudio_get_runtime_version(void);
IBAUDIO_API const char *ibaudio_status_string(ibaudio_status_t status);
IBAUDIO_API ibaudio_status_t ibaudio_error_get_last(ibaudio_error_info_v1 *out_error);
IBAUDIO_API void ibaudio_error_clear(void);

/* Initializers set struct_size, api_version, and release-candidate defaults. */
IBAUDIO_API void ibaudio_runtime_options_init(ibaudio_runtime_options_v1 *options);
IBAUDIO_API void ibaudio_model_load_options_init(ibaudio_model_load_options_v1 *options);
IBAUDIO_API void ibaudio_session_options_init(ibaudio_session_options_v1 *options);
IBAUDIO_API void ibaudio_audio_process_options_init(ibaudio_audio_process_options_v1 *options);
IBAUDIO_API void ibaudio_stream_options_init(ibaudio_stream_options_v1 *options);

/* Runtime owns policy, registry, diagnostics, metrics, and the model cache. */
IBAUDIO_API ibaudio_status_t ibaudio_runtime_create(
    const ibaudio_runtime_options_v1 *options,
    ibaudio_runtime_t **out_runtime);
IBAUDIO_API ibaudio_status_t ibaudio_runtime_release(ibaudio_runtime_t **runtime);
IBAUDIO_API ibaudio_status_t ibaudio_runtime_get_capabilities(
    const ibaudio_runtime_t *runtime,
    ibaudio_capabilities_v1 *out_capabilities);
IBAUDIO_API ibaudio_status_t ibaudio_runtime_get_backend_count(
    const ibaudio_runtime_t *runtime,
    uint32_t *out_count);
IBAUDIO_API ibaudio_status_t ibaudio_runtime_get_backend_info(
    const ibaudio_runtime_t *runtime,
    uint32_t index,
    ibaudio_backend_info_v1 *out_info);
IBAUDIO_API ibaudio_status_t ibaudio_runtime_get_model_count(
    const ibaudio_runtime_t *runtime,
    uint32_t *out_count);
IBAUDIO_API ibaudio_status_t ibaudio_runtime_get_model_descriptor(
    const ibaudio_runtime_t *runtime,
    uint32_t index,
    ibaudio_model_descriptor_v1 *out_descriptor);
IBAUDIO_API ibaudio_status_t ibaudio_runtime_get_diagnostics_json(
    const ibaudio_runtime_t *runtime,
    ibaudio_buffer_t **out_json);
IBAUDIO_API ibaudio_status_t ibaudio_runtime_get_metrics(
    const ibaudio_runtime_t *runtime,
    ibaudio_metrics_v1 *out_metrics);
IBAUDIO_API ibaudio_status_t ibaudio_runtime_get_audio_cpp_status(
    const ibaudio_runtime_t *runtime,
    ibaudio_audio_cpp_status_v1 *out_status);
IBAUDIO_API ibaudio_status_t ibaudio_runtime_reset_metrics(ibaudio_runtime_t *runtime);

/* Models are immutable logical adapters. Reference models contain no weights. */
IBAUDIO_API ibaudio_status_t ibaudio_model_load(
    ibaudio_runtime_t *runtime,
    const ibaudio_model_load_options_v1 *options,
    ibaudio_model_t **out_model);
IBAUDIO_API ibaudio_status_t ibaudio_model_get_descriptor(
    const ibaudio_model_t *model,
    ibaudio_model_descriptor_v1 *out_descriptor);
IBAUDIO_API ibaudio_status_t ibaudio_model_release(ibaudio_model_t **model);

/* Sessions are single-flight. Concurrent inference returns IBAUDIO_STATUS_BUSY. */
IBAUDIO_API ibaudio_status_t ibaudio_session_create(
    ibaudio_model_t *model,
    const ibaudio_session_options_v1 *options,
    ibaudio_session_t **out_session);
IBAUDIO_API ibaudio_status_t ibaudio_session_reset(ibaudio_session_t *session);
IBAUDIO_API ibaudio_status_t ibaudio_session_release(ibaudio_session_t **session);
IBAUDIO_API ibaudio_status_t ibaudio_session_run_asr(
    ibaudio_session_t *session,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_buffer_t **out_text);
IBAUDIO_API ibaudio_status_t ibaudio_session_run_tts(
    ibaudio_session_t *session,
    ibaudio_string_view_v1 text,
    ibaudio_buffer_t **out_audio);
IBAUDIO_API ibaudio_status_t ibaudio_session_run_vad(
    ibaudio_session_t *session,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_buffer_t **out_segments);
IBAUDIO_API ibaudio_status_t ibaudio_session_run_kws(
    ibaudio_session_t *session,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_buffer_t **out_result);

/* Asynchronous jobs copy the borrowed request before returning. */
IBAUDIO_API ibaudio_status_t ibaudio_job_start_asr(
    ibaudio_session_t *session,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_job_t **out_job);
IBAUDIO_API ibaudio_status_t ibaudio_job_start_tts(
    ibaudio_session_t *session,
    ibaudio_string_view_v1 text,
    ibaudio_job_t **out_job);
IBAUDIO_API ibaudio_status_t ibaudio_job_start_vad(
    ibaudio_session_t *session,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_job_t **out_job);
IBAUDIO_API ibaudio_status_t ibaudio_job_get_info(
    const ibaudio_job_t *job,
    ibaudio_job_info_v1 *out_info);
IBAUDIO_API ibaudio_status_t ibaudio_job_wait(ibaudio_job_t *job, uint32_t timeout_ms);
IBAUDIO_API ibaudio_status_t ibaudio_job_cancel(ibaudio_job_t *job);
IBAUDIO_API ibaudio_status_t ibaudio_job_take_result(
    ibaudio_job_t *job,
    ibaudio_buffer_t **out_result);
IBAUDIO_API ibaudio_status_t ibaudio_job_release(ibaudio_job_t **job);

/* Pull-based streams. Poll never invokes user code and owns no caller callback. */
IBAUDIO_API ibaudio_status_t ibaudio_stream_start(
    ibaudio_session_t *session,
    const ibaudio_stream_options_v1 *options,
    ibaudio_stream_t **out_stream);
IBAUDIO_API ibaudio_status_t ibaudio_tts_stream_start(
    ibaudio_session_t *session,
    ibaudio_string_view_v1 text,
    const ibaudio_stream_options_v1 *options,
    ibaudio_stream_t **out_stream);
IBAUDIO_API ibaudio_status_t ibaudio_stream_push_audio(
    ibaudio_stream_t *stream,
    const ibaudio_audio_view_v1 *audio);
IBAUDIO_API ibaudio_status_t ibaudio_stream_finish(ibaudio_stream_t *stream);
IBAUDIO_API ibaudio_status_t ibaudio_stream_cancel(ibaudio_stream_t *stream);
IBAUDIO_API ibaudio_status_t ibaudio_stream_poll_event(
    ibaudio_stream_t *stream,
    uint32_t timeout_ms,
    ibaudio_stream_event_v1 *out_event);
IBAUDIO_API void ibaudio_stream_event_release(ibaudio_stream_event_v1 *event);
IBAUDIO_API ibaudio_status_t ibaudio_stream_release(ibaudio_stream_t **stream);

/* Playback/VAD coordination. Sustained input above threshold interrupts jobs. */
IBAUDIO_API ibaudio_status_t ibaudio_session_set_playback_active(
    ibaudio_session_t *session,
    uint32_t active);
IBAUDIO_API ibaudio_status_t ibaudio_session_report_input_level(
    ibaudio_session_t *session,
    float rms_dbfs,
    uint32_t duration_ms,
    ibaudio_barge_in_state_t *out_state,
    uint32_t *out_should_interrupt);
IBAUDIO_API ibaudio_status_t ibaudio_session_get_barge_in_state(
    const ibaudio_session_t *session,
    ibaudio_barge_in_state_t *out_state);

/* Audio utilities: validation, conversion, gain, normalization, clipping, WAV. */
IBAUDIO_API ibaudio_status_t ibaudio_audio_process(
    ibaudio_runtime_t *runtime,
    const ibaudio_audio_view_v1 *input,
    const ibaudio_audio_process_options_v1 *options,
    ibaudio_buffer_t **out_audio);
IBAUDIO_API ibaudio_status_t ibaudio_wav_decode_memory(
    ibaudio_runtime_t *runtime,
    const void *bytes,
    uint64_t byte_count,
    ibaudio_buffer_t **out_audio);
IBAUDIO_API ibaudio_status_t ibaudio_wav_encode_pcm16(
    ibaudio_runtime_t *runtime,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_buffer_t **out_wav);

/* Immutable owned buffers. Release accepts NULL and clears the caller's handle. */
IBAUDIO_API ibaudio_status_t ibaudio_buffer_get_kind(
    const ibaudio_buffer_t *buffer,
    ibaudio_buffer_kind_t *out_kind);
IBAUDIO_API ibaudio_status_t ibaudio_buffer_get_data(
    const ibaudio_buffer_t *buffer,
    const void **out_data,
    uint64_t *out_size_bytes);
IBAUDIO_API ibaudio_status_t ibaudio_buffer_get_audio_view(
    const ibaudio_buffer_t *buffer,
    ibaudio_audio_view_v1 *out_audio);
IBAUDIO_API ibaudio_status_t ibaudio_buffer_get_audio_info(
    const ibaudio_buffer_t *buffer,
    ibaudio_audio_info_v1 *out_info);
IBAUDIO_API ibaudio_status_t ibaudio_buffer_release(ibaudio_buffer_t **buffer);

/* SHA-256 helper for immutable model/package verification. */
IBAUDIO_API ibaudio_status_t ibaudio_sha256_file(
    ibaudio_runtime_t *runtime,
    ibaudio_string_view_v1 path,
    char out_hex[65]);

/* Innovation APIs (v0.2.0) */

#if defined(IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_API)
typedef struct ibaudio_prosody_controller ibaudio_prosody_controller_t;
typedef struct ibaudio_turn_manager ibaudio_turn_manager_t;
typedef struct ibaudio_conversation_state ibaudio_conversation_state_t;
typedef struct ibaudio_environment_adapter ibaudio_environment_adapter_t;
typedef struct ibaudio_voice_clone_engine ibaudio_voice_clone_engine_t;
typedef struct ibaudio_codeswitch_detector ibaudio_codeswitch_detector_t;
typedef struct ibaudio_neural_codec ibaudio_neural_codec_t;
typedef struct ibaudio_context_aware_output ibaudio_context_aware_output_t;

typedef int32_t ibaudio_turn_action_t;
enum {
    IBAUDIO_TURN_CONTINUE = 1,
    IBAUDIO_TURN_YIELD = 2,
    IBAUDIO_TURN_BACKCHANNEL = 3,
    IBAUDIO_TURN_BARGE_IN = 4,
    IBAUDIO_TURN_ACCIDENTAL = 5,
    IBAUDIO_TURN_UNCERTAIN = 6
};

typedef int32_t ibaudio_conversation_state_enum_t;
enum {
    IBAUDIO_CONVERSATION_LISTENING = 1,
    IBAUDIO_CONVERSATION_THINKING = 2,
    IBAUDIO_CONVERSATION_SPEAKING = 3,
    IBAUDIO_CONVERSATION_OVERLAP = 4,
    IBAUDIO_CONVERSATION_YIELDING = 5
};

typedef struct ibaudio_environment_profile_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    float noise_floor_dbfs;
    float reverb_time_ms;
    float signal_to_noise_db;
    uint32_t is_noisy;
    uint32_t is_reverberant;
} ibaudio_environment_profile_v1;

typedef struct ibaudio_language_score_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    float english;
    float hindi;
    float hinglish;
    float confidence;
} ibaudio_language_score_v1;

typedef struct ibaudio_output_adjustment_v1 {
    uint32_t struct_size;
    uint32_t api_version;
    float volume_scale;
    float rate_scale;
    float emphasis_scale;
    float pause_scale;
} ibaudio_output_adjustment_v1;

/* Prosody controller for emotion, rate, pause, emphasis, and urgency. */
IBAUDIO_API ibaudio_prosody_controller_t *ibaudio_prosody_controller_create(void);
IBAUDIO_API void ibaudio_prosody_controller_destroy(ibaudio_prosody_controller_t *controller);
IBAUDIO_API ibaudio_status_t ibaudio_prosody_controller_set_emotion(
    ibaudio_prosody_controller_t *controller, float valence, float arousal);
IBAUDIO_API ibaudio_status_t ibaudio_prosody_controller_set_rate(
    ibaudio_prosody_controller_t *controller, float rate);
IBAUDIO_API ibaudio_status_t ibaudio_prosody_controller_set_urgency(
    ibaudio_prosody_controller_t *controller, float urgency);

/* Turn manager for semantic turn-taking and barge-in classification. */
IBAUDIO_API ibaudio_turn_manager_t *ibaudio_turn_manager_create(void);
IBAUDIO_API void ibaudio_turn_manager_destroy(ibaudio_turn_manager_t *manager);
IBAUDIO_API ibaudio_status_t ibaudio_turn_manager_update(
    ibaudio_turn_manager_t *manager,
    float speech_duration_ms,
    float silence_duration_ms,
    float speech_energy,
    float pitch_trend,
    uint32_t has_lexical_completion,
    uint32_t is_clause_boundary,
    float asr_confidence);
IBAUDIO_API ibaudio_status_t ibaudio_turn_manager_classify(
    ibaudio_turn_manager_t *manager,
    ibaudio_turn_action_t *out_action);

/* Conversation state machine for full-duplex dialogue. */
IBAUDIO_API ibaudio_conversation_state_t *ibaudio_conversation_state_create(void);
IBAUDIO_API void ibaudio_conversation_state_destroy(ibaudio_conversation_state_t *state);
IBAUDIO_API ibaudio_status_t ibaudio_conversation_state_transition(
    ibaudio_conversation_state_t *state,
    ibaudio_turn_action_t action,
    ibaudio_conversation_state_enum_t *out_state);
IBAUDIO_API ibaudio_status_t ibaudio_conversation_state_should_generate(
    ibaudio_conversation_state_t *state,
    uint32_t *out_should_generate);
IBAUDIO_API ibaudio_status_t ibaudio_conversation_state_should_listen(
    ibaudio_conversation_state_t *state,
    uint32_t *out_should_listen);

/* Environment adapter for noise suppression and room correction. */
IBAUDIO_API ibaudio_environment_adapter_t *ibaudio_environment_adapter_create(void);
IBAUDIO_API void ibaudio_environment_adapter_destroy(ibaudio_environment_adapter_t *adapter);
IBAUDIO_API ibaudio_status_t ibaudio_environment_adapter_analyze(
    ibaudio_environment_adapter_t *adapter,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_environment_profile_v1 *out_profile);
IBAUDIO_API ibaudio_status_t ibaudio_environment_adapter_suppress_noise(
    ibaudio_environment_adapter_t *adapter,
    ibaudio_audio_view_v1 *audio,
    const ibaudio_environment_profile_v1 *profile);

/* Voice clone engine for speaker enrollment and conditioning. */
IBAUDIO_API ibaudio_voice_clone_engine_t *ibaudio_voice_clone_engine_create(void);
IBAUDIO_API void ibaudio_voice_clone_engine_destroy(ibaudio_voice_clone_engine_t *engine);
IBAUDIO_API ibaudio_status_t ibaudio_voice_clone_engine_enroll(
    ibaudio_voice_clone_engine_t *engine,
    const ibaudio_audio_view_v1 *reference,
    const char *speaker_id,
    uint32_t consent_verified);
IBAUDIO_API ibaudio_status_t ibaudio_voice_clone_engine_verify_consent(
    ibaudio_voice_clone_engine_t *engine,
    const char *speaker_id);
IBAUDIO_API ibaudio_status_t ibaudio_voice_clone_engine_delete_speaker(
    ibaudio_voice_clone_engine_t *engine,
    const char *speaker_id);

/* Code-switch detector for English/Hindi/Hinglish. */
IBAUDIO_API ibaudio_codeswitch_detector_t *ibaudio_codeswitch_detector_create(void);
IBAUDIO_API void ibaudio_codeswitch_detector_destroy(ibaudio_codeswitch_detector_t *detector);
IBAUDIO_API ibaudio_status_t ibaudio_codeswitch_detector_detect(
    ibaudio_codeswitch_detector_t *detector,
    const char *transcript,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_language_score_v1 *out_score);
IBAUDIO_API ibaudio_status_t ibaudio_codeswitch_detector_is_code_switching(
    ibaudio_codeswitch_detector_t *detector,
    const ibaudio_language_score_v1 *score,
    uint32_t *out_is_switching);

/* Neural codec for efficient low-latency audio representation. */
IBAUDIO_API ibaudio_neural_codec_t *ibaudio_neural_codec_create(
    uint32_t sample_rate,
    uint32_t frame_size,
    float target_bitrate_kbps);
IBAUDIO_API void ibaudio_neural_codec_destroy(ibaudio_neural_codec_t *codec);
IBAUDIO_API ibaudio_status_t ibaudio_neural_codec_encode(
    ibaudio_neural_codec_t *codec,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_buffer_t **out_encoded);
IBAUDIO_API ibaudio_status_t ibaudio_neural_codec_decode(
    ibaudio_neural_codec_t *codec,
    const ibaudio_buffer_t *encoded,
    ibaudio_buffer_t **out_audio);
IBAUDIO_API ibaudio_status_t ibaudio_neural_codec_get_bitrate(
    ibaudio_neural_codec_t *codec,
    float *out_bitrate_kbps);

/* Context-aware output for volume/rate adjustment. */
IBAUDIO_API ibaudio_context_aware_output_t *ibaudio_context_aware_output_create(void);
IBAUDIO_API void ibaudio_context_aware_output_destroy(ibaudio_context_aware_output_t *output);
IBAUDIO_API ibaudio_status_t ibaudio_context_aware_output_compute(
    ibaudio_context_aware_output_t *output,
    float environment_noise_dbfs,
    ibaudio_conversation_state_enum_t conversation_state,
    float user_engagement,
    float time_pressure,
    ibaudio_output_adjustment_v1 *out_adjustment);
IBAUDIO_API ibaudio_status_t ibaudio_context_aware_output_apply(
    ibaudio_context_aware_output_t *output,
    ibaudio_audio_view_v1 *audio,
    const ibaudio_output_adjustment_v1 *adjustment);

#endif /* IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_API */

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* INBHARAT_IBAUDIO_H */
