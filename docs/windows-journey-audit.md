# Windows journey audit

Reviewed read-only: workflows created from `2026-10-10T00:00:00Z` through
`2026-10-13T04:00:00Z`. Pagination without a status filter returned 618 workflows,
including 105 CI runs. CI alone defines `cross (windows-latest)`. All 110 completed
Windows jobs across 104 of those CI runs were read, including completed jobs whose
workflow was queued when first read. Run 38337019687 had no completed Windows job;
workflow-validation failure 38165691367 had no jobs. No CI run was rerun,
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
- Rework: PID reuse made the drafter share its worker’s stream identity, so its answer could be discarded. Already fixed by `4c23608` (#865); `tests/e2e/rework.rs::a_bodyless_drafter_that_reuses_its_workers_pid_is_read_as_itself` and the adjacent reused-PID drafter journey force that condition.
- Draft suite: a timer could expire between the check-rollup and activity reads, mixing absent required checks with completed activity and lifting early. Already fixed by `7681b6e` (#875): `crates/testfakes/src/bin/fake-gh.rs::reported_draft_running` retains one rollup’s phase; `tests/e2e/draft_lifecycle.rs::a_drafts_activity_finishes_only_after_its_next_check_read` forces expiry between those reads.
- Other causes and finished-tree validation are recorded below as their fixes land.

## Failures without unchanged passing evidence

The following failures were read but do not qualify: pooled-identity lifecycle
(38144885835, 38153801146); cancelled merge-path lifecycle (38136653966); adoption
and unpublished SDK assertions (38127287963); retry-replacement note (38092184145);
malformed/unwritable unpublished memory (38239153709); rerun-step configuration
(38318278032). No same-commit passing run/attempt or passing diagnostic repetition
was found for these in the reviewed set. Later changed commits are not unchanged
reruns.
