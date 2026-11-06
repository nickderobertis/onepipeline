#!/usr/bin/env bash
# The workflow project provisions the same pinned CLI the release tail uses.
set -euo pipefail
onevcs_version="$(bash .github/read-onevcs-cli-version.sh)"
npm ci --prefix .github --ignore-scripts
npm install --no-save --no-package-lock --no-audit --no-fund \
  --prefix .github/node_modules/.onevcs-cli \
  "onevcs-cli@$onevcs_version"
