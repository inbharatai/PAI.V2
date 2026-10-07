# Stage 1 mounted ChatView and real-core tests

All commands run from the repository SOURCE ROOT, never an installed UnoOne folder.

## Mounted React integration

This suite mounts the actual production ChatView with real installed React 19, ReactDOM, esbuild and Tauri API bindings. Native IPC is explicitly replaced by the official Tauri mockIPC test double. JSDOM has no real browser layout, Windows, WebView2, model or vault. A passing test proves mounted frontend behavior/call arguments, not native IPC or on-device acceptance.

No runtime/repository dependencies or lockfiles were added. Install the pinned test DOM separately, outside the source tree:

    npm install --prefix ../unoone-stage1-ui-tools --no-save --package-lock=false jsdom@26.1.0
    npm --prefix apps/desktop/src ci

In Git Bash/Linux/macOS, point the suite to that separate tool root (an absolute path avoids ambiguity):

    CHAT_VIEW_TEST_TOOL_ROOT="$(cd ../unoone-stage1-ui-tools && pwd)" npm --prefix apps/desktop/src run test:context:mounted

PowerShell equivalent:

    $env:CHAT_VIEW_TEST_TOOL_ROOT = (Resolve-Path ../unoone-stage1-ui-tools).Path
    npm --prefix apps/desktop/src run test:context:mounted

Tested environment: Node 24.14.1, React/ReactDOM 19.2.7, esbuild 0.28.1, Tauri API 2.11.1, JSDOM 26.1.0. Expected mounted result: 23 passed. Assertions cover actual emitted camelCase IPC arguments after the existing binding conversion, restoration and races, named/active/new task context, full-access failure and read-only fallback, typed persistence, stopped turns, attachments, and streamed namespace filtering. Layout/scroll methods are explicit spies, not visual correctness measurements. Test health/model/vault/parser responses are clearly labelled doubles. Restore failure tests do not prove inference works while a real vault is locked.

## Actual Core memory-query/pair boundary integration

This dependency-free suite compiles the unchanged Harness core and production Rust helper with rustc, generates paired histories using the actual TypeScript selector, discovers the actual MemoryQuery validator boundary, and executes counted test providers through the real Harness runtime. It is not a real model/vault/Tauri test.

    npm --prefix apps/desktop/src run test:context:core

Requires Node with TypeScript stripping and installed rustc (the runner resolves ~/.cargo/bin/rustc or RUSTC). It uses a cleaned temporary directory, no repository build artifacts, dependency additions or cloud calls. Expected output: one Node driver test passes; the nested Rust binary reports 44 tests passed (40 helper tests + four integration tests; overlapping with standalone helper coverage).

The checked query threshold is discovered from Core, not guessed. Prompts over that threshold disable long-term retrieval rather than aborting valid current input. Current text stays unchanged; model context-fit is still independently unverified. Recognized adjacent supplied user/assistant pairs are preserved atomically under entry/byte pressure; standalone/tool entries from older callers retain their existing behavior.

## Other Stage 1 checks

    npm --prefix apps/desktop/src run test:context
    npm --prefix apps/desktop/src run test:context:parity
    npm --prefix apps/desktop/src run lint
    npm --prefix apps/desktop/src run build

Expected selector count: 53. Greeting parity: 458. Full Windows/native app checks remain a separate acceptance gate even when all these tests pass. Do not report finite lexical routing tests as universal intent recognition.
