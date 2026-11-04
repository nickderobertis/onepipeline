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
    for (const [job, config] of Object.entries(ci.jobs)) {
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
