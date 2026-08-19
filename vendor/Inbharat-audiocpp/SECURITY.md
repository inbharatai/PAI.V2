# Security policy

Report suspected memory-safety, parser, path-containment, hash-verification, lifecycle, or JNI issues privately through the project owner's established security channel. Do not attach proprietary model files or personal voice data to a public report.

RC2 processes untrusted PCM/WAV under documented bounds but is a local release candidate, not a hardened sandbox. Strict external-path mode fails closed without an explicit allowed root, stream queues have an absolute cancellation ceiling, output publication is allocation-safe, and JNI performs bounded standard UTF conversion. It does not parse neural-model formats. Run external artifacts only after provenance/hash/license admission and consider process isolation for untrusted or non-preemptible adapters. See `docs/SECURITY_PATH_POLICY.md`.
