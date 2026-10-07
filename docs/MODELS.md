# UnoOne Models — Installation, Integrity, Profiles

> **Updated 2026-10-07:** replaced the stale Gemma 3n / legacy-folder / "URL-only, no hash" claims
> with the shipped Android brains — Gemma 4 **E2B** (Lite, default) and **E4B** (Medium) `.litertlm`,
> both pinned (URL + SHA-256 + size) in `models_manifest.json`; corrected the health-state label, the
> E2B context size (32,768 max, not 128K), the install paths, the non-existent `punctuation` entry,
> and the object-detection section (shipped detector is the bundled EfficientDet-Lite2, see
> [`BLIND_AID_MODEL.md`](BLIND_AID_MODEL.md)); fixed the dead README anchor.

How on-device models are installed, verified, and organized on **Android**. Source manifest:
`android-app/UnoOneAgent/modelmanager/src/main/assets/models_manifest.json`. (Desktop GGUF tiers
are covered in [`MODEL_ACQUISITION_AND_DISTRIBUTION.md`](MODEL_ACQUISITION_AND_DISTRIBUTION.md) and
the repository README.)

## 1. Integrity model

Every file in the bundled manifest carries `sha256` + `sizeBytes`; network files carry a `url`, and
bundled files (e.g. `espeak-ng-data.zip`) name an APK `asset` instead. The installer
(`ModelInstaller`) streams the download, integrity-checks it, and `ModelManager.modelHealth`
reports three states:

| State | Meaning |
|---|---|
| **Verified** | File present, size matches, SHA-256 matches |
| **Present — integrity metadata incomplete; release blocked** | File present & non-empty, but its manifest entry has no sha256/size to byte-check (no bundled entry is in this state today) |
| **Needs repair** | Missing, wrong size, or hash mismatch |

### Gemma (LLM) — honest status

UnoOne has **two Gemma 4 planning-brain tiers** (see [README → Local model contract](../README.md#local-model-contract);
authoritative spec `core/src/main/java/com/unoone/agent/core/model/BrainModel.kt`):

- **Gemma 4 E2B (Lite, default)** — manifest id `gemma-4-e2b`, folder `brain/gemma-4-e2b/`, file
  `gemma-4-E2B-it.litertlm`, 2,588,147,712 bytes, SHA-256
  `181938105e0eefd105961417e8da75903eacda102c4fce9ce90f50b97139a63c`, 32,768-token maximum context,
  6,144 MB minimum RAM. Loaded on the primary Xiaomi 14 (CPU backend; see
  [`DEVICE_VERIFICATION.md`](../DEVICE_VERIFICATION.md) §2).
- **Gemma 4 E4B (Medium)** — manifest id `gemma-4-e4b`, folder `brain/gemma-4-e4b/`, file
  `gemma-4-E4B-it.litertlm`, 3,659,530,240 bytes, SHA-256
  `0b2a8980ce155fd97673d8e820b4d29d9c7d99b8fa6806f425d969b145bd52e0`, 32,768-token maximum context,
  8,192 MB minimum RAM. **Not yet tested on a device**, and at HEAD the app's load paths load E2B only
  (`ModelTierSelector` is JVM-tested in `:core` but not yet called from `:app`).

Both entries ship with the upstream `litert-community` URL **and** the exact SHA-256 + byte size, so
an installed or manually imported file with the right bytes reports **Verified** and any other bytes
report **Needs repair**. The E2B pin is also asserted by `scripts/ci/check_repo_invariants.py`, which
additionally prohibits the legacy Gemma 3n identifiers and folder. Both `BrainModelSpec`s keep
`isDeviceVerified = false`: a hash match proves integrity, not device qualification — do not call the
LLM production-ready until `modelHealth` reports Verified on a real device and the device matrix in
`DEVICE_VERIFICATION.md` is green.

### Sherpa voice models — verified

English STT (`sherpa-asr-en`), the shared Indic Omnilingual STT (`sherpa-asr-indic`), English TTS
(`sherpa-tts-en`), per-language Indic MMS TTS (`sherpa-tts-{hin,ben,tam,tel,kan,mal}`), and the
wake-word (`vad`) all carry stream-computed sha256 + sizeBytes and are integrity-checked on
install. No punctuation model is in the bundled manifest (the `punctuation` type exists in
`ModelType` but has no entry).

## 2. Current models

| Model | Type | Backend | Size | Hash | Languages |
|---|---|---|---|---|---|
| `gemma-4-E2B-it.litertlm` (`gemma-4-e2b`) | llm | any (GPU→CPU) | 2,588,147,712 B | ✅ pinned | planning — **default (Lite) brain** |
| `gemma-4-E4B-it.litertlm` (`gemma-4-e4b`) | llm | any (GPU→CPU) | 3,659,530,240 B | ✅ pinned | planning — Medium tier, **not yet device-tested** |
| `sherpa-asr-en` | asr | cpu | ~70 MB | ✅ | English |
| `sherpa-asr-indic` | asr | cpu | ~279 MB archive / ~348 MB extracted | ✅ | hi/bn/ta/te/kn/ml (shared Omnilingual CTC) |
| `sherpa-tts-en` | tts | cpu | ~110 MB | ✅ | English (Coqui VITS + espeak-ng-data) |
| `sherpa-tts-<lang>` | tts | cpu | ~109 MB each | ✅ | hi/bn/ta/te/kn/ml (MMS VITS) |
| `vad` | vad | cpu | ~70 MB | ✅ | English wake word |

Not in the manifest: the Blind Aid object detector is bundled in the APK —
EfficientDet-Lite2 int8 at `phonecontrol/src/main/assets/models/efficientdet_lite2_int8.tflite`
(7,515,971 bytes; provenance, SHA-256 and licence in [`BLIND_AID_MODEL.md`](BLIND_AID_MODEL.md)).

Wake word is English-only (no public Indic KWS transducer exists). The *command* may be Indic; only
the wake phrase is English.

## 3. Installation paths

All paths below are relative to the app-private models root
`Android/data/com.unoone.agent/files/models/` and must match the manifest `folder` values.

- **In-app:** Model Status screen → Install (streaming progress + integrity check) or Uninstall.
  The "UnoOne Brain" card offers **Load Brain** and **Run Self-Test** for E2B.
- **Manual (ADB):** prefer in-app install. The old `scripts/adb-push-models/` scripts used a
  pre-manifest layout (flat per-model folders and a legacy Gemma folder that `ModelManager` no longer
  reads) and were removed on 2026-10-07. If you push by hand, mirror the manifest folders
  (`brain/…`, `speech/shared/…`, `speech/languages/<locale>/tts`).
- **Gemma (default E2B):** push `gemma-4-E2B-it.litertlm` into `brain/gemma-4-e2b/`, then use Model
  Status → Load Brain / Run Self-Test. The file must match the pinned SHA-256 and size to report
  Verified.
- **Gemma (E4B Medium):** push `gemma-4-E4B-it.litertlm` into `brain/gemma-4-e4b/`. It is pinned
  and health-checked, but no app load path selects it yet and it is **not device-tested**.

## 4. Download source abstraction (spec)

Roadmap: classify each manifest source as `bundled | public_url | authenticated_hf | manual_import`.
Hugging Face URLs can be gated/change; for Gemma, prefer `manual_import` (copy file → verify SHA →
load) as the primary path, with `public_url` as a convenience.

## 5. Model profiles (spec)

Don't force users to download everything. Roadmap installer profiles:

| Profile | Contents |
|---|---|
| Tiny | Rules engine only + English STT/TTS |
| Voice | English + one Indic language (STT + TTS + wake word) |
| Agent | Voice profile + Gemma 4 E2B (E4B Medium optional) |
| Blind Aid | Camera + object detection only |
| Full | Everything |

## 6. Object detection (Blind Aid)

Shipped: the bundled **EfficientDet-Lite2 int8** detector (MediaPipe `ObjectDetector`, COCO labels),
documented in [`BLIND_AID_MODEL.md`](BLIND_AID_MODEL.md). It runs independently of Gemma.
Optional override: if a user-installed detector exists at
`models/vision/blind-aid/custom_yolov8.tflite`, `BlindAidManager` uses it instead of the bundled
asset. That override is not a manifest entry (no install or integrity flow). Roadmap: a separate
optional manifest entry for object detection, so a custom detector gets the same integrity checks.

## 7. Storage

All models live in app-private storage (`getExternalFilesDir("models")`), so no
`MANAGE_EXTERNAL_STORAGE` / `READ_EXTERNAL_STORAGE` (on API 29+) is needed. Storage usage is
shown on the Model Status screen.
