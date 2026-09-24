#!/usr/bin/env python3
"""The offline world the capture drives, and the journey it drives through it.

Which fixtures this composes and why, and the rule that every wait here polls a
file rather than a clock, are `AGENTS.md` beside it. Shared by `capture.py` and
`demo-gif.py` so the stills and the animated hero cannot disagree about what run
they photographed.
"""

# llmlint: ignore-file[changed_behavior_has_e2e] what this file *renders through* is a
# third-party tool `just check` does not install (`freeze`, `screencomp`), so a journey
# over a scene could only assert against stubs of those two; what it produces is checked
# instead, by the visual-docs workflow re-deriving the committed baseline on every pull
# request. The part that needs neither tool is journeyed rather than excused:
# `tests/visual_docs.rs` drives this file against real `git` for the two properties a
# rejected push turned up — which repository `clear_inherited_settings` leaves the world's
# git commands acting on, and that `run`'s refusal carries what the failing command said.

from __future__ import annotations

import json
import math
import os
import re
import shutil
import subprocess
import time
from pathlib import Path

def repo_root() -> Path:
    """The repository root: this file's directory is `screenshots/` under it."""
    return Path(__file__).resolve().parent.parent


# **Nothing below restates the fixtures.** Every plan id, run id and node id the
# journey names is read out of the shipped `examples/plan-store` documents at
# capture time, so a task that is renamed, regrouped or given another step moves
# the capture with it instead of leaving a copy here to go stale. What is stated
# is only *which* plan of that store this capture is about, by the role it plays
# — the multi-node one and the smallest one — and each is resolved below.

STORE = Path("examples/plan-store")

#: What a plan, node or step may be called. Every one of these becomes a command
#: argument, a path under the runs root, and a filename the doubles are scripted
#: from, so a document that had grown a space, a separator or a quote would
#: otherwise put it into all three.
NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")


def _named(value: str, what: str, where: Path) -> str:
    """One identifier read out of a fixture, or a refusal naming where it came from."""
    if not NAME.match(value):
        raise SystemExit(
            f"screenshots: {where} names the {what} {value!r}, which is not a plain "
            "identifier. This capture turns it into a command argument, a run "
            "directory and a script filename, so fix the document."
        )
    return value


def _source() -> str:
    """The one source the shipped store's own `onetaskgraph.yaml` declares."""
    document = repo_root() / STORE / "onetaskgraph.yaml"
    names = re.findall(r"^  ([A-Za-z0-9_-]+):$", document.read_text(), re.M)
    if len(names) != 1:
        raise SystemExit(
            f"screenshots: {document} declares {len(names)} sources, and this "
            "capture qualifies its plan ids with exactly one. Name which to use "
            "here, or restore the store to a single source."
        )
    return _named(names[0], "store source", document)


SOURCE = _source()


def _plan_named(name: str) -> str:
    """One shipped project document's `onepipeline.name`, refused if it moved."""
    document = repo_root() / STORE / "projects" / f"{name}.md"
    found = re.findall(r'"onepipeline\.name":\s*"([^"]+)"', document.read_text())
    if len(found) != 1:
        raise SystemExit(
            f"screenshots: {document} declares `onepipeline.name` {len(found)} "
            "times, and the capture reads one — it is what names the run every "
            "scene is of. Leave exactly one."
        )
    return _named(found[0], "plan", document)


def _nodes_of(plan: str) -> dict[str, list[str]]:
    """Each node id of a shipped plan, with the step ids it declares, in file order."""
    nodes: dict[str, list[str]] = {}
    for task in sorted((repo_root() / STORE / "tasks" / plan).glob("*.md")):
        text = task.read_text()
        node = re.search(r'"onepipeline\.id":\s*"([^"]+)"', text)
        if not node:
            continue
        id_ = _named(node.group(1), "node", task)
        if id_ in nodes:
            raise SystemExit(
                f"screenshots: two tasks under {STORE}/tasks/{plan} declare the "
                f"node id {id_!r}. A later one would replace the earlier here and "
                "the capture would drive a graph the plan does not hold."
            )
        nodes[id_] = [
            _named(step, "step", task)
            for step in re.findall(r'^\s*-\s*"id":\s*"([^"]+)"', text, re.M)
        ]
    if not nodes:
        raise SystemExit(
            f"screenshots: no task under {STORE}/tasks/{plan} declares an "
            "`onepipeline.id`, so the plan has no node for the capture to name."
        )
    return nodes


def _lifecycle_keys(plan: str) -> list[str]:
    """The doubles' script key for each node that targets a repository.

    A node's dispatch is scripted under its own id, and a node with steps under
    `<node>.<first step>` — which is the key `crates/testfakes` derives from the
    labels the engine passes it. Read off the documents so a plan that grew a
    step is held open at the right key rather than at a name that no longer
    dispatches anything.
    """
    keys = []
    for task in sorted((repo_root() / STORE / "tasks" / plan).glob("*.md")):
        text = task.read_text()
        node = re.search(r'"onepipeline\.id":\s*"([^"]+)"', text)
        if not node or "repositories:" not in text:
            continue
        node = _named(node.group(1), "node", task)
        steps = [
            _named(step, "step", task)
            for step in re.findall(r'^\s*-\s*"id":\s*"([^"]+)"', text, re.M)
        ]
        if not steps:
            # A node the engine dispatches directly: its own id is the key.
            keys.append(node)
            continue
        keys.append(f"{node}.{steps[0]}")
    return keys


#: The multi-node plan the scenes are of, and the smallest one, which the
#: capture also drives so the listing has a second project to group.
PLAN = _plan_named("tracked-release")
SMALL = _plan_named("single-node")
PLAN_PROJECT = f"{SOURCE}:{PLAN}"
PLAN_RUN = PLAN
SMALL_PROJECT = f"{SOURCE}:{SMALL}"
SMALL_RUN = SMALL

#: The lifecycle dispatches the capture holds open. Holding them is what lets
#: `status` be read with work genuinely in flight, gives `watch` something to
#: return on, and — because they are released one at a time — fixes the order
#: they settle in, which a host would otherwise decide per run.
HELD = _lifecycle_keys(PLAN)
SMALL_KEYS = _lifecycle_keys(SMALL)

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

POLL_SECONDS = 0.02


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
        # first (see `clear_inherited_settings`), so nothing an operator exported can
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
        env = {n: v for n, v in os.environ.items() if not n.startswith("GIT_")}
        env.update(
            {
                "PATH": path,
                "HOSTNAME": HOST,
                # Every git setting this world runs under is stated here, and
                # nothing inherited survives the comprehension above: a hook's
                # `GIT_DIR` would make the seed commits below land in the
                # repository being pushed (see `clear_inherited_settings`), and
                # a host `/etc/gitconfig` would decide what these repositories
                # are, which is this world's to say.
                "GIT_CONFIG_GLOBAL": str(self.root / "gitconfig"),
                "GIT_CONFIG_NOSYSTEM": "1",
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
        for key in HELD + SMALL_KEYS:
            # What each lifecycle worker "did": a file in the worktree, so the
            # node's branch carries a diff and the node publishes rather than
            # settling `empty-branch`.
            self.script(f"{key}.work", "What this node was asked for, delivered.\n")
        for key in HELD:
            # The holds. Each lifecycle dispatch of the multi-node plan waits
            # for a file this capture writes, which is what stands the run still
            # while `status` and `watch` are read and what makes the order they
            # settle in this capture's to state rather than the host's.
            self.script(f"{key}.wait", "")
            # And each announces its member and opens its turn before it waits,
            # as a real dispatch does, so a `status` read during the hold shows
            # work that has said something rather than one that has recorded
            # nothing yet.
            self.script(f"{key}.turn-open", "")

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


def said(done: subprocess.CompletedProcess) -> str:
    """Everything a failing command said, on whichever stream it said it.

    A diagnostic that quotes `stderr` alone arrives **empty** for a command that
    reports on stdout, and `git` is one of those: `git commit` with nothing
    staged exits non-zero having written `nothing to commit` to *stdout*. That
    is not hypothetical — it is how a capture once failed, under the `GIT_DIR`
    defect `clear_inherited_settings` now closes, with the words "Fix what its
    own error below names" followed by nothing at all. A reader given no
    diagnostic cannot tell a silent command from a message that was thrown away,
    so both streams are reported, each named, and a command that genuinely said
    nothing says so in words.
    """
    parts = [
        f"{stream}:\n{text.rstrip()}"
        for stream, text in (("stdout", done.stdout or ""), ("stderr", done.stderr or ""))
        if text.strip()
    ]
    return "\n".join(parts) or "(the command wrote nothing to stdout or to stderr)"


def run(argv: list[str], env: dict) -> None:
    done = subprocess.run(argv, env=env, capture_output=True, text=True)
    if done.returncode != 0:
        raise SystemExit(
            f"screenshots: building the capture's world failed at "
            f"`{' '.join(argv)}` ({done.returncode}). Fix what its own error "
            f"below names, then re-run `just screenshots`:\n{said(done)}"
        )


def clear_inherited_settings() -> None:
    """Unset every `ONE*_` and `GIT_*` setting, before the capture states its own.

    The scenes render the real binary, and this engine and both siblings read
    their settings from the environment ahead of most other layers — so a
    `ONEPIPELINE_RUNS_DIR` exported in the capturing shell points the whole
    journey somewhere else, and a `ONEHARNESS_*` rides into a dispatch. Either
    drifts the capture against a baseline taken in a clean shell.

    `GIT_*` is the same hazard with teeth, because one of this capture's callers
    is a **git hook**, and git runs a hook with `GIT_DIR` naming the repository
    being pushed. `GIT_DIR` beats `-C` and beats discovery, so an inherited one
    aims the world's own `git add -A && git commit` at that repository instead
    of at the throwaway one. The whole prefix goes rather than the dangerous
    names, which would be a list to keep current against git's releases:
    `_environment` then states the handful the world actually wants.
    """
    prefixes = (
        "ONEPIPELINE_",
        "ONEVCS_",
        "ONETASKGRAPH_",
        "ONEAGENTGRAPH_",
        "ONEHARNESS_",
        "ONEJUDGE_",
        "ONEMESSAGEBUS_",
        "GIT_",
    )
    for name in [n for n in os.environ if n.startswith(prefixes)]:
        # The capture's own inputs are named with a `SHOTS_` prefix precisely so
        # that clearing the inherited settings cannot take them away.
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
        for key in HELD:
            world.until_journal("node-dispatched", key.split(".", 1)[0])
        if while_held is not None:
            # Called with both lifecycle dispatches genuinely in flight. It may
            # release a hold itself — the `watch` scene does, because what it is
            # a picture of is a bounded wait *returning* on a node settling —
            # and the releases below are written so that doing so changes
            # nothing about the order the run settles in.
            while_held()
        # Released one at a time, each release waited out on the journal before
        # the next, so the lifecycle nodes settle in this order every run. A
        # node whose worker step is followed by a human one raises its own
        # approval on the way, cleared exactly as the plan's node-level ones are.
        for key in HELD:
            node = key.split(".", 1)[0]
            world.release(key)
            if _pending_attestations(world.run_root / "events.jsonl") or "." in key:
                _clear_next_approval(world)
            world.until_journal("node-settled", node)
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
        # Only a string is a reference: this value becomes the argument of an
        # `attest` the journey then runs, and the journal is what the binary
        # under test wrote rather than something this capture composed.
        if kind == "decision-pending" and payload.get("kind") == "attestation":
            reference = payload.get("reference")
            if isinstance(reference, str) and reference and reference not in raised:
                raised.append(reference)
        elif kind == "human-attested":
            labels = _mapping(record, "labels")
            for name in (payload.get("reference"), labels.get("node")):
                if isinstance(name, str) and name:
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
            f"{REPAIR}\n{said(done)}"
        )


def _expect_code(actual: int, code: int, what: str) -> None:
    if actual != code:
        raise SystemExit(
            f"screenshots: `{what}` exited {actual}, expected {code}. {REPAIR}"
        )
