// Native transport frame codec tests — round-trip fidelity plus malformed/bounds
// rejection. These pin the wire contract in docs/NATIVE_TRANSPORT.md.

#include "../src/transport/transport.hpp"

#include <cassert>
#include <cmath>
#include <iostream>
#include <vector>

using ibaudio::transport::Frame;
using ibaudio::transport::decode_frame;
using ibaudio::transport::encode_frame;
using ibaudio::transport::kHeaderBytes;

namespace {

Frame sample_frame() {
    Frame f;
    f.flags = ibaudio::transport::kFlagEos;
    f.session_id = 0x1122334455667788ULL;
    f.timestamp_ns = 987654321ULL;
    f.sample_rate = 16000;
    f.channels = 1;
    f.frame_count = 4;
    f.pcm = {0.5f, -0.25f, 0.0f, 1.0f};
    return f;
}

void test_roundtrip() {
    const Frame in = sample_frame();
    std::vector<uint8_t> wire;
    const bool encoded = encode_frame(in, wire);  // store, then assert: assert(f()) is
    assert(encoded);                              // discarded under -DNDEBUG and miscompiles
    assert(wire.size() == kHeaderBytes + in.pcm.size() * sizeof(float));

    Frame out;
    const bool decoded = decode_frame(wire.data(), wire.size(), 1000000, out);
    assert(decoded);
    assert(out.flags == in.flags);
    assert(out.session_id == in.session_id);
    assert(out.timestamp_ns == in.timestamp_ns);
    assert(out.sample_rate == in.sample_rate);
    assert(out.channels == in.channels);
    assert(out.frame_count == in.frame_count);
    assert(out.pcm.size() == in.pcm.size());
    for (size_t i = 0; i < in.pcm.size(); ++i) assert(out.pcm[i] == in.pcm[i]);
    std::cout << "PASS roundtrip\n";
}

void test_bounds_and_malformed() {
    const Frame in = sample_frame();
    std::vector<uint8_t> wire;
    const bool encoded = encode_frame(in, wire);
    assert(encoded);

    Frame out;
    // Each rejection is stored in a variable before asserting, so the decode call is
    // never a discarded-value expression inside assert() (which -O3 -DNDEBUG removes).
    const bool truncated = decode_frame(wire.data(), wire.size() - 1, 1000000, out);
    assert(!truncated);
    std::vector<uint8_t> bad = wire; bad[0] ^= 0xFF;
    const bool bad_magic = decode_frame(bad.data(), bad.size(), 1000000, out);
    assert(!bad_magic);
    // Oversized frame_count (hostile length): patch frame_count field (offset 34) huge.
    std::vector<uint8_t> huge = wire;
    huge[34] = 0xFF; huge[35] = 0xFF; huge[36] = 0xFF; huge[37] = 0x7F;  // ~2^31 frames
    const bool oversized = decode_frame(huge.data(), huge.size(), 16000, out);  // exceeds max_frames
    assert(!oversized);
    const bool null_data = decode_frame(nullptr, wire.size(), 1000000, out);
    assert(!null_data);
    std::cout << "PASS bounds_and_malformed\n";
}

void test_inconsistent_encode() {
    Frame bad = sample_frame();
    bad.pcm.pop_back();  // pcm size no longer matches frame_count*channels
    std::vector<uint8_t> wire;
    const bool bad_encoded = encode_frame(bad, wire);
    assert(!bad_encoded);
    Frame zero_ch = sample_frame();
    zero_ch.channels = 0;
    const bool zero_ch_encoded = encode_frame(zero_ch, wire);
    assert(!zero_ch_encoded);
    std::cout << "PASS inconsistent_encode\n";
}

} // namespace

int main() {
    test_roundtrip();
    test_bounds_and_malformed();
    test_inconsistent_encode();
    std::cout << "All transport tests passed!\n";
    return 0;
}
