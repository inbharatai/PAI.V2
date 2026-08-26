#include "speech_mesh.hpp"

#include <algorithm>
#include <cctype>
#include <cmath>
#include <map>

namespace ibaudio::mesh {
namespace {

float clamp01(float value) { return std::max(0.0f, std::min(1.0f, value)); }

float candidate_score(const ProviderEvidence &candidate, const RoutePolicy &policy) {
    // Quality dominates. Confidence rewards strong same-language evidence. Memory and
    // latency are bounded penalties so a tiny weak model cannot outrank an accurate one.
    const float quality = clamp01(candidate.quality);
    const float confidence = clamp01(candidate.confidence);
    float memory_penalty = 0.0f;
    if (policy.max_memory_mb > 0 && candidate.peak_memory_mb > 0) {
        memory_penalty = 0.08f * std::min(1.0f,
            static_cast<float>(candidate.peak_memory_mb) / static_cast<float>(policy.max_memory_mb));
    }
    // 2 seconds reaches the maximum latency penalty; latency must be measured.
    const float latency_penalty = candidate.latency_ms > 0.0f
        ? 0.07f * std::min(1.0f, candidate.latency_ms / 2000.0f) : 0.0f;
    return quality * 0.72f + confidence * 0.28f - memory_penalty - latency_penalty;
}

std::string normalize_text(const std::string &input) {
    std::string out;
    bool space = false;
    for (char raw : input) {
        const unsigned char c = static_cast<unsigned char>(raw);
        if (std::isspace(c) != 0) { space = !out.empty(); continue; }
        if (space) { out.push_back(' '); space = false; }
        out.push_back(c < 128 ? static_cast<char>(std::tolower(c)) : raw);
    }
    while (!out.empty() && (out.back() == '.' || out.back() == ',' || out.back() == '!' || out.back() == '?')) {
        out.pop_back();
    }
    return out;
}

} // namespace

RouteDecision select_provider(const std::vector<ProviderEvidence> &candidates,
                              const std::string &language_code,
                              ibaudio_task_t task,
                              const RoutePolicy &policy) {
    struct Ranked { const ProviderEvidence *candidate; float score; };
    std::vector<Ranked> eligible;
    RouteDecision decision;
    for (const auto &candidate : candidates) {
        if (candidate.language_code != language_code || candidate.task != task) continue;
        if (candidate.device != policy.device) continue;
        if (candidate.evidence != EvidenceState::Verified) continue;
        if (candidate.report_hash.empty()) continue;
        if (candidate.remote && !policy.allow_remote) continue;
        if (policy.require_true_streaming && !candidate.true_streaming) continue;
        if (policy.max_memory_mb > 0 && candidate.peak_memory_mb > policy.max_memory_mb) continue;
        if (!std::isfinite(candidate.quality) || candidate.quality < policy.min_quality) continue;
        if (!std::isfinite(candidate.confidence) || candidate.confidence < policy.min_confidence) continue;
        eligible.push_back({&candidate, candidate_score(candidate, policy)});
    }
    if (eligible.empty()) {
        decision.kind = DecisionKind::Unavailable;
        decision.reasons.push_back("no_verified_candidate_satisfies_policy");
        return decision;
    }
    std::sort(eligible.begin(), eligible.end(), [](const Ranked &a, const Ranked &b) {
        if (a.score != b.score) return a.score > b.score;
        return a.candidate->provider_id < b.candidate->provider_id;
    });
    if (eligible.size() > 1 && std::fabs(eligible[0].score - eligible[1].score) < policy.ambiguity_margin) {
        decision.kind = DecisionKind::Abstained;
        decision.score = eligible[0].score;
        decision.reasons.push_back("provider_scores_ambiguous");
        decision.reasons.push_back(eligible[0].candidate->provider_id);
        decision.reasons.push_back(eligible[1].candidate->provider_id);
        return decision;
    }
    decision.kind = DecisionKind::Selected;
    decision.provider_id = eligible[0].candidate->provider_id;
    decision.score = eligible[0].score;
    decision.reasons.push_back("highest_verified_policy_compliant_score");
    return decision;
}

ArbitrationDecision arbitrate_text(const std::vector<OutputHypothesis> &hypotheses,
                                   float min_confidence,
                                   float decisive_gap) {
    ArbitrationDecision result;
    std::vector<OutputHypothesis> eligible;
    for (const auto &h : hypotheses) {
        if (!h.text.empty() && std::isfinite(h.confidence) && h.confidence >= min_confidence) eligible.push_back(h);
    }
    if (eligible.empty()) {
        result.kind = DecisionKind::Unavailable;
        result.reason = "no_confident_hypothesis";
        return result;
    }
    std::map<std::string, std::vector<const OutputHypothesis *>> groups;
    for (const auto &h : eligible) groups[normalize_text(h.text)].push_back(&h);
    auto agreement = std::max_element(groups.begin(), groups.end(), [](const auto &a, const auto &b) {
        return a.second.size() < b.second.size();
    });
    if (agreement != groups.end() && agreement->second.size() >= 2) {
        result.kind = DecisionKind::Selected;
        result.text = agreement->second.front()->text;
        result.reason = "independent_provider_agreement";
        for (const auto *h : agreement->second) result.agreeing_providers.push_back(h->provider_id);
        return result;
    }
    std::sort(eligible.begin(), eligible.end(), [](const auto &a, const auto &b) {
        if (a.confidence != b.confidence) return a.confidence > b.confidence;
        return a.provider_id < b.provider_id;
    });
    if (eligible.size() == 1 || eligible[0].confidence - eligible[1].confidence >= decisive_gap) {
        result.kind = DecisionKind::Selected;
        result.text = eligible[0].text;
        result.agreeing_providers.push_back(eligible[0].provider_id);
        result.reason = "decisive_calibrated_confidence_gap";
        return result;
    }
    result.kind = DecisionKind::Abstained;
    result.reason = "conflicting_provider_outputs";
    return result;
}

} // namespace ibaudio::mesh
