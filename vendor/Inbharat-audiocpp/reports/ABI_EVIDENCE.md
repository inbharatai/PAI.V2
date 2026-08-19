# ABI evidence — C ABI 1.0

- Public header: `include/inbharat/ibaudio.h` (C99-compatible).
- Version integer: `0x00010000`; runtime string `0.1.0-rc1`.
- Opaque handles: runtime/model/session/job/stream/buffer.
- Export manifest: `abi/ibaudio_symbols_v1.txt`.
- Dynamic export result: **58 symbols exactly matched** (`linux-release-abi.log`).
- C smoke: C99 compile/link/run passed; asserts 32-bit statuses, leading struct header offsets, and VAD segment layout.
- Linux SONAME: `libibaudio.so.1`; install consumer found and ran the C API.
- Hidden C/C++ visibility prevents internal STL/vtable symbols from becoming product API.

The optional GNU `IBAUDIO_1.0` linker version script is present but disabled for the audited Zig linker, which rejects GNU version scripts. SONAME + integer negotiation + exact symbol/layout manifest is the audited portable policy. A GNU-compatible release can enable the script separately.
