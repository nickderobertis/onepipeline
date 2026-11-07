"""Drives pr_files and q6 against a `gh` double on PATH, through the real subprocess call: python3 test_history.py"""

from __future__ import annotations

import contextlib
import io
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import analyse  # noqa: E402
import history as h  # noqa: E402

GH = r"""#!/usr/bin/env python3
import json, os, re, sys
path = sys.argv[-1]
if "rate_limit" in sys.argv:
    print(os.environ.get("FAKE_RATE_LIMIT", "4000"))
    raise SystemExit(0)
with open(os.environ["FAKE_CALLS"], "a") as calls:
    calls.write(path + "\n")
if os.environ.get("FAKE_FAIL"):
    print("gh: Forbidden (HTTP 403)", file=sys.stderr)
    raise SystemExit(1)
page = int(re.search(r"[?&]page=(\d+)", path).group(1))
total = int(os.environ["FAKE_TOTAL"])
names = [f"f{i}" for i in range(total)] + ["justfile"]
print(json.dumps([{"filename": n} for n in names[(page - 1) * 100:page * 100]]))
"""


def pr(number: int, total: int, title: str = "fix: x", **times: str) -> dict:
    nodes = [{"path": f"f{i}"} for i in range(min(total, 100))]
    return {"number": number, "title": title, "files": {"totalCount": total, "nodes": nodes}, "headRefOid": f"h{number}", **times}


class Files(unittest.TestCase):
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        (self.root / "bin").mkdir()
        gh = self.root / "bin" / "gh"
        gh.write_text(GH.replace("/usr/bin/env python3", sys.executable), encoding="utf-8")
        gh.chmod(gh.stat().st_mode | stat.S_IEXEC)
        self.calls = self.root / "calls"
        saved = dict(os.environ)
        self.addCleanup(lambda: (os.environ.clear(), os.environ.update(saved)))
        os.environ["PATH"] = f"{self.root / 'bin'}:{saved['PATH']}"
        os.environ["FAKE_CALLS"] = str(self.calls)
        self.store = h.Store(self.root / "cache")

    def requested(self) -> list[str]:
        return self.calls.read_text(encoding="utf-8").splitlines() if self.calls.exists() else []

    def test_a_short_list_needs_no_request(self) -> None:
        self.assertEqual(h.pr_files(self.store, "r", pr(1, 2)), ["f0", "f1"])
        self.assertEqual(self.requested(), [])

    def test_every_page_of_a_long_list_is_read_once(self) -> None:
        os.environ["FAKE_TOTAL"] = "261"  # with justfile, 262 paths over three pages
        paths = h.pr_files(self.store, "r", pr(2, 262))
        self.assertEqual(len(paths), 262)
        self.assertIn("justfile", paths)
        self.assertEqual(len(self.requested()), 3)
        h.pr_files(self.store, "r", pr(2, 262))
        self.assertEqual(len(self.requested()), 3)

    def test_a_list_short_of_its_total_is_refused(self) -> None:
        os.environ["FAKE_TOTAL"] = "149"
        with self.assertRaisesRegex(h.IncompleteFiles, "read 150 of 262"):
            h.pr_files(self.store, "r", pr(3, 262))

    def _q6(self) -> str:
        moved = pr(11, 1, mergedAt="2026-11-03T00:00:00Z")
        other = pr(12, 150, mergedAt="2026-11-02T00:00:00Z")
        analyse.h.merged_prs = lambda *_: [other, moved]  # type: ignore[assignment]
        analyse.h.workflow_runs = lambda *_: [  # type: ignore[assignment]
            {"event": "pull_request", "head_sha": "h11", "created_at": "2026-11-01T00:00:00Z"}
        ]
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            analyse.q6(self.store, "onepipeline")
        return output.getvalue()

    def test_q6_counts_a_global_input_beyond_the_first_hundred(self) -> None:
        os.environ["FAKE_TOTAL"] = "149"
        merged, runs = h.merged_prs, h.workflow_runs
        self.addCleanup(lambda: (setattr(h, "merged_prs", merged), setattr(h, "workflow_runs", runs)))
        report = self._q6()
        self.assertIn("{'justfile': 1}", report)
        self.assertIn("#12 150 of 150", report)
        self.assertNotIn("LOWER BOUNDS", report)

    def test_q6_marks_lower_bounds_when_the_read_is_refused(self) -> None:
        os.environ["FAKE_FAIL"] = "1"
        merged, runs = h.merged_prs, h.workflow_runs
        self.addCleanup(lambda: (setattr(h, "merged_prs", merged), setattr(h, "workflow_runs", runs)))
        report = self._q6()
        self.assertIn("LOWER BOUNDS", report)
        self.assertIn("#12 100 of 150", report)
        self.assertIn("pulls/<n>/files", report)

    def test_q6_marks_lower_bounds_under_the_rate_floor_without_a_request(self) -> None:
        os.environ["FAKE_RATE_LIMIT"] = "2499"
        merged, runs = h.merged_prs, h.workflow_runs
        self.addCleanup(lambda: (setattr(h, "merged_prs", merged), setattr(h, "workflow_runs", runs)))
        report = self._q6()
        self.assertIn("LOWER BOUNDS", report)
        self.assertEqual(self.requested(), [])


if __name__ == "__main__":
    unittest.main()
