#!/usr/bin/env bash
# Reproduce the report's figures for one run and one PR offline, from the raw
# responses committed under fixture/raw, and compare them with the full run's.
#
#   spike/onepipeline-ci/check-fixture.sh
#
# Makes no GitHub request (--offline) and reads no git checkout (the tag answers
# are under fixture/raw/git). Exits non-zero naming any field that differs.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cp -r "$here/fixture/raw" "$tmp/raw"
python3 -I "$here/actions_spike.py" --repo nickderobertis/onepipeline --since 2026-09-15 \
  --raw "$tmp/raw" --log "$tmp/requests.jsonl" --out "$tmp/fixture.json" --offline \
  --pr 942 --run 38782078021 >/dev/null
python3 -I - "$tmp/fixture.json" "$here/out/figures.json.gz" <<'EOF'
import gzip, json, sys
fixture = json.load(open(sys.argv[1]))
full = json.load(gzip.open(sys.argv[2]))
# `merge_push_runs` is the repeat figure read from the workflow run lists, which
# the fixture does not carry; every other field of the PR, both cycle-time
# phases included, must match.
strip = lambda pr: {k: v for k, v in pr.items() if k != "merge_push_runs"}
checks = {
    "PR #942 (both cycle-time phases and its head runs)": (strip(fixture["per_pr"]["942"]), strip(full["per_pr"]["942"])),
    "run 38782078021 (ci.yml, PR #942's head commit: every job)": (fixture["per_run"]["38782078021"], full["per_run"]["38782078021"]),
    "release v0.73.0 (PR #942's CD runs)": (
        {k: v for k, v in fixture["per_release"]["v0.73.0"].items() if k not in ("prs_carried", "release_cycle_s")},
        {k: v for k, v in full["per_release"]["v0.73.0"].items() if k not in ("prs_carried", "release_cycle_s")}),
}
bad = [name for name, (a, b) in checks.items() if a != b]
pr = fixture["per_pr"]["942"]
print(f"PR #942: checks phase {pr['checks_phase_s']:.0f}s, merge-to-release {pr['merge_to_release_s']:.0f}s ({pr['release']})")
run = fixture["per_run"]["38782078021"]
print(f"run 38782078021: {run['jobs_executed']} jobs, longest {run['longest_job']['name']} {run['longest_job']['duration_s']:.0f}s")
print("requests made:", fixture["requests"]["this_invocation"])
for name in checks:
    print(("DIFFERS: " if name in bad else "matches: ") + name)
sys.exit(1 if bad or fixture["requests"]["this_invocation"] else 0)
EOF
