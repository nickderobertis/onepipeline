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
import { commandMapping, lockPackages, run } from "./command-replay.mjs";
import { SECTIONED_DOCS, scopeChanged } from "./contract-sections.mjs";
import { SPLIT_PATH, checkSplit, deriveSplit, targetCatalog } from "./test-split.mjs";
import { affectedSuites, compileMapping, mergedChanges } from "../crate-split-replay.mjs";
import { testFakes } from "./test-fakes.mjs";

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

const WALL_PATH = "scripts/modularization/ci-wall.json";
const duration = (value) => typeof value === "number" && Number.isFinite(value) && value >= 0;
const record = (value, keys, where) => {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error(`invalid ${WALL_PATH}: ${where} is not an object`);
  const unknown = Object.keys(value).find((key) => !keys.includes(key));
  if (unknown !== undefined)
    throw new Error(`invalid ${WALL_PATH}: ${where} has unknown field ${unknown}`);
};

/** The measured job inputs, refused unless every duration is a finite nonnegative number in place. */
export function readWall(text) {
  const wall = JSON.parse(text);
  record(wall, ["schema", "source", "limitSeconds", "platforms", "local"], "the file");
  if (wall.schema !== 1) throw new Error(`invalid ${WALL_PATH}: schema must be 1`);
  if (typeof wall.source !== "string" || !wall.source.trim())
    throw new Error(`invalid ${WALL_PATH}: source must name where the measurements come from`);
  if (!duration(wall.limitSeconds) || wall.limitSeconds === 0)
    throw new Error(`invalid ${WALL_PATH}: limitSeconds must be a positive number`);
  record(wall.platforms, ["linux", "macos", "windows"], "platforms");
  for (const platform of ["linux", "macos", "windows"]) {
    const data = wall.platforms[platform];
    record(data, ["legs", "compileStressSeconds", "breakdown"], platform);
    if (!duration(data.compileStressSeconds))
      throw new Error(`invalid ${WALL_PATH}: ${platform}.compileStressSeconds is not a duration`);
    if (!Array.isArray(data.legs) || !data.legs.length)
      throw new Error(`invalid ${WALL_PATH}: ${platform}.legs must list at least one job`);
    for (const [index, leg] of data.legs.entries()) {
      const where = `${platform}.legs[${index}]`;
      record(
        leg,
        ["job", "name", "jobSeconds", "bootstrapSeconds", "e2eSeconds", "e2eTests"],
        where,
      );
      if (
        !Number.isInteger(leg.job) ||
        typeof leg.name !== "string" ||
        !Number.isInteger(leg.e2eTests)
      )
        throw new Error(`invalid ${WALL_PATH}: ${where} needs a job id, name and e2e test count`);
      for (const key of ["jobSeconds", "bootstrapSeconds", "e2eSeconds"])
        if (!duration(leg[key]))
          throw new Error(`invalid ${WALL_PATH}: ${where}.${key} is not a duration`);
      if (leg.e2eSeconds + leg.bootstrapSeconds > leg.jobSeconds)
        throw new Error(
          `invalid ${WALL_PATH}: ${where} spends more on bootstrap and e2e than the job took`,
        );
    }
    const breakdown = data.breakdown;
    record(
      breakdown,
      ["how", "segments", "legs", "overlapping", "unreported"],
      `${platform}.breakdown`,
    );
    const legs = breakdown.legs ?? [{ segments: breakdown.segments }];
    if (legs.length !== data.legs.length)
      throw new Error(`invalid ${WALL_PATH}: ${platform}.breakdown must break down every leg`);
    for (const [index, leg] of legs.entries()) {
      if (!Array.isArray(leg.segments) || !leg.segments.length)
        throw new Error(
          `invalid ${WALL_PATH}: ${platform}.breakdown leg ${index + 1} has no segments`,
        );
      for (const segment of [...leg.segments, ...(breakdown.overlapping ?? [])])
        if (typeof segment?.part !== "string" || !duration(segment.seconds))
          throw new Error(
            `invalid ${WALL_PATH}: ${platform}.breakdown has a segment without a part and duration`,
          );
      const total = leg.segments.reduce((sum, segment) => sum + segment.seconds, 0);
      if (Math.abs(total - data.legs[index].jobSeconds) > 5)
        throw new Error(
          `invalid ${WALL_PATH}: ${platform}.breakdown leg ${index + 1} does not add up to its job`,
        );
    }
  }
  record(wall.local, ["source", "testPhaseSeconds"], "local");
  if (!duration(wall.local.testPhaseSeconds) || typeof wall.local.source !== "string")
    throw new Error(`invalid ${WALL_PATH}: local needs a source and a test-phase duration`);
  return wall;
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

/** Packages of the sibling engines this repository links, as the lockfile names them. */
export const SIBLING = /^one(?!pipeline)[a-z]/;
/** This workspace's own packages, whose lock entries move with its version, not with a dependency. */
const WORKSPACE = /^onepipeline(?:-|$)/;

/**
 * What kind of input a PR's effective path is, and the design change that would remove it. A test
 * file's path is a genuine helper or test change unless that file's hunks fell outside every item.
 */
export function classify(path, row) {
  const whole = (file) =>
    (row?.sourceProjection ?? []).some((entry) => entry.file === file && entry.retainedWholeFile);
  if (path.startsWith("resolved/") && path.endsWith("/lock.json"))
    return ["lockfile resolution", "no: a crate's resolved dependencies changed"];
  if (path.startsWith("resolved/") && path.endsWith("/manifest.json"))
    return ["manifest requirement", "no: a crate's dependency requirement changed"];
  if (path.startsWith("resolved/")) return ["toolchain", "no: the compiler changed"];
  if (path.startsWith("migrated-tests/") && path.includes("/config/"))
    return ["test configuration section", "no: the section every runner reads changed"];
  if (path.startsWith("migrated-tests/")) {
    const file = path.split("/").slice(2).join("/");
    if (file === "tests/e2e/harness.rs")
      return whole(file)
        ? [
            "harness item outside one helper",
            "by a further split: harness.rs into per-concern files, so World's core, its Drop and its imports stop sharing a file with the store and hook helpers (helpers move; no assertion changes)",
          ]
        : ["harness helper", "no: a helper its users call changed"];
    return whole(file)
      ? [
          "test file outside one test",
          "by a further split: each part's tests in their own file, so a module's imports and added tests reach only that part (tests move; no assertion changes)",
        ]
      : ["test code", "no: the part's own tests changed"];
  }
  if (path.startsWith("src/"))
    return [
      "engine source outside any attributed item",
      "by a further split: the file physically divided along the subsystem boundaries, so a hunk outside every item lands in one crate's file",
    ];
  if (path.startsWith("moved/")) return ["engine source", "no: shared engine code changed"];
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

/** Kinds a further design change could free; each is measured as if it selected nothing. */
export const REMOVABLE = [
  "harness item outside one helper",
  "test file outside one test",
  "engine source outside any attributed item",
];

/**
 * For the PRs still selecting at least 90% of Linux seconds: the input kinds that alone select
 * that much, how many lockfile changes bump a sibling engine versus a third-party package, and the
 * skip shares an upper bound on each further split would reach.
 */
export function causeAnalysis(result, lockChanges) {
  const compiled = compileMapping(result.mapping.selection);
  const linux = result.platforms.linux;
  const share = (affected) =>
    affected.reduce((sum, suite) => sum + linux.suiteSeconds[suite], 0) / linux.secondsPerRun;
  const broad = result.rows.filter((row) => share(row.affected) >= 0.9);
  const prs = {};
  const sole = {};
  const lock = { sibling: 0, thirdParty: 0, both: 0 };
  for (const row of broad) {
    const byKind = {};
    for (const file of row.effectiveFiles) {
      const kind = classify(file, row)[0];
      byKind[kind] ??= [];
      byKind[kind].push(file);
    }
    const forcing = Object.keys(byKind).filter(
      (kind) => share(affectedSuites(compiled, byKind[kind]).affected) >= 0.9,
    );
    const named = forcing.length ? forcing : ["several inputs together"];
    for (const kind of named) prs[kind] = (prs[kind] ?? 0) + 1;
    if (named.length === 1) sole[named[0]] = (sole[named[0]] ?? 0) + 1;
    if (forcing.includes("lockfile resolution")) {
      const changed = lockChanges[row.commit] ?? [];
      const sibling = changed.some((name) => SIBLING.test(name));
      const third = changed.some((name) => !SIBLING.test(name));
      lock[sibling && third ? "both" : sibling ? "sibling" : "thirdParty"]++;
    }
  }
  const counterfactual = (kinds) => {
    const affected = result.rows.map(
      (row) =>
        affectedSuites(
          compiled,
          row.effectiveFiles.filter((file) => !kinds.includes(classify(file, row)[0])),
        ).affected,
    );
    return {
      kinds,
      suiteSkip:
        affected.reduce((sum, list) => sum + result.suites - list.length, 0) /
        (result.suites * affected.length),
      secondsSkip: 1 - affected.reduce((sum, list) => sum + share(list), 0) / affected.length,
      broad: affected.filter((list) => share(list) >= 0.9).length,
    };
  };
  return {
    broad: broad.length,
    prs,
    sole,
    lock,
    // Every forcing kind as if it selected nothing: an upper bound for the kinds a further split
    // could free, and only a measure of weight for the kinds no design change removes.
    freed: [
      ...Object.keys(prs)
        .filter((kind) => kind !== "several inputs together")
        .sort()
        .map((kind) => counterfactual([kind])),
      counterfactual(REMOVABLE),
    ],
  };
}

/** The kind of crate a resolved path belongs to, for following a sibling bump to the suites. */
export function crateRole(name, fakes) {
  if (name.startsWith("runner-")) return "the suite's own test code";
  if (fakes.has(name)) return "the shared test-fake crates";
  if (name === "cli-dispatcher" || name === "onepipeline")
    return "the dispatcher or facade every e2e suite runs";
  if (name.startsWith("cmd-")) return "a command crate";
  return "a shared engine crate";
}

/**
 * For the broad PRs whose lockfile change moved only sibling-engine packages: which crates' resolved
 * sets changed, how each reaches the moved engine (naming it, naming another sibling that depends
 * on it, or through a third-party package), and the Linux suite-seconds each crate role selects.
 * The adapter projection keeps sibling resolution only where a command crate, a suite's test code,
 * a test fake or the dispatcher names an engine, as if shared engine crates held their own value
 * types and reached the engines only behind adapters the command crates wire: an upper bound.
 */
export function siblingPaths(result, lockChanges, fakes) {
  const compiled = compileMapping(result.mapping.selection);
  const linux = result.platforms.linux;
  const seconds = (suites) => suites.reduce((sum, suite) => sum + linux.suiteSeconds[suite], 0);
  const share = (suites) => seconds(suites) / linux.secondsPerRun;
  const siblingOnly = (row) => {
    const moved = lockChanges[row.commit];
    return moved?.length && moved.every((name) => SIBLING.test(name)) ? moved : null;
  };
  const resolvedOf = (row) =>
    row.effectiveFiles.filter(
      (file) => file.startsWith("resolved/") && file.endsWith("/lock.json"),
    );
  const prs = [];
  const roles = {};
  const crates = {};
  for (const row of result.rows) {
    const moved = siblingOnly(row);
    if (!moved || share(row.affected) < 0.9) continue;
    const lockFiles = resolvedOf(row);
    if (share(affectedSuites(compiled, lockFiles).affected) < 0.9) continue;
    const byRole = {};
    for (const file of lockFiles) {
      const crate = file.split("/")[1];
      const role = crateRole(crate, fakes);
      byRole[role] ??= [];
      byRole[role].push(file);
      const roots = (result.mapping.crates[crate]?.external ?? []).filter((root) =>
        SIBLING.test(root),
      );
      const reach = roots.some((root) => moved.includes(root))
        ? "names the moved engine"
        : roots.length
          ? "names a sibling engine that depends on the moved one"
          : "through a third-party package that depends on it";
      crates[crate] ??= { role, reach: new Set(), prs: 0, roots };
      crates[crate].reach.add(reach);
      crates[crate].prs++;
    }
    const selectedBy = Object.fromEntries(
      Object.entries(byRole).map(([role, files]) => [
        role,
        affectedSuites(compiled, files).affected,
      ]),
    );
    for (const [role, suites] of Object.entries(selectedBy)) {
      const others = new Set(
        Object.entries(selectedBy)
          .filter(([other]) => other !== role)
          .flatMap(([, list]) => list),
      );
      roles[role] ??= { prs: 0, seconds: 0, only: 0 };
      roles[role].prs++;
      roles[role].seconds += seconds(suites);
      roles[role].only += seconds(suites.filter((suite) => !others.has(suite)));
    }
    prs.push({
      pr: /#(\d+)\)/.exec(row.subject)?.[1] ?? null,
      commit: row.commit,
      moved,
      crates: lockFiles.length,
      byRole: Object.fromEntries(
        Object.entries(byRole).map(([role, files]) => [role, files.length]),
      ),
      lockShare: share(affectedSuites(compiled, lockFiles).affected),
    });
  }
  const confined = (row) =>
    siblingOnly(row)
      ? row.effectiveFiles.filter(
          (file) =>
            !(
              file.startsWith("resolved/") &&
              /\/(lock|manifest)\.json$/.test(file) &&
              crateRole(file.split("/")[1], fakes) === "a shared engine crate"
            ),
        )
      : row.effectiveFiles;
  const affected = result.rows.map((row) => affectedSuites(compiled, confined(row)).affected);
  return {
    prs,
    roles,
    crates: Object.fromEntries(
      Object.entries(crates)
        .sort((a, b) => b[1].prs - a[1].prs || (a[0] < b[0] ? -1 : 1))
        .map(([name, entry]) => [name, { ...entry, reach: [...entry.reach].sort() }]),
    ),
    adapters: {
      suiteSkip:
        affected.reduce((sum, list) => sum + result.suites - list.length, 0) /
        (result.suites * affected.length),
      secondsSkip: 1 - affected.reduce((sum, list) => sum + share(list), 0) / affected.length,
      broad: affected.filter((list) => share(list) >= 0.9).length,
      stillBroad: prs.filter(
        (entry) =>
          share(affected[result.rows.findIndex((row) => row.commit === entry.commit)]) >= 0.9,
      ).length,
    },
  };
}

/** The lockfile packages each PR changed, by name. */
function lockChanges(changes) {
  const out = {};
  for (const change of changes.filter((entry) => entry.files.includes("Cargo.lock"))) {
    const [before, after] = [`${change.commit}^`, change.commit].map(
      (revision) =>
        new Map(
          [...lockPackages(show(revision, "Cargo.lock")).values()].map((pkg) => [
            pkg.key,
            pkg.block,
          ]),
        ),
    );
    const names = new Set();
    for (const [key, block] of before) if (after.get(key) !== block) names.add(key.split(" ")[0]);
    for (const key of after.keys()) if (!before.has(key)) names.add(key.split(" ")[0]);
    out[change.commit] = [...names].filter((name) => !WORKSPACE.test(name)).sort();
  }
  return out;
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
  const wall = readWall(read(WALL_PATH));
  const locks = lockChanges(changes);
  return {
    split,
    result: {
      ...result,
      scopes: phaseMap.scopes,
      wall: wallProjection(selectedShares(result.rows, result.platforms), wall),
      wallBreakdown: Object.fromEntries(
        Object.entries(wall.platforms).map(([platform, data]) => [platform, data.breakdown]),
      ),
      splitMigration: splitMigration(result, split),
      causes: causeAnalysis(result, locks),
      siblings: siblingPaths(result, locks, new Set(Object.keys(testFakes().crates))),
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
