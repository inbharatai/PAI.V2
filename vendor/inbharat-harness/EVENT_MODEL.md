# Session Format v1

Each physical JSONL line is one canonical JSON event:

```json
{
  "format": 1,
  "session_id": "s-...",
  "seq": "42",
  "time_ms": "1730000000000",
  "type": "assistant.message",
  "event_version": 1,
  "required_for_replay": true,
  "previous_chain": "0123456789abcdef",
  "chain": "fedcba9876543210",
  "data": {}
}
```

Numbers that can exceed signed 64-bit JSON support are decimal strings. Objects use deterministic lexical key order. Format and event versions start at 1; unknown required types and unsupported versions fail loud.

## Invariants

- one writer; `seq` equals the zero-based physical event index;
- event session ids never change inside one log;
- each checksum covers the envelope excluding its own `chain` field and names the prior chain;
- turn and step boundaries are balanced;
- request headers precede provider dispatch and contain route, provider/model, system text, and visible tools;
- tool-call ids are unique, and a result refers to exactly one earlier call before step end;
- every `approval.asked` id is unique and has exactly one `approval.decided` outcome;
- checkpoints occur before model requests and top-level tool effects;
- credentials are references and attachments are metadata only.

## Trajectories

- `minimal`: durable boundaries, request, assembled message, and tool facts; no raw stream chunks.
- `standard`: text/tool/finish chunks plus authoritative facts.
- `diagnostic`: every indexed chunk including start/end, reasoning, and usage.

All modes retain enough authoritative facts to audit the request and final output; diagnostic mode maximizes provider-stream evidence.

## Repair

A physically torn, unterminated final JSON record is truncated to the last valid byte boundary before logical repair. On interrupted tails, missing tool outcomes become synthesized `TOOL_OUTCOME_UNKNOWN` results and unanswered approvals become `unavailable`. The core never blindly re-executes an uncertain side effect. It then closes the open step and turn as interrupted. Repair counts every synthesized event against a hard bound.

## Fork

Fork requires an expected source revision and a boundary whose event is `turn.end`. The new session records parent and boundary and imports the completed transcript as explicitly untrusted context. Process-local inbox, grants, jobs, and approval state are not inherited.

The checksum chain is corruption detection, not cryptographic authentication.
