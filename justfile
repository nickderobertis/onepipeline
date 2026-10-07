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
# _crate-coverage-clean, _crate-coverage, _crate-msrv) add one explicitly.

# The store every plan is read out of is linked into this crate (`onetaskgraph-core`
# in `[workspace.dependencies]`), so cargo builds it with everything else and the
# release is the one `Cargo.lock` resolves. The `onetaskgraph` **binary** is installed
# for one reason only: the template journeys (`tests/e2e/templates.rs`) pipe
# `onepipeline template resolve --json` into the released `onetaskgraph task create
# --template-loader -`, which is the seam those journeys prove. It is installed at this
# release into this clone's own build directory, never onto `PATH`, and
# `tests/linked_engines.rs` fails if this line ever names a release other than the one
# the lock links.
onetaskgraph-version := "0.3.2"
onetaskgraph-root := justfile_directory() / "target" / "tools" / ("onetaskgraph-" + onetaskgraph-version)

# The renderer the visual-docs capture draws each scene with (`just screenshots`).
# NOT part of `check`, `gate` or `bootstrap`: screenshots are informational, and
# this is the only version of `freeze` a capture of this repository is ever taken
# with.
#
# **This is the only place the release is named.** `just screenshots-tools`
# installs it from here, and CI installs it through `screenshots/install-freeze.sh`,
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
    @just _ensure-tool onebudgetspec
    @just _ensure-strace
    @just _ensure-onetaskgraph
    @cargo fetch --locked --quiet

# The tracer the Linux-only e2e journeys (`agents.rs`, `channel.rs`, `listing.rs`,
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
      || { echo "strace not installed — the Linux-only e2e journeys in tests/e2e/agents.rs, channel.rs, listing.rs and unwatched.rs run the binary under it and refuse without it: sudo apt-get install -y strace (or your distribution's strace package), then re-run" >&2; exit 1; }

# The released `onetaskgraph` the template journeys drive, at the release the lock
# links. Network, so it is installed by `bootstrap` and only asked for by the tiers.
_ensure-onetaskgraph:
    @[ -x "{{onetaskgraph-root}}/bin/onetaskgraph" ] || [ -x "{{onetaskgraph-root}}/bin/onetaskgraph.exe" ] \
      || cargo install onetaskgraph --locked --quiet --version {{onetaskgraph-version}} --root "{{onetaskgraph-root}}" \
      || { echo "onetaskgraph {{onetaskgraph-version}} could not be installed into {{onetaskgraph-root}}; the template journeys refuse without it" >&2; exit 1; }

# Asked ahead of the tiers that hold the template journeys, so the missing binary is
# named up front rather than by a panic inside a journey.
_onetaskgraph-preflight:
    @[ -x "{{onetaskgraph-root}}/bin/onetaskgraph" ] || [ -x "{{onetaskgraph-root}}/bin/onetaskgraph.exe" ] \
      || { echo "onetaskgraph {{onetaskgraph-version}} is not installed — the template journeys in tests/e2e/templates.rs drive it and refuse without it: run 'just bootstrap' (or 'just _ensure-onetaskgraph'), then re-run" >&2; exit 1; }

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
# that is measured on Linux alone, so the suite runs through every project's
# `test-quick` instead, and `doc` is a link check no second platform can answer
# differently.
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

# The offline tier, in the four parts its four projects run: the crate's unit
# tests and remaining integration binaries (`onepipeline`), the `e2e` binary
# (`onepipeline-e2e`), the `contract` binary (`onepipeline-contract`), and the
# `note` binary (`onepipeline-note-journeys`). `smoke` and `release_channel` are
# in none: they reach GitHub and PyPI, and each is run by its own project's
# uncached target (`just smoke-real`, `just release-compat`) alone, excluded by
# name rather than by `#[ignore]` so neither journey is ever a skipped test.
#
# `rest-tier` is spelled as the complement of every other binary, so a test
# binary added later lands in the crate's tier rather than in none.
#
# `note-tier` drops `harness::`. The note binary shares `tests/e2e/harness.rs`
# through `#[path]`, exactly as the smoke binary does, so the harness's own
# twelve self-tests compile into it too — and they belong to the binary that
# owns the file, which `e2e-tier` runs. Selecting them here would run one set of
# tests twice for 4.8s and print every name twice.
#
# `adoption` stays inside `e2e-tier` although it is that binary's most expensive
# module: a tier of its own would compile the same binary from the same inputs,
# so no change could ever select one without the other, and running the two as
# separate nextest processes would put eight process trees on the machine where
# `.config/nextest.toml`'s one group allows four.
rest-tier := "not binary(smoke) and not binary(release_channel) and not binary(note) and not binary(e2e) and not binary(contract)"
e2e-tier := "binary(e2e)"
contract-tier := "binary(contract)"
note-tier := "binary(note) and not test(/^harness::/)"

# Their union, and the whole offline tier, spelled once from the four parts so it
# cannot drift from them: what a check that the tiers partition the suite lists
# against (`cargo nextest list -E "$(just --evaluate offline-tiers)"`).
offline-tiers := "(" + rest-tier + ") or (" + e2e-tier + ") or (" + contract-tier + ") or (" + note-tier + ")"

# 95% line coverage is the gate; lower it only with a documented reason in
# AGENTS.md. It is measured over the **whole** offline tier, which is why the four
# tier runs below report nothing and one merge reports them all: each tier is its
# own Nx project, and splitting the run must not split the floor.

# cargo-llvm-cov's instrumented tree; `tests/coverage.rs` holds this path against
# `LLVM_PROFILE_FILE`, so a drift fails there rather than cleaning the wrong tree.
llvm-cov-target-dir := justfile_directory() / "target" / "llvm-cov-target"
export ONEPIPELINE_COVERAGE_ROOT := justfile_directory()

# Remove the tree before any test tier: a stale binary retains a coverage map
# and would count as uncovered in the later report.
#
# Resolve existing arguments before removal so symlinks cannot reach outside
# this clone's build directory. A missing tree is normal on the first run, even
# with no build directory yet, so it is bounded through its nearest existing
# ancestor; existing paths that cannot be entered fail instead of silently passing.
# llmlint: ignore-block[cli_output_contract] Both invalid-path refusals are
# one error class to the caller, so both exit 1 like adjacent _crate-coverage;
# distinct stderr names the invalid input that needs correcting.
_crate-coverage-clean dir=llvm-cov-target-dir:
    @if [ -z "$1" ]; then echo "refusing to clean an empty target path" >&2; exit 1; fi; \
      root="$(cd -P -- "$ONEPIPELINE_COVERAGE_ROOT" && pwd -P)"; \
      if [ "$(pwd -P)" != "$root" ]; then echo "refusing to clean from outside this clone's root ($root)" >&2; exit 1; fi; \
      built="$root/target"; \
      if [ ! -e "$1" ] && [ ! -L "$1" ]; then \
        base="$1"; rest=""; \
        while [ ! -e "$base" ] && [ ! -L "$base" ]; do \
          part="$(basename -- "$base")"; \
          if [ "$part" = ".." ]; then echo "refusing to clean '$1': it climbs out of a directory that does not exist" >&2; exit 1; fi; \
          rest="/$part$rest"; base="$(dirname -- "$base")"; \
        done; \
        reached_base="$(cd -P -- "$base" && pwd -P)" || { echo "refusing to clean '$1': its nearest existing ancestor cannot be entered" >&2; exit 1; }; \
        case "$reached_base$rest" in "$built"/?*) exit 0;; \
          *) echo "refusing to clean '$1': it is outside this clone's build directory ($built)" >&2; exit 1;; \
        esac; \
      fi; \
      reached="$(cd -P -- "$1" && pwd -P)" || { echo "refusing to remove '$1': it is there but is not a directory this step can enter, so where it leads cannot be held against this clone's build directory — nothing was removed" >&2; exit 1; }; \
      case "$reached" in "$built"/?*) ;; \
        *) echo "refusing to remove '$reached': the instrumented tree has to sit under this clone's build directory ($built), which is where .cargo/config.toml pins every build in it — nothing was removed" >&2; exit 1;; \
      esac; \
      rm -rf -- "$reached" || { echo "could not remove instrumented tree '$reached'" >&2; exit 1; }
# llmlint: ignore-end[cli_output_contract]

# One tier of the offline suite, instrumented, reporting nothing. Each tier names
# its profiles after itself, and its Nx `test` target declares exactly those
# files as its outputs: a tier replayed from the cache then restores the profiles
# the merge below reads, and only its own, beside whatever the tiers that ran
# wrote. `cargo llvm-cov report` merges every `*.profraw` in the tree, so the
# name changes nothing about what is counted.
_tier-test tier filter:
    @LLVM_PROFILE_FILE_NAME="onepipeline-$1-%p-%14m.profraw" RUSTFLAGS="-D warnings" cargo llvm-cov --no-report nextest --locked -E "$2" --final-status-level fail

# The instrumented build every tier runs, archived once, ahead of them all, when
# CI names where (the `onepipeline:test-archive` target every tier's `test`
# depends on). Built under the same flags the tiers build with, so each finds
# it fresh and the archive is the build the step tested.
_crate-test-archive:
    @[ -z "${ONEPIPELINE_TEST_ARCHIVE:-}" ] || RUSTFLAGS="-D warnings" cargo llvm-cov --no-report nextest-archive --locked --archive-file "$ONEPIPELINE_TEST_ARCHIVE"

# The crate's own tier: its unit tests and the integration binaries no other
# project owns.
_crate-test-rest: (_tier-test "rest" rest-tier)

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

# The test-tier projects' own targets (`onepipeline-e2e`, `onepipeline-contract`,
# `onepipeline-note-journeys`, `onepipeline-smoke`, `onepipeline-release-compat`),
# each scoped to the one test binary it owns. They are not the crate's targets
# narrowed for show: a project that declared the uniform set and ran nothing of
# its own would drop out of every repo-wide verb while appearing to be covered by
# it.
#
# `rustfmt` and `clippy` reach `tests/e2e/harness.rs` through the `#[path]`
# include in the note, smoke and release_channel binaries, which is right — it
# is part of what each compiles — and both are idempotent with the crate's own
# workspace-wide pass.
_tier-build bin:
    @RUSTFLAGS="-D warnings" cargo build --locked --test "$1" --quiet

_tier-format path:
    @rustfmt "$1"

_tier-fmt-check path:
    @rustfmt --check "$1" \
      || { echo "formatting drift above — run 'just format'" >&2; exit 1; }

_tier-lint bin:
    @cargo clippy --locked --quiet --test "$1" -- -D warnings

# Each tier's instrumented run, counted by `_crate-coverage` in the one floor.
# The e2e journeys are the ones that run the binary under `strace` and drive the
# released `onetaskgraph`, so that tier asks for both up front.
_e2e-test: _strace-preflight _onetaskgraph-preflight (_tier-test "e2e" e2e-tier)

_contract-test: (_tier-test "contract" contract-tier)

_note-test: (_tier-test "note" note-tier)

# Coverage instrumentation is measured on Linux only, so the cross-platform CI
# legs run the same suite through every project's `test-quick` target instead of
# `test`, one project at a time (each declares `parallelism: false`), which is the
# one nextest process at a time the suite ran as before it was split.
# The offline suite without coverage instrumentation.
test-quick:
    @ONEPIPELINE_NX_SHOW_OUTPUT=1 bash scripts/nx.sh run-many -t test-quick --outputStyle=stream-without-prefixes

# Streamed rather than summarised, and without Nx's per-line project prefix: the
# cross legs report a line per test as it finishes so that a leg cut short names
# where it stopped, and `rerun-failed` reads nextest's status lines off the
# start of each line.
_tier-test-quick filter:
    @bash scripts/nextest-run.sh cargo nextest run --locked -E "$1"

_crate-test-quick: (_tier-test-quick rest-tier)

_e2e-test-quick: _strace-preflight _onetaskgraph-preflight (_tier-test-quick e2e-tier)

_contract-test-quick: (_tier-test-quick contract-tier)

_note-test-quick: (_tier-test-quick note-tier)

# The uninstrumented build every project's `test-quick` runs, archived once,
# ahead of them all (the `onepipeline:test-quick-archive` target).
# llmlint: ignore-block[diagnostics_error_or_absent] the archive line has to build
# exactly what the runs after it build, so that each run finds it fresh and the
# archive is the build the step tested; the runs set no RUSTFLAGS, so neither
# can the archive. Warnings are denied by `just lint`, which `check-cross` runs
# before this.
_crate-test-quick-archive:
    @[ -z "${ONEPIPELINE_TEST_ARCHIVE:-}" ] || cargo nextest archive --locked --archive-file "$ONEPIPELINE_TEST_ARCHIVE"
# llmlint: ignore-end[diagnostics_error_or_absent]

# `ONEPIPELINE_TEST_ARCHIVE` (CI sets it; nothing local does) makes the
# `test-archive` and `test-quick-archive` targets archive the build the tiers are
# about to run, with nextest's own archive command, before any of them runs.
# Each run then finds that build fresh, so the archive is the build the step
# tested, and it holds every tier's binaries, so a test any tier failed can be
# re-run from it. `rerun-failed` re-runs from that archive and nothing else:
# nextest refuses cargo's build options beside `--archive-file`, so a re-run
# cannot compile.

# How many times `rerun-failed` re-runs each test a failed CI test step failed.
# Three tells "fails every time" from "fails some of the time" and stays bounded.
# A test that hangs every time is ended at `.config/nextest.toml`'s 360-second
# `terminate-after`, and each re-run first extracts the archive, allowed two
# minutes. Three of those are 24 minutes, inside the 25 `ci.yml` gives the step.
rerun-times := "3"

# What CI runs once a test step has failed: why any tests went unrun, then how
# often each failed test fails again on the build that step archived. Reports
# only; it never turns a failed run green. `scripts/known-flakes.txt` annotates
# and excuses nothing.
# Re-run a failed step's failed tests from the build it archived, given its LOG and ARCHIVE.
rerun-failed log archive:
    @bash scripts/rerun-failed.sh --log "$1" --archive "$2" --times {{rerun-times}} --known-flakes scripts/known-flakes.txt -- cargo nextest run

# The same for the gate's instrumented suite, whose archive `_crate-test-archive`
# makes with cargo-llvm-cov so the re-runs run under its coverage environment.
# cargo-llvm-cov extracts an archive into its own target directory every time,
# so each re-run after the first overwrites the one before.
# llmlint: ignore-block[changed_behavior_has_e2e] the script behind this recipe is
# driven end to end by tests/nextest_runs.rs; what only this recipe adds is the
# instrumented runner. A test of that needs cargo-llvm-cov, which the cross legs
# that run every test do not install, so it could only pass there by skipping.
# The gate job runs this recipe for real whenever one of its tests fails.
# Re-run a failed gate step's failed tests from its instrumented archive, given its LOG and ARCHIVE.
rerun-failed-coverage log archive:
    @RUSTFLAGS="-D warnings" bash scripts/rerun-failed.sh --log "$1" --archive "$2" --times {{rerun-times}} --known-flakes scripts/known-flakes.txt -- cargo llvm-cov --no-report nextest --extract-overwrite
# llmlint: ignore-end[changed_behavior_has_e2e]

# The one journey that is not offline: the real `onevcs`, real git against a real
# remote, and the real GitHub API opening and merging a pull request on a scratch
# repository. Deliberately outside `check` and `gate` — those stay offline and
# credential-free — and this is the same entry point CI's `smoke` job calls, so
# there is one definition of the journey rather than two.
#
# It needs `gh` and a credential (`gh auth login`, or GH_TOKEN). With neither it
# fails and names what is missing; it never skips and never falls back to a fake.
# Set ONEPIPELINE_SMOKE_REPO to publish somewhere other than the default scratch
# repository. `--no-capture`, because its whole value is the evidence it prints,
# and streamed through Nx for the same reason. It runs the `onepipeline-smoke`
# project's uncached `smoke` target, which no `check` reaches.
# Real everything: onevcs, git, and the GitHub API, over one whole lifecycle.
smoke-real:
    @ONEPIPELINE_NX_SHOW_OUTPUT=1 bash scripts/nx.sh run onepipeline-smoke:smoke --outputStyle=stream-without-prefixes

# The `smoke` target's body. Arguments are passed through to nextest, so the
# target's wiring can be shown without the credential: `just nx run
# onepipeline-smoke:smoke --no-run` builds the binary and runs nothing.
_smoke-run *args:
    @cargo nextest run --locked -E 'binary(smoke)' --no-capture --status-level all "$@"

# Outside `check` and `gate` because uv fetches the wheel from a package
# registry; CI's `release-compat` job calls this, and without uv it fails naming
# it. It runs the `onepipeline-release-compat` project's uncached target, which
# no `check` reaches.
# The 0.28.2 channel journeys: this build's channel against the pinned wheel's.
release-compat:
    @bash scripts/nx.sh run onepipeline-release-compat:release-compat

# The `release-compat` target's body. `harness::` is the shared harness's own
# self-tests, which the e2e tier already runs. Arguments are passed through to
# nextest, as `_smoke-run`'s are.
_release-compat-run *args:
    @RUSTFLAGS="-D warnings" cargo nextest run --locked -E 'binary(release_channel) and not test(/^harness::/)' "$@"

# Drives the compiled binary — never an in-process `main()`.
# The end-to-end binary journeys in isolation (also run by `test`/`check`),
# narrowed to the journeys a nextest filter names when one is given.
test-e2e filter="":
    @just _strace-preflight
    @just _onetaskgraph-preflight
    @cargo nextest run --locked -E '{{e2e-tier}}{{ if filter == "" { "" } else { " and (" + filter + ")" } }}'

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

# Separate from `check`: `cargo deny` needs a network-fetched advisory DB. A
# repo-level target of the crate's project (`onepipeline:deps-check`), uncached,
# because what it reads is the advisory database as of now.
# Advisory + license audit and unused-dependency check.
deps-check:
    @bash scripts/nx.sh run onepipeline:deps-check

# The `deps-check` target's body.
_crate-deps-check:
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

# Outside `check` and `gate` because it pulls the manylinux image and a toolchain
# over the network and needs Docker. release.yml's Linux `build-wheels` legs and
# ci.yml's `wheel` legs both call this, so there is one Linux wheel build.
# Build the Linux wheel for TARGET inside the release's manylinux image, into OUT.
wheel-linux target out="dist":
    @bash scripts/build-linux-wheel.sh --target {{quote(target)}} --out {{quote(out)}}
# llmlint: ignore-end[comments_earn_their_place]

# Reads the floor from Cargo.toml's `rust-version`; that toolchain must be
# installed (`rustup toolchain install <version>`). Warnings are errors here too.
# A repo-level target of the crate's project (`onepipeline:msrv`), uncached,
# because its answer is the installed toolchain's as much as the tree's.
# Build under the declared MSRV.
msrv:
    @bash scripts/nx.sh run onepipeline:msrv

# The `msrv` target's body.
_crate-msrv:
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
