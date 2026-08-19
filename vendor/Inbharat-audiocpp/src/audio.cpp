#include "internal.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <fstream>
#include <iomanip>
#include <limits>
#include <sstream>
#include <stdexcept>

namespace ibaudio {
namespace {

constexpr double kPi = 3.141592653589793238462643383279502884;

void throw_if_cancelled(const CancellationToken *cancel) {
    if (cancel != nullptr && cancel->requested.load(std::memory_order_relaxed)) {
        throw std::runtime_error("operation cancelled");
    }
}

float dbfs_from_rms(double rms) {
    if (!(rms > 1.0e-12)) {
        return -120.0f;
    }
    return static_cast<float>(20.0 * std::log10(rms));
}

uint16_t read_u16le(const uint8_t *p) {
    return static_cast<uint16_t>(p[0]) |
           static_cast<uint16_t>(static_cast<uint16_t>(p[1]) << 8u);
}

uint32_t read_u32le(const uint8_t *p) {
    return static_cast<uint32_t>(p[0]) |
           (static_cast<uint32_t>(p[1]) << 8u) |
           (static_cast<uint32_t>(p[2]) << 16u) |
           (static_cast<uint32_t>(p[3]) << 24u);
}

void append_u16le(std::vector<uint8_t> &out, uint16_t value) {
    out.push_back(static_cast<uint8_t>(value & 0xffu));
    out.push_back(static_cast<uint8_t>((value >> 8u) & 0xffu));
}

void append_u32le(std::vector<uint8_t> &out, uint32_t value) {
    out.push_back(static_cast<uint8_t>(value & 0xffu));
    out.push_back(static_cast<uint8_t>((value >> 8u) & 0xffu));
    out.push_back(static_cast<uint8_t>((value >> 16u) & 0xffu));
    out.push_back(static_cast<uint8_t>((value >> 24u) & 0xffu));
}

std::vector<float> convert_channels(
    const float *input,
    uint64_t frame_count,
    uint32_t input_channels,
    uint32_t output_channels) {
    if (frame_count > std::numeric_limits<size_t>::max() / output_channels) {
        throw std::length_error("channel conversion size overflow");
    }
    std::vector<float> result(static_cast<size_t>(frame_count) * output_channels, 0.0f);
    if (input_channels == output_channels) {
        std::copy(input, input + result.size(), result.begin());
        return result;
    }
    for (uint64_t frame = 0; frame < frame_count; ++frame) {
        double mono = 0.0;
        for (uint32_t channel = 0; channel < input_channels; ++channel) {
            mono += input[static_cast<size_t>(frame) * input_channels + channel];
        }
        mono /= static_cast<double>(input_channels);
        if (output_channels == 1u) {
            result[static_cast<size_t>(frame)] = static_cast<float>(mono);
        } else if (input_channels == 1u) {
            for (uint32_t channel = 0; channel < output_channels; ++channel) {
                result[static_cast<size_t>(frame) * output_channels + channel] = input[frame];
            }
        } else {
            for (uint32_t channel = 0; channel < output_channels; ++channel) {
                if (channel < input_channels) {
                    result[static_cast<size_t>(frame) * output_channels + channel] =
                        input[static_cast<size_t>(frame) * input_channels + channel];
                } else {
                    result[static_cast<size_t>(frame) * output_channels + channel] = static_cast<float>(mono);
                }
            }
        }
    }
    return result;
}

std::vector<float> resample_linear(
    const std::vector<float> &input,
    uint64_t input_frames,
    uint32_t channels,
    uint32_t input_rate,
    uint32_t output_rate,
    const CancellationToken *cancel) {
    if (input_rate == output_rate || input_frames == 0u) {
        return input;
    }
    const long double exact = static_cast<long double>(input_frames) * output_rate / input_rate;
    const uint64_t output_frames = static_cast<uint64_t>(std::ceil(exact));
    if (output_frames > std::numeric_limits<size_t>::max() / channels) {
        throw std::length_error("resample output size overflow");
    }
    std::vector<float> result(static_cast<size_t>(output_frames) * channels, 0.0f);
    for (uint64_t output_frame = 0; output_frame < output_frames; ++output_frame) {
        if ((output_frame & 0x3fffu) == 0u) {
            throw_if_cancelled(cancel);
        }
        const long double position = static_cast<long double>(output_frame) * input_rate / output_rate;
        const uint64_t left = std::min<uint64_t>(static_cast<uint64_t>(position), input_frames - 1u);
        const uint64_t right = std::min<uint64_t>(left + 1u, input_frames - 1u);
        const float fraction = static_cast<float>(position - static_cast<long double>(left));
        for (uint32_t channel = 0; channel < channels; ++channel) {
            const float a = input[static_cast<size_t>(left) * channels + channel];
            const float b = input[static_cast<size_t>(right) * channels + channel];
            result[static_cast<size_t>(output_frame) * channels + channel] = a + (b - a) * fraction;
        }
    }
    return result;
}

AudioData canonical_mono_16k(const AudioData &source, const CancellationToken *cancel) {
    ibaudio_audio_view_v1 view{};
    view.struct_size = sizeof(view);
    view.api_version = IBAUDIO_API_VERSION;
    view.interleaved_f32 = source.samples.data();
    view.frame_count = source.channels == 0u ? 0u : source.samples.size() / source.channels;
    view.sample_rate = source.sample_rate;
    view.channels = source.channels;
    ibaudio_audio_process_options_v1 options{};
    options.struct_size = sizeof(options);
    options.api_version = IBAUDIO_API_VERSION;
    options.target_sample_rate = 16000;
    options.target_channels = 1;
    options.gain_db = 0.0f;
    options.normalize_peak = 0.0f;
    options.clip_peak = 1.0f;
    options.sanitize_non_finite = 1;
    return process_audio(view, options, kAbsoluteMaxInputFrames, cancel);
}

} // namespace

AudioData process_audio(
    const ibaudio_audio_view_v1 &input,
    const ibaudio_audio_process_options_v1 &options,
    uint64_t max_input_frames,
    const CancellationToken *cancel) {
    if (input.channels == 0u || input.channels > kMaxChannels) {
        throw std::invalid_argument("audio channels must be in [1, 32]");
    }
    if (input.sample_rate < kMinSampleRate || input.sample_rate > kMaxSampleRate) {
        throw std::invalid_argument("audio sample rate must be in [1000, 384000]");
    }
    if (input.frame_count > max_input_frames || input.frame_count > kAbsoluteMaxInputFrames) {
        throw std::length_error("audio exceeds configured frame limit");
    }
    if (input.frame_count > 0u && input.interleaved_f32 == nullptr) {
        throw std::invalid_argument("audio sample pointer is null");
    }
    if (input.frame_count > std::numeric_limits<size_t>::max() / input.channels) {
        throw std::length_error("audio sample count overflow");
    }
    const uint32_t output_rate = options.target_sample_rate == 0u ? input.sample_rate : options.target_sample_rate;
    const uint32_t output_channels = options.target_channels == 0u ? input.channels : options.target_channels;
    if (output_rate < kMinSampleRate || output_rate > kMaxSampleRate) {
        throw std::invalid_argument("target sample rate must be in [1000, 384000]");
    }
    if (output_channels == 0u || output_channels > kMaxChannels) {
        throw std::invalid_argument("target channels must be in [1, 32]");
    }
    if (!std::isfinite(options.gain_db) || options.gain_db < -120.0f || options.gain_db > 120.0f) {
        throw std::invalid_argument("gain_db must be finite and in [-120, 120]");
    }
    if (!std::isfinite(options.normalize_peak) || options.normalize_peak < 0.0f || options.normalize_peak > 1.0f) {
        throw std::invalid_argument("normalize_peak must be in [0, 1]");
    }
    if (!std::isfinite(options.clip_peak) || options.clip_peak < 0.0f || options.clip_peak > 1.0f) {
        throw std::invalid_argument("clip_peak must be in [0, 1]");
    }

    AudioData output;
    output.sample_rate = output_rate;
    output.channels = output_channels;
    output.info.struct_size = sizeof(output.info);
    output.info.api_version = IBAUDIO_API_VERSION;
    output.info.sample_rate = output_rate;
    output.info.channels = output_channels;

    std::vector<float> sanitized;
    const size_t sample_count = static_cast<size_t>(input.frame_count) * input.channels;
    sanitized.resize(sample_count);
    float input_peak = 0.0f;
    for (size_t index = 0; index < sample_count; ++index) {
        if ((index & 0xffffu) == 0u) {
            throw_if_cancelled(cancel);
        }
        float sample = input.interleaved_f32[index];
        if (!std::isfinite(sample)) {
            if (options.sanitize_non_finite == 0u) {
                throw std::invalid_argument("audio contains a non-finite sample");
            }
            sample = 0.0f;
            ++output.info.sanitized_samples;
        }
        sanitized[index] = sample;
        input_peak = std::max(input_peak, std::fabs(sample));
    }
    output.info.input_peak = input_peak;

    std::vector<float> converted = convert_channels(
        sanitized.data(), input.frame_count, input.channels, output_channels);
    converted = resample_linear(
        converted, input.frame_count, output_channels, input.sample_rate, output_rate, cancel);
    const uint64_t output_frames = output_channels == 0u ? 0u : converted.size() / output_channels;
    if (output_frames > max_input_frames * static_cast<uint64_t>(output_rate) / input.sample_rate + 2u) {
        throw std::length_error("resample output exceeds safe size");
    }

    double gain = std::pow(10.0, static_cast<double>(options.gain_db) / 20.0);
    float converted_peak = 0.0f;
    for (float sample : converted) {
        converted_peak = std::max(converted_peak, std::fabs(sample));
    }
    if (options.normalize_peak > 0.0f && converted_peak > 0.0f) {
        gain *= static_cast<double>(options.normalize_peak) / (converted_peak * gain);
    }
    output.info.applied_gain = static_cast<float>(gain);
    output.samples.resize(converted.size());
    float output_peak = 0.0f;
    for (size_t index = 0; index < converted.size(); ++index) {
        double value = static_cast<double>(converted[index]) * gain;
        if (options.clip_peak > 0.0f && std::fabs(value) > options.clip_peak) {
            value = std::copysign(static_cast<double>(options.clip_peak), value);
            ++output.info.clipped_samples;
        }
        output.samples[index] = static_cast<float>(value);
        output_peak = std::max(output_peak, std::fabs(output.samples[index]));
    }
    output.info.output_peak = output_peak;
    output.info.frame_count = output_frames;
    return output;
}

std::vector<ibaudio_vad_segment_v1> run_energy_vad(
    const AudioData &audio,
    const VadConfig &config,
    const CancellationToken *cancel,
    uint64_t *processed_frames) {
    if (audio.channels != 1u || audio.sample_rate == 0u) {
        throw std::invalid_argument("energy VAD requires mono audio");
    }
    if (!std::isfinite(config.threshold_dbfs) || config.threshold_dbfs > 0.0f || config.threshold_dbfs < -120.0f) {
        throw std::invalid_argument("VAD threshold must be in [-120, 0] dBFS");
    }
    const uint64_t frame_size = std::max<uint64_t>(1u, static_cast<uint64_t>(audio.sample_rate) * config.frame_ms / 1000u);
    const uint64_t hop_size = std::max<uint64_t>(1u, static_cast<uint64_t>(audio.sample_rate) * config.hop_ms / 1000u);
    const uint32_t speech_hops = std::max<uint32_t>(1u, (config.min_speech_ms + config.hop_ms - 1u) / config.hop_ms);
    const uint32_t silence_hops = std::max<uint32_t>(1u, (config.min_silence_ms + config.hop_ms - 1u) / config.hop_ms);
    std::vector<ibaudio_vad_segment_v1> segments;
    bool active = false;
    uint32_t speech_run = 0;
    uint32_t silence_run = 0;
    uint64_t candidate_start = 0;
    float max_confidence = 0.0f;
    float peak_db = -120.0f;
    const uint64_t sample_count = audio.samples.size();
    uint64_t last_analyzed_end = 0;

    for (uint64_t start = 0; start < sample_count; start += hop_size) {
        if (((start / hop_size) & 0xffu) == 0u) {
            throw_if_cancelled(cancel);
        }
        const uint64_t end = std::min<uint64_t>(sample_count, start + frame_size);
        double energy = 0.0;
        for (uint64_t index = start; index < end; ++index) {
            const double sample = audio.samples[static_cast<size_t>(index)];
            energy += sample * sample;
        }
        const double rms = end == start ? 0.0 : std::sqrt(energy / static_cast<double>(end - start));
        const float db = dbfs_from_rms(rms);
        const bool speech = db >= config.threshold_dbfs;
        const float confidence = std::clamp((db - config.threshold_dbfs + 12.0f) / 24.0f, 0.0f, 1.0f);
        last_analyzed_end = end;
        if (!active) {
            if (speech) {
                if (speech_run == 0u) {
                    candidate_start = start;
                    max_confidence = confidence;
                    peak_db = db;
                }
                ++speech_run;
                max_confidence = std::max(max_confidence, confidence);
                peak_db = std::max(peak_db, db);
                if (speech_run >= speech_hops) {
                    active = true;
                    silence_run = 0;
                }
            } else {
                speech_run = 0;
            }
        } else {
            max_confidence = std::max(max_confidence, confidence);
            peak_db = std::max(peak_db, db);
            if (speech) {
                silence_run = 0;
            } else {
                ++silence_run;
                if (silence_run >= silence_hops) {
                    const uint64_t trailing = static_cast<uint64_t>(silence_run) * hop_size;
                    const uint64_t segment_end = end > trailing ? end - trailing + frame_size : candidate_start;
                    ibaudio_vad_segment_v1 segment{};
                    segment.start_frame = candidate_start;
                    segment.end_frame = std::min<uint64_t>(sample_count, std::max(candidate_start + 1u, segment_end));
                    segment.confidence = max_confidence;
                    segment.peak_dbfs = peak_db;
                    segments.push_back(segment);
                    active = false;
                    speech_run = 0;
                    silence_run = 0;
                }
            }
        }
        if (end == sample_count) {
            break;
        }
    }
    if (active) {
        ibaudio_vad_segment_v1 segment{};
        segment.start_frame = candidate_start;
        segment.end_frame = std::max<uint64_t>(candidate_start + 1u, last_analyzed_end);
        segment.end_frame = std::min<uint64_t>(segment.end_frame, sample_count);
        segment.confidence = max_confidence;
        segment.peak_dbfs = peak_db;
        segments.push_back(segment);
    }
    if (processed_frames != nullptr) {
        *processed_frames = sample_count;
    }
    return segments;
}

std::string run_reference_asr(
    const AudioData &input,
    const CancellationToken *cancel,
    uint64_t *processed_frames) {
    const AudioData audio = canonical_mono_16k(input, cancel);
    VadConfig config;
    config.threshold_dbfs = -45.0f;
    config.min_speech_ms = 30;
    config.min_silence_ms = 50;
    const auto segments = run_energy_vad(audio, config, cancel, processed_frames);
    if (segments.empty()) {
        return "[reference-asr silence]";
    }
    uint64_t speech_frames = 0;
    uint64_t positive_crossings = 0;
    double energy = 0.0;
    for (const auto &segment : segments) {
        speech_frames += segment.end_frame - segment.start_frame;
        for (uint64_t index = segment.start_frame + 1u; index < segment.end_frame; ++index) {
            if ((index & 0x3fffu) == 0u) {
                throw_if_cancelled(cancel);
            }
            const float previous = audio.samples[static_cast<size_t>(index - 1u)];
            const float current = audio.samples[static_cast<size_t>(index)];
            if (previous <= 0.0f && current > 0.0f) {
                ++positive_crossings;
            }
            energy += static_cast<double>(current) * current;
        }
    }
    const double seconds = static_cast<double>(speech_frames) / audio.sample_rate;
    const uint64_t pitch_hz = seconds > 0.0
        ? static_cast<uint64_t>(std::llround(static_cast<double>(positive_crossings) / seconds))
        : 0u;
    const double rms = speech_frames > 0u ? std::sqrt(energy / static_cast<double>(speech_frames)) : 0.0;
    std::ostringstream text;
    text.imbue(std::locale::classic());
    text << "[reference-asr speech duration=" << std::fixed << std::setprecision(3) << seconds
         << "s segments=" << segments.size() << " pitch=" << pitch_hz
         << "Hz level=" << std::setprecision(1) << dbfs_from_rms(rms) << "dBFS]";
    return text.str();
}

AudioData run_reference_tts(
    const std::string &text,
    const CancellationToken *cancel,
    uint64_t *processed_chars) {
    if (text.empty()) {
        throw std::invalid_argument("TTS text must not be empty");
    }
    if (text.size() > 16384u) {
        throw std::length_error("TTS text exceeds 16384-byte reference-engine limit");
    }
    constexpr uint32_t sample_rate = 24000;
    constexpr uint32_t tone_frames = 960;
    constexpr uint32_t gap_frames = 240;
    constexpr uint32_t edge_frames = 480;
    if (text.size() > (std::numeric_limits<size_t>::max() - 2u * edge_frames) / (tone_frames + gap_frames)) {
        throw std::length_error("TTS output size overflow");
    }
    AudioData output;
    output.sample_rate = sample_rate;
    output.channels = 1;
    output.samples.reserve(2u * edge_frames + text.size() * (tone_frames + gap_frames));
    output.samples.insert(output.samples.end(), edge_frames, 0.0f);
    uint64_t completed = 0;
    for (char character : text) {
        const unsigned char byte = static_cast<unsigned char>(character);
        throw_if_cancelled(cancel);
        if (byte == static_cast<unsigned char>(' ') || byte == static_cast<unsigned char>('\n') ||
            byte == static_cast<unsigned char>('\t')) {
            output.samples.insert(output.samples.end(), tone_frames + gap_frames, 0.0f);
        } else {
            const double frequency = 180.0 + static_cast<double>(byte % 48u) * 10.0;
            for (uint32_t frame = 0; frame < tone_frames; ++frame) {
                if ((frame & 0xffu) == 0u) {
                    throw_if_cancelled(cancel);
                }
                const double phase = 2.0 * kPi * frequency * frame / sample_rate;
                const double envelope = std::sin(kPi * frame / static_cast<double>(tone_frames));
                output.samples.push_back(static_cast<float>(0.22 * envelope * std::sin(phase)));
            }
            output.samples.insert(output.samples.end(), gap_frames, 0.0f);
        }
        ++completed;
        if (processed_chars != nullptr) {
            *processed_chars = completed;
        }
        if ((completed & 0x1fu) == 0u) {
            std::this_thread::yield();
        }
    }
    output.samples.insert(output.samples.end(), edge_frames, 0.0f);
    output.info.struct_size = sizeof(output.info);
    output.info.api_version = IBAUDIO_API_VERSION;
    output.info.sample_rate = sample_rate;
    output.info.channels = 1;
    output.info.frame_count = output.samples.size();
    output.info.input_peak = 0.0f;
    output.info.output_peak = 0.22f;
    output.info.applied_gain = 1.0f;
    return output;
}

std::vector<uint8_t> wav_encode_pcm16(const ibaudio_audio_view_v1 &audio, uint64_t max_input_frames) {
    if (audio.channels == 0u || audio.channels > kMaxChannels ||
        audio.sample_rate < kMinSampleRate || audio.sample_rate > kMaxSampleRate ||
        audio.frame_count > max_input_frames || (audio.frame_count > 0u && audio.interleaved_f32 == nullptr)) {
        throw std::invalid_argument("invalid audio view for WAV encoding");
    }
    const uint64_t sample_count = audio.frame_count * audio.channels;
    const uint64_t data_bytes_64 = sample_count * 2u;
    if (data_bytes_64 > 0xffffffffu - 36u || data_bytes_64 > kMaxOwnedBytes) {
        throw std::length_error("PCM16 WAV exceeds RIFF or runtime size limit");
    }
    const uint32_t data_bytes = static_cast<uint32_t>(data_bytes_64);
    std::vector<uint8_t> output;
    output.reserve(static_cast<size_t>(44u + data_bytes));
    output.insert(output.end(), {'R', 'I', 'F', 'F'});
    append_u32le(output, 36u + data_bytes);
    output.insert(output.end(), {'W', 'A', 'V', 'E', 'f', 'm', 't', ' '});
    append_u32le(output, 16u);
    append_u16le(output, 1u);
    append_u16le(output, static_cast<uint16_t>(audio.channels));
    append_u32le(output, audio.sample_rate);
    append_u32le(output, audio.sample_rate * audio.channels * 2u);
    append_u16le(output, static_cast<uint16_t>(audio.channels * 2u));
    append_u16le(output, 16u);
    output.insert(output.end(), {'d', 'a', 't', 'a'});
    append_u32le(output, data_bytes);
    for (uint64_t index = 0; index < sample_count; ++index) {
        float sample = audio.interleaved_f32[index];
        if (!std::isfinite(sample)) {
            sample = 0.0f;
        }
        sample = std::clamp(sample, -1.0f, 1.0f);
        const int32_t quantized = sample <= -1.0f
            ? -32768
            : static_cast<int32_t>(std::lrint(sample * 32767.0f));
        append_u16le(output, static_cast<uint16_t>(static_cast<int16_t>(quantized)));
    }
    return output;
}

AudioData wav_decode(const void *bytes, uint64_t size, uint64_t max_input_frames) {
    if (bytes == nullptr || size < 12u) {
        throw std::invalid_argument("WAV input is null or too short");
    }
    if (size > kMaxOwnedBytes || size > std::numeric_limits<size_t>::max()) {
        throw std::length_error("WAV input exceeds runtime size limit");
    }
    const auto *data = static_cast<const uint8_t *>(bytes);
    if (std::memcmp(data, "RIFF", 4u) != 0 || std::memcmp(data + 8u, "WAVE", 4u) != 0) {
        throw std::invalid_argument("invalid RIFF/WAVE signature");
    }
    const uint64_t riff_end = std::min<uint64_t>(size, static_cast<uint64_t>(read_u32le(data + 4u)) + 8u);
    uint16_t format = 0;
    uint16_t channels = 0;
    uint32_t sample_rate = 0;
    uint16_t bits = 0;
    uint16_t block_align = 0;
    const uint8_t *payload = nullptr;
    uint32_t payload_size = 0;
    uint64_t offset = 12u;
    while (offset + 8u <= riff_end) {
        const uint8_t *chunk = data + offset;
        const uint32_t chunk_size = read_u32le(chunk + 4u);
        const uint64_t body = offset + 8u;
        if (body + chunk_size > riff_end) {
            throw std::invalid_argument("truncated WAV chunk");
        }
        if (std::memcmp(chunk, "fmt ", 4u) == 0) {
            if (chunk_size < 16u) {
                throw std::invalid_argument("short WAV fmt chunk");
            }
            format = read_u16le(data + body);
            channels = read_u16le(data + body + 2u);
            sample_rate = read_u32le(data + body + 4u);
            block_align = read_u16le(data + body + 12u);
            bits = read_u16le(data + body + 14u);
            if (format == 0xfffeu && chunk_size >= 40u) {
                format = read_u16le(data + body + 24u);
            }
        } else if (std::memcmp(chunk, "data", 4u) == 0 && payload == nullptr) {
            payload = data + body;
            payload_size = chunk_size;
        }
        offset = body + chunk_size + (chunk_size & 1u);
    }
    if (channels == 0u || channels > kMaxChannels || sample_rate < kMinSampleRate ||
        sample_rate > kMaxSampleRate || payload == nullptr || block_align == 0u) {
        throw std::invalid_argument("incomplete or unsupported WAV metadata");
    }
    const uint16_t expected_align = static_cast<uint16_t>(channels * ((bits + 7u) / 8u));
    if (block_align != expected_align || payload_size % block_align != 0u) {
        throw std::invalid_argument("malformed WAV block alignment");
    }
    const uint64_t frame_count = payload_size / block_align;
    if (frame_count > max_input_frames || frame_count > kAbsoluteMaxInputFrames) {
        throw std::length_error("WAV frame count exceeds configured limit");
    }
    const uint64_t sample_count = frame_count * channels;
    if (sample_count > std::numeric_limits<size_t>::max()) {
        throw std::length_error("WAV decoded sample count overflow");
    }
    AudioData output;
    output.sample_rate = sample_rate;
    output.channels = channels;
    output.samples.resize(static_cast<size_t>(sample_count));
    if (format == 1u && bits == 16u) {
        for (uint64_t index = 0; index < sample_count; ++index) {
            const int16_t value = static_cast<int16_t>(read_u16le(payload + index * 2u));
            output.samples[static_cast<size_t>(index)] = static_cast<float>(value) / 32768.0f;
        }
    } else if (format == 1u && bits == 24u) {
        for (uint64_t index = 0; index < sample_count; ++index) {
            const uint8_t *p = payload + index * 3u;
            int32_t value = static_cast<int32_t>(p[0]) |
                            (static_cast<int32_t>(p[1]) << 8u) |
                            (static_cast<int32_t>(p[2]) << 16u);
            if ((value & 0x00800000) != 0) {
                value |= static_cast<int32_t>(0xff000000u);
            }
            output.samples[static_cast<size_t>(index)] = static_cast<float>(value) / 8388608.0f;
        }
    } else if (format == 3u && bits == 32u) {
        for (uint64_t index = 0; index < sample_count; ++index) {
            const uint32_t bits_value = read_u32le(payload + index * 4u);
            float value = 0.0f;
            static_assert(sizeof(value) == sizeof(bits_value), "float32 required");
            std::memcpy(&value, &bits_value, sizeof(value));
            output.samples[static_cast<size_t>(index)] = value;
        }
    } else {
        throw std::invalid_argument("unsupported WAV encoding; expected PCM16, PCM24, or float32");
    }
    output.info.struct_size = sizeof(output.info);
    output.info.api_version = IBAUDIO_API_VERSION;
    output.info.sample_rate = sample_rate;
    output.info.channels = channels;
    output.info.frame_count = frame_count;
    float peak = 0.0f;
    for (float value : output.samples) {
        if (std::isfinite(value)) {
            peak = std::max(peak, std::fabs(value));
        }
    }
    output.info.input_peak = peak;
    output.info.output_peak = peak;
    output.info.applied_gain = 1.0f;
    return output;
}

} // namespace ibaudio
