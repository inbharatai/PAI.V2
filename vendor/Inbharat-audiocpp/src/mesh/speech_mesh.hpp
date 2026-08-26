#ifndef INBHARAT_IBAUDIO_SPEECH_MESH_HPP
#define INBHARAT_IBAUDIO_SPEECH_MESH_HPP

// Bharat Speech Mesh — deterministic evidence router for local speech providers.
//
// The mesh never turns a publisher language claim into support. A candidate must carry
// same-language/device/task evidence and pass the caller's memory, streaming, privacy,
// quality, and confidence policy. Ambiguous provider results abstain rather than guess.
// Standard-library-only; no network or model dependency in the core.

#include "inbharat/ibaudio.h"

#include <cstdint>
#include <string>
#include <vector>

namespace ibaudio::mesh {

enum class DeviceClass { Windows, AndroidArm64 };
enum class EvidenceState { Pending, Failed, Verified };
enum class DecisionKind { Selected, Abstained, Unavailable };

struct ProviderEvidence {
    std::string provider_id;
    std::string language_code;
    ibaudio_task_t task = IBAUDIO_TASK_ASR;
    DeviceClass device = DeviceClass::Windows;
    EvidenceState evidence = EvidenceState::Pending;
    bool remote = false;
    bool true_streaming = false;
    uint32_t peak_memory_mb = 0;
    float quality = 0.0f;       // [0,1], benchmark-derived for this language/task/device
    float confidence = 0.0f;    // [0,1], calibration quality / evidence confidence
    float latency_ms = 0.0f;    // measured, not estimated
    std::string report_hash;    // immutable evidence/report identity
};

struct RoutePolicy {
    DeviceClass device = DeviceClass::Windows;
    uint32_t max_memory_mb = 0; // 0 = no explicit cap
    bool allow_remote = false;
    bool require_true_streaming = false;
    float min_quality = 0.0f;
    float min_confidence = 0.0f;
    float ambiguity_margin = 0.03f; // top candidates closer than this -> abstain
};

struct RouteDecision {
    DecisionKind kind = DecisionKind::Unavailable;
    std::string provider_id;
    float score = 0.0f;
    std::vector<std::string> reasons;
};

RouteDecision select_provider(const std::vector<ProviderEvidence> &candidates,
                              const std::string &language_code,
                              ibaudio_task_t task,
                              const RoutePolicy &policy);

struct OutputHypothesis {
    std::string provider_id;
    std::string text;
    float confidence = 0.0f;
};

struct ArbitrationDecision {
    DecisionKind kind = DecisionKind::Unavailable;
    std::string text;
    std::vector<std::string> agreeing_providers;
    std::string reason;
};

// Deterministic output arbitration. Agreement is normalized exact-text agreement;
// semantic fusion belongs to an explicitly configured embedding backend, never a hidden
// heuristic. Conflicting high-confidence outputs abstain unless one exceeds the next by
// the configured confidence gap.
ArbitrationDecision arbitrate_text(const std::vector<OutputHypothesis> &hypotheses,
                                   float min_confidence,
                                   float decisive_gap);

} // namespace ibaudio::mesh

#endif // INBHARAT_IBAUDIO_SPEECH_MESH_HPP
