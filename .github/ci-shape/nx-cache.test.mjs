import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { parse } from "yaml";
import { OP_NX_COMMIT, PROMOTION_BRANCH_PATTERN } from "../../scripts/nx-remote-cache.mjs";

const workflow = (name) => parse(readFileSync(new URL(`../workflows/${name}.yml`, import.meta.url), "utf8"));
const ci = workflow("ci");
for (const [name, job] of Object.entries(ci.jobs)) {
  test(`${name} records its real Nx run through the pinned cache sidecar`, () => {
    const steps = job.steps;
    const trust = steps.findIndex((s) => s.id === "nx-cache-trust");
    const server = steps.findIndex((s) => s.id === "nx-cache");
    const ready = steps.findIndex((s) => s.run?.includes("nx-remote-cache.mjs ready"));
    const run = steps.findIndex((s) => s.run?.includes(`just ci-${name}`));
    const report = steps.findIndex((s) => s.run?.includes("nx-remote-cache.mjs report"));
    const upload = steps.findIndex((s) => s.with?.name === "nx-cache-entries-${{ github.job }}");
    assert.ok(trust >= 0 && trust < server && server < ready && ready < run && run < report && report < upload);
    assert.match(steps[trust].run, /node scripts\/nx-remote-cache.mjs trust/);
    assert.equal(steps[server].uses, `op-nx/github-cache/start-cache-server@${OP_NX_COMMIT}`);
    assert.equal(steps[server].background, true);
    assert.equal(steps[server]["working-directory"], undefined);
    assert.equal(steps[server].env.CACHE_READ_ONLY, "${{ steps.nx-cache-trust.outputs.read_only == 'true' && '1' || '' }}");
    assert.equal(steps[report].if, "always()");
    assert.equal(steps[report]["working-directory"], "${{ github.workspace }}");
    assert.equal(steps[upload].if, "always() && steps.nx-cache-trust.outputs.promotable == 'true'");
    assert.equal(steps[upload].with["retention-days"], 7);
    const cancel = steps.find((s) => s.cancel === "nx-cache");
    assert.equal(cancel.if, "always()");
    assert.equal(cancel["continue-on-error"], true);
  });
}

test("promotion runs on merge or the dedicated branch, before main-only release automation", () => {
  const release = workflow("release-plz");
  assert.deepEqual(release.on.push.branches, ["main", PROMOTION_BRANCH_PATTERN]);
  assert.equal(release.on.workflow_dispatch, undefined);
  assert.equal(release.jobs["release-plz"].permissions.actions, "write");
  const steps = release.jobs["release-plz"].steps;
  const promote = steps.findIndex((s) => s.run?.includes("nx-remote-cache.mjs promote"));
  assert.ok(promote > 0);
  assert.match(steps[promote].run, /promote --sha "\$GITHUB_SHA" --workflow ci.yml/);
  assert.match(steps[promote].run, /promote --pr "\$pr" --workflow ci.yml/);
  for (const step of steps) {
    if (step.run?.includes("release-plz ") || step.run?.includes("release-pr-auto-merge.sh")) {
      assert.ok(steps.indexOf(step) > promote);
      assert.match(step.if, /github.event_name == 'push'/);
      assert.match(step.if, /github.ref == 'refs\/heads\/main'/);
    }
    for (const key of Object.keys(step.env ?? {})) {
      assert.ok(key === "GITHUB_TOKEN" || !key.startsWith("GITHUB_"), `${key} overrides a GitHub-provided variable`);
    }
  }
});
