"""THROWAWAY spike mock for `authoring:pr-feedback-rework` — never landed.

Renders the "after" pictures of the five terminal views that plan changes, from
hand-written fixture text, through this repository's own capture renderer
(`capture.render`: the pinned `freeze` and the vendored font). Nothing here runs
the engine: the text is what the plan's views are expected to print for a run of
five lifecycle nodes on a team identity stacked `a <- b <- c` and `a, d <- e`.

    python3 screenshots/spike_pr_feedback_rework.py OUT_DIR

writes `OUT_DIR/after-<scene>.svg` for every scene below.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import capture  # noqa: E402

RUN = "pr-feedback-rework-demo"
REPO = "github.com/acme/widgets"
PR = "https://github.com/acme/widgets/pull"
TAIL = (
    "  free space: 184.2 GiB of 460.0 GiB (40% free) on the filesystem holding the runs root "
    "/home/you/onepipeline/runs and\nthe lifecycle workspaces under /home/you/.onevcs/workspaces\n"
    "  providers: fake-provider: 1 identity bound, 0% utilized\n"
)

STACKED = f"""{RUN}  ACTIVE  NO OBSERVER  0/5 done
  a: draft {PR}/101 — drafted against main
  b: draft {PR}/102 — drafted against a's branch (#101)
  c: draft {PR}/103 — drafted against b's branch (#102)
  d: draft {PR}/104 — drafted against main
  e: draft {PR}/105 — drafted against stack base onepipeline/{RUN}/base-e (a #101 + d #104)
{TAIL}"""

REWORKED = f"""{RUN}  ACTIVE  NO OBSERVER  0/5 done
  a: draft {PR}/101 — drafted against main
  b: reworked from 2 review comment(s) — still a draft {PR}/102 against a's branch (#101)
     reason: rename --stack-base to --base and cover the empty-stack case
     {PR}/102#discussion_r1873001
     {PR}/102#discussion_r1873002
  c: draft {PR}/103 — drafted against b's branch (#102)
  d: draft {PR}/104 — drafted against main
  e: draft {PR}/105 — drafted against stack base onepipeline/{RUN}/base-e (a #101 + d #104)
{TAIL}"""

RESTACKED = f"""{RUN}  ACTIVE  NO OBSERVER  0/5 done
  a: reworked from 1 review comment(s) — still a draft {PR}/101 against main
     reason: keep the plan loader's error message stable
     {PR}/101#discussion_r1873010
  b: reworked from 2 review comment(s) — still a draft {PR}/102 against a's branch (#101)
     reason: rename --stack-base to --base and cover the empty-stack case
     {PR}/102#discussion_r1873001
     {PR}/102#discussion_r1873002
  c: restacked onto b at 4e1c9a7 — applied cleanly, no agent turn; draft {PR}/103
  d: draft {PR}/104 — drafted against main
  e: restacked onto stack base (a + d) at 9b03f52 — conflict in src/plan.rs resolved by a worker
     (1 turn, 2m 14s); draft {PR}/105
{TAIL}"""

NEXT = (
    '{"status":"running","surface":{"kind":"review-comment","source":"vcs","blocking":false,'
    '"workstream":"d","queued_at":1777893601000,"message":"review comment on d\'s draft '
    f'{PR}/104 by @rivera-review at src/graph.rs:212: \\"this loses the edge when the '
    'parent is dropped; keep it until the restack settles\\". Answer with one of: reply '
    '{\\"op\\":\\"rework\\",\\"id\\":\\"d\\",\\"comments\\":[...]} | '
    '{\\"op\\":\\"context\\",\\"id\\":\\"d\\"} | {\\"op\\":\\"dismiss-comment\\"}",'
    '"review_comment":{"node":"d","change_request":"' + PR + '/104","author":"rivera-review",'
    '"location":{"path":"src/graph.rs","line":212},"url":"' + PR + '/104#discussion_r1873020",'
    '"ops":["rework","context","dismiss-comment"]}},"events":[{"v":2,"ts":"2026-05-04T11:20:00.000Z",'
    '"stream":"shots-host-41000","seq":9,"source":"pipeline","kind":"node-dispatched",'
    f'"labels":{{"run_id":"{RUN}","node":"d"}},"payload":{{"persona":"engineer","attempt":1}},'
    '"artifacts":[]}]}\n'
)

CONTINUED = f"""{RUN}  ACTIVE  NO OBSERVER  0/5 done
  launched on build-host-2 from the store: 5 node(s) continued, none re-run
  a: continued from the store — branch team/{RUN}/a, change request {PR}/101 (draft)
  b: continued from the store — branch team/{RUN}/b, change request {PR}/102 (draft)
  c: continued from the store — branch team/{RUN}/c, change request {PR}/103 (draft)
  d: continued from the store — branch team/{RUN}/d, change request {PR}/104 (draft)
  e: continued from the store — branch team/{RUN}/e, change request {PR}/105 (draft)
{TAIL}"""

SCENES = {
    "stacked-drafts": STACKED,
    "rework": REWORKED,
    "restack": RESTACKED,
    "review-comment": NEXT,
    "continued": CONTINUED,
}


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    for name, text in SCENES.items():
        capture.render(text, out / f"after-{name}.svg")
        print(out / f"after-{name}.svg")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
