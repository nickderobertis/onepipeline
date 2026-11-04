import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { githubCache } from "./support/github-cache.mjs";
import { command, workspace } from "./support/cache-workspace.mjs";
import { sourceMetadata, zip } from "./support/cache-history.mjs";
import { repo, pull, sourceRun, releaseJob } from "./support/cache-workflow.mjs";

// Release selection alone is simulated. Task execution, remote bodies, manifests,
// execution records and the promotion receipt come from the production workflow.
test("simulated release selection measures real retry and promoted replay through onebudgetspec", { timeout: 240000 }, async () => {
  const ws = workspace(); const api = await githubCache();
  try {
    const pr = pull(ws);
    const first = await sourceRun(ws, api, pr, 1);
    sourceMetadata(api, ws, first.report, { id: 1, pr });
    const retry = await sourceRun(ws, api, pr, 2);
    pr.merged_at = new Date().toISOString();
    sourceMetadata(api, ws, retry.report, { id: 2, pr });
    const earlyLocal = await sourceRun(ws, api, pr, 6, "refs/heads/main", false);
    sourceMetadata(api, ws, earlyLocal.report, { id: 6, pr });
    const push = (file, text) => {
      writeFileSync(join(ws.dir, file), readFileSync(join(ws.dir, file), "utf8") + text);
      ws.git("add", "-A");
      ws.git("-c", "user.name=Cache journey", "-c", "user.email=cache@example.invalid", "commit", "-qm", "test: later push");
      ws.head = ws.git("rev-parse", "HEAD"); pr.head.sha = ws.head; pr.merge_commit_sha = ws.head;
    };
    push("pool-maintenance/README.md", "\nChanged task inputs.\n");
    const changed = await sourceRun(ws, api, pr, 7);
    sourceMetadata(api, ws, changed.report, { id: 7, pr });
    push("README.md", "\nLater push, same task inputs.\n");
    const laterPush = await sourceRun(ws, api, pr, 8);
    pr.merged_at = new Date().toISOString();
    sourceMetadata(api, ws, laterPush.report, { id: 8, pr });
    const promoted = await releaseJob(ws, api);
    const receiptDir = join(promoted.temp, "nx-cache-promotion");
    const receipt = JSON.parse(readFileSync(join(receiptDir, "receipt.json")));
    assert.equal(receipt.entries.length, 1);
    const promotion = { id: 100, run_attempt: 1, event: "push", head_repository: repo, head_sha: ws.head,
      head_branch: "main", updated_at: new Date().toISOString() };
    const artifact = { id: 10000, name: `nx-cache-promotion-${ws.head}`, expired: false,
      workflow_run: { id: 100, head_sha: ws.head, head_repository_id: repo.id } };
    api.metadata.set(`/repos/${repo.full_name}/actions/workflows/release-plz.yml/runs`, { workflow_runs: [promotion] });
    api.metadata.set(`/repos/${repo.full_name}/actions/runs/100/artifacts`, { artifacts: [artifact] });
    api.metadata.set(`/repos/${repo.full_name}/actions/artifacts/10000/zip`, zip(receiptDir, "promotion.zip"));
    api.metadata.set(`/repos/${repo.full_name}/commits/${ws.head}`, { sha: ws.head });
    writeFileSync(join(ws.dir, "CHANGELOG.md"), readFileSync(join(ws.dir, "CHANGELOG.md"), "utf8") + "\n## Fixture release\n");
    ws.git("add", "CHANGELOG.md");
    ws.git("-c", "user.name=Cache journey", "-c", "user.email=cache@example.invalid", "commit", "-qm", "chore: release fixture");
    const sourceHead = ws.head; ws.head = ws.git("rev-parse", "HEAD");
    const release = pull(ws, 2); release.head.ref = "release-plz-fixture"; release.base.sha = sourceHead;
    const replay = await sourceRun(ws, api, release, 3);
    release.merged_at = new Date().toISOString();
    sourceMetadata(api, ws, replay.report, { id: 3, pr: release });
    const prefix = `/repos/${repo.full_name}`;
    const published = new Date(Date.now() + 60000).toISOString();
    api.metadata.set(`${prefix}/releases`, [{ tag_name: "v-fixture", published_at: published, draft: false },
      { tag_name: "v-before", published_at: "2020-01-01T00:00:00.000Z", draft: false }]);
    api.metadata.set(`${prefix}/commits/v-fixture`, { sha: ws.head });
    api.metadata.set(`${prefix}/compare/v-before...v-fixture`, { commits: [{ sha: sourceHead }, { sha: ws.head }] });
    const measure = async () => {
      const temp = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR, "cache-figure-"));
      return command(ws.dir, "onebudgetspec", ["check", "budgets.yaml", "--id", "onepipeline-nx-cache-use", "--json"], {
        GITHUB_REPOSITORY: repo.full_name, GITHUB_REF_NAME: "v-fixture", GITHUB_API_URL: api.url.replace(/\/$/, ""),
        GH_TOKEN: "fixture-token", GITHUB_TOKEN: "fixture-token", RUNNER_TEMP: temp,
      });
    };
    api.metadata.set(`${prefix}/compare/${sourceHead}...${sourceHead}`, { status: "identical" });
    const result = await measure();
    assert.equal(result.code, 0, result.stdout + result.stderr);
    const output = JSON.parse(result.stdout);
    assert.match(JSON.stringify(output), /within/);
    assert.match(result.stdout + result.stderr, /aggregate: 3\/3 remote hits/);
    assert.match(result.stdout + result.stderr, /release PR: 1\/1/);
    assert.match(result.stdout + result.stderr, /same-PR retry\/push: 2\/2/);
    // Eviction does not erase eligibility: the same proven hash executing
    // again is a measured miss, with comparison left to onebudgetspec.
    api.entries.clear();
    const evicted = await sourceRun(ws, api, release, 4);
    release.merged_at = new Date().toISOString();
    const evictedMetadata = sourceMetadata(api, ws, evicted.report, { id: 4, pr: release });
    const miss = await measure();
    assert.equal(miss.code, 1, miss.stdout + miss.stderr);
    const missed = JSON.parse(miss.stdout).results[0];
    assert.equal(missed.actual, 100 * 2 / 3);
    assert.equal(missed.verdict, "over");
    assert.match(missed.detail, /aggregate: 2\/3 remote hits/);
    assert.match(missed.detail, /miss pool-maintenance:test\/linux/);
    // A missing expected job is unavailable evidence, never a passing rate.
    api.metadata.set(`${prefix}/actions/runs/4/attempts/1/jobs`, { jobs: [
      { name: "test-linux", conclusion: "success", labels: ["ubuntu-latest"] },
      { name: "test-windows", conclusion: "success", labels: ["windows-latest"] },
    ] });
    const unavailable = await measure();
    assert.notEqual(unavailable.code, 0);
    assert.match(unavailable.stdout + unavailable.stderr, /missing artifact.*test-windows/);
    api.metadata.set(`${prefix}/actions/runs/4/attempts/1/jobs`, { jobs: [{ name: "test-linux", conclusion: "success", labels: ["ubuntu-latest"] }] });
    evictedMetadata.artifact.expired = true;
    const expired = await measure();
    assert.notEqual(expired.code, 0);
    assert.match(expired.stdout + expired.stderr, /expired artifact/);
    evictedMetadata.artifact.expired = false;
    writeFileSync(join(receiptDir, "receipt.json"), JSON.stringify({ ...receipt, scope: "refs/heads/untrusted" }));
    api.metadata.set(`${prefix}/actions/artifacts/10000/zip`, zip(receiptDir, "untrusted.zip"));
    const untrusted = await measure();
    assert.notEqual(untrusted.code, 0);
    assert.match(untrusted.stdout + untrusted.stderr, /untrusted promotion/);
    writeFileSync(join(receiptDir, "receipt.json"), JSON.stringify({ ...receipt, schema: 999 }));
    api.metadata.set(`${prefix}/actions/artifacts/10000/zip`, zip(receiptDir, "invalid.zip"));
    const invalid = await measure();
    assert.notEqual(invalid.code, 0);
    assert.match(invalid.stdout + invalid.stderr, /invalid promotion receipt/);
    writeFileSync(join(receiptDir, "receipt.json"), JSON.stringify(receipt));
    api.metadata.set(`${prefix}/actions/artifacts/10000/zip`, zip(receiptDir, "trusted.zip"));
    const local = await sourceRun(ws, api, release, 5, "refs/heads/main", false);
    release.merged_at = new Date().toISOString();
    sourceMetadata(api, ws, local.report, { id: 5, pr: release });
    const skipped = await measure();
    assert.equal(skipped.code, 0, skipped.stdout + skipped.stderr);
    const localFigure = JSON.parse(skipped.stdout).results[0];
    assert.equal(localFigure.actual, 100);
    assert.match(localFigure.detail, /aggregate: 2\/2 remote hits/);
    assert.match(localFigure.detail, /skipped local hits: 2/);
    // A release with only a first execution has no eligible denominator.
    api.metadata.set(`${prefix}/compare/v-before...v-fixture`, { commits: [{ sha: ws.head }] });
    api.metadata.set(`${prefix}/actions/workflows/ci.yml/runs`, { workflow_runs: [evictedMetadata.run] });
    const empty = await measure();
    assert.notEqual(empty.code, 0);
    assert.match(empty.stdout + empty.stderr, /zero eligible executions/);
  } finally { await api.close(); ws.close(); }
});

for (const [name, fault] of [
  ["throttled store", { store: 429 }], ["failed store", { store: 500 }],
  ["throttled promotion", { promotion: 429 }], ["failed promotion", { promotion: 500 }],
  ["absent promotion", { absentPromotion: true }],
]) {
  test(`simulated release selection counts the matching hash after a ${name} as an eligible miss`, { timeout: 180000 }, async () => {
    const { faultFigure } = await import("./support/cache-figure.mjs");
    const measured = await faultFigure(fault);
    assert.equal(measured.code, 1, measured.stdout + measured.stderr);
    assert.equal(measured.result.actual, 50);
    assert.equal(measured.result.verdict, "over");
    assert.match(measured.result.detail, /aggregate: 1\/2 remote hits/);
    assert.match(measured.result.detail, /miss pool-maintenance:test\/linux/);
    if (fault.store === 429 || fault.promotion === 429) {
      // op-nx 91839c maps saveCache's ambiguous -1 after GitHub 429 to
      // HTTP 409 after an empty lookup. Eligibility survives even though the throttle label is lost.
      const attempts = fault.store ? measured.firstExecution.store_attempts : measured.promotionReceipt.attempts;
      assert.equal(attempts.length, 1);
      assert.equal(attempts[0].status, "failed");
      assert.equal(attempts[0].http_status, 409);
      const refused = measured.calls.findIndex((call) => call.status === 429);
      const upload = measured.calls[refused];
      const probes = measured.calls.slice(refused + 1).filter((call) => call.method === "GetCacheEntryDownloadURL" && call.scope === upload.scope && call.body.key === upload.body.key);
      assert.ok(probes.length >= (fault.promotion ? 2 : 1), "backend probe and promotion confirmation must read the failed key");
      assert.ok(probes.every((call) => call.found === false), "the 409 must not be classified already_present");
    }
    if (fault.store || fault.promotion) assert.ok(measured.calls.some((call) => call.status === (fault.store ?? fault.promotion)));
  });
}
