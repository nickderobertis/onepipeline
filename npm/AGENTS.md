# npm distribution

`onepipeline-cli` on npm is a **launcher** that carries no binary: the prebuilt
binary ships in a per-platform package (`onepipeline-cli-<platform>-<arch>`)
that npm selects by `os`/`cpu`, and `bin/onepipeline.js` resolves it and execs
it with the caller's argv.

Five places name that platform matrix and must move together:

1. `bin/onepipeline.js`'s `PACKAGES` map,
2. `package.json`'s `optionalDependencies`,
3. `scripts/npm-build.mjs`'s `TARGETS` table,
4. the `build-npm` matrix in `.github/workflows/release.yml`, and
5. that file's `verify-npm` matrix, whose `package:` key names the platform
   package each leg exists to prove.

The fifth is the one that used to be missing. A platform a release builds and
publishes but never installs is a platform nothing watches, which is how aarch64
Linux went unnoticed. `npm/test/platform-matrix.test.mjs` reconciles all five.

The committed `package.json` carries `0.0.0-managed`, not a real version. The
version has exactly one source — `Cargo.toml`, written by release-plz — and
`scripts/npm-build.mjs` stamps it into the launcher and every platform pin at
publish time. Never hand-edit a version here.

## Deliberately absent

No `typecheck` target: this project is three JavaScript files with no type
system, so there is nothing for one to check.

No ESLint `enforce-module-boundaries` rule over the project tags. The two
projects share no imports — one is a Rust crate, the other a launcher that
`exec`s its binary — so it would mean adding ESLint to a repo whose JavaScript
is linted by Biome, purely to enforce a boundary nothing can cross.

No third Nx project for the PyPI wheel. maturin builds it from the same crate,
so it is a second packaging of one deliverable rather than a second
deliverable; CI's `wheel` job proves it end to end.

Nothing in this directory is published from a developer's machine:
`.github/workflows/release.yml` assembles, packs, and publishes it, and
`scripts/publish-npm.sh` makes that publish idempotent.

**`npm publish` exiting 0 does not mean the registry serves the version.** It
means the upload was accepted; the version becomes resolvable afterwards, and on
registry.npmjs.org that gap ran from seconds to eight minutes. So
`scripts/publish-npm.sh` waits for the registry to actually serve what it just
published, and refuses to publish anything before the exact versions its own
manifest pins are served — which is what stops the launcher being offered
against platform packages an install would silently skip. The evidence, and the
journeys that drive it against a registry that lags, are in that script's header
and in `npm/test/publish-gate.test.mjs`.
