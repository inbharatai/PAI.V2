# ABI compatibility policy

`IBAUDIO_API_VERSION` is `(major << 16) | minor`; RC1 is 1.0. The shared-library ABI major/SONAME is 1.

Compatible minor changes may add functions, status values, capability bits, and trailing struct fields. Callers must initialize with the helper and pass their `struct_size`. The library must not read beyond it. Existing enum numeric values, field order/type/meaning, ownership, symbol signatures, and handle semantics cannot change in major 1.

A major bump is required to remove/rename a symbol, reorder/change a public field, change enum numeric values, alter frame/sample interpretation, or reverse ownership/thread guarantees. New status values remain nonzero failures to old callers.

The release gate compares:

1. `abi/ibaudio_symbols_v1.txt` against ELF dynamic symbols;
2. C99 compilation and static layout assertions in `tests/abi_c_smoke.c`;
3. the public header diff;
4. SONAME/install artifacts;
5. behavior tests for null release, borrowed input, owned output, errors, and parent/child teardown.

The optional GNU version script is disabled by default because Zig's linker does not support it; hidden visibility plus the reviewed export manifest is the portable baseline. GNU-compatible release toolchains may set `IBAUDIO_ENABLE_ELF_VERSION_SCRIPT=ON` to label exports `IBAUDIO_1.0`.
