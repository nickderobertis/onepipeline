#!/usr/bin/env node
// Renders the engine re-homing's evidence into docs/modularization-engine-tests.md, after its marker.
// `--collect A.json B.json C.json` gathers engine-replay.mjs outputs, one per migration step in
// order, into engine-evidence.json; `--write` and `--check` render that file into the document.
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { ENGINE_PATH, KEEP, PARENTS } from "./engine-tests.mjs";

export const ENGINE_MARKER = "<!-- BEGIN ENGINE TESTS -->";
export const EVIDENCE_PATH = "scripts/modularization/engine-evidence.json";
const DOC = "docs/modularization-engine-tests.md";
const SPLIT_DOC = "docs/modularization-test-split.md";
const PLATFORMS = ["linux", "macos", "windows", "local"];
const pct = (value) => `${(value * 100).toFixed(2)}%`;
const mins = (value) => `${Math.round(value)} s (${(value / 60).toFixed(1)} min)`;
const cell = (value) => String(value).replaceAll("|", "\\|").replaceAll("\n", " ");
const read = (path) => readFileSync(path, "utf8");

/** The crates-and-test-split figures, as the test-split document's generated table states them. */
export function testSplitOnly(splitDoc) {
  const rows = {};
  for (const match of splitDoc.matchAll(
    /^\| (local|linux|macos|windows) \| ([\d.]+)% \| ([\d.]+)% \| ([\d.]+)% \| ([\d.]+)% \| ([\d.]+)% \| ([\d.]+)% \|$/gm,
  ))
    rows[match[1]] ??= { suites: match[6], seconds: match[7] };
  if (PLATFORMS.some((platform) => !rows[platform]))
    throw new Error(`${SPLIT_DOC} names no crates-and-test-split figure for every platform`);
  return rows;
}

/**
 * Refuses evidence that is not one replay per migration step, in order, each re-homing one more
 * parent than the last, with every share a fraction and every count within what it counts.
 */
export function validateEngineEvidence(evidence) {
  const fraction = (value) =>
    typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 1;
  if (
    !evidence ||
    typeof evidence !== "object" ||
    Array.isArray(evidence) ||
    Object.keys(evidence).some((key) => !["schema", "steps"].includes(key)) ||
    evidence.schema !== 1 ||
    !Array.isArray(evidence.steps) ||
    !evidence.steps.length
  )
    throw new Error(`invalid ${EVIDENCE_PATH}`);
  const moving = PARENTS.filter((parent) => parent !== "cross-command");
  for (const [index, step] of evidence.steps.entries()) {
    const where = `${EVIDENCE_PATH} step ${index + 1}`;
    if (JSON.stringify(step?.parents) !== JSON.stringify(moving.slice(0, index + 1)))
      throw new Error(`${where} must re-home ${moving.slice(0, index + 1).join(", ")}`);
    if (step.schema !== 1 || step.prs !== 144 || !Number.isInteger(step.suites))
      throw new Error(`${where} is not a 144-PR replay`);
    for (const platform of PLATFORMS)
      if (
        !fraction(step.platforms?.[platform]?.suiteSkip) ||
        !fraction(step.platforms[platform].secondsSkip)
      )
        throw new Error(`${where} has no skip shares for ${platform}`);
    if (!Number.isInteger(step.broad) || step.broad < 0 || step.broad > step.prs)
      throw new Error(`${where} counts its broad PRs out of range`);
    if (!Array.isArray(step.overHalf) || step.overHalf.some((row) => !(row.share > 0.5)))
      throw new Error(`${where} lists a PR under half the seconds as over it`);
    if (!Array.isArray(step.crateReach) || !Array.isArray(step.engineSuites))
      throw new Error(`${where} lacks its crate reach or engine suites`);
  }
  if (evidence.steps.length !== moving.length)
    throw new Error(`${EVIDENCE_PATH} must hold one step per re-homed parent`);
  return evidence;
}

/** The generated part of the engine-tests document. */
export function engineAppendix(evidence, split, splitDoc) {
  validateEngineEvidence(evidence);
  const final = evidence.steps.at(-1);
  const before = testSplitOnly(splitDoc);
  const lines = [ENGINE_MARKER, ""];
  const moved = Object.entries(split.moved);
  const kept = Object.entries(split.kept);
  lines.push(
    "## Replay of crates, test split and engine re-homing together",
    "",
    `**Projected** (replay model): 144 PRs; ${moved.length} test phases re-homed onto ${new Set(moved.map(([, entry]) => entry.crate)).size} engine crates in ${final.engineSuites.length} engine suites; ${kept.length} stay full journeys through the binary; ${final.suites} suites in all. Seconds are the profile's measured module seconds divided evenly among each module's tests. The test-split columns are the test-split document's projections.`,
    "",
    "| Platform | Crates and test split: suite runs | Crates and test split: seconds | With engine re-homing: suite runs | With engine re-homing: seconds |",
    "| --- | ---: | ---: | ---: | ---: |",
    ...PLATFORMS.map(
      (platform) =>
        `| ${platform} | ${before[platform].suites}% | ${before[platform].seconds}% | ${pct(final.platforms[platform].suiteSkip)} | ${pct(final.platforms[platform].secondsSkip)} |`,
    ),
    "",
    "## Engine re-homing migration steps",
    "",
    "**Projected** (replay model): each row is the skip share once that step has landed on top of the complete crate layout and every test-split step. Suite-run shares use that step's own suite count as their denominator.",
    "",
    "| Step | Suites | Suite runs skipped | Linux seconds | macOS seconds | Windows seconds | Local seconds |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...evidence.steps.map(
      (step, index) =>
        `| ${8 + index}. re-home ${step.parents.at(-1)}'s engine assertions | ${step.suites} | ${pct(step.platforms.linux.suiteSkip)} | ${PLATFORMS.map((platform) => pct(step.platforms[platform].secondsSkip)).join(" | ")} |`,
    ),
    "",
    "## Projected job wall clock against the 30-minute line",
    "",
    "**Projected**: the measured job less its e2e tier is held fixed and the e2e tier scales with the PR's selected share, as in the test-split document. Typical is the median PR, worst the PR selecting the most estimated seconds.",
    "",
    "| Job | Today, full sweep | Fixed residual | Typical selected share | Typical projected | Worst projected | PRs over 30 min |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...["linux", "macos", "windows"].map((platform) => {
      const wall = final.wall[platform];
      return `| ${platform} | ${mins(wall.today)} | ${mins(wall.residual)} | ${pct(wall.typicalShare)} | ${mins(wall.median)} | ${mins(wall.max)} | ${wall.over}/144 |`;
    }),
    "",
    `Local gate test phase: ${mins(final.wall.local.testPhase)} **measured** (the profile's run F); **projected** typical ${mins(final.wall.local.median)}, worst ${mins(final.wall.local.max)}.`,
    "",
    "## What still selects the seconds",
    "",
    `**Projected** (replay model): ${final.broad} of 144 PRs still select at least 90% of a full run's estimated Linux seconds. A PR counts under every input kind whose own inputs alone select at least 90%; one with no such kind counts under "several inputs together".`,
    "",
    "| Forcing input kind | PRs it forces | PRs it alone forces |",
    "| --- | ---: | ---: |",
    ...Object.entries(final.causes)
      .sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1))
      .map(([kind, count]) => `| ${kind} | ${count} | ${final.sole[kind] ?? 0} |`),
    "",
    `Of the lockfile-forced PRs, ${final.lock.sibling} moved only sibling-engine packages, ${final.lock.thirdParty} only third-party packages and ${final.lock.both} both.`,
    "",
    "### Crates whose change still selects more than half the suites",
    "",
    "**Projected**: what a change to the crate alone selects, and how many of the 144 PRs changed its source, its resolved dependencies or its manifest requirement. Every production crate here is in the closure of `cmd-start`, `cmd-reply` or `cmd-adopt`, whose kept full journeys link the whole command, and every test-fake crate here is one those journeys start or link; no further crate split removes that. Only moving more of those journeys onto engine crates, which needs attributable assertions (see the kept reasons below), frees it.",
    "",
    "| Crate | Suites selected | Linux seconds selected | PRs changing it |",
    "| --- | ---: | ---: | ---: |",
    ...final.crateReach.map(
      (entry) =>
        `| ${entry.crate} | ${entry.suites}/${final.suites} | ${pct(entry.linuxShare)} | ${entry.prs} |`,
    ),
    "",
    "### PRs still selecting more than half the Linux seconds",
    "",
    `**Projected**: ${final.overHalf.length} of 144 PRs, each with the input kinds whose own inputs select more than half the seconds.`,
    "",
    "| PR | Linux seconds selected | Forcing input kinds |",
    "| --- | ---: | --- |",
    ...final.overHalf.map(
      (row) =>
        `| ${row.pr ? `#${row.pr}` : row.commit} / ${row.commit} | ${pct(row.share)} | ${cell(row.forcing.join("; "))} |`,
    ),
    "",
    "## Every engine suite",
    "",
    "**Projected**: each engine suite's crate, its tests, the crates its runner links (the crate's closure), its share of a full Linux run and the share of the 144 PRs that skip it.",
    "",
    "| Suite | Crate | Tests | Closure crates | Linux share of a full run | PRs skipping it |",
    "| --- | --- | ---: | ---: | ---: | ---: |",
    ...final.engineSuites.map(
      (suite) =>
        `| ${suite.suite} | ${suite.crate} | ${suite.tests} | ${suite.closure} | ${pct(suite.linuxShare)} | ${pct(suite.skipped)} |`,
    ),
    "",
    `The engine suites together hold ${pct(final.engineSuites.reduce((sum, suite) => sum + suite.linuxShare, 0))} of a full Linux run's estimated seconds; the median engine suite is skipped by ${pct([...final.engineSuites.map((suite) => suite.skipped)].sort((a, b) => a - b)[final.engineSuites.length >> 1])} of the 144 PRs.`,
    "",
    "## The full journeys that stay",
    "",
    "| Parent | Reason | Tests |",
    "| --- | --- | ---: |",
  );
  const keptCounts = {};
  for (const [, entry] of kept) {
    const key = `${entry.parent}|${entry.reason}`;
    keptCounts[key] = (keptCounts[key] ?? 0) + 1;
  }
  for (const [key, count] of Object.entries(keptCounts).sort((a, b) => (a[0] < b[0] ? -1 : 1))) {
    const [parent, reason] = key.split("|");
    lines.push(`| ${parent} | ${reason}: ${KEEP[reason]} | ${count} |`);
  }
  const assertedBy = {};
  for (const [, entry] of kept)
    for (const crate of new Set(Object.values(entry.evidence).flat()))
      assertedBy[crate] = (assertedBy[crate] ?? 0) + 1;
  lines.push(
    "",
    "Each engine crate that receives tests, with how many of the kept full journeys still assert a literal it alone (or with one other crate) writes, end to end through the binary:",
    "",
    "| Engine crate | Re-homed tests | Kept full journeys still asserting it |",
    "| --- | ---: | ---: |",
    ...[...new Set(moved.map(([, entry]) => entry.crate))]
      .sort()
      .map(
        (crate) =>
          `| ${crate} | ${moved.filter(([, entry]) => entry.crate === crate).length} | ${assertedBy[crate] ?? 0} |`,
      ),
    "",
    "## Every test that leaves the binary",
    "",
    "Each re-homed test phase, the crate-level test that keeps every assertion it makes, the recorded fixture that test reads, and the literals that attributed it (at most three shown; `engine-tests.json` holds all).",
    "",
    "| Test | Parent | Engine suite | Crate-level test | Recorded fixture | Attributed by |",
    "| --- | --- | --- | --- | --- | --- |",
    ...moved.map(
      ([id, entry]) =>
        `| ${id} | ${entry.parent} | ${entry.suite} | ${entry.crateTest} | ${entry.fixture} | ${cell(
          Object.keys(entry.evidence)
            .slice(0, 3)
            .map((literal) => `\`${literal}\``)
            .join(", "),
        )} |`,
    ),
    "",
    "## Every full journey that stays",
    "",
    "| Test | Parent | Reason |",
    "| --- | --- | --- |",
    ...kept.map(([id, entry]) => `| ${id} | ${entry.parent} | ${entry.reason} |`),
    "",
  );
  return lines.join("\n");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const mode = process.argv[2];
    if (mode === "--collect") {
      if (process.argv.length < 4)
        throw new Error("usage: engine-report.mjs --collect STEP.json...");
      const evidence = validateEngineEvidence({
        schema: 1,
        steps: process.argv.slice(3).map((path) => JSON.parse(read(path))),
      });
      writeFileSync(EVIDENCE_PATH, `${JSON.stringify(evidence, null, 2)}\n`);
    } else if (process.argv.length !== 3 || !["--write", "--check"].includes(mode))
      throw new Error("usage: engine-report.mjs --collect STEP.json... | --write | --check");
    const parts = read(DOC).split(ENGINE_MARKER);
    if (parts.length !== 2) throw new Error(`${DOC} must hold its marker exactly once`);
    const expected = `${parts[0]}${engineAppendix(
      JSON.parse(read(EVIDENCE_PATH)),
      JSON.parse(read(ENGINE_PATH)),
      read(SPLIT_DOC),
    )}`;
    if (mode === "--check") {
      if (read(DOC) !== expected)
        throw new Error(`${DOC} differs from what engine-report.mjs renders`);
    } else writeFileSync(DOC, expected);
    console.log(`engine report: ${mode === "--check" ? "checked" : "wrote"} ${DOC}`);
  } catch (error) {
    const next =
      error.code && /^(?:EACCES|EPERM|ENOENT|EISDIR|ENOTDIR|ENOSPC|EROFS)$/.test(error.code)
        ? "check the named path's type, existence, permissions and disk space; rerun the same engine-report.mjs invocation"
        : "regenerate the evidence with engine-replay.mjs and engine-report.mjs --collect, then engine-report.mjs --write";
    console.error(`${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}
