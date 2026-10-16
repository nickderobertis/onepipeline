#!/usr/bin/env python3
"""Assemble the spike-report template's answers for spike-onepipeline-ci.

    python3 -I spike/onepipeline-ci/report_answers.py spike/onepipeline-ci/out > answers.json

The measurement tables come from report_tables.py over the committed outputs;
the local-profile figures from out/local/*.json (local_tables.py). The prose
below cites those figures; every projection in it says it is one and names the
measurements it rests on. JSON is valid YAML, which the template answers file is.
"""

import json
import subprocess
import sys
from pathlib import Path

out = Path(sys.argv[1])
here = Path(__file__).resolve().parent
tables = subprocess.run([sys.executable, "-I", str(here / "report_tables.py"), str(out)],
                        capture_output=True, text=True, check=True).stdout
local = subprocess.run([sys.executable, "-I", str(here / "local_tables.py"), str(out / "local")],
                       capture_output=True, text=True, check=True).stdout
LOCAL = json.loads(subprocess.run([sys.executable, "-I", str(here / "local_tables.py"), str(out / "local"), "--json"],
                                  capture_output=True, text=True, check=True).stdout)
assert not LOCAL["valid_runs"], "a valid local run exists: rewrite the local-profile sentences below from it"
HITS = json.load(open(out / "nx-input-hits.json"))["summary"]

BRANCH = "nick/ci-time-budgets-wave1/spike-onepipeline-ci"
INVOCATION = (
    "python3 -I spike/onepipeline-ci/actions_spike.py --repo nickderobertis/onepipeline --since 2026-09-15 "
    "--raw <store> --out <figures.json> --log <store>/requests.jsonl --jobs-for \"\" --newest-first "
    "--shape-since 2026-10-07T10:35:42Z --merge-push-for ci.yml,visual-docs.yml "
    "--required-checks-out spike/onepipeline-ci/required-status-checks.json "
    "--extra \"repos/nickderobertis/screencomp/contents/.github/workflows/visual-docs-reusable.yml?ref=v0.4.8\" "
    "--job-log 116698687585 --job-log 116660719008 --job-log 116631701735 --job-log 116631701780 --job-log 115817452216"
)

harness = f"""Everything is under `spike/onepipeline-ci/` on `{BRANCH}`. Python 3 standard library and `gh` only; every GitHub read is a `gh api --method GET`.

**The one command** — `actions_spike.py`, given a repository and a start date, reads its Actions run, job and pull-request records read-only, keeps each response body in a raw store keyed by its request path (a path the store holds is never requested again), and writes the per-job, per-run, per-PR (both cycle-time phases) and per-release figures as one JSON document. Before every request it holds the core quota the `rate_limit` endpoint last reported (re-read every 10 requests; that read is itself logged and counted), refuses any request once that figure less the requests since it is not above `--floor 2500`, or once `--cap 3000` requests are logged across every invocation, and logs each request with that quota and the `X-RateLimit-Remaining` GitHub answered. `--offline` makes no request and computes from the store alone. Tag containment and tag commits come from local git and are kept in the store too, so `--offline` needs no checkout.

The report's figures are the output of exactly this invocation, run over the store every earlier invocation filled (it made 0 requests; `out/requests.jsonl` holds all 2,243 that the spike's invocations made):

```
{INVOCATION}
```

Its output is `out/figures.json.gz`. To register a budget command from it: `--pr <n>` / `--run <id>` skip enumeration and compute one PR's two cycle-time phases or one run's jobs; `summary.ci_check_time_s`, `summary.cd_job_time_s`, `summary.release_cycle_s`, `summary.pr_head_job_count_all_workflows` and `summary.cd_job_count` are the five GitHub candidates' distributions.

**Offline reproduction** — `fixture/raw/` holds the 16 raw responses (and 2 git answers) the command read for PR #942 and its head-commit `ci.yml` run 38782078021, including the CD runs of v0.73.0, the release whose tag first contains #942's merge commit. `check-fixture.sh` runs the command `--offline --pr 942 --run 38782078021` over them, with no request and no checkout, and compares the result with `out/figures.json.gz`: PR #942's checks phase 7,844 s and merge-to-release 13,395 s, every one of run 38782078021's 18 jobs, and v0.73.0's CD runs. It exits 0 on the branch; altering one job's `completed_at` in the fixture makes it exit 1 naming the PR and the run (checked, then restored).

**Derived figures** — `analyze.py out/figures.json.gz [--since <instant>]` → `out/derived-window.json`, `out/derived-shape.json` (repeat families, `needs` waits, CD phase timeline, merge-to-release split, macOS runner-seconds, current-shape per-job statistics); `merge-trees.py` → `out/merge-tree-equality.json` (whether each merge commit's tree is the tree its PR head was tested on, fetched over git, no REST); `report_tables.py` and `report_answers.py` render this report from those files.

**Local profile** — `profile-chain.sh <work-dir> <standalone-clone>` runs `local-profile.sh` for `just gate` and `just check-affected`, each Nx-cold (the clone's `.nx/cache` and `.nx/workspace-data` deleted) and then Nx-warm (what that run left), recording wall clock, the uptime clock (a VM pause shows as wall clock the uptime clock did not see), host load, and every Nx task each invocation ran from Nx's own `.nx/cache/run.json`; `local_tables.py out/local` summarizes them. It must run in a standalone clone, not a git worktree (see Method). `required-status-checks.json` is the dated snapshot of `main`'s required status checks."""

method = f"""**Real systems.** GitHub's REST API for `nickderobertis/onepipeline`, read through this host's `gh` login, read-only; local git for tag containment and merge trees; this repository's own `just` recipes in a standalone clone of the session branch on this host. No workflow, protection, pull request, job run or release of any repository was changed.

**Window and sample.** Every run of the eight workflows created on or after 2026-09-15 was listed (`run_counts` below: 3,124 runs). Jobs were read, with `filter=latest` (a re-run's last attempt), for: the last head commit's runs of all **224 PRs merged in the window** (144 feature PRs, 80 release-plz PRs), every workflow, `pull_request` runs created before the merge; the CD runs of **all 80 releases** published in the window — the push runs the release-PR merge started (`ci.yml`, `release-plz.yml`, `engine-currency.yml`, `visual-docs.yml`), the `release` run, and the `published-smoke.yml` `workflow_run` follow-ups; and the `ci.yml` and `visual-docs.yml` push runs each of the 144 feature merges started on `main`. Sampled out, and so missing: jobs of superseded (earlier-commit) PR runs and of unmerged PRs' runs (their run-level conclusions are counted, not their jobs), and the `release-plz.yml` / `engine-currency.yml` push runs of feature merges. 1,413 runs' jobs were read (12,437 executed jobs). Job logs were read for 5 jobs: the latest successful `cross (windows-latest)` (116631701780) and `cross (macos-latest)` (116631701735) legs, `gate` (116660719008), `release-sweep` (115817452216) and Release `test` (116698687585).

**Current shape.** `release-sweep.yml` and `notignored.yml` first ran at 2026-10-07T10:35:42Z (#783 split the e2e tiers into Nx projects and added the release-PR sweep) and `visual-docs.yml` on 2026-09-23, so the window spans three workflow shapes. Every figure is given over the whole window and over the **current shape** (PRs merged and releases published since 2026-10-07T10:35:42Z: 31 feature PRs, 21 release PRs, 21 releases). Durations have grown through the window (`cross (windows-latest)` median 4,538 s window, 5,873 s current; the latest green leg 9,155 s), so projections use current-shape figures.

**Definitions.** Duration: job `started_at` → `completed_at`. Queue: job `created_at` → `started_at`, and separately run `created_at` → job `started_at` (which also holds `needs` waits). Median, nearest-rank p90 and max over successful jobs. Jobs per run exclude skipped jobs and count matrix legs. Checks phase: first head-commit run's `created_at` → last of its jobs' `completed_at`. Merge-to-release: PR `merged_at` → last CD job `completed_at` of the first release whose tag contains the merge commit (earliest published). Release cycle: the maximum over the feature PRs a release carries of checks + merge-to-release.

**Eligibility.** 144 feature PRs were merged in the window; 143 had a completed release (#922 merged at 22:18Z on 2026-10-15 after v0.73.0 was cut), so cycle time covers 143 PRs (30 of the 31 current-shape PRs).

**Quota.** 2,243 logged requests: 2,037 data reads (all `GET`, all `200`) and 206 `rate_limit` reads. Before every data read the last-reported core quota was at least 2,502 (at least 2,501 net of reads since that report); the floor stopped the first invocation at seq 685 (2,502 remaining, other sessions on this host were spending ~65 requests a minute from the same token) and the next invocation waited for the reset. The five `rate_limit` reads that opened an invocation had no earlier report; GitHub does not count `rate_limit` reads against the core quota. One further attempt is not in the log: the harness version before commit bdc4372c raised on a connection timeout (`dial tcp 140.82.113.5:443: i/o timeout`, during the VM pause below) before logging it; it never reached GitHub. 2,244 attempts against the 3,000 cap.

**Limits on how far the numbers carry.** GitHub-hosted runners differ run to run and macOS jobs share the account-wide 5-job macOS limit with every other repository (GitHub, *Job concurrency limits for GitHub-hosted runners*: Free/Pro/Team 5 concurrent macOS jobs), so queue figures are this account's, at this traffic. The local host is an aarch64 VM with 14 vCPUs shared with other sessions (load average 20–57 during the attempts) and a disk other sessions filled; no valid local profile was obtained (*Local profile on this host*, below), so every local figure the task asked for is named there as missing.

{tables}

#### Local profile on this host

{local}"""


# The user's target CI/CD shape (ci-time-budgets-wave1 rulings): every job this
# repository runs today folded into lint + test-<os> per PR and release-plz +
# build-<os> + publish per release. Each target job is projected from the
# current-shape durations of the jobs it absorbs: its floor is the longest of them
# (all absorbed work overlapping on one runner), its ceiling their serial sum
# (no setup shared). Neither is measured; both are PROJECTIONS.
SHAPE = json.load(open(out / "derived-shape.json"))["per_job"]


def _stat(job, key):
    return SHAPE[job]["duration_success_s"][key]


def fold(jobs, key="median"):
    vals = [_stat(j, key) for j in jobs]
    return round(max(vals)), round(sum(vals))


def n(x):
    return f"{round(x):,}"


TARGET_CI = {
    "lint": ["ci.yml :: llmlint", "ci.yml :: pr-title", "ci.yml :: msrv", "ci.yml :: deny", "ci.yml :: release-compat",
             "engine-currency.yml :: engines-current", "notignored.yml :: suppressions",
             "visual-docs.yml :: visual-docs / arches", "visual-docs.yml :: visual-docs / pages-preflight",
             "visual-docs.yml :: visual-docs / capture (x86_64, shots/current, shots/verify, Visual docs)",
             "visual-docs.yml :: visual-docs / report (x86_64, shots/current, shots/verify, Visual docs)",
             "visual-docs.yml :: visual-docs / deploy-pages"],
    "test-linux": ["ci.yml :: gate", "ci.yml :: smoke", "ci.yml :: install (ubuntu-latest)",
                   "ci.yml :: install-documented (ubuntu-latest)", "ci.yml :: wheel",
                   "ci.yml :: wheel (aarch64-unknown-linux-gnu)"],
    "test-windows": ["ci.yml :: cross (windows-latest)", "ci.yml :: install (windows-latest)",
                     "ci.yml :: install-documented (windows-latest)"],
    "test-macos": ["ci.yml :: cross (macos-latest)", "ci.yml :: install (macos-latest)",
                   "ci.yml :: install-documented (macos-latest)"],
}
TARGETS = {"linux": ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"],
           "macos": ["aarch64-apple-darwin", "x86_64-apple-darwin"],
           "windows": ["x86_64-pc-windows-msvc"]}
VERIFY = {"linux": ["release.yml :: verify-crate", "release.yml :: verify-pypi (ubuntu-latest)",
                    "release.yml :: verify-npm (ubuntu-latest)", "release.yml :: verify-npm (ubuntu-24.04-arm)"],
          "macos": ["release.yml :: verify-pypi (macos-latest)", "release.yml :: verify-pypi (macos-15-intel)",
                    "release.yml :: verify-npm (macos-latest)", "release.yml :: verify-npm (macos-15-intel)"],
          "windows": ["release.yml :: verify-pypi (windows-latest)", "release.yml :: verify-npm (windows-latest)"]}
PUBLISH = ["release.yml :: guard", "release.yml :: linked-engines", "release.yml :: publish-crate",
           "release.yml :: publish-pypi", "release.yml :: publish-npm", "release.yml :: record the release outcome"]
RELEASE_PLZ = ["release-plz.yml :: guard", "release-plz.yml :: release-plz"]


def build(os_, key="median"):
    # One build per target packaging all three artifacts: floor = that target's
    # longest of today's three builds; ceiling = all three, as today, in series.
    lo = hi = 0
    for t in TARGETS[os_]:
        three = [_stat(f"release.yml :: {k} ({t})", key) for k in ("upload", "build-npm", "build-wheels")]
        lo += max(three)
        hi += sum(three)
    verify = sum(_stat(j, key) for j in VERIFY[os_])
    return round(lo + verify), round(hi + verify)


T_CI = {k: (fold(v), fold(v, "p90")) for k, v in TARGET_CI.items()}
T_BUILD = {o: (build(o), build(o, "p90")) for o in TARGETS}
T_PUBLISH = round(sum(_stat(j, "median") for j in PUBLISH))
T_PLZ = round(sum(_stat(j, "median") for j in RELEASE_PLZ))
CI_LO = max(v[0][0] for k, v in T_CI.items() if k != "lint")
CI_HI = max(v[0][1] for v in T_CI.values())
CI_HI90 = max(v[1][1] for v in T_CI.values())
B_LO = max(v[0][0] for v in T_BUILD.values())
B_HI = max(v[0][1] for v in T_BUILD.values())
CD_PATH_LO = T_PLZ + B_LO + T_PUBLISH
CD_PATH_HI = T_PLZ + B_HI + T_PUBLISH
CYCLE_LO = 2 * CI_LO + CD_PATH_LO
CYCLE_HI = 2 * CI_HI + CD_PATH_HI


def ci_line(k):
    (lo, hi), (lo90, hi90) = T_CI[k]
    return f"`{k}` {n(lo)}–{n(hi)} median ({n(lo90)}–{n(hi90)} p90)"


def b_line(o):
    (lo, hi), (lo90, hi90) = T_BUILD[o]
    return f"`build-{o}` {n(lo)}–{n(hi)} median ({n(lo90)}–{n(hi90)} p90)"


TARGET_SHAPE = (
    "**The user's target CI/CD shape, projected from this repository's measurements (PROJECTIONS: each target job's range runs from the longest current-shape median of the jobs it absorbs, all of their work overlapping on one runner, to their serial sum, no setup shared; p90 likewise from p90s; `report_answers.py` computes them from `out/derived-shape.json`).** "
    f"Per PR, 4 jobs (screencomp.toml's arches are `[x86_64]` only, so no second visual-docs job): {ci_line('lint')} (llmlint, pr-title, msrv, deny, release-compat, engines-current, notignored, and visual-docs' arches, pages-preflight, capture, report and preview deploy); "
    f"{ci_line('test-linux')} (`gate`, `smoke`, install and install-documented on ubuntu, both `wheel` legs); "
    f"{ci_line('test-windows')} (`cross`, install, install-documented); {ci_line('test-macos')} (the same on macOS). "
    f"Nx-affected selection does not lower the test jobs: 31 of 31 current-shape feature PRs changed a `rustBuild` input every Rust tier hashes. "
    f"**Consequence for the `timeout-minutes: 30` rule: `lint` fits (ceiling {n(T_CI['lint'][0][1])} s), but `test-linux`, `test-windows` and `test-macos` each exceed 1,800 s at their floor, so under that rule all three would be cancelled on every PR that touches Rust until the Windows, macOS and Linux test tiers are cut (the expensive-strategy finding's options); that is a decision for the user, not a threshold to set.** "
    f"Per release, 5 jobs: `release-plz` {n(T_PLZ)} (guard + release-plz); {b_line('linux')}, {b_line('macos')}, {b_line('windows')} — each building its targets once (floor: the longest of today's three release builds per target; ceiling: all three, as today) plus today's post-publish verify legs for that OS (pre-publish test-installs of the exact files are not measured separately and are taken as no longer than those verify legs); `publish` {n(T_PUBLISH)} (guard, linked-engines, publish-crate, publish-pypi, publish-npm, record outcome; the budget-reader tail is not measured). "
    f"If each build job waits for `publish` before its post-publish verification, add ≈{n(T_PUBLISH)} s to every build job, making the longest CD job {n(B_LO + T_PUBLISH)}–{n(B_HI + T_PUBLISH)} s; otherwise it is {n(B_LO)}–{n(B_HI)} s. "
    f"Removed against today: the release test re-run (3,738 s median off the CD critical path), the 15-job ci.yml push re-run on the release merge, the 9 published-smoke jobs, and release.yml's 32 jobs folded into 4: 5 jobs per release against today's 66 median. "
    f"Release cycle with zero queue and the release PR merged the moment its checks pass: 2 × the PR checks phase ({n(CI_LO)}–{n(CI_HI)}) + release-plz + longest build + publish ({n(CD_PATH_LO)}–{n(CD_PATH_HI)}) = {n(CYCLE_LO)}–{n(CYCLE_HI)} s, against today's current-shape median release cycle of 38,235 s; the macOS queue (cross macOS job created → started 1,172 s median, 12,419 p90; macOS release builds 1,761–3,847 s median) and the wait for someone to merge the release PR come on top and are not projected. "
    "Removing the push re-run is conditional on requiring branches to be up to date: the snapshot reads `strict: false` today, and 12 of the 31 current-shape feature merges (52 of 144 in the window; `out/merge-tree-equality.json`) landed a tree their PR head was not tested on, so under `strict: true` each such PR would first be updated and its whole checks phase re-run — at the projected test-windows duration per update."
)


def cand(budget, measure, workload, achievable, limits, consumed):
    return {"budget": budget, "measure": measure, "workload": workload, "achievable": achievable,
            "limits": limits, "consumed": consumed}


GH_CONSUMED = "share of the spike's 2,244 GitHub REST attempts (2,037 data reads) and the floor was never crossed; no CI run was triggered"
candidates = [
    cand("onepipeline-ci-check-time",
         "Longest job (started→completed, s) of one PR head-commit ci.yml run",
         "144 feature PRs' last-commit ci.yml runs, 2026-09-15→10-15; 31 in the current shape (since 2026-10-07T10:35Z)",
         "Today: current shape 5,988 median / 7,824 p90 / 13,037 max (window 4,560 / 6,030 / 13,037); the longest job is cross (windows-latest) in 29 of 31. Achievable after the recommended coverage-preserving cuts: unchanged — PROJECTION 5,988 / 7,824, because no measured coverage-preserving cut shortens the Windows leg (Nx cache restore hits no Rust tier on 31 of 31 current PRs). With the coverage-reducing Windows e2e cut (finding: Windows e2e subset) the longest becomes cross (macos-latest): PROJECTION 3,941 / 4,731 from its measured current durations. No choice reaches 1,800 without sharding.",
         "Windows e2e alone: 28,682 test-seconds (run 116631701780) over the e2e test group's 4 threads on a 4-vCPU runner = 7,170 s floor; macOS e2e 12,447 test-s over 4 = 3,112 s; job timeout 220 min (13,200 s) and GitHub's 6 h job limit (21,600 s)",
         GH_CONSUMED),
    cand("onepipeline-cd-job-time",
         "Longest job (s) of one release's CD runs (release-PR merge push runs, release run, workflow_run follow-ups)",
         "All 80 releases 2026-09-15→10-15; 21 in the current shape",
         "Today: current shape 5,719 / 7,828 / 7,886 (window 3,857 / 5,990 / 7,886); the longest is ci.yml's push run's cross (windows-latest) on the release merge commit in 18 of 21. Achievable: PROJECTION 3,738 / 4,663 / 5,058 (release.yml test's measured current durations) once the push re-run of a tree its release PR already passed is skipped (19 of 21 current release merges had that tree); with the release-model change that skips test on a swept tree, PROJECTION 926 median (longest measured release build).",
         "release.yml test is a full instrumented `just check` on a 4-vCPU ubuntu runner: 3,738 s median; builds 417–895 s each",
         GH_CONSUMED),
    cand("onepipeline-release-cycle-time",
         "Max over the feature PRs one release carries of (last-commit checks phase + merge to the release's last CD job), s",
         "80 releases carrying 143 feature PRs; 21 releases / 30 PRs in the current shape",
         "Today: current shape 38,235 / 76,486 / 100,919 (window 34,653 / 63,261 / 123,149). Achievable: PROJECTION about 34,500 median / 69,800 p90 after the deterministic coverage-preserving shifts (release builds start beside test: −3,738 median; install no longer behind gate: −2,935 at p90), before any macOS-queue relief, which is not projected because it depends on the account's other repositories. The 90-minute target is not reachable without a release-model change: see the findings' critical-path arithmetic (≈7,300 s even with every job at 1,800 s and no queue).",
         "Serial path with every job at the 1,800 s line and zero queue: checks 1,800 + release-plz 40 + release-PR checks 1,800 + release test 1,800 + longest build 926 + publish-npm 818 + verify-npm 36 + outcome 9 + published-smoke 72 ≈ 7,300 s > 5,400 s; macOS cap 5 concurrent jobs against 17 macOS CD jobs per release",
         GH_CONSUMED),
    cand("onepipeline-ci-job-count",
         "Jobs executed (skipped excluded, matrix legs counted) in one PR head commit's runs, all workflows",
         "144 feature PRs' last head commit; 31 in the current shape",
         "Today: 25 in every current-shape PR (ci.yml 18, visual-docs 5, engine-currency 1, notignored 1; 19 Linux, 3 Windows, 3 macOS); window 24 median / 25 max. Achievable: PROJECTION 26 with the verdict job added and visual-docs moved into ci.yml (no job removed by a coverage-preserving cut on the PR head); 25 if the macOS install smoke runs inside cross (macos-latest) instead of its own job.",
         "GitHub Free/Pro/Team: 20/40/60 concurrent jobs and 5 macOS per account; each PR head holds 3 macOS slots",
         GH_CONSUMED),
    cand("onepipeline-cd-job-count",
         "Jobs executed in one release's CD runs",
         "80 releases; 21 in the current shape",
         "Today: current shape 66 / 75 / 87 (ci.yml 15, release.yml 32, published-smoke 9, visual-docs 6, release-plz 2, engine-currency 1; 38 Linux, 10 Windows, 17 macOS); window 64.5 / 74 / 87. Achievable: PROJECTION 51 after skipping the identical-tree ci.yml push re-run (−15, in 19 of 21 releases); 43 if published-smoke's workflow_run trigger is dropped (−8, release-model decision); 39 if each darwin target is built once for archive, wheel and npm (−4, artifact decision).",
         "macOS: 17 of 66 CD jobs against an account-wide cap of 5 concurrent",
         GH_CONSUMED),
    cand("onepipeline-gate-time",
         "`just gate` wall clock on this host (check + the diff-scoped judged lint), s, Nx-cold and Nx-warm",
         "This host: aarch64, 14 vCPUs shared with other sessions, load average 20–57; the session branch (its diff is the spike harness)",
         "Today: MISSING, cold and warm — four attempts, none valid (one VM pause, two disk-floor stops, one invalidated by the spike's own clone setup; Method names each). Measured substitute on a CI runner, not this host: the same full `just check` is release-sweep 3,954 / 4,912 / 5,173 s and the judged tier (CI llmlint) 205 / 320 / 525 s, so PROJECTION ≈4,160 s median for `just gate` on a 4-vCPU ubuntu runner, Nx-cold. Achievable after cuts: MISSING (no measured local per-target breakdown to project from); restoring an Nx cache locally replays only targets whose inputs a change leaves alone, which no current-shape PR's Rust tiers were.",
         "On this host a cold `just check` needs about 15–17 GB of cargo target (the clone's grew to 7.5 GB in its first 40 minutes and 17 GB in the worktree); free space ranged 1.1–29 GB during the spike as other sessions wrote",
         "Four local attempts, about 3.3 h of host time (attempt 1 about 1 h 29 min of uptime before it was stopped, attempt 2 50 min, attempt 3 43 min, attempt 4 18 min) and two clone bootstraps (493 s and 145 s); no GitHub request"),
]

# What each GitHub candidate becomes in the user's target shape (TARGET_SHAPE finding).
TARGET_ACH = {
    "onepipeline-ci-check-time": f"In the user's target shape (lint + test-linux/windows/macos; design finding *target CI/CD shape*): PROJECTION {n(CI_LO)}–{n(CI_HI)} s median ({n(CI_HI90)} p90 ceiling), the longest being `test-windows`, which absorbs cross, install and install-documented on Windows — consolidation lengthens the longest job, and `timeout-minutes: 30` would cancel every test job today.",
    "onepipeline-cd-job-time": f"In the user's target shape: PROJECTION {n(B_LO)}–{n(B_HI)} s for the longest per-platform build job, or {n(B_LO + T_PUBLISH)}–{n(B_HI + T_PUBLISH)} s if each build job waits for `publish` ({n(T_PUBLISH)} s) before its post-publish verification.",
    "onepipeline-release-cycle-time": f"In the user's target shape: PROJECTION {n(CYCLE_LO)}–{n(CYCLE_HI)} s with zero queue and the release PR merged as soon as its checks pass (2 × PR checks + release-plz + longest build + publish), before macOS queueing and the release-PR merge wait.",
    "onepipeline-ci-job-count": "In the user's target shape: 4 (lint, test-linux, test-windows, test-macos; no second visual-docs platform, since screencomp.toml's arches are `[x86_64]`) — the ruling's threshold, met by construction.",
    "onepipeline-cd-job-count": "In the user's target shape: 5 (release-plz, build-linux, build-macos, build-windows, publish) — the ruling's threshold, met by construction.",
}
for c in candidates:
    if c["budget"] in TARGET_ACH:
        c["achievable"] += " " + TARGET_ACH[c["budget"]]

findings = [
    # repeats
    "**Checks that run more than once per change, measured (current shape, median job-seconds per change; full families in `out/derived-shape.json`).** "
    "(1) **`just check`** runs on the PR head (`gate`, check-affected: 3,428 s), again on the merge push to `main` (3,492 s), on the release PR (`gate` + `release-sweep`: 7,305 s) and in CD (the release merge's push `gate` + Release `test`: 6,846 s) — 21,071 job-seconds per one-PR release cycle where one PR-head run and one release gate are the model's. The merge tree equals the tested PR-head tree in 19 of 31 current feature merges (92 of 144 in the window) and the release merge tree equals the release-PR head in 19 of 21 (78 of 80), so those re-runs check a byte-identical tree. "
    "(2) **`cross` (check-cross on macOS + Windows)** runs on the PR head (10,007 s), the merge push (9,416 s), the release PR (9,797 s) and the release merge push (9,573 s): 38,793 job-seconds, four times per change. "
    "(3) **`scripts/smoke-published.sh`** runs in CI `install` ×3, `install-documented` ×3 and `wheel` ×2 on every one of those four ci.yml runs (head 4,087 s, merge push 4,680 s, release PR 4,271 s, CD 4,981 s including verify-crate, verify-pypi ×4, verify-npm ×5 and published-smoke ×8); `published-smoke.yml` repeats verify-pypi's 4 OSes and 4 of verify-npm's 5 within minutes of them (8 jobs, 230 job-s median, but 4 macOS slots). "
    "(4) **Release-mode builds of the 5 targets** run three times per release (`upload`, `build-npm`, `build-wheels`: 9,054 job-s, 6 of the 15 jobs on macOS), and `just wheel-linux` runs in CI `wheel` ×2 (529 + 458 s) on each ci.yml run and again in `build-wheels`' 2 Linux legs (503 + 457 s). "
    "(5) **visual-docs capture** runs on the PR head (873 s), the release PR (847 s) and the release merge push (379 s median, deploying the canonical gallery). "
    "Per change, the merge push re-run costs 17,964 job-s (5,171 macOS) and the release PR's head-commit runs 23,017 job-s (4,862 macOS); per release CD ci.yml costs 16,877 job-s. The repeats sit on the critical path where they gate something: the release-PR checks phase (8,302 s median, 23,839 p90) precedes the release, and Release `test` (3,738 s) precedes every build.",
    # needs edges
    "**`needs` edges that wait without consuming the needed job's output.** "
    "(a) `ci.yml` `install` (line 400) `needs: [changes, gate]` but reads only `changes.outputs.crate`: its legs start 3 s after `gate` ends on Linux/Windows (run created → started 3,527 s median), and the macOS leg then queues for a macOS slot (2,332 s median, 13,414 p90 after gate's end). `install (macos-latest)` finished last in 9 of 31 current PR-head CI runs, outlasting every other job by up to 9,844 s (p90 2,935 s): that is the edge's measured cost on the checks phase; 0 s at the median, because `cross (windows-latest)` usually ends later. "
    "(b) `release.yml` `upload` (188), `build-wheels` (307) and `build-npm` (451) `needs: test`, and `publish-crate` (233) `needs: [guard, test]`; none reads an output of `test`, they only wait for it. The first build starts 2 s (median) after `test` ends, so the full 3,738 s (median, 4,663 p90) of `test` precedes every build on the CD critical path, and the macOS builds then queue 1,761–3,847 s (median, job created → started; p90 8,789–14,405). `publish-pypi` (355) and `publish-npm` (503) reach `test` only through the builds, so cutting the build edges means adding `needs: test` to those two, keeping publication gated. "
    "(c) `published-smoke.yml` runs on `workflow_run: Release completed` (lines 36–42): it waits for all of Release, including the verify legs it repeats; it was the last CD job in 79 of 80 releases. "
    "Every other `needs` in ci.yml (`changes` → cross, msrv, smoke, release-compat, deny, wheel) reads `changes.outputs.crate`; visual-docs' reusable chain passes artifacts; `release-plz` reads `guard`'s output.",
    # per job projections
    "**Projected duration of every CI and CD job after the recommended coverage-preserving cuts (PROJECTIONS; current-shape median / p90 durations measured above, unless a cut removes the run).** The recommended cuts: P1 skip a push re-run on `main` whose tree equals a tree whose PR head passed every required check (keep `deny` and `install-documented`, which read the advisory DB and the remote's default branch, and keep visual-docs' push, which deploys the gallery); P2 start `release.yml`'s builds beside `test`, gating `upload`'s release upload and the publishes on `test`; P3 drop `install`'s `needs: gate`; P4 skip the stop-verdict tier's build of every other test binary (measured locally, below); P5 Windows e2e speed work (unmeasured; see the expensive-strategy finding). Durations per job are unchanged by P1–P3 — they move start times and remove runs: "
    "ci.yml PR head — changes 13/17; gate 3,440/4,237 (P4's saving is not projected: the only local measure is attempt 3's partial cold `onepipeline-stop-verdict:test`, 1,388 s on this host including its build of every test binary, and CI exposes no split); cross (windows-latest) 5,873/7,281; cross (macos-latest) 3,941/4,731; install macOS 564/753, ubuntu 387/591, windows 646/998, now starting at run start instead of 3,527 s in (P3); install-documented macOS 609/817, ubuntu 483/581, windows 861/1,059; wheel 529/602; wheel aarch64 458/513; msrv 79/246; deny 36/39; release-compat 152/345; smoke 398/464; pr-title 4/5; llmlint 205/320; visual-docs capture 856/1,081, report 13/18, arches 7/8, pages-preflight 3/4, deploy-pages 31/70; engines-current 8/9; notignored 10/12; release-sweep 3,954/4,912. "
    "ci.yml / visual-docs.yml push on a merge — removed where the tree was tested (61% of current feature merges, 90% of release merges), otherwise as on the PR head. "
    "release.yml — guard 3/4, linked-engines 9/11, test 3,738/4,663, upload 417–895, build-npm 426–890, build-wheels 457–838 (each now starting at run start, P2), publish-crate 218/232, verify-crate 482/569, publish-pypi 33/42, publish-npm 818/1,250, verify-pypi 11–27, verify-npm 8–25, record the release outcome 9/14; release-plz guard 3/4, release-plz 39/41; published-smoke legs 8–26. The CD critical path: PROJECTION −3,738 s (median) from P2 alone; the Release run's wall clock (11,282 s median) is otherwise macOS queueing.",
    TARGET_SHAPE,
    # cannot reach 1800
    "**Checks that cannot reach 1,800 s without sharding** (current-shape median / p90 / max, s): "
    "**`cross (windows-latest)`** 5,873 / 7,281 / 9,155 (required). The latest green leg spent 8,598 s in the gate step: e2e 7,958 s wall for 1,348 tests (28,682 test-s, 134 slow), note 127 s, contract 1 s, rest 177 s, bootstrap 435 s. At the e2e group's 4 threads on 4 vCPUs the e2e floor is 7,170 s. "
    "**`cross (macos-latest)`** 3,941 / 4,731 / 5,670: e2e 4,541 s wall for 1,557 tests (12,447 test-s); floor 3,112 s. "
    "**`gate`** 3,440 / 4,237 / 5,034 (required): the latest run's quality-gate step 4,630 s, bootstrap 303 s; what inside that step takes the time is not measured — CI prints no per-target timing and the local profile is MISSING. "
    "**`release-sweep`** 3,954 / 4,912 / 5,173 and **release.yml `test`** 3,738 / 4,663 / 5,058: both a full instrumented `just check`. "
    "Restoring the Nx cache cannot bring any of them under 1,800 s on a PR: 31 of 31 current-shape feature PRs (135 of 144 in the window) change a `rustBuild` input, which every Rust tier hashes, and only 3 of 144 left the e2e tier's inputs untouched. Every other CI and CD job is under 1,800 s today (longest: install-documented windows 1,200 max, visual-docs capture 1,185, publish-npm 1,634).",
    # expensive strategies
    "**Especially expensive test strategies and their alternatives (time saved against coverage lost; measured inputs, PROJECTED savings).** "
    "*The Windows matrix leg (e2e on Windows)*, 5,873 s median, 28,682 e2e test-s; per test 21.3 s against macOS's 8.0 s (2.66×), worst in the git-heavy modules: readjust 4,446 test-s (3.6× macOS), rework 3,188 (4.2×), adoption 2,734 (3.0×), lifecycle 2,670 (2.4×) — 45% of Windows e2e time. "
    "Coverage-preserving: (i) Windows filesystem work — a Dev Drive (ReFS) for the workspace and temp dirs and a Defender exclusion — UNMEASURED here; even at macOS's per-test speed the e2e wall would be ≈1,348 × 8.0 / 3.6 ≈ 3,000 s (PROJECTION), so the leg stays over 1,800 s; (ii) a larger Windows runner with the e2e group's `max-threads` raised to its cores, keeping the job name `cross (windows-latest)` through an explicit `name:` — at 8 threads the e2e floor halves to 3,585 s (PROJECTION, arithmetic on measured test-s); larger runners are billed even on public repositories. "
    "Coverage-reducing, for the user: (iii) run on Windows only the e2e modules that exercise platform behaviour, keeping the full e2e on Linux (`gate`) and macOS: saves up to 7,958 s per Windows leg (measured e2e wall), losing Windows coverage of whatever modules are dropped (1,348 tests run there today); (iv) move the Windows leg off the PR path to the release PR only: saves 5,873 s median from every PR's checks phase (the longest job becomes cross (macos-latest), 3,941 s), losing pre-merge Windows coverage for every change between releases; (v) drop the release-PR and release-merge re-runs of `cross` on a tree that differs from a tested `main` only by release-plz's version bump: saves 9,797 s (release PR) + 9,573 s (CD) job-s and the release-PR checks phase falls to its next-longest required job (PROJECTION ≈ gate 3,440 s), losing a cross-platform run of the version-bumped tree. "
    "*The macOS leg* (cross 3,941 s + install 564 + install-documented 609 per ci.yml run, 5 slots account-wide): preserving — run the cargo-install smoke inside cross (macos-latest) after its tests, sharing its target dir (saves one macOS job and one from-source build per run, ≈564 s macOS time; `install (macos-latest)` is not a required context); reducing — move install-documented's macOS leg to the release PR only (saves 609 s and a macOS slot per run, losing per-PR evidence that the documented one-liner installs on macOS). "
    "*The e2e tier under coverage instrumentation on Linux* (`gate`, release-sweep, Release test): its share of those jobs is not measured (CI hides the split; the local profile is MISSING), so no alternative is costed for it beyond R1 and R4 below. "
    "*Slow single tests* (Windows durations, run 116631701780): rework::a_rework_restacks_every_draft_stacked_on_it_by_merges_alone 306.7 s, rework::a_rework_conflicting_with_the_draft_above_is_concluded_by_that_draft_s_worker 293.1, readjust::the_store_is_written_only_for_what_its_projection_holds_changed 274.8, out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique 262.9, rework::a_rework_whose_change_outgrew_its_title_is_retitled_by_the_drafter 254.6, rework::a_title_nothing_verified_is_left_unverified_until_a_retitle_sets_one 247.8, readjust::a_whole_lower_stack_landing_moves_every_draft_above_it_in_one_readjustment 222.7, unwatched::a_host_sized_runs_root_is_answered_in_well_under_a_second_whatever_its_journals_hold 220.5, rework::a_restack_whose_worker_fails_its_merge_settles_the_draft_failed 219.7, rework::a_rework_the_run_cannot_take_is_refused_naming_why 213.0, rework::the_op_s_title_outranks_the_drafter_s_and_a_keeps_leaves_the_title_alone 206.8, listing::a_host_sized_runs_root_lists_in_seconds_and_ten_times_the_journal_bytes_barely_moves_it 206.6, rework::a_restack_its_checks_refuse_is_asked_of_the_draft_s_worker_again 198.9, readjust::deferred_republications_are_read_by_the_next_trigger_green_or_repaired_red 198.6, review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it 198.0, rework::a_rework_title_check_is_validated_by_the_real_producer 195.8, readjust::one_of_several_parents_landing_advances_the_stack_base_then_moves_the_draft 193.6, readjust::every_deferred_verdict_word_is_recorded_and_only_pending_stays_outstanding 182.7, landing::a_render_asks_the_landing_read_once_per_node_it_prints_and_does_nothing_else_per_node 180.6, loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on 178.5. These 20 are 4,456 test-s (16% of Windows e2e); the host-sized and loopcost tests already take `threads-required = 4` (.config/nextest.toml). Preserving: shrink their fixtures' repetition counts where the assertion is a shape, not a scale; reducing: run the host-sized/loopcost scale tests only on Linux.",
    # release model / artifacts
    "**Cuts that change a published artifact, a release target or the release model (each the user's decision; savings measured, current-shape medians).** "
    "(R1) Skip `release.yml`'s `test` when the tag's tree is one `release-sweep` passed on the release PR (19 of 21 current release merges had their release PR head's exact tree): saves 3,738 s (median, 4,663 p90) from the CD critical path and a 4-vCPU job per release; a hand-cut Release from an unswept tree still runs it — AGENTS.md's stated reason for the re-sweep is preserved only if that tree check is exact. "
    "(R2) Build each darwin target once and package the archive, wheel and npm tarball from that one binary instead of three separate release builds (`upload`, `build-npm`, `build-wheels`): changes how three published artifacts are produced (maturin's build vs. cargo's); saves 4 of 6 macOS jobs per release and ≈2,480 macOS job-s (measured 683+550+601+589+653+650 = 3,726 → ≈1,240), and their queue. "
    "(R3) Drop published-smoke's `workflow_run` trigger, keeping `workflow_dispatch`: it repeats verify-pypi and 4 of verify-npm's legs minutes later and was the last CD job in 21 of 21 current releases; saves 8–9 CD jobs (4 macOS) and the CD tail after Release completes, losing the check that the registries' `latest` resolves to an installable artifact. "
    "(R4) Do not re-run cross, gate and the install legs on the release PR (its tree is `main` plus release-plz's version bump): saves 23,017 job-s and the release-PR checks phase (8,302 s median, 23,839 p90) falls to its packaging legs and `release-sweep` (3,954 s, not required, so it would gate nothing unless required); losing a cross-platform test of the bumped version string. "
    "(R5) The 90-minute cycle: with every job held to 1,800 s and zero queueing, the serial path — head checks 1,800, release-plz 40, release-PR checks 1,800, Release test 1,800, longest build 926, publish-npm 818, verify-npm 36, outcome 9, published-smoke 72 — is ≈7,300 s, so 5,400 s needs at least one of R1, R3 or R4 (each removes 72–1,800 s of that path) on top of every job reaching 1,800 s; today's measured path is 38,235 s median.",
    # verdict
    "**Required-check plan.** `required-status-checks.json` (taken 2026-10-16T00:41:20Z, GET /repos/nickderobertis/onepipeline/branches/main/protection/required_status_checks; `strict: false`) lists 14 contexts; the `ci.yml` job behind each, which a `verdict` job must `needs:`: `gate` → `gate` (ci.yml 63); `pr-title` → `pr-title` (535); `llmlint` → `llmlint` (549); `msrv` → `msrv` (250); `deny` → `deny` (378); `wheel` → `wheel`, its x86_64 leg, named by `name: ${{ matrix.check }}` (501–510); `cross (windows-latest)` → `cross` (187, matrix 193); `install (ubuntu-latest)` and `install (windows-latest)` → `install` (399, matrix 405); `install-documented (ubuntu-latest)` and `install-documented (windows-latest)` → `install-documented` (454, matrix 458); `smoke` → `smoke` (288); `release-compat` → `release-compat` (356); `visual-docs / report (x86_64, shots/current, shots/verify, Visual docs)` → a new `visual-docs` job in ci.yml (today visual-docs.yml 30). So `verdict: needs: [gate, pr-title, llmlint, msrv, deny, wheel, cross, install, install-documented, smoke, release-compat, visual-docs]`, `if: always()`, failing on any `failure` or `cancelled` and passing `skipped` (smoke, pr-title and llmlint skip on push; the crate jobs skip when `changes` says the crate is unaffected). Two consequences for the human step: a matrix job's `needs` result covers every leg, so `verdict` would also hold `cross (macos-latest)`, `install (macos-latest)`, `install-documented (macos-latest)` and `wheel (aarch64-unknown-linux-gnu)`, which are not required today — a tightening; and AGENTS.md says visual-docs is *not* required and ci.yml (183–185) says `cross (macos-latest)` is, both contradicted by the snapshot (follow-up drafted). The user's later ruling (ci-time-budgets-wave1 rulings, *No verdict job*) supersedes this verdict plan: the required set becomes exactly `lint`, `test-linux`, `test-windows` and `test-macos`, so each of the 14 contexts above maps to the target job that absorbs it — `pr-title`, `llmlint`, `msrv`, `deny`, `release-compat` and the visual-docs context → `lint`; `gate`, `smoke`, `wheel`, `install (ubuntu-latest)` and `install-documented (ubuntu-latest)` → `test-linux`; `cross (windows-latest)`, `install (windows-latest)` and `install-documented (windows-latest)` → `test-windows`; and `test-macos` newly requires the macOS legs no context requires today. Under that ruling the visual-docs context name below matters only to the switch-over, since the new required contexts are the four job names.",
    "**Visual-docs context.** A `visual-docs` job in ci.yml calling `nickderobertis/screencomp/.github/workflows/visual-docs-reusable.yml@v0.4.8` with the same `with:` block and no `name:` reproduces `visual-docs / report (x86_64, shots/current, shots/verify, Visual docs)` byte for byte. Evidence: the prefix is the caller's job id — visual-docs.yml line 30 `visual-docs:` with no `name:` — and every job record read under it is named `visual-docs / <called job>`; the called job is `report` (reusable line 387); its parenthesised suffix is the matrix the `arches` job builds (reusable lines 305–316: `arch`, `project`, `current`, `verify`, `manifest`, `gallery_title`), rendered with the empty `project` and `manifest` omitted; and `Visual docs` is the `gallery-title` input's default (reusable lines 111–114), not the calling workflow's `name: Visual docs` (visual-docs.yml line 7) — so renaming the workflow to `CI` does not change it. It stays exact only while ci.yml's job id is `visual-docs` with no `name:`, `gallery-title` is unset, `screencomp.toml`'s arches stay `[x86_64]`, and the pin's reusable workflow keeps those lines. GitHub's documentation pages read (*Reuse workflows*, *Workflow syntax*) state no naming rule for called jobs, so this rests on the workflow lines and the job records, not on a documented guarantee. The caller job must grant `contents: write` and `pull-requests: write` (visual-docs.yml 38–40), because ci.yml's workflow-level `permissions: contents: read` (9–10) would otherwise cap the called jobs (reusable deploy-pages comment: a called job's permissions must be a subset of the caller's). "
    "Triggers once the PR run moves: visual-docs.yml must keep `pull_request: types: [closed]` — `cleanup-preview` runs only on `closed` (reusable line 523) — and its monthly `schedule` — `prune-history` runs only on `schedule` (reusable line 548); its `push: branches: [main]` keeps deploying the canonical gallery (or moves to ci.yml's push with `if:` guards). Its `pull_request` types must drop `opened, synchronize, reopened`, or both workflows would report the same context on every PR; a `closed` run's `report` is skipped (capture's `if` excludes `closed`, reusable line 324), so it posts no conflicting result. The two files then carry the same `with:` block, which can drift.",
    "**Token permissions for the CD-tail reader.** The reads are `GET /repos/{o}/{r}/actions/runs`, `.../actions/workflows/{id}/runs`, `.../actions/runs/{id}/jobs`, `.../actions/jobs/{id}/logs` (GitHub, *Permissions required for GitHub Apps*, section \"Repository permissions for \\\"Actions\\\"\", read), `.../pulls` and `.../pulls/{n}` (section \"Repository permissions for \\\"Pull requests\\\"\", read; `pulls/{n}` also under \"Contents\"), and `.../releases` (section \"Repository permissions for \\\"Contents\\\"\", read). "
    "A `release`-triggered job in release.yml starts from `permissions: {}` (release.yml line 91), and *Workflow syntax* → *Defining access for the GITHUB_TOKEN scopes* says that once any permission is specified \"all of those that are not specified are set to `none`\", so the job must declare `actions: read`, `pull-requests: read` and `contents: read`. Its `GITHUB_SHA` is the tag's commit and `GITHUB_REF` the tag ref (*Events that trigger workflows* → `release`). It cannot measure the CD tail itself: it runs inside the Release run, and published-smoke — the last CD job in 79 of 80 releases — starts after it. "
    "A `workflow_run`-triggered job (published-smoke.yml, workflow-level `permissions: contents: read`, lines 50–51) needs `actions: read` and `pull-requests: read` added; that event's workflow \"is able to access secrets and write tokens, even if the previous workflow was not\", and its `GITHUB_SHA` is the last commit on the default branch, not the release's (*Events that trigger workflows* → `workflow_run`), so the reader must take the triggering run from the event payload, not from `GITHUB_SHA`. Neither token can read the required set: `GET .../branches/{b}/protection/required_status_checks` needs Administration read (*Permissions required for GitHub Apps*, \"Repository permissions for \\\"Administration\\\"\"), and the GITHUB_TOKEN scope list has no `administration` key (*Workflow syntax*, same section) — which is why the snapshot comparison stays the human step's.",
    # local profile caveats
    "**What restoring the Nx cache in CI would save — measured from CI's side, since the local cold/warm profile is MISSING.** On a PR, `check-affected` already leaves out projects a change does not reach, and the projects it keeps miss the cache: " + f"{HITS['current_shape']['touching']['rustBuild']} of {HITS['current_shape']['feature_prs']}" + " current-shape feature PRs (" + f"{HITS['window']['touching']['rustBuild']} of {HITS['window']['feature_prs']}" + " in the window) changed a `rustBuild` input, which every Rust tier hashes, and only " + f"{HITS['window']['feature_prs'] - HITS['window']['touching']['e2eSource']} of {HITS['window']['feature_prs']}" + " left the e2e tier's inputs alone (`out/nx-input-hits.json`). A cache saved by a PR's run cannot be restored by its merge push: GitHub, *Dependency caching* → *Restrictions for accessing a cache*: a pull request's cache \"is created for the merge ref (`refs/pull/.../merge`)\" and \"cannot be restored by the base branch\". So the 19 of 31 merge pushes whose tree was already tested gain nothing from it; skipping them (P1) is what recovers that time. PROJECTION: restoring the Nx cache in CI saves ≈0 s on the median PR's `gate` and `cross` legs; it would replay a re-run of an unchanged commit, which this window does not count.",
    "**Design gaps the finalize should know.** CI exposes no per-target timing: `scripts/nx.sh` writes Nx's output to `.logs/nx.log` and CI never uploads it (job 116660719008's log goes 20:48:46 → 22:05:56 with only `nx: requested targets succeeded`), so the per-target split here is the local profile's; a speedup node that wants CI's own split must surface `.logs/nx.log` or `.nx/cache/run.json` (follow-up drafted). And `onepipeline-stop-verdict:test`'s `cargo nextest run -E binary(stop_verdict)` built every integration binary (e2e, smoke, emitter …) during `just check` on this host (follow-up drafted).",
]

answers = {"spike": "spike-onepipeline-ci", "branch": BRANCH, "harness": harness, "method": method,
           "candidates": candidates, "findings": findings, "visual_changes": []}
json.dump(answers, sys.stdout, indent=1, ensure_ascii=False)
sys.stdout.write("\n")
