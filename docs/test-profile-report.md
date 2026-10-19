# onepipeline e2e test profile

This is the measurement step of the onepipeline 30-minute CI chain: profile, then crate split, then test cost, then CI speedup. It reports where each e2e test's time goes on each platform, and what a crate split would let Nx-affected selection skip. Every figure names the commit its evidence was taken on.

The records come from `onepipeline_testfakes::profile` (`crates/testfakes/src/profile.rs`, schema 2, golden `tests/golden/test-profile-record-v2.json`). Every test process writes one, at `target/test-profile/<binary>/<test>.json`. The breakdowns below come from `scripts/test-profile-report.mjs`. The replays come from `scripts/crate-split-replay.mjs` over `scripts/crate-split/proposed.json`.

## Evidence revisions

| record set | where | commit | tests | e2e test-s |
| --- | --- | --- | ---: | ---: |
| local Linux (current) | this dev host (20 cores, WSL2, shared, load 12-59), the full e2e tier instrumented as `gate` runs it, `--no-fail-fast`, run `9b7d3bb7` | 4b94e43 | 1664 | 18295.1 |
| local Linux (previous) | the same host and command, load 4-10, run `a989b0b5` | 7574f2f | 1661 | 13874.8 |
| local Linux (earlier) | the same host and command, run `80afd02e` | c551671 | 1660 | 14571.5 |
| CI Linux (head) | CI `gate`, run [39138389849](https://github.com/nickderobertis/onepipeline/actions/runs/39138389849) | 4b94e43 | 1662 | 10347.9 |
| CI Linux (current) | CI `gate`, run [39098714538](https://github.com/nickderobertis/onepipeline/actions/runs/39098714538) | dd2e5d6 | 1658 | 10407.2 |
| macOS (current) | CI `Mac PR proofs`, run 39098714538 | dd2e5d6 | 1633 | 13334.1 |
| Windows (current) | CI `cross proof (windows-latest)`, run 39098714538: **partial**, cut off by the 180-minute step limit after 1,108 of 1,413 e2e tests, with no failing test | dd2e5d6 | 1112 | 37565.0 |
| local Linux (earlier) | this dev host, load 22-87, run `ed1ae191` | a94701b | 1660 | 26665.8 |
| CI Linux (earlier) | CI `gate`, run [39063305714](https://github.com/nickderobertis/onepipeline/actions/runs/39063305714) | a94701b | 1658 | 9407.0 |
| macOS (earlier) | CI `Mac PR proofs`, run [39043863724](https://github.com/nickderobertis/onepipeline/actions/runs/39043863724) | 3c728a6 | 1631 | 12084.4 |
| Windows (earlier) | CI `cross proof (windows-latest)`, run [39028272312](https://github.com/nickderobertis/onepipeline/actions/runs/39028272312): **partial**, 1101 of 1409, step timeout | a17a96a | 1101 | 37172.2 |

Commits after dd2e5d6:
- **c551671** only moves `Cargo.lock` to the engine patch releases the manifest already permits: oneagentgraph 0.5.14, onevcs 0.52.1, onevcs-testing 0.19.5.
- **f2b9566**, the commit adding this document, writes `docs/test-profile-report.md` and nothing else.
- **c93fa56** changes product timing: a driver no longer writes a refused publications fence again before its 5 s retry is due (`src/continuation.rs`). It also adds one e2e test, `continuation::a_pass_inside_a_refused_fences_retry_does_not_write_it_early`.
- **7574f2f** and 644b02e change only this document.
- **953fda8** names the record directory once, as `profile::RECORD_DIR`. The harness writes to the same `target/test-profile` it did before, and each CI leg's upload of it now warns if it found no records. It changes no test's work, so the local figures below, taken on 7574f2f, stand for it.
- **997aaa8** merges main at 54cfa4a (#982), which splits the Windows leg into two `cross` shards. Each shard now keeps its records as `test-profile-windows-latest-<shard>`, because `upload-artifact` refuses a second artifact of one name. To read a run's Windows records, download both (`gh run download <run> -p 'test-profile-windows-latest-*' -D windows`) and pass each shard as its own label, e.g. `windows-shard-1=windows/test-profile-windows-latest-1 windows-shard-2=windows/test-profile-windows-latest-2`. The script reads one nextest run per label and sets the rest aside, and each shard is its own run. One label over both directories would therefore read only the larger shard: on run 39138389849 it read 708 tests and set 165 records aside. #982 also changes the timing of four e2e journeys, so records from after the merge are not directly comparable with these for those tests.
- **8b5655c** merges main at 4d975bc: #994 holds one driver journey's node, and #1000 moves `tests/smoke/continuation.rs` onto a `Clock`. Its wall clock's sleep is recorded through `profile::thread::sleep`.
- **a07bbdd** merges main at 1e18274: #1016 drives `waited_on` from a `Clock` (with `RealClock` and `ManualClock`) and sizes a two-source run's wait to its longest chain; #986 publishes the GitHub loopback endpoint's URL atomically. The wait still runs inside `profile::polled`, and both clocks pause through `profile::thread::sleep`, so polling waits stay attributed. `linear_writeback`'s new `StartedEndpoint` holds the profiled `Child` that `GitHubEndpoint::start` spawns. These change the timing of the `multi_source` and `linear_writeback` journeys they touch; the figures below predate them.
- **9809e12** links oneharness-core 0.28.3 (from 0.28.2), the newest its requirement permits, which `just engines-current` asked for after the merge. Only that entry's version and checksum move.
- **875a01c** merges main at 589d81b (#935). `src/continuation.rs` takes main's fence retry, which holds every refused fence to its retry, an empty one included; this branch's c93fa56 had exempted clearing. Its `continuation::a_pass_inside_a_refused_fences_retry_does_not_write_it_early` still holds under main's form. `tests/e2e/project.json` keeps both main's review-round quota record and this branch's `target/test-profile/e2e`. #935's new `review_reply` and `review_round` journeys spawn and sleep through `profile::process` and `profile::thread` like the rest of the suite. Main now links oneagentgraph 0.5.14, onevcs 0.52.1 and onevcs-testing 0.19.5 itself, so `Cargo.lock` differs from main only in oneharness-core.
- **ff19d5c** merges main at 645ca97 (#1001, #1021). `tests/e2e/harness.rs`'s `end_process` takes main's `process_ended` wait in place of `kill -0`, inside this branch's profiled spawn and sleep.
- **059dfb4** merges main at e788a5c (#904), with no conflict. #904 links oneharness-core 0.28.2, onejudge 0.19.5 and oneagentgraph 0.5.14 and adds two `dispatch` journeys, `a_dispatched_codex_turn_starts_with_plugins_off_unless_its_config_opts_in` and `a_judged_codex_turn_starts_with_plugins_off_unless_its_config_opts_in`. They spawn through `profile::process` like the rest of the suite, and `harness::oneharness_binary` builds through the harness's profiled `build`. `Cargo.lock` keeps this branch's oneharness-core 0.28.3, so it still differs from main only in that entry.
- **6c4a488** runs one maintenance journey at a three-second pace (see **maintenance on Windows, run 39129002719**). Its test-s move by a few seconds, which the figures below do not include.

The hosted dd2e5d6 records therefore predate the lock refresh and the fence fix. The local runs show neither moved the harness. The c551671 counts match the a94701b run: spawn 13,686 vs 13,575, git 9,048 vs 9,048, build 3,261 vs 3,259, fixture 7,766 vs 7,765. The 7574f2f counts are spawn 13,822, git 9,055, build 3,263 and fixture 7,773. The difference from c551671 is mostly the one added test and run-to-run variation in retries and polls.

The local 4b94e43 run (`9b7d3bb7`, on the tree after every commit above, including both merges of main) is the one every local figure below uses. It ran 1,664 tests in 5,938.0 s, at a load average of 12 to 59 on this shared host, against 4-10 for 7574f2f:
- **1,661 passed.**
- **`host_view::the_host_view_answers_a_host_sized_root_steady_cold_and_steady_again` failed**, at `tests/e2e/host_view.rs:799:5` with `"read_bytes":0`. That is the known host-only failure, which fails the same way at base 049ba74 (see **host_view on this host**).
- **Two tests hit nextest's 360 s timeout:** `adoption::a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs` and `loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on`. They were tests 1 and 5 of the run, started while the load average was 34-59. Neither is a known failure. They passed in the 7574f2f run (129.5 s and 209.7 s). Re-run together on 4b94e43 at load 10, both passed (130.1 s and 235.0 s). Their two records in this set are the runner-ended ones, cut at 355.2 s, and they are labelled unfinished below.

The new counts are spawn 13,605, git 9,063, build 3,270 and fixture 7,793, close to 7574f2f's. Test-seconds rose 32% (13,874.8 to 18,295.1), which is the load: the categories' shares moved by a few points, and the CI Linux run on the same commit (run 39138389849, 10,347.9 test-s) is within 1% of dd2e5d6's 10,407.2. #982, #994 and #1000, merged from main, change four journeys' timing, so test-level differences from 7574f2f are not all load.

### The local profile is a snapshot at 4b94e43

Every local Linux figure in this document comes from run `9b7d3bb7` on **4b94e43**. It is a snapshot of that commit. It makes no claim that those timings hold for the final head. These are the first-parent commits after 4b94e43:

| commit | what it is |
| --- | --- |
| 7996923 | this document only |
| a07bbdd | merge of main at 1e18274 (conflict: `tests/e2e/harness.rs`) |
| 9809e12 | `Cargo.lock` only: oneharness-core 0.28.2 to 0.28.3, the refresh `just engines-current` asked for after a07bbdd |
| 6aee778 | this document only |
| 875a01c | merge of main at 589d81b (conflicts: `src/continuation.rs`, `tests/e2e/project.json`) |
| 570c8b7 | this document only |
| ff19d5c | merge of main at 645ca97 (conflict: `tests/e2e/harness.rs`) |
| 41000a7, 55717cb | this document only |
| 059dfb4 | merge of main at e788a5c, no conflict |
| (the commit reporting 059dfb4) | this document only |

None of them changes this branch's recording (`crates/testfakes/src/profile.rs`), the analysis scripts (`scripts/test-profile-report.mjs`, `scripts/crate-split-replay.mjs`, `scripts/crate-split/`) or their tests. `git diff --stat 4b94e43 HEAD` over those paths and `tests/e2e/profiling.rs` is empty. The merges do bring main's own changes to tests the profile timed, listed above, so test-level figures for those journeys may differ on the head. 9809e12 is a lock refresh rather than a merge. It is listed because the amendment requires the engines-current check to pass, and it touches nothing the profile records or reads.

## Linux breakdown on 4b94e43 (local)

`node scripts/test-profile-report.mjs --top-tests 30 --top-modules 10 linux=<records>`, over the full-tier run of 4b94e43. The records were copied aside before the two timed-out tests were re-run, so this set holds that run alone.

### linux

Run `9b7d3bb7-ee30-4956-a80d-59dec3dfb0f3`: 1664 tests, 18295.1 test-s, 2 of them unfinished (ended by the runner; their figures are their last heartbeat's). Counts: spawn 13605, git 9063, build 3270, fixture 7793, wait 116942, fs 382991, polls 18447.

#### Whole run

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 1664 | 18295.1 | 63.1% | 1.4% | 10.9% | 0.0% | 20.6% | 0.4% | 3.5% |

#### The 30 costliest tests

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::host_view::the_budget_measurement_records_what_each_scaling_read_opened | 1 | 358.4 | 99.8% | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% | 0.2% |
| e2e::adoption::a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs | 1 | 355.2 | 0.6% | 0.3% | 62.4% | 0.0% | 36.6% | 0.1% | 0.2% |
| e2e::loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on | 1 | 355.2 | 2.8% | 0.0% | 0.3% | 0.0% | 83.2% | 0.1% | 13.5% |
| e2e::host_view::the_host_view_answers_a_host_sized_root_steady_cold_and_steady_again | 1 | 302.9 | 98.9% | 0.0% | 1.0% | 0.0% | 0.0% | 0.0% | 0.1% |
| e2e::listing::a_host_sized_runs_root_lists_in_seconds_and_ten_times_the_journal_bytes_barely_moves_it | 1 | 262.6 | 55.6% | 0.1% | 0.4% | 0.0% | 0.0% | 3.2% | 40.7% |
| e2e::unwatched::a_host_sized_runs_root_is_answered_in_well_under_a_second_whatever_its_journals_hold | 1 | 173.7 | 43.5% | 0.0% | 0.6% | 0.0% | 0.1% | 3.1% | 52.6% |
| e2e::review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it | 1 | 136.5 | 92.5% | 6.1% | 0.9% | 0.0% | 0.0% | 0.1% | 0.3% |
| e2e::linear_writeback::a_linear_destination_lands_current_values_after_each_way_a_call_fails | 1 | 109.2 | 6.8% | 0.0% | 1.0% | 0.0% | 91.8% | 0.0% | 0.3% |
| e2e::out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique | 1 | 85.9 | 97.0% | 1.1% | 1.1% | 0.1% | 0.0% | 0.3% | 0.6% |
| e2e::linear_writeback::command_end_refusal_and_timeout_rebuild_the_workers_store | 1 | 84.4 | 4.5% | 0.0% | 0.8% | 0.0% | 94.4% | 0.0% | 0.3% |
| e2e::writeback_budget::a_seven_item_copy_held_at_eleven_seconds_per_item_lands_inside_its_deadline | 1 | 83.2 | 2.3% | 0.0% | 1.1% | 0.0% | 96.4% | 0.0% | 0.2% |
| e2e::delivers::a_first_projection_held_past_its_deadline_does_not_hold_back_the_first_dispatch | 1 | 78.0 | 4.4% | 0.0% | 1.9% | 0.0% | 93.0% | 0.2% | 0.4% |
| e2e::writeback_budget::a_record_an_older_build_wrote_is_adopted_and_its_update_runs_under_the_shipped_default | 1 | 75.7 | 29.7% | 0.0% | 1.3% | 0.0% | 68.5% | 0.1% | 0.4% |
| e2e::continuation::a_publication_held_past_the_whole_takeover_wait_holds_the_takeover_until_it_has_finished | 1 | 75.5 | 3.6% | 1.0% | 1.6% | 0.0% | 93.6% | 0.1% | 0.2% |
| e2e::continuation::a_takeover_of_an_idle_driver_starts_on_its_acknowledgement_within_the_interval | 1 | 74.0 | 8.1% | 0.9% | 0.9% | 0.0% | 8.8% | 0.1% | 81.2% |
| e2e::writeback_budget::a_targeted_update_held_past_the_floor_still_lands_inside_its_item_budget | 1 | 72.2 | 2.5% | 0.0% | 0.9% | 0.0% | 96.3% | 0.0% | 0.2% |
| e2e::linear_writeback::a_retained_stores_blocked_configuration_read_is_bounded_and_rebuilt | 1 | 71.3 | 4.4% | 0.0% | 1.2% | 0.0% | 93.7% | 0.0% | 0.7% |
| e2e::dispatch::a_legacy_plan_path_launch_record_is_still_reportable_and_adoptable | 1 | 70.7 | 96.2% | 0.0% | 3.6% | 0.0% | 0.0% | 0.0% | 0.1% |
| e2e::loopcost::another_runs_ledger_is_read_on_its_own_interval_and_not_on_the_loops | 1 | 70.2 | 9.9% | 0.0% | 1.8% | 0.0% | 88.2% | 0.0% | 0.1% |
| e2e::writeback_budget::a_creating_copy_held_past_a_tiny_budget_is_cancelled_and_the_next_attempt_creates_it | 1 | 70.0 | 2.2% | 0.0% | 1.4% | 0.0% | 96.0% | 0.2% | 0.2% |
| e2e::loopcost::a_converged_run_does_no_scheduling_work_while_it_records_nothing | 1 | 69.0 | 3.6% | 0.0% | 5.1% | 0.0% | 91.0% | 0.1% | 0.2% |
| e2e::continuation::a_settlement_dispatch_or_claim_that_is_not_one_refuses_the_launch | 1 | 67.7 | 28.4% | 0.2% | 1.2% | 0.0% | 68.4% | 0.1% | 1.7% |
| e2e::writeback_budget::an_update_held_past_a_tiny_budget_is_cancelled_and_the_refusal_names_the_arithmetic | 1 | 67.6 | 33.5% | 0.0% | 1.1% | 0.0% | 64.9% | 0.1% | 0.5% |
| e2e::adoption::a_cross_dag_dependency_is_pinned_against_git_and_named_from_the_upstreams_ledger | 1 | 66.5 | 5.9% | 0.7% | 3.9% | 0.0% | 88.5% | 0.8% | 0.1% |
| e2e::continuation::an_in_flight_dispatch_left_on_the_store_is_continued_by_a_host_that_takes_the_project_over | 1 | 66.0 | 93.8% | 1.4% | 1.7% | 0.0% | 2.6% | 0.1% | 0.4% |
| e2e::dispatch_env_hook::a_hook_that_fails_times_out_cannot_start_or_prints_a_malformed_document_refuses_the_launch | 1 | 66.0 | 98.0% | 0.0% | 1.3% | 0.0% | 0.0% | 0.2% | 0.5% |
| e2e::store::a_run_whose_store_is_rate_limited_throughout_settles_as_it_does_against_a_healthy_one | 1 | 64.6 | 92.5% | 0.0% | 1.3% | 0.0% | 5.8% | 0.1% | 0.3% |
| e2e::writeback_budget::a_destination_read_held_past_the_floor_is_cancelled_retried_and_recovers | 1 | 64.2 | 2.5% | 0.0% | 1.5% | 0.0% | 95.7% | 0.2% | 0.1% |
| e2e::driver::a_legacy_unqualified_project_does_not_prevent_adoption | 1 | 63.4 | 98.6% | 0.0% | 1.3% | 0.0% | 0.0% | 0.0% | 0.1% |
| e2e::writeback_budget::a_store_that_does_not_open_within_the_floor_is_refused_retried_and_recovers | 1 | 62.8 | 1.7% | 0.0% | 1.3% | 0.0% | 96.4% | 0.2% | 0.5% |

#### The 10 costliest modules

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::continuation | 44 | 1401.0 | 40.5% | 1.7% | 3.4% | 0.0% | 44.4% | 0.2% | 9.8% |
| e2e::adoption | 51 | 1359.3 | 21.2% | 2.9% | 20.9% | 0.1% | 53.7% | 0.6% | 0.6% |
| e2e::lifecycle | 102 | 1307.4 | 80.4% | 1.9% | 8.6% | 0.0% | 7.9% | 0.3% | 0.8% |
| e2e::readjust | 44 | 907.5 | 91.8% | 1.2% | 4.1% | 0.0% | 2.0% | 0.2% | 0.6% |
| e2e::host_view | 10 | 706.7 | 96.6% | 0.0% | 2.8% | 0.0% | 0.3% | 0.0% | 0.3% |
| e2e::rework | 20 | 653.9 | 91.7% | 1.0% | 3.4% | 0.0% | 3.0% | 0.2% | 0.7% |
| e2e::dispatch | 71 | 549.1 | 66.2% | 0.5% | 28.7% | 0.1% | 2.9% | 0.4% | 1.3% |
| e2e::writeback_budget | 12 | 528.9 | 15.2% | 0.0% | 2.1% | 0.0% | 82.2% | 0.1% | 0.4% |
| e2e::loopcost | 10 | 527.8 | 6.5% | 0.0% | 2.7% | 0.0% | 81.2% | 0.1% | 9.4% |
| e2e::store | 46 | 513.1 | 44.1% | 0.1% | 8.7% | 0.0% | 40.4% | 0.3% | 6.3% |

This host is shared with other dispatches, so the wall figures move with load. The categories' shares and the counts are what to compare across runs.

## Hosted platforms on 4b94e43 (run 39138389849)

`node scripts/test-profile-report.mjs --top-tests 30 --top-modules 10 --baseline ci-linux ci-linux=<gate> macos=<macos-latest> windows-shard-1=<windows-latest-1> windows-shard-2=<windows-latest-2>`, over the run's `test-profile-*` artifacts. The run is on this branch's head, after both merges of main.

What the run produced:
- **`gate` and `Mac PR proofs`** passed, so CI Linux (1,662 e2e tests) and macOS (1,637) are whole.
- **Windows shard 2** (job 117711816624) passed: 708 e2e tests.
- **Windows shard 1** (job 117711816829) stopped at its first failure, which nextest's fail-fast ends the e2e tier on. It holds 136 e2e tests, and its other 573 were not run. That failure was `continuation::the_claim_is_held_while_a_run_drives_and_released_at_closeout_and_at_stop` (see **maintenance on Windows, run 39129002719**).

So the Windows figures cover 844 of about 1,417 e2e tests. There is no whole-Windows ratio on 4b94e43: the script compares each shard on the tests it ran, and the shards' ratios are not summed here. The whole-tier Windows comparison available is dd2e5d6's, below, which is itself partial (1,108 tests).

### ci-linux

Run `17be4f0f-3578-4460-8ece-9d026f66eef7`: 1662 tests, 10347.9 test-s (32 records of other runs set aside). Counts: spawn 14091, git 9063, build 3266, fixture 7787, wait 83946, fs 373328, polls 18452.

#### Whole run

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 1662 | 10347.9 | 61.1% | 0.7% | 8.4% | 0.0% | 27.0% | 0.2% | 2.5% |

#### The 30 costliest tests

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::host_view::the_budget_measurement_records_what_each_scaling_read_opened | 1 | 153.5 | 99.9% | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% |
| e2e::loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on | 1 | 136.0 | 3.4% | 0.0% | 0.3% | 0.0% | 96.2% | 0.1% | 0.1% |
| e2e::adoption::a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs | 1 | 125.4 | 0.9% | 0.1% | 0.3% | 0.0% | 98.5% | 0.1% | 0.0% |
| e2e::linear_writeback::a_linear_destination_lands_current_values_after_each_way_a_call_fails | 1 | 103.2 | 3.9% | 0.0% | 0.5% | 0.0% | 95.4% | 0.0% | 0.2% |
| e2e::review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it | 1 | 100.2 | 97.9% | 1.4% | 0.4% | 0.0% | 0.0% | 0.1% | 0.2% |
| e2e::linear_writeback::command_end_refusal_and_timeout_rebuild_the_workers_store | 1 | 80.1 | 2.7% | 0.0% | 0.5% | 0.0% | 96.7% | 0.0% | 0.1% |
| e2e::writeback_budget::a_seven_item_copy_held_at_eleven_seconds_per_item_lands_inside_its_deadline | 1 | 79.0 | 0.7% | 0.0% | 0.5% | 0.0% | 98.8% | 0.0% | 0.0% |
| e2e::writeback_budget::a_record_an_older_build_wrote_is_adopted_and_its_update_runs_under_the_shipped_default | 1 | 73.2 | 28.3% | 0.0% | 0.6% | 0.0% | 70.9% | 0.0% | 0.1% |
| e2e::delivers::a_first_projection_held_past_its_deadline_does_not_hold_back_the_first_dispatch | 1 | 73.0 | 0.5% | 0.0% | 0.8% | 0.0% | 98.6% | 0.1% | 0.1% |
| e2e::continuation::a_publication_held_past_the_whole_takeover_wait_holds_the_takeover_until_it_has_finished | 1 | 70.9 | 2.3% | 0.2% | 0.6% | 0.0% | 96.9% | 0.0% | 0.1% |
| e2e::listing::a_host_sized_runs_root_lists_in_seconds_and_ten_times_the_journal_bytes_barely_moves_it | 1 | 70.0 | 58.7% | 0.1% | 0.5% | 0.0% | 0.0% | 2.4% | 38.3% |
| e2e::linear_writeback::a_retained_stores_blocked_configuration_read_is_bounded_and_rebuilt | 1 | 68.1 | 1.3% | 0.0% | 0.7% | 0.0% | 97.7% | 0.0% | 0.3% |
| e2e::writeback_budget::a_creating_copy_held_past_a_tiny_budget_is_cancelled_and_the_next_attempt_creates_it | 1 | 67.7 | 0.8% | 0.0% | 0.7% | 0.0% | 98.4% | 0.1% | 0.0% |
| e2e::writeback_budget::a_targeted_update_held_past_the_floor_still_lands_inside_its_item_budget | 1 | 66.8 | 0.6% | 0.0% | 0.6% | 0.0% | 98.8% | 0.0% | 0.0% |
| e2e::continuation::a_takeover_of_an_idle_driver_starts_on_its_acknowledgement_within_the_interval | 1 | 65.3 | 5.6% | 0.2% | 0.6% | 0.0% | 1.7% | 0.0% | 91.9% |
| e2e::dispatch::a_legacy_plan_path_launch_record_is_still_reportable_and_adoptable | 1 | 65.1 | 99.4% | 0.0% | 0.6% | 0.0% | 0.0% | 0.0% | 0.0% |
| e2e::writeback_budget::an_update_held_past_a_tiny_budget_is_cancelled_and_the_refusal_names_the_arithmetic | 1 | 65.0 | 31.8% | 0.0% | 0.6% | 0.0% | 67.5% | 0.1% | 0.1% |
| e2e::loopcost::another_runs_ledger_is_read_on_its_own_interval_and_not_on_the_loops | 1 | 64.5 | 4.7% | 0.0% | 0.6% | 0.0% | 94.7% | 0.0% | 0.0% |
| e2e::loopcost::a_converged_run_does_no_scheduling_work_while_it_records_nothing | 1 | 62.7 | 0.4% | 0.0% | 0.6% | 0.0% | 98.9% | 0.0% | 0.0% |
| e2e::writeback_budget::a_destination_read_held_past_the_floor_is_cancelled_retried_and_recovers | 1 | 61.7 | 0.8% | 0.0% | 0.8% | 0.0% | 98.3% | 0.1% | 0.0% |
| e2e::host_view::the_host_view_answers_a_host_sized_root_steady_cold_and_steady_again | 1 | 61.4 | 99.1% | 0.0% | 0.6% | 0.0% | 0.0% | 0.0% | 0.3% |
| e2e::driver::a_legacy_unqualified_project_does_not_prevent_adoption | 1 | 61.2 | 99.1% | 0.0% | 0.9% | 0.0% | 0.0% | 0.0% | 0.0% |
| e2e::store::a_run_whose_store_is_rate_limited_throughout_settles_as_it_does_against_a_healthy_one | 1 | 61.2 | 95.2% | 0.0% | 0.8% | 0.0% | 4.0% | 0.0% | 0.0% |
| e2e::writeback_budget::a_store_that_does_not_open_within_the_floor_is_refused_retried_and_recovers | 1 | 61.0 | 0.3% | 0.0% | 0.6% | 0.0% | 98.8% | 0.1% | 0.2% |
| e2e::unwatched::a_host_sized_runs_root_is_answered_in_well_under_a_second_whatever_its_journals_hold | 1 | 57.4 | 49.4% | 0.0% | 0.6% | 0.0% | 0.1% | 3.1% | 46.8% |
| e2e::out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique | 1 | 56.6 | 98.2% | 0.6% | 0.9% | 0.0% | 0.0% | 0.2% | 0.1% |
| e2e::continuation::a_settlement_dispatch_or_claim_that_is_not_one_refuses_the_launch | 1 | 55.8 | 15.9% | 0.1% | 0.7% | 0.0% | 82.7% | 0.0% | 0.6% |
| e2e::continuation::an_in_flight_dispatch_left_on_the_store_is_continued_by_a_host_that_takes_the_project_over | 1 | 53.8 | 97.4% | 0.2% | 0.7% | 0.0% | 1.5% | 0.1% | 0.1% |
| e2e::adoption::a_cross_dag_dependency_is_pinned_against_git_and_named_from_the_upstreams_ledger | 1 | 51.6 | 1.8% | 0.3% | 2.4% | 0.0% | 95.1% | 0.5% | 0.0% |
| e2e::continuation::a_takeover_finding_a_fence_naming_no_publication_waits_until_it_is_cleared | 1 | 50.6 | 5.8% | 0.2% | 0.8% | 0.0% | 92.1% | 0.1% | 1.0% |

#### The 10 costliest modules

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::continuation | 44 | 1098.5 | 35.2% | 0.3% | 1.7% | 0.0% | 52.3% | 0.1% | 10.4% |
| e2e::lifecycle | 102 | 744.2 | 85.6% | 0.8% | 6.7% | 0.0% | 6.5% | 0.2% | 0.1% |
| e2e::readjust | 44 | 621.2 | 92.1% | 0.6% | 4.1% | 0.0% | 2.5% | 0.2% | 0.4% |
| e2e::adoption | 51 | 607.9 | 21.6% | 1.6% | 4.9% | 0.0% | 71.2% | 0.4% | 0.3% |
| e2e::writeback_budget | 12 | 495.3 | 12.3% | 0.0% | 1.2% | 0.0% | 86.3% | 0.0% | 0.1% |
| e2e::store | 46 | 365.2 | 37.7% | 0.0% | 5.7% | 0.0% | 49.7% | 0.1% | 6.8% |
| e2e::rework | 20 | 349.3 | 91.4% | 0.4% | 3.1% | 0.0% | 4.4% | 0.2% | 0.5% |
| e2e::linear_writeback | 10 | 326.6 | 4.2% | 0.0% | 1.2% | 0.0% | 94.4% | 0.0% | 0.2% |
| e2e::review | 21 | 294.1 | 91.8% | 1.1% | 3.4% | 0.0% | 3.2% | 0.2% | 0.3% |
| e2e::loopcost | 9 | 276.0 | 4.7% | 0.0% | 1.9% | 0.0% | 93.2% | 0.1% | 0.1% |

### macos

Run `61ac9f98-5e1f-4afd-ba08-b3a6f743dd9b`: 1637 tests, 13165.5 test-s (30 records of other runs set aside). Counts: spawn 13379, git 9044, build 3215, fixture 7721, wait 25205, fs 319515, polls 18387.

#### Whole run

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 1637 | 13165.5 | 62.9% | 2.0% | 6.3% | 0.0% | 26.4% | 0.3% | 2.0% |

#### The 30 costliest tests

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on | 1 | 130.0 | 2.5% | 0.0% | 0.4% | 0.0% | 96.9% | 0.1% | 0.1% |
| e2e::adoption::a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs | 1 | 128.9 | 1.5% | 0.2% | 0.2% | 0.0% | 97.9% | 0.1% | 0.0% |
| e2e::dispatch_env_hook::a_hook_that_fails_times_out_cannot_start_or_prints_a_malformed_document_refuses_the_launch | 1 | 114.7 | 99.2% | 0.0% | 0.4% | 0.0% | 0.0% | 0.2% | 0.2% |
| e2e::review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it | 1 | 106.3 | 97.5% | 2.0% | 0.3% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::store::a_cache_enabled_github_store_reads_and_writes_like_an_uncached_store | 1 | 96.8 | 1.8% | 0.0% | 0.9% | 0.0% | 73.8% | 0.0% | 23.5% |
| e2e::linear_writeback::a_linear_destination_lands_current_values_after_each_way_a_call_fails | 1 | 92.9 | 4.8% | 0.0% | 1.2% | 0.0% | 93.7% | 0.0% | 0.2% |
| e2e::out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique | 1 | 88.7 | 97.6% | 1.5% | 0.4% | 0.1% | 0.0% | 0.2% | 0.1% |
| e2e::writeback_budget::a_seven_item_copy_held_at_eleven_seconds_per_item_lands_inside_its_deadline | 1 | 80.3 | 0.8% | 0.0% | 0.9% | 0.0% | 98.2% | 0.0% | 0.1% |
| e2e::continuation::a_publication_held_past_the_whole_takeover_wait_holds_the_takeover_until_it_has_finished | 1 | 75.4 | 3.5% | 0.4% | 0.6% | 0.0% | 95.4% | 0.0% | 0.0% |
| e2e::delivers::a_first_projection_held_past_its_deadline_does_not_hold_back_the_first_dispatch | 1 | 75.3 | 0.4% | 0.0% | 0.6% | 0.0% | 98.8% | 0.1% | 0.0% |
| e2e::linear_writeback::command_end_refusal_and_timeout_rebuild_the_workers_store | 1 | 74.2 | 2.6% | 0.0% | 0.6% | 0.0% | 96.6% | 0.0% | 0.2% |
| e2e::writeback_budget::a_record_an_older_build_wrote_is_adopted_and_its_update_runs_under_the_shipped_default | 1 | 73.4 | 28.6% | 0.0% | 0.7% | 0.0% | 70.6% | 0.1% | 0.1% |
| e2e::writeback_budget::a_creating_copy_held_past_a_tiny_budget_is_cancelled_and_the_next_attempt_creates_it | 1 | 67.6 | 0.9% | 0.0% | 0.6% | 0.0% | 98.3% | 0.2% | 0.1% |
| e2e::writeback_budget::a_targeted_update_held_past_the_floor_still_lands_inside_its_item_budget | 1 | 67.3 | 0.5% | 0.0% | 0.5% | 0.0% | 98.9% | 0.0% | 0.0% |
| e2e::continuation::a_takeover_of_an_idle_driver_starts_on_its_acknowledgement_within_the_interval | 1 | 67.3 | 5.9% | 0.4% | 0.7% | 0.0% | 4.0% | 0.0% | 89.1% |
| e2e::writeback_budget::an_update_held_past_a_tiny_budget_is_cancelled_and_the_refusal_names_the_arithmetic | 1 | 65.7 | 32.0% | 0.0% | 1.1% | 0.0% | 66.7% | 0.1% | 0.1% |
| e2e::continuation::an_in_flight_dispatch_left_on_the_store_is_continued_by_a_host_that_takes_the_project_over | 1 | 65.6 | 96.1% | 0.6% | 0.8% | 0.0% | 2.3% | 0.1% | 0.1% |
| e2e::dispatch::a_legacy_plan_path_launch_record_is_still_reportable_and_adoptable | 1 | 65.6 | 99.1% | 0.0% | 0.8% | 0.0% | 0.0% | 0.0% | 0.0% |
| e2e::loopcost::another_runs_ledger_is_read_on_its_own_interval_and_not_on_the_loops | 1 | 65.2 | 5.2% | 0.0% | 0.8% | 0.0% | 93.9% | 0.0% | 0.0% |
| e2e::linear_writeback::a_retained_stores_blocked_configuration_read_is_bounded_and_rebuilt | 1 | 65.0 | 1.5% | 0.0% | 0.7% | 0.0% | 97.5% | 0.0% | 0.4% |
| e2e::loopcost::a_converged_run_does_no_scheduling_work_while_it_records_nothing | 1 | 63.7 | 0.4% | 0.0% | 1.1% | 0.0% | 98.4% | 0.0% | 0.0% |
| e2e::writeback_budget::a_destination_read_held_past_the_floor_is_cancelled_retried_and_recovers | 1 | 62.3 | 1.2% | 0.0% | 0.6% | 0.0% | 98.0% | 0.1% | 0.0% |
| e2e::driver::a_legacy_unqualified_project_does_not_prevent_adoption | 1 | 61.7 | 98.8% | 0.0% | 1.0% | 0.0% | 0.2% | 0.0% | 0.0% |
| e2e::store::a_run_whose_store_is_rate_limited_throughout_settles_as_it_does_against_a_healthy_one | 1 | 61.5 | 94.2% | 0.0% | 0.8% | 0.0% | 5.0% | 0.0% | 0.0% |
| e2e::landing::a_render_asks_the_landing_read_once_per_node_it_prints_and_does_nothing_else_per_node | 1 | 61.5 | 95.6% | 3.3% | 0.9% | 0.0% | 0.0% | 0.2% | 0.0% |
| e2e::continuation::a_settlement_dispatch_or_claim_that_is_not_one_refuses_the_launch | 1 | 61.5 | 23.2% | 0.3% | 0.6% | 0.0% | 75.4% | 0.1% | 0.5% |
| e2e::writeback_budget::a_store_that_does_not_open_within_the_floor_is_refused_retried_and_recovers | 1 | 61.4 | 0.3% | 0.0% | 0.7% | 0.0% | 98.7% | 0.2% | 0.2% |
| e2e::adoption::a_cross_dag_dependency_is_pinned_against_git_and_named_from_the_upstreams_ledger | 1 | 58.0 | 1.5% | 0.9% | 1.5% | 0.0% | 95.9% | 0.1% | 0.0% |
| e2e::rework::a_title_nothing_verified_is_left_unverified_until_a_retitle_sets_one | 1 | 55.8 | 97.6% | 0.3% | 1.4% | 0.0% | 0.0% | 0.2% | 0.5% |
| e2e::stacked_drafts::landing_dependents_of_unlanded_parents_are_drafted_on_them | 1 | 55.7 | 98.2% | 0.6% | 0.8% | 0.0% | 0.0% | 0.1% | 0.2% |

#### The 10 costliest modules

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::continuation | 44 | 1274.0 | 39.4% | 0.7% | 1.9% | 0.0% | 47.9% | 0.1% | 9.9% |
| e2e::readjust | 44 | 1162.2 | 93.3% | 1.5% | 2.9% | 0.0% | 2.0% | 0.2% | 0.2% |
| e2e::lifecycle | 102 | 1049.0 | 82.7% | 2.3% | 4.8% | 0.0% | 9.8% | 0.3% | 0.1% |
| e2e::adoption | 51 | 839.4 | 16.4% | 3.6% | 3.1% | 0.1% | 76.3% | 0.3% | 0.2% |
| e2e::rework | 20 | 670.1 | 91.3% | 1.1% | 2.8% | 0.0% | 4.3% | 0.2% | 0.3% |
| e2e::writeback_budget | 12 | 499.2 | 12.6% | 0.0% | 1.2% | 0.0% | 86.0% | 0.1% | 0.1% |
| e2e::store | 46 | 452.6 | 32.8% | 0.2% | 4.1% | 0.0% | 57.3% | 0.2% | 5.4% |
| e2e::linear_writeback | 11 | 411.6 | 4.3% | 0.0% | 1.4% | 0.0% | 94.2% | 0.0% | 0.2% |
| e2e::review | 21 | 364.6 | 91.2% | 1.8% | 2.2% | 0.0% | 4.3% | 0.2% | 0.2% |
| e2e::publication_boundary | 35 | 318.4 | 72.5% | 6.0% | 5.7% | 0.0% | 15.0% | 0.5% | 0.3% |

### windows-shard-1

Run `40a2ff79-d9bf-4fe4-8a4b-2f834d5a801d`: 136 tests, 4749.6 test-s (15 records of other runs set aside). Counts: spawn 1593, git 865, build 270, fixture 740, wait 63360, fs 82300, polls 7875.

#### Whole run

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 136 | 4749.6 | 33.7% | 3.9% | 5.2% | 0.0% | 52.6% | 0.7% | 3.7% |

#### The 30 costliest tests

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::listing::a_host_sized_runs_root_lists_in_seconds_and_ten_times_the_journal_bytes_barely_moves_it | 1 | 162.7 | 44.5% | 0.4% | 0.6% | 0.0% | 0.0% | 1.6% | 52.9% |
| e2e::adoption::the_elapsed_wait_a_supervisor_is_shown_counts_from_the_hold_that_is_running | 1 | 155.9 | 37.4% | 2.9% | 1.3% | 0.0% | 57.6% | 0.7% | 0.0% |
| e2e::adoption::a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs | 1 | 154.2 | 2.1% | 0.9% | 0.6% | 0.0% | 96.0% | 0.4% | 0.1% |
| e2e::adoption::a_change_request_settled_with_no_release_baseline_is_answered_with_the_command_to_record_it | 1 | 151.2 | 16.7% | 2.6% | 1.6% | 0.0% | 78.7% | 0.4% | 0.0% |
| e2e::adoption::a_release_stated_on_the_settle_is_recorded_and_releases_the_node_waiting_on_it | 1 | 143.1 | 20.0% | 2.4% | 1.7% | 0.0% | 75.2% | 0.6% | 0.0% |
| e2e::continuation::a_node_added_on_a_continued_node_is_drafted_on_its_recorded_branch | 1 | 141.5 | 97.5% | 1.1% | 1.2% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::continuation::an_in_flight_dispatch_left_on_the_store_is_continued_by_a_host_that_takes_the_project_over | 1 | 127.3 | 91.2% | 1.0% | 0.9% | 0.0% | 6.6% | 0.1% | 0.1% |
| e2e::continuation::a_settlement_dispatch_or_claim_that_is_not_one_refuses_the_launch | 1 | 127.3 | 60.0% | 0.8% | 1.5% | 0.0% | 36.8% | 0.1% | 0.7% |
| e2e::adoption::a_fast_node_whose_release_is_not_out_settles_complete_but_draft_and_nothing_merges_it | 1 | 117.3 | 10.0% | 19.9% | 1.7% | 0.0% | 63.8% | 0.9% | 3.7% |
| e2e::adoption::an_unchanged_release_wait_is_queued_again_only_once_read_and_a_changed_one_at_once | 1 | 117.1 | 56.0% | 3.6% | 1.3% | 0.0% | 37.7% | 1.0% | 0.3% |
| e2e::adoption::a_published_node_whose_dependency_cannot_be_resolved_is_held_until_it_can_be | 1 | 116.6 | 43.0% | 2.9% | 1.6% | 0.0% | 51.5% | 0.8% | 0.1% |
| e2e::adoption::a_cross_dag_dependency_settled_from_evidence_is_correlated_through_its_landing | 1 | 108.6 | 10.7% | 3.6% | 2.5% | 0.0% | 82.0% | 1.2% | 0.0% |
| e2e::adoption::two_producers_that_state_the_same_instruction_are_coalesced | 1 | 108.6 | 3.1% | 4.8% | 1.8% | 0.0% | 89.0% | 1.3% | 0.0% |
| e2e::adoption::the_two_release_styles_take_one_scheduling_path_and_are_reported_apart | 1 | 102.5 | 4.6% | 10.6% | 2.3% | 0.0% | 81.1% | 1.4% | 0.1% |
| e2e::adoption::a_squash_commit_baselined_before_any_release_is_answered_through_its_change_request | 1 | 100.7 | 16.9% | 4.0% | 2.0% | 0.0% | 76.4% | 0.7% | 0.0% |
| e2e::adoption::a_node_settled_from_evidence_is_correlated_through_the_change_request_it_names | 1 | 99.9 | 9.3% | 4.4% | 2.3% | 0.0% | 83.1% | 0.8% | 0.0% |
| e2e::adoption::a_release_that_arrives_puts_a_worker_back_on_the_same_branch_and_lifts_the_draft | 1 | 99.1 | 2.5% | 3.1% | 2.1% | 0.0% | 90.9% | 1.3% | 0.1% |
| e2e::adoption::a_node_settled_at_a_change_request_reads_landed_in_every_surface | 1 | 95.3 | 22.8% | 5.5% | 2.6% | 0.0% | 68.2% | 0.9% | 0.1% |
| e2e::adoption::a_published_node_held_on_a_landing_with_no_baseline_is_released_by_its_acknowledgement | 1 | 91.9 | 9.5% | 2.7% | 1.6% | 0.0% | 85.0% | 1.2% | 0.0% |
| e2e::adoption::an_answer_this_host_cannot_read_is_never_read_as_a_release_that_has_not_happened | 1 | 90.8 | 3.3% | 3.2% | 2.1% | 0.0% | 90.2% | 1.2% | 0.1% |
| e2e::adoption::a_fast_node_pins_against_git_and_is_told_when_the_release_arrives | 1 | 88.4 | 3.6% | 3.7% | 2.4% | 0.0% | 89.3% | 1.0% | 0.0% |
| e2e::continuation::a_takeover_while_the_displaced_driver_publishes_starts_once_the_publication_has_finished | 1 | 87.0 | 9.7% | 1.1% | 1.5% | 0.0% | 87.4% | 0.2% | 0.0% |
| e2e::adoption::a_published_node_is_never_held_as_a_draft_and_settles_done_on_its_release | 1 | 85.3 | 4.0% | 3.2% | 2.4% | 0.0% | 89.1% | 1.2% | 0.1% |
| e2e::adoption::a_probe_that_could_not_answer_holds_the_node_and_is_never_read_as_not_released | 1 | 84.2 | 4.6% | 5.0% | 3.1% | 0.0% | 86.0% | 1.3% | 0.0% |
| e2e::adoption::a_dependency_inside_the_nodes_own_repository_is_not_pinned_against_git | 1 | 79.4 | 1.7% | 2.5% | 3.0% | 0.0% | 91.6% | 1.0% | 0.0% |
| e2e::continuation::a_takeover_finding_a_fence_naming_no_publication_waits_until_it_is_cleared | 1 | 74.2 | 12.4% | 3.1% | 2.7% | 0.0% | 73.5% | 0.2% | 8.1% |
| e2e::continuation::a_takeover_whose_displaced_driver_died_proceeds_once_its_lease_has_run_out | 1 | 73.3 | 65.0% | 2.1% | 1.6% | 0.0% | 24.1% | 0.2% | 7.0% |
| e2e::loopcost::another_runs_ledger_is_read_on_its_own_interval_and_not_on_the_loops | 1 | 71.3 | 5.7% | 0.0% | 8.1% | 0.0% | 86.0% | 0.1% | 0.0% |
| e2e::adoption::a_deferred_arrival_note_rides_the_next_dispatch_and_is_consumed_by_it | 1 | 70.0 | 11.5% | 3.7% | 2.8% | 0.0% | 81.0% | 0.9% | 0.0% |
| e2e::continuation::a_landed_project_relaunches_continuing_it_and_runs_afresh_with_fresh | 1 | 64.3 | 96.5% | 1.1% | 2.1% | 0.0% | 0.0% | 0.2% | 0.1% |

#### The 10 costliest modules

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::adoption | 26 | 2448.3 | 16.2% | 4.7% | 2.2% | 0.0% | 75.7% | 0.9% | 0.2% |
| e2e::continuation | 22 | 1277.5 | 50.1% | 1.7% | 2.7% | 0.0% | 39.1% | 0.3% | 6.1% |
| e2e::branch_template | 8 | 178.2 | 87.3% | 4.6% | 7.7% | 0.0% | 0.0% | 0.4% | 0.1% |
| e2e::listing | 1 | 162.7 | 44.5% | 0.4% | 0.6% | 0.0% | 0.0% | 1.6% | 52.9% |
| e2e::change_telemetry | 5 | 152.3 | 92.3% | 2.3% | 5.1% | 0.0% | 0.0% | 0.3% | 0.1% |
| e2e::maintenance | 10 | 131.8 | 17.9% | 28.8% | 7.1% | 0.0% | 45.1% | 0.8% | 0.4% |
| e2e::channel | 29 | 125.4 | 46.6% | 0.0% | 41.5% | 0.0% | 6.1% | 1.2% | 4.6% |
| e2e::loopcost | 1 | 71.3 | 5.7% | 0.0% | 8.1% | 0.0% | 86.0% | 0.1% | 0.0% |
| e2e::ask | 4 | 36.7 | 70.6% | 0.0% | 27.6% | 0.0% | 0.7% | 0.5% | 0.6% |
| e2e::boundary | 5 | 30.3 | 60.0% | 0.0% | 38.4% | 0.0% | 0.2% | 0.7% | 0.7% |

### windows-shard-2

Run `11712a6e-3089-4df3-a36f-b305f5ac054c`: 708 tests, 20321.5 test-s (14 records of other runs set aside). Counts: spawn 4848, git 3791, build 1377, fixture 3423, wait 120675, fs 210755, polls 7286.

#### Whole run

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 708 | 20321.5 | 68.4% | 3.1% | 6.3% | 0.0% | 19.6% | 0.5% | 2.1% |

#### The 30 costliest tests

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique | 1 | 371.9 | 97.1% | 1.9% | 0.7% | 0.0% | 0.0% | 0.3% | 0.0% |
| e2e::readjust::a_whole_lower_stack_landing_moves_every_draft_above_it_in_one_readjustment | 1 | 366.6 | 98.3% | 0.7% | 0.7% | 0.0% | 0.0% | 0.1% | 0.2% |
| e2e::readjust::deferred_republications_are_read_by_the_next_trigger_green_or_repaired_red | 1 | 315.7 | 98.9% | 0.4% | 0.6% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::readjust::every_deferred_verdict_word_is_recorded_and_only_pending_stays_outstanding | 1 | 277.5 | 98.0% | 0.9% | 0.9% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it | 1 | 257.4 | 94.1% | 4.9% | 0.6% | 0.0% | 0.0% | 0.3% | 0.1% |
| e2e::rework::a_rework_conflicting_with_the_draft_above_is_concluded_by_that_draft_s_worker | 1 | 247.8 | 98.2% | 0.6% | 0.9% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::landing::a_render_asks_the_landing_read_once_per_node_it_prints_and_does_nothing_else_per_node | 1 | 239.0 | 97.8% | 1.4% | 0.7% | 0.0% | 0.0% | 0.1% | 0.0% |
| e2e::readjust::drafts_merged_into_same_named_branches_of_two_repositories_are_absorbed_by_their_own | 1 | 235.0 | 97.1% | 1.7% | 1.0% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::readjust::a_repository_whose_read_failed_is_left_as_it_was_and_readjusted_by_the_next_trigger | 1 | 228.1 | 97.2% | 1.7% | 0.9% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::landing::the_counting_views_report_what_a_read_taken_now_says | 1 | 221.4 | 96.9% | 1.9% | 1.0% | 0.0% | 0.0% | 0.1% | 0.0% |
| e2e::readjust::a_draft_above_two_absorptions_is_placed_on_the_last_absorber | 1 | 219.8 | 98.1% | 0.6% | 1.0% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::readjust::a_restack_readjusts_before_it_starts | 1 | 214.9 | 88.2% | 0.6% | 1.0% | 0.0% | 10.1% | 0.1% | 0.0% |
| e2e::rework::a_restack_its_checks_refuse_is_asked_of_the_draft_s_worker_again | 1 | 210.6 | 98.0% | 0.7% | 1.1% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::rework::a_rework_whose_change_outgrew_its_title_is_retitled_by_the_drafter | 1 | 208.2 | 97.3% | 1.3% | 1.2% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::rework::a_draft_in_review_is_reworked_on_its_own_branch_and_change_request | 1 | 203.5 | 97.9% | 1.1% | 0.8% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on | 1 | 197.7 | 3.8% | 0.0% | 0.4% | 0.0% | 95.4% | 0.2% | 0.1% |
| e2e::rework::a_restack_whose_worker_fails_its_merge_settles_the_draft_failed | 1 | 191.6 | 97.5% | 0.9% | 1.4% | 0.0% | 0.0% | 0.2% | 0.0% |
| e2e::rework::the_op_s_title_outranks_the_drafter_s_and_a_keeps_leaves_the_title_alone | 1 | 177.6 | 98.0% | 0.5% | 1.3% | 0.0% | 0.0% | 0.2% | 0.1% |
| e2e::adoption::a_release_that_arrived_is_not_awaited_again_when_its_probe_stops_answering | 1 | 164.8 | 24.2% | 3.7% | 1.3% | 0.0% | 69.9% | 0.9% | 0.0% |
| e2e::readjust::a_draft_merged_into_an_unknown_branch_raises_a_blocking_question | 1 | 157.3 | 96.4% | 1.8% | 1.5% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::stacked_drafts::a_republished_stack_keeps_one_block_and_the_text_around_it | 1 | 152.2 | 0.8% | 1.2% | 1.0% | 0.0% | 11.1% | 0.1% | 85.9% |
| e2e::rework::a_rework_the_run_cannot_take_is_refused_naming_why | 1 | 152.0 | 96.8% | 1.1% | 1.8% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::unwatched::a_host_sized_runs_root_is_answered_in_well_under_a_second_whatever_its_journals_hold | 1 | 140.0 | 25.3% | 0.0% | 0.7% | 0.0% | 0.1% | 1.7% | 72.2% |
| e2e::continuation::a_dispatchs_branch_wins_over_a_settlements_and_an_authored_branch_over_both | 1 | 136.8 | 96.5% | 2.2% | 0.9% | 0.0% | 0.0% | 0.2% | 0.1% |
| e2e::review::driver_closeout_hands_review_to_the_owner_and_refuses_a_foreign_session | 1 | 135.5 | 72.0% | 1.4% | 1.0% | 0.0% | 24.9% | 0.6% | 0.2% |
| e2e::adoption::a_squash_merge_commit_no_release_can_be_attributed_through_is_answered_at_the_settle | 1 | 133.6 | 18.1% | 3.6% | 1.6% | 0.0% | 76.1% | 0.5% | 0.1% |
| e2e::adoption::a_cross_dag_dependency_is_pinned_against_git_and_named_from_the_upstreams_ledger | 1 | 132.2 | 2.5% | 2.9% | 8.8% | 0.0% | 84.7% | 0.9% | 0.0% |
| e2e::lifecycle::a_publication_over_an_unchanged_tree_is_settled_without_spending_the_budget | 1 | 131.6 | 96.6% | 1.7% | 1.5% | 0.0% | 0.0% | 0.2% | 0.0% |
| e2e::readjust::a_change_absorbed_during_its_own_rework_is_refused_before_the_push | 1 | 127.4 | 95.2% | 1.3% | 2.1% | 0.0% | 1.2% | 0.2% | 0.0% |
| e2e::readjust::a_rework_of_a_node_merged_since_the_last_trigger_is_refused_naming_add | 1 | 125.4 | 97.0% | 1.0% | 1.7% | 0.0% | 0.0% | 0.1% | 0.1% |

#### The 10 costliest modules

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::readjust | 22 | 3360.2 | 95.6% | 1.2% | 1.4% | 0.0% | 1.5% | 0.1% | 0.1% |
| e2e::adoption | 25 | 2188.4 | 11.4% | 6.2% | 4.4% | 0.0% | 76.8% | 1.2% | 0.1% |
| e2e::lifecycle | 50 | 2143.6 | 79.8% | 3.3% | 4.5% | 0.0% | 11.9% | 0.4% | 0.1% |
| e2e::rework | 10 | 1661.1 | 96.0% | 1.0% | 1.4% | 0.0% | 1.3% | 0.1% | 0.1% |
| e2e::continuation | 22 | 1203.8 | 51.7% | 2.0% | 3.0% | 0.0% | 33.1% | 0.3% | 9.8% |
| e2e::review | 10 | 950.6 | 89.2% | 2.8% | 1.9% | 0.0% | 5.6% | 0.3% | 0.2% |
| e2e::out_of_band | 11 | 748.5 | 89.0% | 7.8% | 2.7% | 0.0% | 0.0% | 0.4% | 0.0% |
| e2e::publication_boundary | 15 | 537.9 | 71.2% | 6.7% | 5.1% | 0.0% | 16.2% | 0.7% | 0.1% |
| e2e::stacked_drafts | 6 | 535.1 | 50.7% | 1.5% | 2.6% | 0.0% | 20.1% | 0.6% | 24.5% |
| e2e::landing | 3 | 500.7 | 96.7% | 2.1% | 1.0% | 0.0% | 0.0% | 0.1% | 0.0% |

### Against ci-linux

Each figure is the platform's seconds in that category over ci-linux's, summed over the tests both ran.

| platform | tests both ran | wall | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| macos | 1633 | 1.31x | 1.36x | 3.63x | 0.97x | 1.61x | 1.24x | 1.65x | 1.03x |
| windows-shard-1 | 136 | 3.41x | 2.83x | 20.47x | 3.40x | 1.14x | 3.62x | 8.98x | 3.51x |
| windows-shard-2 | 705 | 4.40x | 4.90x | 20.64x | 3.49x | 1.60x | 3.27x | 9.96x | 2.62x |

### Every category's ratio against local Linux (4b94e43), for this run's sets

| platform | tests both ran | wall | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ci-linux | 1662 | 0.57x | 0.55x | 0.29x | 0.44x | 0.34x | 0.75x | 0.32x | 0.40x |
| macos | 1635 | 0.75x | 0.77x | 1.04x | 0.43x | 0.54x | 0.92x | 0.53x | 0.42x |
| windows-shard-1 | 136 | 1.93x | 1.60x | 4.41x | 0.66x | 0.38x | 2.88x | 2.21x | 1.15x |
| windows-shard-2 | 707 | 2.55x | 2.82x | 5.60x | 1.70x | 0.53x | 2.25x | 3.30x | 1.16x |

## Hosted platforms on dd2e5d6 (run 39098714538)

`node scripts/test-profile-report.mjs --top-tests 30 --top-modules 10 --baseline ci-linux ...`, over the run's `test-profile-gate`, `test-profile-macos-latest` and `test-profile-windows-latest` artifacts.

### ci-linux

Run `4868bf48-7d3a-42a9-8994-258e4d6298ea`: 1658 tests, 10407.2 test-s (32 records of other runs set aside). Counts: spawn 14042, git 9048, build 3257, fixture 7760, wait 83543, fs 371301, polls 18424.

#### Whole run

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 1658 | 10407.2 | 61.0% | 0.7% | 8.6% | 0.0% | 26.9% | 0.2% | 2.5% |

#### The 30 costliest tests

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::host_view::the_budget_measurement_records_what_each_scaling_read_opened | 1 | 153.9 | 99.9% | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% |
| e2e::loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on | 1 | 137.8 | 3.4% | 0.0% | 0.3% | 0.0% | 96.2% | 0.1% | 0.1% |
| e2e::adoption::a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs | 1 | 125.0 | 0.9% | 0.1% | 0.3% | 0.0% | 98.5% | 0.1% | 0.1% |
| e2e::linear_writeback::a_linear_destination_lands_current_values_after_each_way_a_call_fails | 1 | 100.2 | 3.8% | 0.0% | 0.8% | 0.0% | 95.2% | 0.0% | 0.2% |
| e2e::review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it | 1 | 100.1 | 97.8% | 1.3% | 0.6% | 0.0% | 0.0% | 0.1% | 0.2% |
| e2e::writeback_budget::a_seven_item_copy_held_at_eleven_seconds_per_item_lands_inside_its_deadline | 1 | 79.0 | 0.7% | 0.0% | 0.5% | 0.0% | 98.8% | 0.0% | 0.0% |
| e2e::linear_writeback::command_end_refusal_and_timeout_rebuild_the_workers_store | 1 | 77.3 | 2.9% | 0.0% | 0.5% | 0.0% | 96.4% | 0.0% | 0.1% |
| e2e::writeback_budget::a_record_an_older_build_wrote_is_adopted_and_its_update_runs_under_the_shipped_default | 1 | 73.3 | 28.2% | 0.0% | 0.9% | 0.0% | 70.8% | 0.1% | 0.1% |
| e2e::delivers::a_first_projection_held_past_its_deadline_does_not_hold_back_the_first_dispatch | 1 | 73.1 | 0.6% | 0.0% | 0.8% | 0.0% | 98.5% | 0.1% | 0.1% |
| e2e::listing::a_host_sized_runs_root_lists_in_seconds_and_ten_times_the_journal_bytes_barely_moves_it | 1 | 71.2 | 58.9% | 0.1% | 0.5% | 0.0% | 0.0% | 2.5% | 37.9% |
| e2e::continuation::a_publication_held_past_the_whole_takeover_wait_holds_the_takeover_until_it_has_finished | 1 | 70.3 | 2.4% | 0.2% | 0.8% | 0.0% | 96.5% | 0.0% | 0.1% |
| e2e::linear_writeback::a_retained_stores_blocked_configuration_read_is_bounded_and_rebuilt | 1 | 68.1 | 1.4% | 0.0% | 0.6% | 0.0% | 97.6% | 0.0% | 0.3% |
| e2e::writeback_budget::a_creating_copy_held_past_a_tiny_budget_is_cancelled_and_the_next_attempt_creates_it | 1 | 67.7 | 0.7% | 0.0% | 0.8% | 0.0% | 98.4% | 0.1% | 0.0% |
| e2e::writeback_budget::a_targeted_update_held_past_the_floor_still_lands_inside_its_item_budget | 1 | 67.0 | 0.7% | 0.0% | 0.6% | 0.0% | 98.7% | 0.0% | 0.0% |
| e2e::continuation::a_takeover_of_an_idle_driver_starts_on_its_acknowledgement_within_the_interval | 1 | 66.0 | 5.5% | 0.2% | 0.6% | 0.0% | 2.8% | 0.0% | 90.9% |
| e2e::dispatch::a_legacy_plan_path_launch_record_is_still_reportable_and_adoptable | 1 | 65.1 | 99.4% | 0.0% | 0.6% | 0.0% | 0.0% | 0.0% | 0.0% |
| e2e::writeback_budget::an_update_held_past_a_tiny_budget_is_cancelled_and_the_refusal_names_the_arithmetic | 1 | 65.0 | 31.8% | 0.0% | 0.6% | 0.0% | 67.5% | 0.1% | 0.1% |
| e2e::loopcost::another_runs_ledger_is_read_on_its_own_interval_and_not_on_the_loops | 1 | 64.5 | 4.7% | 0.0% | 0.6% | 0.0% | 94.7% | 0.0% | 0.0% |
| e2e::loopcost::a_converged_run_does_no_scheduling_work_while_it_records_nothing | 1 | 62.8 | 0.5% | 0.0% | 0.6% | 0.0% | 98.9% | 0.0% | 0.0% |
| e2e::host_view::the_host_view_answers_a_host_sized_root_steady_cold_and_steady_again | 1 | 62.1 | 99.1% | 0.0% | 0.6% | 0.0% | 0.0% | 0.0% | 0.3% |
| e2e::writeback_budget::a_destination_read_held_past_the_floor_is_cancelled_retried_and_recovers | 1 | 61.7 | 0.8% | 0.0% | 0.9% | 0.0% | 98.3% | 0.1% | 0.0% |
| e2e::driver::a_legacy_unqualified_project_does_not_prevent_adoption | 1 | 61.2 | 99.1% | 0.0% | 0.8% | 0.0% | 0.0% | 0.0% | 0.0% |
| e2e::store::a_run_whose_store_is_rate_limited_throughout_settles_as_it_does_against_a_healthy_one | 1 | 61.0 | 95.4% | 0.0% | 0.6% | 0.0% | 4.0% | 0.0% | 0.0% |
| e2e::writeback_budget::a_store_that_does_not_open_within_the_floor_is_refused_retried_and_recovers | 1 | 61.0 | 0.3% | 0.0% | 0.6% | 0.0% | 98.8% | 0.1% | 0.2% |
| e2e::unwatched::a_host_sized_runs_root_is_answered_in_well_under_a_second_whatever_its_journals_hold | 1 | 58.3 | 49.8% | 0.0% | 0.7% | 0.0% | 0.1% | 3.2% | 46.3% |
| e2e::continuation::a_settlement_dispatch_or_claim_that_is_not_one_refuses_the_launch | 1 | 56.1 | 16.3% | 0.1% | 0.7% | 0.0% | 82.2% | 0.0% | 0.6% |
| e2e::out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique | 1 | 54.6 | 97.8% | 0.7% | 1.1% | 0.0% | 0.0% | 0.2% | 0.1% |
| e2e::continuation::an_in_flight_dispatch_left_on_the_store_is_continued_by_a_host_that_takes_the_project_over | 1 | 53.6 | 97.3% | 0.2% | 0.7% | 0.0% | 1.6% | 0.1% | 0.1% |
| e2e::adoption::a_cross_dag_dependency_is_pinned_against_git_and_named_from_the_upstreams_ledger | 1 | 52.1 | 1.8% | 0.3% | 2.0% | 0.0% | 95.4% | 0.5% | 0.0% |
| e2e::dispatch_env_hook::a_hook_that_fails_times_out_cannot_start_or_prints_a_malformed_document_refuses_the_launch | 1 | 51.1 | 98.7% | 0.0% | 0.8% | 0.0% | 0.0% | 0.2% | 0.4% |

#### The 10 costliest modules

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::continuation | 43 | 1092.9 | 35.4% | 0.3% | 1.7% | 0.0% | 52.0% | 0.1% | 10.5% |
| e2e::lifecycle | 102 | 745.0 | 85.2% | 0.9% | 7.0% | 0.0% | 6.5% | 0.2% | 0.1% |
| e2e::readjust | 44 | 627.9 | 92.3% | 0.6% | 4.2% | 0.0% | 2.3% | 0.2% | 0.5% |
| e2e::adoption | 51 | 625.9 | 20.8% | 1.6% | 5.1% | 0.0% | 71.8% | 0.4% | 0.3% |
| e2e::writeback_budget | 12 | 496.0 | 12.3% | 0.0% | 1.3% | 0.0% | 86.2% | 0.0% | 0.1% |
| e2e::store | 46 | 361.4 | 37.8% | 0.0% | 5.1% | 0.0% | 50.0% | 0.1% | 6.9% |
| e2e::rework | 20 | 359.5 | 91.9% | 0.5% | 3.0% | 0.0% | 4.0% | 0.2% | 0.4% |
| e2e::linear_writeback | 10 | 318.0 | 4.3% | 0.0% | 1.4% | 0.0% | 94.1% | 0.0% | 0.2% |
| e2e::review | 21 | 303.6 | 91.2% | 1.1% | 3.5% | 0.0% | 3.7% | 0.2% | 0.3% |
| e2e::loopcost | 9 | 277.6 | 4.7% | 0.0% | 1.8% | 0.0% | 93.3% | 0.0% | 0.1% |

### macos

Run `a58ae3f8-8006-4947-8184-5bd8881229e4`: 1633 tests, 13334.1 test-s (30 records of other runs set aside). Counts: spawn 13340, git 9029, build 3206, fixture 7694, wait 25376, fs 318406, polls 18358.

#### Whole run

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 1633 | 13334.1 | 62.8% | 2.2% | 6.2% | 0.0% | 26.5% | 0.3% | 1.9% |

#### The 30 costliest tests

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::adoption::a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs | 1 | 138.2 | 2.4% | 1.0% | 0.9% | 0.1% | 95.2% | 0.2% | 0.1% |
| e2e::loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on | 1 | 131.5 | 3.2% | 0.0% | 0.6% | 0.0% | 95.9% | 0.1% | 0.1% |
| e2e::dispatch_env_hook::a_hook_that_fails_times_out_cannot_start_or_prints_a_malformed_document_refuses_the_launch | 1 | 129.5 | 99.4% | 0.0% | 0.3% | 0.0% | 0.0% | 0.2% | 0.2% |
| e2e::review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it | 1 | 115.2 | 97.2% | 2.2% | 0.4% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique | 1 | 96.4 | 98.0% | 1.1% | 0.5% | 0.0% | 0.0% | 0.2% | 0.1% |
| e2e::store::a_cache_enabled_github_store_reads_and_writes_like_an_uncached_store | 1 | 95.7 | 2.7% | 0.0% | 0.4% | 0.0% | 74.2% | 0.0% | 22.8% |
| e2e::linear_writeback::a_linear_destination_lands_current_values_after_each_way_a_call_fails | 1 | 93.1 | 5.5% | 0.0% | 0.8% | 0.0% | 93.4% | 0.0% | 0.2% |
| e2e::readjust::the_store_is_written_only_for_what_its_projection_holds_changed | 1 | 86.9 | 98.6% | 0.6% | 0.5% | 0.0% | 0.1% | 0.1% | 0.1% |
| e2e::writeback_budget::a_seven_item_copy_held_at_eleven_seconds_per_item_lands_inside_its_deadline | 1 | 80.0 | 0.9% | 0.0% | 1.0% | 0.0% | 98.1% | 0.0% | 0.0% |
| e2e::continuation::a_publication_held_past_the_whole_takeover_wait_holds_the_takeover_until_it_has_finished | 1 | 74.1 | 2.9% | 0.3% | 0.4% | 0.0% | 96.3% | 0.0% | 0.0% |
| e2e::linear_writeback::command_end_refusal_and_timeout_rebuild_the_workers_store | 1 | 74.1 | 2.9% | 0.0% | 0.7% | 0.0% | 96.2% | 0.0% | 0.2% |
| e2e::writeback_budget::a_record_an_older_build_wrote_is_adopted_and_its_update_runs_under_the_shipped_default | 1 | 74.0 | 28.4% | 0.0% | 0.8% | 0.0% | 70.6% | 0.1% | 0.1% |
| e2e::delivers::a_first_projection_held_past_its_deadline_does_not_hold_back_the_first_dispatch | 1 | 72.8 | 0.6% | 0.0% | 0.4% | 0.0% | 98.8% | 0.1% | 0.0% |
| e2e::writeback_budget::a_creating_copy_held_past_a_tiny_budget_is_cancelled_and_the_next_attempt_creates_it | 1 | 68.0 | 1.2% | 0.0% | 0.6% | 0.0% | 98.0% | 0.1% | 0.1% |
| e2e::writeback_budget::a_targeted_update_held_past_the_floor_still_lands_inside_its_item_budget | 1 | 67.5 | 0.6% | 0.0% | 0.8% | 0.0% | 98.5% | 0.0% | 0.0% |
| e2e::continuation::a_takeover_of_an_idle_driver_starts_on_its_acknowledgement_within_the_interval | 1 | 66.7 | 5.8% | 0.4% | 0.6% | 0.0% | 3.7% | 0.0% | 89.6% |
| e2e::writeback_budget::an_update_held_past_a_tiny_budget_is_cancelled_and_the_refusal_names_the_arithmetic | 1 | 65.6 | 31.8% | 0.0% | 0.9% | 0.0% | 67.1% | 0.1% | 0.1% |
| e2e::loopcost::another_runs_ledger_is_read_on_its_own_interval_and_not_on_the_loops | 1 | 65.4 | 5.0% | 0.0% | 1.2% | 0.0% | 93.7% | 0.0% | 0.0% |
| e2e::dispatch::a_legacy_plan_path_launch_record_is_still_reportable_and_adoptable | 1 | 65.4 | 99.5% | 0.0% | 0.4% | 0.0% | 0.0% | 0.0% | 0.0% |
| e2e::linear_writeback::a_retained_stores_blocked_configuration_read_is_bounded_and_rebuilt | 1 | 65.2 | 2.0% | 0.0% | 0.6% | 0.0% | 97.1% | 0.0% | 0.3% |
| e2e::loopcost::a_converged_run_does_no_scheduling_work_while_it_records_nothing | 1 | 63.7 | 0.6% | 0.0% | 1.1% | 0.0% | 98.3% | 0.0% | 0.0% |
| e2e::writeback_budget::a_destination_read_held_past_the_floor_is_cancelled_retried_and_recovers | 1 | 62.4 | 1.9% | 0.0% | 0.6% | 0.0% | 97.3% | 0.1% | 0.0% |
| e2e::adoption::a_cross_dag_dependency_is_pinned_against_git_and_named_from_the_upstreams_ledger | 1 | 62.3 | 1.9% | 1.6% | 2.2% | 0.1% | 93.9% | 0.2% | 0.0% |
| e2e::driver::a_legacy_unqualified_project_does_not_prevent_adoption | 1 | 61.6 | 98.9% | 0.0% | 1.1% | 0.0% | 0.0% | 0.0% | 0.0% |
| e2e::writeback_budget::a_store_that_does_not_open_within_the_floor_is_refused_retried_and_recovers | 1 | 61.6 | 0.5% | 0.0% | 0.9% | 0.0% | 98.2% | 0.2% | 0.2% |
| e2e::listing::a_host_sized_runs_root_lists_in_seconds_and_ten_times_the_journal_bytes_barely_moves_it | 1 | 61.2 | 61.2% | 0.6% | 2.8% | 0.0% | 0.0% | 0.5% | 34.8% |
| e2e::store::a_run_whose_store_is_rate_limited_throughout_settles_as_it_does_against_a_healthy_one | 1 | 60.9 | 95.3% | 0.0% | 0.5% | 0.0% | 4.2% | 0.0% | 0.0% |
| e2e::continuation::an_in_flight_dispatch_left_on_the_store_is_continued_by_a_host_that_takes_the_project_over | 1 | 59.9 | 96.9% | 0.4% | 0.5% | 0.0% | 2.1% | 0.1% | 0.1% |
| e2e::continuation::a_settlement_dispatch_or_claim_that_is_not_one_refuses_the_launch | 1 | 58.3 | 19.4% | 0.2% | 0.5% | 0.0% | 79.5% | 0.0% | 0.4% |
| e2e::lifecycle::a_change_the_host_never_lands_is_redispatched_on_the_branch_it_preserved | 1 | 56.7 | 98.3% | 0.5% | 0.9% | 0.0% | 0.0% | 0.1% | 0.3% |

#### The 10 costliest modules

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::readjust | 44 | 1332.5 | 93.3% | 1.2% | 3.0% | 0.0% | 2.1% | 0.2% | 0.2% |
| e2e::continuation | 43 | 1196.8 | 38.4% | 0.6% | 1.3% | 0.0% | 49.6% | 0.1% | 10.0% |
| e2e::lifecycle | 102 | 1186.8 | 81.5% | 2.6% | 5.6% | 0.0% | 9.8% | 0.3% | 0.1% |
| e2e::adoption | 51 | 974.2 | 16.6% | 4.3% | 3.5% | 0.1% | 74.9% | 0.4% | 0.2% |
| e2e::rework | 20 | 526.3 | 92.8% | 1.2% | 2.4% | 0.0% | 3.3% | 0.2% | 0.2% |
| e2e::writeback_budget | 12 | 500.9 | 12.8% | 0.0% | 1.3% | 0.0% | 85.7% | 0.1% | 0.1% |
| e2e::store | 46 | 439.2 | 32.4% | 0.1% | 3.3% | 0.0% | 58.7% | 0.1% | 5.3% |
| e2e::review | 21 | 424.7 | 90.4% | 2.0% | 2.8% | 0.0% | 4.4% | 0.2% | 0.3% |
| e2e::linear_writeback | 11 | 412.4 | 4.3% | 0.0% | 1.3% | 0.0% | 94.1% | 0.0% | 0.2% |
| e2e::retirement | 38 | 401.9 | 63.2% | 9.8% | 5.4% | 0.0% | 20.9% | 0.4% | 0.2% |

### windows

Run `de89d6dd-b5e5-41a8-ac12-dacf0d991c60`: 1112 tests, 37565.0 test-s (29 records of other runs set aside), 4 of them unfinished (ended by the runner; their figures are their last heartbeat's). Counts: spawn 7564, git 6673, build 2171, fixture 5601, wait 196296, fs 337163, polls 14733.

#### Whole run

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 1112 | 37565.0 | 69.9% | 3.3% | 5.3% | 0.0% | 19.4% | 0.5% | 1.6% |

#### The 30 costliest tests

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::readjust::the_store_is_written_only_for_what_its_projection_holds_changed | 1 | 440.5 | 98.5% | 0.7% | 0.6% | 0.0% | 0.0% | 0.1% | 0.0% |
| e2e::out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique | 1 | 371.3 | 97.3% | 1.8% | 0.5% | 0.0% | 0.0% | 0.3% | 0.0% |
| e2e::readjust::a_whole_lower_stack_landing_moves_every_draft_above_it_in_one_readjustment | 1 | 364.9 | 98.7% | 0.4% | 0.6% | 0.0% | 0.0% | 0.1% | 0.2% |
| e2e::readjust::deferred_republications_are_read_by_the_next_trigger_green_or_repaired_red | 1 | 329.3 | 98.8% | 0.4% | 0.6% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::readjust::one_of_several_parents_landing_advances_the_stack_base_then_moves_the_draft | 1 | 313.8 | 98.0% | 1.0% | 0.8% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::rework::a_rework_restacks_every_draft_stacked_on_it_by_merges_alone | 1 | 313.3 | 97.9% | 1.0% | 0.8% | 0.0% | 0.0% | 0.1% | 0.2% |
| e2e::landing::a_render_asks_the_landing_read_once_per_node_it_prints_and_does_nothing_else_per_node | 1 | 303.6 | 97.5% | 1.5% | 0.8% | 0.0% | 0.0% | 0.1% | 0.0% |
| e2e::readjust::a_landing_during_a_rework_readjusts_the_rework_once_it_settles | 1 | 299.0 | 90.3% | 0.6% | 0.7% | 0.0% | 8.2% | 0.2% | 0.1% |
| e2e::rework::a_title_nothing_verified_is_left_unverified_until_a_retitle_sets_one | 1 | 285.4 | 98.1% | 0.7% | 0.7% | 0.0% | 0.0% | 0.3% | 0.3% |
| e2e::readjust::every_deferred_verdict_word_is_recorded_and_only_pending_stays_outstanding | 1 | 283.4 | 98.4% | 0.6% | 0.8% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::rework::a_rework_conflicting_with_the_draft_above_is_concluded_by_that_draft_s_worker | 1 | 274.7 | 98.1% | 0.5% | 1.2% | 0.0% | 0.0% | 0.2% | 0.1% |
| e2e::landing::the_counting_views_report_what_a_read_taken_now_says | 1 | 268.0 | 97.1% | 2.0% | 0.7% | 0.0% | 0.0% | 0.1% | 0.0% |
| e2e::rework::a_rework_whose_change_outgrew_its_title_is_retitled_by_the_drafter | 1 | 263.9 | 97.8% | 1.0% | 1.0% | 0.0% | 0.0% | 0.1% | 0.0% |
| e2e::landing::every_answer_a_landing_read_gives_reads_back_on_the_node_line | 1 | 259.6 | 97.3% | 1.7% | 0.9% | 0.0% | 0.0% | 0.1% | 0.0% |
| e2e::readjust::a_draft_above_two_absorptions_is_placed_on_the_last_absorber | 1 | 258.1 | 98.3% | 0.6% | 0.9% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it | 1 | 252.4 | 93.1% | 5.7% | 0.8% | 0.0% | 0.0% | 0.3% | 0.2% |
| e2e::readjust::drafts_closed_under_same_named_branches_of_two_repositories_are_each_recovered | 1 | 241.3 | 96.8% | 2.0% | 1.0% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::readjust::a_draft_merged_into_its_parent_s_branch_is_absorbed_and_lands_with_it | 1 | 231.6 | 97.8% | 1.0% | 1.1% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::readjust::drafts_merged_into_same_named_branches_of_two_repositories_are_absorbed_by_their_own | 1 | 224.9 | 96.5% | 2.1% | 1.2% | 0.0% | 0.0% | 0.2% | 0.1% |
| e2e::readjust::a_cold_host_reads_a_moved_heads_pending_verdict_again_until_it_is_green | 1 | 222.9 | 96.8% | 1.7% | 1.2% | 0.0% | 0.0% | 0.1% | 0.0% |
| e2e::session_reuse::every_other_shape_cuts_a_fresh_session_onto_its_branch_or_from_the_base | 1 | 215.5 | 89.4% | 0.9% | 0.9% | 0.0% | 0.0% | 0.0% | 8.8% |
| e2e::readjust::a_restack_readjusts_before_it_starts | 1 | 210.3 | 88.1% | 0.9% | 1.2% | 0.0% | 9.7% | 0.1% | 0.0% |
| e2e::readjust::a_rework_after_a_landing_places_its_replacement_on_the_root | 1 | 209.6 | 97.9% | 0.7% | 1.1% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::rework::a_restack_whose_worker_fails_its_merge_settles_the_draft_failed | 1 | 209.5 | 98.1% | 0.7% | 1.0% | 0.0% | 0.0% | 0.2% | 0.0% |
| e2e::readjust::a_repository_whose_read_failed_is_left_as_it_was_and_readjusted_by_the_next_trigger | 1 | 208.4 | 97.9% | 1.3% | 0.7% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::rework::a_restack_its_checks_refuse_is_asked_of_the_draft_s_worker_again | 1 | 204.6 | 98.0% | 0.7% | 1.1% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::rework::a_draft_in_review_is_reworked_on_its_own_branch_and_change_request | 1 | 198.8 | 96.9% | 1.8% | 1.0% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::readjust::a_change_merged_during_its_own_rework_is_refused_before_the_push | 1 | 197.9 | 96.3% | 1.1% | 1.3% | 0.0% | 1.2% | 0.1% | 0.0% |
| e2e::loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on | 1 | 196.0 | 4.0% | 0.0% | 0.5% | 0.0% | 95.3% | 0.2% | 0.0% |
| e2e::readjust::a_failed_read_leaves_a_draft_an_earlier_trigger_left_owed_as_it_was | 1 | 192.8 | 97.6% | 1.2% | 1.1% | 0.0% | 0.0% | 0.1% | 0.0% |

#### The 10 costliest modules

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::readjust | 44 | 7694.6 | 95.7% | 1.2% | 1.4% | 0.0% | 1.5% | 0.2% | 0.1% |
| e2e::adoption | 51 | 4724.4 | 14.0% | 5.5% | 2.5% | 0.0% | 76.8% | 1.0% | 0.2% |
| e2e::lifecycle | 100 | 3990.8 | 76.1% | 3.4% | 4.7% | 0.0% | 15.3% | 0.4% | 0.1% |
| e2e::rework | 19 | 3227.3 | 92.9% | 1.7% | 1.5% | 0.0% | 3.5% | 0.2% | 0.1% |
| e2e::continuation | 43 | 2529.7 | 53.0% | 1.8% | 2.8% | 0.0% | 33.7% | 0.3% | 8.3% |
| e2e::review | 21 | 1996.0 | 88.1% | 3.2% | 2.2% | 0.0% | 6.0% | 0.3% | 0.1% |
| e2e::publication_boundary | 30 | 1533.3 | 71.9% | 7.5% | 4.2% | 0.0% | 15.5% | 0.7% | 0.1% |
| e2e::out_of_band | 23 | 1252.8 | 89.6% | 6.7% | 3.2% | 0.0% | 0.0% | 0.4% | 0.1% |
| e2e::retirement | 23 | 983.7 | 69.3% | 11.0% | 4.6% | 0.0% | 14.1% | 0.7% | 0.2% |
| e2e::publish_preserve | 28 | 946.3 | 66.9% | 6.1% | 6.5% | 0.0% | 19.5% | 0.9% | 0.1% |

### Every category's ratio against CI Linux (dd2e5d6)

Each figure is the platform's seconds in that category over ci-linux's, summed over the tests both ran.

| platform | tests both ran | wall | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| macos | 1629 | 1.31x | 1.37x | 3.86x | 0.94x | 1.48x | 1.26x | 1.57x | 0.99x |
| windows | 1108 | 4.99x | 5.54x | 21.61x | 3.35x | 1.52x | 3.81x | 9.58x | 2.81x |

### Every category's ratio against local Linux (4b94e43)

Each figure is the platform's seconds in that category over local Linux's, summed over the tests both ran. `ci-linux`, `macos` and `windows` are dd2e5d6's sets, from run 39098714538's `test-profile-*` artifacts; `ci-linux-4b94e43` is run 39138389849's `test-profile-gate`. That run's macOS and per-shard Windows ratios against this local set are in **Hosted platforms on 4b94e43**. The local set ran at a load average of 12-59, so every ratio here is about a quarter lower than against the 7574f2f set (CI Linux 0.57x wall, against 0.75x then). For platform-to-platform comparison, the ratios against CI Linux above are the load-free ones.

| platform | tests both ran | wall | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ci-linux | 1658 | 0.57x | 0.55x | 0.29x | 0.45x | 0.35x | 0.75x | 0.34x | 0.41x |
| macos | 1631 | 0.76x | 0.78x | 1.13x | 0.43x | 0.52x | 0.94x | 0.53x | 0.40x |
| windows | 1110 | 2.85x | 3.18x | 5.97x | 1.40x | 0.51x | 2.69x | 3.14x | 1.11x |
| ci-linux-4b94e43 | 1662 | 0.57x | 0.55x | 0.29x | 0.44x | 0.34x | 0.75x | 0.32x | 0.40x |

## Earlier evidence (a94701b, 3c728a6, a17a96a)

These are the earlier record sets, kept as they were reported. CI Linux and local Linux are at a94701b, macOS at 3c728a6, and Windows at a17a96a (partial).

### Where the time goes

| platform | spawn | git | build | fixture | wait | fs | unattributed | `onepipeline` runs | s per run |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| linux (dev host) | 62.6% | 1.9% | 17.4% | 0.1% | 15.4% | 0.4% | 2.3% | | |
| linux-ci | 60.1% | 0.8% | 7.1% | 0.0% | 28.5% | 0.3% | 3.3% | 57.5% of e2e | 0.49 |
| macos | 63.3% | 1.8% | 5.4% | 0.0% | 27.2% | 0.2% | 2.0% | 62.0% | 0.71 |
| windows (partial) | 68.7% | 3.5% | 5.4% | 0.0% | 20.4% | 0.6% | 1.3% | 67.7% | 3.55 |

`spawn` is time a test is held by a child process, the `onepipeline` binary's own runs above all. `build` is the `cargo build --offline` of the test doubles, which every test process runs (about 2 a test). Windows `git` costs 0.18 s a run against CI Linux's 0.01, which points at per-process start cost on Windows.

Per-category ratio over CI Linux, summed over the tests both ran:

| platform | tests both ran | wall | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| linux (dev host) | 1658 | 2.83x | 2.95x | 7.12x | 6.89x | 8.17x | 1.52x | 4.49x | 1.97x |
| macos | 1627 | 1.31x | 1.40x | 3.10x | 0.99x | 1.57x | 1.22x | 1.17x | 0.81x |
| windows | 1097 | 5.47x | 6.07x | 23.34x | 4.59x | 1.96x | 4.16x | 11.21x | 1.98x |

The ratio against the dev-host Linux run (the script's `--baseline linux`) is in the per-platform section below. The dev host was heavily loaded, so the CI-Linux baseline is the fair comparison.

**Recording overhead.** Over the same tests, the profiled runs against same-day unprofiled CI runs: macOS **0.92x** (1611 tests; unprofiled = run of branch `onepipeline-ci-flakes`, job 117456309222). Windows **1.25x** (1083 tests; unprofiled = release-PR run 39012326838), with per-module ratios from 0.85x to 1.48x. A successful `gate` prints no per-test durations, so there is no Linux pair. Each recorded operation is one mutex lock and two `Instant::now()` calls, and the heartbeat writes one small file every 5 s. The Windows figure reads as runner variance (the spike measured p90 24% over median), but one pair of runs cannot rule out a Windows-specific cost.

### Per-platform breakdown (`node scripts/test-profile-report.mjs --top-tests 30 --top-modules 10 --baseline linux ...`)

Linux, this host, the finished tree:

### linux

Run `ed1ae191-2b99-44d9-9e12-53f6c01d79da`: 1660 tests, 26665.8 test-s. Counts: spawn 13586, git 9048, build 3261, fixture 7766, wait 132138, fs 388630, polls 18422.

#### Whole run

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 1660 | 26665.8 | 62.6% | 1.9% | 17.4% | 0.1% | 15.4% | 0.4% | 2.3% |

#### The 30 costliest tests

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::host_view::the_budget_measurement_records_what_each_scaling_read_opened | 1 | 287.8 | 99.9% | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% | 0.1% |
| e2e::loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on | 1 | 255.0 | 6.9% | 0.0% | 0.5% | 0.0% | 92.4% | 0.1% | 0.0% |
| e2e::out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique | 1 | 253.3 | 95.6% | 2.0% | 1.6% | 0.1% | 0.0% | 0.4% | 0.4% |
| e2e::review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it | 1 | 195.2 | 90.6% | 7.4% | 1.5% | 0.0% | 0.0% | 0.2% | 0.4% |
| e2e::listing::a_host_sized_runs_root_lists_in_seconds_and_ten_times_the_journal_bytes_barely_moves_it | 1 | 187.4 | 53.7% | 0.1% | 0.7% | 0.0% | 0.0% | 3.2% | 42.2% |
| e2e::readjust::the_store_is_written_only_for_what_its_projection_holds_changed | 1 | 145.7 | 94.6% | 1.2% | 3.4% | 0.0% | 0.4% | 0.1% | 0.3% |
| e2e::host_view::the_host_view_answers_a_host_sized_root_steady_cold_and_steady_again | 1 | 145.0 | 99.1% | 0.0% | 0.8% | 0.0% | 0.0% | 0.0% | 0.1% |
| e2e::unwatched::the_fixture_is_assembled_beside_a_template_still_replacing_its_documents | 1 | 144.2 | 95.7% | 0.0% | 2.4% | 0.0% | 0.5% | 0.2% | 1.3% |
| e2e::adoption::a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs | 1 | 140.7 | 2.9% | 0.3% | 5.4% | 0.0% | 91.2% | 0.2% | 0.1% |
| e2e::unwatched::a_host_sized_runs_root_is_answered_in_well_under_a_second_whatever_its_journals_hold | 1 | 130.0 | 46.4% | 0.0% | 0.6% | 0.0% | 0.1% | 3.3% | 49.6% |
| e2e::out_of_band::a_branch_with_no_single_owner_lands_under_the_onevcs_default | 1 | 110.7 | 93.6% | 2.3% | 3.3% | 0.1% | 0.0% | 0.4% | 0.2% |
| e2e::linear_writeback::a_linear_destination_lands_current_values_after_each_way_a_call_fails | 1 | 110.7 | 8.6% | 0.0% | 1.4% | 0.0% | 89.5% | 0.0% | 0.5% |
| e2e::manager_landings::three_hosted_landings_at_once_each_relay_their_whole_check_sequence_to_their_owner | 1 | 104.6 | 83.7% | 2.2% | 4.7% | 0.0% | 8.5% | 0.3% | 0.7% |
| e2e::readjust::one_of_several_parents_landing_advances_the_stack_base_then_moves_the_draft | 1 | 103.4 | 93.2% | 0.9% | 5.3% | 0.0% | 0.0% | 0.1% | 0.5% |
| e2e::readjust::deferred_republications_are_read_by_the_next_trigger_green_or_repaired_red | 1 | 103.2 | 96.0% | 0.5% | 2.6% | 0.0% | 0.0% | 0.2% | 0.7% |
| e2e::readjust::a_whole_lower_stack_landing_moves_every_draft_above_it_in_one_readjustment | 1 | 97.2 | 93.3% | 0.6% | 4.5% | 0.0% | 0.0% | 0.1% | 1.4% |
| e2e::writeback_budget::a_seven_item_copy_held_at_eleven_seconds_per_item_lands_inside_its_deadline | 1 | 94.9 | 5.5% | 0.0% | 4.8% | 0.0% | 89.2% | 0.1% | 0.3% |
| e2e::readjust::every_deferred_verdict_word_is_recorded_and_only_pending_stays_outstanding | 1 | 94.7 | 93.7% | 1.0% | 4.6% | 0.0% | 0.0% | 0.2% | 0.5% |
| e2e::publication_boundary::the_host_branch_prefix_leads_every_public_branch_and_is_itself_screened | 1 | 94.3 | 82.6% | 3.5% | 4.9% | 0.1% | 8.1% | 0.5% | 0.4% |
| e2e::out_of_band::an_unusable_bound_is_refused_before_a_landing_pushes_or_writes_to_the_host | 1 | 90.1 | 73.1% | 20.1% | 4.0% | 0.1% | 0.0% | 1.5% | 1.2% |
| e2e::linear_writeback::command_end_refusal_and_timeout_rebuild_the_workers_store | 1 | 87.8 | 9.0% | 0.0% | 1.3% | 0.0% | 89.3% | 0.0% | 0.4% |
| e2e::rework::a_title_nothing_verified_is_left_unverified_until_a_retitle_sets_one | 1 | 86.1 | 95.7% | 0.4% | 2.2% | 0.0% | 0.0% | 0.2% | 1.5% |
| e2e::templates::the_host_root_is_the_flag_then_the_variable_then_the_schema_12_key_and_adopt_keeps_it | 1 | 85.6 | 96.8% | 0.0% | 2.3% | 0.1% | 0.0% | 0.2% | 0.6% |
| e2e::readjust::a_draft_above_two_absorptions_is_placed_on_the_last_absorber | 1 | 85.6 | 92.8% | 0.6% | 5.9% | 0.0% | 0.0% | 0.2% | 0.5% |
| e2e::readjust::a_landing_during_a_rework_readjusts_the_rework_once_it_settles | 1 | 83.1 | 84.9% | 0.8% | 3.1% | 0.0% | 10.3% | 0.2% | 0.6% |
| e2e::writeback_budget::a_record_an_older_build_wrote_is_adopted_and_its_update_runs_under_the_shipped_default | 1 | 81.6 | 32.5% | 0.0% | 3.7% | 0.0% | 63.0% | 0.2% | 0.6% |
| e2e::writeback_budget::a_targeted_update_held_past_the_floor_still_lands_inside_its_item_budget | 1 | 80.1 | 4.4% | 0.0% | 4.6% | 0.0% | 90.3% | 0.1% | 0.7% |
| e2e::stacked_drafts::landing_dependents_of_unlanded_parents_are_drafted_on_them | 1 | 77.9 | 96.8% | 0.6% | 1.8% | 0.0% | 0.0% | 0.1% | 0.6% |
| e2e::readjust::drafts_merged_into_same_named_branches_of_two_repositories_are_absorbed_by_their_own | 1 | 77.8 | 80.1% | 2.4% | 16.7% | 0.0% | 0.0% | 0.3% | 0.5% |
| e2e::review::a_cold_host_restores_rework_provenance_from_the_store_without_review_state_keys | 1 | 77.3 | 89.3% | 4.9% | 5.0% | 0.0% | 0.0% | 0.2% | 0.5% |

#### The 10 costliest modules

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::readjust | 44 | 2486.1 | 88.7% | 1.4% | 6.9% | 0.0% | 2.1% | 0.2% | 0.5% |
| e2e::continuation | 43 | 1280.3 | 38.7% | 1.7% | 2.7% | 0.0% | 46.8% | 0.1% | 9.9% |
| e2e::lifecycle | 102 | 1211.4 | 75.7% | 1.7% | 15.1% | 0.0% | 6.8% | 0.3% | 0.4% |
| e2e::publication_boundary | 35 | 1074.6 | 70.1% | 4.9% | 13.3% | 0.1% | 10.4% | 0.6% | 0.7% |
| e2e::out_of_band | 27 | 1055.3 | 76.6% | 6.4% | 14.7% | 0.1% | 0.9% | 0.7% | 0.7% |
| e2e::rework | 20 | 1002.0 | 91.3% | 1.1% | 4.0% | 0.0% | 2.6% | 0.2% | 0.6% |
| e2e::adoption | 51 | 967.0 | 22.1% | 2.6% | 12.5% | 0.1% | 61.7% | 0.6% | 0.4% |
| e2e::retirement | 38 | 929.8 | 62.5% | 6.9% | 13.6% | 0.1% | 16.0% | 0.4% | 0.6% |
| e2e::review | 21 | 900.9 | 84.3% | 3.6% | 6.3% | 0.0% | 4.7% | 0.3% | 0.7% |
| e2e::manager_landings | 39 | 856.8 | 57.0% | 5.7% | 23.8% | 0.1% | 12.2% | 0.6% | 0.7% |

<details><summary>CI Linux, macOS, Windows: the same tables, and the ratios against dev-host Linux</summary>

### linux-ci

Run `07782520-64bc-42f0-a3fd-c61fd1abf293`: 1658 tests, 9407.0 test-s (32 records of other runs set aside). Counts: spawn 14375, git 9048, build 3257, fixture 7760, wait 80106, fs 380802, polls 18424.

#### Whole run

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 1658 | 9407.0 | 60.1% | 0.8% | 7.1% | 0.0% | 28.5% | 0.3% | 3.3% |

#### The 30 costliest tests

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::adoption::a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs | 1 | 123.9 | 0.7% | 0.1% | 0.2% | 0.0% | 98.9% | 0.1% | 0.0% |
| e2e::loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on | 1 | 121.9 | 2.8% | 0.0% | 0.3% | 0.0% | 96.8% | 0.1% | 0.0% |
| e2e::host_view::the_budget_measurement_records_what_each_scaling_read_opened | 1 | 111.5 | 99.9% | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% | 0.1% |
| e2e::linear_writeback::a_linear_destination_lands_current_values_after_each_way_a_call_fails | 1 | 103.5 | 3.2% | 0.0% | 0.3% | 0.0% | 96.4% | 0.0% | 0.1% |
| e2e::listing::a_host_sized_runs_root_lists_in_seconds_and_ten_times_the_journal_bytes_barely_moves_it | 1 | 101.9 | 32.1% | 8.4% | 0.3% | 0.0% | 0.0% | 1.5% | 57.8% |
| e2e::review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it | 1 | 98.7 | 97.8% | 1.3% | 0.4% | 0.0% | 0.0% | 0.2% | 0.2% |
| e2e::unwatched::a_host_sized_runs_root_is_answered_in_well_under_a_second_whatever_its_journals_hold | 1 | 83.1 | 24.7% | 0.0% | 0.4% | 0.0% | 0.1% | 1.8% | 73.1% |
| e2e::writeback_budget::a_seven_item_copy_held_at_eleven_seconds_per_item_lands_inside_its_deadline | 1 | 78.7 | 0.6% | 0.0% | 0.7% | 0.0% | 98.7% | 0.0% | 0.0% |
| e2e::linear_writeback::command_end_refusal_and_timeout_rebuild_the_workers_store | 1 | 78.3 | 2.3% | 0.0% | 0.4% | 0.0% | 97.2% | 0.0% | 0.1% |
| e2e::writeback_budget::a_record_an_older_build_wrote_is_adopted_and_its_update_runs_under_the_shipped_default | 1 | 73.0 | 28.2% | 0.0% | 0.7% | 0.0% | 71.1% | 0.0% | 0.0% |
| e2e::delivers::a_first_projection_held_past_its_deadline_does_not_hold_back_the_first_dispatch | 1 | 72.8 | 0.3% | 0.0% | 0.7% | 0.0% | 98.9% | 0.1% | 0.0% |
| e2e::continuation::a_publication_held_past_the_whole_takeover_wait_holds_the_takeover_until_it_has_finished | 1 | 70.3 | 2.0% | 0.1% | 0.5% | 0.0% | 97.2% | 0.0% | 0.1% |
| e2e::linear_writeback::a_retained_stores_blocked_configuration_read_is_bounded_and_rebuilt | 1 | 67.9 | 1.2% | 0.0% | 0.5% | 0.0% | 98.0% | 0.0% | 0.2% |
| e2e::writeback_budget::a_creating_copy_held_past_a_tiny_budget_is_cancelled_and_the_next_attempt_creates_it | 1 | 67.4 | 0.7% | 0.0% | 0.5% | 0.0% | 98.8% | 0.1% | 0.0% |
| e2e::writeback_budget::a_targeted_update_held_past_the_floor_still_lands_inside_its_item_budget | 1 | 66.6 | 0.5% | 0.0% | 0.5% | 0.0% | 99.0% | 0.0% | 0.0% |
| e2e::continuation::a_takeover_of_an_idle_driver_starts_on_its_acknowledgement_within_the_interval | 1 | 65.4 | 5.3% | 0.1% | 0.5% | 0.0% | 2.3% | 0.0% | 91.7% |
| e2e::dispatch::a_legacy_plan_path_launch_record_is_still_reportable_and_adoptable | 1 | 64.9 | 99.5% | 0.0% | 0.5% | 0.0% | 0.0% | 0.0% | 0.0% |
| e2e::writeback_budget::an_update_held_past_a_tiny_budget_is_cancelled_and_the_refusal_names_the_arithmetic | 1 | 64.8 | 31.6% | 0.0% | 0.5% | 0.0% | 67.7% | 0.1% | 0.1% |
| e2e::loopcost::another_runs_ledger_is_read_on_its_own_interval_and_not_on_the_loops | 1 | 64.7 | 5.0% | 0.0% | 0.5% | 0.0% | 94.5% | 0.0% | 0.0% |
| e2e::loopcost::a_converged_run_does_no_scheduling_work_while_it_records_nothing | 1 | 62.7 | 0.3% | 0.0% | 0.6% | 0.0% | 99.1% | 0.0% | 0.0% |
| e2e::writeback_budget::a_destination_read_held_past_the_floor_is_cancelled_retried_and_recovers | 1 | 61.4 | 0.7% | 0.0% | 0.5% | 0.0% | 98.7% | 0.1% | 0.0% |
| e2e::store::a_run_whose_store_is_rate_limited_throughout_settles_as_it_does_against_a_healthy_one | 1 | 60.9 | 95.3% | 0.0% | 0.7% | 0.0% | 4.0% | 0.0% | 0.0% |
| e2e::driver::a_legacy_unqualified_project_does_not_prevent_adoption | 1 | 60.9 | 99.3% | 0.0% | 0.7% | 0.0% | 0.0% | 0.0% | 0.0% |
| e2e::writeback_budget::a_store_that_does_not_open_within_the_floor_is_refused_retried_and_recovers | 1 | 60.9 | 0.3% | 0.0% | 0.5% | 0.0% | 99.0% | 0.1% | 0.1% |
| e2e::host_view::the_host_view_answers_a_host_sized_root_steady_cold_and_steady_again | 1 | 56.7 | 99.1% | 0.0% | 0.5% | 0.0% | 0.0% | 0.0% | 0.3% |
| e2e::continuation::a_settlement_dispatch_or_claim_that_is_not_one_refuses_the_launch | 1 | 54.6 | 14.4% | 0.1% | 0.6% | 0.0% | 84.4% | 0.1% | 0.5% |
| e2e::out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique | 1 | 53.0 | 98.1% | 0.9% | 0.7% | 0.0% | 0.0% | 0.2% | 0.1% |
| e2e::continuation::an_in_flight_dispatch_left_on_the_store_is_continued_by_a_host_that_takes_the_project_over | 1 | 52.6 | 97.0% | 0.2% | 0.6% | 0.0% | 2.1% | 0.1% | 0.1% |
| e2e::continuation::a_takeover_finding_a_fence_it_cannot_read_waits_until_it_is_cleared | 1 | 50.8 | 5.5% | 0.2% | 1.0% | 0.0% | 91.6% | 0.1% | 1.6% |
| e2e::continuation::a_takeover_finding_a_fence_naming_no_publication_waits_until_it_is_cleared | 1 | 50.7 | 5.4% | 0.2% | 0.6% | 0.0% | 91.8% | 0.1% | 1.9% |

#### The 10 costliest modules

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::continuation | 43 | 1072.7 | 35.1% | 0.2% | 1.4% | 0.0% | 52.6% | 0.1% | 10.5% |
| e2e::lifecycle | 102 | 687.4 | 87.1% | 0.7% | 5.8% | 0.0% | 6.0% | 0.2% | 0.1% |
| e2e::adoption | 51 | 546.2 | 19.7% | 1.4% | 4.2% | 0.0% | 74.0% | 0.5% | 0.2% |
| e2e::readjust | 44 | 535.8 | 92.6% | 0.6% | 3.7% | 0.0% | 2.4% | 0.2% | 0.4% |
| e2e::writeback_budget | 12 | 491.6 | 12.1% | 0.0% | 0.9% | 0.0% | 86.8% | 0.0% | 0.1% |
| e2e::store | 46 | 349.2 | 37.3% | 0.0% | 4.3% | 0.0% | 51.2% | 0.1% | 7.0% |
| e2e::linear_writeback | 10 | 317.3 | 3.7% | 0.0% | 1.0% | 0.0% | 95.2% | 0.0% | 0.1% |
| e2e::rework | 20 | 302.9 | 91.6% | 0.4% | 2.8% | 0.0% | 4.5% | 0.2% | 0.4% |
| e2e::review | 21 | 288.2 | 92.0% | 1.2% | 2.9% | 0.0% | 3.3% | 0.4% | 0.3% |
| e2e::loopcost | 9 | 260.0 | 4.3% | 0.0% | 1.6% | 0.0% | 93.9% | 0.1% | 0.1% |

### macos

Run `df9ae281-e219-4268-8a10-f72afecbc69a`: 1631 tests, 12084.4 test-s (30 records of other runs set aside). Counts: spawn 13440, git 9022, build 3206, fixture 7691, wait 23462, fs 322379, polls 18357.

#### Whole run

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 1631 | 12084.4 | 63.3% | 1.8% | 5.4% | 0.0% | 27.2% | 0.2% | 2.0% |

#### The 30 costliest tests

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::adoption::a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs | 1 | 127.8 | 0.9% | 0.3% | 0.3% | 0.0% | 98.4% | 0.1% | 0.0% |
| e2e::review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it | 1 | 123.2 | 96.4% | 2.8% | 0.5% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on | 1 | 109.9 | 2.4% | 0.0% | 0.3% | 0.0% | 97.1% | 0.1% | 0.1% |
| e2e::store::a_cache_enabled_github_store_reads_and_writes_like_an_uncached_store | 1 | 96.2 | 2.9% | 0.0% | 0.5% | 0.0% | 74.6% | 0.0% | 21.9% |
| e2e::dispatch_env_hook::a_hook_that_fails_times_out_cannot_start_or_prints_a_malformed_document_refuses_the_launch | 1 | 95.9 | 99.4% | 0.0% | 0.3% | 0.0% | 0.0% | 0.2% | 0.1% |
| e2e::linear_writeback::a_linear_destination_lands_current_values_after_each_way_a_call_fails | 1 | 91.7 | 4.4% | 0.0% | 0.3% | 0.0% | 94.9% | 0.0% | 0.2% |
| e2e::writeback_budget::a_seven_item_copy_held_at_eleven_seconds_per_item_lands_inside_its_deadline | 1 | 80.2 | 0.9% | 0.0% | 0.8% | 0.0% | 98.3% | 0.0% | 0.0% |
| e2e::readjust::the_store_is_written_only_for_what_its_projection_holds_changed | 1 | 76.5 | 97.0% | 0.8% | 1.9% | 0.0% | 0.1% | 0.1% | 0.1% |
| e2e::out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique | 1 | 74.6 | 97.5% | 1.4% | 0.8% | 0.1% | 0.0% | 0.2% | 0.1% |
| e2e::linear_writeback::command_end_refusal_and_timeout_rebuild_the_workers_store | 1 | 73.7 | 2.8% | 0.0% | 0.4% | 0.0% | 96.6% | 0.0% | 0.2% |
| e2e::writeback_budget::a_record_an_older_build_wrote_is_adopted_and_its_update_runs_under_the_shipped_default | 1 | 73.3 | 28.4% | 0.0% | 0.5% | 0.0% | 71.0% | 0.1% | 0.1% |
| e2e::delivers::a_first_projection_held_past_its_deadline_does_not_hold_back_the_first_dispatch | 1 | 72.8 | 0.4% | 0.0% | 0.5% | 0.0% | 99.0% | 0.1% | 0.0% |
| e2e::continuation::a_publication_held_past_the_whole_takeover_wait_holds_the_takeover_until_it_has_finished | 1 | 72.5 | 1.8% | 0.3% | 0.5% | 0.0% | 97.2% | 0.0% | 0.0% |
| e2e::writeback_budget::a_creating_copy_held_past_a_tiny_budget_is_cancelled_and_the_next_attempt_creates_it | 1 | 67.9 | 0.8% | 0.0% | 0.5% | 0.0% | 98.5% | 0.1% | 0.1% |
| e2e::writeback_budget::a_targeted_update_held_past_the_floor_still_lands_inside_its_item_budget | 1 | 67.7 | 0.6% | 0.0% | 0.7% | 0.0% | 98.6% | 0.0% | 0.0% |
| e2e::continuation::a_takeover_of_an_idle_driver_starts_on_its_acknowledgement_within_the_interval | 1 | 67.1 | 5.8% | 0.3% | 0.4% | 0.0% | 4.2% | 0.0% | 89.3% |
| e2e::writeback_budget::an_update_held_past_a_tiny_budget_is_cancelled_and_the_refusal_names_the_arithmetic | 1 | 65.7 | 32.1% | 0.0% | 0.9% | 0.0% | 66.7% | 0.1% | 0.1% |
| e2e::dispatch::a_legacy_plan_path_launch_record_is_still_reportable_and_adoptable | 1 | 65.1 | 99.6% | 0.0% | 0.4% | 0.0% | 0.0% | 0.0% | 0.0% |
| e2e::linear_writeback::a_retained_stores_blocked_configuration_read_is_bounded_and_rebuilt | 1 | 65.0 | 1.6% | 0.0% | 0.5% | 0.0% | 97.5% | 0.0% | 0.5% |
| e2e::loopcost::another_runs_ledger_is_read_on_its_own_interval_and_not_on_the_loops | 1 | 64.8 | 4.9% | 0.0% | 0.6% | 0.0% | 94.5% | 0.0% | 0.0% |
| e2e::loopcost::a_converged_run_does_no_scheduling_work_while_it_records_nothing | 1 | 62.9 | 0.5% | 0.0% | 0.6% | 0.0% | 98.8% | 0.0% | 0.1% |
| e2e::writeback_budget::a_destination_read_held_past_the_floor_is_cancelled_retried_and_recovers | 1 | 61.7 | 1.1% | 0.0% | 0.5% | 0.0% | 98.2% | 0.1% | 0.0% |
| e2e::writeback_budget::a_store_that_does_not_open_within_the_floor_is_refused_retried_and_recovers | 1 | 61.3 | 0.4% | 0.0% | 0.7% | 0.0% | 98.6% | 0.2% | 0.1% |
| e2e::store::a_run_whose_store_is_rate_limited_throughout_settles_as_it_does_against_a_healthy_one | 1 | 61.1 | 95.1% | 0.0% | 0.6% | 0.0% | 4.3% | 0.0% | 0.0% |
| e2e::driver::a_legacy_unqualified_project_does_not_prevent_adoption | 1 | 60.9 | 99.5% | 0.0% | 0.4% | 0.0% | 0.0% | 0.0% | 0.0% |
| e2e::continuation::an_in_flight_dispatch_left_on_the_store_is_continued_by_a_host_that_takes_the_project_over | 1 | 60.5 | 96.6% | 0.3% | 0.5% | 0.0% | 2.5% | 0.1% | 0.1% |
| e2e::rework::a_rework_conflicting_with_the_draft_above_is_concluded_by_that_draft_s_worker | 1 | 58.5 | 97.8% | 0.8% | 1.0% | 0.0% | 0.0% | 0.1% | 0.4% |
| e2e::rework::a_rework_restacks_every_draft_stacked_on_it_by_merges_alone | 1 | 56.6 | 94.8% | 2.3% | 2.3% | 0.0% | 0.0% | 0.1% | 0.5% |
| e2e::continuation::a_settlement_dispatch_or_claim_that_is_not_one_refuses_the_launch | 1 | 56.5 | 17.1% | 0.2% | 0.5% | 0.0% | 81.9% | 0.0% | 0.3% |
| e2e::adoption::a_cross_dag_dependency_is_pinned_against_git_and_named_from_the_upstreams_ledger | 1 | 55.6 | 1.5% | 0.7% | 0.9% | 0.0% | 96.7% | 0.1% | 0.0% |

#### The 10 costliest modules

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::continuation | 43 | 1157.5 | 37.3% | 0.5% | 1.1% | 0.0% | 50.6% | 0.1% | 10.4% |
| e2e::readjust | 44 | 1108.9 | 93.6% | 1.3% | 2.5% | 0.0% | 2.3% | 0.2% | 0.2% |
| e2e::lifecycle | 102 | 867.3 | 84.9% | 1.8% | 4.0% | 0.0% | 8.9% | 0.2% | 0.1% |
| e2e::adoption | 51 | 736.4 | 15.3% | 3.2% | 2.5% | 0.1% | 78.4% | 0.3% | 0.2% |
| e2e::rework | 20 | 680.0 | 92.7% | 1.1% | 2.5% | 0.0% | 3.3% | 0.2% | 0.2% |
| e2e::writeback_budget | 12 | 498.4 | 12.6% | 0.0% | 1.1% | 0.0% | 86.2% | 0.1% | 0.1% |
| e2e::review | 21 | 467.1 | 90.1% | 2.3% | 2.7% | 0.0% | 4.3% | 0.2% | 0.3% |
| e2e::store | 46 | 453.1 | 32.8% | 0.2% | 4.4% | 0.0% | 57.4% | 0.2% | 5.1% |
| e2e::linear_writeback | 11 | 405.4 | 4.1% | 0.0% | 0.7% | 0.0% | 95.0% | 0.0% | 0.2% |
| e2e::retirement | 38 | 401.0 | 62.7% | 10.0% | 5.2% | 0.0% | 21.4% | 0.4% | 0.2% |

### windows

Run `78432a10-7c68-483b-bdd6-596463c7ff2e`: 1101 tests, 37172.2 test-s (29 records of other runs set aside). Counts: spawn 7588, git 6604, build 2157, fixture 5533, wait 202322, fs 342987, polls 14700.

#### Whole run

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 1101 | 37172.2 | 68.7% | 3.5% | 5.4% | 0.0% | 20.4% | 0.6% | 1.3% |

#### The 30 costliest tests

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique | 1 | 479.0 | 97.1% | 2.2% | 0.4% | 0.0% | 0.0% | 0.3% | 0.0% |
| e2e::readjust::the_store_is_written_only_for_what_its_projection_holds_changed | 1 | 438.5 | 98.8% | 0.5% | 0.5% | 0.0% | 0.0% | 0.2% | 0.0% |
| e2e::readjust::a_whole_lower_stack_landing_moves_every_draft_above_it_in_one_readjustment | 1 | 358.4 | 98.5% | 0.6% | 0.6% | 0.0% | 0.0% | 0.1% | 0.2% |
| e2e::readjust::deferred_republications_are_read_by_the_next_trigger_green_or_repaired_red | 1 | 321.6 | 98.6% | 0.5% | 0.7% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::readjust::one_of_several_parents_landing_advances_the_stack_base_then_moves_the_draft | 1 | 310.1 | 98.1% | 1.1% | 0.7% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::readjust::every_deferred_verdict_word_is_recorded_and_only_pending_stays_outstanding | 1 | 284.7 | 98.6% | 0.5% | 0.7% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::rework::a_title_nothing_verified_is_left_unverified_until_a_retitle_sets_one | 1 | 279.8 | 98.0% | 0.7% | 0.8% | 0.0% | 0.0% | 0.2% | 0.2% |
| e2e::rework::a_rework_restacks_every_draft_stacked_on_it_by_merges_alone | 1 | 276.7 | 97.8% | 1.1% | 0.8% | 0.0% | 0.0% | 0.2% | 0.1% |
| e2e::landing::a_render_asks_the_landing_read_once_per_node_it_prints_and_does_nothing_else_per_node | 1 | 270.6 | 97.3% | 1.9% | 0.6% | 0.0% | 0.0% | 0.1% | 0.0% |
| e2e::rework::a_rework_conflicting_with_the_draft_above_is_concluded_by_that_draft_s_worker | 1 | 261.7 | 98.4% | 0.6% | 0.8% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it | 1 | 260.1 | 93.4% | 5.3% | 0.7% | 0.0% | 0.0% | 0.3% | 0.1% |
| e2e::rework::a_rework_whose_change_outgrew_its_title_is_retitled_by_the_drafter | 1 | 252.5 | 98.0% | 1.0% | 0.8% | 0.0% | 0.0% | 0.1% | 0.0% |
| e2e::readjust::drafts_closed_under_same_named_branches_of_two_repositories_are_each_recovered | 1 | 244.9 | 97.8% | 1.1% | 0.9% | 0.0% | 0.0% | 0.1% | 0.0% |
| e2e::landing::the_counting_views_report_what_a_read_taken_now_says | 1 | 227.9 | 97.3% | 1.7% | 0.9% | 0.0% | 0.0% | 0.1% | 0.0% |
| e2e::readjust::drafts_merged_into_same_named_branches_of_two_repositories_are_absorbed_by_their_own | 1 | 225.7 | 97.0% | 1.6% | 1.1% | 0.0% | 0.0% | 0.2% | 0.1% |
| e2e::landing::every_answer_a_landing_read_gives_reads_back_on_the_node_line | 1 | 222.2 | 96.9% | 2.2% | 0.8% | 0.0% | 0.0% | 0.1% | 0.0% |
| e2e::readjust::a_draft_above_two_absorptions_is_placed_on_the_last_absorber | 1 | 221.3 | 98.1% | 0.6% | 1.0% | 0.0% | 0.0% | 0.2% | 0.1% |
| e2e::readjust::a_landing_during_a_rework_readjusts_the_rework_once_it_settles | 1 | 221.2 | 88.4% | 0.5% | 1.0% | 0.0% | 9.8% | 0.2% | 0.1% |
| e2e::loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on | 1 | 213.5 | 3.4% | 0.0% | 0.4% | 0.0% | 96.0% | 0.2% | 0.0% |
| e2e::readjust::a_repository_whose_read_failed_is_left_as_it_was_and_readjusted_by_the_next_trigger | 1 | 212.3 | 97.6% | 1.2% | 1.0% | 0.0% | 0.0% | 0.2% | 0.1% |
| e2e::rework::a_restack_whose_worker_fails_its_merge_settles_the_draft_failed | 1 | 207.6 | 98.1% | 0.6% | 1.1% | 0.0% | 0.0% | 0.2% | 0.0% |
| e2e::readjust::a_restack_readjusts_before_it_starts | 1 | 207.6 | 86.9% | 0.8% | 1.1% | 0.0% | 11.0% | 0.1% | 0.0% |
| e2e::readjust::a_rework_after_a_landing_places_its_replacement_on_the_root | 1 | 205.7 | 97.5% | 1.0% | 1.3% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::readjust::a_draft_merged_into_its_parent_s_branch_is_absorbed_and_lands_with_it | 1 | 204.3 | 97.7% | 0.8% | 1.1% | 0.0% | 0.0% | 0.3% | 0.1% |
| e2e::rework::a_restack_its_checks_refuse_is_asked_of_the_draft_s_worker_again | 1 | 200.8 | 97.7% | 0.8% | 1.3% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::out_of_band::a_branch_with_no_single_owner_lands_under_the_onevcs_default | 1 | 198.2 | 95.5% | 2.5% | 1.6% | 0.0% | 0.0% | 0.3% | 0.0% |
| e2e::listing::a_host_sized_runs_root_lists_in_seconds_and_ten_times_the_journal_bytes_barely_moves_it | 1 | 195.7 | 37.1% | 0.3% | 0.5% | 0.0% | 0.0% | 1.0% | 61.0% |
| e2e::adoption::the_elapsed_wait_a_supervisor_is_shown_counts_from_the_hold_that_is_running | 1 | 191.6 | 33.8% | 1.8% | 0.7% | 0.0% | 62.7% | 0.9% | 0.0% |
| e2e::rework::a_draft_in_review_is_reworked_on_its_own_branch_and_change_request | 1 | 191.4 | 96.6% | 1.9% | 1.3% | 0.0% | 0.0% | 0.1% | 0.1% |
| e2e::readjust::a_cold_host_reads_a_moved_heads_pending_verdict_again_until_it_is_green | 1 | 184.9 | 94.8% | 4.0% | 0.9% | 0.0% | 0.0% | 0.2% | 0.0% |

#### The 10 costliest modules

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::readjust | 44 | 6983.8 | 95.5% | 1.2% | 1.4% | 0.0% | 1.6% | 0.2% | 0.1% |
| e2e::adoption | 51 | 4884.0 | 13.6% | 6.0% | 2.3% | 0.0% | 76.9% | 1.0% | 0.2% |
| e2e::lifecycle | 100 | 3921.6 | 76.6% | 3.5% | 4.6% | 0.0% | 14.8% | 0.5% | 0.1% |
| e2e::rework | 19 | 3078.6 | 94.3% | 1.2% | 1.5% | 0.0% | 2.8% | 0.2% | 0.1% |
| e2e::continuation | 43 | 2604.1 | 53.8% | 2.0% | 2.7% | 0.0% | 33.1% | 0.3% | 8.1% |
| e2e::review | 21 | 1958.3 | 88.8% | 2.7% | 2.0% | 0.0% | 6.0% | 0.4% | 0.1% |
| e2e::publication_boundary | 30 | 1664.7 | 70.1% | 8.1% | 4.1% | 0.0% | 16.6% | 0.9% | 0.1% |
| e2e::out_of_band | 23 | 1489.3 | 89.3% | 6.9% | 3.1% | 0.0% | 0.0% | 0.5% | 0.0% |
| e2e::publish_preserve | 28 | 1129.0 | 62.1% | 6.8% | 8.3% | 0.0% | 20.6% | 1.9% | 0.3% |
| e2e::retirement | 23 | 986.7 | 68.5% | 10.7% | 4.5% | 0.0% | 15.2% | 0.8% | 0.2% |

### Against linux

Each figure is the platform's seconds in that category over linux's, summed over the tests both ran.

| platform | tests both ran | wall | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| linux-ci | 1658 | 0.35x | 0.34x | 0.14x | 0.15x | 0.12x | 0.66x | 0.22x | 0.51x |
| macos | 1629 | 0.46x | 0.47x | 0.44x | 0.14x | 0.19x | 0.80x | 0.26x | 0.41x |
| windows | 1099 | 2.05x | 2.18x | 3.64x | 0.72x | 0.26x | 2.74x | 2.87x | 1.24x |

</details>

## The crate split, and what it buys

### Population: 144 pull requests

The window is every first-parent commit on `origin/main` committed in [2026-09-15T00:00:00Z, 2026-10-16T00:00:00Z), less release-plz's `chore: release` commits. That is **144** squash-merged pull requests: those merged 2026-09-15 through 2026-10-15, inclusive. It is the population the earlier replay used:

    node scripts/crate-split-replay.mjs --mapping scripts/crate-split/proposed.json \
      --since 2026-09-15T00:00:00Z --until 2026-10-16T00:00:00Z --records <dir> --out <file>

A regeneration earlier in this dispatch passed `--until 2026-10-15`. That date is exclusive, so it dropped the window's last day and replayed 142. The two pull requests it left out both merged on 2026-10-15, and nothing else about them excludes them:
- 60d317d, `feat(deps)!: link the versioned project and document cache (#942)`, merged 2026-10-15T20:39:42Z;
- e6d813c, `fix(test): end every process a test spawns before the test returns (#922)`, merged 2026-10-15T22:18:40Z.

Both affect every suite under the proposed mapping. With them back in, the regeneration below is over 144, and the 142 figures are withdrawn.

### Proposed mapping (`scripts/crate-split/proposed.json`)

**Crates:**
- `onepipeline-sys` (no deps): the 5 `src` modules outside the dependency cycle, `src/sys.rs`, `error.rs`, `refusal.rs`, `rendercost.rs` and `hostphases.rs`.
- `onepipeline-core` (deps `onepipeline-sys`): the rest of `src/**` (the cycle the other 68 modules form) and `schemas/**`. It is also the fallback for any `src`/`crates`/`schemas` file no crate claims.
- `onepipeline` (deps `onepipeline-core`, `onepipeline-testfakes`): `src/main.rs`.
- `onepipeline-testfakes` (no deps): `crates/testfakes/**`.

**Shared by every crate:** `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `.cargo/config.toml`, `.config/nextest.toml`, `nx.json`, `package.json`, `package-lock.json`, `justfile`.

**Suites:** the e2e binary splits into nine suites, each exercising `onepipeline` and so every crate. Each suite's `paths` list the module files its binary compiles, its own and those it borrows through `crate::<module>::`, plus the files they read. Its own modules are:

| suite | own modules |
| --- | --- |
| e2e-landing | lifecycle, draft_lifecycle, landing, manager_landings, readjust, rework, stacked_drafts, publish_preserve, publication_boundary, real_vcs, amend, retirement, adoption, session, session_reuse, branch_template, destination, review |
| e2e-store | store, writeback_budget, writeback_cost, writeback_projections, delivers, multi_source, linear_writeback, holds, continuation, crossdag, plan, plan_check, live_edit, templates, shipped, criteria, criteria_rule, filter, linear_loopback |
| e2e-views | views, listing, host_view, summary, surface, journal, checkpoint, change_telemetry, compatibility, layout_document |
| e2e-dispatch | dispatch, agents, dispatch_env_hook, envelope_reviewer, node_validator, turns, malformed_envelopes, bus_config |
| e2e-channel | channel, ask, boundary, recorded_channel |
| e2e-runs | driver, driver_exit, shutdown, cancellation, run_ending, run_end_hooks, concurrency, out_of_band, unreadable_sessions, older_launch_record, flow |
| e2e-watch | watch, unwatched, stop_guard, wake_budget, loopcost, unpublished |
| e2e-maintenance | maintenance, scratch |
| e2e-harness | harness, profiling |

### Replay over the 144, weighted by the current records

**By the CI Linux records of dd2e5d6:**

144 merged changes, 9 suites: 58 of 1296 suite runs skipped (4.5%).
Test-seconds: 10407.2 per full run; 63440.7 skipped over the window (4.2%).

| suite | changes it runs on | unaffected | test-s per run |
| --- | ---: | ---: | ---: |
| e2e-landing | 139 | 3.5% | 3903.0 |
| e2e-store | 137 | 4.9% | 2956.8 |
| e2e-views | 136 | 5.6% | 743.4 |
| e2e-dispatch | 137 | 4.9% | 581.9 |
| e2e-channel | 137 | 4.9% | 242.3 |
| e2e-runs | 138 | 4.2% | 934.3 |
| e2e-watch | 138 | 4.2% | 794.3 |
| e2e-maintenance | 140 | 2.8% | 217.5 |
| e2e-harness | 136 | 5.6% | 33.7 |

Today's shape, the same records: 144 merged changes, 1 suites: 3 of 144 suite runs skipped (2.1%). Test-seconds: 10407.2 per full run; 31221.7 skipped over the window (2.1%).

**By the local Linux records of 4b94e43** (run `9b7d3bb7`, all 1,664 tests, `host_view` and the two timed-out tests included):

144 merged changes, 9 suites: 58 of 1296 suite runs skipped (4.5%).
Test-seconds: 18295.1 per full run; 111534.0 skipped over the window (4.2%).

| suite | changes it runs on | unaffected | test-s per run |
| --- | ---: | ---: | ---: |
| e2e-landing | 139 | 3.5% | 6896.7 |
| e2e-store | 137 | 4.9% | 4145.7 |
| e2e-views | 136 | 5.6% | 1736.5 |
| e2e-dispatch | 137 | 4.9% | 1106.8 |
| e2e-channel | 137 | 4.9% | 464.8 |
| e2e-runs | 138 | 4.2% | 1816.3 |
| e2e-watch | 138 | 4.2% | 1553.4 |
| e2e-maintenance | 140 | 2.8% | 420.1 |
| e2e-harness | 136 | 5.6% | 154.9 |

Today's shape, the same records (`--today`): 144 merged changes, 1 suites: 3 of 144 suite runs skipped (2.1%). Test-seconds: 18295.1 per full run; 54885.4 skipped over the window (2.1%).

**Earlier replay** over the same 144, weighted by the CI Linux records of a94701b, with the mapping at dd2e5d6 (identical to the mapping today), as first reported:

`node scripts/crate-split-replay.mjs --mapping scripts/crate-split/proposed.json --since 2026-09-15T00:00:00Z --until 2026-10-16T00:00:00Z --records <linux-ci records>` replays the **144** feature PRs merged 2026-09-15 to 2026-10-15 (release PRs excluded), with diffs from local git. Mapping at dd2e5d6; test-seconds from the CI-Linux records of a94701b.

**Proposed mapping** (`scripts/crate-split/proposed.json`):
- `onepipeline-sys`: the 5 `src` modules outside the dependency cycle (`sys`, `error`, `refusal`, `rendercost`, `hostphases`).
- `onepipeline-core`: the cycle that the other 68 modules form, plus `schemas/`.
- `onepipeline`: the binary.
- `onepipeline-testfakes`: the doubles.
- The e2e binary splits into 9 suites (`landing`, `store`, `views`, `dispatch`, `channel`, `runs`, `watch`, `maintenance`, `harness`). Each lists the module files its binary compiles and the files they read. Every suite exercises the binary crate and so every crate. The shared inputs are the Cargo manifest and lock, the toolchain, `.cargo/config.toml`, nextest config, `nx.json`, `package*.json` and `justfile`.

**Correction at f6a526e.** Suites compile modules that other suites own: e2e-landing reaches `driver`, `dispatch`, `maintenance`, `run_end_hooks` and four more through `crate::<module>::`. The mapping listed none of these borrowed files, so a change to one did not count as affecting the suites that compile it. The mapping now lists them, and the npm test holds every suite to the modules its binary compiles. The result moved from 4.7% / 4.6% to:

| suite | PRs it runs on | PRs leaving it unaffected | test-s per run |
| --- | ---: | ---: | ---: |
| e2e-landing | 139 | 3.5% | 3426.2 |
| e2e-store | 137 | 4.9% | 2815.3 |
| e2e-views | 136 | 5.6% | 666.6 |
| e2e-dispatch | 137 | 4.9% | 519.3 |
| e2e-channel | 137 | 4.9% | 198.8 |
| e2e-runs | 138 | 4.2% | 835.4 |
| e2e-watch | 138 | 4.2% | 724.0 |
| e2e-maintenance | 140 | 2.8% | 191.9 |
| e2e-harness | 136 | 5.6% | 29.6 |

- **Share of e2e suite runs Nx would skip: 4.5%** (58 of 1296), and **4.2% of e2e test-seconds** (57,558 of 1,354,608 over the window). Today's shape skips **2.1%**.
- **Ceiling.** Only **8 of 144** PRs touch none of `src/` (106 do), the shared inputs (85 do), or the harness and testfakes (75 do). Every suite drives the whole binary, so no split of this kind can skip much beyond about 5.6%.

### Compile and incremental build, measured on a prototype (f6a526e)

**The prototype.** `git archive` of f6a526e went into three scratch copies, never committed:
- **base:** today's layout.
- **split:** the proposal as mapped. `onepipeline-sys` is its own crate, re-exported into the root lib under the same module names. `reaped_pid` is compiled unconditionally, standing in for a test-support feature. The e2e binary becomes nine `[[test]]` targets, each with the harness and the modules it compiles, by `#[path]`.
- **split + harness lib:** the same, with the harness as a library crate (`onepipeline-e2e-harness`).

All three type-check (`cargo check --workspace --tests`) with no warnings.

**Settings, identical across layouts.**
- sccache off (`RUSTC_WRAPPER=`; this host's `~/.cargo/config.toml` wraps `rustc` in sccache, which CI does not), `--offline`, and each copy's own target dir.
- Each round runs the three layouts back to back.
- "Cold" is CI's case: `CARGO_INCREMENTAL=0`, dependencies built, workspace packages cleaned (CI's rust-cache restores dependencies only).
- "Incremental" is local `just gate`'s case: incremental on, warm, then one function appended to the file and `cargo test --workspace --no-run` timed.

**CPU** is user plus system time of cargo and every `rustc` and linker under it. It is the reliable figure here, since the host ran other dispatches at load 18-92 throughout. Wall time is given as medians but moves with that load.

| build | base | split | split + harness lib | split vs base |
| --- | ---: | ---: | ---: | ---: |
| **cold, whole job** (`cargo test --workspace --no-run`), CPU s, `-j20` x2 and `-j4` x2 | 574 (565-584) | 795 (774-835) | 785 (770-815) | **+38% CPU** |
| cold, whole job, wall s, median (`-j20`, 4 rounds) | 247 | 309 | 347 | +25% |
| cold, whole job, wall s, median (`-j4`, 3 rounds; a 4-vCPU runner's job count) | 326 | 312 | 426 | within noise |
| cold, one e2e binary only (`--lib --bins --test T`), CPU s | 405 (`e2e`) | 331 (`landing`), 286 (`maintenance`) | 337, 282 | **-18% to -30% CPU** |
| incremental, edit `src/views.rs` (core), CPU s, 2 rounds | 193 | 264 | 258 | **+36%** |
| incremental, edit the `sys` module, CPU s | 195 | 268 | 260 | **+38%** |
| incremental, edit `tests/e2e/landing.rs` (one test area), CPU s | 22.4 | 32.4 | 34.3 | **+45%** |
| incremental wall s, median of 5 (core / sys / test) | 51 / 55 / 30 | 74 / 75 / 22 | 72 / 111 / 35 | within noise |

Per-unit `--timings` (cold, `-j20`) show where the extra CPU goes. The e2e test binaries' summed unit time is 121-210 s for today's one binary, 575-597 s for the nine suites, and 459-550 s with the harness as a library. Each suite binary links every dependency again and compiles its borrowed modules again. A separate harness library removes only the harness's own share, which is why it barely helps.

**What the measurements replace.** The earlier figures in this report are superseded and should not be cited:
- The earlier cold timings (lib 227 s, e2e 122 s) were taken with sccache active.
- The "≤34% of the e2e rebuild" incremental figure was a projection by source share; the measured one-test-area rebuild costs **45% more** CPU, not less.
- The inferred "saves nothing for `src` changes" is confirmed and sharpened: `src` edits cost **36-38% more** CPU.
- A job that builds one suite alone does save 18-30% of compile CPU. But the replay has 95% of PRs running every suite, so per-suite jobs would build nine binaries where one sufficed, and a whole-job build costs 38% more.

**Runner and platform limitations.**
- Measured on this 20-core WSL2 Linux host only, under heavy load from other dispatches. CPU figures are robust to that; wall figures are not. `-j4` mimics a 4-vCPU runner's parallelism, not its CPU, memory or disk.
- macOS (3-core `macos-latest`) and Windows (`link.exe`, where per-binary linking is typically dearer) were not measured: no such host here, and the prototype is not to land, so it cannot run in this PR's CI. The split adds eight more test-binary links per job, so the cost is not expected to be smaller there, but that is not measured.
- Coverage-instrumented builds (`gate` uses `cargo llvm-cov`) were not measured; instrumentation adds per-binary cost.
- The prototype compiles faithfully but only partly runs. Its `e2e_harness` suite passed 26 of the 32 tests run before fail-fast stopped it. The failures come from the copy's location inside another repository (whose `onetaskgraph` config the binary discovers) and from records named after a binary called `e2e`, not from the split.

### Whole benefit

| | effect of the proposed split | evidence |
| --- | --- | --- |
| e2e suite runs Nx skips | **4.5%** (58 of 1,296), against **2.1%** for today's shape; ceiling about 5.6% | replay of the 144 over the mapping at dd2e5d6, the same on CI-Linux dd2e5d6, local 4b94e43 and CI-Linux a94701b weights |
| e2e test-seconds skipped | **4.2%**: 63,441 of 1,498,637 by CI-Linux dd2e5d6, 111,534 of 2,634,494 by local 4b94e43, 57,558 of 1,354,608 by CI-Linux a94701b | the same replay |
| compile per CI job | **+38% CPU** for a whole-job build (every suite, which 95% of PRs need); -18% to -30% only for a job that builds one suite | prototype of f6a526e, local Linux, uninstrumented |
| incremental local rebuild | **+36-38%** CPU for a source edit, **+45%** for a one-area test edit | the same prototype |
| per-test cost | **unchanged**: every test spawns the same binary and does the same work | the records: per-test work is the binary's, not the crate layout's |

**Verdict: the split as proposed buys no meaningful saving. It is a net cost.** It skips about 4% of e2e test-seconds and adds about 38% to every job's compile CPU and 36-45% to local rebuilds. Moving the harness into a library does not change that. The manager has taken the drop-or-reorder decision to the user.

## Cheaper strategies, per e2e module

The strategy codes and their addressable seconds, computed as first reported over CI Linux a94701b, macOS 3c728a6 and Windows a17a96a:

Seconds addressable (CI Linux a94701b / macOS 3c728a6 / Windows a17a96a partial; shares of e2e test-seconds):

| code | strategy | addressable | coverage |
| --- | --- | --- | --- |
| **A** | Build the test doubles once per run (a nextest setup script, or the Nx target before the tests) and hand each test their paths, instead of `cargo build --offline` in every test process | the whole `build` category: 671 s (7.1%) / 653 s (5.4%) / 2,025 s (5.4%) | **Keeps** all of it: same binaries, same tests |
| **B** | Scale the intervals the waits stand for (claim interval, lease, takeover wait, retry and grace, already env-configurable) so each "nothing happened for N s" holds a shorter N in the same ratio | `wait` in these modules: 1,689 s (18.0%) / 2,110 s (17.5%) / 5,405 s (14.5%), up to that saved | **Keeps** the behaviour if each ratio is kept. A shorter absolute window is less margin over scheduler jitter, and the Windows fence and takeover flakes show the margin is already thin, so each needs checking on Windows |
| **C** | Shorten the budget-measuring journeys (`writeback_budget`, `writeback_cost`, `loopcost`), whose duration *is* the evidence | `wait` 703 s (7.5%) / 696 s (5.8%) / 337 s (0.9%) | **Reduces** it: a shorter measurement window is a weaker budget proof. **Manager decides** |
| **D** | Prove the logic these journeys assert (projection, restack, store writes, title rules) at library level in-process through `src/lib.rs`, keeping one or two binary-driven journeys per verb | `onepipeline` runs in these modules: 2,657 s (28.2%) / 4,336 s (35.9%) / 20,257 s (54.5%) | **Reduces** it: per-case coverage of the process boundary (CLI parsing, exit codes, stdout/stderr, detached drivers), which AGENTS.md's realistic-test invariant holds e2e to. **Manager decides** |
| **E** | Build the host-sized fixture once and share it (or write it through the library and golden-check one copy against a CLI-built one), instead of dozens of CLI runs per test | spawn 319 s (3.4%) / 134 s (1.1%) / 147 s (0.4%) | **Keeps** it, since the verb under test still reads the fixture through the binary, provided the golden check holds |

Cheaper Windows per-process cost (Dev Drive, a Defender exclusion) multiplies through everything above. Windows runs `onepipeline` at 3.55 s a run against Linux's 0.49, and that is 68% of its e2e time. The spike proposed this; it stays unmeasured here.

**For the manager to decide.** Two of these reduce coverage and are not applied:
- **C**, shortening the budget-measuring journeys in `writeback_budget`, `writeback_cost` and `loopcost`: a weaker budget proof.
- **D**, moving per-case assertions to library level in `lifecycle`, `readjust`, `rework`, `review`, `retirement`, `run_end_hooks`, `publication_boundary`, `out_of_band`, `publish_preserve`, `manager_landings`, `stacked_drafts`, `branch_template`, `draft_lifecycle`, `criteria`, `landing`, `unpublished`, `change_telemetry` and `real_vcs`: less per-case coverage of the process boundary.

A, B and E keep coverage. B keeps it only if each timing ratio is held and each change is checked on Windows.

### Every e2e module, on the current records

Seconds per module: local Linux 4b94e43 (run `9b7d3bb7`), and CI Linux, macOS and Windows from run 39098714538 at dd2e5d6 ("–" means Windows did not reach the module before the step limit). The category shares are of the local 4b94e43 seconds. The proposal codes are the ones assigned above.

| module | tests | local Linux s (4b94e43) | CI Linux s | macOS s | Windows s | spawn | wait | build | proposal |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| continuation | 44 | 1401 | 1093 | 1197 | 2530 | 41% | 44% | 3% | A, B |
| adoption | 51 | 1359 | 626 | 974 | 4724 | 21% | 54% | 21% | A, B |
| lifecycle | 102 | 1307 | 745 | 1187 | 3991 | 80% | 8% | 9% | A, D |
| readjust | 44 | 907 | 628 | 1333 | 7695 | 92% | 2% | 4% | A, D |
| host_view | 10 | 707 | 225 | – | – | 97% | 0% | 3% | A, E |
| rework | 20 | 654 | 359 | 526 | 3227 | 92% | 3% | 3% | A, D |
| dispatch | 71 | 549 | 252 | 262 | 531 | 66% | 3% | 29% | A |
| writeback_budget | 12 | 529 | 496 | 501 | – | 15% | 82% | 2% | A, C |
| loopcost | 10 | 528 | 278 | 274 | 363 | 7% | 81% | 3% | A, C |
| store | 46 | 513 | 361 | 439 | – | 44% | 40% | 9% | A, B |
| driver | 90 | 503 | 252 | 276 | 273 | 75% | 7% | 16% | A |
| review | 21 | 497 | 304 | 425 | 1996 | 87% | 4% | 4% | A, D |
| retirement | 38 | 431 | 245 | 402 | 984 | 66% | 19% | 8% | A, B,D |
| linear_writeback | 11 | 380 | 318 | 412 | 252 | 9% | 88% | 2% | A, B |
| unwatched | 27 | 345 | 155 | 111 | 108 | 61% | 3% | 7% | A, E |
| run_end_hooks | 32 | 344 | 177 | 185 | 296 | 85% | 2% | 11% | A, D |
| listing | 10 | 332 | 114 | 98 | 228 | 62% | 0% | 2% | A, E |
| channel | 62 | 326 | 153 | 133 | 271 | 63% | 9% | 23% | A |
| publication_boundary | 35 | 321 | 202 | 390 | 1533 | 72% | 12% | 10% | A, D |
| views | 66 | 312 | 188 | 190 | – | 73% | 7% | 17% | A |
| live_edit | 46 | 302 | 111 | 105 | 257 | 61% | 13% | 23% | A |
| out_of_band | 27 | 292 | 153 | 276 | 1253 | 84% | 1% | 8% | A, D |
| manager_landings | 39 | 268 | 139 | 203 | – | 65% | 12% | 16% | A, D |
| watch | 44 | 263 | 150 | 138 | – | 72% | 8% | 16% | A |
| scratch | 29 | 227 | 120 | 107 | 170 | 78% | 7% | 13% | A |
| stacked_drafts | 13 | 223 | 121 | 183 | 310 | 75% | 7% | 6% | A, D |
| publish_preserve | 30 | 221 | 145 | 239 | 946 | 77% | 10% | 10% | A, D |
| maintenance | 23 | 193 | 97 | 127 | 206 | 36% | 38% | 10% | A, B |
| shutdown | 30 | 189 | 83 | 105 | 201 | 64% | 14% | 19% | A, B |
| wake_budget | 22 | 181 | 106 | 92 | – | 77% | 8% | 13% | A |
| unpublished | 23 | 180 | 74 | 124 | – | 82% | 0% | 12% | A, D |
| branch_template | 16 | 179 | 111 | 155 | 609 | 90% | 0% | 8% | A, D |
| dispatch_env_hook | 10 | 165 | 117 | 208 | 157 | 91% | 2% | 5% | A |
| flow | 22 | 158 | 80 | 84 | – | 67% | 14% | 17% | A |
| templates | 24 | 156 | 95 | 70 | – | 78% | 0% | 19% | A |
| landing | 5 | 155 | 80 | 165 | 923 | 94% | 0% | 3% | A, D |
| criteria | 17 | 153 | 74 | 98 | 447 | 78% | 5% | 13% | A, D |
| delivers | 16 | 139 | 104 | 100 | 125 | 20% | 67% | 12% | A, B |
| draft_lifecycle | 15 | 129 | 79 | 154 | 562 | 88% | 0% | 9% | A, D |
| harness | 30 | 129 | 24 | 16 | 32 | 75% | 2% | 16% | A |
| driver_exit | 19 | 124 | 87 | 95 | 43 | 68% | 18% | 13% | A, B |
| summary | 12 | 121 | 68 | 56 | – | 68% | 1% | 13% | A |
| malformed_envelopes | 12 | 97 | 61 | 63 | 82 | 83% | 3% | 12% | A |
| session_reuse | 4 | 94 | 45 | 67 | 289 | 75% | 20% | 4% | A, B |
| journal | 16 | 91 | 49 | 50 | 55 | 75% | 3% | 19% | A |
| filter | 14 | 91 | 49 | 53 | 75 | 81% | 0% | 17% | A |
| change_telemetry | 9 | 83 | 55 | 73 | 312 | 85% | 3% | 10% | A, D |
| writeback_projections | 11 | 83 | 30 | 33 | – | 58% | 20% | 17% | A, B |
| node_validator | 11 | 81 | 47 | 48 | 86 | 84% | 0% | 14% | A |
| plan | 22 | 78 | 40 | 45 | 77 | 54% | 18% | 25% | A |
| ask | 10 | 76 | 52 | 50 | 66 | 73% | 11% | 14% | A, B |
| agents | 6 | 66 | 26 | 32 | 140 | 60% | 14% | 23% | A |
| envelope_reviewer | 11 | 63 | 36 | 45 | 72 | 81% | 4% | 13% | A |
| run_ending | 8 | 60 | 31 | 28 | 40 | 77% | 3% | 16% | A |
| stop_guard | 11 | 57 | 31 | 37 | – | 73% | 3% | 20% | A |
| unreadable_sessions | 5 | 56 | 33 | 40 | – | 67% | 22% | 9% | A, B |
| multi_source | 10 | 56 | 31 | 35 | 186 | 41% | 36% | 13% | A, B |
| destination | 25 | 55 | 24 | 19 | 33 | 35% | 0% | 53% | A (under 20 s: little to gain) |
| crossdag | 8 | 55 | 36 | 35 | 50 | 83% | 1% | 14% | A |
| cancellation | 10 | 53 | 22 | 25 | 42 | 59% | 16% | 22% | A, B |
| bus_config | 12 | 51 | 23 | 22 | 42 | 74% | 1% | 23% | A (under 20 s: little to gain) |
| boundary | 10 | 47 | 28 | 27 | 44 | 79% | 0% | 20% | A |
| writeback_cost | 1 | 46 | 37 | 37 | – | 10% | 86% | 2% | A, C |
| criteria_rule | 7 | 46 | 22 | 20 | 42 | 79% | 0% | 19% | A (under 20 s: little to gain) |
| checkpoint | 6 | 44 | 26 | 24 | 40 | 72% | 13% | 14% | A, B |
| holds | 6 | 44 | 22 | 23 | 32 | 64% | 17% | 16% | A, B |
| amend | 8 | 44 | 20 | 21 | 77 | 71% | 2% | 25% | A (under 20 s: little to gain) |
| real_vcs | 4 | 43 | 26 | 50 | 211 | 90% | 0% | 8% | A, D |
| shipped | 15 | 41 | 23 | 23 | 42 | 74% | 0% | 17% | A |
| turns | 6 | 35 | 20 | 19 | – | 80% | 0% | 19% | A (under 20 s: little to gain) |
| concurrency | 3 | 33 | 14 | 12 | 66 | 66% | 18% | 13% | A, B |
| plan_check | 14 | 33 | 15 | 15 | 40 | 46% | 0% | 50% | A (under 20 s: little to gain) |
| profiling | 22 | 26 | 10 | 12 | 22 | 55% | 20% | 17% | A (under 20 s: little to gain) |
| surface | 12 | 23 | 10 | 5 | – | 58% | 0% | 39% | A (under 20 s: little to gain) |
| compatibility | 7 | 17 | 7 | 6 | 15 | 53% | 8% | 36% | A (under 20 s: little to gain) |
| recorded_channel | 8 | 16 | 9 | 15 | 23 | 65% | 0% | 30% | A (under 20 s: little to gain) |
| session | 2 | 8 | 4 | 4 | 7 | 65% | 7% | 25% | A (under 20 s: little to gain) |
| older_launch_record | 1 | 6 | 4 | 3 | 7 | 83% | 0% | 14% | A (under 20 s: little to gain) |
| layout_document | 2 | 6 | 2 | 2 | 9 | 75% | 3% | 18% | A (under 20 s: little to gain) |
| linear_loopback | 1 | 0 | 0 | 0 | 0 | 0% | 0% | 0% | A (under 20 s: little to gain) |

<details><summary>Every e2e module, earlier records (CI Linux a94701b, macOS 3c728a6, Windows a17a96a partial: "–" = not reached before the timeout)</summary>

| module | tests | CI-Linux s | macOS s | Windows s | spawn | wait | build | proposal |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| continuation | 43 | 1073 | 1157 | 2604 | 35% | 53% | 1% | A, B |
| lifecycle | 102 | 687 | 867 | 3922 | 87% | 6% | 6% | A, D |
| adoption | 51 | 546 | 736 | 4884 | 20% | 74% | 4% | A, B |
| readjust | 44 | 536 | 1109 | 6984 | 93% | 2% | 4% | A, D |
| writeback_budget | 12 | 492 | 498 | – | 12% | 87% | 1% | A, C |
| store | 46 | 349 | 453 | – | 37% | 51% | 4% | A, B |
| linear_writeback | 10 | 317 | 405 | 272 | 4% | 95% | 1% | A, B |
| rework | 20 | 303 | 680 | 3079 | 92% | 4% | 3% | A, D |
| review | 21 | 288 | 467 | 1958 | 92% | 3% | 3% | A, D |
| loopcost | 9 | 260 | 250 | 384 | 4% | 94% | 2% | A, C |
| retirement | 38 | 228 | 401 | 987 | 66% | 24% | 7% | A, B,D |
| driver | 88 | 226 | 221 | 284 | 74% | 9% | 17% | A |
| dispatch | 71 | 225 | 237 | 575 | 80% | 3% | 16% | A |
| host_view | 10 | 175 | 0 | – | 98% | 0% | 2% | A, E |
| run_end_hooks | 32 | 168 | 200 | 290 | 90% | 2% | 7% | A, D |
| views | 66 | 164 | 218 | – | 74% | 8% | 17% | A |
| publication_boundary | 35 | 157 | 238 | 1665 | 75% | 12% | 9% | A, D |
| unwatched | 27 | 156 | 116 | 127 | 50% | 2% | 7% | A, E |
| listing | 10 | 142 | 78 | 246 | 49% | 0% | 2% | A, E |
| out_of_band | 27 | 137 | 219 | 1489 | 87% | 1% | 8% | A, D |
| watch | 44 | 133 | 151 | – | 76% | 7% | 14% | A |
| publish_preserve | 30 | 132 | 172 | 1129 | 81% | 9% | 9% | A, D |
| channel | 62 | 118 | 113 | 266 | 58% | 12% | 26% | A |
| dispatch_env_hook | 10 | 111 | 167 | 179 | 96% | 1% | 3% | A |
| manager_landings | 39 | 108 | 163 | – | 65% | 13% | 18% | A, D |
| scratch | 29 | 102 | 115 | 164 | 85% | 4% | 11% | A |
| branch_template | 16 | 100 | 130 | 574 | 94% | 0% | 6% | A, D |
| delivers | 16 | 98 | 98 | 128 | 6% | 87% | 7% | A, B |
| stacked_drafts | 12 | 97 | 218 | – | 82% | 1% | 5% | A, D |
| maintenance | 23 | 90 | 110 | 211 | 27% | 60% | 7% | A, B |
| wake_budget | 22 | 90 | 102 | – | 82% | 6% | 10% | A |
| driver_exit | 19 | 81 | 88 | 46 | 67% | 23% | 9% | A, B |
| live_edit | 46 | 81 | 82 | 303 | 62% | 8% | 28% | A |
| templates | 24 | 76 | 72 | – | 83% | 0% | 16% | A |
| draft_lifecycle | 15 | 75 | 105 | 565 | 92% | 0% | 7% | A, D |
| shutdown | 30 | 69 | 122 | 196 | 65% | 14% | 19% | A, B |
| flow | 22 | 69 | 72 | – | 76% | 12% | 12% | A |
| criteria | 17 | 66 | 91 | 509 | 84% | 5% | 9% | A, D |
| landing | 5 | 65 | 108 | 813 | 95% | 0% | 3% | A, D |
| unpublished | 23 | 62 | 136 | – | 84% | 0% | 13% | A, D |
| summary | 12 | 57 | 61 | – | 77% | 3% | 7% | A |
| malformed_envelopes | 12 | 56 | 59 | 90 | 87% | 4% | 7% | A |
| change_telemetry | 9 | 48 | 65 | 303 | 90% | 2% | 7% | A, D |
| ask | 10 | 47 | 49 | 63 | 73% | 18% | 9% | A, B |
| journal | 16 | 45 | 45 | 59 | 81% | 2% | 17% | A |
| filter | 14 | 43 | 42 | 84 | 88% | 0% | 11% | A |
| session_reuse | 4 | 41 | 83 | 71 | 70% | 24% | 4% | A, B |
| node_validator | 11 | 39 | 42 | 98 | 88% | 0% | 12% | A |
| writeback_cost | 1 | 34 | 35 | – | 5% | 94% | 1% | A, C |
| crossdag | 8 | 34 | 35 | 52 | 89% | 1% | 9% | A |
| plan | 22 | 33 | 31 | 158 | 51% | 15% | 34% | A |
| unreadable_sessions | 5 | 28 | 44 | – | 78% | 14% | 6% | A, B |
| envelope_reviewer | 11 | 28 | 28 | 77 | 77% | 1% | 20% | A |
| boundary | 10 | 27 | 28 | 40 | 87% | 0% | 13% | A |
| run_ending | 8 | 26 | 31 | 41 | 88% | 1% | 10% | A |
| real_vcs | 4 | 25 | 48 | 210 | 93% | 0% | 6% | A, D |
| multi_source | 10 | 25 | 33 | 237 | 26% | 50% | 16% | A, B |
| agents | 6 | 24 | 23 | 138 | 73% | 9% | 15% | A |
| stop_guard | 11 | 23 | 50 | – | 75% | 1% | 22% | A |
| writeback_projections | 11 | 23 | 36 | – | 54% | 22% | 17% | A, B |
| checkpoint | 6 | 23 | 23 | 45 | 68% | 22% | 9% | A, B |
| shipped | 15 | 22 | 25 | 42 | 87% | 0% | 12% | A |
| harness | 30 | 21 | 16 | 38 | 29% | 25% | 35% | A |
| holds | 6 | 20 | 22 | 37 | 71% | 17% | 12% | A, B |
| criteria_rule | 7 | 19 | 20 | 44 | 88% | 0% | 11% | A (under 20 s: little to gain) |
| turns | 6 | 19 | 20 | – | 86% | 0% | 14% | A (under 20 s: little to gain) |
| cancellation | 10 | 18 | 20 | 44 | 56% | 23% | 20% | A, B |
| bus_config | 12 | 18 | 17 | 44 | 67% | 1% | 31% | A (under 20 s: little to gain) |
| destination | 25 | 17 | 16 | 33 | 22% | 0% | 69% | A (under 20 s: little to gain) |
| amend | 8 | 17 | 19 | 59 | 74% | 1% | 24% | A (under 20 s: little to gain) |
| plan_check | 14 | 12 | 10 | 86 | 34% | 0% | 65% | A (under 20 s: little to gain) |
| concurrency | 3 | 10 | 13 | 66 | 47% | 31% | 19% | A, B |
| profiling | 22 | 9 | 9 | 25 | 10% | 60% | 26% | A (under 20 s: little to gain) |
| surface | 12 | 7 | 6 | – | 55% | 0% | 44% | A (under 20 s: little to gain) |
| recorded_channel | 8 | 7 | 6 | 20 | 58% | 0% | 37% | A (under 20 s: little to gain) |
| compatibility | 7 | 5 | 4 | 14 | 45% | 4% | 50% | A (under 20 s: little to gain) |
| session | 2 | 4 | 4 | 7 | 76% | 4% | 18% | A (under 20 s: little to gain) |
| older_launch_record | 1 | 3 | 3 | 7 | 87% | 0% | 13% | A (under 20 s: little to gain) |
| layout_document | 2 | 1 | 1 | 8 | 69% | 3% | 26% | A (under 20 s: little to gain) |
| linear_loopback | 1 | 0 | 0 | 0 | 0% | 0% | 0% | A (under 20 s: little to gain) |

</details>

## host_view on this host

`host_view::the_host_view_answers_a_host_sized_root_steady_cold_and_steady_again` is the only e2e test that fails on this host, and it fails the same way with and without this branch.

**Why it fails here.** Its cold-read check reads `rchar` from `/proc/thread-self/io`. This WSL2 kernel (6.18.54.1-microsoft-standard-WSL2, `# CONFIG_TASKSTATS is not set`) has no `/proc/*/io` at all; `ls /proc/self/io` answers *No such file or directory*. Every phase therefore records `read_bytes` 0, and the check cannot pass on this host under any load. The cause is the missing I/O accounting, not page caching.

**Where it passes.** It passes in CI `gate` at a94701b and at dd2e5d6.

**Comparison runs:**

| run | commit | `Cargo.lock` | load average | result |
| --- | --- | --- | --- | --- |
| full e2e tier | 7574f2f | head's | 4.0 → 9.8 | `FAIL [97.408s]` at `tests/e2e/host_view.rs:799:5`, `"read_bytes":0`, the only failure of 1,661 |
| full e2e tier | c551671 | head's | (shared host) | `FAIL [111.636s]`, the only failure of 1,660 |
| single test, back to back | 049ba74 (base) | byte-identical copy of c551671's | 6.3 → 7.0 | failed at `tests/e2e/host_view.rs:799:5` in 168.64 s, `"read_bytes":0`, `"cold-journal-bytes":261654914` |
| single test, back to back | c551671 | head's | 7.0 → 13.9 | failed at the same line in 170.87 s, the same `read_bytes` and journal bytes |
| single test | 049ba74 | base's own | 13.6 → 30.7 | the same failure |
| single test | dd2e5d6 | base's own | 15–18 | the same failure |

The earlier runs at a94701b, dd2e5d6 and 049ba74 are recorded under the Checks heading below. This branch's only change to that file is the `Command`/`Child`/`Stdio` import.

## maintenance on Windows, run 39129002719

`maintenance::a_sweep_over_identities_that_declare_nothing_journals_nothing` failed once in `cross proof (windows-latest)`, job 117680287750, on 0cbe9aa. It failed at `tests/e2e/maintenance.rs:2191`, the assertion that the run journaled no `pool-maintenance` record. The same job's `rerun-failed` step passed it 3 of 3 times on the same build.

**What the journal shows.** The driver had started 3 sweeps (`maintenance_sweeps: 3`). The test waits for 2. The one record is the third sweep:
- It started at 13:27:08.526 and recorded at .665 with `identities: []` and `cut_short: {reason: driver-closing, unreached: [github.com/owner/service]}`.
- The released node's last event is at .529, and the checkpoint still records the node as `running`.

**Why it happens.** The driver starts a sweep whenever a pass is idle and the pace has elapsed since the last sweep *started* (`Maintenance::consider`). Under the one-second pace, the test has one second less sweep 2's length to do four things: see sweep 2 end, release the node, let the node finish, and let the driver close. On this runner that took the whole second. Sweep 3 began as the node finished, and the closing driver recorded it as cut short. Recording a cut-short pass is the product's contract (`maintenance::tests::a_pass_cut_short_is_recorded_with_what_it_never_reached_and_a_whole_one_is_not_marked`). The test's empty-journal assertion, made after the close, therefore holds only while the release beats the next sweep.

**Not this branch's code.** This branch changes:
- neither `src/maintenance.rs`, the driver loop, nor the fake doubles;
- in this test, nothing: its `profile::thread::sleep` replacements are in five other tests in the file.

What the test process gains is the profiling wrappers around `world.until`'s poll and `release`'s write: a span opened and closed, which is microseconds. That cannot move a window of about a second.

**Not shown on a tree without this branch.**
- Main's Windows and macOS proofs passed this test in each of the 7 job logs read that ran it: runs 39115147571, 39080139044, 39023278916, 39012247843, 39008698390, 38998935272 and 38887444753.
- On this host it did not fail:
  - 48 of 48 passed at the one-second pace, 16 concurrent copies at load 10-12, on 997aaa8;
  - with an injected 0.5-1.2 s delay before `release`, every run passed under both paces, because a sweep here ends long before the node's close. Those delays were reverted.

**The fix (6c4a488).** Under the amendment, a failure not shown without the change is fixed with every assertion kept, so the journey now runs at `CLOSING_PACE`, three seconds. A close then lands inside a sweep only if the release takes about 2.7 s, against the roughly one second this runner took. Every assertion and wait is unchanged, and the test passes on this host in 6.3 s. That the Windows flake is gone rests on the timeline above, because Linux does not reproduce it.

**The hosted result on 6c4a488's successor (4b94e43, run 39138389849).** The test **passed on Windows** in 13.614 s: `cross proof (windows-latest, shard 1/2)`, job 117711816829, test 8 of 709. It passed again in `Mac PR proofs`, and that job and `gate` both passed.

**That shard failed on another test.** `continuation::the_claim_is_held_while_a_run_drives_and_released_at_closeout_and_at_stop` panicked at `tests/e2e/harness.rs:1253`. Its in-process read of the world's plan store through `onetaskgraph-core` was refused: `...\plan-store\projects\stopped-board.md escapes configured root ...\plan-store`. The job's `rerun-failed` step passed it 3 of 3 on the same build. It is not traced to this change:
- the store code is the sibling engine's, and `Cargo.lock` links the same onetaskgraph releases as main (on 4b94e43 it differed only in oneagentgraph, onevcs and onevcs-testing; 9809e12 adds oneharness-core);
- this branch changes no store path or call;
- main's latest Windows shards (run 39135168556, at 4d975bc) did not show it, and neither did the 7 earlier main job logs read above.

The `cross (windows-latest)` carrier failed because shard 1 did. That flake is drafted as a follow-up rather than fixed here.

Five other journeys in `tests/e2e/maintenance.rs` assert an empty journal after a `release` under the same pace. None of them is named here, so they are left as a follow-up.

## Checks

**On 059dfb4** (the merge of main at e788a5c, with no conflict):
- With `ONETASKGRAPH_*` unset, `cargo nextest run --locked --no-fail-fast --test e2e -E 'test(/plugins_off_unless/) or test(/^profiling::/) or test(/^dispatch::/)'` passed **95 of 95**, at load 18-35. That covers both of #904's new journeys and the 22 `profiling::` journeys.
- `cargo fmt --check` and `cargo clippy --locked --all-targets -- -D warnings`: clean.
- `just engines-current`: passed. oneagentgraph 0.5.14, onevcs 0.52.1, onevcs-testing 0.19.5, onejudge 0.19.5 and oneharness-core 0.28.3 are each the newest their own requirement permits. onevcs-testing 0.19.11 is held back by `onevcs = "0.52.0"`, which the check names and passes.
- On c8a0f9e, the commit that added this entry: `just lint-llm-diff origin/main`, against e788a5c, gave 39 rules, 21 passed, 0 failed. `LLMLINT_ONEHARNESS_BIN` was unset because it names a missing wrapper. The commit after it adds only this line, so that run did not judge this line.
- Not re-run on 059dfb4: the full e2e tier and the hosted CI.

**On ff19d5c** (the merge of main at 645ca97, whose conflict was `tests/e2e/harness.rs`):
- `cargo nextest run --locked --no-fail-fast` over `build_config`, the e2e `driver::`, `harness::` and `profiling::` modules, and the 18 e2e tests elsewhere that reach `end_process`: 174 of 175 passed, at load 31-37.
- **The one failure is a load flake that main shows too.** `harness::a_worlds_teardown_names_a_killed_descendant_it_could_not_wait_out` failed at `tests/e2e/harness.rs:4436`: "pid … never finished its exit". In the failing runs the teardown, given 1 s of patience, never signalled the doubles: they were still in state `S` minutes later. Alone, it then failed 3 more times and passed once. Run alternately, 4 times with main's `crates/testfakes/src/containment.rs` and 4 with this branch's (which routes `ps` and its sleeps through the profiler), it failed once with each, at load 57 and 29. Each other run passed. The 1 s patience racing `ps` listings on a loaded host fails the same way without this change. The run of `harness::` on a07bbdd, at lower load, passed it.
- **On 41000a7 it passed.** Re-run alone and unchanged (`cargo nextest run --locked --test e2e -E 'test(=harness::a_worlds_teardown_names_a_killed_descendant_it_could_not_wait_out)'`), with 585G free on disk and load 49.6, it passed on the first attempt in 16.6 s. 41000a7 differs from ff19d5c only in this document.
- `cargo clippy --locked --all-targets -- -D warnings` and `cargo fmt --check`: clean. `just engines-current`: passed.
- Not re-run since these merges: the full e2e tier, and the hosted CI on this commit.

**On 875a01c** (the merge of main at 589d81b, whose conflicts were `src/continuation.rs` and `tests/e2e/project.json`):
- `cargo nextest run --locked --no-fail-fast` over `ci_workflows`, `budgets`, `nextest_runs`, the e2e `continuation`, `review_reply`, `review_round` and `profiling` modules, and the lib's `continuation` tests: 160 of 160 passed.
- The smoke binary's `continuation::` tests: the 4 offline ones passed. `a_stored_project_is_carried_on_by_a_cold_host_that_takes_it_over` refused, because `ONEPIPELINE_SMOKE_REPO` is unset here; it is the credentialled tier (`just smoke-real`) and refuses by design.
- `cargo clippy --locked --all-targets -- -D warnings` and `cargo fmt --check`: clean.
- `just engines-current`: passed.

**On 9809e12** (the merge of main at 1e18274, whose conflict was `tests/e2e/harness.rs`, and the oneharness-core refresh):
- On a07bbdd, the merge: `cargo nextest run --test e2e -E 'test(/harness::|multi_source::|profiling::|linear_writeback::/)'`, 76 of 76 passed. That includes #1016's `harness::a_runs_settling_patience_is_one_step_per_node_on_its_longest_chain` and `multi_source::a_settlement_wait_outlasts_one_step_while_each_dispatch_on_a_cross_source_chain_is_held`, and #986's `linear_writeback::the_github_endpoint_url_is_readable_only_once_completely_written`.
- On a07bbdd: `cargo clippy --test e2e -- -D warnings` and `cargo fmt --check`: clean.
- On a07bbdd: `just lint-llm-diff origin/main`, against 1e18274: 39 rules, 21 passed, 0 failed. The dispatch's `LLMLINT_ONEHARNESS_BIN` named a wrapper that no longer exists, so the run unset it and used `oneharness` from PATH.
- On 9809e12: `just engines-current` passed; `cargo build --workspace --tests --locked` built; `cargo nextest run --test linked_engines` (33 of 33) and the lib's `agentgraph` tests (34 of 34) passed. Both test runs were on the 9809e12 lock.

**On 4b94e43** (head before this evidence):
- **The full e2e tier**, instrumented as `gate` runs it (`cargo llvm-cov --no-report nextest --locked -E 'binary(e2e)' --final-status-level fail --no-fail-fast`, `ONETASKGRAPH_*` unset), ran as `9b7d3bb7` at load 12-59. **1,661 of 1,664 passed in 5,938.0 s.**
- `host_view` failed at its known host-only line, which base 049ba74 fails the same way.
- `adoption::a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs` and `loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on` timed out at 360 s at the start of the run. Re-run together afterwards, both passed (130.1 s and 235.0 s).
- The run's 1,664 records were copied aside before that re-run. Every local figure above is regenerated from them: the Linux breakdown, the ratios against local Linux, the local replay weighting, the whole-benefit figure and the per-module table's local column.
- **Hosted run 39138389849:** `gate` passed, `Mac PR proofs` passed and Windows shard 2 passed. Windows shard 1 failed on the `continuation` flake above, with `maintenance::a_sweep_over_identities_that_declare_nothing_journals_nothing` passing in it.

**On 8b5655c** (the merge of main at 4d975bc; its conflict was `tests/smoke/continuation.rs`):
- `continuation::waiting::` in the smoke binary: 4 of 4 passed.
- The 7 e2e `driver::` observer journeys, including #994's `a_restart_the_observers_graph_refuses_is_reported_and_not_asked_again`: 7 of 7 passed.
- `cargo fmt --check` and `cargo clippy --test smoke --test e2e --locked -D warnings`: clean.
- `just engines-current`: passed. `Cargo.lock` still differs from main only in the three engines.
- `just lint-llm-diff origin/main`, against 4d975bc: 39 rules, 21 passed, 0 failed.

**On d085c0d.** The judged lint on 9c102f4 reported one `changed_behavior_has_e2e` finding: nothing held a record's persisted `unattributed_s` to what the writer subtracts. `profiling::a_records_inclusive_and_background_figures_are_the_ones_the_test_measured` now holds it to the record's `wall_s` less every category's own `s`, within a microsecond. It failed as intended at `tests/e2e/profiling.rs:405` when the writer persisted the whole wall clock as unattributed; that was reverted. Re-run on d085c0d:
- the 22 e2e `profiling::` journeys: 22 of 22 passed;
- `cargo fmt --check` and `cargo clippy --test e2e --locked -D warnings`: clean;
- `just lint-llm-diff origin/main`: 39 rules, 21 passed, 0 failed.

**On 6c4a488** (the merge of main at 54cfa4a, and the maintenance pace):
- **The merge.** It kept both sides of `tests/ci_workflows.rs`: main's sharded-Windows test and this branch's two profile-record tests. `ci_workflows::every_test_leg_keeps_the_profile_records_from_where_the_harness_writes_them` now renders each leg's artifact name and requires them distinct. Against the merged `ci.yml`, before the rename, it failed as intended: `["test-profile-gate", "test-profile-windows-latest", "test-profile-windows-latest", "test-profile-macos-latest"]`.
- `cargo test --test ci_workflows`: 11 of 11 passed, on 997aaa8.
- `node --test .github/tests/`: 43 of 43 passed, on 997aaa8.
- `just cross-shards`: ok. 3,002 tests across 4 tiers, and the Windows inventory's 2,651, each in exactly one of 2 shards. Taken on 6c4a488.
- `cargo build --workspace --tests`: clean, on 997aaa8.
- `just engines-current`: passed on 997aaa8, with oneagentgraph 0.5.14, onevcs 0.52.1 and onevcs-testing 0.19.5. onevcs-testing 0.19.9 is held back by `onevcs = "0.52.0"`, which the check names and passes.
- The maintenance journey passed on 6c4a488.
- `cargo fmt --check`, and `cargo clippy --test e2e --test ci_workflows --locked -D warnings`: clean on 6c4a488.

**On c93fa56** (the fence fix, after this document). Hosted run 39118487503 on f2b9566 failed `cross (macos-latest)` on one test. `Mac PR proofs` failed `continuation::a_publication_whose_fence_the_board_refuses_starts_once_a_later_write_lands` with "the fence was written again 166.698583ms after the refusal". It then passed 3 of 3 reruns, and it is the same test as the Windows flake below. The cause is in the product: any driver pass inside the 5 s retry wrote a refused fence again at once. `ClaimWatch::fence` now holds a non-empty fence until its retry is due. The new `continuation::a_pass_inside_a_refused_fences_retry_does_not_write_it_early` wakes the driver inside the retry on purpose. Before the fix it failed for that reason ("written again 124.828284ms after the refusal"); with the fix it passes. Re-run on c93fa56:
- `cargo nextest run --test e2e -E 'test(/fence/) or test(/publication/)'`: 78 of 78 passed.
- Every `continuation` test in the e2e and lib binaries: 58 of 58 passed.
- `cargo clippy --all-targets --locked -- -D warnings`: clean.
- `just lint-llm-diff origin/main`: 39 rules, 23 passed, 0 failed.

The fix adds one e2e test, 1,661 in all.

**On 953fda8** (after the regeneration). The judged lint on 644b02e reported one `changed_behavior_has_e2e` finding: the three profile uploads in `ci.yml` had no structural assertion, and `if-no-files-found: ignore` hid a wrong path. `ci_workflows::every_test_leg_keeps_the_profile_records_from_where_the_harness_writes_them` now holds `gate`, `cross` and `macos` to one `always()` upload of `RECORD_DIR`, with `warn`. It failed as intended when one path was changed to `target/test-profiles/` (the count assertion) and when one `warn` was put back to `ignore`; both were reverted. Re-run on 953fda8:
- `cargo clippy --all-targets` (and `-p onepipeline-testfakes --all-targets`) with `-D warnings`, and `cargo fmt --check`: clean.
- `cargo nextest run -p onepipeline-testfakes`: 2 of 2 passed.
- `binary(ci_workflows)` with the e2e `profiling::` journeys: 31 of 31 passed.

**On 02bd929.** The next judged lint, on 255f974, reported one more `changed_behavior_has_e2e` finding: a record's persisted `programs` (each program's count and time) was checked for shape only. `profiling::a_records_inclusive_and_background_figures_are_the_ones_the_test_measured` now holds them equal to what the test measured in memory, with `git` non-zero. It failed as intended when the record wrote each program's count plus one, at `tests/e2e/profiling.rs:365`; that was reverted. Re-run on 02bd929: clippy over all targets and `cargo fmt --check` are clean, and the 22 e2e `profiling::` journeys pass. Neither this commit nor 953fda8 changes what an e2e test does, so the 7574f2f figures stand.

**On 8ea8908.** The judged lint on 1a26c98 reported `contracts_have_one_source_or_a_drift_gate`: `tests/e2e/project.json` declares the record directory as an Nx output of `test` and `test-quick` without anything tying it to `RECORD_DIR`. `ci_workflows::the_e2e_test_targets_declare_the_profile_records_as_outputs` now does. It failed as intended when `test-quick`'s output was changed to `target/test-profiles/e2e`; that was reverted. Re-run on 8ea8908: `binary(ci_workflows)` 10 of 10 passed, and `cargo clippy --test ci_workflows -D warnings` and `cargo fmt --check` are clean.

**On 7574f2f** (the tree containing c93fa56; the commit after it changes only this document). The full e2e tier ran instrumented as `gate` runs it: `cargo llvm-cov --no-report nextest --locked -E 'binary(e2e)' --final-status-level fail --no-fail-fast`, with the `ONETASKGRAPH_*` dispatch variables unset. That was run `a989b0b5`, at load 4-10: **1,660 of 1,661 passed in 4,175.7 s**. Both fence tests passed: `a_publication_whose_fence_the_board_refuses_starts_once_a_later_write_lands` in 8.2 s and `a_pass_inside_a_refused_fences_retry_does_not_write_it_early` in 9.1 s. The only failure was `host_view`, at `tests/e2e/host_view.rs:799:5` with `"read_bytes":0`, as before. All 1,661 records the figures read are from that run. They were copied aside before the later re-runs below rewrote the profiling journeys' records. The Linux breakdown, the ratios against local Linux, the local replay weighting and the per-module table's local column were regenerated from it.

**On c551671** (head before this document):
- The full e2e tier: 1,659 of 1,660 passed, in 4,412.6 s. The only failure was host_view, above.
- `cargo test --workspace --lib`: 955 passed, including the oneagentgraph floor test in `src/agentgraph.rs`.
- `cargo test --test contract`: 131 passed.
- `cargo test --workspace --all-targets --no-run`: compiled.
- `just engines-current`: passed.
- `just lint-llm-diff origin/main`: 21 passed, 0 failed.

Two of those runs needed this host's dispatch environment adjusted, with no repository change:
- **The lib and e2e runs** unset the `ONETASKGRAPH_*` variables, because the dispatch's `ONETASKGRAPH_SOURCES__HELLOPATIENT__CONFIG__TEAM` leaks into 7 lib tests with or without the lock change.
- **The judged lint** unset `LLMLINT_ONEHARNESS_BIN`, because the dispatch points it at a missing wrapper.

**Hosted, at dd2e5d6** (run 39098714538): every required job passed except `cross (windows-latest)`:
- **Passed:** `gate` (84.5 min), `cross (macos-latest)` (Mac PR proofs 2h12m), `install` and `install-documented` on all three platforms, `llmlint`, `smoke`, `msrv`, `deny`, `release-compat`, `wheel`, `pr-title`.
- **Windows:** its proof hit the 180-minute step limit after 1,108 of 1,413 e2e tests, with no failing test.
- **The same on main:** main's runs 39080139044, 39023278916, 39012247843 and 39008698390 failed the same Windows proof.

### Earlier checks (a94701b, dd2e5d6), as first reported

The head is dd2e5d6. The two commits after a94701b change only `npm/test/crate-split-replay.test.mjs` and `scripts/crate-split/proposed.json`. Only the `onepipeline-npm` project reads those, so the Rust checks below, taken on a94701b, are not affected by them.

- `node --test npm/test/test-profile-report.test.mjs npm/test/crate-split-replay.test.mjs`: **42 of 42 passed on dd2e5d6**. The extended mapping test failed as intended on the uncorrected mapping ("e2e-landing compiles run_end_hooks but does not list tests/e2e/run_end_hooks.rs") before f6a526e corrected it.
- biome `format` and `lint` over `npm scripts`: clean on dd2e5d6.
- `just lint-llm-diff origin/main` on dd2e5d6: **39 rules, 22 passed, 0 failed**. On a94701b: 21 passed, 0 failed. On 3c728a6 it found one gap (persisted `inclusive_s`/`background_*` not asserted), which a94701b closed.
- `just lint`: passed on 3c728a6. On a94701b, `cargo fmt --check` and `cargo clippy --test e2e -- -D warnings` (the only target a94701b changed) both pass.
- `profiling::a_records_inclusive_and_background_figures_are_the_ones_the_test_measured` fails as intended when `background_s` is written from the wrong field (induced, then reverted; it reports `spawn.background_s`).
- CI `gate` on a94701b (run 39063305714): **success**. That is the whole affected deterministic tier, including the coverage floor and all 1658 e2e tests.
- Local full e2e on a94701b on this host: **1659 of 1660 passed**. The one failure, `host_view::the_host_view_answers_a_host_sized_root_steady_cold_and_steady_again`, is a platform limitation of this host. Its cold-read check reads `rchar` from `/proc/thread-self/io`, and this WSL2 kernel (6.18.54.1-microsoft-standard-WSL2, `# CONFIG_TASKSTATS is not set`) has no `/proc/*/io` at all, so every phase records `read_bytes: 0`. No load or cache condition changes that. A retry at load 27-29 ran to completion in 175 s, after an earlier retry at load around 60 timed out at 360 s, and failed only on that assertion, with `"read_bytes":0`. The test passes in CI `gate` on a94701b, and this branch changes only its imports. Drafted as a follow-up: the reader or test should name the missing accounting. **Base comparison:** back to back on this host, the test fails identically at this task's base commit 049ba74 (`merge-base HEAD origin/main`, from a `git archive` copy) and at the head dd2e5d6:
  - at 049ba74, from load 35.7 to 22.7: `FAIL [255.668s]`, panicked at `tests/e2e/host_view.rs:799:5`, `` `cold-fold` read fewer bytes than the journals it folded ``, `"cold-fold":{..."read_bytes":0,"count":96}`;
  - at dd2e5d6, from load 21.9 to 16.3: `FAIL [118.890s]`, the same line, message and `"read_bytes":0,"count":96`.

  The branch's only change to that file is the `Command`/`Child`/`Stdio` import, and it does not touch `src/hostphases.rs`.
- CI on a94701b, failed on flakes that each passed 3 of 3 reruns from the same build:
  - `Mac PR proofs`: `unwatched::a_host_sized_runs_root_is_answered_in_well_under_a_second_whatever_its_journals_hold`, a 1 s `PARKED` window race.
  - `cross proof (windows-latest)`: `continuation::a_publication_whose_fence_the_board_refuses_starts_once_a_later_write_lands` (it also fails without this change, on `nick/843/op-review` run 39043946659) and `continuation::a_publication_held_past_the_whole_takeover_wait_holds_the_takeover_until_it_has_finished`.

  Each is drafted as a follow-up.
