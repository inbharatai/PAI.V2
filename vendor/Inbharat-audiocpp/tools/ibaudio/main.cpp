#include "inbharat/ibaudio.h"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <map>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

namespace {

constexpr double kPi = 3.141592653589793238462643383279502884;

struct RuntimeOwner {
    ibaudio_runtime_t *value = nullptr;
    RuntimeOwner() = default;
    RuntimeOwner(const RuntimeOwner &) = delete;
    RuntimeOwner &operator=(const RuntimeOwner &) = delete;
    RuntimeOwner(RuntimeOwner &&other) noexcept : value(other.value) { other.value = nullptr; }
    ~RuntimeOwner() { ibaudio_runtime_release(&value); }
};
struct ModelOwner {
    ibaudio_model_t *value = nullptr;
    ModelOwner() = default;
    ModelOwner(const ModelOwner &) = delete;
    ModelOwner &operator=(const ModelOwner &) = delete;
    ModelOwner(ModelOwner &&other) noexcept : value(other.value) { other.value = nullptr; }
    ~ModelOwner() { ibaudio_model_release(&value); }
};
struct SessionOwner {
    ibaudio_session_t *value = nullptr;
    SessionOwner() = default;
    SessionOwner(const SessionOwner &) = delete;
    SessionOwner &operator=(const SessionOwner &) = delete;
    SessionOwner(SessionOwner &&other) noexcept : value(other.value) { other.value = nullptr; }
    ~SessionOwner() { ibaudio_session_release(&value); }
};
struct BufferOwner {
    ibaudio_buffer_t *value = nullptr;
    BufferOwner() = default;
    BufferOwner(const BufferOwner &) = delete;
    BufferOwner &operator=(const BufferOwner &) = delete;
    BufferOwner(BufferOwner &&other) noexcept : value(other.value) { other.value = nullptr; }
    ~BufferOwner() { ibaudio_buffer_release(&value); }
};
struct StreamOwner {
    ibaudio_stream_t *value = nullptr;
    StreamOwner() = default;
    StreamOwner(const StreamOwner &) = delete;
    StreamOwner &operator=(const StreamOwner &) = delete;
    StreamOwner(StreamOwner &&other) noexcept : value(other.value) { other.value = nullptr; }
    ~StreamOwner() { ibaudio_stream_release(&value); }
};

ibaudio_string_view_v1 view(const std::string &value) {
    return {sizeof(ibaudio_string_view_v1), IBAUDIO_API_VERSION, value.data(), value.size()};
}

std::string last_error() {
    ibaudio_error_info_v1 error{};
    ibaudio_error_get_last(&error);
    std::ostringstream text;
    text << ibaudio_status_string(error.status) << " (" << error.function_name << "): " << error.message;
    return text.str();
}

void require(ibaudio_status_t status, const std::string &context) {
    if (status != IBAUDIO_STATUS_OK) {
        throw std::runtime_error(context + ": " + last_error());
    }
}

std::map<std::string, std::string> parse_options(int argc, char **argv, int start) {
    std::map<std::string, std::string> result;
    for (int index = start; index < argc; ++index) {
        const std::string argument = argv[index];
        if (argument.rfind("--", 0u) != 0u) {
            throw std::runtime_error("unexpected positional argument: " + argument);
        }
        const size_t equal = argument.find('=');
        if (equal != std::string::npos) {
            result[argument.substr(2u, equal - 2u)] = argument.substr(equal + 1u);
        } else if (index + 1 < argc && std::string(argv[index + 1]).rfind("--", 0u) != 0u) {
            result[argument.substr(2u)] = argv[++index];
        } else {
            result[argument.substr(2u)] = "true";
        }
    }
    return result;
}

bool flag(const std::map<std::string, std::string> &options, const std::string &name) {
    const auto it = options.find(name);
    return it != options.end() && it->second != "false" && it->second != "0";
}

std::string option(
    const std::map<std::string, std::string> &options,
    const std::string &name,
    const std::string &fallback = {}) {
    const auto it = options.find(name);
    return it == options.end() ? fallback : it->second;
}

uint32_t option_u32(
    const std::map<std::string, std::string> &options,
    const std::string &name,
    uint32_t fallback) {
    const std::string value = option(options, name);
    if (value.empty()) return fallback;
    size_t consumed = 0u;
    const unsigned long parsed = std::stoul(value, &consumed);
    if (consumed != value.size() || parsed > UINT32_MAX) throw std::runtime_error("invalid --" + name);
    return static_cast<uint32_t>(parsed);
}

float option_float(
    const std::map<std::string, std::string> &options,
    const std::string &name,
    float fallback) {
    const std::string value = option(options, name);
    if (value.empty()) return fallback;
    size_t consumed = 0u;
    const float parsed = std::stof(value, &consumed);
    if (consumed != value.size()) throw std::runtime_error("invalid --" + name);
    return parsed;
}

std::vector<uint8_t> read_file(const std::string &path) {
    std::ifstream input(path, std::ios::binary);
    if (!input) throw std::runtime_error("cannot open input file: " + path);
    input.seekg(0, std::ios::end);
    const std::streamoff size = input.tellg();
    if (size < 0 || size > static_cast<std::streamoff>(512ull * 1024ull * 1024ull)) {
        throw std::runtime_error("input file size is invalid or exceeds 512 MiB");
    }
    input.seekg(0, std::ios::beg);
    std::vector<uint8_t> bytes(static_cast<size_t>(size));
    if (!bytes.empty()) input.read(reinterpret_cast<char *>(bytes.data()), size);
    if (!input) throw std::runtime_error("failed to read input file: " + path);
    return bytes;
}

void write_file(const std::string &path, const void *bytes, uint64_t size) {
    std::ofstream output(path, std::ios::binary | std::ios::trunc);
    if (!output) throw std::runtime_error("cannot open output file: " + path);
    if (size > 0u) output.write(static_cast<const char *>(bytes), static_cast<std::streamsize>(size));
    if (!output) throw std::runtime_error("failed to write output file: " + path);
}

std::string buffer_text(ibaudio_buffer_t *buffer) {
    const void *data = nullptr;
    uint64_t size = 0u;
    require(ibaudio_buffer_get_data(buffer, &data, &size), "read text buffer");
    return std::string(static_cast<const char *>(data), static_cast<size_t>(size));
}

RuntimeOwner make_runtime(const std::map<std::string, std::string> &options) {
    ibaudio_runtime_options_v1 config{};
    ibaudio_runtime_options_init(&config);
    const std::string cache = option(options, "cache");
    const std::string root = option(options, "model-root");
    config.cache_directory = view(cache);
    config.allowed_model_root = view(root);
    config.cpu_threads = option_u32(options, "threads", 1u);
    const std::string backend = option(options, "backend", "auto");
    if (backend == "auto") config.requested_backend = IBAUDIO_BACKEND_AUTO;
    else if (backend == "cpu") config.requested_backend = IBAUDIO_BACKEND_CPU;
    else if (backend == "vulkan") config.requested_backend = IBAUDIO_BACKEND_VULKAN;
    else if (backend == "cuda") config.requested_backend = IBAUDIO_BACKEND_CUDA;
    else if (backend == "hip") config.requested_backend = IBAUDIO_BACKEND_HIP;
    else throw std::runtime_error("unknown backend: " + backend);
    config.allow_auto_cpu_fallback = flag(options, "no-fallback") ? 0u : 1u;
    RuntimeOwner runtime;
    require(ibaudio_runtime_create(&config, &runtime.value), "create runtime");
    return runtime;
}

ModelOwner load_model(ibaudio_runtime_t *runtime, const std::string &id) {
    ibaudio_model_load_options_v1 options{};
    ibaudio_model_load_options_init(&options);
    options.model_id = view(id);
    ModelOwner model;
    require(ibaudio_model_load(runtime, &options, &model.value), "load model " + id);
    return model;
}

SessionOwner make_session(ibaudio_model_t *model, ibaudio_task_t task, bool streaming, float threshold = -42.0f) {
    ibaudio_session_options_v1 options{};
    ibaudio_session_options_init(&options);
    options.task = task;
    options.streaming = streaming ? 1u : 0u;
    options.vad_threshold_dbfs = threshold;
    SessionOwner session;
    require(ibaudio_session_create(model, &options, &session.value), "create session");
    return session;
}

BufferOwner decode_wav(ibaudio_runtime_t *runtime, const std::string &path) {
    const std::vector<uint8_t> bytes = read_file(path);
    BufferOwner audio;
    require(ibaudio_wav_decode_memory(runtime, bytes.data(), bytes.size(), &audio.value), "decode WAV");
    return audio;
}

std::string json_escape(const std::string &value) {
    std::ostringstream output;
    for (unsigned char c : value) {
        if (c == '\"') output << "\\\"";
        else if (c == '\\') output << "\\\\";
        else if (c == '\n') output << "\\n";
        else if (c == '\r') output << "\\r";
        else if (c == '\t') output << "\\t";
        else if (c < 0x20u) output << "?";
        else output << static_cast<char>(c);
    }
    return output.str();
}

void command_info(const std::map<std::string, std::string> &options) {
    RuntimeOwner runtime = make_runtime(options);
    ibaudio_capabilities_v1 capabilities{};
    require(ibaudio_runtime_get_capabilities(runtime.value, &capabilities), "get capabilities");
    if (flag(options, "json")) {
        BufferOwner diagnostics;
        require(ibaudio_runtime_get_diagnostics_json(runtime.value, &diagnostics.value), "get diagnostics");
        std::cout << buffer_text(diagnostics.value) << '\n';
        return;
    }
    std::cout << "InBharat Audio " << ibaudio_get_runtime_version() << "\n"
              << "C ABI: " << capabilities.abi_major << '.' << capabilities.abi_minor << "\n"
              << "Models: " << capabilities.model_count << " (reference engines via the provider registry; deferred entries unavailable)\n"
              << "Backends: " << capabilities.backend_count << " (CPU mandatory; accelerators explicitly probed/unavailable)\n"
              << "Max input frames: " << capabilities.max_input_frames << '\n';
    uint32_t count = 0u;
    require(ibaudio_runtime_get_backend_count(runtime.value, &count), "get backend count");
    for (uint32_t index = 0; index < count; ++index) {
        ibaudio_backend_info_v1 backend{};
        require(ibaudio_runtime_get_backend_info(runtime.value, index, &backend), "get backend info");
        std::cout << "  " << backend.name << ": "
                  << (backend.selected ? "selected" : backend.reason) << '\n';
    }
}

void command_models(const std::map<std::string, std::string> &options) {
    RuntimeOwner runtime = make_runtime(options);
    uint32_t count = 0u;
    require(ibaudio_runtime_get_model_count(runtime.value, &count), "get model count");
    const bool json = flag(options, "json");
    if (json) std::cout << '[';
    for (uint32_t index = 0; index < count; ++index) {
        ibaudio_model_descriptor_v1 model{};
        require(ibaudio_runtime_get_model_descriptor(runtime.value, index, &model), "get model descriptor");
        if (json) {
            if (index > 0u) std::cout << ',';
            std::cout << "{\"id\":\"" << json_escape(model.id) << "\",\"task\":" << model.task
                      << ",\"available\":" << (model.available ? "true" : "false")
                      << ",\"streaming_class\":" << model.streaming_class
                      << ",\"streaming_label\":\"" << json_escape(model.streaming_label)
                      << "\",\"sha256\":\"" << model.artifact_sha256
                      << "\",\"spdx_license\":\"" << model.spdx_license << "\"}";
        } else {
            std::cout << model.id << " task=" << model.task << " available=" << model.available
                      << " license=" << model.spdx_license << "\n  " << model.streaming_label << '\n';
        }
    }
    if (json) std::cout << "]\n";
}

void command_diagnostics(const std::map<std::string, std::string> &options) {
    RuntimeOwner runtime = make_runtime(options);
    BufferOwner diagnostics;
    require(ibaudio_runtime_get_diagnostics_json(runtime.value, &diagnostics.value), "get diagnostics");
    std::cout << buffer_text(diagnostics.value) << '\n';
}

void command_audio_cpp_status(const std::map<std::string, std::string> &options) {
    RuntimeOwner runtime = make_runtime(options);
    ibaudio_audio_cpp_status_v1 status{};
    require(ibaudio_runtime_get_audio_cpp_status(runtime.value, &status), "get audio.cpp status");
    if (flag(options, "json")) {
        std::cout << "{\"schema\":\"inbharat.ibaudio.audio_cpp_status.v1\","
                  << "\"adapter_compiled\":" << (status.adapter_compiled ? "true" : "false") << ','
                  << "\"inference_ready\":" << (status.inference_ready ? "true" : "false") << ','
                  << "\"reviewed_commit\":\"" << json_escape(status.reviewed_commit) << "\","
                  << "\"upstream_source\":\"" << json_escape(status.upstream_source) << "\","
                  << "\"reason\":\"" << json_escape(status.reason) << "\"}\n";
        return;
    }
    std::cout << "audio.cpp adapter: " << (status.adapter_compiled ? "compiled" : "not compiled") << '\n'
              << "production inference: " << (status.inference_ready ? "READY" : "GATED") << '\n'
              << "reviewed commit: " << status.reviewed_commit << '\n'
              << "upstream: " << status.upstream_source << '\n'
              << "reason: " << status.reason << '\n';
}

void command_asr(const std::map<std::string, std::string> &options) {
    const std::string input_path = option(options, "input");
    if (input_path.empty()) throw std::runtime_error("asr requires --input FILE.wav");
    RuntimeOwner runtime = make_runtime(options);
    ModelOwner model = load_model(runtime.value, option(options, "model", "reference-asr-v1"));
    const bool streaming = flag(options, "stream");
    SessionOwner session = make_session(model.value, IBAUDIO_TASK_ASR, streaming);
    BufferOwner decoded = decode_wav(runtime.value, input_path);
    ibaudio_audio_view_v1 audio{};
    require(ibaudio_buffer_get_audio_view(decoded.value, &audio), "get audio view");
    std::string transcript;
    if (!streaming) {
        BufferOwner result;
        require(ibaudio_session_run_asr(session.value, &audio, &result.value), "run ASR");
        transcript = buffer_text(result.value);
    } else {
        ibaudio_stream_options_v1 stream_options{};
        ibaudio_stream_options_init(&stream_options);
        stream_options.preferred_chunk_frames = option_u32(options, "chunk-frames", 1600u);
        StreamOwner stream;
        require(ibaudio_stream_start(session.value, &stream_options, &stream.value), "start ASR stream");
        for (uint64_t start = 0u; start < audio.frame_count; start += stream_options.preferred_chunk_frames) {
            ibaudio_audio_view_v1 chunk = audio;
            chunk.start_frame = start;
            chunk.frame_count = std::min<uint64_t>(stream_options.preferred_chunk_frames, audio.frame_count - start);
            chunk.interleaved_f32 = audio.interleaved_f32 + static_cast<size_t>(start) * audio.channels;
            require(ibaudio_stream_push_audio(stream.value, &chunk), "push ASR audio");
            while (true) {
                ibaudio_stream_event_v1 event{};
                const ibaudio_status_t status = ibaudio_stream_poll_event(stream.value, 0u, &event);
                if (status == IBAUDIO_STATUS_WOULD_BLOCK) break;
                require(status, "poll ASR partial");
                if (event.type == IBAUDIO_EVENT_PARTIAL_TEXT && flag(options, "partials")) {
                    std::cerr << buffer_text(event.payload) << '\n';
                }
                ibaudio_stream_event_release(&event);
            }
        }
        require(ibaudio_stream_finish(stream.value), "finish ASR stream");
        bool done = false;
        while (!done) {
            ibaudio_stream_event_v1 event{};
            require(ibaudio_stream_poll_event(stream.value, 100u, &event), "poll ASR final");
            if (event.type == IBAUDIO_EVENT_FINAL_TEXT) transcript = buffer_text(event.payload);
            done = event.type == IBAUDIO_EVENT_FINAL;
            ibaudio_stream_event_release(&event);
        }
    }
    if (flag(options, "json")) {
        std::cout << "{\"model\":\"" << json_escape(option(options, "model", "reference-asr-v1")) << "\",\"transcript\":\""
                  << json_escape(transcript) << "\",\"streaming\":" << (streaming ? "true" : "false") << "}\n";
    } else {
        std::cout << transcript << '\n';
    }
}

void command_tts(const std::map<std::string, std::string> &options) {
    const std::string text = option(options, "text");
    const std::string output_path = option(options, "output");
    if (text.empty() || output_path.empty()) throw std::runtime_error("tts requires --text TEXT --output FILE.wav");
    RuntimeOwner runtime = make_runtime(options);
    ModelOwner model = load_model(runtime.value, "reference-tts-v1");
    SessionOwner session = make_session(model.value, IBAUDIO_TASK_TTS, false);
    BufferOwner audio;
    require(ibaudio_session_run_tts(session.value, view(text), &audio.value), "run TTS");
    ibaudio_audio_view_v1 audio_view{};
    require(ibaudio_buffer_get_audio_view(audio.value, &audio_view), "get TTS audio view");
    BufferOwner wav;
    require(ibaudio_wav_encode_pcm16(runtime.value, &audio_view, &wav.value), "encode TTS WAV");
    const void *bytes = nullptr;
    uint64_t size = 0u;
    require(ibaudio_buffer_get_data(wav.value, &bytes, &size), "get WAV bytes");
    write_file(output_path, bytes, size);
    if (flag(options, "json")) {
        std::cout << "{\"model\":\"reference-tts-v1\",\"output\":\"" << json_escape(output_path)
                  << "\",\"sample_rate\":" << audio_view.sample_rate << ",\"frames\":" << audio_view.frame_count << "}\n";
    } else {
        std::cout << "wrote " << output_path << " (" << audio_view.frame_count << " frames @ "
                  << audio_view.sample_rate << " Hz)\n";
    }
}

void command_vad(const std::map<std::string, std::string> &options) {
    const std::string input_path = option(options, "input");
    if (input_path.empty()) throw std::runtime_error("vad requires --input FILE.wav");
    RuntimeOwner runtime = make_runtime(options);
    ModelOwner model = load_model(runtime.value, option(options, "model", "energy-vad-v1"));
    SessionOwner session = make_session(model.value, IBAUDIO_TASK_VAD, false, option_float(options, "threshold-dbfs", -42.0f));
    BufferOwner decoded = decode_wav(runtime.value, input_path);
    ibaudio_audio_view_v1 audio{};
    require(ibaudio_buffer_get_audio_view(decoded.value, &audio), "get audio view");
    BufferOwner segments;
    require(ibaudio_session_run_vad(session.value, &audio, &segments.value), "run VAD");
    const void *data = nullptr;
    uint64_t size = 0u;
    require(ibaudio_buffer_get_data(segments.value, &data, &size), "read VAD segments");
    const size_t count = static_cast<size_t>(size / sizeof(ibaudio_vad_segment_v1));
    const auto *values = static_cast<const ibaudio_vad_segment_v1 *>(data);
    if (flag(options, "json")) {
        std::cout << "{\"model\":\"" << json_escape(option(options, "model", "energy-vad-v1")) << "\",\"segments\":[";
        for (size_t index = 0; index < count; ++index) {
            if (index > 0u) std::cout << ',';
            std::cout << "{\"start_frame\":" << values[index].start_frame
                      << ",\"end_frame\":" << values[index].end_frame
                      << ",\"confidence\":" << values[index].confidence
                      << ",\"peak_dbfs\":" << values[index].peak_dbfs << '}';
        }
        std::cout << "]}\n";
    } else {
        for (size_t index = 0; index < count; ++index) {
            std::cout << values[index].start_frame << ',' << values[index].end_frame << ','
                      << values[index].confidence << ',' << values[index].peak_dbfs << '\n';
        }
    }
}

void command_benchmark(const std::map<std::string, std::string> &options) {
    const uint32_t iterations = option_u32(options, "iterations", 5u);
    if (iterations == 0u || iterations > 10000u) throw std::runtime_error("iterations must be in [1, 10000]");
    RuntimeOwner runtime = make_runtime(options);
    ModelOwner asr_model = load_model(runtime.value, "reference-asr-v1");
    ModelOwner vad_model = load_model(runtime.value, "energy-vad-v1");
    ModelOwner tts_model = load_model(runtime.value, "reference-tts-v1");
    SessionOwner asr = make_session(asr_model.value, IBAUDIO_TASK_ASR, false);
    SessionOwner vad = make_session(vad_model.value, IBAUDIO_TASK_VAD, false);
    SessionOwner tts = make_session(tts_model.value, IBAUDIO_TASK_TTS, false);
    std::vector<float> signal(16000u);
    for (size_t index = 0; index < signal.size(); ++index) {
        signal[index] = static_cast<float>(0.25 * std::sin(2.0 * kPi * 440.0 * index / 16000.0));
    }
    ibaudio_audio_view_v1 audio{sizeof(ibaudio_audio_view_v1), IBAUDIO_API_VERSION,
        signal.data(), signal.size(), 16000u, 1u, 0u, 0u};
    auto measure = [&](const std::string &name, auto &&operation) {
        const auto start = std::chrono::steady_clock::now();
        for (uint32_t iteration = 0; iteration < iterations; ++iteration) operation();
        const auto end = std::chrono::steady_clock::now();
        const double ms = std::chrono::duration<double, std::milli>(end - start).count();
        return std::make_pair(name, ms / iterations);
    };
    const auto asr_result = measure("asr", [&]() {
        BufferOwner result;
        require(ibaudio_session_run_asr(asr.value, &audio, &result.value), "benchmark ASR");
    });
    const auto vad_result = measure("vad", [&]() {
        BufferOwner result;
        require(ibaudio_session_run_vad(vad.value, &audio, &result.value), "benchmark VAD");
    });
    const std::string phrase = option(options, "text", "InBharat reference audio");
    const auto tts_result = measure("tts", [&]() {
        BufferOwner result;
        require(ibaudio_session_run_tts(tts.value, view(phrase), &result.value), "benchmark TTS");
    });
    std::ostringstream json;
    json.imbue(std::locale::classic());
    json << "{\"schema\":\"inbharat.ibaudio.benchmark.v1\",\"runtime_version\":\""
         << ibaudio_get_runtime_version() << "\",\"backend\":\"cpu\",\"iterations\":" << iterations
         << ",\"results\":[{\"operation\":\"asr\",\"mean_ms\":" << std::fixed << std::setprecision(6)
         << asr_result.second << "},{\"operation\":\"vad\",\"mean_ms\":" << vad_result.second
         << "},{\"operation\":\"tts\",\"mean_ms\":" << tts_result.second << "}]}";
    std::ostringstream csv;
    csv.imbue(std::locale::classic());
    csv << "schema,runtime_version,backend,operation,iterations,mean_ms\n";
    for (const auto &result : {asr_result, vad_result, tts_result}) {
        csv << "inbharat.ibaudio.benchmark.v1," << ibaudio_get_runtime_version()
            << ",cpu," << result.first << ',' << iterations << ',' << std::fixed << std::setprecision(6)
            << result.second << '\n';
    }
    const std::string json_path = option(options, "output-json");
    const std::string csv_path = option(options, "output-csv");
    if (!json_path.empty()) write_file(json_path, json.str().data(), json.str().size());
    if (!csv_path.empty()) write_file(csv_path, csv.str().data(), csv.str().size());
    std::cout << json.str() << '\n';
}

void print_help() {
    std::cout <<
        "ibaudio " IBAUDIO_RUNTIME_VERSION "\n"
        "Usage: ibaudio <command> [options]\n\n"
        "Commands:\n"
        "  info [--json] [--backend auto|cpu|vulkan|cuda|hip]\n"
        "  models [--json]\n"
        "  asr --input FILE.wav [--stream] [--partials] [--chunk-frames N] [--json]\n"
        "  tts --text TEXT --output FILE.wav [--json]\n"
        "  vad --input FILE.wav [--threshold-dbfs DB] [--json]\n"
        "  benchmark [--iterations N] [--output-json FILE] [--output-csv FILE]\n"
        "  diagnostics\n"
        "  audio-cpp-status [--json]\n\n"
        "Common options: --threads N --cache DIR --model-root DIR --backend NAME --no-fallback\n";
}

} // namespace

int main(int argc, char **argv) {
    try {
        if (argc < 2 || std::string(argv[1]) == "help" || std::string(argv[1]) == "--help") {
            print_help();
            return 0;
        }
        const std::string command = argv[1];
        const auto options = parse_options(argc, argv, 2);
        if (command == "info") command_info(options);
        else if (command == "models") command_models(options);
        else if (command == "asr") command_asr(options);
        else if (command == "tts") command_tts(options);
        else if (command == "vad") command_vad(options);
        else if (command == "benchmark") command_benchmark(options);
        else if (command == "diagnostics") command_diagnostics(options);
        else if (command == "audio-cpp-status") command_audio_cpp_status(options);
        else throw std::runtime_error("unknown command: " + command);
        return 0;
    } catch (const std::exception &error) {
        std::cerr << "ibaudio: " << error.what() << '\n';
        return 2;
    }
}
