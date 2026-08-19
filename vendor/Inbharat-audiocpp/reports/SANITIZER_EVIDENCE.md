# Sanitizer evidence — 0.1.0-rc1

**Result: PASS.** A clean Debug build used Zig/Clang 19.1.7 with `-fsanitize=address,undefined -fno-omit-frame-pointer` on library, CLI, and tests.

Test environment:

```text
ASAN_OPTIONS=detect_leaks=1:halt_on_error=1
UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1
```

CTest ran serially: **11/11 passed, 0 failed**. No AddressSanitizer, leak, or UndefinedBehaviorSanitizer diagnostic occurred. Raw configure/build/test output is in `linux-asan-ubsan-*.log`.

Not claimed: ThreadSanitizer, MemorySanitizer, multi-hour libFuzzer campaign, or platform sanitizers. Functional concurrency and deterministic fuzz-style loops did run in both Release and sanitized builds.
