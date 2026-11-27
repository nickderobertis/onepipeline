#!/usr/bin/env bash
# Build the helpers the tier already needs before nextest starts its journey deadlines.
set -euo pipefail
if [ "$#" -eq 0 ]; then
  echo 'e2e-prepare: missing test command. ACTION: pass the coverage-unit command and its arguments.' >&2
  exit 2
fi
if [ "$(uname -s)" != Linux ]; then
  exec "$@"
fi
started=$SECONDS
if metadata="$(cargo metadata --offline --locked --no-deps --format-version 1)"; then
  :
else
  status=$?
  echo 'e2e-prepare: cannot inspect workspace dependencies. ACTION: resolve the Cargo metadata failure above before running tests.' >&2
  exit "$status"
fi
if helpers="$(printf '%s' "$metadata" | node -e '
  const data = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
  if (!Array.isArray(data.packages) || data.packages.some(p => typeof p.name !== "string" || !Array.isArray(p.targets) || p.targets.some(t => !Array.isArray(t.kind) || t.kind.some(k => typeof k !== "string")))) {
    throw new Error("Cargo metadata must list named packages and their target kinds");
  }
  const dump = JSON.parse(require("node:child_process").execFileSync("just", ["--dump", "--dump-format", "json"], { encoding: "utf8" }));
  const recipe = dump.recipes?.["_e2e-doubles-build"];
  if (!Array.isArray(recipe?.body)) throw new Error("the helper build recipe is missing");
  const text = recipe.body.flat().filter(part => typeof part === "string").join("\n");
  const selections = [...text.matchAll(/--package\s+([A-Za-z0-9_-]+)/g)].map(match => match[1]);
  if (!selections.length || selections.length !== (text.match(/--package\b/g) ?? []).length) {
    throw new Error("the helper recipe must declare literal Cargo package selections");
  }
  const selected = new Set(selections);
  const needed = data.packages.some(p => selected.has(p.name) && p.targets.some(t => t.kind.includes("bin")));
  process.stdout.write(needed ? "needed" : "absent");
')"; then
  :
else
  status=$?
  echo 'e2e-prepare: cannot read helper requirements. ACTION: restore valid Cargo metadata and the canonical helper recipe, with Just on PATH, before running tests.' >&2
  exit "$status"
fi
if [ "$helpers" = absent ]; then
  exec "$@"
fi
if just _e2e-doubles-build instrumented; then
  echo "e2e-prepare: helpers ready in $((SECONDS - started)) seconds" >&2
else
  status=$?
  echo "e2e-prepare: helper build failed after $((SECONDS - started)) seconds. ACTION: resolve the Cargo build failure above before running the tier." >&2
  exit "$status"
fi
exec "$@"
