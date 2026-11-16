# Git-read measurement record

Preserved experimental branch; nothing here lands. Report:
`authoring:process-launch-speed-2026-11-14-spike-git-reads-report` in
`authoring:process-launch-speed-2026-11-14`, rendered from `spike-report`.

## Fold the hosted evidence

Each side contains `RUN-ATTEMPT/test-PLATFORM/` with `test-profile/`, timestamped
`job.log` and `meta.json`. The command is:

    node scripts/git-reads-compare.mjs compare baseline=DIR batched=DIR native=DIR

It prints run/job identities, checkout/head/workflow SHAs, CPU/core counts,
job/pre-e2e/e2e minutes, Nx selection, cache restore receipts, named main outcomes,
separate diagnostic reruns, means/spread decisions, PR scaling, and Git attempts
and recorded durations by engine verb, read shape and engine-owned read site.
`counts` replaces `compare` for local traces and omits elapsed comparisons.
Shared-host timings are never priced. Missing exit durations stay unknown;
batch-child lifetimes include idle residency and are not wall-time savings.

`evidence/final-readers-comparison.md.gz` is actual command stdout from c7977aa6,
with empty stderr and exit 0. The report names its later identical re-execution
on the finished commit. One quoted Windows batched row is:

    | windows | 2/2 | 110.053 | 100.685 | 9.368 | undecided (inside a side's spread) | 8.43 | 398027 | 358029 | 19365.6 (+192.5 unknown) | 22290.9 (+382.5 unknown) |

The columns and complete named catalogs are in that archive. All six
platform/reader timing comparisons are undecided. Native has the lower observed
means and roughly halves Git starts, but these samples establish no reliable
expected Windows minutes saved. Native macOS includes a five-core M2 point
against the three-core M1 baselines; the report retains this hardware confounder.

## Selection and stopped options

Baseline: 43684231509 attempts 1/2, accepted under c11.0. User decision a62
requires two valid after points/platform. A69 selects the first two valid
chronological jobs before looking at their timing:

- Batched 43753064453: Linux attempts 2/3; macOS 1/2; Windows 1/3.
- Native 43753067613: attempts 1/2 on all three platforms.
- Batched Linux attempt 1 is partial and rejected. Windows attempt 2 is held
  under a84 after an active-path adoption failure. macOS attempt 3 is
  supplementary and excluded.
- Reuse 43789062933 attempt 1 is rejected under a93: Windows check_visibility
  fails main and both retained reruns. Linux/macOS observations are retained,
  with no candidate pricing. No second reuse run.
- Combination is NOT MEASURED/rejected under a94 because it includes unsafe
  reuse. Its native-reader variant would have the better observed reader means;
  no combination saving is inferred. Original reuse/combination valid-point
  quotas are intentionally unmet under that amendment.

Six Windows duplicate-dispatch observations across four journey names, versus
none found on either baseline, remain a shipping risk. Recovered no-answer
journeys qualify under a75; their strict main failures stay separate from own
rerun passes. No-answer checks do not exclude admission/interleaving effects.
The held-adoption audit's 297 matching local answers do not clear the historical
hosted failure. Reuse's stale-memo versus admission-cost cause is unresolved.

Expanded-workload comparison (c20.1). Every job's `ci-job` selection line, on
both sides, selects 37 Linux tasks and 39 macOS and Windows tasks. A src-only
change selects 29 Linux tasks. Within each OS, all 24 jobs select the same task
list, because both sides run workflow blob db2bbe08 (the read-only Nx-cache
trust-step env) and carry the opt-level `Cargo.toml` edit. The pairs are valid,
and they compare a widened workload that includes `github-workflows:test-ci-shape`.
Job minutes therefore exceed a typical PR's. The e2e-tier nextest minutes, the
figures priced here, come from the same `onepipeline-e2e:test` task on both
sides. `evidence/nx-selection-parity.json` gives every job's count, log line
and task-list hash.

## Source and local checks

`ONEVCS_SPIKE_GIT_READS` selects `stock`, `batched`, `native`, `reuse`,
`reuse+batched` or `reuse+native`. Runtime source is 07444653; reader/test source
is 33edc69c. Demonstration runtime content differs only by selector; archived
scoped patches name their comparison base. Engine adapters are unchanged.

Recorded local proof, at the exact commits/commands cited in the report:
17 real-Git differential prototype tests, existing Git 25 and checkout-reader
13 tests, 63 Node command/fixture tests, root clippy, formatting, shellcheck,
Biome and static llmlint validation passed. The six-mode local slice passed
12 e2e and 18 engine-read unit executions. Fresh trace/fold slices precede every
hosted round. Later evidence/docs changes do not alter those code inputs.

The final local judged pass remains RED; a47 caps refinement and requires
remaining proof limits in the report. No green final judged/gate claim is made.
The unchanged vendored pool/maintain.rs:507 clippy finding is excluded under
that ruling. Every own suppression and its reason is inventoried in the report.
Merged Linux coverage is NOT PRODUCED on either side: tests/patch_pin.rs's
vendored patch audit stops test-rest and the merge. The LCOV floor stays 95.74;
no test, justfile or workflow was changed to manufacture coverage.

## Evidence and handoff

Use `gzip -dc` for compressed files. Per-job facts/provenance retain identities,
models, timings, starts, per-verb/shape/site durations, selection, named results
and each restore's hit/miss/time. Host-metadata archives retain all six started
workflow attempts. Lint/budgets receipts are separate. Cache-bearing jobs show
no Actions-cache saves and zero Nx stored entries/bytes; budgets has no
cache-bearing step, and absent Nx telemetry is not inferred as a measured zero.
No cache was deleted.

`final-primary-name-parity.json` compares all 18 primary platform jobs by name;
only the explicitly authorized overwritten Linux loopcost proofs are inferred.
`all-platform-summary-failure-compile-citations.json.gz` preserves numbered
summary/failure/timeout/rerun/compile lines from all 24 platform jobs, including
rejected and supplementary observations. Held-adoption audit artifacts retain
the negative calibration, corrected optimized control and replay limits.

All runs are complete. Demonstration PRs 1633, 1634 and 1653 are CLOSED; their
remote/local branches and owned worktrees are removed. No runs are in flight
and no further CI remains. Session draft PR 1620 stays for lifecycle review.
Follow-ups cover six duplicate-dispatch observations, rejected reuse correctness,
recovered live-note presentation ordering and the frozen shared harness limits.
Nominal adoption costs/releases and remaining production proof obligations are
in the report; no shipping recommendation is made for these prototypes.
