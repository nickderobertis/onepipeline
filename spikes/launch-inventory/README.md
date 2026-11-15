# Shared process-launch inventory

This measurement branch never lands. It supplies the trace, fold command and
shared baseline for the git-read, engine-startup and child-wait option spikes.

Trace switch: ONEPIPELINE_LAUNCH_TRACE. The e2e harness enables it for its engines;
records go into target/test-profile and the existing uploaded job artifacts.
Producer: 076a7b6c. Final reader: ab27ccee. Fold one downloaded job with:

    node scripts/launch-inventory.mjs fold linux=/path/to/unpacked-job

Use multiple label=directory arguments for cross-platform tables. Each directory
contains test-profile/ and the timestamped job.log; OS observations are optional.
The command prints programs, callers/verbs, exclusive timing, start costs,
candidate Git repeats, child waits, top tests/sites, relaunches and trace cost.
It prints excluded records when a completed nextest invocation provides a scope.

The report is rendered from spike-report in authoring:process-launch-speed-2026-11-14,
document process-launch-speed-2026-11-14-spike-launch-inventory-report.
Its forthcoming final sections will supersede the historical checkpoints.
Read the compressed evidence with gzip -dc FILE.

By the user's decision (c3.0), two complete baseline points per OS satisfy the
original three-point requirement. No third attempt was started. Both are run
43684231509, attempts 1 and 2, demonstration head
36237191db78133b709b3d893ccc11d91d2a5272, GITHUB_SHA
37341b0a57e77d18f6db6c11c1b32d7b30fb8dac. Workflow blob:
db2bbe081a7b9319eb38bd014e72350b389e8b01. Both carry optimized dependencies and
the same no-behaviour src edit. The final report will give each OS's two measurements,
mean and range; dependent spikes remeasure their own baselines.

Verification on ab27ccee: 46 fold-command tests pass; formatting/static lint pass;
full branch judged lint excluding unchanged vendor lines passes 17 rules with
zero failures (history 20261115T143319Z-0ac73). Earlier targeted Rust/profile/script
checks and all 132 in-scope llmlint sites plus five other suppression sites/reasons will be recorded in the final report.
No shared enforcement configuration or threshold was changed; the report names the existing vendored compatibility allowances and their crate-wide scope (manager a23).

Important limits: one Linux profile per baseline point was overwritten by the
later budgets target; time scope removes later trace starts, not that aggregate
uncertainty. Repeated Git reads are candidates requiring invalidation proof.
The repeat-context capsules include intervening non-Git children by verb/site.
OS-only descendants have counts, not measured lifetimes. Windows ETW leaves 654
starts with unproven lineage excluded from pricing, and eight unmatched starts
with absent final parent records. Linux coverage uses the earlier complete
strace record plus the newer partial cross-check under a14. Nested strace
disturbed the observed tests; that run is not a passing baseline. Per-test
macOS/Linux differences are preserved, with remaining causal uncertainty stated.

Known Windows store timing behaviour remains outside the comparable passed set
under a2; its investigation is drafted. Outside-tier cold-build helper timeouts
remain report-only under a15. These limits are not claimed to be resolved.
No launch cut is prototyped here. No Actions/Nx cache entry was written or deleted.

Raw artifacts, logs and scratch helpers remain under:
    /home/whoop/ai-orchestrator/runs/brief-process-launch-speed-2026-11-14-spikes/scratch/1794352-0

Session PR1591 remains a draft measurement record. OS demonstration PR1593 is closed; baseline PR1592 remains open while Windows
finishes. No demonstration branch remains locally.

Checkpoint: attempt2 Linux passed2155 in41.055683 minutes; macOS passed2121
in94.377033 minutes. Windows is still running. c3.0 forbids a third attempt.
The linux-macos.checkpoint capsules preserve these two completed legs; the final
report and complete second-point capsule follow the Windows job.
