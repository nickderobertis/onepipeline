#!/usr/bin/env python3
"""THROWAWAY MOCK — spike-visual of authoring:tracker-dependency-reads-2026-10-21.

Stands in for the `onepipeline` binary of the `engine` node: it runs the
unchanged base binary (`$SPIKE_BASE_BIN`) and rewrites what that prints about a
node held on a private source into what the plan says the change prints. Nothing
here is the implementation; it is hard-coded wording over the base's facts.

    mock.py start <project> --detach   -> the launch notice on stderr
    mock.py status <run>               -> the held line, or `ready` once unheld
    mock.py next <run> ...             -> the `source-wait` surface's message

Inputs it reads (all set by capture.py):
  SPIKE_BASE_BIN         the unchanged base `onepipeline`
  SPIKE_EDGED_NODES      comma list of nodes whose task carries an opaque edge to
                         the private source — the store's read-scope rule, stubbed:
                         after the change only these are held on it
  ONEPIPELINE_FAKE_DIR   where `vault.query_tasks.refuse` holds the scripted
                         refusal, read for its `retry_after_seconds`
"""

import datetime as dt
import json
import os
import re
import subprocess
import sys

BASE = os.environ["SPIKE_BASE_BIN"]
EDGED = {n for n in os.environ.get("SPIKE_EDGED_NODES", "").split(",") if n}
POLL = int(os.environ.get("ONEPIPELINE_SOURCE_POLL_SECONDS", "60"))
CAP = 30 * POLL


def refusal() -> dict:
    path = os.path.join(os.environ["ONEPIPELINE_FAKE_DIR"], "vault.query_tasks.refuse")
    return json.load(open(path))


def cause(reason: str) -> str:
    return "is rate limited" if "rate-limited" in reason else "could not answer"


def when(retry_after) -> str:
    """When the source is read again: the provider's reset, else the backoff."""
    if retry_after is None:
        return (
            f"no reset time was given, so it is read again in {POLL}s, the wait "
            f"doubling each time it still does not answer, up to {CAP}s "
            f"(30 poll intervals)"
        )
    # The stub's reset is the refusal's relative wait from a synthetic launch instant
    # capture.py fixes, so a re-capture prints the same time. The real engine
    # journals the provider's reset as `retry_at_ms`.
    launched = float(os.environ["SPIKE_LAUNCHED_AT"])  # a fixed synthetic clock, never now
    at = dt.datetime.fromtimestamp(launched + retry_after, dt.timezone.utc)
    return f"it is read again at {at:%Y-%m-%d %H:%M:%S} UTC, the reset the provider gave"


def quoted(ids) -> str:
    return ", ".join(f"'{i}'" for i in ids)


def start(argv) -> int:
    done = subprocess.run([BASE, *argv], capture_output=True, text=True)
    sys.stdout.write(done.stdout)
    retry_after = refusal().get("retry_after_seconds")
    for line in done.stderr.splitlines():
        m = re.match(
            r"onepipeline: source '([^']+)', declared private, could not be asked what\s+"
            r"it records of the tasks of (.*?): (.*?) — the plan loads",
            line,
        )
        if not m:
            print(line, file=sys.stderr)
            continue
        source, _nodes, reason = m.groups()
        held = sorted(EDGED)
        if not held:
            continue  # nothing's edges name the source, so nothing is held on it
        print(
            f"onepipeline: source '{source}', declared private, {cause(reason)} and could not "
            f"be asked about the tasks whose edges name it, {quoted(held)}: {reason} — the "
            f"plan loads, and each of those nodes is held, never dispatched; {when(retry_after)}. "
            f"Nothing times this out.",
            file=sys.stderr,
        )
    return done.returncode


def status(argv) -> int:
    done = subprocess.run([BASE, *argv], capture_output=True, text=True)
    out = []
    for line in done.stdout.splitlines():
        m = re.match(r"^(  )([^:]+): held — source '[^']+', declared private.*$", line)
        if m and m.group(2) not in EDGED:
            # Its read never consulted the private source, so nothing holds it.
            out.append(f"{m.group(1)}{m.group(2)}: ready — queued for dispatch")
            continue
        u = re.match(r"^  (\d+) planner update\(s\) waiting \((.*)\)(.*)$", line)
        if u and not EDGED:
            parts = [p for p in u.group(2).split(", ") if not p.endswith("source-wait")]
            left = int(u.group(1)) - sum(
                int(p.split()[0]) for p in u.group(2).split(", ") if p.endswith("source-wait")
            )
            if left == 0:
                continue
            line = f"  {left} planner update(s) waiting ({', '.join(parts)}){u.group(3)}"
        out.append(line)
    sys.stdout.write("\n".join(out) + "\n")
    sys.stderr.write(done.stderr)
    return done.returncode


def surface_message(node: str, source: str, reason: str, retry_after) -> str:
    return (
        f"node '{node}' is held: source '{source}', declared private, {cause(reason)}, so it "
        f"could not be asked what it records of the item this task's edge names: {reason}, so "
        f"it may have a prerequisite this run cannot see. {when(retry_after)[0].upper()}"
        f"{when(retry_after)[1:]}; it dispatches the pass a read of its edges is answered and "
        f"finds nothing missing. Nothing times this out and nothing will fail the node; or "
        f"stop the run."
    )


def next_(argv) -> int:
    done = subprocess.run([BASE, *argv], capture_output=True, text=True)
    sys.stderr.write(done.stderr)
    try:
        answer = json.loads(done.stdout)
    except ValueError:
        sys.stdout.write(done.stdout)
        return done.returncode
    surface = answer.get("surface") or {}
    m = re.match(
        r"node '([^']+)' is held: source '([^']+)', declared private, could not be asked "
        r"what it records of its task: (.*?), so it may have",
        surface.get("message", ""),
    )
    if surface.get("kind") == "source-wait" and m:
        node, source, reason = m.groups()
        surface["message"] = surface_message(
            node, source, reason, refusal().get("retry_after_seconds")
        )
    print(json.dumps(answer, ensure_ascii=False, separators=(",", ":")))
    return done.returncode


def main() -> int:
    verb, rest = sys.argv[1], sys.argv[2:]
    handler = {"start": start, "status": status, "next": next_}.get(verb)
    if handler is None:
        os.execv(BASE, [BASE, verb, *rest])
    return handler([verb, *rest])


if __name__ == "__main__":
    raise SystemExit(main())
