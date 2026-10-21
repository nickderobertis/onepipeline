#!/usr/bin/env python3
"""THROWAWAY — re-capture every screenshot of the tracker-dependency-reads spike-visual report.

    python3 spike/tracker-holds/capture.py OUT_DIR

Each pair is the same command, in the same world, at the same terminal size (the
gallery's 118 columns): the before is the unchanged base binary
(`target/release/onepipeline`, built from this branch's base and untouched by it);
the after is `mock.py` wrapping that binary. Run through `capture.sh`, which builds
the binaries once. The world is `screenshots/world.py`'s offline one, plus a source
`vault` declared private: a real `scripted-source` that answers every task query
rate limited.
"""

import json
import os
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "screenshots"))
import capture as gallery  # noqa: E402  (render(): the gallery's own freeze call)
import normalise  # noqa: E402
import world as W  # noqa: E402

BIN = W.repo_root() / "target" / "release"
MOCK = HERE / "mock.py"
RATE_LIMITED = "Rate limit exceeded"


def plan(store: Path, name: str, tasks: list[tuple[str, str]]) -> str:
    (store / "projects" / f"{name}.md").write_text(
        f'---\ntitle: "{name}"\nmetadata:\n  "onepipeline.name": "{name}"\n'
        f'  "onepipeline.schema_version": 3\n  "onepipeline.goal":\n'
        f'    "text": "Deliver {name}"\n---\n'
    )
    folder = store / "tasks" / name
    folder.mkdir(parents=True)
    for node, repo in tasks:
        (folder / f"{node}.md").write_text(
            f'---\ntitle: "feat: {node}"\nproject: "{name}"\nclassification: public\nrepositories:\n  - "{repo}"\n'
            f'metadata:\n  "onepipeline.id": "{node}"\n  "onepipeline.persona": "engineer"\n---\n\n'
            f"## What\nDo {node}.\n\n## Why\nSo the run settles.\n\n## Acceptance criteria\n- {node} is done.\n"
        )
    return f"{W.SOURCE}:{name}"


def world(root: Path, retry_after) -> W.World:
    w = W.World(root, BIN)
    w.build()
    vault = root / "vault"
    vault.mkdir()
    p = "ONETASKGRAPH_SOURCES__VAULT"
    w.env.update(
        {
            "ONETASKGRAPH_SOURCES__PLANS__VISIBILITY": "public",
            f"{p}__PLUGIN": "subprocess",
            f"{p}__VISIBILITY": "private",
            f"{p}__CONFIG__COMMAND": str(BIN / "scripted-source"),
            f"{p}__CONFIG__SETTINGS__ROOT": str(vault),
            f"{p}__CONFIG__SETTINGS__SCRIPT": str(w.fakes),
            f"{p}__CONFIG__SETTINGS__KEY": "vault",
            "SPIKE_BASE_BIN": str(BIN / "onepipeline"),
        }
    )
    w.script(
        "vault.query_tasks.refuse",
        json.dumps(
            {"kind": "rate-limited", "retry_after_seconds": retry_after, "message": RATE_LIMITED}
        ),
    )
    return w


def run(w: W.World, mocked: bool, edged: str, args: list[str], stream: str) -> str:
    argv = [sys.executable, str(MOCK), *args] if mocked else [str(BIN / "onepipeline"), *args]
    env = dict(w.env, SPIKE_EDGED_NODES=edged)
    done = subprocess.run(argv, env=env, capture_output=True, text=True, stdin=subprocess.DEVNULL)
    if done.returncode != 0:
        raise SystemExit(f"{args} exited {done.returncode}:\n{done.stdout}\n{done.stderr}")
    return done.stdout if stream == "stdout" else done.stderr


def shot(out: Path, name: str, w: W.World, shown: str, text: str) -> None:
    paths = {
        str(w.root / "onevcs-home"): "/home/you/.onevcs",
        str(w.runs): "/home/you/onepipeline/runs",
        str(w.store): "/home/you/plans",
        str(w.root): "/home/you/onepipeline",
    }
    body = normalise.Normaliser(paths).text(f"$ {shown}\n{text.rstrip()}\n")
    gallery.render(body, out / f"{name}.png")
    (out / f"{name}.txt").write_text(body)


def until_held(w: W.World, run_id: str, count: int) -> None:
    journal = w.runs / run_id / "events.jsonl"
    deadline = time.time() + 60
    while time.time() < deadline:
        if journal.exists():
            queued = [
                line for line in journal.read_text().splitlines()
                if '"planner-surface-queued"' in line and "source-wait" in line
            ]
            if len(queued) >= count:
                return
        time.sleep(0.2)
    raise SystemExit(f"{run_id}: no source-wait surfaced")


def main() -> int:
    out = Path(sys.argv[1]).resolve()
    out.mkdir(parents=True, exist_ok=True)
    scratch = Path(os.environ.get("ONEPIPELINE_NODE_SCRATCH_DIR", "/tmp")) / "spike-tracker-holds"
    W.clear_inherited_engine_and_git_settings()
    public = [("api-tests", "github.com/nickderobertis/some-service"),
              ("docs-refresh", "github.com/nickderobertis/some-docs")]
    edged = [("changelog", "github.com/nickderobertis/some-docs"),
             ("service-fix", "github.com/nickderobertis/some-service")]

    # 1 + 2: a provider that gave a reset (30 minutes out).
    for mocked, tag in ((False, "before"), (True, "after")):
        w = world(scratch / f"reset-{tag}", 1800)
        a = plan(w.store, "public-only", public)
        b = plan(w.store, "vault-edge", edged)
        run(w, mocked, "", ["start", a, "--detach"], "stderr")
        until_held(w, "public-only", 2)
        cmd = ["status", "public-only", "--no-providers"]
        shot(out, f"public-plan-status-{tag}", w, "onepipeline " + " ".join(cmd),
             run(w, mocked, "", cmd, "stdout"))
        w.env["SPIKE_LAUNCHED_AT"] = str(time.time())
        cmd = ["start", b, "--detach"]
        shot(out, f"launch-notice-reset-{tag}", w, "onepipeline " + " ".join(cmd) + " 2>&1 >/dev/null",
             run(w, mocked, "changelog", cmd, "stderr"))
        for r in ("public-only", "vault-edge"):
            run(w, False, "", ["stop", r], "stdout")

    # 3: a provider that gave no reset time.
    for mocked, tag in ((False, "before"), (True, "after")):
        w = world(scratch / f"noreset-{tag}", None)
        b = plan(w.store, "vault-edge", edged)
        run(w, False, "", ["start", b, "--detach"], "stderr")
        until_held(w, "vault-edge", 1)
        cmd = ["next", "vault-edge", "--filter", '{"include":[{"kind":"none"}]}']
        shown = "onepipeline next vault-edge --filter '{\"include\":[{\"kind\":\"none\"}]}'"
        shot(out, f"source-wait-backoff-{tag}", w, shown, run(w, mocked, "changelog", cmd, "stdout"))
        run(w, False, "", ["stop", "vault-edge"], "stdout")
    print(f"captured into {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
