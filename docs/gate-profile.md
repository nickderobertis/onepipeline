# Where `just gate`'s time goes

A ranking of the gate's costs, measured once. It is a snapshot. Re-measure before you act on
a figure; don't edit one in place.

**Commit measured:** `072b17cd` (#1701, which carries `just gate-profile` and the span
records). **Host:** 14 CPUs, aarch64 Linux, a shared host on which other dispatches run, so
every figure below carries the 1-minute load it was taken under. CPU (user + system) is the
figure that load disturbs least; compare on it first.

## How each figure was taken

| figure | command | when (UTC), load |
| --- | --- | --- |
| gate profile | `just gate-profile` (recipe `gate`, fresh Nx cache), report at `target/gate-profile/<stamp>/report.{md,json}` | 2026-11-18 18:48–01:20, mean 17.5 (0.5–47.7) |
| e2e tier records | the instrumented tier's per-test records, `target/test-profile/e2e/`, folded by `node scripts/test-profile-report.mjs --out FILE linux=target/test-profile/e2e` | 2026-11-19 01:24–01:53, about 9.9 |
| module, plain | `ONEPIPELINE_TEST_PROFILE_SPANS=1 just test-e2e 'test(/^readjust::/)'` (and `lifecycle`), then the same report script over the records | 2026-11-19 10:30–10:37, 2.2–8.2 |
| module, instrumented | `ONEPIPELINE_TEST_PROFILE_SPANS=1 just _tier-test onepipeline-e2e-readjust 'binary(e2e) and test(/^readjust::/)'` (the e2e tier's own recipe, narrowed) | 10:40–10:46, 1.6–2.5 |
| single verbs | `hyperfine -N` over `target/{debug,release}/onepipeline` with `ONEPIPELINE_RUNS_DIR` set to a copy of `tests/recorded/run-root` | 10:37, 1.0–1.2 |
| sampling | `perf record -F 999 -g` over 200 `goals` runs, and `-F 499 -g` over the `readjust` module; `kernel.perf_event_paranoid` was 1 only for those runs and read 4 afterwards | 10:38–10:39, 1.7–1.9 |

Wall and CPU for every command come from `/usr/bin/time` (the children's `getrusage`).

## The gate profile, and what spoiled it

| | wall | CPU | user | system | load |
| --- | ---: | ---: | ---: | ---: | --- |
| whole run (exit 1) | 23,487 s | 201,888 s | 94,249 s | 107,638 s | 17.5 |
| `fmt-check` | 9.3 s | 18.6 s | | | 0.75 |
| `lint` | 158 s | 178 s | | | 4.4 |
| `test` (exit 1) | 23,320 s | 201,691 s | 94,089 s | 107,602 s | |

The report's phase table leaves out `test` and `gate`, which `phases.jsonl` records. The `test`
row above comes from that file. `lint-llm-diff` never ran, because `test` failed first.

The critical path is 22,514 s: `format-check` 3.8 s, then `narrow-write:lint` 87 s, then
`onepipeline:build` 9,878 s (load 36.5), then `onepipeline-npm:test` 12,545 s (load 10.3).

Three things make the whole-run figures an overstatement of a real gate:

- **Profiler shim loops.** `coverage-cache:test` (9,149 s wall, 47,368 s CPU) and `npm:test`
  (12,545 s, 51,091 s CPU) each held processes in an exec loop between `gate-profile/shims`
  and a test's own fake `cargo` or `just`. That loop is a defect of the profiler, not of the
  gate. It accounts for 48,524 s of the sampled `bash` CPU in those two tasks. Those processes
  were ended by hand under a manager ruling. Neither task's time or outcome is representative.
- **The e2e tier never ran.** `_onetaskgraph-preflight` refused it in 0.03 s, because a fresh
  worktree has no `target/tools/onetaskgraph-*` until `just bootstrap` runs. `gate` does not
  install it, and only reports the missing tool once the tier's turn comes, after hours of
  other work.
- **Other reds, all environmental as far as was established.** `report-job:test` lacked
  `target/tools/onebudgetspec` (bootstrap again). `test-rest`'s
  `a_sources_token_and_settings_come_from_the_engines_own_environment` read the host's own
  onetaskgraph configuration (`no source named "authoring"`). `github-workflows`,
  `llmlint-diff`, `ci-jobs`, `release` and `pool-maintenance` failed for causes not
  established here. Together those seven took 269 s of task wall, mostly in parallel. Each
  task's output is under `target/gate-profile/<stamp>/nx-state/cache/terminalOutputs/<hash>`.

The e2e tier's figures therefore come from the tier run on its own on the same commit. That
run reached 1,640 of 2,199 tests, alphabetically up to `rework`, in 1,776 s before the disk
filled. Not reached: the rest of `rework` and all of `rulings` through `writeback_projections`
(32 modules, among them `store` 83, `views` 66, `watch` 45, `shutdown` 33, `wake_budget` 22,
and the budget journeys `writeback_budget` and `writeback_cost`). **A whole-tier wall of about
2,380 s is an extrapolation** (1,776 s × 2,199 / 1,640, at load 9.9). It is likely low, since
the budget journeys left out are wait-bound.

## Ranked targets

**1. Linking the debug test binaries: about 85,000 s CPU, and the critical path.**
`onepipeline:build` (`cargo build --all-targets`) took 9,878 s wall and 46,353 s CPU (20,124 s
of it system). The instrumented build inside `narrow-write:test` took 10,025 s and 47,813 s
(20,312 s system). Both ran beside the release build, at load 36 (cargo invocations 19 and
21). The sampler's per-process ticks (`processes.json`, by task) give
GNU `ld` 2.42 40,446 s and 43,410 s of those tasks, against `rustc`'s 6,351 s and 4,760 s.
The slowest compile units are each test binary's final link: `onepipeline lib (test)`
9,286 s, `e2e (test)` 8,934 s, and 17 of the 46 test binaries over 3,000 s each, at load 37–40 (the report's *Compile units of cargo invocation 19*). An incremental plain rebuild
today, at load 1.2–11.8, took 481 s wall and 4,577 s CPU. `ld.lld` is installed and not used.
*Rough gain:* most of the link CPU, and a large share of the critical path's 9,878 s. That
is an estimate, unmeasured. Fewer test binaries would also cut the link count.

**2. The e2e tier waits and does little work, under a 4-thread cap: about 1,780–2,380 s of
wall, about 950 s of child CPU.** The partial tier spent 5,632 test-seconds in 1,776 s on
953 s of child CPU: 39.4% `wait`, 48.9% `spawn` (*Whole run* row of its report). Inside
`spawn`, the `onepipeline` runs wait too. In `readjust`, `start` averages 3,040 ms wall on
434 ms CPU, and 87% of that wall is `dispatch` self-time. In `lifecycle` it averages
2,828 ms on 215 ms CPU, 98% `dispatch` (each module's *Where the `onepipeline` runs went*).
Sampling found no hot code to cut: in `readjust` no symbol holds more than 1% self time, and
`lifecycle::publish` (18.8%) and `engine::converge` (14.9%) lead inclusive CPU. The tier runs
in nextest's `e2e` group, `max-threads = 4` (`.config/nextest.toml`), and averaged 3.2 of
those slots. Five journeys need all four slots, and their waits ran with nothing beside them:
`adoption`'s held release 124 s and three `loopcost` journeys 62–70 s each, about 317 s.
*Rough gain:* the tier's wall scales with how many waits run at once. Doubling the cap would
save roughly half of the roughly 1,460 s that does not run exclusively, if the host holds. An
estimate.

**3. Strategy B, unscaled intervals: the `wait` category, about 2,220 test-seconds of the
partial tier.** No shared scale exists. Each interval has its own environment variable
(`src/continuation.rs:109`, `src/release.rs:57`, `src/engine.rs:165`, `src/pool.rs:76`), and
the takeover windows are constants (`src/continuation.rs:117-127`). `loopcost` pins 60 s
windows (`tests/e2e/loopcost.rs:43`) and `adoption` waits out the shipped 60 s release poll
(`tests/e2e/adoption.rs:3558`). Which interval `dispatch`'s self-wall waits out is not broken
down by any span. *Rough gain:* up to that `wait` share, in proportion to the scale chosen,
less what the budget journeys must keep.

**4. `onevcs` landing reads: 142 ms each.** In `readjust`, 128 `onevcs:landing_status` spans
took 18.2 s, 12.5% of `start`'s wall, and 87–89% of `results` and `status`. In `lifecycle` they
are 1%. *Rough gain:* about 10% of the restack and review modules' `onepipeline` wall.

## Not targets

- **The binary's own work.** Release `results` 4.3 ms (2.9 ms user), debug `results` 10.5 ms,
  debug `goals` 24 ms, the process floor (`--version`) 1.0 ms release and 1.8 ms debug
  (`hyperfine`). `perf` over `goals` shows JSON line scanning and `memchr` at 2% or less each.
- **Coverage instrumentation's time.** Each module ran instrumented and plain, with spans on
  in both runs:

  | module | nextest wall, plain / instrumented | CPU, plain / instrumented | `start` mean wall and CPU, plain / instrumented |
  | --- | --- | --- | --- |
  | `readjust` (45 tests) | 59.2 s / 59.7 s | 65.2 s / 70.2 s (+7.6%) | 3,040 / 3,000 ms; 434 / 441 ms |
  | `lifecycle` (104 tests) | 98.8 s / 99.2 s | 58.2 s / 66.0 s (+13.4%) | 2,828 / 2,821 ms; 215 / 209 ms |

  Without spans, `readjust` took 59.5 s and 65.9 s CPU plain, against 63.0 s and 70.8 s
  instrumented. The merge and report steps add about 2–2.6 s per unit. Instrumentation
  costs CPU, not wall, and nothing per spawn. Its real cost is disk. One `readjust` run
  writes 3,213 raw profiles (0.46 GiB), and `lifecycle` writes 4,309. The tier run above
  filled the host's disk at test 1,640. `scripts/coverage-unit.sh:180` deletes a unit's raw
  profiles only before its next run, so a whole tier's raw profiles stay on disk after it.

## The old report's strategies

- **A (build the test doubles once):** landed in #1278 (`da460300`). Each nextest run builds
  them once under a lock (`tests/e2e/harness.rs:6828` `build_once`). The tier's `build`
  category is now 0.2%. In a cold module run it is 13.2% (about 60 s of one build), and 0.1%
  warm.
- **B (scale the intervals):** not landed, as target 3 says.
- **E (share the host-sized fixture):** partly landed in #1278. `host_view` builds its fixture
  once per nextest run and clones it (`tests/e2e/host_view.rs:489`). `listing`'s host-sized
  journey still builds its two templates per test.
- **#1320** made the gate's other builds reuse `onepipeline:build`'s debug build, and moved
  the stop-verdict release build out of its timing window into its own `test-build` task. On
  this commit that task ran 10,276 s wall, 7,212 s CPU, off the critical path, alongside the
  two debug builds of target 1.
