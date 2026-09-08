#!/usr/bin/env bash
# Publish npm package directories/tarballs idempotently, and do not return until
# the registry actually serves what was published.
#
# A release job can be re-run — after a flaky sibling job, or to finish a partial
# publish — and an npm version is immutable, so a second `npm publish` of a
# version already live would red-fail a release that actually succeeded. This
# asks the registry first: `npm pack --dry-run --json` validates each manifest and
# yields its canonical name@version, and only a registry 404 permits publication.
# Auth, network, and server errors fail closed rather than being mistaken for
# "not published yet".
#
# **`npm publish` exiting 0 does not mean the registry serves the version.** It
# means the upload was accepted; the registry completes the version write
# afterwards, and until it does, an install cannot resolve that version. That is
# not a theory — it is what made every macOS npm verification red. In v0.23.0 the
# `publish-npm` job published all five platform packages and then the launcher,
# in exactly that order, and the job log says so:
#
#   17:31:52 published onepipeline-cli-darwin-arm64@0.23.0   registry: 17:33:07
#   17:31:55 published onepipeline-cli-darwin-x64@0.23.0     registry: 17:33:10
#   17:31:58 published onepipeline-cli-linux-arm64@0.23.0    registry: 17:33:13
#   17:32:02 published onepipeline-cli-linux-x64@0.23.0      registry: 17:32:01
#   17:32:06 published onepipeline-cli-win32-x64@0.23.0      registry: 17:32:06
#   17:32:09 published onepipeline-cli@0.23.0                registry: 17:32:08
#
# — the job ended at 17:32:11, and three of the five became live seventy-five
# seconds after that. So the loop's ordering was never the problem; the problem
# is that the loop's steps did not mean what they said. `verify-npm` installed
# the launcher at 17:32:27, npm silently skipped the two optional dependencies
# the registry did not yet serve, and the launcher could not start.
#
# Two rules close it, and neither is a fixed wait:
#
#   * nothing is published before every exact version its own manifest pins is
#     served — which is the launcher's whole relationship to its platform
#     packages; and
#   * this does not return until the registry serves what it just published,
#     so a caller sequencing publishes gets the ordering it wrote.
#
# The wait is bounded and it is a *refusal*, not a shrug: a launcher offered
# against a platform package the registry cannot serve is the outage itself.
set -euo pipefail

fail() {
  printf 'publish-npm: %s\n' "$1" >&2
  exit 1
}

# How long to keep asking the registry, and how often. Ten minutes is far past
# the worst lag observed (v0.22.3's linux-arm64 took eight) and still inside a
# release job's patience. Both are settable so the journeys in
# npm/test/publish-gate.test.mjs can drive the exhausted budget in seconds
# rather than in minutes; nothing in CI sets either.
await_budget="${PUBLISH_NPM_AWAIT_BUDGET:-600}"
await_interval="${PUBLISH_NPM_AWAIT_INTERVAL:-3}"

[ "$#" -gt 0 ] || fail "pass at least one package directory or tarball"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# What the registry says about one `name@version` right now: 0 it serves it,
# 1 it does not, 2 it did not answer. `--prefer-online` because npm caches a
# packument it has already read, and a poll that re-read its own memory would
# spend the whole budget learning nothing.
registry_state() {
  if npm view "$1" version --prefer-online >/dev/null 2>"$work/view-error"; then
    return 0
  fi
  if grep -Eq 'E404|404 Not Found' "$work/view-error"; then
    return 1
  fi
  return 2
}

# Block until the registry serves `name@version`, or refuse. `$2` is what the
# caller was about to do, so an exhausted budget says which promise broke.
await_served() {
  local identity="$1" next="$2" waited=0 state
  while true; do
    state=0
    registry_state "$identity" || state=$?
    case "$state" in
      0)
        if [ "$waited" -gt 0 ]; then
          printf 'publish-npm: the registry took %ss to serve %s\n' "$waited" "$identity" >&2
        fi
        return 0
        ;;
      2)
        cat "$work/view-error" >&2
        fail "cannot query '$identity'; re-run the release when the npm registry is reachable"
        ;;
    esac
    if [ "$waited" -ge "$await_budget" ]; then
      fail "the registry still does not serve '$identity' after ${waited}s, so $next would install against a version nothing can resolve; re-run the release once https://www.npmjs.com/package/${identity%@*} shows it"
    fi
    sleep "$await_interval"
    waited=$((waited + await_interval))
  done
}

# The manifest inside a package, whichever of the two shapes this was handed.
manifest_of() {
  case "$1" in
    *.tgz | *.tar.gz) tar -xzOf "$1" package/package.json ;;
    *) cat "$1/package.json" ;;
  esac
}

# One `name@version` line per optionalDependency pinned to an exact version.
# A range is not a pin and names no single version to wait for, so it is left to
# npm to resolve; everything scripts/npm-build.mjs stamps is exact.
#
# The single-quoted program is JavaScript; its template expression is not shell.
# shellcheck disable=SC2016
pinned_optional_deps() {
  manifest_of "$1" | node -e '
    let input = "";
    process.stdin.on("data", chunk => input += chunk).on("end", () => {
      const manifest = JSON.parse(input);
      for (const [name, pin] of Object.entries(manifest.optionalDependencies || {})) {
        if (/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(pin)) {
          process.stdout.write(`${name}@${pin}\n`);
        }
      }
    });
  '
}

published=""
skipped=""

for package in "$@"; do
  if ! metadata="$(npm pack --dry-run --json "$package" 2>"$work/pack-error")"; then
    cat "$work/pack-error" >&2
    fail "cannot read package metadata from '$package'; rebuild the npm artifact with scripts/npm-build.mjs"
  fi
  # The single-quoted program is JavaScript; its template expression is not shell.
  # shellcheck disable=SC2016
  if ! identity="$(printf '%s' "$metadata" | node -e '
    let input = "";
    process.stdin.on("data", chunk => input += chunk).on("end", () => {
      const items = JSON.parse(input);
      if (!Array.isArray(items) || items.length !== 1 ||
          typeof items[0]?.name !== "string" || typeof items[0]?.version !== "string") {
        throw new Error("npm pack did not return one name/version");
      }
      process.stdout.write(`${items[0].name}@${items[0].version}`);
    });
  ' 2>"$work/metadata-error")"; then
    cat "$work/metadata-error" >&2
    fail "npm returned invalid metadata for '$package'; rebuild the npm artifact with scripts/npm-build.mjs"
  fi

  # Nothing reaches the registry before the exact versions its own manifest pins
  # do. For the launcher those pins are the five platform packages, so this is
  # the rule that stops a user from installing a launcher whose binary npm
  # silently declined to fetch.
  if ! pinned="$(pinned_optional_deps "$package" 2>"$work/manifest-error")"; then
    cat "$work/manifest-error" >&2
    fail "cannot read the manifest inside '$package'; rebuild the npm artifact with scripts/npm-build.mjs"
  fi
  while read -r pin; do
    [ -n "$pin" ] || continue
    await_served "$pin" "offering $identity, which pins it"
  done <<<"$pinned"

  if npm view "$identity" version --prefer-online >/dev/null 2>"$work/view-error"; then
    skipped="$skipped $identity"
  elif grep -Eq 'E404|404 Not Found' "$work/view-error"; then
    if ! npm publish "$package" --access public >"$work/publish-output" 2>&1; then
      cat "$work/publish-output" >&2
      fail "npm could not publish '$identity'; fix the reported authentication or package error, then re-run the release"
    fi
    # The upload was accepted. Whether the registry *serves* it is the separate
    # question this whole file exists to stop assuming.
    await_served "$identity" "whatever this release publishes next"
    published="$published $identity"
  else
    cat "$work/view-error" >&2
    fail "cannot query '$identity'; re-run the release when the npm registry is reachable"
  fi
done

# One line for the whole run, whatever it was handed: what a release log needs is
# which versions this push added and which were already live, not a running
# commentary. `# none` keeps the line readable when a re-run publishes nothing.
printf 'publish-npm: published%s; already on npm%s\n' \
  "${published:- none}" "${skipped:- none}"
