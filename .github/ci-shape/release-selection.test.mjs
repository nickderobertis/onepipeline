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
