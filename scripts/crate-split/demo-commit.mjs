#!/usr/bin/env node
// Write one hosted compile-cost demonstration into a checkout of the session head: the
// measurement-only changes every demonstration carries byte-identically, and the one
// layout under test.
//
//   node scripts/crate-split/demo-commit.mjs --repo DIR --variant today|b|c
//
// The measurement-only changes: `.github/workflows/ci.yml` becomes MEASURE_WORKFLOW,
// which builds the test archive the CI tiers run from (`measure-archive.mjs`) on each
// of hosted Linux (instrumented, as test-linux), macOS and Windows (plain, as their
// `test-quick` tiers), twice each on separate runners, after the same checkout,
// toolchain, Dev Drive, Defender exclusion, tool install and read-only Rust cache
// restore those jobs make; and `.github/workflows/visual-docs.yml`'s triggers become
// `workflow_dispatch` alone, so closing the demonstration deploys and deletes nothing
// (the file stays: `tests/visual_docs.rs` compiles it in). Nothing in it writes an
// Actions-cache or Nx remote-cache entry: the Rust cache restores with `save-if:
// false`, there is no Nx sidecar, no `actions/cache/save` and no `setup-uv` cache.
//
// The layout: `today` changes nothing else; `b` is `prototype-b.mjs` with the one e2e
// binary replaced by its suites; `c` is `prototype-c.mjs`, the lock refreshed offline
// for its two new workspace members.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { prototypeB } from "./prototype-b.mjs";
import { prototypeC } from "./prototype-c.mjs";

/** A GitHub Actions expression, spelled for the workflow rather than for this script. */
const expr = (body) => `$\{{ ${body} }}`;

const job = (platform) => {
  const runner = { linux: "ubuntu-24.04", macos: "macos-latest", windows: "windows-latest" }[
    platform
  ];
  const key = { linux: "test-linux", macos: "test-macos", windows: "test-windows" }[platform];
  const tools = platform === "linux" ? "linux_tools" : "quick_tools";
  const mode = platform === "linux" ? "coverage" : "quick";
  const dir = platform === "windows" ? expr("env.DEV_DRIVE_WORKSPACE") : expr("github.workspace");
  const windows =
    platform === "windows"
      ? `      - uses: samypr100/setup-dev-drive@v3
        with:
          drive-size: 60GB
          drive-format: ReFS
          drive-type: Dynamic
          trusted-dev-drive: true
          workspace-copy: true
          env-mapping: |
            TEMP,{{ DEV_DRIVE }}/tmp
            TMP,{{ DEV_DRIVE }}/tmp
      - name: Keep Defender's real-time scan off the build
        shell: pwsh
        run: |
          Add-MpPreference -ExclusionPath $env:DEV_DRIVE, "$env:USERPROFILE\\.cargo", "$env:USERPROFILE\\.rustup", $env:RUNNER_TEMP
          New-Item -ItemType Directory -Force -Path $env:TEMP | Out-Null
`
      : "";
  return `  measure-${platform}:
    runs-on: ${runner}
    timeout-minutes: 120
    strategy:
      fail-fast: false
      matrix:
        sample: [1, 2]
    env:
      NEXTEST_PROFILE: ci
    defaults:
      run:
        shell: bash
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - name: Read the exact CI tool versions
        id: ci-tools
        run: node .github/read-ci-tool-versions.mjs >> "$GITHUB_OUTPUT"
${windows}      - uses: Swatinem/rust-cache@v2.9.2
        with:
          shared-key: ${key}
          save-if: false${platform === "windows" ? `\n          workspaces: ${expr("env.DEV_DRIVE_WORKSPACE")} -> target` : ""}
      - uses: actions/setup-node@v4
        with:
          node-version: \${{ steps.ci-tools.outputs.node }}
      - uses: taiki-e/install-action@v2
        with:
          tool: \${{ steps.ci-tools.outputs.${tools} }}
      - name: The tree this run measures
        working-directory: ${dir}
        run: |
          echo "GITHUB_SHA=$GITHUB_SHA head=\${{ github.event.pull_request.head.sha }} tree=$(git rev-parse 'HEAD^{tree}')"
          echo "workflow blob=$(git rev-parse HEAD:.github/workflows/ci.yml)"
          echo "runner=$RUNNER_OS/$RUNNER_ARCH cpus=$(node -p 'require("os").cpus().length')"
      - name: Build the ${mode} test archive, timed
        working-directory: ${dir}
        run: node scripts/crate-split/measure-archive.mjs ${mode}
`;
};

export const MEASURE_WORKFLOW = `# A throwaway compile-cost demonstration (scripts/crate-split/demo-commit.mjs): it
# builds the CI tiers' test archive on each platform and times it. It writes no cache.
name: CI

on:
  pull_request:

permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always

jobs:
${["linux", "macos", "windows"].map(job).join("\n")}`;

export function demoCommit(repo, variant) {
  writeFileSync(join(repo, ".github", "workflows", "ci.yml"), MEASURE_WORKFLOW);
  // Kept, since a test compiles it in (`include_str!`), but started by nothing a
  // demonstration does: its pull_request triggers become a manual dispatch.
  const visual = join(repo, ".github", "workflows", "visual-docs.yml");
  const text = readFileSync(visual, "utf8");
  const triggers = /^on:\n(?:[ \t].*\n|\n)*/m;
  if (!triggers.test(text))
    throw new Error("visual-docs.yml has no top-level on: block to replace");
  writeFileSync(visual, text.replace(triggers, "on:\n  workflow_dispatch:\n\n"));
  if (variant === "b") {
    prototypeB(repo);
    const manifest = join(repo, "Cargo.toml");
    const text = readFileSync(manifest, "utf8");
    const e2e = /\n\[\[test\]\]\nname = "e2e"\npath = "tests\/e2e\/main\.rs"\n/;
    if (!e2e.test(text)) throw new Error("Cargo.toml declares no e2e test target to replace");
    writeFileSync(manifest, text.replace(e2e, "\n"));
  } else if (variant === "c") {
    prototypeC(repo);
    execFileSync("cargo", ["metadata", "--format-version", "1", "--offline"], {
      cwd: repo,
      stdio: ["ignore", "ignore", "inherit"],
    });
  } else if (variant !== "today") throw new Error(`--variant ${variant} is not today, b or c`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const arg = (name) => process.argv[process.argv.indexOf(name) + 1];
  if (!process.argv.includes("--repo") || !process.argv.includes("--variant")) {
    console.error("demo-commit: --repo DIR --variant today|b|c");
    process.exitCode = 1;
  } else {
    demoCommit(arg("--repo"), arg("--variant"));
    console.log(`demo-commit: wrote the ${arg("--variant")} demonstration into ${arg("--repo")}`);
  }
}
