import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { affectedSuites, compileMapping } from "../../scripts/crate-split-replay.mjs";
import { moduleGraph } from "../../scripts/modularization/graph.mjs";

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
      crates: {
        command: { files: ["src/command.rs"], deps: ["record"] },
        record: { files: ["src/record.rs"], deps: [] },
      },
      edges: { "command->record": { kind: "shared", fix: "Record is shared durable state." } },
    };
    put(root, "layout.json", layout);
    const cli = resolve("scripts/modularization/graph.mjs");
    const run = () =>
      spawnSync(process.execPath, [cli, "layout.json"], { cwd: root, encoding: "utf8" });
    assert.equal(run().status, 0);
    layout.crates.record.deps.push("command");
    put(root, "layout.json", layout);
    const failed = run();
    assert.equal(failed.status, 1);
    assert.match(failed.stderr, /command -> record -> command/);
    layout.crates.record.deps = [];
    layout.crates.record.files = [];
    put(root, "layout.json", layout);
    assert.match(run().stderr, /src\/record.rs: expected one owner, found 0/);
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
      'use crate::{b::{Thing}, c};\n// crate::d::x\nconst S: &str = "crate::d::x";',
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
    const mapping = compileMapping({ shared, sources: [], crates, suites });
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
