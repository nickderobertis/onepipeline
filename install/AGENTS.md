# Install-proof selection

`onepipeline-install-smoke-selection` and `onepipeline-install-documented-selection`
run nothing. Each answers one question for `scripts/ci-job.sh`, through
`scripts/nx-affected.sh --affects`: does this change reach what decides that install
proof? The proofs themselves are the crate's `install-smoke` and
`install-documented` targets, so `test-linux` keeps running `install-smoke`
whenever the crate is affected.

**Where each runs.** `install-smoke` (`cargo install --path .` of this revision)
runs in `test-windows` and `test-macos` only when its selection project is
affected. Otherwise `just release-<platform> build` covers those sources: a locked
release build under `-D warnings`, then `scripts/smoke-published.sh` over each
artifact it test-installed. `install-documented` installs main, never the pull
request's code. Every pull-request test job therefore runs it only when its
selection project is affected, and `ci.yml`'s `timed` runs it on Linux on every
push to `main` that affects the crate.

**What selects a proof is its project's selection list and nothing else.** A file
that decides the install beyond the code joins `installSmokeSelection`; a file the
documented install reads from this tree joins `installDocumentedSelection`. The
binary's sources stay off both, so a change only under `src/`, `crates/`,
`schemas/` or `tests/` runs neither on Windows or macOS. Both proofs are
uncached, so a narrower selection never replays a stale pass.

**The crate's `project.json` is on both lists**, because the targets are defined
there. Any edit to that file therefore selects them. This is wider than the target
definitions, and it errs toward running.

**A version selects nothing.** The `selection:version-free` tag has
`scripts/nx-affected.sh` drop the smoke's selection project when the only selecting
change is the crate's version in the Cargo manifests. A dependency moved or added
still selects it. The documented install reads no manifest of this tree, since it
builds main's.
