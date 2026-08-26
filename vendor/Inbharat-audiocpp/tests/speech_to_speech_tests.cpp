#include "../src/pipeline/speech_to_speech.hpp"

#include <atomic>
#include <cassert>
#include <iostream>
#include <vector>

using namespace ibaudio::pipeline;

namespace {

AudioInput sample(std::vector<float> &audio) {
    audio.assign(1600, 0.1f);
    return {audio.data(), audio.size(), 16000};
}

StageSet passing_stages() {
    StageSet s;
    s.vad_provider = "vad"; s.stt_provider = "stt"; s.translation_provider = "nmt"; s.tts_provider = "tts";
    s.vad = [](const AudioInput &) { return VadOutput{true, 0.95f}; };
    s.stt = [](const AudioInput &, const std::string &language) {
        return TextOutput{"namaste", 0.90f, language};
    };
    s.translate = [](const std::string &, const std::string &, const std::string &target) {
        return TextOutput{"hello", 0.88f, target};
    };
    s.tts = [](const std::string &, const std::string &) {
        return AudioOutput{std::vector<float>(800, 0.2f), 24000, 0.91f};
    };
    return s;
}

void test_completed_pipeline() {
    std::vector<float> audio;
    auto result = run_speech_to_speech(sample(audio), "hi-IN", "en-IN", {}, passing_stages());
    assert(result.status == PipelineStatus::Completed);
    assert(result.transcript == "namaste" && result.output_text == "hello");
    assert(!result.audio.samples.empty() && result.audio.sample_rate == 24000);
    assert(result.events.size() == 4);
    assert(result.overall_confidence == 0.88f); // weakest admitted stage
    for (const auto &event : result.events) assert(event.latency_ms >= 0.0);
    std::cout << "PASS speech_to_speech_completed\n";
}

void test_abstention_and_no_silent_fallback() {
    std::vector<float> audio;
    StageSet s = passing_stages();
    s.stt = [](const AudioInput &, const std::string &) { return TextOutput{"maybe", 0.20f, "und"}; };
    auto low = run_speech_to_speech(sample(audio), "as-IN", "as-IN", {}, s);
    assert(low.status == PipelineStatus::Abstained);
    assert(low.audio.samples.empty());
    assert(low.events.back().detail == "stt_confidence_below_floor");

    s = passing_stages();
    s.translate = {};
    auto missing = run_speech_to_speech(sample(audio), "hi-IN", "en-IN", {}, s);
    assert(missing.status == PipelineStatus::Failed);
    assert(missing.events.back().detail == "translation_stage_missing");
    std::cout << "PASS speech_to_speech_abstention\n";
}

void test_cancellation() {
    std::vector<float> audio;
    std::atomic<bool> cancel{true};
    auto result = run_speech_to_speech(sample(audio), "hi-IN", "hi-IN", {}, passing_stages(), &cancel);
    assert(result.status == PipelineStatus::Cancelled);
    assert(result.events.empty());

    // Cancellation raised inside the long-running TTS callback must discard audio.
    cancel.store(false);
    StageSet stages = passing_stages();
    stages.tts = [&cancel](const std::string &, const std::string &) {
        cancel.store(true);
        return AudioOutput{std::vector<float>(800, 0.2f), 24000, 0.95f};
    };
    auto during_tts = run_speech_to_speech(sample(audio), "hi-IN", "hi-IN", {}, stages, &cancel);
    assert(during_tts.status == PipelineStatus::Cancelled);
    assert(during_tts.audio.samples.empty());
    assert(!during_tts.events.empty() && during_tts.events.back().detail == "cancelled_during_tts");
    std::cout << "PASS speech_to_speech_cancelled\n";
}

} // namespace

int main() {
    test_completed_pipeline();
    test_abstention_and_no_silent_fallback();
    test_cancellation();
    std::cout << "All speech-to-speech tests passed!\n";
    return 0;
}
