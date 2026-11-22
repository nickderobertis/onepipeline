#!/usr/bin/env bash
# DEMONSTRATION ONLY (stop-verdict-split evidence): run one cargo build with cargo's
# fingerprint log on, keep the log under target/demo-evidence/, and print what it compiled.
set -uo pipefail
label="$1"; shift
dir="target/demo-evidence"; mkdir -p "$dir"
log="$dir/$label-$(date -u +%H%M%S).log"
start=$(date -u +%s)
echo "demo-evidence $label: start $(date -u +%FT%TZ): $*"
CARGO_LOG=cargo::core::compiler::fingerprint=info "$@" 2> "$log"
status=$?
end=$(date -u +%s)
dirty=$(grep -c "fingerprint dirty for" "$log")
echo "demo-evidence $label: end $(date -u +%FT%TZ), $((end - start))s, exit $status, $dirty unit(s) dirty (compiled); log $log"
grep -o "dirty: [A-Za-z]*" "$log" | sort | uniq -c | sed "s/^/demo-evidence $label: /"
grep "fingerprint dirty for" "$log" | sed -E 's/.*fingerprint dirty for ([^ ]+ v[^ ]+).*/\1/' | sort | uniq -c | sort -rn | head -15 | sed "s/^/demo-evidence $label: unit /"
[ "$status" -eq 0 ] || { grep -v "INFO\|TRACE\|DEBUG" "$log" | tail -40 >&2; }
exit "$status"
