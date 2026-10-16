#!/usr/bin/env python3
"""Render the report's measurement tables from the committed outputs, as Markdown.

    python3 -I spike/onepipeline-ci/report_tables.py spike/onepipeline-ci/out > tables.md

Reads out/figures.json.gz, out/derived-window.json and out/derived-shape.json;
computes nothing new, so every cell is a figure those documents hold.
"""

import gzip
import json
import sys
from pathlib import Path

out = Path(sys.argv[1])
fig = json.load(gzip.open(out / "figures.json.gz"))
win = json.load(open(out / "derived-window.json"))
shp = json.load(open(out / "derived-shape.json"))


def mpm(s):
    return "–" if not s else f"{s['median']:.0f} / {s['p90']:.0f} / {s['max']:.0f}"


def mp(s):
    return "–" if not s else f"{s['median']:.0f} / {s['p90']:.0f}"


print("#### Every CI and CD job and matrix leg (seconds; median / p90 / max over successful runs unless a column says otherwise)\n")
print("| Workflow :: job (leg) | OS | Window: ok / failed / cancelled | Window duration | Current shape: ok / failed / cancelled | Current-shape duration | Current-shape queue, job created → started (median / p90) | Window wait, run created → started | Current-shape wait, run created → started |")
print("| --- | --- | --- | --- | --- | --- | --- | --- | --- |")
for key, w in fig["per_job"].items():
    s = shp["per_job"].get(key)
    if not w["duration_success_s"] and not (s and s["duration_success_s"]):
        continue
    sc = f"{s['success']} / {s['failed']} / {s['cancelled']}" if s else "–"
    print(f"| {key.replace('|', '/')} | {', '.join(w['os'])} | {w['conclusions'].get('success', 0)} / {w['failed']} / {w['cancelled']} "
          f"| {mpm(w['duration_success_s'])} | {sc} | {mpm(s['duration_success_s']) if s else '–'} "
          f"| {mp(s['queue_from_job_created_success_s']) if s else '–'} | {mpm(w['queue_from_run_created_success_s'])} "
          f"| {mpm(s['queue_from_run_created_success_s']) if s else '–'} |")

print("\n#### Runs per workflow in the window (run-level conclusions, every run listed)\n")
print("| Workflow | Runs | First run in window | success | failure | cancelled | skipped | other |")
print("| --- | --- | --- | --- | --- | --- | --- | --- |")
for wf, c in fig["run_counts"].items():
    b = c["by_conclusion"]
    other = sum(v for k, v in b.items() if k not in ("success", "failure", "cancelled", "skipped"))
    print(f"| {wf} | {c['runs']} | {c['first_created_at']} | {b.get('success', 0)} | {b.get('failure', 0)} | {b.get('cancelled', 0)} | {b.get('skipped', 0)} | {other} |")

print("\n#### Jobs executed per run (skipped jobs excluded, matrix legs counted; median / p90 / max)\n")
print("| Scope | Measure | Whole window | Current shape |")
print("| --- | --- | --- | --- |")
for label, key in [("PR head commit, every workflow", "pr_head_job_count_all_workflows"),
                   ("PR head commit, ci.yml only", "ci_job_count_ci_yml"),
                   ("One release's CD runs", "cd_job_count")]:
    print(f"| {label} | total | {mpm(fig['summary'][key])} | {mpm(fig['summary_shape'][key])} |")
for label, key in [("PR head commit (feature PR)", "pr_head_jobs_by_workflow"), ("PR head commit (feature PR)", "pr_head_jobs_by_os"),
                   ("Release PR head commit", "release_pr_head_jobs_by_workflow"),
                   ("One release's CD runs", "cd_jobs_by_workflow"), ("One release's CD runs", "cd_jobs_by_os")]:
    keys = sorted(set(fig["summary"][key]) | set(fig["summary_shape"][key]))
    for k in keys:
        print(f"| {label} | {k} | {mpm(fig['summary'][key].get(k))} | {mpm(fig['summary_shape'][key].get(k))} |")

print("\n#### Cycle time of merged feature PRs (seconds; median / p90 / max)\n")
print("| Phase | Whole window | Current shape |")
print("| --- | --- | --- |")
for label, key in [("Last-commit checks phase", "checks_phase_s"), ("Merge to release (last CD job)", "merge_to_release_s"),
                   ("Cycle (sum)", "cycle_s"), ("Release-PR checks phase (release PRs)", "release_pr_checks_phase_s")]:
    a, b = fig["summary"][key], fig["summary_shape"][key]
    print(f"| {label} | {mpm(a)} (n={a['n']}) | {mpm(b)} (n={b['n']}) |")
for label, key in [("Merge → its release PR merges", "feature_merge_to_release_pr_merge"),
                   ("Release PR merges → last CD job", "release_pr_merge_to_last_cd_job")]:
    a, b = win["merge_to_release_split_s"][key], shp["merge_to_release_split_s"][key]
    print(f"| {label} | {mpm(a)} (n={a['n']}) | {mpm(b)} (n={b['n']}) |")
