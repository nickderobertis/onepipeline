#!/usr/bin/env python3
"""Summarize the local profile (profile-chain.sh's outputs, copied to out/local/) for the report.

    python3 -I spike/onepipeline-ci/local_tables.py spike/onepipeline-ci/out/local          # Markdown
    python3 -I spike/onepipeline-ci/local_tables.py spike/onepipeline-ci/out/local --json   # figures

Reads <label>.json (local-profile.sh's summary), <label>.log.gz (the recipe's
timestamped output) and <label>.e2e.txt (the e2e tier's Nx terminal output) for
the labels gate-cold, gate-warm, check-affected-cold and check-affected-warm,
and attempts.json (the runs that did not produce a valid profile, and why).
A label with no summary is reported as missing, never estimated.
"""

import gzip
import json
import re
import sys
from pathlib import Path

d = Path(sys.argv[1])
as_json = "--json" in sys.argv
LABELS = ["gate-cold", "gate-warm", "check-affected-cold", "check-affected-warm"]
runs = {lb: json.load(open(d / f"{lb}.json")) for lb in LABELS if (d / f"{lb}.json").exists()}
attempts = json.load(open(d / "attempts.json")) if (d / "attempts.json").exists() else []


def lines(label):
    p = d / f"{label}.log.gz"
    if not p.exists():
        return []
    out = []
    for raw in gzip.open(p, "rt", errors="replace"):
        parts = raw.rstrip("\n").split(" ", 2)
        if len(parts) == 3:
            try:
                out.append((float(parts[0]), re.sub(r"\x1b\[[0-9;]*m", "", parts[2])))
            except ValueError:
                pass
    return out


def check_phase(label):
    """Seconds from the recipe's start to its `check: ok` line, and from there to the end."""
    r = runs.get(label)
    if not r:
        return None, None
    start = r["start_ms"] / 1000 if "start_ms" in r else None
    ok = next((t for t, text in lines(label) if text.strip() == "check: ok"), None)
    if ok is None or start is None:
        return None, None
    return ok - start, r["wall_s"] - (ok - start)


def tasks(label):
    """Each Nx task's first execution in the recipe (a later cached replay of it is not counted)."""
    seen = {}
    for inv in runs.get(label, {}).get("invocations", []):
        for t in inv["tasks"]:
            seen.setdefault(t["task"], (t["cache"], t["duration_s"], t["status"]))
    return seen


def e2e(label):
    p = d / f"{label}.e2e.txt"
    if not p.exists():
        return None
    text = re.sub(r"\x1b\[[0-9;]*m", "", p.read_text(errors="replace"))
    m = re.search(r"Summary \[\s*([0-9.]+)s\]\s+(\d+) tests? run: (\d+) passed(?: \((\d+) slow\))?", text)
    slow = {}
    for sm in re.finditer(r"SLOW \[>\s*([0-9.]+)s\]\s+(?:\(\s*\d+/\d+\)\s+)?onepipeline::e2e (\S+)", text):
        slow[sm.group(2)] = max(slow.get(sm.group(2), 0.0), float(sm.group(1)))
    return {"wall_s": float(m.group(1)), "tests": int(m.group(2)), "passed": int(m.group(3)),
            "slow": int(m.group(4) or 0), "slow_lower_bounds": dict(sorted(slow.items(), key=lambda kv: -kv[1]))} if m else None


def fmt(v):
    return "–" if v is None else f"{v:,.0f}"


cold_check, cold_lint = check_phase("gate-cold")
warm_check, warm_lint = check_phase("gate-warm")
fig = {
    "nproc": next((r["nproc"] for r in runs.values()), "?"),
    "load_range": ", ".join(sorted({r["loadavg_before"].split()[0] for r in runs.values()} | {r["loadavg_after"].split()[0] for r in runs.values()})) or "?",
    "walls": {lb: {"wall_s": r["wall_s"], "uptime_s": r["uptime_s"], "paused": r["paused"], "exit": r["exit"],
                   "load_before": r["loadavg_before"], "load_after": r["loadavg_after"]} for lb, r in runs.items()},
    "check_in_gate_s": {"cold": cold_check, "warm": warm_check},
    "lint_llm_diff_in_gate_s": {"cold": cold_lint, "warm": warm_lint},
    "e2e": {lb: e2e(lb) for lb in LABELS},
    "attempts": attempts,
}

if as_json:
    json.dump(fig, sys.stdout, indent=1)
    sys.stdout.write("\n")
    sys.exit(0)

print("Attempts that produced no valid profile, and why (each named, none estimated):\n")
for a in attempts:
    print(f"- **{a['name']}** — {a['why']}")
print()
print("| Recipe run | Exit | Wall clock s | Uptime clock s | VM pause | Load avg before → after |")
print("| --- | --- | --- | --- | --- | --- |")
for lb in LABELS:
    r = runs.get(lb)
    if not r:
        print(f"| `just {lb.rsplit('-', 1)[0]}` {lb.rsplit('-', 1)[1]} | MISSING | – | – | – | – |")
        continue
    print(f"| `just {r['recipe']}` {r['mode']} | {r['exit']} | {r['wall_s']:,.0f} | {r['uptime_s']:,.0f} | {'yes' if r['paused'] else 'no'} | {r['loadavg_before'].split()[0]} → {r['loadavg_after'].split()[0]} |")
print(f"| `just check` inside `gate` cold (start → `check: ok`) | | {fmt(cold_check)} | | | |")
print(f"| `just check` inside `gate` warm | | {fmt(warm_check)} | | | |")
print(f"| `lint-llm-diff` inside `gate` cold (`check: ok` → end) | | {fmt(cold_lint)} | | | |")
print(f"| `lint-llm-diff` inside `gate` warm | | {fmt(warm_lint)} | | | |")
print()
print("Every Nx task each recipe ran, first execution only (s; `miss` ran, `hit` replayed from the cache):\n")
print("| Nx task | gate cold | gate warm | check-affected cold | check-affected warm |")
print("| --- | --- | --- | --- | --- |")
ts = {lb: tasks(lb) for lb in LABELS}
names = sorted({n for t in ts.values() for n in t}, key=lambda n: -max((ts[lb].get(n, ("", 0, 0))[1] for lb in LABELS)))
for n in names:
    cells = []
    for lb in LABELS:
        c = ts[lb].get(n)
        cells.append("–" if not c else f"{c[1]:,.1f} {'hit' if 'hit' in (c[0] or '') else 'miss'}{'' if c[2] == 0 else ' FAILED'}")
    print(f"| `{n}` | " + " | ".join(cells) + " |")
print()
print("Nx invocations (s):\n")
print("| Recipe run | Invocation | Duration |")
print("| --- | --- | --- |")
for lb in LABELS:
    for inv in runs.get(lb, {}).get("invocations", []):
        if inv["command"].startswith("nx run-many") or inv["command"].startswith("nx affected") or inv["duration_s"] > 5:
            print(f"| {lb} | `{inv['command']}` | {inv['duration_s']:,.1f} |")
for lb in LABELS:
    e = fig["e2e"][lb]
    if e:
        print(f"\nThe e2e tier (instrumented, `onepipeline-e2e:test`) in {lb}: nextest `Summary [{e['wall_s']:.1f}s] {e['tests']} tests run: {e['passed']} passed ({e['slow']} slow)`.")
        if e["slow_lower_bounds"]:
            top = list(e["slow_lower_bounds"].items())[:20]
            print("Its tests nextest marked slow, with the largest threshold each crossed (a lower bound on its duration; the default profile prints no per-test duration for a passing test): "
                  + "; ".join(f"{k} > {v:.0f} s" for k, v in top) + ".")
