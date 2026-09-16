# 117 — Blind-Aid Camera/OCR Acceptance: Two Layers of False FAILs (an Impossible Monkey-Patch, Then a Real Render-Placement Defect), Two Real Product Defects Caught (#45, #46), and an 11/11 Final Suite

**Date live-caught / accepted:** 2026-09-16 (physical drive D:\UNOONE — blind-aid camera/OCR live acceptance, open lanes #54/#55/#66; final suite executed on staged main `e05c730`)
**Severity:** High — the two UI-driven blind-aid lanes failed across **three** consecutive runs on two different builds; the failures were one part harness illusion and one part a genuine product defect that made the one-press flow **speak but never show** its result.

## 1. What the acceptance run kept catching

Every run of `blind-aid-ocr-live.js` — on the pre-#45 staged build, again after the defect-#45 fix — failed the same two lanes the same way:

- **FAIL — UI what's-in-front:** "describe timed out after 300s" — no error banner, no status, no DOM change
- **FAIL — narration loop:** "first tick timed out" (the 2026-09-16 morning run) — later passing consistently once the harness observed the right surfaces

while all engine-driven lanes (OCR, scene describe, TTS, screen describe, chat bridges) passed every time.

## 2. Forensics layer 1 — the harness illusion (impossible monkey-patch)

The original harness "verified" the UI lanes by monkey-patching `window.__TAURI_INTERNALS__.invoke` to log every IPC call, then waiting for the log. **`window.__TAURI_INTERNALS__.invoke` is a non-writable, non-configurable property** — in sloppy mode the assignment silently no-ops (no throw). The harness set `__visionPatched=true` / `__visionLog=[]` on `window` (writable), believed it had patched, and every app invoke bypassed the log; both lanes waited on entries that could never exist.

Harness fix: observe the product's own surfaces —

- **DOM:** the visionResult div (inline `whiteSpace: 'pre-wrap'`; `[style*=…]` attribute selectors do NOT work on CSSOM-set inline styles), the `[role=status]` narration line, honest error banners
- **Disk:** `%TEMP%\unoone-vision\*.jpg` and `%TEMP%\unoone-tts\*.wav`, compared by **set-difference of filenames** — the TTS dir keeps only recent WAVs, so count comparisons can miss a fresh WAV when an old one is cleaned up

## 3. Forensics layer 2 — the run kept failing, and this time the product was guilty (#46)

With the honest harness, the what's-in-front lane **still** timed out on the defect-#45-fixed build — capture jpg landed, no describe text, no error, while a React fiber dump held a fresh, accurate live-camera description ("A brown and grey mesh chair backrest is in the bottom left foreground… Watch out for the chair directly in front of you as you move forward") and a TTS WAV had been produced. The DOM had **zero** pre-wrap divs; the served JS bundle provably contained all defect-#45 strings (the staged frontend was current). The contradiction resolved in the JSX:

- the `visionError` banner, the "Running vision model…" indicator, the `visionResult` text block, and its **Ask in Chat** button all rendered **inside the `(screenReaderDescription || ocrExtraction)` section** (the OCR / Screen-Reader lane), while
- the one-press what's-in-front flow (and the narration ticks) set those states from the **Camera Blind Aid** section.

With only Camera Blind Aid on — the phone-parity blind-user configuration, exactly what the one-press flow targets — the app **captured, described, and spoke** the scene but never showed it:

- the button disables for the whole multi-minute run with no visible reason (a dead-looking button),
- a failed describe is invisible — **silently neutering the defect-#45 honest-error work in the most common configuration**,
- `speechNotice` claims "the complete text is shown above" while nothing is shown,
- the result cannot be handed to chat from the panel (the #54 alignment lane).

**Fix (PR #53, merged as `e05c730`, all 13 CI checks green):** the five blocks moved one level up so they render whenever the Vision Lab is open — every configuration that can produce a vision result also surfaces it. No logic change; the OCR/Screen-Reader lanes only see the result appear above the file-path section.

## 4. Defect #45 (caught during the same forensics, fixed + shipped earlier in the day)

1. **`save_vision_snapshot` was the last raw vision invoke** (defect-#23 IPC-drop wedge) in `captureSnapshot`, `narrateOnce`, and `whatsInFront` — a dropped IPC response after the file lands strands the flow forever (jpg on disk, describe never starts, no error). All three call sites now wrap the save in `withVisionTimeout(…, 30_000, 'Snapshot save')`.
2. **A swallowed press:** `whatsInFront` silently returned while a narration tick was still finishing after Narrate was switched off — the button looked enabled, nothing happened. It now says so: *"Finishing the last narration — press 'What's in front of me?' again in a moment."*

Shipped as commit `b83bd1d` → **PR #52** (13/13 CI green, merged 2026-09-16T07:59:26Z) → main `4b7cd92`.

## 5. Two more live discoveries during the chase (harness-side)

- **Vault auto-lock under the harness:** a re-run between runs hit the lock screen (App.tsx arms auto-lock on window `blur` + `auto_lock_minutes`, default 5; the app is unfocused during runs). The harness now dispatches a synthetic `focus` event every 60 s as keep-alive and aborts with an explicit `ABORT: vault is locked` guard instead of garbage FAILs. The focused lane re-test (`ui-lanes-only.js`) also gained a **90-second quiet-window reset** (model not GENERATING + no new jpg/WAV) so a mid-flight narration tick can never swallow the next lane.
- **`wait-model.js` false green:** the old banner-text heuristic green-lit the suite ~50 s after relaunch while `get_model_status` was still `NOT_LOADED` (banner read "SCANNING HOST", not "STARTING MODEL") — the suite then failed OCR/describe with *"Model manager not initialized"*. It now polls the app's own `get_model_status` invoke until `LOADED`.
- **Image-bridge grading by count:** a leftover pending frame from an aborted run made the count comparison see 1 → 1 and FAIL while the app had correctly replaced it. The step now clears any stale pending image (the × chip) and grades by **identity** — the composer must carry exactly the dispatched frame.

## 6. Staging the final build

Drive re-staged from main `e05c730` (2026-09-16, `Stage-PocketAiDrive.ps1`):

- Bundle self-verified against its SHA256SUMS.txt, then staged: `UnoOnePower.exe` **sha256 `df4a6c7c15cc78c72ddc880e49428258a62ab1b0fdb6394805f269d895236bc1`**, `UnoOneDock.exe` `f14c0c2d3b09fdce8fac3bfe4ad3e8e7328363ad0f79d90ee41bf8941c13e111`, `Start UnoOne.exe` `1eb34c53cfc283aec47789e0f920460551cf2e33f2376fc5f9b3265376a8160a`
- Frontend-embedding gate on the staged exe: **PASS** (`VERIFIED_WORKING`, both hashed assets embedded — including the defect-#46 frontend `index-cAHQslUQ.js`)
- Strict schema-v2 manifest regenerated over the drive's real assets (158 runtime, 2 model, 381 voice, 2 mobile, 3 speech), `Start UnoOne.exe --verify-only`: `failure_count: 0, valid: true`
- Previous exes (the `4b7cd92` set) preserved in `D:\UNOONE\RECOVERY\package-backups\20260916-145334`

## 7. Live acceptance (final build, staged drive)

**Executed 2026-09-16 on the re-staged drive, app driven through the real CDP-attached UI, model `LOADED`.** Suite: `blind-aid-ocr-live.js` — 13 steps, 5 small model calls total (OCR, scene describe, screen describe, UI what's-in-front describe, narration tick describe); every other step is engine- or DOM-only.

- [x] `get_accessibility_status` — honest host accessibility state (no screen reader detected)
- [x] Model server ready (`get_model_status` = `LOADED` via the app's own invoke)
- [x] **OCR lane:** rendered "UNOONE POCKET AI / LIVE OCR TEST 42" transcribed with all four anchors (unoone, pocket, ocr, 42)
- [x] **Blind-aid describe lane:** synthetic scene described naming red square, blue circle, green rectangle, STOP 42 text
- [x] **Speakability:** the scene description synthesizes to a real TTS WAV (`status=AVAILABLE`, >10 KB)
- [x] **Screen-reader lane:** GDI screen snapshot captured and described (real on-screen UI described)
- [x] **Chat alignment (#54):** `unoone:ask-in-chat` text event prefills the composer; image event attaches **exactly the dispatched frame** in the composer (identity-graded) — vision results never dead-end
- [x] **Camera presence:** WebView `enumerateDevices` sees the real cameras (videoinputs=2)
- [x] **UI what's-in-front (#55, the defect-#46 lane):** the real one-press button captures the live camera (new jpg in `%TEMP%\unoone-vision`), the description **renders in the Vision Lab panel** (visible now, not spoken-only), and a fresh TTS WAV is spoken by the app itself
- [x] **Live narration loop:** Narrate My Surroundings runs a full first tick — new capture + spoken WAV + the success status line ("I will speak when what is in front of you changes", set only after describe+TTS fully resolve)
- [x] **Cleanup:** narration + camera stopped, Camera Blind Aid toggle restored — session state clean

**Result: 13 pass / 0 fail, EXIT=0** (`blind-aid-final2.log`, 09:27:52–09:36:58 UTC: OCR 12.7 s, scene describe 42 s, TTS 97 s, screen describe 55 s, what's-in-front describe landed in the panel at 44 s after the press, its TTS WAV at ~3 min, narration first tick 2 min 48 s end-to-end).

The intermediate run on the defect-#45 build (`4b7cd92`) recorded 10 pass / 2 fail — both FAILs fully explained: one was defect #46 (the describe that never rendered), one was the image-bridge count/contamination harness issue fixed in section 5.

## 8. Harness hardening summary (test-suite assets, local-only)

- `blind-aid-ocr-live.js` — UI lanes observe DOM + disk (set-difference file detection, CSSOM style reads); auto-lock keep-alive; identity-graded image bridge
- `ui-lanes-only.js` — focused 2-lane re-test with lock-screen abort guard, quiet-window reset, 90 s idle proof
- `wait-model.js` — real `get_model_status` polling (no banner heuristics)
- `whatsinfront-forensics.js`, `fiber-probe2.js`, `pages-probe.js`, `bundle-probe.js` — the CDP forensics chain that pinned #46 (fiber state → DOM contradiction → served-bundle grep → JSX structure)
- Known measurement truths recorded: one-press describe ≈ 55-60 s (120 s app bound), CPU TTS of a scene summary ≈ 2.5-3 min, narration tick ≈ 4 min end-to-end

## 9. Campaign status (open lanes closed)

| Lane | Status |
| --- | --- |
| #44 describe reasoning budget (PR #49, main `19c3271`) | Fixed, merged, staged |
| #45 vision invoke bounds + swallowed press (PR #52, main `4b7cd92`) | Fixed, merged, staged |
| #46 vision result render placement (PR #53, main `e05c730`) | Fixed, merged, **live-verified on the staged drive** |
| #51 README accuracy audit / innovation gap analysis (PR #51) | Merged (2026-09-16T06:30:49Z) |
| #54 vision→chat alignment | Live PASS (text + image bridges, identity-graded) |
| #55 UI-driven camera lanes | Live PASS (what's-in-front renders + speaks; narration loop) |
| #66 blind-aid live acceptance | **CLOSED — 13/13 on staged `e05c730`** |

Remaining: the innovation roadmap items in the #51 gap analysis are evidence-grounded proposals, not defects — nothing in the blind-aid surface is known-broken as of this acceptance.