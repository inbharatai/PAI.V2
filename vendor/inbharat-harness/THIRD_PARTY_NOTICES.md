# Third-party notices

The InBharat Harness Rust workspace has no third-party crate dependencies. `Cargo.lock` contains only the three workspace packages.

The project was designed after a source-level study of DeepSeek Harness at commit `47f943859bef60e4160492346772ded9b24f765a`. No DeepSeek Harness or Cordis source code is copied into this repository. Architectural provenance and source-review boundaries are documented in `UPSTREAM.md`, `DSH_AUDIT.md`, and `docs/SOURCE_LEDGER.md`.

Rust, Cargo, platform C libraries, and build tools are development/runtime toolchain components and retain their own licences. A distributor must regenerate an SBOM and licence report for the exact target toolchain and any future provider, sandbox, RPC, UI, or model adapter dependencies.
