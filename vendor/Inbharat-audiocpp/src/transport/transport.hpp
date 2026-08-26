#ifndef INBHARAT_IBAUDIO_TRANSPORT_HPP
#define INBHARAT_IBAUDIO_TRANSPORT_HPP

// Native streaming transport — binary PCM frame codec (wire contract in
// docs/NATIVE_TRANSPORT.md). Platform socket/named-pipe plumbing lives under
// platform/; this header is the shared, platform-neutral frame encode/decode.
//
// Discipline: bounds-checked, no unbounded allocation, malformed frames rejected.
// Dependency-free (standard library only).

#include <cstddef>
#include <cstdint>
#include <vector>

namespace ibaudio {
namespace transport {

constexpr uint32_t kMagic = 0x49424146u;  // "IBAF"
constexpr uint16_t kVersion = 1u;
constexpr uint16_t kFormatF32Interleaved = 1u;
constexpr uint16_t kFlagEos = 1u << 0;
constexpr uint16_t kFlagDiscontinuity = 1u << 1;
// Fixed header size in bytes (see encode/decode offset table in transport.cpp):
//   0 magic(4) 4 version(2) 6 flags(2) 8 session_id(8) 16 timestamp(8)
//  24 format(2) 26 sample_rate(4) 30 channels(2) 32 reserved(2)
//  34 frame_count(4) 38 payload_len(4)  -> 42 bytes, then PAYLOAD.
constexpr size_t kHeaderBytes = 42;

struct Frame {
    uint16_t flags = 0;
    uint64_t session_id = 0;
    uint64_t timestamp_ns = 0;
    uint16_t format = kFormatF32Interleaved;
    uint32_t sample_rate = 16000;
    uint16_t channels = 1;
    uint32_t frame_count = 0;
    std::vector<float> pcm;  // frame_count * channels samples
};

// Encode a frame to wire bytes. Returns false on an inconsistent frame
// (pcm size != frame_count * channels, unsupported format, zero channels).
bool encode_frame(const Frame &frame, std::vector<uint8_t> &out);

// Decode wire bytes into a frame. `max_frames` bounds the accepted frame_count to
// stop a hostile/corrupt length from forcing a huge allocation. Returns false on
// any malformed input (bad magic, version, truncated, length mismatch, oversize).
bool decode_frame(const uint8_t *data, size_t size, uint64_t max_frames, Frame &out);

} // namespace transport
} // namespace ibaudio

#endif // INBHARAT_IBAUDIO_TRANSPORT_HPP
