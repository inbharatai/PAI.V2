#ifndef INBHARAT_IBAUDIO_PACK_REGISTRY_HPP
#define INBHARAT_IBAUDIO_PACK_REGISTRY_HPP

// Hash-verified hot-swappable language-pack metadata registry.
//
// Packs remain data: this registry activates/deactivates their metadata and provider
// priorities without linking model weights into the core. Provider sessions load models
// lazily. The catalog authenticates *integrity* through SHA-256; it is not a signature or
// publisher identity proof (portable signatures remain a separate trust-root feature).

#include <cstddef>
#include <deque>
#include <filesystem>
#include <string>
#include <unordered_map>
#include <vector>

namespace ibaudio::packs {

struct PackEntry {
    std::string language_code;
    std::filesystem::path manifest_path;
    std::string manifest_sha256;
    std::vector<std::string> scripts;
};

class PackRegistry {
public:
    explicit PackRegistry(std::size_t max_active = 4);

    // Load catalog.v1.tsv from root; replaces prior catalog and clears active packs.
    // Throws std::runtime_error on malformed lines, duplicate language codes, paths that
    // escape root, missing manifests, or hash mismatches (fail closed).
    void load_catalog(const std::filesystem::path &root);

    // Verify the manifest hash again and make the pack active. LRU-evicts metadata when
    // max_active is exceeded. Model memory is owned/evicted by the provider runtime.
    void activate(const std::string &language_code);
    void deactivate(const std::string &language_code);

    bool is_active(const std::string &language_code) const;
    const PackEntry *find(const std::string &language_code) const;
    std::vector<std::string> active_languages() const;
    std::size_t catalog_size() const noexcept { return catalog_.size(); }

private:
    bool inside_root(const std::filesystem::path &path) const;
    void verify_manifest(const PackEntry &entry) const;
    void touch(const std::string &language_code);

    std::filesystem::path root_;
    std::size_t max_active_;
    std::unordered_map<std::string, PackEntry> catalog_;
    std::deque<std::string> lru_;
};

} // namespace ibaudio::packs

#endif // INBHARAT_IBAUDIO_PACK_REGISTRY_HPP
