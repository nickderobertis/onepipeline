// Every file a coverage tier's build or journeys can read, against what real Nx hashes for it.
import assert from "node:assert/strict";
import {
  appendFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { execFileSync } from "node:child_process";
import { keyGaps, readSets } from "./read-set.mjs";
import {
  TIERS,
  git,
  nx,
  root,
  scratch,
  test_all,
  tierOutcome,
} from "../coverage_cache/support.mjs";

// Reads the task hash does not name by file, each for a stated reason.
const VARIABLE_ONLY = {
  justfile:
    "read only for `onetaskgraph-version`, which every tier's recipe input names with --variable",
};
const VERSION_ONE_TEST =
  "this tier runs one test of the binary, which reads none of what the binary's other tests name";
const EXCEPTIONS = {
  "onepipeline-e2e": VARIABLE_ONLY,
  "onepipeline-version-e2e": VARIABLE_ONLY,
  "onepipeline-note-journeys": VARIABLE_ONLY,
  "onepipeline-contract": {
    "tests/parity.rs": "named as a word in an expected list, never opened",
  },
  "onepipeline-version-contract": {
    "docs/contract-divergences.md": VERSION_ONE_TEST,
    "docs/stop-guard.md": VERSION_ONE_TEST,
    "docs/test-profile-report.md": VERSION_ONE_TEST,
    "docs/windows-journey-audit.md": VERSION_ONE_TEST,
    "tests/parity.rs": VERSION_ONE_TEST,
  },
};

function copyTree(t) {
  const dir = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "coverage-keys-"),
  );
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (const path of execFileSync(
    "git",
    ["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
    { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  )
    .split("\0")
    .filter(Boolean)) {
    if (!existsSync(join(root, path))) continue;
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    cpSync(join(root, path), join(dir, path), { dereference: false });
  }
  symlinkSync(join(root, "node_modules"), join(dir, "node_modules"));
  git(dir, "init", "--quiet");
  git(dir, "add", "-A");
  return dir;
}

const report = (gaps) =>
  gaps
    .map(({ tier, path, why }) => `${tier} reads ${path}, which its key does not hash (${why})`)
    .join("\n");

test("every tier's key covers every workspace file it can read", { timeout: 600000 }, (t) => {
  const dir = copyTree(t);
  const { gaps, sets, tasks, taskOf } = keyGaps(dir, EXCEPTIONS);
  assert.equal(sets.size, TIERS.length, "the scan did not reach every coverage tier");
  assert.equal(report(gaps), "", "");
  for (const tier of TIERS) {
    const hashed = new Set([
      ...tasks[taskOf(tier)].sourceFiles,
      ...tasks[taskOf(tier)].runtimeFiles,
    ]);
    for (const required of [
      "Cargo.lock",
      "rust-toolchain.toml",
      ".github/ci-tool-versions.json",
      "scripts/coverage-unit.sh",
      "scripts/coverage-merge.mjs",
    ]) {
      assert.ok(hashed.has(required), `${tier} does not hash ${required}`);
    }
    assert.ok(!hashed.has("CHANGELOG.md"), `${tier} hashes CHANGELOG.md, the file no tier reads`);
    assert.ok(!sets.get(tier).has("CHANGELOG.md"), `${tier} is scanned as reading CHANGELOG.md`);
  }
});

test("a tier that leaves out a file only its journeys read at run time is named, and with it back in its key an edit re-runs that tier alone", {
  timeout: 900000,
}, (t) => {
  const SECRET = "secret/only-the-note-tier-reads-this.txt";
  const dir = copyTree(t);
  mkdirSync(join(dir, "secret"), { recursive: true });
  writeFileSync(join(dir, SECRET), "first\n");
  appendFileSync(
    join(dir, "tests/note/main.rs"),
    `\n#[test]\nfn reads_a_file_only_this_tier_opens() {\n    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));\n    assert!(std::fs::read_to_string(root.join("${SECRET}")).is_ok());\n}\n`,
  );
  git(dir, "add", "-A");
  const missing = keyGaps(dir, EXCEPTIONS).gaps;
  assert.ok(
    missing.some(({ tier, path }) => tier === "onepipeline-note-journeys" && path === SECRET),
    report(missing),
  );

  const repaired = (workspace) => {
    const path = join(workspace, "nx.json");
    const nx = JSON.parse(readFileSync(path, "utf8"));
    nx.namedInputs.noteJourneySource.push(`{workspaceRoot}/${SECRET}`);
    writeFileSync(path, `${JSON.stringify(nx, null, 2)}\n`);
  };
  repaired(dir);
  assert.deepEqual(keyGaps(dir, EXCEPTIONS).gaps, []);

  // The real thing, on a fixture tier that does open that file at run time.
  const fixture = scratch(t);
  mkdirSync(join(fixture, "secret"), { recursive: true });
  writeFileSync(join(fixture, SECRET), "first\n");
  appendFileSync(
    join(fixture, "tests/note/main.rs"),
    `\n#[test]\nfn reads_a_file_only_this_tier_opens() {\n    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));\n    assert!(std::fs::read_to_string(root.join("${SECRET}")).is_ok());\n}\n`,
  );
  repaired(fixture);
  git(fixture, "add", "-A");
  assert.equal(nx(fixture, test_all).status, 0);
  writeFileSync(join(fixture, SECRET), "second\n");
  const edited = nx(fixture, test_all);
  assert.equal(edited.status, 0, edited.output);
  assert.deepEqual(tierOutcome(edited).executed, ["onepipeline-note-journeys"]);
  assert.deepEqual(
    tierOutcome(edited).replayed,
    TIERS.filter((tier) => tier !== "onepipeline-note-journeys").sort(),
  );
});

test("readSets expands a directory the code joins at the workspace root", (t) => {
  const dir = copyTree(t);
  const reads = readSets(dir).get("onepipeline-contract");
  assert.ok(reads.has("docs/windows-journey-audit.md"), "the contract tier walks docs/");
});
