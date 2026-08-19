/**
 * CodeSwitchDetector: Detect English, Hindi, and Hinglish in speech.
 * 
 * Uses language identification to route to appropriate ASR model.
 * Supports code-switching detection for mixed-language utterances.
 */

#include "inbharat/ibaudio.h"
#include "../../internal.hpp"
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
        
        // Decode UTF-8 and count script code points. The previous byte-wise
        // `std::isalpha` logic never recognized Devanagari in the default C
        // locale and incorrectly treated any non-ASCII alphabetic byte as
        // Hindi. Keep this research heuristic explicit and Unicode-correct.
        int latin_count = 0;
        int devanagari_count = 0;
        int digit_count = 0;
        int space_count = 0;
        const auto bytes = reinterpret_cast<const unsigned char *>(transcript.data());
        size_t index = 0u;
        while (index < transcript.size()) {
            uint32_t codepoint = 0u;
            size_t width = 0u;
            const unsigned char first = bytes[index];
            if (first < 0x80u) {
                codepoint = first;
                width = 1u;
            } else if ((first & 0xE0u) == 0xC0u && index + 1u < transcript.size()) {
                codepoint = static_cast<uint32_t>(first & 0x1Fu) << 6u;
                codepoint |= static_cast<uint32_t>(bytes[index + 1u] & 0x3Fu);
                width = 2u;
            } else if ((first & 0xF0u) == 0xE0u && index + 2u < transcript.size()) {
                codepoint = static_cast<uint32_t>(first & 0x0Fu) << 12u;
                codepoint |= static_cast<uint32_t>(bytes[index + 1u] & 0x3Fu) << 6u;
                codepoint |= static_cast<uint32_t>(bytes[index + 2u] & 0x3Fu);
                width = 3u;
            } else if ((first & 0xF8u) == 0xF0u && index + 3u < transcript.size()) {
                codepoint = static_cast<uint32_t>(first & 0x07u) << 18u;
                codepoint |= static_cast<uint32_t>(bytes[index + 1u] & 0x3Fu) << 12u;
                codepoint |= static_cast<uint32_t>(bytes[index + 2u] & 0x3Fu) << 6u;
                codepoint |= static_cast<uint32_t>(bytes[index + 3u] & 0x3Fu);
                width = 4u;
            } else {
                ++index;
                continue;
            }
            index += width;

            if ((codepoint >= 'A' && codepoint <= 'Z') ||
                (codepoint >= 'a' && codepoint <= 'z')) {
                ++latin_count;
            } else if (codepoint >= 0x0900u && codepoint <= 0x097Fu) {
                ++devanagari_count;
            } else if (codepoint >= '0' && codepoint <= '9') {
                ++digit_count;
            } else if (codepoint == ' ' || codepoint == '\t' || codepoint == '\n' || codepoint == '\r') {
                ++space_count;
            }
        }
        
        const int total = latin_count + devanagari_count + digit_count + space_count;
        if (total == 0) {
            score.confidence = 0.0f;
            return score;
        }
        
        // Compute language scores
        score.english = static_cast<float>(latin_count) / static_cast<float>(total);
        score.hindi = static_cast<float>(devanagari_count) / static_cast<float>(total);
        
        // Hinglish is a mix of both
        if (score.english > 0.2f && score.hindi > 0.2f) {
            score.hinglish = std::min(score.english, score.hindi) * 2.0f;
        }
        
        // Normalize scores
        const float sum = score.english + score.hindi + score.hinglish;
        if (sum > 1.0e-12f) {
            score.english /= sum;
            score.hindi /= sum;
            score.hinglish /= sum;
        }
        
        // Compute confidence from acoustic features if available
        if (audio != nullptr && audio->frame_count > 0) {
            // Simple confidence based on speech energy
            double energy = 0.0;
            for (uint64_t i = 0; i < audio->frame_count * audio->channels; ++i) {
                const double sample = audio->interleaved_f32[i];
                energy += sample * sample;
            }
            const double rms = std::sqrt(energy / static_cast<double>(audio->frame_count * audio->channels));
            score.confidence = std::min(1.0f, static_cast<float>(rms * 10.0));
        } else {
            score.confidence = 0.7f;  // Default confidence
        }
        
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
