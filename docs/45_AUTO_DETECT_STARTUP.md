# 45 — UnoOne Power Auto-Detect Startup

Status: `IMPLEMENTED_NOT_TESTED`

The existing UnoOne Power interface now follows this source flow:

```text
STARTING
→ VALIDATING_PAI / CHECKING_ASSETS
→ WAITING_FOR_UNLOCK
→ UNLOCKING
→ SCANNING_HOST
→ SELECTING_BACKEND
→ STARTING_MODEL
→ VERIFYING_MODEL
→ READY or LIMITED_MODE
```

All requested states are represented, including `WAITING_FOR_PAI`,
`PAI_INVALID`, `PAI_CONNECTED`, `DISCONNECTED`, `ERROR`, and
`SHUTTING_DOWN`.

Implemented:

- consumes `--vault-root` from Dock/Starter;
- otherwise automatically scans removable volumes;
- strict validation before password UI;
- active waiting/validating/invalid states;
- `Rescan` retained only as fallback;
- single-instance reconnect/focus handoff;
- background removal monitor;
- removal cleanup for model, capture buffers, and vault keys;
- automatic hardware scan, backend selection, Gemma 12B start, and health
  verification after password unlock;
- `READY` is assigned only by Rust after a managed server identity exists and
  the health endpoint succeeds;
- host Ollama and LM Studio fallbacks removed.

Evidence:

- `npm run build`: passed;
- `npm run lint`: passed;
- Rust format: passed;
- Rust compile/runtime: `BLOCKED_BY_ENVIRONMENT` (WDAC error 4551).
