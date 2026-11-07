#!/usr/bin/env python3
"""Read-only, on-disk-cached GitHub history for spike-cache-eviction's 30-day questions.

Every GET lands in a cache directory keyed by its path, so a re-analysis costs no request, and no
REST request is made once `gh api rate_limit` reports under the floor of 2,500 core requests
remaining (checked at the first request and after every 25). Pulls merged pull requests over
GraphQL, which has its own limit, and workflow runs, their jobs, job logs and the full file lists
of pull requests over 100 files over REST.

Imported by analyse.py, which holds the arithmetic and is what a person runs.
"""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from urllib.parse import quote

OWNER = "nickderobertis"
SINCE = datetime(2026, 10, 7, 17, 0, tzinfo=timezone.utc)


ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
LOG_LINE = re.compile(
    r"Cache Key:|Restoring cache|No cache found|Restored from cache|Cache restored|restored from key|Cache hit|"
    r"Cache not found|Cache Size:|Saving cache|Cache saved|Not saving|exact match|\(100\.0%\)|\(100%\)|Sent 0 of|"
    r"Failed to save|Unable to reserve|another job|##\[group\]Run|##\[error\]|Warning: |##\[warning\]"
)


class UnderFloor(Exception):
    pass


def ts(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


class Store:
    def __init__(self, cache: Path, floor: int = 2500) -> None:
        self.cache = cache
        self.floor = floor
        self.requests = 0
        self.since_check = 25
        cache.mkdir(parents=True, exist_ok=True)

    def _file(self, kind: str, key: str) -> Path:
        return self.cache / f"{kind}-{hashlib.sha1(key.encode()).hexdigest()}"

    def _guard(self) -> None:
        if self.since_check >= 25:
            self.since_check = 0
            done = subprocess.run(
                ["gh", "api", "rate_limit", "--jq", ".resources.core.remaining"],
                capture_output=True, text=True, check=True,
            )
            if int(done.stdout) < self.floor:
                raise UnderFloor(done.stdout.strip())

    def cached(self, path: str) -> bool:
        return self._file("get", path).exists()

    def _gh(self, *args: str) -> subprocess.CompletedProcess[str]:
        """One gh call, retried after a 5xx, every attempt written to the ledger."""
        self._guard()
        for attempt in range(3):
            self.requests += 1
            self.since_check += 1
            with (self.cache / "requests.ledger").open("a", encoding="utf-8") as ledger:
                ledger.write(" ".join(args) + "\n")
            done = subprocess.run(["gh", "api", *args], capture_output=True, text=True, check=False, errors="replace")
            if done.returncode == 0 or not re.search(r"HTTP 5\d\d", done.stderr):
                break
            time.sleep(15 * (attempt + 1))
        return done

    def forget(self, path: str) -> None:
        self._file("get", path).unlink(missing_ok=True)

    def get(self, path: str) -> object:
        file = self._file("get", path)
        if file.exists():
            return json.loads(file.read_text())
        done = self._gh("--method", "GET", path)
        if done.returncode != 0:
            if "HTTP 404" in done.stderr or "HTTP 410" in done.stderr:
                body: object = {"unavailable": done.stderr.strip()[:200]}
            else:
                raise RuntimeError(f"{path}: {done.stderr[:300]}")
        else:
            body = json.loads(done.stdout)
        file.write_text(json.dumps(body))
        return body

    def log(self, repo: str, job_id: int) -> str | None:
        file = self._file("log", f"{repo}/{job_id}")
        if file.exists():
            return file.read_text()
        done = self._gh("--allow-escape-sequences", "--method", "GET", f"repos/{OWNER}/{repo}/actions/jobs/{job_id}/logs")
        if done.returncode != 0:
            if "HTTP 404" in done.stderr or "HTTP 410" in done.stderr:
                file.write_text("")
                return ""
            raise RuntimeError(f"log {job_id}: {done.stderr[:300]}")
        # Only the lines any question reads, so the cache stays small.
        lines = [ANSI.sub("", line) for line in done.stdout.splitlines()]
        keep = []
        for number, line in enumerate(lines):
            if LOG_LINE.search(line) or (number and lines[number - 1].rstrip().endswith("Cache Key:")):
                keep.append(line)
        text = "\n".join(keep)
        file.write_text(text)
        return text

    def graphql(self, query: str, **variables: str) -> dict:
        key = query + json.dumps(variables, sort_keys=True)
        file = self._file("gql", key)
        if file.exists():
            return json.loads(file.read_text())
        args = ["gh", "api", "graphql", "-f", f"query={query}"]
        for name, value in variables.items():
            args += ["-f", f"{name}={value}"]
        done = subprocess.run(args, capture_output=True, text=True, check=True)
        body = json.loads(done.stdout)
        file.write_text(done.stdout)
        return body


PRS = """query($repo:String!,$owner:String!,$cursor:String){repository(owner:$owner,name:$repo){
pullRequests(states:MERGED,first:50,after:$cursor,orderBy:{field:UPDATED_AT,direction:DESC}){
pageInfo{hasNextPage endCursor}
nodes{number title createdAt mergedAt baseRefName headRefName baseRefOid headRefOid
mergeCommit{oid committedDate}
commits(last:1){nodes{commit{oid committedDate pushedDate}}}
files(first:100){totalCount nodes{path}}}}}}"""


class IncompleteFiles(Exception):
    """A pull request's complete file list could not be read."""


def pr_files(store: Store, repo: str, pr: dict) -> list[str]:
    """Every path a merged pull request changed, paged over REST past GraphQL's first 100.

    Refuses a list that does not match the pull request's totalCount, which REST's own
    3,000-file cap would otherwise pass off as complete.
    """
    total = pr["files"]["totalCount"]
    if total <= len(pr["files"]["nodes"]):
        return [node["path"] for node in pr["files"]["nodes"]]
    paths: dict[str, None] = {}
    page = 1
    while True:
        path = f"repos/{OWNER}/{repo}/pulls/{pr['number']}/files?per_page=100&page={page}"
        try:
            body = store.get(path)
        except (UnderFloor, RuntimeError) as failed:
            raise IncompleteFiles(f"{repo}#{pr['number']}: {failed}") from failed
        if not isinstance(body, list):
            raise IncompleteFiles(f"{repo}#{pr['number']}: page {page} answered {str(body)[:120]}")
        paths.update((entry["filename"], None) for entry in body)
        if len(body) < 100:
            break
        page += 1
    if len(paths) != total:
        raise IncompleteFiles(f"{repo}#{pr['number']}: read {len(paths)} of {total} files")
    return list(paths)


def merged_prs(store: Store, repo: str) -> list[dict]:
    """Merged into main since SINCE, oldest first."""
    prs: list[dict] = []
    cursor = ""
    while True:
        variables = {"repo": repo, "owner": OWNER}
        if cursor:
            variables["cursor"] = cursor
        body = store.graphql(PRS, **variables)
        block = body["data"]["repository"]["pullRequests"]
        nodes = block["nodes"]
        prs += [pr for pr in nodes if ts(pr["mergedAt"]) >= SINCE and pr["baseRefName"] == "main"]
        if not block["pageInfo"]["hasNextPage"] or all(ts(pr["mergedAt"]) < SINCE - timedelta(days=2) for pr in nodes):
            break
        cursor = block["pageInfo"]["endCursor"]
    return sorted(prs, key=lambda pr: pr["mergedAt"])


def workflow_runs(store: Store, repo: str, workflow: str, *, since: datetime = SINCE, fresh: bool = False) -> list[dict]:
    """A workflow's runs created since `since`, oldest first; fresh re-reads what the cache holds."""
    runs: list[dict] = []
    created = quote(f">={since:%Y-%m-%dT%H:%M:%SZ}")
    page = 1
    while True:
        path = f"repos/{OWNER}/{repo}/actions/workflows/{workflow}/runs?per_page=100&page={page}&created={created}"
        if fresh:
            store.forget(path)
        body = store.get(path)
        batch = body.get("workflow_runs", [])  # type: ignore[union-attr]
        runs += batch
        if len(batch) < 100:
            break
        page += 1
    return sorted({run["id"]: run for run in runs}.values(), key=lambda run: run["created_at"])


def jobs(store: Store, repo: str, run_id: int, attempt: int | None = None) -> list[dict]:
    path = f"repos/{OWNER}/{repo}/actions/runs/{run_id}/jobs?per_page=100&filter=latest"
    body = store.get(path)
    return body.get("jobs", [])  # type: ignore[union-attr]
