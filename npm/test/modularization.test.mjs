import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { affectedSuites, compileMapping } from "../../scripts/crate-split-replay.mjs";
import { moduleGraph } from "../../scripts/modularization/graph.mjs";
import { replayMapping, buildReport } from "../../scripts/modularization/replay.mjs";

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
    put(root, "tests/e2e/main.rs", "mod harness;\nmod status;\n");
    layout.suites.status.modules = ["unknown"];
    put(root, "layout.json", layout);
    assert.match(run().stderr, /unknown test module unknown/);
    layout.suites.status.modules = ["status"];
    layout.edges = { "command->record": { kind: "shared", fix: "Shared record." } };
    layout.auxiliarySuites = {
      contract: {
        paths: ["tests/contract/**"],
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
        String.raw`
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
  const { profileTable } = await import("../../scripts/modularization/replay.mjs");
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
    put(
      root,
      "tests/e2e/b.rs",
      'const RECORD: &str = "fixtures/constant.json";\nconst DIRECTORY: &str = "fixtures/runtime";\nfn helper() { repo_file("fixtures/record.json"); repo_file(RECORD); Path::new(env!("CARGO_MANIFEST_DIR")).join(DIRECTORY); }',
    );
    put(root, "fixtures/runtime/data.json", "{}");
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
        "cmd-template-list": { files: [], deps: [] },
        "cmd-template-resolve": { files: [], deps: [] },
        "cmd-template-check": { files: [], deps: [] },
      },
      edges: {},
      suites: { a: { modules: ["a"], paths: [], crates: ["onepipeline"] } },
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
    put(root, "layout.json", modular);
    assert.equal(run().status, 0);
    assert.ok(
      JSON.parse(readFileSync(join(root, "layout.json"), "utf8")).suites.a.crates.includes(
        "cmd-template-inspect",
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
  const { suiteInputs, commandCrates } = await import(
    "../../scripts/modularization/suite-inputs.mjs"
  );
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
    assert.equal(report.migration.length, 73);
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
      readFileSync(join(root, "docs/modularization-design.md"), "utf8") + "stale\n",
    );
    assert.equal(run("render").status, 1);
    assert.equal(run("render", "--write").status, 0);
    assert.equal(run("render").status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
