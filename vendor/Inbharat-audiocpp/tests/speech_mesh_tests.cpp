#include "../src/mesh/speech_mesh.hpp"

#include <cassert>
#include <iostream>
#include <vector>

using namespace ibaudio::mesh;

namespace {
ProviderEvidence candidate(const char *id, const char *lang, float quality, float confidence,
                           uint32_t memory, bool remote = false, bool streaming = true) {
    ProviderEvidence c;
    c.provider_id = id;
    c.language_code = lang;
    c.task = IBAUDIO_TASK_ASR;
    c.device = DeviceClass::Windows;
    c.evidence = EvidenceState::Verified;
    c.remote = remote;
    c.true_streaming = streaming;
    c.peak_memory_mb = memory;
    c.quality = quality;
    c.confidence = confidence;
    c.latency_ms = 180.0f;
    c.report_hash = "sha256:evidence";
    return c;
}

void test_route_evidence_and_privacy() {
    RoutePolicy p;
    p.device = DeviceClass::Windows;
    p.max_memory_mb = 4096;
    p.require_true_streaming = true;
    p.allow_remote = false;
    p.min_quality = 0.70f;
    p.min_confidence = 0.70f;
    std::vector<ProviderEvidence> candidates = {
        candidate("local-strong", "as-IN", 0.90f, 0.92f, 1800),
        candidate("remote-higher", "as-IN", 0.99f, 0.99f, 400, true),
        candidate("wrong-language", "hi-IN", 0.99f, 0.99f, 400),
    };
    const auto d = select_provider(candidates, "as-IN", IBAUDIO_TASK_ASR, p);
    assert(d.kind == DecisionKind::Selected && d.provider_id == "local-strong");

    // Pending publisher coverage never becomes eligible support.
    candidates[0].evidence = EvidenceState::Pending;
    const auto unavailable = select_provider(candidates, "as-IN", IBAUDIO_TASK_ASR, p);
    assert(unavailable.kind == DecisionKind::Unavailable);
    std::cout << "PASS mesh_route_evidence_privacy\n";
}

void test_route_abstains_on_tie() {
    RoutePolicy p;
    p.max_memory_mb = 4096;
    p.min_quality = 0.70f;
    p.min_confidence = 0.70f;
    p.ambiguity_margin = 0.05f;
    const auto a = candidate("a", "ta-IN", 0.90f, 0.90f, 1200);
    const auto b = candidate("b", "ta-IN", 0.90f, 0.89f, 1200);
    const auto d = select_provider({a, b}, "ta-IN", IBAUDIO_TASK_ASR, p);
    assert(d.kind == DecisionKind::Abstained);
    std::cout << "PASS mesh_route_ambiguity\n";
}

void test_output_arbitration() {
    const auto agreement = arbitrate_text({
        {"p1", "Namaste, duniya!", 0.80f},
        {"p2", "namaste, duniya", 0.82f},
        {"p3", "different", 0.86f}}, 0.70f, 0.15f);
    assert(agreement.kind == DecisionKind::Selected);
    assert(agreement.reason == "independent_provider_agreement");
    assert(agreement.agreeing_providers.size() == 2);

    const auto conflict = arbitrate_text({
        {"p1", "one answer", 0.86f}, {"p2", "another answer", 0.82f}}, 0.70f, 0.15f);
    assert(conflict.kind == DecisionKind::Abstained);

    const auto decisive = arbitrate_text({
        {"p1", "strong", 0.96f}, {"p2", "weak", 0.72f}}, 0.70f, 0.15f);
    assert(decisive.kind == DecisionKind::Selected && decisive.text == "strong");
    std::cout << "PASS mesh_output_arbitration\n";
}
} // namespace

int main() {
    test_route_evidence_and_privacy();
    test_route_abstains_on_tie();
    test_output_arbitration();
    std::cout << "All speech-mesh tests passed!\n";
    return 0;
}
