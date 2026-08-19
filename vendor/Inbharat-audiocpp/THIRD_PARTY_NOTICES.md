# Third-party notices

The authoritative notice file for this release candidate is `licenses/THIRD_PARTY_NOTICES.md`; the Apache-2.0 text is in `licenses/Apache-2.0.txt`.

The default InBharat Audio build contains independently authored runtime code and no copied audio.cpp, ggml, SentencePiece, model-weight, tokenizer, or voice asset. The optional audio.cpp adapter scaffold records the reviewed Apache-2.0 upstream pin but does not compile or link upstream code.

If selective upstream reuse is enabled later, preserve the audio.cpp Apache-2.0 notice requirements, mark modified files, carry the exact licences and notices for the shipped closure, and resolve missing provenance before distribution. Static linking does not remove notice obligations. Model and accelerator runtime licences require separate review.
