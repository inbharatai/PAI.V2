#ifndef INBHARAT_IBAUDIO_PERSONAL_ADAPTATION_HPP
#define INBHARAT_IBAUDIO_PERSONAL_ADAPTATION_HPP

// Reversible local adaptation — names, pronunciation, script preference, acronyms and
// correction statistics without mutating neural weights. Every patch is inspectable,
// language-scoped, fingerprinted, and individually rollbackable.

#include <cstdint>
#include <string>
#include <vector>

namespace ibaudio::adaptation {

enum class EntryKind { TranscriptCorrection, Pronunciation, Acronym, ScriptPreference, DomainTerm };

struct Entry {
    EntryKind kind = EntryKind::TranscriptCorrection;
    std::string input;
    std::string output;
};

struct Patch {
    std::string patch_id;
    std::string language_code;
    std::vector<Entry> entries;
};

struct Snapshot {
    uint64_t revision = 0;
    std::string fingerprint_sha256;
    std::vector<std::string> active_patch_ids;
};

class AdaptationStore {
public:
    // Apply a patch. Throws on duplicate id, empty fields, oversized fields or empty
    // entries. Returns a rollback token bound to the resulting revision/fingerprint.
    std::string apply(const Patch &patch);

    // Remove exactly the patch named by a valid rollback token; returns false for an
    // unknown/stale/tampered token. Other patches remain active.
    bool rollback(const std::string &token);

    // Resolve using newest matching patch first. Unknown input returns the original.
    std::string resolve(EntryKind kind, const std::string &language_code,
                        const std::string &input) const;

    Snapshot snapshot() const;

private:
    struct ActivePatch { Patch patch; std::string rollback_token; };
    void validate(const Patch &patch) const;
    std::string fingerprint() const;
    std::vector<ActivePatch> active_;
    uint64_t revision_ = 0;
};

} // namespace ibaudio::adaptation

#endif // INBHARAT_IBAUDIO_PERSONAL_ADAPTATION_HPP
