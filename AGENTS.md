# AGENTS.md

Durable instructions for anyone — human or agent — working in this repo.
Terse on purpose: this file is always-loaded context.

> `CLAUDE.md` is a symlink to this file — edit `AGENTS.md` only.

## What this is

`onepipeline` is the **composition layer**: it owns a task DAG, executes it over
[`oneagentgraph`](https://github.com/nickderobertis/oneagentgraph) (the agents)
and [`onevcs`](https://github.com/nickderobertis/onevcs) (the repositories), and
merges the three libraries' event streams into one. It is `ai-orchestrator`'s
`run-plan`/`orchestrate` core, extracted as a public tool.

Dependency direction is one-way and must stay that way: `onepipeline` →
`{oneagentgraph, onevcs}`. Neither sibling may depend on this crate, and the
agent/harness and repository/host concerns stay in theirs — do not regrow
harness selection, identity chains, or merge policy here.

Ships as a Rust library plus the `onepipeline` binary, distributed on crates.io,
PyPI (`onepipeline-cli`), and npm (`onepipeline-cli`).

## The contract is the source of truth

[`docs/contract.md`](docs/contract.md) is the **approved, verbatim** contract:
the plan schema, the driver and channel contracts, the executor seam, the
executor-rules grammar, the views, and the shipped content. It is committed as
approved and is not edited to match the code — the code is written to match it.
A change to the interface is a proposal to the planner who owns that contract,
never a unilateral edit.

`tests/contract.rs` parses the fenced blocks **out of `docs/contract.md` itself**
and drives them through the public types, so the doc and the types cannot drift.
Adding a contract type without extending that test leaves the doc unproven.

The crate implements that surface and **nothing beyond it**: the engine behind
it is private, so anything published promises what the contract says and no
more. [`docs/contract-divergences.md`](docs/contract-divergences.md) records
every place the contract could not be compiled exactly as written, and what the
planner who owns the contract ruled on each. That file is the status; do not
restate it anywhere, here included. A new divergence goes there as a proposal
and is never resolved unilaterally.

## Stack and composition

- **Product shape:** cli (a Rust library + the `onepipeline` binary)
- **Language(s):** rust (plus Bash provisioning, Node packaging scripts, and
  YAML/JSON/TOML config)
- **References composed:** base.md, shapes/cli.md, languages/rust.md,
  intersections/rust-cli.md, ci.md, llmlint.md, releasing.md, monorepo.md
- **Excluded, and why:** `install.sh` / a composite `action.yml` / a container
  image — the documented install surfaces are crates.io, PyPI, and npm, all of
  which *carry* the artifact rather than downloading a release asset by name, so
  there is no second asset-naming contract to drift. The GitHub Release archives
  are attached for manual download only. asdf / direnv — the committed
  `rust-toolchain.toml`, `Cargo.lock`, and `package-lock.json` already pin the
  workspace. A benchmark tier — nothing here is a hot path yet. What each
  *project* deliberately does without is recorded with that project, under its
  own **Deliberately absent** heading.

## Command surface

`just --list` is the index; do not hand-roll equivalents. `just check` is the
deterministic gate and `just gate` is the complete pre-push bar — `check` plus
the diff-scoped llmlint tier — and a change is not done until `gate` is green.
`deps-check`, `engines-current`, `msrv`, and `smoke-real` sit outside both
because two need the network — an advisory database and the crates.io index —
one a second toolchain, and one a GitHub credential; CI runs each as its own job,
through the recipe rather than around it.

The repo-wide verbs delegate to **Nx**, which fans a uniformly-named target out
across every project; what a target *does* stays with its project. Never loop
over projects by hand in a recipe, and declare a cross-project dependency in the
consuming `project.json` — an undeclared one silently drops that project out of
`nx affected`, so a pull request runs a gate that never touched it.

## Invariants (non-negotiable)

- **Coverage is enforced at 95% line coverage.** `just check` fails below it.
  Lower the bar only with the reason written here.
- **Tests are realistic — never mock the layer under test.** Drive the compiled
  binary as a subprocess and assert on exit code, stdout, and stderr; assemble
  the real package around the real binary. An in-process `main()` call is not an
  e2e, and nothing is `#[ignore]`d or skipped to pass.
- **Two tiers, split by credential.** The offline tier runs in `just check`. The
  credentialled one is `just smoke-real`, kept out of `check` and `gate` so a
  laptop pays nothing for it, and run by CI's `smoke` job through that same
  recipe. A journey that needs a credential belongs in the second tier and
  **refuses** when it is missing — a skip is a pass nobody earned.
- **Validate external input at its trust boundary.** Plan files, executor-rules
  files, and reply envelopes are external input: the schema structs reject
  unknown fields, so a typo fails loudly instead of being silently dropped.
- **The lock may not lag what the manifest already permits, and it resolves each
  engine once.** Every pin in `[workspace.dependencies]` is declared at the newest
  release the registry carries, and `just engines-current` fails on either
  departure. Neither a tag, a changelog, nor a requirement says what a build
  contains; only the lock does — and where it carries an engine twice it does not
  say that either, because each copy is separately the newest its own requirement
  permits.
- **Secrets never enter the tree.** `gh-secrets.json` names the required secrets
  and where they come from; the values live in the platform secret store.

## Commits, releases, and merging

**Squash-merge only**, auto-merge on, head branches deleted on merge. The PR
title becomes the squash subject and the PR body the squash message, so the PR
title *is* the release-driving commit and is linted against Conventional Commits
as a required check. PRs follow `.github/pull_request_template.md` (terse
**What** / **Why**).

Branch protection on `main` requires **every** gating job in `ci.yml`, each
matrix leg by its rendered name, with linear history, no force-pushes, and admins
able to override. Apply it with the create-repo skill's
`setup_github_governance.py`, passing every context, and **re-apply it whenever a
job or a matrix is added or renamed** — GitHub holds the required set, nothing
reconciles it against the workflow, and a leg nobody required is advisory, which
auto-merge lands straight past.

**Releases are fully automated; the only human action is merging a PR.**
`release-plz` is the single version driver: it opens a release PR, and merging it
tags `vX.Y.Z` and cuts the GitHub Release. That Release — created with a PAT,
because a tag from the default `GITHUB_TOKEN` triggers nothing — fires
`release.yml`, which builds the archives, wheels, and npm packages and publishes
them. **Nothing else writes a version:** maturin reads it from `Cargo.toml` via
`dynamic = ["version"]` and `scripts/npm-build.mjs` stamps it from the same
place.

**What this repository publishes is declared once**, in
`scripts/release-probe.sh`'s `TARGETS`, registry-qualified (`crate:`/`pypi:`/
`npm:`) because one of the names is served by two registries on two cadences.
That script is also the probe a consuming run asks before launching work that
depends on a release of this one: a version on stdout, *nothing* for no release
yet, and a non-zero exit for **not answered** — which is never the same answer
and must never be collapsed into the second, because a consumer holds
indefinitely on it. `npm/test/release-targets.test.mjs` is the gate on that list.

Bump policy, **pre-1.0**: `feat` → minor, `feat!` / `BREAKING CHANGE` → minor (a
breaking change pre-1.0 is not yet a major), `fix` / `perf` / `refactor` /
`build` → patch, and `chore` / `docs` / `ci` / `test` / `style` → no release.

## After the main task

Two standing goals beyond the ask: (1) engineer the context for next time — a
real e2e for any journey a bug slipped through, a script for a step done by
hand, a terse note here for what the code doesn't show; (2) keep the repo and
its environment clean and reproducible. Fold either in when it is the
lowest-error path; otherwise propose it. Skip busywork.
