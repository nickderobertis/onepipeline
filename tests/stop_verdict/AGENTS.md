# Stop-verdict journeys

- `counted_stop_verdict` is the pull request's: it times nothing and asserts every
  counted call's exact counts. A change that moves a count updates
  `counts.rs`'s `EXPECTED` in its own diff; never loosen a count to a bound.
- `timed_stop_verdict` is `onepipeline-stop-verdict:timed`'s, on main alone: regenerate
  its record with `just stop-verdict-journeys`. Never time anything, build a fixture or
  launch a process in a reader.
- Keep these workloads on their own Nx edge: an ordinary e2e edit must not run them.
- Write telemetry only after every verdict assertion has passed, and never time a
  call with the git-counting shim or a tracer attached.
- Start every host-shaped call from `AgedHost::restore` then `AgedHost::verify`
  (and the acknowledgements file put back): a read records decisions, so a call
  made after another read measures a state nobody ships.
- Assert rows against the classes the fixture declares, never against another read.
- `scripts/stop-guard-unpublished-build-inputs.json` and this project's `test`
  inputs name the same files; change them together.
