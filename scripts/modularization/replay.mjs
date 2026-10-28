#!/usr/bin/env node
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { pathToFileURL } from "node:url";
import { mergedChanges, replay, todaysMapping } from "../crate-split-replay.mjs";
import { moduleGraph, validate } from "./graph.mjs";

/** Use existing source paths as a conservative attribution of historical edits. */
export function replayMapping(layout, extracted = new Set(Object.keys(layout.crates))) {
  const residual = Object.entries(layout.crates).filter(([name]) => !extracted.has(name));
  const crates = Object.fromEntries(
    Object.entries(layout.crates)
      .filter(([name]) => extracted.has(name))
      .map(([name, crate]) => [
        name,
        { paths: [...crate.files, ...(layout.assets?.[name] ?? [])], deps: crate.deps },
      ]),
  );
  if (residual.length)
    crates.residual = {
      paths: residual.flatMap(([name, crate]) => [
        ...crate.files,
        ...(layout.assets?.[name] ?? []),
      ]),
      deps: [...extracted],
    };
  crates.testfakes = { paths: ["crates/testfakes/**"], deps: [] };
  return {
    shared: [
      "Cargo.toml",
      "Cargo.lock",
      "rust-toolchain.toml",
      ".cargo/config.toml",
      ".config/nextest.toml",
      "nx.json",
      "package.json",
      "package-lock.json",
      "justfile",
      "schemas/**",
      "src/AGENTS.md",
    ],
    sources: ["src/**", "crates/**"],
    fallback: residual.length ? "residual" : "onepipeline",
    crates,
    suites: Object.fromEntries(
      Object.entries(layout.suites).map(([name, suite]) => [
        name,
        {
          paths: suite.paths,
          modules: suite.modules,
          crates: [
            ...new Set([
              ...suite.crates.map((crate) => (extracted.has(crate) ? crate : "residual")),
              "testfakes",
            ]),
          ],
        },
      ]),
    ),
  };
}

/** Reuse the report's all-module table, retaining missing values as missing. */
export function profileTable(text) {
  const section = text
    .split("### Every e2e module, on the current records")[1]
    ?.split("<details>")[0];
  if (!section) throw new Error("profile report has no all-module table");
  const platforms = Object.fromEntries(
    ["local", "linux", "macos", "windows"].map((name) => [name, new Map()]),
  );
  let rows = 0;
  const seen = new Set();
  for (const line of section.split("\n")) {
    if (!line.startsWith("|")) continue;
    const cells = line.split("|").map((cell) => cell.trim());
    if (cells[1] === "module" || /^-+$/.test(cells[1])) continue;
    if (!/^[a-z][a-z0-9_]*$/.test(cells[1]) || !/^\d+$/.test(cells[2] ?? "") || cells.length !== 12)
      throw new Error(`malformed timing row: ${line}`);
    if (seen.has(cells[1])) throw new Error(`duplicate timing module: ${cells[1]}`);
    rows++;
    seen.add(cells[1]);
    for (const [index, platform] of Object.keys(platforms).entries()) {
      const cell = cells[index + 3];
      if (cell === "–") continue;
      if (!/^\d+$/.test(cell ?? "") || !Number.isSafeInteger(Number(cell)))
        throw new Error(`invalid ${platform} seconds for ${cells[1]}: ${cell}`);
      platforms[platform].set(cells[1], Number(cell));
    }
  }
  if (!rows) throw new Error("profile table contains no timing rows");
  return platforms;
}

export function buildReport(layout, changes, weights) {
  const mapping = replayMapping(layout);
  const full = Object.fromEntries(
    Object.entries(weights).map(([platform, seconds]) => {
      const result = replay(mapping, changes, seconds);
      const missing = Object.values(layout.suites)
        .flatMap((suite) => suite.modules)
        .filter((module) => !seconds.has(module));
      return [platform, { summary: result.summary, missingModules: missing }];
    }),
  );
  const order = [];
  const extracted = new Set();
  while (extracted.size < Object.keys(layout.crates).length) {
    const next = Object.keys(layout.crates).find(
      (name) => !extracted.has(name) && layout.crates[name].deps.every((dep) => extracted.has(dep)),
    );
    if (!next) throw new Error("migration cannot advance: dependency cycle");
    extracted.add(next);
    const stage = replay(replayMapping(layout, extracted), changes);
    order.push({ crate: next, skippedSuiteFraction: stage.summary.skippedSuiteFraction });
  }
  return {
    schema: 1,
    evidenceRevision: layout.evidenceRevision,
    changes: changes.length,
    platforms: full,
    migration: order,
  };
}

/** Assemble historical evidence from the repository, refusing a different corpus. */
export function evidence(
  root = ".",
  layoutPath = join(root, "scripts/modularization/layout.json"),
) {
  const layout = JSON.parse(readFileSync(layoutPath, "utf8"));
  const graph = moduleGraph(root);
  validate(layout, graph);
  const changes = mergedChanges(root, {
    ref: "origin/main",
    since: "2026-09-15T00:00:00Z",
    until: "2026-10-16T00:00:00Z",
    exclude: "^chore: release",
  });
  if (changes.length !== 144) throw new Error(`expected 144 PRs, found ${changes.length}`);
  const weights = profileTable(readFileSync(join(root, "docs/test-profile-report.md"), "utf8"));
  const report = buildReport(layout, changes, weights);
  const old = JSON.parse(readFileSync(join(root, "scripts/crate-split/proposed.json"), "utf8"));
  report.comparisons = Object.fromEntries(
    Object.entries(weights).map(([platform, seconds]) => [
      platform,
      {
        today: replay(todaysMapping(root), changes, seconds).summary,
        profileSplit: replay(old, changes, seconds).summary,
      },
    ]),
  );
  return { layout, graph, report };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length !== 4)
      throw new Error("usage: node scripts/modularization/replay.mjs LAYOUT.json REPORT.json");
    const { report } = evidence(".", process.argv[2]);
    mkdirSync(dirname(process.argv[3]), { recursive: true });
    writeFileSync(process.argv[3], `${JSON.stringify(report, null, 2)}\n`);
    console.log(
      `modularization replay: ${report.changes} PRs; ${(100 * report.platforms.linux.summary.skippedSuiteFraction).toFixed(2)}% suite runs skipped; wrote ${process.argv[3]}`,
    );
  } catch (error) {
    console.error(
      `${error.message}\nnext: correct the named layout/timing input or restore the 144-PR origin/main history; rerun with LAYOUT.json REPORT.json`,
    );
    process.exitCode = 1;
  }
}
