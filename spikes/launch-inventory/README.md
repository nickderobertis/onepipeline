# Launch inventory measurement checkpoint

Nothing in this spike lands. The compressed files in `evidence/` preserve generated
fold stdout and the current authoring report draft; read one with `gzip -dc FILE`.
The report is rendered from `spike-report` in
`authoring:process-launch-speed-2026-11-14`, document
`process-launch-speed-2026-11-14-spike-launch-inventory-report`.

## Handoff: after baseline point 1, 2026-11-15

Producer: `076a7b6c`; fold: `71fa65b8`. Session draft PR 1591. The clean source
has 44 passing fold-command tests and green targeted deterministic/judged lint
(18 source-diff passes at 99e9b97d; seven passes for the interrupted-summary fix and time-scope repair; eight passes for the final ancestry classification; zero failures). The report records exact commands and earlier
Rust evidence, exceptions, uncertainty bounds and suppression reasons.

Baseline run **43684231509, attempt 1**: Linux passed 2,155 e2e tests in 37.919
minutes; Windows passed 1,832 in 87.112 minutes; macOS passed 2,121 in
90.112 minutes (Apple M1 Virtual, three cores). Attempt 2 of the whole workflow
has been requested on the same PR/head. The report
flags one Linux profile overwritten by the later budgets target. Keep the known
Windows store exception outside the comparable passed set (ruling a2).

OS run **43684237592, attempt 2**: Linux hit the 180-minute job limit with
1,607 non-version profiles (five unfinished); Windows and macOS completed their full e2e selections successfully. Its
first attempt was canceled under the manager's capacity ruling. Demo PR 1593 was read and closed after attempt 2; its remote branch is deleted.
Baseline PR 1592 remains open. Do not cancel any other run.

Ruling a14 accepts the earlier full Linux OS record (43653434537, head
2a14d00a) plus the current partial cross-check; no extra Linux OS run. All 2,155
current journey names have earlier strace files. The two onetaskgraph spawn files
are byte-identical across the linked 0.14.15-to-0.14.16 release change. The report
records nested-strace failures and the remaining start-count uncertainty.

Ruling a15 keeps the three outside-tier macOS codex_server timeouts report-only:
cold nested builds with optimized dependencies are the probable cause (PR 1586),
not proved here. All measured e2e tests passed on every platform in point 1.

Rulings a16/a17 scope appended traces to the matching completed nextest run;
Linux point 1 excludes 174 records/56 starts, while macOS/Windows exclude none.
Ruling a19 is the final fold classification: only observed ancestry proves a
descendant; the fresh Windows ETW observation has 654 starts with unproven
lineage, excluded from pricing. Eight unmatched engine-parent starts remain
record-loss uncertainty. No further fold refinements: report limitations instead.

Ruling a18 calls repeated Git reads candidates, not proved cacheable. Supply the
intervening non-Git children to the git-reads spike; scratch repeat-intervening.mjs
asserts its candidate count agrees with the fold.

Next: collect baseline attempt 2 artifacts/logs, finish the count comparison and
report tables, then collect baseline attempt 3. Ruling a10 requires each
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
