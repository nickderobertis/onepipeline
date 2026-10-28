#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { components } from "./graph.mjs";
import { evidence } from "./replay.mjs";

export const MARKER = "<!-- BEGIN GENERATED MODULARIZATION -->";
const pct = (fraction) => `${(100 * fraction).toFixed(2)}%`;

/** Render all duplicated graph, timing and migration claims from their inputs. */
export function appendix({ layout: l, graph: g, report: r }) {
  const cyclic = components(g.modules, g.edges).filter((group) => group.length > 1);
  const within = g.edges.filter((edge) =>
    cyclic.some((group) => group.includes(edge.from) && group.includes(edge.to)),
  );
  const lines = [
    MARKER,
    "",
    "## Replay output",
    "",
    "Rounded module-table weights; missing modules are not imputed. Today's and the profile split's columns are freshly replayed with the corrected borrowed inputs, rather than copied from the historical report.",
    "",
    "| Platform | Today: suites / seconds skipped | Corrected profile split: suites / seconds skipped | Proposed: suite runs skipped | Proposed: observed seconds skipped | Observed seconds/full run | Missing modules |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
  ];
  for (const [platform, value] of Object.entries(r.platforms)) {
    const s = value.summary;
    const c = r.comparisons[platform];
    lines.push(
      `| ${platform} | ${pct(c.today.skippedSuiteFraction)} / ${pct(c.today.testSecondsSkippedFraction)} | ${pct(c.profileSplit.skippedSuiteFraction)} / ${pct(c.profileSplit.testSecondsSkippedFraction)} | ${pct(s.skippedSuiteFraction)} (${s.skippedSuiteRuns}/${144 * s.suites}) | ${pct(s.testSecondsSkippedFraction)} (${s.testSecondsSkipped}) | ${s.testSecondsPerRun} | ${value.missingModules.length} |`,
    );
  }
  lines.push("", "Missing timing evidence by platform:", "");
  for (const [platform, value] of Object.entries(r.platforms))
    lines.push(`- ${platform}: ${value.missingModules.join(", ") || "none"}.`);
  lines.push(
    "",
    "## Source cycle",
    "",
    `${g.modules.length} root/file modules, ${Object.keys(g.files).length} Rust files, ${g.edges.length} discovered edges; ${within.length} edges inside the nontrivial component. Every internal edge of a strongly connected component closes some cycle; this lists all of them, not a minimum feedback set. Directory children remain individually inventoried below.`,
    "",
    ...cyclic.map((group) => group.map((name) => `\`${name}\``).join(", ")),
    "",
    "## Crate graph and exact file ownership",
    "",
    "The dependency column is the proposed graph; the checker refuses a cycle and names its path. Empty ownership means an adapter extraction described above.",
    "",
    "| Crate | Owned Rust files | Direct dependencies |",
    "| --- | --- | --- |",
  );
  for (const [name, crate] of Object.entries(l.crates))
    lines.push(
      `| ${name} | ${crate.files.join(", ") || "adapter from driver/verbs"} | ${crate.deps.join(", ") || "none"} |`,
    );
  lines.push(
    "",
    "## Every cycle-closing edge",
    "",
    "Classification: **move** = move command-only code to its command owner; **shared** = keep/extract the named concept; **seam** = depending concept owns an interface and composition supplies its adapter. Same-crate edges need no new Cargo dependency. Sites refer to the source evidence revision above.",
    "",
    "| Edge | Source sites | Class | Proposed fix |",
    "| --- | --- | --- | --- |",
  );
  for (const edge of within) {
    const fix = l.edges[`${edge.from}->${edge.to}`];
    lines.push(
      `| ${edge.from} → ${edge.to} | ${edge.sites.join(", ")} | ${fix.kind} | ${fix.fix} |`,
    );
  }
  lines.push(
    "",
    "## Suite inventory and code changes at the boundary",
    "",
    "Every row retains its complete test inventory. All require only the runner/import/helper ownership changes described above; assertions remain in their owning module.",
    "",
    "| Suite | Direct crate roots | Reason |",
    "| --- | --- | --- |",
  );
  for (const [name, suite] of Object.entries({ ...l.suites, ...l.auxiliarySuites }))
    lines.push(`| ${name} | ${suite.crates.join(", ")} | ${suite.reason} |`);
  lines.push(
    "",
    "## Stepwise conservative replay",
    "",
    "Apply the preservation procedure above at every row. The packaging recommendation is for user review, with no release target change in this task.",
    "",
    "| Step | Extract | Suite runs skipped once landed |",
    "| ---: | --- | ---: |",
  );
  for (const [index, stage] of r.migration.entries())
    lines.push(`| ${index + 1} | ${stage.crate} | ${pct(stage.skippedSuiteFraction)} |`);
  return `${lines.join("\n")}\n`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length > 3 || (process.argv[2] && process.argv[2] !== "--write"))
      throw new Error("usage: node scripts/modularization/render.mjs [--write]");
    const file = "docs/modularization-design.md";
    const current = readFileSync(file, "utf8");
    if (current.split(MARKER).length !== 2)
      throw new Error("expected exactly one generation marker in design document");
    const expected = `${current.split(MARKER)[0].trimEnd()}\n\n${appendix(evidence())}`;
    if (process.argv[2] === "--write") writeFileSync(file, expected);
    else if (current !== expected) throw new Error("design tables are stale");
    console.log(
      `modularization design: ${process.argv[2] === "--write" ? "updated" : "verified"} ${file}`,
    );
  } catch (error) {
    console.error(
      `${error.message}\nnext: run node scripts/modularization/render.mjs --write, review the generated diff, then rerun without --write`,
    );
    process.exitCode = 1;
  }
}
