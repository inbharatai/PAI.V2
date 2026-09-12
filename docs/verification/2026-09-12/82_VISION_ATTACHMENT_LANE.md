# 82 — Vision Attachment Lane: Preview Blocked by CSP + Untruthful "I Cannot See Any Images" Answers

**Date:** 2026-09-12
**Scope:** `apps/desktop` (CSP config, harness system prefix), `packages/pai-harness-adapter` (wire-capture regression tests)
**Posture:** fifth live-caught defect during user-perspective pendrive acceptance.

## 1. Symptom (live, through the real app UI)

Attaching an image in Chat and asking what it shows produced two failures:

1. The attachment preview thumbnail never rendered (a broken-image element
   with a webview console error: *"Loading the image 'data:image/png;base64,…'
   violates the following Content Security Policy directive: 'default-src
   'self''"*).
2. Twice (15:25 and 15:52 on the 8431d80 build) the model answered
   *"I cannot see any images provided in your message. Please upload or
   attach the image"* — while a third run on a fresh server described the
   image correctly *and* claimed *"I cannot see any shapes"* in the same
   sentence, proving the vision tokens were rendered but the model
   contradicted itself about having seen them.

## 2. Root-cause isolation (live forensics)

The transport pipeline was proven innocent step by step:

- **Server:** the app's llama-server advertises `multimodal`; direct
  `/v1/chat/completions` requests with `image_url` parts — with and without
  tools, with and without long history — are answered with the correct colors
  (red/blue/green stripes) every time.
- **App wire path:** a port-squat logging proxy on the app's model-server
  port captured the exact request the app posts: `image_url` parts present,
  correct base64, correct multimodal shape (3 independent captures). Replaying
  the captured body verbatim against a real llama-server describes the image
  correctly.
- **Adapter:** a new mock-server integration test asserts attachment bytes
  reach the wire as `image_url` parts, and that missing local bytes fail
  closed instead of silently degrading to a text-only request.

Conclusion: the pixels always reach the model. The two defects are:

1. **CSP:** the Tauri CSP had no `img-src` directive, so `default-src 'self'`
   covered images and blocked the `data:` URLs the attachment preview uses —
   the thumbnail can never render.
2. **Model honesty:** the full-access system prefix briefed every capability
   the session holds *except* vision, so when a rendered image conflicted with
   the model's expectation it denied seeing one — the same class of untruthful
   self-reporting already fixed for the tool lane in doc 79's briefing.

## 3. Fix

- **`tauri.conf.json`** — CSP gains `img-src 'self' data:` so attachment
  previews render.
- **`harness_bridge.rs` (`desktop_system_prefix`)** — the briefing now states
  truthfully that attached images are delivered inline through the vision
  encoder and must be described, never disclaimed.
- **`pai-harness-adapter/src/llama_local.rs`** — two wire-capture regression
  tests (`attachment_bytes_reach_the_wire_as_image_url_parts`,
  `attachments_without_local_bytes_fail_closed`) pin the transport contract
  so a future refactor cannot silently drop images from the request.

## 4. Why CI never caught it

No test rendered an attachment preview under the real CSP, and no test asked
the live model to describe an attached image through the full chat pipeline.
The transport was untested end-to-end; the model-honesty gap is only visible
with a live model.

## 5. Post-fix acceptance (must be re-run live)

Attach an image in Chat → the thumbnail preview renders (no CSP console
error) → ask what it shows → the answer describes the actual colors without
claiming the image is missing, across repeated runs.