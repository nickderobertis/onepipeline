#!/usr/bin/env bash
# Exercise the real vendored public matcher API in both selectable configurations.
set -euo pipefail
scratch="${ONEPIPELINE_NODE_SCRATCH_DIR:?set ONEPIPELINE_NODE_SCRATCH_DIR to a writable scratch directory}"
build="$scratch/privateterms-build.$$.jsonl"
cargo build --locked --offline -p privateterms --message-format=json > "$build"
lib="$(python - "$build" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    record = json.loads(line)
    if record.get('reason') == 'compiler-artifact' and record['target']['name'] == 'privateterms':
        print(next(path for path in record['filenames'] if path.endswith('.rlib')))
        break
else:
    sys.exit('no privateterms build artifact; inspect cargo output')
PY
)"
rustc --edition=2021 -O spikes/engine-startup/verify.rs --extern "privateterms=$lib" \
  -L "dependency=$(dirname "$lib")" -L "dependency=$(dirname "$lib")/deps" -o "$scratch/verify-matcher.$$"
"$scratch/verify-matcher.$$"
