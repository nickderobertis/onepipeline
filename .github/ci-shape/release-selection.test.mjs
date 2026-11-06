import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { workspace, command } from "./support/cache-workspace.mjs";

test("release manifest edits select version readers and proven Rust tiers on every OS job", { timeout: 120000 }, async () => {
  const ws = workspace();
  try {
    const base = ws.head;
    const version = readFileSync(join(ws.dir, "Cargo.toml"), "utf8").match(/\[package\][\s\S]*?\nversion = "([^"]+)"/)[1];
    const [major, minor] = version.split(".").map(Number);
    const next = `${major}.${minor + 1}.0`;
    for (const file of ["Cargo.toml", "Cargo.lock"]) {
      const path = join(ws.dir, file);
      writeFileSync(path, readFileSync(path, "utf8").replace(`version = "${version}"`, `version = "${next}"`));
    }
    writeFileSync(join(ws.dir, "CHANGELOG.md"), readFileSync(join(ws.dir, "CHANGELOG.md"), "utf8") + "\n## Next release\n");
    ws.git("add", "-A");
    ws.git("-c", "user.name=Cache journey", "-c", "user.email=cache@example.invalid", "commit", "-qm", "chore: release fixture");
    for (const job of ["lint", "test-linux", "test-windows", "test-macos"]) {
      const result = await command(ws.dir, "bash", ["scripts/ci-job.sh", job, "--plan"], {
        CI: "true", ONEPIPELINE_NX_BASE_SHA: base, NX_DAEMON: "false", NX_ISOLATE_PLUGINS: "false",
      });
      assert.equal(result.code, 0, result.stdout + result.stderr);
      if (job === "lint") { assert.match(result.stdout, /onepipeline:lint/); continue; }
      const target = job === "test-linux" ? "test" : "test-quick";
      for (const project of ["onepipeline-contract", "onepipeline-e2e", "onepipeline-version-contract", "onepipeline-version-e2e", "onepipeline-version-package"])
        assert.ok(result.stdout.includes(`${project}:${target}`), `${job} omitted ${project}:${target}: ${result.stdout}`);
      assert.doesNotMatch(result.stdout, /:input-selection/);
    }
  } finally { ws.close(); }
});

test("every task selected on multiple OS legs, including bootstrap and uncached tasks, hashes the platform", { timeout: 120000 }, async () => {
  const { execFileSync } = await import("node:child_process");
  const { parse } = await import("yaml");
  const ws = workspace();
  try {
    const nx = JSON.parse(readFileSync(join(ws.dir, "nx.json")));
    const paths = execFileSync("git", ["ls-files", "-z"], { cwd: ws.dir, encoding: "utf8" }).split("\0").filter((p) => /(^|\/)project\.json$/.test(p));
    const projects = new Map(paths.map((p) => { const project = JSON.parse(readFileSync(join(ws.dir, p))); return [project.name, project]; }));
    const ci = parse(readFileSync(join(ws.dir, ".github/workflows/ci.yml"), "utf8"));
    const legs = new Map();
    for (const job of ["lint", "test-linux", "test-windows", "test-macos"]) {
      const config = ci.jobs[job];
      const os = job === "test-windows" ? "win32" : job === "test-macos" ? "darwin" : "linux";
      const planned = await command(ws.dir, "bash", ["scripts/ci-job.sh", job, "--plan"], { CI: "true", ONEPIPELINE_NX_BASE_SHA: "", GITHUB_BASE_REF: "", NX_DAEMON: "false" });
      assert.equal(planned.code, 0, planned.stdout + planned.stderr);
      const line = planned.stdout.split("\n").find((line) => line.startsWith("ci-job:"));
      const ids = line.split("task(s): ")[1].split(" ");
      if (config.steps.some((step) => /\bjust bootstrap\b/.test(step.run ?? "")))
        for (const project of projects.values()) if (project.targets.bootstrap) ids.push(`${project.name}:bootstrap`);
      for (const id of ids) { if (!legs.has(id)) legs.set(id, new Set()); legs.get(id).add(os); }
    }
    for (const [id, os] of legs) {
      if (os.size < 2) continue;
      const [project, target] = id.split(":");
      const inputs = projects.get(project).targets[target].inputs ?? nx.targetDefaults[target]?.inputs ?? ["default", "^default"];
      assert.ok(inputs.some((input) => input.runtime === "node -p process.platform"), `${id} runs on ${[...os].join(",")} without a platform input`);
    }
  } finally { ws.close(); }
});

test("dependency-only manifest and lock edits retain the base's real Nx affected projects", { timeout: 300000 }, async () => {
  const original = workspace("96c876ab");
  const current = workspace();
  const bump = (value) => value.replace(/(\d+)(?![\s\S]*\d)/, (part) => String(Number(part) + 1));
  const jsonDependency = (text) => {
    const doc = JSON.parse(text);
    const dependencies = doc.dependencies ?? doc.devDependencies ?? doc.optionalDependencies;
    const key = Object.keys(dependencies)[0];
    dependencies[key] = bump(dependencies[key]);
    return JSON.stringify(doc, null, 2) + "\n";
  };
  const jsonLock = (text) => {
    const doc = JSON.parse(text);
    const key = Object.keys(doc.packages).find((name) => name && doc.packages[name].version);
    doc.packages[key].version = bump(doc.packages[key].version);
    return JSON.stringify(doc, null, 2) + "\n";
  };
  const cases = [
    ["Cargo.toml", (text) => text.replace('async-trait = "0.1"', 'async-trait = "0.2"')],
    ["Cargo.lock", (text) => text.replace(/(\[\[package\]\]\nname = "[^"]+"\nversion = ")([^"]+)/, (_, prefix, version) => prefix + bump(version))],
    ["crates/testfakes/Cargo.toml", (text) => text.replace('serde_json = "1"', 'serde_json = "2"')],
    ["pyproject.toml", (text) => text.replace("maturin>=1.7", "maturin>=1.8")],
    ...["package.json", ".github/package.json", "npm/onepipeline-cli/package.json"].map((path) => [path, jsonDependency]),
    ...["package-lock.json", ".github/package-lock.json"].map((path) => [path, jsonLock]),
  ];
  try {
    for (const [path, mutate] of cases) {
      const selections = [];
      for (const ws of [original, current]) {
        const file = join(ws.dir, path);
        const before = readFileSync(file, "utf8");
        const after = mutate(before);
        assert.notEqual(after, before, `${path} fixture changed no dependency`);
        const base = ws.git("rev-parse", "HEAD");
        writeFileSync(file, after);
        ws.git("add", path);
        ws.git("-c", "user.name=Selection journey", "-c", "user.email=cache@example.invalid", "commit", "-qm", `test: change ${path} dependency`);
        const result = await command(ws.dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", "show", "projects", "--affected", `--base=${base}`, "--head=HEAD", "--json"], { NX_DAEMON: "false", NX_ISOLATE_PLUGINS: "false" });
        assert.equal(result.code, 0, result.stdout + result.stderr);
        selections.push(new Set(JSON.parse(result.stdout)));
        writeFileSync(file, before);
        ws.git("add", path);
        ws.git("-c", "user.name=Selection journey", "-c", "user.email=cache@example.invalid", "commit", "-qm", "test: restore dependency");
      }
      assert.ok(selections[0].size, `${path} selected no base projects`);
      for (const project of selections[0]) assert.ok(selections[1].has(project), `${path} lost affected project ${project}`);
    }
  } finally { original.close(); current.close(); }
});
