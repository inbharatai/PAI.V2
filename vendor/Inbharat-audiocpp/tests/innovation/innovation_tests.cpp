// Innovation-module tests for the modules that ship in the DEFAULT build:
// turn_manager, conversation_state, environment_adapter, codeswitch_detector,
// context_aware_output. These are deterministic heuristics/FSMs — not ML — and
// are tested for their actual computed behavior. The experimental placeholder
// modules (prosody_controller, voice_clone_engine, neural_codec) live in
// innovation_experimental_tests.cpp behind IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES.

#include "inbharat/ibaudio.h"

#include <cassert>
#include <cmath>
#include <cstring>
#include <iostream>
#include <vector>

namespace {

void test_turn_manager() {
    auto *manager = ibaudio_turn_manager_create();
    assert(manager != nullptr);

    // Pin branches of the rule-based classifier to their actual behavior.
    // >300ms + pitch_trend>0.5 + energy>0.7 -> BARGE_IN
    assert(ibaudio_turn_manager_update(
        manager, 400.0f, 0.0f, 0.8f, 0.6f, 0u, 0u, 0.9f) == IBAUDIO_STATUS_OK);
    ibaudio_turn_action_t action = IBAUDIO_TURN_UNCERTAIN;
    assert(ibaudio_turn_manager_classify(manager, &action) == IBAUDIO_STATUS_OK);
    assert(action == IBAUDIO_TURN_BARGE_IN);

    // <100ms -> ACCIDENTAL
    assert(ibaudio_turn_manager_update(
        manager, 50.0f, 0.0f, 0.2f, 0.0f, 0u, 0u, 0.1f) == IBAUDIO_STATUS_OK);
    assert(ibaudio_turn_manager_classify(manager, &action) == IBAUDIO_STATUS_OK);
    assert(action == IBAUDIO_TURN_ACCIDENTAL);

    ibaudio_turn_manager_destroy(manager);
    std::cout << "PASS turn_manager\n";
}

void test_conversation_state() {
    auto *state = ibaudio_conversation_state_create();
    assert(state != nullptr);

    ibaudio_conversation_state_enum_t new_state = IBAUDIO_CONVERSATION_LISTENING;
    assert(ibaudio_conversation_state_transition(state, IBAUDIO_TURN_BARGE_IN, &new_state) == IBAUDIO_STATUS_OK);
    assert(new_state == IBAUDIO_CONVERSATION_THINKING);

    uint32_t should_generate = 0u;
    assert(ibaudio_conversation_state_should_generate(state, &should_generate) == IBAUDIO_STATUS_OK);
    assert(should_generate == 1u);

    uint32_t should_listen = 0u;
    assert(ibaudio_conversation_state_should_listen(state, &should_listen) == IBAUDIO_STATUS_OK);

    ibaudio_conversation_state_destroy(state);
    std::cout << "PASS conversation_state\n";
}

void test_environment_adapter() {
    auto *adapter = ibaudio_environment_adapter_create();
    assert(adapter != nullptr);

    std::vector<float> samples(1600, 0.1f);  // 100ms at 16kHz, constant
    ibaudio_audio_view_v1 audio{};
    audio.struct_size = sizeof(audio);
    audio.api_version = IBAUDIO_API_VERSION;
    audio.interleaved_f32 = samples.data();
    audio.frame_count = 1600;
    audio.sample_rate = 16000;
    audio.channels = 1;

    ibaudio_environment_profile_v1 profile{};
    profile.struct_size = sizeof(profile);
    profile.api_version = IBAUDIO_API_VERSION;
    assert(ibaudio_environment_adapter_analyze(adapter, &audio, &profile) == IBAUDIO_STATUS_OK);
    // Constant 0.1 audio: finite, sub-0 dBFS noise floor is the only honest assert.
    assert(profile.noise_floor_dbfs < 0.0f);
    assert(std::isfinite(profile.signal_to_noise_db));

    // suppress_noise is an amplitude gate; on constant low audio it must not crash.
    assert(ibaudio_environment_adapter_suppress_noise(adapter, &audio, &profile) == IBAUDIO_STATUS_OK);

    ibaudio_environment_adapter_destroy(adapter);
    std::cout << "PASS environment_adapter\n";
}

void test_codeswitch_detector() {
    auto *detector = ibaudio_codeswitch_detector_create();
    assert(detector != nullptr);

    ibaudio_language_score_v1 score{};
    score.struct_size = sizeof(score);
    score.api_version = IBAUDIO_API_VERSION;
    // Script-ratio heuristic: ASCII -> english bucket.
    assert(ibaudio_codeswitch_detector_detect(detector, "Hello world", nullptr, &score) == IBAUDIO_STATUS_OK);
    assert(score.english > 0.5f);

    // Devanagari -> hindi bucket.
    assert(ibaudio_codeswitch_detector_detect(detector, "नमस्ते", nullptr, &score) == IBAUDIO_STATUS_OK);
    assert(score.hindi > 0.5f);

    // Mixed -> nonzero hinglish.
    assert(ibaudio_codeswitch_detector_detect(detector, "Hello नमस्ते", nullptr, &score) == IBAUDIO_STATUS_OK);
    assert(score.hinglish > 0.0f);

    uint32_t is_switching = 0u;
    assert(ibaudio_codeswitch_detector_is_code_switching(detector, &score, &is_switching) == IBAUDIO_STATUS_OK);

    ibaudio_codeswitch_detector_destroy(detector);
    std::cout << "PASS codeswitch_detector\n";
}

void test_context_aware_output() {
    auto *output = ibaudio_context_aware_output_create();
    assert(output != nullptr);

    ibaudio_output_adjustment_v1 adjustment{};
    adjustment.struct_size = sizeof(adjustment);
    adjustment.api_version = IBAUDIO_API_VERSION;
    // is_noisy requires noise > -30 dBFS strictly; use -20 so the branch fires.
    assert(ibaudio_context_aware_output_compute(
        output, -20.0f, IBAUDIO_CONVERSATION_SPEAKING, 0.8f, 0.2f, &adjustment) == IBAUDIO_STATUS_OK);
    assert(adjustment.volume_scale > 1.0f);  // noisy environment boosts volume

    std::vector<float> samples(1600, 0.5f);
    ibaudio_audio_view_v1 audio{};
    audio.struct_size = sizeof(audio);
    audio.api_version = IBAUDIO_API_VERSION;
    audio.interleaved_f32 = samples.data();
    audio.frame_count = 1600;
    audio.sample_rate = 16000;
    audio.channels = 1;

    // apply() performs volume gain with hard clip; verify samples actually scaled.
    assert(ibaudio_context_aware_output_apply(output, &audio, &adjustment) == IBAUDIO_STATUS_OK);
    assert(samples[0] > 0.5f);

    ibaudio_context_aware_output_destroy(output);
    std::cout << "PASS context_aware_output\n";
}

} // namespace

int main() {
    test_turn_manager();
    test_conversation_state();
    test_environment_adapter();
    test_codeswitch_detector();
    test_context_aware_output();

    std::cout << "All default-build innovation tests passed!\n";
    return 0;
}
