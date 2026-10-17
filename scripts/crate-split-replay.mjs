#!/usr/bin/env node
// Replay merged pull requests against a proposed crate split, and say which test
// suites Nx-affected selection would have run for each one.
//
//   node scripts/crate-split-replay.mjs --mapping FILE [--repo DIR] [--ref REF]
//        [--since DATE] [--until DATE] [--exclude REGEX] [--records DIR] [--run RUN_ID] [--json]
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
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { loadRecords, moduleOf, selectRun } from "./test-profile-report.mjs";

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

/** Check a mapping's shape and compile it. */
export function compileMapping(mapping) {
  const crates = mapping.crates ?? {};
  for (const [name, crate] of Object.entries(crates)) {
    for (const dep of crate.deps ?? []) {
      if (!(dep in crates)) throw new Error(`crate ${name} depends on unknown crate ${dep}`);
    }
  }
  if (mapping.fallback !== undefined && !(mapping.fallback in crates)) {
    throw new Error(`fallback ${mapping.fallback} is not a crate`);
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
      if (!(crate in crates)) throw new Error(`suite ${name} exercises unknown crate ${crate}`);
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
export function moduleSeconds(records) {
  const out = {};
  for (const record of records) {
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

function main(argv) {
  const options = {
    repo: ".",
    ref: "origin/main",
    since: "2026-09-15T00:00:00Z",
    until: "2026-10-16T00:00:00Z",
    exclude: "^chore: release",
  };
  let json = false;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const value = () => {
      if (i + 1 >= argv.length) throw new Error(`${arg} needs a value`);
      return argv[++i];
    };
    if (arg === "--json") json = true;
    else if (
      [
        "--mapping",
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
    } else throw new Error(`not an option: ${arg}`);
  }
  if (!options.mapping) throw new Error("--mapping names the proposed split");
  const mapping = JSON.parse(readFileSync(options.mapping, "utf8"));
  // Refused before any history is read: a proposal that does not hold together is
  // the mistake to report, whatever the repository says.
  compileMapping(mapping);
  const changes = mergedChanges(options.repo, options);
  const seconds = options.records
    ? moduleSeconds(selectRun(loadRecords(options.records), options.run).records)
    : null;
  const result = replay(mapping, changes, seconds);
  process.stdout.write(json ? `${JSON.stringify(result, null, 2)}\n` : `${render(result)}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`crate-split-replay: ${error.message}`);
    process.exitCode = 1;
  }
}
