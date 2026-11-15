# Launch inventory measurement checkpoint

Nothing in this spike lands. The compressed files in `evidence/` preserve generated
fold stdout and the current authoring report draft; read one with `gzip -dc FILE`.
The report is rendered from `spike-report` in
`authoring:process-launch-speed-2026-11-14`, document
`process-launch-speed-2026-11-14-spike-launch-inventory-report`.

## Handoff: 2026-11-15 09:34 UTC

Producer: `076a7b6c`; fold: `99e9b97d`. Session draft PR 1591. The clean source
has 38 passing fold-command tests and green targeted deterministic/judged lint
(18 judged passes, zero failures). The report records exact commands and earlier
Rust evidence, exceptions, uncertainty bounds and suppression reasons.

Baseline run **43684231509, attempt 1**: Linux passed 2,155 e2e tests in 37.919
minutes; Windows passed 1,832 in 87.112 minutes. macOS is still active. The report
flags one Linux profile overwritten by the later budgets target. Keep the known
Windows store exception outside the comparable passed set (ruling a2).

OS run **43684237592, attempt 2**: Linux and Windows active, macOS queued. Its
first attempt was canceled under the manager's capacity ruling. Both demo PRs
1592 and 1593 remain open. Do not cancel any other run.

Next: collect completed artifacts/logs, finish OS coverage and macOS/Linux count
reconciliation, then collect baseline attempts 2 and 3. Ruling a10 requires each
repeat to rerun the **whole workflow after it completes**, on the same two demo
PRs. No single-job reruns. Close the demo PRs only after all evidence is read.
The report remains explicitly incomplete; no three-run mean is claimed.

Scratch helpers and raw artifacts are in the prior dispatch's
`ONEPIPELINE_NODE_SCRATCH_DIR`:
`/home/whoop/ai-orchestrator/runs/brief-process-launch-speed-2026-11-14-spikes/scratch/1794352-0`.
`poll-hosted.py` enforces the three-minute interval and API reserve;
`fetch-completed.py RUN ATTEMPT NAME` collects completed jobs from its snapshot.
All GitHub reads use this host's `gh` login, stopping below 2,500 core requests
or 2,000 GraphQL points. No run may write Actions/Nx caches.

The first point's head is `36237191db78133b709b3d893ccc11d91d2a5272`, checkout
merge `37341b0a57e77d18f6db6c11c1b32d7b30fb8dac`, executed workflow blob
`db2bbe081a7b9319eb38bd014e72350b389e8b01`. Collected Linux/Windows/lint logs
report zero Actions-cache saves and zero Nx entries stored. Remaining criteria
and exact source citations are recorded in the compressed report.
