#!/usr/bin/env python3
"""Which Nx inputs each merged feature PR's change touched: an upper bound on what a restored Nx cache could skip.

    python3 -I spike/onepipeline-ci/nx-input-hits.py <figures.json.gz> <bare-repo> > out/nx-input-hits.json

<bare-repo> is the one merge-trees.py fetched. A PR whose change touches a named
input misses every cached target hashing it, so a restored cache could only
replay targets whose inputs it left alone. Globs are nx.json's named inputs at
this checkout, expanded through nested named inputs.
"""

import fnmatch
import gzip
import json
import subprocess
import sys

figures, repo = sys.argv[1:]
d = json.load(gzip.open(figures))
nx = json.load(open("nx.json"))["namedInputs"]


def globs(name):
    out = []
    for g in nx[name]:
        if isinstance(g, str) and g in nx:
            out += globs(g)
        elif isinstance(g, str):
            out.append(g.replace("{workspaceRoot}/", ""))
    return out


sets = {k: globs(k) for k in ["rustBuild", "crateTestSource", "e2eSource", "contractSource", "noteJourneySource", "visualDocsSource"]}
sets["npm project (npm/** + sharedGlobals)"] = ["npm/**/*"] + globs("sharedGlobals")


def hit(files, pats):
    for f in files:
        for p in pats:
            if p.endswith("/**/*") and f.startswith(p[:-5] + "/"):
                return True
            if fnmatch.fnmatch(f, p):
                return True
    return False


shape = d["summary_shape"]["since"]
rows = {}
for p in d["per_pr"].values():
    if p["release_pr"] or not p["merge_commit_sha"]:
        continue
    files = subprocess.run(["git", "-C", repo, "diff", "--name-only", f"{p['merge_commit_sha']}^", p["merge_commit_sha"]],
                           capture_output=True, text=True, check=True).stdout.split()
    rows[str(p["number"])] = {"current_shape": p["merged_at"] >= shape, "files": len(files),
                              "touches": {k: hit(files, v) for k, v in sets.items()}}
summary = {}
for scope in ("window", "current_shape"):
    rs = [r for r in rows.values() if scope == "window" or r["current_shape"]]
    summary[scope] = {"feature_prs": len(rs), "touching": {k: sum(r["touches"][k] for r in rs) for k in sets}}
json.dump({"summary": summary, "per_pr": rows}, sys.stdout, indent=1, sort_keys=True)
sys.stdout.write("\n")
