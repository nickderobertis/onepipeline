"""Drives watch.py as a subprocess against a `gh` double on PATH: python3 test_watch.py"""

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path

WATCH = Path(__file__).with_name("watch.py")

GH = r"""#!/usr/bin/env python3
import json, os, re, sys
path = sys.argv[-1]
state = os.environ["FAKE_STATE"]
with open(state + "/calls", "a") as calls:
    calls.write(" ".join(sys.argv[1:]) + "\n")
if path == "rate_limit" or "rate_limit" in sys.argv:
    print(os.environ.get("FAKE_RATE_LIMIT", "4000"))
    raise SystemExit(0)
fail = os.environ.get("FAKE_FAIL", "")
if fail and fail in path:
    print(os.environ["FAKE_FAIL_MESSAGE"], file=sys.stderr)
    raise SystemExit(1)
if "cache/storage-limit" in path:
    body = {"max_cache_size_gb": 10}
elif "cache/usage" in path:
    body = {"active_caches_size_in_bytes": 150, "active_caches_count": 150}
else:
    page = int(re.search(r"[?&]page=(\d+)", path).group(1))
    ids = range((page - 1) * 100, min(page * 100, 150))
    body = {"total_count": 150, "actions_caches": [
        {"id": i, "key": f"k{i}", "ref": "refs/heads/main", "size_in_bytes": 1, "version": "v",
         "created_at": "c", "last_accessed_at": "a"} for i in ids]}
print("HTTP/2.0 200 OK\r\nX-Ratelimit-Remaining: " + os.environ.get("FAKE_HEADER_REMAINING", "4000") + "\r\n\r\n" + json.dumps(body))
"""


class Watch(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        root = Path(self.dir.name)
        (root / "bin").mkdir()
        gh = root / "bin" / "gh"
        gh.write_text(GH.replace("/usr/bin/env python3", sys.executable), encoding="utf-8")
        gh.chmod(gh.stat().st_mode | stat.S_IEXEC)
        self.root = root
        self.out = root / "readings.jsonl"

    def run_watch(self, *args: str, **env: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(WATCH), "--repo", "o/r", "--out", str(self.out), *args],
            env={**os.environ, "PATH": f"{self.root / 'bin'}:{os.environ['PATH']}", "FAKE_STATE": str(self.root), **env},
            capture_output=True,
            text=True,
            check=False,
        )

    def readings(self) -> list[dict]:
        return [json.loads(line) for line in self.out.read_text(encoding="utf-8").splitlines()]

    def calls(self) -> list[str]:
        path = self.root / "calls"
        return path.read_text(encoding="utf-8").splitlines() if path.exists() else []

    def test_one_reading_reads_every_page_with_get_only(self) -> None:
        done = self.run_watch("--once")
        self.assertEqual(done.returncode, 0, done.stderr)
        reading = self.readings()[0]
        self.assertTrue(reading["complete"])
        self.assertEqual((reading["listed_count"], reading["pages"], reading["requests"]), (150, 2, 4))
        self.assertEqual(reading["max_cache_size_gb"], 10)
        self.assertNotIn("version", reading["entries"][0])
        self.assertTrue(all("--method GET" in call for call in self.calls() if "rate_limit" not in call))

    def test_a_second_reading_inside_two_minutes_is_refused_without_a_request(self) -> None:
        self.run_watch("--once")
        before = len(self.calls())
        done = self.run_watch("--once")
        self.assertEqual(done.returncode, 3)
        self.assertEqual(len(self.calls()), before)

    def test_a_reading_two_minutes_old_may_be_followed(self) -> None:
        stale = (datetime.now(timezone.utc) - timedelta(seconds=121)).strftime("%Y-%m-%dT%H:%M:%SZ")
        self.out.write_text(json.dumps({"repo": "o/r", "started_at": stale}) + "\n", encoding="utf-8")
        self.assertEqual(self.run_watch("--once").returncode, 0)

    def test_an_interval_under_two_minutes_is_refused(self) -> None:
        self.assertEqual(self.run_watch("--once", "--interval", "119").returncode, 2)
        self.assertEqual(self.calls(), [])

    def test_under_the_floor_makes_no_cache_request(self) -> None:
        done = self.run_watch("--once", FAKE_RATE_LIMIT="2499")
        self.assertEqual(done.returncode, 3)
        self.assertEqual(self.readings()[0]["skipped"], "under-floor")
        self.assertEqual([call for call in self.calls() if "rate_limit" not in call], [])

    def test_response_headers_under_the_floor_stop_the_listing(self) -> None:
        done = self.run_watch("--once", FAKE_HEADER_REMAINING="2499")
        self.assertEqual(done.returncode, 3)
        reading = self.readings()[0]
        self.assertFalse(reading["complete"])
        self.assertEqual(reading["requests"], 1)

    def test_a_rate_limit_refusal_ends_a_watch(self) -> None:
        done = self.run_watch("--hours", "0.01", FAKE_FAIL="cache/usage", FAKE_FAIL_MESSAGE="gh: You have exceeded a secondary rate limit (HTTP 403)")
        self.assertEqual(done.returncode, 4)
        self.assertEqual(self.readings()[0]["outcome"], "refused")

    def test_a_server_error_is_recorded_and_the_watch_goes_on(self) -> None:
        # Stops only at its own deadline, so it proves the watch outlived the failure.
        started = time.time()
        done = self.run_watch("--hours", "0.0005", FAKE_FAIL="cache/usage", FAKE_FAIL_MESSAGE="gh: Bad gateway (HTTP 502)")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertGreater(time.time() - started, 1)
        self.assertEqual(self.readings()[0]["outcome"], "unanswered")


if __name__ == "__main__":
    unittest.main()
