// The crate-split replay, run as its command over a real git repository with two
// crates and two suites, under two proposed mappings that differ only in whether one
// crate depends on the other.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { globToRegExp } from "../../scripts/crate-split-replay.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const script = join(root, "scripts", "crate-split-replay.mjs");

let scratch;
let repo;

function git(args, date) {
  return execFileSync("git", ["-C", repo, ...args], {
    encoding: "utf8",
    env: {
      ...process.env,
      GIT_AUTHOR_NAME: "split",
      GIT_AUTHOR_EMAIL: "split@example.invalid",
      GIT_COMMITTER_NAME: "split",
      GIT_COMMITTER_EMAIL: "split@example.invalid",
      GIT_AUTHOR_DATE: date,
      GIT_COMMITTER_DATE: date,
      GIT_CONFIG_GLOBAL: join(scratch, "gitconfig"),
    },
  });
}

function commit(subject, date, files) {
  for (const [path, body] of Object.entries(files)) {
    mkdirSync(dirname(join(repo, path)), { recursive: true });
    writeFileSync(join(repo, path), body);
  }
  git(["add", "-A"], date);
  git(["commit", "-qm", subject], date);
}

before(() => {
  scratch = mkdtempSync(join(tmpdir(), "crate-split-replay-"));
  writeFileSync(join(scratch, "gitconfig"), "");
  repo = join(scratch, "repo");
  mkdirSync(repo);
  git(["init", "-q", "--initial-branch=main"], "2026-09-01T00:00:00Z");
  // Before the window, so the first change in it has a parent to be diffed against.
  commit("chore: seed", "2026-09-01T00:00:00Z", {
    "Cargo.lock": "seed\n",
    "README.md": "seed\n",
    "crates/alpha/src/lib.rs": "seed\n",
    "crates/beta/src/lib.rs": "seed\n",
    "tests/one.rs": "seed\n",
    "tests/two.rs": "seed\n",
  });
  commit("feat: alpha", "2026-09-16T00:00:00Z", { "crates/alpha/src/lib.rs": "a\n" });
  commit("feat: beta", "2026-09-17T00:00:00Z", { "crates/beta/src/lib.rs": "b\n" });
  commit("test: suite one", "2026-09-18T00:00:00Z", { "tests/one.rs": "one\n" });
  commit("fix: the lock", "2026-09-19T00:00:00Z", { "Cargo.lock": "lock\n" });
  commit("docs: the readme", "2026-09-20T00:00:00Z", { "README.md": "readme\n" });
  commit("chore: release v0.2.0", "2026-09-21T00:00:00Z", { "CHANGELOG.md": "0.2.0\n" });
  commit("feat: a crate the mapping forgot", "2026-09-22T00:00:00Z", {
    "crates/gamma/src/lib.rs": "g\n",
  });
  // After the window.
  commit("feat: alpha again", "2026-10-20T00:00:00Z", { "crates/alpha/src/lib.rs": "a2\n" });

  const records = join(scratch, "records", "e2e");
  mkdirSync(records, { recursive: true });
  for (const [test, wall] of [
    ["one::first", 10],
    ["one::second", 5],
    ["two::only", 20],
  ]) {
    writeFileSync(
      join(records, `${test.replace("::", ".")}.json`),
      JSON.stringify({ schema: 1, binary: "e2e", test, run_id: "r", started_at: 1, wall_s: wall }),
    );
  }
});

after(() => rmSync(scratch, { recursive: true, force: true }));

function mapping(betaDependsOnAlpha) {
  return {
    shared: ["Cargo.lock"],
    sources: ["crates/**"],
    fallback: "alpha",
    crates: {
      alpha: { paths: ["crates/alpha/**"], deps: [] },
      beta: { paths: ["crates/beta/**"], deps: betaDependsOnAlpha ? ["alpha"] : [] },
    },
    suites: {
      one: { paths: ["tests/one.rs"], crates: ["alpha"], modules: ["one"] },
      two: { paths: ["tests/two.rs"], crates: ["beta"], modules: ["two"] },
    },
  };
}

function replayed(name, proposal) {
  const file = join(scratch, `${name}.json`);
  writeFileSync(file, JSON.stringify(proposal));
  const ran = spawnSync(
    process.execPath,
    [
      script,
      "--mapping",
      file,
      "--repo",
      repo,
      "--ref",
      "main",
      "--since",
      "2026-09-15T00:00:00Z",
      "--until",
      "2026-10-16T00:00:00Z",
      "--records",
      join(scratch, "records"),
      "--json",
    ],
    { encoding: "utf8" },
  );
  assert.equal(ran.status, 0, ran.stderr);
  return JSON.parse(ran.stdout);
}

const affectedBySubject = (result) =>
  Object.fromEntries(result.perChange.map((change) => [change.subject, change.affected]));

test("independent crates: a change runs only the suites that exercise what it touched", () => {
  const result = replayed("independent", mapping(false));
  assert.deepEqual(affectedBySubject(result), {
    "feat: alpha": ["one"],
    "feat: beta": ["two"],
    "test: suite one": ["one"],
    "fix: the lock": ["one", "two"],
    "docs: the readme": [],
    "feat: a crate the mapping forgot": ["one"],
  });
  assert.deepEqual(result.summary.unclaimedFiles, ["crates/gamma/src/lib.rs"]);
  assert.equal(result.suites.one.unaffected, 2);
  assert.equal(result.suites.two.unaffected, 4);
  assert.equal(result.summary.skippedSuiteRuns, 6);
  // `one` is 15 test-seconds and `two` 20: skipped twice and four times.
  assert.equal(result.summary.testSecondsPerRun, 35);
  assert.equal(result.summary.testSecondsSkipped, 2 * 15 + 4 * 20);
});

test("a dependency carries a change to the crates that depend on it", () => {
  const result = replayed("dependent", mapping(true));
  assert.deepEqual(affectedBySubject(result), {
    "feat: alpha": ["one", "two"],
    "feat: beta": ["two"],
    "test: suite one": ["one"],
    "fix: the lock": ["one", "two"],
    "docs: the readme": [],
    "feat: a crate the mapping forgot": ["one", "two"],
  });
  assert.equal(result.suites.two.unaffected, 2);
  assert.equal(result.summary.testSecondsSkipped, 2 * 15 + 2 * 20);
});

test("a mapping that names a crate it does not declare is refused", () => {
  const proposal = mapping(false);
  proposal.suites.two.crates = ["delta"];
  const file = join(scratch, "broken.json");
  writeFileSync(file, JSON.stringify(proposal));
  const ran = spawnSync(process.execPath, [script, "--mapping", file, "--repo", repo], {
    encoding: "utf8",
  });
  assert.equal(ran.status, 1);
  assert.match(ran.stderr, /suite two exercises unknown crate delta/);
});

test("globs cross directories only at a double star", () => {
  assert.ok(globToRegExp("src/**").test("src/channel/layout.rs"));
  assert.ok(globToRegExp("tests/e2e/**/*").test("tests/e2e/harness.rs"));
  assert.ok(globToRegExp("**/project.json").test("project.json"));
  assert.ok(!globToRegExp("src/*.rs").test("src/channel/layout.rs"));
  assert.ok(globToRegExp("src/*.rs").test("src/plan.rs"));
});
