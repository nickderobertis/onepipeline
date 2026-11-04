import assert from "node:assert/strict";
import { test } from "node:test";
import { githubCache } from "./support/github-cache.mjs";

test("the GitHub HTTP double rejects malformed identities and uploads before mutation", async () => {
  const api = await githubCache();
  try {
    const endpoint = `${api.url}CreateCacheEntry`;
    const valid = { key: "fixture-key", version: "fixture-version" };
    const post = (body, authorization = "Bearer refs/heads/main") => fetch(endpoint, {
      method: "POST", headers: { authorization }, body: JSON.stringify(body),
    });
    assert.equal((await post(valid, "")).status, 401);
    for (const body of [{}, null, [], { key: "", version: "v" }, { key: "k" }, { key: 1, version: "v" }])
      assert.equal((await post(body)).status, 400);
    assert.equal(api.uploads.size, 0);
    assert.equal(api.entries.size, 0);
    const accepted = await post(valid);
    assert.equal(accepted.status, 200);
    const { signedUploadUrl } = await accepted.json();
    assert.equal((await fetch(`${api.url}upload/unknown`, { method: "PUT", body: "x" })).status, 404);
    assert.equal((await fetch(`${signedUploadUrl}?comp=block`, { method: "PUT", body: "x" })).status, 400);
    for (const body of ["", "<BlockList/>", "<BlockList><Latest>missing</Latest></BlockList>", "<BlockList><other>x</other></BlockList>"])
      assert.equal((await fetch(`${signedUploadUrl}?comp=blocklist`, { method: "PUT", body })).status, 400);
    const token = signedUploadUrl.split("/").at(-1);
    assert.equal(api.uploads.get(token).blocks.size, 0);
    assert.equal(api.uploads.get(token).body, undefined);
    // Recover on the same reservation, just as the real uploader does.
    assert.equal((await fetch(`${signedUploadUrl}?comp=block&blockid=YmxvY2s=`, { method: "PUT", body: "real payload" })).status, 201);
    assert.equal((await fetch(`${signedUploadUrl}?comp=blocklist`, { method: "PUT", body: "<BlockList><Latest>YmxvY2s=</Latest></BlockList>" })).status, 201);
    const finalized = await fetch(`${api.url}FinalizeCacheEntryUpload`, { method: "POST", headers: { authorization: "Bearer refs/heads/main" }, body: JSON.stringify(valid) });
    assert.equal(finalized.status, 200);
    assert.deepEqual([...api.entries.values()], [Buffer.from("real payload")]);
  } finally { await api.close(); }
});

test("nested fixture Nx tasks cannot store or record against the enclosing CI job", { timeout: 60000 }, async () => {
  const { createServer } = await import("node:http");
  const { existsSync, mkdtempSync, rmSync } = await import("node:fs");
  const { join } = await import("node:path");
  const { workspace, command } = await import("./support/cache-workspace.mjs");
  const ws = workspace();
  const temp = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR, "parent-ci-"));
  let requests = 0;
  const parent = createServer((req, res) => { requests++; req.resume(); res.writeHead(req.method === "PUT" ? 409 : 404).end(); });
  await new Promise((resolve) => parent.listen(0, "127.0.0.1", resolve));
  const values = { GITHUB_ACTIONS: "true", GITHUB_JOB: "enclosing-job", RUNNER_TEMP: temp,
    NX_SELF_HOSTED_REMOTE_CACHE_SERVER: `http://127.0.0.1:${parent.address().port}`, NX_SELF_HOSTED_REMOTE_CACHE_ACCESS_TOKEN: "parent-token" };
  const previous = Object.fromEntries(Object.keys(values).map((key) => [key, process.env[key]]));
  Object.assign(process.env, values);
  try {
    const result = await command(ws.dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", "run", "pool-maintenance:test", "--outputStyle=static"], { NX_DAEMON: "false", NX_ISOLATE_PLUGINS: "false" });
    assert.equal(result.code, 0, result.stdout + result.stderr);
    assert.equal(requests, 0, "fixture results reached the parent sidecar");
    assert.equal(existsSync(join(temp, "nx-cache-recording")), false, "fixture executions contaminated the CI report");
  } finally {
    for (const [key, value] of Object.entries(previous)) { if (value === undefined) delete process.env[key]; else process.env[key] = value; }
    await new Promise((resolve) => parent.close(resolve)); ws.close(); rmSync(temp, { recursive: true, force: true });
  }
});
