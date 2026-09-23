#!/usr/bin/env python3
"""The offline world the capture drives, and the journey it drives through it.

Which fixtures this composes and why, and the rule that every wait here polls a
file rather than a clock, are `AGENTS.md` beside it. Shared by `capture.py` and
`demo-gif.py` so the stills and the animated hero cannot disagree about what run
they photographed.
"""

# llmlint: ignore-file[changed_behavior_has_e2e] the capture's every step is a
# third-party tool `just check` does not install (`freeze`, `screencomp`), so a journey
# could only assert against stubs of those two. What it produces is checked instead: the
# visual-docs workflow re-derives the committed baseline on every pull request.

from __future__ import annotations

import json
import math
import os
import re
import shutil
import subprocess
import time
from pathlib import Path

# The shipped plan the capture drives, and the nodes the journey names. Read
# here and nowhere else, so a rename in the example store fails the capture
# loudly instead of quietly photographing a different run.
PLAN_PROJECT = "plans:tracked-release"
PLAN_RUN = "tracked-release"
SMALL_PROJECT = "plans:single-node"
SMALL_RUN = "single-node"

#: The two lifecycle dispatches the capture holds open. Holding them is what
#: lets `status` be read with work genuinely in flight, gives `watch` something
#: to return on, and — because they are released one at a time — fixes the order
#: the two settle in, which a host would otherwise decide per run.
HELD_DOCS = "docs"
HELD_SERVICE = "service.implement"

#: The one node of the smallest plan this repository ships. The capture drives
#: that plan too, so the listing has a second project to group.
SMALL_NODE = "cover-report-failures"

#: The human approvals the capture clears, in the order the plan reaches them.
#: A planner clears these with `onepipeline attest`, exactly as the README
#: documents; nothing here is a shortcut around the product.
APPROVALS = ["design-approval", "service", "release-approval"]

#: The launching session and the host name every command runs under. Fixed
#: rather than this machine's, because both print into a rendered view.
SESSION = "planner-a1b2c3d4"
HOST = "shots-host"

#: How long a world that never reaches a record takes to fail. The backstop,
#: never the signal.
DEFAULT_DEADLINE_SECONDS = 180.0


def _deadline() -> float:
    """`SHOTS_DEADLINE_SECONDS`, refused rather than coerced.

    Read leniently, a malformed or non-positive value becomes a backstop of zero
    — every wait fails on its first poll — or of infinity, where a world that
    never gets there hangs the capture instead of failing it.
    """
    stated = os.environ.get("SHOTS_DEADLINE_SECONDS")
    if stated is None or stated.strip() == "":
        return DEFAULT_DEADLINE_SECONDS
    try:
        seconds = float(stated)
    except ValueError:
        seconds = float("nan")
    if not math.isfinite(seconds) or seconds <= 0:
        raise SystemExit(
            f"screenshots: SHOTS_DEADLINE_SECONDS is {stated!r}, which is not a "
            f"positive number of seconds. Give it one (the default is "
            f"{DEFAULT_DEADLINE_SECONDS:g}), or unset it: "
            f"`unset SHOTS_DEADLINE_SECONDS`."
        )
    return seconds


DEADLINE_SECONDS = _deadline()

#: How often a wait re-reads the file it is watching.
POLL_SECONDS = 0.02


def repo_root() -> Path:
    """The repository root: this file's directory is `screenshots/` under it."""
    return Path(__file__).resolve().parent.parent


class Waited(RuntimeError):
    """A world that never reached a record the journey waits for."""


class World:
    """One capture's world: a runs root, a store, two origins, and two doubles."""

    def __init__(self, root: Path, binaries: Path) -> None:
        self.root = root
        self.bin = binaries
        self.runs = root / "runs"
        self.fakes = root / "fakes"
        self.store = root / "store"

    def build(self) -> None:
        if self.root.exists():
            shutil.rmtree(self.root)
        for name in ("runs", "fakes", "project", "xdg", "xdg-state", "onevcs-home"):
            (self.root / name).mkdir(parents=True)
        shutil.copytree(repo_root() / "examples" / "plan-store", self.store)
        (self.root / "gitconfig").write_text("[core]\n\tlongpaths = true\n")
        self.env = self._environment()
        self._register_origins()
        self._script_the_doubles()

    def _environment(self) -> dict:
        # Every setting the engine and its two siblings read, stated by this
        # world. The capturing process's own environment is cleared of them
        # first (see `clear_stack_settings`), so nothing an operator exported can
        # print into a shot or point the journey somewhere else.
        path = os.pathsep.join([str(self.bin), os.environ.get("PATH", "")])
        stated = os.environ.get("SHOTS_ONETASKGRAPH_BIN")
        if stated:
            # An override that does not name a runnable program would otherwise
            # fail several steps later, as a plan the launcher could not read.
            if not (Path(stated).is_file() and os.access(stated, os.X_OK)):
                raise SystemExit(
                    f"screenshots: SHOTS_ONETASKGRAPH_BIN names {stated!r}, which is "
                    "not an executable file. Point it at one, or unset it to take "
                    "`onetaskgraph` from PATH: `unset SHOTS_ONETASKGRAPH_BIN`."
                )
        onetaskgraph = stated or shutil.which("onetaskgraph")
        if not onetaskgraph:
            raise SystemExit(
                "screenshots: no `onetaskgraph` on PATH. The capture reads every plan "
                "through the real binary, exactly as the e2e suite does. Install it "
                "with `just bootstrap`, or point SHOTS_ONETASKGRAPH_BIN at an "
                "executable one, then re-run `just screenshots`."
            )
        env = dict(os.environ)
        env.update(
            {
                "PATH": path,
                "HOSTNAME": HOST,
                "GIT_CONFIG_GLOBAL": str(self.root / "gitconfig"),
                "GIT_AUTHOR_NAME": "onepipeline shots",
                "GIT_AUTHOR_EMAIL": "shots@onepipeline.invalid",
                "GIT_COMMITTER_NAME": "onepipeline shots",
                "GIT_COMMITTER_EMAIL": "shots@onepipeline.invalid",
                "ONEVCS_HOME": str(self.root / "onevcs-home"),
                "ONEVCS_GH": str(self.bin / "fake-gh"),
                # The host behind the stand-in answers instantly, so the
                # sibling's hour-long default would only decide how long a
                # capture waits for an answer that is already there.
                "ONEVCS_CHECKS_TIMEOUT_SECONDS": "2",
                "ONEVCS_CHECKS_POLL_SECONDS": "0.05",
                "ONEPIPELINE_RUNS_DIR": str(self.runs),
                "ONETASKGRAPH_BIN": onetaskgraph,
                "XDG_CONFIG_HOME": str(self.root / "xdg"),
                "XDG_STATE_HOME": str(self.root / "xdg-state"),
                "ONETASKGRAPH_DEFAULT_SOURCES": "plans",
                "ONETASKGRAPH_SOURCES__PLANS__PLUGIN": "local-md",
                "ONETASKGRAPH_SOURCES__PLANS__CONFIG__ROOT": str(self.store),
                "ONEPIPELINE_ONEAGENTGRAPH_BIN": str(self.bin / "fake-oneagentgraph"),
                "ONEPIPELINE_FAKE_DIR": str(self.fakes),
                "ONEPIPELINE_FAKE_CLI_BIN": str(self.bin / "onepipeline"),
                "ONEPIPELINE_LAUNCHER": "shots",
                "ONEPIPELINE_LAUNCHER_SESSION": SESSION,
                "ONEPIPELINE_PROJECT_DIR": str(self.root / "project"),
                "ONEPIPELINE_NODE_GRAPH": str(
                    repo_root() / "graphs" / "node-scope.yaml"
                ),
                "ONEPIPELINE_BOUNDARY_BACKOFF_SECONDS": "0",
                "ONEPIPELINE_REPLY_TIMEOUT_SECONDS": "20",
                "ONEPIPELINE_FAKE_RENDEZVOUS_SECONDS": "180",
            }
        )
        return env

    def _register_origins(self) -> None:
        # The launcher asks `onevcs` about a targeted repository's live holders
        # before it mints a run, so an identity this host does not have refuses
        # the launch before the plan is even mapped. The identities are read out
        # of the shipped task documents rather than restated here, for the
        # reason `tests/e2e/shipped.rs` reads them out of the same files: a task
        # that moved repository would otherwise leave this registering an
        # identity nothing launches under.
        identities = example_repositories()
        if not identities:
            raise SystemExit(
                "screenshots: no task under examples/plan-store/tasks/ carries a "
                "`repositories:` list, so this world registers no identity and the "
                "launcher refuses the run before it maps the plan. Restore the "
                "repository the example tasks target, or point this capture at a "
                "plan that names one."
            )
        for nth, identity in enumerate(identities):
            checkout = self.root / f"repo-{nth}"
            run(["git", "init", "--initial-branch=main", "--quiet", str(checkout)], self.env)
            (checkout / "README.md").write_text("placeholder\n")
            run(["git", "-C", str(checkout), "add", "-A"], self.env)
            run(["git", "-C", str(checkout), "commit", "--quiet", "-m", "seed"], self.env)
            run(
                [
                    str(self.bin / "onevcs"),
                    "register",
                    str(checkout),
                    "--origin",
                    f"https://{identity}.git",
                ],
                self.env,
            )

    def _script_the_doubles(self) -> None:
        # What each lifecycle worker "did": a file in the worktree, so the
        # node's branch carries a diff and the node publishes rather than
        # settling `empty-branch`.
        self.script(f"{HELD_DOCS}.work", "The approved API and its rollout, documented.\n")
        self.script(
            f"{HELD_SERVICE}.work",
            "The approved API and rollout behaviour, implemented.\n",
        )
        self.script(
            f"{SMALL_NODE}.work",
            "The reporting service's failure paths, covered.\n",
        )
        # And the holds: both lifecycle dispatches wait for a file this capture
        # writes. That is what stands the run still while `status` and `watch`
        # are read, and what makes the order the two settle in this capture's
        # to state rather than the host's to decide.
        self.script(f"{HELD_DOCS}.wait", "")
        self.script(f"{HELD_SERVICE}.wait", "")
        # Each held dispatch announces its member and opens its turn before it
        # waits, as a real one does, so `status` read during the hold shows work
        # that has said something rather than a dispatch that has recorded
        # nothing yet.
        self.script(f"{HELD_DOCS}.turn-open", "")
        self.script(f"{HELD_SERVICE}.turn-open", "")

    def script(self, name: str, body: str) -> None:
        (self.fakes / name).write_text(body)

    def release(self, key: str) -> None:
        """Let a held dispatch finish, the way the doubles' own hold is released."""
        self.script(f"{key}.go", "")

    @property
    def run_root(self) -> Path:
        return self.runs / PLAN_RUN

    def cli(self, *args: str, **kwargs) -> subprocess.CompletedProcess:
        return subprocess.run(
            [str(self.bin / "onepipeline"), *args],
            env=self.env,
            capture_output=True,
            text=True,
            stdin=subprocess.DEVNULL,
            **kwargs,
        )

    def until_journal(self, kind: str, node: str | None = None, run: str | None = None) -> None:
        """Wait until the run's journal carries `kind` (about `node`, when named)."""
        journal = (self.runs / (run or PLAN_RUN)) / "events.jsonl"
        self._until(
            lambda: journal_has(journal, kind, node),
            f"{kind}{'' if node is None else f' for {node}'} in {journal}",
        )

    def until_file_says(self, path: Path, needle: str) -> None:
        self._until(
            lambda: path.is_file() and needle in path.read_text(errors="replace"),
            f"{needle!r} in {path}",
        )

    def until_lines_matching(self, path: Path, pattern: str, count: int) -> None:
        matcher = re.compile(pattern)
        self._until(
            lambda: path.is_file()
            and sum(1 for line in path.read_text(errors="replace").splitlines() if matcher.search(line))
            >= count,
            f"{count} line(s) matching {pattern!r} in {path}",
        )

    @staticmethod
    def _until(ready, what: str) -> None:
        deadline = time.monotonic() + DEADLINE_SECONDS
        while True:
            if ready():
                return
            if time.monotonic() > deadline:
                raise Waited(
                    f"screenshots: the world never reached {what} within "
                    f"{DEADLINE_SECONDS:g}s. The deadline is the backstop rather "
                    "than the signal, so this is a world that did not get there. "
                    "Read the run's own journal and driver log under the runs root "
                    "named in the environment above — `events.jsonl` says how far "
                    "the run got and `driver.log` why it stopped — and raise "
                    "SHOTS_DEADLINE_SECONDS only if the run is genuinely still "
                    "moving."
                )
            time.sleep(POLL_SECONDS)


def _mapping(record: dict, key: str) -> dict:
    """A record's nested object, or an empty one for anything that is not.

    A journal is external input here exactly as a plan file is: this capture
    reads what the binary under test wrote, and a `labels` or `payload` that is
    not an object is a record this journey has no reading of rather than a crash
    inside `.get`.
    """
    value = record.get(key)
    return value if isinstance(value, dict) else {}


def _record(line: str) -> dict | None:
    """One journal line as a record, or `None` for anything that is not one.

    A line mid-write is not yet JSON, and a journal is read while it is being
    appended to — so a partial line is a "not yet" rather than a failure. Nor is
    every well-formed JSON value a record: only an object has the keys read
    below, and a bare string or list would otherwise reach `.get`.
    """
    line = line.strip()
    if not line:
        return None
    try:
        record = json.loads(line)
    except json.JSONDecodeError:
        return None
    return record if isinstance(record, dict) else None


def journal_has(journal: Path, kind: str, node: str | None) -> bool:
    """Whether one **finished** record of `kind` (about `node`) is in the journal."""
    if not journal.is_file():
        return False
    for line in journal.read_text(errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        record = _record(line)
        if record is None:
            continue
        if record.get("kind") != kind:
            continue
        if node is None:
            return True
        labels = _mapping(record, "labels")
        payload = _mapping(record, "payload")
        if node in (labels.get("node"), payload.get("reference"), payload.get("node")):
            return True
    return False


#: What a repository identity in an example task may look like. The value is
#: interpolated into the `https://<identity>.git` origin every registration is
#: made against, so a task carrying a line that is not one would put arbitrary
#: text into a URL this capture then hands `git`.
IDENTITY = re.compile(r"^[A-Za-z0-9.-]+(?:/[A-Za-z0-9._-]+){2,}$")


def example_repositories() -> list[str]:
    """Every repository origin the shipped example tasks name."""
    found: set[str] = set()
    for task in sorted((repo_root() / "examples" / "plan-store" / "tasks").rglob("*.md")):
        listing = False
        for line in task.read_text().splitlines():
            if line.startswith("repositories:"):
                listing = True
                continue
            if listing and line.startswith("  - "):
                identity = line[4:].strip().strip('"')
                if not IDENTITY.match(identity):
                    raise SystemExit(
                        f"screenshots: {task.name} names the repository "
                        f"{identity!r}, which is not a `host/owner/name` identity. "
                        "This capture registers each one as an origin URL, so fix "
                        "the task's `repositories:` entry."
                    )
                found.add(identity)
                continue
            listing = False
    return sorted(found)


def run(argv: list[str], env: dict) -> None:
    done = subprocess.run(argv, env=env, capture_output=True, text=True)
    if done.returncode != 0:
        raise SystemExit(
            f"screenshots: building the capture's world failed at "
            f"`{' '.join(argv)}` ({done.returncode}). Fix what its own error "
            f"below names, then re-run `just screenshots`:\n{done.stderr}"
        )


def clear_stack_settings() -> None:
    """Unset every `ONE*_` setting this stack reads, before the capture sets its own.

    The scenes render the real binary, and this engine and both siblings read
    their settings from the environment ahead of most other layers — so a
    `ONEPIPELINE_RUNS_DIR` exported in the capturing shell points the whole
    journey somewhere else, and a `ONEHARNESS_*` rides into a dispatch. Either
    drifts the capture against a baseline taken in a clean shell.
    """
    prefixes = (
        "ONEPIPELINE_",
        "ONEVCS_",
        "ONETASKGRAPH_",
        "ONEAGENTGRAPH_",
        "ONEHARNESS_",
        "ONEJUDGE_",
        "ONEMESSAGEBUS_",
    )
    for name in [n for n in os.environ if n.startswith(prefixes)]:
        # The capture's own inputs are named with a `SHOTS_` prefix precisely so
        # that clearing the stack's settings cannot take them away.
        del os.environ[name]




def drive(world: World, attach_log: Path, while_held=None) -> None:
    """Drive the shipped six-node plan from launch to `complete`, attached.

    The journey is exactly what a planner does. `start --attach` streams the
    run's own monitor lines to stderr as the DAG is driven; each `kind: human`
    approval is cleared with `onepipeline attest`, as the README documents; and
    the last approval — the one the plan puts at the end, which nothing depends
    on — is reached after the driver has converged and handed the run back, so
    it takes an `adopt --attach` to resume, which is the product's own shape.
    Every attached stream is appended to `attach_log` in the order it was
    produced: that is the run's own append-only monitor document, and it is what
    the animated hero renders.

    **Nothing here races the driver.** An approval other nodes are blocked
    behind leaves the engine waiting on the channel rather than converging, so
    the attestation is read by the driver that is still attached however long
    the wait for it takes. The two lifecycle dispatches are held by the double
    until this capture writes their release file, so `while_held` is called with
    both of them genuinely in flight, and they are released one at a time so the
    order they settle in is stated here rather than decided by the host.
    """
    attach_log.write_text("")
    started = subprocess.Popen(
        [str(world.bin / "onepipeline"), "start", PLAN_PROJECT, "--attach"],
        env=world.env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        # The attached stream is on **stderr**: the descriptor split is
        # deliberate — human progress on stderr, the one machine-readable
        # settlement record on stdout — so the capture takes stderr and says so.
        stderr=attach_log.open("w"),
        text=True,
    )
    try:
        _clear_next_approval(world)
        world.until_journal("node-dispatched", HELD_DOCS)
        world.until_journal("node-dispatched", "service")
        if while_held is not None:
            # Called with both lifecycle dispatches genuinely in flight. It may
            # release a hold itself — the `watch` scene does, because what it is
            # a picture of is a bounded wait *returning* on a node settling —
            # and the releases below are written so that doing so changes
            # nothing about the order the run settles in.
            while_held()
        # Released one at a time, each release waited out on the journal before
        # the next, so the two lifecycle nodes settle in this order every run.
        world.release(HELD_DOCS)
        world.until_journal("node-settled", HELD_DOCS)
        world.release(HELD_SERVICE)
        # The lifecycle node's own human step, raised once its worker step has
        # settled and cleared the same way the plan's node-level approvals are.
        _clear_next_approval(world)
        world.until_journal("node-settled", "service")
        started.wait(timeout=DEADLINE_SECONDS)
    finally:
        if started.poll() is None:
            started.kill()
            started.wait()
    _expect_code(started.returncode, 0, "start --attach")

    # The last approval is the one the plan puts at the end, and **nothing
    # depends on it** — so the engine converges and hands the run back before
    # the decision is even raised, rather than waiting on the channel as it does
    # for the two an unfinished subtree sits behind. That is the product's own
    # shape, and it is why the tail of this journey is two `adopt --attach`
    # calls: the first raises the decision and hands the run back again, the
    # attestation clears it, and the second carries the run to `complete`.
    for _ in range(2):
        resumed = world.cli("adopt", PLAN_RUN, "--attach")
        with attach_log.open("a") as out:
            out.write(resumed.stderr)
        _expect(resumed, 0, "adopt --attach")
        if _pending_attestations(world.run_root / "events.jsonl"):
            _clear_next_approval(world)

    results = world.cli("results", PLAN_RUN)
    if not results.stdout.startswith(f"{PLAN_RUN}  complete"):
        raise SystemExit(
            "screenshots: the driven run did not reach `complete`, so the scenes "
            "would photograph a run that failed. " + REPAIR + "\n" + results.stdout
        )


def _clear_next_approval(world: World) -> None:
    """Wait for the next unanswered human action and attest it.

    The reference is read off the journal rather than restated here, so a plan
    that renamed an approval — or grew one — is followed rather than missed.
    """
    journal = world.run_root / "events.jsonl"
    reference: list[str] = []

    def pending() -> bool:
        waiting = _pending_attestations(journal)
        if waiting:
            reference.append(waiting[0])
            return True
        return False

    World._until(pending, f"an unanswered human action in {journal}")
    _expect(
        world.cli("attest", PLAN_RUN, reference[0]), 0, f"attest {reference[0]}"
    )
    world.until_journal("human-attested", reference[0])


def _pending_attestations(journal: Path) -> list[str]:
    """Every human action that has been raised and not yet attested, in order."""
    if not journal.is_file():
        return []
    raised: list[str] = []
    answered: set[str] = set()
    for line in journal.read_text(errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        record = _record(line)
        if record is None:
            continue
        payload = _mapping(record, "payload")
        kind = record.get("kind")
        if kind == "decision-pending" and payload.get("kind") == "attestation":
            reference = payload.get("reference")
            if reference and reference not in raised:
                raised.append(reference)
        elif kind == "human-attested":
            labels = _mapping(record, "labels")
            for name in (payload.get("reference"), labels.get("node")):
                if name:
                    answered.add(name)
    return [name for name in raised if name not in answered]


def expect(done: subprocess.CompletedProcess, code: int, what: str) -> None:
    """Refuse a command of the journey that did not end the way it has to."""
    _expect(done, code, what)


#: What to do about any journey step that ended wrong. The journey is this
#: repository's own offline fixtures driving its own binary, so a step that
#: ended wrong is a fixture or a binary that moved rather than a flake.
REPAIR = (
    "The capture drives the real binary against the fixtures in "
    "examples/plan-store and crates/testfakes. Read the output above, then "
    "either rebuild (`cargo build --release --locked`) if the binary is stale, "
    "or fix the fixture the step names. `just test-e2e shipped` drives the same "
    "plan through the suite and fails with more detail."
)


def _expect(done: subprocess.CompletedProcess, code: int, what: str) -> None:
    if done.returncode != code:
        raise SystemExit(
            f"screenshots: `{what}` exited {done.returncode}, expected {code}. "
            f"{REPAIR}\n{done.stdout}\n{done.stderr}"
        )


def _expect_code(actual: int, code: int, what: str) -> None:
    if actual != code:
        raise SystemExit(
            f"screenshots: `{what}` exited {actual}, expected {code}. {REPAIR}"
        )
