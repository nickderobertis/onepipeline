# Engine startup measurement record

Nothing on this branch lands. Manager c13.0 makes profiling local only; hosted
comparisons use the inventory's existing launch trace and complete e2e selection.

Prototype: `ONEPIPELINE_STARTUP_CUT=contributors` bypasses privateterms' conservative
contributor prefilter. Unset it for the unchanged matcher path. The full normalization,
matching, phrase exemptions, replacement, refusal and span mapping remain. This is
an experiment in trading a per-process Unicode table for full scans, not a production
recommendation for long-lived matchers.

Verify both public matcher configurations with:

    bash scripts/engine-startup-verify.sh

Record the real e2e harness and its child processes locally (requires samply and
sampling permission; use the compiled debug e2e binary built with optimized dependencies,
then the release e2e binary):

    bash scripts/engine-startup-profile.sh E2E_BINARY OUTPUT_DIR
    python scripts/engine-startup-cpu.py OUTPUT_DIR/profile.json.gz OUTPUT_DIR/profile.json.syms.json

Build each binary with `cargo test --locked --test e2e --no-run --message-format=json`
and select its `compiler-artifact` executable; add `--release` for release. For the
debug build include `--config 'profile.dev.package."*".opt-level=2'`, as the
hosted comparison does. For release sampling preserve symbols with
`CARGO_PROFILE_RELEASE_STRIP=none`; this changes symbol retention, so record that
qualification with its evidence. No wall-clock saving is inferred on this host.

Fold hosted artifacts and compare sides with the single command:

    node scripts/engine-startup.mjs fold before/1=BASE_JOB contributors/1=AFTER_JOB

Use one label per job; each directory holds its unpacked `test-profile/` and
its timestamped `job.log`. The shared inventory validates and scopes the inputs.
It prints runner, e2e minutes, completed start/adopt means, incomplete counts,
profile-name digest, each side's mean and spread, the measured difference, and
its 27/30 typical-PR estimate. A difference inside either spread is undecided.
Presence of a profile does not prove a pass; the nextest outcomes remain required.
The recorded baseline's executable-only test-profile keys cannot yield verb means;
completed launch-trace lifetimes are separately identified and exclude creation.

<!-- llmlint: ignore[work_goes_through_command_surface] This task requires the narrow existing Node test modules; the available just test recipe fans out the whole workspace and accepts no module filter. This never-land measurement invokes that project’s real runner for these two modules rather than adding or weakening a shared recipe (manager a1). -->
Local interval accounting: `node scripts/engine-startup.mjs phases local=OUTPUT_DIR`. It prints spawn-call and operation-live interval unions, unallocated wall, each program and non-additive wait overlays; sampled CPU and filesystem latency are not fabricated as disjoint buckets.

Parser journeys: `node --test npm/test/engine-startup-*.test.mjs`.
Fixture provenance is in `npm/test/golden/engine-startup/README.md`.
The authoring report is
`authoring:process-launch-speed-2026-11-14-spike-engine-startup-report`.
