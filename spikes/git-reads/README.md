# Git-read measurement record

Nothing on this branch lands. The report is
`authoring:process-launch-speed-2026-11-14-spike-git-reads-report`, rendered
from `spike-report` in `authoring:process-launch-speed-2026-11-14`.

Fold hosted artifacts (each side contains run-attempt/test-platform directories
with test-profile/, timestamped job.log and meta.json):

    node scripts/git-reads-compare.mjs compare baseline=DIR batched=DIR native=DIR reuse=DIR combination=DIR

The output contains run/job identities, processors, timing, selection, cache
receipts, named outcomes, mean differences, spread decisions, PR scaling and Git
launch attempts/recorded durations by verb, shape and engine-owned site.
Failed spawn calls are costed separately; missing exit durations remain unknown. Use `counts` instead of
`compare` for local verification; shared-host durations are never priced.

Preserved baseline facts and named outcomes are in evidence/*.gz; use gzip -dc.
Their provenance is in evidence/baseline-provenance.json. The before side is
43684231509 attempts1/2, accepted under c11.0. Later archive-only commits leave
the executable check evidence at its cited source commit unchanged.

## Handoff — 2026-11-16 02:56 UTC

- Valid after points are batched Linux/macOS/Windows1of2 and nativeLinux/Windows1of2;
  other cells0of2. Strict main failures and positive diagnostic reruns stay separate.
- Batched PR1633 run43753064453 attempt2: Linux132996793865 validpoint1, Windows132996793856
  HELD undera84, macOS132996793757 running; head698a7d0e855c5bb04a362c385bab2508e3e4651e.
- Native PR1634 run43753067613 attempt1: macOS132931087312 queued; other jobs read;
  head016c5adf2a8760d1a77134c1b1ef655aebd4c70e.
- Reuse PR1653 run43789062933 attempt1: Linux133043119134/Windows133043119352
  running, macOS133043119250 queued, head1c95ed5b6de89074c982d810662b5577fc67621a.
  Opened after fresh six-mode local trace/fold passed, with69workflow/cache tests green.
- A79 raises own cap to3; plan cap7 remains. Opening followed the01:54 cap6read.
  All three heads frozen. Expected workflowdb2bbe081a7b9319eb38bd014e72350b389e8b01;
  actual receipts await finished jobs. Original incomplete batched Linux remains quarantined.
- Remaining: two valid points/platform/option, choose reader from measured means,
  measure reuse/combination, final fold/pricing/proof, close/remove demos. No final saving.
  Read about15minutes apart with core2500/GraphQL2000 reserve. No cache writes/deletions.

Local code proof: 63 fold/command tests,17 prototype tests, the six-mode slice's
12 e2e and18 engine-read unit executions pass at the source commits cited in the
report. The final judged pass was red; a47 caps further passes and directs its
remaining demands into report limitations. No green judged result is claimed.

Ruling a62: named parity covers every main e2e-tier test. Report the separate
version summary and other selected Rust tiers’ counts with the unchanged Nx
selection; disclose every other job failure. No broader name inference is built.

Linux run43753064453/1 is quarantined, not a timing point. Evidence and the
failure journal are in evidence/rejected-43753064453-1-linux*. Reader33edc69c
refuses the partial invocation instead of pricing a smaller unit tier. Ruling
a65 says aborted attempts do not consume the two-valid-point quota. Investigation
found no Git starts in the failed journey and an original driver consuming requeue
before exit. Ruling a68 freezes the test, classifies a pre-existing race, and
authorizes a same-head whole-workflow replacement after current jobs finish. Stock and
batched each pass10 local unoptimized repetitions; the isolated stock control passed (its earlier CLI-only optimized label is discarded). Attempt2 subsequently started as the replacement.

Ruling a69 fixes sample selection before timings: per platform, use the first two
valid jobs in run order. Later valid jobs are supplementary and excluded from the
mean. Whole-workflow reruns may produce such extra platform jobs while Linux
still needs its two valid points; every attempt remains listed.

The deferred test repair is drafted as
20261115T220221Z-verify-cancellation-journey-waits-for-driver-exi.

Native Linux point1 is preserved in evidence/native-43753067613-1-linux*.
It has2154 individually retained passes plus one loopcost pass inferred under
a70 from full2155-name catalog/selection/summary; strict fold stays2154. The
canonical comparison command ran at40f3487d; its Linux row is undecided with
2before/1after points. The isolated stock cancellation
control passes1/1; its original CLI-only optimized label is discarded;20 earlier nested-checkout configuration refusals were not
journey outcomes.

Batched Windows point1 is preserved in evidence/batched-43753064453-1-windows*.
Its main verdict stays red; the withdrawn duplicate-dispatch test passed3/3
job-own reruns. A72’s no-prototype-answer condition is checked from the saved
main trace plus process-local cache/spawn paths, documented in provenance.
Batch mode reuse is off, no batch child/cache seed existed in that invocation.
The duplicate-dispatch follow-up is drafted separately; test bodies stay frozen.

Reuse-only demonstration was prepared at1c95ed5b and is now open asPR1653. Its
workflow/cache tests pass69of69. A temporary dependency link accidentally entered
655e06fc and was removed in1c95ed5b before publication; final diff contains no link.
First-point named set comparison is in evidence/round1-name-parity.json:
no extra or unexplained missing main names beyond the explicitly inferred
Linux loopcost and three positively rerun failures.

02:23UTC delta: batched attempt2Linux132996793865 is valid2155of2155,41.114383e2e
minutes (all2155names retained). Windows132996793856 completed1832main with2failures
and3of3own-rerun passes each, but is HELD undera83 because the prototype served the
failed held-adoption journey. It is not in the pricing directory pending bounded
real-Git answer audit and10stock/10batched correctness repetitions. Human-items
duplicate dispatch has no prototype answer; shipping signal now3after versus0baseline.
Batched attempt2Macstarted; original nativeMacstillqueued. ReuseLinux/Windowsrunning,
Macqueued. Completed reuse lint reports inherited vendor/onejudge workspace/lock
finding and required-title failure; cache receipts/log preserved, no cache writes.

A84: Windowsattempt2 stays quarantined after10of10stock/10of10batched passes and
297of297same-ref Git comparisons,0mismatch. Exact hosted historical replay is
unavailable; this does not clear the failure. Audit negative/positive calibration
and receipts are preserved under evidence/held-adoption*. Continue unchanged
prototype; the needed Linuxpoint2whole-workflow replacement can supply its next
validWindows point once currentMacfinishes. Both active-path failure and3duplicate
dispatches versus0baseline are shipping risks; investigation effort is unpriced.

ReuseLinux43789062933/1 job133043119134 is validpoint1: full2155/2155, allnames
retained,41.717517e2e minutes,378387Gitattempts versus377822baseline mean. Only2
config-path probes occurred. A91 keeps this fixed never-landing option unchanged
for its two-point measurement: report adverse/undecided result, do not retune/profile
a new cohort. Snapshot/layout admission overhead is suspected; cheaper admission
for nonmemoizable queries is an untested design step. Original nativeMac is now
running; a90 withdraws cancellation, none performed.

05:40UTC delta: batchedMacattempt2 job132996793757 valid2121/2121,86.517200e2e
minutes, soMacnow2of2. SubsequentbatchedMacvalidjobsare supplementary, excluded
frommeans. Whole-workflowattempt3 started on unchanged698a7d0e after fresh
e0f5e5b8six-mode12e2e/18unit trace/fold. It suppliesLinuxpoint2 and the next
eligibleWindows replacement undera84; no demoheadpush. Native/reusecurrentjobs
continue. Actualattempt3jobs/checkout/blob/cache receipts await finished logs.

A93/a94amendment: REUSEisREJECTED; no run2/no furtherinvestigation. ItsWindows
check_visibilityworkloadfailedmainandbothretainedreruns, despitebaselinepasses;
stalememo versuslayoutcostunknown. Human-itemsduplicateisfourthafterobservation
versus0baseline. COMBINATIONisNOTMEASURED/rejectedbecauseitincludesunsafeReuse;
runno combinationCI. Originalreuse/combinationtwo-valid-pointquotasareintentionally
unmetunderthisruling. Finishafterreadinginflightbatched3/native2andclosingdemos.

Reuse PR1653 closed; remote/local branch and worktree removed after all data was
read. Native macOS point1 passed2121/2121,80.238450e2e minutes. Native attempt2
started unchanged after fresh35657b66six-mode12e2e/18unit proof. Read batched3
and native2, close remaining demos, then finish undera94; no reuse/combination CI.


### Final reader lint receipts (2026-11-16 07:00 UTC)

Batched run 43753064453 attempt 3 lint job 133114993842 reports 25 judged rules passed, one failed, eight skipped and six not relevant (review 20261116T060721Z-187da). The finding is comments_earn_their_place at demonstration-only src/lib.rs:191: the no-behaviour source edit required to select the full Nx tier. Native run 43753067613 attempt 2 lint job 133127607591 reports 26 passed, zero failed, eight skipped and six not relevant. Both lint jobs still fail their required Conventional Commit title check because their authorized titles begin DO NOT MERGE, and their advisory engines-current check also fails. These are hosted results on the unchanged demonstration heads, not a green final local judged-lint claim. Ruling a47 caps further local judged refinement; in-flight heads remain untouched. Both logs and parsed restore receipts are checked in as evidence/{batched-attempt3,native-attempt2}-lint*. Their cache-save arrays are empty and Nx reports zero stored entries, zero stored bytes and zero replayed entries.

Handoff: batched attempt 3 and native attempt 2 are the only in-flight runs. Reuse PR 1653 is closed and the combination is not measured under a94. Finish by reading those two runs and closing PRs 1633 and 1634.

### Batched Linux second point, 2026-11-16 08:13 UTC

Run 43753064453 attempt 3, job 133114993964, uses unchanged head 698a7d0e855c5bb04a362c385bab2508e3e4651e, checkout 3a841969676267c2e6cac47a0a0de12f7f77dbbc and workflow db2bbe081a7b9319eb38bd014e72350b389e8b01. CPU is Intel Xeon 6975P-C, four logical CPUs. Job time is 136.683333 minutes; pre-e2e work is 97.258667 minutes, including compilation and other selected tasks, not a pure compile measurement. The main e2e summary at job.log:9849 is 2155/2155 passed in 2247.075 seconds, or 37.451250 minutes. No main failure or diagnostic recovery is needed. Strict named proof retains 2154 names; complete baseline named catalog coverage and the all-passed summary infer the overwritten loopcost name under a70. The later budget rerun passes separately at log:9859 and is not substituted into the main strict count. The separate version journey also passes 1/1.

The trace records 338745 Git launch attempts, 2510.589256 seconds of recorded durations and 308 unknown durations. Batch-child residency includes idle time; these seconds are not wall-time savings. The 37-task Nx selection ran; all Actions-cache save receipts are empty and Nx stored zero entries/bytes. The unchanged patch audit still fails at log:10027/10059/10091 and prevents coverage merge. Other selected tier counts, including the partial root audit tier, remain in the archived summary excerpt.

Sources: `evidence/batched-43753064453-3-linux{.json.gz,-summaries.txt.gz,-provenance.json,-parity.json}` and hosted job https://github.com/nickderobertis/onepipeline/actions/runs/43753064453/job/133114993964. Reader source 33edc69c; observation processed on 3430d1dd. This is Linux's second valid chronological point. Remaining jobs: batched macOS (supplementary only) and Windows, plus all three native attempt-2 platforms. No head was pushed, and no new workflow was started.

Canonical reader-only fold executed on 3430d1dd, exit 0, empty stderr:
`| linux | 2/2 | 39.487 | 39.283 | 0.205 | undecided (inside a side's spread) | 0.18 | 377822 | 340039 | 1372.8 (+81.0 unknown) | 2982.6 (+304.5 unknown) |`
This is the batched comparison, not a decided saving. Full output is
`evidence/readers-after-batched-linux2.md.gz`; reuse is excluded under a94.
