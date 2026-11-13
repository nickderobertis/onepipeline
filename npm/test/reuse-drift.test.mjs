// The reuse spike's drift gates: each place its harness restates something the
// repository owns elsewhere is held equal to that owner here, so the restatement fails
// a test the day the owner changes rather than going quietly stale.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";
import { MEASURE_WORKFLOW } from "../../scripts/crate-split/demo-commit.mjs";
import { JOB_PLATFORM, NX_JOBS, restTierExclusions } from "../../scripts/crate-split/observed.mjs";

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
  // And nothing in it saves a cache.
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
