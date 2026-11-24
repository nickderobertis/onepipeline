# Where the e2e tier's setup-heavy journeys spend their time on CI

A ranking of what the slowest setup-heavy e2e journeys spend their time on, on each of the
three CI platforms, measured once. It is a snapshot. Re-measure before you act on a figure;
don't edit one in place. `docs/gate-profile.md` is the same kind of record for the local gate.

**In short.** These journeys are not slow because of `onepipeline`'s own start-up, and on
Windows most of their own cost is not engine computation either. It is the **git processes
the engine starts** through the `onevcs` library: 1,100 to 5,700 in each of 13 of the 18
sampled journeys, and 228,431 across the tier, against 2 to 18 `onepipeline` processes in
each of those 13. git's own work in each one is 1 to 2 ms. What costs is starting it, which
run 2's runners priced at 19.95 ms on Windows, 7.02 ms on macOS and 1.26 ms on Linux. On
Windows, contention between the four tests running at once adds 37 to 46% to these journeys
on top of their own cost. Which
processor the hosted runner turns out to have moved the whole tier by 20 to 35 minutes, more
than any code cause measured here.

**Commits measured.** The product code is `main` at `41da6759` (#1824). The demonstration
change request #1830, stacked on this one's draft (#1829) and closed, added only the
measuring instruments listed under *What the demonstration changed*. Its two executing heads
were `6e5322b4` (run 1) and `67c20eb3` (run 2). The tier-wide git count was taken on the
measuring host at `c475f826`, the demonstration's last commit, over the same product code.
The baseline is run [45109721932](https://github.com/nickderobertis/onepipeline/actions/runs/45109721932),
an executing run of `646b6205` (#1809), in which the whole tier ran.

**Runs and jobs read.** Each job's `test-profile-test-<platform>` artifact (the tier's
per-test records and span records) and, for runs 1 and 2, its `demo-profile-<platform>`
artifact (the sample run alone, the spawn log, git's trace and the floor).

| run | Windows | macOS | Linux |
| --- | --- | --- | --- |
| baseline [45109721932](https://github.com/nickderobertis/onepipeline/actions/runs/45109721932) | [137479623602](https://github.com/nickderobertis/onepipeline/actions/runs/45109721932/job/137479623602): AMD EPYC 7763, 4 CPUs | [137479623505](https://github.com/nickderobertis/onepipeline/actions/runs/45109721932/job/137479623505): Apple M1 (Virtual), 3 CPUs | [137479623587](https://github.com/nickderobertis/onepipeline/actions/runs/45109721932/job/137479623587): AMD EPYC 9V74, 4 CPUs |
| run 1 [45251256131](https://github.com/nickderobertis/onepipeline/actions/runs/45251256131) | [137961194136](https://github.com/nickderobertis/onepipeline/actions/runs/45251256131/job/137961194136): AMD EPYC 7763, 4 CPUs | [137961194713](https://github.com/nickderobertis/onepipeline/actions/runs/45251256131/job/137961194713): Apple M2 Pro (Virtual), 5 CPUs | [137961194908](https://github.com/nickderobertis/onepipeline/actions/runs/45251256131/job/137961194908): Intel Xeon 6975P-C, 4 CPUs. Tier cut short, not used |
| run 2 [45275089955](https://github.com/nickderobertis/onepipeline/actions/runs/45275089955) | [138044703779](https://github.com/nickderobertis/onepipeline/actions/runs/45275089955/job/138044703779): AMD EPYC 9V74, 4 CPUs | [138044703431](https://github.com/nickderobertis/onepipeline/actions/runs/45275089955/job/138044703431): Apple M1 (Virtual), 3 CPUs | [138044703911](https://github.com/nickderobertis/onepipeline/actions/runs/45275089955/job/138044703911): Intel Xeon 6975P-C, 4 CPUs |

Each processor is what the job's first step printed (`lscpu`, `sysctl
machdep.cpu.brand_string hw.ncpu`, `Win32_Processor` and the processor count). **Run 2 is
the primary measurement**: it has every pass below on the same runner for all three
platforms. Run 1's Windows and macOS jobs are a second sample of the same passes on other
processors. Every figure below is from run 2 unless it says otherwise.

## What the demonstration changed

The three test jobs ran as `ci.yml` runs them on a pull request, with the same recipe
(`just ci-test-<platform>`), the same affected selection and the same neighbours under Nx.
`lint`, `budgets` and `timed` were trimmed. What was added:

- **Span records on.** `ONEPIPELINE_TEST_PROFILE_SPANS=1` on the gate step, so each
  `onepipeline` process wrote its span record (`src/spans.rs`) beside its test's record.
  Windows' tier took 5,429.4 s with them on in run 1 against 5,402.9 s in the baseline on
  the same processor, so recording them costs the tier nothing visible. The floor below puts
  it at 1.4 to 3.1 ms a process.
- **Spawns paired with their processes.** The harness gave each `onepipeline` it started an
  id in `ONEPIPELINE_DEMO_SPAWN_ID`, which the binary copied into its span record and took
  out of its environment, and logged its own start time and duration of that spawn to a
  file of its own (`ONEPIPELINE_DEMO_SPAWN_LOG_DIR`). In run 1 that log sat inside each
  span directory, which the two `profiling` journeys read file by file, so they failed
  there. Run 2 moved it out, and they passed.
- **`NEXTEST_NO_FAIL_FAST=1`** on the gate step in run 2. Linux's tier runs without the
  `ci` nextest profile, so in run 1 the two `profiling` failures stopped it at 1,399 of 2,262
  tests. Run 1's Linux is therefore not used.
- **After the gate step**, three extra steps on the same runner: the 18 sampled journeys
  again one at a time (`NEXTEST_TEST_THREADS=1`, through `just _tier-test` on Linux and
  `just _tier-test-quick` elsewhere, as each tier runs), then once more with
  `GIT_TRACE2_EVENT` set, then the floor (`.github/demo/spawn-floor.mjs`: 200 timed runs
  of each case from Node after 5 unmeasured ones).
- **Job timeouts** were raised so the extra steps fit. No measured step's timeout changed.

What else failed: in run 2 one e2e test on macOS
(`channel::a_verdict_naming_its_question_binds_to_it_and_a_stranger_is_refused_by_name`,
3.1 s, not in the sample, cause not investigated), and in every job the CI-shape tests the
trimmed workflow breaks (`ci_workflows::every_required_context_is_reported_on_every_pull_request`
and two more). Windows ran all 1,908 e2e tests and Linux all 2,262. The harness, the
scripts and the workflow are on #1830's commits.

## How each figure was taken

| figure | how |
| --- | --- |
| a journey's wall and its harness split | its record (`target/test-profile/e2e/<module>.<test>.json`): `wall_s`, the categories `spawn`, `git`, `fixture`, `wait`, `fs`, `build`, and `unattributed_s`. Each platform's run is the one with the most records, as `scripts/test-profile-report.mjs` picks it |
| process start-up, outside the binary | per foreground spawn, the process's `started_at` minus the harness's start of the spawn (the OS starting it, up to the binary's first initializer), and the harness's duration minus that and the process's own `wall_ms` (exit, reaping and writing the record) |
| process start-up, inside the binary | the `startup` span and every `registry:events*` span. `registry:events:compile` is the validator compilation onemessagebus#218 names |
| engine work | the rest of each foreground `onepipeline` run's own wall, plus the spawn calls of the ones the test started in the background |
| git, the harness's own | the record's `git` category |
| git, the engine's | how many git processes the engine started, times the runner's floor for `git rev-parse --git-dir`. Placed first in the journey's foreground engine time and then in its waiting, never past either. A lower bound, explained below |
| waiting | the record's `wait` category, plus the harness waiting on a background `onepipeline` to exit |
| other | other programs the harness ran (`fake-gh`, `onevcs`, `cargo`), `build` and `fs` |
| `onepipeline` processes per journey | the span records in the journey's directory, one per process, drivers included |
| a journey's own cost, and contention | each journey run alone on the same runner after the tier. The tier's time minus that time is what running beside the rest of the tier added |
| git processes per journey | the alone pass again with `GIT_TRACE2_EVENT`: each process's `start` and `atexit` events, attributed to the test whose record's time window holds the start. That pass is serial, so no git was left unattributed |
| git processes per test, tier-wide | the whole e2e tier on the measuring host, the harness pointing each test's `onepipeline` processes at `GIT_TRACE2` files of their own, so only git the engine started was counted. 2,262 tests ran and 37 failed, so their counts stop where they failed. 15 were `templates`, which need the `onetaskgraph` this host had not installed, and J8 failed in its budget step for want of the npm packages, after its round had run. The rest were not investigated, and none is in the sample |
| the floor | `.github/demo/spawn-floor.mjs` on each runner: means over 200 runs |
| the tier's concurrency | summed test wall over the tier's span from the first test's start to the last test's end, which equals the time-weighted mean of tests running at once |

**Why the engine's git is priced at the floor.** No span sees it: `onevcs` starts git from
inside a library call, and when `start` drives a run in the foreground its driver threads
record no spans. git can time itself, but only from inside (its trace's `atexit` `t_abs`).
The floor times a minimal real read (`git rev-parse --git-dir` in a fresh repository) from
outside, which is its start-up and exit plus almost no work. Every engine git costs at least
that. The real reads cost more, so the figure is a floor. On Linux and macOS git's trace adds
little (nextest timed Linux's alone pass at 209.4 s plain and 207.6 s traced). There, each
journey's traced in-process total plus each process's start-up gives a higher estimate (Linux's redo
journey, J1: 5.5 s against 3.9 s at the floor; macOS's: 37.6 s against 22.6 s). On Windows,
tracing adds 9.5 ms a process (29.47 ms traced against 19.95 ms untraced) and slowed the
alone pass from 934.8 s to 1,341.4 s, so only its counts are used there.

## The floor on each runner

Means over 200 runs, in ms, from Node's `spawnSync` start to exit, run 2.

| case | Windows, EPYC 9V74 | macOS, M1 | Linux, Xeon 6975P-C |
| --- | ---: | ---: | ---: |
| `git --version` | 18.96 | 8.37 | 1.21 |
| `git rev-parse --git-dir` | 19.95 | 7.02 | 1.26 |
| the same, traced: outside / git's own `atexit` | 29.47 / 20.55 | 8.42 / 1.54 | 1.36 / 0.28 |
| `onepipeline --version` (exits while parsing) | 8.56 | 8.38 | 5.57 |
| `onepipeline runs` over an empty runs root | 27.30 | 20.41 | 17.95 |
| the same, recording its span record | 30.39 | 22.22 | 19.40 |

Linux's figures are the instrumented binary its tier runs. Its uninstrumented build measured
1.29 ms for `git rev-parse` and 16.93 ms for `runs`. In run 1, Windows on the EPYC 7763
measured `git --version` at 26.5 ms and `runs` at 38.5 ms, and macOS on the M2 Pro at 4.9 ms
and 14.7 ms. That run had no `rev-parse` case.

A trivial git process costs 15.8 times Linux's on Windows (19.95 / 1.26) and 5.6 times on
macOS (7.02 / 1.26). `onepipeline`'s own start costs 1.5 times Linux's on Windows (27.30 /
17.95).

## The sample

Eighteen journeys. The four the plan named, the two spawn-bound tests the clock node leaves on
real time (manager ruling `a1`: J6 and J7), at least one of each of `readjust`, `adoption`,
`lifecycle`, `continuation`, `rework` and `publication_boundary`, and the slowest remaining
journeys on Windows whose Windows time was two or more times their Linux time in the
baseline. The 62 tests the clock node moves are left out, except J3 and J8, whose setup
stays here. Where a module's slowest such journey was in the move set, its slowest outside
it was taken.

| | journey | baseline Windows / macOS / Linux s |
| --- | --- | --- |
| J1 | `redo::a_launch_redoes_each_node_whose_task_changed_and_its_done_dependents` | 208.8 / 69.5 / 29.2 |
| J2 | `out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique` | 203.3 / 59.6 / 30.3 |
| J3 | `check_visibility::the_check_visibility_workload_reads_at_its_cadence_and_records_what_it_spent` | 192.4 / 29.0 / 26.8 |
| J4 | `readjust::one_of_several_parents_landing_advances_the_stack_base_then_moves_the_draft` | 103.4 / 27.4 / 14.5 |
| J5 | `readjust::the_store_is_written_only_for_what_its_projection_holds_changed` | 161.1 / 52.9 / 22.6 |
| J6 | `review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it` | 68.5 / 54.2 / 43.1 |
| J7 | `dispatch_env_hook::a_hook_that_fails_times_out_cannot_start_or_prints_a_malformed_document_refuses_the_launch` | 12.2 / 63.3 / 13.6 |
| J8 | `review_round::the_busy_review_round_ends_answered_and_true_and_records_what_it_spent` | 139.6 / 73.4 / 29.7 |
| J9 | `session_reuse::every_other_shape_cuts_a_fresh_session_onto_its_branch_or_from_the_base` | 103.0 / 28.5 / 12.6 |
| J10 | `out_of_band::an_unusable_bound_is_refused_before_a_landing_pushes_or_writes_to_the_host` | 92.6 / 16.6 / 7.0 |
| J11 | `landing::a_render_asks_the_landing_read_once_per_node_it_prints_and_does_nothing_else_per_node` | 86.4 / 27.7 / 10.7 |
| J12 | `rework::a_rework_restacks_every_draft_stacked_on_it_by_merges_alone` | 81.6 / 23.0 / 10.8 |
| J13 | `publication_boundary::the_host_branch_prefix_leads_every_public_branch_and_is_itself_screened` | 59.0 / 17.0 / 11.3 |
| J14 | `adoption::a_release_stated_on_the_settle_is_recorded_and_releases_the_node_waiting_on_it` | 54.7 / 11.5 / 5.2 |
| J15 | `continuation::a_dispatchs_branch_wins_over_a_settlements_and_an_authored_branch_over_both` | 50.8 / 21.4 / 8.3 |
| J16 | `lifecycle::a_publication_over_an_unchanged_tree_is_settled_without_spending_the_budget` | 38.6 / 13.4 / 7.0 |
| J17 | `unpublished::a_projects_preserved_branches_are_discarded_or_kept_with_onevcs_refusal` | 81.9 / 22.5 / 10.4 |
| J18 | `stacked_drafts::landing_dependents_of_unlanded_parents_are_drafted_on_them` | 81.3 / 39.7 / 15.7 |

J6 and J7 are below the two-times bar (1.6 and 0.9 times) and are in the sample by ruling.
`lifecycle`'s slowest journeys at two times or more were in the move set or just under the
bar (`an_armed_change_the_host_never_lands_spends_its_reads_and_is_redispatched`, 1.96
times), so J16 is its slowest above it.

## Each journey, split

Seconds. *Tier* is the journey's wall in run 2's whole tier, *alone* on the same runner after
it, and *contention* their difference. The split is of the alone run, so it is the journey's
own cost, and its columns add up to *alone*. *Engine git ≥* is the lower bound above, and
*engine other* the engine time it leaves. *Git processes* counts all of them, the harness's
own included.

### Windows, AMD EPYC 9V74 (job 138044703779)

| journey | tier | alone | contention | start-up | engine git ≥ | engine other | harness git | fixture | waiting | unattributed | other | git processes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| J1 `redo` | 140.9 | 83.2 | +57.7 | 0.5 | 64.2 | 11.4 | 0.5 | 0.0 | 4.4 | 2.1 | 0.1 | 3,229 |
| J2 `out_of_band` | 123.9 | 84.6 | +39.3 | 0.6 | 54.0 | 28.2 | 1.5 | 0.0 | 0.0 | 0.2 | 0.2 | 2,741 |
| J3 `check_visibility` | 168.5 | 154.0 | +14.6 | 0.0 | 35.4 | 0.0 | 1.1 | 0.0 | 9.0 | 108.1 | 0.2 | 1,795 |
| J4 `readjust` | 72.3 | 46.4 | +25.9 | 0.3 | 32.3 | 12.1 | 0.6 | 0.0 | 0.0 | 0.2 | 1.0 | 1,641 |
| J5 `readjust` | 110.7 | 67.6 | +43.2 | 0.2 | 49.7 | 16.3 | 0.5 | 0.0 | 0.4 | 0.2 | 0.4 | 2,503 |
| J6 `review` | 57.9 | 54.7 | +3.2 | 0.9 | 11.0 | 38.3 | 2.5 | 0.0 | 1.3 | 0.6 | 0.1 | 579 |
| J7 `dispatch_env_hook` | 9.8 | 6.6 | +3.2 | 1.1 | 0.0 | 4.9 | 0.0 | 0.0 | 0.0 | 0.4 | 0.2 | 1 |
| J8 `review_round` | 104.7 | 106.9 | -2.2 | 0.0 | 102.1 | 0.0 | 0.4 | 0.0 | 0.0 | 3.0 | 1.3 | 5,728 |
| J9 `session_reuse` | 74.5 | 50.5 | +24.0 | 0.2 | 32.8 | 11.5 | 0.5 | 0.0 | 0.0 | 0.2 | 5.3 | 1,659 |
| J10 `out_of_band` | 44.6 | 29.1 | +15.5 | 0.3 | 7.4 | 0.0 | 6.8 | 0.0 | 0.0 | 0.0 | 14.5 | 1,159 |
| J11 `landing` | 55.9 | 34.0 | +21.9 | 0.3 | 24.2 | 8.2 | 1.2 | 0.0 | 0.0 | 0.1 | 0.0 | 1,251 |
| J12 `rework` | 56.3 | 33.5 | +22.8 | 0.1 | 32.2 | 0.0 | 0.7 | 0.0 | 0.0 | 0.2 | 0.3 | 1,743 |
| J13 `publication_boundary` | 41.5 | 28.3 | +13.3 | 0.2 | 23.6 | 0.6 | 1.0 | 0.0 | 2.6 | 0.1 | 0.1 | 1,203 |
| J14 `adoption` | 36.5 | 20.6 | +15.9 | 0.0 | 17.7 | 0.0 | 0.8 | 0.0 | 1.2 | 0.2 | 0.6 | 903 |
| J15 `continuation` | 33.5 | 23.7 | +9.8 | 0.1 | 14.3 | 8.5 | 0.6 | 0.0 | 0.0 | 0.1 | 0.0 | 726 |
| J16 `lifecycle` | 27.6 | 19.9 | +7.7 | 0.1 | 11.3 | 7.9 | 0.4 | 0.0 | 0.0 | 0.1 | 0.0 | 576 |
| J17 `unpublished` | 60.5 | 38.7 | +21.8 | 0.6 | 26.6 | 9.6 | 0.4 | 0.0 | 0.0 | 0.1 | 1.2 | 1,346 |
| J18 `stacked_drafts` | 61.4 | 52.3 | +9.0 | 0.1 | 34.9 | 16.5 | 0.5 | 0.0 | 0.0 | 0.2 | 0.0 | 1,769 |
| **sum** | **1281.0** | **934.5** | **+346.5** | 5.7 | 574.1 | 174.0 | 20.0 | 0.1 | 18.9 | 116.3 | 25.5 | 30,552 |

### macOS, Apple M1 (job 138044703431)

| journey | tier | alone | contention | start-up | engine git ≥ | engine other | harness git | fixture | waiting | unattributed | other | git processes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| J1 `redo` | 59.0 | 54.2 | +4.8 | 0.3 | 22.6 | 25.8 | 0.3 | 0.0 | 3.7 | 1.4 | 0.1 | 3,234 |
| J2 `out_of_band` | 62.6 | 60.5 | +2.1 | 0.5 | 17.7 | 40.7 | 0.8 | 0.0 | 0.0 | 0.4 | 0.4 | 2,552 |
| J3 `check_visibility` | 32.4 | 35.2 | -2.8 | 0.0 | 10.9 | 0.0 | 0.6 | 0.0 | 12.6 | 11.0 | 0.2 | 1,573 |
| J4 `readjust` | 31.5 | 31.2 | +0.3 | 0.2 | 10.6 | 19.2 | 0.3 | 0.0 | 0.0 | 0.2 | 0.7 | 1,525 |
| J5 `readjust` | 56.4 | 48.9 | +7.5 | 0.1 | 15.7 | 32.0 | 0.3 | 0.0 | 0.3 | 0.2 | 0.3 | 2,250 |
| J6 `review` | 57.6 | 56.3 | +1.3 | 0.7 | 3.7 | 48.0 | 1.9 | 0.0 | 1.1 | 0.6 | 0.3 | 558 |
| J7 `dispatch_env_hook` | 74.8 | 67.9 | +6.9 | 1.0 | 0.0 | 66.2 | 0.0 | 0.0 | 0.0 | 0.4 | 0.3 | 3 |
| J8 `review_round` | 81.3 | 78.0 | +3.3 | 0.0 | 36.9 | 0.0 | 0.2 | 0.0 | 37.6 | 2.3 | 1.0 | 5,264 |
| J9 `session_reuse` | 29.6 | 30.8 | -1.2 | 0.2 | 11.4 | 15.3 | 0.3 | 0.0 | 0.2 | 0.2 | 3.3 | 1,631 |
| J10 `out_of_band` | 18.3 | 13.1 | +5.2 | 0.2 | 3.5 | 0.0 | 3.4 | 0.0 | 0.0 | 0.1 | 5.8 | 1,129 |
| J11 `landing` | 23.1 | 22.9 | +0.2 | 0.2 | 8.0 | 13.7 | 0.7 | 0.0 | 0.0 | 0.1 | 0.1 | 1,181 |
| J12 `rework` | 26.4 | 26.2 | +0.2 | 0.1 | 11.1 | 13.9 | 0.3 | 0.0 | 0.2 | 0.3 | 0.4 | 1,606 |
| J13 `publication_boundary` | 20.9 | 20.1 | +0.8 | 0.2 | 7.9 | 8.3 | 0.6 | 0.0 | 2.6 | 0.3 | 0.3 | 1,142 |
| J14 `adoption` | 10.3 | 10.7 | -0.4 | 0.0 | 4.3 | 0.0 | 0.5 | 0.0 | 5.4 | 0.2 | 0.3 | 633 |
| J15 `continuation` | 22.4 | 22.0 | +0.3 | 0.1 | 4.8 | 16.5 | 0.4 | 0.0 | 0.0 | 0.2 | 0.1 | 688 |
| J16 `lifecycle` | 11.9 | 12.6 | -0.7 | 0.1 | 3.8 | 8.3 | 0.2 | 0.0 | 0.0 | 0.1 | 0.1 | 550 |
| J17 `unpublished` | 19.9 | 18.2 | +1.6 | 0.4 | 9.3 | 7.8 | 0.2 | 0.0 | 0.0 | 0.1 | 0.5 | 1,335 |
| J18 `stacked_drafts` | 40.3 | 40.0 | +0.3 | 0.1 | 11.6 | 27.9 | 0.2 | 0.0 | 0.0 | 0.2 | 0.1 | 1,677 |
| **sum** | **678.5** | **648.9** | **+29.6** | 4.5 | 193.8 | 343.5 | 11.0 | 0.1 | 63.7 | 18.0 | 14.2 | 28,531 |

### Linux, Intel Xeon 6975P-C (job 138044703911)

| journey | tier | alone | contention | start-up | engine git ≥ | engine other | harness git | fixture | waiting | unattributed | other | git processes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| J1 `redo` | 20.5 | 16.8 | +3.6 | 0.3 | 3.9 | 10.6 | 0.1 | 0.0 | 0.8 | 1.2 | 0.0 | 3,058 |
| J2 `out_of_band` | 20.9 | 15.3 | +5.6 | 0.4 | 3.2 | 11.3 | 0.1 | 0.0 | 0.0 | 0.2 | 0.1 | 2,557 |
| J3 `check_visibility` | 20.7 | 20.4 | +0.4 | 0.0 | 2.0 | 0.0 | 0.2 | 0.0 | 7.3 | 10.9 | 0.0 | 1,582 |
| J4 `readjust` | 9.9 | 8.3 | +1.6 | 0.2 | 1.9 | 6.0 | 0.0 | 0.0 | 0.0 | 0.1 | 0.1 | 1,518 |
| J5 `readjust` | 17.2 | 11.5 | +5.7 | 0.1 | 2.8 | 8.2 | 0.1 | 0.0 | 0.2 | 0.1 | 0.0 | 2,213 |
| J6 `review` | 39.0 | 36.9 | +2.1 | 0.6 | 0.7 | 33.8 | 0.9 | 0.0 | 0.7 | 0.3 | 0.0 | 568 |
| J7 `dispatch_env_hook` | 11.4 | 11.3 | +0.1 | 0.8 | 0.0 | 10.2 | 0.0 | 0.0 | 0.0 | 0.3 | 0.0 | 8 |
| J8 `review_round` | 21.6 | 24.2 | -2.7 | 0.0 | 6.2 | 0.0 | 0.2 | 0.0 | 15.2 | 2.2 | 0.4 | 4,916 |
| J9 `session_reuse` | 9.3 | 6.9 | +2.4 | 0.1 | 2.0 | 4.1 | 0.0 | 0.0 | 0.0 | 0.1 | 0.4 | 1,625 |
| J10 `out_of_band` | 4.0 | 2.4 | +1.5 | 0.2 | 0.6 | 0.0 | 0.5 | 0.0 | 0.0 | 0.0 | 1.1 | 1,131 |
| J11 `landing` | 8.7 | 8.0 | +0.7 | 0.2 | 1.4 | 6.3 | 0.1 | 0.0 | 0.0 | 0.1 | 0.0 | 1,175 |
| J12 `rework` | 8.1 | 7.7 | +0.4 | 0.0 | 1.9 | 5.4 | 0.0 | 0.0 | 0.1 | 0.2 | 0.1 | 1,557 |
| J13 `publication_boundary` | 8.0 | 6.1 | +1.8 | 0.1 | 1.4 | 3.7 | 0.1 | 0.0 | 0.7 | 0.1 | 0.0 | 1,117 |
| J14 `adoption` | 4.5 | 4.1 | +0.4 | 0.0 | 0.6 | 0.0 | 0.1 | 0.0 | 3.2 | 0.1 | 0.1 | 526 |
| J15 `continuation` | 7.2 | 7.2 | +0.0 | 0.1 | 0.8 | 6.1 | 0.1 | 0.0 | 0.0 | 0.1 | 0.0 | 680 |
| J16 `lifecycle` | 4.9 | 4.9 | +0.0 | 0.1 | 0.7 | 4.0 | 0.0 | 0.0 | 0.0 | 0.1 | 0.0 | 550 |
| J17 `unpublished` | 7.8 | 6.7 | +1.0 | 0.4 | 1.7 | 4.4 | 0.0 | 0.0 | 0.0 | 0.1 | 0.1 | 1,327 |
| J18 `stacked_drafts` | 12.0 | 10.3 | +1.7 | 0.1 | 1.9 | 8.1 | 0.0 | 0.0 | 0.0 | 0.1 | 0.0 | 1,521 |
| **sum** | **235.7** | **209.1** | **+26.6** | 3.9 | 33.8 | 122.2 | 2.5 | 0.1 | 28.1 | 16.1 | 2.6 | 27,629 |

What the three tables show:

- **The engine's git is the largest part of the git-heavy journeys' own cost on Windows.**
  Across the sample alone, wall fits 20.3 s + 18.6 ms per git process on Windows (r = 0.66),
  21.0 s + 9.5 ms on macOS (r = 0.56) and 7.6 s + 2.6 ms on Linux (r = 0.34). Those slopes sit
  near each runner's floor. Where the floor-priced git reaches all of a journey's engine
  time (Windows' J12, J14 and J8), git is at least the whole of it.
- **`onepipeline`'s own start-up is small everywhere**: 5.7 s of Windows' 934.5 s alone,
  4.5 s of macOS' 648.9 s and 3.9 s of Linux's 209.1 s.
- **J3's 108.1 s unattributed on Windows is a clock wait.** The workload sits out
  `std::thread::sleep(hold)` with `hold = 3 h / SCALE`, and `SCALE` is 100 on Windows and
  1,000 elsewhere (`tests/e2e/check_visibility.rs:1382`): 108 s on Windows, 10.8 s on macOS
  and Linux, which is that column. It is the clock part the clock node moves.
- **J8's waiting is git on Windows.** Its 5,728 git processes at the floor (114 s) exceed its
  harness wait, which is the test waiting on the round's drivers. On macOS they fill half
  of it.
- **J6 is spawn-bound but not git-bound.** Its engine time is spread over about 40 short
  `adopt` and `start` runs, much the same on every platform (Linux 33.8 s, macOS 48.0,
  Windows 38.3). J7 is not git-bound either: its engine time is inside `start`'s `dispatch`,
  where the hook runs, and is 66.2 s on macOS against 10.2 s on Linux and 4.9 s on Windows.

## The sample against run 45109721932

Sum of the sample, and each journey, in the baseline's whole tier, in run 1's and run 2's
whole tiers, and alone in run 2 (W Windows, M macOS, L Linux).

| journey | W base | W r1 | W r2 | W alone | M base | M r1 | M r2 | M alone | L base | L r2 | L alone |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| J1 `redo` | 208.8 | 214.3 | 140.9 | 83.2 | 69.5 | 44.3 | 59.0 | 54.2 | 29.2 | 20.5 | 16.8 |
| J2 `out_of_band` | 203.3 | 169.5 | 123.9 | 84.6 | 59.6 | 43.7 | 62.6 | 60.5 | 30.3 | 20.9 | 15.3 |
| J3 `check_visibility` | 192.4 | 189.2 | 168.5 | 154.0 | 29.0 | 28.9 | 32.4 | 35.2 | 26.8 | 20.7 | 20.4 |
| J4 `readjust` | 103.4 | 102.2 | 72.3 | 46.4 | 27.4 | 24.4 | 31.5 | 31.2 | 14.5 | 9.9 | 8.3 |
| J5 `readjust` | 161.1 | 161.7 | 110.7 | 67.6 | 52.9 | 38.8 | 56.4 | 48.9 | 22.6 | 17.2 | 11.5 |
| J6 `review` | 68.5 | 68.1 | 57.9 | 54.7 | 54.2 | 49.2 | 57.6 | 56.3 | 43.1 | 39.0 | 36.9 |
| J7 `dispatch_env_hook` | 12.2 | 12.8 | 9.8 | 6.6 | 63.3 | 59.0 | 74.8 | 67.9 | 13.6 | 11.4 | 11.3 |
| J8 `review_round` | 139.6 | 143.4 | 104.7 | 106.9 | 73.4 | 62.3 | 81.3 | 78.0 | 29.7 | 21.6 | 24.2 |
| J9 `session_reuse` | 103.0 | 101.8 | 74.5 | 50.5 | 28.5 | 18.9 | 29.6 | 30.8 | 12.6 | 9.3 | 6.9 |
| J10 `out_of_band` | 92.6 | 63.3 | 44.6 | 29.1 | 16.6 | 12.7 | 18.3 | 13.1 | 7.0 | 4.0 | 2.4 |
| J11 `landing` | 86.4 | 93.8 | 55.9 | 34.0 | 27.7 | 17.5 | 23.1 | 22.9 | 10.7 | 8.7 | 8.0 |
| J12 `rework` | 81.6 | 82.5 | 56.3 | 33.5 | 23.0 | 17.3 | 26.4 | 26.2 | 10.8 | 8.1 | 7.7 |
| J13 `publication_boundary` | 59.0 | 58.2 | 41.5 | 28.3 | 17.0 | 12.5 | 20.9 | 20.1 | 11.3 | 8.0 | 6.1 |
| J14 `adoption` | 54.7 | 53.8 | 36.5 | 20.6 | 11.5 | 9.7 | 10.3 | 10.7 | 5.2 | 4.5 | 4.1 |
| J15 `continuation` | 50.8 | 49.5 | 33.5 | 23.7 | 21.4 | 17.4 | 22.4 | 22.0 | 8.3 | 7.2 | 7.2 |
| J16 `lifecycle` | 38.6 | 39.2 | 27.6 | 19.9 | 13.4 | 11.4 | 11.9 | 12.6 | 7.0 | 4.9 | 4.9 |
| J17 `unpublished` | 81.9 | 84.3 | 60.5 | 38.7 | 22.5 | 15.6 | 19.9 | 18.2 | 10.4 | 7.8 | 6.7 |
| J18 `stacked_drafts` | 81.3 | 82.2 | 61.4 | 52.3 | 39.7 | 31.0 | 40.3 | 40.0 | 15.7 | 12.0 | 10.3 |
| **sum** | **1819.2** | **1769.8** | **1281.0** | **934.5** | **650.6** | **514.7** | **678.5** | **648.9** | **308.7** | **235.7** | **209.1** |

Processors: Windows base and r1 EPYC 7763, r2 and alone EPYC 9V74. macOS base and r2 and alone
M1, r1 M2 Pro. Linux base EPYC 9V74, r2 and alone Xeon 6975P-C.

- **Where the processor matched, the tier reproduced the baseline.** Windows on the 7763:
  1,819.2 s and 1,769.8 s. macOS on the M1: 650.6 s and 678.5 s. So the baseline's figures are
  the tier's ordinary cost on that processor, not an outlier.
- **Contention, from one runner's tier against its own alone pass**: Windows +346.5 s on
  934.5 (+37%) on the 9V74, and +555.7 s on 1,214.1 (+46%) on the 7763 in run 1, where the
  sample alone took 1,214.1 s. macOS +29.6 s on 648.9 (+5%) on the M1 and −11.0 s on 525.7 on
  the M2 Pro in run 1. Linux +26.6 s on 209.1 (+13%). Against the baseline's processor, the
  baseline-to-alone difference also holds the processor's own difference, which is why it is
  not used for contention. Measured contention is on the sample only. The tier's other tests
  did not run alone.
- **The plan's premise holds.** Even alone, the sample costs 4.5 times Linux's on Windows
  (934.5 / 209.1) and 3.1 times on macOS (648.9 / 209.1). Contention adds to that on Windows
  and barely anywhere else. It is not the cause.

## Runner hardware

| platform | processor | e2e tier span s | summed s | tests at once |
| --- | --- | ---: | ---: | ---: |
| Windows | EPYC 7763 (baseline) | 5,402.9 | 19,763.6 | 3.66 |
| Windows | EPYC 7763 (run 1) | 5,429.4 | 19,897.4 | 3.66 |
| Windows | EPYC 9V74 (run 2) | 4,226.5 | 14,941.2 | 3.54 |
| macOS | M1, 3 CPUs (baseline) | 4,608.1 | 12,746.2 | 2.77 |
| macOS | M2 Pro, 5 CPUs (run 1) | 2,556.0 | 11,000.9 | 4.30 |
| macOS | M1, 3 CPUs (run 2) | 4,631.2 | 12,899.4 | 2.79 |
| Linux | EPYC 9V74 (baseline) | 2,347.7 | 7,612.7 | 3.24 |
| Linux | Xeon 6975P-C (run 2) | 1,966.4 | 6,434.6 | 3.27 |

The same code took 1,203 s less on Windows' 9V74 than its 7763 (5,429.4 − 4,226.5), and
2,075 s less on macOS' M2 Pro than its M1 (4,631.2 − 2,556.0). `windows-latest` and
`macos-latest` handed out both in three runs. Three runs say nothing of how often each comes.

## The tier, whole, per platform

Run 2's whole e2e tier, split as the journeys are, summed over every test. Engine git is
each test's tier-wide count from the measuring host times the runner's floor, placed within
the test's own engine and waiting time. 24.4 s on Windows and 1.9 s on macOS did not fit
there and are left out. Every row includes the 62 tests the clock node moves, and the columns marked *of it, the
62* show their part.

| | Windows, EPYC 9V74 | of it, the clock node's 62 | macOS, M1 | of it, the 62 | Linux, Xeon | of it, the 62 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| tests | 1,908 | 62 | 2,226 | 62 | 2,261 | 62 |
| summed wall s | 14,941.2 | | 12,899.4 | | 6,434.6 | |
| engine git, at the floor | 4,262.7 | 326.0 | 1,601.9 | 114.8 | 288.8 | 20.7 |
| engine other | 4,273.9 | 217.6 | 3,548.0 | 253.7 | 1,884.0 | 248.1 |
| `onepipeline` start-up, OS start and exit | 171.4 | 11.4 | 88.5 | 4.6 | 84.4 | 7.1 |
| `onepipeline` start-up, preamble | 202.7 | 14.9 | 170.4 | 7.8 | 157.0 | 13.0 |
| … of it the event registry / its validator compile | 184.8 / 78.7 | | 148.8 / 55.1 | | 140.6 / 55.4 | |
| harness git | 546.5 | 19.7 | 285.7 | 8.9 | 62.5 | 2.5 |
| fixture | 2.8 | 0.1 | 5.9 | 0.3 | 2.6 | 0.1 |
| waiting | 4,220.4 | 1,676.5 | 6,280.2 | 1,852.2 | 3,299.9 | 1,724.9 |
| unattributed | 863.2 | 206.5 | 590.1 | 103.9 | 472.2 | 95.8 |
| other programs | 397.7 | 22.3 | 328.6 | 22.5 | 183.3 | 17.7 |
| engine git processes | 214,861 | 16,350 | 228,427 | 16,350 | 228,431 | 16,350 |
| foreground `onepipeline` processes | 8,387 | | 8,895 | | 10,178 | |
| tier span s, and tests at once | 4,226.5, 3.535 | | 4,631.2, 2.785 | | 1,966.4, 3.272 | |

The engine git rows count only the tests that ran on that platform, so Windows, which runs
1,908, has fewer. Where git came from, tier-wide on the measuring host: 55% were started by a
`onepipeline` the test spawned, 20% by a detached driver, and the rest by git itself (a
local-transport `fetch` starts `sh` and `git-upload-pack`: 14,870 `sh` under git) or by
`fake-gh` (8,975). The leading commands were `rev-parse` 31,962, `git-upload-pack` 13,445,
`rev-list` 12,894, `remote` 12,742, `for-each-ref` 10,618, `fetch` 10,605 and `diff` 10,391.
`onevcs:landing_status*` spans held 810.0 s of foreground engine wall on Windows, 559.2 s on
macOS and 144.8 s on Linux.

## Ranked causes

By the minutes each would save across the whole e2e tier on each platform, as summed test
time and as wall at the tier's measured concurrency: summed divided by tests at once, the
method `docs/gate-profile.md` uses. Every gain is an estimate from these figures, not a
measurement of the change.

**1. The git processes the engine starts: at least 4,262.7 summed s on Windows, 1,601.9 on
macOS, 288.8 on Linux.** *Evidence:* the tier table, the per-journey fits (18.6, 9.5 and 2.6 ms
a git process) and the floor. *Gain if every one went:* Windows 4,262.7 s = 71.0 min summed,
4,262.7 / 3.535 = 1,205.9 s = 20.1 min of wall. macOS 26.7 min summed, 1,601.9 / 2.785 =
575.2 s = 9.6 min. Linux 4.8 min summed, 288.8 / 3.272 = 88.3 s = 1.5 min. A fraction of
them saves that fraction. *Fix it points at:* fewer git processes per engine operation, in
`onevcs` and at its call sites. Batch the reads (`cat-file --batch`, one `for-each-ref` a
pass), cache landing reads within a pass, avoid local-transport fetches between a world's own
repositories, or read in-process. *Uncertain:* it is a lower bound, because real reads cost
more than the floor and the tier more than alone. It also overlaps cause 2, since fewer
processes would also cut the contention they cause. How many spawns each change removes is
not measured.

**2. Contention on Windows: +37% (run 2) and +46% (run 1) on the sample.** *Evidence:* the
sample comparison above, 346.5 s and 555.7 s over 18 journeys. macOS +5%, Linux +13%. *Gain:*
not measured tier-wide, because only the sample ran alone. *Fix it points at:* the e2e
concurrency cap or sharding on Windows, which this plan leaves out of scope, or cause 1.
*Uncertain:* the size across the tier, and how much cause 1 removes.

**3. The harness's own git: 546.5 summed s on Windows, 285.7 on macOS, 62.5 on Linux.**
*Gain:* Windows 9.1 min summed, 546.5 / 3.535 = 154.6 s = 2.6 min of wall. macOS 4.8 min,
102.6 s. Linux 1.0 min, 19.1 s. *Fix it points at:* building each world's repositories with
fewer git processes, from a template made once a run or through `git fast-import`.
*Uncertain:* how much of it a template removes where worlds differ.

**4. `onepipeline`'s own start-up: 374.1 summed s on Windows, 258.9 on macOS, 241.4 on Linux.**
That is OS start and exit plus the binary's preamble. *Gain:* Windows 6.2 min summed,
105.8 s of wall. macOS 4.3 min, 93.0 s. Linux 4.0 min, 73.8 s. Within it, the event
registry is 184.8 / 148.8 / 140.6 s, and **onemessagebus#218's per-process validator
compilation** is 78.7 / 55.1 / 55.4 s, which is 22.3 / 19.8 / 16.9 s of wall. *Fix it points at:*
compiling validators lazily (onemessagebus#218), and building less of the registry before a
verb runs. *Uncertain:* the remaining OS start and exit is fixed per process, so only fewer
processes cut it.

**5. J7's hook failure cases on macOS: 56.6 s over Linux's in one journey.** *Evidence:*
67.9 s alone on macOS against 11.3 s on Linux, 66.2 s and 10.2 s of it engine time (63.3 s
in the baseline on macOS). *Gain:* 67.9 − 11.3 = 56.6 summed s, 56.6 / 2.785 = 20.3 s of
macOS wall. *Fix it points at:* the dispatch env hook's timeout and
output-grace path on macOS (`src/dispatchenv.rs`). *Uncertain:* which of its cases holds the
time. The spans do not reach inside the hook run.

**Not ranked with a fix, because nothing measured here splits them:**

- **Engine other: 4,273.9 / 3,548.0 / 1,884.0 summed s.** It is engine time left after
  floor-priced git. It holds git above the floor price, the engine's worker processes and the
  in-process driver's waits. The in-process driver's threads record no spans, so it is one
  figure. Splitting it needs those threads recorded.
- **Waiting outside the clock node's tests: 2,543.9 / 4,428.0 / 1,575.0 summed s.** For the
  setup-heavy journeys it is the test waiting on a background driver. J8 shows that on
  Windows such a wait can be all git.
- **Unattributed: 863.2 / 590.1 / 472.2 summed s**, of which the 62 hold 206.5 / 103.9 / 95.8
  and J3's `hold` 108.1 / 11.0 / 10.9. The rest is test-thread work outside the harness's
  spans. It was not split.
- **The runner's processor.** Not a code cause, but the largest swing measured: 1,203 s of
  Windows tier wall and 2,075 s of macOS, above.

## Where the records went

Every downloaded artifact, span record, spawn log and git trace was removed from the measuring
host once these figures were taken, as were the host's own tier records and traces. Each CI
job's artifacts keep their retention: `test-profile-test-<platform>` 30 days,
`demo-profile-<platform>` 7. The analysis scripts that turned them into these tables are on
#1830's last commit, `2c73a200`, under `.github/demo/analysis/`. Their method is the *How
each figure was taken* table.
