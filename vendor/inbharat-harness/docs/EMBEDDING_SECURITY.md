# Embedding security contract

InBharat Harness has two builder postures:

- `HarnessBuilder::new` / `local` preserve the standalone CLI built-in tool set.
- `HarnessBuilder::embedded` / `local_embedded` register **no generic tools**.

Applications and devices SHOULD use the embedding constructors and explicitly
register typed product tools. This prevents an embedded model from gaining a
filesystem, process or network surface merely because the standalone harness
ships those tools.

## Product adapter boundary

The core must not import product vaults, UI frameworks, model runtimes or
platform SDKs. Product code implements the provider/tool traits outside the
core. A typical device embedding supplies:

1. a verified `ModelProvider`;
2. an encrypted `MemoryProvider`;
3. a permission provider tied to product policy;
4. typed tools with minimum capability manifests;
5. a confirmation provider connected to the product UI when mutation requires
   user approval;
6. a safety provider that may narrow or deny a route but never widen core
   authority.

Persistent session/trajectory data that contains user content must use the
product's protected persistence mechanism. The standalone JSONL session store
is a developer/diagnostic format and is not a substitute for an encrypted
product vault.

## Production model-provider rule

Production builds have no synthetic model provider enabled by default. `EchoModelProvider`, `MockModelProvider`, and `MockStep` are compiled/exported only with `cfg(test)` or the explicit `test-providers` Cargo feature. `RunOptions::default()` uses an unconfigured provider/model sentinel so a caller cannot accidentally receive synthetic model output. Embedding products must register and select a real `ModelProvider` explicitly.
