#include "internal.hpp"
#include "provider.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <functional>
#include <memory>
#include <stdexcept>
#include <utility>

namespace {

class BusyGuard {
public:
    explicit BusyGuard(ibaudio_session *session) : session_(session) {}
    ~BusyGuard() {
        if (session_ != nullptr) session_->busy.store(false, std::memory_order_release);
    }
    BusyGuard(const BusyGuard &) = delete;
    BusyGuard &operator=(const BusyGuard &) = delete;
    void dismiss() noexcept { session_ = nullptr; }
private:
    ibaudio_session *session_;
};

ibaudio_status_t acquire_session(ibaudio_session *session, const char *function_name) {
    if (session == nullptr) {
        return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                  function_name, "session handle is null");
    }
    bool expected = false;
    if (!session->busy.compare_exchange_strong(expected, true, std::memory_order_acq_rel)) {
        if (session->model != nullptr && session->model->runtime != nullptr) {
            session->model->runtime->metrics.calls_rejected_busy.fetch_add(1u, std::memory_order_relaxed);
        }
        return ibaudio::set_error(IBAUDIO_STATUS_BUSY, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                  function_name, "session is single-flight and already running", true);
    }
    return IBAUDIO_STATUS_OK;
}

ibaudio_buffer *make_text(ibaudio_runtime *runtime, const std::string &text) {
    auto buffer = std::make_unique<ibaudio_buffer>();
    buffer->kind = IBAUDIO_BUFFER_UTF8;
    buffer->metrics = runtime == nullptr ? nullptr : &runtime->metrics;
    buffer->bytes.assign(text.begin(), text.end());
    buffer->bytes.push_back(0u);
    if (buffer->metrics != nullptr) {
        buffer->metrics->live_owned_buffers.fetch_add(1u, std::memory_order_relaxed);
    }
    return buffer.release();
}

ibaudio_buffer *make_audio(ibaudio_runtime *runtime, ibaudio::AudioData data) {
    auto buffer = std::make_unique<ibaudio_buffer>();
    buffer->kind = IBAUDIO_BUFFER_AUDIO_F32;
    buffer->metrics = runtime == nullptr ? nullptr : &runtime->metrics;
    buffer->audio = std::move(data);
    if (buffer->metrics != nullptr) {
        buffer->metrics->live_owned_buffers.fetch_add(1u, std::memory_order_relaxed);
    }
    return buffer.release();
}

ibaudio_buffer *make_segments(
    ibaudio_runtime *runtime,
    std::vector<ibaudio_vad_segment_v1> segments) {
    auto buffer = std::make_unique<ibaudio_buffer>();
    buffer->kind = IBAUDIO_BUFFER_VAD_SEGMENTS;
    buffer->metrics = runtime == nullptr ? nullptr : &runtime->metrics;
    buffer->segments = std::move(segments);
    if (buffer->metrics != nullptr) {
        buffer->metrics->live_owned_buffers.fetch_add(1u, std::memory_order_relaxed);
    }
    return buffer.release();
}

void destroy_buffer(ibaudio_buffer *buffer) {
    if (buffer == nullptr) return;
    ibaudio::MetricsData *metrics = buffer->metrics;
    delete buffer;
    if (metrics != nullptr) metrics->live_owned_buffers.fetch_sub(1u, std::memory_order_relaxed);
}

ibaudio::AudioData prepare_audio(
    ibaudio_session *session,
    const ibaudio_audio_view_v1 &audio,
    uint32_t target_rate,
    const ibaudio::CancellationToken *cancel = nullptr) {
    ibaudio_audio_process_options_v1 options{};
    ibaudio_audio_process_options_init(&options);
    options.target_sample_rate = target_rate;
    options.target_channels = 1u;
    options.sanitize_non_finite = 1u;
    return ibaudio::process_audio(audio, options, session->model->runtime->max_input_frames, cancel);
}

ibaudio_status_t validate_audio_header(const ibaudio_audio_view_v1 *audio, const char *function_name) {
    if (audio == nullptr || !ibaudio::valid_header(audio->struct_size, sizeof(*audio), audio->api_version)) {
        return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                  function_name, "audio view is null or has an invalid ABI header");
    }
    return IBAUDIO_STATUS_OK;
}

std::vector<float> copy_job_audio(ibaudio_session *session, const ibaudio_audio_view_v1 &audio) {
    if (audio.channels == 0u || audio.channels > ibaudio::kMaxChannels ||
        audio.sample_rate < ibaudio::kMinSampleRate || audio.sample_rate > ibaudio::kMaxSampleRate ||
        audio.frame_count > session->model->runtime->max_input_frames ||
        audio.frame_count > std::numeric_limits<size_t>::max() / audio.channels ||
        (audio.frame_count > 0u && audio.interleaved_f32 == nullptr)) {
        throw std::invalid_argument("invalid asynchronous audio shape, pointer, or sample rate");
    }
    const size_t count = static_cast<size_t>(audio.frame_count) * audio.channels;
    if (count == 0u) return {};
    return std::vector<float>(audio.interleaved_f32, audio.interleaved_f32 + count);
}

void set_job_terminal(
    ibaudio_job *job,
    ibaudio_job_state_t state,
    ibaudio_status_t status,
    ibaudio_buffer *result,
    uint64_t processed) {
    {
        std::lock_guard<std::mutex> lock(job->mutex);
        job->state = state;
        job->result_status = status;
        job->result = result;
        job->processed_units = processed;
        job->finished_ns = ibaudio::monotonic_ns();
    }
    {
        std::lock_guard<std::mutex> lock(job->session->active_job_mutex);
        if (job->session->active_job == job) job->session->active_job = nullptr;
    }
    job->session->busy.store(false, std::memory_order_release);
    job->cv.notify_all();
}

ibaudio_status_t start_job(
    ibaudio_session *session,
    ibaudio_job **out_job,
    std::function<std::pair<ibaudio_buffer *, uint64_t>(ibaudio_job *)> operation) {
    if (out_job == nullptr) {
        return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                  "start_job", "output job pointer is null");
    }
    *out_job = nullptr;
    auto job = std::make_unique<ibaudio_job>();
    job->session = session;
    session->live_jobs.fetch_add(1u, std::memory_order_release);
    session->model->runtime->metrics.jobs_started.fetch_add(1u, std::memory_order_relaxed);
    {
        std::lock_guard<std::mutex> lock(session->active_job_mutex);
        session->active_job = job.get();
    }
    ibaudio_job *raw = job.get();
    try {
        raw->worker = std::thread([raw, operation = std::move(operation)]() mutable {
            {
                std::lock_guard<std::mutex> lock(raw->mutex);
                raw->state = IBAUDIO_JOB_RUNNING;
                raw->started_ns = ibaudio::monotonic_ns();
            }
            try {
                if (raw->cancellation.requested.load(std::memory_order_relaxed)) {
                    set_job_terminal(raw, IBAUDIO_JOB_CANCELLED, IBAUDIO_STATUS_CANCELLED, nullptr, 0u);
                    return;
                }
                auto output = operation(raw);
                if (raw->cancellation.requested.load(std::memory_order_relaxed)) {
                    destroy_buffer(output.first);
                    set_job_terminal(raw, IBAUDIO_JOB_CANCELLED, IBAUDIO_STATUS_CANCELLED, nullptr, output.second);
                } else {
                    set_job_terminal(raw, IBAUDIO_JOB_SUCCEEDED, IBAUDIO_STATUS_OK, output.first, output.second);
                }
            } catch (const std::exception &) {
                const bool cancelled = raw->cancellation.requested.load(std::memory_order_relaxed);
                set_job_terminal(raw, cancelled ? IBAUDIO_JOB_CANCELLED : IBAUDIO_JOB_FAILED,
                                 cancelled ? IBAUDIO_STATUS_CANCELLED : IBAUDIO_STATUS_INTERNAL_ERROR,
                                 nullptr, raw->processed_units);
            } catch (...) {
                set_job_terminal(raw, IBAUDIO_JOB_FAILED, IBAUDIO_STATUS_INTERNAL_ERROR, nullptr, raw->processed_units);
            }
        });
    } catch (...) {
        {
            std::lock_guard<std::mutex> lock(session->active_job_mutex);
            if (session->active_job == raw) session->active_job = nullptr;
        }
        session->live_jobs.fetch_sub(1u, std::memory_order_release);
        session->busy.store(false, std::memory_order_release);
        throw;
    }
    *out_job = job.release();
    return IBAUDIO_STATUS_OK;
}

} // namespace

extern "C" {

ibaudio_status_t ibaudio_session_create(
    ibaudio_model_t *model,
    const ibaudio_session_options_v1 *options,
    ibaudio_session_t **out_session) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (model == nullptr || options == nullptr || out_session == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "model, options, and output session are required");
        }
        *out_session = nullptr;
        if (!ibaudio::valid_header(options->struct_size, sizeof(*options), options->api_version)) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "invalid session-options header");
        }
        if (options->task != model->record.descriptor.task) {
            return ibaudio::set_error(IBAUDIO_STATUS_UNSUPPORTED, IBAUDIO_ERROR_DOMAIN_MODEL,
                                      __func__, "requested task does not match model descriptor");
        }
        if (options->streaming != 0u && model->record.descriptor.streaming_class == IBAUDIO_STREAMING_OFFLINE_ONLY) {
            return ibaudio::set_error(IBAUDIO_STATUS_UNSUPPORTED, IBAUDIO_ERROR_DOMAIN_MODEL,
                                      __func__, "model is offline-only");
        }
        if (!std::isfinite(options->vad_threshold_dbfs) || options->vad_threshold_dbfs < -120.0f ||
            options->vad_threshold_dbfs > 0.0f || options->vad_frame_ms == 0u || options->vad_hop_ms == 0u ||
            options->vad_frame_ms > 1000u || options->vad_hop_ms > 1000u ||
            options->vad_min_speech_ms == 0u || options->vad_min_speech_ms > 60000u ||
            options->vad_min_silence_ms == 0u || options->vad_min_silence_ms > 60000u ||
            !std::isfinite(options->barge_in_threshold_dbfs) ||
            options->barge_in_threshold_dbfs < -160.0f || options->barge_in_threshold_dbfs > 12.0f ||
            options->barge_in_hold_ms > 10000u) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "invalid VAD or barge-in configuration");
        }
        auto session = std::make_unique<ibaudio_session>();
        session->model = model;
        session->task = options->task;
        session->streaming_enabled = options->streaming != 0u;
        session->vad.threshold_dbfs = options->vad_threshold_dbfs;
        session->vad.frame_ms = options->vad_frame_ms;
        session->vad.hop_ms = options->vad_hop_ms;
        session->vad.min_speech_ms = options->vad_min_speech_ms;
        session->vad.min_silence_ms = options->vad_min_silence_ms;
        session->barge_threshold_dbfs = options->barge_in_threshold_dbfs;
        session->barge_hold_ms = std::max<uint32_t>(1u, options->barge_in_hold_ms);
        model->live_sessions.fetch_add(1u, std::memory_order_release);
        model->runtime->metrics.sessions_created.fetch_add(1u, std::memory_order_relaxed);
        *out_session = session.release();
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_session_reset(ibaudio_session_t *session) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (session == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "session handle is null");
        }
        if (session->busy.load(std::memory_order_acquire) ||
            session->live_jobs.load(std::memory_order_acquire) != 0u ||
            session->live_streams.load(std::memory_order_acquire) != 0u) {
            return ibaudio::set_error(IBAUDIO_STATUS_BUSY, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                      __func__, "session reset requires no live work handles", true);
        }
        std::lock_guard<std::mutex> lock(session->barge_mutex);
        session->barge_state = IBAUDIO_BARGE_IN_IDLE;
        session->barge_accumulated_ms = 0u;
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_session_release(ibaudio_session_t **session) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (session == nullptr || *session == nullptr) return IBAUDIO_STATUS_OK;
        if ((*session)->busy.load(std::memory_order_acquire) ||
            (*session)->live_jobs.load(std::memory_order_acquire) != 0u ||
            (*session)->live_streams.load(std::memory_order_acquire) != 0u) {
            return ibaudio::set_error(IBAUDIO_STATUS_BUSY, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                      __func__, "session still owns live jobs or streams", true);
        }
        ibaudio_model *model = (*session)->model;
        delete *session;
        *session = nullptr;
        if (model != nullptr) model->live_sessions.fetch_sub(1u, std::memory_order_release);
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_session_run_asr(
    ibaudio_session_t *session,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_buffer_t **out_text) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (session == nullptr || session->task != IBAUDIO_TASK_ASR || out_text == nullptr ||
            validate_audio_header(audio, __func__) != IBAUDIO_STATUS_OK) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "ASR session, valid audio, and output are required");
        }
        *out_text = nullptr;
        const ibaudio_status_t acquired = acquire_session(session, __func__);
        if (acquired != IBAUDIO_STATUS_OK) return acquired;
        BusyGuard guard(session);
        ibaudio::AudioData prepared = prepare_audio(session, *audio, 16000u);
        ibaudio::Provider *provider = session->model->provider;
        if (provider == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_UNAVAILABLE, IBAUDIO_ERROR_DOMAIN_MODEL,
                                      __func__, "no provider resolved for this model", true);
        }
        uint64_t processed = 0u;
        std::string text;
        const ibaudio_status_t status = provider->run_asr(prepared, nullptr, &processed, text);
        if (status != IBAUDIO_STATUS_OK) return status;
        session->model->runtime->metrics.audio_frames_in.fetch_add(audio->frame_count, std::memory_order_relaxed);
        *out_text = make_text(session->model->runtime, text);
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_session_run_tts(
    ibaudio_session_t *session,
    ibaudio_string_view_v1 text,
    ibaudio_buffer_t **out_audio) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (session == nullptr || session->task != IBAUDIO_TASK_TTS || out_audio == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "TTS session and output are required");
        }
        *out_audio = nullptr;
        const std::string input = ibaudio::from_view(text);
        const ibaudio_status_t acquired = acquire_session(session, __func__);
        if (acquired != IBAUDIO_STATUS_OK) return acquired;
        BusyGuard guard(session);
        ibaudio::Provider *provider = session->model->provider;
        if (provider == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_UNAVAILABLE, IBAUDIO_ERROR_DOMAIN_MODEL,
                                      __func__, "no provider resolved for this model", true);
        }
        uint64_t processed = 0u;
        ibaudio::AudioData output;
        const ibaudio_status_t status = provider->run_tts(input, nullptr, &processed, output);
        if (status != IBAUDIO_STATUS_OK) return status;
        session->model->runtime->metrics.audio_frames_out.fetch_add(output.info.frame_count, std::memory_order_relaxed);
        *out_audio = make_audio(session->model->runtime, std::move(output));
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_session_run_vad(
    ibaudio_session_t *session,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_buffer_t **out_segments) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (session == nullptr || session->task != IBAUDIO_TASK_VAD || out_segments == nullptr ||
            validate_audio_header(audio, __func__) != IBAUDIO_STATUS_OK) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "VAD session, valid audio, and output are required");
        }
        *out_segments = nullptr;
        const ibaudio_status_t acquired = acquire_session(session, __func__);
        if (acquired != IBAUDIO_STATUS_OK) return acquired;
        BusyGuard guard(session);
        ibaudio::AudioData prepared = prepare_audio(session, *audio, 16000u);
        ibaudio::Provider *provider = session->model->provider;
        if (provider == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_UNAVAILABLE, IBAUDIO_ERROR_DOMAIN_MODEL,
                                      __func__, "no provider resolved for this model", true);
        }
        uint64_t processed = 0u;
        std::vector<ibaudio_vad_segment_v1> segments;
        const ibaudio_status_t status = provider->run_vad(prepared, session->vad, nullptr, &processed, segments);
        if (status != IBAUDIO_STATUS_OK) return status;
        session->model->runtime->metrics.audio_frames_in.fetch_add(audio->frame_count, std::memory_order_relaxed);
        *out_segments = make_segments(session->model->runtime, std::move(segments));
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_session_run_kws(
    ibaudio_session_t *,
    const ibaudio_audio_view_v1 *,
    ibaudio_buffer_t **out_result) {
    if (out_result != nullptr) *out_result = nullptr;
    return ibaudio::set_error(IBAUDIO_STATUS_DEFERRED, IBAUDIO_ERROR_DOMAIN_MODEL, __func__,
        "KWS ABI is reserved, but inference is deferred pending a licensed immutable model and parity evidence", true);
}

ibaudio_status_t ibaudio_job_start_asr(
    ibaudio_session_t *session,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_job_t **out_job) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (session == nullptr || session->task != IBAUDIO_TASK_ASR || out_job == nullptr ||
            validate_audio_header(audio, __func__) != IBAUDIO_STATUS_OK) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "ASR session and valid audio are required");
        }
        const ibaudio_status_t acquired = acquire_session(session, __func__);
        if (acquired != IBAUDIO_STATUS_OK) return acquired;
        BusyGuard reservation(session);
        ibaudio::Provider *provider = session->model->provider;
        if (provider == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_UNAVAILABLE, IBAUDIO_ERROR_DOMAIN_MODEL,
                                      __func__, "no provider resolved for this model", true);
        }
        std::vector<float> samples = copy_job_audio(session, *audio);
        ibaudio_audio_view_v1 copied = *audio;
        const ibaudio_status_t status = start_job(session, out_job,
            [session, provider, samples = std::move(samples), copied](ibaudio_job *job) mutable {
                copied.interleaved_f32 = samples.data();
                ibaudio::AudioData prepared = prepare_audio(session, copied, 16000u, &job->cancellation);
                uint64_t processed = 0u;
                std::string text;
                const ibaudio_status_t run_status = provider->run_asr(prepared, &job->cancellation, &processed, text);
                if (run_status != IBAUDIO_STATUS_OK) {
                    throw std::runtime_error(std::string("provider ASR failed: ") + ibaudio_status_string(run_status));
                }
                session->model->runtime->metrics.audio_frames_in.fetch_add(copied.frame_count, std::memory_order_relaxed);
                return std::make_pair(make_text(session->model->runtime, text), processed);
            });
        if (status == IBAUDIO_STATUS_OK) reservation.dismiss();
        return status;
    });
}

ibaudio_status_t ibaudio_job_start_tts(
    ibaudio_session_t *session,
    ibaudio_string_view_v1 text,
    ibaudio_job_t **out_job) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (session == nullptr || session->task != IBAUDIO_TASK_TTS || out_job == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "TTS session and output job are required");
        }
        const ibaudio_status_t acquired = acquire_session(session, __func__);
        if (acquired != IBAUDIO_STATUS_OK) return acquired;
        BusyGuard reservation(session);
        ibaudio::Provider *provider = session->model->provider;
        if (provider == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_UNAVAILABLE, IBAUDIO_ERROR_DOMAIN_MODEL,
                                      __func__, "no provider resolved for this model", true);
        }
        std::string copied = ibaudio::from_view(text);
        if (copied.empty() || copied.size() > 16384u) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "TTS text must contain 1 to 16384 bytes");
        }
        const ibaudio_status_t status = start_job(session, out_job,
            [session, provider, copied = std::move(copied)](ibaudio_job *job) {
                uint64_t processed = 0u;
                ibaudio::AudioData output;
                const ibaudio_status_t run_status = provider->run_tts(copied, &job->cancellation, &processed, output);
                if (run_status != IBAUDIO_STATUS_OK) {
                    throw std::runtime_error(std::string("provider TTS failed: ") + ibaudio_status_string(run_status));
                }
                session->model->runtime->metrics.audio_frames_out.fetch_add(output.info.frame_count, std::memory_order_relaxed);
                return std::make_pair(make_audio(session->model->runtime, std::move(output)), processed);
            });
        if (status == IBAUDIO_STATUS_OK) reservation.dismiss();
        return status;
    });
}

ibaudio_status_t ibaudio_job_start_vad(
    ibaudio_session_t *session,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_job_t **out_job) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (session == nullptr || session->task != IBAUDIO_TASK_VAD || out_job == nullptr ||
            validate_audio_header(audio, __func__) != IBAUDIO_STATUS_OK) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "VAD session and valid audio are required");
        }
        const ibaudio_status_t acquired = acquire_session(session, __func__);
        if (acquired != IBAUDIO_STATUS_OK) return acquired;
        BusyGuard reservation(session);
        ibaudio::Provider *provider = session->model->provider;
        if (provider == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_UNAVAILABLE, IBAUDIO_ERROR_DOMAIN_MODEL,
                                      __func__, "no provider resolved for this model", true);
        }
        std::vector<float> samples = copy_job_audio(session, *audio);
        ibaudio_audio_view_v1 copied = *audio;
        const ibaudio_status_t status = start_job(session, out_job,
            [session, provider, samples = std::move(samples), copied](ibaudio_job *job) mutable {
                copied.interleaved_f32 = samples.data();
                ibaudio::AudioData prepared = prepare_audio(session, copied, 16000u, &job->cancellation);
                uint64_t processed = 0u;
                std::vector<ibaudio_vad_segment_v1> segments;
                const ibaudio_status_t run_status = provider->run_vad(prepared, session->vad, &job->cancellation, &processed, segments);
                if (run_status != IBAUDIO_STATUS_OK) {
                    throw std::runtime_error(std::string("provider VAD failed: ") + ibaudio_status_string(run_status));
                }
                session->model->runtime->metrics.audio_frames_in.fetch_add(copied.frame_count, std::memory_order_relaxed);
                return std::make_pair(make_segments(session->model->runtime, std::move(segments)), processed);
            });
        if (status == IBAUDIO_STATUS_OK) reservation.dismiss();
        return status;
    });
}

ibaudio_status_t ibaudio_job_get_info(const ibaudio_job_t *job, ibaudio_job_info_v1 *out_info) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (job == nullptr || out_info == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "job or output info is null");
        }
        std::lock_guard<std::mutex> lock(job->mutex);
        *out_info = {};
        out_info->struct_size = sizeof(*out_info);
        out_info->api_version = IBAUDIO_API_VERSION;
        out_info->state = job->state;
        out_info->result_status = job->result_status;
        out_info->started_monotonic_ns = job->started_ns;
        out_info->finished_monotonic_ns = job->finished_ns;
        out_info->processed_units = job->processed_units;
        out_info->cancellation_requested = job->cancellation.requested.load(std::memory_order_relaxed) ? 1u : 0u;
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_job_wait(ibaudio_job_t *job, uint32_t timeout_ms) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (job == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "job handle is null");
        }
        std::unique_lock<std::mutex> lock(job->mutex);
        const bool done = job->cv.wait_for(lock, std::chrono::milliseconds(timeout_ms),
                                           [&]() { return ibaudio::is_terminal(job->state); });
        if (!done) {
            return ibaudio::set_error(IBAUDIO_STATUS_TIMEOUT, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                      __func__, "job did not settle before timeout", true);
        }
        if (job->result_status == IBAUDIO_STATUS_CANCELLED) {
            return ibaudio::set_error(IBAUDIO_STATUS_CANCELLED, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                      __func__, "job was cancelled", true);
        }
        if (job->result_status != IBAUDIO_STATUS_OK) {
            return ibaudio::set_error(job->result_status, IBAUDIO_ERROR_DOMAIN_INTERNAL,
                                      __func__, "job failed inside the native worker");
        }
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_job_cancel(ibaudio_job_t *job) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (job == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "job handle is null");
        }
        {
            std::lock_guard<std::mutex> lock(job->mutex);
            if (ibaudio::is_terminal(job->state)) return IBAUDIO_STATUS_OK;
        }
        if (!job->cancellation.requested.exchange(true, std::memory_order_acq_rel)) {
            job->session->model->runtime->metrics.jobs_cancelled.fetch_add(1u, std::memory_order_relaxed);
        }
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_job_take_result(ibaudio_job_t *job, ibaudio_buffer_t **out_result) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (job == nullptr || out_result == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "job and output result are required");
        }
        *out_result = nullptr;
        std::lock_guard<std::mutex> lock(job->mutex);
        if (!ibaudio::is_terminal(job->state)) {
            return ibaudio::set_error(IBAUDIO_STATUS_WOULD_BLOCK, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                      __func__, "job has not completed", true);
        }
        if (job->state != IBAUDIO_JOB_SUCCEEDED) {
            return ibaudio::set_error(job->result_status,
                job->result_status == IBAUDIO_STATUS_CANCELLED ? IBAUDIO_ERROR_DOMAIN_LIFECYCLE : IBAUDIO_ERROR_DOMAIN_INTERNAL,
                __func__, job->result_status == IBAUDIO_STATUS_CANCELLED ? "job was cancelled" : "job failed", true);
        }
        if (job->result_taken || job->result == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_STATE, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                      __func__, "job result was already taken");
        }
        *out_result = job->result;
        job->result = nullptr;
        job->result_taken = true;
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_job_release(ibaudio_job_t **job) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (job == nullptr || *job == nullptr) return IBAUDIO_STATUS_OK;
        ibaudio_job *value = *job;
        ibaudio_job_cancel(value);
        if (value->worker.joinable()) value->worker.join();
        destroy_buffer(value->result);
        ibaudio_session *session = value->session;
        delete value;
        *job = nullptr;
        if (session != nullptr) session->live_jobs.fetch_sub(1u, std::memory_order_release);
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_session_set_playback_active(ibaudio_session_t *session, uint32_t active) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (session == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "session handle is null");
        }
        std::lock_guard<std::mutex> lock(session->barge_mutex);
        session->barge_state = active != 0u ? IBAUDIO_BARGE_IN_OUTPUT_ACTIVE : IBAUDIO_BARGE_IN_IDLE;
        session->barge_accumulated_ms = 0u;
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_session_report_input_level(
    ibaudio_session_t *session,
    float rms_dbfs,
    uint32_t duration_ms,
    ibaudio_barge_in_state_t *out_state,
    uint32_t *out_should_interrupt) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (session == nullptr || out_state == nullptr || out_should_interrupt == nullptr ||
            !std::isfinite(rms_dbfs) || rms_dbfs > 12.0f || rms_dbfs < -160.0f) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "invalid session, level, or output pointers");
        }
        bool interrupt = false;
        {
            std::lock_guard<std::mutex> lock(session->barge_mutex);
            if (session->barge_state == IBAUDIO_BARGE_IN_OUTPUT_ACTIVE ||
                session->barge_state == IBAUDIO_BARGE_IN_SPEECH_CANDIDATE) {
                if (rms_dbfs >= session->barge_threshold_dbfs) {
                    session->barge_accumulated_ms = std::min<uint32_t>(
                        std::numeric_limits<uint32_t>::max() - duration_ms,
                        session->barge_accumulated_ms) + duration_ms;
                    session->barge_state = IBAUDIO_BARGE_IN_SPEECH_CANDIDATE;
                    if (session->barge_accumulated_ms >= session->barge_hold_ms) {
                        session->barge_state = IBAUDIO_BARGE_IN_INTERRUPTED;
                        interrupt = true;
                    }
                } else {
                    session->barge_accumulated_ms = 0u;
                    session->barge_state = IBAUDIO_BARGE_IN_OUTPUT_ACTIVE;
                }
            }
            *out_state = session->barge_state;
        }
        if (interrupt) {
            std::lock_guard<std::mutex> lock(session->active_job_mutex);
            if (session->active_job != nullptr) {
                ibaudio_job_cancel(session->active_job);
            }
        }
        *out_should_interrupt = interrupt ? 1u : 0u;
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_session_get_barge_in_state(
    const ibaudio_session_t *session,
    ibaudio_barge_in_state_t *out_state) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (session == nullptr || out_state == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "session or output state is null");
        }
        std::lock_guard<std::mutex> lock(session->barge_mutex);
        *out_state = session->barge_state;
        return IBAUDIO_STATUS_OK;
    });
}

} /* extern "C" */
