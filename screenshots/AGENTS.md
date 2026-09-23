# Terminal screenshots

Deterministic SVGs of the **real** CLI's output, gated on their content hash by
[screencomp](https://github.com/nickderobertis/screencomp), plus one animated GIF
that is deliberately not gated. Informational, like `deps-check` — **never part
of `just check`, `just gate`, or `ci.yml`'s `gate` job**.

> `CLAUDE.md` is a symlink to this file — edit `AGENTS.md` only.

## The scenes

One per surface the README explains, in the section that explains it. A shot
that would say less than the sentence beside it is left out rather than padded
in, which is why `agents` (the offline fixture runs no real `oneharness`, so it
would read `no sessions recorded`), `transcript`, `goals`, `host` and
`channel queue` are absent.

| scene | why it documents that surface |
| --- | --- |
| `demo.gif` | `start --attach` is the tool's main usage, and the arrival of those lines **is** the experience of it |
| `status` | the richest static view, and the only one answering "what is happening right now" |
| `watch` | the README spends ~30 lines on its three line kinds — event, heartbeat, return |
| `monitor` | the other ~30. Not progressive: one pass writes the whole document, its trailer and its resume line |
| `results` | the one view showing what a finished DAG produced, with each node's own evidence |
| `runs` | the paragraph claiming a listing groups its runs by project |
| `telemetry` | the eight buckets and the per-party cost — real numbers, from the recorded layer below |
| `help` | the whole verb surface, which the README never lists, and the only colourised view this tool has |

**No colour beyond that one.** Nothing in this stack emits an ANSI escape
outside `clap`'s own help. The shots are monochrome because the output is.

## Two fixture layers

**Driven** — `examples/plan-store`'s `tracked-release`, copied byte for byte as
`tests/e2e/shipped.rs` copies it and executed by the real compiled binary: six
nodes, two `kind: human` approvals, a lifecycle node with a human step, two real
repositories on real bare origins. Feeds `status`, `watch`, `results`, `monitor`,
`runs` and the GIF — the views that exist to show a graph have to show one.

**Recorded** — `tests/recorded/run-root/onemessagebus-repair-2`, read exactly as
`tests/parity.rs` reads it. Feeds `telemetry --breakdown` **and nothing else**:
its buckets are a real run's 41 minutes and its cost a real run's $5.35, which no
offline drive of a scripted double can be. It is a *one-node* run, which is why
there are two layers at all.

**Nothing costs anything.** Real binary, real `onetaskgraph`, real linked
`onevcs` over real git. Two subprocesses are substituted, and they are the two
this repository's own offline tier substitutes: `fake-oneagentgraph` at
`ONEPIPELINE_ONEAGENTGRAPH_BIN` and `fake-gh` at `onevcs`'s `ONEVCS_GH`. No model
call, no harness turn, no network, no credential, no real GitHub.

**Every wait polls a file** — `tests/AGENTS.md`'s one rule for this repository's
suite, and a capture that slept to let output settle is what that note forbids.
The two lifecycle dispatches sit in the doubles' own `<key>.wait`/`<key>.go` hold
and are released one at a time, each waited out on the journal, so the order the
run settles in is stated here rather than decided by the host; and the `watch`
scene's heartbeat count is decided by reading `watch`'s own output file.

## What the bytes are pinned to

- **The renderer** — `freeze`, at the justfile's `freeze-version` line, which is
  the **only** place this repository names it: `just screenshots-tools` installs
  from it and `install-freeze.sh` reads that same line for CI.
- **The font** — `fonts/JetBrainsMono-Regular.ttf` (OFL), vendored, embedded into
  each SVG as base64, named once in `capture.py`. Nothing is fetched.
- **The lane** — `[capture].arches` in `screencomp.toml` is the only place a lane
  is declared and `host-arch.sh` the only place this host's is derived. One lane,
  `x86_64`: an SVG is layout maths, so its bytes are identical on every CPU and a
  second lane would prove the same bytes twice. The guard still classifies the
  host's own lane and refuses an undeclared one, so an `arm64` developer adds
  that lane and blesses it once.
- **The environment** — `world.py` clears every ambient `ONEPIPELINE_*`,
  `ONEVCS_*`, `ONETASKGRAPH_*`, `ONEAGENTGRAPH_*`, `ONEHARNESS_*`, `ONEJUDGE_*`
  and `ONEMESSAGEBUS_*` before setting its own. Each is read ahead of most other
  layers, so an exported one prints into a shot or moves the whole journey.

Everything per-run is normalised in the capture instead: `normalise.py` is the
list — the instant on each line, the pid in an agent stream id, the session token
`onevcs` mints, the driver's pid, the scratch world's paths, this machine's free
space, a clock-derived age, and a cursor's journal byte offset.

**No clock override, deterministic-id switch, `--no-color`, `--width` or demo
flag may be added for a capture's convenience.** The CLI surface is the approved
`docs/contract.md`'s and the code is written to match it, so such a flag is a
contract change. That is why the normalisation is here.

## Why this is not a second statement of the contract

`docs/contract.md` is the authority for the CLI surface and the judged lint's
`contracts_have_one_source_or_a_drift_gate` fires on a second spelling of one. A
committed screenshot of CLI output would be such a spelling **if it were
hand-made**. It is not: every image is what the real binary printed, and CI
refuses the change the moment its bytes leave `shots/baseline/<arch>.json`. The
baseline is the drift gate and the binary stays the one source — an image cannot
say what the binary does not, because one that did is a red check.

For the same reason, a capture that contradicted the contract is a **contract bug
to report**, not a shot to adjust: the disagreement is between the binary and the
document.

## Commands

| command | what it does |
| --- | --- |
| `just screenshots-tools` | install the pinned `freeze` (needs Go) |
| `just screenshots` | capture: build, drive, render to `shots/current/<arch>/` and `images/` |
| `just screenshots-gif` | regenerate `images/demo.gif` (needs Python 3 + Pillow) |
| `just screenshots-bless` | after an **intended** output change: recapture and rewrite this host's lane |

`just screenshots` is the `onepipeline-visual-docs` project's own target, so its
inputs are declared (`visualDocsSource` in `nx.json`) and a slow check needing two
third-party tools sits behind its own edge rather than inside the CLI
application's dependency surface. That project declares no `check`, `build` or
`test` target because it has none — declaring the uniform set and running nothing
would drop it out of every repo-wide verb while appearing covered by it.

Committed: `shots/baseline/<arch>.json`, `images/*.svg`, `images/demo.gif`.
Gitignored: `shots/current/`, `shots/review/`, `shots/verify/`. **No image lives
under `docs/`**: that directory holds the contract and its divergences, both
machine-consumed and both named in a cached build input.

## The gate, and activating its local half

CI fails on drift (`fail-on-drift: true`). `.githooks/pre-push` is the local
half: it re-captures only when a `[guard].paths` file changes, and on drift
regenerates this lane, builds `shots/review/index.html`, and **blocks the push**.

`core.hooksPath` is per-clone state and is never committed, so a committed guard
that nothing activates runs nothing. **`just bootstrap` activates it** — it
depends on `_hooks`, which sets `core.hooksPath` to `.githooks`, and
`tests/provisioning.rs` drives that recipe over a fresh clone and asserts the
result. `screencomp doctor --env` reports whether this clone has it.

The directory carries the visual guard **and nothing else**: `just gate` stays
unhooked, which is how this repository already was, and nothing was displaced
because it installed no git hook before.

## When the output legitimately changes

Editing what a verb prints, the CLI surface, the order the engine writes events
in, the shipped example plan, the doubles, or the capture will move the SVGs.
Run `just screenshots-bless` and commit the baseline with `images/`. Bumping
`freeze-version` or the font reflows every shot — bless once.

The GIF is **not** hash-gated, so nothing tells you when it has gone stale:
regenerate it with `just screenshots-gif` whenever what the attached stream
prints changes — the kinds in `src/event.rs`, the line `src/views.rs` renders one
as, or the plan the journey drives.
