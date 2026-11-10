// The merge against what it merges: `onepipeline:test` run through real Nx, its report compared
// by this file's own LCOV reader, never by `coverage-merge.mjs` itself.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { appendFileSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import {
  TIERS,
  VERSION_TIERS,
  assertSameReport,
  fixtureEnvironment,
  git,
  lcovOf,
  mergeReport,
  nx,
  readLcov,
  scratch,
  sumReports,
  summarize,
  test_all,
  tierOutcome,
  unitsDir,
} from "./support.mjs";

const ALL_LCOV = (dir) => TIERS.map((tier) => lcovOf(dir, tier));
const sorted = [...TIERS].sort();
const editNote = (dir) => appendFileSync(join(dir, "tests/note/main.rs"), "\n// edited\n");

test("the merge is exact over every tier's report, replayed or re-run", {
  timeout: 1200000,
}, (t) => {
  // Enough uncovered lines to fall below the floor, so the failing run prints each file's lines.
  const dir = scratch(t, { uncovered: 200 });
  const cold = nx(dir, test_all);
  assert.notEqual(cold.status, 0, "this fixture is below the floor");
  assert.deepEqual(tierOutcome(cold).executed, sorted, "every tier executes on the cold run");
  const allExecuted = mergeReport(cold.output);
  assertSameReport(assert, allExecuted, sumReports(ALL_LCOV(dir)), "every tier executed");

  // cargo-llvm-cov's own combined report over the same profiles.
  const own = join(dir, "own.lcov");
  execFileSync(
    "cargo",
    ["llvm-cov", "report", "--failure-mode", "all", "--lcov", "--output-path", own],
    {
      cwd: dir,
      encoding: "utf8",
      env: fixtureEnvironment(),
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  const ownFiles = readLcov(readFileSync(own, "utf8").replaceAll(`SF:${dir}/`, "SF:"));
  const mergedFiles = new Map();
  for (const path of ALL_LCOV(dir)) readLcov(readFileSync(path, "utf8"), mergedFiles);
  const [ownSummary, mergedSummary] = [summarize(ownFiles), summarize(mergedFiles)];
  assert.equal(
    mergedSummary.covered,
    ownSummary.covered,
    "lines covered differ from cargo-llvm-cov's combined report",
  );
  assert.equal(
    mergedSummary.total,
    ownSummary.total,
    "lines total differ from cargo-llvm-cov's combined report",
  );
  assert.deepEqual(
    [...mergedSummary.uncovered].sort(),
    [...ownSummary.uncovered].sort(),
    "uncovered lines differ",
  );

  editNote(dir);
  const partial = nx(dir, test_all);
  assert.notEqual(partial.status, 0);
  const { executed, replayed } = tierOutcome(partial);
  assert.deepEqual(executed, ["onepipeline-note-journeys"]);
  assert.equal(replayed.length, TIERS.length - 1);
  assertSameReport(
    assert,
    mergeReport(partial.output),
    sumReports(ALL_LCOV(dir)),
    "every tier but one replayed",
  );
});

test("a version-only bump replays the non-version tiers and merges to the same line sum", {
  timeout: 600000,
}, (t) => {
  const dir = scratch(t, { uncovered: 200 });
  assert.notEqual(nx(dir, test_all).status, 0, "this fixture is below the floor");
  const version = readFileSync(join(dir, "Cargo.toml"), "utf8").match(/\nversion = "([^"]+)"/)[1];
  for (const file of ["Cargo.toml", "Cargo.lock"]) {
    writeFileSync(
      join(dir, file),
      readFileSync(join(dir, file), "utf8").replaceAll(
        `version = "${version}"`,
        'version = "42.0.0"',
      ),
    );
  }
  const bumped = nx(dir, test_all);
  assert.deepEqual(
    tierOutcome(bumped).replayed,
    TIERS.filter((tier) => !VERSION_TIERS.includes(tier)).sort(),
  );
  assertSameReport(
    assert,
    mergeReport(bumped.output),
    sumReports(ALL_LCOV(dir)),
    "after a version-only bump",
  );
});

test("onepipeline:test refuses a total under the floor and names the uncovered lines", {
  timeout: 900000,
}, (t) => {
  const dir = scratch(t);
  const passing = nx(dir, test_all);
  assert.equal(passing.status, 0, passing.output);
  const { covered } = mergeReport(passing.output);
  const library = readFileSync(join(dir, "src/lib.rs"), "utf8");
  // Uncovered lines that put the merged total at about 95.37%: between today's 95 and the 95.73 floor.
  // `never` carries three lines of its own besides its body.
  const extra = Math.max(1, Math.ceil(covered * (100 / 95.37 - 1)) - 3);
  writeFileSync(
    join(dir, "src/lib.rs"),
    library.replace(
      /pub fn never[\s\S]*$/,
      `pub fn never(mut v: u64) -> u64 {\n${"    v += 1;\n".repeat(extra)}    v\n}\n`,
    ),
  );
  const failing = nx(dir, test_all);
  assert.notEqual(failing.status, 0);
  const { percent, uncovered } = mergeReport(failing.output);
  assert.ok(percent > 95 && percent < 95.73, `expected between 95 and 95.73, got ${percent}`);
  assert.ok(uncovered.size > 0, failing.output);
  assert.match(failing.output, /below the floor of 95\.73/);
  const merged = spawnSync("just", ["_crate-coverage"], {
    cwd: dir,
    encoding: "utf8",
    env: fixtureEnvironment(),
  });
  assert.equal(merged.status, 1, merged.stdout + merged.stderr);
});

test("onepipeline:test refuses a coverage tier it does not count", { timeout: 300000 }, (t) => {
  const dir = scratch(t);
  mkdirSync(join(dir, "tests/uncounted"), { recursive: true });
  writeFileSync(
    join(dir, "tests/uncounted/project.json"),
    JSON.stringify({
      name: "onepipeline-uncounted",
      targets: {
        test: {
          command: "true",
          outputs: ["{workspaceRoot}/target/coverage-units/onepipeline-uncounted.lcov"],
        },
      },
    }),
  );
  git(dir, "add", "-A");
  const run = nx(dir, test_all);
  assert.notEqual(run.status, 0);
  assert.match(run.output, /onepipeline-uncounted/);
  const direct = spawnSync("node", ["scripts/coverage-tiers.mjs"], {
    cwd: dir,
    encoding: "utf8",
    env: fixtureEnvironment(),
  });
  assert.equal(direct.status, 2, direct.stderr);
  assert.match(direct.stderr, /does not depend on coverage tier\(s\): onepipeline-uncounted/);

  // A target whose declared report is not the unit its recipe writes is refused too.
  writeFileSync(
    join(dir, "tests/uncounted/project.json"),
    JSON.stringify({
      name: "onepipeline-uncounted",
      targets: {
        test: {
          command: "just _note-test",
          outputs: ["{workspaceRoot}/target/coverage-units/onepipeline-other.lcov"],
        },
      },
    }),
  );
  const mismatched = spawnSync("node", ["scripts/coverage-tiers.mjs"], {
    cwd: dir,
    encoding: "utf8",
    env: fixtureEnvironment(),
  });
  assert.equal(mismatched.status, 2, mismatched.stderr);
  assert.match(
    mismatched.stderr,
    /declares report onepipeline-other but its recipe _note-test writes unit onepipeline-note-journeys/,
  );
});

test("a stale report never reaches the total, and the merge refuses what no tier owns", {
  timeout: 900000,
}, (t) => {
  const dir = scratch(t, { uncovered: 200 });
  assert.notEqual(nx(dir, test_all).status, 0, "this fixture is below the floor");
  const cached = new Map(TIERS.map((tier) => [tier, readFileSync(lcovOf(dir, tier), "utf8")]));

  for (const tier of TIERS)
    writeFileSync(lcovOf(dir, tier), `SF:src/planted-${tier}.rs\nDA:1,1\nend_of_record\n`);
  editNote(dir);
  const mixed = nx(dir, test_all);
  const { replayed, executed } = tierOutcome(mixed);
  assert.deepEqual(executed, ["onepipeline-note-journeys"]);
  for (const tier of replayed)
    assert.equal(
      readFileSync(lcovOf(dir, tier), "utf8"),
      cached.get(tier),
      `${tier} kept planted content`,
    );
  assert.doesNotMatch(
    readFileSync(lcovOf(dir, "onepipeline-note-journeys"), "utf8"),
    /planted/,
    "the executed tier kept planted content",
  );
  assertSameReport(
    assert,
    mergeReport(mixed.output),
    sumReports(ALL_LCOV(dir)),
    "after stale reports were planted",
  );

  writeFileSync(
    join(unitsDir(dir), "nobody-owns-this.lcov"),
    "SF:src/x.rs\nDA:1,1\nend_of_record\n",
  );
  const stray = spawnSync("just", ["_crate-coverage"], {
    cwd: dir,
    encoding: "utf8",
    env: fixtureEnvironment(),
  });
  assert.equal(stray.status, 2, stray.stdout + stray.stderr);
  assert.match(stray.stderr, /nobody-owns-this\.lcov/);
  execFileSync("rm", [
    join(unitsDir(dir), "nobody-owns-this.lcov"),
    lcovOf(dir, "onepipeline-pooled-slot"),
  ]);
  const missing = spawnSync("just", ["_crate-coverage"], {
    cwd: dir,
    encoding: "utf8",
    env: fixtureEnvironment(),
  });
  assert.equal(missing.status, 2, missing.stdout + missing.stderr);
  assert.match(missing.stderr, /no report for unit\(s\) onepipeline-pooled-slot/);
  assert.ok(readdirSync(unitsDir(dir)).every((name) => name.endsWith(".lcov")));
});

test("a truncated profile in a tier's run leaves its export and the rest of its profiles counted", {
  timeout: 300000,
}, (t) => {
  const dir = scratch(t);
  // Eight bytes as far as a killed child's header got, written where the unit merges its profiles.
  appendFileSync(
    join(dir, "tests/note/main.rs"),
    `#[test]
fn plants_a_truncated_profile() {
    let pattern = std::env::var("LLVM_PROFILE_FILE").expect("instrumented");
    let dir = std::path::Path::new(&pattern).parent().expect("a directory");
    let unit = std::path::Path::new(&pattern).file_name().unwrap().to_string_lossy().split('.').next().unwrap().to_string();
    std::fs::write(dir.join(format!("{unit}.truncated_0.profraw")), [0x81, b'r', b'f', b'o', b'r', b'p', b'l', 0xff]).unwrap();
}
`,
  );
  const run = nx(dir, test_all);
  assert.equal(run.status, 0, run.output);
  const report = readLcov(readFileSync(lcovOf(dir, "onepipeline-note-journeys"), "utf8"));
  const lines = [...report.get("src/lib.rs").values()];
  assert.ok(
    lines.some((count) => count > 0),
    "the tier's other profiles were not counted",
  );
  assert.ok(
    readdirSync(join(dir, "target/llvm-cov-target")).some((name) =>
      name.endsWith(".truncated_0.profraw"),
    ),
    "the truncated profile was not planted",
  );
});
