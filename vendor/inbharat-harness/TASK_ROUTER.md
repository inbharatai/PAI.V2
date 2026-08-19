# Routing Contract

Routing is a pure deterministic function evaluated before any loop starts.

| Level | Entry rule | Runtime | Default hard bounds |
|---|---|---|---|
| L0 | fallback for ordinary text | one direct model request, zero tools | 1 step, 0 tools, 15 s, 32 KiB |
| L1 | anchored `read file`, `show file`, `list files`, `write file`, `run command` grammar | exactly one parsed action, no agent loop | 1 step, 1 tool, 20 s, 64 KiB |
| L2 | explicit `/l2`/`agent:l2` or anchored bounded-task grammar | finite model/tool loop | 8 steps, 12 tools, 60 s, 256 KiB |
| L3 | explicit `/l3`/`agent:l3` or anchored repository/workspace goal grammar plus one-shot confirmation | finite verified goal/workspace rounds | 32 steps, 64 tools, 8 rounds/jobs, depth 2, 300 s, 2 MiB |

Unknown language fails cheap to L0. Substring words such as “agent”, “file”, “command”, “build”, and “workspace” do not trigger by themselves. Explicit levels still pass deployment ceiling and capability checks.

Runtime escalation is monotonic and can advance only one adjacent level per decision. A cause requesting L3 from L0 is denied; callers must receive a new auditable decision at each boundary. No policy or provider may silently widen a core denial.

The test/benchmark confusion set contains 600 generated ordinary prompts and dedicated adversarial word-placement cases. The RC gate is zero false L2/L3 activations on this set.
