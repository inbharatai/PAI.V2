# Model policy and local RC registry

RC1 contains no downloaded model, tokenizer, dataset, voice, or neural weight. Three executable entries are deterministic built-in reference algorithms; KWS is a deliberately deferred interface. `registry.v1.json` mirrors the compiled descriptors and is documentation/release metadata, not a moving network catalog.

A future model entry must include SPDX license, immutable source/revision, exact SHA-256 and size, redistribution review, consent/provenance for voices, minimum runtime, task/mode capabilities, honest streaming class, CPU parity fixtures, cancellation coverage, memory budget, and per-platform evidence. A URL to `main`, an absent license, or an unchecked artifact is rejected.
