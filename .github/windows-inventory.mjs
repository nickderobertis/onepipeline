// Write the inventory `.github/cross-shards.mjs` checks the Windows split against:
// every test one green Windows leg ran, by the tier that ran it, read off that
// leg's own log.
//
//   node .github/windows-inventory.mjs <job log> <run> <job> <commit>
//
// A tier is the `test-quick` target whose nextest run a test's status line falls
// under, and a run is accepted only when the tests it names are exactly as many
// as its `Starting` line said it would run and its `Summary` line says it ran,
// each once: a log cut short, or one this cannot read, is refused rather than
// written down as an inventory with tests missing.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

function fail(message) {
  console.error(`windows-inventory: ${message}`);
  process.exit(1);
}

const [log, run, job, commit] = process.argv.slice(2);
if (!log || !run || !job || !commit || process.argv.length !== 6)
  fail("usage: windows-inventory.mjs <job log> <run> <job> <commit>");

// The projects whose `test-quick` target runs a tier, and the tier each runs, read
// where they are declared: each project's `test-quick` command is `just <recipe>`,
// and that recipe runs one tier through `_tier-test-quick`.
function projectFiles(dir, depth) {
  const found = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name.startsWith(".") || ["node_modules", "target"].includes(entry.name)) continue;
    const path = join(dir, entry.name);
    if (entry.isFile() && entry.name === "project.json") found.push(path);
    else if (entry.isDirectory() && depth > 0) found.push(...projectFiles(path, depth - 1));
  }
  return found;
}
const justfile = readFileSync("justfile", "utf8");
const tierOf = {};
for (const file of projectFiles(".", 2)) {
  const project = JSON.parse(readFileSync(file, "utf8"));
  const command = project.targets?.["test-quick"]?.command;
  if (command === undefined) continue;
  const recipe = command.match(/^just (_[a-z0-9-]+-test-quick)$/)?.[1];
  if (!recipe)
    fail(`${project.name}:test-quick runs ${JSON.stringify(command)}, not one justfile recipe`);
  const body = justfile.match(new RegExp(`^${recipe}:(.*)$`, "m"))?.[1];
  const runs = [...(body ?? "").matchAll(/\(_tier-test-quick ([a-z0-9-]+)\)/g)].map((m) => m[1]);
  if (runs.length !== 1)
    fail(`${project.name}:test-quick runs ${recipe}, which runs ${runs.length} tiers, not one`);
  tierOf[project.name] = runs[0];
}
if (!Object.keys(tierOf).length) fail("no project here has a `test-quick` target");

const runs = [];
let current = null;
const status =
  /^(?:PASS|FAIL|LEAK|LEAK-FAIL|FLAKY \d+\/\d+|TRY \d+ PASS)\s+\[\s*[0-9.]+s\]\s+(?:\[[^\]]*\]\s+)?\(\s*\d+\/\s*\d+\)\s+(\S+)\s+(\S+)$/;
for (const raw of readFileSync(log, "utf8").split("\n")) {
  const line = raw
    // biome-ignore lint/suspicious/noControlCharactersInRegex: the log's colour escapes
    .replace(/\x1b\[[0-9;]*m/g, "")
    .replace(/^﻿?\S+Z /, "")
    .trim();
  const nx = line.match(/^> nx run ([a-z0-9-]+):test-quick$/);
  if (nx) {
    const tier = tierOf[nx[1]];
    if (!tier) fail(`${nx[1]}:test-quick runs no tier this inventory knows`);
    current = { tier, starting: null, summary: null, tests: new Set() };
    runs.push(current);
    continue;
  }
  if (!current) continue;
  const starting = line.match(/^Starting (\d+) tests? across/);
  if (starting) current.starting = Number(starting[1]);
  const summary = line.match(/^Summary \[\s*[0-9.]+s\] (\d+) tests? run: (\d+) passed/);
  if (summary) current.summary = [Number(summary[1]), Number(summary[2])];
  const ran = line.match(status);
  if (ran && current.summary === null) {
    const test = `${ran[1]}\t${ran[2]}`;
    if (current.tests.has(test)) fail(`${current.tier}: ${ran[1]} ${ran[2]} reported twice`);
    current.tests.add(test);
  }
}

const tiers = Object.values(tierOf);
const seen = new Set(runs.map((r) => r.tier));
if (runs.length !== tiers.length || seen.size !== tiers.length)
  fail(
    `the log holds ${runs.length} test-quick runs over ${[...seen].join(", ")}, not one of each tier`,
  );
const lines = [];
for (const r of runs) {
  if (r.starting === null || r.summary === null)
    fail(`${r.tier}: the run has no Starting or no Summary line, so it did not finish`);
  const [counted, passed] = r.summary;
  if (r.tests.size !== r.starting || counted !== r.starting || passed !== r.starting)
    fail(
      `${r.tier}: ${r.tests.size} tests named, ${r.starting} starting, ${counted} run and ${passed} passed`,
    );
  for (const test of r.tests) lines.push(`${r.tier}\t${test}`);
}
lines.sort();
console.log(`# Every test the Windows leg of run ${run} (job ${job}) ran, at ${commit}.`);
console.log("# Written by .github/windows-inventory.mjs from that job's log; checked by");
console.log("# .github/cross-shards.mjs. Columns: tier, binary, test.");
for (const line of lines) console.log(line);
