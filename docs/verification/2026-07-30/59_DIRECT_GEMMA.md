# 59 — Direct Gemma 12B (bundled llama.cpp only, no Ollama/LM Studio)

**Status: VERIFIED_WORKING** (runtime layer, two independent runs this session).

## What ran

`D:\UNOONE\RUNTIMES\WINDOWS\CPU\llama-server.exe` (manifest-declared, hash-verified
by the 545-check strict pass) with
`D:\UNOONE\MODELS\DESKTOP\Gemma-12B\gemma-4-12B-it-Q4_K_M.gguf` (7,662,531,872 B,
sha256 `D333B368BE6CD655…` per manifest) and mmproj
`mmproj-gemma-4-12B-it-f16.gguf` (run 1), bound to `127.0.0.1` on a
dynamically chosen free port.

## Run 1 (with mmproj loaded)

- Port 57496, pid 27244. `/health` 200 after **58.4 s** (multi-GB load from
  exFAT removable media — matches the recovered llama.rs fix that tolerates
  503 "Loading model" during startup).
- Completion: 16 tokens consumed but visible `content` was empty — the
  thinking-mode model spent the small `max_tokens=16` budget on reasoning.
  Server-side timing: prompt eval 54.28 ms/tok (18.42 tok/s), generation
  210.91 ms/tok (**4.74 tok/s**).
- Clean shutdown: `STOPPED cleanly` via SIGTERM.

## Run 2 (plain chat, 96-token budget, no mmproj)

- `/health` 200 after **60.5 s**.
- Prompt: `What is 2+2? Answer with just the number.`
- **`content: "4"`** with reasoning tokens
  `The user is asking for the result of "2+2"… The answer should be "4".`
- Usage: 29 prompt + 47 completion = 76 tokens; latency 12.56 s
  (≈3.7 completion tok/s CPU).
- Clean shutdown via SIGTERM.

## Binding/network checks

- Server bound to 127.0.0.1 only (dynamic port), never exposed beyond loopback.
- No process other than `llama-server.exe` from the manifest-declared CPU
  directory participated. No Ollama, no LM Studio.

## What is NOT covered here (left to the human gate)

Model start/stop driven by the app's UI (needs unlocked vault), GPU backends
(CUDA/Vulkan DLL trees exist and are hash-verified but this run used CPU),
image inference via mmproj (asset loaded successfully in run 1; no image sent),
mid-inference USB removal, occupied-port handling. These are app-integrated
paths, not runtime-layer paths; statuses are BUILDS_NOT_RUNTIME_TESTED until
physically exercised.
