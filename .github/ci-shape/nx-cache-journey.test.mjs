import assert from "node:assert/strict";
import { readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { parse } from "yaml";
import { githubCache } from "./support/github-cache.mjs";
import { command, root, sidecar, workspace } from "./support/cache-workspace.mjs";

const ci = parse(readFileSync(join(root, ".github/workflows/ci.yml"), "utf8"));
const steps = ci.jobs["test-linux"].steps;
for (const [name, expected] of [["fork", true], ["dependabot", true], ["same-repository", false]]) {
  test(`the workflow's real cache steps ${expected ? "refuse writes" : "store the result"} for recorded ${name} metadata`, { timeout: 180000 }, async () => {
    const ws = workspace();
    const api = await githubCache();
    const record = JSON.parse(readFileSync(new URL(`./fixtures/${name}.json`, import.meta.url)));
    let active;
    try {
      active = await sidecar(ws, api, record.event, steps);
      assert.equal(active.trust.read_only, String(expected));
      assert.equal(active.trust.promotable, String(!expected));
      assert.equal(active.env.CACHE_READ_ONLY, expected ? "1" : "");
      assert.equal(active.env.NX_BASE, record.event.pull_request.base.sha);
      assert.equal(active.env.NX_HEAD, record.event.pull_request.head.sha);
      rmSync(join(ws.dir, ".nx/cache"), { recursive: true, force: true });
      const ran = await command(ws.dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", "run", "pool-maintenance:test", "--outputStyle=static"], active.env);
      assert.equal(ran.code, 0, ran.stdout + ran.stderr + active.log());
      const report = steps.find((s) => s.run?.includes("nx-remote-cache.mjs report"));
      const reported = await command(ws.dir, "bash", ["-eo", "pipefail", "-c", report.run], active.env);
      assert.equal(reported.code, 0, reported.stderr);
      const manifest = JSON.parse(readFileSync(join(active.temp, "nx-cache-report/manifest.json")));
      const executions = JSON.parse(readFileSync(join(active.temp, "nx-cache-report/executions.json")));
      assert.deepEqual(executions.selection, { mode: "affected", base_sha: record.event.pull_request.base.sha, head_sha: record.event.pull_request.head.sha });
      assert.equal(executions.tasks.find((t) => t.task === "pool-maintenance:test").result, "executed");
      if (expected) {
        assert.deepEqual(manifest.entries, []);
        assert.equal(api.entries.size, 0);
        assert.equal(api.calls.some((c) => c.method === "CreateCacheEntry"), false);
      } else {
        assert.ok(manifest.entries.length > 0);
        assert.ok(manifest.stored_bytes > 0);
        assert.equal(api.entries.size, manifest.entries.length);
        assert.ok(manifest.entries.every((e) => e.source === "stored"));
      }
    } finally {
      if (active) {
        await command(ws.dir, process.execPath, ["scripts/nx-remote-cache.mjs", "report", "--job", "test-linux", "--out", join(active.temp, "final-report")], active.env);
        await active.close();
      }
      await api.close(); ws.close();
    }
  });
}
