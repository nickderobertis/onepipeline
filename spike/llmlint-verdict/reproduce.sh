#!/usr/bin/env bash
# Run `llmlint --diff --diff-base <base>` against the stand-in oneharness beside
# this script once per answer it can give, and print each run's exit status and
# summary line. No model is called: the stand-in is the only harness.
#
#   spike/llmlint-verdict/reproduce.sh [LLMLINT_BIN] [MODE...]
#
# LLMLINT_BIN defaults to `llmlint` on PATH. With no MODE, every mode runs.
# A scratch repository (two rules, a diff touching one Python file) is built
# under ${ONEPIPELINE_NODE_SCRATCH_DIR:-$TMPDIR}/llmlint-verdict.
set -euo pipefail
here="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
bin="${1:-llmlint}"
shift || true
modes=("$@")
((${#modes[@]})) || modes=(pass fail structured_empty_obj drop_first_rule wrong_rule_name relevant_false
  pass_exit1 only_first_call_answers empty_exit0 empty_exit1 garbage_exit0 nonzero_exit json_no_results
  results_empty structured_null structured_missing structured_string structured_list holds_string
  schema_invalid verdict_no_fields verdict_name_only)

root="${ONEPIPELINE_NODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/llmlint-verdict"
repo="$root/repo"
rm -rf "$root"
mkdir -p "$repo"
cd "$repo"
git init -q
cat >llmlint.yml <<'EOF'
rules:
  - name: no_todo
    description: "True when the changed code contains no TODO comments; false otherwise."
  - name: no_print
    description: "True when the changed code contains no print statements; false otherwise."
files:
  include: ["**/*.py"]
EOF
# `judges: 3` on one rule, for the mode where only the first judge call answers.
cat >judges.yml <<'EOF'
rules:
  - name: no_todo
    judges: 3
    description: "True when the changed code contains no TODO comments; false otherwise."
files:
  include: ["**/*.py"]
EOF
printf 'def a():\n    return 1\n' >a.py
printf 'def b():\n    return 2\n' >b.py
git add -A && git -c user.name=s -c user.email=s@localhost commit -qm base
base="$(git rev-parse HEAD)"
printf 'def a():\n    # TODO later\n    print("x")\n    return 1\n' >a.py
git add -A && git -c user.name=s -c user.email=s@localhost commit -qm change

echo "llmlint: $("$bin" --version)"
for mode in "${modes[@]}"; do
  config=llmlint.yml extra=()
  if [ "$mode" = only_first_call_answers ]; then
    config=judges.yml extra=(--max-parallel 1)
    rm -f "$root/counter"
  fi
  status=0
  STANDIN_MODE="$mode" STANDIN_COUNTER="$root/counter" STANDIN_LOG="$root/calls.log" \
    "$bin" --diff --diff-base "$base" --oneharness-bin "$here/oneharness" --no-history \
    --progress never -c "$config" "${extra[@]}" >"$root/out" 2>"$root/err" || status=$?
  # -c is required: llmlint's upward config search would otherwise find a config
  # in a directory above the scratch root.
  printf '%-26s exit=%s  %s\n' "$mode" "$status" "$(grep -m1 -E '^[0-9]+ rules: ' "$root/out" || echo '(no summary line)')"
done
