"""THROWAWAY mock of the proposed `onepipeline host` renderer.

Reads today's text output over the spike fixture and prints what the proposed
view would: `text` (an ended run's dispatch counted as proven gone rather than
listed UNPROVEN) or `json` (the version-1 `--json` reading, pretty-printed).
"""
import json, re, sys

mode, before = sys.argv[1], open(sys.argv[2]).read().splitlines()
host = before[0].split(" ", 1)[1]
root = before[1].strip().removeprefix("reading ")
# The proposed reason: the ended run's recorded stop, and its completed teardown.
GONE_WHY = "its run was stopped 26 days ago and that stop's teardown completed"
units = {"ms": 1, "s": 1000, "m": 60_000, "h": 3_600_000, "d": 86_400_000}

free, rows, gone = [], [], []
for line in before[2:]:
    if line.startswith("  free space: "):
        free.append(line)
        continue
    m = re.match(r"  (\S+)\s+(\S+)\s+(\S+)\s+(\S+)(?:  UNPROVEN: (.*))?$", line)
    if not m:
        continue
    run, node, launcher, age, unproven = m.groups()
    if unproven and "dispatch registry cannot be read" in unproven:
        gone.append({"run": run, "node": node, "reason": GONE_WHY})
        continue
    n, u = re.match(r"(\d+)(\D+)", age).groups()
    rows.append({"run": run, "node": node, "launcher": launcher,
                 "age_ms": int(n) * units[u], "proof": "unproven" if unproven else "held",
                 "unproven": unproven, "_line": line})

if mode == "text":
    print(before[0]); print(before[1])
    for line in free:
        print(line)
    for r in rows:
        print(r["_line"])
    if not rows:
        print("  no live dispatches")
    if gone:
        print(f"  {len(gone)} dispatch{'es' if len(gone) > 1 else ''} proven gone: "
              + "; ".join(f"{g['run']}/{g['node']}: {g['reason']}" for g in gone))
else:
    def space(line):
        m = re.match(r"  free space: ([\d.]+) GiB of ([\d.]+) GiB .*runs root (\S+) "
                     r"and the lifecycle workspaces under (\S+)", line)
        gib = lambda s: int(float(s) * 2**30)
        return {"roots": [{"role": "runs-root", "path": m.group(3), "measured_at": None},
                          {"role": "workspaces", "path": m.group(4), "measured_at": None}],
                "free_bytes": gib(m.group(1)), "total_bytes": gib(m.group(2)), "error": None}
    print(json.dumps({
        "schema_version": 1, "host": host, "root": root, "root_error": None,
        "free_space": [space(l) for l in free],
        "dispatches": [{k: v for k, v in r.items() if k != "_line"} for r in rows],
        "gone": gone, "skipped": [],
    }, indent=2))
