#!/usr/bin/env bash
# Publish npm package directories/tarballs idempotently, and do not return until
# the registry serves what was published.
#
# Why the registry is asked rather than the exit code is in README.md's "How the
# npm launcher is published"; exhausting the bounded wait there is a refusal
# rather than a shrug.
#
# Idempotent because an npm version is immutable and a release job can be re-run:
# only a registry 404 permits publication, and auth, network and server errors
# fail closed rather than being read as "not published yet".
#
# Exits 0 having published or skipped every package it was handed; 2 on a caller
# error, refused before anything is published; 1 when the registry did.
set -euo pipefail

# The registry, the publish, or the propagation went wrong: something outside
# this invocation has to change before a re-run behaves differently.
fail() {
  printf 'publish-npm: %s\n' "$1" >&2
  exit 1
}

# The caller asked for something this cannot do, and nothing has been published.
# Its own exit code, so a release job's log separates "fix the call" from "ask
# the registry again" — the same split scripts/retry-install.sh makes.
refuse() {
  printf 'publish-npm: %s\n' "$1" >&2
  printf 'ACTION: %s\n' "$2" >&2
  exit 2
}

# How long to keep asking the registry, and how often. Ten minutes is far past
# the worst lag observed (v0.22.3's linux-arm64 took eight) and still inside a
# release job's patience. Both are settable so the journeys in
# npm/test/publish-gate.test.mjs can drive the exhausted budget in seconds
# rather than in minutes; nothing in CI sets either.
await_budget="${PUBLISH_NPM_AWAIT_BUDGET:-600}"
await_interval="${PUBLISH_NPM_AWAIT_INTERVAL:-3}"

# Both reach `sleep` and the arithmetic below, so they are checked here rather
# than where a typo would become an infinite loop or a wait that never happens.
case "$await_budget" in
  "" | *[!0-9]*)
    refuse "PUBLISH_NPM_AWAIT_BUDGET is '$await_budget', which is not a whole number of seconds" \
      "unset it to take the default, or set it to a whole number of seconds" ;;
esac
case "$await_interval" in
  "" | *[!0-9]*)
    refuse "PUBLISH_NPM_AWAIT_INTERVAL is '$await_interval', which is not a whole number of seconds" \
      "unset it to take the default, or set it to a whole number of seconds" ;;
esac
# Base 10 explicitly: the digits check above accepts `08`, and bash arithmetic
# reads a leading zero as octal and refuses it — half-way through a wait, with a
# message about base conversion rather than about the value somebody set.
#
# Those zeros come off here too, so the conversion can be compared against what
# it was given. An all-zero string trims to nothing, which is 0.
without_leading_zeros() {
  local trimmed="${1#"${1%%[!0]*}"}"
  printf '%s' "${trimmed:-0}"
}

budget_digits="$await_budget"
interval_digits="$await_interval"
await_budget=$((10#$budget_digits))
await_interval=$((10#$interval_digits))

# All digits is not yet a number of seconds: bash counts in `intmax_t` and
# **wraps silently** rather than failing, so 2^63 comes back negative and 2^64
# comes back 0. The negative one is why this is checked at all — `waited -ge
# budget` below is true on the first miss, so an operator who asked for an
# enormous wait would get none, and publish the unresolvable launcher this
# script exists to hold back. Refused rather than clamped: a wait nobody asked
# for is not a repair.
[ "$await_budget" = "$(without_leading_zeros "$budget_digits")" ] || refuse \
  "PUBLISH_NPM_AWAIT_BUDGET is '$budget_digits', which is more seconds than this shell can hold" \
  "unset it to take the default, or set it to a whole number of seconds below $((1 << 62))"
[ "$await_interval" = "$(without_leading_zeros "$interval_digits")" ] || refuse \
  "PUBLISH_NPM_AWAIT_INTERVAL is '$interval_digits', which is more seconds than this shell can hold" \
  "unset it to take the default, or set it to a whole number of seconds below $((1 << 62))"

[ "$await_interval" -gt 0 ] || refuse \
  "PUBLISH_NPM_AWAIT_INTERVAL is 0, so a wait would spin without ever pausing" \
  "unset it to take the default, or set it to at least 1 second"

[ "$#" -gt 0 ] || refuse "pass at least one package directory or tarball" \
  "run 'publish-npm.sh <package-dir-or-tarball>...'"

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
        # llmlint: ignore-block[tool_output_is_signal] this line survives a later
        # success on purpose, and it is printed only when the registry actually
        # lagged — a healthy release prints none. It is the one warning anybody
        # gets while a publish is stalled, before the run that exhausts the
        # budget, and it names which package stalled. scripts/retry-install.sh
        # carries the same directive for the same reason.
        if [ "$waited" -gt 0 ]; then
          printf 'publish-npm: the registry took %ss to serve %s\n' "$waited" "$identity" >&2
        fi
        # llmlint: ignore-end[tool_output_is_signal]
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
      // A manifest is external input, and `Object.entries` walks a string or an
      // array as happily as a map — a package whose optionalDependencies is
      // either would have its pins read as nonsense and waited for, or not read
      // at all. Neither is a package this will publish.
      const pins = manifest.optionalDependencies;
      if (pins !== undefined && (typeof pins !== "object" || pins === null || Array.isArray(pins))) {
        throw new Error("optionalDependencies is not an object");
      }
      for (const [name, pin] of Object.entries(pins || {})) {
        // A value that is not a string is refused rather than tested: `RegExp.test`
        // coerces, so `["1.2.3"]` would read as a pin nobody wrote and `null` would
        // read as no pin at all — and "no pin" here means publishing this package
        // without waiting for that dependency, which is the whole failure.
        if (typeof pin !== "string") {
          throw new Error(`optionalDependencies["${name}"] is not a string`);
        }
        if (/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(pin)) {
          process.stdout.write(`${name}@${pin}\n`);
        }
      }
    });
  '
}

# Every argument is read and checked before any of them is published, so that
# `refuse`'s promise — nothing reached the registry — still holds when it is the
# last argument that is unreadable.
identities=()
pin_lists=()

for package in "$@"; do
  if ! metadata="$(npm pack --dry-run --json "$package" 2>"$work/pack-error")"; then
    cat "$work/pack-error" >&2
    refuse "cannot read package metadata from '$package'" \
      "rebuild the npm artifact with scripts/npm-build.mjs, then re-run"
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
    refuse "npm returned invalid metadata for '$package'" \
      "rebuild the npm artifact with scripts/npm-build.mjs, then re-run"
  fi

  if ! pinned="$(pinned_optional_deps "$package" 2>"$work/manifest-error")"; then
    cat "$work/manifest-error" >&2
    refuse "cannot read the manifest inside '$package'" \
      "rebuild the npm artifact with scripts/npm-build.mjs, then re-run"
  fi

  identities+=("$identity")
  pin_lists+=("$pinned")
done

published=""
skipped=""
at=0

for package in "$@"; do
  identity="${identities[$at]}"
  pinned="${pin_lists[$at]}"
  at=$((at + 1))

  # Nothing reaches the registry before the exact versions its own manifest pins
  # do. For the launcher those pins are the five platform packages, so this is
  # the rule that stops a user from installing a launcher whose binary npm
  # silently declined to fetch.
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
