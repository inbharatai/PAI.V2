#include "personal_adaptation.hpp"
#include "../internal.hpp"

#include <algorithm>
#include <stdexcept>

namespace ibaudio::adaptation {
namespace {

constexpr std::size_t kMaxPatches = 4096;
constexpr std::size_t kMaxEntriesPerPatch = 4096;
constexpr std::size_t kMaxFieldBytes = 4096;

void append_field(std::string &out, const std::string &field) {
    out += std::to_string(field.size());
    out.push_back(':');
    out += field;
    out.push_back('|');
}

} // namespace

void AdaptationStore::validate(const Patch &patch) const {
    if (patch.patch_id.empty() || patch.language_code.empty() || patch.entries.empty()) {
        throw std::invalid_argument("adaptation patch requires id, language and entries");
    }
    if (patch.patch_id.size() > 128 || patch.language_code.size() > 32 ||
        patch.entries.size() > kMaxEntriesPerPatch || active_.size() >= kMaxPatches) {
        throw std::invalid_argument("adaptation patch exceeds bounded policy");
    }
    if (std::any_of(active_.begin(), active_.end(), [&](const ActivePatch &p) {
            return p.patch.patch_id == patch.patch_id;
        })) throw std::invalid_argument("adaptation patch id already active");
    for (const auto &entry : patch.entries) {
        if (entry.input.empty() || entry.output.empty() ||
            entry.input.size() > kMaxFieldBytes || entry.output.size() > kMaxFieldBytes) {
            throw std::invalid_argument("adaptation entry has empty or oversized field");
        }
    }
}

std::string AdaptationStore::fingerprint() const {
    std::string canonical;
    for (const auto &active : active_) {
        append_field(canonical, active.patch.patch_id);
        append_field(canonical, active.patch.language_code);
        for (const auto &entry : active.patch.entries) {
            append_field(canonical, std::to_string(static_cast<int>(entry.kind)));
            append_field(canonical, entry.input);
            append_field(canonical, entry.output);
        }
    }
    return ibaudio::sha256_hex(reinterpret_cast<const uint8_t *>(canonical.data()), canonical.size());
}

std::string AdaptationStore::apply(const Patch &patch) {
    validate(patch);
    ++revision_;
    active_.push_back({patch, {}});
    // Token is bound to the resulting active-patch fingerprint, not the pre-apply state.
    std::string token_material = patch.patch_id + ":" + std::to_string(revision_) + ":" + fingerprint();
    const std::string token = patch.patch_id + "." + std::to_string(revision_) + "." +
        ibaudio::sha256_hex(reinterpret_cast<const uint8_t *>(token_material.data()), token_material.size());
    active_.back().rollback_token = token;
    return token;
}

bool AdaptationStore::rollback(const std::string &token) {
    const auto it = std::find_if(active_.begin(), active_.end(), [&](const ActivePatch &p) {
        return p.rollback_token == token;
    });
    if (it == active_.end()) return false;
    active_.erase(it);
    ++revision_;
    return true;
}

std::string AdaptationStore::resolve(EntryKind kind, const std::string &language_code,
                                     const std::string &input) const {
    for (auto patch = active_.rbegin(); patch != active_.rend(); ++patch) {
        if (patch->patch.language_code != language_code) continue;
        for (auto entry = patch->patch.entries.rbegin(); entry != patch->patch.entries.rend(); ++entry) {
            if (entry->kind == kind && entry->input == input) return entry->output;
        }
    }
    return input;
}

Snapshot AdaptationStore::snapshot() const {
    Snapshot s;
    s.revision = revision_;
    s.fingerprint_sha256 = fingerprint();
    for (const auto &active : active_) s.active_patch_ids.push_back(active.patch.patch_id);
    return s;
}

} // namespace ibaudio::adaptation
