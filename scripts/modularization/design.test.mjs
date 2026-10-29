import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { affectedSuites, compileMapping } from "../crate-split-replay.mjs";
import { moduleGraph, codeOnly } from "./graph.mjs";
import { manifestChanges } from "./manifest-projection.mjs";
import { replayMapping, buildReport } from "./replay.mjs";

const timingHeader =
  "| module | tests | local Linux s (7a017fa) | CI Linux s | macOS s | Windows s | spawn | wait | build | proposal |";

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
    assert.equal(JSON.parse(readFileSync(join(root, "graph.json"), "utf8")).edges, 1);
    const surplus = spawnSync(process.execPath, [cli, "layout.json", "graph.json", "extra"], {
      cwd: root,
      encoding: "utf8",
    });
    assert.equal(surplus.status, 1);
    assert.match(surplus.stderr, /usage: node scripts\/modularization\/graph.mjs/);

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
    layout.edges["command->record"] = { kind: "shared", fix: "Restore the shared record edge." };
    layout.edges["command->record"].interface = {
      owner: "command",
      trait: "Reader",
      adapter: "missing",
    };
    put(root, "layout.json", layout);
    assert.match(run().stderr, /invalid interface command->record/);
    delete layout.edges["command->record"].interface;
    layout.edges["command->record"].movedTo = 2;
    put(root, "layout.json", layout);
    assert.match(run().stderr, /invalid movedTo command->record/);
    delete layout.edges["command->record"].movedTo;
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
    layout.suites = { status: { paths: [], crates: ["command"] } };
    put(root, "layout.json", layout);
    assert.match(run().stderr, /modules is required/);
    put(root, "tests/e2e/main.rs", "mod harness;\nmod status;\nmod support;\n");
    put(root, "tests/e2e/harness.rs", "#[test] fn helper_test() {} ");
    put(root, "tests/e2e/status.rs", "#[test] fn status_test() {} ");
    put(root, "tests/e2e/support.rs", "fn support() {} ");
    layout.suites.status.modules = ["unknown"];
    put(root, "layout.json", layout);
    assert.match(run().stderr, /unknown test module unknown/);
    layout.suites.status.modules = ["status"];
    put(root, "layout.json", layout);
    assert.match(run().stderr, /unowned test modules: harness/);
    layout.suites.status.modules = ["status", "harness"];
    layout.edges = { "command->record": { kind: "shared", fix: "Shared record." } };
    put(root, "nx.json", { namedInputs: {} });
    put(root, "tests/contract/project.json", {
      sourceRoot: "tests/contract",
      tags: ["lang:rust"],
      targets: { test: { inputs: ["{workspaceRoot}/tests/contract/assertions.rs"] } },
    });
    layout.auxiliarySuites = {
      contract: {
        paths: ["tests/contract/**/*", "tests/contract/assertions.rs"],
        crates: ["command"],
        reason: "Full facade assertions.",
      },
    };
    put(root, "layout.json", layout);
    assert.equal(run().status, 0);
    layout.auxiliarySuites.contract.crates = ["absent"];
    put(root, "layout.json", layout);
    assert.match(run().stderr, /unknown crate absent/);
    layout.auxiliarySuites.contract.crates = ["command"];
    layout.auxiliarySuites.contract.paths = "invalid";
    put(root, "layout.json", layout);
    assert.match(run().stderr, /contract.paths/);
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
    put(
      root,
      "src/a.rs",
      readFileSync(join(root, "src/a.rs"), "utf8") +
        "\nfn borrow<'a>(value: &'a str) { let quote = '\\''; let slash = '\\\\'; let newline = '\\n'; crate::c::use_it(); }",
    );
    put(
      root,
      "src/a.rs",
      readFileSync(join(root, "src/a.rs"), "utf8") +
        `
fn quoted<'life>(v: &'life str) { let double = '"'; crate::e::real(); }`,
    );
    for (const name of ["b", "c", "d", "e"]) put(root, `src/${name}.rs`, "");
    assert.deepEqual(
      moduleGraph(root).edges.map(({ from, to }) => `${from}->${to}`),
      ["a->b", "a->c", "a->e"],
    );
    put(root, "layout.json", {
      schema: 1,
      crates: {
        a: { files: ["src/a.rs"], deps: ["b", "c", "e"] },
        b: { files: ["src/b.rs"], deps: [] },
        c: { files: ["src/c.rs"], deps: [] },
        d: { files: ["src/d.rs"], deps: [] },
        e: { files: ["src/e.rs"], deps: [] },
      },
      edges: {
        "a->b": { kind: "shared", fix: "Shared B." },
        "a->c": { kind: "shared", fix: "Shared C." },
        "a->e": { kind: "shared", fix: "Shared E." },
      },
    });
    const result = spawnSync(
      process.execPath,
      [resolve("scripts/modularization/graph.mjs"), "layout.json", "graph.json"],
      { cwd: root, encoding: "utf8" },
    );
    assert.equal(result.status, 0, result.stderr);
    assert.equal(JSON.parse(readFileSync(join(root, "graph.json"), "utf8")).edges, 3);
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
    symlinkSync(resolve("node_modules"), join(root, "node_modules"), "junction");
    put(root, ".gitignore", "node_modules\n.nx\n");
    put(root, "package.json", { private: true });
    const shared = ["Cargo.lock", "rust-toolchain.toml", "custom-build-input.txt"];
    put(root, "nx.json", {
      namedInputs: {
        globals: shared.map((path) => `{workspaceRoot}/${path}`),
        rustBuild: ["globals", "{workspaceRoot}/src/**"],
        e2eSource: ["rustBuild", "{workspaceRoot}/tests/e2e/**"],
      },
    });
    put(root, "tests/e2e/main.rs", "mod harness;\n");
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
      replayMapping(
        {
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
        },
        undefined,
        root,
      ),
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
  const { profileTable } = await import("./replay.mjs");
  const fixture = `### Every e2e module, on the current records\n${timingHeader}\n| x | 1 | 10 | 20 | 30 | – | 0% | 0% | 0% | A |\n<details>\n| x | 1 | 99 | 99 | 99 | 99 |`;
  const weights = profileTable(fixture);
  assert.equal(weights.local.get("x"), 10);
  assert.equal(weights.linux.get("x"), 20);
  assert.equal(
    profileTable(fixture.replace("CI Linux s | macOS s", "macOS s | CI Linux s")).linux.get("x"),
    30,
  );
  assert.equal(weights.windows.has("x"), false);
  assert.throws(() => profileTable("no table"), /no all-module table/);
  assert.throws(
    () => profileTable(fixture.replace("| 20 |", "| broken |")),
    /invalid linux seconds/,
  );
  assert.throws(() => profileTable(fixture.replace("| 1 |", "| broken |")), /malformed timing row/);
});

test("finished layout accounts for current source edges and each e2e module once", async () => {
  const { validate } = await import("./graph.mjs");
  const layout = JSON.parse(readFileSync("scripts/modularization/layout.json", "utf8"));
  assert.ok(validate(layout, moduleGraph()).files > 0);
  const expected = [...readFileSync("tests/e2e/main.rs", "utf8").matchAll(/^mod (\w+);/gm)]
    .map((match) => match[1])
    .filter((module) =>
      /#\s*\[\s*test\s*\]/.test(codeOnly(readFileSync(`tests/e2e/${module}.rs`, "utf8"))),
    )
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
    put(
      root,
      "tests/e2e/b.rs",
      'const RECORD: &str = "fixtures/constant.json";\nconst DIRECTORY: &str = "fixtures/runtime";\nconst INCLUDED: &str = include_str!("../../fixtures/included.json");\nfn helper() { repo_file("fixtures/record.json"); repo_file(RECORD); Path::new(env!("CARGO_MANIFEST_DIR")).join(DIRECTORY); }',
    );
    put(root, "fixtures/runtime/data.json", "{}");
    put(root, "fixtures/included.json", "{}");
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
    assert.ok(result.suites.a.paths.includes("fixtures/constant.json"));
    assert.ok(result.suites.a.paths.includes("fixtures/included.json"));
    assert.ok(result.suites.a.paths.includes("fixtures/runtime"));
    assert.ok(result.suites.a.paths.includes("fixtures/runtime/**"));
    const first = readFileSync(join(root, "layout.json"), "utf8");
    assert.equal(run().status, 0);
    assert.equal(readFileSync(join(root, "layout.json"), "utf8"), first);
    put(root, "src/main.rs", "fn main() {} ");
    put(
      root,
      "src/cli.rs",
      "pub enum Command {\n    Template(TemplateCommand),\n}\npub enum TemplateCommand {\n    List(Args),\n    Resolve(Args),\n    Check(Args),\n}\n",
    );
    put(root, "tests/e2e/a.rs", 'fn x() { crate::b::helper(); run(&["template", "list"]); }');
    const modular = {
      schema: 1,
      crates: {
        onepipeline: { files: ["src/main.rs", "src/cli.rs"], deps: [] },
        "cmd-template": { files: [], deps: [] },
        "cmd-watch": { files: [], deps: [] },
        "cmd-template-list": { files: [], deps: [] },
        "cmd-template-resolve": { files: [], deps: [] },
        "cmd-template-check": { files: [], deps: [] },
      },
      edges: {},
      suites: { a: { modules: ["a", "b", "harness"], paths: [], crates: ["onepipeline"] } },
    };
    put(root, "layout.json", modular);
    assert.equal(run().status, 0);
    const roots = JSON.parse(readFileSync(join(root, "layout.json"), "utf8")).suites.a.crates;
    for (const name of [
      "cmd-template",
      "cmd-template-list",
      "cmd-template-resolve",
      "cmd-template-check",
    ])
      assert.ok(roots.includes(name), name);
    put(
      root,
      "src/cli.rs",
      readFileSync(join(root, "src/cli.rs"), "utf8").replace(
        "    Check(Args),",
        "    Check(Args),\n    Inspect(Args),",
      ),
    );
    modular.crates["cmd-template-inspect"] = { files: [], deps: [] };
    modular.schema = 2;
    modular.commands = {};
    modular.dispatcher = {
      crate: "onepipeline",
      routingFiles: ["src/main.rs", "src/cli.rs"],
      routingFunctions: ["main"],
    };
    modular.generatedEdges = [];
    put(root, "layout.json", modular);
    assert.equal(run().status, 0);
    assert.ok(
      JSON.parse(readFileSync(join(root, "layout.json"), "utf8")).suites.a.crates.includes(
        "cmd-template-inspect",
      ),
    );
    put(
      root,
      "src/cli.rs",
      readFileSync(join(root, "src/cli.rs"), "utf8").replace(
        "    Template(TemplateCommand),",
        "    Template(TemplateCommand),\n    Fresh(Args),",
      ),
    );
    put(root, "layout.json", modular);
    assert.match(run().stderr, /missing command mapping: cmd-fresh/);
    modular.crates["cmd-fresh"] = { files: [], deps: [] };
    put(root, "layout.json", modular);
    assert.equal(run().status, 0);
    put(
      root,
      "src/cli.rs",
      readFileSync(join(root, "src/cli.rs"), "utf8").replace(
        "    Fresh(Args),",
        "    Fresh(Args),\n    Monitor(Args),",
      ),
    );
    put(root, "tests/e2e/b.rs", 'fn helper() { run(&["monitor"]); }');
    put(root, "layout.json", modular);
    assert.match(run().stderr, /missing command mapping: cmd-observer-view/);
    modular.crates["cmd-observer-view"] = { files: [], deps: [] };
    put(root, "layout.json", modular);
    assert.equal(run().status, 0);
    assert.ok(
      JSON.parse(readFileSync(join(root, "layout.json"), "utf8")).suites.a.crates.includes(
        "cmd-observer-view",
      ),
    );
    modular.suites.a.modules = ["missing"];
    put(root, "layout.json", modular);
    const failed = run();
    assert.equal(failed.status, 1);
    assert.match(failed.stderr, /unknown test module missing/);
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
  const { suiteInputs, commandCrates } = await import("./suite-inputs.mjs");
  const layout = JSON.parse(readFileSync("scripts/modularization/layout.json", "utf8"));
  const compiled = compileMapping(replayMapping(layout));
  for (const [name, suite] of Object.entries(layout.suites)) {
    for (const crate of commandCrates(suite.modules, new Set(Object.keys(layout.crates))))
      assert.ok(suite.crates.includes(crate), `${name} omits literal command root ${crate}`);
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

test("CLI replay weights, malformed profile recovery, and render write/freshness journeys", {
  timeout: 180000,
}, () => {
  const root = scratch();
  const run = (script, ...args) =>
    spawnSync(process.execPath, [resolve(`scripts/modularization/${script}.mjs`), ...args], {
      cwd: root,
      encoding: "utf8",
    });
  try {
    // The real historical corpus is reused, while all induced edits stay in this private clone.
    execFileSync("git", ["clone", "--shared", "--quiet", process.cwd(), root], { stdio: "pipe" });
    put(
      root,
      "scripts/modularization/layout.json",
      readFileSync("scripts/modularization/layout.json", "utf8"),
    );
    const base = execFileSync("git", ["rev-parse", "origin/main"], { encoding: "utf8" }).trim();
    execFileSync("git", ["update-ref", "refs/remotes/origin/main", base], { cwd: root });
    const layout = JSON.parse(
      readFileSync(join(root, "scripts/modularization/layout.json"), "utf8"),
    );
    const modules = Object.values(layout.suites).flatMap((suite) => suite.modules);
    const profile =
      `### Every e2e module, on the current records\n${timingHeader}\n` +
      modules.map((name) => `| ${name} | 1 | 2 | 1 | 3 | – | x | x | x | x |`).join("\n");
    put(root, "docs/test-profile-report.md", profile);
    const result = run("replay", "scripts/modularization/layout.json", "report.json");
    assert.equal(result.status, 0, result.stderr);
    const report = JSON.parse(readFileSync(join(root, "report.json"), "utf8"));
    assert.equal(report.platforms.linux.summary.skippedSuiteRuns, 519);
    assert.equal(report.platforms.linux.summary.testSecondsPerRun, 88);
    assert.equal(report.platforms.linux.summary.testSecondsSkipped, 519);
    assert.equal(report.platforms.local.summary.testSecondsSkipped, 1038);
    assert.equal(report.platforms.macos.summary.testSecondsSkipped, 1557);
    assert.deepEqual(report.platforms.windows.missingModules.sort(), modules.sort());
    assert.equal(report.migration[0].crate, "render-cost");
    assert.equal(report.migration[0].skippedSuiteFraction, 519 / (144 * 88));
    assert.equal(report.migration.length, 74);
    assert.equal(report.migration.at(-1).skippedSuiteFraction, 519 / (144 * 88));
    put(
      root,
      "docs/test-profile-report.md",
      profile.replace("| 2 | 1 | 3 |", "| 2 | broken | 3 |"),
    );
    const malformed = run("replay", "scripts/modularization/layout.json", "refused.json");
    assert.equal(malformed.status, 1);
    assert.match(malformed.stderr, /invalid linux seconds/);
    put(
      root,
      "docs/test-profile-report.md",
      profile.replace("CI Linux s | macOS s", "macOS s | CI Linux s"),
    );
    assert.equal(run("replay", "scripts/modularization/layout.json", "reordered.json").status, 0);
    assert.equal(
      JSON.parse(readFileSync(join(root, "reordered.json"), "utf8")).platforms.linux.summary
        .testSecondsSkipped,
      1557,
    );
    put(root, "docs/test-profile-report.md", profile.replace("CI Linux s", "unknown seconds"));
    const badHeader = run("replay", "scripts/modularization/layout.json", "refused-header.json");
    assert.equal(badHeader.status, 1);
    assert.match(badHeader.stderr, /invalid timing header/);
    for (const [invalid, diagnostic] of [
      [
        `${profile}\n| ${modules[0]} | 1 | 2 | 1 | 3 | – | x | x | x | x |`,
        /duplicate timing module/,
      ],
      [`### Every e2e module, on the current records\n${timingHeader}\n`, /no timing rows/],
    ]) {
      put(root, "docs/test-profile-report.md", invalid);
      const refused = run("replay", "scripts/modularization/layout.json", "refused-table.json");
      assert.equal(refused.status, 1);
      assert.match(refused.stderr, diagnostic);
    }
    put(root, "docs/test-profile-report.md", profile);
    const stale = run("render");
    assert.equal(stale.status, 1);
    assert.match(stale.stderr, /design tables are stale/);
    const written = run("render", "--write");
    assert.equal(written.status, 0, written.stderr);
    assert.match(readFileSync(join(root, "docs/modularization-design.md"), "utf8"), /519/);
    assert.equal(run("render").status, 0);
    put(
      root,
      "docs/modularization-design.md",
      `${readFileSync(join(root, "docs/modularization-design.md"), "utf8")}stale\n`,
    );
    assert.equal(run("render").status, 1);
    assert.equal(run("render", "--write").status, 0);
    assert.equal(run("render").status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("graph CLI discovers composition declarations and direct/grouped diagnostic aliases", () => {
  const root = scratch();
  try {
    put(
      root,
      "src/lib.rs",
      "pub mod direct;\nmod grouped;\npub mod error;\npub use error::{Error, Result};\n",
    );
    put(root, "src/direct.rs", "fn run() -> crate::Result<()> { let _: crate::Error; }");
    put(root, "src/grouped.rs", "use crate::{Error, Result};\nfn run() -> Result<()> {}");
    put(root, "src/error.rs", "pub struct Error;\npub type Result<T> = T;");
    const expected = [
      "direct->error",
      "grouped->error",
      "lib->direct",
      "lib->error",
      "lib->grouped",
    ];
    assert.deepEqual(
      moduleGraph(root).edges.map(({ from, to }) => `${from}->${to}`),
      expected,
    );
    const layout = {
      schema: 1,
      crates: {
        lib: { files: ["src/lib.rs"], deps: ["direct", "grouped", "error"] },
        direct: { files: ["src/direct.rs"], deps: ["error"] },
        grouped: { files: ["src/grouped.rs"], deps: ["error"] },
        error: { files: ["src/error.rs"], deps: [] },
      },
      edges: Object.fromEntries(
        expected.map((edge) => [
          edge,
          { kind: "shared", fix: "Explicit composition or shared diagnostic type." },
        ]),
      ),
    };
    put(root, "layout.json", layout);
    const cli = resolve("scripts/modularization/graph.mjs");
    const run = () =>
      spawnSync(process.execPath, [cli, "layout.json", "graph.json"], {
        cwd: root,
        encoding: "utf8",
      });
    assert.equal(run().status, 0);
    assert.equal(JSON.parse(readFileSync(join(root, "graph.json"), "utf8")).edges, 5);
    put(
      root,
      "src/lib.rs",
      "pub mod direct;\nmod grouped;\npub mod error;\npub use error::{Error as Fault, Result};\n",
    );
    put(root, "src/direct.rs", "fn run() -> crate::Result<()> { let _: crate::Fault; }");
    assert.deepEqual(
      moduleGraph(root).edges.map(({ from, to }) => `${from}->${to}`),
      expected,
    );
    delete layout.edges["direct->error"];
    put(root, "layout.json", layout);
    const refused = run();
    assert.equal(refused.status, 1);
    assert.match(refused.stderr, /unclassified edge direct->error/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("graph CLI refuses source-asset and auxiliary project drift, then accepts repaired ownership", () => {
  const root = scratch();
  const cli = resolve("scripts/modularization/graph.mjs");
  const run = () =>
    spawnSync(process.execPath, [cli, "layout.json"], { cwd: root, encoding: "utf8" });
  const layout = {
    schema: 1,
    crates: { onepipeline: { files: ["src/main.rs"], deps: [] } },
    edges: {},
    assets: { onepipeline: ["src/message.txt"] },
    auxiliarySuites: {},
  };
  const save = () => put(root, "layout.json", layout);
  try {
    put(root, "src/main.rs", "fn main() {}");
    put(root, "src/message.txt", "hello");
    put(root, "tests/support.txt", "support");
    put(root, "nx.json", { namedInputs: { journey: ["{workspaceRoot}/tests/fixture.rs"] } });
    save();
    assert.equal(run().status, 0);
    put(root, "src/new.txt", "new asset");
    assert.match(run().stderr, /source asset src\/new.txt: expected one owner/);
    layout.assets.onepipeline.push("src/new.txt");
    save();
    assert.equal(run().status, 0);
    rmSync(join(root, "src/message.txt"));
    assert.match(run().stderr, /stale source asset: src\/message.txt/);
    layout.assets.onepipeline = ["src/new.txt"];
    save();
    const project = {
      sourceRoot: "tests/journey",
      tags: ["lang:rust"],
      targets: { test: { inputs: ["journey"] } },
    };
    put(root, "tests/journey/project.json", project);
    assert.match(run().stderr, /auxiliary suite membership differs/);
    layout.auxiliarySuites.journey = {
      paths: ["tests/fixture.rs", "tests/journey/**/*"],
      crates: ["onepipeline"],
    };
    save();
    assert.equal(run().status, 0);
    put(root, "nx.json", { namedInputs: { journey: ["{workspaceRoot}/tests/changed.rs"] } });
    assert.match(run().stderr, /auxiliary suite journey: inputs differ/);
    put(root, "nx.json", { namedInputs: { journey: ["journey"] } });
    assert.match(run().stderr, /auxiliary input cycle: journey -> journey/);
    put(root, "nx.json", { namedInputs: { journey: [{ fileset: "tests/bad.rs" }] } });
    assert.match(run().stderr, /unsupported auxiliary file input/);
    put(root, "nx.json", { namedInputs: { journey: ["^default"] } });
    assert.match(run().stderr, /unsupported auxiliary file input/);
    put(root, "nx.json", {
      namedInputs: {
        journey: [
          "{workspaceRoot}/tests/changed.rs",
          { env: "PARTITION" },
          { runtime: "node --version" },
        ],
      },
    });
    layout.auxiliarySuites.journey.paths = ["tests/changed.rs", "tests/journey/**/*"];
    save();
    assert.equal(run().status, 0);
    for (const input of [{ env: 7 }, { env: "X", fileset: "tests/data/**" }]) {
      put(root, "nx.json", { namedInputs: { journey: [input] } });
      assert.match(run().stderr, /unsupported auxiliary file input/);
    }
    for (const namedInputs of [null, [], { journey: "tests/fixture.rs" }]) {
      put(root, "nx.json", { namedInputs });
      assert.match(run().stderr, /invalid namedInputs/);
    }
    put(root, "nx.json", { namedInputs: { journey: ["{workspaceRoot}/tests/changed.rs"] } });
    for (const patch of [
      { tags: "lang:rust" },
      { sourceRoot: 7 },
      { targets: [] },
      { targets: { test: null } },
      { targets: { test: { inputs: "journey" } } },
    ]) {
      put(root, "tests/journey/project.json", { ...project, ...patch });
      assert.match(run().stderr, /invalid auxiliary project/);
    }
    put(root, "tests/journey/project.json", project);
    assert.equal(run().status, 0);
    project.targets.test.inputs.push("journey");
    put(root, "tests/journey/project.json", project);
    assert.equal(run().status, 0, "duplicated inputs do not change the fileset");
    project.targets.test.inputs = ["{workspaceRoot}/tests/other.rs"];
    put(root, "tests/journey/project.json", project);
    assert.match(run().stderr, /auxiliary suite journey: inputs differ/);
    layout.auxiliarySuites.journey.paths = ["tests/journey/**/*", "tests/other.rs"];
    save();
    assert.equal(run().status, 0);
    rmSync(join(root, "tests/journey/project.json"));
    assert.match(run().stderr, /auxiliary suite membership differs/);
    delete layout.auxiliarySuites.journey;
    save();
    assert.equal(run().status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("render CLI refuses missing or repeated generation markers without writing", () => {
  const root = scratch();
  try {
    for (const document of [
      "Design without marker\n",
      "<!-- BEGIN GENERATED MODULARIZATION -->\ntext\n<!-- BEGIN GENERATED MODULARIZATION -->\n",
    ]) {
      put(root, "docs/modularization-design.md", document);
      for (const args of [[], ["--write"]]) {
        const result = spawnSync(
          process.execPath,
          [resolve("scripts/modularization/render.mjs"), ...args],
          { cwd: root, encoding: "utf8" },
        );
        assert.equal(result.status, 1);
        assert.match(result.stderr, /expected exactly one generation marker/);
        assert.match(result.stderr, /restore exactly one generation marker/);
        assert.equal(readFileSync(join(root, "docs/modularization-design.md"), "utf8"), document);
      }
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("graph CLI transforms source edges, enforces command ownership and routing-only dispatch", () => {
  const root = scratch();
  const cli = resolve("scripts/modularization/graph.mjs");
  try {
    put(root, "src/main.rs", "fn main() {}\n");
    put(root, "src/status.rs", "use crate::record::Record;\nfn status() {}\n");
    put(root, "src/record.rs", "use crate::status::Status;\npub struct Record;\n");
    const layout = {
      schema: 2,
      generatedEdges: [],
      crates: {
        dispatcher: { files: ["src/main.rs"], deps: ["cmd-status", "record"] },
        "cmd-status": { files: ["src/status.rs"], deps: ["record"] },
        record: { files: ["src/record.rs"], deps: [] },
      },
      commands: { "src/status.rs": "cmd-status" },
      dispatcher: {
        crate: "dispatcher",
        routingFiles: ["src/main.rs"],
        routingFunctions: ["main"],
      },
      edges: {
        "status->record": { kind: "shared", fix: "Read the shared run record." },
        "record->status": {
          kind: "seam",
          fix: "Record owns the StatusProvider trait, installed by composition.",
          interface: { owner: "record", trait: "StatusProvider", adapter: "dispatcher" },
        },
      },
    };
    const run = () => {
      put(root, "layout.json", layout);
      return spawnSync(process.execPath, [cli, "layout.json", "graph.json"], {
        cwd: root,
        encoding: "utf8",
      });
    };
    assert.equal(run().status, 0);
    const report = JSON.parse(readFileSync(join(root, "graph.json"), "utf8"));
    assert.deepEqual(report.transformedCrates.record.deps, []);
    assert.deepEqual(report.transformedCrates.dispatcher.deps, ["cmd-status", "record"]);
    assert.ok(
      report.transformedEdges.some(
        (e) => e.from === "record" && e.to === "record" && e.via === "StatusProvider",
      ),
    );
    put(root, "src/main.rs", "fn main() {}\nfn read_lock() {}\n");
    layout.dispatcher.functionMoves = { "src/main.rs::read_lock": "record" };
    assert.equal(run().status, 0);
    assert.ok(
      JSON.parse(readFileSync(join(root, "graph.json"), "utf8")).transformedEdges.some(
        (edge) => edge.source === "src/main.rs::read_lock" && edge.to === "record",
      ),
    );
    delete layout.dispatcher.functionMoves;
    put(root, "src/main.rs", "fn main() {}\n");
    layout.edges["record->status"] = {
      kind: "move",
      fix: "Move command-owned rendering out of the record.",
      movedTo: "record",
    };
    assert.match(run().stderr, /move must land in command owner cmd-status/);
    layout.edges["record->status"].movedTo = "cmd-status";
    assert.equal(run().status, 0);
    const moved = JSON.parse(readFileSync(join(root, "graph.json"), "utf8"));
    assert.ok(
      moved.transformedEdges.some(
        (edge) =>
          edge.source === "record->status" &&
          edge.from === "cmd-status" &&
          edge.to === "cmd-status",
      ),
    );
    layout.edges["record->status"] = {
      kind: "seam",
      fix: "Restore the provider seam.",
      interface: { owner: "record", trait: "StatusProvider", adapter: "dispatcher" },
    };
    put(root, "src/record/child.rs", "pub struct Child;\n");
    layout.crates["cmd-status"].files.push("src/record/child.rs");
    assert.match(run().stderr, /module record: split ownership/);
    layout.crates["cmd-status"].files.pop();
    rmSync(join(root, "src/record"), { recursive: true });
    layout.crates.dispatcher.files.push("src/missing.rs");
    assert.match(run().stderr, /stale source file src\/missing.rs/);
    layout.crates.dispatcher.files.pop();
    const savePort = layout.edges["record->status"].interface;
    for (const moves of [
      null,
      [],
      "bad",
      { "src/main.rs::missing": "record" },
      { "src/main.rs::main": "absent" },
    ]) {
      layout.dispatcher.functionMoves = moves;
      assert.match(run().stderr, /invalid dispatcher functionMoves/);
    }
    delete layout.dispatcher.functionMoves;
    for (const port of [
      { ...savePort, owner: "absent" },
      { ...savePort, adapter: 4 },
      { ...savePort, trait: "bad-trait" },
    ]) {
      layout.edges["record->status"].interface = port;
      assert.match(run().stderr, /invalid interface record->status/);
    }
    layout.edges["record->status"].interface = savePort;
    layout.edges["main->status"] = { kind: "shared", fix: "Stale edge must fail." };
    assert.match(run().stderr, /stale source edge main->status/);
    delete layout.edges["main->status"];
    layout.edges["status->record"].interface = {
      owner: "absent",
      trait: "UnusedProvider",
      adapter: "dispatcher",
    };
    assert.match(run().stderr, /invalid interface status->record/);
    delete layout.edges["status->record"].interface;
    layout.edges["record->status"] = {
      kind: "shared",
      fix: "Deliberately leave the import in place.",
    };
    assert.match(run().stderr, /cmd-status -> record -> cmd-status/);
    layout.edges["record->status"] = {
      kind: "seam",
      fix: "Inject provider.",
      interface: { owner: "record", trait: "StatusProvider", adapter: "dispatcher" },
    };
    layout.crates.record.files.push("src/status.rs");
    layout.crates["cmd-status"].files = [];
    assert.match(run().stderr, /command module src\/status.rs: expected command crate cmd-status/);
    layout.crates.record.files = ["src/record.rs"];
    layout.crates["cmd-status"].files = ["src/status.rs"];
    put(root, "src/main.rs", "fn main() {}\nfn perform_business_logic() {}\n");
    assert.match(
      run().stderr,
      /dispatcher holds non-routing function src\/main.rs::perform_business_logic/,
    );
    put(root, "src/main.rs", "fn main() {}\n");
    layout.crates.dispatcher.files.push("src/record.rs");
    layout.crates.record.files = [];
    assert.match(run().stderr, /dispatcher holds non-routing module src\/record.rs/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("command lock projection CLI matches Nx on transitive source, resolved lock and toolchain commits", {
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
  const cli = resolve("scripts/modularization/command-replay.mjs");
  const lock = (version) =>
    `version = 4\n\n[[package]]\nname = "record-lib"\nversion = "${version}"\n\n[[package]]\nname = "reader-lib"\nversion = "1.0.0"\ndependencies = ["record-lib"]\n\n[[package]]\nname = "template-lib"\nversion = "1.0.0"\n`;
  try {
    symlinkSync(resolve("node_modules"), join(root, "node_modules"), "junction");
    put(root, ".gitignore", "node_modules\n.nx\n");
    put(root, "package.json", { private: true });
    put(root, "nx.json", { namedInputs: { default: ["{projectRoot}/**/*"] } });
    const crates = {
      record: {
        paths: ["record/**", "resolved/record/**"],
        deps: [],
        external: ["record-lib"],
        toolchain: true,
      },
      reader: {
        paths: ["reader/**", "resolved/reader/**"],
        deps: ["record"],
        external: ["reader-lib"],
        toolchain: true,
      },
      status: {
        paths: ["status/**", "resolved/status/**"],
        deps: ["reader"],
        external: [],
        toolchain: true,
      },
      template: {
        paths: ["template/**", "resolved/template/**"],
        deps: [],
        external: ["template-lib"],
        toolchain: true,
      },
    };
    const suites = {
      "status-e2e": { paths: ["status-e2e/**"], crates: ["status"] },
      "template-e2e": { paths: ["template-e2e/**"], crates: ["template"] },
    };
    const mapping = compileMapping({
      crates: Object.fromEntries(
        Object.entries(crates).map(([name, c]) => [name, { paths: c.paths, deps: c.deps }]),
      ),
      suites,
    });
    for (const [name, item] of [...Object.entries(crates), ...Object.entries(suites)]) {
      put(root, `${name}/project.json`, {
        name,
        root: name,
        implicitDependencies: item.deps ?? item.crates,
        targets: {
          test: {
            executor: "nx:noop",
            inputs: ["default", "^default", `{workspaceRoot}/resolved/${name}/**`],
          },
        },
      });
      put(root, `${name}/source.rs`, "initial");
      if (crates[name]) {
        put(root, `resolved/${name}/project.json`, {
          name: `resolved-${name}`,
          root: `resolved/${name}`,
        });
        // Nx's affected graph follows these explicit generated-input projects.
        const project = JSON.parse(readFileSync(join(root, `${name}/project.json`)));
        project.implicitDependencies.push(`resolved-${name}`);
        put(root, `${name}/project.json`, project);
        put(root, `resolved/${name}/lock.json`, "initial");
        put(root, `resolved/${name}/toolchain.json`, "initial");
      }
    }
    put(root, "Cargo.lock", lock("1.0.0"));
    put(root, "rust-toolchain.toml", '[toolchain]\nchannel = "1.90.0"\n');
    git("init", "-b", "main");
    git("config", "user.email", "fixture@example.invalid");
    git("config", "user.name", "Fixture");
    git("add", ".");
    git("commit", "-m", "test: initial command graph");
    // Projection inputs live outside the workspace: only materialized effective inputs join its graph.
    const inputs = scratch();
    try {
      put(
        inputs,
        "crates.json",
        Object.fromEntries(
          Object.entries(crates).map(([name, c]) => [
            name,
            { external: c.external, toolchain: c.toolchain },
          ]),
        ),
      );
      put(inputs, "before.lock", lock("1.0.0"));
      put(inputs, "after.lock", lock("1.1.0"));
      put(inputs, "before-tool", '[toolchain]\nchannel = "1.90.0"\n');
      put(inputs, "after-tool", '[toolchain]\nchannel = "1.91.0"\n');
      const project = (afterLock, afterTool) => {
        const result = spawnSync(
          process.execPath,
          [
            cli,
            "--resolve",
            join(inputs, "crates.json"),
            join(inputs, "before.lock"),
            join(inputs, afterLock),
            join(inputs, "before-tool"),
            join(inputs, afterTool),
            join(inputs, "changes.json"),
          ],
          { encoding: "utf8" },
        );
        assert.equal(result.status, 0, result.stderr);
        return JSON.parse(readFileSync(join(inputs, "changes.json"), "utf8"));
      };
      const cases = [
        ["status/source.rs"],
        ["record/source.rs"],
        project("after.lock", "before-tool"),
        project("before.lock", "after-tool"),
      ];
      assert.deepEqual(cases[2], ["resolved/record/lock.json", "resolved/reader/lock.json"]);
      assert.equal(cases[3].length, 4);
      for (const [index, projected] of cases.entries()) {
        const paths = [...projected];
        if (index === 2) {
          put(root, "Cargo.lock", lock("1.1.0"));
          paths.push("Cargo.lock");
        }
        if (index === 3) {
          put(root, "rust-toolchain.toml", '[toolchain]\nchannel = "1.91.0"\n');
          paths.push("rust-toolchain.toml");
        }
        for (const path of projected)
          put(root, path, `${readFileSync(join(root, path), "utf8")}changed\n`);
        git("add", ".");
        git("commit", "-m", "test: projected input change");
        const nx = JSON.parse(
          execFileSync(
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
          ),
        )
          .filter((name) => Object.hasOwn(suites, name))
          .sort();
        assert.deepEqual(affectedSuites(mapping, paths).affected.sort(), nx, paths.join(","));
        assert.ok(nx.includes("status-e2e"));
        assert.equal(
          nx.includes("template-e2e"),
          paths.some((p) => p.includes("toolchain")),
        );
      }
      put(inputs, "after.lock", lock("1.0.0"));
      assert.deepEqual(project("after.lock", "before-tool"), []);
      put(inputs, "after.lock", lock("1.0.0").replace('"record-lib"]', '"missing-lib"]'));
      const refused = spawnSync(
        process.execPath,
        [
          cli,
          "--resolve",
          join(inputs, "crates.json"),
          join(inputs, "before.lock"),
          join(inputs, "after.lock"),
          join(inputs, "before-tool"),
          join(inputs, "before-tool"),
          join(inputs, "changes.json"),
        ],
        { encoding: "utf8" },
      );
      assert.equal(refused.status, 1);
      assert.match(refused.stderr, /unresolved lock dependency missing-lib/);
      put(inputs, "after.lock", lock("1.1.0"));
      assert.equal(project("after.lock", "before-tool").length, 2);
    } finally {
      rmSync(inputs, { recursive: true, force: true });
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("command replay CLI retains every individual test and renders a fresh evidence appendix", {
  timeout: 180000,
}, async () => {
  const root = scratch();
  try {
    const { commandAppendix, COMMAND_MARKER } = await import("./command-report.mjs");
    const { commandMapping } = await import("./command-replay.mjs");
    const layout = JSON.parse(readFileSync("scripts/modularization/layout.json", "utf8"));
    const catalog = JSON.parse(readFileSync("scripts/modularization/command-tests.json", "utf8"));
    const result = spawnSync(
      process.execPath,
      ["scripts/modularization/command-replay.mjs", join(root, "report.json")],
      { encoding: "utf8" },
    );
    assert.equal(result.status, 0, result.stderr);
    const report = JSON.parse(readFileSync(join(root, "report.json"), "utf8"));
    assert.equal(report.rows.length, 144);
    const declared = [...readFileSync("tests/e2e/main.rs", "utf8").matchAll(/^mod (\w+);/gm)].map(
      (m) => m[1],
    );
    const count = declared.reduce(
      (sum, module) =>
        sum +
        [
          ...codeOnly(readFileSync(`tests/e2e/${module}.rs`, "utf8")).matchAll(
            /#\s*\[\s*test\s*\]/g,
          ),
        ].length,
      0,
    );
    assert.equal(report.tests, count);
    const all = Object.values(report.mapping.suites).flatMap((suite) => suite.tests);
    assert.equal(new Set(all).size, count);
    assert.equal(all.length, count);
    assert.ok(Object.values(report.mapping.suites).every((suite) => suite.tests.length > 0));
    for (const [name, suite] of Object.entries(report.mapping.suites))
      if (name !== "cross-command") {
        assert.deepEqual(suite.crates, [`runner-${name}`]);
        assert.deepEqual(report.mapping.crates[`runner-${name}`].deps, [name]);
      }
    const compiled = compileMapping(report.mapping.selection);
    for (const row of report.rows)
      assert.deepEqual(affectedSuites(compiled, row.effectiveFiles).affected, row.affected);
    assert.equal(report.platforms.local.secondsPerRun.toFixed(0), "36620");
    assert.equal(report.platforms.linux.secondsPerRun.toFixed(0), "10409");
    assert.ok(report.platforms.windows.missingModules.length > 0);
    assert.equal(report.migration.at(-1).suiteSkip, report.platforms.linux.suiteSkip);
    const doc = readFileSync("docs/modularization-command-replay.md", "utf8");
    assert.equal(doc.split(COMMAND_MARKER).length, 2);
    assert.equal(
      `${COMMAND_MARKER}${doc.split(COMMAND_MARKER)[1]}`,
      commandAppendix(report, layout, catalog),
    );
    const broken = structuredClone(catalog);
    const first = Object.keys(broken.tests)[0];
    broken.tests[first].suite = "cmd-not-a-command";
    assert.throws(() => commandMapping(layout, broken), /invalid test owner/);
    const withCommands = Object.keys(catalog.tests).find(
      (name) => catalog.tests[name].commands.length > 0,
    );
    broken.tests[withCommands] = { ...catalog.tests[withCommands], commands: [] };
    assert.throws(() => commandMapping(layout, broken), /test command attribution drift/);
    broken.tests[withCommands] = catalog.tests[withCommands];
    broken.tests[first] = catalog.tests[first];
    const badArguments = spawnSync(
      process.execPath,
      ["scripts/modularization/command-replay.mjs", "--bogus"],
      { encoding: "utf8" },
    );
    assert.equal(badArguments.status, 1);
    assert.match(badArguments.stderr, /usage: command-replay/);
    broken.typo = true;
    assert.throws(() => commandMapping(layout, broken), /unknown command catalog field/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("target graph applies named source moves and refuses a broken extraction then recovers", async () => {
  const { targetGraph } = await import("./target-graph.mjs");
  const base = JSON.parse(readFileSync("scripts/modularization/layout.json", "utf8"));
  const moves = JSON.parse(readFileSync("scripts/modularization/target-moves.json", "utf8"));
  const mapped = targetGraph(base, moves);
  const graph = compileMapping({
    crates: Object.fromEntries(
      Object.entries(mapped.crates).map(([name, crate]) => [
        name,
        { paths: crate.files, deps: crate.deps },
      ]),
    ),
    suites: Object.fromEntries(
      Object.keys(mapped.crates)
        .filter((name) => name.startsWith("cmd-"))
        .map((name) => [name, { paths: [], crates: [name] }]),
    ),
  });
  for (const [name, suite] of Object.entries(graph.suites))
    assert.deepEqual(
      [...suite.crates].filter((crate) => crate.startsWith("cmd-")),
      [name],
    );
  const broken = structuredClone(moves);
  broken.commandSlices[0].symbols.push("nonexistent_renderer");
  assert.throws(() => targetGraph(base, broken), /missing extraction symbol.*nonexistent_renderer/);
  broken.commandSlices[0].symbols.pop();
  assert.deepEqual(targetGraph(base, broken).crates, mapped.crates);
  broken.wholeModules[0].file = "src/branchname.rs";
  broken.wholeModules[0].symbols = ["render"];
  assert.throws(() => targetGraph(base, broken), /unmodeled production dependency/);
  broken.wholeModules = structuredClone(moves.wholeModules);
  broken.renames = {};
  assert.throws(() => targetGraph(base, broken), /actual CLI routes exactly/);
  broken.renames = true;
  assert.throws(() => targetGraph(base, broken), /invalid target field renames/);
  broken.renames = structuredClone(moves.renames);
  broken.edgeMoves[1].replacement = "onepipeline";
  assert.throws(() => targetGraph(base, broken), /cycle.*onepipeline|onepipeline.*->/);
  const root = scratch();
  try {
    const refused = spawnSync(
      process.execPath,
      [resolve("scripts/modularization/target-graph.mjs"), "out.json", "extra"],
      { cwd: root, encoding: "utf8" },
    );
    assert.equal(refused.status, 1);
    assert.match(refused.stderr, /usage: target-graph.mjs/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("decomposition CLI separates setup, preserves live ownership, derives argv slots and recovers from invalid inputs", async () => {
  const root = scratch();
  try {
    const { expandCatalog } = await import("./catalog.mjs");
    const config = {
      schema: 1,
      paths: { start: "cmd-start", status: "cmd-status", stop: "cmd-stop", watch: "cmd-watch" },
    };
    put(root, "scripts/modularization/cli-commands.json", config);
    put(
      root,
      "src/cli.rs",
      "pub enum Command {\n    Start(StartArgs),\n    Status(StatusArgs),\n    Stop(StopArgs),\n    Watch(WatchArgs),\n}\n",
    );
    const source = `use crate::harness::World;
#[test] fn setup_then_status() { let w = World; w.run(&["start"]); w.run(&["status"]); }
#[test] fn stop_while_running() { let w = World; w.run(&["start"]); w.run(&["stop"]); w.wait(); }
#[test] fn dynamic_family() { let w = World; let args = unresolvable(); w.run(&args); }
#[test] fn support_only() { assert!(true); }
#[test] fn public_view() { let _ = onepipeline::verbs::status(root, None); }
#[test] fn simple_view() { let w = World; w.run(&["status"]); }
#[test] fn causal_control() { let w = World; w.run(&["start"]); w.run(&["stop"]); w.until(|| true); }
#[test] fn observer_pair() { let w = World; w.run(&["start"]); w.cmd(&["watch"]).spawn(); }
#[test] fn generic_args() { let w = World; let args = unresolvable(); w.run(&args); }
#[test] fn external_program() { let w = World; w.cmd_on(program, &["leave-a-descendant"]).spawn(); }
#[test] fn explicit_binary() { Command::new(env!("CARGO_BIN_EXE_onepipeline")).args(["unknown"]).output(); }
`;
    put(
      root,
      "tests/e2e/harness.rs",
      "pub struct World; impl World { pub fn run(&self, args: &[&str]) {} pub fn cmd(&self, args: &[&str]) {} pub fn cmd_on(&self, program: &Path, args: &[&str]) {} }",
    );
    put(root, "tests/e2e/example.rs", source);
    const { sourceHash } = await import("./attribution.mjs");
    const { functions } = await import("./rust-syntax.mjs");
    const writeAudit = (text) =>
      put(root, "scripts/modularization/command-attributions.json", {
        schema: 1,
        functions: Object.fromEntries(
          ["dynamic_family", "generic_args", "explicit_binary"].map((name) => {
            const fn = functions(text).find((entry) => entry.name === name);
            return [
              `example::${name}`,
              {
                sha256: sourceHash(text.slice(fn.start, fn.end)),
                moduleSha256: sourceHash(text),
                commands: [],
                kind: "dispatch",
                reason:
                  "This fixture explicitly exercises the parser refusal boundary with dynamic argv.",
              },
            ];
          }),
        ),
      });
    writeAudit(source);
    const baseline = {
      schema: 1,
      tests: {
        "example::explicit_binary": {
          suite: "cross-command",
          commands: [],
          reason: "compiled parser refusal",
        },
        "example::external_program": {
          suite: "cross-command",
          commands: [],
          reason: "real subprocess support boundary",
        },
        "example::support_only": {
          suite: "cross-command",
          commands: [],
          reason: "fixture support",
        },
        "example::public_view": {
          suite: "cross-command",
          commands: ["cmd-status"],
          reason: "library area fallback",
        },
        "example::simple_view": {
          suite: "cmd-status",
          commands: ["cmd-status"],
          reason: "one real command",
        },
        "example::causal_control": {
          suite: "cross-command",
          commands: ["cmd-start", "cmd-stop"],
          reason: "live control",
        },
        "example::observer_pair": {
          suite: "cross-command",
          commands: ["cmd-start", "cmd-watch"],
          reason: "armed watcher",
        },
        "example::generic_args": {
          suite: "cmd-status",
          commands: ["cmd-status"],
          reason: "unresolved actual argv",
        },
        "example::dynamic_family": {
          suite: "cross-command",
          commands: ["cmd-template", "cmd-start"],
          reason: "legacy profile bucket with dynamic argv",
        },
        "example::setup_then_status": {
          suite: "cross-command",
          commands: ["cmd-start", "cmd-status"],
          reason: "setup and read",
        },
        "example::stop_while_running": {
          suite: "cross-command",
          commands: ["cmd-start", "cmd-stop"],
          reason: "live stop",
        },
      },
    };
    put(root, "scripts/modularization/layout.json", {
      schema: 1,
      crates: {
        "cmd-start": { files: ["src/cli.rs"], deps: [] },
        "cmd-status": { files: [], deps: [] },
        "cmd-stop": { files: [], deps: [] },
        "cmd-template": { files: [], deps: [] },
      },
      edges: {},
    });
    const baselinePath = "scripts/modularization/command-tests.json";
    const outputPath = "scripts/modularization/command-tests-target.json";
    put(root, baselinePath, baseline);
    const cli = resolve("scripts/modularization/decompose.mjs");
    const run = (...args) =>
      spawnSync(process.execPath, [cli, ...args], { cwd: root, encoding: "utf8" });
    const expectFailure = (pattern, ...args) => {
      const result = run(...args);
      assert.equal(result.status, 1);
      assert.match(result.stderr, pattern);
    };
    const good = run();
    assert.equal(good.status, 0, good.stderr);
    assert.match(good.stdout, /11 original tests classified/);
    const result = expandCatalog(JSON.parse(readFileSync(join(root, outputPath), "utf8")));
    assert.equal(result.dispositions["example::setup_then_status"].kind, "decompose");
    assert.equal(result.dispositions["example::dynamic_family"].kind, "dispatch");
    assert.equal(result.tests["example::dynamic_family"].suite, "full-dispatch");
    assert.deepEqual(result.tests["example::dynamic_family"].commands, []);
    assert.equal(result.dispositions["example::support_only"].kind, "support");
    assert.equal(result.dispositions["example::external_program"].kind, "support");
    assert.equal(result.dispositions["example::explicit_binary"].kind, "dispatch");
    assert.equal(result.dispositions["example::public_view"].kind, "library");
    assert.equal(result.dispositions["example::simple_view"].kind, "isolated");
    assert.equal(result.dispositions["example::generic_args"].kind, "dispatch");
    assert.equal(result.dispositions["example::causal_control"].reason, "live-control");
    assert.equal(result.dispositions["example::observer_pair"].reason, "live-observer");
    assert.equal(result.dispositions["example::stop_while_running"].kind, "integration");
    assert.deepEqual(result.dispositions["example::setup_then_status"].originalCommands, [
      "cmd-start",
      "cmd-status",
    ]);
    for (const id of Object.keys(baseline.tests))
      assert.equal(
        Object.values(result.tests)
          .filter((t) => t.original === id)
          .reduce((n, t) => n + t.share, 0),
        1,
      );
    const heldAudit = readFileSync(
      join(root, "scripts/modularization/command-attributions.json"),
      "utf8",
    );
    put(root, "scripts/modularization/command-attributions.json", { schema: 1, functions: {} });
    expectFailure(/unreviewed dispatcher attribution/);
    for (const bad of [null, { schema: 2, functions: {} }, { schema: 1, functions: [] }]) {
      put(root, "scripts/modularization/command-attributions.json", bad);
      expectFailure(/invalid command attribution audit/);
    }
    const parsedAudit = JSON.parse(heldAudit);
    const auditId = "example::dynamic_family";
    for (const patch of [
      { commands: ["cmd-imaginary"] },
      { forward: null },
      { boundary: "unknown" },
      { sha256: "a".repeat(64) },
      { typo: true },
      { kind: "integration" },
    ]) {
      const changed = structuredClone(parsedAudit);
      Object.assign(changed.functions[auditId], patch);
      put(root, "scripts/modularization/command-attributions.json", changed);
      expectFailure(
        /invalid command attribution|invalid attribution|stale attribution|incompatible commands/,
      );
    }
    put(root, "scripts/modularization/command-attributions.json", heldAudit);
    expectFailure(/usage: decompose.mjs/, "extra");
    for (const bad of [
      { ...baseline, typo: true },
      { schema: 1, tests: 42 },
    ]) {
      put(root, baselinePath, bad);
      expectFailure(/invalid baseline test catalog/);
    }
    put(root, baselinePath, {
      ...baseline,
      tests: {
        ...baseline.tests,
        "example::dynamic_family": {
          ...baseline.tests["example::dynamic_family"],
          commands: ["cmd-imaginary"],
        },
      },
    });
    expectFailure(/unmapped baseline command crate/);
    put(root, baselinePath, baseline);
    for (const bad of [
      { ...config, schema: 2 },
      { ...config, typo: true },
      { schema: 1, paths: 42 },
      { schema: 1, paths: { start: "missing" } },
    ]) {
      put(root, "scripts/modularization/cli-commands.json", bad);
      expectFailure(/invalid CLI command mapping/);
    }
    put(root, "scripts/modularization/cli-commands.json", {
      schema: 1,
      paths: { ...config.paths, unknown: "cmd-start" },
    });
    expectFailure(/CLI path mapping differs/);
    put(root, "scripts/modularization/cli-commands.json", {
      schema: 1,
      paths: { ...config.paths, status: "cmd-unknown" },
    });
    expectFailure(/unmapped CLI command crate/);
    put(root, "scripts/modularization/cli-commands.json", config);
    put(
      root,
      "tests/e2e/harness.rs",
      "pub struct World; impl World { pub fn run(&self, cwd: &Path, args: &[&str]) {} pub fn cmd(&self, args: &[&str]) {} pub fn cmd_on(&self, program: &Path, args: &[&str]) {} }",
    );
    const changedSource = source.replaceAll("w.run(&[", "w.run(path, &[");
    put(root, "tests/e2e/example.rs", changedSource);
    expectFailure(/stale attribution source/);
    writeAudit(changedSource);
    assert.equal(run().status, 0);
    assert.deepEqual(
      expandCatalog(JSON.parse(readFileSync(join(root, outputPath), "utf8"))).dispositions,
      result.dispositions,
    );
    rmSync(join(root, outputPath));
    mkdirSync(join(root, outputPath));
    expectFailure(/EISDIR/);
    rmSync(join(root, outputPath), { recursive: true });
    assert.equal(run().status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

// expandCatalog is the public in-process input boundary; these assertions drive that parser directly.
test("catalog expansion refuses malformed records, inherited reason names and duplicate owners", async () => {
  const { expandCatalog, baselineHash } = await import("./catalog.mjs");
  const good = {
    schema: 4,
    baselineSha256: baselineHash({ schema: 1, tests: {} }),
    dispositions: {
      "example::works": {
        kind: "decompose",
        originalCommands: ["cmd-start", "cmd-status"],
        reason: "command-phases",
      },
    },
  };
  assert.equal(Object.keys(expandCatalog(JSON.parse(JSON.stringify(good))).tests).length, 2);
  for (const dispositions of [true, 42, [], null])
    assert.throws(() => expandCatalog({ ...good, dispositions }), /invalid decomposition catalog/);
  assert.throws(() => expandCatalog({ ...good, schema: 3 }), /invalid decomposition catalog/);
  for (const reason of ["constructor", "toString", 42]) {
    const broken = structuredClone(good);
    broken.dispositions["example::works"].reason = reason;
    assert.throws(() => expandCatalog(broken), /invalid disposition/);
  }
  for (const originalCommands of [["cmd-start", "cmd-start"], []]) {
    const broken = structuredClone(good);
    broken.dispositions["example::works"].originalCommands = originalCommands;
    assert.throws(() => expandCatalog(broken), /no unique owners|incompatible commands/);
  }
  for (const [kind, originalCommands] of [
    ["isolated", ["cmd-start", "cmd-status"]],
    ["decompose", ["cmd-start"]],
    ["integration", []],
    ["support", ["cmd-start"]],
    ["library", ["cmd-start"]],
  ]) {
    const broken = structuredClone(good);
    Object.assign(broken.dispositions["example::works"], { kind, originalCommands });
    assert.throws(() => expandCatalog(broken), /incompatible commands/);
  }
  for (const [kind, originalCommands] of [
    ["isolated", ["cmd-start"]],
    ["integration", ["cmd-start", "cmd-status"]],
    ["support", []],
    ["library", []],
  ]) {
    const valid = structuredClone(good);
    Object.assign(valid.dispositions["example::works"], { kind, originalCommands });
    assert.equal(Object.keys(expandCatalog(valid).tests).length, 1);
  }
  const broken = structuredClone(good);
  broken.dispositions["example::works"].typo = true;
  assert.throws(() => expandCatalog(broken), /invalid disposition/);
  assert.equal(Object.keys(expandCatalog(good).tests).length, 2);
});

test("inventory and documentation CLIs preserve prefixes and refuse missing/duplicate markers", {
  timeout: 180000,
}, () => {
  const root = scratch();
  try {
    execFileSync("git", ["clone", "--quiet", "--shared", "--no-hardlinks", ".", root], {
      stdio: "pipe",
    });
    const git = (...args) => execFileSync("git", args, { cwd: root, encoding: "utf8" });
    git(
      "update-ref",
      "refs/remotes/origin/main",
      execFileSync("git", ["rev-parse", "origin/main"], { encoding: "utf8" }).trim(),
    );
    // Run the actual revised CLI in the scratch checkout, including its source/schema inputs.
    for (const name of [
      "command-replay.mjs",
      "catalog.mjs",
      "rust-syntax.mjs",
      "phase-inputs.mjs",
      "attribution.mjs",
      "command-attributions.json",
      "source-projection.mjs",
      "manifest-projection.mjs",
      "cli-commands.json",
      "graph.mjs",
      "layout.json",
    ])
      put(
        root,
        `scripts/modularization/${name}`,
        readFileSync(`scripts/modularization/${name}`, "utf8"),
      );
    const run = (...args) =>
      spawnSync(process.execPath, ["scripts/modularization/command-replay.mjs", ...args], {
        cwd: root,
        encoding: "utf8",
      });
    const inventoried = run("--inventory");
    assert.equal(inventoried.status, 0, inventoried.stderr);
    assert.deepEqual(
      JSON.parse(readFileSync(join(root, "scripts/modularization/command-tests.json"), "utf8")),
      JSON.parse(readFileSync("scripts/modularization/command-tests.json", "utf8")),
    );
    const doc = "docs/modularization-command-replay.md";
    const marker = "<!-- BEGIN COMMAND REPLAY -->";
    put(root, doc, "prefix retained exactly\n");
    assert.match(run("out.json", "--write-doc").stderr, /expected one command replay marker/);
    put(root, doc, `prefix retained exactly\n${marker}\n${marker}\n`);
    assert.match(run("out.json", "--write-doc").stderr, /expected one command replay marker/);
    put(root, doc, `prefix retained exactly\n${marker}\n`);
    const repaired = run("out.json", "--write-doc");
    assert.equal(repaired.status, 0, repaired.stderr);
    assert.ok(
      readFileSync(join(root, doc), "utf8").startsWith(`prefix retained exactly\n${marker}\n`),
    );
    assert.equal(JSON.parse(readFileSync(join(root, "out.json"), "utf8")).rows.length, 144);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("target replay CLI covers 144 PRs and every original assertion in fresh target documentation", {
  timeout: 600000,
}, async () => {
  const root = scratch();
  try {
    const { decompose, expandCatalog } = await import("./decompose.mjs");
    const { targetAppendix, TARGET_MARKER } = await import("./target-report.mjs");
    const baseline = JSON.parse(readFileSync("scripts/modularization/command-tests.json", "utf8"));
    const catalog = JSON.parse(
      readFileSync("scripts/modularization/command-tests-target.json", "utf8"),
    );
    assert.deepEqual(catalog, decompose(baseline));
    const output = join(root, "target.json");
    const run = spawnSync(process.execPath, ["scripts/modularization/target-replay.mjs", output], {
      encoding: "utf8",
    });
    assert.equal(run.status, 0, run.stderr);
    const evidence = JSON.parse(readFileSync(output, "utf8"));
    assert.equal(evidence.result.rows.length, 144);
    assert.equal(evidence.result.originalTests, 1922);
    assert.equal(evidence.result.tests, Object.keys(evidence.catalog.tests).length);
    const expanded = expandCatalog(catalog);
    for (const id of Object.keys(baseline.tests)) {
      const pieces = Object.values(expanded.tests).filter((test) => test.original === id);
      const expected =
        catalog.dispositions[id].kind === "decompose"
          ? catalog.dispositions[id].originalCommands.length
          : 1;
      assert.equal(pieces.length, expected, id);
      assert.ok(
        pieces.every((piece) => piece.share === 1 / expected),
        id,
      );
    }
    assert.equal(
      evidence.result.migration.at(-1).suiteSkip,
      evidence.result.platforms.linux.suiteSkip,
    );
    const doc = readFileSync("docs/modularization-target-layout.md", "utf8");
    assert.equal(doc.split(TARGET_MARKER).length, 2);
    assert.match(doc, /\| linux \| 6\.61% \| 2\.55% \|/);
    assert.match(doc, /frees at least one suite on \*\*12 PRs\*\*/);
    assert.match(doc, /removes \*\*104 suite runs\*\*/);
    assert.equal(`${TARGET_MARKER}${doc.split(TARGET_MARKER)[1]}`, targetAppendix(evidence));
    const agentsRow = doc.split("\n").find((line) => line.startsWith("| agents | cmd-agents |"));
    assert(agentsRow);
    assert.match(
      agentsRow,
      /agents::every_dispatch_of_a_run_is_visible_through_its_pointer_file_with_nothing_configured \(cross-command\)/,
    );
    assert.match(doc, /Exception suite \| Complete runner crate closure/);
    assert.match(doc, /Source-backed argv and boundary audit/);
    symlinkSync(resolve("src"), join(root, "src"), "dir");
    mkdirSync(join(root, "tests"), { recursive: true });
    symlinkSync(resolve("tests/e2e"), join(root, "tests/e2e"), "dir");
    for (const name of [
      "command-tests.json",
      "cli-commands.json",
      "command-tests-target.json",
      "target-moves.json",
    ])
      put(
        root,
        `scripts/modularization/${name}`,
        readFileSync(`scripts/modularization/${name}`, "utf8"),
      );
    const renderer = resolve("scripts/modularization/target-report.mjs");
    const render = () =>
      spawnSync(process.execPath, [renderer, output], { cwd: root, encoding: "utf8" });
    const targetDoc = "docs/modularization-target-layout.md";
    put(root, targetDoc, "prefix\n");
    assert.match(render().stderr, /expected one target layout marker/);
    put(root, targetDoc, `prefix\n${TARGET_MARKER}\n${TARGET_MARKER}\n`);
    assert.match(render().stderr, /expected one target layout marker/);
    put(root, targetDoc, `prefix\n${TARGET_MARKER}\n`);
    evidence.typo = true;
    put(root, "target.json", evidence);
    assert.match(render().stderr, /unknown target evidence field/);
    delete evidence.typo;
    const oldFile = evidence.moves.extractions[0].file;
    evidence.moves.extractions[0].file = "../../outside.rs";
    put(root, "target.json", evidence);
    assert.match(render().stderr, /differs from the current reviewed moves/);
    evidence.moves.extractions[0].file = oldFile;
    evidence.layout.crates["cmd-start"].files.push("src/lib.rs");
    put(root, "target.json", evidence);
    assert.match(render().stderr, /layout\/selection differs/);
    evidence.layout.crates["cmd-start"].files.pop();
    const deps = evidence.result.mapping.selection.crates["cmd-status"].deps;
    evidence.result.mapping.selection.crates["cmd-status"].deps = [];
    put(root, "target.json", evidence);
    assert.match(render().stderr, /layout\/selection differs/);
    evidence.result.mapping.selection.crates["cmd-status"].deps = deps;
    const affected = evidence.result.rows[0].affected;
    evidence.result.rows[0].affected = [];
    put(root, "target.json", evidence);
    assert.match(render().stderr, /row selection differs/);
    evidence.result.rows[0].affected = affected;
    const seconds = evidence.result.platforms.linux.secondsSkip;
    evidence.result.platforms.linux.secondsSkip = 2;
    put(root, "target.json", evidence);
    assert.match(render().stderr, /invalid target platform data/);
    evidence.result.platforms.linux.secondsSkip = seconds;
    for (const key of ["originalTests", "tests"]) {
      evidence.result[key]--;
      put(root, "target.json", evidence);
      assert.match(render().stderr, /target test\/phase count mismatch/);
      evidence.result[key]++;
    }
    put(root, "target.json", evidence);
    const repaired = render();
    assert.equal(repaired.status, 0, repaired.stderr);
    assert.equal(
      readFileSync(join(root, targetDoc), "utf8"),
      `prefix\n${targetAppendix(evidence)}`,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("resolution CLI refuses malformed maps, lock identities and compiler pins, then recovers", () => {
  const root = scratch();
  try {
    const cli = resolve("scripts/modularization/command-replay.mjs");
    const crates = { record: { external: ["record-lib"], toolchain: true } };
    const lock = 'version = 4\n[[package]]\nname = "record-lib"\nversion = "1.0.0"\n';
    const pin = '[toolchain]\nchannel = "1.90.0"\n';
    put(root, "crates.json", crates);
    put(root, "before.lock", lock);
    put(root, "after.lock", lock);
    put(root, "before-tool", pin);
    put(root, "after-tool", pin);
    const run = () =>
      spawnSync(
        process.execPath,
        [
          cli,
          "--resolve",
          ...[
            "crates.json",
            "before.lock",
            "after.lock",
            "before-tool",
            "after-tool",
            "out.json",
          ].map((file) => join(root, file)),
        ],
        { encoding: "utf8" },
      );
    const failure = (pattern) => {
      const result = run();
      assert.equal(result.status, 1, result.stdout);
      assert.match(result.stderr, pattern);
    };
    assert.equal(run().status, 0);
    assert.deepEqual(JSON.parse(readFileSync(join(root, "out.json"), "utf8")), []);
    for (const bad of [null, true, 42, []]) {
      put(root, "crates.json", JSON.stringify(bad));
      failure(/resolution configuration must be an object/);
    }
    for (const entry of [
      { external: [], toolchain: true, typo: true },
      { external: 42, toolchain: true },
      { external: [], toolchain: "true" },
    ]) {
      put(root, "crates.json", { record: entry });
      failure(/invalid resolution inputs/);
    }
    put(root, "crates.json", { record: { external: ["missing"], toolchain: true } });
    failure(/unresolved external root/);
    put(root, "crates.json", crates);
    for (const [bad, pattern] of [
      ["version = 4\n", /no packages/],
      ['[[package]]\nname = "record-lib"\n', /needs name and version/],
      [lock + lock.slice(lock.indexOf("[[package]]")), /duplicate lock package identity/],
    ]) {
      put(root, "after.lock", bad);
      failure(pattern);
    }
    for (const dependencies of ['"not an array"', '["record-lib", 42]']) {
      put(root, "after.lock", `${lock}dependencies = ${dependencies}\n`);
      failure(/lock package dependencies must be a string array/);
    }
    put(root, "after.lock", lock);
    for (const bad of [
      '[toolchain]\nchannel = "stable"\nprofile = 42\n',
      '[toolchain]\nchannel = 42\npath = "/toolchain"\n',
      '[toolchain]\nchannel = "stable"\npath = 42\n',
      '[toolchain]\nchannel = "stable"\nprofile = "other"\n',
    ]) {
      put(root, "after-tool", bad);
      failure(/invalid toolchain/);
    }
    put(root, "after-tool", pin);
    assert.equal(run().status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

// The report's source-intent reader is a public in-process file boundary, exercised against real test source.
test("test intent comes from the existing journey and refuses path-like ids", async () => {
  const { intentFor } = await import("./target-report.mjs");
  assert.match(
    intentFor("adoption::the_three_release_kinds_reach_a_planner_through_the_shipped_profile"),
    /The three kinds this crate emits reach a planner through the \*\*shipped profile with no filter change/,
  );
  assert.throws(() => intentFor("../outside::read"), /invalid test intent id/);
  assert.throws(() => intentFor("adoption::nonexistent_test"), /missing test intent source/);
});

test("narrow docs/scripts inputs free only unreachable suites, matching real Nx", async () => {
  const { compareGlobInputs } = await import("./glob-impact.mjs");
  const changes = [
    "docs/unrelated.md",
    "docs/status.md",
    "scripts/unrelated.js",
    "scripts/status.js",
    "record/source.rs",
  ];
  const selection = {
    crates: {
      record: { paths: ["record/**"], deps: [] },
      status: { paths: ["status/**"], deps: ["record"] },
      template: { paths: ["template/**"], deps: [] },
    },
    suites: {
      "status-e2e": {
        paths: ["status-e2e/**", "docs/status.md", "scripts/status.js"],
        crates: ["status"],
      },
      "template-e2e": { paths: ["template-e2e/**"], crates: ["template"] },
    },
  };
  const rows = changes.map((path, index) => ({
    commit: String(index),
    subject: `fixture #${index}`,
    files: [path],
    effectiveFiles: [path],
  }));
  const compared = compareGlobInputs(selection, rows);
  assert.equal(compared.freed.length, 4);
  assert.equal(compared.fullyFreed, 2);
  assert.equal(compared.suiteRunsSaved, 6);
  assert.equal(compared.wideSkip, 0.1);
  assert.equal(compared.narrowSkip, 0.7);
  for (const wide of [true, false]) {
    const root = scratch();
    try {
      symlinkSync(resolve("node_modules"), join(root, "node_modules"), "junction");
      put(root, ".gitignore", "node_modules\n.nx\n");
      put(root, "package.json", { private: true });
      put(root, "nx.json", {
        namedInputs: {
          default: [
            "{projectRoot}/**/*",
            ...(wide ? ["{workspaceRoot}/docs/**/*", "{workspaceRoot}/scripts/**/*"] : []),
          ],
        },
      });
      for (const [name, crate] of Object.entries(selection.crates)) {
        put(root, `${name}/source.rs`, "// initial\n");
        put(root, `${name}/project.json`, {
          name,
          root: name,
          implicitDependencies: crate.deps,
          targets: { test: { executor: "nx:noop", inputs: ["default", "^default"] } },
        });
      }
      for (const [name, suite] of Object.entries(selection.suites))
        put(root, `${name}/project.json`, {
          name,
          root: name,
          implicitDependencies: suite.crates,
          namedInputs: {
            default: [
              "{projectRoot}/**/*",
              ...(wide
                ? ["{workspaceRoot}/docs/**/*", "{workspaceRoot}/scripts/**/*"]
                : suite.paths
                    .filter((path) => !path.startsWith(name))
                    .map((path) => `{workspaceRoot}/${path}`)),
            ],
          },
          targets: { test: { executor: "nx:noop", inputs: ["default", "^default"] } },
        });
      for (const path of changes) put(root, path, "initial\n");
      const git = (...args) =>
        execFileSync("git", args, {
          cwd: root,
          encoding: "utf8",
          stdio: ["ignore", "pipe", "pipe"],
        });
      git("init", "-q");
      git("config", "user.email", "fixture@example.invalid");
      git("config", "user.name", "Fixture");
      git("add", ".");
      git("commit", "-qm", "test: glob fixture");
      for (const [index, path] of changes.entries()) {
        put(root, path, "changed\n");
        git("add", ".");
        git("commit", "-qm", `test: changed ${index}`);
        const actual = JSON.parse(
          execFileSync(
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
            {
              cwd: root,
              encoding: "utf8",
              env: { ...process.env, NX_DAEMON: "false", NX_ISOLATE_PLUGINS: "false" },
            },
          ),
        )
          .filter((name) => Object.hasOwn(selection.suites, name))
          .sort();
        const expected = affectedSuites(
          compileMapping({ ...selection, ...(wide ? { shared: ["docs/**", "scripts/**"] } : {}) }),
          [path],
        ).affected.sort();
        assert.deepEqual(actual, expected, `${wide ? "blanket" : "narrow"}: ${path}`);
      }
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }
  assert.throws(() => compareGlobInputs(selection, []), /needs validated PR rows/);
  assert.throws(
    () =>
      compareGlobInputs(selection, [
        { commit: "x", subject: "bad", files: [], effectiveFiles: 42 },
      ]),
    /needs validated PR rows/,
  );
});

test("reviewed argv attribution keeps release transitions, agents identities and double teardown in their actual suites", () => {
  const result = spawnSync(
    process.execPath,
    [
      "--input-type=module",
      "--eval",
      `
    import {readFileSync} from "node:fs";
    import {decompose} from "./scripts/modularization/decompose.mjs";
    console.log(JSON.stringify(decompose(JSON.parse(readFileSync("scripts/modularization/command-tests.json", "utf8")))));
  `,
    ],
    { encoding: "utf8", maxBuffer: 8 * 1024 * 1024 },
  );
  assert.equal(result.status, 0, result.stderr);
  const catalog = JSON.parse(result.stdout);
  assert.equal(catalog.schema, 4);
  const release =
    catalog.dispositions[
      "adoption::the_three_release_kinds_reach_a_planner_through_the_shipped_profile"
    ];
  assert.equal(release.kind, "integration");
  assert.deepEqual(release.originalCommands, ["cmd-observer-view", "cmd-start", "cmd-stop"]);
  assert.match(release.rationale, /releases_at.*monitor/);
  const status =
    catalog.dispositions[
      "adoption::status_names_the_release_a_held_node_awaits_rather_than_calling_it_queued"
    ];
  assert.equal(status.kind, "integration");
  assert.deepEqual(status.originalCommands, ["cmd-start", "cmd-status"]);
  const agents =
    catalog.dispositions[
      "agents::every_dispatch_of_a_run_is_visible_through_its_pointer_file_with_nothing_configured"
    ];
  assert.equal(agents.kind, "integration");
  assert(agents.originalCommands.includes("cmd-agents"));
  for (const [id, entry] of Object.entries(catalog.dispositions)) {
    if (id.startsWith("harness::")) assert.notEqual(entry.kind, "dispatch", id);
    if (entry.kind === "dispatch") assert(id.startsWith("surface::"), id);
  }
  assert.equal(
    catalog.dispositions[
      "harness::a_worlds_teardown_ends_a_double_it_started_and_the_descendant_that_double_started"
    ].kind,
    "support",
  );
  assert.equal(
    catalog.dispositions["closures::publish_branch_closes_the_issues_its_landing_closes"].kind,
    "isolated",
  );
  assert.deepEqual(
    catalog.dispositions[
      "templates::a_dangling_link_at_a_layer_is_refused_rather_than_passed_to_the_layer_under_it"
    ].originalCommands,
    ["cmd-template-resolve"],
  );
});

test("manifest edits select exactly their dependency, inherited metadata, version, feature and profile consumers in real Nx", () => {
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
    symlinkSync(resolve("node_modules"), join(root, "node_modules"), "junction");
    put(root, ".gitignore", "node_modules\n.nx\n");
    put(root, "package.json", { private: true });
    put(root, "nx.json", { namedInputs: { default: ["{projectRoot}/**/*"] } });
    const crates = {
      record: { external: ["serde"], deps: [] },
      start: { external: [], deps: ["record"] },
      template: { external: ["minijinja"], deps: [] },
      onepipeline: { external: [], deps: ["start", "template"] },
      "cli-dispatcher": { external: [], deps: ["onepipeline"] },
    };
    const suites = {
      "start-e2e": { paths: ["start-e2e/**"], crates: ["start"] },
      "template-e2e": { paths: ["template-e2e/**"], crates: ["template"] },
      "routing-e2e": { paths: ["routing-e2e/**"], crates: ["cli-dispatcher"] },
    };
    const mapping = compileMapping({
      crates: Object.fromEntries(
        Object.entries(crates).map(([name, crate]) => [
          name,
          { paths: [`${name}/**`, `resolved/${name}/**`], deps: crate.deps },
        ]),
      ),
      suites,
    });
    for (const [name, item] of [...Object.entries(crates), ...Object.entries(suites)]) {
      put(root, `${name}/project.json`, {
        name,
        root: name,
        implicitDependencies: [
          ...(item.deps ?? item.crates),
          ...(crates[name] ? [`resolved-${name}`] : []),
        ],
        targets: { test: { executor: "nx:noop", inputs: ["default", "^default"] } },
      });
      if (crates[name]) {
        put(root, `resolved/${name}/project.json`, {
          name: `resolved-${name}`,
          root: `resolved/${name}`,
        });
        put(root, `resolved/${name}/manifest.json`, "initial");
      }
    }
    const manifest = `[package]
name = "onepipeline"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
[workspace]
resolver = "2"
[workspace.package]
edition = "2021"
rust-version = "1.90"
[workspace.dependencies]
serde = "1.0"
[dependencies]
serde.workspace = true
minijinja = "2.0"
unused = "1.0"
[features]
default = []
[profile.dev]
opt-level = 0
`;
    put(root, "Cargo.toml", manifest);
    git("init", "-b", "main");
    git("config", "user.email", "fixture@example.invalid");
    git("config", "user.name", "Fixture");
    git("add", ".");
    git("commit", "-m", "test: manifest graph");
    const base = git("rev-parse", "HEAD").trim();
    const all = ["routing-e2e", "start-e2e", "template-e2e"];
    const cases = [
      [
        "inherited dependency",
        manifest.replace('serde = "1.0"', 'serde = "1.1"'),
        ["resolved/record/manifest.json"],
        ["routing-e2e", "start-e2e"],
      ],
      [
        "direct dependency",
        manifest.replace('minijinja = "2.0"', 'minijinja = "2.1"'),
        ["resolved/template/manifest.json"],
        ["routing-e2e", "template-e2e"],
      ],
      ["unused dependency", manifest.replace('unused = "1.0"', 'unused = "1.1"'), [], []],
      [
        "inherited edition",
        manifest.replace('edition = "2021"', 'edition = "2024"'),
        Object.keys(crates).map((name) => `resolved/${name}/manifest.json`),
        all,
      ],
      [
        "inherited rust version",
        manifest.replace('rust-version = "1.90"', 'rust-version = "1.91"'),
        Object.keys(crates).map((name) => `resolved/${name}/manifest.json`),
        all,
      ],
      [
        "package version",
        manifest.replace('version = "0.1.0"', 'version = "0.2.0"'),
        ["resolved/onepipeline/manifest.json", "resolved/cli-dispatcher/manifest.json"],
        ["routing-e2e"],
      ],
      [
        "features",
        manifest.replace("default = []", 'default = ["serde/derive"]'),
        Object.keys(crates).map((name) => `resolved/${name}/manifest.json`),
        all,
      ],
      [
        "profile",
        manifest.replace("opt-level = 0", "opt-level = 1"),
        Object.keys(crates).map((name) => `resolved/${name}/manifest.json`),
        all,
      ],
    ];
    for (const [name, after, expectedPaths, expectedSuites] of cases) {
      const effective = manifestChanges(crates, manifest, after);
      assert.deepEqual([...effective].sort(), [...expectedPaths].sort(), name);
      assert.deepEqual(affectedSuites(mapping, effective).affected.sort(), expectedSuites, name);
      put(root, "Cargo.toml", after);
      for (const path of effective) put(root, path, `${name}\n`);
      git("add", ".");
      git("commit", "-m", `test: ${name}`);
      const selected = JSON.parse(
        execFileSync(
          process.execPath,
          [
            resolve("node_modules/nx/dist/bin/nx.js"),
            "show",
            "projects",
            "--affected",
            `--base=${base}`,
            "--head=HEAD",
            "--with-target=test",
            "--json",
          ],
          { cwd: root, env, encoding: "utf8" },
        ),
      )
        .filter((name) => Object.hasOwn(suites, name))
        .sort();
      assert.deepEqual(selected, expectedSuites, name);
      git("reset", "--hard", base);
    }
    assert.throws(() => manifestChanges(crates, manifest, "[package"));
    assert.deepEqual(manifestChanges(crates, manifest, manifest), []);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
