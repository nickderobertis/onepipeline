# Terminal screenshots

Deterministic SVGs of `onepipeline`'s **real** output, gated on their content
hash by [screencomp](https://github.com/nickderobertis/screencomp), plus one
animated GIF that is deliberately not gated. Informational, like `deps-check`
and `engines-current` — **never part of `just check`, `just gate`, or `ci.yml`'s
`gate` job**; `.github/workflows/visual-docs.yml` owns the comparison on pull
requests, and `.githooks/pre-push` is its local half.

> `CLAUDE.md` is a symlink to this file — edit `AGENTS.md` only.

## The scenes, and why each one

One scene per surface the README explains, placed in the section that explains
it. Every one is real output of the real binary; none is hand-made.

| scene | what it shows | why it earns its place |
| --- | --- | --- |
| `demo.gif` | `start --attach` driving the six-node plan from launch to `6/6 done SETTLED complete` | the tool's main usage, and the arrival of those lines **is** the experience of it. A still of the same text says strictly less |
| `status` | a live run with two lifecycle dispatches in flight, each with its event count and age, beside the run's free space and provider health | the richest static view the tool has, and the only one that answers "what is happening right now" |
| `watch` | a completed bounded wait: event lines, a heartbeat tick, and the `-- watch … cursor …` return line | the README spends some thirty lines describing exactly these three kinds of line |
| `monitor` | the whole concise event document, its trailer, and its resume line | the other thirty. It is **not** progressive: one pass writes the whole document and returns |
| `results` | six nodes' outcomes with each one's evidence — a change request's URL, a `no-changes` settlement, an undecided landing | the one view that shows what a finished DAG actually produced |
| `runs` | two projects, each with its own header line and its run beneath it | the paragraph claiming a listing groups its runs by project |
| `telemetry` | the eight wall-clock buckets that sum exactly, and the per-party usage and cost | numbers no offline drive can produce; see the fixture layers below |
| `help` | the whole verb surface, one line each | the README documents the verbs it explains and never lists them; this is also the **only** colourised surface this tool has |

**Deliberately absent.** `agents` would need a real `oneharness` under a real
`oneagentgraph` to write the pointer lines it reads — the offline fixture runs
neither, so the shot would read `no sessions recorded`, which says less than the
paragraph beside it. `transcript`, `goals`, `host` and `channel queue` are each
a shape the prose already states in fewer words than the picture would take.
A shot that would say less than the sentence it sits beside is left out rather
than padded in.

**No colour, honestly.** Nothing in this stack emits an ANSI escape outside
`clap`'s own help: there is no colour or TUI crate anywhere in it. These shots
are therefore monochrome, which is what a terminal shows. Nothing here adds
colour the tool does not print.

## Two fixture layers, and which scene each one feeds

**The driven layer — `examples/plan-store`'s `tracked-release`.** Copied byte for
byte into a scratch world exactly as `tests/e2e/shipped.rs` copies it, and
executed by the real compiled binary: six nodes, two `kind: human` approvals, a
lifecycle node carrying a human step, and two real repositories on real bare
origins. `status`, `watch`, `results`, `monitor`, `runs` and the GIF come from
here, because the views that exist to show a graph have to show one.

**The recorded layer — `tests/recorded/run-root/onemessagebus-repair-2`.** A
finished run a real release drove, checked into this repository byte for byte and
read exactly the way `tests/parity.rs` reads it: a copy under a runs root of its
own, told it is on the recording host, under the launching session its launch
record names, with the driver's pid rewritten to one no platform issues so it
reads as finished here, and the sibling pointed at an executable that is not
there so the provider-health probe stays silent. **`telemetry --breakdown` comes
from here and nowhere else**: its buckets are a real run's 41 minutes and its
cost is a real run's $5.35, and an offline drive of a scripted double cannot be
either. It is a *one-node* run, which is the whole reason there are two layers:
`status` and `results` over it are three lines and would be a poor advertisement
for a DAG engine.

## Nothing costs anything, and nothing is mocked that matters

The capture drives the **real** `onepipeline` binary as a subprocess, reads its
plan through the **real** released `onetaskgraph` binary, and publishes through
the **real** `onevcs` linked into the binary, against real git repositories with
real bare origins on disk. Two subprocesses are substituted, and they are the two
this repository's own offline tier substitutes, at the two boundaries that would
otherwise cost a paid model turn or a network call:

- `ONEPIPELINE_ONEAGENTGRAPH_BIN` → `fake-oneagentgraph`
- `onevcs`'s own `ONEVCS_GH` → `fake-gh`

both built from `crates/testfakes`. **No model call, no harness turn, no network,
no credential, no real GitHub.**

**Every wait polls a file.** `tests/AGENTS.md` carries exactly one rule for this
repository's suite — a wait polls files, and a wall-clock deadline is the
backstop for the product's own asynchrony rather than the signal — and a capture
that slept to let output settle is precisely what that note forbids. The two
lifecycle dispatches are held open by the doubles' own `<key>.wait` /`<key>.go`
hold and released one at a time, each release waited out on the journal before
the next, so the order the run settles in is stated by the capture rather than
decided by the host. The `watch` scene's heartbeat count is decided by reading
`watch`'s own output file until the tick it is a picture of has been printed.

## Why it is byte-reproducible, and what it is pinned to

screencomp gates on the **hash** of each image, so two captures of one build have
to be identical. Unlike a rasterised PNG, whose anti-aliasing drifts across CPUs,
an SVG is pure layout maths. Three things are pinned and one thing is normalised:

- **The renderer.** [`freeze`](https://github.com/charmbracelet/freeze) at the
  version the justfile's `freeze-version` line names. **That line is the only
  place this repository names it**: `just screenshots-tools` installs from it,
  and CI installs through `scripts/install-freeze.sh`, which reads that same line
  out of the justfile rather than keeping a second pin.
- **The font.** `fonts/JetBrainsMono-Regular.ttf` (OFL — see
  `fonts/JetBrainsMono-OFL.txt`), vendored and passed with `--font.file`, so
  `freeze` never fetches one over the network and the file renders the same on
  GitHub and on crates.io with nothing external to load. One copy, named once, in
  `scripts/screenshots.py`.
- **The lane.** `[capture].arches` in `screencomp.toml` is the only place a lane
  name is declared; `scripts/host-arch.sh` is the only place this host's lane is
  derived from `uname -m`, and the capture, the guard and `just screenshots-bless`
  all read it from there. One lane, `x86_64`, is declared, because the SVG bytes
  are identical on every CPU and a second lane would prove the same bytes twice —
  but the guard classifies **the lane of the host it runs on** and refuses a host
  no lane declares, so an `arm64` developer adds that lane and blesses it once.
- **The environment is cleared.** `scripts/shotworld.py` unsets every ambient
  `ONEPIPELINE_*`, `ONEVCS_*`, `ONETASKGRAPH_*`, `ONEAGENTGRAPH_*`,
  `ONEHARNESS_*`, `ONEJUDGE_*` and `ONEMESSAGEBUS_*` variable before it sets the
  ones the world needs. Every one of those is read ahead of most other layers, so
  an exported one either prints into a shot or points the whole journey somewhere
  else.

**And everything per-run is normalised, in the capture.** `scripts/shotnorm.py`
is the whole list: the wall-clock instant on every line, the pid inside an agent
stream id, the session token `onevcs` mints per publication, the driver's own
pid, the absolute paths of the scratch world, the free space on this machine's
disk, a node's age computed from the clock, and the journal byte offset a cursor
carries. Each is replaced by a fixed value of its own; no line is added, dropped
or reordered, and no word a verb chose is rewritten into a different word.

**This node adds no clock override, no deterministic-id switch, no `--no-color`,
no `--width` and no demo flag, and none may be added for a screenshot's
convenience.** The CLI surface is the approved `docs/contract.md`'s, the code is
written to match it, and a flag added here would be a contract change. That is
why all of the above lives in the capture.

## Why a hash-gated capture is not a second statement of the contract

`docs/contract.md` is the approved authority for this CLI surface, `tests/contract.rs`
drives fixtures out of it, and the judged lint's
`contracts_have_one_source_or_a_drift_gate` rule fires on a second spelling of a
contract. A committed screenshot of CLI output *would* be such a second spelling
if it were hand-made. It is not: every image here is produced by running the real
binary, and CI refuses the pull request the moment its bytes diverge from
`shots/baseline/<arch>.json`. **The baseline is the drift gate and the binary
remains the one source** — an image cannot say something the binary does not,
because an image that did is a red check rather than a document to reconcile.

The same reasoning is why a capture that contradicted the contract would be a
**contract bug to report** rather than a shot to adjust: the shot is what the
binary prints, so the disagreement is between the binary and the document.

## Commands

- `just screenshots-tools` — install the pinned `freeze` (needs Go). screencomp
  is installed separately; see its README. CI installs both itself.
- `just screenshots` — capture. Builds the release binaries, drives the journey,
  writes `shots/current/<arch>/` and the committed copies in `images/`. Quiet on
  success. It is the `onepipeline-visual-docs` Nx project's own target, so that
  every path that can change a rendered shot is declared in `nx.json`'s
  `visualDocsSource` named input rather than being a command nothing knows the
  inputs of — and so that a slow check needing two third-party tools sits behind
  its own edge rather than inside the CLI application's dependency surface. That
  project declares no `check`, `build` or `test` target, because it has none: a
  project that declared the uniform set and ran nothing of its own would drop out
  of every repo-wide verb while appearing to be covered by it.
- `just screenshots-gif` — regenerate `images/demo.gif` (needs Python 3 +
  Pillow). Drives the same journey and renders the attached stream frame by frame
  with the same vendored font, Pillow only — no `ttyd`, no `ffmpeg`.
- `just screenshots-bless` — after an **intended** output change: recapture and
  rewrite this host's lane, `shots/baseline/<arch>.json`. Commit it alongside
  `images/`.

## Outputs, and what is committed

- `shots/current/<arch>/captures.json` + the SVGs — the capture screencomp reads.
  **Gitignored**; regenerated. `$SHOTS_OUT` overrides the directory, and the
  reusable workflow exports it per lane.
- `shots/baseline/<arch>.json` — the committed digest baseline. No images in it.
- `images/*.svg` and `images/demo.gif` — the committed copies the README embeds.
- `shots/review/` — the local review gallery the pre-push guard builds on drift.
  Gitignored.

There is **no image under `docs/`**, deliberately: that directory holds
`contract.md` and `contract-divergences.md`, both machine-consumed and both named
in a cached build input.

## The strict gate, and its local half

CI (`fail-on-drift: true`) fails when a capture diverges from the committed
baseline. The local guard, `.githooks/pre-push`, re-captures **only** when a
`[guard].paths` file changes (`screencomp.toml`), and on drift it regenerates
this host's lane baseline, builds the review gallery, and blocks the push — so an
intended change is committed deliberately and the workflow stays green.

**Activation is real rather than committed.** `core.hooksPath` is per-clone state
and is never committed, so `just bootstrap` sets it: it depends on `_hooks`,
which runs `git config core.hooksPath .githooks`. Running `just bootstrap` in a
fresh clone leaves the guard active, and `screencomp doctor --env` reports
whether it is. By hand:

```bash
git config core.hooksPath .githooks
screencomp doctor --env
```

**The directory carries the visual guard and nothing else.** `just gate` — this
repository's complete pre-push bar — is deliberately not wired into the hook:
hooking a whole gate into `git push` changes the development loop, and that is a
separate decision nobody has made. Nothing was displaced either; this repository
installed no git hook before.

## When the output legitimately changes

Editing what a verb prints (`src/views.rs`, `src/watch.rs`, `src/telemetry.rs`),
the CLI surface (`src/cli.rs`), the order the engine writes events in
(`src/engine.rs`, `src/driver.rs`), the shipped example plan
(`examples/plan-store/`), the doubles (`crates/testfakes/`), or the capture
itself will change the SVGs. That is expected: run `just screenshots-bless` and
commit the new baseline with `images/`. Bumping `freeze-version` or the vendored
font reflows every shot — bless once.

The GIF is **not** hash-gated (a GIF is not byte-reproducible across Pillow
versions), so nothing tells you when it has gone stale. Regenerate it with
`just screenshots-gif` whenever what the attached stream prints changes: the
event kinds in `src/event.rs`, the line `src/views.rs` renders one as, or the
plan the journey drives.
