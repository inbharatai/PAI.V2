#include "internal.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <limits>
#include <memory>
#include <stdexcept>

namespace {

ibaudio_status_t acquire_stream_session(ibaudio_session *session, const char *function_name) {
    if (session == nullptr) {
        return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                  function_name, "session handle is null");
    }
    bool expected = false;
    if (!session->busy.compare_exchange_strong(expected, true, std::memory_order_acq_rel)) {
        session->model->runtime->metrics.calls_rejected_busy.fetch_add(1u, std::memory_order_relaxed);
        return ibaudio::set_error(IBAUDIO_STATUS_BUSY, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                  function_name, "session already has an active operation", true);
    }
    return IBAUDIO_STATUS_OK;
}

ibaudio_buffer *text_buffer(ibaudio_runtime *runtime, const std::string &text) {
    auto buffer = std::make_unique<ibaudio_buffer>();
    buffer->kind = IBAUDIO_BUFFER_UTF8;
    buffer->metrics = &runtime->metrics;
    buffer->bytes.assign(text.begin(), text.end());
    buffer->bytes.push_back(0u);
    runtime->metrics.live_owned_buffers.fetch_add(1u, std::memory_order_relaxed);
    return buffer.release();
}

ibaudio_buffer *audio_buffer(ibaudio_runtime *runtime, ibaudio::AudioData data) {
    auto buffer = std::make_unique<ibaudio_buffer>();
    buffer->kind = IBAUDIO_BUFFER_AUDIO_F32;
    buffer->metrics = &runtime->metrics;
    buffer->audio = std::move(data);
    runtime->metrics.live_owned_buffers.fetch_add(1u, std::memory_order_relaxed);
    return buffer.release();
}

void destroy_payload(ibaudio_buffer *buffer) {
    if (buffer == nullptr) return;
    ibaudio::MetricsData *metrics = buffer->metrics;
    delete buffer;
    if (metrics != nullptr) metrics->live_owned_buffers.fetch_sub(1u, std::memory_order_relaxed);
}

void release_session_busy(ibaudio_stream *stream);

void enqueue_event(ibaudio_stream *stream, ibaudio_stream_event_v1 event) {
    if (stream->cancelled && event.type != IBAUDIO_EVENT_CANCELLED) {
        destroy_payload(event.payload);
        return;
    }
    const size_t soft_limit = std::max<uint32_t>(4u, stream->options.max_queued_events);
    if (stream->events.size() >= soft_limit &&
        event.type == IBAUDIO_EVENT_AUDIO_CHUNK && event.payload != nullptr &&
        !stream->events.empty()) {
        auto &queued = stream->events.back();
        if (queued.type == IBAUDIO_EVENT_AUDIO_CHUNK && queued.payload != nullptr &&
            queued.payload->kind == IBAUDIO_BUFFER_AUDIO_F32 &&
            event.payload->kind == IBAUDIO_BUFFER_AUDIO_F32 &&
            queued.end_frame == event.start_frame &&
            queued.payload->audio.sample_rate == event.payload->audio.sample_rate &&
            queued.payload->audio.channels == event.payload->audio.channels) {
            auto &destination = queued.payload->audio;
            auto &source = event.payload->audio;
            if (source.samples.size() <= std::numeric_limits<size_t>::max() - destination.samples.size() &&
                destination.samples.size() + source.samples.size() <= ibaudio::kMaxOwnedBytes / sizeof(float)) {
                try {
                    destination.samples.insert(
                        destination.samples.end(), source.samples.begin(), source.samples.end());
                } catch (...) {
                    destroy_payload(event.payload);
                    throw;
                }
                destination.info.frame_count += source.info.frame_count;
                destination.info.output_peak = std::max(destination.info.output_peak, source.info.output_peak);
                queued.end_frame = event.end_frame;
                ++stream->coalesced_audio_events;
                destroy_payload(event.payload);
                return;
            }
        }
    }

    if (stream->events.size() >= soft_limit) {
        const auto droppable = std::find_if(stream->events.begin(), stream->events.end(), [](const auto &queued) {
            return queued.type == IBAUDIO_EVENT_PARTIAL_TEXT || queued.type == IBAUDIO_EVENT_DIAGNOSTIC;
        });
        if (droppable != stream->events.end()) {
            destroy_payload(droppable->payload);
            stream->events.erase(droppable);
        }
    }

    constexpr size_t hard_limit = 4096u;
    if (stream->events.size() >= hard_limit) {
        for (auto &queued : stream->events) destroy_payload(queued.payload);
        stream->events.clear();
        destroy_payload(event.payload);
        stream->cancelled = true;
        release_session_busy(stream);
        ibaudio_stream_event_v1 cancelled{};
        cancelled.struct_size = sizeof(cancelled);
        cancelled.api_version = IBAUDIO_API_VERSION;
        cancelled.type = IBAUDIO_EVENT_CANCELLED;
        cancelled.is_final = 1u;
        cancelled.sequence = ++stream->sequence;
        stream->events.push_back(cancelled);
        stream->cv.notify_all();
        return;
    }

    event.struct_size = sizeof(event);
    event.api_version = IBAUDIO_API_VERSION;
    event.sequence = ++stream->sequence;
    try {
        stream->events.push_back(event);
    } catch (...) {
        destroy_payload(event.payload);
        throw;
    }
    stream->cv.notify_all();
}

void release_session_busy(ibaudio_stream *stream) {
    if (stream->session_busy_held) {
        stream->session_busy_held = false;
        stream->session->busy.store(false, std::memory_order_release);
    }
}

void emit_diagnostic(ibaudio_stream *stream, const std::string &message) {
    ibaudio_stream_event_v1 event{};
    event.type = IBAUDIO_EVENT_DIAGNOSTIC;
    event.payload = text_buffer(stream->session->model->runtime, message);
    enqueue_event(stream, event);
}

void resample_available(ibaudio_stream *stream, bool flush) {
    if (stream->source_sample_rate == 0u) return;
    const uint64_t source_frames = stream->source_mono.size();
    const long double exact = static_cast<long double>(source_frames) * 16000.0L / stream->source_sample_rate;
    const uint64_t target_count = flush
        ? static_cast<uint64_t>(std::ceil(exact))
        : static_cast<uint64_t>(std::floor(exact));
    while (stream->next_resample_output < target_count) {
        const long double position = static_cast<long double>(stream->next_resample_output) *
                                     stream->source_sample_rate / 16000.0L;
        const uint64_t left = static_cast<uint64_t>(position);
        if (left >= source_frames) break;
        uint64_t right = left + 1u;
        if (right >= source_frames) {
            if (!flush) break;
            right = left;
        }
        const float fraction = static_cast<float>(position - static_cast<long double>(left));
        const float a = stream->source_mono[static_cast<size_t>(left)];
        const float b = stream->source_mono[static_cast<size_t>(right)];
        stream->canonical_mono.push_back(a + (b - a) * fraction);
        ++stream->next_resample_output;
    }
}

float dbfs(double rms) {
    return rms > 1.0e-12 ? static_cast<float>(20.0 * std::log10(rms)) : -120.0f;
}

void process_vad_frames(ibaudio_stream *stream, bool flush) {
    const auto &config = stream->session->vad;
    const uint64_t frame_size = std::max<uint64_t>(1u, 16000ull * config.frame_ms / 1000u);
    const uint64_t hop_size = std::max<uint64_t>(1u, 16000ull * config.hop_ms / 1000u);
    const uint32_t speech_hops = std::max<uint32_t>(1u, (config.min_speech_ms + config.hop_ms - 1u) / config.hop_ms);
    const uint32_t silence_hops = std::max<uint32_t>(1u, (config.min_silence_ms + config.hop_ms - 1u) / config.hop_ms);
    while (stream->next_vad_hop_frame < stream->canonical_mono.size()) {
        const uint64_t start = stream->next_vad_hop_frame;
        if (!flush && start + frame_size > stream->canonical_mono.size()) break;
        const uint64_t end = std::min<uint64_t>(stream->canonical_mono.size(), start + frame_size);
        double energy = 0.0;
        for (uint64_t index = start; index < end; ++index) {
            const double value = stream->canonical_mono[static_cast<size_t>(index)];
            energy += value * value;
        }
        const float level = dbfs(end > start ? std::sqrt(energy / static_cast<double>(end - start)) : 0.0);
        const bool speech = level >= config.threshold_dbfs;
        const float confidence = std::clamp((level - config.threshold_dbfs + 12.0f) / 24.0f, 0.0f, 1.0f);
        if (!stream->vad_active) {
            if (speech) {
                if (stream->vad_speech_run_ms == 0u) {
                    stream->vad_candidate_start = start;
                    stream->vad_max_confidence = confidence;
                    stream->vad_peak_dbfs = level;
                }
                stream->vad_speech_run_ms += config.hop_ms;
                stream->vad_max_confidence = std::max(stream->vad_max_confidence, confidence);
                stream->vad_peak_dbfs = std::max(stream->vad_peak_dbfs, level);
                if (stream->vad_speech_run_ms >= speech_hops * config.hop_ms) {
                    stream->vad_active = true;
                    stream->vad_silence_run_ms = 0u;
                    ibaudio_stream_event_v1 event{};
                    event.type = IBAUDIO_EVENT_VAD_SPEECH_START;
                    event.start_frame = stream->vad_candidate_start;
                    event.end_frame = stream->vad_candidate_start;
                    event.confidence = stream->vad_max_confidence;
                    enqueue_event(stream, event);
                }
            } else {
                stream->vad_speech_run_ms = 0u;
            }
        } else {
            stream->vad_max_confidence = std::max(stream->vad_max_confidence, confidence);
            stream->vad_peak_dbfs = std::max(stream->vad_peak_dbfs, level);
            if (speech) {
                stream->vad_silence_run_ms = 0u;
            } else {
                stream->vad_silence_run_ms += config.hop_ms;
                if (stream->vad_silence_run_ms >= silence_hops * config.hop_ms) {
                    const uint64_t trailing = static_cast<uint64_t>(stream->vad_silence_run_ms) * 16u;
                    const uint64_t segment_end = end > trailing ? end - trailing + frame_size : stream->vad_candidate_start + 1u;
                    ibaudio_stream_event_v1 end_event{};
                    end_event.type = IBAUDIO_EVENT_VAD_SPEECH_END;
                    end_event.start_frame = stream->vad_candidate_start;
                    end_event.end_frame = std::min<uint64_t>(stream->canonical_mono.size(), segment_end);
                    end_event.confidence = stream->vad_max_confidence;
                    enqueue_event(stream, end_event);
                    ibaudio_stream_event_v1 segment = end_event;
                    segment.type = IBAUDIO_EVENT_VAD_SEGMENT;
                    enqueue_event(stream, segment);
                    stream->vad_active = false;
                    stream->vad_speech_run_ms = 0u;
                    stream->vad_silence_run_ms = 0u;
                }
            }
        }
        stream->next_vad_hop_frame += hop_size;
        if (end == stream->canonical_mono.size()) break;
    }
    if (flush && stream->vad_active) {
        ibaudio_stream_event_v1 end_event{};
        end_event.type = IBAUDIO_EVENT_VAD_SPEECH_END;
        end_event.start_frame = stream->vad_candidate_start;
        end_event.end_frame = stream->canonical_mono.size();
        end_event.confidence = stream->vad_max_confidence;
        enqueue_event(stream, end_event);
        ibaudio_stream_event_v1 segment = end_event;
        segment.type = IBAUDIO_EVENT_VAD_SEGMENT;
        enqueue_event(stream, segment);
        stream->vad_active = false;
    }
}

void process_asr_partials(ibaudio_stream *stream) {
    if (stream->options.emit_partial_results == 0u) return;
    while (stream->canonical_mono.size() >= stream->next_asr_partial_frame) {
        ibaudio::AudioData audio;
        audio.samples.assign(stream->canonical_mono.begin(),
            stream->canonical_mono.begin() + static_cast<std::ptrdiff_t>(stream->next_asr_partial_frame));
        audio.sample_rate = 16000u;
        audio.channels = 1u;
        std::string provisional = ibaudio::run_reference_asr(audio);
        provisional += " [provisional]";
        ibaudio_stream_event_v1 event{};
        event.type = IBAUDIO_EVENT_PARTIAL_TEXT;
        event.start_frame = 0u;
        event.end_frame = stream->next_asr_partial_frame;
        event.payload = text_buffer(stream->session->model->runtime, provisional);
        enqueue_event(stream, event);
        stream->next_asr_partial_frame += 3200u;
    }
}

ibaudio_status_t start_common(
    ibaudio_session *session,
    const ibaudio_stream_options_v1 *options,
    ibaudio_stream **out_stream) {
    if (session == nullptr || options == nullptr || out_stream == nullptr ||
        !ibaudio::valid_header(options->struct_size, sizeof(*options), options->api_version)) {
        return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                  "start_common", "session, valid stream options, and output are required");
    }
    *out_stream = nullptr;
    if (!session->streaming_enabled) {
        return ibaudio::set_error(IBAUDIO_STATUS_INVALID_STATE, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                  "start_common", "session was not created in streaming mode");
    }
    if (options->max_queued_events == 0u || options->max_queued_events > 65536u ||
        options->preferred_chunk_frames == 0u) {
        return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                  "start_common", "invalid stream queue or chunk configuration");
    }
    const ibaudio_status_t acquired = acquire_stream_session(session, "start_common");
    if (acquired != IBAUDIO_STATUS_OK) return acquired;
    auto stream = std::make_unique<ibaudio_stream>();
    stream->session = session;
    stream->options = *options;
    session->live_streams.fetch_add(1u, std::memory_order_release);
    session->model->runtime->metrics.streams_started.fetch_add(1u, std::memory_order_relaxed);
    *out_stream = stream.release();
    return IBAUDIO_STATUS_OK;
}

} // namespace

extern "C" {

ibaudio_status_t ibaudio_stream_start(
    ibaudio_session_t *session,
    const ibaudio_stream_options_v1 *options,
    ibaudio_stream_t **out_stream) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (session == nullptr || (session->task != IBAUDIO_TASK_ASR && session->task != IBAUDIO_TASK_VAD)) {
            return ibaudio::set_error(IBAUDIO_STATUS_UNSUPPORTED, IBAUDIO_ERROR_DOMAIN_MODEL,
                                      __func__, "audio-input streams support ASR and VAD sessions");
        }
        return start_common(session, options, out_stream);
    });
}

ibaudio_status_t ibaudio_tts_stream_start(
    ibaudio_session_t *session,
    ibaudio_string_view_v1 text,
    const ibaudio_stream_options_v1 *options,
    ibaudio_stream_t **out_stream) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (session == nullptr || session->task != IBAUDIO_TASK_TTS) {
            return ibaudio::set_error(IBAUDIO_STATUS_UNSUPPORTED, IBAUDIO_ERROR_DOMAIN_MODEL,
                                      __func__, "TTS stream requires a TTS session");
        }
        const std::string input = ibaudio::from_view(text);
        const ibaudio_status_t status = start_common(session, options, out_stream);
        if (status != IBAUDIO_STATUS_OK) return status;
        ibaudio_stream *stream = *out_stream;
        try {
            ibaudio::AudioData audio = ibaudio::run_reference_tts(input);
            const uint64_t chunk_frames = std::max<uint32_t>(1u, options->preferred_chunk_frames);
            for (uint64_t start = 0u; start < audio.samples.size(); start += chunk_frames) {
                const uint64_t count = std::min<uint64_t>(chunk_frames, audio.samples.size() - start);
                ibaudio::AudioData chunk;
                chunk.sample_rate = audio.sample_rate;
                chunk.channels = 1u;
                chunk.samples.assign(audio.samples.begin() + static_cast<std::ptrdiff_t>(start),
                                     audio.samples.begin() + static_cast<std::ptrdiff_t>(start + count));
                chunk.info.struct_size = sizeof(chunk.info);
                chunk.info.api_version = IBAUDIO_API_VERSION;
                chunk.info.sample_rate = audio.sample_rate;
                chunk.info.channels = 1u;
                chunk.info.frame_count = count;
                chunk.info.output_peak = 0.22f;
                chunk.info.applied_gain = 1.0f;
                ibaudio_stream_event_v1 event{};
                event.type = IBAUDIO_EVENT_AUDIO_CHUNK;
                event.start_frame = start;
                event.end_frame = start + count;
                event.payload = audio_buffer(session->model->runtime, std::move(chunk));
                enqueue_event(stream, event);
            }
            session->model->runtime->metrics.audio_frames_out.fetch_add(audio.samples.size(), std::memory_order_relaxed);
            ibaudio_stream_finish(stream);
            return IBAUDIO_STATUS_OK;
        } catch (...) {
            ibaudio_stream_release(out_stream);
            throw;
        }
    });
}

ibaudio_status_t ibaudio_stream_push_audio(
    ibaudio_stream_t *stream,
    const ibaudio_audio_view_v1 *audio) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (stream == nullptr || audio == nullptr ||
            !ibaudio::valid_header(audio->struct_size, sizeof(*audio), audio->api_version)) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "stream and valid audio view are required");
        }
        std::lock_guard<std::mutex> lock(stream->mutex);
        if (stream->finished || stream->cancelled) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_STATE, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                      __func__, "stream is already terminal");
        }
        if (audio->channels == 0u || audio->channels > ibaudio::kMaxChannels ||
            audio->sample_rate < ibaudio::kMinSampleRate || audio->sample_rate > ibaudio::kMaxSampleRate ||
            (audio->frame_count > 0u && audio->interleaved_f32 == nullptr) ||
            audio->frame_count > stream->session->model->runtime->max_input_frames) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_AUDIO,
                                      __func__, "invalid streamed audio shape or sample rate");
        }
        const bool discontinuity = (audio->flags & IBAUDIO_AUDIO_FLAG_DISCONTINUITY) != 0u;
        if (stream->source_sample_rate == 0u || discontinuity) {
            if (discontinuity && stream->source_sample_rate != 0u) {
                emit_diagnostic(stream, "input discontinuity reset streaming analysis state");
                stream->source_mono.clear();
                stream->canonical_mono.clear();
                stream->next_resample_output = 0u;
                stream->next_asr_partial_frame = 3200u;
                stream->next_vad_hop_frame = 0u;
                stream->vad_active = false;
                stream->vad_speech_run_ms = 0u;
                stream->vad_silence_run_ms = 0u;
            }
            stream->source_sample_rate = audio->sample_rate;
            stream->source_channels = audio->channels;
            stream->expected_start_frame = audio->start_frame;
            stream->has_expected_start = true;
        }
        if (audio->sample_rate != stream->source_sample_rate || audio->channels != stream->source_channels) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_AUDIO,
                                      __func__, "stream sample rate/channels changed without discontinuity");
        }
        if (stream->has_expected_start && audio->start_frame != stream->expected_start_frame && !discontinuity) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_AUDIO,
                                      __func__, "non-contiguous start_frame requires DISCONTINUITY flag");
        }
        if (audio->frame_count > std::numeric_limits<size_t>::max() / audio->channels ||
            stream->source_mono.size() + audio->frame_count > stream->session->model->runtime->max_input_frames) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_AUDIO,
                                      __func__, "stream exceeds configured input-frame limit");
        }
        for (uint64_t frame = 0; frame < audio->frame_count; ++frame) {
            double mono = 0.0;
            for (uint32_t channel = 0; channel < audio->channels; ++channel) {
                float value = audio->interleaved_f32[static_cast<size_t>(frame) * audio->channels + channel];
                if (!std::isfinite(value)) value = 0.0f;
                mono += value;
            }
            stream->source_mono.push_back(static_cast<float>(mono / audio->channels));
        }
        stream->expected_start_frame = audio->start_frame + audio->frame_count;
        resample_available(stream, (audio->flags & IBAUDIO_AUDIO_FLAG_END_OF_INPUT) != 0u);
        if (stream->session->task == IBAUDIO_TASK_ASR) process_asr_partials(stream);
        if (stream->session->task == IBAUDIO_TASK_VAD) process_vad_frames(stream, false);
        stream->session->model->runtime->metrics.audio_frames_in.fetch_add(audio->frame_count, std::memory_order_relaxed);
        if ((audio->flags & IBAUDIO_AUDIO_FLAG_END_OF_INPUT) != 0u) {
            /* Finish outside the recursive call path by finalizing inline. */
            resample_available(stream, true);
        }
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_stream_finish(ibaudio_stream_t *stream) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (stream == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "stream handle is null");
        }
        std::lock_guard<std::mutex> lock(stream->mutex);
        if (stream->cancelled) {
            return ibaudio::set_error(IBAUDIO_STATUS_CANCELLED, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                      __func__, "stream was cancelled", true);
        }
        if (stream->finished) return IBAUDIO_STATUS_OK;
        resample_available(stream, true);
        if (stream->session->task == IBAUDIO_TASK_ASR) {
            ibaudio::AudioData audio;
            audio.samples = stream->canonical_mono;
            audio.sample_rate = 16000u;
            audio.channels = 1u;
            const std::string final_text = ibaudio::run_reference_asr(audio);
            ibaudio_stream_event_v1 event{};
            event.type = IBAUDIO_EVENT_FINAL_TEXT;
            event.is_final = 1u;
            event.start_frame = 0u;
            event.end_frame = audio.samples.size();
            event.payload = text_buffer(stream->session->model->runtime, final_text);
            enqueue_event(stream, event);
        } else if (stream->session->task == IBAUDIO_TASK_VAD) {
            process_vad_frames(stream, true);
        }
        ibaudio_stream_event_v1 terminal{};
        terminal.type = IBAUDIO_EVENT_FINAL;
        terminal.is_final = 1u;
        terminal.start_frame = 0u;
        terminal.end_frame = stream->canonical_mono.size();
        enqueue_event(stream, terminal);
        stream->finished = true;
        release_session_busy(stream);
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_stream_cancel(ibaudio_stream_t *stream) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (stream == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "stream handle is null");
        }
        std::lock_guard<std::mutex> lock(stream->mutex);
        if (stream->cancelled || stream->finished) return IBAUDIO_STATUS_OK;
        stream->cancelled = true;
        ibaudio_stream_event_v1 event{};
        event.type = IBAUDIO_EVENT_CANCELLED;
        event.is_final = 1u;
        enqueue_event(stream, event);
        release_session_busy(stream);
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_stream_poll_event(
    ibaudio_stream_t *stream,
    uint32_t timeout_ms,
    ibaudio_stream_event_v1 *out_event) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (stream == nullptr || out_event == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "stream and output event are required");
        }
        *out_event = {};
        std::unique_lock<std::mutex> lock(stream->mutex);
        const bool ready = stream->cv.wait_for(lock, std::chrono::milliseconds(timeout_ms), [&]() {
            return !stream->events.empty() || stream->terminal_polled;
        });
        if (!ready || stream->events.empty()) {
            if (stream->terminal_polled) {
                return ibaudio::set_error(IBAUDIO_STATUS_INVALID_STATE, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                          __func__, "terminal event was already consumed");
            }
            return ibaudio::set_error(timeout_ms == 0u ? IBAUDIO_STATUS_WOULD_BLOCK : IBAUDIO_STATUS_TIMEOUT,
                                      IBAUDIO_ERROR_DOMAIN_LIFECYCLE, __func__, "no stream event available", true);
        }
        *out_event = stream->events.front();
        stream->events.pop_front();
        if (out_event->type == IBAUDIO_EVENT_FINAL || out_event->type == IBAUDIO_EVENT_CANCELLED) {
            stream->terminal_polled = true;
        }
        return IBAUDIO_STATUS_OK;
    });
}

void ibaudio_stream_event_release(ibaudio_stream_event_v1 *event) {
    if (event == nullptr) return;
    if (event->payload != nullptr) ibaudio_buffer_release(&event->payload);
    *event = {};
}

ibaudio_status_t ibaudio_stream_release(ibaudio_stream_t **stream) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (stream == nullptr || *stream == nullptr) return IBAUDIO_STATUS_OK;
        ibaudio_stream *value = *stream;
        {
            std::lock_guard<std::mutex> lock(value->mutex);
            if (!value->finished && !value->cancelled) value->cancelled = true;
            release_session_busy(value);
            for (auto &event : value->events) destroy_payload(event.payload);
            value->events.clear();
        }
        ibaudio_session *session = value->session;
        delete value;
        *stream = nullptr;
        if (session != nullptr) session->live_streams.fetch_sub(1u, std::memory_order_release);
        return IBAUDIO_STATUS_OK;
    });
}

} /* extern "C" */
