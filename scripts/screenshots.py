#!/usr/bin/env python3
"""Capture the terminal screenshots screencomp gates, galleries and posts to PRs.

Run it through `just screenshots` (or `bash scripts/screenshots.sh`, which is
what CI and the pre-push guard call); see `screenshots/AGENTS.md` for what the
scenes are and why each one documents the surface it shows.

**Two fixture layers, because two different things are being photographed.**

- The **driven** layer is a real multi-node run: the shipped
  `examples/plan-store` `tracked-release` plan — six nodes, two `kind: human`
  approvals and a lifecycle node carrying a human step — executed by the real
  compiled binary against this repository's own offline doubles. It is what
  `status`, `results`, `monitor`, `watch` and `runs` are read from, because the
  views that exist to show a graph have to show one.
- The **recorded** layer is `tests/recorded/run-root/onemessagebus-repair-2`, a
  finished run a real release drove and this repository checks in byte for byte.
  `telemetry --breakdown` comes from there and nowhere else: its eight wall-clock
  buckets and its per-party usage and cost are a real run's real numbers, which
  no offline drive of a scripted double can be.

Output (screencomp's capture contract):

    $SHOTS_OUT/captures.json   {schema, shots:[{name,toggles,hash,image}]}
    $SHOTS_OUT/<scene>.svg     one SVG per scene

`$SHOTS_OUT` defaults to `shots/current/<arch>` — the path the reusable workflow
exports per lane and the guard sets to the same thing. The SVGs are also copied
to `screenshots/images/`, which is what the README embeds and what is committed.

llmlint: ignore-file[new_code_lands_in_a_project] the capture is informational
machinery deliberately outside the crate: `just check` enforces a 95% line
coverage floor over the crate's own offline tier, and capture code compiled into
the crate would be counted in it. It is not an Nx project of its own either —
nothing builds or tests it — but every path that can change a rendered shot is
declared in `nx.json`'s `visualDocsSource` named input.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import shotnorm  # noqa: E402
import shotworld as world_module  # noqa: E402
from shotworld import PLAN_RUN, SMALL_PROJECT, SMALL_RUN, World  # noqa: E402

REPO = world_module.repo_root()

#: The recorded run the `telemetry` scene is read from, and the four settings
#: the binary reads it as a finished run under. `tests/parity.rs` is the worked
#: example and the offline contract; this is the same recipe, in Python.
RECORDED_RUN = "onemessagebus-repair-2"
RECORDING_HOST = "U-17UN402ICR95C"
RECORDING_SESSION = "28f7a3f8-23c8-535f-bf37-c37d2dfb31bd"
#: A pid no platform issues — above Linux's `PID_MAX_LIMIT` and macOS's
#: `PID_MAX`, and not a multiple of four, which every Windows pid is — written
#: over the recorded driver's, so the run reads as finished on any host.
NO_PROCESS = "2147483647"
#: An executable that is not there, so the provider-health probe stays silent
#: rather than reporting whatever this machine happens to have installed.
NO_SIBLING = "/nonexistent/onepipeline-shots/oneagentgraph"

#: The window every scene is rendered at. Fixed, because the gallery and the
#: README display each SVG at one width: a per-scene auto-width renders a narrow
#: view's text huge and a wide view's tiny. 118 columns clears the widest line
#: this capture produces with margin; 60px of padding plus 118 × ~8.42px/char.
WRAP_COLUMNS = 118
WINDOW_WIDTH = 60 + round(WRAP_COLUMNS * 8.42)


def main() -> int:
    world_module.clean_environment()
    binaries = REPO / "target" / "release"
    arch = host_arch()
    out = Path(os.environ.get("SHOTS_OUT") or REPO / "shots" / "current" / arch)
    if out.exists() and not out.is_dir():
        print(
            f"screenshots: SHOTS_OUT must name a directory to capture into; {out} is not one.",
            file=sys.stderr,
        )
        return 1
    if not shutil.which("freeze"):
        print(
            "screenshots: 'freeze' not on PATH. Install the pinned version with:\n"
            "             just screenshots-tools",
            file=sys.stderr,
        )
        return 1
    for name in ("onepipeline", "onevcs", "fake-oneagentgraph", "fake-gh"):
        if not (binaries / name).is_file():
            print(
                f"screenshots: {binaries / name} is not built. Run `just screenshots`,\n"
                "             which builds the release binaries this capture drives.",
                file=sys.stderr,
            )
            return 1

    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    images = REPO / "screenshots" / "images"
    images.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="onepipeline-shots-") as scratch:
        scratch = Path(scratch)
        scenes = capture(scratch, binaries)

    index = []
    for name, text in scenes:
        image = f"{name}.svg"
        render(text, out / image)
        shutil.copy2(out / image, images / image)
        index.append(
            {
                "name": name,
                "toggles": {},
                "hash": hashlib.sha256((out / image).read_bytes()).hexdigest(),
                "image": image,
            }
        )
    index.sort(key=lambda shot: (shot["name"], json.dumps(shot["toggles"], sort_keys=True)))
    (out / "captures.json").write_text(
        json.dumps({"schema": 1, "shots": index}, indent=2) + "\n"
    )
    print(
        f"screenshots: wrote {len(index)} shots to {out} and screenshots/images/",
        file=sys.stderr,
    )
    return 0


def capture(scratch: Path, binaries: Path) -> list[tuple[str, str]]:
    """Every scene's text, normalised, in the order the README reads them."""
    scenes: list[tuple[str, str]] = []
    world = World(scratch / "world", binaries)
    world.build()
    # One normaliser per scene: the instants are handed out in order of first
    # appearance, so a shared map would leave a later scene's stream reading as
    # though it ran backwards. `paths` is the same for all of them.
    paths = {
        str(world.root / "onevcs-home"): "/home/you/.onevcs",
        str(world.runs): "/home/you/onepipeline/runs",
        str(world.store): "/home/you/plans",
        str(world.root): "/home/you/onepipeline",
        str(REPO): ".",
    }
    scene_of = lambda text: shotnorm.Normaliser(paths).text(text)  # noqa: E731

    held: list[tuple[str, str]] = []

    def while_held() -> None:
        # Both lifecycle dispatches are inside the double's hold, so this is the
        # one moment the run has work genuinely in flight. Waited for on the
        # journal — the dispatch's own first record — rather than on a clock.
        world.until_journal("turn-started", "docs")
        world.until_journal("turn-started", "service")
        held.append(("status", world.cli("status", PLAN_RUN).stdout))
        held.append(("watch", bounded_watch(world)))

    world_module.drive(world, scratch / "attach.log", while_held=while_held)

    # A second run, so the listing has more than one project to group. The
    # smallest plan this repository ships, driven the same way and settling on
    # its own: it declares no human action.
    world_module.expect(
        world.cli("start", SMALL_PROJECT, "--attach"), 0, "start single-node --attach"
    )

    for name, text in held:
        scenes.append((name, shotnorm.realign(scene_of(text))))
    scenes.append(("help", help_scene(binaries)))
    scenes.append(("results", scene_of(world.cli("results", PLAN_RUN).stdout)))
    scenes.append(
        ("monitor", shotnorm.realign(scene_of(world.cli("monitor", PLAN_RUN).stdout)))
    )
    scenes.append(("runs", scene_of(world.cli("runs").stdout)))
    scenes.append(("telemetry", telemetry_scene(scratch, binaries, scene_of)))
    for name, text in scenes:
        if not text.strip():
            raise SystemExit(f"screenshots: scene '{name}' produced no output")
    return scenes


def bounded_watch(world: World) -> str:
    """A completed bounded `watch`: event lines, a heartbeat tick, the return line.

    The run is standing still inside the doubles' hold, so the wait is quiet and
    the heartbeat ticks. **How many ticks the shot shows is decided by reading
    the watch's own output file, never by sleeping**: once the wait has printed
    the tick this scene is about, the held `docs` dispatch is released, the node
    settles, its events reach the wait, and the `--until node=docs` condition
    returns it. Nothing here waits on the clock, and nothing about the run's
    order depends on how fast this machine is.
    """
    log = world.root / "watch.err"
    machine = world.root / "watch.out"
    watching = subprocess.Popen(
        [
            str(world.bin / "onepipeline"),
            "watch",
            PLAN_RUN,
            "--tick-interval",
            "1",
            # The only ending this wait may take is the one the scene is about.
            # A `--timeout` would add a second, and which of the two fired would
            # then depend on how fast this machine is.
            "--timeout",
            "none",
            "--until",
            "node=docs",
        ],
        env=world.env,
        stdin=subprocess.DEVNULL,
        stdout=machine.open("w"),
        stderr=log.open("w"),
        text=True,
    )
    try:
        world.until_lines_matching(log, r"^-- watching ", 1)
        world.release(world_module.HELD_DOCS)
        watching.wait(timeout=world_module.DEADLINE_SECONDS)
    finally:
        if watching.poll() is None:
            watching.kill()
            watching.wait()
    # The human lines are on standard error and one NDJSON record per line on
    # standard output — the split is the contract, so this scene takes the human
    # half and says so in `screenshots/AGENTS.md`.
    return log.read_text()


def help_scene(binaries: Path) -> str:
    """`onepipeline --help`: the whole verb surface, and the one colourised view.

    Nothing in this stack emits colour of its own — there is no colour or TUI
    crate anywhere in it — so `clap`'s own help is the only place a reader sees
    any. `CLICOLOR_FORCE` is anstream's own convention, which clap already
    honours: this capture adds no flag to the CLI to obtain it, and the text is
    byte for byte what a terminal shows.

    Nothing in it varies per run or per host, so it is the one scene with
    nothing to normalise.
    """
    env = dict(os.environ, CLICOLOR_FORCE="1")
    done = subprocess.run(
        [str(binaries / "onepipeline"), "--help"],
        env=env,
        capture_output=True,
        text=True,
        stdin=subprocess.DEVNULL,
    )
    if done.returncode != 0:
        raise SystemExit(f"screenshots: `--help` exited {done.returncode}:\n{done.stderr}")
    if "\033[" not in done.stdout:
        raise SystemExit(
            "screenshots: `--help` produced no ANSI, so the one colourised scene "
            "would render as plain text without saying so"
        )
    return done.stdout


def telemetry_scene(scratch: Path, binaries: Path, normalise) -> str:
    """`telemetry --breakdown` over the checked-in recorded run.

    The recorded layer, read exactly as `tests/parity.rs` reads it: a copy of
    the run under a runs root of its own, told it is on the recording host,
    under the launching session the recorded launch record names, with the
    driver's pid rewritten to one no platform issues so the run reads as
    finished here, and the sibling pointed at an executable that is not there so
    the provider-health block is silence rather than a probe of this machine.
    """
    root = scratch / "recorded" / "runs"
    run = root / RECORDED_RUN
    shutil.copytree(REPO / "tests" / "recorded" / "run-root" / RECORDED_RUN, run)
    (run / "README.md").unlink(missing_ok=True)
    (run / "dispatches").mkdir(exist_ok=True)
    launch = run / "launch.json"
    record = launch.read_text()
    driver = '"pid": 3682555,'
    if record.count(driver) != 1:
        raise SystemExit(
            "screenshots: the recorded launch record no longer names its driver once, "
            "so the copy cannot be made to read as finished"
        )
    launch.write_text(record.replace(driver, f'"pid": {NO_PROCESS},'))

    env = dict(os.environ)
    env.update(
        {
            "ONEPIPELINE_RUNS_DIR": str(root),
            "HOSTNAME": RECORDING_HOST,
            "ONEPIPELINE_LAUNCHER_SESSION": RECORDING_SESSION,
            "ONEPIPELINE_ONEAGENTGRAPH_BIN": NO_SIBLING,
        }
    )
    done = subprocess.run(
        [str(binaries / "onepipeline"), "telemetry", RECORDED_RUN, "--breakdown"],
        env=env,
        capture_output=True,
        text=True,
        stdin=subprocess.DEVNULL,
    )
    if done.returncode != 0:
        raise SystemExit(
            f"screenshots: telemetry over the recorded run exited {done.returncode}:\n"
            f"{done.stderr}"
        )
    return normalise(done.stdout)


def render(text: str, image: Path) -> None:
    """Render one scene to a deterministic SVG with the vendored, pinned font."""
    font = REPO / "screenshots" / "fonts" / "JetBrainsMono-Regular.ttf"
    source = image.with_suffix(".txt")
    source.write_text(text)
    try:
        subprocess.run(
            [
                "freeze",
                str(source),
                # Unconditional ANSI mode. freeze's content-based detection is
                # flaky: it intermittently reads plain aligned text as a source
                # file, then ignores `--font.file` and hangs fetching a default
                # font over the network. `--language ansi` is offline and
                # byte-identical to the auto-detected render.
                "--language",
                "ansi",
                "--font.file",
                str(font),
                "--font.family",
                "JetBrains Mono",
                "--font.size",
                "14",
                "--window",
                "--background",
                "#0d1117",
                "--padding",
                "20,30",
                "--margin",
                "0",
                "--border.radius",
                "8",
                "--width",
                str(WINDOW_WIDTH),
                "--wrap",
                str(WRAP_COLUMNS),
                "-o",
                str(image),
            ],
            check=True,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
        )
    finally:
        source.unlink(missing_ok=True)


def host_arch() -> str:
    done = subprocess.run(
        ["bash", str(REPO / "scripts" / "host-arch.sh")],
        capture_output=True,
        text=True,
        check=True,
    )
    return done.stdout.strip()


if __name__ == "__main__":
    raise SystemExit(main())
