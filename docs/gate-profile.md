# Where `just gate`'s time goes

A ranking of the gate's costs, measured once. It is a snapshot. Re-measure before you act on
a figure; don't edit one in place.

**Commits measured:** the whole e2e tier on `d12ec799` (#1715), `origin/main`'s tip when that
measurement started. It carries this document, `just gate-profile`, the span records and the
profiler fix (#1716). The cold gate profile and the single-module figures were taken earlier,
on `072b17cd` (#1701). **Host:** 14 CPUs, aarch64 Linux, a shared host on which other
dispatches run, so every figure below carries the 1-minute load it was taken under. CPU
(user + system) is the figure that load disturbs least; compare on it first.

**The two profiles ran under different load.** The cold full gate averaged a load of 17.5
(0.5–47.7). The e2e tier averaged 2.66 (0.35–10.99). Compare their walls with that in mind,
and their CPU before their walls.

## How each figure was taken

| figure | command | when (UTC), load |
| --- | --- | --- |
| e2e tier, whole | after `just bootstrap` and `cargo nextest run --locked --no-run -E 'binary(e2e)'`: `NEXTEST_NO_FAIL_FAST=1 ONEPIPELINE_TEST_PROFILE_SPANS=1 /usr/bin/time -v just test-e2e`, with `/proc/loadavg` read every 10 s through the run (217 samples). Not coverage-instrumented. Run `f551e804`, on `d12ec799` | 2026-11-19 16:38:12–17:14:12, mean 2.66, median 2.29, p90 4.41, 0.35–10.99 |
| e2e tier report | `node scripts/test-profile-report.mjs --out FILE linux=target/test-profile/e2e` (Markdown), and the same with `--json --top-spans 1000` for the sums below | as the tier |
| tier-wide verbs | `jq '[.platforms[0].spans.modules[].verbs[]] \| group_by(.verb) \| map({verb: .[0].verb, runs: (map(.invocations)\|add), wall_s: (map(.wall_ms)\|add/1000), cpu_s: (map(.cpu_ms)\|add/1000), child_cpu_s: (map(.children_cpu_ms // 0)\|add/1000)}) \| sort_by(-.wall_s)' REPORT.json` | as the tier |
| tier-wide spans | `jq '[.platforms[0].spans.modules[].verbs[].spans[]] \| group_by(.name) \| map({span: .[0].name, count: (map(.count)\|add), self_wall_s: (map(.self_wall_ms)\|add/1000), self_cpu_s: (map(.self_cpu_ms // 0)\|add/1000)}) \| sort_by(-.self_wall_s)' REPORT.json` | as the tier |
| tier concurrency | `jq -s --arg run RUN_ID '[.[] \| select(.run_id == $run) \| [.started_at, 1], [.started_at + .wall_s, -1]] \| sort \| reduce .[] as $e ({n: 0, t: null, s: {}}; (if .t then .s[.n\|tostring] += ($e[0] - .t) else . end) \| .n += $e[1] \| .t = $e[0]) \| .s' target/test-profile/e2e/*.json`: seconds of wall with *n* tests running | as the tier |
| gate profile | `just gate-profile` (recipe `gate`, fresh Nx cache), report at `target/gate-profile/<stamp>/report.{md,json}`, on `072b17cd` | 2026-11-18 18:48–01:20, mean 17.5 (0.5–47.7) |
| module, plain | `ONEPIPELINE_TEST_PROFILE_SPANS=1 just test-e2e 'test(/^readjust::/)'` (and `lifecycle`), then the same report script over the records, on `072b17cd` | 2026-11-19 10:30–10:37, 2.2–8.2 |
| module, instrumented | `ONEPIPELINE_TEST_PROFILE_SPANS=1 just _tier-test onepipeline-e2e-readjust 'binary(e2e) and test(/^readjust::/)'` (the e2e tier's own recipe, narrowed), on `072b17cd` | 10:40–10:46, 1.6–2.5 |
| single verbs | `hyperfine -N` over `target/{debug,release}/onepipeline` with `ONEPIPELINE_RUNS_DIR` set to a copy of `tests/recorded/run-root`, on `072b17cd` | 10:37, 1.0–1.2 |
| sampling | `perf record -F 999 -g` over 200 `goals` runs, and `-F 499 -g` over the `readjust` module, on `072b17cd`; `kernel.perf_event_paranoid` was 1 only for those runs and read 4 afterwards. The whole-tier run was not sampled | 10:38:29–10:39:32, 1.66–1.86 |

Wall and CPU for every command come from `/usr/bin/time` (the children's `getrusage`).
`NEXTEST_NO_FAIL_FAST=1` is there because of the first whole-tier attempt on the same commit,
run with the same command but without that variable, at 16:07–16:37 and load 2.58. One test failed after 0.491 s,
`store::a_retried_project_launches_again_reading_the_lineage_head_under_the_roots_id`, which
panicked at `tests/e2e/harness.rs:3757:37` with `cannot read
/tmp/op-2538408-1/runs/writeback-retried/checkpoint.json: No such file or directory`.
nextest's summary was `1865/2199 tests run: 1864 passed (13 slow), 1 failed, 1 skipped`, and
fail-fast left 334 of the 2,199 tests unrun, so that attempt is not a whole-tier figure and none
of its figures appear below. It took 1,829.0 s wall and 1,424.2 s CPU. The test did not fail
again. It passed three times run alone, the selection
`just test-e2e 'test(=store::a_retried_project_launches_again_reading_the_lineage_head_under_the_roots_id)'`
makes (0.8–0.9 s each), and it passed in the measured run (0.45 s). Its cause is unconfirmed: those
passes show only that it does not reproduce alone or in a second whole-tier run.

The paths above are where each command writes when you re-run it. The `072b17cd` copies are
partly gone:

- **Removed with the measuring worktree:** the gate profile's directory,
  `target/gate-profile/20261118T184839Z/` (`report.{md,json}`, `phases.jsonl`,
  `processes.json`, `nx-state`). The gate report's text survives only as printed in the
  measuring session's transcript. The figures read from `phases.jsonl` (the `test` row's user
  and system split) and from `processes.json` (target 3's `ld` and `rustc` ticks) cannot be
  rechecked.
- **Kept in that dispatch's scratch directory:** both modules' span reports (plain and
  instrumented), every module run's `/usr/bin/time` and load record, `perf-goals.data` and
  `perf-readjust.data`, and `sample.log` with the sysctl reading before and after sampling.
  The `perf report` and `hyperfine` outputs survive as printed in the transcript.

## The e2e tier, whole

| | wall | CPU | user | system | load |
| --- | ---: | ---: | ---: | ---: | --- |
| `just test-e2e`, 2,199 tests, all passed | 2,159.3 s | 1,275.2 s | 799.5 s | 475.7 s | mean 2.66 (0.35–10.99) |

nextest reports 2,161.8 s for 2,199 tests run, 2,199 passed (23 slow), 1 skipped. The skipped
test is `surface::version_reports_the_crate_version`, which the tier's filter leaves to its own
tier. The tier's CPU over its wall is **0.59 cores** of the host's 14.

The report's *Whole run* row:

| | tests | wall s | child cpu s | load 1m | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 2199 | 6887.9 | 1014.6 | 2.34 → 2.33 | 39.9% | 1.1% | 0.0% | 0.0% | 49.8% | 0.2% | 8.9% |

In seconds, that is `spawn` 2,749.1, `wait` 3,431.1, `git` 76.6, `fs` 15.5, `build` 3.0 and
`fixture` 2.3, with 610 s unattributed. *Child cpu* is the CPU of the programs the tests
started. The 260 s that `/usr/bin/time` counts beyond it is the test processes' own and
nextest's. `build` is near zero because the set-aside run had already built the test doubles,
which cost it 103.6 s. In a run that builds them, they cost one build of that size.

**The costliest modules by wall** (wall and child CPU in seconds):

| module | tests | wall | child CPU | spawn | wait |
| --- | ---: | ---: | ---: | ---: | ---: |
| `continuation` | 52 | 781.0 | 29.5 | 17.3% | 65.2% |
| `writeback_budget` | 13 | 707.6 | 3.0 | 11.9% | 87.9% |
| `store` | 83 | 675.4 | 12.0 | 4.8% | 81.7% |
| `adoption` | 64 | 484.9 | 54.8 | 13.9% | 83.0% |
| `lifecycle` | 104 | 384.4 | 71.5 | 88.2% | 9.9% |
| `linear_writeback` | 17 | 299.9 | 3.0 | 3.2% | 95.5% |
| `readjust` | 45 | 225.2 | 90.6 | 92.2% | 4.2% |
| `check_visibility` | 23 | 212.2 | 19.2 | 50.7% | 32.8% |
| `loopcost` | 11 | 203.3 | 1.3 | 1.1% | 97.9% |

**The costliest modules by `spawn`** (seconds):

| module | tests | spawn | wall | child CPU |
| --- | ---: | ---: | ---: | ---: |
| `lifecycle` | 104 | 339.0 | 384.4 | 71.5 |
| `readjust` | 45 | 207.6 | 225.2 | 90.6 |
| `continuation` | 52 | 134.9 | 781.0 | 29.5 |
| `driver` | 110 | 133.0 | 169.5 | 22.4 |
| `dispatch` | 95 | 131.0 | 163.7 | 24.6 |
| `rework` | 23 | 110.6 | 121.1 | 50.9 |
| `check_visibility` | 23 | 107.5 | 212.2 | 19.2 |
| `review` | 22 | 98.7 | 114.7 | 59.1 |

**The slowest tests** (seconds):

| test | wall | wait | spawn | child CPU |
| --- | ---: | ---: | ---: | ---: |
| `writeback_budget::adoption_replaces_the_budget_and_later_adoptions_keep_it` | 172.5 | 131.2 | 41.2 | 1.3 |
| `store::a_source_naming_no_reset_is_read_again_on_a_doubling_backoff_capped_at_thirty_intervals` | 131.7 | 131.5 | 0.2 | 0.1 |
| `adoption::a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs` | 123.6 | 122.4 | 0.4 | 0.6 |
| `writeback_budget::a_creating_copy_held_past_a_tiny_budget_is_cancelled_and_the_next_attempt_creates_it` | 120.2 | 119.7 | 0.4 | 0.1 |
| `linear_writeback::a_linear_destination_lands_current_values_after_each_way_a_call_fails` | 101.3 | 98.9 | 2.2 | 0.6 |
| `check_visibility::an_optional_check_with_no_verdict_is_raised_once_and_moves_nothing` | 92.2 | 0.0 | 91.6 | 10.4 |
| `linear_writeback::command_end_refusal_and_timeout_rebuild_the_workers_store` | 78.7 | 77.2 | 1.3 | 0.2 |
| `writeback_budget::a_seven_item_copy_held_at_eleven_seconds_per_item_lands_inside_its_deadline` | 77.9 | 77.4 | 0.3 | 0.1 |
| `writeback_budget::a_destination_read_held_past_the_floor_is_cancelled_retried_and_recovers` | 72.8 | 72.3 | 0.3 | 0.1 |
| `delivers::a_first_projection_held_past_its_deadline_does_not_hold_back_the_first_dispatch` | 72.2 | 72.0 | 0.1 | 0.1 |

The 48 tests of 30 s or more hold 2,862 of the 6,888 test-seconds.

### Where the `onepipeline` runs went

The tier left 12,924 span records, one per `onepipeline` process. Together those processes
took 5,694.7 s of wall on 786.8 s of their own CPU, plus 295.3 s of their children's. The
recorder counts 12,221 of them as a test's own spawns (2,615.8 s of `spawn` wall, 891.0 s of
CPU). The rest were started by another process, such as a driver, and many of them run beside
a test's `wait`.

| verb | runs | wall s | share of the runs' wall | mean wall ms | CPU s | mean CPU ms | child CPU s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `drive-run` | 687 | 2,464.5 | 43.3% | 3,587 | 135.0 | 197 | 60.4 |
| `start` | 2,636 | 1,878.8 | 33.0% | 713 | 333.9 | 127 | 153.7 |
| `adopt` | 381 | 439.0 | 7.7% | 1,152 | 87.2 | 229 | 29.0 |
| `reply` | 1,159 | 248.0 | 4.4% | 214 | 35.4 | 31 | 1.7 |
| `watch` | 443 | 125.4 | 2.2% | 283 | 8.3 | 19 | 0.0 |
| `stop` | 244 | 73.4 | 1.3% | 301 | 9.5 | 39 | 10.2 |
| `status` | 1,620 | 42.2 | 0.7% | 26 | 36.6 | 23 | 1.9 |
| `next` | 1,963 | 35.3 | 0.6% | 18 | 38.6 | 20 | 0.0 |

| span | count | self wall s | share of the runs' wall | self CPU s |
| --- | ---: | ---: | ---: | ---: |
| `dispatch` | 12,924 | 5,056.7 | 88.8% | 187.8 |
| `onevcs:landing_status` | 1,130 | 148.1 | 2.6% | 10.8 |
| `registry:events` | 12,924 | 123.9 | 2.2% | 122.6 |
| `onemessagebus:transport::poll_for_change` | 237 | 90.9 | 1.6% | 0.9 |
| `onetaskgraph:project` | 3,548 | 42.1 | 0.7% | 1.8 |
| `journal:read` | 120,213 | 19.9 | 0.3% | 19.1 |
| `startup` | 12,924 | 18.8 | 0.3% | 18.5 |
| `ledger:read` | 249,383 | 17.4 | 0.3% | 12.2 |

Spans hold 407.3 s of self CPU. `dispatch` holds 46.1% of it, and `registry:events` holds
30.1%. A span's CPU is its own thread's, which is why that sum is below the processes'
786.8 s.

### Concurrency against `max-threads = 4`

The tier runs in nextest's `e2e` group, `max-threads = 4` (`.config/nextest.toml`). Across its
2,158.6 s, it averaged **3.19 tests at once**:

| tests running | wall s | share |
| ---: | ---: | ---: |
| 4 | 1,546.1 | 71.6% |
| 3 | 16.7 | 0.8% |
| 2 | 38.1 | 1.8% |
| 1 | 552.6 | 25.6% |
| 5–6 | 4.9 | 0.2% |

(Five and six are the edges of a test's record overlapping the next test's start.) The 552.6 s
with one test running has two sources:

- **Exclusive journeys.** The 34 tests that `.config/nextest.toml` gives `threads-required = 4`
  took 489.1 s of wall on 23.5 s of child CPU, 389.1 s of it `wait`. They were three `loopcost`
  journeys (195.2 s, 0.7 s CPU), `adoption`'s held release (123.6 s, 0.6 s CPU), the three
  host-sized journeys (79.1 s, 16.6 s CPU), 25 `maintenance` journeys (73.5 s, 5.5 s CPU) and
  two `review_round` journeys (17.7 s, 0.2 s CPU).
- **The tail.** The last moment four tests ran together was 103.7 s before the tier ended. The
  test holding that tail, `writeback_budget::adoption_replaces_the_budget_and_later_adoptions_keep_it`
  (172.5 s), started at +1,986 s.

## The cold gate profile, and what spoiled it

Taken on `072b17cd`, at mean load 17.5.

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
  and a test's own fake `cargo` or `just`. That loop is a defect of the profiler, fixed in
  #1716, not of the gate. It accounts for 48,524 s of the sampled `bash` CPU in those two
  tasks. Those processes were ended by hand under a manager ruling. Neither task's time or
  outcome is representative.
- **The e2e tier never ran.** `_onetaskgraph-preflight` refused it in 0.03 s, because a fresh
  worktree has no `target/tools/onetaskgraph-*` until `just bootstrap` runs. `gate` does not
  install it, and only reports the missing tool once the tier's turn comes, after hours of
  other work. The tier was therefore measured on its own, above.
- **Other reds, all environmental as far as was established.** `report-job:test` lacked
  `target/tools/onebudgetspec` (bootstrap again). `test-rest`'s
  `a_sources_token_and_settings_come_from_the_engines_own_environment` read the host's own
  onetaskgraph configuration (`no source named "authoring"`). `github-workflows`,
  `llmlint-diff`, `ci-jobs`, `release` and `pool-maintenance` failed for causes not
  established here. Together those seven took 269 s of task wall, mostly in parallel. Each
  task's output is under `target/gate-profile/<stamp>/nx-state/cache/terminalOutputs/<hash>`.

## Ranked targets

On an everyday gate the build is incremental, so the e2e tier's 2,159 s is most of the wait.
The tier is wait-bound: 0.59 cores of CPU on average, half its test-seconds in `wait`, and
almost nine-tenths of its `onepipeline` runs' wall in `dispatch`'s own waiting. The first two
targets act on that. Every gain below is an estimate from these figures, not a measurement of
the change.

**1. The 4-slot cap and the exclusive journeys: 552.6 s of the tier's 2,159 s wall with one
test running, on 23.5 s of child CPU in the exclusive journeys. The whole tier used 1,275 s of
CPU.** *Evidence:* the concurrency table above, where four slots are full for 71.6% of the
wall, and 25.6% of it runs one test. The exclusive journeys behind that one test are themselves
wait-bound: `loopcost` and `adoption`'s held release waited 315 s on 1.3 s of CPU, holding all
four slots. The tail adds 103.7 s of one long `writeback_budget` journey that started late.
*Rough gain:* filling the three idle slots of the one-test windows is worth up to about 415 s
of wall (552.6 s × 3/4). Starting the longest journeys first would recover part of the
103.7 s tail. A higher cap would scale the 1,546 s of full-slot wall down with it, since the
tier holds well under a core of CPU, provided the waits stay correct with more beside them,
which the cap's comment says they did not once. No CPU gain.

**2. Strategy B, unscaled intervals and deadlines: 3,431 test-seconds of `wait` (49.8%), and
`dispatch`'s self-wall of 5,056.7 s in the `onepipeline` runs, on 187.8 s of CPU.**
*Evidence:* the *Whole run* row, and the spans table above, where `dispatch` is 88.8% of the
runs' wall at 3.7% CPU. `drive-run` alone is 2,464.5 s of wall on 135.0 s of CPU, a mean of
3,587 ms per run. Seven modules hold 2,655 s of the `wait`: `writeback_budget` 622, `store` 552,
`continuation` 509, `adoption` 403, `linear_writeback` 286, `loopcost` 199 and `delivers` 84.
Their child CPU totals 105 s. No shared scale exists. Each interval has its own environment
variable (`src/continuation.rs:109`, `src/release.rs:57`, `src/engine.rs:165`, `src/pool.rs:76`),
and the takeover windows are constants (`src/continuation.rs:117-127`). Several journeys wait
out the *shipped* value: `writeback_budget` reads the shipped `floor_seconds` and
`default_seconds` (`tests/e2e/writeback_budget.rs:156`), `loopcost` pins 60 s windows
(`tests/e2e/loopcost.rs:43`), and `adoption` waits out the shipped 60 s release poll
(`tests/e2e/adoption.rs:3558`). On `072b17cd`, one wait-bound `continuation` test spent 1.0 s
of its own CPU across 37.0 s of `wait`. No span says which interval `dispatch`'s self-wall
waits out. *Rough gain:* up to the `wait` share of the wall in proportion to the scale chosen.
At 3.19 tests at once, the whole `wait` is worth about 1,075 s of tier wall, less what the
budget journeys must keep to prove their deadlines. Almost no CPU.

**3. Linking the debug test binaries: about 85,000 s CPU in the cold gate, and its critical
path.** These figures come from the cold gate profile on `072b17cd` at load 36, and they still
stand. No change since touches the build. `onepipeline:build` (`cargo build --all-targets`)
took 9,878 s wall and 46,353 s CPU (20,124 s of it system). The instrumented build inside
`narrow-write:test` took 10,025 s and 47,813 s (20,312 s system). Both ran beside the release
build (cargo invocations 19 and 21). The sampler's per-process ticks (`processes.json`, by task)
give GNU `ld` 2.42 40,446 s and 43,410 s of those tasks, against `rustc`'s 6,351 s and
4,760 s. The slowest compile units are each test binary's final link: `onepipeline lib (test)`
9,286 s, `e2e (test)` 8,934 s, and 17 of the 46 test binaries over 3,000 s each, at load
37–40 (the report's *Compile units of cargo invocation 19*). On an everyday gate the build is
incremental: a plain incremental rebuild on `072b17cd`, at load 1.2–11.8, took 481 s wall and
4,577 s CPU. The cold test build before this tier's run, `cargo nextest run --no-run`, took
1,092.5 s wall and 10,604 s CPU, with a 1-minute load of 9.2 when it ended. `ld.lld` is
installed and not used. *Rough gain:* most of the link CPU, and a large share of the cold
critical path. Fewer test binaries would also cut the link count.

**4. Building the event registry in every process: 122.6 s CPU, 123.9 s wall over 12,924
`onepipeline` runs, 9.5 ms each.** `src/lib.rs:175` builds `event::registry()` under the span
`registry:events` before any verb runs. *Evidence:* the spans table above. The span is
CPU-bound (122.6 s of CPU in 123.9 s of wall). It holds 15.6% of the processes' 786.8 s CPU
and 30.1% of the span-attributed CPU, second only to `dispatch`. In short verbs it dominates.
`next` costs 9.7 ms of it per run against a mean of 20 ms CPU, and `status` 9.3 ms of 23 ms.
The tests run the debug binary. Release `results` took 4.3 ms on `072b17cd` (`hyperfine`), so
the registry is far cheaper there, which leaves the debug build's optimisation of the
registry's dependencies as one suspect. What inside the registry costs the time needs sampling,
which this run did not do. *Rough gain:* up to about 120 s of the tier's 1,275 s CPU (9%), and
about 40 s of its wall at 3.19 tests at once.

**5. `onevcs` landing reads: 1,130 reads, 148.1 s of self wall (131 ms each), 10.8 s of CPU.**
*Evidence:* the `onevcs:landing_status` row of the spans table, 2.6% of the runs' wall. On
`072b17cd` it held 17% of `readjust`'s runs' wall, and `perf` over that module put
`onevcs::landing_status` at 7.20% of sampled CPU inclusive and every `git` process together
at 0.99%. A `git` child of the landing read therefore costs at most that much. *Rough gain:*
about 46 s of tier wall (148.1 s over 3.19 tests at once) and up to 11 s of CPU.

## Not targets

- **The binary's own work outside the registry.** Release `results` 4.3 ms (2.9 ms user),
  debug `results` 10.5 ms, debug `goals` 24 ms, the process floor (`--version`) 1.0 ms release
  and 1.8 ms debug (`hyperfine`, `072b17cd`). `perf` over `goals` shows JSON line scanning and
  `memchr` at 2% or less each. Across the tier, `startup` holds 18.5 s of CPU, `journal:read`
  19.1 s and `ledger:read` 12.2 s.
- **Coverage instrumentation's time.** Each module ran instrumented and plain on `072b17cd`,
  with spans on in both runs:

  | module | nextest wall, plain / instrumented | CPU, plain / instrumented | `start` mean wall and CPU, plain / instrumented |
  | --- | --- | --- | --- |
  | `readjust` (45 tests) | 59.2 s / 59.7 s | 65.2 s / 70.2 s (+7.6%) | 3,040 / 3,000 ms; 434 / 441 ms |
  | `lifecycle` (104 tests) | 98.8 s / 99.2 s | 58.2 s / 66.0 s (+13.4%) | 2,828 / 2,821 ms; 215 / 209 ms |

  Without spans, `readjust` took 59.5 s and 65.9 s CPU plain, against 63.0 s and 70.8 s
  instrumented. The merge and report steps add about 2–2.6 s per unit. Instrumentation
  costs CPU, not wall, and nothing per spawn. Its real cost is disk. One `readjust` run
  writes 3,213 raw profiles (0.46 GiB), and `lifecycle` writes 4,309. An instrumented tier run
  filled the host's disk at test 1,640. `scripts/coverage-unit.sh:180` deletes a unit's raw
  profiles only before its next run, so a whole tier's raw profiles stay on disk after it.

## The old report's strategies

- **A (build the test doubles once):** landed in #1278 (`da460300`). Each nextest run builds
  them once under a lock (`tests/e2e/harness.rs:6828` `build_once`). In the measured tier
  `build` is 3.0 s, since they were already built. In the set-aside run that built them, the
  build cost 103.6 s inside one test.
- **B (scale the intervals):** not landed, as target 2 says.
- **E (share the host-sized fixture):** partly landed in #1278. `host_view` builds its fixture
  once per nextest run and clones it (`tests/e2e/host_view.rs:489`). `listing`'s host-sized
  journey still builds its two templates per test.
- **#1320** made the gate's other builds reuse `onepipeline:build`'s debug build, and moved
  the stop-verdict release build out of its timing window into its own `test-build` task. On
  `072b17cd` that task ran 10,276 s wall, 7,212 s CPU, off the critical path, alongside the
  two debug builds of target 3.
