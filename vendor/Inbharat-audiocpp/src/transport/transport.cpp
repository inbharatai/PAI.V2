#include "transport.hpp"

#include <cstring>

namespace ibaudio {
namespace transport {
namespace {

void put_u16(std::vector<uint8_t> &o, uint16_t v) { o.push_back(static_cast<uint8_t>(v)); o.push_back(static_cast<uint8_t>(v >> 8)); }
void put_u32(std::vector<uint8_t> &o, uint32_t v) { for (int k = 0; k < 4; ++k) o.push_back(static_cast<uint8_t>(v >> (8 * k))); }
void put_u64(std::vector<uint8_t> &o, uint64_t v) { for (int k = 0; k < 8; ++k) o.push_back(static_cast<uint8_t>(v >> (8 * k))); }

uint16_t get_u16(const uint8_t *p) { return static_cast<uint16_t>(p[0] | (static_cast<uint16_t>(p[1]) << 8)); }
uint32_t get_u32(const uint8_t *p) {
    return static_cast<uint32_t>(p[0]) | (static_cast<uint32_t>(p[1]) << 8) |
           (static_cast<uint32_t>(p[2]) << 16) | (static_cast<uint32_t>(p[3]) << 24);
}
uint64_t get_u64(const uint8_t *p) { uint64_t v = 0; for (int k = 7; k >= 0; --k) v = (v << 8) | p[k]; return v; }

} // namespace

bool encode_frame(const Frame &frame, std::vector<uint8_t> &out) {
    if (frame.channels == 0 || frame.format != kFormatF32Interleaved) return false;
    if (frame.pcm.size() != static_cast<size_t>(frame.frame_count) * frame.channels) return false;
    out.clear();
    out.reserve(kHeaderBytes + frame.pcm.size() * sizeof(float));
    put_u32(out, kMagic);            // 0
    put_u16(out, kVersion);          // 4
    put_u16(out, frame.flags);       // 6
    put_u64(out, frame.session_id);  // 8
    put_u64(out, frame.timestamp_ns);// 16
    put_u16(out, frame.format);      // 24
    put_u32(out, frame.sample_rate); // 26
    put_u16(out, frame.channels);    // 30
    put_u16(out, 0);                 // 32 reserved
    put_u32(out, frame.frame_count); // 34
    put_u32(out, static_cast<uint32_t>(frame.pcm.size() * sizeof(float)));  // 38
    for (float s : frame.pcm) {      // 42.. payload
        uint32_t bits;
        std::memcpy(&bits, &s, sizeof(bits));
        put_u32(out, bits);
    }
    return true;
}

bool decode_frame(const uint8_t *data, size_t size, uint64_t max_frames, Frame &out) {
    out = Frame{};  // never leave the caller with a partially-filled frame
    if (data == nullptr || size < kHeaderBytes) return false;
    if (get_u32(data + 0) != kMagic) return false;
    if (get_u16(data + 4) != kVersion) return false;
    out.flags = get_u16(data + 6);
    out.session_id = get_u64(data + 8);
    out.timestamp_ns = get_u64(data + 16);
    out.format = get_u16(data + 24);
    if (out.format != kFormatF32Interleaved) return false;
    out.sample_rate = get_u32(data + 26);
    out.channels = get_u16(data + 30);
    out.frame_count = get_u32(data + 34);
    const uint32_t payload_len = get_u32(data + 38);
    if (out.channels == 0) return false;
    if (out.frame_count > max_frames) return false;  // bound before any allocation
    const uint64_t expected = static_cast<uint64_t>(out.frame_count) * out.channels * sizeof(float);
    if (payload_len != expected) return false;
    if (size != kHeaderBytes + payload_len) return false;  // exact size, no trailing bytes
    out.pcm.resize(static_cast<size_t>(out.frame_count) * out.channels);
    const uint8_t *p = data + kHeaderBytes;
    for (size_t i = 0; i < out.pcm.size(); ++i) {
        const uint32_t bits = get_u32(p + i * 4);
        float s;
        std::memcpy(&s, &bits, sizeof(s));
        out.pcm[i] = s;
    }
    return true;
}

} // namespace transport
} // namespace ibaudio
