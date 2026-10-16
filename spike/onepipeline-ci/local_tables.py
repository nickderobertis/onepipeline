#!/usr/bin/env python3
"""Summarize the local profile (out/local/) for the report.

    python3 -I spike/onepipeline-ci/local_tables.py spike/onepipeline-ci/out/local          # Markdown
    python3 -I spike/onepipeline-ci/local_tables.py spike/onepipeline-ci/out/local --json   # figures

Reads attempts.json (every attempt, and why it did or did not yield a profile),
<label>.json for any valid run (labels gate-cold, gate-warm, check-affected-cold,
check-affected-warm), and the partial records of invalid attempts:
attempt3-gate-cold.json (local-profile.sh's summary) and
attempt4-gate-cold.runs.json (Nx's run.json snapshots). A figure with no valid
run behind it is reported as MISSING, never estimated; a partial task timing is
labelled with the attempt it came from.
"""

import datetime as dt
import json
import sys
from pathlib import Path

d = Path(sys.argv[1])
as_json = "--json" in sys.argv
LABELS = ["gate-cold", "gate-warm", "check-affected-cold", "check-affected-warm"]
valid = {lb: json.load(open(d / f"{lb}.json")) for lb in LABELS if (d / f"{lb}.json").exists()}
attempts = json.load(open(d / "attempts.json"))


def ms(v):
    return dt.datetime.fromisoformat(v.replace("Z", "+00:00")).timestamp()


partial = {}
if (d / "attempt3-gate-cold.json").exists():
    a3 = json.load(open(d / "attempt3-gate-cold.json"))
    for inv in a3["invocations"]:
        for t in inv["tasks"]:
            partial.setdefault(t["task"], {})["attempt 3"] = (t["duration_s"], t["cache"], t["status"])
if (d / "attempt4-gate-cold.runs.json").exists():
    for run in json.load(open(d / "attempt4-gate-cold.runs.json")):
        for t in run["tasks"]:
            partial.setdefault(t["taskId"], {})["attempt 4"] = (ms(t["endTime"]) - ms(t["startTime"]), t["cacheStatus"], t["status"])

REQUIRED = [
    "`just gate` wall clock, Nx-cold and Nx-warm",
    "`just check` wall clock (full), Nx-cold and Nx-warm",
    "`just check-affected` wall clock, Nx-cold and Nx-warm",
    "each Nx target's cold and warm time, including `doc` and `budgets`",
    "each test tier's (`test-rest`, e2e, contract, note, stop-verdict, npm, `test-quick`, coverage) cold and warm time",
    "the e2e tier's 20 slowest tests on this host",
]
fig = {"valid_runs": sorted(valid), "missing": [r for r in REQUIRED] if not valid else [],
       "attempts": attempts,
       "partial_cold_tasks": {k: {a: {"duration_s": round(v[0], 1), "cache": v[1], "exit": v[2]} for a, v in vs.items()}
                              for k, vs in sorted(partial.items())}}

if as_json:
    json.dump(fig, sys.stdout, indent=1)
    sys.stdout.write("\n")
    sys.exit(0)

if not valid:
    print("**No valid local profile was obtained, so these figures are MISSING:** " + "; ".join(REQUIRED) + ". "
          "Four attempts were made; none is estimated from. The CI figures above (gate, release-sweep and Release `test` "
          "durations, and the cross legs' per-test timings from nextest's own output) are the measured substitutes the "
          "findings use, each named where it is used.\n")
for a in attempts:
    print(f"- **{a['name']}**, started {a['started']}. {a['why']}")
print()
print("Partial cold task timings from the invalid attempts (s; `miss` ran, `exit` is the task's own; these are "
      "real measurements of the tasks that finished, on this loaded aarch64 host, and are not a gate or check figure):\n")
print("| Nx task | Attempt 3 (cargo-cold, Nx-cold) | Attempt 4 (cargo-warm, Nx-cold) |")
print("| --- | --- | --- |")
for task, vs in sorted(partial.items(), key=lambda kv: -max(v[0] for v in kv[1].values())):
    cells = []
    for a in ("attempt 3", "attempt 4"):
        v = vs.get(a)
        cells.append("–" if not v else f"{v[0]:,.1f} {'hit' if 'hit' in (v[1] or '') else 'miss'}, exit {v[2]}")
    print(f"| `{task}` | " + " | ".join(cells) + " |")
