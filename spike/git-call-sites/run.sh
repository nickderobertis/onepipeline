#!/usr/bin/env bash
# SPIKE (spike-git-call-sites, never landed): attribute every git process the
# engine starts in the named e2e modules to the code that started it, and print
# the tables the report quotes.
#
#   bash spike/git-call-sites/run.sh OUT_DIR MODULE...       run the modules, then analyse
#   bash spike/git-call-sites/run.sh --analyse OUT_DIR MODULE...   analyse an earlier run
#
# OUT_DIR receives one directory per run: `traces/` (each test's GIT_TRACE2 brief
# file, GIT_TRACE2_EVENT file, and the vendored onevcs's per-spawn caller log),
# `nextest.log`, and `ci/` (CI run 3's Linux `git-tier/` traces, downloaded once,
# for the cross-check). Linux only: counts, never times. TEST_THREADS (default 4,
# the CI runner's CPU count) sets nextest's concurrency.
set -euo pipefail

analyse_only=0
if [ "${1:-}" = "--analyse" ]; then
    analyse_only=1
    shift
fi
out="${1:?OUT_DIR}"
shift
[ "$#" -gt 0 ] || { echo "name at least one e2e module" >&2; exit 2; }
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
mkdir -p "$out"
out="$(cd "$out" && pwd)"

# CI run 3 (45298442141), Linux job 138124645947: its demo-profile-linux artifact.
ci_artifact=14881828449
if [ ! -d "$out/ci/git-tier" ]; then
    mkdir -p "$out/ci"
    gh api "repos/nickderobertis/onepipeline/actions/artifacts/$ci_artifact/zip" >"$out/ci/demo-profile-linux.zip"
    (cd "$out/ci" && unzip -q demo-profile-linux.zip 'git-tier/*' && rm demo-profile-linux.zip)
fi

if [ "$analyse_only" = 0 ]; then
    filter=""
    for module in "$@"; do
        filter="${filter:+$filter | }test(/^${module}::/)"
    done
    rm -rf "$out/traces"
    mkdir -p "$out/traces"
    status=0
    (
        cd "$root"
        ONEPIPELINE_SPIKE_GIT_DIR="$out/traces" NEXTEST_NO_FAIL_FAST=1 \
            cargo nextest run --test e2e --no-fail-fast \
            --test-threads "${TEST_THREADS:-4}" -E "$filter" \
            --final-status-level fail >"$out/nextest.log" 2>&1
    ) || status=$?
    grep -E '^\s+Summary|^\s+(FAIL|TIMEOUT|SIGTERM|SIGKILL) ' "$out/nextest.log" || true
    if [ "$status" != 0 ]; then
        # A test that failed or never ran left a short trace, so every count would be short.
        echo "run.sh: cargo nextest exited $status, so these traces undercount; the failures and" \
            "build errors are in $out/nextest.log. Fix them and run again; to read these traces" \
            "anyway, run: bash $0 --analyse $out $*" >&2
        exit 1
    fi
fi

# Every test of this tree's e2e binary, so run 3's traces of the smoke binary's tests
# (two of which share a module name with e2e) are left out of the comparison.
if ! (cd "$root" && cargo nextest list --test e2e --message-format oneline \
    >"$out/e2e-list.txt" 2>"$out/e2e-list.log"); then
    cat "$out/e2e-list.log" >&2
    echo "run.sh: listing the e2e tests failed (above, also in $out/e2e-list.log);" \
        "fix the build, then run: bash $0 --analyse $out $*" >&2
    exit 1
fi
awk '{print $2}' "$out/e2e-list.txt" | sed 's/::/./' >"$out/e2e-tests.txt"

python3 -I "$here/attribute.py" "$out/traces" "$out/ci/git-tier" "$out/e2e-tests.txt" "$@"
