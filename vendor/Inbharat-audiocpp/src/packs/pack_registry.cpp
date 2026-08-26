#include "pack_registry.hpp"
#include "../internal.hpp"

#include <algorithm>
#include <fstream>
#include <sstream>
#include <stdexcept>

namespace ibaudio::packs {
namespace {

std::vector<std::string> split(const std::string &text, char delimiter) {
    std::vector<std::string> out;
    std::stringstream stream(text);
    std::string item;
    while (std::getline(stream, item, delimiter)) out.push_back(item);
    return out;
}

} // namespace

PackRegistry::PackRegistry(std::size_t max_active) : max_active_(std::max<std::size_t>(1, max_active)) {}

bool PackRegistry::inside_root(const std::filesystem::path &path) const {
    const auto canonical_root = std::filesystem::weakly_canonical(root_);
    const auto canonical_path = std::filesystem::weakly_canonical(path);
    auto root_it = canonical_root.begin();
    auto path_it = canonical_path.begin();
    for (; root_it != canonical_root.end(); ++root_it, ++path_it) {
        if (path_it == canonical_path.end() || *root_it != *path_it) return false;
    }
    return true;
}

void PackRegistry::verify_manifest(const PackEntry &entry) const {
    if (!inside_root(entry.manifest_path)) throw std::runtime_error("pack manifest escapes pack root");
    if (!std::filesystem::is_regular_file(entry.manifest_path)) throw std::runtime_error("pack manifest missing");
    const std::string actual = ibaudio::sha256_file_path(entry.manifest_path);
    if (actual != entry.manifest_sha256) throw std::runtime_error("pack manifest SHA-256 mismatch");
}

void PackRegistry::load_catalog(const std::filesystem::path &root) {
    root_ = std::filesystem::weakly_canonical(root);
    const auto catalog_path = root_ / "catalog.v1.tsv";
    if (!std::filesystem::is_regular_file(catalog_path)) throw std::runtime_error("language-pack catalog missing");
    std::ifstream input(catalog_path);
    if (!input) throw std::runtime_error("language-pack catalog unreadable");
    std::unordered_map<std::string, PackEntry> next;
    std::string line;
    std::size_t line_number = 0;
    while (std::getline(input, line)) {
        ++line_number;
        if (line.empty() || line.front() == '#') continue;
        const auto fields = split(line, '\t');
        if (fields.size() != 4 || fields[0].empty() || fields[2].size() != 64) {
            throw std::runtime_error("malformed language-pack catalog line " + std::to_string(line_number));
        }
        PackEntry entry;
        entry.language_code = fields[0];
        entry.manifest_path = root_ / fields[1];
        entry.manifest_sha256 = fields[2];
        entry.scripts = split(fields[3], ',');
        if (entry.scripts.empty()) throw std::runtime_error("language-pack script list empty");
        if (next.find(entry.language_code) != next.end()) throw std::runtime_error("duplicate language pack");
        verify_manifest(entry);
        next.emplace(entry.language_code, std::move(entry));
    }
    if (next.empty()) throw std::runtime_error("language-pack catalog contains no packs");
    catalog_ = std::move(next);
    lru_.clear();
}

void PackRegistry::touch(const std::string &language_code) {
    lru_.erase(std::remove(lru_.begin(), lru_.end(), language_code), lru_.end());
    lru_.push_back(language_code);
    while (lru_.size() > max_active_) lru_.pop_front();
}

void PackRegistry::activate(const std::string &language_code) {
    const auto it = catalog_.find(language_code);
    if (it == catalog_.end()) throw std::runtime_error("language pack not registered");
    verify_manifest(it->second); // re-verify at every activation; catalog load is not enough
    touch(language_code);
}

void PackRegistry::deactivate(const std::string &language_code) {
    lru_.erase(std::remove(lru_.begin(), lru_.end(), language_code), lru_.end());
}

bool PackRegistry::is_active(const std::string &language_code) const {
    return std::find(lru_.begin(), lru_.end(), language_code) != lru_.end();
}

const PackEntry *PackRegistry::find(const std::string &language_code) const {
    const auto it = catalog_.find(language_code);
    return it == catalog_.end() ? nullptr : &it->second;
}

std::vector<std::string> PackRegistry::active_languages() const {
    return {lru_.begin(), lru_.end()};
}

} // namespace ibaudio::packs
