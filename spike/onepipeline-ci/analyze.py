#!/usr/bin/env python3
"""Derive the report's repeat, serialization and timeline figures from actions_spike.py's output.

    python3 -I spike/onepipeline-ci/analyze.py <figures.json> [--since <instant>] > derived.json

Reads nothing but that document. `--since` restricts every figure to PRs merged
and releases published on or after the instant (the current workflow shape).
Every number is a median / p90 / max over the PRs or releases it names, from
measured job durations; nothing here is a projection.
"""

from __future__ import annotations

import argparse
import datetime as dt
import gzip
import json
import math
import sys


def ts(v):
    return dt.datetime.fromisoformat(v.replace("Z", "+00:00")) if v else None


def secs(a, b):
    return (ts(b) - ts(a)).total_seconds() if a and b else None


def stats(values):
    vals = sorted(v for v in values if v is not None)
    if not vals:
        return None
    n = len(vals)
    median = vals[n // 2] if n % 2 else (vals[n // 2 - 1] + vals[n // 2]) / 2
    return {"n": n, "median": median, "p90": vals[max(0, math.ceil(0.9 * n) - 1)], "max": vals[-1]}


# The families of checks that run more than once per change, by job name.
FAMILIES = {
    "just check (gate / release-sweep / release test)": lambda wf, n: (wf, n) in {
        ("ci.yml", "gate"), ("release-sweep.yml", "release-sweep"), ("release.yml", "test")},
    "cross legs (check-cross)": lambda wf, n: wf == "ci.yml" and n.startswith("cross ("),
    "smoke-published.sh legs": lambda wf, n: (
        (wf == "ci.yml" and (n.startswith("install") or n.startswith("wheel")))
        or (wf == "release.yml" and n.startswith("verify-"))
        or (wf == "published-smoke.yml" and (n.startswith("pypi (") or n.startswith("npm (")))),
    "release-mode builds of the 5 targets": lambda wf, n: wf == "release.yml" and n.split(" (")[0] in {
        "upload", "build-npm", "build-wheels"},
    "visual-docs capture": lambda wf, n: wf == "visual-docs.yml" and n.startswith("visual-docs / capture ("),
    "msrv / deny / release-compat / smoke": lambda wf, n: wf == "ci.yml" and n in {
        "msrv", "deny", "release-compat", "smoke"},
}


def family_seconds(jobs):
    out = {k: 0.0 for k in FAMILIES}
    count = {k: 0 for k in FAMILIES}
    for j in jobs:
        for k, pred in FAMILIES.items():
            if pred(j["workflow"], j["name"]) and j["duration_s"] is not None:
                out[k] += j["duration_s"]
                count[k] += 1
    return out, count


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("figures", help="actions_spike.py output, plain or .gz")
    ap.add_argument("--since", default="")
    args = ap.parse_args()
    d = json.load(gzip.open(args.figures) if args.figures.endswith(".gz") else open(args.figures))
    runs = d["per_run"]
    prs = [p for p in d["per_pr"].values() if p["merged_at"] and p["merged_at"] >= args.since]
    feature = [p for p in prs if not p["release_pr"]]
    by_merge = {p["merge_commit_sha"]: p for p in d["per_pr"].values() if p["release_pr"]}
    rels = [r for r in d["per_release"].values()
            if r["has_release_run"] and (r["published_at"] or "") >= args.since]

    def jobs_of(run_ids):
        return [j for rid in run_ids for j in runs[str(rid)]["jobs"]]

    # -- per feature change: what its head commit and its merge push ran ----
    change = []
    for p in feature:
        head = [j for s in p["head_runs"] for j in s["jobs"]]
        push = jobs_of([r["run_id"] for r in p.get("merge_push_runs", [])])
        fh, ch = family_seconds(head)
        fp, cp = family_seconds(push)
        ci = [s for s in p["head_runs"] if s["workflow"] == "ci.yml"]
        mac = lambda js: sum(j["duration_s"] or 0 for j in js if j["os"] == "macOS")
        change.append({"pr": p["number"], "head_macos_s": mac(head), "merge_push_macos_s": mac(push), "head": fh, "head_count": ch, "merge_push": fp, "merge_push_count": cp,
                       "head_job_seconds": sum(j["duration_s"] or 0 for j in head),
                       "merge_push_job_seconds": sum(j["duration_s"] or 0 for j in push),
                       "ci_wall_s": max((s["wall_s"] or 0 for s in ci), default=None)})

    # -- per release: the release PR's checks and the CD runs ----------------
    release = []
    for r in rels:
        rp = by_merge.get(r["tag_commit"])
        rp_jobs = [j for s in rp["head_runs"] for j in s["jobs"]] if rp else []
        cd = [runs[str(rid)] for rid in r["run_ids"]]
        cd_jobs = [j for s in cd for j in s["jobs"]]
        fr, cr = family_seconds(rp_jobs)
        fc, cc = family_seconds(cd_jobs)
        start = rp["merged_at"] if rp else None
        phases = {}
        for s in cd:
            key = f"{s['workflow']}:{s['event']}"
            phases[key] = {"created_after_merge_s": secs(start, s["created_at"]), "wall_s": s["wall_s"],
                           "longest": s["longest_job"]}
        rel_run = next((s for s in cd if s["workflow"] == "release.yml"), None)
        serial = None
        if rel_run:
            jobs = {j["name"]: j for j in rel_run["jobs"]}
            test = jobs.get("test")
            builds = [j for j in rel_run["jobs"] if j["name"].split(" (")[0] in {"upload", "build-npm", "build-wheels"}]
            if test and builds:
                serial = {"test_s": test["duration_s"],
                          "first_build_started_after_test_end_s": min(secs(test["completed_at"], b["started_at"]) for b in builds),
                          "longest_build_s": max(b["duration_s"] for b in builds),
                          "release_run_wall_s": rel_run["wall_s"]}
        mac = lambda js: sum(j["duration_s"] or 0 for j in js if j["os"] == "macOS")
        release.append({"tag": r["tag"], "release_pr_macos_s": mac(rp_jobs), "cd_macos_s": mac(cd_jobs),
                        "release_pr_number": rp["number"] if rp else None,
                        "release_pr_checks_s": rp["checks_phase_s"] if rp else None,
                        "release_pr": fr, "release_pr_count": cr, "cd": fc, "cd_count": cc,
                        "release_pr_job_seconds": sum(j["duration_s"] or 0 for j in rp_jobs),
                        "cd_job_seconds": sum(j["duration_s"] or 0 for j in cd_jobs),
                        "merge_to_last_cd_job_s": secs(start, r["last_job_completed_at"]),
                        "phases": phases, "release_yml_serialization": serial,
                        "prs_carried": len(r.get("prs_carried", []))})

    def fam_stats(rows, key):
        return {k: stats([row[key][k] for row in rows]) for k in FAMILIES}

    # -- needs edges that wait without reading the needed job's output -------
    install_wait = []
    for p in feature:
        for s in p["head_runs"]:
            if s["workflow"] != "ci.yml":
                continue
            jobs = {j["name"]: j for j in s["jobs"]}
            gate = jobs.get("gate")
            for name, j in jobs.items():
                if name.startswith("install (") and gate and j["started_at"]:
                    install_wait.append({"leg": name, "started_after_gate_end_s": secs(gate["completed_at"], j["started_at"]),
                                         "waited_from_run_created_s": j["queue_from_run_created_s"],
                                         "gate_s": gate["duration_s"]})
    phase_keys = sorted({k for r in release for k in r["phases"]})
    # Merge-to-release, split where the release PR merges.
    rel_pr_by_tag = {}
    for rp in d["per_pr"].values():
        if rp["release_pr"] and rp["release"]:
            rel_pr_by_tag.setdefault(rp["release"], rp)
    split = []
    for p in feature:
        rp = rel_pr_by_tag.get(p["release"])
        r = d["per_release"].get(p["release"]) if p["release"] else None
        if rp and r and p["merge_to_release_s"] is not None:
            split.append({"to_release_pr_merge_s": secs(p["merged_at"], rp["merged_at"]),
                          "release_pr_merge_to_last_cd_s": secs(rp["merged_at"], r["last_job_completed_at"])})
    # Job-seconds per release, by workflow.
    cd_by_wf = {}
    for r in rels:
        sums = {}
        for rid in r["run_ids"]:
            for j in runs[str(rid)]["jobs"]:
                sums[j["workflow"]] = sums.get(j["workflow"], 0) + (j["duration_s"] or 0)
        for wf, v in sums.items():
            cd_by_wf.setdefault(wf, []).append(v)
    # Which job of a PR head-commit ci.yml run completed last, and by how much
    # an install leg (held behind `gate`) outlasted every other job.
    last_job, install_overrun = {}, []
    for p in feature:
        for s in p["head_runs"]:
            if s["workflow"] != "ci.yml" or not s["jobs"]:
                continue
            end = max(s["jobs"], key=lambda j: j["completed_at"] or "")
            name = end["name"].split(" (")[0] if end["name"].startswith(("install (", "install-documented (")) else end["name"]
            last_job[end["name"]] = last_job.get(end["name"], 0) + 1
            inst = [j for j in s["jobs"] if j["name"].startswith("install (")]
            rest = [j for j in s["jobs"] if not j["name"].startswith("install (")]
            if inst and rest:
                install_overrun.append(max(0.0, secs(max(j["completed_at"] for j in rest), max(j["completed_at"] for j in inst))))
    # Per job, over the runs these PRs and releases started: head commits, merge
    # pushes, release-PR head commits and CD runs.
    run_ids = {s["run_id"] for p in prs for s in p["head_runs"]}
    run_ids |= {r["run_id"] for p in feature for r in p.get("merge_push_runs", [])}
    run_ids |= {rid for r in rels for rid in r["run_ids"]}
    groups = {}
    for rid in run_ids:
        run = runs[str(rid)]
        for j in run["jobs"]:
            groups.setdefault(f"{j['workflow']} :: {j['name']}", []).append(j)
    per_job = {}
    for key, jobs in sorted(groups.items()):
        ok = [j for j in jobs if j["conclusion"] == "success"]
        per_job[key] = {"runs": len(jobs), "success": len(ok),
                        "failed": sum(j["conclusion"] in ("failure", "timed_out") for j in jobs),
                        "cancelled": sum(j["conclusion"] == "cancelled" for j in jobs),
                        "duration_success_s": stats([j["duration_s"] for j in ok]),
                        "queue_from_run_created_success_s": stats([j["queue_from_run_created_s"] for j in ok]),
                        "queue_from_job_created_success_s": stats([j["queue_from_job_created_s"] for j in ok])}
    out = {
        "since": args.since or d["since"],
        "feature_changes": len(change), "releases": len(release),
        "per_change": {
            "head_family_seconds": fam_stats(change, "head"),
            "merge_push_family_seconds": fam_stats(change, "merge_push"),
            "head_job_seconds": stats([c["head_job_seconds"] for c in change]),
            "merge_push_job_seconds": stats([c["merge_push_job_seconds"] for c in change]),
            "ci_head_run_wall_s": stats([c["ci_wall_s"] for c in change]),
            "head_macos_job_seconds": stats([c["head_macos_s"] for c in change]),
            "merge_push_macos_job_seconds": stats([c["merge_push_macos_s"] for c in change]),
        },
        "per_release": {
            "release_pr_family_seconds": fam_stats(release, "release_pr"),
            "cd_family_seconds": fam_stats(release, "cd"),
            "release_pr_job_seconds": stats([r["release_pr_job_seconds"] for r in release]),
            "cd_job_seconds": stats([r["cd_job_seconds"] for r in release]),
            "release_pr_macos_job_seconds": stats([r["release_pr_macos_s"] for r in release]),
            "cd_macos_job_seconds": stats([r["cd_macos_s"] for r in release]),
            "release_pr_checks_s": stats([r["release_pr_checks_s"] for r in release]),
            "merge_to_last_cd_job_s": stats([r["merge_to_last_cd_job_s"] for r in release]),
            "prs_carried": stats([r["prs_carried"] for r in release]),
            "phase_created_after_merge_s": {k: stats([r["phases"].get(k, {}).get("created_after_merge_s") for r in release]) for k in phase_keys},
            "phase_wall_s": {k: stats([r["phases"].get(k, {}).get("wall_s") for r in release]) for k in phase_keys},
            "release_yml_test_s": stats([r["release_yml_serialization"]["test_s"] for r in release if r["release_yml_serialization"]]),
            "release_yml_first_build_after_test_end_s": stats([r["release_yml_serialization"]["first_build_started_after_test_end_s"] for r in release if r["release_yml_serialization"]]),
            "release_yml_longest_build_s": stats([r["release_yml_serialization"]["longest_build_s"] for r in release if r["release_yml_serialization"]]),
            "release_yml_wall_s": stats([r["release_yml_serialization"]["release_run_wall_s"] for r in release if r["release_yml_serialization"]]),
            "last_cd_run": stats([]),
        },
        "ci_install_waits_on_gate": {
            leg: {"started_after_gate_end_s": stats([w["started_after_gate_end_s"] for w in install_wait if w["leg"] == leg]),
                  "waited_from_run_created_s": stats([w["waited_from_run_created_s"] for w in install_wait if w["leg"] == leg])}
            for leg in sorted({w["leg"] for w in install_wait})},
        "per_job": per_job,
        "merge_to_release_split_s": {
            "feature_merge_to_release_pr_merge": stats([x["to_release_pr_merge_s"] for x in split]),
            "release_pr_merge_to_last_cd_job": stats([x["release_pr_merge_to_last_cd_s"] for x in split])},
        "cd_job_seconds_by_workflow": {wf: stats(v) for wf, v in sorted(cd_by_wf.items())},
        "ci_head_run_last_job": dict(sorted(last_job.items(), key=lambda kv: -kv[1])),
        "ci_head_run_install_overrun_s": stats(install_overrun),
        "families": list(FAMILIES),
        "rows": {"changes": change, "releases": release},
    }
    # Which CD run finished last, per release.
    last = {}
    for r in rels:
        cd = [runs[str(rid)] for rid in r["run_ids"]]
        end = max(cd, key=lambda s: s["last_job_completed_at"] or "")
        key = f"{end['workflow']}:{end['event']}"
        last[key] = last.get(key, 0) + 1
    out["per_release"]["last_cd_run"] = last
    json.dump(out, sys.stdout, indent=1, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
