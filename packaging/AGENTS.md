# Packaging proofs

`onepipeline-packaging` owns the checks that build what the release ships:
`test`, the npm journeys under `npm/test/packaging/` that build, pack, install or
publish an artifact, and `wheel-smoke`, the x86_64 wheel built in the release's
manylinux image. It has no sources of its own; the launcher and the release
scripts belong to their projects, which format and lint them.

**What selects it is its own `packagingSelection` and `input-selection`, and
nothing else.** A file that decides what a package contains joins that list; a
file the proofs only read, like the binary's sources, goes into `packagingHash` as
a runtime input, which moves the cache key without selecting. So a change only
under `src/`, `crates/`, `tests/` or `schemas/` never runs them, and never replays
a stale pass either.

**A version and an unrelated recipe select nothing.** The `selection:version-free`
tag has `scripts/nx-affected.sh` drop this project when the only selecting change
is the crate's version in the Cargo manifests (every release pull request's), or a
justfile edit to no recipe `packagingHash` names. A dependency moved or added
still selects it. When that check cannot answer, Nx's file-level answer stands.

## Deliberately absent

No wheel build cache. The cache rule recorded above `onepipeline-cache-headroom` in
`budgets.yaml` selected none: the repository's Actions cache was already past its
headroom before any new family, so `scripts/build-linux-wheel.sh` reads no cache
and the release's build is the one CI runs.
