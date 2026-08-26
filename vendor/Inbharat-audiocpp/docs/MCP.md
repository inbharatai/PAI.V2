# MCP Gateway — ibaudio-mcp

`ibaudio-mcp` exposes InBharat Audio to AI applications over the Model Context Protocol. It is a **control plane**: it lets an AI host discover capabilities, list models, detect language mix in transcript text, and read diagnostics. **Continuous PCM audio never travels over MCP** — live voice uses the native streaming API / transport (see docs/STREAMING.md and the native transport spec).

## Transport

stdio — newline-delimited JSON-RPC over the standard streams of a client-launched subprocess. This is the standard MCP binding for a local server. The gateway is **dual-era**:

- **Modern (2026-07-28)** — stateless; `server/discover` answers with protocol version, server info, and capabilities. Per-request metadata in `_meta` is accepted; requests are self-describing.
- **Legacy** — an `initialize` handshake is answered with the same capabilities so older clients connect.

## Tools

| Tool | Description | Input |
|---|---|---|
| `audio.capabilities` | Runtime capabilities + feature flags | `{}` |
| `audio.models` | Registered model catalog (task, availability, honest streaming label, availability reason) | `{}` |
| `audio.detect_language` | Script/text language-mix estimate — **not acoustic LID**; shared scripts and Romanized Indian text remain uncertain | `{"text": "…"}` |
| `audio.language_packs` | List the hash-pinned all-22 pack catalog (language, manifest, SHA-256, scripts). A pack entry is not a support claim. | optional `{"root":"packs"}` |
| `audio.health` | Diagnostics snapshot | `{}` |

## Resources

`ibaudio://capabilities`, `ibaudio://models`, `ibaudio://language-packs`, `ibaudio://metrics` — JSON read via `resources/read`.

## Verified in this sandbox

The gateway was exercised over real stdio JSON-RPC: `server/discover`, `tools/list`, `tools/call` for `audio.models` and `audio.detect_language`, and `resources/read` for `ibaudio://capabilities` all returned correct responses. A real defect was found and fixed during this verification: `audio.detect_language` initially returned `hindi:0` for mixed text because the codeswitch module used a byte loop that never counted Devanagari (isalpha is false for UTF-8 lead bytes). The module now delegates to the UTF-8-correct Bharat adaptation layer; mixed `Hello नमस्ते` returns nonzero hindi/hinglish.

## Boundaries

- No audio payload tools are exposed. Adding PCM-over-MCP is explicitly out of scope.
- Everything goes through the public C ABI; the gateway holds its own runtime and never touches engine internals.
- Dependency-free: a minimal purpose-built JSON reader/writer is used (no external JSON library), consistent with the project's no-dependency rule.
- Streamable HTTP transport is a later phase; the 2026-07-28 revision's `MCP-Protocol-Version`/`Mcp-Method`/`Mcp-Name` header rules are noted for it but not implemented here.
