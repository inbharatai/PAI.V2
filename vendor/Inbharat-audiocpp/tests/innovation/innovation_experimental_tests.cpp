// Experimental research-module tests — built ONLY when
// IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES=ON. These modules are placeholders
// (prosody has no inference output path, voice_clone has no synthesis path,
// neural_codec contains no neural network). Tests here pin their actual behavior,
// including the honest consent gate — they do not assert any ML quality.

#include "inbharat/ibaudio.h"

#include <cassert>
#include <iostream>
#include <vector>

namespace {

void test_prosody_controller() {
    auto *controller = ibaudio_prosody_controller_create();
    assert(controller != nullptr);

    // Setters store clamped parameters; there is no public compute/output path.
    assert(ibaudio_prosody_controller_set_emotion(controller, 0.5f, 0.8f) == IBAUDIO_STATUS_OK);
    assert(ibaudio_prosody_controller_set_rate(controller, 1.2f) == IBAUDIO_STATUS_OK);
    assert(ibaudio_prosody_controller_set_urgency(controller, 0.9f) == IBAUDIO_STATUS_OK);

    ibaudio_prosody_controller_destroy(controller);
    std::cout << "PASS prosody_controller\n";
}

void test_voice_clone_engine() {
    auto *engine = ibaudio_voice_clone_engine_create();
    assert(engine != nullptr);

    std::vector<float> samples(48000, 0.1f);  // 3s at 16kHz
    ibaudio_audio_view_v1 audio{};
    audio.struct_size = sizeof(audio);
    audio.api_version = IBAUDIO_API_VERSION;
    audio.interleaved_f32 = samples.data();
    audio.frame_count = 48000;
    audio.sample_rate = 16000;
    audio.channels = 1;

    // Consent gate: enrollment without consent must be denied.
    assert(ibaudio_voice_clone_engine_enroll(engine, &audio, "test-speaker", 0u) == IBAUDIO_STATUS_PERMISSION_DENIED);

    // With consent the enrollment registry accepts the speaker.
    assert(ibaudio_voice_clone_engine_enroll(engine, &audio, "test-speaker", 1u) == IBAUDIO_STATUS_OK);
    assert(ibaudio_voice_clone_engine_verify_consent(engine, "test-speaker") == IBAUDIO_STATUS_OK);
    assert(ibaudio_voice_clone_engine_delete_speaker(engine, "test-speaker") == IBAUDIO_STATUS_OK);

    ibaudio_voice_clone_engine_destroy(engine);
    std::cout << "PASS voice_clone_engine\n";
}

void test_neural_codec() {
    auto *codec = ibaudio_neural_codec_create(24000, 80, 6.0f);
    assert(codec != nullptr);

    std::vector<float> samples(24000, 0.1f);  // 1s at 24kHz
    ibaudio_audio_view_v1 audio{};
    audio.struct_size = sizeof(audio);
    audio.api_version = IBAUDIO_API_VERSION;
    audio.interleaved_f32 = samples.data();
    audio.frame_count = 24000;
    audio.sample_rate = 24000;
    audio.channels = 1;

    // Placeholder: assert buffer plumbing only, not fidelity (decode != encode input).
    ibaudio_buffer_t *encoded = nullptr;
    assert(ibaudio_neural_codec_encode(codec, &audio, &encoded) == IBAUDIO_STATUS_OK);
    assert(encoded != nullptr);

    ibaudio_buffer_t *decoded = nullptr;
    assert(ibaudio_neural_codec_decode(codec, encoded, &decoded) == IBAUDIO_STATUS_OK);
    assert(decoded != nullptr);

    float bitrate = 0.0f;
    assert(ibaudio_neural_codec_get_bitrate(codec, &bitrate) == IBAUDIO_STATUS_OK);
    assert(bitrate > 0.0f);

    ibaudio_buffer_release(&encoded);
    ibaudio_buffer_release(&decoded);
    ibaudio_neural_codec_destroy(codec);
    std::cout << "PASS neural_codec\n";
}

} // namespace

int main() {
    test_prosody_controller();
    test_voice_clone_engine();
    test_neural_codec();

    std::cout << "All experimental research-module tests passed!\n";
    return 0;
}
