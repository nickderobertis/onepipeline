// The cache paths this repository's workflows state, run the way a runner runs them: the
// exact command or expression read from the workflow, GitHub doubled at its HTTP boundary,
// and the real scripts under it.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it, test } from "node:test";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";
import { evaluate } from "../workflow-graph.mjs";
import { sourceMetadata } from "./support/cache-history.mjs";
import { command, workspace } from "./support/cache-workspace.mjs";
import { pull, repo, sourceRun } from "./support/cache-workflow.mjs";
import { githubCache } from "./support/github-cache.mjs";
import { githubListing, recordedEntries } from "./support/github-listing.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const flow = (name) => parse(readFileSync(join(root, ".github/workflows", name), "utf8"));
const warm = flow("rust-cache-warm.yml");
const ci = flow("ci.yml");
const LEGS = ["linux", "windows", "macos"];
const scratch = (name) => mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), name));
const cleanupOf = (leg) => warm.jobs[leg].steps.find((step) => step.run?.includes("actions-cache-prune.mjs"));
const familiesOf = (leg) => [...cleanupOf(leg).run.matchAll(/--family (\S+)/g)].map((match) => match[1]);
const allFamilies = LEGS.flatMap(familiesOf);
const entriesOf = (api, family) => api.held().filter((entry) => entry.key.startsWith(family));

// The environment a runner gives the step, with the step's own `env:` rendered from the
// workflow, so a token handed some other way would not reach the script.
async function runCleanup(leg, api) {
  const step = cleanupOf(leg);
  const summary = join(scratch("prune-summary-"), "summary.md");
  const env = { PATH: process.env.PATH, GITHUB_API_URL: api.url, GITHUB_REPOSITORY: "nickderobertis/onepipeline", GITHUB_STEP_SUMMARY: summary };
  for (const [name, value] of Object.entries(step.env))
    env[name] = String(value).replace(/\$\{\{(.*?)\}\}/g, (_, expression) => evaluate(expression, { github: { token: "fixture-token" } }));
  const run = await command(root, "bash", ["-eo", "pipefail", "-c", step.run], env);
  assert.equal(run.code, 0, run.stdout + run.stderr);
  return { ...run, summary: readFileSync(summary, "utf8") };
}

describe("a warm leg's cleanup, the command its step states, against GitHub's REST listing", () => {
  // The recording holds one copy per family, so a labelled copy of each kind is added.
  const superseded = (family) => [
    { id: 1, key: `${family}older-main`, ref: "refs/heads/main", size_in_bytes: 500, created_at: "2026-11-01T00:00:00Z", last_accessed_at: "2026-11-01T00:00:00Z", version: "fixture" },
    { id: 2, key: `${family}pull-request`, ref: "refs/pull/7/merge", size_in_bytes: 300, created_at: "2026-12-01T00:00:00Z", last_accessed_at: "2026-12-01T00:00:00Z", version: "fixture" },
    { id: 3, key: `${family}tag`, ref: "refs/tags/v0.1.0", size_in_bytes: 200, created_at: "2026-12-02T00:00:00Z", last_accessed_at: "2026-12-02T00:00:00Z", version: "fixture" },
  ];
  for (const leg of LEGS) {
    it(`${leg}: superseded copies leave, the newest main copy stays, and nothing else moves`, async () => {
      let id = 10;
      const recorded = recordedEntries();
      const seeded = recorded.concat(familiesOf(leg).flatMap((family) => superseded(family).map((entry) => ({ ...entry, id: id++ }))));
      const api = await githubListing(seeded);
      try {
        const others = api.held().filter((entry) => !familiesOf(leg).some((family) => entry.key.startsWith(family)));
        const run = await runCleanup(leg, api);
        for (const family of familiesOf(leg)) {
          const newest = recorded.find((entry) => entry.key.startsWith(family));
          const expected = newest?.key ?? `${family}older-main`;
          assert.deepEqual(entriesOf(api, family).map((entry) => entry.key), [expected], family);
          assert.ok(run.summary.includes(expected), `the summary names what ${family} kept: ${run.summary}`);
        }
        assert.deepEqual(api.held().filter((entry) => !familiesOf(leg).some((family) => entry.key.startsWith(family))), others);
        assert.ok(others.some((entry) => entry.key.startsWith("nx-cache-")), "the recording holds test results to keep");
      } finally { await api.close(); }
    });
  }

  it("the double refuses a page size or number GitHub would refuse", async () => {
    const api = await githubListing();
    try {
      for (const query of ["per_page=0", "per_page=101", "page=0", "page=x"]) {
        const answer = await fetch(`${api.url}/repos/nickderobertis/onepipeline/actions/caches?${query}`, { headers: { authorization: "Bearer fixture" } });
        assert.equal(answer.status, 422, query);
      }
    } finally { await api.close(); }
  });

  it("the double fails a sort or direction it does not model instead of inventing an order", async () => {
    const api = await githubListing();
    try {
      for (const query of ["sort=key", "direction=up"]) {
        const answer = await fetch(`${api.url}/repos/nickderobertis/onepipeline/actions/caches?${query}`, { headers: { authorization: "Bearer fixture" } });
        assert.equal(answer.status, 501, query);
      }
    } finally { await api.close(); }
  });

  it("deletes nothing from a family with no copy on main, and says so", async () => {
    const pullOnly = { id: 1, key: "v0-rust-release-plz-Linux-x64-pull-only", ref: "refs/pull/7/merge", size_in_bytes: 300, created_at: "2026-12-01T00:00:00Z", last_accessed_at: "2026-12-01T00:00:00Z", version: "fixture" };
    const api = await githubListing([...recordedEntries(), pullOnly]);
    try {
      const run = await runCleanup("linux", api);
      assert.equal(entriesOf(api, "v0-rust-release-plz-Linux-x64-").length, 1);
      assert.match(run.stderr, /::warning::.*v0-rust-release-plz-Linux-x64-/);
    } finally { await api.close(); }
  });

  it("through consecutive pushes, a cancelled run among them, leaves no family more than two entries", async () => {
    const api = await githubListing();
    // Which saves each leg makes and when: the capture and the tools save before the
    // cleanup, as ordinary steps; rust-cache and release-plz's job save after it.
    const before = { linux: ["ci-capture-Linux-X64-", "bootstrap-tools-Linux-X64-"], windows: ["bootstrap-tools-Windows-X64-"], macos: ["bootstrap-tools-macOS-ARM64-"] };
    const after = { linux: ["v0-rust-test-linux-Linux-x64-", "v0-rust-release-plz-Linux-x64-"], windows: ["v0-rust-test-windows-Windows_NT-x64-"], macos: ["v0-rust-test-macos-Darwin-arm64-"] };
    const withinTwo = (when) => {
      for (const family of allFamilies) {
        const held = entriesOf(api, family).length;
        assert.ok(held <= 2, `${family} held ${held} entries ${when}`);
      }
    };
    try {
      for (const cycle of [1, 2, 3, 4, 5, 6]) {
        // Cycle 3 is cancelled after Linux's save and before the other legs get there.
        const legs = cycle === 3 ? ["linux"] : LEGS;
        for (const leg of legs) {
          for (const family of before[leg]) api.save(`${family}cycle-${cycle}`);
          withinTwo(`after ${leg}'s own early saves in cycle ${cycle}`);
          await runCleanup(leg, api);
          withinTwo(`after ${leg}'s cleanup in cycle ${cycle}`);
          for (const family of after[leg]) api.save(`${family}cycle-${cycle}`);
          withinTwo(`after ${leg}'s post-step saves in cycle ${cycle}`);
        }
      }
      for (const family of after.linux) assert.equal(entriesOf(api, family).at(-1).key, `${family}cycle-6`);
    } finally { await api.close(); }
  });
});

describe("a fork's run writes to no cache, under GitHub's recorded payloads", () => {
  const payload = (name) => JSON.parse(readFileSync(join(root, ".github/ci-shape/fixtures", name), "utf8"));
  const context = (name) => {
    const recorded = payload(name);
    return { github: { repository: recorded.repository, event: recorded.event }, steps: {} };
  };
  const pullRequestJobs = Object.entries(ci.jobs).filter(([, job]) => job.if === "github.event_name == 'pull_request'");
  const trustOutputs = (name) => {
    const recorded = payload(name);
    const dir = scratch("trust-");
    const event = join(dir, "event.json");
    writeFileSync(event, JSON.stringify(recorded.event));
    const outputs = join(dir, "outputs");
    const trust = ci.jobs["test-linux"].steps.find((step) => step.id === "nx-cache-trust");
    const run = spawnSync("bash", ["-eo", "pipefail", "-c", trust.run], { cwd: root, encoding: "utf8", env: {
      PATH: process.env.PATH, GITHUB_EVENT_PATH: event, GITHUB_REPOSITORY: recorded.repository, GITHUB_EVENT_NAME: "pull_request", GITHUB_OUTPUT: outputs, RUNNER_TEMP: dir,
    } });
    assert.equal(run.status, 0, run.stderr);
    return Object.fromEntries(readFileSync(outputs, "utf8").trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  };

  for (const [id, job] of pullRequestJobs) {
    it(`${id}: setup-uv's own cache is off for the fork and on for the same repository`, () => {
      const steps = job.steps.filter((step) => step.uses?.startsWith("astral-sh/setup-uv@"));
      assert.equal(steps.length, 1);
      assert.equal(evaluate(steps[0].with["enable-cache"], context("fork.json")), false);
      assert.equal(evaluate(steps[0].with["enable-cache"], context("same-repository.json")), true);
    });
    it(`${id}: the Nx sidecar is read-only for the fork and writes for the same repository`, () => {
      const sidecar = job.steps.find((step) => step.id === "nx-cache");
      const decide = (name) => evaluate(sidecar.env.CACHE_READ_ONLY, { ...context(name), steps: { "nx-cache-trust": { outputs: trustOutputs(name) } } });
      assert.equal(decide("fork.json"), "1");
      // Measurement only (spike-onepipeline-test-cost): every run, the same repository's included, is read-only.
      assert.equal(decide("same-repository.json"), "1");
    });
  }
});

describe("a warm leg's conditional target deletion, its step's real expression and command", () => {
  const coldOf = (leg) => {
    const steps = warm.jobs[leg].steps;
    const rust = steps.find((step) => step.uses?.startsWith("Swatinem/rust-cache@"));
    return { rust, cold: steps[steps.indexOf(rust) + 1] };
  };
  // On Windows the step's own working directory is the Dev Drive's copy of the checkout.
  function runDeletion(leg, cacheHit) {
    const { rust, cold } = coldOf(leg);
    const checkout = scratch("cold-");
    const devDrive = join(checkout, "dev-drive");
    const context = { env: { DEV_DRIVE_WORKSPACE: devDrive }, steps: { [rust.id]: { outputs: { "cache-hit": cacheHit } } } };
    const cwd = cold["working-directory"] ? String(evaluate(cold["working-directory"], context)) : checkout;
    mkdirSync(join(cwd, "target/debug/deps"), { recursive: true });
    mkdirSync(join(checkout, "src"), { recursive: true });
    const taken = Boolean(evaluate(cold.if, context));
    if (taken) assert.equal(spawnSync("bash", ["-eo", "pipefail", "-c", cold.run], { cwd }).status, 0);
    return { taken, targetStands: existsSync(join(cwd, "target/debug/deps")), sourceStands: existsSync(join(checkout, "src")) };
  }
  for (const leg of LEGS) {
    it(`${leg}: an inexact restore deletes target, and an exact one does not`, () => {
      assert.deepEqual(runDeletion(leg, "false"), { taken: true, targetStands: false, sourceStands: true });
      assert.deepEqual(runDeletion(leg, "true"), { taken: false, targetStands: true, sourceStands: true });
    });
  }
  it("windows deletes the Dev Drive workspace's target", () => {
    assert.equal(coldOf("windows").cold["working-directory"], "${{ env.DEV_DRIVE_WORKSPACE }}");
  });
});

test("the Linux leg's capture deletion leaves the rest of target for the family", () => {
  const step = warm.jobs.linux.steps.find((candidate) => candidate.run?.trim() === "rm -rf target/ci-capture-target");
  const dir = scratch("capture-");
  mkdirSync(join(dir, "target/ci-capture-target/release"), { recursive: true });
  mkdirSync(join(dir, "target/debug"), { recursive: true });
  assert.equal(spawnSync("bash", ["-eo", "pipefail", "-c", step.run], { cwd: dir }).status, 0);
  assert.equal(existsSync(join(dir, "target/ci-capture-target")), false);
  assert.equal(existsSync(join(dir, "target/debug")), true);
  rmSync(dir, { recursive: true, force: true });
});

describe("the registered headroom budget, through onebudgetspec, against GitHub's REST listing", () => {
  const ALLOWANCE = 10 * 1024 ** 3;
  const older = (family, id) => ({ id, key: `${family}older`, ref: "refs/heads/main", size_in_bytes: 3e9, created_at: "2026-01-01T00:00:00Z", last_accessed_at: "2026-01-01T00:00:00Z", version: "fixture" });
  const measure = async (api) => {
    const measured = await command(root, "onebudgetspec", ["check", "budgets.yaml", "--id", "onepipeline-cache-headroom", "--json"], {
      GITHUB_REPOSITORY: "nickderobertis/onepipeline", GITHUB_API_URL: api.url, GH_TOKEN: "fixture-token", GITHUB_TOKEN: "fixture-token",
    });
    return JSON.parse(measured.stdout).results[0];
  };
  const sizes = (entries) => entries.reduce((total, entry) => total + entry.size_in_bytes, 0);

  it("reads every page, counts results, and leaves a named family's older copy in the detail", async () => {
    const recorded = recordedEntries();
    const family = "v0-rust-test-linux-Linux-x64-";
    const api = await githubListing([...recorded, older(family, 1)]);
    try {
      const result = await measure(api);
      assert.ok(Math.abs(result.actual - (ALLOWANCE - sizes(recorded)) / 1e9) < 0.001, `${result.actual}: ${result.detail}`);
      assert.equal(result.verdict, "over", "the recorded listing has far less room than the target");
      assert.match(result.detail, new RegExp(`family ${family}: newest .*1 older copy`));
      assert.match(result.detail, /result/);
    } finally { await api.close(); }
  });

  it("reads within target once the families are shrunk to what the target allows", async () => {
    const small = recordedEntries().map((entry) => (entry.key.startsWith("nx-cache-") ? entry : { ...entry, size_in_bytes: Math.min(entry.size_in_bytes, 1e9) }));
    const api = await githubListing(small);
    try {
      const result = await measure(api);
      assert.equal(result.verdict, "within", `${result.actual}: ${result.detail}`);
    } finally { await api.close(); }
  });

  // A pull request's `budgets` job: its token cannot read the storage limit, and a listing
  // whose entries share a size and a creation time is reordered between pages.
  it("writes a figure against the stated 10 GiB under a refused limit read and a tied listing", async () => {
    const at = "2026-11-01T00:00:00Z";
    const tied = (key, id) => ({ id, key, ref: "refs/heads/main", size_in_bytes: 10_000_000, created_at: at, last_accessed_at: at, version: "fixture" });
    const entries = [
      ...Array.from({ length: 150 }, (_, n) => tied(`nx-cache-${String(n).padStart(3, "0")}`, n + 1)),
      ...Array.from({ length: 90 }, (_, n) => tied(`sccache-Linux-${n.toString(16).padStart(6, "0")}`, n + 1001)),
    ];
    const api = await githubListing(entries, "nickderobertis/onepipeline", { storageLimit: 403, reorderTies: true });
    try {
      // The double reorders ties as GitHub does: paging the whole listing repeats and omits.
      const paged = new Set();
      for (const page of [1, 2, 3]) {
        const answer = await fetch(`${api.url}/repos/nickderobertis/onepipeline/actions/caches?per_page=100&page=${page}`, { headers: { authorization: "Bearer fixture" } });
        for (const entry of (await answer.json()).actions_caches) paged.add(entry.id);
      }
      assert.ok(paged.size < entries.length, `a paged read saw ${paged.size} of ${entries.length}`);

      const result = await measure(api);
      assert.equal(typeof result.detail, "string", `no figure was written: ${JSON.stringify(result)}`);
      assert.equal(result.actual, Math.round(((ALLOWANCE - sizes(entries)) / 1e9) * 1000) / 1000, result.detail);
      assert.equal(ALLOWANCE, 10_737_418_240);
      assert.match(result.detail, /allowance: 10737418240 bytes \(GitHub's stated 10 GiB allowance; actions\/cache\/storage-limit answered HTTP 403\)/);
      assert.match(result.detail, new RegExp(`counted ${sizes(entries)} bytes`));
      assert.match(result.detail, /results: 150 entries, 1500000000 bytes/);
      assert.match(result.detail, /non-result sccache-Linux: 900000000 bytes/);
    } finally { await api.close(); }
  });

  it("reads 0 for a named family over two entries", async () => {
    const family = "ci-capture-Linux-X64-";
    const api = await githubListing([...recordedEntries().map((entry) => ({ ...entry, size_in_bytes: Math.min(entry.size_in_bytes, 1000) })), older(family, 1), { ...older(family, 2), key: `${family}oldest` }]);
    try {
      const result = await measure(api);
      assert.equal(result.actual, 0, result.detail);
      assert.equal(result.verdict, "over");
      assert.match(result.detail, new RegExp(family));
    } finally { await api.close(); }
  });
});

// The evidence is what the production workflow steps wrote, on real merge commits: the
// pull request's run tested one base and its merge landed on a later one, which changed
// the task's input. The release's history selection and GitHub's HTTP service are doubled.
test("the copied cache-use reads a merge landing after the final run as a base_moved miss", { timeout: 240000 }, async () => {
  const ws = workspace();
  const api = await githubCache();
  const commit = (message) => ws.git("-c", "user.name=Cache journey", "-c", "user.email=cache@example.invalid", "commit", "-qm", message);
  const merge = (first, second) => {
    ws.git("checkout", "-q", "--detach", first);
    ws.git("-c", "user.name=Cache journey", "-c", "user.email=cache@example.invalid", "merge", "-q", "--no-ff", "-m", "merge", second);
    return ws.git("rev-parse", "HEAD");
  };
  try {
    const prefix = `/repos/${repo.full_name}`;
    const feature = ws.head;
    const pr = pull(ws);
    // What the merge ref checked out: the base the run tested, then the head.
    ws.head = merge(ws.base, feature);
    const run = await sourceRun(ws, api, pr, 1);
    const execution = JSON.parse(readFileSync(join(run.report, "executions.json"), "utf8"));
    assert.equal(execution.base_sha, ws.base, "the run records the base its merge ref tested");

    // Another merge lands first and changes what the task reads; this one lands on it.
    ws.git("checkout", "-q", "--detach", ws.base);
    writeFileSync(join(ws.dir, "pool-maintenance/intervening.md"), "an intervening merge\n");
    ws.git("add", "-A");
    commit("fix: an intervening merge");
    const intervening = ws.git("rev-parse", "HEAD");
    const landed = merge(intervening, feature);
    pr.merge_commit_sha = landed;
    pr.merged_at = new Date().toISOString();
    sourceMetadata(api, ws, run.report, { id: 1, pr });
    api.metadata.set(`${prefix}/commits/${landed}`, { sha: landed, parents: [{ sha: intervening }, { sha: feature }] });
    api.metadata.set(`${prefix}/actions/workflows/release-plz.yml/runs`, { workflow_runs: [] });

    writeFileSync(join(ws.dir, "CHANGELOG.md"), `${readFileSync(join(ws.dir, "CHANGELOG.md"), "utf8")}\n## Release\n`);
    ws.git("add", "-A");
    commit("chore: release fixture");
    ws.head = ws.git("rev-parse", "HEAD");
    const release = pull(ws, 2);
    release.base.sha = landed;
    release.head.ref = "release-plz-fixture";
    const replay = await sourceRun(ws, api, release, 3);
    release.merged_at = new Date().toISOString();
    sourceMetadata(api, ws, replay.report, { id: 3, pr: release });
    api.metadata.set(`${prefix}/releases`, [
      { tag_name: "v-fixture", published_at: new Date(Date.now() + 1000).toISOString(), draft: false },
      { tag_name: "v-before", published_at: "2020-01-01T00:00:00.000Z", draft: false },
    ]);
    api.metadata.set(`${prefix}/commits/v-fixture`, { sha: ws.head });
    api.metadata.set(`${prefix}/compare/v-before...v-fixture`, { commits: [{ sha: landed }, { sha: ws.head }] });
    api.metadata.set(`${prefix}/compare/${landed}...${landed}`, { status: "identical" });
    const measured = await command(ws.dir, "onebudgetspec", ["check", "budgets.yaml", "--id", "onepipeline-nx-cache-use", "--json"], {
      GITHUB_REPOSITORY: repo.full_name, GITHUB_REF_NAME: "v-fixture", GITHUB_API_URL: api.url.replace(/\/$/, ""),
      GH_TOKEN: "fixture-token", GITHUB_TOKEN: "fixture-token", RUNNER_TEMP: scratch("base-moved-"),
    });
    const said = measured.stdout + measured.stderr;
    assert.match(said, /miss pool-maintenance:test\/\w+\/[0-9a-f]+: base_moved, #1 tested [0-9a-f]+ on base/, said);
    assert.match(said, /aggregate: 0\/1 remote hits/, said);
    assert.equal(JSON.parse(measured.stdout).results[0].verdict, "over", said);
  } finally { await api.close(); ws.close(); }
});
