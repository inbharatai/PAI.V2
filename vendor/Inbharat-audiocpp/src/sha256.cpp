#include "internal.hpp"

#include <array>
#include <cstring>
#include <fstream>
#include <iomanip>
#include <sstream>
#include <stdexcept>
#include <vector>

namespace ibaudio {
namespace {

constexpr std::array<uint32_t, 64> kRoundConstants = {
    0x428a2f98u, 0x71374491u, 0xb5c0fbcfu, 0xe9b5dba5u, 0x3956c25bu, 0x59f111f1u, 0x923f82a4u, 0xab1c5ed5u,
    0xd807aa98u, 0x12835b01u, 0x243185beu, 0x550c7dc3u, 0x72be5d74u, 0x80deb1feu, 0x9bdc06a7u, 0xc19bf174u,
    0xe49b69c1u, 0xefbe4786u, 0x0fc19dc6u, 0x240ca1ccu, 0x2de92c6fu, 0x4a7484aau, 0x5cb0a9dcu, 0x76f988dau,
    0x983e5152u, 0xa831c66du, 0xb00327c8u, 0xbf597fc7u, 0xc6e00bf3u, 0xd5a79147u, 0x06ca6351u, 0x14292967u,
    0x27b70a85u, 0x2e1b2138u, 0x4d2c6dfcu, 0x53380d13u, 0x650a7354u, 0x766a0abbu, 0x81c2c92eu, 0x92722c85u,
    0xa2bfe8a1u, 0xa81a664bu, 0xc24b8b70u, 0xc76c51a3u, 0xd192e819u, 0xd6990624u, 0xf40e3585u, 0x106aa070u,
    0x19a4c116u, 0x1e376c08u, 0x2748774cu, 0x34b0bcb5u, 0x391c0cb3u, 0x4ed8aa4au, 0x5b9cca4fu, 0x682e6ff3u,
    0x748f82eeu, 0x78a5636fu, 0x84c87814u, 0x8cc70208u, 0x90befffau, 0xa4506cebu, 0xbef9a3f7u, 0xc67178f2u};

uint32_t rotate_right(uint32_t value, uint32_t count) {
    return (value >> count) | (value << (32u - count));
}

class Sha256 {
public:
    void update(const uint8_t *data, size_t size) {
        if (size > 0u && data == nullptr) {
            throw std::invalid_argument("SHA-256 data pointer is null");
        }
        const uint64_t max_bytes = std::numeric_limits<uint64_t>::max() / 8u;
        if (size > max_bytes || total_bytes_ > max_bytes - static_cast<uint64_t>(size)) {
            throw std::length_error("SHA-256 input length overflow");
        }
        total_bytes_ += size;
        while (size > 0u) {
            const size_t step = std::min(size, block_.size() - block_size_);
            std::memcpy(block_.data() + block_size_, data, step);
            block_size_ += step;
            data += step;
            size -= step;
            if (block_size_ == block_.size()) {
                transform(block_.data());
                block_size_ = 0u;
            }
        }
    }

    std::array<uint8_t, 32> finalize() {
        const uint64_t bit_count = total_bytes_ * 8u;
        block_[block_size_++] = 0x80u;
        if (block_size_ > 56u) {
            while (block_size_ < 64u) {
                block_[block_size_++] = 0u;
            }
            transform(block_.data());
            block_size_ = 0u;
        }
        while (block_size_ < 56u) {
            block_[block_size_++] = 0u;
        }
        for (int shift = 56; shift >= 0; shift -= 8) {
            block_[block_size_++] = static_cast<uint8_t>((bit_count >> shift) & 0xffu);
        }
        transform(block_.data());
        std::array<uint8_t, 32> digest{};
        for (size_t index = 0; index < state_.size(); ++index) {
            digest[index * 4u] = static_cast<uint8_t>(state_[index] >> 24u);
            digest[index * 4u + 1u] = static_cast<uint8_t>(state_[index] >> 16u);
            digest[index * 4u + 2u] = static_cast<uint8_t>(state_[index] >> 8u);
            digest[index * 4u + 3u] = static_cast<uint8_t>(state_[index]);
        }
        return digest;
    }

private:
    void transform(const uint8_t *block) {
        std::array<uint32_t, 64> words{};
        for (size_t index = 0; index < 16u; ++index) {
            const size_t offset = index * 4u;
            words[index] = (static_cast<uint32_t>(block[offset]) << 24u) |
                           (static_cast<uint32_t>(block[offset + 1u]) << 16u) |
                           (static_cast<uint32_t>(block[offset + 2u]) << 8u) |
                           static_cast<uint32_t>(block[offset + 3u]);
        }
        for (size_t index = 16u; index < 64u; ++index) {
            const uint32_t s0 = rotate_right(words[index - 15u], 7u) ^
                                rotate_right(words[index - 15u], 18u) ^
                                (words[index - 15u] >> 3u);
            const uint32_t s1 = rotate_right(words[index - 2u], 17u) ^
                                rotate_right(words[index - 2u], 19u) ^
                                (words[index - 2u] >> 10u);
            words[index] = words[index - 16u] + s0 + words[index - 7u] + s1;
        }
        uint32_t a = state_[0];
        uint32_t b = state_[1];
        uint32_t c = state_[2];
        uint32_t d = state_[3];
        uint32_t e = state_[4];
        uint32_t f = state_[5];
        uint32_t g = state_[6];
        uint32_t h = state_[7];
        for (size_t index = 0; index < 64u; ++index) {
            const uint32_t s1 = rotate_right(e, 6u) ^ rotate_right(e, 11u) ^ rotate_right(e, 25u);
            const uint32_t choose = (e & f) ^ ((~e) & g);
            const uint32_t temp1 = h + s1 + choose + kRoundConstants[index] + words[index];
            const uint32_t s0 = rotate_right(a, 2u) ^ rotate_right(a, 13u) ^ rotate_right(a, 22u);
            const uint32_t majority = (a & b) ^ (a & c) ^ (b & c);
            const uint32_t temp2 = s0 + majority;
            h = g;
            g = f;
            f = e;
            e = d + temp1;
            d = c;
            c = b;
            b = a;
            a = temp1 + temp2;
        }
        state_[0] += a;
        state_[1] += b;
        state_[2] += c;
        state_[3] += d;
        state_[4] += e;
        state_[5] += f;
        state_[6] += g;
        state_[7] += h;
    }

    std::array<uint32_t, 8> state_ = {
        0x6a09e667u, 0xbb67ae85u, 0x3c6ef372u, 0xa54ff53au,
        0x510e527fu, 0x9b05688cu, 0x1f83d9abu, 0x5be0cd19u};
    std::array<uint8_t, 64> block_{};
    size_t block_size_ = 0u;
    uint64_t total_bytes_ = 0u;
};

std::string to_hex(const std::array<uint8_t, 32> &digest) {
    std::ostringstream output;
    output << std::hex << std::setfill('0');
    for (uint8_t value : digest) {
        output << std::setw(2) << static_cast<unsigned>(value);
    }
    return output.str();
}

} // namespace

std::array<uint8_t, 32> sha256_bytes(const uint8_t *data, size_t size) {
    Sha256 hash;
    hash.update(data, size);
    return hash.finalize();
}

std::string sha256_hex(const uint8_t *data, size_t size) {
    return to_hex(sha256_bytes(data, size));
}

std::string sha256_file_path(const std::filesystem::path &path) {
    std::ifstream input(path, std::ios::binary);
    if (!input) {
        throw std::runtime_error("unable to open file for SHA-256: " + path.string());
    }
    Sha256 hash;
    std::array<uint8_t, 1024u * 1024u> block{};
    while (input) {
        input.read(reinterpret_cast<char *>(block.data()), static_cast<std::streamsize>(block.size()));
        const std::streamsize count = input.gcount();
        if (count > 0) {
            hash.update(block.data(), static_cast<size_t>(count));
        }
    }
    if (!input.eof()) {
        throw std::runtime_error("failed while reading file for SHA-256: " + path.string());
    }
    return to_hex(hash.finalize());
}

} // namespace ibaudio
