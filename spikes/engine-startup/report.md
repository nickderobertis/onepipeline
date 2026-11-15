# Spike `spike-engine-startup`

- **Branch:** `nick/process-launch-speed-2026-11-14/spike-engine-startup`

## Harness

The branch carries scripts/engine-startup.mjs, engine-startup-cpu.py, engine-startup-profile.sh and engine-startup-verify.sh. Fold a demonstration with `node scripts/engine-startup.mjs fold before/1=BASE_JOB contributors/1=AFTER_JOB`. It prints per-job start/adopt completed trace means and e2e minutes, then per-side means, spreads, differences and typical-PR estimates. See spikes/engine-startup/README.md for profiling and equivalence commands.

## Method

IN PROGRESS. Ruling c13.0 replaces hosted sampling with local attribution and existing hosted launch-trace/tier measurements. Ruling c11.1 accepts run 43684231509 attempts 1 and 2 as the before side. Rulings c7.0/c9.1 require reporting Linux merged coverage as not produced because the vendored patch audit fails test-rest and Nx skips coverage merge; floor remains 95.74 LCOV.

Handoff: no hosted runs started by this dispatch; none in flight. Local prototype and checks are underway. Demonstration capacity is held by child-waits under c11.1; wait for a slot, then start round peers together. Remaining: source-identified local sampling on current debug/release builds, hosted after rounds, outcomes and cache audit, final judged lint and comparison. No performance saving is claimed yet.



### Baseline fold checkpoint (source 8f51bf37)

The following output was read from `/home/whoop/ai-orchestrator/runs/brief-process-launch-speed-2026-11-14-spikes/scratch/3469976-4/baseline-finished-8f51bf37.txt`, produced on commit 8f51bf37 by `node scripts/engine-startup.mjs fold` with six `before/POINT/OS=/home/whoop/ai-orchestrator/runs/brief-process-launch-speed-2026-11-14-spikes/scratch/1794352-0/runs/baseline-POINT/test-OS` arguments (POINT=1,2; OS=linux,macos,windows). These are trace lifetimes, not unavailable per-verb test-profile program records. The command was run after that commit; later validation-only source repairs do not establish a new measurement. A finished-source fold remains owed.

Hosted source: run 43684231509 attempts 1 and 2, head 36237191db78133b709b3d893ccc11d91d2a5272, GITHUB_SHA 37341b0a57e77d18f6db6c11c1b32d7b30fb8dac; executed workflow blob db2bbe081a7b9319eb38bd014e72350b389e8b01. Baseline facts and full test-name catalogs are committed under spikes/launch-inventory/evidence/baseline-{1,2}.facts.json.gz.

| Attempt | Job | Job id | Artifact id(s) | Job min | E2e min |

| --- | --- | --- | --- | ---: | ---: |

| 1 | test-macos | 132721606835 | 14159457280 | 201.850000 | 90.112450 |

| 1 | test-windows | 132721606851 | 14153829104 | 144.700000 | 87.112333 |

| 1 | test-linux | 132721606859 | 14153153329 | 136.300000 | 37.919033 |

| 2 | test-linux | 132799383400 | 14165992711 | 171.466667 | 41.055683 |

| 2 | test-windows | 132799383582 | 14167358776 | 251.516667 | 132.993667 |

| 2 | test-macos | 132799383592 | 14166951348 | 198.550000 | 94.377033 |

```text

before/1/linux (linux)
runner: Intel(R) Xeon(R) 6975P-C; cores: 4
e2e minutes: 37.919033
start: 820.300928 ms mean; 2351 completed / 2371 observed
adopt: 1396.245420 ms mean; 340 completed / 345 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2155; names digest: 83531b68a69ebaf7; scope: {"id":"21fd59ea-9add-43aa-abae-0fd219c703a7","start":1794728919.828,"end":1794731194.969,"seconds":2275.142,"run":2155,"passed":2155,"line":"Summary [2275.142s] 2155 tests run: 2155 passed (23 slow), 1 skipped","records":174,"starts":56,"os":0,"otherProfiles":1}

before/2/linux (linux)
runner: INTEL(R) XEON(R) PLATINUM 8573C; cores: 4
e2e minutes: 41.055683
start: 891.417304 ms mean; 2351 completed / 2371 observed
adopt: 1476.363930 ms mean; 340 completed / 346 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2155; names digest: 83531b68a69ebaf7; scope: {"id":"2077affc-b3f8-4207-9d22-fc387055d3ae","start":1794750371.568,"end":1794752834.909,"seconds":2463.341,"run":2155,"passed":2155,"line":"Summary [2463.341s] 2155 tests run: 2155 passed (24 slow), 1 skipped","records":19,"starts":5,"os":0,"otherProfiles":1}

before/1/macos (macos)
runner: Apple M1 (Virtual); cores: 3
e2e minutes: 90.112450
start: 1847.244006 ms mean; 2328 completed / 2348 observed
adopt: 2601.639218 ms mean; 349 completed / 354 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2121; names digest: 224faf8a6439b9ed; scope: {"id":"b0545ab0-aab0-419c-bd08-8def1645ea5d","start":1794736078.613,"end":1794741485.336,"seconds":5406.747,"run":2121,"passed":2121,"line":"Summary [5406.747s] 2121 tests run: 2121 passed (40 slow), 1 skipped","records":0,"starts":0,"os":0,"otherProfiles":0}

before/2/macos (macos)
runner: Apple M1 (Virtual); cores: 3
e2e minutes: 94.377033
start: 1978.519577 ms mean; 2328 completed / 2348 observed
adopt: 2796.896188 ms mean; 349 completed / 354 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2121; names digest: 224faf8a6439b9ed; scope: {"id":"be48b42d-fa3b-44c3-8a1c-13e5ef37bf20","start":1794748917.563,"end":1794754580.181,"seconds":5662.622,"run":2121,"passed":2121,"line":"Summary [5662.622s] 2121 tests run: 2121 passed (41 slow), 1 skipped","records":0,"starts":0,"os":0,"otherProfiles":0}

before/1/windows (windows)
runner: AMD EPYC 9V45 96-Core Processor                ; cores: 4
e2e minutes: 87.112333
start: 3375.080794 ms mean; 2083 completed / 2089 observed
adopt: 4658.311521 ms mean; 302 completed / 305 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 1832; names digest: a91139c2b34c8c42; scope: {"id":"60af6ffa-12c2-429c-848b-1cffb3033ad3","start":1794726570.108,"end":1794731796.94,"seconds":5226.74,"run":1832,"passed":1832,"line":"Summary [5226.740s] 1832 tests run: 1832 passed (54 slow), 1 skipped","records":0,"starts":0,"os":0,"otherProfiles":0}

before/2/windows (windows)
runner: AMD EPYC 7763 64-Core Processor                ; cores: 4
e2e minutes: 132.993667
start: 5663.412835 ms mean; 2083 completed / 2089 observed
adopt: 8031.223044 ms mean; 302 completed / 305 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 1832; names digest: a91139c2b34c8c42; scope: {"id":"68cf7a38-6e72-43ac-ba8c-0d9e9a850d56","start":1794748390.004,"end":1794756369.667,"seconds":7979.62,"run":1832,"passed":1832,"line":"Summary [7979.620s] 1832 tests run: 1832 passed (134 slow), 1 skipped","records":0,"starts":0,"os":0,"otherProfiles":0}

before/linux: e2e mean 39.487358 min; range 37.919033..41.055683; n=2

before/macos: e2e mean 92.244742 min; range 90.112450..94.377033; n=2

before/windows: e2e mean 110.053000 min; range 87.112333..132.993667; n=2


```

For example the command prints Windows baseline e2e mean 110.053000 minutes (two accepted points). This is a measured baseline, not a saving. All source and job citations above apply to every number in that quoted output.

### Prototype, proof and adoption (in progress)

8f51bf37 commits the first contributor-prefilter bypass and harness. Its initial local checks: `cargo check --offline --bin onepipeline` passed; `ONEPIPELINE_STARTUP_CUT=contributors just test-e2e` with the two exact driver journey names in scratch/e2e-cut.log passed both; `bash scripts/engine-startup-verify.sh` passed 6,588 public-API inputs (find, hits, replace, locator screening). Those pre-commit runs exercise the same source committed in 8f51bf37; they are not a claim about the later uncommitted selector/input repairs. Five initial fold tests and the shared reader’s 46 tests passed. The new API equivalence check was observed failing on an intentional mutation returning no matches on the cut path: SECRET yielded baseline [0], cut []; the mutation was restored.

The first judged diff lint, history 20261115T171137Z-8baa9, failed seven rules. The working repair validates the selector, shares its names with the verifier, validates profile tables and cycles, adds nonzero/spread/missing-profile and malformed-profile journeys, and documents command exits. Final judged status is pending.

Cut implementation removes calls to privateterms::normalize::Contributors::new and Contributors::possible in the selected engine processes, including the global source-map initializer and per-matcher mask-table allocation. Full Matcher::scan normalization, candidate stability, folded/exact automata, public-phrase exemptions, hits span mapping, replace and locator screening remain exercised. Added experimental branches include StartupCut::read baseline/contributors/invalid input and the optional prefilter branch; no source function is claimed covered by merged coverage. A long-lived matcher with many negative Unicode texts may lose throughput by doing more full scans; this prototype alone does not justify removing that filter in production.

Production effort if supported by hosted results: one privateterms implementation/validation dispatch, one consumer lock/adoption dispatch in onepipeline. privateterms would release a patch for a perf-only compatible change. onevcs’s privateterms requirement is ^0.1.2, and onetaskgraph-core’s is ^0.1.0, so both admit a privateterms patch without a release of either consumer library; onepipeline updates the privateterms lock entry and releases a perf patch. No change to a serialized schema is proposed. Existing e2e journeys prove the same user behavior; they exercise less of the contributor prefilter and more of the full scanner on its formerly rejected candidates.

Linux merged line coverage is not produced on the baseline sides under c7.0/c9.1: the path-patch audit fails test-rest and Nx skips the coverage merge. Unchanged floor: 95.74 LCOV. Exact log excerpts and the after side remain to be included with the hosted comparison.

### Current handoff

Runs started by this dispatch: none. Runs in flight: none. Session draft: https://github.com/nickderobertis/onepipeline/pull/1621. Baseline means above are read from inherited artifacts; no hosted quota was consumed to remeasure them. Remaining: final local debug/release samples, capacity for three after runs/platform, outcome/cache/coverage evidence, final-source fold and judged lint. No expected saving is claimed before after measurements.

Manager a37 accepts the identically computed trace means on both sides in place of unavailable per-verb test-profile keys, with completed/observed counts and exclusions as printed.


## Candidate budgets

This spike measured no candidate budget itself.

## Design findings

- The baseline test-profile programs are keyed by executable, not by verb. Manager question c-c079002ebedf688e823b2c9cea9a6c46 asks whether completed launch-trace start/adopt lifetimes may replace unavailable per-verb profile means on both sides; these exclude process creation.
