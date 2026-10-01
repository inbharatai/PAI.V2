#!/usr/bin/env python3
"""Tool-contract sync gate: tools.v1.json  ==  the REAL production tool tables.

Checks both directions:
  1. Every tool in CanonicalToolRegistry.kt (42 pinned ids) appears in the contract with
     the risk class SafetyGuard.kt actually assigns and the permission ToolPermissionRegistry.kt
     actually requires, plus matching param names/types/required/order.
  2. Every contract android tool exists in the registry (no invented tools).
  3. Every SafetyGuard BLOCK-tier name appears as status=blocked in the contract.
  4. Contract-internal invariants (risk enum, unique ids, equivalence resolution,
     confirmation-window timeouts) — the same rules the Rust crate enforces, so the
     Android CI lane fails even though it does not build Rust.

Modeled on scripts/check_speech_language_sync.py (the proven speech-contract pattern).
Run from repo root: python scripts/check_tool_contract_sync.py
Exits 0 when in sync, 1 with a printed diff and fix instruction otherwise.
"""
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CONTRACT = os.path.join(ROOT, "packages", "tool-contracts", "tools.v1.json")
REGISTRY = os.path.join(ROOT, "android-app", "UnoOneAgent", "core", "src", "main", "java",
                        "com", "unoone", "agent", "core", "model", "CanonicalToolRegistry.kt")
SAFETY = os.path.join(ROOT, "android-app", "UnoOneAgent", "safetyguard", "src", "main", "java",
                       "com", "unoone", "agent", "safetyguard", "SafetyGuard.kt")
PERMS = os.path.join(ROOT, "android-app", "UnoOneAgent", "core", "src", "main", "java",
                     "com", "unoone", "agent", "core", "safety", "ToolPermissionRegistry.kt")
CRATE_MARKER = os.path.join(ROOT, "packages", "tool-contracts", "src", "lib.rs")

RISK_CLASSES = {"DIRECT", "CONFIRM", "STRONG_CONFIRM", "BLOCK"}
TYPE_MAP = {
    "STRING": "string",
    "INT": "integer",
    "BOOLEAN": "boolean",
    "FLOAT": "number",
    "DOUBLE": "number",
    "STRING_LIST": {"type": "array", "items": {"type": "string"}},
}


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def parse_registry(src):
    """CanonicalToolRegistry: id -> (description, ordered params)."""
    body = src.split("object CanonicalToolRegistry {", 1)[1]
    blocks = re.findall(
        r'val\s+\w+\s*=\s*ToolSchema\(\s*"([a-z_]+)",\s*"((?:[^"\\]|\\.)*)",\s*'
        r'(?:listOf\((.*?)\)\s*\)|emptyList\(\))\s*\)',
        body, re.S)
    tools, order = {}, []
    for tid, desc, params_src in blocks:
        params = []
        for pm in re.finditer(
            r'ToolParamSchema\(\s*"([A-Za-z_]+)",\s*ToolParamType\.(\w+),\s*'
            r'(?:required\s*=\s*)?(true|false)(?:,\s*description\s*=\s*"((?:[^"\\]|\\.)*)")?\s*\)',
            params_src, re.S):
            params.append({
                "name": pm.group(1),
                "type": TYPE_MAP[pm.group(2)],
                "required": pm.group(3) == "true",
            })
        if tid not in tools:
            order.append(tid)
        tools[tid] = params
    return tools, order


def parse_risks(src):
    return dict(re.findall(r'"([a-z_]+)"\s+to\s+RiskLevel\.(\w+)', src))


def parse_permissions(src):
    perms = {}
    for m in re.finditer(r'"([a-z_]+)"\s+to\s+listOf\(', src):
        tid, pos = m.group(1), m.end() - 1
        depth, end = 0, None
        for i in range(pos, len(src)):
            if src[i] == "(":
                depth += 1
            elif src[i] == ")":
                depth -= 1
                if depth == 0:
                    end = i
                    break
        reqs = []
        for p in re.finditer(r'PermissionRequirement\.(\w+)(?:\(Manifest\.permission\.(\w+)\))?',
                             src[pos + 1:end]):
            kind, runtime = p.group(1), p.group(2)
            if kind == "None":
                reqs.append("none")
            elif kind == "RuntimePerm":
                reqs.append("runtime:" + runtime)
            else:
                reqs.append(kind.lower().replace("mediaprojection", "media_projection"))
        perms[tid] = " + ".join(reqs)
    return perms


def main():
    if not os.path.exists(CRATE_MARKER):
        fail("packages/tool-contracts/src/lib.rs is missing — the Rust embed of the contract is required.")
    contract = json.loads(read(CONTRACT))
    if contract.get("schema") != "inbharat.pai.tools.v1":
        fail("contract schema must be inbharat.pai.tools.v1")

    registry, order = parse_registry(read(REGISTRY))
    risks = parse_risks(read(SAFETY))
    perms = parse_permissions(read(PERMS))

    problems = []

    # Invariants over the contract itself.
    ids = [t["id"] for t in contract["tools"]]
    if len(ids) != len(set(ids)):
        problems.append("duplicate tool ids in contract")
    by_id = {t["id"]: t for t in contract["tools"]}
    for t in contract["tools"]:
        if t["risk_class"] not in RISK_CLASSES:
            problems.append(f"{t['id']}: invalid risk_class {t['risk_class']}")
        if t.get("equivalent") and t["equivalent"] not in by_id:
            problems.append(f"{t['id']}: equivalent '{t['equivalent']}' does not exist")
        if t["risk_class"] == "BLOCK" and t.get("status") != "blocked":
            problems.append(f"{t['id']}: BLOCK risk must carry status=blocked")
        if t.get("status") == "blocked" and t["risk_class"] != "BLOCK":
            problems.append(f"{t['id']}: status=blocked requires risk_class=BLOCK")
        if "android" in t["platforms"] and t["status"] == "active":
            if t["risk_class"] in ("CONFIRM", "STRONG_CONFIRM") and t.get("timeout_ms") != 60000:
                problems.append(f"{t['id']}: CONFIRM/STRONG_CONFIRM must carry the 60s window")

    # Registry -> contract (every production tool declared, fields agree).
    for tid in order:
        t = by_id.get(tid)
        if t is None:
            problems.append(f"tool '{tid}' exists in CanonicalToolRegistry but NOT in tools.v1.json")
            continue
        if tid in risks and t["risk_class"] != risks[tid]:
            problems.append(f"{tid}: risk drift — contract={t['risk_class']} SafetyGuard={risks[tid]}")
        if tid in perms and t["permission"] != perms[tid]:
            problems.append(f"{tid}: permission drift — contract={t['permission']} registry={perms[tid]}")
        reg_params = registry[tid]
        con_params = t.get("params", [])
        reg_names = [p["name"] for p in reg_params]
        con_names = [p["name"] for p in con_params]
        if reg_names != con_names:
            problems.append(f"{tid}: param drift — contract={con_names} registry={reg_names}")
        for rp, cp in zip(reg_params, con_params):
            if rp["type"] != cp["type"]:
                problems.append(f"{tid}.{rp['name']}: param type drift "
                                f"(contract={cp['type']}, registry={rp['type']})")
            if rp["required"] != cp["required"]:
                problems.append(f"{tid}.{rp['name']}: required drift")

    # Contract -> registry (no invented android tools).
    for t in contract["tools"]:
        if "android" in t["platforms"] and t["status"] == "active" and t["id"] not in registry:
            problems.append(f"contract tool '{t['id']}' is not a real CanonicalToolRegistry tool")

    # SafetyGuard BLOCK-tier classes must be present and blocked.
    for tid, risk in risks.items():
        if risk == "BLOCK":
            t = by_id.get(tid)
            if t is None:
                problems.append(f"BLOCK-tier class '{tid}' missing from contract")
            elif t.get("status") != "blocked":
                problems.append(f"BLOCK-tier class '{tid}' must be status=blocked")

    if problems:
        print("TOOL CONTRACT SYNC FAILED:", len(problems), "problem(s):")
        for p in problems:
            print("  -", p)
        print("\nFix: edit packages/tool-contracts/tools.v1.json AND/OR the Kotlin table in the "
              "same commit. tools.v1.json is the shared contract; the Kotlin tables are the "
              "production truth this script pins against.")
        sys.exit(1)
    print(f"tool contract sync OK: {len(order)} registry tools, "
          f"{sum(1 for t in contract['tools'] if t['status'] == 'blocked')} blocked classes, "
          f"{len(contract['tools'])} contract entries.")


def fail(msg):
    print("TOOL CONTRACT SYNC FAILED:", msg)
    sys.exit(1)


if __name__ == "__main__":
    main()