#!/usr/bin/env python3
"""SPIKE (spike-visual, throwaway — never lands): before/after captures of the
three view changes the driver-journal-scaling plan makes.

Run through `screenshots/spike-visual.sh OUT_DIR`, which builds the base's and
this branch's `onepipeline` and calls this with both.

One fixture is synthesized once and every scene, before and after, reads a copy
of it: the shipped `single-node` plan, driven by the base's real driver over the
repository's offline doubles until its one dispatch is held mid-turn, and its
run directory then copied. In every copy a `sleep` this script started stands in
as the run's live driver (its pid and start token written into `launch.json` and
the ownership lock `owner.lock`),
and this script plays that driver's part where a scene needs one — the claim
sidecar of scenes 1 and 2, and claiming the reply envelope of scene 3.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import capture  # noqa: E402
import normalise  # noqa: E402
import world as world_module  # noqa: E402
from world import SMALL_KEYS, SMALL_PROJECT, SMALL_RUN, World  # noqa: E402

NODE = SMALL_KEYS[0].split(".", 1)[0]
#: The claim sidecar scenes 1 and 2 read: 12.5 min since the last confirming
#: read, over a 300 s lease, declined for the host's rate limit.
UNCONFIRMED_SECONDS = 12 * 60 + 30
LEASE_SECONDS = 300
RATE_LIMIT = (
    "GitHub refused the GraphQL read with its secondary rate limit "
    "(HTTP 403, retry after 60 s)"
)
AMEND = "Also cover the store timing out mid-request, not only refusing the connection."
NOTE = "Amended: add the mid-request timeout case before you settle."
REPLY_WAIT_SECONDS = "4"


def main(argv: list[str]) -> int:
    if len(argv) != 4:
        print("usage: spike_visual.py BASE_BIN_DIR AFTER_BIN_DIR OUT_DIR", file=sys.stderr)
        return 2
    base, after, out = (Path(a).resolve() for a in argv[1:])
    out.mkdir(parents=True, exist_ok=True)
    world_module.clear_inherited_engine_and_git_settings()
    with tempfile.TemporaryDirectory(prefix="onepipeline-spike-visual-") as scratch:
        scratch = Path(scratch)
        # The base's `onepipeline` beside this build's `onevcs` and doubles: the
        # fixture is written by the unchanged engine.
        base_world_bin = scratch / "base-bin"
        base_world_bin.mkdir()
        for name in ("onevcs", "fake-oneagentgraph", "fake-gh"):
            (base_world_bin / name).symlink_to(after / name)
        (base_world_bin / "onepipeline").symlink_to(base / "onepipeline")
        world = World(scratch / "world", base_world_bin)
        fixture = synthesize(world, scratch / "fixture")
        for label, binaries in (("before", base), ("after", after)):
            for scene, take in (
                ("claim-status", claim_status),
                ("claim-watch", claim_watch),
                ("note-queued", note_queued),
            ):
                text = take(world, fixture, scratch / f"{label}-{scene}", binaries)
                image = out / f"{scene}-{label}.svg"
                capture.render(text, image)
                (out / f"{scene}-{label}.txt").write_text(text)
                print(f"spike-visual: wrote {image}", file=sys.stderr)
    return 0


def synthesize(world: World, fixture: Path) -> Path:
    """Drive `single-node` until its dispatch is mid-turn, and copy its run."""
    world.build()
    for key in SMALL_KEYS:
        world.script(f"{key}.wait", "")
        world.script(f"{key}.turn-open", "")
    log = world.root / "attach.log"
    driver = subprocess.Popen(
        [str(world.bin / "onepipeline"), "start", SMALL_PROJECT, "--attach"],
        env=world.env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=log.open("w"),
    )
    try:
        world.until_journal("turn-started", NODE, run=SMALL_RUN)
        shutil.copytree(
            world.runs / SMALL_RUN,
            fixture / SMALL_RUN,
            ignore=shutil.ignore_patterns("*.tmp.*"),
        )
        for key in SMALL_KEYS:
            world.release(key)
        driver.wait(timeout=world_module.DEADLINE_SECONDS)
    finally:
        if driver.poll() is None:
            driver.kill()
            driver.wait()
    return fixture


class Scene:
    """A copy of the fixture under its own runs root, with a live stand-in driver."""

    def __init__(self, world: World, fixture: Path, root: Path, binaries: Path) -> None:
        self.runs = root / "runs"
        shutil.copytree(fixture, self.runs)
        self.run = self.runs / SMALL_RUN
        self.binaries = binaries
        self.env = dict(world.env, ONEPIPELINE_RUNS_DIR=str(self.runs))
        self.stand_in = subprocess.Popen(["sleep", "3600"], stdin=subprocess.DEVNULL)
        ticks = Path(f"/proc/{self.stand_in.pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
        launch = json.loads((self.run / "launch.json").read_text())
        launch["pid"] = self.stand_in.pid
        launch["started"] = f"linux-proc-stat:{ticks}"
        (self.run / "launch.json").write_text(json.dumps(launch, indent=2))
        # And holding the run's ownership lock, as a live driver does.
        lock = json.loads((self.run / "owner.lock").read_text())
        lock["pid"] = self.stand_in.pid
        lock["started"] = f"linux-proc-stat:{ticks}"
        (self.run / "owner.lock").write_text(json.dumps(lock))
        self.normaliser = normalise.Normaliser(
            {
                str(world.root / "onevcs-home"): "/home/you/.onevcs",
                str(self.runs): "/home/you/onepipeline/runs",
                str(world.store): "/home/you/plans",
                str(world.root): "/home/you/onepipeline",
            }
        )

    def cli(self, *args: str, **kwargs) -> subprocess.CompletedProcess:
        return subprocess.run(
            [str(self.binaries / "onepipeline"), *args],
            env=kwargs.pop("env", self.env),
            capture_output=True,
            text=True,
            stdin=subprocess.DEVNULL,
            **kwargs,
        )

    def done(self) -> None:
        self.stand_in.kill()
        self.stand_in.wait()

    def text(self, raw: str) -> str:
        return normalise.realign(self.normaliser.text(raw))


def prompt(*args: str) -> str:
    return "$ onepipeline " + " ".join(args) + "\n"


def write_claim(scene: Scene) -> None:
    last = time.time() - UNCONFIRMED_SECONDS
    (scene.run / "claim-confirmation.json").write_text(
        json.dumps(
            {
                "last_confirmed_at": time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime(last)),
                "reason": "rate-limited",
                "detail": RATE_LIMIT,
                "lease_seconds": LEASE_SECONDS,
            }
        )
    )


def claim_status(world: World, fixture: Path, root: Path, binaries: Path) -> str:
    """Scene 1: `status` of a live-driven run whose claim no read has confirmed."""
    scene = Scene(world, fixture, root, binaries)
    try:
        write_claim(scene)
        args = ("status", SMALL_RUN, "--no-providers")
        done = scene.cli(*args)
        world_module.expect(done, 0, " ".join(args))
        return scene.text(prompt(*args) + done.stdout)
    finally:
        scene.done()


def claim_watch(world: World, fixture: Path, root: Path, binaries: Path) -> str:
    """Scene 2: a bounded `watch` of the same run — a heartbeat, then its ending."""
    scene = Scene(world, fixture, root, binaries)
    try:
        write_claim(scene)
        args = ("watch", SMALL_RUN, "--tick-interval", "3", "--timeout", "5")
        done = scene.cli(*args)
        return scene.text(prompt(*args) + done.stderr)
    finally:
        scene.done()


def note_queued(world: World, fixture: Path, root: Path, binaries: Path) -> str:
    """Scene 3: `reply` [amend, note] to the mid-turn node, then `status`.

    The stand-in driver claims the envelope as the base's driver does. Before: it
    hands the note to the running turn and answers nothing, writing the
    `delivering` sidecar entry the base driver writes. After: it writes the
    outcome the planned change writes — the amend applied, the note `queued` —
    and the inbox entry the mock status reads.
    """
    scene = Scene(world, fixture, root, binaries)
    after = "after" in root.name
    envelope = {
        "version": 3,
        "commands": [
            {"op": "amend", "id": NODE, "text": AMEND},
            {"op": "note", "id": NODE, "addressee": "worker", "text": NOTE},
        ],
    }
    file = root / "reply.json"
    file.write_text(json.dumps(envelope, indent=2) + "\n")
    try:
        replying = subprocess.Popen(
            [str(binaries / "onepipeline"), "reply", SMALL_RUN, str(file)],
            env=dict(scene.env, ONEPIPELINE_REPLY_TIMEOUT_SECONDS=REPLY_WAIT_SECONDS),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        channel = scene.run / "channel"
        commands = channel / "commands.jsonl"
        World._until(
            lambda: replying.poll() is not None
            or (commands.is_file() and NOTE in commands.read_text()),
            f"the envelope on {commands}",
        )
        if replying.poll() is not None:
            raise SystemExit(f"spike-visual: reply ended early:\n{replying.communicate()}")
        lines = commands.read_text().splitlines()
        envelope_id = json.loads(lines[-1])["id"]
        now = time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime())
        (channel / "commands-cursor.json").write_text(str(len(lines)))
        if after:
            with (channel / "command-outcomes.jsonl").open("a") as outcomes:
                outcomes.write(
                    json.dumps(
                        {
                            "id": envelope_id,
                            "applied": True,
                            "results": [
                                {"index": 0, "op": "amend", "outcome": "applied"},
                                {"index": 1, "op": "note", "outcome": "queued"},
                            ],
                        },
                        separators=(",", ":"),
                    )
                    + "\n"
                )
        state = "queued" if after else "delivering"
        (channel / "deliveries.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "deliveries": [
                        {"envelope": str(envelope_id), "state": state, "node": NODE, "since": now}
                    ],
                }
            )
        )
        stdout, stderr = replying.communicate(timeout=world_module.DEADLINE_SECONDS)
        if replying.returncode != 0:
            raise SystemExit(f"spike-visual: reply exited {replying.returncode}:\n{stderr}")
        status_args = ("status", SMALL_RUN, "--no-providers")
        status = scene.cli(*status_args)
        world_module.expect(status, 0, " ".join(status_args))
        transcript = (
            "$ cat reply.json\n"
            + file.read_text()
            + prompt("reply", SMALL_RUN, "reply.json")
            + stdout
            + stderr
            + prompt(*status_args)
            + status.stdout
        )
        return scene.text(transcript)
    finally:
        scene.done()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
