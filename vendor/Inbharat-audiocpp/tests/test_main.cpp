#include "inbharat/ibaudio.h"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <functional>
#include <iostream>
#include <limits>
#include <random>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

namespace {

constexpr double kPi = 3.141592653589793238462643383279502884;
const std::filesystem::path kSource(IBAUDIO_TEST_SOURCE_DIR);
const std::filesystem::path kBinary(IBAUDIO_TEST_BINARY_DIR);

#define CHECK(condition) do { if (!(condition)) throw std::runtime_error(std::string("CHECK failed: ") + #condition + " at " + __FILE__ + ":" + std::to_string(__LINE__)); } while (false)

std::string error_text() {
    ibaudio_error_info_v1 error{};
    ibaudio_error_get_last(&error);
    return std::string(ibaudio_status_string(error.status)) + " " + error.function_name + ": " + error.message;
}

void ok(ibaudio_status_t status) {
    if (status != IBAUDIO_STATUS_OK) throw std::runtime_error(error_text());
}

ibaudio_string_view_v1 view(const std::string &text) {
    return {sizeof(ibaudio_string_view_v1), IBAUDIO_API_VERSION, text.data(), text.size()};
}

struct Runtime {
    ibaudio_runtime_t *p = nullptr;
    Runtime() {
        ibaudio_runtime_options_v1 options{};
        ibaudio_runtime_options_init(&options);
        const std::string cache = (kBinary / "cache").string();
        options.cache_directory = view(cache);
        options.max_input_frames = 16000u * 60u * 5u;
        ok(ibaudio_runtime_create(&options, &p));
    }
    Runtime(const Runtime &) = delete;
    ~Runtime() { if (p != nullptr) ok(ibaudio_runtime_release(&p)); }
};

struct Model {
    ibaudio_model_t *p = nullptr;
    Model(ibaudio_runtime_t *runtime, const std::string &id) {
        ibaudio_model_load_options_v1 options{};
        ibaudio_model_load_options_init(&options);
        options.model_id = view(id);
        ok(ibaudio_model_load(runtime, &options, &p));
    }
    Model(const Model &) = delete;
    ~Model() { if (p != nullptr) ok(ibaudio_model_release(&p)); }
};

struct Session {
    ibaudio_session_t *p = nullptr;
    Session(ibaudio_model_t *model, ibaudio_task_t task, bool streaming = false, float threshold = -42.0f) {
        ibaudio_session_options_v1 options{};
        ibaudio_session_options_init(&options);
        options.task = task;
        options.streaming = streaming ? 1u : 0u;
        options.vad_threshold_dbfs = threshold;
        ok(ibaudio_session_create(model, &options, &p));
    }
    Session(const Session &) = delete;
    ~Session() { if (p != nullptr) ok(ibaudio_session_release(&p)); }
};

struct Buffer {
    ibaudio_buffer_t *p = nullptr;
    Buffer() = default;
    explicit Buffer(ibaudio_buffer_t *value) : p(value) {}
    Buffer(const Buffer &) = delete;
    Buffer &operator=(const Buffer &) = delete;
    Buffer(Buffer &&other) noexcept : p(other.p) { other.p = nullptr; }
    Buffer &operator=(Buffer &&other) noexcept {
        if (this != &other) {
            ibaudio_buffer_release(&p);
            p = other.p;
            other.p = nullptr;
        }
        return *this;
    }
    ~Buffer() { ibaudio_buffer_release(&p); }
};

struct Job {
    ibaudio_job_t *p = nullptr;
    Job() = default;
    Job(const Job &) = delete;
    ~Job() { ibaudio_job_release(&p); }
};

struct Stream {
    ibaudio_stream_t *p = nullptr;
    Stream() = default;
    Stream(const Stream &) = delete;
    ~Stream() { ibaudio_stream_release(&p); }
};

std::vector<uint8_t> bytes(const std::filesystem::path &path) {
    std::ifstream input(path, std::ios::binary);
    CHECK(input.good());
    return std::vector<uint8_t>(std::istreambuf_iterator<char>(input), {});
}

std::string text_file(const std::filesystem::path &path) {
    std::ifstream input(path);
    CHECK(input.good());
    std::string value((std::istreambuf_iterator<char>(input)), {});
    if (!value.empty() && value.back() == '\n') value.pop_back();
    return value;
}

std::string buffer_text(ibaudio_buffer_t *buffer) {
    const void *data = nullptr;
    uint64_t size = 0u;
    ok(ibaudio_buffer_get_data(buffer, &data, &size));
    return std::string(static_cast<const char *>(data), static_cast<size_t>(size));
}

Buffer decode(ibaudio_runtime_t *runtime, const std::filesystem::path &path) {
    auto content = bytes(path);
    ibaudio_buffer_t *result = nullptr;
    ok(ibaudio_wav_decode_memory(runtime, content.data(), content.size(), &result));
    return Buffer(result);
}

ibaudio_audio_view_v1 audio_view(ibaudio_buffer_t *buffer) {
    ibaudio_audio_view_v1 audio{};
    ok(ibaudio_buffer_get_audio_view(buffer, &audio));
    return audio;
}

std::vector<float> tone(uint32_t frames, uint32_t sample_rate, float amplitude = 0.25f, float hz = 440.0f) {
    std::vector<float> values(frames);
    for (uint32_t index = 0; index < frames; ++index) {
        values[index] = amplitude * static_cast<float>(std::sin(2.0 * kPi * hz * index / sample_rate));
    }
    return values;
}

ibaudio_audio_view_v1 make_view(const std::vector<float> &samples, uint32_t rate = 16000u, uint32_t channels = 1u) {
    ibaudio_audio_view_v1 audio{};
    audio.struct_size = sizeof(audio);
    audio.api_version = IBAUDIO_API_VERSION;
    audio.interleaved_f32 = samples.data();
    audio.frame_count = samples.size() / channels;
    audio.sample_rate = rate;
    audio.channels = channels;
    return audio;
}

void test_unit() {
    CHECK(ibaudio_get_api_version() == IBAUDIO_API_VERSION);
    CHECK(std::string(ibaudio_get_runtime_version()) == IBAUDIO_RUNTIME_VERSION);
    CHECK(std::string(ibaudio_status_string(IBAUDIO_STATUS_BUSY)) == "BUSY");

    Runtime runtime;
    ibaudio_capabilities_v1 capabilities{};
    ok(ibaudio_runtime_get_capabilities(runtime.p, &capabilities));
    CHECK(capabilities.abi_major == 1u);
    CHECK(capabilities.model_count == 4u);
    CHECK((capabilities.feature_flags & IBAUDIO_CAP_CANCELLATION) != 0u);
    uint32_t backend_count = 0u;
    ok(ibaudio_runtime_get_backend_count(runtime.p, &backend_count));
    CHECK(backend_count >= 8u);
    ibaudio_backend_info_v1 cpu{};
    ok(ibaudio_runtime_get_backend_info(runtime.p, 0u, &cpu));
    CHECK(cpu.backend == IBAUDIO_BACKEND_CPU && cpu.selected == 1u && cpu.compiled == 1u);
    ibaudio_backend_info_v1 vulkan{};
    ok(ibaudio_runtime_get_backend_info(runtime.p, 3u, &vulkan));
    CHECK(vulkan.selected == 0u);
    CHECK(vulkan.availability != IBAUDIO_BACKEND_AVAILABLE);
    {
        ibaudio_runtime_options_v1 backend_options{};
        ibaudio_runtime_options_init(&backend_options);
        backend_options.requested_backend = IBAUDIO_BACKEND_VULKAN;
        backend_options.allow_auto_cpu_fallback = 0u;
        ibaudio_runtime_t *explicit_runtime = nullptr;
        CHECK(ibaudio_runtime_create(&backend_options, &explicit_runtime) == IBAUDIO_STATUS_UNAVAILABLE);
        CHECK(explicit_runtime == nullptr);
        backend_options.allow_auto_cpu_fallback = 1u;
        ok(ibaudio_runtime_create(&backend_options, &explicit_runtime));
        ibaudio_metrics_v1 fallback_metrics{};
        ok(ibaudio_runtime_get_metrics(explicit_runtime, &fallback_metrics));
        CHECK(fallback_metrics.backend_fallbacks == 1u);
        ok(ibaudio_runtime_release(&explicit_runtime));
    }

    Buffer diagnostics;
    ok(ibaudio_runtime_get_diagnostics_json(runtime.p, &diagnostics.p));
    CHECK(buffer_text(diagnostics.p).find("\"selected_backend\":\"cpu\"") != std::string::npos);
    diagnostics = Buffer();

    uint32_t model_count = 0u;
    ok(ibaudio_runtime_get_model_count(runtime.p, &model_count));
    CHECK(model_count == 4u);
    ibaudio_model_descriptor_v1 descriptor{};
    ok(ibaudio_runtime_get_model_descriptor(runtime.p, 0u, &descriptor));
    CHECK(std::string(descriptor.id) == "reference-asr-v1");
    CHECK(std::strlen(descriptor.artifact_sha256) == 64u);
    CHECK(std::string(descriptor.spdx_license) == "Apache-2.0");
    ok(ibaudio_runtime_get_model_descriptor(runtime.p, 3u, &descriptor));
    CHECK(descriptor.task == IBAUDIO_TASK_KWS && descriptor.available == 0u);
    ibaudio_model_load_options_v1 deferred{};
    ibaudio_model_load_options_init(&deferred);
    const std::string kws = "kws-deferred-v1";
    deferred.model_id = view(kws);
    ibaudio_model_t *not_loaded = nullptr;
    CHECK(ibaudio_model_load(runtime.p, &deferred, &not_loaded) == IBAUDIO_STATUS_DEFERRED);
    CHECK(not_loaded == nullptr);

    Model asr(runtime.p, "reference-asr-v1");
    Session asr_session(asr.p, IBAUDIO_TASK_ASR);
    Buffer speech = decode(runtime.p, kSource / "fixtures/speech_440hz_16k_mono.wav");
    ibaudio_audio_view_v1 speech_view = audio_view(speech.p);
    Buffer transcript;
    ok(ibaudio_session_run_asr(asr_session.p, &speech_view, &transcript.p));
    CHECK(buffer_text(transcript.p) == text_file(kSource / "expected/asr_speech.txt"));

    Buffer silence = decode(runtime.p, kSource / "fixtures/silence_16k.wav");
    ibaudio_audio_view_v1 silence_view = audio_view(silence.p);
    Buffer silence_text;
    ok(ibaudio_session_run_asr(asr_session.p, &silence_view, &silence_text.p));
    CHECK(buffer_text(silence_text.p) == text_file(kSource / "expected/asr_silence.txt"));

    Model vad_model(runtime.p, "energy-vad-v1");
    Session vad(vad_model.p, IBAUDIO_TASK_VAD);
    Buffer segments;
    ok(ibaudio_session_run_vad(vad.p, &speech_view, &segments.p));
    const void *segment_data = nullptr;
    uint64_t segment_bytes = 0u;
    ok(ibaudio_buffer_get_data(segments.p, &segment_data, &segment_bytes));
    CHECK(segment_bytes == sizeof(ibaudio_vad_segment_v1));
    const auto *segment = static_cast<const ibaudio_vad_segment_v1 *>(segment_data);
    CHECK(segment->start_frame == 3840u);
    CHECK(segment->end_frame == 20480u);
    CHECK(segment->confidence > 0.99f);

    Model tts_model(runtime.p, "reference-tts-v1");
    Session tts(tts_model.p, IBAUDIO_TASK_TTS);
    const std::string namaste = "namaste";
    Buffer tts_audio;
    ok(ibaudio_session_run_tts(tts.p, view(namaste), &tts_audio.p));
    ibaudio_audio_view_v1 tts_view = audio_view(tts_audio.p);
    CHECK(tts_view.sample_rate == 24000u && tts_view.channels == 1u && tts_view.frame_count == 9360u);
    Buffer tts_wav;
    ok(ibaudio_wav_encode_pcm16(runtime.p, &tts_view, &tts_wav.p));
    const void *wav_data = nullptr;
    uint64_t wav_size = 0u;
    ok(ibaudio_buffer_get_data(tts_wav.p, &wav_data, &wav_size));
    CHECK(wav_size == bytes(kSource / "expected/reference_tts_namaste.wav").size());
    const auto expected_wav = bytes(kSource / "expected/reference_tts_namaste.wav");
    CHECK(std::memcmp(wav_data, expected_wav.data(), expected_wav.size()) == 0);

    Buffer stereo = decode(runtime.p, kSource / "fixtures/stereo_48k.wav");
    ibaudio_audio_view_v1 stereo_view = audio_view(stereo.p);
    ibaudio_audio_process_options_v1 process{};
    ibaudio_audio_process_options_init(&process);
    process.target_sample_rate = 16000u;
    process.target_channels = 1u;
    process.gain_db = 6.0f;
    process.normalize_peak = 0.5f;
    process.clip_peak = 0.5f;
    Buffer normalized;
    ok(ibaudio_audio_process(runtime.p, &stereo_view, &process, &normalized.p));
    ibaudio_audio_info_v1 info{};
    ok(ibaudio_buffer_get_audio_info(normalized.p, &info));
    CHECK(info.sample_rate == 16000u && info.channels == 1u && info.frame_count == 8000u);
    CHECK(info.output_peak <= 0.50001f && info.output_peak > 0.49f);

    std::vector<float> non_finite = {0.0f, std::numeric_limits<float>::quiet_NaN(),
                                     std::numeric_limits<float>::infinity(), 2.0f};
    ibaudio_audio_view_v1 bad_samples = make_view(non_finite);
    Buffer sanitized;
    ibaudio_audio_process_options_init(&process);
    ok(ibaudio_audio_process(runtime.p, &bad_samples, &process, &sanitized.p));
    ok(ibaudio_buffer_get_audio_info(sanitized.p, &info));
    CHECK(info.sanitized_samples == 2u && info.clipped_samples == 1u);

    const std::filesystem::path hash_file = kBinary / "sha256-abc.txt";
    { std::ofstream output(hash_file, std::ios::binary); output << "abc"; }
    char digest[65]{};
    const std::string hash_path = hash_file.string();
    CHECK(ibaudio_sha256_file(runtime.p, view(hash_path), digest) == IBAUDIO_STATUS_SECURITY_ERROR);

    {
        const std::filesystem::path allowed = kBinary / "allowed-models";
        const std::filesystem::path allowed_cache = kBinary / "restricted-cache";
        std::filesystem::create_directories(allowed);
        const std::filesystem::path artifact = allowed / "artifact.bin";
        { std::ofstream output(artifact, std::ios::binary); output << "abc"; }
        const std::string allowed_text = allowed.string();
        const std::string cache_text = allowed_cache.string();
        ibaudio_runtime_options_v1 restricted_options{};
        ibaudio_runtime_options_init(&restricted_options);
        restricted_options.allowed_model_root = view(allowed_text);
        restricted_options.cache_directory = view(cache_text);
        ibaudio_runtime_t *restricted = nullptr;
        ok(ibaudio_runtime_create(&restricted_options, &restricted));
        const std::string artifact_text = artifact.string();
        ok(ibaudio_sha256_file(restricted, view(artifact_text), digest));
        CHECK(std::string(digest) == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        CHECK(ibaudio_sha256_file(restricted, view(hash_path), digest) == IBAUDIO_STATUS_SECURITY_ERROR);
        ibaudio_model_load_options_v1 load{};
        ibaudio_model_load_options_init(&load);
        const std::string model_id = "reference-asr-v1";
        const std::string expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        const std::string outside = hash_file.string();
        load.model_id = view(model_id);
        load.artifact_path = view(outside);
        load.expected_sha256 = view(expected);
        ibaudio_model_t *external = nullptr;
        CHECK(ibaudio_model_load(restricted, &load, &external) == IBAUDIO_STATUS_SECURITY_ERROR);
        load.artifact_path = view(artifact_text);
        const std::string wrong(64u, '0');
        load.expected_sha256 = view(wrong);
        CHECK(ibaudio_model_load(restricted, &load, &external) == IBAUDIO_STATUS_INTEGRITY_ERROR);
        load.expected_sha256 = view(expected);
        ok(ibaudio_model_load(restricted, &load, &external));
        ibaudio_model_descriptor_v1 external_descriptor{};
        ok(ibaudio_model_get_descriptor(external, &external_descriptor));
        CHECK(std::string(external_descriptor.artifact_sha256) == expected);
        ok(ibaudio_model_release(&external));
        ok(ibaudio_runtime_release(&restricted));
    }

    ibaudio_metrics_v1 metrics{};
    ok(ibaudio_runtime_get_metrics(runtime.p, &metrics));
    CHECK(metrics.models_loaded >= 3u && metrics.sessions_created >= 3u);
    CHECK(metrics.audio_frames_in > 0u && metrics.audio_frames_out > 0u);
    CHECK(metrics.errors_reported > 0u);
}

void drain_nonterminal(ibaudio_stream_t *stream, std::vector<std::string> *partials = nullptr) {
    while (true) {
        ibaudio_stream_event_v1 event{};
        const ibaudio_status_t status = ibaudio_stream_poll_event(stream, 0u, &event);
        if (status == IBAUDIO_STATUS_WOULD_BLOCK) return;
        ok(status);
        if (event.type == IBAUDIO_EVENT_PARTIAL_TEXT && partials != nullptr) {
            partials->push_back(buffer_text(event.payload));
        }
        ibaudio_stream_event_release(&event);
    }
}

void test_streaming() {
    Runtime runtime;
    Buffer speech = decode(runtime.p, kSource / "fixtures/speech_440hz_16k_mono.wav");
    ibaudio_audio_view_v1 source = audio_view(speech.p);
    Model asr_model(runtime.p, "reference-asr-v1");
    Session asr(asr_model.p, IBAUDIO_TASK_ASR, true);
    ibaudio_stream_options_v1 options{};
    ibaudio_stream_options_init(&options);
    options.preferred_chunk_frames = 777u;
    options.max_queued_events = 8u;
    Stream stream;
    ok(ibaudio_stream_start(asr.p, &options, &stream.p));
    std::vector<std::string> partials;
    for (uint64_t start = 0u; start < source.frame_count; start += 777u) {
        ibaudio_audio_view_v1 chunk = source;
        chunk.start_frame = start;
        chunk.frame_count = std::min<uint64_t>(777u, source.frame_count - start);
        chunk.interleaved_f32 = source.interleaved_f32 + start;
        ok(ibaudio_stream_push_audio(stream.p, &chunk));
        drain_nonterminal(stream.p, &partials);
    }
    CHECK(!partials.empty());
    CHECK(partials.front().find("[provisional]") != std::string::npos);
    ok(ibaudio_stream_finish(stream.p));
    std::string final_text;
    bool terminal = false;
    while (!terminal) {
        ibaudio_stream_event_v1 event{};
        ok(ibaudio_stream_poll_event(stream.p, 100u, &event));
        if (event.type == IBAUDIO_EVENT_FINAL_TEXT) final_text = buffer_text(event.payload);
        terminal = event.type == IBAUDIO_EVENT_FINAL;
        ibaudio_stream_event_release(&event);
    }
    CHECK(final_text == text_file(kSource / "expected/asr_speech.txt"));
    ibaudio_stream_event_v1 after{};
    CHECK(ibaudio_stream_poll_event(stream.p, 0u, &after) == IBAUDIO_STATUS_INVALID_STATE);

    Session gap_session(asr_model.p, IBAUDIO_TASK_ASR, true);
    Stream gap;
    ok(ibaudio_stream_start(gap_session.p, &options, &gap.p));
    ibaudio_audio_view_v1 first = source;
    first.frame_count = 100u;
    first.start_frame = 0u;
    ok(ibaudio_stream_push_audio(gap.p, &first));
    first.start_frame = 101u;
    CHECK(ibaudio_stream_push_audio(gap.p, &first) == IBAUDIO_STATUS_INVALID_ARGUMENT);
    first.flags = IBAUDIO_AUDIO_FLAG_DISCONTINUITY;
    ok(ibaudio_stream_push_audio(gap.p, &first));
    ok(ibaudio_stream_cancel(gap.p));
    ok(ibaudio_stream_cancel(gap.p));
    CHECK(ibaudio_stream_finish(gap.p) == IBAUDIO_STATUS_CANCELLED);
    ibaudio_error_info_v1 cancelled_error{};
    ok(ibaudio_error_get_last(&cancelled_error));
    CHECK(cancelled_error.status == IBAUDIO_STATUS_CANCELLED);

    Model vad_model(runtime.p, "energy-vad-v1");
    Session vad(vad_model.p, IBAUDIO_TASK_VAD, true);
    Stream vad_stream;
    ok(ibaudio_stream_start(vad.p, &options, &vad_stream.p));
    for (uint64_t start = 0u; start < source.frame_count; start += 503u) {
        ibaudio_audio_view_v1 chunk = source;
        chunk.start_frame = start;
        chunk.frame_count = std::min<uint64_t>(503u, source.frame_count - start);
        chunk.interleaved_f32 = source.interleaved_f32 + start;
        ok(ibaudio_stream_push_audio(vad_stream.p, &chunk));
    }
    ok(ibaudio_stream_finish(vad_stream.p));
    uint32_t starts = 0u, ends = 0u, segment_count = 0u;
    terminal = false;
    while (!terminal) {
        ibaudio_stream_event_v1 event{};
        ok(ibaudio_stream_poll_event(vad_stream.p, 100u, &event));
        if (event.type == IBAUDIO_EVENT_VAD_SPEECH_START) ++starts;
        if (event.type == IBAUDIO_EVENT_VAD_SPEECH_END) ++ends;
        if (event.type == IBAUDIO_EVENT_VAD_SEGMENT) ++segment_count;
        terminal = event.type == IBAUDIO_EVENT_FINAL;
        ibaudio_stream_event_release(&event);
    }
    CHECK(starts == 1u && ends == 1u && segment_count == 1u);

    Model tts_model(runtime.p, "reference-tts-v1");
    Session tts(tts_model.p, IBAUDIO_TASK_TTS, true);
    Stream tts_stream;
    const std::string phrase = "namaste";
    ok(ibaudio_tts_stream_start(tts.p, view(phrase), &options, &tts_stream.p));
    uint64_t audio_frames = 0u;
    terminal = false;
    while (!terminal) {
        ibaudio_stream_event_v1 event{};
        ok(ibaudio_stream_poll_event(tts_stream.p, 100u, &event));
        if (event.type == IBAUDIO_EVENT_AUDIO_CHUNK) {
            ibaudio_audio_view_v1 chunk = audio_view(event.payload);
            audio_frames += chunk.frame_count;
        }
        terminal = event.type == IBAUDIO_EVENT_FINAL;
        ibaudio_stream_event_release(&event);
    }
    CHECK(audio_frames == 9360u);

    Session bounded_tts(tts_model.p, IBAUDIO_TASK_TTS, true);
    ibaudio_stream_options_v1 bounded_options{};
    ibaudio_stream_options_init(&bounded_options);
    bounded_options.preferred_chunk_frames = 8u;
    bounded_options.max_queued_events = 4u;
    Stream bounded_stream;
    const std::string long_phrase(64u, 'a');
    ok(ibaudio_tts_stream_start(
        bounded_tts.p, view(long_phrase), &bounded_options, &bounded_stream.p));
    uint32_t event_count = 0u;
    uint64_t bounded_frames = 0u;
    terminal = false;
    while (!terminal) {
        ibaudio_stream_event_v1 event{};
        ok(ibaudio_stream_poll_event(bounded_stream.p, 100u, &event));
        ++event_count;
        if (event.type == IBAUDIO_EVENT_AUDIO_CHUNK) {
            bounded_frames += audio_view(event.payload).frame_count;
        }
        terminal = event.type == IBAUDIO_EVENT_FINAL;
        ibaudio_stream_event_release(&event);
    }
    CHECK(event_count <= 5u);
    CHECK(bounded_frames == 960u + 64u * 1200u);
}

void test_lifecycle() {
    ibaudio_runtime_release(nullptr);
    ibaudio_model_release(nullptr);
    ibaudio_session_release(nullptr);
    ibaudio_buffer_release(nullptr);
    ibaudio_job_release(nullptr);
    ibaudio_stream_release(nullptr);
    Runtime runtime;
    {
        Model parent_model(runtime.p, "reference-asr-v1");
        Session child_session(parent_model.p, IBAUDIO_TASK_ASR);
        CHECK(ibaudio_model_release(&parent_model.p) == IBAUDIO_STATUS_BUSY);
        CHECK(parent_model.p != nullptr);
        CHECK(ibaudio_runtime_release(&runtime.p) == IBAUDIO_STATUS_BUSY);
        CHECK(runtime.p != nullptr);
    }
    {
        Buffer owned;
        ok(ibaudio_runtime_get_diagnostics_json(runtime.p, &owned.p));
        CHECK(ibaudio_runtime_release(&runtime.p) == IBAUDIO_STATUS_BUSY);
        CHECK(runtime.p != nullptr);
    }
    ok(ibaudio_runtime_reset_metrics(runtime.p));
    for (uint32_t iteration = 0u; iteration < 10000u; ++iteration) {
        ibaudio_model_load_options_v1 model_options{};
        ibaudio_model_load_options_init(&model_options);
        const std::string id = "reference-asr-v1";
        model_options.model_id = view(id);
        ibaudio_model_t *model = nullptr;
        ok(ibaudio_model_load(runtime.p, &model_options, &model));
        ibaudio_session_options_v1 session_options{};
        ibaudio_session_options_init(&session_options);
        session_options.task = IBAUDIO_TASK_ASR;
        ibaudio_session_t *session = nullptr;
        ok(ibaudio_session_create(model, &session_options, &session));
        ok(ibaudio_session_reset(session));
        ok(ibaudio_session_release(&session));
        ok(ibaudio_session_release(&session));
        ok(ibaudio_model_release(&model));
        ok(ibaudio_model_release(&model));
    }
    ibaudio_metrics_v1 metrics{};
    ok(ibaudio_runtime_get_metrics(runtime.p, &metrics));
    CHECK(metrics.models_loaded == 10000u && metrics.sessions_created == 10000u);
    CHECK(metrics.model_cache_hits >= 9999u);
}

void test_concurrency() {
    Runtime runtime;
    Model model(runtime.p, "reference-tts-v1");
    Session session(model.p, IBAUDIO_TASK_TTS);
    const std::string long_text(16384u, 'a');
    Job job;
    ok(ibaudio_job_start_tts(session.p, view(long_text), &job.p));
    ibaudio_buffer_t *second = nullptr;
    CHECK(ibaudio_session_run_tts(session.p, view(long_text), &second) == IBAUDIO_STATUS_BUSY);
    CHECK(second == nullptr);
    Job duplicate_job;
    CHECK(ibaudio_job_start_tts(session.p, view(long_text), &duplicate_job.p) ==
          IBAUDIO_STATUS_BUSY);
    CHECK(duplicate_job.p == nullptr);
    ok(ibaudio_job_cancel(job.p));
    const ibaudio_status_t settled = ibaudio_job_wait(job.p, 10000u);
    CHECK(settled == IBAUDIO_STATUS_CANCELLED || settled == IBAUDIO_STATUS_OK);

    constexpr size_t kThreads = 8u;
    std::atomic<uint32_t> failures{0u};
    std::vector<std::thread> workers;
    for (size_t thread_index = 0u; thread_index < kThreads; ++thread_index) {
        workers.emplace_back([&]() {
            try {
                Session own(model.p, IBAUDIO_TASK_TTS);
                for (uint32_t iteration = 0u; iteration < 20u; ++iteration) {
                    const std::string text = "parallel";
                    Buffer result;
                    ok(ibaudio_session_run_tts(own.p, view(text), &result.p));
                }
            } catch (...) {
                failures.fetch_add(1u, std::memory_order_relaxed);
            }
        });
    }
    for (auto &worker : workers) worker.join();
    CHECK(failures.load() == 0u);
}

void test_cancellation() {
    Runtime runtime;
    Model model(runtime.p, "reference-tts-v1");
    Session session(model.p, IBAUDIO_TASK_TTS);
    const std::string long_text(16384u, 'z');
    for (uint32_t iteration = 0u; iteration < 100u; ++iteration) {
        Job job;
        ok(ibaudio_job_start_tts(session.p, view(long_text), &job.p));
        ok(ibaudio_job_cancel(job.p));
        ok(ibaudio_job_cancel(job.p));
        const ibaudio_status_t status = ibaudio_job_wait(job.p, 10000u);
        CHECK(status == IBAUDIO_STATUS_CANCELLED);
        Buffer result;
        CHECK(ibaudio_job_take_result(job.p, &result.p) == IBAUDIO_STATUS_CANCELLED);
    }
    ok(ibaudio_session_set_playback_active(session.p, 1u));
    Job barge_job;
    ok(ibaudio_job_start_tts(session.p, view(long_text), &barge_job.p));
    ibaudio_barge_in_state_t state = IBAUDIO_BARGE_IN_IDLE;
    uint32_t interrupt = 0u;
    ok(ibaudio_session_report_input_level(session.p, -10.0f, 200u, &state, &interrupt));
    CHECK(state == IBAUDIO_BARGE_IN_INTERRUPTED && interrupt == 1u);
    CHECK(ibaudio_job_wait(barge_job.p, 10000u) == IBAUDIO_STATUS_CANCELLED);
    ibaudio_metrics_v1 metrics{};
    ok(ibaudio_runtime_get_metrics(runtime.p, &metrics));
    CHECK(metrics.jobs_cancelled >= 101u);
}

void test_malformed() {
    Runtime runtime;
    const std::vector<std::string> names = {
        "malformed_empty.wav", "malformed_riff.wav", "malformed_truncated_data.wav", "malformed_bad_align.wav"};
    for (const auto &name : names) {
        const auto content = bytes(kSource / "fixtures" / name);
        ibaudio_buffer_t *output = nullptr;
        const ibaudio_status_t status = ibaudio_wav_decode_memory(runtime.p,
            content.empty() ? nullptr : content.data(), content.size(), &output);
        CHECK(status == IBAUDIO_STATUS_INVALID_ARGUMENT);
        CHECK(output == nullptr);
        ibaudio_error_info_v1 error{};
        ok(ibaudio_error_get_last(&error));
        CHECK(error.status == IBAUDIO_STATUS_INVALID_ARGUMENT);
    }
    std::vector<float> audio = tone(100u, 16000u);
    ibaudio_audio_view_v1 invalid = make_view(audio);
    ibaudio_audio_process_options_v1 process{};
    ibaudio_audio_process_options_init(&process);
    ibaudio_buffer_t *output = nullptr;
    invalid.channels = 0u;
    CHECK(ibaudio_audio_process(runtime.p, &invalid, &process, &output) == IBAUDIO_STATUS_INVALID_ARGUMENT);
    invalid = make_view(audio);
    invalid.struct_size = 4u;
    CHECK(ibaudio_audio_process(runtime.p, &invalid, &process, &output) == IBAUDIO_STATUS_INVALID_ARGUMENT);
    invalid = make_view(audio);
    invalid.interleaved_f32 = nullptr;
    CHECK(ibaudio_audio_process(runtime.p, &invalid, &process, &output) == IBAUDIO_STATUS_INVALID_ARGUMENT);

    Model vad_model(runtime.p, "energy-vad-v1");
    ibaudio_session_options_v1 session_options{};
    ibaudio_session_options_init(&session_options);
    session_options.task = IBAUDIO_TASK_VAD;
    session_options.vad_min_speech_ms = std::numeric_limits<uint32_t>::max();
    ibaudio_session_t *invalid_session = nullptr;
    CHECK(ibaudio_session_create(vad_model.p, &session_options, &invalid_session) ==
          IBAUDIO_STATUS_INVALID_ARGUMENT);
    CHECK(invalid_session == nullptr);
    ibaudio_session_options_init(&session_options);
    session_options.task = IBAUDIO_TASK_VAD;
    session_options.barge_in_threshold_dbfs = 1000.0f;
    CHECK(ibaudio_session_create(vad_model.p, &session_options, &invalid_session) ==
          IBAUDIO_STATUS_INVALID_ARGUMENT);
}

void test_stress() {
    Runtime runtime;
    Model model(runtime.p, "reference-asr-v1");
    Session session(model.p, IBAUDIO_TASK_ASR, true);
    ibaudio_stream_options_v1 options{};
    ibaudio_stream_options_init(&options);
    options.preferred_chunk_frames = 16u;
    options.max_queued_events = 4u;
    Stream stream;
    ok(ibaudio_stream_start(session.p, &options, &stream.p));
    const std::vector<float> tiny = tone(16u, 16000u);
    uint64_t start = 0u;
    for (uint32_t chunk = 0u; chunk < 5000u; ++chunk) {
        ibaudio_audio_view_v1 audio = make_view(tiny);
        audio.start_frame = start;
        start += audio.frame_count;
        ok(ibaudio_stream_push_audio(stream.p, &audio));
        if ((chunk % 25u) == 0u) drain_nonterminal(stream.p);
    }
    ok(ibaudio_stream_finish(stream.p));
    bool terminal = false;
    while (!terminal) {
        ibaudio_stream_event_v1 event{};
        ok(ibaudio_stream_poll_event(stream.p, 100u, &event));
        terminal = event.type == IBAUDIO_EVENT_FINAL;
        ibaudio_stream_event_release(&event);
    }
    ibaudio_metrics_v1 metrics{};
    ok(ibaudio_runtime_get_metrics(runtime.p, &metrics));
    CHECK(metrics.audio_frames_in == 80000u);

    Model vad_model(runtime.p, "energy-vad-v1");
    ibaudio_session_options_v1 vad_options{};
    ibaudio_session_options_init(&vad_options);
    vad_options.task = IBAUDIO_TASK_VAD;
    vad_options.streaming = 1u;
    vad_options.vad_threshold_dbfs = -30.0f;
    vad_options.vad_frame_ms = 1u;
    vad_options.vad_hop_ms = 1u;
    vad_options.vad_min_speech_ms = 1u;
    vad_options.vad_min_silence_ms = 1u;
    ibaudio_session_t *bounded_session = nullptr;
    ok(ibaudio_session_create(vad_model.p, &vad_options, &bounded_session));
    ibaudio_stream_options_v1 bounded_options{};
    ibaudio_stream_options_init(&bounded_options);
    bounded_options.max_queued_events = 4u;
    ibaudio_stream_t *bounded_stream = nullptr;
    ok(ibaudio_stream_start(bounded_session, &bounded_options, &bounded_stream));
    std::vector<float> alternating(16u * 5000u, 0.0f);
    for (size_t block = 0; block < 5000u; block += 2u) {
        std::fill_n(alternating.begin() + static_cast<std::ptrdiff_t>(block * 16u), 16u, 0.5f);
    }
    ibaudio_audio_view_v1 alternating_view = make_view(alternating);
    ok(ibaudio_stream_push_audio(bounded_stream, &alternating_view));
    ibaudio_stream_event_v1 bounded_event{};
    ok(ibaudio_stream_poll_event(bounded_stream, 100u, &bounded_event));
    CHECK(bounded_event.type == IBAUDIO_EVENT_CANCELLED);
    ibaudio_stream_event_release(&bounded_event);
    ok(ibaudio_stream_release(&bounded_stream));
    ok(ibaudio_session_release(&bounded_session));
}

void test_fuzz() {
    Runtime runtime;
    std::mt19937_64 random(0x4942415544494full);
    for (uint32_t iteration = 0u; iteration < 50000u; ++iteration) {
        const size_t size = static_cast<size_t>(random() % 1024u);
        std::vector<uint8_t> content(size);
        for (uint8_t &byte : content) byte = static_cast<uint8_t>(random());
        ibaudio_buffer_t *output = nullptr;
        const ibaudio_status_t status = ibaudio_wav_decode_memory(runtime.p,
            content.empty() ? nullptr : content.data(), content.size(), &output);
        CHECK(status == IBAUDIO_STATUS_OK || status == IBAUDIO_STATUS_INVALID_ARGUMENT);
        ibaudio_buffer_release(&output);
    }
    std::uniform_real_distribution<float> sample(-4.0f, 4.0f);
    for (uint32_t iteration = 0u; iteration < 10000u; ++iteration) {
        const uint32_t channels = 1u + static_cast<uint32_t>(random() % 8u);
        const uint32_t frames = static_cast<uint32_t>(random() % 512u);
        std::vector<float> values(static_cast<size_t>(channels) * frames);
        for (float &value : values) value = sample(random);
        ibaudio_audio_view_v1 audio = make_view(values, 8000u + static_cast<uint32_t>(random() % 48000u), channels);
        ibaudio_audio_process_options_v1 process{};
        ibaudio_audio_process_options_init(&process);
        process.target_sample_rate = 16000u;
        process.target_channels = 1u;
        process.gain_db = static_cast<float>(static_cast<int32_t>(random() % 40u) - 20);
        ibaudio_buffer_t *output = nullptr;
        ok(ibaudio_audio_process(runtime.p, &audio, &process, &output));
        ibaudio_buffer_release(&output);
    }
}

} // namespace

int main(int argc, char **argv) {
    try {
        if (argc != 2) throw std::runtime_error("expected test group");
        const std::string group = argv[1];
        if (group == "unit") test_unit();
        else if (group == "streaming") test_streaming();
        else if (group == "lifecycle") test_lifecycle();
        else if (group == "concurrency") test_concurrency();
        else if (group == "cancellation") test_cancellation();
        else if (group == "malformed") test_malformed();
        else if (group == "stress") test_stress();
        else if (group == "fuzz") test_fuzz();
        else throw std::runtime_error("unknown test group: " + group);
        std::cout << "PASS " << group << '\n';
        return 0;
    } catch (const std::exception &error) {
        std::cerr << "FAIL: " << error.what() << '\n';
        return 1;
    }
}
