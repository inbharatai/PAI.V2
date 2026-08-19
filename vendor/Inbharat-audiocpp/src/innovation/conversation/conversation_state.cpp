/**
 * ConversationStateMachine: Full-duplex dialogue state management.
 * 
 * States: LISTENING, THINKING, SPEAKING, OVERLAP, YIELDING
 * Transitions based on turn-taking signals, ASR results, and TTS state.
 */

#include "inbharat/ibaudio.h"
#include "../../internal.hpp"
#include <chrono>
#include <mutex>

namespace ibaudio::innovation {

enum class ConversationState {
    Listening,    // Waiting for user input
    Thinking,     // Processing user input, planning response
    Speaking,     // Generating and playing response
    Overlap,      // User is speaking while agent is speaking
    Yielding      // Agent is yielding the floor to user
};

struct ConversationContext {
    ConversationState current_state = ConversationState::Listening;
    uint64_t state_entered_ms = 0;
    uint64_t last_user_speech_ms = 0;
    uint64_t last_agent_speech_ms = 0;
    bool user_is_speaking = false;
    bool agent_is_speaking = false;
    bool user_wants_floor = false;
    bool agent_has_more = false;
    float overlap_duration_ms = 0.0f;
    float overlap_threshold_ms = 200.0f;
};

class ConversationStateMachine {
public:
    ConversationStateMachine() {
        context_.state_entered_ms = now_ms();
    }
    
    // Get current context (for external use)
    ConversationContext get_context() const {
        return context_;
    }
    
    // Set context (for external use)
    void set_context(const ConversationContext &context) {
        context_ = context;
    }
    
    // Process a state transition
    ConversationState transition(ConversationContext &context, ibaudio_turn_action_t action) {
        const uint64_t now = now_ms();
        
        switch (context.current_state) {
            case ConversationState::Listening:
                if (action == IBAUDIO_TURN_BARGE_IN || action == IBAUDIO_TURN_CONTINUE) {
                    context.current_state = ConversationState::Thinking;
                    context.state_entered_ms = now;
                }
                break;
                
            case ConversationState::Thinking:
                if (action == IBAUDIO_TURN_YIELD) {
                    context.current_state = ConversationState::Speaking;
                    context.state_entered_ms = now;
                } else if (action == IBAUDIO_TURN_BARGE_IN) {
                    context.current_state = ConversationState::Overlap;
                    context.state_entered_ms = now;
                }
                break;
                
            case ConversationState::Speaking:
                if (action == IBAUDIO_TURN_BARGE_IN) {
                    context.current_state = ConversationState::Overlap;
                    context.state_entered_ms = now;
                } else if (!context.agent_has_more) {
                    context.current_state = ConversationState::Yielding;
                    context.state_entered_ms = now;
                }
                break;
                
            case ConversationState::Overlap:
                if (action == IBAUDIO_TURN_YIELD) {
                    context.current_state = ConversationState::Yielding;
                    context.state_entered_ms = now;
                } else if (action == IBAUDIO_TURN_CONTINUE && 
                          context.overlap_duration_ms > context.overlap_threshold_ms) {
                    context.current_state = ConversationState::Listening;
                    context.state_entered_ms = now;
                }
                break;
                
            case ConversationState::Yielding:
                if (action == IBAUDIO_TURN_CONTINUE || action == IBAUDIO_TURN_BARGE_IN) {
                    context.current_state = ConversationState::Listening;
                    context.state_entered_ms = now;
                }
                break;
        }
        
        return context.current_state;
    }
    
    // Get current state
    ConversationState get_state() const {
        return context_.current_state;
    }
    
    // Check if we should generate a response
    bool should_generate_response() const {
        return context_.current_state == ConversationState::Thinking;
    }
    
    // Check if we should play audio
    bool should_play_audio() const {
        return context_.current_state == ConversationState::Speaking ||
               context_.current_state == ConversationState::Overlap;
    }
    
    // Check if we should listen for input
    bool should_listen() const {
        return context_.current_state == ConversationState::Listening ||
               context_.current_state == ConversationState::Overlap;
    }
    
    // Check if we should yield
    bool should_yield() const {
        return context_.current_state == ConversationState::Yielding;
    }
    
    // Update overlap duration
    void update_overlap(float duration_ms) {
        context_.overlap_duration_ms = duration_ms;
    }
    
    // Set whether agent has more to say
    void set_agent_has_more(bool has_more) {
        context_.agent_has_more = has_more;
    }
    
private:
    ConversationContext context_;
    
    static uint64_t now_ms() {
        return static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::steady_clock::now().time_since_epoch()).count());
    }
};

} // namespace ibaudio::innovation

// C ABI wrapper
extern "C" {

struct ibaudio_conversation_state {
    ibaudio::innovation::ConversationStateMachine impl;
    std::mutex mutex;
};

ibaudio_conversation_state_t *ibaudio_conversation_state_create(void) {
    return new ibaudio_conversation_state{};
}

void ibaudio_conversation_state_destroy(ibaudio_conversation_state_t *state) {
    delete state;
}

ibaudio_status_t ibaudio_conversation_state_transition(
    ibaudio_conversation_state_t *state,
    ibaudio_turn_action_t action,
    ibaudio_conversation_state_enum_t *out_state) {
    if (state == nullptr || out_state == nullptr) return IBAUDIO_STATUS_INVALID_ARGUMENT;
    
    std::lock_guard<std::mutex> lock(state->mutex);
    ibaudio::innovation::ConversationContext context = state->impl.get_context();
    const auto new_state = state->impl.transition(context, action);
    state->impl.set_context(context);
    
    switch (new_state) {
        case ibaudio::innovation::ConversationState::Listening:
            *out_state = static_cast<ibaudio_conversation_state_enum_t>(IBAUDIO_CONVERSATION_LISTENING);
            break;
        case ibaudio::innovation::ConversationState::Thinking:
            *out_state = static_cast<ibaudio_conversation_state_enum_t>(IBAUDIO_CONVERSATION_THINKING);
            break;
        case ibaudio::innovation::ConversationState::Speaking:
            *out_state = static_cast<ibaudio_conversation_state_enum_t>(IBAUDIO_CONVERSATION_SPEAKING);
            break;
        case ibaudio::innovation::ConversationState::Overlap:
            *out_state = static_cast<ibaudio_conversation_state_enum_t>(IBAUDIO_CONVERSATION_OVERLAP);
            break;
        case ibaudio::innovation::ConversationState::Yielding:
            *out_state = static_cast<ibaudio_conversation_state_enum_t>(IBAUDIO_CONVERSATION_YIELDING);
            break;
    }
    
    return IBAUDIO_STATUS_OK;
}

ibaudio_status_t ibaudio_conversation_state_should_generate(
    ibaudio_conversation_state_t *state,
    uint32_t *out_should_generate) {
    if (state == nullptr || out_should_generate == nullptr) return IBAUDIO_STATUS_INVALID_ARGUMENT;
    *out_should_generate = state->impl.should_generate_response() ? 1u : 0u;
    return IBAUDIO_STATUS_OK;
}

ibaudio_status_t ibaudio_conversation_state_should_listen(
    ibaudio_conversation_state_t *state,
    uint32_t *out_should_listen) {
    if (state == nullptr || out_should_listen == nullptr) return IBAUDIO_STATUS_INVALID_ARGUMENT;
    *out_should_listen = state->impl.should_listen() ? 1u : 0u;
    return IBAUDIO_STATUS_OK;
}

} // extern "C"
