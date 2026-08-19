# Contributing

Keep the public C ABI minimal and backward compatible. Add tests before changing behavior; run build/test, sanitizer where available, fixture reproducibility, and ABI export checks. Do not add copied upstream source, weights, tokenizers, datasets, voices, binary Gradle wrappers, or moving network dependencies without explicit provenance/license/hash review.

New public structs start with `struct_size`/`api_version`; new outputs use owned buffers; no exception/STL/JNI type crosses the C boundary. Document ownership, thread safety, cancellation, streaming truth class, and limitations. Update model registry, notices, SPDX inventory, ABI manifest, changelog, and evidence as applicable.
