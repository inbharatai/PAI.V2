/**
 * ContextAwareOutput: Adjust volume, rate, and style based on environment and conversation state.
 * 
 * Adapts output based on:
 * - Environment noise level
 * - Conversation state (listening, speaking, overlap)
 * - User engagement level
 * - Time of day / context
 */

#include "inbharat/ibaudio.h"
#include "../../internal.hpp"
#include <cmath>
#include <mutex>

namespace ibaudio::innovation {

struct OutputContext {
    float environment_noise_dbfs = -60.0f;
    ibaudio_conversation_state_enum_t conversation_state = static_cast<ibaudio_conversation_state_enum_t>(IBAUDIO_CONVERSATION_LISTENING);
    float user_engagement = 0.5f;        // 0 (disengaged) to 1 (engaged)
    float time_pressure = 0.0f;          // 0 (relaxed) to 1 (urgent)
    bool is_quiet_environment = false;
    bool is_noisy_environment = false;
};

class ContextAwareOutput {
public:
    ContextAwareOutput() = default;
    
    // Compute output adjustments
    struct OutputAdjustment {
        float volume_scale = 1.0f;       // Volume multiplier
        float rate_scale = 1.0f;         // Speaking rate multiplier
        float emphasis_scale = 1.0f;     // Emphasis multiplier
        float pause_scale = 1.0f;        // Pause duration multiplier
    };
    
    OutputAdjustment compute(const OutputContext &context) const {
        OutputAdjustment adjustment;
        
        // Adjust volume based on environment noise
        if (context.is_noisy_environment) {
            adjustment.volume_scale = 1.2f;  // Increase volume in noisy environments
        } else if (context.is_quiet_environment) {
            adjustment.volume_scale = 0.8f;  // Decrease volume in quiet environments
        }
        
        // Adjust rate based on conversation state
        switch (context.conversation_state) {
            case IBAUDIO_CONVERSATION_LISTENING:
                adjustment.rate_scale = 1.0f;
                break;
            case IBAUDIO_CONVERSATION_THINKING:
                adjustment.rate_scale = 0.9f;  // Slightly slower when thinking
                break;
            case IBAUDIO_CONVERSATION_SPEAKING:
                adjustment.rate_scale = 1.0f;
                break;
            case IBAUDIO_CONVERSATION_OVERLAP:
                adjustment.rate_scale = 1.1f;  // Slightly faster in overlap
                break;
            case IBAUDIO_CONVERSATION_YIELDING:
                adjustment.rate_scale = 0.8f;  // Slower when yielding
                break;
        }
        
        // Adjust emphasis based on user engagement
        adjustment.emphasis_scale = 0.8f + context.user_engagement * 0.4f;
        
        // Adjust pauses based on time pressure
        adjustment.pause_scale = 1.0f - context.time_pressure * 0.5f;
        
        return adjustment;
    }
    
    // Apply adjustments to audio
    void apply(ibaudio_audio_view_v1 *audio, const OutputAdjustment &adjustment) const {
        if (audio == nullptr || audio->interleaved_f32 == nullptr) {
            return;
        }
        
        // Apply volume scaling
        float *samples = const_cast<float *>(audio->interleaved_f32);
        for (uint64_t i = 0; i < audio->frame_count * audio->channels; ++i) {
            samples[i] *= adjustment.volume_scale;
            // Clamp to prevent clipping
            samples[i] = std::max(-1.0f, std::min(1.0f, samples[i]));
        }
    }
    
private:
    OutputContext last_context_;
};

} // namespace ibaudio::innovation

// C ABI wrapper
extern "C" {

struct ibaudio_context_aware_output {
    ibaudio::innovation::ContextAwareOutput impl;
    std::mutex mutex;
};

ibaudio_context_aware_output_t *ibaudio_context_aware_output_create(void) {
    return new ibaudio_context_aware_output{};
}

void ibaudio_context_aware_output_destroy(ibaudio_context_aware_output_t *output) {
    delete output;
}

ibaudio_status_t ibaudio_context_aware_output_compute(
    ibaudio_context_aware_output_t *output,
    float environment_noise_dbfs,
    ibaudio_conversation_state_enum_t conversation_state,
    float user_engagement,
    float time_pressure,
    ibaudio_output_adjustment_v1 *out_adjustment) {
    if (output == nullptr || out_adjustment == nullptr) {
        return IBAUDIO_STATUS_INVALID_ARGUMENT;
    }
    
    std::lock_guard<std::mutex> lock(output->mutex);
    
    ibaudio::innovation::OutputContext context;
    context.environment_noise_dbfs = environment_noise_dbfs;
    context.conversation_state = conversation_state;
    context.user_engagement = std::max(0.0f, std::min(1.0f, user_engagement));
    context.time_pressure = std::max(0.0f, std::min(1.0f, time_pressure));
    context.is_quiet_environment = environment_noise_dbfs < -50.0f;
    context.is_noisy_environment = environment_noise_dbfs >= -30.0f;
    
    const auto adjustment = output->impl.compute(context);
    out_adjustment->volume_scale = adjustment.volume_scale;
    out_adjustment->rate_scale = adjustment.rate_scale;
    out_adjustment->emphasis_scale = adjustment.emphasis_scale;
    out_adjustment->pause_scale = adjustment.pause_scale;
    
    return IBAUDIO_STATUS_OK;
}

ibaudio_status_t ibaudio_context_aware_output_apply(
    ibaudio_context_aware_output_t *output,
    ibaudio_audio_view_v1 *audio,
    const ibaudio_output_adjustment_v1 *adjustment) {
    if (output == nullptr || audio == nullptr || adjustment == nullptr) {
        return IBAUDIO_STATUS_INVALID_ARGUMENT;
    }
    
    std::lock_guard<std::mutex> lock(output->mutex);
    
    ibaudio::innovation::ContextAwareOutput::OutputAdjustment cpp_adjustment;
    cpp_adjustment.volume_scale = adjustment->volume_scale;
    cpp_adjustment.rate_scale = adjustment->rate_scale;
    cpp_adjustment.emphasis_scale = adjustment->emphasis_scale;
    cpp_adjustment.pause_scale = adjustment->pause_scale;
    
    output->impl.apply(audio, cpp_adjustment);
    return IBAUDIO_STATUS_OK;
}

} // extern "C"
