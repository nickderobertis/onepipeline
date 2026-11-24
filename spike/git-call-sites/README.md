# SPIKE: where the engine's git processes come from

Branch-only harness for the plan `authoring:onepipeline-e2e-setup-cost`, node
`spike-git-call-sites`. **Never landed.** It measures; it fixes nothing. The report is
the plan document `onepipeline-e2e-setup-cost-spike-git-call-sites-report`.

## The one command

    bash spike/git-call-sites/run.sh OUT_DIR readjust adoption lifecycle rework \
        publication_boundary continuation review_reply review out_of_band

runs the named e2e modules (Linux, `TEST_THREADS`, default 4, the CI runner's CPU
count), then prints the tables the report quotes. `--analyse` before `OUT_DIR` re-reads
an earlier run without running the tests again. It downloads CI run 3's (45298442141)
Linux `demo-profile-linux` artifact once for the cross-check, with `gh`. Run
`just _ensure-onetaskgraph` first: without `onetaskgraph` 0.15.0 in `target/tools/`
nine `publication_boundary` template journeys refuse, and the counts come up short.

## What the harness is

- `onevcs/` — `onevcs` 0.69.0 as released, vendored and path-patched in the root
  `Cargo.toml` (`[patch.crates-io]`, so `onevcs-testing` takes it too). The changes are
  marked `SPIKE`: `run_fed` in `src/git.rs`, where every production git process
  starts, writes one line per process to `ONEVCS_SPIKE_SPAWN_LOG`
  (`src/git/spike_log.rs`): argv, directory, thread, the in-call memo's scope, times,
  status, a hash of what git answered, and the backtrace frames. It gives the process
  `ONEVCS_SPIKE_SPAWN` and names it in `GIT_TRACE2_ENV_VARS`, so every git that process
  starts in turn writes the same id into its trace2 events. A checkout's own
  `GIT_TRACE2_EVENT` file (`HookTrace`) is appended to the test's before it is removed.
- `crates/testfakes/src/profile.rs` — under `ONEPIPELINE_SPIKE_GIT_DIR`, each
  `onepipeline` a test starts gets `GIT_TRACE2` (brief, the shape CI run 3 counted),
  `GIT_TRACE2_EVENT` and the spawn log, each at a file of that test's own.
- `src/spike_git.rs` — logs the git the engine starts itself with `std::process::Command`
  (`src/vcs.rs`, `src/destination.rs`, `src/land.rs`, `src/reply.rs`) to the same spawn
  log, with its call-site frames.
- `src/loopstats.rs`, `src/main.rs` — each reconcile pass and each `onepipeline`
  process marks its start in the spawn log, so a git process is placed in its pass.
- `attribute.py` — joins the three files and prints the tables.
