# C ABI v1

The supported embedding boundary is `crates/ffi/include/inbharat_harness.h`. The Rust implementation emits a static library named `libinbharat_harness.a`; distributors may additionally select a platform-supported dynamic crate type.

Rules:

- every exported symbol ends in `_v1`;
- structs are `repr(C)` and begin with `struct_size` where evolution is expected;
- only fixed-width integers, `size_t`, raw byte spans, status codes, and opaque handles cross the boundary;
- strings are UTF-8 bytes, never Rust strings or exceptions;
- borrowed `IbByteSpanV1` remains caller-owned for the call;
- `IbOwnedBytesV1` is library-owned and must be freed once with `ib_harness_bytes_free_v1`;
- handles must be destroyed once; null destroy is a no-op;
- `IbCancellationHandle` can be requested from another caller thread and must outlive the run using it;
- cancellation cause codes are 0 user, 1 parent, 2 deadline, 3 policy, 4 shutdown, and 5 disposed;
- every exported operation contains Rust panics and returns status 255;
- no Rust future, allocator object, file descriptor, or trait object crosses C.

`abi-v1.json` is the checked symbol/ownership manifest. ABI v1 exposes harness create/destroy, route, synchronous run, cancellable run, cancellation create/request/destroy, output free, API version, and status text. Job polling/callbacks are reserved for a future size-tagged ABI version rather than added incompatibly.
