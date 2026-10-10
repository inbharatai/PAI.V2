# Power: bounded readiness and PDF text-layer upgrade

## Preservation boundary

This change keeps the existing desktop navigation, visual tokens, controls, model package discovery, selected load target behavior, vault paths/records, runtime configuration and speech/memory routing. It adds no application dependency, cloud provider, MCP, scheduler, migration, persistent source schema or automatic model/permission action. Vendored Harness and Audio are untouched.

## Readiness is an observation, not a qualification

`ModelManager` uses the existing list/config/status/cache APIs. Its checklist separates:

- **Selected**: the target of the existing Load Model action, not necessarily the running model.
- **Present / missing**: `ModelInfo.available` is discovery/file-presence evidence. The existing `manifest-verified` quantization string is presented as **Manifest-listed**, not treated as a hash result.
- **Verified cache marker**: cache staging verifies against the manifest; a cheap status probe reads the marker and file metadata, not a new hash of a multi-GB asset. Probe failure stays unknown. A late staging response cannot label a different current selection verified.
- **Loaded**: native runtime state plus the last reported runtime configuration path, matched to the selection or its staged cache path. Another or unidentified model is shown separately. Editable configuration does not itself establish runtime identity.
- **Projector configured**: a path is not proof of file presence, vision qualification or a loaded multimodal workflow. Speech is not tested here.

Refresh observations performs reads only. Checklist links scroll to existing controls. Health responses remain on the page instead of replacing all controls with an error screen. Existing Load, Unload/Cancel loading, cache staging, health and advanced configuration controls remain explicit actions.

Chat and Model Manager disclose ordinary trusted-host execution, partial enforcement and static `AllowedOnce` confirmation. They do not imply per-action approval, general undo, network isolation or reviewed-worktree protection. The separate Coding Tasks isolation lane remains blocked on Windows/macOS; this change does not modify admission checks or execution policy.

## PDF extraction

`documents.rs` delegates to the pure `documents_pdf.rs` module using the already-declared lopdf **0.33** dependency:

- Resolves page streams, parses text operators and invokes lopdf `extract_text`; no parenthesis/line scanning fallback.
- Handles `Tj`, `TJ`, escaped/octal/hex strings, supported named encodings, quote operators and basic vertical line separation. Unicode is limited to encodings lopdf actually implements; it is not general CMap support.
- Rejects encryption, Identity-H/ToUnicode/custom encoding mappings, Form or unresolved XObjects, malformed page trees/content, missing resources and unsupported filters instead of returning a success with silently skipped pages. Image-only/empty text layers fail with an explicit **no OCR fallback** message. Mixed text/image documents report pages without readable text.
- Bounds input to **20 MiB**, page count to **100**, decoded page content to **2 MiB**, aggregate decoded content to **16 MiB**, and excerpt text to **8,000 UTF-8 bytes** plus bounded notices. Excerpts use the existing grapheme-safe truncation helper and explicit truncation/page markers. Later pages are still validated after the excerpt fills; detected page failures discard the whole result.
- lopdf 0.33 can silently accept content prefixes and some decompression failures. A parser sentinel detects trailing malformed content. Only ordinary unfiltered or single Flate streams without DecodeParms are supported; zlib header/checksum validation rejects partial Flate output. Other filter chains are explicitly unsupported.

**Resource caveat:** lopdf has no bounded decompression/cancellation API here. The decoded-size checks run after its allocation, and PDF object-stream parsing also happens inside the dependency. These limits are not a sandbox or a proven worst-case memory/time bound. The pre-existing synchronous extraction API has no cancellation hook; no cancellation claim is added. Full hostile-parser isolation would require a separate reviewed scope.

**Necessary writer compatibility correction:** the existing `doc_writer.rs` deliberately interpolated unescaped raw strings to suit the old scanner. That is not compatible with real PDF parsing and could turn text into operators. It now escapes literals and writes WinAnsi using the existing lopdf encoder. Base-14 fonts cannot represent general Unicode: PDF export rejects unrepresentable text with DOCX/TXT guidance instead of silently producing broken glyphs. It does not modify old files. Previously generated malformed/unescaped/nonstandard UTF-8 PDFs are not guaranteed to parse; no legacy scanner fallback or data migration is introduced.

Documents shows existing source ID/platform/count metadata without treating it as authenticated provenance or complete extraction evidence. Its search box is labeled as a metadata filter. Page labels describe an excerpt, not persistent answer citations, vector retrieval, or a durable imported library.

## Tests and remaining validation

- `apps/desktop/src-tauri/src/documents_pdf_tests.rs`: real generated/serialized PDFs covering text operators, supported Unicode, compression, genuine encrypted fixture (test-only password verification), malformed/missing pages, image-only/mixed pages, truncation, bounds and unsupported mappings/forms.
- `apps/desktop/src-tauri/tests/fixtures/generate-encrypted-pdf.py`: deterministic, dependency-free generator for the synthetic encrypted fixture. No user data.
- `apps/desktop/src/tests/readiness.test.mjs`: production pure helper tests, runnable with Node 24 native TypeScript stripping:
  `node --test apps/desktop/src/tests/readiness.test.mjs`
- `apps/desktop/src/tests/model-readiness-mounted.test.mjs`: real React + official Tauri mockIPC mounted tests following the existing external JSDOM harness pattern. Install the **existing lockfile** and use an external tool root with jsdom@26.1.0, then run:
  `READINESS_TEST_TOOL_ROOT=/path/to/tool-root node --test --test-concurrency=1 apps/desktop/src/tests/model-readiness-mounted.test.mjs`

Current implementation evidence is outside the checkout at `/agent/workspace/power-upgrade-evidence`; the exact handoff is `/agent/workspace/power-upgrade-handoff.md`. Full desktop/native builds, mounted UI checks, rendered layout/accessibility and physical model/speech/vault regressions are separate gates, not implied by helper/PDF test results. No speed, safety certification or workflow qualification claim is made.
