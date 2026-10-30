#!/usr/bin/env node
// Renders the split replay's evidence into docs/modularization-test-split.md, after its marker.
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { affectedSuites, compileMapping } from "../crate-split-replay.mjs";

export const SPLIT_MARKER = "<!-- BEGIN TEST SPLIT -->";
const DOC = "docs/modularization-test-split.md";
const PLATFORMS = ["linux", "macos", "windows", "local"];
const pct = (value) => `${(value * 100).toFixed(2)}%`;
const secs = (value) => `${Math.round(value)} s (${(value / 60).toFixed(1)} min)`;
const cell = (value) => String(value).replaceAll("|", "\\|").replaceAll("\n", " ");
const pr = (row) => /#(\d+)\)/.exec(row.subject)?.[0]?.replace(")", "") ?? row.commit.slice(0, 8);

/** The crate-only target's figures, as the target layout's generated table states them. */
export function crateOnly(targetDoc) {
  const rows = {};
  for (const match of targetDoc.matchAll(
    /^\| (local|linux|macos|windows) \| ([\d.]+)% \| ([\d.]+)% \| ([\d.]+)% \| ([\d.]+)% \|$/gm,
  ))
    rows[match[1]] = {
      baselineSuites: match[2],
      baselineSeconds: match[3],
      suites: match[4],
      seconds: match[5],
    };
  if (PLATFORMS.some((platform) => !rows[platform]))
    throw new Error("the target layout's baseline table names no figure for every platform");
  return rows;
}

/** What kind of input a forcing path is, and whether a design change removes it. */
export function classify(path) {
  if (path.startsWith("resolved/") && path.endsWith("/lock.json"))
    return ["lockfile resolution", "no: a crate's resolved dependencies changed"];
  if (path.startsWith("resolved/") && path.endsWith("/manifest.json"))
    return ["manifest requirement", "no: a crate's dependency requirement changed"];
  if (path.startsWith("resolved/")) return ["toolchain", "no: the compiler changed"];
  if (path.startsWith("migrated-tests/") && path.endsWith("tests/e2e/harness.rs"))
    return [
      "harness item outside one helper",
      "by a further split: harness.rs into per-concern files, so World's core, its Drop and its imports stop sharing a file with the store and hook helpers (helpers move; no assertion changes)",
    ];
  if (path.startsWith("migrated-tests/") && path.includes("/config/"))
    return ["test configuration section", "no: the section every runner reads changed"];
  if (path.startsWith("migrated-tests/"))
    return [
      "test file outside one test",
      "by a further split: each part's tests in their own file, so a module's imports and added tests reach only that part (tests move; no assertion changes)",
    ];
  if (path.startsWith("moved/") || path.startsWith("src/"))
    return ["engine source", "no: shared engine code changed (see the target layout)"];
  if (path.startsWith("crates/testfakes/"))
    return ["test fake", "no: a fake every reaching suite starts"];
  if (/^(nx\.json|package(-lock)?\.json)$/.test(path))
    return ["workspace tooling", "no: Nx and its lockfile are inputs of every target"];
  if (path.startsWith("docs/"))
    return [
      "document read whole",
      "only by a test edit giving the read a needle that names its section",
    ];
  return ["other", "no"];
}

export function splitAppendix({ split, result }, targetDoc) {
  const before = crateOnly(targetDoc);
  const compiled = compileMapping(result.mapping.selection);
  const linux = result.platforms.linux;
  const total = linux.secondsPerRun * result.rows.length;
  const lines = [
    SPLIT_MARKER,
    "",
    "## Replay of crates and test split together",
    "",
    `${result.rows.length} PRs, ${result.tests} test phases of ${result.originalTests} existing e2e tests, ${result.suites} suites. Seconds are the profile's module measurements divided evenly among each module's tests, as in the target layout.`,
    "",
    "| Platform | Baseline suite runs skipped | Baseline seconds skipped | Crates only: suite runs | Crates only: seconds | Crates and test split: suite runs | Crates and test split: seconds |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...PLATFORMS.map(
      (platform) =>
        `| ${platform} | ${before[platform].baselineSuites}% | ${before[platform].baselineSeconds}% | ${before[platform].suites}% | ${before[platform].seconds}% | ${pct(result.platforms[platform].suiteSkip)} | ${pct(result.platforms[platform].secondsSkip)} |`,
    ),
    "",
    "## Test-split migration steps",
    "",
    "Each row is the skip share once that step has landed on top of the complete crate layout. Suite-run shares use that step's own suite count as their denominator.",
    "",
    "| Step | Suites | Suite runs skipped | Linux seconds | macOS seconds | Windows seconds | Local seconds |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...result.splitMigration.map(
      (step, index) =>
        `| ${index + 1}. ${step.step} | ${step.suites} | ${pct(step.suiteSkip)} | ${pct(step.secondsSkip.linux)} | ${pct(step.secondsSkip.macos)} | ${pct(step.secondsSkip.windows)} | ${pct(step.secondsSkip.local)} |`,
    ),
    "",
    "## Projected job wall clock against the 30-minute line",
    "",
    "Typical is the median PR, worst is the PR selecting the most estimated seconds. A projection holds the measured job less its e2e tier fixed and scales the e2e tier by the PR's selected share; the stress columns add the target layout's all-runner compile stress. Windows is one job whose e2e tier is both of today's legs added: a second leg would be sharding, which does not count toward the line.",
    "",
    "| Job | Today, full sweep | Fixed residual | e2e tier | Typical selected share | Typical projected | Worst projected | PRs over 30 min | Typical with compile stress | Worst with compile stress |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...["linux", "macos", "windows"].map((platform) => {
      const wall = result.wall[platform];
      return `| ${platform} | ${secs(wall.today)} | ${secs(wall.residual)} | ${secs(wall.e2e)} | ${pct(wall.typicalShare)} | ${secs(wall.projected.median)} | ${secs(wall.projected.max)} | ${wall.projected.over}/${result.rows.length} | ${secs(wall.withCompileStress.median)} | ${secs(wall.withCompileStress.max)} |`;
    }),
    "",
    `Local gate test phase: ${secs(result.wall.local.testPhase)} measured; typical projected ${secs(result.wall.local.projected.median)}, worst ${secs(result.wall.local.projected.max)}. This scales only the test phase; the gate's compilation and other tiers are not modelled.`,
    "",
  ];
  // Every selected suite-second charged evenly to the inputs that selected that suite.
  const charged = {};
  const removals = new Map([["unattributed", "no"]]);
  for (const row of result.rows) {
    const causes = {};
    for (const file of row.effectiveFiles)
      for (const suite of affectedSuites(compiled, [file]).affected) {
        causes[suite] ??= new Set();
        causes[suite].add(classify(file)[0]);
      }
    for (const file of row.effectiveFiles) removals.set(...classify(file));
    for (const suite of row.affected) {
      const kinds = [...(causes[suite] ?? ["unattributed"])];
      for (const kind of kinds) {
        charged[kind] ??= { seconds: 0, prs: new Set(), removal: removals.get(kind) };
        charged[kind].seconds += linux.suiteSeconds[suite] / kinds.length;
        charged[kind].prs.add(row.commit);
      }
    }
  }
  lines.push(
    "## What still selects the seconds",
    "",
    "Each selected suite-second is charged evenly to the kinds of input that selected that suite in that PR.",
    "",
    "| Input kind | Share of all Linux suite-seconds the 144 PRs would run | PRs | Removed by a design change |",
    "| --- | ---: | ---: | --- |",
    ...Object.entries(charged)
      .sort((a, b) => b[1].seconds - a[1].seconds)
      .map(
        ([kind, entry]) =>
          `| ${kind} | ${pct(entry.seconds / total)} | ${entry.prs.size} | ${cell(entry.removal)} |`,
      ),
    "",
    "## Every PR still running more than half the suites",
    "",
    "The forcing inputs are the PR's inputs that each select more than half the suites.",
    "",
    "| PR / commit | Suites selected | Linux seconds selected | Forcing inputs | Input kinds | Removed by a design change |",
    "| --- | ---: | ---: | --- | --- | --- |",
  );
  for (const row of result.rows.filter((entry) => entry.affected.length > result.suites / 2)) {
    const forcing = row.effectiveFiles.filter(
      (file) => affectedSuites(compiled, [file]).affected.length > result.suites / 2,
    );
    const share =
      row.affected.reduce((sum, suite) => sum + linux.suiteSeconds[suite], 0) / linux.secondsPerRun;
    const kinds = [...new Set(forcing.map((file) => classify(file)[0]))];
    const removable = [...new Set(forcing.map((file) => classify(file)[1]))];
    // A raw src/ path is a hunk outside every attributed item, so its whole file stays an input.
    const named = [...new Set(forcing.map((file) => file.replace(/^moved\/[^/]+\//, "")))].map(
      (file) => (forcing.includes(file) && file.startsWith("src/") ? `${file} (whole file)` : file),
    );
    lines.push(
      `| ${pr(row)} / ${row.commit.slice(0, 8)} | ${row.affected.length} | ${pct(share)} | ${cell(named.join(", ") || "several narrower inputs together")} | ${cell(kinds.join("; ") || "none alone")} | ${cell(removable.join("; ") || "no single input selects more than half")} |`,
    );
  }
  const scopes = Object.entries(result.scopes);
  const whole = {};
  for (const [name, suite] of Object.entries(result.mapping.selection.suites))
    for (const path of suite.paths)
      if (/^docs\/(contract|contract-divergences|stop-guard)\.md$/.test(path)) {
        whole[path] ??= [];
        whole[path].push(name);
      }
  lines.push(
    "",
    "## Contract-document reads scoped to sections",
    "",
    "A scoped read is selected only when the heading-bounded section its needles name changes, or when its needles stop naming exactly one section.",
    "",
    "| Scoped input | Needles | Suites reading it |",
    "| --- | --- | --- |",
    ...scopes.map(
      ([key, entry]) =>
        `| ${cell(key)} | ${cell(entry.needles.map((needle) => JSON.stringify(needle)).join(", "))} | ${Object.entries(
          result.mapping.selection.suites,
        )
          .filter(([, suite]) => suite.paths.includes(key))
          .map(([name]) => name)
          .join(", ")} |`,
    ),
    "",
    "Reads still keeping a whole document, because no needle in the reading code names one section (a module-level `include_str!` constant, or a passage found by a needle the document repeats):",
    "",
    "| Document | Suites reading it whole |",
    "| --- | --- |",
    ...Object.entries(whole).map(([path, names]) => `| ${path} | ${names.join(", ")} |`),
    "",
    "## Every split suite and the tests it runs",
    "",
    "Each part runs as a nextest filter over its parent's runner binary, so no runner is added. Runner crates are the crates the part's tests reach; a cross-command part links only the commands its journeys drive, and keeps the dispatcher and facade where a phase uses the public API or has unresolved argv.",
    "",
    "| Suite | Parent | Tests | Linux s per run | Runner crates | Tests it runs |",
    "| --- | --- | ---: | ---: | ---: | --- |",
  );
  for (const [name, suite] of Object.entries(split.suites))
    lines.push(
      `| ${name} | ${suite.parent} | ${suite.tests.length} | ${linux.suiteSeconds[name].toFixed(1)} | ${result.mapping.crates[`runner-${name}`].deps.length} | ${suite.tests.join(", ")} |`,
    );
  return `${lines.join("\n")}\n`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length !== 3) throw new Error("usage: split-report.mjs SPLIT-REPLAY.json");
    const parts = readFileSync(DOC, "utf8").split(SPLIT_MARKER);
    if (parts.length !== 2) throw new Error("expected one test split marker");
    const evidence = JSON.parse(readFileSync(process.argv[2], "utf8"));
    if (!evidence || Object.keys(evidence).some((key) => !["split", "result"].includes(key)))
      throw new Error("unknown split evidence field");
    writeFileSync(
      DOC,
      `${parts[0]}${splitAppendix(evidence, readFileSync("docs/modularization-target-layout.md", "utf8"))}`,
    );
    console.log(`test split: rendered ${evidence.result.suites} suites`);
  } catch (error) {
    const next =
      error.code && /^(?:EACCES|EPERM|ENOENT|EISDIR|ENOTDIR|ENOSPC|EROFS)$/.test(error.code)
        ? "check the named path's type, existence, permissions and disk space; rerun the same split-report.mjs invocation"
        : "generate a split replay and restore exactly one test split marker";
    console.error(`${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}
