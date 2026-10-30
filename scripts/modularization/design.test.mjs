import assert from "node:assert/strict";
import { execFileSync, spawn, spawnSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
  symlinkSync,
} from "node:fs";
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

test("research CLIs name the path, not their inputs, when an output or input path is unusable", () => {
  const root = scratch();
  try {
    put(root, "not-a-directory", "a file where a directory is expected");
    const blocked = join(root, "not-a-directory", "out.json");
    const cli = (name, ...args) =>
      spawnSync(process.execPath, [`scripts/modularization/${name}`, ...args], {
        encoding: "utf8",
      });
    for (const [name, args] of [
      ["replay.mjs", ["scripts/modularization/layout.json", blocked]],
      ["target-graph.mjs", [blocked]],
      ["suite-inputs.mjs", [root]],
      ["target-report.mjs", [root]],
    ]) {
      const result = cli(name, ...args);
      assert.equal(result.status, 1, name);
      assert.match(result.stderr, /EEXIST|ENOTDIR|EISDIR/, name);
      assert.match(
        result.stderr,
        new RegExp(
          `next: check the named path's type.*rerun the same ${name.replace(".", "\\.")} invocation`,
        ),
        name,
      );
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

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
    const badGraphOutput = run(".");
    assert.equal(badGraphOutput.status, 1);
    assert.match(badGraphOutput.stderr, /output parent is writable/);
    assert.equal(run("graph.json").status, 0);
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
      put(
        inputs,
        "current.toml",
        '[dependencies]\nreader = { package = "modern-reader", version = "1" }\nfuture-only = "1"\n',
      );
      put(
        inputs,
        "historical.toml",
        '[dependencies]\nreader = { package = "reader-lib", version = "1" }\n',
      );
      const project = (afterLock, afterTool, historical = false, expectedStatus = 0) => {
        const configured = JSON.parse(readFileSync(join(inputs, "crates.json")));
        configured.reader.external = historical ? ["modern-reader", "future-only"] : ["reader-lib"];
        put(inputs, "crates.json", configured);
        const result = spawnSync(
          process.execPath,
          [
            cli,
            historical ? "--resolve-history" : "--resolve",
            join(inputs, "crates.json"),
            join(inputs, "before.lock"),
            join(inputs, afterLock),
            join(inputs, "before-tool"),
            join(inputs, afterTool),
            join(inputs, "changes.json"),
            ...(historical
              ? ["current.toml", "historical.toml", "historical.toml"].map((file) =>
                  join(inputs, file),
                )
              : []),
          ],
          { encoding: "utf8" },
        );
        assert.equal(result.status, expectedStatus, result.stderr);
        if (expectedStatus !== 0) return result.stderr;
        return JSON.parse(readFileSync(join(inputs, "changes.json"), "utf8"));
      };
      assert.deepEqual(
        project("after.lock", "before-tool", true),
        project("after.lock", "before-tool"),
      );
      assert.deepEqual(project("before.lock", "before-tool", true), []);
      const historicalManifest = readFileSync(join(inputs, "historical.toml"), "utf8");
      for (const invalidManifest of [
        "[dependencies]\nreader={version=42}",
        '[dependencies]\nreader={version="1",optional=42}',
        '[dependencies]\nreader={version="1",default-features=42}',
        '[dependencies]\nreader={version="1",features=[42]}',
        "[dependencies]\nreader=[]",
        "[target]\nlinux=42",
        "[package]\nbuild=42",
        "[package]\nedition={workspace=false}",
        "[workspace]\npackage=42",
        "[workspace]\nresolver=42",
        "[workspace.package]\nedition=42",
        "dependencies=42",
        "dependencies=[]",
        "[dependencies]\nreader=42",
        '[dependencies]\nreader={package=42,version="1"}',
        "[workspace]\ndependencies=42",
        "target=42",
        "[target.'cfg(unix)']\ndependencies=42",
        "package=42",
        "workspace=42",
        "features=42",
        '[features]\ndefault="feature"',
        "profile=42",
        "[profile]\ndev=42",
        "[package]\nedition=42",
        "[dependencies]\nreader={workspace=false}",
        '[dependencies]\nreader={version="1",features=42}',
      ]) {
        put(inputs, "historical.toml", invalidManifest);
        assert.match(project("before.lock", "before-tool", true, 1), /invalid Cargo/);
      }
      put(inputs, "historical.toml", historicalManifest);
      assert.deepEqual(project("before.lock", "before-tool", true), []);

      // A modern package can coexist historically for another crate without becoming this crate's root.
      const modern = (version) => `\n[[package]]\nname = "modern-reader"\nversion = "${version}"\n`;
      put(inputs, "before.lock", lock("1.0.0") + modern("1.0.0"));
      put(inputs, "after.lock", lock("1.0.0") + modern("1.1.0"));
      put(
        inputs,
        "historical.toml",
        '[dependencies]\nreader = { package = "reader-lib", version = "1" }\nother = { package = "modern-reader", version = "1" }\n',
      );
      assert.deepEqual(project("after.lock", "before-tool", true), []);
      put(inputs, "before.lock", lock("1.0.0"));
      put(inputs, "after.lock", lock("1.1.0"));
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
    // A baseline entry must name the suite it belonged to.
    for (const suite of [undefined, 42, "Not A Suite"]) {
      const entry = { ...baseline.tests["example::dynamic_family"], suite };
      if (suite === undefined) delete entry.suite;
      put(root, baselinePath, {
        ...baseline,
        tests: { ...baseline.tests, "example::dynamic_family": entry },
      });
      expectFailure(/invalid baseline test example::dynamic_family/);
    }
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
    expectFailure(/EISDIR|EPERM|EACCES/);
    assert.match(
      run().stderr,
      /output parent is writable; rerun the same decompose.mjs invocation/,
    );
    rmSync(join(root, outputPath), { recursive: true });
    assert.equal(run().status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

// Direct API proof isolates parser rejection branches: the repository CLI can refuse first on inventory drift.
// A subprocess for each object would obscure that boundary; the target replay CLI test proves serialized integration.
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
    for (const name of [
      "command-replay.mjs",
      "catalog.mjs",
      "rust-syntax.mjs",
      "phase-inputs.mjs",
      "attribution.mjs",
      "command-attributions.json",
      "source-projection.mjs",
      "manifest-projection.mjs",
      "config-projection.mjs",
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
    // scripts/modularization/project.json targets.test runs this file through pinned.mjs and its
    // targets.check depends on test; justfile's check runs Nx check targets, so this regeneration
    // comparison gates the catalog against the pinned reviewed source.
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
    assert.ok(
      evidence.result.rows.every(
        (row) => !row.reasons.some((reason) => reason.includes("historically unresolvable")),
      ),
    );
    // Each runner links only the test fakes its tests start or its closure executes.
    const runnerDeps = (suite) => evidence.result.mapping.selection.crates[`runner-${suite}`].deps;
    assert.deepEqual(
      runnerDeps("cmd-telemetry").filter((crate) => /fake|^test-|held-git|source$/.test(crate)),
      ["test-fake-codec", "test-profile", "test-containment"],
    );
    assert.ok(runnerDeps("test-support").includes("held-git"));
    assert.ok(runnerDeps("cmd-start").includes("label-strict-source"));
    assert.ok(!runnerDeps("cmd-status").includes("fake-oneagentgraph"));
    assert.ok(
      evidence.result.rows.every(
        (row) => !row.effectiveFiles.includes("crates/testfakes/Cargo.toml"),
      ),
    );
    assert.ok(!evidence.result.mapping.selection.shared.includes("justfile"));
    assert.ok(!evidence.result.mapping.selection.shared.includes(".cargo/config.toml"));
    assert.ok(!evidence.result.mapping.selection.shared.includes(".config/nextest.toml"));
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
    assert.match(doc, /frees at least one suite on \*\*40 PRs\*\*/);
    assert.match(doc, /removes \*\*\d+ suite runs\*\*/);
    assert.equal(`${TARGET_MARKER}${doc.split(TARGET_MARKER)[1]}`, targetAppendix(evidence));
    const agentsRow = doc.split("\n").find((line) => line.startsWith("| `agents` | cmd-agents |"));
    assert(agentsRow);
    assert.match(
      agentsRow,
      /agents::every_dispatch_of_a_run_is_visible_through_its_pointer_file_with_nothing_configured \(cross-command\)/,
    );
    assert.match(doc, /Exception suite \| Complete runner crate closure/);
    assert.match(doc, /Source-backed argv and boundary audit/);
    assert.match(
      doc,
      /src\/ledger.rs::a_lock_without_a_start_token_reads_and_is_written_back_without_one \| run-locks \|/,
    );
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
    // A plausible fraction that contradicts the rows and weights is refused too.
    evidence.result.platforms.linux.secondsSkip = seconds > 0.5 ? seconds / 2 : 0.5;
    put(root, "target.json", evidence);
    assert.match(render().stderr, /seconds skipped differs from its rows and weights/);
    evidence.result.platforms.linux.secondsSkip = seconds;
    // Each suite's weight must be a finite nonnegative number, even when offsetting values keep the total.
    const weights = structuredClone(evidence.result.platforms.linux.suiteSeconds);
    const [first, second] = Object.keys(weights);
    for (const corrupt of [
      { [first]: -1, [second]: weights[second] + weights[first] + 1 },
      { [first]: "12" },
      { [first]: Number.NaN },
    ]) {
      evidence.result.platforms.linux.suiteSeconds = { ...weights, ...corrupt };
      put(root, "target.json", evidence);
      assert.match(render().stderr, /invalid target platform data/);
    }
    evidence.result.platforms.linux.suiteSeconds = weights;
    const migration = structuredClone(evidence.result.migration);
    for (const mutate of [
      (value) => {
        value.migration = null;
      },
      (value) => {
        value.migration.pop();
      },
      (value) => {
        value.migration[0].crate = "unknown-crate";
      },
      (value) => {
        value.migration[0].suiteSkip = "not-a-number";
      },
      (value) => {
        value.migration[0].suiteSkip = null;
      },
      (value) => {
        value.migration[0].suiteSkip = 2;
      },
      (value) => {
        value.migration[0].suiteSkip = -1;
      },
      (value) => {
        value.migration[0].extra = true;
      },
      (value) => {
        value.migration[1].crate = value.migration[0].crate;
      },
      (value) => {
        value.migration.reverse();
      },
      (value) => {
        value.migration.at(-1).suiteSkip += 0.001;
      },
    ]) {
      evidence.result.migration = structuredClone(migration);
      mutate(evidence.result);
      put(root, "target.json", evidence);
      const refused = render();
      assert.equal(refused.status, 1);
      assert.match(refused.stderr, /invalid target migration/);
    }
    evidence.result.migration = migration;
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
    const run = (output = "out.json") =>
      spawnSync(
        process.execPath,
        [
          cli,
          "--resolve",
          ...["crates.json", "before.lock", "after.lock", "before-tool", "after-tool", output].map(
            (file) => join(root, file),
          ),
        ],
        { encoding: "utf8" },
      );
    const failure = (pattern) => {
      const result = run();
      assert.equal(result.status, 1, result.stdout);
      assert.match(result.stderr, pattern);
      assert.match(result.stderr, /next: correct the named resolution.*same --resolve invocation/);
    };
    assert.equal(run().status, 0);
    assert.deepEqual(JSON.parse(readFileSync(join(root, "out.json"), "utf8")), []);
    const badOutput = run(".");
    assert.equal(badOutput.status, 1);
    assert.match(badOutput.stderr, /EISDIR|EPERM|EACCES/);
    assert.match(
      badOutput.stderr,
      /output parent is writable; rerun the same --resolve invocation/,
    );
    assert.equal(run().status, 0);
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

test("pinned runs a command over the reviewed source with the working tree's design files, and refuses a snapshot it cannot pin", () => {
  const root = scratch();
  const clones = scratch();
  const env = { ...process.env, ONEPIPELINE_NODE_SCRATCH_DIR: clones };
  const git = (...args) =>
    execFileSync(
      "git",
      [
        "-c",
        "user.name=test",
        "-c",
        "user.email=test@localhost",
        "-c",
        "commit.gpgsign=false",
        ...args,
      ],
      { cwd: root, encoding: "utf8", stdio: "pipe" },
    ).trim();
  const pinned = (...args) =>
    spawnSync(process.execPath, ["scripts/modularization/pinned.mjs", ...args], {
      cwd: root,
      env,
      encoding: "utf8",
    });
  try {
    git("init", "--quiet");
    put(root, "src/lib.rs", "reviewed\n");
    put(root, "docs/design.md", "committed design\n");
    git("add", "-A");
    git("commit", "--quiet", "-m", "reviewed");
    const reviewed = git("rev-parse", "HEAD");
    put(root, "src/lib.rs", "a later change\n");
    put(
      root,
      "scripts/modularization/pinned.mjs",
      readFileSync("scripts/modularization/pinned.mjs", "utf8"),
    );
    put(root, "scripts/modularization/snapshot.json", {
      commit: reviewed,
      design: ["docs/design.md", "scripts/modularization"],
    });
    git("add", "-A");
    git("commit", "--quiet", "-m", "later");
    git("update-ref", "refs/remotes/origin/main", "HEAD");
    // An uncommitted edit to a design file is what the command sees.
    put(root, "docs/design.md", "working design\n");
    const report = [
      "const { execFileSync } = require('node:child_process');",
      "const { readFileSync, writeFileSync } = require('node:fs');",
      "const git = (...a) => execFileSync('git', a, { encoding: 'utf8' }).trim();",
      "console.log(JSON.stringify({ parent: git('rev-parse', 'HEAD^'), main: git('rev-parse', 'origin/main'),",
      "  src: readFileSync('src/lib.rs', 'utf8'), design: readFileSync('docs/design.md', 'utf8') }));",
      "if (process.argv[1] === 'regenerate') writeFileSync('docs/design.md', 'regenerated\\n');",
      "process.exit(Number(process.argv[2] ?? 0));",
    ].join("\n");
    const seen = pinned("--", process.execPath, "-e", report);
    assert.equal(seen.status, 0, seen.stderr);
    assert.deepEqual(JSON.parse(seen.stdout), {
      parent: reviewed,
      main: git("rev-parse", "origin/main"),
      src: "reviewed\n",
      design: "working design\n",
    });
    // The command's exit status is the runner's, the clone is removed, and without --write-back a
    // regenerated design file stays in the clone.
    const failed = pinned("--", process.execPath, "-e", report, "regenerate", "3");
    assert.equal(failed.status, 3, failed.stderr);
    assert.equal(readFileSync(join(root, "docs/design.md"), "utf8"), "working design\n");
    assert.deepEqual(readdirSync(clones), []);
    const regenerated = pinned("--write-back", "--", process.execPath, "-e", report, "regenerate");
    assert.equal(regenerated.status, 0, regenerated.stderr);
    assert.equal(readFileSync(join(root, "docs/design.md"), "utf8"), "regenerated\n");
    assert.equal(readFileSync(join(root, "src/lib.rs"), "utf8"), "a later change\n");
    assert.match(pinned("extra").stderr, /usage: pinned.mjs/);
    put(root, "scripts/modularization/snapshot.json", {
      commit: reviewed,
      design: ["docs/design.md"],
      typo: true,
    });
    const typo = pinned("--", "true");
    assert.equal(typo.status, 1);
    assert.match(typo.stderr, /invalid design snapshot scripts\/modularization\/snapshot.json/);
    put(root, "scripts/modularization/snapshot.json", {
      commit: reviewed,
      design: ["../outside"],
    });
    assert.match(pinned("--", "true").stderr, /invalid design snapshot/);
    put(root, "scripts/modularization/snapshot.json", {
      commit: "0".repeat(40),
      design: ["docs/design.md"],
    });
    const missing = pinned("--", "true");
    assert.equal(missing.status, 1);
    assert.match(missing.stderr, /design snapshot commit 0{40} is not in this repository/);
    assert.match(missing.stderr, /next: fetch the snapshot commit/);
    put(root, "scripts/modularization/snapshot.json", {
      commit: reviewed,
      design: ["docs/design.md", "scripts/modularization"],
    });
    // git failing for its own reason is reported as that, not as a missing commit.
    const broken = join(root, "not-a-repository");
    mkdirSync(join(broken, "scripts/modularization"), { recursive: true });
    for (const name of ["pinned.mjs", "snapshot.json"])
      put(
        broken,
        `scripts/modularization/${name}`,
        readFileSync(join(root, "scripts/modularization", name), "utf8"),
      );
    const unreadable = spawnSync(
      process.execPath,
      ["scripts/modularization/pinned.mjs", "--", "true"],
      {
        cwd: broken,
        env: { ...env, GIT_CEILING_DIRECTORIES: root },
        encoding: "utf8",
      },
    );
    assert.equal(unreadable.status, 1);
    assert.match(unreadable.stderr, /git could not read .*not a git repository/i);
    assert.match(
      unreadable.stderr,
      /next: check that git is installed and .* is a readable git repository/,
    );
    assert.doesNotMatch(unreadable.stderr, /is not in this repository/);
    // A command ended by a signal is reported as that, with what to check.
    const signalled = pinned("--", process.execPath, "-e", "process.kill(process.pid, 'SIGTERM')");
    assert.equal(signalled.status, 1);
    assert.match(
      signalled.stderr,
      /ended on signal SIGTERM before it exited\nnext: rerun it if the signal was an interruption/,
    );
    // A missing command and an unusable scratch parent each name their own repair.
    const absent = pinned("--", "no-such-command-for-pinned");
    assert.equal(absent.status, 1);
    assert.match(
      absent.stderr,
      /next: install no-such-command-for-pinned or correct the command given after --/,
    );
    const nowhere = spawnSync(
      process.execPath,
      ["scripts/modularization/pinned.mjs", "--", "true"],
      {
        cwd: root,
        env: { ...env, ONEPIPELINE_NODE_SCRATCH_DIR: join(root, "missing", "dir") },
        encoding: "utf8",
      },
    );
    assert.equal(nowhere.status, 1);
    assert.match(nowhere.stderr, /next: make .*missing\/dir an existing, writable directory/);
    assert.deepEqual(readdirSync(clones), []);
  } finally {
    rmSync(root, { recursive: true, force: true });
    rmSync(clones, { recursive: true, force: true });
  }
});

// The CLI enumerates known test IDs itself, so it cannot receive an arbitrary intentFor ID.
// Direct API refusal checks cover that caller boundary; target-report CLI tests prove rendered source intent.
test("test intent comes from the existing journey and refuses path-like ids", async () => {
  const { intentFor } = await import("./target-report.mjs");
  assert.match(
    intentFor("adoption::the_three_release_kinds_reach_a_planner_through_the_shipped_profile"),
    /The three kinds this crate emits reach a planner through the \*\*shipped profile with no filter change/,
  );
  // docs/ may not carry an observer-graph member word (tests/contract.rs), so an excerpt using one is
  // cited by location, and a test without documentation is named by its identifier.
  assert.equal(
    intentFor(
      "driver::a_detached_observers_unclassified_provider_death_reaches_the_manager_as_a_finding",
    ),
    "documented above `a_detached_observers_unclassified_provider_death_reaches_the_manager_as_a_finding` in tests/e2e/driver.rs; not excerpted here because it names an observer-graph member, which docs/ may not",
  );
  assert.equal(
    intentFor("shipped::the_dag_scope_graph_is_the_monitor_and_the_resettable_check_in"),
    "`the_dag_scope_graph_is_the_monitor_and_the_resettable_check_in`",
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
  // The report CLI rejects malformed row shapes before invoking this comparison helper.
  // Direct API refusal proof reaches these guards; the real Nx journey above proves valid selection.
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

test("configuration projection selects real test recipes and semantic Cargo/nextest inputs", async () => {
  const root = scratch();
  const cli = resolve("scripts/modularization/config-projection.mjs");
  put(root, "project.json", readFileSync("project.json", "utf8"));
  put(root, "tests/e2e/project.json", readFileSync("tests/e2e/project.json", "utf8"));
  put(root, "justfile", readFileSync("justfile", "utf8"));
  put(root, ".github/workflows/ci.yml", readFileSync(".github/workflows/ci.yml", "utf8"));
  const runnerProject = JSON.parse(readFileSync(join(root, "tests/e2e/project.json"), "utf8"));
  runnerProject.targets.test.dependsOn = [];
  runnerProject.targets["test-quick"] = { executor: "nx:noop" };
  put(root, "tests/e2e/project.json", runnerProject);
  const suites = ["cmd-start", "cmd-status"];
  const configChanges = (file, before, after, owners) => {
    put(root, "before", before);
    put(root, "after", after);
    put(root, "suites.json", owners);
    const result = spawnSync(
      process.execPath,
      [
        cli,
        file,
        join(root, "before"),
        join(root, "after"),
        join(root, "suites.json"),
        join(root, "output.json"),
      ],
      { encoding: "utf8", cwd: root },
    );
    if (result.status !== 0) throw new Error(result.stderr);
    return JSON.parse(readFileSync(join(root, "output.json"), "utf8"));
  };
  try {
    const just =
      'tool := "1"\nfreeze := "1"\n_e2e-test:\n    cargo nextest run {{tool}}\nscreenshots:\n    freeze {{freeze}}\n';
    assert.deepEqual(
      configChanges("justfile", just, just.replace('freeze := "1"', 'freeze := "2"'), suites),
      [],
    );
    assert.throws(
      () => configChanges("justfile", "unrelated:\n    echo ready\n", just, suites),
      /unresolved just recipe _e2e-test \(read from tests\/e2e\/project.json targets.test.command\)/,
    );
    assert.equal(
      configChanges("justfile", just, just.replace('tool := "1"', 'tool := "2"'), suites).length,
      2,
    );
    for (const file of [".cargo/config.toml", ".config/nextest.toml"])
      for (const invalid of ["profile=42", "profile=[]", "[profile]\nci=42", "[profile]\nci=[]"])
        assert.throws(
          () => configChanges(file, "", invalid, suites),
          /invalid configuration profile/,
        );
    const otherTarget = `${just}\n_contract-test:\n    cargo nextest run contract\n`;
    const evaluated = just.replace("cargo nextest run {{tool}}", "echo $(just --evaluate tool)");
    assert.equal(
      configChanges("justfile", evaluated, evaluated.replace('tool := "1"', 'tool := "2"'), suites)
        .length,
      2,
    );
    const separateBuild = `${just}\n_tier-build bin:\n    cargo build\n`;
    assert.deepEqual(
      configChanges(
        "justfile",
        separateBuild,
        separateBuild.replace("cargo build", "cargo build --locked"),
        suites,
      ),
      [],
    );
    const helperTarget = `${just.replace("_e2e-test:", "_e2e-test: helper")}\nhelper:\n    echo prepare\n`;
    assert.equal(
      configChanges(
        "justfile",
        helperTarget,
        helperTarget.replace("echo prepare", "echo ready"),
        suites,
      ).length,
      2,
    );
    const executedHelper = `${just.replace("cargo nextest run {{tool}}", "just helper\n    cargo nextest run {{tool}}")}\nhelper:\n    echo prepare\n`;
    assert.equal(
      configChanges(
        "justfile",
        executedHelper,
        executedHelper.replace("echo prepare", "echo ready"),
        suites,
      ).length,
      2,
    );
    const diagnostic = just.replace(
      "cargo nextest run {{tool}}",
      'echo "run just screenshots"; cargo nextest run {{tool}}',
    );
    assert.deepEqual(
      configChanges(
        "justfile",
        diagnostic,
        diagnostic.replace('freeze := "1"', 'freeze := "2"'),
        suites,
      ),
      [],
    );
    const runTarget = `${just}\nrun:\n    cargo run\n`;
    assert.deepEqual(
      configChanges(
        "justfile",
        runTarget,
        runTarget.replace("    cargo run", "    cargo run --locked"),
        suites,
      ),
      [],
    );
    assert.deepEqual(
      configChanges(
        "justfile",
        otherTarget,
        otherTarget.replace("run contract", "run --locked contract"),
        suites,
      ),
      [],
    );
    assert.equal(
      configChanges("justfile", just, just.replace('tool := "1"', 'tool := "2"'), suites).length,
      2,
    );
    assert.equal(
      configChanges(
        "justfile",
        just,
        just.replace("cargo nextest run", "cargo nextest run --locked"),
        suites,
      ).length,
      2,
    );
    assert.deepEqual(
      configChanges(".cargo/config.toml", '[alias]\nx="check"', '[alias]\nx="build"', suites),
      [],
    );
    assert.equal(
      configChanges(
        ".cargo/config.toml",
        "[build]\nrustflags=[]",
        '[build]\nrustflags=["-Dwarnings"]',
        suites,
      ).length,
      2,
    );
    assert.deepEqual(
      configChanges(
        ".config/nextest.toml",
        '[profile.ci]\nstatus-level="pass"',
        '# comment\n[profile.ci]\nstatus-level="pass"',
        suites,
      ),
      [],
    );
    assert.throws(() => configChanges(".config/nextest.toml", "", "[broken", suites));
    assert.throws(
      () => configChanges(".cargo/config.toml", "", "", ["../outside"]),
      /suites must be nonempty project names/,
    );
    assert.throws(() => configChanges("unknown.toml", "", "", suites), /usage: config-projection/);
    assert.equal(
      configChanges(
        "justfile",
        'export UNUSED := "one"\n_e2e-test:\n    echo ready\n',
        'export UNUSED := "two"\n_e2e-test:\n    echo ready\n',
        suites,
      ).length,
      2,
    );
    assert.throws(
      () => configChanges("justfile", just, "not valid just syntax ???", suites),
      /error/,
    );
    for (const profile of ["dev", "test", "release"]) {
      assert.equal(
        configChanges(
          ".cargo/config.toml",
          `[profile.${profile}]\ndebug=0`,
          `[profile.${profile}]\ndebug=1`,
          suites,
        ).length,
        profile === "release" ? 0 : 2,
      );
    }
    for (const [section, before, after, selected] of [
      ["profile.default", 'status-level="pass"', 'status-level="slow"', 2],
      ["profile.unused", 'status-level="pass"', 'status-level="slow"', 0],
      ["test-groups", "e2e={max-threads=1}", "e2e={max-threads=2}", 2],
      ["store", 'dir="target/nextest"', 'dir="target/alternate"', 2],
    ]) {
      assert.equal(
        configChanges(
          ".config/nextest.toml",
          `[profile.ci]\n[${section}]\n${before}`,
          `[profile.ci]\n[${section}]\n${after}`,
          suites,
        ).length,
        selected,
      );
    }
    const inherited = '[profile.ci]\ninherits="base"\n[profile.base]\nretries=0';
    assert.equal(
      configChanges(
        ".config/nextest.toml",
        inherited,
        inherited.replace("retries=0", "retries=1"),
        suites,
      ).length,
      2,
    );
    assert.throws(
      () =>
        configChanges(
          ".config/nextest.toml",
          inherited,
          inherited.replace('inherits="base"', 'inherits="missing"'),
          suites,
        ),
      /invalid nextest inherited profile/,
    );
    assert.throws(
      () => configChanges(".config/nextest.toml", inherited, `${inherited}\ninherits="ci"`, suites),
      /inheritance cycle/,
    );
    assert.equal(
      configChanges(
        ".config/nextest.toml",
        inherited,
        inherited.replace("retries=0", "retries=2"),
        suites,
      ).length,
      2,
    );
    const badOutput = spawnSync(
      process.execPath,
      [
        cli,
        ".cargo/config.toml",
        join(root, "before"),
        join(root, "after"),
        join(root, "suites.json"),
        root,
      ],
      { encoding: "utf8", cwd: root },
    );
    assert.equal(badOutput.status, 1);
    assert.match(badOutput.stderr, /EISDIR|EPERM|EACCES/);
    assert.match(badOutput.stderr, /output parent is writable; rerun the same config-projection/);
    assert.equal(
      configChanges(
        ".config/nextest.toml",
        '[profile.ci]\nstatus-level="pass"',
        '[profile.ci]\nstatus-level="slow"',
        suites,
      ).length,
      2,
    );
    const project = JSON.parse(readFileSync(join(root, "tests/e2e/project.json"), "utf8"));
    const savedTarget = project.targets.test;
    const savedProject = structuredClone(project);
    for (const dependency of [
      null,
      42,
      {},
      [null],
      [42],
      ["^test"],
      [{ target: "test", projcts: ["onepipeline"] }],
      [{ target: 42 }],
      [{ target: "test", projects: "dependencies" }],
      [{ target: "test", projects: [] }],
      [{ target: "test", projects: null }],
      [{ target: "test", projects: [42] }],
      [{ target: "test", projects: ["missing-project"] }],
    ]) {
      project.targets.test = { ...savedTarget, dependsOn: dependency };
      put(root, "tests/e2e/project.json", project);
      assert.throws(
        () => configChanges("justfile", just, just, suites),
        /invalid runner dependency|unsupported runner dependency/,
      );
    }
    project.targets.test = savedTarget;
    for (const mutate of [
      (value) => {
        value.name = 42;
      },
      (value) => {
        value.name = "onepipeline";
      },
      (value) => {
        value.targets = [];
      },
      (value) => {
        value.targets.test = 42;
      },
      (value) => {
        value.targets.test = null;
      },
      (value) => {
        value.targets.test.command = [];
      },
      (value) => {
        value.targets.test.executor = 42;
      },
      (value) => {
        value.targets.test.executor = "custom:executor";
      },
      (value) => {
        value.targets.test.executor = "nx:noop";
      },
    ]) {
      const invalid = structuredClone(savedProject);
      mutate(invalid);
      put(root, "tests/e2e/project.json", invalid);
      assert.throws(
        () => configChanges("justfile", just, just, suites),
        /invalid runner project|invalid runner target|invalid runner command|unsupported runner executor/,
      );
    }
    put(root, "tests/e2e/project.json", project);
    assert.deepEqual(configChanges("justfile", just, just, suites), []);

    delete project.targets.test;
    put(root, "tests/e2e/project.json", project);
    assert.throws(
      () => configChanges("justfile", just, just, suites),
      /unresolved Nx target onepipeline-e2e:test \(read from tests\/e2e\/project.json runner roots\)/,
    );
    project.targets.test = { ...savedTarget, command: "node unsupported.mjs" };
    put(root, "tests/e2e/project.json", project);
    assert.throws(() => configChanges("justfile", just, just, suites), /unresolved runner command/);
    project.targets.test = { ...savedTarget, command: "just _e2e-test && node unsupported.mjs" };
    put(root, "tests/e2e/project.json", project);
    assert.throws(() => configChanges("justfile", just, just, suites), /unresolved runner command/);
    project.targets.test = { executor: "nx:noop" };
    put(root, "tests/e2e/project.json", project);
    assert.deepEqual(configChanges("justfile", just, just, suites), []);
    project.targets.test = savedTarget;
    put(root, "tests/e2e/project.json", project);
    put(root, "justfile", "_e2e-test:\n    cargo nextest run --profile focused\n");
    assert.equal(
      configChanges(
        ".config/nextest.toml",
        "[profile.ci]\n[profile.focused]\nretries=0",
        "[profile.ci]\n[profile.focused]\nretries=1",
        suites,
      ).length,
      2,
    );
    put(root, ".github/workflows/ci.yml", "env:\n  NEXTEST_PROFILE: alternate\n");
    assert.equal(
      configChanges(
        ".config/nextest.toml",
        "[profile.focused]\n[profile.alternate]\nretries=0",
        "[profile.focused]\n[profile.alternate]\nretries=1",
        suites,
      ).length,
      2,
    );
    assert.deepEqual(
      configChanges(
        ".config/nextest.toml",
        "[profile.focused]\n[profile.alternate]\n[profile.ci]\nretries=0",
        "[profile.focused]\n[profile.alternate]\n[profile.ci]\nretries=1",
        suites,
      ),
      [],
    );
    project.targets.test.command = "just renamed-entry";
    put(root, "tests/e2e/project.json", project);
    assert.equal(
      configChanges(
        "justfile",
        "renamed-entry:\n    echo before\n",
        "renamed-entry:\n    echo after\n",
        suites,
      ).length,
      2,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

/** A scratch runner workspace whose every reference resolves; each test breaks exactly one. */
function projectionWorkspace() {
  const root = scratch();
  const runner = JSON.parse(readFileSync("tests/e2e/project.json", "utf8"));
  runner.targets.test = { command: "just _e2e-test", inputs: ["e2eSource"] };
  runner.targets["test-quick"] = { executor: "nx:noop" };
  const files = {
    "project.json": readFileSync("project.json", "utf8"),
    "tests/e2e/project.json": runner,
    justfile: 'tool := "1"\n_e2e-test:\n    cargo nextest run {{tool}}\n',
    ".github/workflows/ci.yml": "env:\n  NEXTEST_PROFILE: ci\n",
  };
  for (const [path, value] of Object.entries(files)) put(root, path, value);
  const project = (file, before, after) => {
    put(root, "before", before);
    put(root, "after", after);
    put(root, "suites.json", ["cmd-start", "cmd-status"]);
    return spawnSync(
      process.execPath,
      [
        resolve("scripts/modularization/config-projection.mjs"),
        file,
        join(root, "before"),
        join(root, "after"),
        join(root, "suites.json"),
        join(root, "output.json"),
      ],
      { encoding: "utf8", cwd: root },
    );
  };
  const selected = (file, before, after) => {
    const result = project(file, before, after);
    assert.equal(result.status, 0, result.stderr);
    return JSON.parse(readFileSync(join(root, "output.json"), "utf8")).length;
  };
  const refused = (file, before, after, message) => {
    const result = project(file, before, after);
    assert.equal(result.status, 1, `expected a refusal, got ${result.stdout}`);
    assert.match(result.stderr, message);
    assert.match(result.stderr, /next: correct the named configuration/);
  };
  return { root, runner, selected, refused };
}

test("configuration projection refuses an unresolved just recipe reference", () => {
  const { root, selected, refused } = projectionWorkspace();
  try {
    const dependency = "_e2e-test: helper\n    cargo nextest run\nhelper:\n    echo ready\n";
    assert.equal(selected("justfile", dependency, dependency.replace("ready", "set")), 2);
    put(root, "justfile", dependency);
    // just itself refuses an undefined dependency, so the unresolved one is in a body invocation.
    for (const [body, message] of [
      [
        "just missing-helper",
        /unresolved just recipe missing-helper \(read from justfile recipe:_e2e-test\)/,
      ],
      [
        "echo $(just --evaluate missing)",
        /unresolved just variable missing \(read from justfile recipe:_e2e-test\)/,
      ],
      [
        'just {{"helper"}}',
        /unresolved just invocation "just \{\{…\}\}" \(read from justfile recipe:_e2e-test body\)/,
      ],
      ["just --justfile other helper", /unresolved just invocation "just --justfile other helper"/],
      [
        "{{just_executable()}} helper",
        /unresolved just invocation just_executable\(\) \(read from justfile recipe:_e2e-test\)/,
      ],
    ]) {
      const broken = dependency.replace("cargo nextest run", body);
      refused("justfile", broken, broken, message);
    }
    // Each setting that reads an input outside this file refuses, by the setting's name.
    for (const [setting, name] of [
      ["set dotenv-load", "dotenv_load"],
      ['set dotenv-filename := "local.env"', "dotenv_filename"],
      ['set dotenv-path := "local.env"', "dotenv_path"],
      ['set dotenv-command := "echo"', "dotenv_command"],
      ["set fallback", "fallback"],
    ]) {
      const configured = `${setting}\n${dependency}`;
      refused(
        "justfile",
        configured,
        configured,
        new RegExp(`unresolved just setting ${name} \\(read from justfile settings`),
      );
    }
    // A module's source file is never beside a projected snapshot, so just refuses it while parsing.
    const module = `mod helper\n${dependency}`;
    refused("justfile", module, module, /could not find source file for module `helper`/);
    // A diagnostic naming a recipe is not an invocation, and an entry recipe the runner names must exist.
    const diagnostic = dependency.replace("cargo nextest run", "echo \"run 'just nowhere' first\"");
    assert.equal(selected("justfile", diagnostic, diagnostic), 0);
    refused(
      "justfile",
      "other:\n    echo\n",
      "other:\n    echo\n",
      /unresolved just recipe _e2e-test \(read from tests\/e2e\/project.json targets.test.command\)/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("configuration projection refuses an unresolved Nx target reference", () => {
  const { root, runner, selected, refused } = projectionWorkspace();
  const just = 'tool := "1"\n_e2e-test:\n    cargo nextest run {{tool}}\n';
  try {
    assert.equal(selected("justfile", just, just.replace('"1"', '"2"')), 2);
    runner.targets.test.dependsOn = [{ projects: ["onepipeline"], target: "no-such-archive" }];
    put(root, "tests/e2e/project.json", runner);
    refused(
      "justfile",
      just,
      just,
      /unresolved Nx target onepipeline:no-such-archive \(read from tests\/e2e\/project.json targets.test.dependsOn\)/,
    );
    runner.targets.test.dependsOn = ["no-such-sibling"];
    put(root, "tests/e2e/project.json", runner);
    refused("justfile", just, just, /unresolved Nx target onepipeline-e2e:no-such-sibling/);
    // Options change what a target executes (cwd, env, args) and are not modelled, so they refuse.
    delete runner.targets.test.dependsOn;
    runner.targets.test.options = { cwd: "tests/e2e" };
    put(root, "tests/e2e/project.json", runner);
    refused(
      "justfile",
      just,
      just,
      /unresolved Nx target field options \(read from tests\/e2e\/project.json targets.test\)/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("configuration projection refuses an unresolved runner command", () => {
  const { root, runner, selected, refused } = projectionWorkspace();
  const just =
    'tool := "1"\n_e2e-test:\n    cargo nextest run {{tool}}\n_tier-test tier:\n    cargo nextest run\n';
  try {
    assert.equal(selected("justfile", just, just.replace('"1"', '"2"')), 2);
    for (const command of [
      "node run-tests.mjs",
      "just _tier-test e2e",
      "just _e2e-test && cargo test",
    ]) {
      runner.targets.test.command = command;
      put(root, "tests/e2e/project.json", runner);
      refused(
        "justfile",
        just,
        just,
        new RegExp(
          `unresolved runner command ${JSON.stringify(command).replace(/[.*+?^${}()|[\]\\]/g, "\\$&")} \\(read from tests/e2e/project.json targets.test.command\\)`,
        ),
      );
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("configuration projection refuses an unresolved nextest profile reference", () => {
  const { root, selected, refused } = projectionWorkspace();
  const config = "[profile.ci]\nretries=0\n[profile.focused]\nretries=0\n";
  const changed = config.replace("[profile.focused]\nretries=0", "[profile.focused]\nretries=1");
  try {
    assert.equal(selected(".config/nextest.toml", config, changed), 0);
    put(root, "justfile", "_e2e-test:\n    cargo nextest run --profile focused\n");
    assert.equal(selected(".config/nextest.toml", config, changed), 2);
    refused(
      ".config/nextest.toml",
      "[profile.ci]\n",
      "[profile.ci]\n",
      /unresolved nextest profile focused \(read from justfile recipe:_e2e-test body\)/,
    );
    put(root, "justfile", 'p := "focused"\n_e2e-test:\n    cargo nextest run --profile {{p}}\n');
    refused(
      ".config/nextest.toml",
      config,
      changed,
      /unresolved nextest profile "\{\{…\}\}" \(read from justfile recipe:_e2e-test body\)/,
    );
    put(root, "justfile", "_e2e-test:\n    cargo nextest run --cargo-profile ci-build\n");
    refused(
      ".config/nextest.toml",
      config,
      changed,
      /unresolved cargo profile selection --cargo-profile/,
    );
    put(root, "justfile", "_e2e-test:\n    cargo nextest run\n");
    // biome-ignore lint/suspicious/noTemplateCurlyInString: a literal GitHub Actions expression the projection must refuse
    put(root, ".github/workflows/ci.yml", "env:\n  NEXTEST_PROFILE: ${{ matrix.profile }}\n");
    refused(
      ".config/nextest.toml",
      config,
      changed,
      /unresolved nextest profile "\$\{\{ matrix.profile \}\}" \(read from .github\/workflows\/ci.yml:2\)/,
    );
    put(root, ".github/workflows/ci.yml", "env:\n  NEXTEST_PROFILE: missing\n");
    refused(
      ".config/nextest.toml",
      config,
      changed,
      /unresolved nextest profile missing \(read from .github\/workflows\/ci.yml:2\)/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("nextest's own parser accepts every script field and section the projection does, and only those", async () => {
  const { scriptEntryFields, nextestSections } = await import("./config-projection.mjs");
  const root = scratch();
  try {
    put(
      root,
      "Cargo.toml",
      '[package]\nname = "probe"\nversion = "0.0.0"\nedition = "2021"\n[workspace]\n',
    );
    put(root, "src/lib.rs", "pub fn probe() {}\n");
    const values = {
      filter: '"all()"',
      platform: '"cfg(all())"',
      setup: '"db"',
      "list-wrapper": '"trace"',
      "run-wrapper": '"trace"',
    };
    const sections = {
      "test-groups": "[test-groups]\nserial = { max-threads = 1 }\n",
      store: '[store]\ndir = "target/nextest"\n',
      experimental: 'experimental = ["setup-scripts", "wrapper-scripts"]\n',
      "nextest-version": 'nextest-version = "0.9.0"\n',
    };
    // Every accepted field and section must have a probe value, so the list cannot grow unchecked.
    assert.deepEqual(Object.keys(values).sort(), [...scriptEntryFields].sort());
    assert.deepEqual(Object.keys(sections).sort(), Object.keys(nextestSections).sort());
    const topLevel = Object.entries(sections).filter(
      ([name]) => !name.includes("groups") && name !== "store",
    );
    const tables = Object.entries(sections).filter(
      ([name]) => name.includes("groups") || name === "store",
    );
    put(
      root,
      ".config/nextest.toml",
      [
        ...topLevel.map(([, text]) => text),
        '[scripts.setup.db]\ncommand = "true"\n[scripts.wrapper.trace]\ncommand = "true"\n',
        ...tables.map(([, text]) => text),
        "[[profile.default.scripts]]\n",
        ...scriptEntryFields
          .filter((field) => field !== "list-wrapper" && field !== "run-wrapper")
          .map((field) => `${field} = ${values[field]}\n`),
        'setpu = "db"\n',
        "[[profile.default.scripts]]\n",
        `filter = ${values.filter}\nlist-wrapper = ${values["list-wrapper"]}\nrun-wrapper = ${values["run-wrapper"]}\n`,
      ].join(""),
    );
    const result = spawnSync("cargo", ["nextest", "show-config", "test-groups"], {
      cwd: root,
      encoding: "utf8",
      env: { ...process.env, CARGO_TARGET_DIR: join(root, "target") },
    });
    assert.equal(result.status, 0, result.stderr);
    const unknown = [...result.stderr.matchAll(/ignoring unknown configuration key: (\S+)/g)].map(
      (m) => m[1],
    );
    // nextest flags exactly the misspelled field: every field and section the projection accepts is one it reads.
    assert.deepEqual(unknown, ["profile.default.scripts.0.setpu"]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("configuration projection refuses an unresolved configuration section", () => {
  const { root, selected, refused } = projectionWorkspace();
  try {
    assert.equal(
      selected(".cargo/config.toml", "[term]\nverbose=false", "[term]\nverbose=true"),
      0,
    );
    assert.equal(selected(".cargo/config.toml", "[env]\nA='1'", "[env]\nA='2'"), 2);
    refused(
      ".cargo/config.toml",
      "",
      "[made-up]\nx=1",
      /unresolved configuration section made-up \(read from .cargo\/config.toml\)/,
    );
    // An unknown section refuses even when both snapshots agree on it, and a known one must have its shape.
    refused(
      ".cargo/config.toml",
      "[made-up]\nx=1\n[env]\nA='1'",
      "[made-up]\nx=1\n[env]\nA='2'",
      /unresolved configuration section made-up \(read from .cargo\/config.toml\)/,
    );
    refused(
      ".cargo/config.toml",
      "build=42",
      "build=42",
      /invalid configuration section build in .cargo\/config.toml/,
    );
    // Runners build with Cargo's dev/test profiles, so a runner selecting another refuses Cargo projection too.
    put(root, "justfile", "_e2e-test:\n    cargo nextest run --release\n");
    refused(
      ".cargo/config.toml",
      "[env]\nA='1'",
      "[env]\nA='2'",
      /unresolved cargo profile selection --release/,
    );
    put(root, "justfile", 'tool := "1"\n_e2e-test:\n    cargo nextest run {{tool}}\n');
    const nextest = (body) => `[profile.ci]\n${body}`;
    refused(
      ".config/nextest.toml",
      nextest('[store]\ndir="a"\n[made-up]\nx=1'),
      nextest('[store]\ndir="b"\n[made-up]\nx=1'),
      /unresolved configuration section made-up \(read from .config\/nextest.toml\)/,
    );
    refused(
      ".config/nextest.toml",
      nextest("").replace("[profile.ci]", 'store="x"\n[profile.ci]'),
      nextest("").replace("[profile.ci]", 'store="x"\n[profile.ci]'),
      /invalid configuration section store/,
    );
    assert.equal(
      selected(
        ".config/nextest.toml",
        nextest('nextest-version="0.9.1"'),
        nextest('nextest-version="0.9.2"'),
      ),
      2,
    );
    refused(
      ".config/nextest.toml",
      nextest(""),
      nextest("[made-up]\nx=1"),
      /unresolved configuration section made-up \(read from .config\/nextest.toml\)/,
    );
    // A setup script changes the runners only when a selected profile runs it, and that script must exist.
    const scripts = (command, used) =>
      `[scripts.setup.db]\ncommand="${command}"\n[profile.ci]\n${used ? '[[profile.default.scripts]]\nfilter="all()"\nsetup="db"\n' : ""}`;
    assert.equal(selected(".config/nextest.toml", scripts("a", false), scripts("b", false)), 0);
    assert.equal(selected(".config/nextest.toml", scripts("a", true), scripts("b", true)), 2);
    // A wrapper script is resolved through run-wrapper or list-wrapper, like a setup script.
    const wrapped = (command) =>
      `[scripts.wrapper.trace]\ncommand="${command}"\n[profile.ci]\n[[profile.default.scripts]]\nfilter="all()"\nrun-wrapper="trace"\n`;
    assert.equal(selected(".config/nextest.toml", wrapped("a"), wrapped("b")), 2);
    // A misspelled field or a non-table entry refuses instead of silently naming no script.
    const misspelled = '[profile.ci]\n[[profile.default.scripts]]\nfilter="all()"\nsetpu="db"\n';
    refused(
      ".config/nextest.toml",
      misspelled,
      misspelled,
      /unresolved nextest script field setpu \(read from .config\/nextest.toml profile.default.scripts\[0\]\)/,
    );
    const untabled = '[profile.ci]\n[profile.default]\nscripts=["db"]\n';
    refused(".config/nextest.toml", untabled, untabled, /invalid nextest script entry/);
    const missing = '[profile.ci]\n[[profile.default.scripts]]\nfilter="all()"\nsetup="absent"\n';
    refused(
      ".config/nextest.toml",
      missing,
      missing,
      /unresolved nextest script "absent" \(read from .config\/nextest.toml profile.default.scripts\[0\]\)/,
    );
    // A script name that is not a string, or a definition that is not a script table, refuses too.
    const numbered = '[profile.ci]\n[[profile.default.scripts]]\nfilter="all()"\nsetup=1\n';
    refused(
      ".config/nextest.toml",
      numbered,
      numbered,
      /invalid nextest script entry setup: expected a script name or a list of them/,
    );
    const valued =
      '[scripts.setup]\ndb=42\n[profile.ci]\n[[profile.default.scripts]]\nfilter="all()"\nsetup="db"\n';
    refused(".config/nextest.toml", valued, valued, /unresolved nextest script "db"/);
    refused("unknown.toml", "", "", /usage: config-projection/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("historical driver hunks outside owned items resolve to what they belong to", async () => {
  const { projectSourceChange } = await import("./source-projection.mjs");
  const root = scratch();
  const git = (...args) =>
    execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: "pipe" }).trim();
  const driver = [
    "use crate::thing::Helper;",
    "",
    "pub fn dispatch(verb: Verb) {",
    "    match verb {",
    "        Verb::Start => start(),",
    "        Verb::Status => status(),",
    "    }",
    "}",
    "",
    "/// Starts.",
    "#[inline]",
    "fn start() { prepare(); Helper::go(); }",
    "",
    "fn status() { Frame::read(); }",
    "",
    "fn prepare() { let ready = 1; }",
    "",
    "struct Frame;",
    "impl Frame {",
    "    fn read() { let frame = 1; }",
    "}",
    "",
    "pub(crate) fn shared() { let shared = 1; }",
    "",
    "#[cfg(test)]",
    "mod tests {",
    "    fn fixture() { let fixture = 1; }",
    "    #[test]",
    "    fn starts() { fixture(); super::prepare(); start(); }",
    "}",
    "",
  ].join("\n");
  const owners = {
    "src/driver.rs": new Map([
      ["dispatch", "onepipeline"],
      ["__arm_Start", "cmd-start"],
      ["__arm_Status", "cmd-status"],
    ]),
  };
  try {
    git("init", "-q");
    git("config", "user.name", "Projection test");
    git("config", "user.email", "projection@example.test");
    put(root, "src/driver.rs", driver);
    git("add", ".");
    git("commit", "-qm", "initial");
    const change = (from, to) => {
      const text = readFileSync(join(root, "src/driver.rs"), "utf8");
      assert.ok(text.includes(from), from);
      put(root, "src/driver.rs", text.replace(from, to));
      git("commit", "-qam", `change ${from}`);
      return projectSourceChange(
        { commit: git("rev-parse", "HEAD"), files: ["src/driver.rs"] },
        owners,
        {},
        root,
      ).files;
    };
    // A private helper belongs to its production callers; the unit test calling it does not own it.
    assert.deepEqual(change("let ready = 1;", "let ready = 2;"), ["moved/cmd-start/src/driver.rs"]);
    // Documentation and attributes belong to the item they precede.
    assert.deepEqual(change("/// Starts.", "/// Starts a run."), ["moved/cmd-start/src/driver.rs"]);
    assert.deepEqual(change("#[inline]", "#[inline(always)]"), ["moved/cmd-start/src/driver.rs"]);
    // An impl block's method belongs to its type, and the type to the code naming it.
    assert.deepEqual(change("let frame = 1;", "let frame = 2;"), [
      "moved/cmd-status/src/driver.rs",
    ]);
    // An import belongs to the declarations naming what it imports.
    assert.deepEqual(change("use crate::thing::Helper;", "use crate::thing::{Helper};"), [
      "moved/cmd-start/src/driver.rs",
    ]);
    // A unit test and its test-only fixture move with the production items they reference.
    assert.deepEqual(change("let fixture = 1;", "let fixture = 2;"), [
      "moved/cmd-start/src/driver.rs",
    ]);
    // A blank line changes no item.
    assert.deepEqual(change("struct Frame;\n", "struct Frame;\n\n"), []);
    // A crate-visible item may have callers in other modules, so it keeps the whole-file input.
    assert.deepEqual(change("let shared = 1;", "let shared = 2;"), ["src/driver.rs"]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

/** A scratch git package shaped like crates/testfakes, with a reviewed split each test then breaks. */
function fakePackage() {
  const root = scratch();
  const git = (...args) =>
    execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: "pipe" }).trim();
  git("init", "-q");
  git("config", "user.name", "Projection test");
  git("config", "user.email", "projection@example.test");
  const files = {
    "crates/testfakes/Cargo.toml":
      '[package]\nname = "fakes"\nversion = "0.0.0"\n\n[dependencies]\nserde_json = "1"\nsha2 = "0.10"\n\n[[bin]]\nname = "fake-a"\npath = "src/bin/fake-a.rs"\n',
    "crates/testfakes/src/lib.rs":
      "pub mod yaml;\n\n/// Records a call.\npub fn record() { stamp(); }\n\nfn stamp() {}\n\npub fn member() { record(); }\n",
    "crates/testfakes/src/yaml.rs": "pub fn from_str() {}\n",
    "crates/testfakes/src/bin/fake-a.rs":
      "use fakes as fake;\nfn main() { fake::record(); serde_json::json!(1); }\n",
    "crates/testfakes/src/bin/fake-b.rs":
      "use fakes as fake;\nfn main() { fake::member(); sha2::digest(); }\n",
    "tests/e2e/journey.rs": 'fn journey() { double("fake-a"); double("fake-b"); }\n',
  };
  for (const [path, text] of Object.entries(files)) put(root, path, text);
  const crate = (fileList, items, modules, selection) => ({
    files: fileList,
    items,
    modules,
    selection,
    interface: "scratch",
  });
  const spec = {
    schema: 1,
    package: "crates/testfakes",
    library: "crates/testfakes/src/lib.rs",
    crates: {
      "test-fake-codec": crate(["crates/testfakes/src/yaml.rs"], ["record"], ["yaml"], {
        everySuite: "the harness links it",
      }),
      "test-fake-member": crate([], ["member"], [], { via: "agent fakes link it" }),
      "fake-a": crate(["crates/testfakes/src/bin/fake-a.rs"], [], [], { doubles: ["fake-a"] }),
      "fake-b": crate(["crates/testfakes/src/bin/fake-b.rs"], [], [], { doubles: ["fake-b"] }),
    },
    wired: [],
    historical: [],
  };
  const write = (value) =>
    put(root, "scripts/modularization/test-fakes.json", JSON.stringify(value));
  write(spec);
  git("add", ".");
  git("commit", "-qm", "initial");
  return { root, git, spec, write };
}

test("the test-fake split owns every fake and derives each crate's real dependencies", async () => {
  const { testFakes, testFakeManifestChanges } = await import("./test-fakes.mjs");
  const { root, git, spec, write } = fakePackage();
  try {
    const fakes = testFakes(root);
    assert.deepEqual(fakes.crates["fake-a"].deps, ["test-fake-codec"]);
    assert.deepEqual(fakes.crates["fake-a"].external, ["serde_json"]);
    assert.deepEqual(fakes.crates["fake-b"].deps, ["test-fake-member"]);
    assert.deepEqual(fakes.crates["fake-b"].external, ["sha2"]);
    // The private helper moves with the item calling it, and so does its dependency edge.
    assert.deepEqual(fakes.crates["test-fake-member"].deps, ["test-fake-codec"]);
    // A dependency change charges only the crates linking it; a [[bin]] change its crate; package fields all.
    const manifest = readFileSync(join(root, "crates/testfakes/Cargo.toml"), "utf8");
    assert.deepEqual(
      testFakeManifestChanges(fakes, manifest, manifest.replace('sha2 = "0.10"', 'sha2 = "0.11"')),
      ["moved/fake-b/crates/testfakes/Cargo.toml"],
    );
    assert.deepEqual(
      testFakeManifestChanges(
        fakes,
        manifest,
        manifest.replace('path = "src/bin/fake-a.rs"', 'path = "src/bin/a.rs"'),
      ),
      ["moved/fake-a/crates/testfakes/Cargo.toml"],
    );
    assert.equal(
      testFakeManifestChanges(
        fakes,
        manifest,
        manifest.replace('version = "0.0.0"', 'version = "0.0.1"'),
      ).length,
      4,
    );
    const refuses = (mutate, message, extra) => {
      const broken = structuredClone(spec);
      mutate(broken);
      write(broken);
      extra?.();
      assert.throws(() => testFakes(root), message);
      write(spec);
    };
    refuses((value) => {
      value.crates["fake-b"].files = [];
      value.crates["fake-b"].selection = { via: "nothing" };
    }, /unowned test-fake file crates\/testfakes\/src\/bin\/fake-b.rs/);
    refuses((value) => {
      value.crates["fake-b"].selection = { via: "nothing" };
    }, /unowned test-fake binary fake-b/);
    refuses((value) => {
      value.crates["test-fake-member"].items = [];
    }, /unowned test-fake library item member/);
    refuses((value) => {
      value.crates["test-fake-codec"].modules = [];
    }, /unowned test-fake library module yaml/);
    refuses((value) => {
      value.crates["fake-a"].selection = { doubles: ["fake-b"] };
    }, /test-fake binary fake-b is owned twice|not in that crate/);
    // Fakes a test starts and dependencies the manifest declares must have an owner too.
    put(root, "tests/e2e/journey.rs", 'fn journey() { double("fake-z"); }\n');
    git("add", ".");
    assert.throws(
      () => testFakes(root),
      /unresolved test fake fake-z \(read from tests\/e2e\/journey.rs\)/,
    );
    git("checkout", "HEAD", "--", ".");
    put(
      root,
      "crates/testfakes/Cargo.toml",
      manifest.replace('sha2 = "0.10"', 'sha2 = "0.10"\ntokio = "1"'),
    );
    assert.throws(() => testFakes(root), /unattributed test-fake dependency tokio/);
    put(root, "crates/testfakes/Cargo.toml", manifest);
    assert.ok(testFakes(root));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("historical test-fake library hunks move to the crate owning their item", async () => {
  const { testFakes, testFakeOwners } = await import("./test-fakes.mjs");
  const { projectSourceChange } = await import("./source-projection.mjs");
  const { root, git } = fakePackage();
  const library = "crates/testfakes/src/lib.rs";
  try {
    const owners = testFakeOwners(testFakes(root));
    const change = (from, to) => {
      const text = readFileSync(join(root, library), "utf8");
      assert.ok(text.includes(from), from);
      put(root, library, text.replace(from, to));
      git("commit", "-qam", `change ${from}`);
      return projectSourceChange(
        { commit: git("rev-parse", "HEAD"), files: [library] },
        owners,
        {},
        root,
      ).files;
    };
    assert.deepEqual(
      change("pub fn member() { record(); }", "pub fn member() { record(); record(); }"),
      [`moved/test-fake-member/${library}`],
    );
    // A private helper and a doc comment belong to the codec item they serve.
    assert.deepEqual(change("fn stamp() {}", "fn stamp() { let _ = 1; }"), [
      `moved/test-fake-codec/${library}`,
    ]);
    assert.deepEqual(change("/// Records a call.", "/// Records one call."), [
      `moved/test-fake-codec/${library}`,
    ]);
    // A module declaration belongs to the crate owning that module.
    assert.deepEqual(change("pub mod yaml;", "pub(crate) mod yaml;"), [
      `moved/test-fake-codec/${library}`,
    ]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("historical CLI flag hunks select their command rather than the enum facade", async () => {
  const { projectSourceChange } = await import("./source-projection.mjs");
  const root = scratch();
  const git = (...args) =>
    execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: "pipe" }).trim();
  try {
    git("init", "-q");
    git("config", "user.name", "Projection test");
    git("config", "user.email", "projection@example.test");
    const before =
      "pub enum Command {\n    Start {\n        verbose: bool,\n    },\n    Status,\n}\n";
    put(root, "src/cli.rs", before);
    git("add", ".");
    git("commit", "-qm", "initial");
    put(root, "src/cli.rs", before.replace("verbose: bool", "quiet: bool"));
    git("add", ".");
    git("commit", "-qm", "flag");
    const owners = {
      "src/cli.rs": new Map([
        ["Command", "onepipeline"],
        ["__variant_Command_Start", "cmd-start"],
        ["__variant_Command_Status", "cmd-status"],
      ]),
    };
    const result = projectSourceChange(
      { commit: git("rev-parse", "HEAD"), files: ["src/cli.rs"] },
      owners,
      {},
      root,
    );
    assert.deepEqual(result.files, ["moved/cmd-start/src/cli.rs"]);
    put(root, "src/cli.rs", before.replace("pub enum", "pub(crate) enum"));
    git("add", ".");
    git("commit", "-qm", "facade visibility");
    const facade = projectSourceChange(
      { commit: git("rev-parse", "HEAD"), files: ["src/cli.rs"] },
      owners,
      {},
      root,
    );
    assert.ok(facade.files.includes("moved/onepipeline/src/cli.rs"));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("engine proposal CLI isolates query and effect capabilities, refuses cycles and missing source items, then recovers", () => {
  const root = scratch();
  const cli = resolve("scripts/modularization/engine-graph-cli.mjs");
  const graphCli = resolve("scripts/modularization/target-graph.mjs");
  try {
    const basePath = join(root, "base.json");
    const proposalPath = join(root, "proposal.json");
    const output = join(root, "mapped.json");
    execFileSync(process.execPath, [graphCli, basePath]);
    const proposal = JSON.parse(
      readFileSync("scripts/modularization/engine-subsystems.json", "utf8"),
    );
    const run = () =>
      spawnSync(process.execPath, [cli, basePath, proposalPath, output], { encoding: "utf8" });
    put(root, "proposal.json", proposal);
    let response = run();
    assert.equal(response.status, 0, response.stderr);
    const mapped = JSON.parse(readFileSync(output, "utf8"));
    assert.deepEqual(mapped, JSON.parse(readFileSync(basePath, "utf8")));
    const graph = compileMapping({
      crates: Object.fromEntries(
        Object.entries(mapped.crates).map(([name, crate]) => [
          name,
          { paths: crate.files, deps: crate.deps },
        ]),
      ),
      suites: {
        status: { paths: [], crates: ["cmd-status"] },
        start: { paths: [], crates: ["cmd-start"] },
        reply: { paths: [], crates: ["cmd-reply"] },
        review: { paths: [], crates: ["cmd-review"] },
        readjust: { paths: [], crates: ["cmd-readjust"] },
      },
    });
    for (const name of ["scheduler", "publication", "plan-edits", "agent-execution"])
      assert(!graph.suites.status.crates.has(name), name);
    assert(graph.suites.status.crates.has("checkpoint"));
    assert.equal(mapped.schema, 3);
    assert.deepEqual(mapped.engineSubsystems, proposal);
    // Test fakes are split by scripts/modularization/test-fakes.json, not grouped as engine subsystems.
    for (const name of ["test-scenario-schema", "test-agent-process", "test-host-process"])
      assert(!Object.hasOwn(mapped.engineSubsystems.subsystems, name), name);
    assert(graph.suites.start.crates.has("publication"));
    assert(!graph.suites.reply.crates.has("scheduler"));
    for (const name of ["edit-reconciler", "publication", "review-replies"])
      assert(graph.suites.reply.crates.has(name), name);
    assert(graph.suites.review.crates.has("scheduler"));
    assert(graph.suites.readjust.crates.has("scheduler"));
    assert(graph.suites.status.crates.has("processes"));
    assert.equal(mapped.commands["src/reply.rs"], undefined);
    assert(mapped.crates["review-replies"].files.includes("src/reply.rs"));
    assert(
      graph.suites.reply.crates.has("run-summary"),
      "journal writers retain real summary maintenance",
    );
    const validLayout = JSON.parse(readFileSync(basePath, "utf8"));
    for (const corrupt of [
      (layout) => {
        layout.schema = 99;
      },
      (layout) => {
        layout.crates["run-paths"].files.push("../outside.rs");
      },
      (layout) => {
        layout.metadata = "unreviewed";
      },
    ]) {
      const invalid = structuredClone(validLayout);
      corrupt(invalid);
      put(root, "base.json", invalid);
      response = run();
      assert.equal(response.status, 1);
      assert.match(response.stderr, /layout differs from the reviewed source-derived layout/);
    }
    put(root, "base.json", validLayout);
    const invalidOutput = spawnSync(process.execPath, [cli, basePath, proposalPath, root], {
      encoding: "utf8",
    });
    assert.equal(invalidOutput.status, 1);
    assert.match(invalidOutput.stderr, /EISDIR|EPERM|EACCES/);
    assert.match(invalidOutput.stderr, /output parent is writable/);
    const usage = spawnSync(process.execPath, [cli], { encoding: "utf8" });
    assert.equal(usage.status, 1);
    assert.match(usage.stderr, /usage: engine-graph/);
    const validProposal = structuredClone(proposal);
    const refusals = [
      [
        "inline edit application",
        (p) => {
          // Reply and attest reach inline application through envelope-submission, which owns submit_envelope.
          p.subsystems["channel-submission"].deps = p.subsystems["channel-submission"].deps.filter(
            (name) => name !== "envelope-submission",
          );
        },
        /inline edit application missing/,
      ],
      [
        "undriven execution continuation",
        (p) => {
          p.subsystems["cmd-review"].deps = p.subsystems["cmd-review"].deps.filter(
            (name) => name !== "scheduler",
          );
        },
        /undriven execution continuation missing/,
      ],
      [
        "real summary maintenance",
        (p) => {
          p.subsystems.journal.deps = p.subsystems.journal.deps.filter(
            (name) => name !== "run-summary",
          );
        },
        /journal writer must retain real summary maintenance/,
      ],
      [
        "canonical channel wire type",
        (p) => {
          p.subsystems["planner-channel"].deps = p.subsystems["planner-channel"].deps.filter(
            (name) => name !== "plan-model",
          );
        },
        /channel Command must retain its canonical plan Node dependency/,
      ],
      [
        "unregistered command",
        (p) => {
          p.subsystems["cmd-ghost"] = structuredClone(p.subsystems["cmd-status"]);
        },
        /invalid subsystem cmd-ghost/,
      ],
      [
        "null subsystem",
        (p) => {
          p.subsystems["run-paths"] = null;
        },
        /invalid subsystem run-paths/,
      ],
      [
        "null item",
        (p) => {
          p.subsystems["run-paths"].items = [null];
        },
        /invalid subsystem items/,
      ],
      [
        "schema",
        (p) => {
          p.schema = 99;
        },
        /invalid engine subsystem proposal/,
      ],
      [
        "unknown field",
        (p) => {
          p.subsystems["run-paths"].unused = true;
        },
        /invalid subsystem run-paths/,
      ],
      [
        "dependency",
        (p) => {
          p.subsystems["run-paths"].deps = ["missing-crate"];
        },
        /invalid subsystem run-paths/,
      ],
      [
        "external root",
        (p) => {
          p.subsystems["run-paths"].external = ["../outside"];
        },
        /invalid subsystem external roots/,
      ],
      [
        "source path",
        (p) => {
          p.subsystems["run-paths"].items[0].file = "../outside.rs";
        },
        /invalid subsystem items/,
      ],
      [
        "symbol type",
        (p) => {
          p.subsystems["run-paths"].items[0].symbols = [null];
        },
        /invalid subsystem items/,
      ],
      [
        "duplicate file",
        (p) => {
          p.subsystems["run-labels"].files.push("src/labels.rs");
        },
        /duplicate subsystem file/,
      ],
      [
        "unknown file",
        (p) => {
          p.subsystems["run-labels"].files = ["src/missing.rs"];
        },
        /unknown or multiply owned subsystem file/,
      ],
      [
        "duplicate item",
        (p) => {
          p.subsystems["run-paths"].items[0].symbols.push("RunPaths");
        },
        /duplicate subsystem item/,
      ],
      [
        "foreign command",
        (p) => {
          p.subsystems["cmd-status"].deps.push("cmd-reply");
        },
        /subsystem command isolation failed/,
      ],
    ];
    for (const [name, mutate, message] of refusals) {
      const invalid = structuredClone(validProposal);
      mutate(invalid);
      put(root, "proposal.json", invalid);
      response = run();
      assert.equal(response.status, 1, name);
      assert.match(response.stderr, message, name);
    }
    proposal.subsystems["journal-reader"].deps.push("checkpoint");
    put(root, "proposal.json", proposal);
    response = run();
    assert.equal(response.status, 1);
    assert.match(response.stderr, /cycle.*checkpoint|checkpoint.*->/s);
    proposal.subsystems["journal-reader"].deps.pop();
    proposal.subsystems["run-paths"].items[0].symbols.push("missing_path_operation");
    put(root, "proposal.json", proposal);
    response = run();
    assert.equal(response.status, 1);
    assert.match(response.stderr, /missing subsystem item.*missing_path_operation/);
    proposal.subsystems["run-paths"].items[0].symbols.pop();
    put(root, "proposal.json", proposal);
    response = run();
    assert.equal(response.status, 0, response.stderr);
    assert.deepEqual(JSON.parse(readFileSync(output, "utf8")).crates, mapped.crates);
    // Valid declared external roots replace the derived ones for that subsystem in the emitted graph.
    proposal.subsystems["run-paths"].external = ["serde", "serde_json"];
    put(root, "proposal.json", proposal);
    response = run();
    assert.equal(response.status, 0, response.stderr);
    assert.deepEqual(JSON.parse(readFileSync(output, "utf8")).externalRoots["run-paths"], [
      "serde",
      "serde_json",
    ]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("the test split assigns every phase to one suite, under the threshold, and refuses a dropped, duplicated or misplaced test", async () => {
  const { checkSplit, linuxSeconds, targetCatalog, THRESHOLD, SPLIT_PATH } = await import(
    "./test-split.mjs"
  );
  const catalog = targetCatalog();
  const text = readFileSync(SPLIT_PATH, "utf8");
  const checked = spawnSync(
    process.execPath,
    ["scripts/modularization/test-split.mjs", "--check"],
    {
      encoding: "utf8",
    },
  );
  assert.equal(checked.status, 0, checked.stderr);
  const suiteOf = checkSplit(text, catalog);
  // Modules the profile never timed are named, not silently weighed as free.
  const { unmeasuredModules } = await import("./test-split.mjs");
  assert.deepEqual(JSON.parse(text).unmeasured, unmeasuredModules(catalog));
  assert.ok(JSON.parse(text).unmeasured.includes("closures"));
  // A catalog the profile timed none of cannot be weighed at all, so it is refused.
  const untimed = Object.fromEntries(
    Object.entries(catalog.tests).filter(([, test]) => test.original.startsWith("closures::")),
  );
  assert.ok(Object.keys(untimed).length);
  assert.throws(
    () => linuxSeconds({ ...catalog, tests: untimed }),
    /the profile measured no Linux seconds for any catalog module/,
  );
  assert.deepEqual([...suiteOf.keys()].sort(), Object.keys(catalog.tests).sort());
  const seconds = linuxSeconds(catalog);
  const total = Object.values(seconds).reduce((sum, value) => sum + value, 0);
  const bySuite = {};
  for (const [id, suite] of suiteOf) bySuite[suite] = (bySuite[suite] ?? 0) + seconds[id];
  for (const [suite, value] of Object.entries(bySuite))
    assert.ok(value / total <= THRESHOLD, `${suite} holds ${value / total}`);
  const split = JSON.parse(text);
  const [first, second] = Object.keys(split.suites).filter(
    (name) => split.suites[name].tests.length > 1,
  );
  const refused = (mutate, pattern) => {
    const copy = structuredClone(split);
    mutate(copy);
    assert.throws(() => checkSplit(JSON.stringify(copy, null, 2), catalog), pattern);
  };
  refused((copy) => copy.suites[first].tests.pop(), /is in no split suite/);
  refused(
    (copy) => copy.suites[second].tests.push(copy.suites[first].tests[0]),
    /is in both|takes .* from/,
  );
  refused((copy) => copy.suites[first].tests.push(copy.suites[first].tests[0]), /is in both/);
  refused((copy) => copy.suites[first].tests.push("views::no_such_test"), /unknown test/);
  // An array key-coerces to its one string in `Object.hasOwn`, but a Map keeps it distinct.
  refused((copy) => {
    copy.suites[first].tests[0] = [copy.suites[first].tests[0]];
  }, /invalid test id/);
  refused((copy) => {
    copy.suites[first].parent = "cmd-label";
  }, /invalid split suite|takes .* from/);
  refused((copy) => {
    copy.typo = 1;
  }, /invalid test split/);
  refused((copy) => {
    copy.unmeasured = [1];
  }, /invalid test split/);
  // Duplicates are found whatever the layout: JSON.parse would keep only the last.
  const minified = JSON.stringify(split).replace(
    `"${first}":`,
    `"${first}":{"parent":"x","tests":["x"]},"${first}":`,
  );
  assert.throws(() => checkSplit(minified, catalog), new RegExp(`names ${first} twice`));
  const twice = text.replace(
    `    "${first}": {`,
    `    "${first}": { "parent": "x", "tests": ["x"] },\n    "${first}": {`,
  );
  assert.throws(() => checkSplit(twice, catalog), new RegExp(`names ${first} twice`));
});

test("test-split CLI writes the derived split, refuses a stale or invalid one with its repair, and recovers", {
  timeout: 300000,
}, () => {
  const root = scratch();
  try {
    execFileSync("git", ["clone", "--quiet", "--shared", ".", root], { stdio: "pipe" });
    const cli = (...args) =>
      spawnSync(process.execPath, ["scripts/modularization/test-split.mjs", ...args], {
        cwd: root,
        encoding: "utf8",
      });
    const path = join(root, "scripts/modularization/test-split.json");
    const committed = readFileSync(path, "utf8");
    rmSync(path);
    const written = cli("--write");
    assert.equal(written.status, 0, written.stderr);
    assert.equal(readFileSync(path, "utf8"), committed);
    // A split the checker accepts but its rule would not produce is stale.
    const split = JSON.parse(committed);
    const [a, b] = Object.keys(split.suites).filter(
      (name) => name.startsWith("cmd-start--continuation--") && split.suites[name].tests.length > 1,
    );
    split.suites[b].tests.push(split.suites[a].tests.pop());
    writeFileSync(path, `${JSON.stringify(split, null, 2)}\n`);
    const stale = cli("--check");
    assert.equal(stale.status, 1);
    assert.match(
      stale.stderr,
      /differs from the split its rule derives\nnext: regenerate with test-split.mjs --write/,
    );
    split.suites[a].tests.push(split.suites[b].tests.pop());
    split.suites[a].tests.pop();
    writeFileSync(path, `${JSON.stringify(split, null, 2)}\n`);
    const dropped = cli("--check");
    assert.equal(dropped.status, 1);
    assert.match(dropped.stderr, /is in no split suite/);
    assert.equal(cli("--write").status, 0);
    assert.equal(cli("--check").status, 0);
    const usage = cli("--both");
    assert.equal(usage.status, 1);
    assert.match(usage.stderr, /usage: test-split.mjs --write \| --check/);
    rmSync(path);
    mkdirSync(path);
    const unreadable = cli("--check");
    assert.equal(unreadable.status, 1);
    assert.match(unreadable.stderr, /next: check the named path's type/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("ci-wall.json is refused unless every measured duration is a finite duration in place", async () => {
  const { readWall } = await import("./split-replay.mjs");
  const text = readFileSync("scripts/modularization/ci-wall.json", "utf8");
  assert.ok(readWall(text).platforms.linux.legs.length);
  const refused = (mutate, pattern) => {
    const wall = JSON.parse(text);
    mutate(wall);
    assert.throws(() => readWall(JSON.stringify(wall)), pattern);
  };
  refused((wall) => {
    wall.typo = 1;
  }, /unknown field typo/);
  refused((wall) => {
    wall.platforms.linux.legs[0].e2eSeconds = "3502";
  }, /linux.legs\[0\].e2eSeconds is not a duration/);
  refused((wall) => {
    wall.platforms.macos.legs[0].e2eSeconds = Number.MAX_VALUE;
  }, /spends more on bootstrap and e2e than the job took/);
  refused((wall) => {
    delete wall.platforms.windows;
  }, /windows is not an object/);
  refused((wall) => {
    wall.platforms.windows.breakdown.legs.pop();
  }, /must break down every leg/);
  refused((wall) => {
    wall.platforms.linux.breakdown.segments[0].seconds += 60;
  }, /does not add up to its job/);
  refused((wall) => {
    wall.limitSeconds = 0;
  }, /limitSeconds must be a positive number/);
});

test("a sectioned document read follows the section its needles name and keeps the whole file when they name none", async () => {
  const { scope, scopeChanged, scopePath } = await import("./contract-sections.mjs");
  const doc = [
    "# Record",
    "",
    "## 44. Earlier",
    "Mentions entry 45. in passing. Shared phrase.",
    "## 45. Reviewer",
    "```json",
    '{ "reviewer": { "flag": "--x" } }',
    "# not a heading inside a fence",
    "```",
    "### 45a. Detail",
    "Refined text.",
    "## 46. Later",
    '{ "other": 1 } Shared phrase.',
    "",
  ].join("\n");
  const entry = ["\n## ", "45.", "```json", "```"];
  const held = scope(doc, entry);
  assert.deepEqual(
    held.map((section) => section.key),
    ["## 45. Reviewer"],
  );
  assert.match(
    scopePath("docs/contract-divergences.md", entry, held),
    /#45-reviewer-[0-9a-f]{10}$/,
  );
  // A key the reader finds inside the entry refines it, a key naming another section adds that
  // section, and a key the document repeats outside the scope leaves the whole file.
  assert.deepEqual(
    scope(doc, ["45.", "Refined text."]).map((section) => section.key),
    ["## 45. Reviewer"],
  );
  assert.deepEqual(
    scope(doc, ["45.", '"other"']).map((section) => section.key),
    ["## 45. Reviewer", "## 46. Later"],
  );
  assert.equal(scope(doc, ["45.", "Shared phrase."]), null);
  assert.equal(scope(doc, ["Shared phrase."]), null);
  assert.equal(scope(doc, ["```json"]), null);
  assert.deepEqual(
    scope(doc, ['"reviewer"']).map((section) => section.key),
    ["## 45. Reviewer"],
  );
  assert.equal(scopeChanged(doc, doc.replace("Later", "Last"), entry), false);
  assert.equal(scopeChanged(doc, doc.replace("Refined", "Changed"), entry), true);
  assert.equal(scopeChanged(doc, doc.replace("## 45. Reviewer", "## 47. Reviewer"), entry), true);
  assert.equal(scopeChanged(doc, `${doc}## 45. Again\n`, entry), true);
});

test("a test file's hunks reach the items they touch: doc comments with their item, blank lines nowhere, imports everywhere", async () => {
  const { projectSourceChange } = await import("./source-projection.mjs");
  const { typeOwners } = await import("./split-replay.mjs");
  const root = scratch();
  const git = (...args) =>
    execFileSync(
      "git",
      [
        "-c",
        "user.name=test",
        "-c",
        "user.email=test@localhost",
        "-c",
        "commit.gpgsign=false",
        ...args,
      ],
      { cwd: root, encoding: "utf8", stdio: "pipe" },
    ).trim();
  const file = "tests/e2e/demo.rs";
  const source = (doc, gap, imports) =>
    [
      `use std::path::Path;${imports}`,
      "",
      `/// ${doc}`,
      "pub fn helper_a() -> u8 {",
      "    1",
      "}",
      gap,
      "pub struct Shape {",
      "    pub side: u8,",
      "}",
      "",
      "pub fn helper_b() -> Shape {",
      "    Shape { side: 2 }",
      "}",
      "",
    ].join("\n");
  const commit = (text) => {
    put(root, file, text);
    git("add", "-A");
    git("commit", "--quiet", "-m", "change");
    return git("rev-parse", "HEAD");
  };
  const previous = process.cwd();
  try {
    git("init", "--quiet");
    commit(source("Helper a.", "", ""));
    const owners = {
      [file]: new Map([
        ["helper_a", new Set(["suite:a"])],
        ["helper_b", new Set(["suite:b"])],
      ]),
    };
    process.chdir(root);
    typeOwners(owners);
    process.chdir(previous);
    assert.deepEqual([...owners[file].get("Shape")], ["suite:b"]);
    const project = (sha, options) =>
      projectSourceChange({ commit: sha, files: [file] }, owners, {}, root, options).projected[0];
    const documented = commit(source("Helper a, reworded.", "", ""));
    assert.deepEqual(project(documented, { testItems: true }), {
      file,
      destinations: ["suite:a"],
      retainedWholeFile: false,
    });
    // Without item attribution the same doc-comment hunk falls outside every item.
    assert.equal(project(documented, {}).retainedWholeFile, true);
    const spaced = commit(source("Helper a, reworded.", "\n\n", ""));
    assert.deepEqual(project(spaced, { testItems: true }).destinations, []);
    const reshaped = commit(
      source("Helper a, reworded.", "\n\n", "").replace("side: u8", "side: u16"),
    );
    assert.deepEqual(project(reshaped, { testItems: true }).destinations, ["suite:b"]);
    const imported = commit(source("Helper a, reworded.", "\n\n", "\nuse std::fs;"));
    assert.deepEqual(project(imported, { testItems: true }), {
      file,
      destinations: ["suite:a", "suite:b"],
      retainedWholeFile: true,
    });
  } finally {
    process.chdir(previous);
    rmSync(root, { recursive: true, force: true });
  }
});

test("split replay CLI replays crates and test split on the 144 PRs and renders fresh test-split documentation", {
  timeout: 1500000,
}, async () => {
  const root = scratch();
  try {
    const { checkSplit, targetCatalog, SPLIT_PATH } = await import("./test-split.mjs");
    const { splitAppendix, SPLIT_MARKER } = await import("./split-report.mjs");
    const output = join(root, "split.json");
    const run = spawnSync(process.execPath, ["scripts/modularization/split-replay.mjs", output], {
      encoding: "utf8",
    });
    assert.equal(run.status, 0, run.stderr);
    const evidence = JSON.parse(readFileSync(output, "utf8"));
    const { result, split } = evidence;
    assert.equal(result.rows.length, 144);
    // Every phase runs in exactly one suite of the replayed mapping.
    const suiteOf = checkSplit(readFileSync(SPLIT_PATH, "utf8"), targetCatalog());
    assert.equal(result.suites, new Set(suiteOf.values()).size);
    assert.deepEqual(
      Object.keys(result.mapping.selection.suites).sort(),
      [...new Set(suiteOf.values())].sort(),
    );
    for (const platform of ["linux", "macos", "windows", "local"]) {
      assert.ok(
        result.platforms[platform].secondsSkip > 0 && result.platforms[platform].secondsSkip < 1,
      );
      // The step sums per merged suite, the replay per suite, so they agree to rounding.
      assert.ok(
        Math.abs(
          result.splitMigration.at(-1).secondsSkip[platform] -
            result.platforms[platform].secondsSkip,
        ) < 1e-9,
      );
    }
    assert.equal(result.splitMigration.at(-1).suiteSkip, result.platforms.linux.suiteSkip);
    assert.equal(result.splitMigration[0].suites, 43);
    for (const platform of ["linux", "macos", "windows"]) {
      const wall = result.wall[platform];
      assert.ok(wall.residual + wall.e2e === wall.today);
      assert.ok(wall.projected.median <= wall.projected.max && wall.projected.max <= wall.today);
    }
    // The decision summary's counts come from the same rows, and each upper bound only adds skipping.
    const { causes } = result;
    const linuxShare = (row) =>
      row.affected.reduce((sum, suite) => sum + result.platforms.linux.suiteSeconds[suite], 0) /
      result.platforms.linux.secondsPerRun;
    assert.equal(causes.broad, result.rows.filter((row) => linuxShare(row) >= 0.9).length);
    assert.equal(
      causes.lock.sibling + causes.lock.thirdParty + causes.lock.both,
      causes.prs["lockfile resolution"],
    );
    const together = causes.freed.at(-1);
    for (const kind of together.kinds) {
      const alone = causes.freed.find(
        (entry) => entry.kinds.length === 1 && entry.kinds[0] === kind,
      );
      if (!alone) continue;
      assert.ok(
        together.secondsSkip >= alone.secondsSkip &&
          alone.secondsSkip >= result.platforms.linux.secondsSkip,
      );
    }
    const wall = JSON.parse(readFileSync("scripts/modularization/ci-wall.json", "utf8"));
    for (const [platform, data] of Object.entries(wall.platforms)) {
      const legs = data.breakdown.legs ?? [{ segments: data.breakdown.segments }];
      for (const [index, leg] of legs.entries())
        assert.ok(
          Math.abs(
            leg.segments.reduce((sum, part) => sum + part.seconds, 0) - data.legs[index].jobSeconds,
          ) < 5,
          `${platform} leg ${index + 1}`,
        );
    }
    // The sibling-engine paths: each listed PR moved only sibling packages and its lockfile alone
    // selects at least 90%, the adapter projection only adds skipping, and no production crate
    // keeps a unit-test-only import as an external root.
    const { siblings } = result;
    assert.ok(siblings.prs.length > 0);
    for (const entry of siblings.prs) {
      assert.ok(
        entry.moved.every((name) => /^one(?!pipeline)[a-z]/.test(name)),
        entry.commit,
      );
      assert.ok(entry.lockShare >= 0.9, entry.commit);
    }
    assert.ok(siblings.adapters.secondsSkip >= result.platforms.linux.secondsSkip);
    assert.ok(siblings.adapters.stillBroad <= siblings.prs.length);
    for (const [name, crate] of Object.entries(result.mapping.crates))
      if (!name.startsWith("runner-"))
        assert.ok(
          !crate.external.includes("onepipeline-testfakes") ||
            /^(fake-|test-|scripted|label-strict)/.test(name),
          `${name} keeps a unit-test import as a production root`,
        );
    // A cross-command part links the commands it drives, not the facade, unless a phase is unattributed.
    const crates = result.mapping.crates;
    const parts = Object.keys(split.suites).filter((name) => name.startsWith("cross-command--"));
    assert.ok(parts.some((name) => !crates[`runner-${name}`].deps.includes("onepipeline")));
    assert.ok(parts.some((name) => crates[`runner-${name}`].deps.includes("cli-dispatcher")));
    assert.ok(Object.keys(result.scopes).length > 0);
    assert.ok(
      result.rows.some((row) =>
        row.effectiveFiles.some((file) => Object.hasOwn(result.scopes, file)),
      ),
    );
    const doc = readFileSync("docs/modularization-test-split.md", "utf8");
    const targetDoc = readFileSync("docs/modularization-target-layout.md", "utf8");
    assert.equal(doc.split(SPLIT_MARKER).length, 2);
    assert.equal(
      `${SPLIT_MARKER}${doc.split(SPLIT_MARKER)[1]}`,
      splitAppendix(evidence, targetDoc),
    );
    assert.match(doc, /\| linux \| 6\.61% \| 2\.55% \| 13\.52% \| 6\.39% \|/);
    const renderer = resolve("scripts/modularization/split-report.mjs");
    const render = () =>
      spawnSync(process.execPath, [renderer, output], { cwd: root, encoding: "utf8" });
    put(root, "docs/modularization-target-layout.md", targetDoc);
    put(root, "docs/modularization-test-split.md", "prefix kept\n");
    assert.match(render().stderr, /expected one test split marker/);
    put(root, "docs/modularization-test-split.md", `prefix kept\n${SPLIT_MARKER}\n`);
    const rendered = render();
    assert.equal(rendered.status, 0, rendered.stderr);
    assert.equal(
      readFileSync(join(root, "docs/modularization-test-split.md"), "utf8"),
      `prefix kept\n${splitAppendix(evidence, targetDoc)}`,
    );
    const refusedRender = (mutate, pattern) => {
      const copy = structuredClone(evidence);
      mutate(copy);
      put(root, "split.json", copy);
      const refused = render();
      assert.equal(refused.status, 1);
      assert.match(refused.stderr, pattern);
    };
    refusedRender((copy) => {
      copy.typo = true;
    }, /unknown split evidence field/);
    refusedRender((copy) => {
      copy.result.rows[0].affected = [];
    }, /row selection differs from its inputs/);
    refusedRender((copy) => {
      copy.result.platforms.linux.secondsSkip /= 2;
    }, /seconds skipped differs from its rows and weights: linux/);
    refusedRender((copy) => {
      copy.result.wall.macos.projected.max = Number.NaN;
    }, /invalid wall projection: macos/);
    // An inflated denominator with the shares rescaled to match is still refused.
    refusedRender((copy) => {
      const linux = copy.result.platforms.linux;
      const ran = (1 - linux.secondsSkip) * linux.secondsPerRun;
      linux.secondsPerRun *= 2;
      linux.secondsSkip = 1 - ran / linux.secondsPerRun;
      copy.result.splitMigration.at(-1).secondsSkip.linux = linux.secondsSkip;
    }, /seconds per run differs from its suite weights: linux/);
    refusedRender((copy) => {
      copy.result.wall.local.projected.median = -1;
    }, /invalid wall projection: local/);
    refusedRender((copy) => {
      delete copy.result.wallBreakdown.windows;
    }, /residual breakdown for every job/);
    refusedRender((copy) => {
      const [name] = Object.keys(copy.split.suites);
      copy.split.suites[name].tests.push(copy.split.suites[name].tests[0]);
    }, /is not a replayed suite with its own tests/);
    refusedRender((copy) => {
      copy.result.originalTests = 0;
    }, /positive phase and original-test counts/);
    refusedRender((copy) => {
      copy.result.wall.linux.projected.over = -1;
    }, /invalid wall projection: linux/);
    refusedRender((copy) => {
      copy.result.causes.lock.sibling = -1;
      copy.result.causes.lock.both += 1;
    }, /decision summary differs from its rows/);
    refusedRender((copy) => {
      copy.result.siblings.adapters.stillBroad = copy.result.siblings.prs.length + 1;
    }, /sibling-engine paths differ from their rows/);
    refusedRender((copy) => {
      copy.result.causes.broad += 1;
    }, /decision summary differs from its rows/);
    refusedRender((copy) => {
      copy.result.siblings.adapters.secondsSkip = 0;
    }, /sibling-engine paths differ from their rows/);
    // Figures that are ordered and in range but do not follow from the rows are refused too.
    refusedRender((copy) => {
      copy.result.wall.linux.projected.median -= 1;
    }, /invalid wall projection: linux/);
    refusedRender((copy) => {
      const [kind] = Object.keys(copy.result.causes.prs).filter(
        (name) => name !== "lockfile resolution" && copy.result.causes.prs[name] > 0,
      );
      copy.result.causes.prs[kind] -= 1;
    }, /decision summary differs from its rows/);
    refusedRender((copy) => {
      copy.result.siblings.prs[0].lockShare = 0.95;
    }, /sibling-engine paths differ from their rows/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("engine split attributes assertions to the crates naming their literals in production code, holds every phase in one place, and refuses a misplaced test", {
  timeout: 300000,
}, async () => {
  const { assertionText, checkEngineSplit, ENGINE_PATH, literalOwners, PARENTS, targetLayout } =
    await import("./engine-tests.mjs");
  // A crate names a literal only in its own production code: not in a comment, not in its unit
  // tests, and not in an item another crate takes from its file.
  const root = scratch();
  try {
    put(
      root,
      "src/a.rs",
      [
        '// "in-a-comment"',
        'pub fn emit() -> &\'static str { "node-settled" }',
        'pub fn taken() -> &\'static str { "taken-away" }',
        "#[cfg(test)]",
        'mod tests { fn t() { assert_eq!(super::emit(), "only-in-tests"); } }',
      ].join("\n"),
    );
    put(root, "src/b.rs", 'pub fn read(kind: &str) -> bool { kind == "node-settled" }');
    const owners = literalOwners({
      crates: { a: { files: [join(root, "src/a.rs")] }, b: { files: [join(root, "src/b.rs")] } },
      symbolSources: { b: [{ file: join(root, "src/a.rs"), symbols: ["taken"] }] },
    });
    assert.deepEqual([...owners.get("node-settled")].sort(), ["a", "b"]);
    assert.deepEqual([...owners.get("taken-away")], ["b"]);
    assert.equal(owners.get("in-a-comment"), undefined);
    assert.equal(owners.get("only-in-tests"), undefined);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
  const { targetCatalog } = await import("./test-split.mjs");
  // Only what an assertion compares against counts: its literals and the bindings it names.
  const text = assertionText(`{
    let setup = "plan-written";
    let expected = "node-settled";
    world.start(&["start", "plan.yaml"]);
    assert_eq!(events_of(&journal, "kind"), expected);
    assert!(stdout.contains("queued"), "the run is {}", "running");
  }`);
  assert.match(text, /"node-settled"/);
  assert.match(text, /"queued"/);
  assert.doesNotMatch(text, /plan-written|plan\.yaml/);
  const checked = spawnSync(
    process.execPath,
    ["scripts/modularization/engine-tests.mjs", "--check"],
    {
      encoding: "utf8",
    },
  );
  assert.equal(checked.status, 0, checked.stderr);
  // --write derives the committed file; --check refuses a stale one and --write repairs it.
  const clone = scratch();
  try {
    execFileSync("git", ["clone", "--quiet", "--shared", ".", clone], { stdio: "pipe" });
    const cli = (...args) =>
      spawnSync(process.execPath, ["scripts/modularization/engine-tests.mjs", ...args], {
        cwd: clone,
        encoding: "utf8",
      });
    const path = join(clone, ENGINE_PATH);
    const expected = readFileSync(path, "utf8");
    rmSync(path);
    const written = cli("--write");
    assert.equal(written.status, 0, written.stderr);
    assert.match(written.stdout, /engine split: \d+ tests move to \d+ engine crates/);
    assert.equal(readFileSync(path, "utf8"), expected);
    const stale = JSON.parse(expected);
    const [first] = Object.keys(stale.moved);
    stale.moved[first].fixture = "tests/recorded/engine/elsewhere/";
    writeFileSync(path, `${JSON.stringify(stale, null, 2)}\n`);
    const refused = cli("--check");
    assert.equal(refused.status, 1);
    assert.match(refused.stderr, /differs from the split its rule derives\nnext: regenerate/);
    assert.equal(cli("--write").status, 0);
    assert.equal(cli("--check").status, 0);
  } finally {
    rmSync(clone, { recursive: true, force: true });
  }
  const catalog = targetCatalog();
  const layout = targetLayout();
  const committed = readFileSync(ENGINE_PATH, "utf8");
  const engineOf = checkEngineSplit(committed, catalog, layout);
  const split = JSON.parse(committed);
  // Every phase of the four parents is moved or kept, never both and never neither.
  const phases = Object.keys(catalog.tests).filter((id) =>
    PARENTS.includes(catalog.tests[id].suite),
  );
  assert.deepEqual([...Object.keys(split.moved), ...Object.keys(split.kept)].sort(), phases.sort());
  assert.equal(engineOf.size, Object.keys(split.moved).length);
  for (const [id, { crate, suite }] of engineOf) {
    assert.ok(suite === `engine--${crate}` || suite === `engine--${crate}--${id.split("::")[0]}`);
    assert.ok(!catalog.dispositions[catalog.tests[id].original].reason.startsWith("live-"));
  }
  const [movedId] = Object.keys(split.moved);
  const [liveId] = Object.keys(split.kept).filter((id) => split.kept[id].reason === "live");
  const refused = (mutate, pattern) => {
    const copy = structuredClone(split);
    mutate(copy);
    assert.throws(() => checkEngineSplit(JSON.stringify(copy, null, 2), catalog, layout), pattern);
  };
  refused((copy) => {
    delete copy.moved[movedId];
  }, /is neither moved nor kept/);
  refused((copy) => {
    copy.kept[movedId] = { parent: copy.moved[movedId].parent, reason: "live", evidence: {} };
  }, /is both moved and kept/);
  refused((copy) => {
    copy.moved[movedId].parent = "cmd-label";
  }, /under cmd-label, not/);
  refused((copy) => {
    copy.moved[liveId] = { ...copy.moved[movedId], parent: copy.kept[liveId].parent };
    delete copy.kept[liveId];
  }, /moves live-ordering test/);
  refused((copy) => {
    copy.moved[movedId].crate = "cmd-start";
    copy.moved[movedId].suite = "engine--cmd-start";
  }, /which is no engine crate/);
  refused((copy) => {
    copy.moved[movedId].suite = "cmd-start--lifecycle";
  }, /not its crate's suite/);
  refused((copy) => {
    copy.moved["views::no_such_test"] = copy.moved[movedId];
  }, /unknown test views::no_such_test/);
  refused((copy) => {
    copy.kept[liveId].reason = "slow";
  }, /for no known reason/);
  const minified = JSON.stringify(split).replace(
    `"${movedId}":`,
    `"${movedId}":${JSON.stringify(split.moved[movedId])},"${movedId}":`,
  );
  assert.throws(() => checkEngineSplit(minified, catalog, layout), /names .* twice/);
});

test("engine replay CLI replays each re-homing step on the 144 PRs and renders fresh engine-tests documentation", {
  timeout: 3000000,
}, async () => {
  const root = scratch();
  try {
    const { EVIDENCE_PATH, engineAppendix, ENGINE_MARKER, validateEngineEvidence } = await import(
      "./engine-report.mjs"
    );
    const usage = spawnSync(
      process.execPath,
      ["scripts/modularization/engine-replay.mjs", join(root, "x.json"), "--parents", "cmd-label"],
      { encoding: "utf8" },
    );
    assert.equal(usage.status, 1);
    assert.match(usage.stderr, /usage: engine-replay.mjs OUTPUT.json \[--parents/);
    const steps = [
      ["cmd-start"],
      ["cmd-start", "cmd-reply"],
      ["cmd-start", "cmd-reply", "cmd-adopt"],
    ];
    // The three steps replay at once: each is an independent read of the same history.
    const runs = await Promise.all(
      steps.map(
        (parents, index) =>
          new Promise((done) => {
            const output = join(root, `step-${index}.json`);
            const child = spawn(
              process.execPath,
              ["scripts/modularization/engine-replay.mjs", output, "--parents", parents.join(",")],
              { stdio: ["ignore", "ignore", "pipe"] },
            );
            let stderr = "";
            child.stderr.on("data", (chunk) => {
              stderr += chunk;
            });
            child.on("close", (status) => done({ status, stderr, output }));
          }),
      ),
    );
    for (const run of runs) assert.equal(run.status, 0, run.stderr);
    const replayed = {
      schema: 1,
      steps: runs.map((run) => JSON.parse(readFileSync(run.output, "utf8"))),
    };
    validateEngineEvidence(replayed);
    // The committed evidence is exactly what the replay produces now.
    assert.deepEqual(replayed, JSON.parse(readFileSync(EVIDENCE_PATH, "utf8")));
    for (const [index, step] of replayed.steps.entries()) {
      assert.equal(step.prs, 144);
      if (index)
        assert.ok(
          step.platforms.linux.secondsSkip >= replayed.steps[index - 1].platforms.linux.secondsSkip,
        );
    }
    // The document is what its evidence renders.
    const doc = readFileSync("docs/modularization-engine-tests.md", "utf8");
    const [prefix] = doc.split(ENGINE_MARKER);
    assert.equal(
      doc,
      `${prefix}${engineAppendix(
        replayed,
        JSON.parse(readFileSync("scripts/modularization/engine-tests.json", "utf8")),
        readFileSync("docs/modularization-test-split.md", "utf8"),
      )}`,
    );
    const check = spawnSync(
      process.execPath,
      ["scripts/modularization/engine-report.mjs", "--check"],
      {
        encoding: "utf8",
      },
    );
    assert.equal(check.status, 0, check.stderr);
    // Evidence that skips a step, or re-homes the steps out of order, is refused.
    assert.throws(
      () => validateEngineEvidence({ schema: 1, steps: replayed.steps.slice(1) }),
      /one step per re-homed parent/,
    );
    assert.throws(
      () => validateEngineEvidence({ schema: 1, steps: [...replayed.steps].reverse() }),
      /step 1 must re-home cmd-start/,
    );
    // A step with a field the replay does not write, or a share out of range, is refused.
    const extra = structuredClone(replayed);
    extra.steps[0].wall.linux.note = "hand-edited";
    assert.throws(
      () => validateEngineEvidence(extra),
      /step 1 projects the linux job inconsistently/,
    );
    const share = structuredClone(replayed);
    share.steps[2].overHalf[0].share = 1.5;
    assert.throws(() => validateEngineEvidence(share), /step 3 lists a PR that is not one/);
    // --collect writes the evidence and the document from the step replays, refusing steps out of
    // order without writing either; --write repairs a document that drifted from its evidence.
    const clone = join(root, "clone");
    execFileSync("git", ["clone", "--quiet", "--shared", ".", clone], { stdio: "pipe" });
    const report = (...args) =>
      spawnSync(process.execPath, ["scripts/modularization/engine-report.mjs", ...args], {
        cwd: clone,
        encoding: "utf8",
      });
    const evidencePath = join(clone, EVIDENCE_PATH);
    const docPath = join(clone, "docs/modularization-engine-tests.md");
    rmSync(evidencePath);
    writeFileSync(docPath, `${prefix}${ENGINE_MARKER}\n`);
    const reversed = report("--collect", ...runs.map((run) => run.output).reverse());
    assert.equal(reversed.status, 1);
    assert.match(reversed.stderr, /step 1 must re-home cmd-start\nnext: regenerate the evidence/);
    assert.throws(() => readFileSync(evidencePath), /ENOENT/);
    assert.equal(readFileSync(docPath, "utf8"), `${prefix}${ENGINE_MARKER}\n`);
    const collected = report("--collect", ...runs.map((run) => run.output));
    assert.equal(collected.status, 0, collected.stderr);
    assert.equal(readFileSync(evidencePath, "utf8"), readFileSync(EVIDENCE_PATH, "utf8"));
    assert.equal(readFileSync(docPath, "utf8"), doc);
    writeFileSync(docPath, doc.replace("| linux | ", "| linux (edited) | "));
    const drifted = report("--check");
    assert.equal(drifted.status, 1);
    assert.match(drifted.stderr, /differs from what engine-report.mjs renders/);
    const rewritten = report("--write");
    assert.equal(rewritten.status, 0, rewritten.stderr);
    assert.equal(readFileSync(docPath, "utf8"), doc);
    // The CLI refuses evidence the replay could not have written, and writes nothing for it.
    const committedEvidence = readFileSync(evidencePath, "utf8");
    const refusedEvidence = (mutate, pattern) => {
      const copy = JSON.parse(committedEvidence);
      mutate(copy);
      writeFileSync(evidencePath, JSON.stringify(copy));
      const refused = report("--write");
      assert.equal(refused.status, 1);
      assert.match(refused.stderr, pattern);
      assert.equal(readFileSync(docPath, "utf8"), doc);
    };
    refusedEvidence((copy) => {
      copy.steps[2].engineSuites[0].suite = `engine--${copy.steps[2].engineSuites[0].crate}x`;
    }, /step 3 lists an engine suite that is not one/);
    refusedEvidence((copy) => {
      copy.steps[0].overHalf[0].pr = 1299;
    }, /step 1 lists a PR that is not one selecting more than half the seconds/);
    refusedEvidence((copy) => {
      copy.steps[1].lock.both += 1;
    }, /step 2 does not split its lockfile-forced PRs by what moved/);
    refusedEvidence((copy) => {
      copy.steps[2].platforms.linux.secondsSkip = 1.2;
    }, /step 3 has no skip shares for linux/);
    refusedEvidence((copy) => {
      copy.steps[0].sole["engine source"] = copy.steps[0].causes["engine source"] + 1;
    }, /step 1 (counts sole out of its broad PRs|has an input kind forcing alone more PRs)/);
    refusedEvidence((copy) => {
      copy.steps[2].crateReach[0].suites = 1;
    }, /step 3 lists a crate whose change does not select more than half the suites/);
    writeFileSync(evidencePath, committedEvidence);
    assert.equal(report("--check").status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
