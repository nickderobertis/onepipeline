# Terminal screenshots

The README's images: deterministic SVGs of the real CLI's real output, gated on
their content hash by [screencomp](https://github.com/nickderobertis/screencomp),
plus one animated GIF that is not. Informational, like `deps-check` — never in
`just check`, `just gate`, or `ci.yml`'s `gate` job.

> `CLAUDE.md` is a symlink to this file — edit `AGENTS.md` only.

## Which scenes, and why not the others

`status`, `watch`, `monitor`, `results`, `runs`, `telemetry --breakdown`, the
`--help` verb list, and `demo.gif` of `start --attach`. Each sits in the README
section that explains that surface.

**The bar for a new one: it must say more than the sentence it would sit
beside.** `agents` fails it (the offline fixture runs no real `oneharness`, so
it reads `no sessions recorded`), and so do `transcript`, `goals`, `host` and
`channel queue`. The stills are monochrome because the output is — `clap`'s help
is the only coloured surface in this stack — so a shot that merely repeats its
paragraph earns nothing, and a shorter set is the better README.

## Two fixture layers

**Driven** — the shipped `examples/plan-store` `tracked-release` plan, executed
by the real binary: six nodes, two human approvals, real repositories on real
bare origins. Feeds every view that exists to show a *graph*.

**Recorded** — `tests/recorded/run-root/onemessagebus-repair-2`, read as
`tests/parity.rs` reads it. Feeds `telemetry --breakdown` and nothing else,
because its buckets and its cost are a real run's and no offline drive of a
scripted double can be. It is one node, which is why there are two layers.

Two subprocesses are substituted and they are the two this repository's own
offline tier substitutes — `fake-oneagentgraph` and `fake-gh`. **Keep it that
way**: no model call, no harness turn, no network, no credential, no real GitHub,
and a scene that cannot be had on those terms is the wrong scene.

**Every wait polls a file** (`tests/AGENTS.md`'s one rule). A capture that slept
to let output settle is what that note forbids; hold a dispatch with the doubles'
own `<key>.wait`/`<key>.go` and release one at a time, so the order the run
settles in is stated rather than decided by the host.

## What the bytes are pinned to, and where each pin lives

| pinned | its one source |
| --- | --- |
| renderer (`freeze`) | the justfile's `freeze-version`; `install-freeze.sh` reads that line for CI |
| font | `fonts/JetBrainsMono-Regular.ttf` (OFL), vendored and embedded, named in `capture.py` |
| arch lane | `[capture].arches` in `screencomp.toml`; `host-arch.sh` derives this host's |
| screencomp | named twice in the workflow, which YAML cannot avoid — `capture.py` refuses a capture when the two have parted |

Beyond those, the world clears every ambient `ONE*_` setting before setting its
own, and `normalise.py` replaces each per-run or per-host value with a fixed one.
**No clock override, deterministic-id switch, `--no-color`, `--width` or demo
flag may be added to make a capture easier**: the CLI surface is the approved
`docs/contract.md`'s, so such a flag is a contract change. Normalise instead.

## Why a generated capture is not a second statement of the contract

`contracts_have_one_source_or_a_drift_gate` fires on a second spelling of a
contract, and a committed screenshot of CLI output would be one **if it were
hand-made**. It is not: every image is what the real binary printed, and CI
refuses the change the moment its bytes leave `shots/baseline/<arch>.json`. The
baseline is the drift gate; the binary stays the one source. For the same reason
a capture that *contradicted* `docs/contract.md` is a contract bug to report, not
a shot to adjust.

<!-- llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the GIF is the one
image here with no gate, and that is a ruling rather than an omission: a GIF is not
byte-reproducible across Pillow versions, so hashing one would fail every capture on a
machine with a different Pillow and say nothing about the tool. The stack-wide decision
is to regenerate and commit it — `llmlint`, the reference adoption, does exactly this
with its own hero — and the restatement it carries is bounded to a rendering of the same
run the gated stills come from. Changing that is the contract owner's, not this
repository's. -->
**The GIF is the one image with no gate.** The hash-gated `monitor` scene covers
most of what it shows — the attached stream is `views::monitor`'s own lines — so
a red `monitor` is usually the signal to re-render. It is not the whole signal:
the launcher's own `-- <run> …` status lines and this renderer's own layout are
outside it. So re-render on purpose — `just screenshots-gif` — whenever what the
attached stream prints changes, and commit the new hero beside the new baseline.

## Working on it

`just --list` indexes the five recipes (`screenshots-tools`, `screenshots`,
`screenshots-gif`, `screenshots-bless`, `screenshots-guard`). What it does not
say:

- **Bless only after an intended output change**, and commit the refreshed
  baseline together with `images/` — CI compares the two against each other.
- The capture is the `onepipeline-visual-docs` project's own target, which
  declares no `check`, `build` or `test` because it has none: declaring the
  uniform set and running nothing would drop it out of every repo-wide verb
  while appearing covered by it. A new path that can change a shot belongs in
  `visualDocsSource` in `nx.json` **and** in `[guard].paths`; the capture
  refuses when a guard path is under no named input, which is the direction that
  would otherwise let a cached target replay a stale result.
- **No image goes under `docs/`** — that directory is machine-consumed and named
  in a cached build input.
- The guard only runs where `core.hooksPath` points at `.githooks`, which is
  per-clone state `just bootstrap` sets; `tests/provisioning.rs` holds that.
  The directory carries the visual guard **and nothing else** — `just gate` stays
  unhooked, as it already was.
- **A hook's environment is not a shell's.** Git runs the guard with `GIT_DIR`
  and `GIT_WORK_TREE` naming the repository being pushed, and `GIT_DIR` beats
  `-C` and beats discovery — so an inherited one seeds the world's throwaway
  repositories into the repository under push. The world states its whole git
  environment and inherits none of it (`clear_inherited_settings` in
  `world.py`). Rehearse with `just screenshots-guard`, which runs the real hook
  with real refs on its stdin and pushes nothing: neither `just screenshots` nor
  `just gate` reaches what only a hook reaches.
- **A failed step reports both streams** — `world.said`, which every step goes
  through. `git` writes `nothing to commit` to *stdout*, so a refusal quoting
  `stderr` alone arrives blank.
- Neither of those needs `freeze` or `screencomp`, so neither is excused from
  the offline tier: `tests/visual_docs.rs` drives `world.py` against real `git`
  for both.
