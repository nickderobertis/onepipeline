#!/usr/bin/env node
// Replay merged pull requests against a proposed crate split, and say which test
// suites Nx-affected selection would have run for each one.
//
//   node scripts/crate-split-replay.mjs (--mapping FILE | --today) --out FILE [--repo DIR]
//        [--ref REF] [--since DATE] [--until DATE] [--exclude REGEX] [--records DIR]
//        [--run RUN_ID] [--json]
//
// --today replays today's shape instead of a proposal: one crate and the whole e2e
// binary as one suite, read from the repository's own `nx.json` (`rustBuild` and
// `e2eSource`) and `tests/e2e/main.rs`, so it cannot drift from what Nx does now.
//
// Each first-parent commit on REF (default `origin/main`) committed in [since, until)
// is one squash-merged pull request, except those whose subject matches --exclude
// (default: release-plz's `chore: release`). Its change is `git diff` against its first
// parent, read from the local clone, so nothing here asks GitHub.
//
// The mapping (JSON) is the proposal:
//
//   {
//     "shared":   [globs every crate is built from: the lock, the toolchain, ...],
//     "crates":   { "<crate>": { "paths": [globs], "deps": ["<crate>", ...] } },
//     "sources":  [globs every changed file under which must belong to a crate],
//     "fallback": "<crate>",
//     "suites":   { "<suite>": { "paths": [globs], "crates": ["<crate>", ...],
//                                "modules": ["<test module>", ...] } }
//   }
//
// A suite is affected — Nx would run it — when a changed file is one of its own
// `paths`, one of `shared`, or one of the `paths` of a crate it exercises or of any
// crate that crate depends on, transitively. A file under `sources` that no crate
// claims is a file the proposal forgot, so it is charged to `fallback` (and counted in
// the output) rather than silently affecting nothing.
//
// With --records (a directory of `target/test-profile` records), each suite is weighed
// by the test-seconds its `modules` spent in that run, so the report also says how
// many e2e test-seconds the window's pull requests would have skipped.
//
// The result (Markdown, or JSON with --json) is written to --out; what is printed is
// one line with the headline figures. It exits 0 once the result is written, and 1
// with the reason and a `next:` line on stderr when anything is refused or fails.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { loadRecords, moduleOf, Refusal, selectRun } from "./test-profile-report.mjs";

/** A glob over repository-relative paths: `**` crosses directories, `*` and `?` do not. */
export function globToRegExp(glob) {
  let out = "";
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === "*" && glob[i + 1] === "*") {
      i++;
      if (glob[i + 1] === "/") {
        i++;
        out += "(?:.*/)?";
      } else out += ".*";
    } else if (c === "*") out += "[^/]*";
    else if (c === "?") out += "[^/]";
    else out += c.replace(/[.+^${}()|[\]\\]/g, "\\$&");
  }
  return new RegExp(`^${out}$`);
}

function matcher(globs = []) {
  const compiled = globs.map(globToRegExp);
  return (file) => compiled.some((pattern) => pattern.test(file));
}

const isStrings = (value) =>
  Array.isArray(value) && value.every((item) => typeof item === "string" && item !== "");

/** Refuse a mapping key that is not one of `known`; `//` is a comment. */
function onlyKeys(object, known, where) {
  if (object === null || typeof object !== "object" || Array.isArray(object)) {
    throw new Refusal(`${where} is not an object`, MAPPING_HELP);
  }
  for (const key of Object.keys(object)) {
    if (key !== "//" && !known.includes(key)) {
      throw new Refusal(`${where} has a key this replay does not read: ${key}`, MAPPING_HELP);
    }
  }
}

function strings(object, key, where, required) {
  if (object[key] === undefined && !required) return;
  if (!isStrings(object[key])) {
    throw new Refusal(`${where}.${key} is not a list of nonempty strings`, MAPPING_HELP);
  }
}

const MAPPING_HELP =
  "write the mapping in the shape the head of scripts/crate-split-replay.mjs gives; scripts/crate-split/proposed.json is one";

/**
 * Refuse a crate or suite name that is not a plain one: a name an object already has
 * (`constructor`, `__proto__`) would read or replace what it inherits, rather than
 * name an entry.
 */
function named(name, what) {
  if (!/^[A-Za-z0-9._-]+$/.test(name) || name in Object.prototype) {
    throw new Refusal(`${JSON.stringify(name)} is not a ${what} name`, MAPPING_HELP);
  }
}

/** Check a mapping's shape and compile it. */
export function compileMapping(mapping) {
  onlyKeys(mapping, ["shared", "crates", "sources", "fallback", "suites"], "the mapping");
  strings(mapping, "shared", "the mapping", false);
  strings(mapping, "sources", "the mapping", false);
  for (const container of ["crates", "suites"]) {
    const value = mapping[container];
    if (
      value !== undefined &&
      (value === null || typeof value !== "object" || Array.isArray(value))
    ) {
      throw new Refusal(`${container} is not an object of named entries`, MAPPING_HELP);
    }
  }
  const crates = mapping.crates ?? {};
  if (Object.keys(crates).length === 0)
    throw new Refusal("the mapping names no crate", MAPPING_HELP);
  for (const [name, crate] of Object.entries(crates)) {
    named(name, "crate");
    onlyKeys(crate, ["paths", "deps"], `crates.${name}`);
    strings(crate, "paths", `crates.${name}`, true);
    strings(crate, "deps", `crates.${name}`, false);
    for (const dep of crate.deps ?? []) {
      if (!Object.hasOwn(crates, dep))
        throw new Refusal(`crate ${name} depends on unknown crate ${dep}`, MAPPING_HELP);
    }
  }
  if (
    mapping.fallback !== undefined &&
    (typeof mapping.fallback !== "string" || !Object.hasOwn(crates, mapping.fallback))
  ) {
    throw new Refusal(`fallback ${mapping.fallback} is not a crate`, MAPPING_HELP);
  }
  const declared = mapping.suites ?? {};
  if (Object.keys(declared).length === 0)
    throw new Refusal("the mapping names no suite", MAPPING_HELP);
  for (const [name, suite] of Object.entries(declared)) {
    named(name, "suite");
    onlyKeys(suite, ["paths", "crates", "modules"], `suites.${name}`);
    strings(suite, "paths", `suites.${name}`, true);
    strings(suite, "crates", `suites.${name}`, true);
    strings(suite, "modules", `suites.${name}`, false);
  }
  const closure = (name, seen = new Set()) => {
    if (seen.has(name)) return seen;
    seen.add(name);
    for (const dep of crates[name].deps ?? []) closure(dep, seen);
    return seen;
  };
  const suites = {};
  for (const [name, suite] of Object.entries(mapping.suites ?? {})) {
    const reach = new Set();
    for (const crate of suite.crates ?? []) {
      if (!Object.hasOwn(crates, crate)) {
        throw new Refusal(`suite ${name} exercises unknown crate ${crate}`, MAPPING_HELP);
      }
      for (const reached of closure(crate)) reach.add(reached);
    }
    suites[name] = { own: matcher(suite.paths), crates: reach, modules: suite.modules ?? [] };
  }
  return {
    shared: matcher(mapping.shared),
    sources: matcher(mapping.sources),
    fallback: mapping.fallback,
    crates: Object.fromEntries(
      Object.entries(crates).map(([name, crate]) => [name, matcher(crate.paths)]),
    ),
    suites,
  };
}

/** Which crates a set of changed files touches, and which source files none claimed. */
export function touchedCrates(compiled, files) {
  const touched = new Set();
  const unclaimed = [];
  for (const file of files) {
    let claimed = false;
    for (const [name, claims] of Object.entries(compiled.crates)) {
      if (claims(file)) {
        touched.add(name);
        claimed = true;
      }
    }
    if (!claimed && compiled.sources(file)) {
      unclaimed.push(file);
      if (compiled.fallback !== undefined) touched.add(compiled.fallback);
    }
  }
  return { touched, unclaimed };
}

/** The suites a set of changed files affects. */
export function affectedSuites(compiled, files) {
  const { touched, unclaimed } = touchedCrates(compiled, files);
  const shared = files.some(compiled.shared);
  const affected = [];
  for (const [name, suite] of Object.entries(compiled.suites)) {
    if (shared || files.some(suite.own) || [...suite.crates].some((crate) => touched.has(crate))) {
      affected.push(name);
    }
  }
  return { affected, unclaimed };
}

function git(repo, args) {
  return execFileSync("git", ["-C", repo, ...args], {
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
  });
}

/** The merged pull requests in the window, oldest first, each with the files it changed. */
export function mergedChanges(repo, { ref, since, until, exclude }) {
  const log = git(repo, [
    "log",
    "--first-parent",
    "--reverse",
    `--since=${since}`,
    `--until=${until}`,
    "--format=%H%x09%cI%x09%s",
    "--end-of-options",
    ref,
  ]);
  const skip = new RegExp(exclude);
  return log
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const [commit, committed, ...subject] = line.split("\t");
      return { commit, committed, subject: subject.join("\t") };
    })
    .filter((change) => !skip.test(change.subject))
    .map((change) => ({
      ...change,
      files: git(repo, ["diff", "--name-only", `${change.commit}^1`, change.commit])
        .split("\n")
        .filter(Boolean),
    }));
}

/** Test-seconds per test module in one run's records. */
export function moduleSeconds(records, binary = "e2e") {
  const out = {};
  // A suite here is a set of e2e modules, so another binary's module of the same
  // name is not one of them.
  for (const record of records.filter((held) => held.binary === binary)) {
    const name = moduleOf(record.test);
    out[name] = (out[name] ?? 0) + (record.wall_s ?? 0);
  }
  return out;
}

/** Replay every change against the mapping. */
export function replay(mapping, changes, seconds) {
  const compiled = compileMapping(mapping);
  const names = Object.keys(compiled.suites);
  const weight = {};
  for (const name of names) {
    weight[name] = seconds
      ? compiled.suites[name].modules.reduce((sum, module) => sum + (seconds[module] ?? 0), 0)
      : null;
  }
  const perChange = changes.map((change) => {
    const { affected, unclaimed } = affectedSuites(compiled, change.files);
    return { commit: change.commit, subject: change.subject, affected, unclaimed };
  });
  const suites = {};
  for (const name of names) {
    const ran = perChange.filter((change) => change.affected.includes(name)).length;
    suites[name] = {
      affected: ran,
      unaffected: changes.length - ran,
      unaffectedFraction: changes.length ? (changes.length - ran) / changes.length : null,
      testSeconds: weight[name],
    };
  }
  const total = names.length * changes.length;
  const skipped = perChange.reduce((sum, change) => sum + names.length - change.affected.length, 0);
  const summary = {
    changes: changes.length,
    suites: names.length,
    skippedSuiteRuns: skipped,
    skippedSuiteFraction: total ? skipped / total : null,
    unclaimedFiles: [...new Set(perChange.flatMap((change) => change.unclaimed))].sort(),
  };
  if (seconds) {
    const perRun = names.reduce((sum, name) => sum + weight[name], 0);
    const saved = perChange.reduce(
      (sum, change) =>
        sum +
        names
          .filter((name) => !change.affected.includes(name))
          .reduce((inner, name) => inner + weight[name], 0),
      0,
    );
    const unmapped = Object.entries(seconds)
      .filter(([module]) => !names.some((name) => compiled.suites[name].modules.includes(module)))
      .map(([module]) => module)
      .sort();
    Object.assign(summary, {
      testSecondsPerRun: perRun,
      testSecondsSkipped: saved,
      testSecondsSkippedFraction:
        perRun > 0 && changes.length ? saved / (perRun * changes.length) : null,
      modulesInNoSuite: unmapped,
    });
  }
  return { summary, suites, perChange };
}

function render(result) {
  const { summary, suites } = result;
  const pct = (x) => (x === null ? "-" : `${(100 * x).toFixed(1)}%`);
  const lines = [
    `${summary.changes} merged changes, ${summary.suites} suites: ${summary.skippedSuiteRuns} of ${summary.changes * summary.suites} suite runs skipped (${pct(summary.skippedSuiteFraction)}).`,
  ];
  if (summary.testSecondsPerRun !== undefined) {
    lines.push(
      `Test-seconds: ${summary.testSecondsPerRun.toFixed(1)} per full run; ${summary.testSecondsSkipped.toFixed(1)} skipped over the window (${pct(summary.testSecondsSkippedFraction)}).`,
    );
    if (summary.modulesInNoSuite.length) {
      lines.push(`Modules in no suite: ${summary.modulesInNoSuite.join(", ")}.`);
    }
  }
  if (summary.unclaimedFiles.length) {
    lines.push(`Source files no crate claims: ${summary.unclaimedFiles.join(", ")}.`);
  }
  lines.push(
    "",
    "| suite | changes it runs on | unaffected | test-s per run |",
    "| --- | ---: | ---: | ---: |",
  );
  for (const [name, suite] of Object.entries(suites)) {
    lines.push(
      `| ${name} | ${suite.affected} | ${pct(suite.unaffectedFraction)} | ${suite.testSeconds === null ? "-" : suite.testSeconds.toFixed(1)} |`,
    );
  }
  return lines.join("\n");
}

/** Today's shape, read from `nx.json` and `tests/e2e/main.rs` in `repo`. */
export function todaysMapping(repo) {
  const refuse = (why) =>
    new Refusal(
      `nx.json cannot give today's shape: ${why}`,
      "run --today from this repository's root, or replay a --mapping",
    );
  let named;
  try {
    named = JSON.parse(readFileSync(join(repo, "nx.json"), "utf8")).namedInputs;
  } catch (error) {
    throw refuse(error.message);
  }
  if (named === null || typeof named !== "object") throw refuse("it has no namedInputs");
  const expand = (name, seen = []) => {
    if (seen.includes(name)) throw refuse(`${[...seen, name].join(" -> ")} is a cycle`);
    const inputs = named[name];
    if (!Object.hasOwn(named, name) || !Array.isArray(inputs)) {
      throw refuse(`${name} is not a list of inputs`);
    }
    // An object input (`dependentTasksOutputFiles` and the like) names no source file.
    return inputs
      .filter((input) => typeof input === "string")
      .flatMap((input) =>
        Object.hasOwn(named, input)
          ? expand(input, [...seen, name])
          : [input.replace("{workspaceRoot}/", "")],
      );
  };
  const build = expand("rustBuild");
  const sources = build.filter((glob) => /^(src|crates|schemas)\//.test(glob));
  if (sources.length === 0)
    throw refuse("rustBuild names no source under src/, crates/ or schemas/");
  const e2e = expand("e2eSource").filter((glob) => !build.includes(glob));
  let main;
  try {
    main = readFileSync(join(repo, "tests", "e2e", "main.rs"), "utf8");
  } catch (error) {
    throw refuse(`tests/e2e/main.rs cannot be read: ${error.message}`);
  }
  const modules = [...main.matchAll(/^mod (\w+);/gm)]
    .map((m) => m[1])
    .filter((m) => m !== "harness");
  return {
    shared: build.filter((glob) => !sources.includes(glob)),
    sources,
    fallback: "onepipeline",
    crates: { onepipeline: { paths: sources, deps: [] } },
    suites: { e2e: { paths: e2e, crates: ["onepipeline"], modules: [...modules, "harness"] } },
  };
}

/** Write `text` to `out`, refusing with what to fix when it cannot be. */
function writeOut(out, text) {
  try {
    writeFileSync(out, text);
  } catch (error) {
    throw new Refusal(
      `--out ${out} cannot be written: ${error.message}`,
      "point --out at a file in a directory that exists and that you can write to",
    );
  }
}

const USAGE =
  "node scripts/crate-split-replay.mjs --mapping scripts/crate-split/proposed.json --out replay.md";

function main(argv) {
  const options = {
    repo: ".",
    ref: "origin/main",
    since: "2026-09-15T00:00:00Z",
    until: "2026-10-16T00:00:00Z",
    exclude: "^chore: release",
  };
  let json = false;
  let today = false;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const value = () => {
      if (i + 1 >= argv.length) throw new Refusal(`${arg} needs a value`, USAGE);
      return argv[++i];
    };
    if (arg === "--json") json = true;
    else if (arg === "--today") today = true;
    else if (
      [
        "--mapping",
        "--out",
        "--repo",
        "--ref",
        "--since",
        "--until",
        "--exclude",
        "--records",
        "--run",
      ].includes(arg)
    ) {
      options[arg.slice(2)] = value();
    } else throw new Refusal(`${arg} is not an option`, USAGE);
  }
  if (!options.out) throw new Refusal("--out names no file for the result", USAGE);
  if (today === (options.mapping !== undefined)) {
    throw new Refusal("name the split to replay: --mapping FILE, or --today, and not both", USAGE);
  }
  const since = Date.parse(options.since);
  const until = Date.parse(options.until);
  if (Number.isNaN(since) || Number.isNaN(until) || since >= until) {
    throw new Refusal(
      `--since ${options.since} and --until ${options.until} are not a window`,
      "give two ISO dates, the first earlier, e.g. --since 2026-09-15T00:00:00Z --until 2026-10-16T00:00:00Z",
    );
  }
  if (options.ref.startsWith("-") || options.ref === "") {
    throw new Refusal(
      `--ref ${options.ref} is not a revision`,
      "name a branch or commit, e.g. --ref origin/main",
    );
  }
  try {
    new RegExp(options.exclude);
  } catch (error) {
    throw new Refusal(
      `--exclude is not a pattern: ${error.message}`,
      "give --exclude a regular expression such as '^chore: release'",
    );
  }
  let mapping;
  if (today) mapping = todaysMapping(options.repo);
  else {
    try {
      mapping = JSON.parse(readFileSync(options.mapping, "utf8"));
    } catch (error) {
      throw new Refusal(
        `${options.mapping} cannot be read as JSON: ${error.message}`,
        MAPPING_HELP,
      );
    }
  }
  // Refused before any history is read: a proposal that does not hold together is
  // the mistake to report, whatever the repository says.
  compileMapping(mapping);
  let changes;
  try {
    changes = mergedChanges(options.repo, options);
  } catch (error) {
    throw new Refusal(
      `the history of ${options.ref} in ${options.repo} cannot be read: ${error.message.split("\n")[0]}`,
      "run from a clone that has the ref, e.g. after git fetch origin",
    );
  }
  const seconds = options.records
    ? moduleSeconds(selectRun(loadRecords(options.records), options.run).records)
    : null;
  const result = replay(mapping, changes, seconds);
  writeOut(options.out, json ? `${JSON.stringify(result, null, 2)}\n` : `${render(result)}\n`);
  const { summary } = result;
  const pct = (x) => (x === null || x === undefined ? "-" : `${(100 * x).toFixed(1)}%`);
  console.log(
    `crate-split-replay: wrote ${options.out}: ${summary.changes} changes, ${summary.suites} suites, ` +
      `${pct(summary.skippedSuiteFraction)} of suite runs skipped` +
      (seconds ? `, ${pct(summary.testSecondsSkippedFraction)} of test-seconds` : ""),
  );
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`crate-split-replay: ${error.message}`);
    console.error(`next: ${error instanceof Refusal ? error.next : USAGE}`);
    process.exitCode = 1;
  }
}
