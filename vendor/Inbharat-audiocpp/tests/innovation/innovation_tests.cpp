#include "inbharat/ibaudio.h"

#include <cassert>
#include <cmath>
#include <cstring>
#include <iostream>
#include <vector>

namespace {

void test_prosody_controller() {
    auto *controller = ibaudio_prosody_controller_create();
    assert(controller != nullptr);
    
    // Test emotion setting
    assert(ibaudio_prosody_controller_set_emotion(controller, 0.5f, 0.8f) == IBAUDIO_STATUS_OK);
    
    // Test rate setting
    assert(ibaudio_prosody_controller_set_rate(controller, 1.2f) == IBAUDIO_STATUS_OK);
    
    // Test urgency setting
    assert(ibaudio_prosody_controller_set_urgency(controller, 0.9f) == IBAUDIO_STATUS_OK);
    
    ibaudio_prosody_controller_destroy(controller);
    std::cout << "PASS prosody_controller\n";
}

void test_turn_manager() {
    auto *manager = ibaudio_turn_manager_create();
    assert(manager != nullptr);
    
    // Test update
    assert(ibaudio_turn_manager_update(
        manager, 300.0f, 0.0f, 0.8f, 0.6f, 1u, 1u, 0.9f) == IBAUDIO_STATUS_OK);
    
    // Test classification
    ibaudio_turn_action_t action = IBAUDIO_TURN_UNCERTAIN;
    assert(ibaudio_turn_manager_classify(manager, &action) == IBAUDIO_STATUS_OK);
    assert(action == IBAUDIO_TURN_BARGE_IN || action == IBAUDIO_TURN_CONTINUE);
    
    ibaudio_turn_manager_destroy(manager);
    std::cout << "PASS turn_manager\n";
}

void test_conversation_state() {
    auto *state = ibaudio_conversation_state_create();
    assert(state != nullptr);
    
    // Test transition
    ibaudio_conversation_state_enum_t new_state = IBAUDIO_CONVERSATION_LISTENING;
    assert(ibaudio_conversation_state_transition(state, IBAUDIO_TURN_BARGE_IN, &new_state) == IBAUDIO_STATUS_OK);
    assert(new_state == IBAUDIO_CONVERSATION_THINKING);
    
    // Test should_generate
    uint32_t should_generate = 0u;
    assert(ibaudio_conversation_state_should_generate(state, &should_generate) == IBAUDIO_STATUS_OK);
    assert(should_generate == 1u);
    
    // Test should_listen
    uint32_t should_listen = 0u;
    assert(ibaudio_conversation_state_should_listen(state, &should_listen) == IBAUDIO_STATUS_OK);
    
    ibaudio_conversation_state_destroy(state);
    std::cout << "PASS conversation_state\n";
}

void test_environment_adapter() {
    auto *adapter = ibaudio_environment_adapter_create();
    assert(adapter != nullptr);
    
    // Create test audio
    std::vector<float> samples(1600, 0.1f);  // 100ms at 16kHz
    ibaudio_audio_view_v1 audio{};
    audio.struct_size = sizeof(audio);
    audio.api_version = IBAUDIO_API_VERSION;
    audio.interleaved_f32 = samples.data();
    audio.frame_count = 1600;
    audio.sample_rate = 16000;
    audio.channels = 1;
    
    // Test analyze
    ibaudio_environment_profile_v1 profile{};
    profile.struct_size = sizeof(profile);
    profile.api_version = IBAUDIO_API_VERSION;
    assert(ibaudio_environment_adapter_analyze(adapter, &audio, &profile) == IBAUDIO_STATUS_OK);
    assert(profile.noise_floor_dbfs < 0.0f);
    
    // Test suppress_noise
    assert(ibaudio_environment_adapter_suppress_noise(adapter, &audio, &profile) == IBAUDIO_STATUS_OK);
    
    ibaudio_environment_adapter_destroy(adapter);
    std::cout << "PASS environment_adapter\n";
}

void test_voice_clone_engine() {
    auto *engine = ibaudio_voice_clone_engine_create();
    assert(engine != nullptr);
    
    // Create test audio (3 seconds at 16kHz)
    std::vector<float> samples(48000, 0.1f);
    ibaudio_audio_view_v1 audio{};
    audio.struct_size = sizeof(audio);
    audio.api_version = IBAUDIO_API_VERSION;
    audio.interleaved_f32 = samples.data();
    audio.frame_count = 48000;
    audio.sample_rate = 16000;
    audio.channels = 1;
    
    // Test enroll without consent (should fail)
    assert(ibaudio_voice_clone_engine_enroll(engine, &audio, "test-speaker", 0u) == IBAUDIO_STATUS_INVALID_ARGUMENT);
    
    // Test enroll with consent (should succeed)
    assert(ibaudio_voice_clone_engine_enroll(engine, &audio, "test-speaker", 1u) == IBAUDIO_STATUS_OK);
    
    // Test verify consent
    assert(ibaudio_voice_clone_engine_verify_consent(engine, "test-speaker") == IBAUDIO_STATUS_OK);
    
    // Test delete
    assert(ibaudio_voice_clone_engine_delete_speaker(engine, "test-speaker") == IBAUDIO_STATUS_OK);
    
    ibaudio_voice_clone_engine_destroy(engine);
    std::cout << "PASS voice_clone_engine\n";
}

void test_codeswitch_detector() {
    auto *detector = ibaudio_codeswitch_detector_create();
    assert(detector != nullptr);
    
    // Test English detection
    ibaudio_language_score_v1 score{};
    score.struct_size = sizeof(score);
    score.api_version = IBAUDIO_API_VERSION;
    assert(ibaudio_codeswitch_detector_detect(detector, "Hello world", nullptr, &score) == IBAUDIO_STATUS_OK);
    assert(score.english > 0.5f);
    
    // Test Hindi detection (simplified)
    assert(ibaudio_codeswitch_detector_detect(detector, "नमस्ते", nullptr, &score) == IBAUDIO_STATUS_OK);
    assert(score.hindi > 0.5f);
    
    // Test Hinglish detection
    assert(ibaudio_codeswitch_detector_detect(detector, "Hello नमस्ते", nullptr, &score) == IBAUDIO_STATUS_OK);
    assert(score.hinglish > 0.0f);
    
    // Test code-switching detection
    uint32_t is_switching = 0u;
    assert(ibaudio_codeswitch_detector_is_code_switching(detector, &score, &is_switching) == IBAUDIO_STATUS_OK);
    
    ibaudio_codeswitch_detector_destroy(detector);
    std::cout << "PASS codeswitch_detector\n";
}

void test_neural_codec() {
    auto *codec = ibaudio_neural_codec_create(24000, 80, 6.0f);
    assert(codec != nullptr);
    
    // Create test audio (1 second at 24kHz)
    std::vector<float> samples(24000, 0.1f);
    ibaudio_audio_view_v1 audio{};
    audio.struct_size = sizeof(audio);
    audio.api_version = IBAUDIO_API_VERSION;
    audio.interleaved_f32 = samples.data();
    audio.frame_count = 24000;
    audio.sample_rate = 24000;
    audio.channels = 1;
    
    // Test encode
    ibaudio_buffer_t *encoded = nullptr;
    assert(ibaudio_neural_codec_encode(codec, &audio, &encoded) == IBAUDIO_STATUS_OK);
    assert(encoded != nullptr);
    
    // Test decode
    ibaudio_buffer_t *decoded = nullptr;
    assert(ibaudio_neural_codec_decode(codec, encoded, &decoded) == IBAUDIO_STATUS_OK);
    assert(decoded != nullptr);
    
    // Test bitrate
    float bitrate = 0.0f;
    assert(ibaudio_neural_codec_get_bitrate(codec, &bitrate) == IBAUDIO_STATUS_OK);
    assert(bitrate > 0.0f);
    
    ibaudio_buffer_release(&encoded);
    ibaudio_buffer_release(&decoded);
    ibaudio_neural_codec_destroy(codec);
    std::cout << "PASS neural_codec\n";
}

void test_context_aware_output() {
    auto *output = ibaudio_context_aware_output_create();
    assert(output != nullptr);
    
    // Test compute adjustment
    ibaudio_output_adjustment_v1 adjustment{};
    adjustment.struct_size = sizeof(adjustment);
    adjustment.api_version = IBAUDIO_API_VERSION;
    assert(ibaudio_context_aware_output_compute(
        output, -30.0f, IBAUDIO_CONVERSATION_SPEAKING, 0.8f, 0.2f, &adjustment) == IBAUDIO_STATUS_OK);
    assert(adjustment.volume_scale > 1.0f);  // Should increase volume in noisy environment
    
    // Test apply adjustment
    std::vector<float> samples(1600, 0.5f);
    ibaudio_audio_view_v1 audio{};
    audio.struct_size = sizeof(audio);
    audio.api_version = IBAUDIO_API_VERSION;
    audio.interleaved_f32 = samples.data();
    audio.frame_count = 1600;
    audio.sample_rate = 16000;
    audio.channels = 1;
    
    assert(ibaudio_context_aware_output_apply(output, &audio, &adjustment) == IBAUDIO_STATUS_OK);
    
    ibaudio_context_aware_output_destroy(output);
    std::cout << "PASS context_aware_output\n";
}

} // namespace

int main() {
    test_prosody_controller();
    test_turn_manager();
    test_conversation_state();
    test_environment_adapter();
    test_voice_clone_engine();
    test_codeswitch_detector();
    test_neural_codec();
    test_context_aware_output();
    
    std::cout << "All innovation tests passed!\n";
    return 0;
}
