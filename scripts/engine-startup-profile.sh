#!/usr/bin/env bash
# Local attribution only (manager c13.0); hosted comparisons use the existing CI trace.
# Exit codes: 0 captured journey; 2 invalid input/tool setup; other failures propagate their command status.
set -euo pipefail
if [ "$#" -ne 2 ]; then
  echo 'usage: bash scripts/engine-startup-profile.sh E2E_BINARY OUTPUT_DIR (requires samply and permission to sample)' >&2
  exit 2
fi
binary="$(realpath "$1" 2>/dev/null)" || { echo 'binary path cannot be read: rebuild with cargo test --test e2e --no-run and pass its executable path' >&2; exit 2; }
case "$(basename "$binary")" in
  e2e-*) ;;
  *) echo 'expected the compiled e2e-HASH binary: build cargo test --test e2e --no-run first' >&2; exit 2 ;;
esac
[ -x "$binary" ] || { echo 'the e2e binary is not executable: rebuild it with cargo test --test e2e --no-run' >&2; exit 2; }
out="$2"
mkdir "$out" || { echo 'capture output must be a fresh directory: choose a new OUTPUT_DIR so old traces cannot satisfy this recording' >&2; exit 2; }
out="$(realpath "$out")"
journey=publish_preserve::a_live_preserve_add_starts_from_the_already_settled_kept_branch
command -v samply >/dev/null || { echo 'install samply before recording' >&2; exit 2; }
# llmlint: ignore-block[changed_behavior_has_e2e] This never-land command's real sampler journey also proves its installed-profiler and libtest-discovery setup. Portable npm tests cannot assume an e2e binary or perf privilege before the Rust tiers finish; the report cites the actual privileged journey (manager a1).
expected="$(cat "$(dirname "$0")/../spikes/engine-startup/samply-version.txt")"
[ "$(samply --version)" = "samply $expected" ] || { echo "install samply $expected before recording (cargo install --locked --version $expected samply)" >&2; exit 2; }
"$binary" --list --format terse > "$out/journeys.txt" || { echo 'binary does not list libtest journeys: rebuild this branch’s e2e binary' >&2; exit 2; }
python - "$out/journeys.txt" "$journey" <<'PYJOURNEY'
import sys
if sys.argv[2] + ': test' not in open(sys.argv[1]).read().splitlines():
    sys.exit("the selected start/adopt journey is absent: rebuild this branch’s e2e binary")
PYJOURNEY
# llmlint: ignore-end[changed_behavior_has_e2e]
# llmlint: ignore-block[changed_behavior_has_e2e] This never-land sampler is proved by the spike’s real samply-on-e2e debug/release journeys, whose source and trace are recorded in the report. Requiring that privileged local sampler in portable npm tests would add a different CI contract; subprocess tests cover the parser and invalid-binary refusal (manager a1).
git rev-parse HEAD > "$out/source-commit.txt"
git diff HEAD > "$out/source.diff"
export ONEPIPELINE_LAUNCH_TRACE="$out/launch"
export NEXTEST_TEST_NAME="$journey"
mkdir -p "$ONEPIPELINE_LAUNCH_TRACE"
target=("$binary" --exact "$journey" --nocapture)
if [ -n "${ONEPIPELINE_PROFILE_UID:-}" ]; then
  target_uid="$ONEPIPELINE_PROFILE_UID"
  target_gid="${ONEPIPELINE_PROFILE_GID:-$target_uid}"
  [[ "$target_uid" =~ ^[0-9]+$ && "$target_gid" =~ ^[0-9]+$ && "$(id -u)" = 0 ]] \
    || { echo 'sampling UID/GID must be numeric and set only by the root sampler; unset them for ordinary sampling' >&2; exit 2; }
  chown "$target_uid:$target_gid" "$ONEPIPELINE_LAUNCH_TRACE"
  target=(setpriv --reuid="$target_uid" --regid="$target_gid" --init-groups --inh-caps=-all "${target[@]}")
fi
samply record --save-only --unstable-presymbolicate --rate 2000 \
  --output "$out/profile.json.gz" -- "${target[@]}"
python - "$out" <<'PYTRACE'
import gzip, json, pathlib, sys
root = pathlib.Path(sys.argv[1])
try:
    with gzip.open(root / 'profile.json.gz', 'rt') as stream:
        profile = json.load(stream)
    if not any(thread.get('processName') == 'onepipeline' for thread in profile['threads']):
        raise ValueError('profile contains no onepipeline process')
    verbs = set()
    for path in (root / 'launch').glob('*.jsonl'):
        for line in path.read_text().splitlines():
            record = json.loads(line)
            if record.get('kind') == 'proc' and record.get('exe') == 'onepipeline':
                verbs.add(record.get('verb'))
    if not {'start', 'adopt'} <= verbs:
        raise ValueError('capture lacks start/adopt trace records')
except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
    sys.exit(f'capture refused: {error}; rerun the selected journey into a fresh OUTPUT_DIR with launch tracing and the pinned samply enabled')

PYTRACE

# llmlint: ignore-end[changed_behavior_has_e2e]
