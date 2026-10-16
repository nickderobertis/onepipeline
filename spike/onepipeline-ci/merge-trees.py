#!/usr/bin/env python3
"""Whether each merged PR's merge commit has the same tree as the PR head CI tested.

    python3 -I spike/onepipeline-ci/merge-trees.py <figures.json.gz> <scratch-bare-repo> > out/merge-tree-equality.json

Fetches main and every PR's `refs/pull/<n>/head` over git (no REST request) into
<scratch-bare-repo>, created if absent, and compares `<sha>^{tree}`. Where the
trees are equal, a push run on the merge commit re-checks exactly what the PR's
head-commit runs checked.
"""

import gzip
import json
import subprocess
import sys

figures, repo = sys.argv[1:]
d = json.load(gzip.open(figures))
prs = [p for p in d["per_pr"].values() if p["merge_commit_sha"]]
subprocess.run(["git", "init", "-q", "--bare", repo], check=True)
specs = [f"+refs/pull/{p['number']}/head:refs/pr/{p['number']}" for p in prs]
subprocess.run(["git", "-C", repo, "fetch", "-q", "https://github.com/nickderobertis/onepipeline",
                "+refs/heads/main:refs/heads/main", *specs], check=True)


def tree(commit):
    r = subprocess.run(["git", "-C", repo, "rev-parse", f"{commit}^{{tree}}"], capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else None


shape = d.get("summary_shape", {}).get("since", "9999")
rows = {}
for p in sorted(prs, key=lambda p: p["number"]):
    head, merge = tree(p["head_sha"]), tree(p["merge_commit_sha"])
    rows[str(p["number"])] = {"release_pr": p["release_pr"], "current_shape": p["merged_at"] >= shape,
                              "head_tree": head, "merge_tree": merge, "same_tree": head is not None and head == merge}
summary = {}
for kind in ("feature", "release"):
    for scope in ("window", "current_shape"):
        rs = [r for r in rows.values() if r["release_pr"] == (kind == "release") and (scope == "window" or r["current_shape"])]
        summary[f"{kind}_{scope}"] = {"prs": len(rs), "head_tree_known": sum(r["head_tree"] is not None for r in rs),
                                      "merge_tree_equals_tested_head": sum(r["same_tree"] for r in rs)}
json.dump({"summary": summary, "per_pr": rows}, sys.stdout, indent=1, sort_keys=True)
sys.stdout.write("\n")
