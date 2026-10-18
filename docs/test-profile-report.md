# onepipeline e2e test profile

This is the measurement step of the onepipeline 30-minute CI chain: profile, then crate split, then test cost, then CI speedup. It reports where each e2e test's time goes on each platform, and what a crate split would let Nx-affected selection skip. Every figure names the commit its evidence was taken on.

The records come from `onepipeline_testfakes::profile` (`crates/testfakes/src/profile.rs`, schema 2, golden `tests/golden/test-profile-record-v2.json`). Every test process writes one, at `target/test-profile/<binary>/<test>.json`. The breakdowns below come from `scripts/test-profile-report.mjs`. The replays come from `scripts/crate-split-replay.mjs` over `scripts/crate-split/proposed.json`.

## Evidence revisions

| record set | where | commit | tests | e2e test-s |
| --- | --- | --- | ---: | ---: |
| local Linux (current) | this dev host (20 cores, WSL2, shared, load 4-10), the full e2e tier instrumented as `gate` runs it, `--no-fail-fast`, run `a989b0b5` | 7574f2f (head) | 1661 | 13874.8 |
| local Linux (previous) | the same host and command, run `80afd02e` | c551671 | 1660 | 14571.5 |
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
- **997aaa8** merges main at 54cfa4a (#982), which splits the Windows leg into two `cross` shards. Each shard now keeps its records as `test-profile-windows-latest-<shard>`, because `upload-artifact` refuses a second artifact of one name. To read a run's Windows records, unpack both into one directory: `gh run download <run> -p 'test-profile-windows-latest-*' -D windows`, then pass `windows=windows`. The script reads a directory recursively, and the shards run disjoint tests. #982 also changes the timing of four e2e journeys, so records from after the merge are not directly comparable with these for those tests.
- **6c4a488** runs one maintenance journey at a three-second pace (see **maintenance on Windows, run 39129002719**). Its test-s move by a few seconds, which the figures below do not include.

The hosted dd2e5d6 records therefore predate the lock refresh and the fence fix. The local runs show neither moved the harness. The c551671 counts match the a94701b run: spawn 13,686 vs 13,575, git 9,048 vs 9,048, build 3,261 vs 3,259, fixture 7,766 vs 7,765. The 7574f2f counts are spawn 13,822, git 9,055, build 3,263 and fixture 7,773. The difference from c551671 is mostly the one added test and run-to-run variation in retries and polls.

The local 7574f2f run is the one every local figure below uses. It ran 1,661 tests in 4,175.7 s: 1,660 passed, and the one failure is `host_view`, the same failure as the earlier runs (see **host_view on this host**).

## Linux breakdown on 7574f2f (local)

`node scripts/test-profile-report.mjs --top-tests 30 --top-modules 10 linux=target/test-profile/e2e`, over the full-tier run of 7574f2f, which contains the fence fix c93fa56.

### linux

Run `a989b0b5-3433-4b3f-9026-72886505d752`: 1661 tests, 13874.8 test-s. Counts: spawn 13822, git 9055, build 3263, fixture 7773, wait 97348, fs 380460, polls 18432.

#### Whole run

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| all | 1661 | 13874.8 | 62.6% | 1.2% | 10.1% | 0.0% | 22.8% | 0.3% | 3.0% |

#### The 30 costliest tests

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::loopcost::an_idle_pass_does_not_grow_with_the_run_it_is_idling_on | 1 | 209.7 | 3.2% | 0.0% | 4.0% | 0.0% | 92.6% | 0.1% | 0.1% |
| e2e::host_view::the_budget_measurement_records_what_each_scaling_read_opened | 1 | 204.7 | 99.9% | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% | 0.1% |
| e2e::adoption::a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs | 1 | 129.5 | 2.2% | 0.2% | 0.5% | 0.0% | 97.0% | 0.1% | 0.1% |
| e2e::listing::a_host_sized_runs_root_lists_in_seconds_and_ten_times_the_journal_bytes_barely_moves_it | 1 | 127.7 | 46.7% | 0.1% | 0.7% | 0.0% | 0.0% | 2.1% | 50.4% |
| e2e::review::a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it | 1 | 119.4 | 93.0% | 6.0% | 0.7% | 0.0% | 0.0% | 0.1% | 0.2% |
| e2e::linear_writeback::a_linear_destination_lands_current_values_after_each_way_a_call_fails | 1 | 104.7 | 4.7% | 0.0% | 0.7% | 0.0% | 94.3% | 0.0% | 0.3% |
| e2e::unwatched::a_host_sized_runs_root_is_answered_in_well_under_a_second_whatever_its_journals_hold | 1 | 97.8 | 43.3% | 0.0% | 0.6% | 0.0% | 0.1% | 3.1% | 52.9% |
| e2e::host_view::the_host_view_answers_a_host_sized_root_steady_cold_and_steady_again | 1 | 97.4 | 99.3% | 0.0% | 0.6% | 0.0% | 0.0% | 0.0% | 0.1% |
| e2e::writeback_budget::a_seven_item_copy_held_at_eleven_seconds_per_item_lands_inside_its_deadline | 1 | 81.8 | 2.0% | 0.0% | 0.8% | 0.0% | 97.0% | 0.0% | 0.1% |
| e2e::linear_writeback::command_end_refusal_and_timeout_rebuild_the_workers_store | 1 | 81.7 | 3.8% | 0.0% | 0.7% | 0.0% | 95.3% | 0.0% | 0.2% |
| e2e::writeback_budget::a_record_an_older_build_wrote_is_adopted_and_its_update_runs_under_the_shipped_default | 1 | 74.9 | 29.0% | 0.0% | 1.3% | 0.0% | 69.2% | 0.1% | 0.3% |
| e2e::out_of_band::a_run_whose_records_cannot_be_read_leaves_no_owner_known_to_be_unique | 1 | 74.8 | 97.4% | 1.1% | 0.9% | 0.0% | 0.0% | 0.2% | 0.4% |
| e2e::continuation::a_publication_held_past_the_whole_takeover_wait_holds_the_takeover_until_it_has_finished | 1 | 73.8 | 3.2% | 0.9% | 0.8% | 0.0% | 95.0% | 0.0% | 0.1% |
| e2e::delivers::a_first_projection_held_past_its_deadline_does_not_hold_back_the_first_dispatch | 1 | 73.7 | 1.1% | 0.0% | 0.8% | 0.0% | 97.9% | 0.2% | 0.1% |
| e2e::writeback_budget::a_targeted_update_held_past_the_floor_still_lands_inside_its_item_budget | 1 | 70.0 | 1.7% | 0.0% | 0.8% | 0.0% | 97.3% | 0.0% | 0.2% |
| e2e::linear_writeback::a_retained_stores_blocked_configuration_read_is_bounded_and_rebuilt | 1 | 69.2 | 2.3% | 0.0% | 0.8% | 0.0% | 96.3% | 0.0% | 0.5% |
| e2e::writeback_budget::a_creating_copy_held_past_a_tiny_budget_is_cancelled_and_the_next_attempt_creates_it | 1 | 69.1 | 1.8% | 0.0% | 1.0% | 0.0% | 97.0% | 0.1% | 0.1% |
| e2e::continuation::a_takeover_of_an_idle_driver_starts_on_its_acknowledgement_within_the_interval | 1 | 67.8 | 6.5% | 1.1% | 0.9% | 0.0% | 3.2% | 0.0% | 88.4% |
| e2e::loopcost::another_runs_ledger_is_read_on_its_own_interval_and_not_on_the_loops | 1 | 66.8 | 6.9% | 0.0% | 0.9% | 0.0% | 92.1% | 0.0% | 0.1% |
| e2e::dispatch::a_legacy_plan_path_launch_record_is_still_reportable_and_adoptable | 1 | 66.6 | 99.0% | 0.0% | 0.9% | 0.0% | 0.0% | 0.0% | 0.1% |
| e2e::writeback_budget::an_update_held_past_a_tiny_budget_is_cancelled_and_the_refusal_names_the_arithmetic | 1 | 66.4 | 32.6% | 0.0% | 0.9% | 0.0% | 66.0% | 0.1% | 0.3% |
| e2e::store::a_run_whose_store_is_rate_limited_throughout_settles_as_it_does_against_a_healthy_one | 1 | 66.2 | 90.9% | 0.0% | 2.7% | 0.0% | 6.0% | 0.1% | 0.3% |
| e2e::writeback_budget::a_destination_read_held_past_the_floor_is_cancelled_retried_and_recovers | 1 | 63.7 | 1.8% | 0.0% | 1.1% | 0.0% | 96.9% | 0.1% | 0.1% |
| e2e::loopcost::a_converged_run_does_no_scheduling_work_while_it_records_nothing | 1 | 63.7 | 1.1% | 0.0% | 1.0% | 0.0% | 97.9% | 0.0% | 0.1% |
| e2e::driver::a_legacy_unqualified_project_does_not_prevent_adoption | 1 | 62.6 | 98.7% | 0.0% | 1.1% | 0.0% | 0.0% | 0.0% | 0.1% |
| e2e::writeback_budget::a_store_that_does_not_open_within_the_floor_is_refused_retried_and_recovers | 1 | 62.2 | 1.2% | 0.0% | 1.1% | 0.0% | 97.2% | 0.2% | 0.3% |
| e2e::dispatch_env_hook::a_hook_that_fails_times_out_cannot_start_or_prints_a_malformed_document_refuses_the_launch | 1 | 61.5 | 98.4% | 0.0% | 1.1% | 0.0% | 0.0% | 0.1% | 0.4% |
| e2e::continuation::a_settlement_dispatch_or_claim_that_is_not_one_refuses_the_launch | 1 | 60.2 | 20.9% | 0.2% | 1.0% | 0.0% | 76.7% | 0.0% | 1.2% |
| e2e::continuation::an_in_flight_dispatch_left_on_the_store_is_continued_by_a_host_that_takes_the_project_over | 1 | 58.5 | 96.4% | 1.1% | 1.0% | 0.0% | 1.2% | 0.1% | 0.2% |
| e2e::continuation::a_takeover_finding_a_fence_it_cannot_read_waits_until_it_is_cleared | 1 | 54.8 | 7.5% | 1.3% | 1.4% | 0.0% | 86.0% | 0.0% | 3.8% |

#### The 10 costliest modules

| | tests | wall s | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| e2e::continuation | 44 | 1216.5 | 37.5% | 1.4% | 2.2% | 0.0% | 48.6% | 0.1% | 10.2% |
| e2e::lifecycle | 102 | 944.8 | 83.4% | 1.4% | 7.4% | 0.0% | 7.0% | 0.2% | 0.5% |
| e2e::readjust | 44 | 942.0 | 91.2% | 1.5% | 4.5% | 0.0% | 2.1% | 0.2% | 0.6% |
| e2e::adoption | 51 | 751.2 | 24.1% | 2.1% | 8.0% | 0.1% | 64.8% | 0.4% | 0.5% |
| e2e::writeback_budget | 12 | 515.6 | 14.1% | 0.0% | 1.6% | 0.0% | 83.9% | 0.1% | 0.2% |
| e2e::store | 46 | 479.8 | 44.0% | 0.1% | 7.5% | 0.0% | 41.9% | 0.3% | 6.2% |
| e2e::rework | 20 | 421.1 | 91.2% | 1.1% | 3.3% | 0.0% | 3.6% | 0.2% | 0.6% |
| e2e::driver | 88 | 409.1 | 75.1% | 0.0% | 15.6% | 0.0% | 7.8% | 0.2% | 1.2% |
| e2e::review | 21 | 389.3 | 88.3% | 3.5% | 3.5% | 0.0% | 3.9% | 0.2% | 0.6% |
| e2e::linear_writeback | 11 | 364.3 | 6.6% | 0.0% | 1.8% | 0.0% | 91.3% | 0.0% | 0.4% |

This host is shared with other dispatches, so the wall figures move with load. The categories' shares and the counts are what to compare across runs.

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

### Every category's ratio against local Linux (7574f2f)

Each figure is the platform's seconds in that category over local-linux's, summed over the tests both ran. The hosted sets are dd2e5d6's, from run 39098714538's `test-profile-*` artifacts.

| platform | tests both ran | wall | spawn | git | build | fixture | wait | fs | unattributed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ci-linux | 1658 | 0.75x | 0.73x | 0.46x | 0.64x | 0.58x | 0.89x | 0.60x | 0.63x |
| macos | 1631 | 0.99x | 1.01x | 1.76x | 0.62x | 0.86x | 1.12x | 0.94x | 0.63x |
| windows | 1110 | 3.77x | 4.13x | 9.29x | 2.16x | 0.90x | 3.32x | 6.02x | 1.78x |

The local host is slower than a CI runner in every category, and least in `wait` (CI Linux 0.89x of local), since a wait's length is set by the test rather than the machine. The CI-Linux baseline above is therefore the fair comparison between platforms.

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

**By the local Linux records of 7574f2f** (run `a989b0b5`, all 1,661 tests, `host_view` included):

144 merged changes, 9 suites: 58 of 1296 suite runs skipped (4.5%).
Test-seconds: 13874.8 per full run; 84776.3 skipped over the window (4.2%).

| suite | changes it runs on | unaffected | test-s per run |
| --- | ---: | ---: | ---: |
| e2e-landing | 139 | 3.5% | 5097.5 |
| e2e-store | 137 | 4.9% | 3543.3 |
| e2e-views | 136 | 5.6% | 1130.3 |
| e2e-dispatch | 137 | 4.9% | 808.3 |
| e2e-channel | 137 | 4.9% | 331.5 |
| e2e-runs | 138 | 4.2% | 1401.5 |
| e2e-watch | 138 | 4.2% | 1102.9 |
| e2e-maintenance | 140 | 2.8% | 309.4 |
| e2e-harness | 136 | 5.6% | 150.1 |

Today's shape, the same records (`--today`): 144 merged changes, 1 suites: 3 of 144 suite runs skipped (2.1%). Test-seconds: 13874.8 per full run; 41624.3 skipped over the window (2.1%).

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
| e2e suite runs Nx skips | **4.5%** (58 of 1,296), against **2.1%** for today's shape; ceiling about 5.6% | replay of the 144 over the mapping at dd2e5d6, the same on CI-Linux dd2e5d6, local 7574f2f and CI-Linux a94701b weights |
| e2e test-seconds skipped | **4.2%**: 63,441 of 1,498,637 by CI-Linux dd2e5d6, 84,776 of 1,997,966 by local 7574f2f, 57,558 of 1,354,608 by CI-Linux a94701b | the same replay |
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

Seconds per module: local Linux 7574f2f, and CI Linux, macOS and Windows from run 39098714538 at dd2e5d6 ("–" means Windows did not reach the module before the step limit). The category shares are of the local 7574f2f seconds. The proposal codes are the ones assigned above.

| module | tests | local Linux s (7574f2f) | CI Linux s | macOS s | Windows s | spawn | wait | build | proposal |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| continuation | 44 | 1216 | 1093 | 1197 | 2530 | 38% | 49% | 2% | A, B |
| lifecycle | 102 | 945 | 745 | 1187 | 3991 | 83% | 7% | 7% | A, D |
| readjust | 44 | 942 | 628 | 1333 | 7695 | 91% | 2% | 4% | A, D |
| adoption | 51 | 751 | 626 | 974 | 4724 | 24% | 65% | 8% | A, B |
| writeback_budget | 12 | 516 | 496 | 501 | – | 14% | 84% | 2% | A, C |
| store | 46 | 480 | 361 | 439 | – | 44% | 42% | 7% | A, B |
| rework | 20 | 421 | 359 | 526 | 3227 | 91% | 4% | 3% | A, D |
| driver | 88 | 409 | 252 | 276 | 273 | 75% | 8% | 16% | A |
| review | 21 | 389 | 304 | 425 | 1996 | 88% | 4% | 4% | A, D |
| linear_writeback | 11 | 364 | 318 | 412 | 252 | 7% | 91% | 2% | A, B |
| loopcost | 10 | 362 | 278 | 274 | 363 | 6% | 90% | 4% | A, C |
| dispatch | 71 | 353 | 252 | 262 | 531 | 75% | 3% | 21% | A |
| host_view | 10 | 349 | 225 | – | – | 89% | 0% | 10% | A, E |
| retirement | 38 | 326 | 245 | 402 | 984 | 67% | 20% | 7% | A, B,D |
| views | 66 | 276 | 188 | 190 | – | 73% | 7% | 18% | A |
| publication_boundary | 35 | 274 | 202 | 390 | 1533 | 71% | 12% | 12% | A, D |
| run_end_hooks | 32 | 238 | 177 | 185 | 296 | 87% | 2% | 10% | A, D |
| unwatched | 27 | 237 | 155 | 111 | 108 | 64% | 3% | 9% | A, E |
| out_of_band | 27 | 223 | 153 | 276 | 1253 | 86% | 1% | 8% | A, D |
| channel | 62 | 218 | 153 | 133 | 271 | 64% | 11% | 22% | A |
| listing | 10 | 215 | 114 | 98 | 228 | 53% | 0% | 15% | A, E |
| watch | 44 | 207 | 150 | 138 | – | 74% | 9% | 14% | A |
| publish_preserve | 30 | 206 | 145 | 239 | 946 | 74% | 10% | 13% | A, D |
| live_edit | 46 | 200 | 111 | 105 | 257 | 53% | 12% | 33% | A |
| scratch | 29 | 176 | 120 | 107 | 170 | 79% | 7% | 12% | A |
| manager_landings | 39 | 175 | 139 | 203 | – | 66% | 12% | 16% | A, D |
| dispatch_env_hook | 10 | 151 | 117 | 208 | 157 | 93% | 2% | 4% | A |
| branch_template | 16 | 146 | 111 | 155 | 609 | 91% | 0% | 7% | A, D |
| stacked_drafts | 12 | 145 | 121 | 183 | 310 | 80% | 2% | 6% | A, D |
| wake_budget | 22 | 143 | 106 | 92 | – | 79% | 7% | 12% | A |
| flow | 22 | 135 | 80 | 84 | – | 56% | 11% | 31% | A |
| maintenance | 23 | 133 | 97 | 127 | 206 | 34% | 45% | 9% | A, B |
| shutdown | 30 | 133 | 83 | 105 | 201 | 65% | 15% | 16% | A, B |
| harness | 30 | 129 | 24 | 16 | 32 | 39% | 1% | 56% | A |
| delivers | 16 | 120 | 104 | 100 | 125 | 15% | 75% | 9% | A, B |
| unpublished | 23 | 115 | 74 | 124 | – | 82% | 0% | 14% | A, D |
| driver_exit | 19 | 114 | 87 | 95 | 43 | 68% | 19% | 11% | A, B |
| draft_lifecycle | 15 | 111 | 79 | 154 | 562 | 89% | 0% | 9% | A, D |
| templates | 24 | 110 | 95 | 70 | – | 83% | 0% | 15% | A |
| criteria | 17 | 101 | 74 | 98 | 447 | 81% | 5% | 10% | A, D |
| summary | 12 | 89 | 68 | 56 | – | 71% | 2% | 11% | A |
| landing | 5 | 89 | 80 | 165 | 923 | 95% | 0% | 3% | A, D |
| malformed_envelopes | 12 | 81 | 61 | 63 | 82 | 86% | 4% | 9% | A |
| change_telemetry | 9 | 71 | 55 | 73 | 312 | 84% | 3% | 11% | A, D |
| session_reuse | 4 | 69 | 45 | 67 | 289 | 75% | 20% | 4% | A, B |
| filter | 14 | 67 | 49 | 53 | 75 | 84% | 0% | 15% | A |
| journal | 16 | 65 | 49 | 50 | 55 | 80% | 3% | 15% | A |
| ask | 10 | 64 | 52 | 50 | 66 | 75% | 13% | 11% | A, B |
| node_validator | 11 | 63 | 47 | 48 | 86 | 86% | 0% | 12% | A |
| plan | 22 | 62 | 40 | 45 | 77 | 52% | 17% | 29% | A |
| envelope_reviewer | 11 | 54 | 36 | 45 | 72 | 81% | 5% | 12% | A |
| writeback_projections | 11 | 53 | 30 | 33 | – | 57% | 22% | 15% | A, B |
| crossdag | 8 | 47 | 36 | 35 | 50 | 87% | 1% | 11% | A |
| unreadable_sessions | 5 | 46 | 33 | 40 | – | 73% | 18% | 8% | A, B |
| writeback_cost | 1 | 44 | 37 | 37 | – | 7% | 90% | 2% | A, C |
| run_ending | 8 | 42 | 31 | 28 | 40 | 83% | 2% | 12% | A |
| agents | 6 | 41 | 26 | 32 | 140 | 71% | 12% | 15% | A |
| stop_guard | 11 | 39 | 31 | 37 | – | 74% | 3% | 20% | A |
| destination | 25 | 39 | 24 | 19 | 33 | 33% | 0% | 55% | A (under 20 s: little to gain) |
| boundary | 10 | 39 | 28 | 27 | 44 | 84% | 0% | 15% | A |
| multi_source | 10 | 36 | 31 | 35 | 186 | 39% | 36% | 15% | A, B |
| checkpoint | 6 | 36 | 26 | 24 | 40 | 72% | 15% | 11% | A, B |
| cancellation | 10 | 35 | 22 | 25 | 42 | 59% | 20% | 19% | A, B |
| bus_config | 12 | 35 | 23 | 22 | 42 | 74% | 1% | 22% | A (under 20 s: little to gain) |
| real_vcs | 4 | 35 | 26 | 50 | 211 | 88% | 0% | 10% | A, D |
| holds | 6 | 34 | 22 | 23 | 32 | 61% | 16% | 22% | A, B |
| criteria_rule | 7 | 32 | 22 | 20 | 42 | 81% | 0% | 17% | A (under 20 s: little to gain) |
| shipped | 15 | 32 | 23 | 23 | 42 | 78% | 0% | 14% | A |
| turns | 6 | 30 | 20 | 19 | – | 81% | 0% | 16% | A (under 20 s: little to gain) |
| plan_check | 14 | 29 | 15 | 15 | 40 | 40% | 0% | 56% | A (under 20 s: little to gain) |
| amend | 8 | 29 | 20 | 21 | 77 | 76% | 3% | 19% | A (under 20 s: little to gain) |
| concurrency | 3 | 22 | 14 | 12 | 66 | 62% | 21% | 13% | A, B |
| profiling | 22 | 21 | 10 | 12 | 22 | 45% | 26% | 22% | A (under 20 s: little to gain) |
| surface | 12 | 15 | 10 | 5 | – | 59% | 0% | 38% | A (under 20 s: little to gain) |
| recorded_channel | 8 | 11 | 9 | 15 | 23 | 65% | 0% | 30% | A (under 20 s: little to gain) |
| compatibility | 7 | 11 | 7 | 6 | 15 | 53% | 9% | 36% | A (under 20 s: little to gain) |
| session | 2 | 6 | 4 | 4 | 7 | 68% | 6% | 24% | A (under 20 s: little to gain) |
| older_launch_record | 1 | 5 | 4 | 3 | 7 | 86% | 0% | 13% | A (under 20 s: little to gain) |
| layout_document | 2 | 3 | 2 | 2 | 9 | 71% | 3% | 23% | A (under 20 s: little to gain) |
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

Five other journeys in `tests/e2e/maintenance.rs` assert an empty journal after a `release` under the same pace. None of them is named here, so they are left as a follow-up.

## Checks

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
