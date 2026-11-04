import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { githubCache } from "./support/github-cache.mjs";
import { workspace } from "./support/cache-workspace.mjs";
import { sourceMetadata } from "./support/cache-history.mjs";
import { repo, pull, sourceRun, releaseJob } from "./support/cache-workflow.mjs";

test("the release-plz job promotes results replayed by a later run and another PR reads the base scope", { timeout: 180000 }, async () => {
  const ws = workspace(); const api = await githubCache();
  try {
    const pr = pull(ws);
    const first = await sourceRun(ws, api, pr, 1);
    sourceMetadata(api, ws, first.report, { id: 1, pr });
    const original = new Map(api.entries);
    const second = await sourceRun(ws, api, pr, 2);
    const replay = JSON.parse(readFileSync(join(second.report, "manifest.json")));
    assert.ok(replay.entries.length > 0 && replay.entries.every((entry) => entry.source === "replayed"), JSON.stringify(replay) + second.output);
    assert.equal(replay.stored_bytes, 0);
    pr.merged_at = new Date().toISOString();
    sourceMetadata(api, ws, second.report, { id: 2, pr });
    const promoted = await releaseJob(ws, api);
    const receipt = JSON.parse(readFileSync(join(promoted.temp, "nx-cache-promotion/receipt.json")));
    assert.equal(receipt.source_run_id, 2);
    assert.equal(receipt.entries.length, replay.entries.length);
    assert.match(readFileSync(join(promoted.temp, "summary"), "utf8"), /1 promoted, 0 already present, 0 failed/);
    assert.match(readFileSync(promoted.env.RELEASE_CALLS, "utf8"), /release/);
    const next = await sourceRun(ws, api, pull(ws, 2), 3);
    const third = JSON.parse(readFileSync(join(next.report, "manifest.json")));
    assert.ok(third.entries.every((entry) => entry.source === "replayed"));
    for (const [key, body] of original) assert.deepEqual(api.entries.get(key), body);
    assert.equal(api.calls.some((call) => call.method === "DELETE"), false);
  } finally { await api.close(); ws.close(); }
});

for (const cause of ["fork", "Dependabot", "expired artifact", "missing artifact"]) {
  test(`a ${cause} merge skips promotion and still runs release-plz`, { timeout: 180000 }, async () => {
    const ws = workspace(); const api = await githubCache();
    try {
      const pr = pull(ws);
      if (cause === "fork") pr.head.repo = { id: 2, full_name: "fork/project" };
      if (cause === "Dependabot") pr.user.login = "dependabot[bot]";
      api.metadata.set(`/repos/${repo.full_name}/commits/${ws.head}/pulls`, [pr]);
      if (cause.endsWith("artifact")) {
        const source = await sourceRun(ws, api, pr, 1);
        sourceMetadata(api, ws, source.report, { id: 1, pr, expired: cause === "expired artifact" });
        if (cause === "missing artifact") api.metadata.set(`/repos/${repo.full_name}/actions/runs/1/artifacts`, { artifacts: [] });
      }
      const before = new Map(api.entries);
      const done = await releaseJob(ws, api);
      assert.deepEqual(api.entries, before);
      assert.match(readFileSync(join(done.temp, "summary"), "utf8"), cause.endsWith("artifact") ? /expired|missing artifact/ : new RegExp(cause === "fork" ? "fork head" : "Dependabot author"));
      assert.match(readFileSync(done.env.RELEASE_CALLS, "utf8"), /release/);
    } finally { await api.close(); ws.close(); }
  });
}


test("a real push to the dedicated promotion branch promotes an open draft and runs no release-plz command", { timeout: 180000 }, async () => {
  const ws = workspace(); const api = await githubCache();
  try {
    const pr = pull(ws);
    pr.merged_at = null; pr.state = "open"; pr.draft = true;
    const source = await sourceRun(ws, api, pr, 1);
    sourceMetadata(api, ws, source.report, { id: 1, pr });
    const done = await releaseJob(ws, api, "refs/heads/cache-promote/1");
    assert.equal(existsSync(done.env.RELEASE_CALLS), false);
    assert.ok([...api.entries.keys()].some((key) => key.startsWith("refs/heads/cache-promote/1:")));
    const receipt = JSON.parse(readFileSync(join(done.temp, "nx-cache-promotion/receipt.json")));
    assert.equal(receipt.scope, "refs/heads/cache-promote/1");
    assert.equal(receipt.source_run_id, 1);
    assert.equal(receipt.entries.length, 1);
    assert.equal(api.calls.some((call) => call.method === "DELETE"), false);
  } finally { await api.close(); ws.close(); }
});
