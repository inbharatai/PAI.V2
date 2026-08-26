/**
 * TurnManager: Semantic turn-taking and barge-in classification.
 * 
 * Distinguishes between:
 * - Continue: user is continuing their turn
 * - Yield: user is yielding the floor
 * - Backchannel: user is providing acknowledgment (mm-hm, right, I see)
 * - Barge-in: user is interrupting intentionally
 * - Accidental: noise or unintentional speech
 */

#include "inbharat/ibaudio.h"
#include "../../internal.hpp"
#include <cmath>
#include <vector>
#include <string>

namespace ibaudio::innovation {

enum class TurnAction {
    Continue,      // User is continuing their turn
    Yield,         // User is yielding the floor
    Backchannel,   // User is acknowledging without taking floor
    BargeIn,       // User is intentionally interrupting
    Accidental,    // Noise or unintentional speech
    Uncertain      // Cannot determine
};

struct TurnContext {
    float speech_duration_ms = 0.0f;
    float silence_duration_ms = 0.0f;
    float speech_energy = 0.0f;
    float pitch_trend = 0.0f;          // Rising, falling, or stable
    bool has_lexical_completion = false;
    bool is_clause_boundary = false;
    float asr_confidence = 0.0f;
    std::string partial_transcript;
    uint64_t last_speech_time_ms = 0;
};

class TurnManager {
public:
    TurnManager() = default;
    
    // Analyze current turn state
    TurnAction classify(const TurnContext &context) const {
        // Very short speech is likely backchannel or noise
        if (context.speech_duration_ms < 100.0f) {
            return TurnAction::Accidental;
        }
        
        // Short, high-confidence, low-energy speech at clause boundary is backchannel
        if (context.speech_duration_ms < 500.0f &&
            context.speech_energy < 0.3f &&
            context.asr_confidence > 0.8f &&
            context.is_clause_boundary) {
            return TurnAction::Backchannel;
        }
        
        // Long speech with rising pitch and high energy is likely barge-in
        if (context.speech_duration_ms > 300.0f &&
            context.pitch_trend > 0.5f &&
            context.speech_energy > 0.7f) {
            return TurnAction::BargeIn;
        }
        
        // Speech with lexical completion and falling pitch is yield
        if (context.has_lexical_completion &&
            context.pitch_trend < -0.3f &&
            context.is_clause_boundary) {
            return TurnAction::Yield;
        }
        
        // Speech without clear completion is continue
        if (!context.has_lexical_completion && context.speech_duration_ms > 200.0f) {
            return TurnAction::Continue;
        }
        
        return TurnAction::Uncertain;
    }
    
    // Check if we should yield based on current state
    bool should_yield(const TurnContext &context) const {
        const auto action = classify(context);
        return action == TurnAction::Yield || action == TurnAction::BargeIn;
    }
    
    // Check if we should continue speaking
    bool should_continue(const TurnContext &context) const {
        const auto action = classify(context);
        return action == TurnAction::Continue || action == TurnAction::Backchannel;
    }
    
    // Check if this is a valid barge-in
    bool is_barge_in(const TurnContext &context) const {
        return classify(context) == TurnAction::BargeIn;
    }
    
    // Check if this is a backchannel
    bool is_backchannel(const TurnContext &context) const {
        return classify(context) == TurnAction::Backchannel;
    }
};

} // namespace ibaudio::innovation

// C ABI wrapper
extern "C" {

struct ibaudio_turn_manager {
    ibaudio::innovation::TurnManager impl;
    ibaudio::innovation::TurnContext context;
};

ibaudio_turn_manager_t *ibaudio_turn_manager_create(void) {
    return new ibaudio_turn_manager{};
}

void ibaudio_turn_manager_destroy(ibaudio_turn_manager_t *manager) {
    delete manager;
}

ibaudio_status_t ibaudio_turn_manager_update(
    ibaudio_turn_manager_t *manager,
    float speech_duration_ms,
    float silence_duration_ms,
    float speech_energy,
    float pitch_trend,
    uint32_t has_lexical_completion,
    uint32_t is_clause_boundary,
    float asr_confidence) {
    if (manager == nullptr) return IBAUDIO_STATUS_INVALID_ARGUMENT;
    
    manager->context.speech_duration_ms = speech_duration_ms;
    manager->context.silence_duration_ms = silence_duration_ms;
    manager->context.speech_energy = std::max(0.0f, std::min(1.0f, speech_energy));
    manager->context.pitch_trend = std::max(-1.0f, std::min(1.0f, pitch_trend));
    manager->context.has_lexical_completion = has_lexical_completion != 0;
    manager->context.is_clause_boundary = is_clause_boundary != 0;
    manager->context.asr_confidence = std::max(0.0f, std::min(1.0f, asr_confidence));
    
    return IBAUDIO_STATUS_OK;
}

ibaudio_status_t ibaudio_turn_manager_classify(
    ibaudio_turn_manager_t *manager,
    ibaudio_turn_action_t *out_action) {
    if (manager == nullptr || out_action == nullptr) return IBAUDIO_STATUS_INVALID_ARGUMENT;
    
    const auto action = manager->impl.classify(manager->context);
    switch (action) {
        case ibaudio::innovation::TurnAction::Continue:
            *out_action = IBAUDIO_TURN_CONTINUE;
            break;
        case ibaudio::innovation::TurnAction::Yield:
            *out_action = IBAUDIO_TURN_YIELD;
            break;
        case ibaudio::innovation::TurnAction::Backchannel:
            *out_action = IBAUDIO_TURN_BACKCHANNEL;
            break;
        case ibaudio::innovation::TurnAction::BargeIn:
            *out_action = IBAUDIO_TURN_BARGE_IN;
            break;
        case ibaudio::innovation::TurnAction::Accidental:
            *out_action = IBAUDIO_TURN_ACCIDENTAL;
            break;
        case ibaudio::innovation::TurnAction::Uncertain:
            *out_action = IBAUDIO_TURN_UNCERTAIN;
            break;
    }
    
    return IBAUDIO_STATUS_OK;
}

} // extern "C"
