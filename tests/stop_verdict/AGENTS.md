# Stop-verdict budget journeys

- Regenerate the records with `just stop-verdict-journeys`; `just budgets` only
  reads them. Never time anything, build a fixture or launch a process in a reader.
- Keep these full workloads on their own Nx edge: an ordinary e2e edit must not
  rebuild them.
- Write telemetry only after every verdict assertion has passed, and never time a
  call with the git-counting shim or a tracer attached.
- Start every host-shaped sample from `AgedHost::restore` then `AgedHost::verify`
  (and the acknowledgements file put back): a read records decisions, so a sample
  taken after another read measures a state nobody ships.
- Assert rows against the classes the fixture declares, never against another read.
- `scripts/stop-guard-unpublished-build-inputs.json` and this project's `test`
  inputs name the same files; change them together.
