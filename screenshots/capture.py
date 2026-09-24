#!/usr/bin/env python3
"""Render each scene of the visual-docs capture to a deterministic SVG.

Run it through `just screenshots`. The scenes, the two fixture layers they come
from, and what the rendering is pinned to are `AGENTS.md` beside it.

Writes screencomp's capture contract into `$SHOTS_OUT` — `captures.json` and one
SVG per scene — and copies each image to `images/`, which is what the README
embeds and what is committed.
"""

# llmlint: ignore-file[changed_behavior_has_e2e] the capture's every step is a
# third-party tool `just check` does not install (`freeze`, `screencomp`), so a journey
# could only assert against stubs of those two. What it produces is checked instead: the
# visual-docs workflow re-derives the committed baseline on every pull request.

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# This directory is not on `sys.path` when the file is run as a script from the
# repository root, which is how `just screenshots` and the pre-push guard run it.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import normalise
import world as world_module
from world import PLAN_RUN, SMALL_PROJECT, World

REPO = world_module.repo_root()

#: The recorded run the `telemetry` scene is read from. The **only** thing
#: stated about it here is which directory it is; the host it was recorded on,
#: the session that launched it and the pid of the driver that drove it are all
#: read out of its own launch record below, because they are facts of that
#: fixture and a copy of them here would go stale the day it is re-recorded.
#: `tests/parity.rs` is the worked example and the offline contract.
RECORDED = Path("tests/recorded/run-root")
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


#: The workflow that calls screencomp, and the two places it names a version of
#: it — the reusable workflow's ref and the version it installs. Contract 2 of
#: the visual-docs adoption requires the copies to be reconciled by something
#: that fails when they part; this capture is that something, because it is what
#: both the local guard and the workflow run.
WORKFLOW = Path(".github/workflows/visual-docs.yml")


def screencomp_pin_refusal() -> str | None:
    """The refusal, when the workflow's two screencomp versions have parted."""
    text = (REPO / WORKFLOW).read_text()
    # `findall`, not `search`: a second declaration of either would leave one of
    # them unchecked, which is exactly the drift this reconciliation exists for.
    found = {
        "calls": re.findall(r"visual-docs-reusable\.yml@(\S+)", text),
        "installs": re.findall(r"^\s*screencomp-version:\s*(\S+)", text, re.M),
    }
    release = re.compile(r"^v?\d+\.\d+\.\d+$")
    for what, seen in found.items():
        if len(seen) != 1:
            return (
                f"screenshots: {WORKFLOW} {what} screencomp {len(seen)} times, and "
                "this reconciliation is of one against one. It has to name the "
                "reusable workflow's ref once and the version to install once, and "
                "they have to agree."
            )
        if not release.match(seen[0]):
            return (
                f"screenshots: {WORKFLOW} {what} screencomp {seen[0]!r}, which is "
                "not an immutable release tag. A moving ref would change the gate "
                "under a baseline nothing recaptured — name a `vMAJOR.MINOR.PATCH` "
                "release."
            )
    if found["calls"][0] != found["installs"][0]:
        return (
            f"screenshots: {WORKFLOW} calls screencomp {found['calls'][0]} and "
            f"installs {found['installs'][0]}. The gallery and the gate would then "
            "come from two releases. Set both to the same tag."
        )
    return None


#: Where the build graph declares what can change a rendered shot, beside
#: `screencomp.toml`'s own list for the local guard. The two are deliberately
#: different shapes — Nx enumerates directories a cached target re-runs on, the
#: guard enumerates files worth paying for a local recapture over — so they are
#: reconciled by containment rather than by equality.
NAMED_INPUTS = Path("nx.json")


def uncovered_guard_path_refusal() -> str | None:
    """The refusal, when a guard path is under no declared named input."""
    document = json.loads((REPO / NAMED_INPUTS).read_text())
    entries = None
    if isinstance(document, dict) and isinstance(document.get("namedInputs"), dict):
        entries = document["namedInputs"].get("visualDocsSource")
    if not isinstance(entries, list) or not all(isinstance(e, str) for e in entries):
        return (
            f"screenshots: {NAMED_INPUTS} does not declare a `visualDocsSource` "
            "named input as a list of path patterns, so nothing in the build graph "
            "says what can change a rendered shot. Restore it."
        )
    declared = [
        entry.removeprefix("{workspaceRoot}/").removesuffix("/**/*").removesuffix("**/*")
        for entry in entries
        if entry.startswith("{workspaceRoot}/")
    ]
    guard = re.search(
        r"^paths = \[(.*?)^\]", (REPO / "screencomp.toml").read_text(), re.M | re.S
    )
    if not guard:
        return (
            "screenshots: screencomp.toml no longer declares `[guard].paths`, so "
            "the local guard would recapture on nothing. Restore the list."
        )
    for path in re.findall(r'"([^"]+)"', guard.group(1)):
        bare = path.removesuffix("/**").removesuffix("**")
        if not any(bare == entry or bare.startswith(entry) for entry in declared):
            return (
                f"screenshots: screencomp.toml's [guard].paths names {path!r}, which "
                "no `visualDocsSource` entry in nx.json covers — so a change there "
                "would make the local guard recapture while the cached target "
                "replayed a stale result. Add it to that named input."
            )
    return None


def main() -> int:
    world_module.clear_inherited_settings()
    for refusal in (screencomp_pin_refusal(), uncovered_guard_path_refusal()):
        if refusal is not None:
            print(refusal, file=sys.stderr)
            return 1
    binaries = REPO / "target" / "release"
    arch = host_arch()
    out = Path(os.environ.get("SHOTS_OUT") or REPO / "shots" / "current" / arch)
    if out.exists() and not out.is_dir():
        print(
            f"screenshots: SHOTS_OUT names {out}, which exists and is not a "
            "directory, so there is nowhere to capture into. Point it at a "
            "directory, or unset it to capture into shots/current/<arch>: "
            "`unset SHOTS_OUT`.",
            file=sys.stderr,
        )
        return 1
    if not shutil.which("freeze"):
        print(
            "screenshots: 'freeze' is not on PATH, and it is what renders each "
            "scene. Install the pinned version with `just screenshots-tools` "
            "(it needs Go), then re-run `just screenshots`.",
            file=sys.stderr,
        )
        return 1
    for name in ("onepipeline", "onevcs", "fake-oneagentgraph", "fake-gh"):
        if not (binaries / name).is_file():
            print(
                f"screenshots: {binaries / name} is not built, and the capture "
                "drives it. Run `just screenshots`, which builds the release "
                "binaries first; if you are running this file directly, drop "
                "SCREENSHOTS_NO_BUILD and go through `screenshots/capture.sh`.",
                file=sys.stderr,
            )
            return 1

    out.mkdir(parents=True, exist_ok=True)
    images = REPO / "screenshots" / "images"
    images.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="onepipeline-shots-") as scratch:
        scratch = Path(scratch)
        scenes = capture(scratch, binaries)

    index = []
    for name, text in scenes:
        image = f"{name}.svg"
        # Written over by name. Nothing here removes a file it is not about to
        # write: `$SHOTS_OUT` comes from a workflow, a hook or a shell, and a
        # sweep of whatever it happened to name would make an environment
        # variable the thing deciding what gets destroyed.
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
    # Rewritten whole, so a scene that has been removed leaves the index on
    # this run even if its old image file is still on disk: `captures.json` is
    # what screencomp classifies against, and it lists exactly what was captured.
    (out / "captures.json").write_text(
        json.dumps({"schema": 1, "shots": index}, indent=2) + "\n"
    )
    print(
        f"screenshots: wrote {len(index)} shots to {out} and screenshots/images/",
        file=sys.stderr,
    )
    return 0


#: Where the watch verb's own exit code for a met `--until` lives. One source:
#: the crate defines it, `docs/contract.md` states that these codes do not move,
#: and a capture that restated the number would be the second spelling.
EXIT_CODES = Path("src/error.rs")


def node_settled_exit() -> int:
    """`EXIT_NODE_SETTLED`, read out of the crate that declares it."""
    found = re.findall(
        r"^pub const EXIT_NODE_SETTLED: i32 = (\d+);", (REPO / EXIT_CODES).read_text(), re.M
    )
    if len(found) != 1:
        raise SystemExit(
            f"screenshots: {EXIT_CODES} declares EXIT_NODE_SETTLED "
            f"{len(found)} times, and the bounded-watch scene is held to exactly "
            "one. That constant is the watch verb's own ending for a met "
            "`--until`; if it was renamed, name the new one here."
        )
    return int(found[0])


def printed(world: World, *args: str) -> str:
    """One verb's stdout, refused unless that verb exited 0.

    A verb that failed still prints, so an unchecked read renders a broken view
    into a shot and blesses it. `world_module.expect` is the refusal every other
    step of the journey already goes through, and it quotes both streams.
    """
    done = world.cli(*args)
    world_module.expect(done, 0, "onepipeline " + " ".join(args))
    return done.stdout


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
    def scene_of(text: str) -> str:
        return normalise.Normaliser(paths).text(text)

    held: list[tuple[str, str]] = []

    def while_held() -> None:
        # Both lifecycle dispatches are inside the double's hold, so this is the
        # one moment the run has work genuinely in flight. Waited for on the
        # journal — the dispatch's own first record — rather than on a clock.
        for key in world_module.HELD:
            world.until_journal("turn-started", key.split(".", 1)[0])
        held.append(("status", printed(world, "status", PLAN_RUN)))
        held.append(("watch", bounded_watch(world)))

    world_module.drive(world, scratch / "attach.log", while_held=while_held)

    # A second run, so the listing has more than one project to group. The
    # smallest plan this repository ships, driven the same way and settling on
    # its own: it declares no human action.
    world_module.expect(
        world.cli("start", SMALL_PROJECT, "--attach"), 0, "start single-node --attach"
    )

    for name, text in held:
        scenes.append((name, normalise.realign(scene_of(text))))
    scenes.append(("help", help_scene(binaries)))
    scenes.append(("results", scene_of(printed(world, "results", PLAN_RUN))))
    scenes.append(
        ("monitor", normalise.realign(scene_of(printed(world, "monitor", PLAN_RUN))))
    )
    scenes.append(("runs", scene_of(printed(world, "runs"))))
    scenes.append(("telemetry", telemetry_scene(scratch, binaries, scene_of)))
    for name, text in scenes:
        if not text.strip():
            raise SystemExit(
                f"screenshots: scene '{name}' produced no output, so there is "
                "nothing to render. Run that verb by hand against the world the "
                "journey builds — `just test-e2e views` drives the same views "
                "through the suite — and fix what it reports."
            )
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
    if not world_module.HELD:
        raise SystemExit(
            "screenshots: the plan this capture drives holds no lifecycle "
            "dispatch open, so there is nothing for a bounded wait to return on. "
            "This scene needs a plan with at least one node that targets a "
            "repository."
        )
    released = world_module.HELD[0]
    log = world.root / "watch.err"
    machine = world.root / "watch.out"
    watching = subprocess.Popen(
        [
            str(world.bin / "onepipeline"),
            "watch",
            PLAN_RUN,
            # Long enough that the window between this capture reading the
            # first tick and the run producing its next event cannot fit a
            # second one. A one-second interval fitted on this machine and not
            # under a tracer, which is exactly the kind of difference a slower
            # runner has.
            "--tick-interval",
            "5",
            # The only ending this wait may take is the one the scene is about.
            # A `--timeout` would add a second, and which of the two fired would
            # then depend on how fast this machine is.
            "--timeout",
            "none",
            "--until",
            f"node={released.split('.', 1)[0]}",
        ],
        env=world.env,
        stdin=subprocess.DEVNULL,
        stdout=machine.open("w"),
        stderr=log.open("w"),
        text=True,
    )
    try:
        world.until_lines_matching(log, r"^-- watching ", 1)
        world.release(released)
        watching.wait(timeout=world_module.DEADLINE_SECONDS)
    finally:
        if watching.poll() is None:
            watching.kill()
            watching.wait()
    # The wait's own answer. `--until node=…` being met is the verb's
    # `EXIT_NODE_SETTLED`, not zero, so the code is read out of the crate that
    # defines it rather than written here: any *other* ending — the wait running
    # out, a surface coming up, nothing driving, or the kill above — renders a
    # different picture under this scene's name. Its streams went to files, so
    # this quotes the log rather than `world.said`.
    settled = node_settled_exit()
    if watching.returncode != settled:
        raise SystemExit(
            f"screenshots: the bounded watch exited {watching.returncode}, and this "
            f"scene is of a wait that returned on its `--until` condition ({settled}). "
            f"{world_module.REPAIR}\nstderr:\n{log.read_text().rstrip()}"
        )
    # What the scene is a picture of is **one** tick. A machine slow enough to
    # fit another between the release and the node settling would render a
    # different picture under the same name, which is a drifted baseline for a
    # reason no diff explains — so it refuses instead.
    ticks = sum(1 for line in log.read_text().splitlines() if line.startswith("-- watching "))
    if ticks != 1:
        raise SystemExit(
            f"screenshots: the bounded watch printed {ticks} heartbeats, and this "
            "scene is of one. The machine was slow enough that another tick "
            "landed between the release and the node settling; re-run "
            "`just screenshots`, and raise the scene's `--tick-interval` if it "
            "keeps happening."
        )
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
        raise SystemExit(
            f"screenshots: `onepipeline --help` exited {done.returncode}, which "
            "means the built binary refuses its own help. Rebuild it "
            "(`cargo build --release --locked --bin onepipeline`) and re-run:\n"
            f"{world_module.said(done)}"
        )
    if "\033[" not in done.stdout:
        raise SystemExit(
            "screenshots: `onepipeline --help` produced no ANSI, so the one "
            "colourised scene would render as plain text without saying so. "
            "`CLICOLOR_FORCE=1` is what asks clap for it; check that NO_COLOR is "
            "not set in this shell (`unset NO_COLOR`) and re-run."
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
    recorded = sorted((REPO / RECORDED).iterdir())
    recorded = [entry for entry in recorded if entry.is_dir()]
    if len(recorded) != 1:
        raise SystemExit(
            f"screenshots: {RECORDED} holds {len(recorded)} recorded runs, and "
            "this scene is about one. Name which to photograph here."
        )
    source = recorded[0]
    root = scratch / "recorded" / "runs"
    run = root / source.name
    shutil.copytree(source, run)
    (run / "README.md").unlink(missing_ok=True)
    (run / "dispatches").mkdir(exist_ok=True)
    launch = run / "launch.json"
    try:
        record = json.loads(launch.read_text())
    except json.JSONDecodeError as broken:
        raise SystemExit(
            f"screenshots: {source}/launch.json is not readable JSON ({broken}), "
            "so the recorded run cannot be read as finished here. Restore it from "
            "git — it is a checked-in recording, not something a run rewrites."
        ) from broken
    host = record.get("host") if isinstance(record, dict) else None
    session = record.get("session") if isinstance(record, dict) else None
    pid = record.get("pid") if isinstance(record, dict) else None
    if not isinstance(host, str) or not isinstance(session, str) or not isinstance(pid, int):
        raise SystemExit(
            f"screenshots: {source}/launch.json does not name a host, a launching "
            "session and a driver pid of the types this capture puts into a "
            "subprocess environment. `tests/parity.rs` reads the same three — "
            "update both together."
        )
    driver = f'"pid": {pid},'
    text = launch.read_text()
    if text.count(driver) != 1:
        raise SystemExit(
            f"screenshots: {source}/launch.json names its driver's pid "
            f"{pid} {text.count(driver)} times, and exactly "
            "one is what can be rewritten to a pid no platform issues. Re-record "
            "the run, or read it the way `tests/parity.rs` does."
        )
    launch.write_text(text.replace(driver, f'"pid": {NO_PROCESS},'))

    env = dict(os.environ)
    env.update(
        {
            "ONEPIPELINE_RUNS_DIR": str(root),
            "HOSTNAME": host,
            "ONEPIPELINE_LAUNCHER_SESSION": session,
            "ONEPIPELINE_ONEAGENTGRAPH_BIN": NO_SIBLING,
        }
    )
    done = subprocess.run(
        [str(binaries / "onepipeline"), "telemetry", run.name, "--breakdown"],
        env=env,
        capture_output=True,
        text=True,
        stdin=subprocess.DEVNULL,
    )
    if done.returncode != 0:
        raise SystemExit(
            f"screenshots: `telemetry --breakdown` over the recorded run exited "
            f"{done.returncode}. The recorded run is read exactly as "
            "`tests/parity.rs` reads it, so run that suite "
            "(`cargo nextest run -E 'binary(parity)'`) to see which half moved:\n"
            f"{world_module.said(done)}"
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
        ["bash", str(Path(__file__).resolve().parent / "host-arch.sh")],
        capture_output=True,
        text=True,
        check=True,
    )
    return done.stdout.strip()


if __name__ == "__main__":
    raise SystemExit(main())
