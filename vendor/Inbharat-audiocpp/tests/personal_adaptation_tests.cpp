#include "../src/adaptation/personal_adaptation.hpp"

#include <cassert>
#include <iostream>
#include <stdexcept>

using namespace ibaudio::adaptation;

namespace {

void test_apply_override_and_rollback() {
    AdaptationStore store;
    Patch names{"names-v1", "as-IN", {
        {EntryKind::TranscriptCorrection, "Gowahati", "Guwahati"},
        {EntryKind::Pronunciation, "Reeturaj", "ree-tu-raaj"},
    }};
    const std::string token1 = store.apply(names);
    assert(store.resolve(EntryKind::TranscriptCorrection, "as-IN", "Gowahati") == "Guwahati");
    assert(store.resolve(EntryKind::Pronunciation, "as-IN", "Reeturaj") == "ree-tu-raaj");
    assert(store.resolve(EntryKind::TranscriptCorrection, "hi-IN", "Gowahati") == "Gowahati");

    // Newest matching patch wins but rolling it back restores the older correction.
    Patch correction{"names-v2", "as-IN", {{EntryKind::TranscriptCorrection, "Gowahati", "গুৱাহাটী"}}};
    const std::string token2 = store.apply(correction);
    assert(store.resolve(EntryKind::TranscriptCorrection, "as-IN", "Gowahati") == "গুৱাহাটী");
    assert(store.rollback(token2));
    assert(store.resolve(EntryKind::TranscriptCorrection, "as-IN", "Gowahati") == "Guwahati");
    assert(!store.rollback("tampered-token"));
    assert(store.rollback(token1));
    assert(store.resolve(EntryKind::TranscriptCorrection, "as-IN", "Gowahati") == "Gowahati");
    std::cout << "PASS adaptation_apply_rollback\n";
}

void test_bounds_and_fingerprint() {
    AdaptationStore store;
    const auto before = store.snapshot();
    assert(before.fingerprint_sha256.size() == 64);
    const std::string token = store.apply({"domain-v1", "hi-IN", {
        {EntryKind::Acronym, "UPI", "यू पी आई"},
        {EntryKind::DomainTerm, "SILT", "SILT"},
    }});
    const auto after = store.snapshot();
    assert(after.revision == 1 && after.active_patch_ids.size() == 1);
    assert(after.fingerprint_sha256 != before.fingerprint_sha256);
    bool duplicate_failed = false;
    try { (void)store.apply({"domain-v1", "hi-IN", {{EntryKind::Acronym, "X", "Y"}}}); }
    catch (const std::invalid_argument &) { duplicate_failed = true; }
    assert(duplicate_failed);
    bool empty_failed = false;
    try { (void)store.apply({"", "hi-IN", {}}); }
    catch (const std::invalid_argument &) { empty_failed = true; }
    assert(empty_failed);
    assert(store.rollback(token));
    assert(store.snapshot().fingerprint_sha256 == before.fingerprint_sha256);
    std::cout << "PASS adaptation_bounds_fingerprint\n";
}

} // namespace

int main() {
    test_apply_override_and_rollback();
    test_bounds_and_fingerprint();
    std::cout << "All personal-adaptation tests passed!\n";
    return 0;
}
