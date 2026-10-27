# npm distribution

`onepipeline-cli` on npm is a **launcher** that carries no binary: the prebuilt
binary ships in a per-platform package (`onepipeline-cli-<platform>-<arch>`)
that npm selects by `os`/`cpu`, and `bin/onepipeline.js` resolves it and starts
it with the caller's argv.

**The installed command is the engine's process, not a parent of it.** Where
Node.js can exec (Linux and macOS), the PID a caller holds must be the engine's:
a caller that marks itself by the PID that execs the engine re-entered itself
without end while the launcher was a child-spawning parent. Where it cannot
(Windows), the engine's status and interrupts pass through and no engine outlives
the launcher. `npm/test/launcher-process.test.mjs` holds both.

Five places name the platform matrix and must move together:

1. `bin/onepipeline.js`'s `PACKAGES` map,
2. `package.json`'s `optionalDependencies`,
3. `scripts/npm-build.mjs`'s `TARGETS` table,
4. `PLATFORMS` in `release/release.py`, the targets `.github/workflows/release.yml`'s
   build jobs build (`just release-<platform> plan` lists them), and
5. those jobs' `verify` steps, which install each platform package from the
   registry and start it.

A platform that is built and published but never installed is a platform nothing
watches, so the fifth is not optional. `npm/test/platform-matrix.test.mjs` holds
the five together.

The committed `package.json` carries `0.0.0-managed`, not a real version. The
version has exactly one source — `Cargo.toml`, written by release-plz — and
`scripts/npm-build.mjs` stamps it into the launcher and every platform pin at
publish time. Never hand-edit a version here.

## Deliberately absent

No `typecheck` target: this project is three JavaScript files with no type
system, so there is nothing for one to check.

No ESLint `enforce-module-boundaries` rule over the project tags. The two
projects share no imports — one is a Rust crate, the other a launcher that
starts its binary — so it would mean adding ESLint to a repo whose JavaScript
is linted by Biome, purely to enforce a boundary nothing can cross.

No third Nx project for the PyPI wheel. maturin builds it from the same crate,
so it is a second packaging of one deliverable rather than a second
deliverable; CI's `wheel` job proves it end to end.

Nothing in this directory is published from a developer's machine:
`.github/workflows/release.yml`'s jobs assemble, pack, and publish it through
`release/release.py`, and `scripts/publish-npm.sh` makes that publish idempotent.

**Never treat an `npm publish` exit code as evidence that the version
resolves.** Ask the registry.
