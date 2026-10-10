"""THROWAWAY: a two-run root for the host-view spike.

`live-run` holds one running node whose dispatch registry names a live pid with
its start token. `ended-run` was launched weeks ago by an engine that predates
the dispatch registry (no `dispatches/` at all): its journal still says its node
was dispatched. Its stop (and completed teardown) is stated by `after.py` only:
the fixture writes no stop record, because today's view never reads one.
"""
import json, sys, time
from datetime import datetime, timedelta, timezone
from pathlib import Path

root, live_pid = Path(sys.argv[1]), int(sys.argv[2])
HOST = "devbox-01"
now = datetime.now(timezone.utc)
iso = lambda t: t.strftime("%Y-%m-%dT%H:%M:%S.") + f"{t.microsecond // 1000:03d}Z"


def run(name, launched, pid, launcher):
    d = root / name
    d.mkdir(parents=True)
    plan = {"schema_version": 3, "name": name, "goal": {"text": "spike"},
            "tasks": [{"id": "build", "task": "build it"}]}
    (d / "launch.json").write_text(json.dumps({
        "run_id": name, "launcher": launcher, "pid": pid, "host": HOST,
        "started": "linux-proc-stat:1", "started_at": iso(launched),
        "heartbeat_interval": 1800, "adoptions": 0}))
    stream = f"{HOST}-{pid}"
    events = [
        {"v": 2, "ts": iso(launched), "stream": stream, "seq": 0, "source": "pipeline",
         "kind": "run-started", "labels": {"run_id": name}, "payload": {"plan": plan}},
        {"v": 2, "ts": iso(launched), "stream": stream, "seq": 1, "source": "pipeline",
         "kind": "node-dispatched", "labels": {"run_id": name, "node": "build"},
         "payload": {"attempt": 1}, "artifacts": []},
    ]
    (d / "events.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events))
    return d


live = run("live-run", now - timedelta(minutes=12), live_pid, "claude-code")
ticks = Path(f"/proc/{live_pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
(live / "dispatches").mkdir()
(live / "dispatches" / f"{live_pid}-0.json").write_text(json.dumps({
    "node": "build", "pid": live_pid, "host": HOST,
    "dispatched_at": iso(now - timedelta(minutes=12)),
    "started": f"linux-proc-stat:{ticks}"}))

# Pre-registry engine: no `dispatches/`.
run("ended-run", now - timedelta(days=26), 2147483647, "codex")
