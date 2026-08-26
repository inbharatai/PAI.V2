#include "../src/packs/pack_registry.hpp"

#include <cassert>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <stdexcept>

using ibaudio::packs::PackRegistry;

namespace {

void copy_pack_root(const std::filesystem::path &from, const std::filesystem::path &to) {
    std::filesystem::remove_all(to);
    std::filesystem::create_directories(to);
    std::filesystem::copy(from, to,
        std::filesystem::copy_options::recursive | std::filesystem::copy_options::overwrite_existing);
}

void test_catalog_and_lru() {
    const std::filesystem::path root = IBAUDIO_TEST_PACK_ROOT;
    PackRegistry registry(2);
    registry.load_catalog(root);
    assert(registry.catalog_size() == 22);
    registry.activate("as-IN");
    registry.activate("hi-IN");
    assert(registry.is_active("as-IN") && registry.is_active("hi-IN"));
    registry.activate("ta-IN");
    assert(!registry.is_active("as-IN")); // oldest metadata LRU-evicted
    assert(registry.is_active("hi-IN") && registry.is_active("ta-IN"));
    assert(registry.find("sat-IN") != nullptr);
    assert(registry.find("xx-IN") == nullptr);
    registry.deactivate("hi-IN");
    assert(!registry.is_active("hi-IN"));
    std::cout << "PASS pack_registry_catalog_lru\n";
}

void test_tamper_and_path_fail_closed() {
    const std::filesystem::path source = IBAUDIO_TEST_PACK_ROOT;
    const auto temp = std::filesystem::temp_directory_path() / "ibaudio-pack-registry-test";
    copy_pack_root(source, temp);
    PackRegistry registry(2);
    registry.load_catalog(temp);
    // Tamper after catalog load: activation must re-hash and fail closed.
    {
        std::ofstream output(temp / "hi-IN" / "pack.json", std::ios::app);
        output << "\n";
    }
    bool hash_failed = false;
    try { registry.activate("hi-IN"); }
    catch (const std::runtime_error &) { hash_failed = true; }
    assert(hash_failed);

    // Catalog traversal must also fail closed.
    copy_pack_root(source, temp);
    {
        std::ofstream output(temp / "catalog.v1.tsv", std::ios::app);
        output << "evil-IN\t../outside.json\t0000000000000000000000000000000000000000000000000000000000000000\tLatn\n";
    }
    bool path_failed = false;
    try { registry.load_catalog(temp); }
    catch (const std::runtime_error &) { path_failed = true; }
    assert(path_failed);
    std::filesystem::remove_all(temp);
    std::cout << "PASS pack_registry_tamper_path\n";
}

} // namespace

int main() {
    test_catalog_and_lru();
    test_tamper_and_path_fail_closed();
    std::cout << "All pack-registry tests passed!\n";
    return 0;
}
