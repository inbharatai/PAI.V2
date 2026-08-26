# Native Streaming Transport

For cross-process speech, InBharat Audio uses a **binary PCM frame transport**, not JSON/base64. This is the path for live microphone audio between a capture process and the runtime; MCP remains the control plane and never carries this audio.

## Binding per platform

| Platform | Binding |
|---|---|
| Linux / macOS | Unix domain socket |
| Windows | Named pipe |
| Android | Local socket |
| Optional | WebSocket (for browser hosts) |

## Frame format (fixed header + payload)

All integers little-endian. No JSON, no base64 — continuous audio stays binary.

```
MAGIC          u32   = 0x49424146  ("IBAF")
VERSION        u16   = 1
FLAGS          u16   bit0 = EOS (end of stream), bit1 = DISCONTINUITY
SESSION_ID     u64   producer-chosen stream identifier
TIMESTAMP_NS   u64   monotonic capture timestamp
FORMAT         u16   1 = f32 interleaved (more formats reserved)
SAMPLE_RATE    u32   e.g. 16000
CHANNELS       u16
_reserved      u16   = 0
FRAME_COUNT    u32   number of frames (samples per channel) in PAYLOAD
PAYLOAD_LEN    u32   byte length of PAYLOAD = FRAME_COUNT * CHANNELS * 4
PAYLOAD        bytes PCM
```

## Discipline

- **Bounded.** `FRAME_COUNT`/`PAYLOAD_LEN` are validated against the runtime's `max_input_frames` budget before any allocation; a malformed or oversized frame is rejected, not trusted.
- **Backpressure.** A transport session carries a bounded ring buffer; when full, the producer blocks or drops per policy — it never grows without limit.
- **No silent fallback.** A transport session is tied to an explicit provider/session; if the session cannot accept audio the transport errors, it does not reroute.

## Status

The frame codec (encode/decode with bounds checks) is implemented and unit-tested in `src/transport/` and `tests/transport_tests.cpp`. Socket/named-pipe plumbing is platform-specific and is wired per-platform; this document defines the wire contract they all share.
