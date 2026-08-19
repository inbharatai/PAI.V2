# 63 — Vision, OCR, camera

**Status: BUILDS_NOT_RUNTIME_TESTED.**

## Source-level facts verified this session

- `SttResult.confidence` is `Option<f32>` — Whisper's CLI exposes no calibrated
  confidence (honest by comment).
- OCR (`accessibility.rs` `perform_ocr`) and image description confidence are
  `Option<f32>` with explicit comments that the vision model returns no
  calibrated confidence. `confidence: None` at both construction sites
  (L212, L282). No hardcoded confidence found anywhere in vision/OCR paths
  (grepped; the only `f32` confidence left is the agent's `ToolAction`, now
  `Option<f32>` too).
- `objects: Vec::new()` — object detection honestly reports unavailability
  instead of fabricating boxes.

## mmproj runtime fact

The mmproj model (`mmproj-gemma-4-12B-it-f16.gguf`, manifest hash-verified)
loaded successfully inside the llama-server CPU run (59 run 1). No image was
sent; image-inference correctness is not claimed.

## Human gate

Real image OCR/description through the app, camera enumeration/`getUserMedia`,
frame capture, Blind View, screen-reader behavior, permission denial, missing
camera, oversized/unsupported images. Requires the packaged app + camera
hardware interaction.
