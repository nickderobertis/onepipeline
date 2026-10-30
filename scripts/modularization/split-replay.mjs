#!/usr/bin/env node
// Replays the 144 PRs against the target crate layout and the test split together: each heavy suite
// runs as the parts test-split.json assigns, a test file's hunks reach only the parts whose tests or
// helpers they touch, and a sectioned document's change reaches only the reads whose section changed.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { sourceOwners, projectSourceChange, declarations } from "./source-projection.mjs";
import { targetGraph } from "./target-graph.mjs";
import { evidence, profileTable } from "./replay.mjs";
import { commandMapping, run } from "./command-replay.mjs";
import { SECTIONED_DOCS, scopeChanged } from "./contract-sections.mjs";
import { SPLIT_PATH, checkSplit, deriveSplit, targetCatalog } from "./test-split.mjs";
import { affectedSuites, compileMapping, mergedChanges } from "../crate-split-replay.mjs";

const read = (path) => readFileSync(path, "utf8");
const show = (revision, file) => {
  try {
    return execFileSync("git", ["show", `${revision}:${file}`], {
      encoding: "utf8",
      maxBuffer: 32 * 1024 * 1024,
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (error) {
    if (/does not exist|exists on disk, but not in/.test(String(error.stderr))) return "";
    throw error;
  }
};

/**
 * Types and constants a test file declares belong to the owned items that name them: a change to a
 * struct only the store helpers use selects the store helpers' users, not every user of the file.
 */
export function typeOwners(owners) {
  const files = Object.keys(owners).filter((file) => file.startsWith("tests/e2e/"));
  const decls = files.flatMap((file) =>
    declarations(read(file)).map((decl) => ({
      ...decl,
      file,
      words: new Set(decl.code.match(/\b\w+\b/g)),
    })),
  );
  const ownersOf = (decl) => [owners[decl.file].get(decl.name)].flat();
  for (let changed = true; changed; ) {
    changed = false;
    // Every word an owned item's code names, with the owners of the items naming it.
    const naming = new Map();
    for (const decl of decls.filter((entry) => owners[entry.file].has(entry.name)))
      for (const word of decl.words) {
        if (!naming.has(word)) naming.set(word, new Set());
        for (const owner of ownersOf(decl))
          for (const value of owner instanceof Set ? owner : [owner]) naming.get(word).add(value);
      }
    for (const decl of decls) {
      if (decl.kind === "fn" || owners[decl.file].has(decl.name) || !naming.has(decl.name))
        continue;
      owners[decl.file].set(decl.name, new Set(naming.get(decl.name)));
      changed = true;
    }
  }
  return owners;
}

const median = (values) => {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = sorted.length >> 1;
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
};
const quantile = (values, q) => [...values].sort((a, b) => a - b)[Math.ceil(q * values.length) - 1];

/** Each PR's share of a full run's estimated seconds that the selection still runs, per platform. */
function selectedShares(rows, platforms) {
  return Object.fromEntries(
    Object.entries(platforms).map(([platform, data]) => [
      platform,
      rows.map(
        (row) =>
          row.affected.reduce((sum, suite) => sum + (data.suiteSeconds[suite] ?? 0), 0) /
          data.secondsPerRun,
      ),
    ]),
  );
}

/**
 * Projected job wall clock: the measured job less its e2e tier is held fixed, and the e2e tier
 * scales with the share of estimated test-seconds the PR selects. Windows is one job here: its two
 * legs' e2e tiers add, and its residual is both legs' less the bootstrap only one job would repeat.
 */
export function wallProjection(shares, wall) {
  const out = {};
  for (const [platform, data] of Object.entries(wall.platforms)) {
    const e2e = data.legs.reduce((sum, leg) => sum + leg.e2eSeconds, 0);
    const residual =
      data.legs.reduce((sum, leg) => sum + leg.jobSeconds - leg.e2eSeconds, 0) -
      data.legs.slice(1).reduce((sum, leg) => sum + leg.bootstrapSeconds, 0);
    const walls = shares[platform].map((share) => residual + e2e * share);
    const summary = (values) => ({
      median: median(values),
      p90: quantile(values, 0.9),
      max: Math.max(...values),
      over: values.filter((value) => value > wall.limitSeconds).length,
    });
    out[platform] = {
      residual,
      e2e,
      today: residual + e2e,
      projected: summary(walls),
      withCompileStress: summary(walls.map((value) => value + data.compileStressSeconds)),
      typicalShare: median(shares[platform]),
      worstShare: Math.max(...shares[platform]),
    };
  }
  const local = shares.local.map((share) => wall.local.testPhaseSeconds * share);
  out.local = {
    testPhase: wall.local.testPhaseSeconds,
    projected: { median: median(local), p90: quantile(local, 0.9), max: Math.max(...local) },
  };
  return out;
}

/** The mapping with the named split parents merged back into one suite each. */
function merged(selection, platforms, parents, splitSuites) {
  const suites = {};
  const into = (name) => {
    const parent = splitSuites[name]?.parent;
    return parent && !parents.has(parent) ? parent : name;
  };
  for (const [name, suite] of Object.entries(selection.suites)) {
    const target = into(name);
    suites[target] ??= { paths: [], crates: [], modules: [] };
    suites[target].paths.push(...suite.paths);
    suites[target].crates.push(...suite.crates);
  }
  for (const suite of Object.values(suites)) {
    suite.paths = [...new Set(suite.paths)].sort();
    suite.crates = [...new Set(suite.crates)].sort();
  }
  const weights = Object.fromEntries(
    Object.entries(platforms).map(([platform, data]) => {
      const seconds = {};
      for (const [name, value] of Object.entries(data.suiteSeconds))
        seconds[into(name)] = (seconds[into(name)] ?? 0) + value;
      return [platform, { seconds, perRun: data.secondsPerRun }];
    }),
  );
  return { mapping: { ...selection, suites }, weights };
}

/** Skip shares once each test-split step has landed, in order: input projection, then each split. */
export function splitMigration(result, split) {
  const { selection } = result.mapping;
  const order = [
    ...new Set(
      Object.values(split.suites)
        .map((suite) => suite.parent)
        .sort(
          (a, b) =>
            Object.entries(result.platforms.linux.suiteSeconds)
              .filter(([name]) => split.suites[name]?.parent === b)
              .reduce((sum, [, value]) => sum + value, 0) -
            Object.entries(result.platforms.linux.suiteSeconds)
              .filter(([name]) => split.suites[name]?.parent === a)
              .reduce((sum, [, value]) => sum + value, 0),
        ),
    ),
  ];
  const steps = [];
  for (let landed = 0; landed <= order.length; landed++) {
    const parents = new Set(order.slice(0, landed));
    const { mapping, weights } = merged(selection, result.platforms, parents, split.suites);
    const compiled = compileMapping(mapping);
    const names = Object.keys(mapping.suites);
    const affected = result.rows.map(
      (row) => affectedSuites(compiled, row.effectiveFiles).affected,
    );
    steps.push({
      step: landed ? `split ${order[landed - 1]}` : "project test inputs per item and section",
      suites: names.length,
      suiteSkip:
        affected.reduce((sum, list) => sum + names.length - list.length, 0) /
        (names.length * affected.length),
      secondsSkip: Object.fromEntries(
        Object.entries(weights).map(([platform, data]) => [
          platform,
          1 -
            affected.reduce(
              (sum, list) => sum + list.reduce((s, name) => s + data.seconds[name], 0),
              0,
            ) /
              (data.perRun * affected.length),
        ]),
      ),
    });
  }
  return steps;
}

export function splitEvidence() {
  const { layout: base } = evidence();
  const moves = JSON.parse(read("scripts/modularization/target-moves.json"));
  const layout = targetGraph(base, moves);
  const catalog = targetCatalog();
  const splitText = read(SPLIT_PATH);
  const suiteOf = checkSplit(splitText, catalog);
  if (splitText !== `${JSON.stringify(deriveSplit(catalog), null, 2)}\n`)
    throw new Error(`${SPLIT_PATH} differs from the split its rule derives`);
  const split = JSON.parse(splitText);
  const options = { suiteOf, sections: true };
  const changes = mergedChanges(".", {
    ref: "origin/main",
    since: "2026-09-15T00:00:00Z",
    until: "2026-10-16T00:00:00Z",
    exclude: "^chore: release",
  });
  if (changes.length !== 144) throw new Error(`expected 144 PRs, got ${changes.length}`);
  const owners = sourceOwners(layout, moves, {
    ...catalog,
    tests: Object.fromEntries(
      Object.entries(catalog.tests).map(([id, test]) => [id, { ...test, suite: suiteOf.get(id) }]),
    ),
  });
  const phaseMap = commandMapping(layout, catalog, options);
  for (const [suite, helpers] of Object.entries(phaseMap.helperConsumers))
    for (const helper of helpers) {
      const [module, rawName] = helper.split("::");
      const name = rawName.replace(/^\$/, "");
      const file = `tests/e2e/${module}.rs`;
      owners[file] ??= new Map();
      const users = owners[file].get(name) ?? new Set();
      users.add(`suite:${suite}`);
      owners[file].set(name, users);
    }
  typeOwners(owners);
  const projector = (change) => {
    const projected = projectSourceChange(change, owners, layout, ".", { testItems: true });
    const files = [...projected.files];
    for (const doc of SECTIONED_DOCS.filter((path) => change.files.includes(path))) {
      const before = show(`${change.commit}^`, doc);
      const after = show(change.commit, doc);
      for (const [key, entry] of Object.entries(phaseMap.scopes))
        if (entry.path === doc && scopeChanged(before, after, entry.needles)) files.push(key);
    }
    return { ...projected, files };
  };
  const weights = profileTable(read("docs/test-profile-report.md"));
  const result = run(layout, catalog, weights, changes, projector, {
    ...options,
    crateMigration: false,
  });
  const wall = JSON.parse(read("scripts/modularization/ci-wall.json"));
  return {
    split,
    result: {
      ...result,
      scopes: phaseMap.scopes,
      wall: wallProjection(selectedShares(result.rows, result.platforms), wall),
      splitMigration: splitMigration(result, split),
    },
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length !== 3) throw new Error("usage: split-replay.mjs OUTPUT.json");
    const output = splitEvidence();
    writeFileSync(process.argv[2], `${JSON.stringify(output, null, 2)}\n`);
    console.log(
      `split replay: ${output.result.rows.length} PRs; ${output.result.suites} suites; ${(output.result.platforms.linux.suiteSkip * 100).toFixed(2)}% suite runs and ${(output.result.platforms.linux.secondsSkip * 100).toFixed(2)}% Linux seconds skipped; wrote ${process.argv[2]}`,
    );
  } catch (error) {
    const next =
      error.code && /^(?:EACCES|EPERM|ENOENT|EISDIR|ENOTDIR|EEXIST|ENOSPC|EROFS)$/.test(error.code)
        ? "check the named path's type, existence, permissions and disk space, and that the output parent is writable; rerun the same split-replay.mjs invocation"
        : "reconcile test-split.json, the target graph and the 144-PR history; rerun split-replay.mjs OUTPUT.json";
    console.error(`${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}
