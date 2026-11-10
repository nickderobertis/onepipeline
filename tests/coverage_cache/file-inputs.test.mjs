// A coverage tier's cached report against real Nx: what edits move, and what they leave replaying.
import assert from "node:assert/strict";
import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import {
  TIERS,
  VERSION_TIERS,
  git,
  nx,
  plantOnetaskgraph,
  scratch,
  test_all,
  tierOutcome,
} from "./support.mjs";

const NOT_VERSION = TIERS.filter((tier) => !VERSION_TIERS.includes(tier));
const setJsonKey = (dir, file, key, value) => {
  const body = JSON.parse(readFileSync(join(dir, file), "utf8"));
  body[key] = value;
  writeFileSync(join(dir, file), `${JSON.stringify(body, null, 2)}\n`);
};

test("a coverage tier replays until what its run reads changes", { timeout: 1800000 }, (t) => {
  const dir = scratch(t);
  const cold = nx(dir, test_all);
  assert.equal(cold.status, 0, cold.output);
  assert.deepEqual(tierOutcome(cold).executed, [...TIERS].sort());
  const unchanged = nx(dir, test_all);
  assert.equal(unchanged.status, 0, unchanged.output);
  assert.deepEqual(
    tierOutcome(unchanged).replayed,
    [...TIERS].sort(),
    "a second run with nothing changed must replay every tier",
  );

  const version = readFileSync(join(dir, "Cargo.toml"), "utf8").match(/\nversion = "([^"]+)"/)[1];
  const newVersion = "99.77.0";
  const oldOnetaskgraph = readFileSync(join(dir, "justfile"), "utf8").match(
    /^onetaskgraph-version := "([^"]+)"/m,
  )[1];
  const CASES = [
    [
      "a tracked file no tier reads (CHANGELOG.md)",
      () => appendFileSync(join(dir, "CHANGELOG.md"), "\n"),
      [],
    ],
    [
      "a document only the contract tier's walk reads",
      () => appendFileSync(join(dir, "docs/windows-journey-audit.md"), "\n"),
      ["onepipeline-contract"],
    ],
    [
      "one tier's own test source (tests/contract)",
      () => addTracked(dir, "tests/contract/extra.rs", "// a source\n"),
      ["onepipeline-contract", "onepipeline-version-contract"],
    ],
    [
      "a file under the crate's src/",
      () => appendFileSync(join(dir, "src/lib.rs"), "\n// edited\n"),
      TIERS,
    ],
    [
      "an input of the nextest archive",
      () => appendFileSync(join(dir, ".config/nextest.toml"), "\n# edited\n"),
      TIERS,
    ],
    [
      ".github/ci-tool-versions.json",
      () => setJsonKey(dir, ".github/ci-tool-versions.json", "fixture-edit", "1"),
      TIERS,
    ],
    [
      "the cargo-llvm-cov pin",
      () => setJsonKey(dir, ".github/ci-tool-versions.json", "cargo-llvm-cov", "9.9.9"),
      TIERS,
    ],
    [
      "the nextest pin",
      () => setJsonKey(dir, ".github/ci-tool-versions.json", "cargo-nextest", "9.9.9"),
      TIERS,
    ],
    [
      "the released onetaskgraph pin",
      () => {
        const justfile = join(dir, "justfile");
        writeFileSync(
          justfile,
          readFileSync(justfile, "utf8").replace(
            `onetaskgraph-version := "${oldOnetaskgraph}"`,
            'onetaskgraph-version := "9.8.7"',
          ),
        );
        plantOnetaskgraph(dir);
      },
      TIERS,
    ],
    [
      "Cargo.lock",
      () =>
        writeFileSync(
          join(dir, "Cargo.lock"),
          readFileSync(join(dir, "Cargo.lock"), "utf8").replace(
            "# It is not intended for manual editing.",
            "# It is not intended for manual editing (fixture).",
          ),
        ),
      TIERS,
    ],
    [
      "rust-toolchain.toml",
      () => appendFileSync(join(dir, "rust-toolchain.toml"), "\n# edited\n"),
      TIERS,
    ],
    [
      "a package-version-only bump",
      () => {
        for (const file of ["Cargo.toml", "Cargo.lock"]) {
          writeFileSync(
            join(dir, file),
            readFileSync(join(dir, file), "utf8").replaceAll(
              `version = "${version}"`,
              `version = "${newVersion}"`,
            ),
          );
        }
      },
      VERSION_TIERS,
    ],
  ];
  for (const [name, edit, expected] of CASES) {
    const snapshot = captureEdit(dir, edit);
    const run = nx(dir, test_all);
    assert.equal(run.status, 0, `${name}: ${run.output}`);
    const { executed, replayed } = tierOutcome(run);
    assert.deepEqual(executed, [...expected].sort(), `${name}: tiers that re-ran`);
    assert.deepEqual(
      replayed,
      TIERS.filter((tier) => !expected.includes(tier)).sort(),
      `${name}: tiers that replayed`,
    );
    if (expected === VERSION_TIERS) assert.deepEqual(replayed, [...NOT_VERSION].sort());
    snapshot.restore();
    const again = nx(dir, test_all);
    assert.deepEqual(
      tierOutcome(again).replayed,
      [...TIERS].sort(),
      `${name}: restored, every tier must replay`,
    );
  }
});

test("inside a GitHub Actions job, the run summary still names every tier and no fixture result reaches the job's cache", {
  timeout: 900000,
}, (t) => {
  const enclosing = {
    GITHUB_ACTIONS: "true",
    NX_BASE: "a".repeat(40),
    NX_HEAD: "b".repeat(40),
    NX_SELF_HOSTED_REMOTE_CACHE_SERVER: "http://127.0.0.1:1",
    NX_SELF_HOSTED_REMOTE_CACHE_ACCESS_TOKEN: "enclosing-token",
  };
  const previous = Object.fromEntries(Object.keys(enclosing).map((key) => [key, process.env[key]]));
  t.after(() => {
    for (const [key, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  });
  Object.assign(process.env, enclosing);
  const dir = scratch(t);
  const cold = nx(dir, test_all);
  assert.equal(cold.status, 0, cold.output);
  assert.doesNotMatch(cold.output, /##\[group\]/, "Nx grouped its output for the enclosing job");
  assert.deepEqual(tierOutcome(cold).executed, [...TIERS].sort(), cold.output);
  const again = nx(dir, test_all);
  assert.equal(again.status, 0, again.output);
  assert.deepEqual(tierOutcome(again).replayed, [...TIERS].sort(), again.output);
});

function addTracked(dir, path, body) {
  mkdirSync(dirname(join(dir, path)), { recursive: true });
  writeFileSync(join(dir, path), body);
  git(dir, "add", path);
}

// Files every case above might touch, kept to restore the tree exactly between cases.
function captureEdit(dir, edit) {
  const paths = [
    "CHANGELOG.md",
    "docs/windows-journey-audit.md",
    "src/lib.rs",
    ".config/nextest.toml",
    ".github/ci-tool-versions.json",
    "justfile",
    "Cargo.lock",
    "rust-toolchain.toml",
    "Cargo.toml",
  ];
  const before = new Map(paths.map((path) => [path, readFileSync(join(dir, path))]));
  edit();
  return {
    restore() {
      for (const [path, bytes] of before) writeFileSync(join(dir, path), bytes);
      try {
        git(dir, "rm", "-q", "--cached", "tests/contract/extra.rs");
      } catch {
        // Only one case adds it.
      }
      git(dir, "clean", "-fdq", "--", "tests/contract/extra.rs");
    },
  };
}
