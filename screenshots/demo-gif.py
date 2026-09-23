#!/usr/bin/env python3
"""Render the animated README hero: `start --attach` driving a DAG to settlement.

Run it through `just screenshots-gif`. Why this one artifact is not hash-gated,
and when to regenerate it, are `AGENTS.md` beside it.

What is local here: both of this tool's live surfaces are **append-only**, so the
frames are simply the real stream revealed as it arrived through a scrolling
window — nothing is reconstructed, unlike `llmlint`'s renderer this is adapted
from, whose live view redraws in place. The text is monochrome because the
tool's is.
"""

# llmlint: ignore-file[changed_behavior_has_e2e] the capture is informational machinery
# whose every step is a third-party tool this repository deliberately does not install
# for `just check` — `freeze` renders the scenes and `screencomp` classifies them — so
# an offline journey of it could only assert against stubs of those two, which tests the
# stubs. What this machinery produces is checked instead, and more strictly than a
# journey would: the committed digest baseline is re-derived by
# `.github/workflows/visual-docs.yml` on every pull request, and a capture that stopped
# working, changed a byte, or lost a scene is a red check there.

from __future__ import annotations

import os
import shutil
import sys
import tempfile
from pathlib import Path

# This directory is not on `sys.path` when the file is run as a script from the
# repository root, which is how `just screenshots-gif` runs it.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import normalise
import world as world_module
from world import World

from PIL import Image, ImageDraw, ImageFont

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
    world_module.clear_stack_settings()
    binaries = REPO / "target" / "release"
    here = Path(__file__).resolve().parent
    out = Path(os.environ.get("DEMO_GIF_OUT") or here / "images" / "demo.gif")
    if out.suffix != ".gif" or not out.parent.is_dir():
        print(
            f"demo-gif: DEMO_GIF_OUT names {out}, which is not a `.gif` inside an "
            "existing directory, so this run would either write the wrong kind of "
            "file or fail on the write. Point it at one, or unset it to write "
            "screenshots/images/demo.gif: `unset DEMO_GIF_OUT`.",
            file=sys.stderr,
        )
        return 1
    font_path = here / "fonts" / "JetBrainsMono-Regular.ttf"
    for needed in (binaries / "onepipeline", binaries / "fake-oneagentgraph"):
        if not needed.exists():
            print(
                f"demo-gif: {needed} is not built, and the hero is rendered from "
                "what it prints. Run `just screenshots-gif`, which builds the "
                "release binaries first.",
                file=sys.stderr,
            )
            return 1
    if not font_path.exists():
        print(
            f"demo-gif: the vendored font {font_path} is missing, so the frames "
            "cannot be drawn. Restore it from git: "
            "`git checkout -- screenshots/fonts`.",
            file=sys.stderr,
        )
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
        stream = normalise.realign(
            normalise.Normaliser(
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
        print(
            "demo-gif: the attached run printed nothing to animate, which means "
            "`start --attach` streamed no line at all. Run "
            "`just test-e2e shipped` to drive the same plan through the suite and "
            "see what refused it.",
            file=sys.stderr,
        )
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
