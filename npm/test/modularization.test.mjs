import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { affectedSuites, compileMapping } from "../../scripts/crate-split-replay.mjs";
import { moduleGraph } from "../../scripts/modularization/graph.mjs";
import { replayMapping, buildReport } from "../../scripts/modularization/replay.mjs";

const scratch = () =>
  mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "modularization-"));
const put = (root, path, value) => {
  mkdirSync(resolve(root, path, ".."), { recursive: true });
  writeFileSync(join(root, path), typeof value === "string" ? value : JSON.stringify(value));
};

test("graph CLI accepts a mapped dependency and names a deliberately introduced cycle", () => {
  const root = scratch();
  try {
    put(root, "src/command.rs", "use crate::record::Record;\nfn run() {}\n");
    put(root, "src/record.rs", "pub struct Record;\n");
    const layout = {
      schema: 1,
      crates: {
        command: { files: ["src/command.rs"], deps: ["record"] },
        record: { files: ["src/record.rs"], deps: [] },
      },
      edges: { "command->record": { kind: "shared", fix: "Record is shared durable state." } },
    };
    put(root, "layout.json", layout);
    const cli = resolve("scripts/modularization/graph.mjs");
    const run = (out) =>
      spawnSync(process.execPath, [cli, "layout.json", ...(out ? [out] : [])], {
        cwd: root,
        encoding: "utf8",
      });
    assert.equal(run("graph.json").status, 0);
    assert.equal(JSON.parse(readFileSync(join(root, "graph.json"), "utf8")).files, 2);
    layout.crates.record.deps.push("command");
    put(root, "layout.json", layout);
    const failed = run();
    assert.equal(failed.status, 1);
    assert.match(failed.stderr, /command -> record -> command/);
    layout.crates.record.deps = [];
    layout.crates.record.files = [];
    put(root, "layout.json", layout);
    assert.match(run().stderr, /src\/record.rs: expected one owner, found 0/);
    layout.crates.record.files = ["src/record.rs"];
    layout.crates.command.deps = [];
    put(root, "layout.json", layout);
    assert.match(run().stderr, /missing dependency command->record/);
    layout.crates.command.deps = ["record"];
    layout.edges = {};
    put(root, "layout.json", layout);
    assert.match(run().stderr, /unclassified edge command->record/);
    layout.typo = true;
    put(root, "layout.json", layout);
    assert.match(run().stderr, /unknown layout field: typo/);
    delete layout.typo;
    layout.suites = { status: { paths: [], crates: ["unknown"] } };
    put(root, "layout.json", layout);
    assert.match(run().stderr, /unknown crate unknown/);
    layout.suites = { status: { paths: [], crates: ["command"], typo: true } };
    put(root, "layout.json", layout);
    assert.match(run().stderr, /unknown suite status field: typo/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("source graph ignores literal/comment dependencies and sees grouped imports", () => {
  const root = scratch();
  try {
    put(
      root,
      "src/a.rs",
      'use crate::{b::{Thing}, c};\n// crate::d::x\nconst S: &str = "crate::d::x"; /* outer /* inner */ crate::d::hidden */ const RAW: &str = r##"crate::d::hidden"##;',
    );
    for (const name of ["b", "c", "d"]) put(root, `src/${name}.rs`, "");
    assert.deepEqual(
      moduleGraph(root).edges.map(({ from, to }) => `${from}->${to}`),
      ["a->b", "a->c"],
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("replay selection matches real Nx for command, transitive shared, lock and toolchain changes", {
  timeout: 120000,
}, () => {
  const root = scratch();
  const env = { ...process.env, NX_DAEMON: "false", NX_ISOLATE_PLUGINS: "false" };
  const git = (...args) =>
    execFileSync("git", args, {
      cwd: root,
      env,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
  try {
    symlinkSync(resolve("node_modules"), join(root, "node_modules"), "dir");
    put(root, ".gitignore", "node_modules\n.nx\n");
    put(root, "package.json", { private: true });
    const shared = ["Cargo.lock", "rust-toolchain.toml"];
    put(root, "nx.json", {
      namedInputs: { globals: shared.map((path) => `{workspaceRoot}/${path}`) },
    });
    const crates = {
      record: { paths: ["record/**"], deps: [] },
      reader: { paths: ["reader/**"], deps: ["record"] },
      status: { paths: ["status/**"], deps: ["reader"] },
      template: { paths: ["template/**"], deps: [] },
    };
    const suites = {
      "status-e2e": { paths: ["status-e2e/**"], crates: ["status"] },
      "template-e2e": { paths: ["template-e2e/**"], crates: ["template"] },
    };
    for (const [name, entry] of [...Object.entries(crates), ...Object.entries(suites)]) {
      put(root, `${name}/project.json`, {
        name,
        root: name,
        implicitDependencies: entry.deps ?? entry.crates,
        targets: {
          test: { executor: "nx:noop", inputs: ["{projectRoot}/**/*", "globals", "^default"] },
        },
      });
      put(root, `${name}/source.txt`, "initial\n");
    }
    for (const path of shared) put(root, path, "initial\n");
    git("init", "-b", "main");
    git("config", "user.email", "fixture@example.invalid");
    git("config", "user.name", "Fixture");
    git("add", ".");
    git("commit", "-m", "test: initial graph");
    const mapping = compileMapping(
      replayMapping({
        crates: {
          ...Object.fromEntries(
            Object.entries(crates).map(([name, crate]) => [
              name,
              { files: crate.paths, deps: crate.deps },
            ]),
          ),
          onepipeline: { files: [], deps: [] },
        },
        suites,
      }),
    );
    for (const path of ["status/source.txt", "record/source.txt", ...shared]) {
      put(root, path, `${readFileSync(join(root, path), "utf8")}changed\n`);
      git("add", path);
      git("commit", "-m", `test: change ${path}`);
      const output = execFileSync(
        process.execPath,
        [
          resolve("node_modules/nx/dist/bin/nx.js"),
          "show",
          "projects",
          "--affected",
          "--base=HEAD~1",
          "--head=HEAD",
          "--json",
        ],
        { cwd: root, env, encoding: "utf8" },
      );
      const nx = JSON.parse(output)
        .filter((name) => Object.hasOwn(suites, name))
        .sort();
      assert.deepEqual(affectedSuites(mapping, [path]).affected.sort(), nx, path);
      assert.ok(nx.includes("status-e2e"), `${path} must reach status through reader`);
      assert.equal(nx.includes("template-e2e"), shared.includes(path));
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("profile weights use only current records and preserve missing platform evidence", async () => {
  const { profileTable } = await import("../../scripts/modularization/replay.mjs");
  const fixture =
    "### Every e2e module, on the current records\n| x | 1 | 10 | 20 | 30 | – | 0% | 0% | 0% | A |\n<details>\n| x | 1 | 99 | 99 | 99 | 99 |";
  const weights = profileTable(fixture);
  assert.equal(weights.local.get("x"), 10);
  assert.equal(weights.linux.get("x"), 20);
  assert.equal(weights.windows.has("x"), false);
  assert.throws(() => profileTable("no table"), /no all-module table/);
  assert.throws(
    () => profileTable(fixture.replace("| 20 |", "| broken |")),
    /invalid linux seconds/,
  );
  assert.throws(() => profileTable(fixture.replace("| 1 |", "| broken |")), /malformed timing row/);
});

test("finished layout accounts for current source edges and each e2e module once", async () => {
  const { validate } = await import("../../scripts/modularization/graph.mjs");
  const layout = JSON.parse(readFileSync("scripts/modularization/layout.json", "utf8"));
  assert.ok(validate(layout, moduleGraph()).files > 0);
  const declared = JSON.parse(readFileSync("scripts/crate-split/proposed.json", "utf8"));
  const expected = Object.values(declared.suites)
    .flatMap((suite) => suite.modules)
    .sort();
  const actual = Object.values(layout.suites)
    .flatMap((suite) => suite.modules)
    .sort();
  assert.deepEqual(actual, expected);
  assert.equal(new Set(actual).size, actual.length);
});

test("suite-input command follows borrowed helpers, records runtime fixtures, and refuses unknown modules", () => {
  const root = scratch();
  try {
    put(root, "tests/e2e/main.rs", "mod harness;\nmod a;\nmod b;\n");
    put(root, "tests/e2e/harness.rs", "");
    put(root, "tests/e2e/a.rs", "fn x() { crate::b::helper(); }");
    put(root, "tests/e2e/b.rs", 'fn helper() { repo_file("fixtures/record.json"); }');
    put(root, "layout.json", {
      crates: { binary: { paths: ["src/**"], deps: [] } },
      suites: { a: { modules: ["a"], paths: [], crates: ["binary"] } },
    });
    const script = resolve("scripts/modularization/suite-inputs.mjs");
    const run = () =>
      spawnSync(process.execPath, [script, "layout.json"], { cwd: root, encoding: "utf8" });
    assert.equal(run().status, 0);
    const result = JSON.parse(readFileSync(join(root, "layout.json"), "utf8"));
    assert.ok(result.suites.a.paths.includes("tests/e2e/b.rs"));
    assert.ok(result.suites.a.paths.includes("fixtures/record.json"));
    const first = readFileSync(join(root, "layout.json"), "utf8");
    assert.equal(run().status, 0);
    assert.equal(readFileSync(join(root, "layout.json"), "utf8"), first);
    result.suites.a.modules = ["missing"];
    put(root, "layout.json", result);
    const failed = run();
    assert.equal(failed.status, 1);
    assert.match(failed.stderr, /undeclared test module: missing/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("weighted migration replay keeps residual edges, assets, missing evidence and dependency order", () => {
  const layout = {
    evidenceRevision: "0".repeat(40),
    crates: {
      record: { files: ["src/record.rs"], deps: [] },
      status: { files: ["src/status.rs"], deps: ["record"] },
      onepipeline: { files: ["src/main.rs"], deps: ["status"] },
    },
    assets: { status: ["assets/status.txt"] },
    suites: {
      status: { paths: ["test/status.rs"], crates: ["status"], modules: ["status"] },
      full: { paths: ["test/full.rs"], crates: ["onepipeline"], modules: ["full"] },
    },
  };
  const changes = [
    { files: ["src/record.rs"] },
    { files: ["assets/status.txt"] },
    { files: ["src/main.rs"] },
  ];
  const result = buildReport(layout, changes, { linux: new Map([["status", 10]]) });
  assert.deepEqual(
    result.migration.map((step) => step.crate),
    ["record", "status", "onepipeline"],
  );
  assert.deepEqual(result.platforms.linux.missingModules, ["full"]);
  assert.equal(result.platforms.linux.summary.testSecondsPerRun, 10);
  assert.equal(result.platforms.linux.summary.testSecondsSkipped, 10);
  assert.equal(result.platforms.linux.summary.skippedSuiteRuns, 1);
  assert.equal(result.migration[0].skippedSuiteFraction, 0);
  assert.equal(result.migration.at(-1).skippedSuiteFraction, 1 / 6);
  layout.crates.record.deps.push("status");
  assert.throws(() => buildReport(layout, changes, { linux: new Map() }), /cycle|cyclic/);
});

test("replay CLI writes the 144-PR artifact and refuses a different history", {
  timeout: 120000,
}, () => {
  const root = scratch();
  try {
    const script = resolve("scripts/modularization/replay.mjs");
    const out = join(root, "report.json");
    const result = spawnSync(
      process.execPath,
      [script, "scripts/modularization/layout.json", out],
      { encoding: "utf8" },
    );
    assert.equal(result.status, 0, result.stderr);
    const report = JSON.parse(readFileSync(out, "utf8"));
    assert.equal(report.changes, 144);
    assert.equal(report.platforms.linux.summary.modulesInNoSuite.length, 0);
    assert.ok(report.platforms.windows.missingModules.length > 0);
    assert.ok(report.migration.length > 0);
    put(root, "src/record.rs", "pub struct Record;");
    put(root, "layout.json", {
      schema: 1,
      crates: { record: { files: ["src/record.rs"], deps: [] } },
      edges: {},
    });
    const git = (...args) => execFileSync("git", args, { cwd: root, stdio: "pipe" });
    git("init", "-b", "main");
    git("config", "user.name", "Fixture");
    git("config", "user.email", "fixture@example.invalid");
    git("add", "src/record.rs");
    git("commit", "-m", "test: seed");
    git("update-ref", "refs/remotes/origin/main", "HEAD");
    const failed = spawnSync(process.execPath, [script, "layout.json", "refused.json"], {
      cwd: root,
      encoding: "utf8",
    });
    assert.equal(failed.status, 1);
    assert.match(failed.stderr, /expected 144 PRs, found 0/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("suite mappings cover transitive source inputs and generated design tables are current", {
  timeout: 120000,
}, async () => {
  const { suiteInputs } = await import("../../scripts/modularization/suite-inputs.mjs");
  const layout = JSON.parse(readFileSync("scripts/modularization/layout.json", "utf8"));
  const compiled = compileMapping(replayMapping(layout));
  for (const [name, suite] of Object.entries(layout.suites)) {
    for (const path of suiteInputs(suite.modules).filter((path) => !path.includes("*"))) {
      const covered =
        compiled.shared(path) ||
        compiled.suites[name].own(path) ||
        compiled.suites[name].own(`${path}/probe`) ||
        Object.values(compiled.crates).some((claims) => claims(path));
      assert.ok(covered, `${name} omits ${path}`);
    }
  }
  const result = spawnSync(process.execPath, ["scripts/modularization/render.mjs"], {
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
});
