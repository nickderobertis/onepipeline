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

## Handoff — 2026-11-15 21:52 UTC

- Valid after samples: native Linux1/2 and batched Windows1/2; other cells0/2.
- Batched: draft PR1633, run43753064453 attempt1, head
  698a7d0e855c5bb04a362c385bab2508e3e4651e. Linux132931076196 finished ineligible
  (main e2e343/2155, cancellation/adoption failure). Windows132931076438 completed
 1832tests (one main failure,3/3diagnostic passes; point1 undera72);
  macOS132931076404 running; lint cancelled.
- Native: draft PR1634, run43753067613 attempt1, head
  016c5adf2a8760d1a77134c1b1ef655aebd4c70e. Linux132931087404 completed2155/2155,
  37.814 e2e minutes; Windows132931087445 running, macOS132931087312 queued; lint cancelled.
- Both heads are frozen. Executed workflow blobs, checkout SHAs, full selection,
  cache receipts and test outcomes await finished-job logs/artifacts. The expected
  head workflow blob is db2bbe081a7b9319eb38bd014e72350b389e8b01.
- Remaining: two VALID full samples per option/platform under a62/a65; select the combination from
  batched/native means; measure reuse and that combination; verify names/cache
  evidence, price internal/API scopes, quote final fold, close demos and remove
  their worktrees/branches. Further rounds wait for child-waits completion under
  a52. Poll about every15minutes and respect the API reserve.

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
batched each pass10 local unoptimized repetitions; an optimized stock control is
compiling. No replacement hosted run has started.

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
2before/1after points. The corrected isolated optimized-stock cancellation
control passes1/1;20 earlier nested-checkout configuration refusals were not
journey outcomes.

Batched Windows point1 is preserved in evidence/batched-43753064453-1-windows*.
Its main verdict stays red; the withdrawn duplicate-dispatch test passed3/3
job-own reruns. A72’s no-prototype-answer condition is checked from the saved
main trace plus process-local cache/spawn paths, documented in provenance.
Batch mode reuse is off, no batch child/cache seed existed in that invocation.
The duplicate-dispatch follow-up is drafted separately; test bodies stay frozen.
