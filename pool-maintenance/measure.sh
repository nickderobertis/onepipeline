#!/usr/bin/env bash
# The measurement behind `onepipeline-pool-maintenance-seconds` (budgets.yaml beside this
# file): one steady-state pass of the declared `just pool-maintenance`, after one ordinary
# run's worth of new superseded builds and never over accumulated history.
#
#   (a) the declared maintenance, untimed, clears any backlog, and the gate's build
#       (`_crate-build`) then puts the ordinary configuration in place, so the next build
#       creates only what its own configuration alone needs (repeated until a pass removes
#       nothing: see below);
#   (b) that build under package overrides that give this workspace's own crates new
#       metadata hashes, exactly the entries it created then backdated past the 2-day
#       window, standing in for a run's builds that have since been superseded;
#   (c) the ordinary gate build again, so each unit's newest build is the ordinary one;
#   (d) the declared maintenance, timed: the pruner reports its own wall-clock seconds and
#       breakdown through the onebudgetspec SDK, and that is the figure.
#
# The backlog pass's seconds are appended to (d)'s detail as separate evidence. The listings
# in target/pool-maintenance-budget/ name each entry with its metadata hash: what the
# ordinary build holds, what (b) created, and what (d) removed. This refuses unless (d)
# removed something, every entry it removed is one (b) created, and every ordinary entry is
# still present, since a figure from any other workload is not this budget's. GNU `find`
# and `date`: the budget runs on the Linux release-PR sweep and on Linux pool hosts.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

result="${ONEBUDGETSPEC_RESULT:-}"
unset ONEBUDGETSPEC_RESULT
evidence="target/pool-maintenance-budget"
profile="target/debug"
alternative=(
  --config 'profile.dev.package.onepipeline.debug-assertions=false'
  --config 'profile.dev.package.onepipeline-testfakes.debug-assertions=false'
)

# Every build entry of the profile the gate builds into, one per line.
entries() {
  local dir
  for dir in deps .fingerprint build incremental; do
    if [ -d "$profile/$dir" ]; then find "$profile/$dir" -mindepth 1 -maxdepth 1 -print; fi
  done | LC_ALL=C sort
}

mkdir -p "$evidence"
rm -f "$evidence"/*

# A unit the ordinary build compiles twice (a dependency built for build scripts and for the
# crates alike) keeps one name for both, so a pass takes the older of the two once it is past
# the window, and the build puts it back fresh. Repeating pass and build until a pass removes
# nothing leaves only builds the ordinary configuration made within the window.
backlog=""
for attempt in 1 2 3 4 5; do
  echo "pool-maintenance budget: (a) untimed backlog pass $attempt" >&2
  ONEBUDGETSPEC_RESULT="$root/$evidence/backlog.json" just pool-maintenance >&2
  pass="$(node -e '
    const reported = JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8"));
    process.stdout.write(`${reported.value} ${/builds removed: (\d+)/.exec(reported.detail)[1]}\n`);
  ' "$evidence/backlog.json")"
  read -r seconds removed <<<"$pass"
  backlog="${backlog:+$backlog, }$seconds"
  echo "pool-maintenance budget: (a) ordinary build" >&2
  just _crate-build
  if [ "$removed" -eq 0 ]; then break; fi
done
if [ "$removed" -ne 0 ]; then
  echo "pool-maintenance budget: the backlog still held superseded builds after $attempt passes, so no pass here is a steady-state one" >&2
  exit 1
fi
entries >"$evidence/ordinary.txt"

# Older than every entry the ordinary build holds, so none of its builds is the older one.
oldest="$(find "$profile" -mindepth 2 -printf '%T@\n' | awk 'NR == 1 || $1 < min { min = $1 } END { print int(min) }')"
stamp="$(date -d '-2 days -1 hour' +%s)"
if [ "$oldest" -le "$stamp" ]; then stamp="$((oldest - 3600))"; fi

echo "pool-maintenance budget: (b) alternative build" >&2
RUSTFLAGS="-D warnings" cargo build --locked --all-targets --quiet "${alternative[@]}"
entries >"$evidence/after-alternative.txt"
LC_ALL=C comm -13 "$evidence/ordinary.txt" "$evidence/after-alternative.txt" >"$evidence/alternative.txt"
while IFS= read -r entry; do
  find "$entry" -exec touch -h -d "@$stamp" {} +
done <"$evidence/alternative.txt"

echo "pool-maintenance budget: (c) ordinary build" >&2
just _crate-build
entries >"$evidence/before-pass.txt"

echo "pool-maintenance budget: (d) timed pass" >&2
if [ -n "$result" ]; then export ONEBUDGETSPEC_RESULT="$result"; fi
just pool-maintenance >&2
unset ONEBUDGETSPEC_RESULT
entries >"$evidence/after-pass.txt"
LC_ALL=C comm -23 "$evidence/before-pass.txt" "$evidence/after-pass.txt" >"$evidence/removed.txt"
LC_ALL=C comm -23 "$evidence/removed.txt" "$evidence/alternative.txt" >"$evidence/removed-not-alternative.txt"
LC_ALL=C comm -23 "$evidence/ordinary.txt" "$evidence/after-pass.txt" >"$evidence/ordinary-missing.txt"
count() { wc -l <"$evidence/$1.txt" | tr -d ' '; }
hashes() { grep -o -E -- '-[0-9a-f]{16}' "$evidence/$1.txt" | LC_ALL=C sort -u | wc -l | tr -d ' '; }

echo "pool-maintenance budget: ordinary build $(count ordinary) entries ($(hashes ordinary) metadata hashes), $(count ordinary-missing) missing after the timed pass; alternative build $(count alternative) entries ($(hashes alternative) hashes); timed pass removed $(count removed) entries ($(hashes removed) hashes), $(count removed-not-alternative) not the alternative build's (listings in $evidence)" >&2
if [ -s "$evidence/removed-not-alternative.txt" ] || [ -s "$evidence/ordinary-missing.txt" ]; then
  echo "pool-maintenance budget: the timed pass removed entries the alternative build did not create, so this is not the budget's workload:" >&2
  cat "$evidence/removed-not-alternative.txt" "$evidence/ordinary-missing.txt" >&2
  exit 1
fi
if [ ! -s "$evidence/removed.txt" ]; then
  echo "pool-maintenance budget: the timed pass removed nothing, so no superseded build was staged for it to time" >&2
  exit 1
fi

if [ -n "$result" ]; then
  node -e '
    const fs = require("node:fs");
    const [path, backlog] = process.argv.slice(1);
    const reported = JSON.parse(fs.readFileSync(path, "utf8"));
    reported.detail = `${reported.detail}; untimed backlog pass seconds, not counted: ${backlog}`;
    fs.writeFileSync(path, JSON.stringify(reported));
  ' "$result" "$backlog"
  cp "$result" "$evidence/timed-pass.json"
fi
