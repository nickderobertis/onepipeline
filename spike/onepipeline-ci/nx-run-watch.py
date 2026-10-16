#!/usr/bin/env python3
"""Snapshot Nx's per-invocation run.json, and summarize the snapshots.

    nx-run-watch.py <run.json> <snapshot-dir>       # watch until killed
    nx-run-watch.py --summarize <snapshot-dir> <start_ms> <end_ms> <exit> <label> \
        <load_before> <load_after> <mode> <recipe> <uptime_before> <uptime_after> <commit>
"""

import datetime as dt
import glob
import json
import os
import shutil
import sys
import time


def watch(src: str, dst: str) -> None:
    seen, n = None, 0
    while True:
        try:
            st = os.stat(src)
            key = (st.st_mtime_ns, st.st_size)
            if key != seen:
                time.sleep(0.2)
                n += 1
                shutil.copy(src, f"{dst}/{n:03d}.json")
                seen = key
        except FileNotFoundError:
            pass
        time.sleep(0.5)


def ms(value: str) -> float:
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp() * 1000


def summarize(runs, start, end, status, label, lb, la, mode, recipe, up_before, up_after, commit) -> None:
    start, end = int(start), int(end)
    invocations, seen = [], set()
    for f in sorted(glob.glob(f"{runs}/*.json")):
        try:
            data = json.load(open(f))
        except json.JSONDecodeError:
            continue
        key = (data["run"]["command"], data["run"]["startTime"])
        if key in seen:
            continue
        seen.add(key)
        invocations.append({
            "command": data["run"]["command"],
            "start_offset_s": (ms(data["run"]["startTime"]) - start) / 1000,
            "duration_s": (ms(data["run"]["endTime"]) - ms(data["run"]["startTime"])) / 1000,
            "tasks": [{"task": t["taskId"], "cache": t.get("cacheStatus"), "status": t.get("status"),
                       "start_offset_s": (ms(t["startTime"]) - start) / 1000,
                       "duration_s": (ms(t["endTime"]) - ms(t["startTime"])) / 1000}
                      for t in data["tasks"]],
        })
    wall = (end - start) / 1000
    uptime = float(up_after) - float(up_before)
    print(json.dumps({"label": label, "recipe": recipe, "mode": mode, "commit": commit, "exit": int(status),
                      "wall_s": wall, "uptime_s": round(uptime, 2),
                      # A VM pause advances the wall clock and not the uptime clock.
                      "paused": abs(wall - uptime) > 30,
                      "loadavg_before": lb, "loadavg_after": la,
                      "nproc": os.cpu_count(), "invocations": invocations}, indent=1))


if __name__ == "__main__":
    if sys.argv[1] == "--summarize":
        summarize(*sys.argv[2:])
    else:
        watch(sys.argv[1], sys.argv[2])
