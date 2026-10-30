#!/usr/bin/env node
// Renders the split replay's evidence into docs/modularization-test-split.md, after its marker.
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { affectedSuites, compileMapping } from "../crate-split-replay.mjs";
import { classify } from "./split-replay.mjs";

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
    `**Projected** (replay model): ${result.rows.length} PRs, ${result.tests} test phases of ${result.originalTests} existing e2e tests, ${result.suites} suites. Seconds are the profile's measured module seconds divided evenly among each module's tests, as in the target layout. The baseline and crate-only columns are the target layout's projections.`,
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
    "**Projected** (replay model): each row is the skip share once that step has landed on top of the complete crate layout. Suite-run shares use that step's own suite count as their denominator.",
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
    "The first three columns are **measured** on run 40947970908; the rest are **projected**. Typical is the median PR, worst is the PR selecting the most estimated seconds. A projection holds the measured job less its e2e tier fixed and scales the e2e tier by the PR's selected share; the stress columns add the target layout's all-runner compile stress. Windows is one job whose e2e tier is both of today's legs added: a second leg would be sharding, which does not count toward the line.",
    "",
    "| Job | Today, full sweep | Fixed residual | e2e tier | Typical selected share | Typical projected | Worst projected | PRs over 30 min | Typical with compile stress | Worst with compile stress |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...["linux", "macos", "windows"].map((platform) => {
      const wall = result.wall[platform];
      return `| ${platform} | ${secs(wall.today)} | ${secs(wall.residual)} | ${secs(wall.e2e)} | ${pct(wall.typicalShare)} | ${secs(wall.projected.median)} | ${secs(wall.projected.max)} | ${wall.projected.over}/${result.rows.length} | ${secs(wall.withCompileStress.median)} | ${secs(wall.withCompileStress.max)} |`;
    }),
    "",
    `Local gate test phase: ${secs(result.wall.local.testPhase)} **measured** (the profile's run F); **projected** typical ${secs(result.wall.local.projected.median)}, worst ${secs(result.wall.local.projected.max)}. This scales only the test phase; the gate's compilation and other tiers are not modelled.`,
    "",
  ];
  const { causes } = result;
  const freedBy = new Map(causes.freed.map((entry) => [entry.kinds.join(" + "), entry]));
  const anyRow = (kind) =>
    result.rows.find((row) => row.effectiveFiles.some((file) => classify(file, row)[0] === kind));
  const removal = (kind) => {
    const row = anyRow(kind);
    const file = row?.effectiveFiles.find((path) => classify(path, row)[0] === kind);
    return file ? classify(file, row)[1] : "no";
  };
  lines.push(
    "## Decision summary",
    "",
    `**Projected** (replay model): ${causes.broad} of ${result.rows.length} PRs still select at least 90% of a full run's estimated Linux seconds. A PR counts under every input kind whose own inputs alone select at least 90%; one with no such kind counts under "several inputs together". The skip shares treat that kind as selecting nothing: an upper bound where a further split could free it, and only a measure of its weight where no design change removes it.`,
    "",
    "| Forcing input kind | PRs it forces (of the broad PRs) | PRs it alone forces | Further design change that would free it | Suite runs skipped if freed | Linux seconds skipped if freed | PRs still at 90% or more |",
    "| --- | ---: | ---: | --- | ---: | ---: | ---: |",
    ...Object.entries(causes.prs)
      .sort((a, b) => b[1] - a[1])
      .map(([kind, count]) => {
        const freed = freedBy.get(kind);
        return `| ${kind} | ${count} | ${causes.sole[kind] ?? 0} | ${cell(kind === "several inputs together" ? "none: no single kind forces these" : removal(kind))} | ${freed ? pct(freed.suiteSkip) : "n/a"} | ${freed ? pct(freed.secondsSkip) : "n/a"} | ${freed ? freed.broad : "n/a"} |`;
      }),
    (() => {
      const all = causes.freed.at(-1);
      return `| every kind a further split could free (${all.kinds.join("; ")}) | | | all three splits together | ${pct(all.suiteSkip)} | ${pct(all.secondsSkip)} | ${all.broad} |`;
    })(),
    "",
    `**Measured** (each PR's own parent and merge lockfiles) for the ${causes.lock.sibling + causes.lock.thirdParty + causes.lock.both} broad PRs a lockfile resolution forces; sibling engines are the \`one*\` packages this repository links:`,
    "",
    "| Lockfile change | PRs |",
    "| --- | ---: |",
    `| sibling-engine packages only | ${causes.lock.sibling} |`,
    `| third-party packages only | ${causes.lock.thirdParty} |`,
    `| both | ${causes.lock.both} |`,
    "",
    "## Where each job's residual goes",
    "",
    "**Measured** on run 40947970908. Segments add up to the job; Linux's quality-gate tasks run concurrently, so its task durations overlap and are listed apart.",
    "",
  );
  for (const [platform, breakdown] of Object.entries(result.wallBreakdown)) {
    const legs = breakdown.legs ?? [{ segments: breakdown.segments }];
    for (const [index, leg] of legs.entries()) {
      lines.push(
        `**${platform}${legs.length > 1 ? `, leg ${index + 1}` : ""}** (${breakdown.how}):`,
        "",
        "| Segment | Seconds |",
        "| --- | ---: |",
        ...leg.segments.map(
          (segment) => `| ${cell(segment.part)} | ${segment.seconds.toFixed(1)} |`,
        ),
        "",
      );
    }
    if (breakdown.overlapping)
      lines.push(
        `Task durations the Linux job log reports, overlapping the segments above: ${breakdown.overlapping.map((task) => `${task.part} ${Math.round(task.seconds)} s`).join(", ")}. Not reported in the job log: ${breakdown.unreported}.`,
        "",
      );
  }
  // Every selected suite-second charged evenly to the inputs that selected that suite.
  const charged = {};
  const removals = new Map([["unattributed", "no"]]);
  for (const row of result.rows) {
    const causes = {};
    for (const file of row.effectiveFiles)
      for (const suite of affectedSuites(compiled, [file]).affected) {
        causes[suite] ??= new Set();
        causes[suite].add(classify(file, row)[0]);
      }
    for (const file of row.effectiveFiles) removals.set(...classify(file, row));
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
    "**Projected** (replay model): each selected suite-second is charged evenly to the kinds of input that selected that suite in that PR.",
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
    "**Projected** (replay model): the forcing inputs are the PR's inputs that each select more than half the suites.",
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
    const kinds = [...new Set(forcing.map((file) => classify(file, row)[0]))];
    const removable = [...new Set(forcing.map((file) => classify(file, row)[1]))];
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
    "Derived from the reviewed source's reading code: a scoped read is selected only when the heading-bounded section its needles name changes, or when its needles stop naming exactly one section.",
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
    "Linux seconds per run are **projected** (the profile's module seconds divided evenly). Each part runs as a nextest filter over its parent's runner binary, so no runner is added. Runner crates are the crates the part's tests reach; a cross-command part links only the commands its journeys drive, and keeps the dispatcher and facade where a phase uses the public API or has unresolved argv.",
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
