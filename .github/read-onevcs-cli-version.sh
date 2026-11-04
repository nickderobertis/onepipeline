#!/usr/bin/env bash
set -euo pipefail
file=.github/onevcs-cli-version
if ! version=$(cat "$file"); then
  echo "cannot read onevcs pin: $file" >&2
  echo "ACTION: restore $file from git and grant read access, then rerun bootstrap." >&2
  exit 2
fi
version=${version%$'\r'}
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "onevcs pin must be a nonempty exact stable version: $file" >&2
  echo "ACTION: write a published exact stable version (for example 1.2.3) to $file, then rerun bootstrap." >&2
  exit 2
fi
printf '%s\n' "$version"
