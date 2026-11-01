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
  intersections/rust-cli.md, ci.md, llmlint.md, releasing.md, project-graph.md
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
That tier is plain llmlint under Nx's cache and stays unwrapped: a fault in it is
fixed in Nx, llmlint or the recipe's run configuration, never by a script here.
A recipe that needs the network, a second toolchain, or a credential sits outside
both, says why in its justfile comment, and runs in CI as its own job, through
the recipe rather than around it.

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
  laptop pays nothing for it, and run by CI's `test-linux` job through the Nx target
  that recipe runs. A journey that needs a credential belongs in the second tier and
  **refuses** when it is missing — a skip is a pass nobody earned.
- **Validate external input at its trust boundary.** Plan files, executor-rules
  files, and reply envelopes are external input: the schema structs reject
  unknown fields, so a typo fails loudly instead of being silently dropped.
- **The lock may not lag what the manifest already permits, and it resolves each
  engine once.** A pin in `[workspace.dependencies]` is a floor, written at the
  newest release the registry carried then; a sibling releasing inside that caret
  moves the lock and not the requirement, and `just engines-current` fails on
  either departure. A newer release that one of this manifest's *other* pins holds
  back — it requires a sibling outside that pin's window, so taking it resolves
  the sibling twice — is neither: the check names it and the pin, prints no
  update, and passes. Neither a tag, a changelog, nor a requirement says what a
  build contains; only the lock does — and where it carries an engine twice it
  does not say that either, because each copy is separately the newest its own
  requirement permits.
- **Secrets never enter the tree.** `gh-secrets.json` names the required secrets
  and where they come from; the values live in the platform secret store.

## Commits, releases, and merging

**Squash-merge only**, auto-merge on, head branches deleted on merge. The PR
title becomes the squash subject and the PR body the squash message, so the PR
title *is* the release-driving commit and is linted against Conventional Commits
as a required check. PRs follow `.github/pull_request_template.md` (terse
**What** / **Why**).

Branch protection on `main` requires exactly `ci.yml`'s four jobs — `lint`,
`test-linux`, `test-windows` and `test-macos` — by their rendered names, with
linear history, no force-pushes, and admins able to override. It does **not**
require a branch to be up to date (`strict: false`), by the user's decision of
2026-10-18, and no workflow re-runs the tests on a push to `main`. That is the
trade: a merge whose tests passed against an older `main` can land untested
against the newer one, and what catches it is `rust-cache-warm.yml`'s build-only
job on every push to `main`, which fails when `main` does not build. Apply the
protection with the create-repo skill's `setup_github_governance.py`, passing
every context in `.github/required-contexts.json`, and **re-apply it whenever a
job is added or renamed** — GitHub holds the required set, nothing reconciles it
against the workflow, and a job nobody required is advisory, which auto-merge
lands straight past.

`ci.yml`'s four jobs are the whole of that set, and the only jobs a pull
request's push runs: every other workflow starts on no `opened`, `synchronize`
or `reopened` event (`.github/ci-shape/events.test.mjs` holds it). What used to
be workflows of their own are steps of `lint`: the engine-currency check, the
judged lint, the PR-title lint, the suppressions comment, and the visual docs'
capture, drift gate, sticky comment and preview. The engine-currency step is
advisory (`continue-on-error`): what it asks is whether the registry has moved
past this lock, so its answer changes without this tree changing and a release
nobody here made must not block an unrelated merge. Red there is closed by one
`cargo update -p <spec>` on any branch. `engine-currency.yml` and `launcher.yml`
remain, run by hand alone (`workflow_dispatch`): the first asks the same
question, and the second re-runs the npm launcher's process-tree journeys on
Windows, which `test-windows` and `test-macos` already run on every pull request
the change reaches (the npm project's `test-launcher` target), as `just check`
does on Linux. `visual-docs.yml` is **not** required: it runs when a pull request
closes (deleting its preview), on a push to `main` (publishing the canonical
gallery, skipped on the release commit), on its monthly schedule (pruning the
gallery's history) and by hand. Local capture and review are manual; no
screenshot step is reachable from `check` or from `gate`.

**Eight budgets in `budgets.yaml` hold CI, CD and the local gate**, each read from
telemetry the run already produced and never measured afresh:

- `onepipeline-ci-check-time` (longest PR check), `onepipeline-cd-job-time`
  (longest CD job, 1800 s), `onepipeline-release-cycle-time` (a PR's checks plus
  its merge to release, 5400 s), `onepipeline-ci-job-count` (jobs one PR's last
  commit runs, 4), `onepipeline-cd-job-count` (jobs one release's CD runs, 6),
  `onepipeline-release-read-requests` (6) and `onepipeline-pr-read-requests` (2)
  carry the label `release-tail`. They are checked **after the release, reporting
  only**: the last step of `release.yml`'s `publish` runs
  `onebudgetspec check budgets.yaml --label release-tail` with the onevcs CLI a
  step before it installs, and turns red with each figure's breakdown when one is
  over; nothing reads its result, so it never blocks, demotes or reverts a
  release. The figures come from `scripts/actions-budget-figures.sh`, a copy of
  dero-skills' create-repo source edited only there. At PR time a check that runs
  long is failed by its own `timeout-minutes`.
- `onepipeline-gate-time` (one `just gate`'s wall clock, 1800 s) carries
  `host-variable` and is checked **on a development machine, as a warning**:
  `just gate` runs its whole body inside `scripts/gate-time.sh`, which records its
  start and end to the gitignored `.gate-time.json` and checks it — over warns
  naming the budget and the gate still passes; a missing record or an invalid
  `budgets.yaml` fails it. No pre-push hook runs it: CI owns the gate.

Every other check of the root `budgets.yaml` (`just budgets`,
`just host-view-budget`) excludes both labels.

**A job's assertions and the artifact it installs come from one revision.**
`onepipeline:install-documented`, which the three test jobs run, installs the
remote's default branch — README's `cargo install --git` one-liner names no ref —
so it is smoked with *that* revision's `scripts/smoke-published.sh`, not the
branch's; `scripts/install-proof.sh` says why. Paired the other way, such a proof
is unsatisfiable for the very pull request that changes what it asserts.

**Releases are fully automated; the only human action is merging a change.**
`release-plz` is the single version driver: `release-plz.yml` opens a release PR
and asks GitHub to merge it automatically once its required checks pass, and
that merge tags `vX.Y.Z` and cuts the GitHub Release. That Release — created
with a PAT, because a tag from the default `GITHUB_TOKEN` triggers nothing —
fires `release.yml`. **Nothing else writes a version:** maturin reads it from
`Cargo.toml` via `dynamic = ["version"]` and `scripts/npm-build.mjs` stamps it
from the same place.

**A release job's work is a recipe, never inline steps.** A target is built once
and every artifact for it is packaged from that one binary; nothing is published
before that exact file installed and ran, and nothing is reported published until
it installs back from its registry. The platform-independent artifacts wait for
every platform. **A failed build job leaves the platforms that built published
and the platform-independent artifacts unpublished** until the failed job and
then `publish` are re-run; every upload skips what its registry already serves.
A rebuild is not byte-identical, so a re-run keeps what PyPI and npm took from
an earlier attempt rather than uploading over it, and the Release's archive
follows them (`release/release.py`'s `publish` says how); verify reports a
platform its registries serve as more than one build.

**Where each tier runs follows from that release model.** Every pull request,
release-plz's release PR included, runs its affected tier against its merge
base in the four jobs (`scripts/ci-job.sh`); with no derivable base it falls
back to every project. On the release PR, whose tree is a `main` commit that
already passed plus only release-plz's version and changelog edits, the three
test jobs prove that from the trees and succeed without re-running
(`scripts/release-pr-shortcut.mjs`), and `test-macos` does not start at all.
Nothing re-sweeps at release time or on a push to `main`. A Release made by
hand — the documented fallback — is refused when its tag does not name
`Cargo.toml`'s version, and is otherwise trusted to name a commit `main` gated.

**What this repository publishes is declared once**, in `release-targets.toml`
at the root, at the canonical release-target schema `onevcs`'s `docs/contract.md`
defines — registry-qualified because one of the names is served by two registries
on two cadences. That document's `probe`, `scripts/release-probe.sh`, is what a
consuming run asks before launching work that depends on a release of this one: a
version on stdout, *nothing* for no release yet, and a non-zero exit for **not
answered** — which is never the same answer and must never be collapsed into the
second, because a consumer holds indefinitely on it.

Bump policy, **pre-1.0**: `feat` → minor, `feat!` / `BREAKING CHANGE` → minor (a
breaking change pre-1.0 is not yet a major), `fix` / `perf` / `refactor` /
`build` → patch, and `chore` / `docs` / `ci` / `test` / `style` → no release.

## After the main task

Two standing goals beyond the ask: (1) engineer the context for next time — a
real e2e for any journey a bug slipped through, a script for a step done by
hand, a terse note here for what the code doesn't show; (2) keep the repo and
its environment clean and reproducible. Fold either in when it is the
lowest-error path; otherwise propose it. Skip busywork.
