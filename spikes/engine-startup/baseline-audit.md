# Baseline job audit

Source: committed inventory capsules `spikes/launch-inventory/evidence/baseline-{1,2}.facts.json.gz` and their saved timestamped job logs. Run 43684231509 attempts 1 and 2; shared head 36237191db78133b709b3d893ccc11d91d2a5272; checkout merge 37341b0a57e77d18f6db6c11c1b32d7b30fb8dac; workflow db2bbe081a7b9319eb38bd014e72350b389e8b01. This audit was generated locally at source dc83dece; it does not measure performance on this host.

## Attempt 1, lint, job 132721606544

Hosted log: https://github.com/nickderobertis/onepipeline/actions/runs/43684231509/job/132721606544. Job 11.650000 minutes; conclusion failure.

Nx selection: 54 tasks (full catalog):

```text
github-workflows:format-check
github-workflows:lint
github-workflows:syntax-check
human-items-e2e:format-check
human-items-e2e:lint
onepipeline-ci-jobs:format-check
onepipeline-ci-jobs:lint
onepipeline-codex-server:format-check
onepipeline-codex-server:lint
onepipeline-contract:format-check
onepipeline-contract:lint
onepipeline-coverage-keys:format-check
onepipeline-coverage-keys:lint
onepipeline-e2e:format-check
onepipeline-e2e:lint
onepipeline-journal-workload:format-check
onepipeline-journal-workload:lint
onepipeline-narrow-write:format-check
onepipeline-narrow-write:lint
onepipeline-note-journeys:format-check
onepipeline-note-journeys:lint
onepipeline-npm:format-check
onepipeline-npm:lint
onepipeline-pool-workload:format-check
onepipeline-pool-workload:lint
onepipeline-pooled-slot:format-check
onepipeline-pooled-slot:lint
onepipeline-real-smoke:format-check
onepipeline-real-smoke:lint
onepipeline-release-compat:format-check
onepipeline-release-compat:lint
onepipeline-release-journeys:format-check
onepipeline-release-journeys:lint
onepipeline-release:format-check
onepipeline-release:lint
onepipeline-scratch-workload:format-check
onepipeline-scratch-workload:lint
onepipeline-status-read:format-check
onepipeline-status-read:lint
onepipeline-stop-verdict:format-check
onepipeline-stop-verdict:lint
onepipeline-version-contract:format-check
onepipeline-version-contract:lint
onepipeline-version-e2e:format-check
onepipeline-version-e2e:lint
onepipeline-version-package:format-check
onepipeline-version-package:lint
onepipeline:deps-check
onepipeline:format-check
onepipeline:lint
pool-maintenance:format-check
pool-maintenance:lint
report-job:format-check
report-job:lint
```

Cache-related steps: durations span each step to the next timestamped step; “no restore” means no restore result is recorded, not a hit.

| Step | Result | Seconds |
| --- | --- | ---: |
| op-nx/github-cache/start-cache-server@91839c24885842cac86567 | no restore | 0.005 |
| Swatinem/rust-cache@v2.9.2 | partial (restore key) | 62.693 |
| actions/cache/restore@v4 | miss | 0.258 |
| actions/setup-node@v4 | no restore | 3.825 |
| actions/setup-python@v5 | miss | 16.246 |
| astral-sh/setup-uv@v6 | no restore | 2.922 |

Observed Actions cache-save lines: []. Nx store receipt: {"entries": 0, "bytes": 0, "replayed": 0}. The read-only trust-step override suppresses writes; an empty save list alone would not prove that guard.

## Attempt 1, test-macos, job 132721606835

Hosted log: https://github.com/nickderobertis/onepipeline/actions/runs/43684231509/job/132721606835. Job 201.850000 minutes; conclusion failure.

Processor: Apple M1 (Virtual); 3 logical cores.

E2e nextest 90.112450 minutes; Summary [5406.747s] 2121 tests run: 2121 passed (40 slow), 1 skipped. Before the e2e tier: 93.010217 minutes including checkout, compile and earlier tiers; this is not compile-only time.

Compile-finish log lines before this e2e tier (repeated helper builds retained):

No Cargo `Finished ... profile` line precedes this tier in this log (quiet archive compilation); compile-only duration is unavailable. The before-tier time above includes that compilation and is not used to compare sides.

Nx selection: 39 tasks (full catalog):

```text
github-workflows:lint
human-items-e2e:lint
onepipeline-ci-jobs:lint
onepipeline-codex-server:lint
onepipeline-codex-server:test-quick
onepipeline-contract:lint
onepipeline-contract:test-quick
onepipeline-coverage-keys:lint
onepipeline-e2e:lint
onepipeline-e2e:test-quick
onepipeline-journal-workload:lint
onepipeline-narrow-write:lint
onepipeline-narrow-write:test-quick
onepipeline-note-journeys:lint
onepipeline-note-journeys:test-quick
onepipeline-npm:lint
onepipeline-npm:test-launcher
onepipeline-pool-workload:lint
onepipeline-pooled-slot:lint
onepipeline-pooled-slot:test-quick
onepipeline-real-smoke:lint
onepipeline-release-compat:lint
onepipeline-release-journeys:lint
onepipeline-release:lint
onepipeline-scratch-workload:lint
onepipeline-status-read:lint
onepipeline-stop-verdict:lint
onepipeline-version-contract:lint
onepipeline-version-contract:test-quick
onepipeline-version-e2e:lint
onepipeline-version-e2e:test-quick
onepipeline-version-package:lint
onepipeline-version-package:test-quick
onepipeline:install-documented
onepipeline:install-smoke
onepipeline:lint
onepipeline:test-quick
pool-maintenance:lint
report-job:lint
```

Cache-related steps: durations span each step to the next timestamped step; “no restore” means no restore result is recorded, not a hit.

| Step | Result | Seconds |
| --- | --- | ---: |
| op-nx/github-cache/start-cache-server@91839c24885842cac86567 | no restore | 0.064 |
| Swatinem/rust-cache@v2.9.2 | partial (restore key) | 40.504 |
| actions/setup-node@v4 | no restore | 0.998 |
| actions/setup-python@v5 | no restore | 0.169 |
| astral-sh/setup-uv@v6 | no restore | 2.871 |
| actions/cache/restore@v4 | hit | 1.427 |

Observed Actions cache-save lines: []. Nx store receipt: {"entries": 0, "bytes": 0, "replayed": 0}. The read-only trust-step override suppresses writes; an empty save list alone would not prove that guard.

The capsule's `names` array is the full 2121-name e2e profile catalog for this point; `summary` supplies the matching passed/run counts. It remains the by-name comparison input, not a substitute for after-run pass evidence.

## Attempt 1, test-windows, job 132721606851

Hosted log: https://github.com/nickderobertis/onepipeline/actions/runs/43684231509/job/132721606851. Job 144.700000 minutes; conclusion failure.

Processor: AMD EPYC 9V45 96-Core Processor; 4 logical cores.

E2e nextest 87.112333 minutes; Summary [5226.740s] 1832 tests run: 1832 passed (54 slow), 1 skipped. Before the e2e tier: 57.185133 minutes including checkout, compile and earlier tiers; this is not compile-only time.

Compile-finish log lines before this e2e tier (repeated helper builds retained):

No Cargo `Finished ... profile` line precedes this tier in this log (quiet archive compilation); compile-only duration is unavailable. The before-tier time above includes that compilation and is not used to compare sides.

Nx selection: 39 tasks (full catalog):

```text
github-workflows:lint
human-items-e2e:lint
onepipeline-ci-jobs:lint
onepipeline-codex-server:lint
onepipeline-codex-server:test-quick
onepipeline-contract:lint
onepipeline-contract:test-quick
onepipeline-coverage-keys:lint
onepipeline-e2e:lint
onepipeline-e2e:test-quick
onepipeline-journal-workload:lint
onepipeline-narrow-write:lint
onepipeline-narrow-write:test-quick
onepipeline-note-journeys:lint
onepipeline-note-journeys:test-quick
onepipeline-npm:lint
onepipeline-npm:test-launcher
onepipeline-pool-workload:lint
onepipeline-pooled-slot:lint
onepipeline-pooled-slot:test-quick
onepipeline-real-smoke:lint
onepipeline-release-compat:lint
onepipeline-release-journeys:lint
onepipeline-release:lint
onepipeline-scratch-workload:lint
onepipeline-status-read:lint
onepipeline-stop-verdict:lint
onepipeline-version-contract:lint
onepipeline-version-contract:test-quick
onepipeline-version-e2e:lint
onepipeline-version-e2e:test-quick
onepipeline-version-package:lint
onepipeline-version-package:test-quick
onepipeline:install-documented
onepipeline:install-smoke
onepipeline:lint
onepipeline:test-quick
pool-maintenance:lint
report-job:lint
```

Cache-related steps: durations span each step to the next timestamped step; “no restore” means no restore result is recorded, not a hit.

| Step | Result | Seconds |
| --- | --- | ---: |
| op-nx/github-cache/start-cache-server@91839c24885842cac86567 | no restore | 0.014 |
| Swatinem/rust-cache@v2.9.2 | partial (restore key) | 66.629 |
| actions/setup-node@v4 | no restore | 3.617 |
| actions/setup-python@v5 | no restore | 0.135 |
| astral-sh/setup-uv@v6 | no restore | 2.191 |
| actions/cache/restore@v4 | hit | 1.364 |

Observed Actions cache-save lines: []. Nx store receipt: {"entries": 0, "bytes": 0, "replayed": 0}. The read-only trust-step override suppresses writes; an empty save list alone would not prove that guard.

The capsule's `names` array is the full 1832-name e2e profile catalog for this point; `summary` supplies the matching passed/run counts. It remains the by-name comparison input, not a substitute for after-run pass evidence.

## Attempt 1, test-linux, job 132721606859

Hosted log: https://github.com/nickderobertis/onepipeline/actions/runs/43684231509/job/132721606859. Job 136.300000 minutes; conclusion failure.

Processor: Intel(R) Xeon(R) 6975P-C; 4 logical cores.

E2e nextest 37.919033 minutes; Summary [2275.142s] 2155 tests run: 2155 passed (23 slow), 1 skipped. Before the e2e tier: 96.330467 minutes including checkout, compile and earlier tiers; this is not compile-only time.

Compile-finish log lines before this e2e tier (repeated helper builds retained):

```text
5761: 2026-11-15T07:27:07.0560482Z     Finished `release` profile [optimized] target(s) in 21m 04s
```

Merged coverage: not produced. Under rulings c7.0/c9.1, the patch audit fails test-rest, blocking the dependent test target/coverage merge. The LCOV floor remains 95.74. No workaround changes tests, workflow or justfile. Log evidence:

```text
2454: 2026-11-15T06:45:04.6856921Z         FAIL [   0.008s] (1370/1443) onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision
2535: 2026-11-15T06:45:20.5641806Z         FAIL [   0.008s] (1370/1443) onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision
9705: 2026-11-15T08:28:05.7754584Z Tasks not run because their dependencies failed or --nx-bail=true:
9707: 2026-11-15T08:28:05.7754960Z - onepipeline:test
9711: 2026-11-15T08:28:05.7755481Z - onepipeline:test-rest
9840: 2026-11-15T08:28:19.1561809Z         FAIL [   0.007s] (1/1) onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision
9864: 2026-11-15T08:28:19.1611628Z rerun-failed: re-run 1 of 3: onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision FAILED
9872: 2026-11-15T08:28:24.7728280Z         FAIL [   0.005s] (1/1) onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision
9896: 2026-11-15T08:28:24.7781993Z rerun-failed: re-run 2 of 3: onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision FAILED
9904: 2026-11-15T08:28:33.7252132Z         FAIL [   0.005s] (1/1) onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision
9928: 2026-11-15T08:28:33.7311458Z rerun-failed: re-run 3 of 3: onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision FAILED
9931: 2026-11-15T08:28:33.7313911Z   onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision: failed 3 of 3 re-runs; fails every time on this build, which reads as a regression: reproduce it with cargo nextest run -E 'binary_id(=onepipeline::patch_pin) and test(=every_patched_member_of_one_repository_comes_from_one_revision)' and fix it
```

Nx selection: 37 tasks (full catalog):

```text
github-workflows:budgets
github-workflows:test
github-workflows:test-ci-shape
human-items-e2e:build
human-items-e2e:test
onepipeline-ci-jobs:test
onepipeline-codex-server:test
onepipeline-contract:test
onepipeline-coverage-keys:test
onepipeline-e2e:test
onepipeline-narrow-write:test
onepipeline-note-journeys:test
onepipeline-npm:test
onepipeline-pooled-slot:test
onepipeline-real-smoke:smoke
onepipeline-release-compat:release-compat
onepipeline-release:test
onepipeline-status-read:build
onepipeline-status-read:test
onepipeline-stop-verdict:test
onepipeline-stop-verdict:test-build
onepipeline-version-contract:test
onepipeline-version-e2e:test
onepipeline-version-package:test
onepipeline:budgets
onepipeline:build
onepipeline:coverage-clean
onepipeline:doc
onepipeline:install-documented
onepipeline:install-smoke
onepipeline:msrv
onepipeline:test
onepipeline:test-rest
onepipeline:wheel-smoke
pool-maintenance:test
report-job:budgets
report-job:test
```

Cache-related steps: durations span each step to the next timestamped step; “no restore” means no restore result is recorded, not a hit.

| Step | Result | Seconds |
| --- | --- | ---: |
| op-nx/github-cache/start-cache-server@91839c24885842cac86567 | no restore | 0.004 |
| Swatinem/rust-cache@v2.9.2 | partial (restore key) | 67.867 |
| actions/setup-node@v4 | no restore | 3.048 |
| actions/setup-python@v5 | miss | 6.016 |
| astral-sh/setup-uv@v6 | no restore | 2.316 |
| actions/cache/restore@v4 | hit | 2.499 |

Observed Actions cache-save lines: []. Nx store receipt: {"entries": 0, "bytes": 0, "replayed": 0}. The read-only trust-step override suppresses writes; an empty save list alone would not prove that guard.

The capsule's `names` array is the full 2155-name e2e profile catalog for this point; `summary` supplies the matching passed/run counts. It remains the by-name comparison input, not a substitute for after-run pass evidence.

## Attempt 1, budgets, job 132798588668

Hosted log: https://github.com/nickderobertis/onepipeline/actions/runs/43684231509/job/132798588668. Job 1.066667 minutes; conclusion success.

Cache-related steps: durations span each step to the next timestamped step; “no restore” means no restore result is recorded, not a hit.

| Step | Result | Seconds |
| --- | --- | ---: |

Observed Actions cache-save lines: []. Nx store receipt: null. The read-only trust-step override suppresses writes; an empty save list alone would not prove that guard.

## Attempt 2, test-linux, job 132799383400

Hosted log: https://github.com/nickderobertis/onepipeline/actions/runs/43684231509/job/132799383400. Job 171.466667 minutes; conclusion failure.

Processor: INTEL(R) XEON(R) PLATINUM 8573C; 4 logical cores.

E2e nextest 41.055683 minutes; Summary [2463.341s] 2155 tests run: 2155 passed (24 slow), 1 skipped. Before the e2e tier: 125.959467 minutes including checkout, compile and earlier tiers; this is not compile-only time.

Compile-finish log lines before this e2e tier (repeated helper builds retained):

```text
5876: 2026-11-15T13:19:12.5169187Z     Finished `release` profile [optimized] target(s) in 27m 56s
```

Merged coverage: not produced. Under rulings c7.0/c9.1, the patch audit fails test-rest, blocking the dependent test target/coverage merge. The LCOV floor remains 95.74. No workaround changes tests, workflow or justfile. Log evidence:

```text
2613: 2026-11-15T12:23:52.4494665Z ..        FAIL [   0.026s] (1370/1443) onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision
2658: 2026-11-15T12:24:00.5021063Z         FAIL [   0.026s] (1370/1443) onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision
9158: 2026-11-15T14:29:19.4337182Z Tasks not run because their dependencies failed or --nx-bail=true:
9160: 2026-11-15T14:29:19.4337984Z - onepipeline:test
9167: 2026-11-15T14:29:19.4340998Z - onepipeline:test-rest
9302: 2026-11-15T14:30:15.8650076Z         FAIL [   0.005s] ( 3/11) onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision
9339: 2026-11-15T14:30:15.8836527Z rerun-failed: re-run 1 of 3: onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision FAILED
9354: 2026-11-15T14:30:58.3983986Z         FAIL [   0.007s] ( 3/11) onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision
9391: 2026-11-15T14:30:58.4180362Z rerun-failed: re-run 2 of 3: onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision FAILED
9406: 2026-11-15T14:31:36.8901911Z         FAIL [   0.007s] ( 3/11) onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision
9443: 2026-11-15T14:31:36.9083789Z rerun-failed: re-run 3 of 3: onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision FAILED
9455: 2026-11-15T14:31:36.9209555Z   onepipeline::patch_pin every_patched_member_of_one_repository_comes_from_one_revision: failed 3 of 3 re-runs; fails every time on this build, which reads as a regression: reproduce it with cargo nextest run -E 'binary_id(=onepipeline::patch_pin) and test(=every_patched_member_of_one_repository_comes_from_one_revision)' and fix it
```

Nx selection: 37 tasks (full catalog):

```text
github-workflows:budgets
github-workflows:test
github-workflows:test-ci-shape
human-items-e2e:build
human-items-e2e:test
onepipeline-ci-jobs:test
onepipeline-codex-server:test
onepipeline-contract:test
onepipeline-coverage-keys:test
onepipeline-e2e:test
onepipeline-narrow-write:test
onepipeline-note-journeys:test
onepipeline-npm:test
onepipeline-pooled-slot:test
onepipeline-real-smoke:smoke
onepipeline-release-compat:release-compat
onepipeline-release:test
onepipeline-status-read:build
onepipeline-status-read:test
onepipeline-stop-verdict:test
onepipeline-stop-verdict:test-build
onepipeline-version-contract:test
onepipeline-version-e2e:test
onepipeline-version-package:test
onepipeline:budgets
onepipeline:build
onepipeline:coverage-clean
onepipeline:doc
onepipeline:install-documented
onepipeline:install-smoke
onepipeline:msrv
onepipeline:test
onepipeline:test-rest
onepipeline:wheel-smoke
pool-maintenance:test
report-job:budgets
report-job:test
```

Cache-related steps: durations span each step to the next timestamped step; “no restore” means no restore result is recorded, not a hit.

| Step | Result | Seconds |
| --- | --- | ---: |
| op-nx/github-cache/start-cache-server@91839c24885842cac86567 | no restore | 0.007 |
| Swatinem/rust-cache@v2.9.2 | partial (restore key) | 66.746 |
| actions/setup-node@v4 | no restore | 1.071 |
| actions/setup-python@v5 | miss | 7.100 |
| astral-sh/setup-uv@v6 | no restore | 2.732 |
| actions/cache/restore@v4 | hit | 1.182 |

Observed Actions cache-save lines: []. Nx store receipt: {"entries": 0, "bytes": 0, "replayed": 0}. The read-only trust-step override suppresses writes; an empty save list alone would not prove that guard.

The capsule's `names` array is the full 2155-name e2e profile catalog for this point; `summary` supplies the matching passed/run counts. It remains the by-name comparison input, not a substitute for after-run pass evidence.

## Attempt 2, lint, job 132799383572

Hosted log: https://github.com/nickderobertis/onepipeline/actions/runs/43684231509/job/132799383572. Job 27.366667 minutes; conclusion failure.

Nx selection: 54 tasks (full catalog):

```text
github-workflows:format-check
github-workflows:lint
github-workflows:syntax-check
human-items-e2e:format-check
human-items-e2e:lint
onepipeline-ci-jobs:format-check
onepipeline-ci-jobs:lint
onepipeline-codex-server:format-check
onepipeline-codex-server:lint
onepipeline-contract:format-check
onepipeline-contract:lint
onepipeline-coverage-keys:format-check
onepipeline-coverage-keys:lint
onepipeline-e2e:format-check
onepipeline-e2e:lint
onepipeline-journal-workload:format-check
onepipeline-journal-workload:lint
onepipeline-narrow-write:format-check
onepipeline-narrow-write:lint
onepipeline-note-journeys:format-check
onepipeline-note-journeys:lint
onepipeline-npm:format-check
onepipeline-npm:lint
onepipeline-pool-workload:format-check
onepipeline-pool-workload:lint
onepipeline-pooled-slot:format-check
onepipeline-pooled-slot:lint
onepipeline-real-smoke:format-check
onepipeline-real-smoke:lint
onepipeline-release-compat:format-check
onepipeline-release-compat:lint
onepipeline-release-journeys:format-check
onepipeline-release-journeys:lint
onepipeline-release:format-check
onepipeline-release:lint
onepipeline-scratch-workload:format-check
onepipeline-scratch-workload:lint
onepipeline-status-read:format-check
onepipeline-status-read:lint
onepipeline-stop-verdict:format-check
onepipeline-stop-verdict:lint
onepipeline-version-contract:format-check
onepipeline-version-contract:lint
onepipeline-version-e2e:format-check
onepipeline-version-e2e:lint
onepipeline-version-package:format-check
onepipeline-version-package:lint
onepipeline:deps-check
onepipeline:format-check
onepipeline:lint
pool-maintenance:format-check
pool-maintenance:lint
report-job:format-check
report-job:lint
```

Cache-related steps: durations span each step to the next timestamped step; “no restore” means no restore result is recorded, not a hit.

| Step | Result | Seconds |
| --- | --- | ---: |
| op-nx/github-cache/start-cache-server@91839c24885842cac86567 | no restore | 0.004 |
| Swatinem/rust-cache@v2.9.2 | partial (restore key) | 56.766 |
| actions/cache/restore@v4 | miss | 0.267 |
| actions/setup-node@v4 | no restore | 0.534 |
| actions/setup-python@v5 | miss | 17.750 |
| astral-sh/setup-uv@v6 | no restore | 2.607 |

Observed Actions cache-save lines: []. Nx store receipt: {"entries": 0, "bytes": 0, "replayed": 0}. The read-only trust-step override suppresses writes; an empty save list alone would not prove that guard.

## Attempt 2, test-windows, job 132799383582

Hosted log: https://github.com/nickderobertis/onepipeline/actions/runs/43684231509/job/132799383582. Job 251.516667 minutes; conclusion failure.

Processor: AMD EPYC 7763 64-Core Processor; 4 logical cores.

E2e nextest 132.993667 minutes; Summary [7979.620s] 1832 tests run: 1832 passed (134 slow), 1 skipped. Before the e2e tier: 92.933400 minutes including checkout, compile and earlier tiers; this is not compile-only time.

Compile-finish log lines before this e2e tier (repeated helper builds retained):

No Cargo `Finished ... profile` line precedes this tier in this log (quiet archive compilation); compile-only duration is unavailable. The before-tier time above includes that compilation and is not used to compare sides.

Nx selection: 39 tasks (full catalog):

```text
github-workflows:lint
human-items-e2e:lint
onepipeline-ci-jobs:lint
onepipeline-codex-server:lint
onepipeline-codex-server:test-quick
onepipeline-contract:lint
onepipeline-contract:test-quick
onepipeline-coverage-keys:lint
onepipeline-e2e:lint
onepipeline-e2e:test-quick
onepipeline-journal-workload:lint
onepipeline-narrow-write:lint
onepipeline-narrow-write:test-quick
onepipeline-note-journeys:lint
onepipeline-note-journeys:test-quick
onepipeline-npm:lint
onepipeline-npm:test-launcher
onepipeline-pool-workload:lint
onepipeline-pooled-slot:lint
onepipeline-pooled-slot:test-quick
onepipeline-real-smoke:lint
onepipeline-release-compat:lint
onepipeline-release-journeys:lint
onepipeline-release:lint
onepipeline-scratch-workload:lint
onepipeline-status-read:lint
onepipeline-stop-verdict:lint
onepipeline-version-contract:lint
onepipeline-version-contract:test-quick
onepipeline-version-e2e:lint
onepipeline-version-e2e:test-quick
onepipeline-version-package:lint
onepipeline-version-package:test-quick
onepipeline:install-documented
onepipeline:install-smoke
onepipeline:lint
onepipeline:test-quick
pool-maintenance:lint
report-job:lint
```

Cache-related steps: durations span each step to the next timestamped step; “no restore” means no restore result is recorded, not a hit.

| Step | Result | Seconds |
| --- | --- | ---: |
| op-nx/github-cache/start-cache-server@91839c24885842cac86567 | no restore | 0.023 |
| Swatinem/rust-cache@v2.9.2 | partial (restore key) | 55.112 |
| actions/setup-node@v4 | no restore | 7.319 |
| actions/setup-python@v5 | no restore | 0.228 |
| astral-sh/setup-uv@v6 | no restore | 2.834 |
| actions/cache/restore@v4 | hit | 1.104 |

Observed Actions cache-save lines: []. Nx store receipt: {"entries": 0, "bytes": 0, "replayed": 0}. The read-only trust-step override suppresses writes; an empty save list alone would not prove that guard.

The capsule's `names` array is the full 1832-name e2e profile catalog for this point; `summary` supplies the matching passed/run counts. It remains the by-name comparison input, not a substitute for after-run pass evidence.

## Attempt 2, test-macos, job 132799383592

Hosted log: https://github.com/nickderobertis/onepipeline/actions/runs/43684231509/job/132799383592. Job 198.550000 minutes; conclusion failure.

Processor: Apple M1 (Virtual); 3 logical cores.

E2e nextest 94.377033 minutes; Summary [5662.622s] 2121 tests run: 2121 passed (41 slow), 1 skipped. Before the e2e tier: 85.509383 minutes including checkout, compile and earlier tiers; this is not compile-only time.

Compile-finish log lines before this e2e tier (repeated helper builds retained):

No Cargo `Finished ... profile` line precedes this tier in this log (quiet archive compilation); compile-only duration is unavailable. The before-tier time above includes that compilation and is not used to compare sides.

Nx selection: 39 tasks (full catalog):

```text
github-workflows:lint
human-items-e2e:lint
onepipeline-ci-jobs:lint
onepipeline-codex-server:lint
onepipeline-codex-server:test-quick
onepipeline-contract:lint
onepipeline-contract:test-quick
onepipeline-coverage-keys:lint
onepipeline-e2e:lint
onepipeline-e2e:test-quick
onepipeline-journal-workload:lint
onepipeline-narrow-write:lint
onepipeline-narrow-write:test-quick
onepipeline-note-journeys:lint
onepipeline-note-journeys:test-quick
onepipeline-npm:lint
onepipeline-npm:test-launcher
onepipeline-pool-workload:lint
onepipeline-pooled-slot:lint
onepipeline-pooled-slot:test-quick
onepipeline-real-smoke:lint
onepipeline-release-compat:lint
onepipeline-release-journeys:lint
onepipeline-release:lint
onepipeline-scratch-workload:lint
onepipeline-status-read:lint
onepipeline-stop-verdict:lint
onepipeline-version-contract:lint
onepipeline-version-contract:test-quick
onepipeline-version-e2e:lint
onepipeline-version-e2e:test-quick
onepipeline-version-package:lint
onepipeline-version-package:test-quick
onepipeline:install-documented
onepipeline:install-smoke
onepipeline:lint
onepipeline:test-quick
pool-maintenance:lint
report-job:lint
```

Cache-related steps: durations span each step to the next timestamped step; “no restore” means no restore result is recorded, not a hit.

| Step | Result | Seconds |
| --- | --- | ---: |
| op-nx/github-cache/start-cache-server@91839c24885842cac86567 | no restore | 0.042 |
| Swatinem/rust-cache@v2.9.2 | partial (restore key) | 41.763 |
| actions/setup-node@v4 | no restore | 0.683 |
| actions/setup-python@v5 | no restore | 0.156 |
| astral-sh/setup-uv@v6 | no restore | 2.149 |
| actions/cache/restore@v4 | hit | 2.098 |

Observed Actions cache-save lines: []. Nx store receipt: {"entries": 0, "bytes": 0, "replayed": 0}. The read-only trust-step override suppresses writes; an empty save list alone would not prove that guard.

The capsule's `names` array is the full 2121-name e2e profile catalog for this point; `summary` supplies the matching passed/run counts. It remains the by-name comparison input, not a substitute for after-run pass evidence.

## Attempt 2, budgets, job 132866973729

Hosted log: https://github.com/nickderobertis/onepipeline/actions/runs/43684231509/job/132866973729. Job 0.700000 minutes; conclusion success.

Cache-related steps: durations span each step to the next timestamped step; “no restore” means no restore result is recorded, not a hit.

| Step | Result | Seconds |
| --- | --- | ---: |

Observed Actions cache-save lines: []. Nx store receipt: null. The read-only trust-step override suppresses writes; an empty save list alone would not prove that guard.
