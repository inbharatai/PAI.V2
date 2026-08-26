# as-IN (Assamese) — staged, not in the default build

Assamese is intentionally a **separate validation effort**, not a bundled pack.

What exists upstream for Assamese today:

- **ASR:** AI4Bharat IndicConformer covers all 22 scheduled languages including Assamese — but it is a NeMo/PyTorch `.nemo` stack, which conflicts with the no-Python-in-core objective. Path: local-service provider first, native conversion research later.
- **TTS:** AI4Bharat IndicF5 covers Assamese — Hugging Face, `trust_remote_code`, Python.
- **Remote:** Sarvam Saaras v3 (STT) and Bulbul v3 (TTS) both list Assamese — but that is a remote provider, OFF by default.

No InBharat Assamese pack is shipped until at least one provider has **inference-level evidence** for Assamese on a supported platform. A manifest asserting `as-IN` without that evidence would violate the capability-is-evidence rule. This directory holds the placeholder so the staging is explicit.
