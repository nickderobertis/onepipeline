#!/usr/bin/env bash
# Size one Rust cache family on its one platform (spike-onepipeline-contents).
#
#   scripts/cache-contents/run.sh test-linux          bytes and owners, about 15 minutes
#   scripts/cache-contents/run.sh test-linux --price  each compile line from cold, about an hour
#   scripts/cache-contents/run.sh --read RUN_ID       print what an earlier run measured
#
# Families: test-linux, test-windows, test-macos, ci-capture. It commits the
# request .github/workflows/cache-contents.yml reads, publishes it onto this
# session's draft (the workflow's only trigger, ci.yml disabled on this branch, so
# no test job runs), waits for that commit's run, and prints the run's `cc|` lines.
#
# Exit: 0 printed, 1 the run failed or never came, 2 a refused call.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

repo=nickderobertis/onepipeline
usage() {
  echo "cache-contents: $1" >&2
  echo "ACTION: run 'scripts/cache-contents/run.sh test-linux|test-windows|test-macos|ci-capture [--price]' or '--read RUN_ID'" >&2
  exit 2
}

print_run() {
  gh run view "$1" -R "$repo" --json jobs --jq '.jobs[] | select(.name | startswith("measure")) | .databaseId' |
    while read -r job; do
      gh run view -R "$repo" --job "$job" --log | sed -n 's/^.*Z cc|/cc|/p'
    done
}

[ "$#" -ge 1 ] || usage "name a family"
if [ "$1" = "--read" ]; then
  [ "$#" -eq 2 ] || usage "--read takes a run id"
  print_run "$2"
  exit 0
fi
family="$1"
mode=size
case "$family" in
  test-linux | test-windows | test-macos | ci-capture) ;;
  *) usage "no family '$family'" ;;
esac
case "${2:-}" in
  "") ;;
  --price) mode=price ;;
  *) usage "unknown argument '$2'" ;;
esac
[ -n "${ONEVCS_SESSION:-}" ] || { echo "cache-contents: ONEVCS_SESSION is not set, so nothing can publish the request" >&2; echo "ACTION: run this inside the session that owns this branch" >&2; exit 2; }

echo "$family $mode" > .github/cache-contents-request
git add .github/cache-contents-request
git commit --quiet --allow-empty -m "test(ci): ask for $family's cache contents ($mode)"
sha="$(git rev-parse HEAD)"
onevcs publish "$ONEVCS_SESSION" --draft >&2

# The pull_request event's run names the head commit on this branch.
run=""
for _ in $(seq 1 60); do
  run="$(gh run list -R "$repo" --workflow cache-contents.yml --commit "$sha" --limit 1 --json databaseId --jq '.[0].databaseId // ""')"
  [ -n "$run" ] && break
  sleep 10
done
[ -n "$run" ] || { echo "cache-contents: no run of cache-contents.yml appeared for $sha" >&2; echo "ACTION: check the draft's Actions tab" >&2; exit 1; }
echo "cache-contents: run $run" >&2
status=0
gh run watch "$run" -R "$repo" --exit-status --interval 60 > /dev/null || status=$?
print_run "$run"
exit "$status"
