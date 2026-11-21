// The reuse spike's drift gates: each place its harness restates something the
// repository owns elsewhere is held equal to that owner here, so the restatement fails
// a test the day the owner changes rather than going quietly stale.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";
import { MEASURE_WORKFLOW } from "../../scripts/crate-split/demo-commit.mjs";
import {
  E2E_TASK,
  JOB_PLATFORM,
  NX_JOBS,
  REST_TASK,
  restTierExclusions,
} from "../../scripts/crate-split/observed.mjs";
import {
  artifacts,
  cachePage,
  executions,
  jobsListing,
  mergedList,
  NX_CACHE_KEY,
  namedInputs,
  rateLimit,
  rollup,
} from "../../scripts/crate-split/validate.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const ci = parse(readFileSync(join(root, ".github", "workflows", "ci.yml"), "utf8"));
const RUNNER_PLATFORM = { ubuntu: "linux", windows: "win32", macos: "darwin" };

test("the jobs the harness reads are the ones ci-job.sh runs, on the platforms ci.yml gives them", () => {
  const script = readFileSync(join(root, "scripts", "ci-job.sh"), "utf8");
  const block = script.slice(
    script.indexOf('case "$job" in'),
    script.indexOf("esac", script.indexOf('case "$job" in')),
  );
  const jobs = [...block.matchAll(/^\s+([\w| -]+)\)/gm)]
    .flatMap((m) => m[1].split("|").map((j) => j.trim()))
    .filter((j) => j !== "*");
  assert.deepEqual([...NX_JOBS].sort(), jobs.sort());
  for (const job of NX_JOBS) {
    const definition = ci.jobs[job];
    assert.ok(definition, `ci.yml has no ${job} job`);
    assert.ok(
      definition.steps.some((step) => String(step.run ?? "").includes(`just ci-${job}`)),
      `${job} runs no just ci-${job}`,
    );
    assert.equal(RUNNER_PLATFORM[definition["runs-on"].split("-")[0]], JOB_PLATFORM[job], job);
  }
});

test("the measurement workflow sets up each platform as ci.yml's test job does", () => {
  const measure = parse(MEASURE_WORKFLOW);
  const step = (steps, uses) => steps.find((s) => String(s.uses ?? "").startsWith(uses));
  for (const [platform, job] of [
    ["linux", "test-linux"],
    ["macos", "test-macos"],
    ["windows", "test-windows"],
  ]) {
    const mine = measure.jobs[`measure-${platform}`];
    const theirs = ci.jobs[job];
    assert.equal(mine["runs-on"], theirs["runs-on"], `${platform} runner`);
    // NEXTEST_PROFILE picks the profile nextest runs tests under, which an archive build
    // does not read; the measurement sets it on every platform alike.
    const { NEXTEST_PROFILE: _mine, ...mineEnv } = mine.env ?? {};
    const { NEXTEST_PROFILE: _theirs, ...theirsEnv } = theirs.env ?? {};
    assert.deepEqual(mineEnv, theirsEnv, `${platform} env`);
    for (const uses of [
      "actions/checkout@",
      "Swatinem/rust-cache@",
      "taiki-e/install-action@",
      "actions/setup-node@",
      "samypr100/setup-dev-drive@",
    ]) {
      const a = step(mine.steps, uses);
      const b = step(theirs.steps, uses);
      assert.equal(Boolean(a), Boolean(b), `${platform}: ${uses} in one and not the other`);
      if (!a) continue;
      assert.equal(a.uses, b.uses, `${platform}: ${uses} version`);
      assert.deepEqual(a.with, b.with, `${platform}: ${uses} inputs`);
    }
    // The Defender exclusion step is the same script on Windows.
    const defender = (steps) =>
      steps.find((s) => String(s.run ?? "").includes("Add-MpPreference"))?.run;
    assert.equal(defender(mine.steps), defender(theirs.steps), `${platform}: Defender exclusion`);
  }
  // And nothing in it saves a cache. This is the offline half of that proof: a hosted
  // run cannot be started from a test, and its other half, the read-only listing of the
  // caches each demonstration's ref holds after its run, is recorded in the spike report.
  for (const job of Object.values(measure.jobs)) {
    for (const s of job.steps) {
      assert.ok(!String(s.uses ?? "").startsWith("actions/cache"), "an actions/cache step");
      if (String(s.uses ?? "").startsWith("Swatinem/rust-cache@"))
        assert.equal(s.with["save-if"], false);
    }
  }
});

test("option (d1)'s keys hold what nx.json's sharedGlobals and rustBuild hold, and name files that exist", () => {
  const nx = JSON.parse(readFileSync(join(root, "nx.json"), "utf8"));
  const mapping = JSON.parse(
    readFileSync(join(root, "scripts", "crate-split", "option-d-install-proofs.json"), "utf8"),
  );
  const expect = new Set();
  const walk = (name) => {
    for (const input of nx.namedInputs[name]) {
      if (typeof input === "string" && nx.namedInputs[input]) walk(input);
      else if (typeof input === "string") expect.add(input.replace("{workspaceRoot}/", ""));
      else if (input.runtime) expect.add(`runtime:${input.runtime}`);
    }
  };
  walk("sharedGlobals");
  for (const input of nx.namedInputs.rustBuild) {
    if (
      typeof input === "string" &&
      !nx.namedInputs[input] &&
      !/\{workspaceRoot\}\/(src|crates|schemas)\//.test(input)
    ) {
      expect.add(input.replace("{workspaceRoot}/", ""));
    }
  }
  // The version-kept Cargo input stands for rustBuild's version-free one: an install
  // proof asserts the version, so a bump reruns it.
  expect.delete("runtime:node scripts/nx-version-free-inputs.mjs --ecosystem cargo");
  expect.add("runtime:node scripts/nx-version-free-inputs.mjs --ecosystem cargo --keep-version");
  // ensure-test-archive.sh builds the tiers' archive and .config/nextest.toml configures
  // nextest; no install proof runs either.
  expect.delete("scripts/ensure-test-archive.sh");
  expect.delete(".config/nextest.toml");
  for (const input of expect)
    assert.ok(mapping.shared.includes(input), `option (d1) misses ${input}`);
  for (const [task, suite] of Object.entries(mapping.suites)) {
    for (const path of suite.paths.filter((p) => !p.includes("*"))) {
      assert.ok(existsSync(join(root, path)), `${task} names ${path}, which does not exist`);
    }
    const [project, target] = task.split(":");
    const graph = JSON.parse(
      execFileSync("npx", ["nx", "show", "project", project, "--json"], {
        cwd: root,
        encoding: "utf8",
        env: { ...process.env, NX_DAEMON: "false" },
      }),
    );
    assert.ok(graph.targets[target], `${task} is no Nx target`);
    assert.equal(
      graph.targets[target].cache,
      false,
      `${task} is cached already, so (d1) is no option for it`,
    );
  }
});

test("the rest tier's exclusions are read from the justfile's own filter", () => {
  const filter = execFileSync("just", ["--evaluate", "rest-tier"], { cwd: root, encoding: "utf8" });
  const excluded = restTierExclusions(filter);
  // Every tier binary the e2e family and the workloads run, and none the rest tier runs.
  for (const binary of ["e2e", "contract", "pooled_slot", "narrow_write", "note", "smoke"]) {
    assert.ok(excluded.has(binary), binary);
  }
  assert.ok(!excluded.has("onepipeline"));
  assert.throws(() => restTierExclusions("all()"), /names no binary/);
});

test("the tier tasks the weights name are the ones each platform's job runs", () => {
  const script = readFileSync(join(root, "scripts", "ci-job.sh"), "utf8");
  // Each arm of ci-job.sh's `case "$job" in`: its job labels and the targets they run.
  const arms = [...script.matchAll(/^\s+([\w| -]+)\) targets="([^"]+)"/gm)].map((m) => ({
    jobs: m[1].split("|").map((j) => j.trim()),
    targets: m[2].split(","),
  }));
  const targets = (job) => arms.find((arm) => arm.jobs.includes(job)).targets;
  const show = (project) =>
    JSON.parse(
      execFileSync("npx", ["nx", "show", "project", project, "--json"], {
        cwd: root,
        encoding: "utf8",
        env: { ...process.env, NX_DAEMON: "false" },
      }),
    );
  const jobOf = { linux: "test-linux", darwin: "test-macos", win32: "test-windows" };
  for (const [platform, job] of Object.entries(jobOf)) {
    for (const task of [E2E_TASK[platform], REST_TASK[platform]]) {
      const [project, target] = task.split(":");
      assert.ok(show(project).targets[target], `${task} is no Nx target`);
      // The job runs the target, or a target of the crate that depends on it.
      const runs =
        targets(job).includes(target) ||
        (show("onepipeline").targets.test.dependsOn ?? []).includes(target);
      assert.ok(runs, `${job} runs no ${task}`);
    }
  }
});

test("option (d1)'s install proofs are keyed on every script install-proof.sh and the recipes it calls read", () => {
  const mapping = JSON.parse(
    readFileSync(join(root, "scripts", "crate-split", "option-d-install-proofs.json"), "utf8"),
  );
  const proof = readFileSync(join(root, "scripts", "install-proof.sh"), "utf8");
  const scripts = new Set(proof.match(/scripts\/[\w./-]+\.(?:sh|mjs|py)/g));
  for (const recipe of proof.matchAll(/\bjust ([\w-]+)/g)) {
    if (recipe[1] === "lint") continue; // named in a message, not run
    const body = execFileSync("just", ["--show", recipe[1]], { cwd: root, encoding: "utf8" });
    for (const s of body.match(/scripts\/[\w./-]+\.(?:sh|mjs|py)/g) ?? []) scripts.add(s);
  }
  scripts.delete("scripts/ci-job.sh"); // named in a message, not run
  for (const task of ["onepipeline:install-smoke", "onepipeline:wheel-smoke"]) {
    const keyed = new Set([...mapping.suites[task].paths, ...mapping.shared]);
    assert.ok(keyed.has("justfile"), `${task} is not keyed on the justfile whose recipes it runs`);
    for (const s of scripts) {
      // The wheel's build script is the wheel proof's own.
      if (task === "onepipeline:install-smoke" && s === "scripts/build-linux-wheel.sh") continue;
      if (task === "onepipeline:wheel-smoke" && s === "scripts/host-view-measure.mjs") continue;
      assert.ok(keyed.has(s), `${task} is not keyed on ${s}`);
    }
  }
});

test("the executions reader reads what nx-remote-cache.mjs's report itself writes", () => {
  const temp = mkdtempSync(join(tmpdir(), "reuse-executions-"));
  const state = join(temp, "nx-cache-recording", "test-linux");
  const at = (s) => new Date(Date.parse("2026-11-01T00:00:00Z") + s * 1000).toISOString();
  mkdirSync(state, { recursive: true });
  writeFileSync(
    join(state, "started.jsonl"),
    `${JSON.stringify({ id: "inv", argv: ["nx", "run-many"] })}\n`,
  );
  writeFileSync(
    join(state, "invocations.jsonl"),
    `${JSON.stringify({
      id: "inv",
      started_at: at(0),
      finished_at: at(60),
      tasks: [
        {
          task: "onepipeline-e2e:test",
          hash: "123",
          cacheable: true,
          status: "success",
          code: 0,
          started_at: at(1),
          finished_at: at(50),
        },
        {
          task: "onepipeline:bootstrap",
          hash: "456",
          cacheable: false,
          status: "success",
          code: 0,
          started_at: at(1),
          finished_at: at(2),
        },
      ],
    })}\n`,
  );
  const sha = "a".repeat(40);
  writeFileSync(
    join(temp, "event.json"),
    JSON.stringify({ pull_request: { number: 9, head: { sha } } }),
  );
  const out = join(temp, "out");
  const run = spawnSync(
    process.execPath,
    [join(root, "scripts", "nx-remote-cache.mjs"), "report", "--job", "test-linux", "--out", out],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        RUNNER_TEMP: temp,
        GITHUB_JOB: "test-linux",
        GITHUB_EVENT_PATH: join(temp, "event.json"),
        GITHUB_REPOSITORY: "o/n",
        GITHUB_REF: "refs/pull/9/merge",
        GITHUB_RUN_ID: "77",
        GITHUB_RUN_ATTEMPT: "1",
        GITHUB_SHA: sha,
        NX_BASE: "b".repeat(40),
        NX_HEAD: sha,
        GITHUB_STEP_SUMMARY: "",
      },
    },
  );
  assert.ok(existsSync(join(out, "executions.json")), run.stderr);
  const doc = JSON.parse(readFileSync(join(out, "executions.json"), "utf8"));
  assert.equal(executions(doc, "the producer's record"), doc);
  assert.deepEqual(
    doc.tasks.map((t) => [t.task, t.result, t.remote_read]),
    [
      ["onepipeline-e2e:test", "executed", "not_attempted"],
      ["onepipeline:bootstrap", "executed", "not_attempted"],
    ],
  );
  rmSync(temp, { recursive: true, force: true });
});

test("the cache key spelling the inventory reads is the pinned sidecar's, which ci.yml still starts", () => {
  const starts = Object.values(ci.jobs).flatMap((job) => (job.steps ?? []).map((s) => s.uses).filter((u) => String(u).startsWith("op-nx/github-cache/")));
  assert.ok(starts.length > 0, "ci.yml starts no op-nx/github-cache sidecar");
  for (const uses of starts) assert.equal(uses, NX_CACHE_KEY.producer, "ci.yml pins another sidecar: read its key spelling again");
  // nx-remote-cache.mjs records against the same pin.
  assert.match(readFileSync(join(root, "scripts", "nx-remote-cache.mjs"), "utf8"), new RegExp(`OP_NX_COMMIT = '${NX_CACHE_KEY.producer.split("@")[1]}'`));
});

test("the GitHub response checks pass GitHub's own answers, as this harness captured them", () => {
  // Captured from nickderobertis/onepipeline by fetch-evidence.mjs and demo-runs' reads
  // on 2026-11-13: a rate-limit answer, merged pull requests, #1543's check suites, run
  // 43308636421's artifacts, a page of the cache listing and run 43403322259's jobs.
  const fixture = (name) => JSON.parse(readFileSync(join(root, "tests", "reuse_harness", "fixtures", "github", name), "utf8"));
  assert.ok(rateLimit(fixture("rate-limit-core.json"), "core").reset > 0);
  assert.equal(mergedList(fixture("pr-list.json")).length, 2);
  assert.match(rollup(fixture("rollup-1543.json"), 1543).oid, /^[0-9a-f]{40}$/);
  assert.equal(artifacts(fixture("artifacts-43308636421.json"), 43308636421).length, 3);
  assert.equal(cachePage(fixture("caches-page.json"), 1).length, 3);
  assert.equal(jobsListing(fixture("jobs-43403322259.json"), 43403322259).length, 1);
});

test("a named input is held to the installed Nx's own input schema, every kind it defines and nothing else", () => {
  const schema = JSON.parse(
    readFileSync(join(root, "node_modules", "nx", "schemas", "nx-schema.json"), "utf8"),
  );
  // One example of every object kind the installed Nx defines, built from its own
  // schema: each property its schema requires, with a value of the type it declares.
  const sample = (type) =>
    ({ string: "{projectRoot}/x", boolean: true, array: ["x"] })[type] ?? "{projectRoot}/x";
  const kinds = schema.definitions.inputs.items.oneOf.filter((v) => v.type === "object");
  assert.ok(kinds.length >= 7, "the installed Nx defines its input kinds");
  for (const kind of kinds) {
    const first = Object.keys(kind.properties).find(
      (k) => kind.required?.includes(k) || !kind.required,
    );
    const required = kind.required ?? (kind.oneOf ? ["input"] : [first]);
    const input = Object.fromEntries(
      required.map((k) => [k, sample(kind.properties[k].type ?? "string")]),
    );
    assert.doesNotThrow(
      () => namedInputs({ n: [input] }, "nx.json"),
      `Nx's ${JSON.stringify(input)} input`,
    );
  }
  // A kind Nx 23 added after the hand-kept list this replaced: a JSON file's fields.
  namedInputs({ n: [{ json: "{projectRoot}/package.json", fields: ["version"] }] }, "nx.json");
  namedInputs({ n: ["{projectRoot}/**/*", { input: "rustBuild", projects: ["a", "b"] }] }, "x");
  for (const bad of [
    { runtme: "node -v" },
    { runtime: 7 },
    { fileset: "x", input: "y" },
    { dependentTasksOutputFiles: "x", dependencies: true },
    { input: "x", projects: "a", dependencies: true },
    {},
    7,
  ]) {
    assert.throws(
      () => namedInputs({ n: [bad] }, "nx.json"),
      /namedInputs is not an object of input lists/,
      JSON.stringify(bad),
    );
  }
});
