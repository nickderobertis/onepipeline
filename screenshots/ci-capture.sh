#!/usr/bin/env bash
# ci.yml's `lint` job's visual-docs capture: visual-docs.yml's own
# `capture-command`, read out of that workflow, run in the `container` image it
# names, against this checkout, writing to $SHOTS_OUT. One capture command, then,
# whichever of the two paths runs it: the reusable workflow hands that text to
# the image as a job container; this hands it to the same image with `docker run`
# under the same `sh -e`, because a step of a job on the runner cannot itself
# run in a container.
#
# It captures HEAD, from a clone of it under `target/ci-capture`, with only
# `shots/` mounted back into this checkout: the capture builds its release
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
    const value = parse(readFileSync(".github/workflows/visual-docs.yml", "utf8"))
      ?.jobs?.["visual-docs"]?.with?.[process.argv[1]];
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
copy="$ROOT/target/ci-capture"
head="$(git rev-parse HEAD)"
if [ "$(git -C "$copy" rev-parse HEAD 2>/dev/null)" != "$head" ]; then
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
docker run --rm \
  -v "$copy":"$inside" -v "$ROOT/shots":"$inside/shots" -w "$inside" \
  -e SHOTS_OUT \
  -e HOST_OWNER="$(id -u):$(id -g)" \
  -e GIT_CONFIG_COUNT=1 -e GIT_CONFIG_KEY_0=safe.directory -e GIT_CONFIG_VALUE_0='*' \
  -e INSIDE="$inside" \
  "$image" sh -e -c 'trap "chown -R \"\$HOST_OWNER\" \"\$INSIDE\"" EXIT
'"$command"
# llmlint: ignore-end[changed_behavior_has_e2e]
