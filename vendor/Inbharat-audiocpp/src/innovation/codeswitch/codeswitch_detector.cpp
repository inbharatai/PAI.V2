/**
 * CodeSwitchDetector: Detect English, Hindi, and Hinglish in speech.
 * 
 * Uses language identification to route to appropriate ASR model.
 * Supports code-switching detection for mixed-language utterances.
 */

#include "inbharat/ibaudio.h"
#include "../../internal.hpp"
#include "../../language/language.hpp"
#include <cmath>
#include <vector>
#include <string>
#include <map>
#include <mutex>

namespace ibaudio::innovation {

enum class Language {
    English,
    Hindi,
    Hinglish,    // Mixed English-Hindi
    Unknown
};

struct LanguageScore {
    float english = 0.0f;
    float hindi = 0.0f;
    float hinglish = 0.0f;
    float confidence = 0.0f;
};

class CodeSwitchDetector {
public:
    CodeSwitchDetector() = default;
    
    // Detect language from ASR transcript and acoustic features
    LanguageScore detect(const std::string &transcript,
                         const ibaudio_audio_view_v1 *audio) const {
        LanguageScore score;
        
        if (transcript.empty()) {
            score.confidence = 0.0f;
            return score;
        }

        // Delegate to the Bharat adaptation layer's UTF-8-correct codepoint scoring.
        // (The previous byte loop never counted Devanagari: isalpha() is false for
        // UTF-8 lead bytes, so Hindi text scored zero. Codepoints fix that.)
        const ibaudio::language::LanguageScore s = ibaudio::language::score_code_mix(transcript);
        score.english = s.english;
        score.hindi = s.hindi;
        score.hinglish = s.hinglish;

        // Confidence reflects input coverage, not loudness: full when there is letter
        // content to classify, zero when there is none. The audio argument no longer
        // drives a decorative RMS-based "confidence".
        (void)audio;
        score.confidence = (score.english > 0.0f || score.hindi > 0.0f) ? 1.0f : 0.0f;

        return score;
    }
    
    // Get primary language
    Language get_primary_language(const LanguageScore &score) const {
        if (score.hinglish > 0.4f) {
            return Language::Hinglish;
        }
        if (score.english > score.hindi) {
            return Language::English;
        }
        if (score.hindi > 0.3f) {
            return Language::Hindi;
        }
        return Language::Unknown;
    }
    
    // Check if code-switching is detected
    bool is_code_switching(const LanguageScore &score) const {
        return score.hinglish > 0.3f && score.english > 0.2f && score.hindi > 0.2f;
    }
    
    // Get recommended ASR model for detected language
    std::string get_recommended_model(const LanguageScore &score) const {
        const auto language = get_primary_language(score);
        switch (language) {
            case Language::English:
                return "moonshine-streaming-tiny";
            case Language::Hindi:
                return "hindi-fastconformer";  // If available
            case Language::Hinglish:
                return "moonshine-streaming-tiny";  // Best available for mixed
            case Language::Unknown:
                return "moonshine-streaming-tiny";  // Default
        }
        return "moonshine-streaming-tiny";
    }
};

} // namespace ibaudio::innovation

// C ABI wrapper
extern "C" {

struct ibaudio_codeswitch_detector {
    ibaudio::innovation::CodeSwitchDetector impl;
    std::mutex mutex;
};

ibaudio_codeswitch_detector_t *ibaudio_codeswitch_detector_create(void) {
    return new ibaudio_codeswitch_detector{};
}

void ibaudio_codeswitch_detector_destroy(ibaudio_codeswitch_detector_t *detector) {
    delete detector;
}

ibaudio_status_t ibaudio_codeswitch_detector_detect(
    ibaudio_codeswitch_detector_t *detector,
    const char *transcript,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_language_score_v1 *out_score) {
    if (detector == nullptr || transcript == nullptr || out_score == nullptr) {
        return IBAUDIO_STATUS_INVALID_ARGUMENT;
    }
    
    std::lock_guard<std::mutex> lock(detector->mutex);
    const auto score = detector->impl.detect(transcript, audio);
    out_score->english = score.english;
    out_score->hindi = score.hindi;
    out_score->hinglish = score.hinglish;
    out_score->confidence = score.confidence;
    
    return IBAUDIO_STATUS_OK;
}

ibaudio_status_t ibaudio_codeswitch_detector_is_code_switching(
    ibaudio_codeswitch_detector_t *detector,
    const ibaudio_language_score_v1 *score,
    uint32_t *out_is_switching) {
    if (detector == nullptr || score == nullptr || out_is_switching == nullptr) {
        return IBAUDIO_STATUS_INVALID_ARGUMENT;
    }
    
    ibaudio::innovation::LanguageScore cpp_score;
    cpp_score.english = score->english;
    cpp_score.hindi = score->hindi;
    cpp_score.hinglish = score->hinglish;
    cpp_score.confidence = score->confidence;
    
    *out_is_switching = detector->impl.is_code_switching(cpp_score) ? 1u : 0u;
    return IBAUDIO_STATUS_OK;
}

} // extern "C"
