# audio.cpp pinned baseline

- Revision: `bb15edd78b56e035967e0eb999a6b28a62337db4`
- Release: `release-0.6`
- Configure: portable Linux CPU/core profile passed with OpenMP, native ISA, and llamafile disabled
- Build: `audiocpp_cli` passed using CMake 3.30.5, Ninja 1.12.1, and Zig/Clang
- Selected framework-test build: passed
- Selected test execution: eight passed, one failed because the external `miotts` model spec was not installed in the test working directory, and `audio_utility_api_test` exceeded the 120-second sandbox command window

The upstream source was not modified. No model weights, Android NDK, GPU backend, or full-catalog parity claim is made.

Raw evidence: `UPSTREAM_CONFIGURE.log`, `UPSTREAM_BUILD.log`, `UPSTREAM_SELECTED_TEST_BUILD.log`, `UPSTREAM_SELECTED_TESTS.log`, and `UPSTREAM_SELECTED_TESTS_REMAINING.log`.
