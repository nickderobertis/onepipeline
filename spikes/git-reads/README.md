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

## Handoff — 2026-11-15 20:24 UTC

- Completed after samples: none.
- Batched: draft PR1633, run43753064453 attempt1, head
  698a7d0e855c5bb04a362c385bab2508e3e4651e. Linux132931076196 and
  Windows132931076438 and macOS132931076404 running; lint cancelled.
- Native: draft PR1634, run43753067613 attempt1, head
  016c5adf2a8760d1a77134c1b1ef655aebd4c70e. Linux132931087404 and
  Windows132931087445 running; macOS132931087312 queued; lint cancelled.
- Both heads are frozen. Executed workflow blobs, checkout SHAs, full selection,
  cache receipts and test outcomes await finished-job logs/artifacts. The expected
  head workflow blob is db2bbe081a7b9319eb38bd014e72350b389e8b01.
- Remaining: three full samples per option/platform; select the combination from
  batched/native means; measure reuse and that combination; verify names/cache
  evidence, price internal/API scopes, quote final fold, close demos and remove
  their worktrees/branches. Further rounds wait for child-waits completion under
  a52. Poll about every15minutes and respect the API reserve.

Local code proof: 62 fold/command tests,17 prototype tests, the six-mode slice's
12 e2e and18 engine-read unit executions pass at the source commits cited in the
report. The final judged pass was red; a47 caps further passes and directs its
remaining demands into report limitations. No green judged result is claimed.
