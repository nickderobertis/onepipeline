#!/usr/bin/env python3
"""Measure a repository's GitHub Actions CI/CD time from its run, job and PR records.

Throwaway spike harness (spike-onepipeline-ci). One command:

    python3 -I spike/onepipeline-ci/actions_spike.py \
        --repo nickderobertis/onepipeline --since 2026-09-15 \
        --raw <dir> --out <figures.json> --log <requests.jsonl>

Every GitHub read goes through `gh api` (GET only). Each response body is kept
under --raw, keyed by its request path, and a path already held there is never
requested again. `--offline` makes no request at all and computes from --raw
alone, which is how the committed fixture reproduces the report's figures.

Before every request the harness holds the core quota GitHub's `rate_limit`
endpoint last reported (re-read every --check-every requests, and that read is
itself logged and counted). It refuses a request once that figure, less the
requests made since it, is not above --floor, or once --cap requests have been
made across every invocation the log records.

Local git (tag containment and tag commits) is read too; each answer is kept
under --raw/git/ exactly like a response, so --offline needs no checkout.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
import subprocess
import sys
import urllib.parse
from pathlib import Path

WORKFLOWS = [
    "ci.yml",
    "release-sweep.yml",
    "release-plz.yml",
    "release.yml",
    "published-smoke.yml",
    "engine-currency.yml",
    "notignored.yml",
    "visual-docs.yml",
]


class QuotaStop(Exception):
    """The floor or the cap refused the next request."""


class Missing(Exception):
    """--offline and the raw store does not hold the response."""


def ts(value: str | None) -> dt.datetime | None:
    if not value:
        return None
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def secs(a: str | None, b: str | None) -> float | None:
    ta, tb = ts(a), ts(b)
    if ta is None or tb is None:
        return None
    return (tb - ta).total_seconds()


def stats(values: list[float]) -> dict | None:
    """Median and nearest-rank p90 and max."""
    vals = sorted(v for v in values if v is not None)
    if not vals:
        return None
    n = len(vals)
    median = vals[n // 2] if n % 2 else (vals[n // 2 - 1] + vals[n // 2]) / 2
    p90 = vals[max(0, math.ceil(0.9 * n) - 1)]
    return {"n": n, "median": median, "p90": p90, "max": vals[-1], "min": vals[0]}


def runner_os(job: dict) -> str:
    labels = " ".join(job.get("labels") or []).lower()
    name = (job.get("runner_name") or "").lower()
    if "windows" in labels or "windows" in name:
        return "Windows"
    if "macos" in labels or "mac" in name:
        return "macOS"
    if labels or name:
        return "Linux"
    return "unknown"


class Store:
    def __init__(self, repo: str, raw: Path, log: Path, offline: bool, seed: Path | None,
                 floor: int, cap: int, check_every: int, git_dir: Path | None) -> None:
        self.repo = repo
        self.raw = raw
        self.log_path = log
        self.offline = offline
        self.seed = seed
        self.floor = floor
        self.cap = cap
        self.check_every = check_every
        self.git_dir = git_dir
        self.last_remaining: int | None = None
        self.since_check = 0
        self.reads: list[str] = []
        self.made = 0
        self.prior = 0
        if log.exists():
            with log.open() as fh:
                self.prior = sum(1 for line in fh if line.strip() and json.loads(line).get("request"))
        raw.mkdir(parents=True, exist_ok=True)

    # -- the request log -------------------------------------------------
    def _log(self, entry: dict) -> None:
        self.log_path.parent.mkdir(parents=True, exist_ok=True)
        with self.log_path.open("a") as fh:
            fh.write(json.dumps(entry, sort_keys=True) + "\n")

    def _total(self) -> int:
        return self.prior + self.made

    def _gh(self, path: str) -> tuple[int, dict, str]:
        # A connection that never reached GitHub is retried, twice, after a pause;
        # each attempt is still logged and counted against the cap by the caller.
        for attempt in range(3):
            try:
                return self._gh_once(path)
            except ConnectionError as exc:
                self._log({"request": True, "seq": self._total() + 1, "at": dt.datetime.now(dt.UTC).isoformat(),
                           "path": path, "status": 0, "error": str(exc)[:200],
                           "core_remaining_last_reported": self.last_remaining,
                           "requests_since_report": self.since_check})
                self.made += 1
                self.since_check += 1
                if attempt == 2 or self._total() >= self.cap:
                    raise
                import time
                time.sleep(30)
        raise AssertionError("unreachable")

    def _gh_once(self, path: str) -> tuple[int, dict, str]:
        proc = subprocess.run(["gh", "api", "--method", "GET", "--include", path],
                              capture_output=True, text=True)
        out = proc.stdout
        head, _, body = out.partition("\r\n\r\n")
        if not body and "\n\n" in out:
            head, _, body = out.partition("\n\n")
        lines = head.splitlines()
        status = 0
        if lines and lines[0].startswith("HTTP"):
            try:
                status = int(lines[0].split()[1])
            except (IndexError, ValueError):
                status = 0
        headers = {}
        for line in lines[1:]:
            if ":" in line:
                k, v = line.split(":", 1)
                headers[k.strip().lower()] = v.strip()
        if proc.returncode != 0 and not status:
            raise ConnectionError(f"gh api {path} failed: {proc.stderr.strip()}")
        return status, headers, body

    def _check_quota(self) -> None:
        if self._total() >= self.cap:
            raise QuotaStop(f"cap of {self.cap} requests reached")
        status, headers, body = self._gh("rate_limit")
        self.made += 1
        core = json.loads(body)["resources"]["core"]
        self._log({"request": True, "seq": self._total(), "at": dt.datetime.now(dt.UTC).isoformat(),
                   "path": "rate_limit", "status": status,
                   "core_remaining_last_reported": self.last_remaining,
                   "requests_since_report": self.since_check,
                   "core_remaining_reported_now": core["remaining"], "core_reset": core["reset"]})
        self.last_remaining = core["remaining"]
        self.since_check = 0

    def get(self, path: str, text: bool = False) -> object:
        key = urllib.parse.quote(path, safe="")
        file = self.raw / f"{key}.{'txt' if text else 'json'}"
        if not file.exists() and self.seed is not None and (self.seed / f"{key}.json").exists():
            file.write_bytes((self.seed / f"{key}.json").read_bytes())
        if file.exists():
            self.reads.append(file.name)
            return file.read_text() if text else json.loads(file.read_text())
        if self.offline:
            raise Missing(path)
        if self.last_remaining is None or self.since_check >= self.check_every:
            self._check_quota()
        effective = self.last_remaining - self.since_check
        if effective <= self.floor:
            raise QuotaStop(f"core quota {self.last_remaining} less {self.since_check} since is at the floor {self.floor}")
        if self._total() >= self.cap:
            raise QuotaStop(f"cap of {self.cap} requests reached")
        entry = {"request": True, "seq": self._total() + 1, "at": dt.datetime.now(dt.UTC).isoformat(),
                 "path": path, "core_remaining_last_reported": self.last_remaining,
                 "requests_since_report": self.since_check}
        status, headers, body = self._gh(path)
        self.made += 1
        self.since_check += 1
        entry.update({"status": status, "x_ratelimit_remaining_after": headers.get("x-ratelimit-remaining"),
                      "x_ratelimit_resource": headers.get("x-ratelimit-resource")})
        self._log(entry)
        if status != 200:
            raise RuntimeError(f"GET {path} answered {status}: {body[:300]}")
        self.reads.append(file.name)
        if text:
            file.write_text(body)
            return body
        data = json.loads(body)
        file.write_text(json.dumps(data, sort_keys=True))
        return data

    def git(self, name: str, argv: list[str]) -> str:
        file = self.raw / "git" / f"{urllib.parse.quote(name, safe='')}.txt"
        if not file.exists() and self.seed is not None:
            seeded = self.seed / "git" / file.name
            if seeded.exists():
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_bytes(seeded.read_bytes())
        if file.exists():
            self.reads.append(f"git/{file.name}")
            return file.read_text()
        if self.offline or self.git_dir is None:
            raise Missing(f"git {name}")
        out = subprocess.run(["git", "-C", str(self.git_dir), *argv], capture_output=True, text=True, check=True).stdout
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(out)
        self.reads.append(f"git/{file.name}")
        return out


# -- reads ---------------------------------------------------------------

def paged(store: Store, path: str, key: str | None, stop=None) -> list:
    items: list = []
    page = 1
    while True:
        sep = "&" if "?" in path else "?"
        data = store.get(f"{path}{sep}per_page=100&page={page}")
        batch = data[key] if key else data
        items.extend(batch)
        if len(batch) < 100 or (stop and stop(batch)):
            return items
        if key and "total_count" in data and len(items) >= data["total_count"]:
            return items
        page += 1


def workflow_runs(store: Store, wf: str, since: str) -> list:
    created = urllib.parse.quote(f">={since}")
    return paged(store, f"repos/{store.repo}/actions/workflows/{wf}/runs?created={created}", "workflow_runs")


def head_runs(store: Store, sha: str) -> list:
    return paged(store, f"repos/{store.repo}/actions/runs?head_sha={sha}", "workflow_runs")


def run_jobs(store: Store, run: dict) -> list:
    if run.get("conclusion") == "skipped":
        return []
    return paged(store, f"repos/{store.repo}/actions/runs/{run['id']}/jobs?filter=latest", "jobs")


def wf_file(run: dict) -> str:
    return (run.get("path") or "").split("/")[-1].split("@")[0]


def job_record(run: dict, job: dict) -> dict:
    return {
        "workflow": wf_file(run), "run_id": run["id"], "run_attempt": run.get("run_attempt"),
        "event": run.get("event"), "job_id": job["id"], "name": job["name"],
        "os": runner_os(job), "labels": job.get("labels"), "conclusion": job.get("conclusion"),
        "status": job.get("status"),
        "duration_s": secs(job.get("started_at"), job.get("completed_at")),
        "queue_from_run_created_s": secs(run.get("created_at"), job.get("started_at")),
        "queue_from_job_created_s": secs(job.get("created_at"), job.get("started_at")),
        "started_at": job.get("started_at"), "completed_at": job.get("completed_at"),
        "steps": [{"name": s["name"], "conclusion": s.get("conclusion"),
                   "duration_s": secs(s.get("started_at"), s.get("completed_at"))}
                  for s in job.get("steps") or [] if s.get("conclusion") not in (None, "skipped")],
    }


def executed(job: dict) -> bool:
    return job.get("conclusion") not in ("skipped", None) and bool(job.get("started_at"))


def run_summary(run: dict, jobs: list) -> dict:
    ran = [j for j in jobs if executed(j)]
    by_os: dict[str, int] = {}
    for j in ran:
        by_os[runner_os(j)] = by_os.get(runner_os(j), 0) + 1
    ends = [j["completed_at"] for j in ran if j.get("completed_at")]
    longest = max(ran, key=lambda j: secs(j["started_at"], j["completed_at"]) or 0, default=None)
    return {
        "run_id": run["id"], "workflow": wf_file(run), "event": run.get("event"),
        "head_sha": run.get("head_sha"), "head_branch": run.get("head_branch"),
        "created_at": run.get("created_at"), "run_attempt": run.get("run_attempt"),
        "conclusion": run.get("conclusion"), "jobs_executed": len(ran), "jobs_by_os": by_os,
        "last_job_completed_at": max(ends) if ends else None,
        "wall_s": secs(run.get("created_at"), max(ends)) if ends else None,
        "longest_job": ({"name": longest["name"], "duration_s": secs(longest["started_at"], longest["completed_at"])}
                        if longest else None),
        "jobs": [job_record(run, j) for j in ran],
    }


# -- releases and PRs ---------------------------------------------------

def releases(store: Store) -> list:
    rels = paged(store, f"repos/{store.repo}/releases", None)
    return [r for r in rels if (r.get("tag_name") or "").startswith("v") and not r.get("draft")]


def tag_commit(store: Store, tag: str) -> str:
    return store.git(f"rev-parse {tag}", ["rev-parse", f"{tag}^{{commit}}"]).strip()


def first_release_containing(store: Store, sha: str, rels: list) -> dict | None:
    out = store.git(f"tags-containing {sha}", ["for-each-ref", "--contains", sha, "--format=%(refname:short)", "refs/tags"])
    tags = set(out.split())
    held = [r for r in rels if r["tag_name"] in tags]
    return min(held, key=lambda r: r.get("published_at") or r.get("created_at"), default=None)


def cd_runs(store: Store, rel: dict) -> dict:
    """Every run the release-PR merge push, the release event and a workflow_run follow-up start."""
    sha = tag_commit(store, rel["tag_name"])
    runs = [r for r in head_runs(store, sha) if r.get("event") in ("push", "release", "workflow_run")]
    release_runs = [r for r in runs if r.get("event") == "release"]
    follow = []
    if release_runs:
        first = min(release_runs, key=lambda r: r["created_at"])
        start = ts(first["created_at"])
        end = start + dt.timedelta(hours=12)
        window = urllib.parse.quote(f"{start.strftime('%Y-%m-%dT%H:%M:%SZ')}..{end.strftime('%Y-%m-%dT%H:%M:%SZ')}")
        smoke = paged(store, f"repos/{store.repo}/actions/workflows/published-smoke.yml/runs?event=workflow_run&created={window}",
                      "workflow_runs")
        held = {r["id"] for r in runs}
        # The first follow-up created after this release's Release run started is the one it triggered.
        smoke = sorted((r for r in smoke if r["id"] not in held), key=lambda r: r["created_at"])
        follow = smoke[:1]
    summaries = []
    for run in sorted(runs + follow, key=lambda r: r["created_at"]):
        summaries.append(run_summary(run, run_jobs(store, run)))
    ran = [j for s in summaries for j in s["jobs"]]
    ends = [j["completed_at"] for j in ran if j["completed_at"]]
    longest = max(ran, key=lambda j: j["duration_s"] or 0, default=None)
    by_wf: dict[str, int] = {}
    by_os: dict[str, int] = {}
    for j in ran:
        by_wf[j["workflow"]] = by_wf.get(j["workflow"], 0) + 1
        by_os[j["os"]] = by_os.get(j["os"], 0) + 1
    return {
        "tag": rel["tag_name"], "tag_commit": sha, "published_at": rel.get("published_at"),
        "runs": summaries, "jobs_executed": len(ran), "jobs_by_workflow": by_wf, "jobs_by_os": by_os,
        "last_job_completed_at": max(ends) if ends else None,
        "longest_job": ({"workflow": longest["workflow"], "name": longest["name"], "duration_s": longest["duration_s"]}
                        if longest else None),
        "has_release_run": bool(release_runs), "has_follow_up": bool(follow),
    }


def pr_figures(store: Store, number: int, rels: list, cd_cache: dict) -> dict:
    pr = store.get(f"repos/{store.repo}/pulls/{number}")
    head = pr["head"]["sha"]
    merged_at = pr.get("merged_at")
    runs = [r for r in head_runs(store, head)
            if r.get("event") == "pull_request" and (not merged_at or r["created_at"] <= merged_at)]
    summaries = [run_summary(r, run_jobs(store, r)) for r in sorted(runs, key=lambda r: r["created_at"])]
    ran = [j for s in summaries for j in s["jobs"]]
    first = min((r["created_at"] for r in runs), default=None)
    last = max((j["completed_at"] for j in ran if j["completed_at"]), default=None)
    by_wf: dict[str, int] = {}
    by_os: dict[str, int] = {}
    for j in ran:
        by_wf[j["workflow"]] = by_wf.get(j["workflow"], 0) + 1
        by_os[j["os"]] = by_os.get(j["os"], 0) + 1
    ci = [s for s in summaries if s["workflow"] == "ci.yml"]
    ci_jobs = [j for s in ci for j in s["jobs"]]
    ci_longest = max(ci_jobs, key=lambda j: j["duration_s"] or 0, default=None)
    out = {
        "number": number, "title": pr.get("title"), "head_ref": pr["head"]["ref"],
        "release_pr": pr["head"]["ref"].startswith("release-plz-"),
        "head_sha": head, "merged_at": merged_at, "merge_commit_sha": pr.get("merge_commit_sha"),
        "head_runs": summaries, "jobs_executed": len(ran), "jobs_by_workflow": by_wf, "jobs_by_os": by_os,
        "ci_jobs_executed": len(ci_jobs),
        "ci_longest_job": ({"name": ci_longest["name"], "duration_s": ci_longest["duration_s"]} if ci_longest else None),
        "checks_first_run_created_at": first, "checks_last_job_completed_at": last,
        "checks_phase_s": secs(first, last),
        "release": None, "merge_to_release_s": None, "cycle_s": None,
    }
    if merged_at and pr.get("merge_commit_sha"):
        rel = first_release_containing(store, pr["merge_commit_sha"], rels)
        if rel:
            if rel["tag_name"] not in cd_cache:
                cd_cache[rel["tag_name"]] = cd_runs(store, rel)
            cd = cd_cache[rel["tag_name"]]
            out["release"] = rel["tag_name"]
            if cd["last_job_completed_at"] and cd["has_release_run"]:
                out["merge_to_release_s"] = secs(merged_at, cd["last_job_completed_at"])
                if out["checks_phase_s"] is not None:
                    out["cycle_s"] = out["checks_phase_s"] + out["merge_to_release_s"]
    return out


# -- aggregation --------------------------------------------------------

def job_stats(records: list[dict]) -> dict:
    groups: dict[tuple, list] = {}
    for rec in records:
        groups.setdefault((rec["workflow"], rec["name"]), []).append(rec)
    out = {}
    for (wf, name), recs in sorted(groups.items()):
        ok = [r for r in recs if r["conclusion"] == "success"]
        mix: dict[str, int] = {}
        for r in recs:
            mix[r["conclusion"]] = mix.get(r["conclusion"], 0) + 1
        out[f"{wf} :: {name}"] = {
            "workflow": wf, "job": name, "os": sorted({r["os"] for r in recs}),
            "conclusions": mix, "failed": mix.get("failure", 0) + mix.get("timed_out", 0),
            "cancelled": mix.get("cancelled", 0),
            "duration_success_s": stats([r["duration_s"] for r in ok]),
            "queue_from_run_created_success_s": stats([r["queue_from_run_created_s"] for r in ok]),
            "queue_from_job_created_success_s": stats([r["queue_from_job_created_s"] for r in ok]),
            "events": sorted({r["event"] for r in recs}),
        }
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--repo", required=True)
    ap.add_argument("--since", required=True, help="YYYY-MM-DD; runs created and PRs merged on or after it")
    ap.add_argument("--raw", required=True, type=Path, help="raw response store, read before any request")
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--log", type=Path, help="request log (JSONL); default <raw>/requests.jsonl")
    ap.add_argument("--offline", action="store_true", help="make no request; compute from --raw alone")
    ap.add_argument("--seed", type=Path, help="copy a response from this store before requesting it")
    ap.add_argument("--git-dir", type=Path, default=Path("."), help="checkout whose tags answer containment")
    ap.add_argument("--pr", type=int, action="append", default=[], help="only these PRs (skips enumeration)")
    ap.add_argument("--run", type=int, action="append", default=[], help="only these runs (skips enumeration)")
    ap.add_argument("--jobs-for", default="ci.yml,release-sweep.yml,release-plz.yml,release.yml,published-smoke.yml,engine-currency.yml,notignored.yml,visual-docs.yml",
                    help="workflows whose every in-window run has its jobs read for the per-job figures")
    ap.add_argument("--newest-first", action="store_true", help="read PRs and releases newest first")
    ap.add_argument("--shape-since", help="also summarize PRs merged and releases published on or after this instant")
    ap.add_argument("--required-checks-out", type=Path,
                    help="also read the default branch's required status checks and write them here, dated")
    ap.add_argument("--branch", default="main")
    ap.add_argument("--extra", action="append", default=[], help="also GET this path into --raw (JSON)")
    ap.add_argument("--job-log", type=int, action="append", default=[], help="also GET this job's log into --raw")
    ap.add_argument("--merge-push-for", default="",
                    help="workflows whose push run on each feature PR's merge commit is read too (a repeat of its checks)")
    ap.add_argument("--floor", type=int, default=2500)
    ap.add_argument("--cap", type=int, default=3000)
    ap.add_argument("--check-every", type=int, default=10)
    args = ap.parse_args()

    store = Store(args.repo, args.raw, args.log or args.raw / "requests.jsonl", args.offline, args.seed,
                  args.floor, args.cap, args.check_every, None if args.offline else args.git_dir)
    focused = bool(args.pr or args.run)
    result: dict = {"repo": args.repo, "since": args.since, "focused": focused,
                    "generated_by": " ".join(["actions_spike.py", *sys.argv[1:]]), "missing": [], "stopped": None}
    records: dict[int, dict] = {}
    per_run: dict[str, dict] = {}

    def add_run(summary: dict) -> None:
        per_run[str(summary["run_id"])] = {k: v for k, v in summary.items()}
        for j in summary["jobs"]:
            records[j["job_id"]] = j

    per_pr: dict[str, dict] = {}
    cd_cache: dict = {}
    try:
        if args.required_checks_out:
            checks = store.get(f"repos/{store.repo}/branches/{args.branch}/protection/required_status_checks")
            # GitHub's own response, with the instant it was read and where from beside it.
            snap = dict(checks)
            snap["taken_at"] = next((json.loads(l)["at"] for l in reversed(store.log_path.read_text().splitlines())
                                     if "required_status_checks" in l), None)
            snap["taken_from"] = f"GET /repos/{store.repo}/branches/{args.branch}/protection/required_status_checks"
            args.required_checks_out.write_text(json.dumps(snap, indent=1, sort_keys=True) + "\n")
        for path in args.extra:
            store.get(path)
        for jid in args.job_log:
            store.get(f"repos/{store.repo}/actions/jobs/{jid}/logs", text=True)
        rels = releases(store)
        in_window_rels = [r for r in rels if (r.get("published_at") or "") >= args.since]
        runs_by_wf: dict[str, list] = {}
        prs: list[int] = list(args.pr)
        if not focused:
            for wf in WORKFLOWS:
                runs_by_wf[wf] = workflow_runs(store, wf, args.since)
            closed = paged(store, f"repos/{store.repo}/pulls?state=closed&sort=updated&direction=desc", None,
                           stop=lambda b: (b[-1].get("updated_at") or "") < args.since)
            prs = sorted((p["number"] for p in closed if (p.get("merged_at") or "") >= args.since),
                         reverse=args.newest_first)
            result["prs_in_window"] = len(prs)
            result["releases_in_window"] = len(in_window_rels)
        result["run_counts"] = {wf: {"runs": len(rs), "first_created_at": min((r["created_at"] for r in rs), default=None),
                                     "by_event": _count(rs, "event"),
                                     "by_conclusion": _count(rs, "conclusion")} for wf, rs in runs_by_wf.items()}
        for number in prs:
            try:
                fig = pr_figures(store, number, rels, cd_cache)
            except Missing as exc:
                result["missing"].append(f"PR #{number}: {exc}")
                continue
            per_pr[str(number)] = fig
            for s in fig["head_runs"]:
                add_run(s)
        if not focused:
            for rel in sorted(in_window_rels, key=lambda r: r["published_at"], reverse=args.newest_first):
                if rel["tag_name"] not in cd_cache:
                    try:
                        cd_cache[rel["tag_name"]] = cd_runs(store, rel)
                    except Missing as exc:
                        result["missing"].append(f"release {rel['tag_name']}: {exc}")
        for cd in cd_cache.values():
            for s in cd["runs"]:
                add_run(s)
        merge_wfs = [w for w in args.merge_push_for.split(",") if w]
        for fig in per_pr.values():
            if fig["release_pr"] or not fig["merge_commit_sha"] or not merge_wfs:
                continue
            pushes = [r for wf in merge_wfs for r in runs_by_wf.get(wf, [])
                      if r.get("event") == "push" and r.get("head_sha") == fig["merge_commit_sha"]
                      and r.get("status") == "completed"]
            fig["merge_push_runs"] = []
            for run in sorted(pushes, key=lambda r: r["created_at"]):
                summary = run_summary(run, run_jobs(store, run))
                add_run(summary)
                fig["merge_push_runs"].append({k: v for k, v in summary.items() if k != "jobs"})
        wanted = [w for w in args.jobs_for.split(",") if w]
        for wf in wanted:
            for run in runs_by_wf.get(wf, []):
                if str(run["id"]) in per_run or run.get("status") != "completed":
                    continue
                add_run(run_summary(run, run_jobs(store, run)))
        for rid in args.run:
            if str(rid) in per_run:
                continue
            run = store.get(f"repos/{store.repo}/actions/runs/{rid}")
            add_run(run_summary(run, run_jobs(store, run)))
    except QuotaStop as exc:
        result["stopped"] = str(exc)

    result["per_run"] = per_run
    result["per_job"] = job_stats(list(records.values()))
    result["per_pr"] = per_pr
    result["per_release"] = {tag: {k: v for k, v in cd.items() if k != "runs"} | {"run_ids": [s["run_id"] for s in cd["runs"]]}
                             for tag, cd in sorted(cd_cache.items())}
    # Release-level cycle: the maximum over the PRs one release carries.
    for tag, rel in result["per_release"].items():
        carried = [p for p in per_pr.values() if p["release"] == tag and not p["release_pr"]]
        cycles = [p["cycle_s"] for p in carried if p["cycle_s"] is not None]
        rel["prs_carried"] = sorted(p["number"] for p in carried)
        rel["release_cycle_s"] = max(cycles) if cycles else None
    result["summary"] = summarize(result["per_pr"], result["per_release"])
    if args.shape_since:
        result["summary_shape"] = {"since": args.shape_since} | summarize(
            {k: p for k, p in result["per_pr"].items() if (p["merged_at"] or "") >= args.shape_since},
            {k: r for k, r in result["per_release"].items() if (r["published_at"] or "") >= args.shape_since})
    result["requests"] = {"this_invocation": store.made, "all_invocations": store._total(),
                          "raw_files_read": len(set(store.reads))}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=1, sort_keys=True) + "\n")
    print(json.dumps({"out": str(args.out), "stopped": result["stopped"], "missing": len(result["missing"]),
                      "requests": result["requests"]}))
    return 0


def _count(items: list, key: str) -> dict:
    out: dict = {}
    for it in items:
        out[str(it.get(key))] = out.get(str(it.get(key)), 0) + 1
    return out


def summarize(per_pr: dict, per_release: dict) -> dict:
    prs = [p for p in per_pr.values() if p["merged_at"]]
    feature = [p for p in prs if not p["release_pr"]]
    rels = [r for r in per_release.values() if r["has_release_run"]]

    def ci_runs(group):
        return [s for p in group for s in p["head_runs"] if s["workflow"] == "ci.yml"]

    def by(group, field):
        keys = sorted({k for p in group for k in p[field]})
        return {k: stats([p[field].get(k, 0) for p in group]) for k in keys}

    return {
        "prs_merged": len(prs), "feature_prs": len(feature), "release_prs": len(prs) - len(feature),
        "prs_with_cycle": sum(1 for p in feature if p["cycle_s"] is not None),
        "checks_phase_s": stats([p["checks_phase_s"] for p in feature]),
        "merge_to_release_s": stats([p["merge_to_release_s"] for p in feature]),
        "cycle_s": stats([p["cycle_s"] for p in feature]),
        "release_pr_checks_phase_s": stats([p["checks_phase_s"] for p in prs if p["release_pr"]]),
        "ci_check_time_s": stats([s["longest_job"]["duration_s"] for s in ci_runs(feature) if s["longest_job"]]),
        "ci_job_count_ci_yml": stats([s["jobs_executed"] for s in ci_runs(feature)]),
        "pr_head_job_count_all_workflows": stats([p["jobs_executed"] for p in feature]),
        "pr_head_jobs_by_workflow": by(feature, "jobs_by_workflow"),
        "pr_head_jobs_by_os": by(feature, "jobs_by_os"),
        "release_pr_head_jobs_by_workflow": by([p for p in prs if p["release_pr"]], "jobs_by_workflow"),
        "cd_job_time_s": stats([r["longest_job"]["duration_s"] for r in rels if r["longest_job"]]),
        "cd_job_count": stats([r["jobs_executed"] for r in rels]),
        "cd_jobs_by_workflow": by(rels, "jobs_by_workflow"),
        "cd_jobs_by_os": by(rels, "jobs_by_os"),
        "release_cycle_s": stats([r["release_cycle_s"] for r in rels if r.get("release_cycle_s") is not None]),
        "releases_complete": len(rels),
    }


if __name__ == "__main__":
    sys.exit(main())
