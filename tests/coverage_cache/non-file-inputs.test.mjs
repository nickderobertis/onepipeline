// Every non-file input of a coverage tier's key: changing it re-runs the tier, and restoring it
// replays the tier again, as real Nx's run summary reports. A wrapper early on PATH changes only
// the version a tool reports, or the platform node reports; every other call reaches the real one.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { TIERS, nx, realTool, scratch, test_all, tierOutcome, wrapper } from "./support.mjs";

const sorted = [...TIERS].sort();
function assertReruns(run, input) {
  assert.equal(run.status, 0, `${input}: ${run.output}`);
  for (const tier of sorted) {
    assert.ok(
      tierOutcome(run).executed.includes(tier),
      `tier ${tier} replayed after ${input} changed`,
    );
  }
}

test("each non-file input moves every tier that names it, and restoring it replays them", {
  timeout: 1800000,
}, (t) => {
  const dir = scratch(t);
  const baseline = nx(dir, test_all);
  assert.equal(baseline.status, 0, baseline.output);
  assert.deepEqual(tierOutcome(nx(dir, test_all)).replayed, sorted);

  const realCargoTool = (name) => realTool(name, dir);
  const realCargo = execFileSync("bash", ["-c", "command -v cargo"], { encoding: "utf8" }).trim();
  const toolCases = [
    // Cargo parses `rustc -vV`, so the wrapper rewrites one field rather than the whole output.
    [
      "rustc's version",
      "rustc",
      '*" -vV "*',
      realCargoTool("rustc"),
      `"${realCargoTool("rustc")}" "$@" | sed 's/^LLVM version: /LLVM version: 99./'; exit 0`,
    ],
    // Cargo looks a subcommand up in CARGO_HOME before PATH, so these wrap cargo itself.
    ["cargo-llvm-cov's version", "cargo", '*" llvm-cov --version "*', realCargo],
    ["nextest's version", "cargo", '*" nextest --version "*', realCargo],
    ["llvm-profdata's version", "llvm-profdata", '*" --version "*', realTool("llvm-profdata", dir)],
    ["llvm-cov's version", "llvm-cov", '*" --version "*', realTool("llvm-cov", dir)],
    ["llvm-readobj's version", "llvm-readobj", '*" --version "*', realTool("llvm-readobj", dir)],
  ];
  const nodeBin = process.execPath;
  const cases = [
    ...["COVERAGE_UNIT_IGNORE_REGEX", "NEXTEST_PROFILE"].map((key) => [
      `environment variable ${key}`,
      { env: { [key]: key === "NEXTEST_PROFILE" ? "default" : "no-such-file-anywhere" } },
    ]),
    ...toolCases.map(([name, tool, pattern, real, rewrite], index) => [
      name,
      () => ({ path: [wrapper(dir, `case-${index}`, tool, pattern, real, rewrite)] }),
    ]),
    [
      "the platform node reports",
      () => ({ path: [wrapper(dir, "platform", "node", '*" -p process.platform "*', nodeBin)] }),
    ],
  ];
  for (const [input, setUp] of cases) {
    const changed = nx(dir, test_all, typeof setUp === "function" ? setUp() : setUp);
    assertReruns(changed, input);
    const restored = nx(dir, test_all);
    assert.deepEqual(
      tierOutcome(restored).replayed,
      sorted,
      `${input}: restored, every tier must replay`,
    );
  }
});

test("an input taken out of a tier's key is what assertReruns catches", {
  timeout: 600000,
}, (t) => {
  const dir = scratch(t);
  const path = join(dir, "nx.json");
  const nxJson = JSON.parse(readFileSync(path, "utf8"));
  nxJson.namedInputs.coverageTierKey = nxJson.namedInputs.coverageTierKey.filter(
    (input) => input?.env !== "COVERAGE_UNIT_IGNORE_REGEX",
  );
  writeFileSync(path, `${JSON.stringify(nxJson, null, 2)}\n`);
  assert.equal(nx(dir, test_all).status, 0);
  const changed = nx(dir, test_all, {
    env: { COVERAGE_UNIT_IGNORE_REGEX: "no-such-file-anywhere" },
  });
  assert.throws(
    () => assertReruns(changed, "environment variable COVERAGE_UNIT_IGNORE_REGEX"),
    /tier onepipeline-[\w-]+ replayed after environment variable COVERAGE_UNIT_IGNORE_REGEX changed/,
  );
});
