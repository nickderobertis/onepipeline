#!/usr/bin/env bash
# Strict CI installation; the optional session-start hook keeps its separate policy.
set -euo pipefail
if [ -z "${GITHUB_PATH:-}" ]; then
  echo "setup-llmlint-ci requires GITHUB_PATH; run this recipe inside a GitHub Actions job." >&2
  exit 2
fi
pins=$(node .github/read-ci-tool-versions.mjs)
llmlint=
oneharness=
while IFS='=' read -r key value; do
  case "$key" in llmlint) llmlint=$value ;; oneharness) oneharness=$value ;; esac
done <<< "$pins"
uv tool install --upgrade "llmlint-cli==$llmlint" --with "oneharness-cli==$oneharness"
bin=$(uv tool dir --bin)
printf '%s\n' "$bin" >> "$GITHUB_PATH"
