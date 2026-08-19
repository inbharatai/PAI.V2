# Security and path policy

Default reference models are file-free. For external artifacts or file hashing, strict mode requires an explicit `allowed_model_root`, canonicalizes the candidate and root, and requires every root path component to match. An empty root fails closed rather than authorizing the host filesystem. Symlink escapes therefore fail. Only regular files are accepted, artifacts and hashing requests are capped at 4 GiB in RC2, and model verification requires a caller-supplied 64-hex SHA-256.

The runtime never searches the process current directory for model specs and never accepts APK pseudo-paths. Android callers should copy approved assets to app-private regular files or later use a reviewed fd+offset abstraction. Cache root is caller-supplied/explicit; RC2 writes no model sidecars.

WAV parsing uses byte assembly, checked chunk ends, block alignment, input/allocation caps, and an explicit supported encoding set. Fuzz-style deterministic random tests and malformed fixtures must remain in release CI. Status messages should not include model contents or credentials.

Threats not solved by RC2 include malicious neural model formats (none are parsed), hard process isolation, signed package manifests, anti-rollback, and encrypted-at-rest model policy. These are gates before external model admission.
