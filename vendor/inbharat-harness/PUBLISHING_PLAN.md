# Publication plan

Status: local only. Do not publish, push, tag remotely, or create a public repository without explicit approval.

Before any publication:

1. Re-run formatting, check, tests, Clippy with warnings denied, release build, CLI smoke, routing benchmark, C ABI manifest comparison, and clean archive extraction tests.
2. Confirm the working tree and local history contain no secrets, credentials, private paths, model files, build caches, or generated session data.
3. Regenerate dependency/SBOM and licence evidence for every enabled optional provider.
4. Review security limitations, threat model, C ABI ownership, session-format compatibility, and platform support claims.
5. Produce deterministic source archives, SHA-256 manifests, provenance, and signed attestations using an approved release environment.
6. Obtain product, security, legal, and trademark approval.
7. Publish source and binaries only for platforms with executed evidence; label build-only and pending targets accurately.

The local release candidate is not a production sandbox, authenticated remote service, encrypted memory store, or general untrusted-code runner.
