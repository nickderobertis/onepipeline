# Windows journey audit

Reviewed read-only: workflows created from `2026-10-10T00:00:00Z` through
`2026-10-13T04:00:00Z`. Pagination without a status filter returned 618 workflows,
including 105 CI runs. CI alone defines `cross (windows-latest)`. All 111 completed
Windows jobs across all 105 of those CI runs were read, including completed jobs whose
workflow was queued when first read. The final read included newly completed
job 115100516667 on run 38337019687; workflow-validation failure 38165691367 had no jobs. No CI run was rerun,
cancelled, or otherwise changed.

## Runs and job logs read

| Run | Attempt | Windows job |
| --- | --- | --- |
| 38011357984 | 1 | 114091924047 |
| 38016686942 | 1 | 114108454849 |
| 38016847752 | 1 | 114108955051 |
| 38016847752 | 2 | 114126078119 |
| 38025940495 | 1 | 114136719756 |
| 38081023746 | 1 | 114297838240 |
| 38087668033 | 1 | 114317499780 |
| 38087862995 | 1 | 114318115634 |
| 38087862995 | 2 | 114336727685 |
| 38092184145 | 1 | 114330747788 |
| 38096315837 | 1 | 114342903955 |
| 38096315837 | 2 | 114363733249 |
| 38097878217 | 1 | 114347514279 |
| 38100765857 | 1 | 114356092586 |
| 38106072120 | 1 | 114371804807 |
| 38108548121 | 1 | 114379154979 |
| 38116514889 | 1 | 114402395760 |
| 38116706897 | 1 | 114402950003 |
| 38122416484 | 1 | 114419755958 |
| 38125225541 | 1 | 114428123544 |
| 38126478160 | 1 | 114431869546 |
| 38127287963 | 1 | 114434216735 |
| 38129089539 | 1 | 114439528294 |
| 38133949709 | 1 | 114453946887 |
| 38136590948 | 1 | 114461718797 |
| 38136590948 | 2 | 114487275782 |
| 38136590948 | 3 | 114495463898 |
| 38136653966 | 1 | 114461839743 |
| 38136851648 | 1 | 114463078000 |
| 38144885835 | 1 | 114486106237 |
| 38147097350 | 1 | 114492697318 |
| 38147321006 | 1 | 114493355856 |
| 38149892230 | 1 | 114501071921 |
| 38153801146 | 1 | 114512535386 |
| 38156428129 | 1 | 114520306948 |
| 38158721536 | 1 | 114527009376 |
| 38158979337 | 1 | 114527777404 |
| 38166299782 | 1 | 114549521992 |
| 38167328829 | 1 | 114552555340 |
| 38167817305 | 1 | 114554050628 |
| 38175346414 | 1 | 114576518989 |
| 38176497235 | 1 | 114579946797 |
| 38181853085 | 1 | 114595950036 |
| 38182268087 | 1 | 114597195903 |
| 38182806877 | 1 | 114598864432 |
| 38186353137 | 1 | 114609457395 |
| 38190680916 | 1 | 114622637126 |
| 38192621109 | 1 | 114628487624 |
| 38197747073 | 1 | 114644180006 |
| 38199012433 | 1 | 114648057110 |
| 38201438249 | 1 | 114655793490 |
| 38202443221 | 1 | 114660926152 |
| 38203875151 | 1 | 114663244272 |
| 38206384117 | 1 | 114670737050 |
| 38210421297 | 1 | 114683173791 |
| 38212384118 | 1 | 114689304672 |
| 38214760468 | 1 | 114696866082 |
| 38215040936 | 1 | 114698625357 |
| 38217116356 | 1 | 114704432062 |
| 38217703351 | 1 | 114706292079 |
| 38219500301 | 1 | 114712187479 |
| 38224021933 | 1 | 114727156161 |
| 38226081742 | 1 | 114734196087 |
| 38228768291 | 1 | 114742839461 |
| 38228903160 | 1 | 114743284328 |
| 38231984505 | 1 | 114753979996 |
| 38231984505 | 2 | 114824488321 |
| 38239153709 | 1 | 114777259256 |
| 38242468819 | 1 | 114788306222 |
| 38246031787 | 1 | 114800074561 |
| 38247783739 | 1 | 114806053956 |
| 38251292706 | 1 | 114818678035 |
| 38253426149 | 1 | 114825617237 |
| 38260253570 | 1 | 114848740424 |
| 38264010829 | 1 | 114862059336 |
| 38265208049 | 1 | 114865647852 |
| 38265421335 | 1 | 114866355012 |
| 38265846562 | 1 | 114868168884 |
| 38267125108 | 1 | 114875668766 |
| 38275640288 | 1 | 114900417651 |
| 38278799556 | 1 | 114910819659 |
| 38279648563 | 1 | 114915100249 |
| 38280286500 | 1 | 114918659364 |
| 38282595304 | 1 | 114923337100 |
| 38287063466 | 1 | 114937944678 |
| 38287372154 | 1 | 114939335142 |
| 38289476635 | 1 | 114945895904 |
| 38289756843 | 1 | 114947341561 |
| 38297243862 | 1 | 114971278174 |
| 38300466326 | 1 | 114981967230 |
| 38303452356 | 1 | 114991791273 |
| 38303685248 | 1 | 114992561689 |
| 38305957354 | 1 | 115000495915 |
| 38308337348 | 1 | 115007784920 |
| 38308680023 | 1 | 115008903698 |
| 38315362238 | 1 | 115030661535 |
| 38318278032 | 1 | 115040127680 |
| 38318659877 | 1 | 115041326280 |
| 38318998852 | 1 | 115042420292 |
| 38319284110 | 1 | 115043884096 |
| 38321559900 | 1 | 115050675680 |
| 38321636205 | 1 | 115050900938 |
| 38325582068 | 1 | 115063434421 |
| 38325780948 | 1 | 115064286522 |
| 38327384364 | 1 | 115069469515 |
| 38330099669 | 1 | 115077746868 |
| 38331892613 | 1 | 115083327613 |
| 38332157439 | 1 | 115084246381 |
| 38336811917 | 1 | 115098701281 |
| 38337019687 | 1 | 115100516667 |
| 38340485051 | 1 | 115110178420 |

## Failures that passed unchanged

Every entry below passed in at least one `rerun-failed` repetition. Watch also
passed in a later attempt of run 38096315837 on the same commit.

| Journey | Failing runs / jobs |
| --- | --- |
| `a_binding_note_enters_the_bar_its_judge_decides_against_as_the_workers_own` | 38176497235 / 114579946797 |
| `a_note_reaching_the_live_judge_re_takes_its_decision_and_rides_it_to_the_worker` | 38217703351 / 114706292079 |
| `dispatch_env_hook::a_hook_that_fails_times_out_cannot_start_or_prints_a_malformed_document_refuses_the_launch` | 38303452356 / 114991791273 |
| `draft_lifecycle::a_draft_whose_head_has_a_check_suite_in_progress_is_held_until_it_is_green` | 38280286500 / 114918659364 |
| `linear_writeback::a_github_source_projecting_a_follow_ups_host_is_read_and_settled_without_writing_host` | 38325582068 / 115063434421; 38318278032 / 115040127680; 38228768291 / 114742839461 |
| `loopcost::every_answer_the_loop_owes_arrives_inside_a_second` | 38264010829 / 114862059336 |
| `malformed_envelopes::an_undecodable_envelope_arriving_while_the_driver_is_leaving_is_answered_before_it_lets_go` | 38330099669 / 115077746868; 38318659877 / 115041326280 |
| `rework::a_title_nothing_verified_is_left_unverified_until_a_retitle_sets_one` | 38210421297 / 114683173791; 38190680916 / 114622637126 |
| `store::a_cache_enabled_github_store_reads_and_writes_like_an_uncached_store` | 38331892613 / 115083327613 |
| `unwatched::the_fixture_is_assembled_beside_a_template_still_replacing_its_documents` | 38246031787 / 114800074561 |
| `watch::every_surface_the_channel_raises_ends_a_default_watch_and_is_counted_while_it_waits` | 38289476635 / 114945895904; 38096315837 / 114342903955 |

## Causes and disposition

- Live-note journeys: `tests/note/main.rs::release_when_the_note_is_queued` releases after a two-second margin; `release_once_the_note_is_taken` adds 500 ms after the offer rename. Neither observes the conversation inbox. The courier/relay/inbox handoff belongs to onemessagebus, oneagentgraph and onejudge. The manager ruled to leave these journeys unchanged and draft the missing inbox acknowledgment against oneagentgraph. Follow-up: `drafts/20261013T041215Z-acknowledge-live-notes-after-conversation-inbox`.
- Rework: job 114683173791 initially timed out at the unchanged 360-second test backstop and then passed three diagnostic repetitions; that log contains no internal failure state identifying a separate cause. Job 114622637126 does identify the missing drafter body. PID reuse made the drafter share its worker’s stream identity, so its answer could be discarded. Already fixed by `4c23608` (#865); `tests/e2e/rework.rs::a_bodyless_drafter_that_reuses_its_workers_pid_is_read_as_itself` and `tests/e2e/rework.rs::a_drafter_that_reuses_its_workers_pid_is_read_as_itself` force that condition.
- Draft suite: a timer could expire between the check-rollup and activity reads, mixing absent required checks with completed activity and lifting early. Already fixed by `7681b6e` (#875): `crates/testfakes/src/bin/fake-gh.rs::reported_draft_running` retains one rollup’s phase; `tests/e2e/draft_lifecycle.rs::a_drafts_activity_finishes_only_after_its_next_check_read` forces expiry between those reads.
- Malformed envelope: `tests/e2e/driver.rs::unwritable_shadow_store` removed a tree while the write-back worker could still hold it open. Windows refuses such deletion, and a successful deletion still only bought a timed close-out window. The helper now waits out transient sharing/deletion refusals. `tests/e2e/malformed_envelopes.rs::leaving_driver_answers` holds the actual final command claim with `src/engine.rs::close_out_and_drain`'s private ordering gate, queues the real bus envelope, then releases that claim. Its Windows regression holds a directory reader without `FILE_SHARE_DELETE` on Windows and imposes the same refusal through the removal seam on Linux. Both journeys retain the exact refusal, cursor, event count and no-adoption assertions.
- Watch: `tests/e2e/watch.rs::every_surface_the_channel_raises_ends_a_default_watch_and_is_counted_while_it_waits` launched three CLI processes inside a five-second watch. Windows process scheduling could expire the watch before all three surfaces were queued. The watch now has a private deadline gate after arming: the journey queues all three surfaces, then advances the clock to the same deadline. `src/watch.rs::watch`'s normal clock, five-second argument, exact count and prose assertion are unchanged.
- GitHub follow-up projection: `tests/e2e/github_loopback.py::save` renamed `state.next` over `state.json` while `tests/e2e/linear_writeback.rs::GitHubEndpoint::state` could be reading it. The Windows log reports `WinError 5`. The fixture now serves snapshots through its own `/state` HTTP route and holds no shared snapshot file. `github_snapshots_and_mutations_do_not_replace_an_open_state_file` imposes the documented replacement refusal, sends actual HTTP mutations and checks both snapshots and an unknown-route refusal. The journey also waited on any earlier projected record and slept 1.5 seconds before counting requests; it now waits for a newly completed record naming the settlement's item, using `projection_completed_after`.
- Fixture assembly: `tests/e2e/unwatched.rs::assembled_run` opened the summary while the writer in `the_fixture_is_assembled_beside_a_template_still_replacing_its_documents` renamed over it. Either the open or replacement could be refused on Windows. Both filesystem operations now wait out transient sharing refusals through `after_document_handles_close`, under the existing harness deadline. `the_fixture_waits_for_a_windows_reader_to_release_a_document` forces one refusal with a held reader, closes it, then performs the real rename and fixture assembly; an unrelated permanent error still fails immediately.
- Loop latency: the historical `every_answer_the_loop_owes_arrives_inside_a_second` journey, now named `tests/e2e/loopcost.rs::ready_settlements_wait_on_no_timer_and_other_loop_answers_arrive_inside_a_second`, subtracted a producer's journal stamp from the loop's stamp. Scheduling could add 1,199 ms even though the loop slept on no timer waiting for the ready settlement. The manager authorized replacing only this failed absolute assertion. The journey now holds receipt until the actual settlement message is queued, then proves zero expired poll waits through the private ordering/count seam in `src/ordering.rs` and `src/engine.rs`. A stepped producer clock forces the old latency assertion to fail without a delay. `budgets.yaml::settlement-readability-latency` retains the 1,000 ms threshold, measured by an unheld subprocess journey's own journal stamps through `scripts/settlement-latency-budget.sh`. The other three latency assertions remain unchanged.
- Hook timeout: the one-second timeout could kill Windows `cmd.exe` before its batch reached the invocation append. `tests/e2e/dispatch_env_hook.rs` compared two records with three attempts even though timing out startup is a valid refusal. The timeout-case invocation count is retained, with no exemption. The compiled hook records and syncs each actual invocation, closes the record, then announces its held step. A peer holds every retry's first timeout evaluation until it observes that step, advances the unchanged one-second timeout, and witnesses process termination through socket closure. The journey compares all invocation records with all dispatch attempts. The earlier report claimed a separate manager ruling authorized dropping this count; no retained evidence substantiates that claim, and this correction supersedes it. `a_compiled_hook_held_before_or_after_output_is_terminated_without_launching_a_worker` runs `crates/testfakes/src/bin/fake-dispatch-env-hook.rs` held before its first record and after valid output, retains the one-second deadline, holds its first timeout evaluation until the child reports its held step through `src/dispatchenv.rs`'s private clock gate, proves termination through the socket's closure, asserts the refusal, and proves no worker launched.
- Cached store: `tests/e2e/store.rs::a_cache_enabled_github_store_reads_and_writes_like_an_uncached_store` accepted a previous projected record while the new projection was still in flight. The failing tuple had matching final state and all three settlement query sequences, but 30 versus 29 total requests. It now waits for the new record and holds the worker at its completed/idle boundary, through `src/writeback.rs`'s private gate, so each mutation and close-out occurs in the same state in both cache modes. Replies are released from that gate when their envelope is actually durable. Total-count equality is unchanged. `linear_writeback::a_previous_projection_does_not_answer_for_a_write_held_in_the_source` holds a real source-protocol write and proves the earlier record cannot answer it.

The sharing seams follow [Microsoft's CreateFileW documentation](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew): omitting `FILE_SHARE_DELETE` prevents delete/rename access until the handle closes. The termination journey follows [TerminateProcess](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess), observing closure rather than assigning a Unix killed-exit code to Windows. Native Windows verification remains on the merge path.

## Regression evidence

With the ordering/OS seams retained, restoring the corresponding base behaviors made seven forced journeys fail in one counterfactual run: one-shot shadow removal (`PermissionDenied`); proceeding before the close-out gate (driver already exited); one-shot summary replacement (`PermissionDenied`); file-based GitHub snapshots (replacement refused before listening); the stale projected-record predicate (true while the new source write was held); the historical timeout-case invocation count under a forced before-first-record termination (0 versus 3); and the absolute settlement bound (the stepped producer stamp). The timeout count is now retained rather than exempted; the forced startup condition is covered separately by the before-first-record termination journey.

The timeout-count correction also has a deterministic counterfactual: with the finished compiled hook and journey retained, restoring the prior `src/dispatchenv.rs` made the real timeout journey fail at 0 acknowledged timeout evaluations versus 3 dispatch attempts. Restoring the every-attempt ordering gate makes each actual invocation durable before advancing its unchanged timeout. This evidence supersedes the earlier disposition that exempted the timeout count; the separate before-first-record termination regression remains unchanged.

Advancing the watch's deadline before publishing surfaces separately reproduced its old scheduling condition deterministically: the unchanged count assertion failed at 0 versus 3. Removing a redundant extra watch read alone did not fail that journey: the summary already reads the queue directly. That read was removed from the finished implementation; the fix is the journey's ordering, not a product summary bug.

The counter also has a positive integration control, `loopcost::an_actual_expired_poll_is_counted_before_a_held_settlement_becomes_readable`: it holds the real settlement before enqueueing, observes one actual expired receiver poll, then records exactly one wait. Removing the increment made this control fail at 0 versus 1; restoring it made both the positive and zero-wait controls pass. All 59 engine/ordering unit tests passed after that change, as did clippy.

All counterfactual edits were restored. The base-fixed PID and draft-phase causes use the existing regressions and commits named above, as the manager ruled. No timeout, coverage threshold, lint rule, CI job or public schema changed. The timeout-case count is retained. Only the failed settlement latency assertion changed under the supplied manager amendment; the original latency bound is retained as a budget.

## Validation

The earlier Linux validation passed 248 file-scoped e2e journeys across driver, malformed envelopes, watch, dispatch-env hook, Linear/GitHub write-back, loop costs, store and fixture assembly, including the base-fixed PID/draft regressions. The additional correct-address validation and full-body reused-PID drafter check passed (2/2). After publishing the private timer count atomically and strengthening the real-rename witness, the directly affected forced checks passed again (3/3).

Before the timeout-count correction, all 945 unit and budget tests passed, including the new budget-script refusal/success test. The initial broad unit run found an unrelated partial source configuration inherited from this dispatch (`sources.hellopatient: missing field plugin`); the successful check removed only inherited `ONETASKGRAPH_SOURCES__` settings from its subprocess environment, without changing the test or the user's environment. That isolation issue is drafted separately as `drafts/20261013T050156Z-isolate-configuration-self-tests-from-partial-am`.

The existing Nx workspace tests passed without suppressions or skips. `nx.json` now includes the loopcost journey the budget registration test reads, and the budget target includes the new measurement script in its inputs; those additions broaden invalidation to keep cached checks honest and change no enforcement rule.

The final latency budget measured 44 ms against 1,000 ms. Formatting and clippy passed. The sharing regressions passed with actual existing files held without delete sharing; restoring the base file-based snapshot writer again failed at the imposed replacement refusal. The hook's clock-controlled termination and disconnected-peer cleanup cases passed, and all eight dispatch-env/ordering unit tests passed. The finished-tree Nx rerun passed all 87 tests.

The first completed llmlint verdict found missing script-path proof/recovery guidance, the converted journey's stale name, and the required private seams. The script now has missing-output, producer-failure and successful-output subprocess checks; its error names a recovery action; and the journey is renamed to state its actual property. Only the exact task-required OS/ordering seam statements carry site-specific `tests_mirror_real_usage` explanations. No shared rule or enforcement configuration was changed. The completed diff-scoped verdict passed all 19 applicable rules, with 0 failures, 15 skipped and 5 not relevant (39 total); its full report is `llmlint history 20261013T064945Z-1de47`. The first llmlint attempt returned no verdict because its harness exceeded the existing 600-second deadline; its configuration was not changed. Native Windows runs only on the merge path.

The timeout-count correction was then verified on its restored implementation: all 10 dispatch-env hook journeys and all 8 dispatch-env/ordering unit tests passed. The prior-driver counterfactual failed at the intended 0-versus-3 ordering assertion; its edits were restored before the successful suite run. Evidence is retained under this node's scratch directory as `hook-every-attempt-negative.log`, `hook-restored-final.log`, `hook-restored-units.log`, `hook-restored-clippy.log`, `hook-restored-fmt.log`, and `hook-restored-llmlint.log`. The earlier verdict above predates this correction; the last file records its fresh diff-scoped verdict.

## Preserved-branch publication repair

The continuation read run 38362686175's Windows job 115180481951 read-only.
This later run is outside the reviewed date window above. Its e2e step passed
all 1,275 journeys, without skips. Publication failed instead at
`tests/budgets.rs::the_settlement_latency_budget_uses_the_journeys_journal_stamps`:
the script subprocess exited 1 instead of 2, including all three diagnostic
reruns. The test passed Rust's native Windows absolute script path to Bash.
Bash interprets that argument as a shell path rather than converting the native
drive/backslash spelling; the script therefore never reaches its missing-result
refusal. The finished test sets the repository working directory explicitly and
passes the same relative script path the registered budget uses. It still
requires exit 2 and the exact missing-result diagnostic, and now includes stderr
in an exit-code failure. No assertion is removed or platform-gated.

The prior audit evidence has been copied from `/tmp/onepipeline-windows-audit`
into this node's scratch directory under `prior-audit/`. The later job log is
`windows-115180481951.log`. `native-path-negative.log` demonstrates Bash refusing
the native drive/backslash argument on Linux; it is a path interpretation
control, not a claim of native Windows verification.

The repaired file's full test binary passed on Linux: `cargo test --test budgets`,
13 passed, zero failures or ignored tests (`budgets-repaired.log`). Formatting
also passed. These changes touch only the budget test and this report; the
e2e implementation that the later Windows job exercised is unchanged.

## Other failures without unchanged passing evidence

The following failures were read but do not qualify: pooled-identity lifecycle
(38144885835, 38153801146); cancelled merge-path lifecycle (38136653966); adoption
and unpublished SDK assertions (38127287963); retry-replacement note (38092184145);
malformed/unwritable unpublished memory (38239153709); rerun-step configuration
(38318278032). No same-commit passing run/attempt or passing diagnostic repetition
was found for these in the reviewed set. Later changed commits are not unchanged
reruns.
