#ifndef INBHARAT_IBAUDIO_SPEECH_TO_SPEECH_HPP
#define INBHARAT_IBAUDIO_SPEECH_TO_SPEECH_HPP

// Stage-gated speech-to-speech orchestration. This is provider-neutral control logic:
// VAD -> STT -> optional translation/normalization -> TTS. Every stage reports confidence
// and measured latency; low-confidence or cancelled runs stop without fabricating audio.

#include <atomic>
#include <cstdint>
#include <functional>
#include <string>
#include <vector>

namespace ibaudio::pipeline {

enum class PipelineStatus { Completed, Abstained, Cancelled, Failed };

struct AudioInput {
    const float *samples = nullptr;
    uint64_t frame_count = 0;
    uint32_t sample_rate = 16000;
};

struct VadOutput { bool speech = false; float confidence = 0.0f; };
struct TextOutput { std::string text; float confidence = 0.0f; std::string language; };
struct AudioOutput { std::vector<float> samples; uint32_t sample_rate = 0; float confidence = 0.0f; };

struct StageEvent {
    std::string stage;
    std::string provider_id;
    PipelineStatus status = PipelineStatus::Failed;
    float confidence = 0.0f;
    double latency_ms = 0.0;
    std::string detail;
};

struct PipelinePolicy {
    float min_vad_confidence = 0.50f;
    float min_stt_confidence = 0.65f;
    float min_translation_confidence = 0.65f;
    float min_tts_confidence = 0.65f;
    bool translate_when_languages_differ = true;
};

struct PipelineResult {
    PipelineStatus status = PipelineStatus::Failed;
    std::string transcript;
    std::string output_text;
    AudioOutput audio;
    float overall_confidence = 0.0f;
    std::vector<StageEvent> events;
};

using VadStage = std::function<VadOutput(const AudioInput &)>;
using SttStage = std::function<TextOutput(const AudioInput &, const std::string &source_language)>;
using TranslationStage = std::function<TextOutput(const std::string &, const std::string &source_language,
                                                   const std::string &target_language)>;
using TtsStage = std::function<AudioOutput(const std::string &, const std::string &target_language)>;

struct StageSet {
    std::string vad_provider;
    std::string stt_provider;
    std::string translation_provider;
    std::string tts_provider;
    VadStage vad;
    SttStage stt;
    TranslationStage translate;
    TtsStage tts;
};

PipelineResult run_speech_to_speech(const AudioInput &input,
                                    const std::string &source_language,
                                    const std::string &target_language,
                                    const PipelinePolicy &policy,
                                    const StageSet &stages,
                                    const std::atomic<bool> *cancel = nullptr);

} // namespace ibaudio::pipeline

#endif // INBHARAT_IBAUDIO_SPEECH_TO_SPEECH_HPP
