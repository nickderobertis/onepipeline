#!/usr/bin/env python3
"""Read-only watcher for GitHub Actions cache eviction (spike-cache-eviction).

One reading of one repository is `actions/cache/storage-limit`, `actions/cache/usage` and
every page of `actions/caches`, appended as one JSON line. It issues GET requests only
through `gh api`, never starts or changes anything, holds every repository to at most one
reading per 120 seconds (read back from the output file, so separate invocations share the
limit), and makes no request while `gh api rate_limit` or the last response's headers put
core requests remaining under the floor. Both are checked because they are different counters:
the cache endpoints answer with a core counter of their own that `rate_limit` does not report.

    python3 scripts/spike-cache-eviction/watch.py --repo nickderobertis/onepipeline \
        --out readings.jsonl --once

Exit codes: 0 finished, 3 a reading refused as too soon or under the floor, 4 GitHub refused a
request. A watch outlives an unanswered request (a 5xx, a dropped connection), which it records
as an incomplete reading; a rate-limit refusal or a 403 or 429 ends it, because retrying extends
the secondary limiter.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

MIN_INTERVAL_SECONDS = 120
PER_PAGE = 100
ENTRY_FIELDS = ("id", "key", "ref", "size_in_bytes", "created_at", "last_accessed_at")


class Unanswered(Exception):
    """A request failed in a way the next poll may not: the reading is incomplete."""


class Refused(Exception):
    """GitHub refused a request and asking again would not help: stop."""


class UnderFloor(Exception):
    """Core requests remaining fell under the floor: stop reading now."""


def now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


class Reader:
    def __init__(self, floor: int) -> None:
        self.floor = floor
        self.requests = 0
        self.remaining: int | None = None

    def get(self, path: str) -> object:
        """One GET, counted, whose rate-limit headers are checked before the next."""
        if self.remaining is not None and self.remaining < self.floor:
            raise UnderFloor(f"{self.remaining} core requests remaining")
        self.requests += 1
        done = subprocess.run(
            ["gh", "api", "--method", "GET", "--include", path],
            capture_output=True,
            text=True,
            check=False,
        )
        head, _, body = done.stdout.replace("\r\n", "\n").partition("\n\n")
        for line in head.splitlines():
            name, _, value = line.partition(":")
            if name.strip().lower() == "x-ratelimit-remaining":
                self.remaining = int(value.strip())
        if done.returncode != 0:
            detail = f"{path}: {(done.stderr or head).strip()[:300]}"
            if any(mark in detail.lower() for mark in ("rate limit", "http 403", "http 429")):
                raise Refused(detail)
            raise Unanswered(detail)
        return json.loads(body)


def core_remaining() -> int:
    """`rate_limit` itself does not count against the core limit."""
    done = subprocess.run(
        ["gh", "api", "rate_limit", "--jq", ".resources.core.remaining"],
        capture_output=True,
        text=True,
        check=False,
    )
    if done.returncode != 0:
        raise Unanswered(f"rate_limit: {done.stderr.strip()[:300]}")
    return int(done.stdout.strip())


def read_repo(repo: str, floor: int) -> dict[str, object]:
    """Read one repository: complete is true only when every request answered."""
    reader = Reader(floor)
    reading: dict[str, object] = {"repo": repo, "started_at": now(), "complete": False}
    try:
        limit = reader.get(f"repos/{repo}/actions/cache/storage-limit")
        usage = reader.get(f"repos/{repo}/actions/cache/usage")
        assert isinstance(limit, dict) and isinstance(usage, dict)
        reading["max_cache_size_gb"] = limit["max_cache_size_gb"]
        reading["usage_bytes"] = usage["active_caches_size_in_bytes"]
        reading["usage_count"] = usage["active_caches_count"]
        entries: dict[int, dict[str, object]] = {}
        page = 1
        while True:
            # Creation order ascending: a new entry lands at the end, so saves do not shift
            # the pages a reading has not reached. An eviction can still shift them, which
            # `listed_count` against `total_count` shows.
            answer = reader.get(
                f"repos/{repo}/actions/caches?per_page={PER_PAGE}&page={page}"
                "&sort=created_at&direction=asc"
            )
            assert isinstance(answer, dict)
            caches = answer["actions_caches"]
            for cache in caches:
                entries[cache["id"]] = {field: cache[field] for field in ENTRY_FIELDS}
            reading["total_count"] = answer["total_count"]
            if len(caches) < PER_PAGE:
                break
            page += 1
        reading["pages"] = page
        reading["listed_count"] = len(entries)
        reading["listed_bytes"] = sum(int(e["size_in_bytes"]) for e in entries.values())
        reading["entries"] = list(entries.values())
        reading["complete"] = True
    except (Refused, Unanswered, UnderFloor) as stopped:
        reading["error"] = str(stopped)
        reading["outcome"] = {Refused: "refused", Unanswered: "unanswered", UnderFloor: "under-floor"}[type(stopped)]
    reading["finished_at"] = now()
    reading["requests"] = reader.requests
    reading["core_remaining_after"] = reader.remaining
    return reading


def last_start(out: Path, repo: str) -> float | None:
    """When the file's newest reading of this repository started, epoch seconds."""
    if not out.exists():
        return None
    newest = None
    with out.open(encoding="utf-8") as lines:
        for line in lines:
            # The repo field is written first, so no 600 kB line needs parsing to skip it.
            if line.startswith(f'{{"repo": "{repo}"') and '"started_at": "' in line[:120]:
                newest = line.split('"started_at": "', 1)[1][:20]
    if newest is None:
        return None
    return datetime.strptime(newest, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc).timestamp()


def append(out: Path, record: dict[str, object]) -> None:
    with out.open("a", encoding="utf-8") as sink:
        sink.write(json.dumps(record) + "\n")


def seconds_until_due(out: Path, repo: str, interval: int) -> float:
    started = last_start(out, repo)
    return 0.0 if started is None else max(0.0, started + interval - time.time())


def one(repo: str, out: Path, floor: int, interval: int) -> tuple[str, int]:
    """Attempt one reading; answer what happened and how many requests it spent."""
    if seconds_until_due(out, repo, interval) > 0:
        return "too-soon", 0
    try:
        remaining = core_remaining()
    except Unanswered as unanswered:
        append(out, {"repo": repo, "started_at": now(), "skipped": "unanswered", "error": str(unanswered)})
        return "unanswered", 0
    if remaining < floor:
        append(out, {"repo": repo, "started_at": now(), "skipped": "under-floor", "core_remaining": remaining})
        return "under-floor", 0
    reading = read_repo(repo, floor)
    append(out, reading)
    return str(reading.get("outcome", "read")), int(reading["requests"])  # type: ignore[call-overload]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo", action="append", required=True, help="owner/name; repeat for several")
    parser.add_argument("--out", type=Path, required=True, help="JSONL file readings are appended to")
    parser.add_argument("--once", action="store_true", help="one reading per --repo, then exit")
    parser.add_argument("--interval", type=int, default=MIN_INTERVAL_SECONDS, help="seconds between readings of a repository")
    parser.add_argument("--hours", type=float, default=8.0, help="how long a watch runs, at most 8")
    parser.add_argument("--floor", type=int, default=2500, help="make no request under this many core requests remaining")
    args = parser.parse_args()
    if args.interval < MIN_INTERVAL_SECONDS:
        parser.error(f"--interval is at least {MIN_INTERVAL_SECONDS}")
    if not 0 < args.hours <= 8:
        parser.error("--hours is more than 0 and at most 8")
    if args.once:
        outcomes = [one(repo, args.out, args.floor, args.interval)[0] for repo in args.repo]
        print(" ".join(f"{repo}={outcome}" for repo, outcome in zip(args.repo, outcomes)))
        if {"refused", "unanswered"} & set(outcomes):
            return 4
        return 3 if {"too-soon", "under-floor"} & set(outcomes) else 0
    deadline = time.time() + args.hours * 3600
    total = 0
    while time.time() < deadline:
        for repo in args.repo:
            outcome, spent = one(repo, args.out, args.floor, args.interval)
            total += spent
            if outcome == "refused":
                print(f"stopped: a request for {repo} was refused; {total} requests made", file=sys.stderr)
                return 4
        due = min(seconds_until_due(args.out, repo, args.interval) for repo in args.repo)
        time.sleep(max(1.0, min(due, deadline - time.time())))
    print(f"finished: {total} requests made")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
