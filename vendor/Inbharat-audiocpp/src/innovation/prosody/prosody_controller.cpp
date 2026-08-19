/**
 * ProsodyController: Compact style predictor for emotion, rate, pause, emphasis, and urgency.
 * 
 * Based on ParaStyleTTS-style two-level separation of prosodic and paralinguistic style.
 * Uses interpretable controls rather than opaque embeddings.
 */

#include "inbharat/ibaudio.h"
#include "../../internal.hpp"
#include <cmath>
#include <vector>
#include <map>

namespace ibaudio::innovation {

struct ProsodyState {
    float emotion_valence = 0.0f;      // -1 (negative) to +1 (positive)
    float emotion_arousal = 0.5f;      // 0 (calm) to 1 (excited)
    float speaking_rate = 1.0f;        // 0.5 (slow) to 2.0 (fast)
    float pause_duration = 1.0f;       // Multiplier for pause length
    float emphasis_level = 0.5f;       // 0 (flat) to 1 (strong emphasis)
    float urgency = 0.5f;              // 0 (relaxed) to 1 (urgent)
    float politeness = 0.5f;           // 0 (direct) to 1 (polite)
    float confidence = 0.5f;           // 0 (uncertain) to 1 (confident)
};

class ProsodyController {
public:
    ProsodyController() = default;
    
    void set_style(const ProsodyState &state) {
        current_ = state;
    }
    
    ProsodyState get_style() const {
        return current_;
    }
    
    // Compute prosody parameters for a text span
    struct ProsodyParams {
        float f0_shift = 0.0f;         // Pitch shift in semitones
        float duration_scale = 1.0f;   // Duration multiplier
        float energy_scale = 1.0f;     // Energy multiplier
        float pause_before = 0.0f;     // Pause before this span (ms)
        float pause_after = 0.0f;      // Pause after this span (ms)
        float emphasis_boost = 0.0f;   // Emphasis boost for this span
    };
    
    ProsodyParams compute(const std::string &text, bool is_clause_end, bool is_question) const {
        (void)text;  // Unused in this simplified implementation
        ProsodyParams params;
        
        // Emotion affects pitch and energy
        params.f0_shift = current_.emotion_valence * 2.0f;  // ±2 semitones
        params.energy_scale = 0.8f + current_.emotion_arousal * 0.4f;
        
        // Rate affects duration
        params.duration_scale = 1.0f / current_.speaking_rate;
        
        // Urgency affects pause and rate
        if (current_.urgency > 0.7f) {
            params.pause_before *= 0.5f;
            params.pause_after *= 0.5f;
            params.duration_scale *= 0.9f;
        }
        
        // Politeness affects pitch contour
        if (current_.politeness > 0.7f) {
            params.f0_shift += 1.0f;  // Slightly higher pitch for politeness
        }
        
        // Confidence affects energy stability
        if (current_.confidence < 0.3f) {
            params.energy_scale *= 0.9f;  // Softer when uncertain
        }
        
        // Clause boundaries get pauses
        if (is_clause_end) {
            params.pause_after = 200.0f * current_.pause_duration;
        }
        
        // Questions get rising pitch
        if (is_question) {
            params.f0_shift += 2.0f;  // Rising intonation
        }
        
        // Emphasis gets energy boost
        params.emphasis_boost = current_.emphasis_level * 3.0f;  // Up to +3dB
        
        return params;
    }
    
    // Apply prosody to audio parameters
    void apply_to_audio(ibaudio_audio_view_v1 *audio, const ProsodyParams &params) const {
        if (audio == nullptr || audio->interleaved_f32 == nullptr) return;
        
        // Apply energy scaling to non-const copy
        float *samples = const_cast<float *>(audio->interleaved_f32);
        for (uint64_t i = 0; i < audio->frame_count * audio->channels; ++i) {
            samples[i] *= params.energy_scale;
            // Clamp to prevent clipping
            samples[i] = std::max(-1.0f, std::min(1.0f, samples[i]));
        }
    }
    
private:
    ProsodyState current_;
};

} // namespace ibaudio::innovation

// C ABI wrapper
extern "C" {

struct ibaudio_prosody_controller {
    ibaudio::innovation::ProsodyController impl;
};

ibaudio_prosody_controller_t *ibaudio_prosody_controller_create(void) {
    return new ibaudio_prosody_controller{};
}

void ibaudio_prosody_controller_destroy(ibaudio_prosody_controller_t *controller) {
    delete controller;
}

ibaudio_status_t ibaudio_prosody_controller_set_emotion(
    ibaudio_prosody_controller_t *controller,
    float valence,
    float arousal) {
    if (controller == nullptr) return IBAUDIO_STATUS_INVALID_ARGUMENT;
    ibaudio::innovation::ProsodyState state = controller->impl.get_style();
    state.emotion_valence = std::max(-1.0f, std::min(1.0f, valence));
    state.emotion_arousal = std::max(0.0f, std::min(1.0f, arousal));
    controller->impl.set_style(state);
    return IBAUDIO_STATUS_OK;
}

ibaudio_status_t ibaudio_prosody_controller_set_rate(
    ibaudio_prosody_controller_t *controller,
    float rate) {
    if (controller == nullptr) return IBAUDIO_STATUS_INVALID_ARGUMENT;
    ibaudio::innovation::ProsodyState state = controller->impl.get_style();
    state.speaking_rate = std::max(0.5f, std::min(2.0f, rate));
    controller->impl.set_style(state);
    return IBAUDIO_STATUS_OK;
}

ibaudio_status_t ibaudio_prosody_controller_set_urgency(
    ibaudio_prosody_controller_t *controller,
    float urgency) {
    if (controller == nullptr) return IBAUDIO_STATUS_INVALID_ARGUMENT;
    ibaudio::innovation::ProsodyState state = controller->impl.get_style();
    state.urgency = std::max(0.0f, std::min(1.0f, urgency));
    controller->impl.set_style(state);
    return IBAUDIO_STATUS_OK;
}

} // extern "C"
