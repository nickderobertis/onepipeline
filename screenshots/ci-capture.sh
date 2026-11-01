#!/usr/bin/env bash
# ci.yml's `lint` job's visual-docs capture: visual-docs.yml's own
# `capture-command`, read out of that workflow, run in the `container` image it
# names, against this checkout, writing to $SHOTS_OUT. One capture command, then,
# whichever of the two paths runs it: the reusable workflow hands that text to
# the image as a job container; this hands it to the same image with `docker run`
# under the same `sh -e`, because a step of a job on the runner cannot itself
# run in a container.
#
# It captures HEAD, from a clone of it at `$RUNNER_TEMP/ci-capture` (in CI) or
# `target/ci-capture` (anywhere else), with `shots/`
# and its own cargo target (`target/ci-capture-target`) mounted from this
# checkout: the capture builds its release
# binaries into its tree's own `target/release`, and `lint` runs it beside its
# own builds, which must not meet that build's lock or its files. The clone is
# kept for the second capture of the same commit, which then builds nothing.
#
# The image runs as root, as the reusable workflow's container does, because it
# installs the pinned toolchain into the image's own rustup; what it writes is
# handed back to the runner's user whatever the capture did.
#
# Exit: the capture's own, or 1 when the workflow cannot be read.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

[ -n "${SHOTS_OUT:-}" ] || {
  echo "ci-capture: SHOTS_OUT is not set; it names the directory the capture writes, e.g. shots/current/x86_64" >&2
  echo "ACTION: run it as 'SHOTS_OUT=shots/current/x86_64 bash screenshots/ci-capture.sh'" >&2
  exit 2
}
# The container writes there, through the one directory mounted back: a path
# that leaves `shots/` would be written where the host never looks, or above it.
case "/$SHOTS_OUT/" in
  */../* | */./* | *//*) bad=1 ;;
  /shots/?*) bad="" ;;
  *) bad=1 ;;
esac
[ -z "$bad" ] && [[ $SHOTS_OUT =~ ^[A-Za-z0-9._/-]+$ ]] || {
  echo "ci-capture: SHOTS_OUT='$SHOTS_OUT' is not a directory under shots/" >&2
  echo "ACTION: name a relative directory under shots/, e.g. shots/current/x86_64" >&2
  exit 2
}
# CI's RUNNER_TEMP holds the capture's clone, which is removed and bind-mounted
# (below), so a value that is not a plain absolute directory is refused rather
# than resolved somewhere else.
if [ -n "${RUNNER_TEMP:-}" ]; then
  case "$RUNNER_TEMP/" in
    */../* | */./*) bad=1 ;;
    /?*) bad="" ;;
    *) bad=1 ;;
  esac
  [ -z "$bad" ] && [[ $RUNNER_TEMP =~ ^[A-Za-z0-9._/-]+$ ]] || {
    echo "ci-capture: RUNNER_TEMP='$RUNNER_TEMP' is not an absolute directory" >&2
    echo "ACTION: unset RUNNER_TEMP, or set it to an absolute path with no '.' or '..' segment" >&2
    exit 2
  }
fi
command -v docker >/dev/null || {
  echo "ci-capture: docker not found; the capture runs in the image visual-docs.yml names" >&2
  echo "ACTION: install Docker, then re-run" >&2
  exit 1
}
[ -e .github/node_modules/yaml/package.json ] || npm ci --prefix .github --ignore-scripts --silent >&2

# Both values, read as YAML rather than matched as text: the capture-command is a
# block scalar, and its indentation is the parser's to undo.
read_with() {
  node --input-type=module -e '
    import { readFileSync } from "node:fs";
    import { createRequire } from "node:module";
    const { parse } = createRequire(process.cwd() + "/.github/package.json")("yaml");
    let workflow;
    try {
      workflow = parse(readFileSync(".github/workflows/visual-docs.yml", "utf8"));
    } catch (error) {
      process.stderr.write(
        `visual-docs.yml could not be read as YAML: ${error.message}\n` +
          "ACTION: restore .github/workflows/visual-docs.yml to a workflow that parses\n",
      );
      process.exit(1);
    }
    const value = workflow?.jobs?.["visual-docs"]?.with?.[process.argv[1]];
    if (typeof value !== "string" || value.trim() === "") {
      process.stderr.write(
        `visual-docs.yml gives the visual-docs job no ${process.argv[1]}\n` +
          `ACTION: restore jobs.visual-docs.with.${process.argv[1]} in .github/workflows/visual-docs.yml\n`,
      );
      process.exit(1);
    }
    process.stdout.write(value);
  ' "$1"
}
image="$(read_with container)" || exit 1
command="$(read_with capture-command)" || exit 1

# llmlint: ignore-block[changed_behavior_has_e2e] the capture runs a Rust image and builds the release binaries inside it;
# `lint` runs it for real on every pull request, against the committed baseline,
# which is its end-to-end proof. Its refusals are in tests/ci_jobs/proofs.test.mjs,
# and the command it runs is held under dash by tests/visual_docs.rs.
# In CI the clone lives outside `target/`: `lint` starts this beside
# Swatinem/rust-cache, whose restore pre-cleans `target/` on a partial key match
# (any change to a manifest or the lock) and deletes everything in a directory
# there but its `build`, `deps` and `.fingerprint`, the clone's `.git` included.
copy="${RUNNER_TEMP:-$ROOT/target}/ci-capture"
head="$(git rev-parse HEAD)"
# Kept only while it is a repository of its own at HEAD: a directory with no
# `.git` of its own would answer with the enclosing checkout's HEAD.
if [ "$(git -C "$copy" rev-parse --show-toplevel 2>/dev/null)" != "$(cd "$copy" 2>/dev/null && pwd -P)" ] ||
  [ "$(git -C "$copy" rev-parse HEAD 2>/dev/null)" != "$head" ]; then
  rm -rf -- "$copy"
  git clone --quiet --no-checkout "$ROOT" "$copy"
  git -C "$copy" checkout --quiet --detach "$head"
fi
mkdir -p shots

echo "ci-capture: running visual-docs.yml's capture-command in $image into $SHOTS_OUT"
# Mounted where the reusable workflow's job container checks out, because the
# capture normalises its own checkout's path out of what the CLI prints: a
# short mount point such as `/w` also matches inside other paths a shot shows.
inside=/__w/onepipeline/onepipeline
# Its cargo target lives outside the clone, so a new HEAD's clone keeps the
# build, and `lint` restores what main's warm run built into it.
build="$ROOT/target/ci-capture-target"
mkdir -p "$build"
# Built for capturing, not for shipping: what a shot shows is the text a verb
# prints, which no optimisation level changes, while the release profile's thin
# LTO and single codegen unit cost `lint` most of its 30 minutes. These are
# cargo's own profile overrides, set for this build alone; the shipped
# profile is untouched.
docker run --rm \
  -e CARGO_PROFILE_RELEASE_LTO=false \
  -e CARGO_PROFILE_RELEASE_CODEGEN_UNITS=256 \
  -e CARGO_PROFILE_RELEASE_OPT_LEVEL=0 \
  -e CARGO_INCREMENTAL=0 \
  -v "$copy":"$inside" -v "$ROOT/shots":"$inside/shots" -v "$build":"$inside/target" -w "$inside" \
  -e SHOTS_OUT \
  -e HOST_OWNER="$(id -u):$(id -g)" \
  -e GIT_CONFIG_COUNT=1 -e GIT_CONFIG_KEY_0=safe.directory -e GIT_CONFIG_VALUE_0="$inside" \
  -e INSIDE="$inside" \
  "$image" sh -e -c 'trap "chown -R \"\$HOST_OWNER\" \"\$INSIDE\"" EXIT
'"$command" || {
  status=$?
  echo "ci-capture: the capture-command failed in $image (exit $status); its output is above" >&2
  echo "ACTION: fix what it reports, then re-run 'SHOTS_OUT=$SHOTS_OUT bash screenshots/ci-capture.sh'; with no output, check 'docker info' answers and 'docker pull $image' succeeds" >&2
  exit "$status"
}
# llmlint: ignore-end[changed_behavior_has_e2e]
