# Canonical command surface for onepipeline.
#
# `just bootstrap` works from a clean clone; `just check` is the deterministic
# quality gate and `just gate` is the complete pre-push bar (check + the llmlint
# diff tier). Recipes are quiet on success and specific on failure.
#
# This is a monorepo: the repo-wide verbs delegate to Nx, which fans the
# uniformly-named target out across every project. They never loop over projects
# by hand. What a target *does* stays with its project — the `_crate-*` recipes
# below are the Rust crate's own tools, and packaging/project.json names its.

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Recipe parameters reach a recipe's shell as `$1`, `$2`, ... as well as through
# `{{ }}`. The judged tier takes its base and its options that way: a value
# interpolated into shell source is read as shell before anything can validate it,
# and that one is typed at a command line.
set positional-arguments := true

# llmlint: ignore-file[tool_output_is_signal] recipes that hand straight to cargo,
# clippy, rustdoc, or cargo-deny inherit those tools' diagnostics, which already
# name the exact problem and its fix; a wrapper message would bury them. The
# recipes whose failure needs project-level context (_crate-fmt-check,
# _crate-coverage, msrv) add one explicitly.

# The released `onetaskgraph` this build's own checks read their plans through.
# A plan is one project of that store and this crate *drives* the binary rather
# than linking it, so cargo cannot bring it into the build graph and `bootstrap`
# installs it instead — pinned to a published release, so a green check says
# something about what a host that installs one runs. It is the newest release
# when it was pinned, and the one the engine's store journeys were run against
# and pass through: installed by this recipe, not merely read. Any release from
# 0.2.0 carries the custom metadata the mapping reads, which is
# `src/taskgraph.rs`'s floor. It is written bare on purpose: `cargo install
# --version` reads a `MAJOR.MINOR.PATCH` with no operator as that exact release,
# never as the caret requirement the same string means in a manifest.
#
# **This is the only place the release is named.** `_ensure-onetaskgraph` reads
# it, and `taskgraph::tests::the_release_the_checks_install_meets_the_floor_and_is_named_once`
# fails if it falls below that floor, is named twice, or stops being what the
# recipe installs.
#
# The crate under test still drives the binary rather than linking it. What does
# link onetaskgraph is `crates/testfakes` — see `[workspace.dependencies]`, which
# requires the same release this line names and is why `rust-version` is 1.97.
onetaskgraph-version := "0.2.32"

# The renderer the visual-docs capture draws each scene with (`just screenshots`).
# NOT part of `check`, `gate` or `bootstrap`: screenshots are informational, and
# this is the only version of `freeze` a capture of this repository is ever taken
# with.
#
# **This is the only place the release is named.** `just screenshots-tools`
# installs it from here, and CI installs it through `scripts/install-freeze.sh`,
# which reads this very line out of this file rather than keeping a second pin
# that could drift from it. Bumping it reflows every shot: bless once and commit
# the new baseline with the images.
freeze-version := "0.2.2"

# The MSRV has one source of truth — Cargo.toml's `rust-version` — so `just msrv`
# cannot promise a floor the manifest no longer declares. CI reads the same field.
msrv-version := `sed -n 's/^rust-version *= *"\([^"]*\)".*/\1/p' Cargo.toml`

# Keep the gate's own output to signal: successes are silent, failures are not.
export CARGO_TERM_QUIET := "true"

# List available recipes.
default:
    @just --list

# Every project's `bootstrap` target, so one clean-clone command provisions the
# whole graph rather than the crate alone. Serialized: the projects share
# installers, and two of those running at once race the same directory.
# Set up the project from a clean clone.
bootstrap: _hooks
    @bash scripts/nx.sh run-many -t bootstrap --parallel=1

# Point this clone's git hooks at the committed directory. `core.hooksPath` is
# per-clone state that is never committed, so a guard nothing activates is a
# guard that runs nothing — this is what makes .githooks/pre-push real. The
# directory carries the screencomp visual guard and nothing else: `just gate`
# stays unhooked, exactly as it is today.
_hooks:
    @git rev-parse --git-dir >/dev/null 2>&1 || exit 0; \
      git config core.hooksPath .githooks

# The Rust crate's own provisioning (the `onepipeline:bootstrap` target).
_crate-bootstrap:
    @rustup show active-toolchain >/dev/null 2>&1 || rustup toolchain install
    @rustup component add rustfmt clippy llvm-tools >/dev/null \
      || { echo "cannot add toolchain components — install rustup (https://rustup.rs/) and re-run" >&2; exit 1; }
    @just _ensure-tool cargo-nextest
    @just _ensure-tool cargo-llvm-cov
    @just _ensure-onetaskgraph
    @just _ensure-strace
    @cargo fetch --locked --quiet

# The one binary this crate composes that cargo cannot build for it: `onevcs` and
# `oneagentgraph` are linked crates, and this one is driven as a subprocess by
# design — that is onetaskgraph's own recorded decision for its SDKs, and it is
# what keeps a crates.io release ordering out of every release here. Installed at
# bootstrap, so `check` itself stays offline; the e2e suite **fails** without one
# rather than skipping, because a plan read through a stand-in would prove the
# stand-in.
_ensure-onetaskgraph:
    @resolved="${ONETASKGRAPH_BIN:-$(command -v onetaskgraph 2>/dev/null || true)}"; \
      cargo_root="${CARGO_HOME:-$HOME/.cargo}"; \
      if [[ "$resolved" == */bin/onetaskgraph ]]; then \
        resolved_root="${resolved%/bin/onetaskgraph}"; \
        if [[ "$resolved_root" != "$cargo_root" ]]; then \
          cargo install onetaskgraph --version {{onetaskgraph-version}} --locked --quiet --force \
            --root "$resolved_root"; \
        else \
          cargo install onetaskgraph --version {{onetaskgraph-version}} --locked --quiet --force; \
        fi; \
      else \
        cargo install onetaskgraph --version {{onetaskgraph-version}} --locked --quiet --force; \
      fi; \
      if [[ -n "$resolved" && "$resolved" != */bin/onetaskgraph ]]; then \
        cp "$cargo_root/bin/onetaskgraph" "$resolved"; \
        chmod +x "$resolved"; \
      fi

# The tracer the Linux-only e2e journeys (`channel.rs`, `listing.rs`,
# `unwatched.rs`) run the binary under, and refuse without. A system package,
# so it is installed only where the machine is this repository's to provision —
# a CI runner, which `CI` marks — and named with its install command elsewhere:
# a `sudo` prompt inside `just bootstrap` is a hang in a session hook. Nothing
# on other platforms, where those journeys are compiled out.
_ensure-strace:
    @[ "$(uname -s)" = Linux ] || exit 0; \
      command -v strace >/dev/null 2>&1 && exit 0; \
      if [ -n "${CI:-}" ]; then \
        { sudo -n apt-get update -qq >/dev/null && sudo -n apt-get install -y -qq --no-install-recommends strace >/dev/null; } \
          || { echo "strace could not be installed through apt-get, and the Linux-only e2e journeys refuse without it" >&2; exit 1; }; \
      else \
        just _strace-preflight; \
      fi

# Asked ahead of the tiers that hold those journeys, so the missing tool is
# named up front rather than by a panic inside a journey.
_strace-preflight:
    @[ "$(uname -s)" = Linux ] || exit 0; \
      command -v strace >/dev/null 2>&1 \
      || { echo "strace not installed — the Linux-only e2e journeys in tests/e2e/channel.rs, listing.rs and unwatched.rs run the binary under it and refuse without it: sudo apt-get install -y strace (or your distribution's strace package), then re-run" >&2; exit 1; }

# These are test runners, not rules: their version cannot change the gate's
# verdict, so both here and CI take the latest rather than keeping two pins that
# drift apart.
# Install a cargo dev tool if it is missing. Quiet when already present.
_ensure-tool tool:
    @command -v {{tool}} >/dev/null 2>&1 || cargo install {{tool}} --locked --quiet

# The tiers run in fail-fast order as dependencies, each fanned across every
# project by Nx. The body then runs the per-project `check` aggregate — the same
# target `just check-affected` uses — which replays from the cache in a second
# and is what stops the full sweep and the affected sweep from covering
# different tiers.
# Deterministic quality gate, every project.
check: fmt-check lint test doc
    @bash scripts/nx.sh run-many -t check
    @echo "check: ok"

# What PR CI runs: the same gate, scoped to the projects this branch's diff can
# reach. Fails closed — with no derivable merge base it runs everything.
# Deterministic quality gate, affected projects only.
check-affected:
    @bash scripts/nx-affected.sh -t check
    @echo "check-affected: ok"

# What the macOS and Windows legs run: the same tiers as `check`, minus the two
# that are Linux-only. `test` enforces the coverage floor off instrumentation
# that is measured on Linux alone, so the suite runs through `test-quick`
# instead, and `doc` is a link check no second platform can answer differently.
# Named here rather than spelled out as workflow steps, so the cross-platform
# legs cannot drift away from what the gate means.
# Deterministic quality gate for the cross-platform legs, without the coverage floor.
check-cross: fmt-check lint test-quick
    @echo "check-cross: ok"

# The complete pre-push bar: the deterministic gate plus the LLM-judge tier
# scoped to what this branch changed. `check` stays offline and credential-free;
# this is where the non-deterministic tier joins it.
# Full gate: `check` plus the diff-scoped llmlint tier.
gate base="origin/main": check
    @just lint-llm-diff "{{base}}"
    @echo "gate: ok"

# Escape hatch for Nx itself, e.g. `just nx show projects` or `just nx graph`.
# Run an arbitrary Nx command against this workspace.
nx *ARGS:
    @bash scripts/nx.sh {{ARGS}}

# The crate's half denies warnings, so a build the gate would reject fails here
# first; the packaging half assembles the npm launcher exactly as a release
# does. Neither publishes anything.
# Build every project's artifact.
build:
    @bash scripts/nx.sh run-many -t build

# Verify formatting without modifying files.
fmt-check:
    @bash scripts/nx.sh run-many -t format-check

# Format the codebase in place.
format:
    @bash scripts/nx.sh run-many -t format

# Lint every project with its own linter; any warning is an error.
lint:
    @bash scripts/nx.sh run-many -t lint

# Every project's test suite; the crate's enforces its coverage floor.
test:
    @bash scripts/nx.sh run-many -t test

# Build the docs with warnings denied (kept in the gate so doc links don't rot).
doc:
    @bash scripts/nx.sh run-many -t doc

# Verify the crate's formatting without modifying files.
_crate-fmt-check:
    @cargo fmt --all -- --check || { echo "formatting drift above — run 'just format'" >&2; exit 1; }

# `--all-targets` so the tests compile too: a build that only proves the binary
# leaves the suite's own compile errors for the test tier to discover.
_crate-build:
    @RUSTFLAGS="-D warnings" cargo build --locked --all-targets --quiet

# Format the crate in place.
_crate-format:
    @cargo fmt --all

# Lint the crate with clippy; any warning is an error.
_crate-lint:
    @cargo clippy --all-targets --locked --quiet -- -D warnings

# How much a run reports as it goes is `.config/nextest.toml`'s to say. Do not put
# a `--status-level` back on these recipes: a flag beats the profile, and `fail` —
# what they used to pass — ranks below `slow`, so it mutes the `SLOW` line a hang
# produces along with the passes, while the profile still appears to ask for it.
# `NEXTEST_PROFILE` is no protection. `smoke-real` states its own because
# `--no-capture` and `all` are what that journey *is*; `all` mutes nothing.

# The offline tier, in the two halves its two projects run — the crate and
# `onepipeline-note-journeys`. `smoke` is in neither: it needs a GitHub credential and a scratch repository and is run by
# `just smoke-real` alone, excluded by name rather than by `#[ignore]` so the
# journey is never a skipped test.
#
# `note-tier` drops `harness::`. The note binary shares `tests/e2e/harness.rs`
# through `#[path]`, exactly as the smoke binary does, so the harness's own
# twelve self-tests compile into it too — and they belong to the binary that
# owns the file. Selecting them here would run one set of tests twice in one
# tier for 4.8s and print every name twice.
rest-tier := "not binary(smoke) and not binary(note) and not binary(release_channel)"
note-tier := "binary(note) and not test(/^harness::/)"

# Their union, and the whole offline tier: what a runner that wants all of it
# asks for, spelled once from the two halves so it cannot drift from them.
offline-tiers := "(" + rest-tier + ") or (" + note-tier + ")"

# 95% line coverage is the gate; lower it only with a documented reason in
# AGENTS.md. It is measured over the **whole** offline tier, which is why the two
# runs below report nothing and one merge reports both: the note journeys are
# their own Nx project, and splitting the run must not split the floor.
# `--profraw-only` clears the profile set they share without touching the build
# artifacts they also share.
_crate-coverage-clean:
    @cargo llvm-cov clean --workspace --profraw-only

# The crate's own half of the offline suite, instrumented, reporting nothing.
_crate-test-rest:
    @just _strace-preflight
    @RUSTFLAGS="-D warnings" cargo llvm-cov --no-report nextest --locked -E '{{rest-tier}}' --final-status-level fail

# `--failure-mode all` is load-bearing, and belongs here rather than on either
# instrumented run: the merge is what this step does. The cancellation journeys
# kill instrumented processes, which leaves truncated `.profraw` files in the
# merge set, and `llvm-profdata`'s default rejects the whole merge over one of
# them — which this recipe then reports as a test failure. `all` refuses only when
# every profile is unmergeable; `tests/coverage.rs` plants one, so removing the
# flag fails the recipe rather than passing quietly.
_crate-coverage:
    @cargo llvm-cov report --failure-mode all --fail-under-lines 95 \
      || { echo "coverage fell below 95% — cover the lines the table above counts as missed" >&2; exit 1; }

# The `onepipeline-note-journeys` project's own targets, each scoped to the one
# test binary it owns. They are not the crate's targets narrowed for show: a
# project that declared the uniform set and ran nothing of its own would drop out
# of every repo-wide verb while appearing to be covered by it.
#
# `rustfmt` and `clippy` reach `tests/e2e/harness.rs` through this binary's
# `#[path]` include, which is right — it is part of what this binary compiles —
# and both are idempotent with the crate's own workspace-wide pass.
_note-build:
    @RUSTFLAGS="-D warnings" cargo build --locked --test note --quiet

_note-format:
    @rustfmt tests/note/main.rs

_note-fmt-check:
    @rustfmt --check tests/note/main.rs \
      || { echo "formatting drift above — run 'just format'" >&2; exit 1; }

_note-lint:
    @cargo clippy --locked --quiet --test note -- -D warnings

# Instrumented and reporting nothing, so `_crate-coverage` counts these journeys
# in the same floor as the rest of the suite.
_note-test:
    @RUSTFLAGS="-D warnings" cargo llvm-cov --no-report nextest --locked -E '{{note-tier}}' --final-status-level fail

# Coverage instrumentation is measured on Linux only, so the cross-platform CI
# legs run the same suite through this instead of `test`.
# The offline suite without coverage instrumentation.
test-quick:
    @just _strace-preflight
    @cargo nextest run --locked -E '{{offline-tiers}}'

# The one journey that is not offline: the real `onevcs`, real git against a real
# remote, and the real GitHub API opening and merging a pull request on a scratch
# repository. Deliberately outside `check` and `gate` — those stay offline and
# credential-free — and this is the same entry point CI's `smoke` job calls, so
# there is one definition of the journey rather than two.
#
# It needs `gh` and a credential (`gh auth login`, or GH_TOKEN). With neither it
# fails and names what is missing; it never skips and never falls back to a fake.
# Set ONEPIPELINE_SMOKE_REPO to publish somewhere other than the default scratch
# repository. `--no-capture`, because its whole value is the evidence it prints.
# Real everything: onevcs, git, and the GitHub API, over one whole lifecycle.
smoke-real:
    @cargo nextest run --locked -E 'binary(smoke)' --no-capture --status-level all

# Outside `check` and `gate` because uv fetches the wheel from a package
# registry; CI's `release-compat` job calls this, and without uv it fails naming
# it. `harness::` is the shared harness's own self-tests, which `test` already runs.
# The 0.28.2 channel journeys: this build's channel against the pinned wheel's.
release-compat:
    @RUSTFLAGS="-D warnings" cargo nextest run --locked -E 'binary(release_channel) and not test(/^harness::/)'

# Drives the compiled binary — never an in-process `main()`.
# The end-to-end binary journeys in isolation (also run by `test`/`check`),
# narrowed to the journeys a nextest filter names when one is given.
test-e2e filter="":
    @just _strace-preflight
    @cargo nextest run --locked -E 'binary(e2e){{ if filter == "" { "" } else { " and (" + filter + ")" } }}'

# Each journey starts a real two-party conversation and holds one side's turn
# open, which is what makes them their own binary and their own Nx project.
# The note delivery journeys in isolation (also run by `test`/`check`).
test-note:
    @cargo nextest run --locked -E '{{note-tier}}'

# Build the crate's docs with warnings denied.
_crate-doc:
    @RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked --quiet

# Run the CLI, e.g. `just run validate examples/graph.yaml`.
run *ARGS:
    cargo run --locked --quiet -- {{ARGS}}

# Upgrade dependencies, then re-run the full gate.
upgrade:
    @cargo update --quiet
    @npm update --silent --no-audit --no-fund
    @just check

# Separate from `check`: `cargo deny` needs a network-fetched advisory DB.
# Advisory + license audit and unused-dependency check.
deps-check:
    @command -v cargo-deny >/dev/null || { echo "cargo-deny not installed: cargo install cargo-deny --locked" >&2; exit 1; }
    @command -v cargo-machete >/dev/null || { echo "cargo-machete not installed: cargo install cargo-machete --locked" >&2; exit 1; }
    @cargo deny --log-level error check
    @# machete prints the unused deps it finds on stdout, so keep it: hiding
    @# them would leave a failing gate with no actionable detail.
    @cargo machete

# llmlint: ignore-block[comments_earn_their_place] the line above a recipe is not a
# comment on it — `just` prints it as that recipe's description, and `just --list`
# is this repository's documented index of its command surface. Read as prose each
# one restates the name below it, which is what a one-line help string does;
# deleting them empties the index rather than tightening it.

# Both are outside `check` for the reason `deps-check` is: they read the
# crates.io index, and the deterministic gate stays offline. The split half of
# this check needs no index and `tests/linked_engines.rs` reaches it there, so
# `check` covers that rule without the network.
# Fail when Cargo.lock resolves a sibling engine older than Cargo.toml admits, or twice; say which newer release a pin holds back.
engines-current:
    @bash scripts/linked-engines.sh --format check

# Compose the release note recording the engine versions this build links.
linked-engines:
    @bash scripts/linked-engines.sh --format notes
# llmlint: ignore-end[comments_earn_their_place]

# Reads the floor from Cargo.toml's `rust-version`; that toolchain must be
# installed (`rustup toolchain install <version>`). Warnings are errors here too.
# Build under the declared MSRV.
msrv:
    @RUSTFLAGS="-D warnings" cargo +{{msrv-version}} check --locked --all-targets --quiet \
      || { echo "the {{msrv-version}} floor no longer builds — install that toolchain, or raise rust-version in Cargo.toml (and clippy.toml)" >&2; exit 1; }

# Ensures `just`, verifies the rest, then runs setup-llmlint. Runs automatically
# via the Claude Code SessionStart hook; this is the manual entry point.
# Provision the dev toolchain for a session. Idempotent, no-ops in CI.
session-setup:
    ./scripts/session-setup.sh

# Install/refresh the llmlint toolchain (oneharness + llmlint). Idempotent.
setup-llmlint:
    ./scripts/setup-llmlint.sh

# Out of the gate for the reason `deps-check` is: these need third-party tools
# `just check` deliberately does not install.

# Install the pinned capture renderer (`freeze`) on demand. Needs Go.
screenshots-tools:
    @command -v go >/dev/null || { echo "go not found: needed to install freeze; see https://go.dev/dl" >&2; exit 1; }
    go install github.com/charmbracelet/freeze@v{{freeze-version}}
    @echo "installed freeze to $(go env GOPATH)/bin (ensure it is on PATH)"

# Capture the screenshots: build the binaries, drive the real CLI against the
# offline fixtures, render each scene to shots/current/<arch>/ and
# screenshots/images/. Needs `freeze` on PATH (`just screenshots-tools`).
screenshots:
    @bash scripts/nx.sh run onepipeline-visual-docs:screenshots

# Regenerate the animated README hero (screenshots/images/demo.gif): the same
# real binary against the same offline fixtures, rendered frame by frame with
# the same vendored font. Informational and deliberately NOT hash-gated — a GIF
# is not byte-reproducible across Pillow versions — so it is regenerated on
# demand and committed. Needs Python 3 + Pillow (`pip install Pillow`).
screenshots-gif:
    @command -v python3 >/dev/null || { echo "python3 not found: needed to render the demo GIF" >&2; exit 1; }
    @python3 -c "import PIL" 2>/dev/null || { echo "Pillow not installed: pip install Pillow" >&2; exit 1; }
    RUSTFLAGS="-D warnings" cargo build --release --locked --bin onepipeline
    RUSTFLAGS="-D warnings" cargo build --release --locked --package onevcs --bin onevcs
    RUSTFLAGS="-D warnings" cargo build --release --locked --package onepipeline-testfakes
    python3 screenshots/demo-gif.py

# Run the pre-push visual guard exactly as git would — `GIT_DIR` in the
# environment, the pushed refs on stdin — without pushing anything. The guard is
# the only caller of the capture that runs inside a hook, and a hook's
# environment is not a shell's, so this is how a defect that only appears there
# is found before a push rather than by a rejected one.
screenshots-guard:
    @bash screenshots/rehearse-guard.sh

# Refresh the committed digest baseline from a fresh capture, after an INTENDED
# output change. Rewrites this host's lane only (screenshots/host-arch.sh names it,
# and the pre-push guard classifies the same one); commit shots/baseline/ and
# screenshots/images/ together.
screenshots-bless:
    @bash scripts/nx.sh run onepipeline-visual-docs:screenshots-bless

# The `onepipeline-visual-docs` project's own target bodies. The capture is its
# own project rather than a target of the CLI application: it is slow, it needs
# two third-party tools the gate does not install, and hanging it off the
# application would put it behind that project's whole dependency surface.
# Through Nx so its inputs are declared in the build graph — `visualDocsSource`
# in nx.json enumerates every path that can change a rendered shot — rather than
# being a command nothing knows the inputs of.
_visual-docs-capture:
    @bash screenshots/capture.sh

_visual-docs-bless:
    @bash screenshots/bless.sh
    @echo "baseline refreshed for the $(bash screenshots/host-arch.sh) lane; commit shots/baseline/ + screenshots/images/"

# Kept OUT of `check` on purpose: the deterministic gate stays offline and
# credential-free. Config is the composed `llmlint.yml`.
# LLM-judge lint — the non-deterministic, harness-backed tier.
lint-llm *paths:
    @command -v llmlint >/dev/null 2>&1 || { echo "llmlint not installed — run 'just setup-llmlint'" >&2; exit 1; }
    llmlint {{paths}}

# CI runs this before the model tier so a broken config fails in milliseconds
# instead of spending a harness call.
# Fast, deterministic llmlint gate — no model calls, no harness credential.
lint-llm-validate *args:
    @command -v llmlint >/dev/null 2>&1 || { echo "llmlint not installed — run 'just setup-llmlint'" >&2; exit 1; }
    llmlint validate {{args}}

# One verdict per tree, base commit, and judge configuration, rather than a fresh
# roll of a non-deterministic judge every time. `scripts/llmlint-diff.sh` holds
# that contract, including the one supported way to force a re-judge.
# The blocking `llmlint` PR check; `just gate` runs it before you push.
# llmlint scoped to the files this branch changed since it forked from main.
lint-llm-diff base="origin/main" *options:
    @bash scripts/llmlint-diff.sh "$@"
