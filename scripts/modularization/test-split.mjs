#!/usr/bin/env node
// Splits every e2e suite holding more than THRESHOLD of a full Linux run's estimated test-seconds
// by what its tests read. A suite splits first by the source module its tests come from: a test's
// body, the module's helpers, constants and fixtures are what it reads beside the shared harness.
// A module part still over the threshold is ordered by the helper set each test reads and cut at
// equal estimated seconds, so tests reading the same helpers stay together and a change to one
// test's own code selects only its part. Every test stays assigned to exactly one suite.
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { expandCatalog } from "./catalog.mjs";
import { phaseInputs } from "./phase-inputs.mjs";
import { commandPaths } from "./source-projection.mjs";
import { profileTable } from "./replay.mjs";

export const THRESHOLD = 0.03;
export const SPLIT_PATH = "scripts/modularization/test-split.json";

/** The catalog's modules the profile measured no Linux seconds for, sorted. */
export function unmeasuredModules(catalog) {
  const linux = profileTable(readFileSync("docs/test-profile-report.md", "utf8")).linux;
  return [...new Set(Object.values(catalog.tests).map((test) => test.original.split("::")[0]))]
    .filter((module) => !linux.has(module))
    .sort();
}

/**
 * Estimated Linux seconds per catalog test: its module's measured seconds, split evenly. A module
 * the profile did not time carries no weight, as in the replay, and `unmeasuredModules` names it;
 * a catalog with no measured module at all is refused.
 */
export function linuxSeconds(catalog) {
  const linux = profileTable(readFileSync("docs/test-profile-report.md", "utf8")).linux;
  const counts = {};
  for (const original of new Set(Object.values(catalog.tests).map((test) => test.original))) {
    const module = original.split("::")[0];
    counts[module] = (counts[module] ?? 0) + 1;
  }
  if (!Object.keys(counts).some((module) => linux.has(module)))
    throw new Error("the profile measured no Linux seconds for any catalog module");
  return Object.fromEntries(
    Object.entries(catalog.tests).map(([id, test]) => {
      const module = test.original.split("::")[0];
      return [id, linux.has(module) ? (linux.get(module) / counts[module]) * test.share : 0];
    }),
  );
}

/** The split the rule above derives from the catalog. */
export function deriveSplit(catalog, threshold = THRESHOLD) {
  const seconds = linuxSeconds(catalog);
  const total = Object.values(seconds).reduce((sum, value) => sum + value, 0);
  const commands = commandPaths(
    JSON.parse(readFileSync("scripts/modularization/cli-commands.json", "utf8")),
  );
  const weigh = (ids) => ids.reduce((sum, id) => sum + seconds[id], 0) / total;
  const bySuite = {};
  for (const [id, test] of Object.entries(catalog.tests)) {
    bySuite[test.suite] ??= [];
    bySuite[test.suite].push(id);
  }
  const suites = {};
  for (const [parent, ids] of Object.entries(bySuite)) {
    if (weigh(ids) <= threshold) continue;
    const byModule = {};
    for (const id of ids) {
      const module = catalog.tests[id].original.split("::")[0];
      byModule[module] ??= [];
      byModule[module].push(id);
    }
    for (const [module, members] of Object.entries(byModule)) {
      const share = weigh(members);
      if (share <= threshold) {
        suites[`${parent}--${module}`] = { parent, tests: members };
        continue;
      }
      const reads = (id) =>
        phaseInputs(
          catalog.tests[id],
          catalog.dispositions[catalog.tests[id].original],
          commands,
        ).helpers.join(" ");
      const ordered = members
        .map((id, position) => ({ id, position, reads: reads(id) }))
        .sort((a, b) => (a.reads < b.reads ? -1 : a.reads > b.reads ? 1 : a.position - b.position));
      const parts = Math.ceil(share / threshold);
      const target = (share * total) / parts;
      let part = 1;
      let filled = 0;
      for (const { id } of ordered) {
        if (filled >= target && part < parts) {
          part++;
          filled = 0;
        }
        const name = `${parent}--${module}--${part}`;
        suites[name] ??= { parent, tests: [] };
        suites[name].tests.push(id);
        filled += seconds[id];
      }
    }
  }
  return {
    schema: 1,
    threshold,
    unmeasured: unmeasuredModules(catalog),
    suites: Object.fromEntries(
      Object.keys(suites)
        .sort()
        .map((name) => [name, suites[name]]),
    ),
  };
}

/** The first key any object of a JSON text names twice, whatever the text's layout; JSON.parse keeps only the last. */
export function duplicateKey(text) {
  const stack = [];
  let expectKey = false;
  for (let at = 0; at < text.length; at++) {
    const c = text[at];
    if (c === '"') {
      let end = at + 1;
      while (text[end] !== '"') end += text[end] === "\\" ? 2 : 1;
      const top = stack.at(-1);
      if (top && expectKey) {
        const key = JSON.parse(text.slice(at, end + 1));
        if (top.has(key)) return key;
        top.add(key);
        expectKey = false;
      }
      at = end;
    } else if (c === "{") {
      stack.push(new Set());
      expectKey = true;
    } else if (c === "[") {
      stack.push(null);
    } else if (c === "}" || c === "]") {
      stack.pop();
    } else if (c === ",") {
      expectKey = Boolean(stack.at(-1));
    }
  }
  return undefined;
}

/**
 * The suite each catalog test runs in once the split applies. Refuses a split that drops a test of
 * a suite it splits, lists a test twice or under the wrong parent, or names a test the catalog lacks.
 */
export function checkSplit(text, catalog) {
  const split = JSON.parse(text);
  const repeated = duplicateKey(text);
  if (
    !split ||
    typeof split !== "object" ||
    Array.isArray(split) ||
    Object.keys(split).some(
      (key) => !["schema", "threshold", "unmeasured", "suites"].includes(key),
    ) ||
    !Array.isArray(split.unmeasured) ||
    split.unmeasured.some(
      (module) => typeof module !== "string" || !/^[a-z][a-z0-9_]*$/.test(module),
    ) ||
    split.schema !== 1 ||
    typeof split.threshold !== "number" ||
    !(split.threshold > 0 && split.threshold < 1) ||
    !split.suites ||
    typeof split.suites !== "object" ||
    Array.isArray(split.suites)
  )
    throw new Error(`invalid test split ${SPLIT_PATH}`);
  if (repeated !== undefined) throw new Error(`test split names ${repeated} twice`);
  const parents = new Set(Object.values(catalog.tests).map((test) => test.suite));
  const suiteOf = new Map();
  for (const [name, suite] of Object.entries(split.suites)) {
    if (
      !/^[A-Za-z0-9._-]+$/.test(name) ||
      !suite ||
      typeof suite !== "object" ||
      Object.keys(suite).some((key) => !["parent", "tests"].includes(key)) ||
      !parents.has(suite.parent) ||
      !name.startsWith(`${suite.parent}--`) ||
      !Array.isArray(suite.tests) ||
      !suite.tests.length
    )
      throw new Error(`invalid split suite ${name}`);
    for (const id of suite.tests) {
      if (typeof id !== "string")
        throw new Error(`split suite ${name} names invalid test id ${JSON.stringify(id)}`);
      if (!Object.hasOwn(catalog.tests, id))
        throw new Error(`split suite ${name} names unknown test ${id}`);
      if (catalog.tests[id].suite !== suite.parent)
        throw new Error(
          `split suite ${name} takes ${id} from ${catalog.tests[id].suite}, not ${suite.parent}`,
        );
      if (suiteOf.has(id)) throw new Error(`test ${id} is in both ${suiteOf.get(id)} and ${name}`);
      suiteOf.set(id, name);
    }
  }
  const split_ = new Set(Object.values(split.suites).map((suite) => suite.parent));
  for (const [id, test] of Object.entries(catalog.tests)) {
    if (split_.has(test.suite) && !suiteOf.has(id))
      throw new Error(`test ${id} of split suite ${test.suite} is in no split suite`);
    if (!suiteOf.has(id)) suiteOf.set(id, test.suite);
  }
  return suiteOf;
}

export const targetCatalog = () =>
  expandCatalog(
    JSON.parse(readFileSync("scripts/modularization/command-tests-target.json", "utf8")),
  );

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const mode = process.argv[2];
    if (process.argv.length !== 3 || !["--write", "--check"].includes(mode))
      throw new Error("usage: test-split.mjs --write | --check");
    const catalog = targetCatalog();
    const derived = `${JSON.stringify(deriveSplit(catalog), null, 2)}\n`;
    if (mode === "--write") writeFileSync(SPLIT_PATH, derived);
    const committed = readFileSync(SPLIT_PATH, "utf8");
    const suiteOf = checkSplit(committed, catalog);
    if (committed !== derived)
      throw new Error(`${SPLIT_PATH} differs from the split its rule derives`);
    console.log(
      `test split: ${suiteOf.size} tests in ${new Set(suiteOf.values()).size} suites; ${Object.keys(JSON.parse(committed).suites).length} split suites`,
    );
  } catch (error) {
    const next =
      error.code && /^(?:EACCES|EPERM|ENOENT|EISDIR|ENOTDIR|ENOSPC|EROFS)$/.test(error.code)
        ? "check the named path's type, existence, permissions and disk space; rerun the same test-split.mjs invocation"
        : "regenerate with test-split.mjs --write and review the split, or restore the named test to exactly one suite";
    console.error(`${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}
