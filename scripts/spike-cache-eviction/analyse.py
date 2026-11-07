#!/usr/bin/env python3
"""spike-cache-eviction's arithmetic over readings.jsonl and history.py's cache.

    python3 scripts/spike-cache-eviction/analyse.py --cache DIR --readings FILE \\
        q1 q2 episodes trigger q3 q4 q5 q6 q7_restores q8

evidence/figures.txt is that command's output as it stood when the report was written.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import re
import sys
from collections import Counter, defaultdict
from datetime import datetime
from pathlib import Path
from statistics import median

sys.path.insert(0, str(Path(__file__).parent))
import history as h  # noqa: E402

REPOS = {
    "onepipeline": {"warm": "rust-cache-warm.yml"},
    "onetaskgraph": {"warm": "rust-cache-warm.yml"},
    "onepipeline-ui": {"warm": "ci-cache.yml"},
}
VERSION_FREE_NPM = ["package.json", "bun.lock", "package-lock.json", "pnpm-lock.yaml"]
VERSION_FREE_CARGO = ["Cargo.toml", "Cargo.lock"]
VERSION_FREE_PYTHON = ["pyproject.toml", "uv.lock"]
# nx.json `sharedGlobals` of each repository at 2026-11-06, with its runtime version-free inputs spelled
# out as the files they read. The manifests are read with the version removed, so a release's
# version bump does not move them; a dependency change does.
GLOBALS = {
    "onepipeline": [
        "nx.json", ".github/ci-tool-versions.json", ".github/read-ci-tool-versions.mjs",
        ".github/onevcs-cli-version", "justfile", "scripts/nx-version-free-inputs.mjs",
        ".github/nx-version-files", *[f"**{name}" for name in VERSION_FREE_NPM],
    ],
    "onetaskgraph": [
        ".python-version", ".tool-versions", "rust-toolchain.toml", ".cargo/config.toml", "clippy.toml",
        "rustfmt.toml", "deny.toml", "biome.json", *[f"**{name}" for name in VERSION_FREE_CARGO + VERSION_FREE_NPM],
    ],
    "onepipeline-ui": [
        "nx.json", "rust-toolchain.toml", "rustfmt.toml", "clippy.toml", "biome.json", "eslint.config.mjs",
        "tsconfig.base.json", "scripts/nx-version-free-inputs.mjs", ".github/nx-version-files",
        *[f"**{name}" for name in VERSION_FREE_NPM + VERSION_FREE_CARGO + VERSION_FREE_PYTHON],
    ],
}


def is_release(pr: dict) -> bool:
    return pr["title"].startswith("chore: release")


def is_global(repo: str, path: str) -> bool:
    return any(
        path.split("/")[-1] == pattern[2:] if pattern.startswith("**") else fnmatch.fnmatch(path, pattern)
        for pattern in GLOBALS[repo]
    )


def pct(values: list[float], q: float) -> float:
    """Nearest rank."""
    ordered = sorted(values)
    return ordered[max(0, min(len(ordered) - 1, int(round(q * len(ordered) + 0.4999)) - 1))]


def minutes(later: str, earlier: str) -> float:
    return (h.ts(later) - h.ts(earlier)).total_seconds() / 60


def final_ci(ci_runs: list[dict]) -> dict[str, dict]:
    """The newest pull_request CI run of each head sha."""
    newest: dict[str, dict] = {}
    for run in ci_runs:
        if run["event"] == "pull_request":
            newest[run["head_sha"]] = run
    return newest


def promotion_times(store: h.Store, repo: str) -> tuple[dict[str, str], float]:
    """Merge sha to when its release-plz run's promote step finished, and the median offset from merge."""
    done: dict[str, str] = {}
    offsets: list[float] = []
    for run in h.workflow_runs(store, repo, "release-plz.yml", since=datetime(2026, 11, 4, tzinfo=h.SINCE.tzinfo)):
        for job in h.jobs(store, repo, run["id"]):
            for step in job.get("steps", []):
                if "romot" in step["name"] and step.get("completed_at") and step["conclusion"] == "success":
                    done[run["head_sha"]] = step["completed_at"]
                    offsets.append(minutes(step["completed_at"], run["created_at"]))
    return done, median(offsets) if offsets else 0.5


def q4(store: h.Store, repo: str) -> None:
    prs = h.merged_prs(store, repo)
    releases = [pr for pr in prs if is_release(pr)]
    ordinary = [pr for pr in prs if not is_release(pr)]
    ci = final_ci(h.workflow_runs(store, repo, "ci.yml"))
    horizon = []
    pending = 0
    for pr in ordinary:
        following = next((r for r in releases if r["mergedAt"] > pr["mergedAt"]), None)
        if following is None:
            pending += 1
        else:
            horizon.append(minutes(following["mergedAt"], pr["mergedAt"]))
    figures = {"p50": pct(horizon, 0.5), "p90": pct(horizon, 0.9), "max": max(horizon)}
    print(f"{repo}: horizon n={len(horizon)} (no release yet {pending}) "
          + " ".join(f"{k}={v:.0f}m" for k, v in figures.items()))
    promoted, offset = promotion_times(store, repo)
    delays: list[tuple[float, int, int]] = []
    for pr in ordinary:
        sha = pr["mergeCommit"]["oid"] if pr["mergeCommit"] else None
        promotion = promoted.get(sha, "")
        promo_min = h.ts(promotion) if promotion else None
        # The release pull request that replays this entry: the first released one whose final CI began after promotion.
        for release in releases:
            run = ci.get(release["headRefOid"])
            if run is None:
                continue
            start = run["created_at"]
            gap = minutes(start, promotion) if promo_min else minutes(start, pr["mergedAt"]) - offset
            if gap > 0:
                delays.append((gap, pr["number"], release["number"]))
                break
    gaps = [d[0] for d in delays]
    worst = max(delays)
    print(f"  promotion to release PR final CI start: n={len(gaps)} p50={pct(gaps, .5):.0f}m p90={pct(gaps, .9):.0f}m "
          f"max={worst[0]:.0f}m (PR #{worst[1]} to release #{worst[2]}); observed promote offset median {offset:.1f}m, "
          f"{len(promoted)} exact promotion times")
    for name, value in figures.items():
        over = sum(1 for gap in gaps if gap > value)
        print(f"  over {name} ({value:.0f}m): {over}/{len(gaps)} = {100 * over / len(gaps):.1f}%")
    for value in (60, 180, 360, 720):
        over = sum(1 for gap in gaps if gap > value)
        print(f"  over {value}m: {over}/{len(gaps)} = {100 * over / len(gaps):.1f}%")


def q6(store: h.Store, repo: str) -> None:
    prs = h.merged_prs(store, repo)
    ci = final_ci(h.workflow_runs(store, repo, "ci.yml"))
    moved = []
    no_ci = 0
    for pr in prs:
        run = ci.get(pr["headRefOid"])
        if run is None:
            no_ci += 1
            continue
        between = [o for o in prs if run["created_at"] < o["mergedAt"] < pr["mergedAt"]]
        if between:
            moved.append((pr, run, between))
    ordinary_moved = [m for m in moved if not is_release(m[0])]
    substantive = [m for m in moved if any(not is_release(o) for o in m[2])]
    print(f"{repo}: merged {len(prs)} (release {sum(map(is_release, prs))}), no CI run found for head {no_ci}; "
          f"base moved {len(moved)} ({100 * len(moved) / len(prs):.1f}%), of them ordinary PRs {len(ordinary_moved)}, "
          f"with a non-release intervening merge {len(substantive)}")
    files: Counter[str] = Counter()
    global_hits: Counter[str] = Counter()
    hit_prs = 0
    truncated = 0
    for pr, run, between in moved:
        hit = False
        for other in between:
            truncated += other["files"]["totalCount"] > 100
            for node in other["files"]["nodes"]:
                files[node["path"]] += 1
                if is_global(repo, node["path"]) and not is_release(other):
                    global_hits[node["path"]] += 1
                    hit = True
        hit_prs += hit
    print(f"  moved PRs: {[(m[0]['number'], [o['number'] for o in m[2]]) for m in moved]}")
    print(f"  intervening files changed: {len(files)} distinct; top {files.most_common(12)}")
    print(f"  in global inputs (excluding release merges, whose version edits are normalised): {dict(global_hits)}; "
          f"{hit_prs} moved PRs hit a global; intervening PRs with >100 files, list truncated: {truncated}")


SENT = re.compile(r"^(\S+) Sent (\d+) of (\d+) \(100\.0%\)")
SENT_START = re.compile(r"^(\S+) Sent 0 of (\d+)")
SAVED_KEY = re.compile(r"Cache saved with key: (\S+)")


def saves(text: str) -> list[dict]:
    """Every upload a job's log shows: its key, bytes, and when it started and finished."""
    found: list[dict] = []
    rust_key = None
    key = None
    start = None
    lines = text.splitlines()
    for number, line in enumerate(lines):
        if line.rstrip().endswith("Cache Key:") and number + 1 < len(lines):
            rust_key = lines[number + 1].split()[-1]
        # A rust-cache key names only the upload that follows its own "Saving cache" line; any
        # other upload in the post steps (setup-uv's, actions/cache/save) names itself or nothing.
        if "Saving cache" in line:
            key = rust_key
        match = SENT_START.match(line)
        if match and start is None:
            start = match.group(1)
        match = SENT.match(line)
        if match:
            found.append({"key": key, "bytes": int(match.group(3)), "started": start, "finished": match.group(1)})
            start = None
            key = None
        match = SAVED_KEY.search(line)
        if match and found and found[-1]["key"] is None:
            found[-1]["key"] = match.group(1)
    return found


def warm_saves(store: h.Store, repo: str) -> list[dict]:
    """Every save of every warm leg, oldest first, with the leg and run around it."""
    out = []
    for run in h.workflow_runs(store, repo, REPOS[repo]["warm"]):
        for job in h.jobs(store, repo, run["id"]):
            if job.get("conclusion") in (None, "skipped"):
                continue
            for save in saves(store.log(repo, job["id"]) or ""):
                out.append({**save, "run": run["id"], "job": job["id"], "leg": job["name"], "sha": run["head_sha"],
                            "run_created": run["created_at"], "leg_finished": job["completed_at"]})
    return sorted(out, key=lambda s: s["finished"])


def family(key: str | None) -> str:
    """Hash-free name: a rust-cache key's prefix, or the leading words of another key."""
    if key is None:
        return "(unnamed)"
    match = re.match(r"(v0-rust-.*?-(?:Linux|Windows|Darwin)-[A-Za-z0-9_]+)-", key)
    if match:
        return match.group(1)
    match = re.match(r"(ci-capture|bootstrap-tools|onepipeline-cli|onepipeline-api-baseline|nx-cache)", key)
    return match.group(1) if match else key[:30]


def q5(store: h.Store, repo: str) -> None:
    by_family: dict[str, list[dict]] = defaultdict(list)
    for save in warm_saves(store, repo):
        by_family[family(save["key"])].append(save)
    print(f"{repo}:")
    for name, entries in sorted(by_family.items()):
        sizes = [e["bytes"] for e in entries]
        steps = [(b["bytes"] - a["bytes"], a, b) for a, b in zip(entries, entries[1:])]
        biggest = max(steps, key=lambda step: step[0]) if steps else None
        print(f"  {name}: saves={len(sizes)} first={sizes[0] / 1e9:.3f}GB ({entries[0]['finished'][:16]}) "
              f"last={sizes[-1] / 1e9:.3f}GB ({entries[-1]['finished'][:16]}) median={median(sizes) / 1e9:.3f}GB "
              f"mean_growth={(sizes[-1] - sizes[0]) / max(1, len(sizes) - 1) / 1e6:+.1f}MB/save "
              + (f"largest_growth={biggest[0] / 1e6:+.0f}MB (job {biggest[1]['job']} to {biggest[2]['job']})" if biggest else ""))


def readings(path: Path, repo: str) -> list[dict]:
    """Complete readings of one repository, without the watcher's skips and partial reads."""
    out = []
    with path.open(encoding="utf-8") as lines:
        for line in lines:
            if line.startswith(f'{{"repo": "nickderobertis/{repo}"') and '"complete": true' in line[:120]:
                out.append(json.loads(line))
    return out


GB, GIB = 10e9, 10 * 2**30


def q1(store: h.Store, repo: str, path: Path) -> None:
    """Every change in the listing, the time each line was crossed, and who went."""
    rows = readings(path, repo)
    print(f"{repo}: {len(rows)} readings {rows[0]['started_at']} to {rows[-1]['started_at']}, "
          f"listed {min(r['listed_bytes'] for r in rows) / 1e9:.3f}..{max(r['listed_bytes'] for r in rows) / 1e9:.3f} GB, "
          f"usage {min(r['usage_bytes'] for r in rows) / 1e9:.3f}..{max(r['usage_bytes'] for r in rows) / 1e9:.3f} GB; "
          f"requests {sum(r['requests'] for r in rows)}")
    prev = None
    for row in rows:
        current = {e["id"]: e for e in row["entries"]}
        if prev is not None:
            previous = {e["id"]: e for e in prev["entries"]}
            gone = [previous[i] for i in previous if i not in current]
            new = [current[i] for i in current if i not in previous]
            big_new = [e for e in new if e["size_in_bytes"] > 5e6]
            if gone or big_new:
                print(f"  {row['started_at'][11:19]} listed {row['listed_bytes'] / 1e9:.3f} (was {prev['listed_bytes'] / 1e9:.3f}) "
                      f"usage {row['usage_bytes'] / 1e9:.3f} entries {len(current)}; new>5MB "
                      f"{[(e['key'][:44], round(e['size_in_bytes'] / 1e6)) for e in big_new]}; "
                      f"gone {len(gone)} = {sum(e['size_in_bytes'] for e in gone) / 1e6:.1f} MB")
                if gone:
                    by_access = sorted(previous.values(), key=lambda e: e["last_accessed_at"])
                    by_created = sorted(previous.values(), key=lambda e: e["created_at"])
                    gone_ids = {e["id"] for e in gone}
                    access_ranks = [n for n, e in enumerate(by_access) if e["id"] in gone_ids]
                    created_ranks = [n for n, e in enumerate(by_created) if e["id"] in gone_ids]
                    print(f"    of {len(previous)}: rank by last access {access_ranks[:6]}..{access_ranks[-1]}, "
                          f"rank by creation {created_ranks[:6]}..{created_ranks[-1]}; contiguous from oldest access "
                          f"{access_ranks == list(range(len(access_ranks)))}, from oldest creation "
                          f"{created_ranks == list(range(len(created_ranks)))}")
                    for e in sorted(gone, key=lambda e: -e["size_in_bytes"])[:6]:
                        print(f"      {e['key'][:60]} {e['ref']} {e['size_in_bytes'] / 1e6:.1f}MB "
                              f"created {e['created_at'][5:16]} last_access {e['last_accessed_at'][5:16]}")
        prev = row


def seconds(step: dict) -> float:
    return (h.ts(step["completed_at"]) - h.ts(step["started_at"])).total_seconds()


RUST_ACTION = re.compile(r"Swatinem/rust-cache|actions-rust-lang/setup-rust-toolchain", re.I)


def rust_restore(text: str) -> str:
    """How the job's rust-cache restore answered: none, miss, or the key it restored."""
    lines = text.splitlines()
    for number, line in enumerate(lines):
        if "Restoring cache" in line:
            for later in lines[number + 1:number + 12]:
                if "No cache found" in later:
                    return "miss"
                found = re.search(r"Restored from cache key \"?([^\" ]+)\"?(.*)", later)
                if found:
                    return found.group(1) + found.group(2)
                found = re.search(r"Cache restored from key: (\S+)", later)
                if found:
                    return found.group(1)
            return "unknown"
    return "none"


def summary(values: list[float]) -> str:
    return f"n={len(values)} p50={pct(values, .5):.0f} p90={pct(values, .9):.0f} max={max(values):.0f}"


def q8(store: h.Store, repo: str) -> None:
    if repo == "onepipeline-ui":
        return
    durations: dict[str, list[float]] = defaultdict(list)
    posts: dict[str, list[float]] = defaultdict(list)
    sizes: dict[str, list[float]] = defaultdict(list)
    restores: Counter[tuple[str, str]] = Counter()
    restored: list[tuple] = []
    attempts = Counter()
    for run in h.workflow_runs(store, repo, "release.yml"):
        attempts[run["run_attempt"]] += 1
        for job in h.jobs(store, repo, run["id"]):
            if job.get("conclusion") not in ("success", "failure"):
                continue
            name = job["name"]
            if job["conclusion"] == "success":
                durations[name].append(minutes(job["completed_at"], job["started_at"]) * 60)
                post = [seconds(st) for st in job["steps"] if st["name"].startswith("Post") and RUST_ACTION.search(st["name"]) and st.get("completed_at")]
                if post:
                    posts[name].append(sum(post))
            text = store.log(repo, job["id"]) or ""
            answer = rust_restore(text)
            kind = answer if answer in ("miss", "none", "unknown") else "restored"
            restores[(name, kind)] += 1
            if kind == "restored":
                restored.append((run["id"], run["run_attempt"], run["head_branch"], name, answer[:80]))
            biggest = [save["bytes"] for save in saves(text) if save["key"]]
            if biggest:
                sizes[name].append(max(biggest))
    print(f"{repo}: runs by attempt {dict(attempts)}")
    for name in sorted(durations):
        print(f"  {name}: duration_s {summary(durations[name])}; post-job rust save_s "
              f"{summary(posts[name]) if posts[name] else 'none'}; saved MB "
              f"{summary([b / 1e6 for b in sizes[name]]) if sizes[name] else 'none'}; restores "
              f"{ {k[1]: v for k, v in restores.items() if k[0] == name} }")
    print(f"  restored rust caches: {len(restored)}")
    for row in restored[:40]:
        print(f"    {row}")


def q3(store: h.Store, repo: str) -> None:
    """Each warm run with a Rust save: legs' save times, the run's end, the window a prune would be late by."""
    all_saves = [e for e in warm_saves(store, repo) if e["bytes"] > 50e6]
    by_run: dict[int, list[dict]] = defaultdict(list)
    for save in all_saves:
        by_run[save["run"]].append(save)
    runs = {r["id"]: r for r in h.workflow_runs(store, repo, REPOS[repo]["warm"])}
    durations = []
    for run in runs.values():
        finished = [job["completed_at"] for job in h.jobs(store, repo, run["id"]) if job.get("completed_at") and job.get("conclusion") != "skipped"]
        if run["conclusion"] == "success" and finished:
            durations.append(minutes(max(finished), run["created_at"]))
    print(f"{repo}: warm runs {len(runs)}, duration min {summary(durations)}; runs with a save >50MB {len(by_run)}")
    windows = []
    added = []
    rows = []
    for run_id, entries in sorted(by_run.items(), key=lambda item: item[1][0]["finished"]):
        ends = [job["completed_at"] for job in h.jobs(store, repo, run_id) if job.get("completed_at") and job.get("conclusion") != "skipped"]
        end = max(ends)
        first = min(e["finished"] for e in entries)
        windows.append(minutes(end, first))
        added.append(sum(e["bytes"] for e in entries) / 1e9)
        rows.append(f"    run {run_id} created {runs[run_id]['created_at'][5:16]} end {end[11:16]} "
                    + " ".join(f"{e['leg'].replace('warm (', '')[:8]}:{e['finished'][11:16]}={e['bytes'] / 1e9:.2f}G" for e in entries))
    print(f"  first big save finished to run end, min {summary(windows)}; bytes saved per run GB {summary(added)}")
    print("\n".join(rows))


def q7(store: h.Store, repo: str) -> None:
    """First push against later pushes, per job, paired within each pull request."""
    if repo != "onetaskgraph":
        return
    runs = [r for r in h.workflow_runs(store, repo, "ci.yml") if r["event"] == "pull_request" and r["conclusion"] != "cancelled"]
    prs = {pr["headRefName"]: pr for pr in h.merged_prs(store, repo) if not is_release(pr)}
    by_branch: dict[str, list[dict]] = defaultdict(list)
    for run in h.workflow_runs(store, repo, "ci.yml"):
        if run["event"] == "pull_request":
            by_branch[run["head_branch"]].append(run)
    table: dict[str, dict[str, list[float]]] = defaultdict(lambda: defaultdict(list))
    pairs: dict[str, list[float]] = defaultdict(list)
    samples: dict[str, list[tuple]] = defaultdict(list)
    for branch, branch_runs in by_branch.items():
        if branch not in prs:
            continue
        first_sha = branch_runs[0]["head_sha"]
        times: dict[str, dict[str, list[float]]] = defaultdict(lambda: defaultdict(list))
        for run in branch_runs:
            if run["conclusion"] == "cancelled" or not store.cached(f"repos/{h.OWNER}/{repo}/actions/runs/{run['id']}/jobs?per_page=100&filter=latest"):
                continue
            push = "first" if run["head_sha"] == first_sha else "later"
            for job in h.jobs(store, repo, run["id"]):
                if job.get("conclusion") == "success":
                    value = minutes(job["completed_at"], job["started_at"])
                    times[job["name"]][push].append(value)
                    samples[f"{job['name']}/{push}"].append((run["id"], job["id"], value, branch))
        for name, pushes in times.items():
            for push, values in pushes.items():
                table[name][push] += values
            if pushes["first"] and pushes["later"]:
                pairs[name].append(median(pushes["later"]) - median(pushes["first"]))
    print(f"{repo}: merged pull-request branches {len([b for b in by_branch if b in prs])}, with >1 push "
          f"{len([b for b in by_branch if b in prs and len({r['head_sha'] for r in by_branch[b]}) > 1])}; jobs in minutes")
    for name in sorted(table):
        first, later = table[name]["first"], table[name]["later"]
        if first and later:
            print(f"  {name}: first {summary(first)}; later {summary(later)}; "
                  f"paired later-first median {median(pairs[name]):+.1f} over {len(pairs[name])} PRs")
    global _q7_samples  # read by the restore sampler below
    _q7_samples = samples


def q7_restores(store: h.Store, repo: str) -> None:
    """What the jobs actually restored: a log sample of the newest first and later pushes per job."""
    if repo != "onetaskgraph":
        return
    q7(store, repo)
    for name, rows in sorted(_q7_samples.items()):
        if not re.match(r"(lint|test-linux|test-windows|test-macos)/", name):
            continue
        sampled = rows[-60:]
        answers: Counter[str] = Counter()
        builds: dict[str, list[float]] = defaultdict(list)
        for run_id, job_id, value, branch in sampled:
            answer = rust_restore(store.log(repo, job_id) or "")
            match = re.search(r"full match: (\w+)", answer)
            kind = answer if answer in ("miss", "none", "unknown") else f"restored full={match.group(1) if match else '?'}"
            answers[kind] += 1
            builds[kind].append(value)
        print(f"  {name}: sampled {len(sampled)} {dict(answers)} median minutes "
              f"{ {k: round(median(v), 1) for k, v in builds.items()} }; runs {[row[0] for row in sampled]}")


def episodes(store: h.Store, repo: str, path: Path) -> None:
    """Each time a reading stood over 10 GiB or lost an entry: every reading through it, and the timings."""
    rows = readings(path, repo)
    print(f"{repo}:")
    previous = None
    open_episode = False
    for row in rows:
        if previous is not None:
            before = {e["id"]: e for e in previous["entries"]}
            after = {e["id"]: e for e in row["entries"]}
            gone = [before[i] for i in before if i not in after]
            new = sorted((after[i] for i in after if i not in before), key=lambda e: e["created_at"])
            over = row["listed_bytes"] > GIB
            if gone or over or open_episode:
                running = previous["listed_bytes"]
                crossed = None
                for entry in new:
                    running += entry["size_in_bytes"]
                    if running > GIB and crossed is None:
                        crossed = entry
                gap = minutes(row["started_at"], previous["started_at"])
                print(f"  {row['started_at'][11:19]} (+{gap:.0f}m) listed {row['listed_bytes'] / 1e9:.4f} usage "
                      f"{row['usage_bytes'] / 1e9:.4f} entries {len(after)} total_count {row['total_count']}; new "
                      f"{sum(e['size_in_bytes'] for e in new) / 1e6:.1f}MB gone {len(gone)} = "
                      f"{sum(e['size_in_bytes'] for e in gone) / 1e6:.1f}MB"
                      + (f"; save crossing 10 GiB at {crossed['created_at'][11:19]} ({crossed['key'][:32]})" if crossed else "")
                      + (f"; newest gone last access {max(e['last_accessed_at'] for e in gone)[5:19]}, oldest survivor "
                         f"{min(e['last_accessed_at'] for e in after.values())[5:19]}" if gone else ""))
            open_episode = over
        previous = row


def kind(entry: dict) -> str:
    key = entry["key"]
    if key.startswith("nx-cache-"):
        return "nx-cache"
    if key.startswith("v0-rust-"):
        return "rust"
    return "other"


def scope(entry: dict) -> str:
    ref = entry["ref"]
    return "pr" if "/pull/" in ref else "tag" if "/tags/" in ref else "main" if ref == "refs/heads/main" else "other"


def q2(store: h.Store, repo: str, path: Path) -> None:
    """Across all evictions: is the order strictly by last access, and what goes before what."""
    rows = readings(path, repo)
    print(f"{repo}:")
    previous = None
    for row in rows:
        if previous is not None:
            before = {e["id"]: e for e in previous["entries"]}
            after = {e["id"]: e for e in row["entries"]}
            gone = [before[i] for i in before if i not in after]
            if gone:
                survivors = [before[i] for i in before if i in after]
                newest_gone = max(e["last_accessed_at"] for e in gone)
                inversions = [e for e in survivors if e["last_accessed_at"] < newest_gone]
                younger_gone = sum(
                    1 for e in gone if any(e["created_at"] > s["created_at"] for s in survivors)
                )
                rust_older = [s for s in survivors if kind(s) == "rust" and any(
                    kind(g) == "nx-cache" and s["created_at"] < g["created_at"] for g in gone)]
                print(f"  {row['started_at'][11:19]}: gone {len(gone)} {dict(Counter((kind(e), scope(e)) for e in gone))}; "
                      f"survivors accessed earlier than the newest gone: {len(inversions)}; gone though created after a "
                      f"survivor: {younger_gone}; older-created Rust surviving gone results: "
                      f"{[(s['key'][:36], s['created_at'][5:16], s['last_accessed_at'][5:16]) for s in rust_older[:4]]}")
                if inversions:
                    print(f"    inversions: {[(s['key'][:30], s['last_accessed_at'][5:19]) for s in inversions[:5]]} newest gone {newest_gone[5:19]}")
        previous = row


def trigger(store: h.Store, repo: str, path: Path) -> None:
    """Test: an eviction needs the listing already over 10 GiB and a save arriving. Counts every pair of readings."""
    rows = readings(path, repo)
    table: Counter[tuple[str, str, str]] = Counter()
    exceptions = []
    longest_over = 0.0
    over_since = None
    for previous, row in zip(rows, rows[1:]):
        before = {e["id"]: e for e in previous["entries"]}
        after = {e["id"]: e for e in row["entries"]}
        new = sorted((after[i] for i in after if i not in before), key=lambda e: e["created_at"])
        gone = [before[i] for i in before if i not in after]
        # Did any new entry find the total, with the saves before it, already over the line?
        running = previous["listed_bytes"]
        found_over = False
        for entry in new:
            if running > GIB:
                found_over = True
            running += entry["size_in_bytes"]
        cell = ("save found it over" if found_over else "no save found it over", "gone" if gone else "none gone")
        table[cell] += 1
        if (found_over and not gone) or (gone and not found_over):
            exceptions.append((row["started_at"][11:19], previous["listed_bytes"] / 1e9, running / 1e9, len(new), len(gone), round(minutes(row["started_at"], previous["started_at"]))))
        if row["listed_bytes"] > GIB:
            over_since = over_since or row["started_at"]
            longest_over = max(longest_over, minutes(row["started_at"], over_since))
        else:
            over_since = None
    print(f"{repo}: {dict(table)}; longest a run of readings stood over 10 GiB without coming under {longest_over:.0f}m")
    print(f"  exceptions (time, listed before GB, before+new GB, new, gone, gap min): {exceptions}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--readings", type=Path)
    parser.add_argument("questions", nargs="+")
    args = parser.parse_args()
    store = h.Store(args.cache)
    for question in args.questions:
        for repo in REPOS:
            if question in ("q1", "q2", "episodes", "trigger"):
                globals()[question](store, repo, args.readings)
            else:
                globals()[question](store, repo)
    print(f"[{store.requests} requests]", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
