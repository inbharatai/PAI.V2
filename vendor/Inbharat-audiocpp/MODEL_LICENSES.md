# Model licences

Runtime and model licences are separate. The default release candidate contains no external neural model weights, tokenizers, voices, or datasets.

The built-in deterministic reference ASR, reference TTS, and energy VAD descriptors identify `Apache-2.0`, use `builtin://` source URIs, and hash their implementation identities. The deferred KWS descriptor is unavailable and carries no weight artifact.

The machine-readable authority is `licenses/MODEL_LICENSES.json` together with `models/registry.v1.json`. Any future model entry must record publisher, immutable source and revision, task/family, format, quantization, languages, streaming class, sample rate, backend compatibility, artifact size, measured RAM, exact cryptographic hash, licence, commercial-use/redistribution/modification restrictions, consent or voice restrictions where applicable, and an explicit release decision. Unknown or moving revisions are prohibited.

No model may be bundled or downloaded automatically until its licence and hash are reviewed. The upstream audio.cpp model catalogue is not inherited as an InBharat release manifest.
