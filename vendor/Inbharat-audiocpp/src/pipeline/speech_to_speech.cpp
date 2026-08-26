#include "speech_to_speech.hpp"

#include <algorithm>
#include <chrono>
#include <cmath>

namespace ibaudio::pipeline {
namespace {
using Clock = std::chrono::steady_clock;

bool cancelled(const std::atomic<bool> *flag) {
    return flag != nullptr && flag->load(std::memory_order_relaxed);
}

float valid_confidence(float confidence) {
    return std::isfinite(confidence) ? std::clamp(confidence, 0.0f, 1.0f) : 0.0f;
}

template <typename Fn>
auto timed(Fn &&fn, double &latency_ms) {
    const auto start = Clock::now();
    auto result = fn();
    latency_ms = std::chrono::duration<double, std::milli>(Clock::now() - start).count();
    return result;
}

StageEvent event(const std::string &stage, const std::string &provider, PipelineStatus status,
                 float confidence, double latency, const std::string &detail) {
    return {stage, provider, status, valid_confidence(confidence), latency, detail};
}

} // namespace

PipelineResult run_speech_to_speech(const AudioInput &input,
                                    const std::string &source_language,
                                    const std::string &target_language,
                                    const PipelinePolicy &policy,
                                    const StageSet &stages,
                                    const std::atomic<bool> *cancel) {
    PipelineResult out;
    if (input.samples == nullptr || input.frame_count == 0 || input.sample_rate == 0 ||
        source_language.empty() || target_language.empty() || !stages.vad || !stages.stt || !stages.tts) {
        out.status = PipelineStatus::Failed;
        return out;
    }
    if (cancelled(cancel)) { out.status = PipelineStatus::Cancelled; return out; }

    double latency = 0.0;
    const VadOutput vad = timed([&] { return stages.vad(input); }, latency);
    const float vad_conf = valid_confidence(vad.confidence);
    if (!vad.speech || vad_conf < policy.min_vad_confidence) {
        out.status = PipelineStatus::Abstained;
        out.events.push_back(event("vad", stages.vad_provider, out.status, vad_conf, latency,
            vad.speech ? "vad_confidence_below_floor" : "no_speech"));
        return out;
    }
    out.events.push_back(event("vad", stages.vad_provider, PipelineStatus::Completed, vad_conf, latency, "speech"));
    if (cancelled(cancel)) { out.status = PipelineStatus::Cancelled; return out; }

    const TextOutput stt = timed([&] { return stages.stt(input, source_language); }, latency);
    const float stt_conf = valid_confidence(stt.confidence);
    if (stt.text.empty() || stt_conf < policy.min_stt_confidence) {
        out.status = PipelineStatus::Abstained;
        out.events.push_back(event("stt", stages.stt_provider, out.status, stt_conf, latency,
            stt.text.empty() ? "empty_transcript" : "stt_confidence_below_floor"));
        return out;
    }
    out.transcript = stt.text;
    out.output_text = stt.text;
    out.events.push_back(event("stt", stages.stt_provider, PipelineStatus::Completed, stt_conf, latency, stt.language));
    float overall = std::min(vad_conf, stt_conf);
    if (cancelled(cancel)) { out.status = PipelineStatus::Cancelled; return out; }

    if (policy.translate_when_languages_differ && source_language != target_language) {
        if (!stages.translate) {
            out.status = PipelineStatus::Failed;
            out.events.push_back(event("translation", stages.translation_provider, out.status, 0.0f, 0.0,
                "translation_stage_missing"));
            return out;
        }
        const TextOutput translated = timed([&] {
            return stages.translate(stt.text, source_language, target_language);
        }, latency);
        const float translation_conf = valid_confidence(translated.confidence);
        if (translated.text.empty() || translation_conf < policy.min_translation_confidence) {
            out.status = PipelineStatus::Abstained;
            out.events.push_back(event("translation", stages.translation_provider, out.status,
                translation_conf, latency, translated.text.empty() ? "empty_translation" : "translation_confidence_below_floor"));
            return out;
        }
        out.output_text = translated.text;
        overall = std::min(overall, translation_conf);
        out.events.push_back(event("translation", stages.translation_provider, PipelineStatus::Completed,
            translation_conf, latency, translated.language));
    }
    if (cancelled(cancel)) { out.status = PipelineStatus::Cancelled; return out; }

    const AudioOutput audio = timed([&] { return stages.tts(out.output_text, target_language); }, latency);
    // TTS can be the longest stage. If cancellation/barge-in arrived during the call,
    // discard the returned audio and never publish a completed result.
    if (cancelled(cancel)) {
        out.status = PipelineStatus::Cancelled;
        out.events.push_back(event("tts", stages.tts_provider, out.status, 0.0f, latency,
            "cancelled_during_tts"));
        return out;
    }
    const float tts_conf = valid_confidence(audio.confidence);
    if (audio.samples.empty() || audio.sample_rate == 0 || tts_conf < policy.min_tts_confidence) {
        out.status = PipelineStatus::Abstained;
        out.events.push_back(event("tts", stages.tts_provider, out.status, tts_conf, latency,
            audio.samples.empty() ? "empty_audio" : "tts_confidence_below_floor"));
        return out;
    }
    out.audio = audio;
    overall = std::min(overall, tts_conf);
    out.overall_confidence = overall;
    out.events.push_back(event("tts", stages.tts_provider, PipelineStatus::Completed, tts_conf, latency,
        target_language));
    out.status = PipelineStatus::Completed;
    return out;
}

} // namespace ibaudio::pipeline
