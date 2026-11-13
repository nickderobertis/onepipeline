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
// binary replaced by its suites (its `[[test]]` and its `tests/e2e/main.rs` root both
// removed, since Cargo would discover the root by itself); `c` is `prototype-c.mjs`, the
// lock refreshed offline for its two new workspace members. `npm/test/reuse-demo.test.mjs`
// holds the workflow's setup steps equal to ci.yml's test jobs'.
//
// Exit 0: the demonstration is written; one line says which and where. 1: an argument or
// the checkout was refused, with a `next:` action on stderr.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { prototypeB } from "./prototype-b.mjs";
import { prototypeC } from "./prototype-c.mjs";
import * as check from "./validate.mjs";

const USAGE = "node scripts/crate-split/demo-commit.mjs --repo DIR --variant today|b|c";

/** A GitHub Actions expression, spelled for the workflow rather than for this script. */
const expr = (body) => `$\{{ ${body} }}`;

// llmlint: ignore-block[tool_output_is_signal] the three echo lines of "The tree this run measures" are the measured run's provenance (GITHUB_SHA, head and tree; the executed workflow's blob; the runner), each read by demo-runs.mjs; one line would change the workflow blob the recorded demonstrations ran (3d81753b817c), which this generator must still reproduce.
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

// llmlint: ignore-end[tool_output_is_signal]

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

export const VARIANTS = ["today", "b", "c"];

/**
 * Write one demonstration into the checkout at `repo`: the measurement-only workflow
 * changes and the layout `variant` names. It edits files and commits nothing; the
 * demonstration's one commit is made from what it wrote.
 */
export function writeDemonstration(repo, variant) {
  check.need(VARIANTS.includes(variant), `--variant ${variant} is not today, b or c`, USAGE);
  const workflows = join(repo, ".github", "workflows");
  check.need(
    existsSync(join(workflows, "visual-docs.yml")),
    `${repo} holds no .github/workflows/visual-docs.yml`,
    "give --repo a checkout of this repository",
  );
  writeFileSync(join(workflows, "ci.yml"), MEASURE_WORKFLOW);
  // Kept, since a test compiles it in (`include_str!`), but started by nothing a
  // demonstration does: its pull_request triggers become a manual dispatch.
  const visual = join(repo, ".github", "workflows", "visual-docs.yml");
  const text = readFileSync(visual, "utf8");
  const triggers = /^on:\n(?:[ \t].*\n|\n)*/m;
  check.need(
    triggers.test(text),
    "visual-docs.yml has no top-level on: block to replace",
    "check the workflow still starts with an on: block",
  );
  writeFileSync(visual, text.replace(triggers, "on:\n  workflow_dispatch:\n\n"));
  if (variant === "b") {
    prototypeB(repo);
    const manifest = join(repo, "Cargo.toml");
    const text = readFileSync(manifest, "utf8");
    const e2e = /\n\[\[test\]\]\nname = "e2e"\npath = "tests\/e2e\/main\.rs"\n/;
    check.need(
      e2e.test(text),
      "Cargo.toml declares no e2e test target to replace",
      "give --repo a checkout whose Cargo.toml has the e2e [[test]]",
    );
    writeFileSync(manifest, text.replace(e2e, "\n"));
    // Cargo discovers `tests/<dir>/main.rs` as a test of that name by itself, so the
    // one binary is gone only once its root, which declares modules and nothing else,
    // is: every suite loads the modules it needs from where they are.
    rmSync(join(repo, "tests", "e2e", "main.rs"));
  } else if (variant === "c") {
    prototypeC(repo);
    try {
      execFileSync("cargo", ["metadata", "--format-version", "1", "--offline"], {
        cwd: repo,
        stdio: ["ignore", "ignore", "pipe"],
      });
    } catch (error) {
      throw new check.Refusal(
        `cargo could not refresh the lock for layout c: ${String(error.stderr ?? error.message)
          .trim()
          .split("\n")
          .at(-1)}`,
        "fetch the workspace's dependencies first (cargo fetch --locked) and re-run",
      );
    }
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    const options = check.parseOptions(process.argv.slice(2), ["repo", "variant"], {
      usage: USAGE,
    });
    check.need(options.repo && options.variant, "name the checkout and the layout", USAGE);
    writeDemonstration(options.repo, options.variant);
    console.log(`demo-commit: wrote the ${options.variant} demonstration into ${options.repo}`);
  } catch (error) {
    console.error(`demo-commit: ${error.message.split("\n")[0]}`);
    console.error(`next: ${error instanceof check.Refusal ? error.next : USAGE}`);
    process.exitCode = 1;
  }
}
