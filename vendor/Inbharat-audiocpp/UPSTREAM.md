# Upstream/provenance evidence

- Audited source: a pristine local checkout of the official `0xShug0/audio.cpp` repository.
- HEAD verified: `bb15edd78b56e035967e0eb999a6b28a62337db4` (`release-0.6`).
- `git status --porcelain`: empty before and after implementation.
- The new repository has no remote; only local traceability commits are permitted.
- Default compile database contains no `upstream/audio.cpp`, `ggml`, or audio.cpp path.
- Default runtime/source contains no copied audio.cpp implementation, model, tokenizer, dataset, demo voice, or bundled upstream asset.
- `third_party/audio_cpp/FILE_PROVENANCE.spdx.json` intentionally records an empty selected file closure.
- Optional scaffold configure validates exact clean HEAD; correct pin built, deliberately wrong pin failed.

The Apache-2.0 text is included as project licensing and for future reviewed reuse. If a future adapter selects upstream files, its file closure/notices/modifications must replace the empty ledger before distribution. Pinned configure/build and selected-test evidence is packaged under `reports/UPSTREAM_*`.
