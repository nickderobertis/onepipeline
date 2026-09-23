#!/usr/bin/env python3
"""Render the animated README hero: `onepipeline start --attach` driving a DAG.

Run it through `just screenshots-gif`. Like `scripts/screenshots.py` it drives
the **real compiled binary** against this repository's own offline fixtures —
the shipped `examples/plan-store` `tracked-release` plan, six nodes with two
`kind: human` approvals, executed through the two scripted doubles the e2e suite
substitutes at their subprocess boundaries. No model call, no harness turn, no
network, no credential.

**What it animates is the real stream, not a reconstruction of one.** Both of
this tool's live surfaces are append-only — `start --attach` polls every 50 ms
and prints the newly appended monitor lines to stderr, and nothing ever redraws
in place — so the frames here are simply that stream revealed as it arrived,
through a scrolling terminal window. (`llmlint`'s renderer, which this is
adapted from, has to reconstruct its frames because its live view redraws; this
one does not, and should stay simpler.)

**It is deliberately NOT hash-gated.** A GIF is not byte-reproducible across
Pillow versions, so it is regenerated on demand and committed, exactly as
`llmlint`'s is. Regenerate it whenever what the attached stream prints changes —
the event kinds in `src/event.rs`, the line `src/views.rs` renders one as, or
the plan in `examples/plan-store`.

The text is monochrome because the tool's output is: nothing in this stack emits
a colour escape outside `clap`'s own help. Colouring it here would be drawing
something no terminal shows.

llmlint: ignore-file[new_code_lands_in_a_project] the visual-docs capture is
informational machinery outside every Nx target the gate reaches, and outside
the crate on purpose (the 95% coverage floor is measured over the crate).
"""

from __future__ import annotations

import os
import shutil
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import shotnorm  # noqa: E402
import shotworld as world_module  # noqa: E402
from shotworld import World  # noqa: E402

from PIL import Image, ImageDraw, ImageFont  # noqa: E402

REPO = world_module.repo_root()

# The same window the still scenes are rendered in (`scripts/screenshots.py`
# hands `freeze` this background), so the hero and the stills read as one set.
BG = (13, 17, 23)
BAR = (22, 27, 34)
FG = (201, 209, 217)
DOTS = [(255, 95, 86), (255, 189, 46), (39, 201, 63)]  # traffic-light window dots

FONT_SIZE = 13
PAD = 18
BAR_H = 32
#: The terminal window's shape. 100 columns clears every line the attached
#: stream prints without folding one, and 18 rows is a window a reader can take
#: in at a glance. Both, with the font size above, also decide how many pixels
#: each frame is — and a scrolling stream re-encodes every pixel of every frame,
#: so the shape of this window is most of what the committed file weighs.
COLUMNS = 100
ROWS = 18
#: How many newly-arrived lines each frame reveals, and how long it is held.
#: Chosen together so the whole run animates in about twelve seconds: a frame
#: per line would be two hundred frames of a mostly-unchanged picture.
LINES_PER_FRAME = 8
FRAME_MS = 240
HOLD_MS = 3000


def main() -> int:
    world_module.clean_environment()
    binaries = REPO / "target" / "release"
    out = Path(os.environ.get("DEMO_GIF_OUT") or REPO / "screenshots" / "images" / "demo.gif")
    font_path = REPO / "screenshots" / "fonts" / "JetBrainsMono-Regular.ttf"
    for needed in (binaries / "onepipeline", binaries / "fake-oneagentgraph", font_path):
        if not needed.exists():
            print(f"demo-gif: missing {needed}", file=sys.stderr)
            return 1

    with tempfile.TemporaryDirectory(prefix="onepipeline-gif-") as scratch:
        scratch = Path(scratch)
        world = World(scratch / "world", binaries)
        world.build()
        log = scratch / "attach.log"
        world_module.drive(world, log)
        # The same normalisation the stills get, for the same reason: the raw
        # stream carries this machine's temporary paths, the doubles' pids and
        # the session tokens `onevcs` minted for this one run.
        stream = shotnorm.realign(
            shotnorm.Normaliser(
                {
                    str(world.root / "onevcs-home"): "/home/you/.onevcs",
                    str(world.runs): "/home/you/onepipeline/runs",
                    str(world.store): "/home/you/plans",
                    str(world.root): "/home/you/onepipeline",
                    str(REPO): ".",
                }
            ).text(log.read_text())
        )

    lines = [line.rstrip("\n") for line in stream.splitlines()]
    if not lines:
        print("demo-gif: the attached run printed nothing to animate", file=sys.stderr)
        return 1
    frames = build_frames(lines)
    out.parent.mkdir(parents=True, exist_ok=True)
    render_gif(frames, str(font_path), str(out))
    print(
        f"demo-gif: wrote {out} ({len(frames)} frames over {len(lines)} lines)",
        file=sys.stderr,
    )
    return 0


def build_frames(lines: list[str]) -> list[tuple[list[str], int]]:
    """The stream revealed a few lines at a time, through a scrolling window."""
    frames: list[tuple[list[str], int]] = []
    shown = 0
    while shown < len(lines):
        shown = min(len(lines), shown + LINES_PER_FRAME)
        frames.append((lines[max(0, shown - ROWS) : shown], FRAME_MS))
    # Hold on the ending — the settled run and the settlement it printed — so a
    # reader arriving mid-loop still sees where it got to.
    frames.append((frames[-1][0], HOLD_MS))
    return frames


def render_gif(frames: list[tuple[list[str], int]], font_path: str, out: str) -> None:
    font = ImageFont.truetype(font_path, FONT_SIZE)
    cell = int(font.getlength("M"))
    ascent, descent = font.getmetrics()
    line_height = ascent + descent + 4
    width = PAD * 2 + COLUMNS * cell
    height = BAR_H + PAD + ROWS * line_height + PAD

    def draw_frame(window: list[str]) -> Image.Image:
        image = Image.new("RGB", (width, height), BG)
        draw = ImageDraw.Draw(image)
        # Window chrome: a title bar with three traffic-light dots, the same
        # frame `freeze` draws around each still.
        draw.rectangle([0, 0, width, BAR_H], fill=BAR)
        for nth, colour in enumerate(DOTS):
            x = PAD + nth * 20
            y = BAR_H // 2
            draw.ellipse([x - 5, y - 5, x + 5, y + 5], fill=colour)
        top = BAR_H + PAD
        for row, text in enumerate(window):
            draw.text((PAD, top + row * line_height), text[:COLUMNS], font=font, fill=FG)
        return image

    # Quantised to a small palette before saving. The picture is one foreground
    # colour on one background and the anti-aliasing between them, so six
    # adaptive colours read the same as the full-colour render at a third of the
    # bytes — which matters for a file the README loads above the fold.
    images = [
        draw_frame(window).convert("P", palette=Image.ADAPTIVE, colors=6)
        for window, _ in frames
    ]
    images[0].save(
        out,
        save_all=True,
        append_images=images[1:],
        duration=[ms for _, ms in frames],
        loop=0,
        optimize=True,
        disposal=2,
    )


if __name__ == "__main__":
    raise SystemExit(main())
