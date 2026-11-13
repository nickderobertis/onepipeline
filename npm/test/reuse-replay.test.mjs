// The reuse spike's harness, run as its commands: the evidence fetch against a `gh`
// double that records every call it is asked to make, the observed replay against a
// real git repository holding a small real Nx workspace (Nx's own hash plans, read at
// each tested tree and its base), the tier mapping generated from it and replayed with
// per-platform weights, and the module graph of a small crate.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const replayScript = join(root, "scripts", "crate-split-replay.mjs");
const fetchScript = join(root, "scripts", "crate-split", "fetch-evidence.mjs");
const tierScript = join(root, "scripts", "crate-split", "tier-mapping.mjs");
const graphScript = join(root, "scripts", "crate-split", "module-graph.mjs");

let scratch;

before(() => {
  scratch = mkdtempSync(join(tmpdir(), "reuse-replay-"));
});
after(() => rmSync(scratch, { recursive: true, force: true }));

function write(dir, files) {
  for (const [path, body] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), body);
  }
}

const gitEnv = (date) => ({
  ...process.env,
  GIT_AUTHOR_NAME: "reuse",
  GIT_AUTHOR_EMAIL: "reuse@example.invalid",
  GIT_COMMITTER_NAME: "reuse",
  GIT_COMMITTER_EMAIL: "reuse@example.invalid",
  GIT_AUTHOR_DATE: date,
  GIT_COMMITTER_DATE: date,
  GIT_CONFIG_GLOBAL: join(scratch, "gitconfig"),
});

function git(repo, args, date = "2026-11-01T00:00:00Z") {
  return execFileSync("git", ["-C", repo, ...args], { encoding: "utf8", env: gitEnv(date) }).trim();
}

// ---- the evidence fetch, against a gh double --------------------------------------

/** A `gh` on PATH that answers from a state file and appends each call to a log. */
function ghDouble(dir, state) {
  const bin = join(dir, "bin");
  mkdirSync(bin, { recursive: true });
  writeFileSync(join(dir, "state.json"), JSON.stringify(state));
  rmSync(join(dir, "calls.log"), { force: true });
  writeFileSync(
    join(bin, "gh"),
    `#!/usr/bin/env node
const fs = require("node:fs");
const dir = ${JSON.stringify(dir)};
const args = process.argv.slice(2);
const state = JSON.parse(fs.readFileSync(dir + "/state.json", "utf8"));
fs.appendFileSync(dir + "/calls.log", JSON.stringify(args) + "\\n");
if (args[0] === "api" && args[1] === "rate_limit") {
  const resource = args[3].match(/resources\\.(\\w+)/)[1];
  const seq = state[resource];
  const at = Math.min(state.reads?.[resource] ?? 0, seq.length - 1);
  state.reads = { ...state.reads, [resource]: (state.reads?.[resource] ?? 0) + 1 };
  fs.writeFileSync(dir + "/state.json", JSON.stringify(state));
  process.stdout.write(JSON.stringify({ remaining: seq[at], reset: Math.floor(Date.now() / 1000) - 29 }));
} else if (args[0] === "pr") {
  process.stdout.write(JSON.stringify(state.merged));
} else if (args[0] === "api" && args[1] === "graphql") {
  process.stdout.write(JSON.stringify(state.rollup));
} else if (args.at(-1).includes("/artifacts?")) {
  process.stdout.write(JSON.stringify({ artifacts: [] }));
} else if (args.at(-1).endsWith("/logs")) {
  process.stdout.write("2026-11-01T00:00:00.0000000Z the job's log\\n");
} else {
  process.stderr.write("the double has no answer for " + args.join(" "));
  process.exit(1);
}
`,
  );
  chmodSync(join(bin, "gh"), 0o755);
  return bin;
}

const merged = [
  { number: 3, title: "feat: three", mergedAt: "2026-11-03T00:00:00Z", headRefOid: "c".repeat(40) },
  {
    number: 2,
    title: "chore: release v1.0.1",
    mergedAt: "2026-11-02T00:00:00Z",
    headRefOid: "b".repeat(40),
  },
  { number: 1, title: "fix: one", mergedAt: "2026-11-01T00:00:00Z", headRefOid: "a".repeat(40) },
  {
    number: 0,
    title: "chore: release v1.0.0",
    mergedAt: "2026-10-30T00:00:00Z",
    headRefOid: "0".repeat(40),
  },
];
const rollup = {
  data: {
    repository: {
      pullRequest: {
        commits: {
          nodes: [
            {
              commit: {
                oid: "c".repeat(40),
                checkSuites: {
                  nodes: [
                    {
                      workflowRun: {
                        databaseId: 77,
                        workflow: { name: "CI" },
                        createdAt: "2026-11-01T00:00:00Z",
                      },
                      conclusion: "SUCCESS",
                      checkRuns: {
                        nodes: [
                          {
                            name: "test-linux",
                            databaseId: 501,
                            conclusion: "SUCCESS",
                            startedAt: "2026-11-01T00:00:00Z",
                            completedAt: "2026-11-01T01:00:00Z",
                          },
                          {
                            name: "budgets",
                            databaseId: 502,
                            conclusion: "SUCCESS",
                            startedAt: "2026-11-01T00:00:00Z",
                            completedAt: "2026-11-01T00:01:00Z",
                          },
                        ],
                      },
                    },
                  ],
                },
              },
            },
          ],
        },
      },
    },
  },
};

function fetchWith(name, state, extra = []) {
  const dir = join(scratch, name);
  const bin = ghDouble(dir, { merged, rollup, ...state });
  const run = spawnSync(
    process.execPath,
    [fetchScript, "--out", join(dir, "out"), "--prs", "2", ...extra],
    {
      encoding: "utf8",
      env: { ...process.env, PATH: `${bin}:${process.env.PATH}` },
    },
  );
  const log = join(dir, "calls.log");
  const calls = existsSync(log)
    ? readFileSync(log, "utf8")
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line))
    : [];
  return { run, calls, out: join(dir, "out") };
}

const reads = (calls) => calls.filter((args) => !(args[0] === "api" && args[1] === "rate_limit"));

test("the fetch reads the population and every Nx job's log and record, and spends nothing a floor protects", () => {
  const { run, calls, out } = fetchWith("fetch-ok", { core: [4000], graphql: [4000] });
  assert.equal(run.status, 0, run.stderr);
  assert.match(run.stdout, /3 pull requests \(2 non-release, 1 release\)/);
  const population = JSON.parse(readFileSync(join(out, "population.json"), "utf8"));
  // The two newest non-release, and the release merged since the older of them.
  assert.deepEqual(
    population.non_release.map((pr) => pr.number),
    [3, 1],
  );
  assert.deepEqual(
    population.release.map((pr) => pr.number),
    [2],
  );
  assert.ok(existsSync(join(out, "logs", "501.log")));
  // budgets runs no Nx task, so its log is not read.
  assert.ok(!existsSync(join(out, "logs", "502.log")));
  assert.deepEqual(JSON.parse(readFileSync(join(out, "executions", "501.json"), "utf8")), {
    absent: "none",
  });
  // Every read was preceded by a read of the allowance it spends.
  const kinds = calls.map((args) =>
    args[1] === "rate_limit"
      ? `limit:${args[3].match(/resources\.(\w+)/)[1]}`
      : args[0] === "pr" || args[1] === "graphql"
        ? "graphql"
        : "core",
  );
  for (const [i, kind] of kinds.entries()) {
    if (kind === "graphql" || kind === "core")
      assert.equal(kinds[i - 1], `limit:${kind}`, kinds.join(" "));
  }
  // A second run fetches nothing it already holds.
  const again = fetchWith("fetch-ok", { core: [4000], graphql: [4000] });
  assert.equal(again.run.status, 0, again.run.stderr);
  assert.deepEqual(reads(again.calls), []);
});

test("below the GraphQL floor the fetch makes no GraphQL read and says what stopped it", () => {
  const { run, calls } = fetchWith("fetch-graphql-floor", { core: [4000], graphql: [1999] });
  assert.equal(run.status, 2);
  assert.match(run.stderr, /1999 graphql remain, below the floor of 2000/);
  assert.match(run.stderr, /^next: /m);
  assert.deepEqual(reads(calls), []);
});

test("below the core floor the fetch stops before its first core read", () => {
  const { run, calls } = fetchWith("fetch-core-floor", { core: [2499], graphql: [4000] });
  assert.equal(run.status, 2);
  assert.match(run.stderr, /2499 core remain, below the floor of 2500/);
  assert.equal(reads(calls).filter((args) => args[0] === "api" && args[1] !== "graphql").length, 0);
});

test("with --wait the fetch pauses at a floor until the allowance resets, then reads", () => {
  const { run, calls } = fetchWith("fetch-wait", { core: [100, 4000], graphql: [4000] }, [
    "--wait",
  ]);
  assert.equal(run.status, 0, run.stderr);
  assert.match(run.stderr, /100 core remain, below the floor of 2500; pausing until/);
  assert.ok(reads(calls).some((args) => args.at(-1).endsWith("/logs")));
});

test("a floor that is not a whole number is refused before anything is read", () => {
  const { run, calls } = fetchWith("fetch-bad-floor", { core: [4000], graphql: [4000] }, [
    "--floor",
    "lots",
  ]);
  assert.equal(run.status, 1);
  assert.match(run.stderr, /--floor is not a whole number/);
  assert.deepEqual(calls, []);
});

// ---- the observed replay, over a real Nx workspace --------------------------------

let repo;
let evidence;
let observedJson;
const commits = {};

function nxWorkspace() {
  repo = join(scratch, "workspace");
  mkdirSync(repo);
  writeFileSync(join(scratch, "gitconfig"), "");
  git(repo, ["init", "-q", "--initial-branch=main"]);
  write(repo, {
    ".gitignore": "node_modules\n.nx\n",
    "package.json": JSON.stringify({ name: "ws", private: true }),
    "nx.json": JSON.stringify({
      namedInputs: {
        rustBuild: ["{workspaceRoot}/src/**/*", "{workspaceRoot}/Cargo.toml"],
        e2eSource: ["rustBuild", "{workspaceRoot}/tests/e2e/**/*"],
        docsSource: ["{workspaceRoot}/docs/**/*"],
      },
    }),
    "project.json": JSON.stringify({
      name: "eng",
      root: ".",
      targets: {
        test: {
          command: "true",
          cache: true,
          inputs: ["e2eSource", "docsSource", { runtime: "node scripts/recipe.cjs" }],
        },
        lint: { command: "true", cache: true, inputs: ["rustBuild"] },
        smoke: { command: "true", cache: false, inputs: ["rustBuild"] },
      },
    }),
    "scripts/recipe.cjs": 'console.log(require("node:fs").readFileSync("recipe.txt", "utf8"))\n',
    "recipe.txt": "v1\n",
    "src/a.rs": "a\n",
    "tests/e2e/x.rs": "x\n",
    "docs/guide.md": "guide\n",
    "CHANGELOG.md": "0.1\n",
  });
  git(repo, ["add", "-A"]);
  git(repo, ["commit", "-qm", "chore: seed"]);
  commits.base = git(repo, ["rev-parse", "HEAD"]);
  const branch = (name, files, from = commits.base) => {
    git(repo, ["checkout", "-q", "-b", name, from]);
    write(repo, files);
    git(repo, ["add", "-A"]);
    git(repo, ["commit", "-qm", name]);
    const head = git(repo, ["rev-parse", "HEAD"]);
    git(repo, ["checkout", "-q", "main"]);
    return head;
  };
  commits.src = branch("src-only", { "src/a.rs": "a2\n" });
  commits.several = branch("several", { "tests/e2e/x.rs": "x2\n", "recipe.txt": "v2\n" });
  commits.release = branch("release", { "CHANGELOG.md": "0.2\n" });
}

const at = (minutes) =>
  new Date(Date.parse("2026-11-01T00:00:00Z") + minutes * 60000).toISOString();

function job(id, name) {
  return { name, databaseId: id, conclusion: "SUCCESS", startedAt: at(0), completedAt: at(60) };
}

function evidenceDir() {
  evidence = join(scratch, "evidence");
  const pr = (number, head, jobs, runId) => {
    write(evidence, {
      [`rollup/${number}.json`]: JSON.stringify({
        data: {
          repository: {
            pullRequest: {
              commits: {
                nodes: [
                  {
                    commit: {
                      oid: head,
                      checkSuites: {
                        nodes: [
                          {
                            workflowRun: {
                              databaseId: runId,
                              workflow: { name: "CI" },
                              createdAt: at(0),
                            },
                            checkRuns: { nodes: jobs },
                          },
                        ],
                      },
                    },
                  },
                ],
              },
            },
          },
        },
      }),
    });
  };
  const log = (head, lines) =>
    [
      `${at(0)} HEAD is now at 1234567 Merge ${head} into ${commits.base}`,
      ...lines.map((line, i) => `${at(i + 1)} ${line}`),
      `${at(30)} Nx read the output from the cache instead of running the command for 1 out of ${lines.length} tasks.`,
    ].join("\n");
  write(evidence, {
    "population.json": JSON.stringify({
      window_from: at(0),
      non_release: [
        { number: 11, title: "feat: change the source", mergedAt: at(1) },
        { number: 12, title: "test: change a journey and a recipe", mergedAt: at(2) },
        { number: 14, title: "fix: a run whose log is gone", mergedAt: at(4) },
      ],
      release: [{ number: 13, title: "chore: release v0.2", mergedAt: at(3) }],
    }),
    // #11: the source changed, so test and lint ran; smoke is never cached.
    "logs/1101.log": log(commits.src, [
      "##[group]> nx run eng:test",
      "     Starting 3 tests across 1 binary",
      "     Summary [  120.000s] 3 tests run: 3 passed",
      "> nx run eng:lint",
      "> nx run eng:smoke",
    ]),
    "executions/1101.json": JSON.stringify({
      tasks: [
        {
          task: "eng:test",
          hash: "111",
          cacheable: true,
          result: "executed",
          remote_read: "miss",
          started_at: at(1),
          finished_at: at(11),
        },
        {
          task: "eng:lint",
          hash: "222",
          cacheable: true,
          result: "executed",
          remote_read: "miss",
          started_at: at(11),
          finished_at: at(12),
        },
        {
          task: "eng:smoke",
          hash: "333",
          cacheable: false,
          result: "executed",
          remote_read: "not_attempted",
          started_at: at(12),
          finished_at: at(15),
        },
      ],
      store_attempts: [{ task: "eng:lint", hash: "999", status: "stored", attempted_at: at(12) }],
    }),
    // #12: a journey and the recipe a runtime input prints both moved; lint replayed.
    "logs/1201.log": log(commits.several, [
      "> nx run eng:test",
      "> nx run eng:lint  [remote cache]",
    ]),
    "executions/1201.json": JSON.stringify({
      tasks: [
        {
          task: "eng:test",
          hash: "444",
          cacheable: true,
          result: "executed",
          remote_read: "miss",
          started_at: at(20),
          finished_at: at(28),
        },
        {
          task: "eng:lint",
          hash: "222",
          cacheable: true,
          result: "remote_hit",
          remote_read: "hit",
          started_at: at(28),
          finished_at: at(28.1),
        },
      ],
      store_attempts: [],
    }),
    // #13: only the changelog moved, which no key holds; lint missed a key #11's run stored.
    "logs/1301.log": log(commits.release, [
      "> nx run eng:test  [remote cache]",
      "> nx run eng:lint",
    ]),
    "executions/1301.json": JSON.stringify({
      tasks: [
        {
          task: "eng:test",
          hash: "555",
          cacheable: true,
          result: "remote_hit",
          remote_read: "hit",
          started_at: at(40),
          finished_at: at(40.1),
        },
        {
          task: "eng:lint",
          hash: "999",
          cacheable: true,
          result: "executed",
          remote_read: "miss",
          started_at: at(40),
          finished_at: at(41),
        },
      ],
      store_attempts: [],
    }),
  });
  pr(11, commits.src, [job(1101, "test-linux"), job(1102, "budgets")], 110);
  pr(12, commits.several, [job(1201, "test-macos")], 120);
  pr(13, commits.release, [job(1301, "test-linux")], 130);
  pr(14, commits.src, [job(1401, "test-linux")], 140);
}

function observedRun(out, extra = []) {
  return spawnSync(
    process.execPath,
    [replayScript, "--observed", evidence, "--clone", repo, "--out", out, ...extra],
    { encoding: "utf8" },
  );
}

test("the observed replay names the key input each pull request's diff matched for every task that ran", () => {
  nxWorkspace();
  evidenceDir();
  observedJson = join(scratch, "observed.json");
  const run = observedRun(observedJson, ["--json"]);
  assert.equal(run.status, 0, run.stderr);
  assert.equal(
    run.stdout.trim(),
    `crate-split-replay: wrote ${observedJson}: 3 of 4 pull requests read, 2 task runs replayed and 5 ran; 3 of those the diff forced, 2 it did not`,
  );
  const result = JSON.parse(readFileSync(observedJson, "utf8"));
  const pr = (n) => result.prs.find((p) => p.pr === n);
  const ran = (n, task) => pr(n).jobs[0].ran.find((row) => row.task === task);

  // A source file is rustBuild's, however many named inputs reach it.
  assert.deepEqual(pr(11).changed, ["src/a.rs"]);
  assert.deepEqual(ran(11, "eng:test").inputs, ["rustBuild"]);
  assert.equal(ran(11, "eng:test").cause, "one input");
  assert.equal(ran(11, "eng:test").minutes, 10);
  // The log's nextest lines split the task into its build and its tests.
  assert.equal(ran(11, "eng:test").buildMinutes, 1);
  assert.equal(ran(11, "eng:test").testMinutes, 2);
  assert.deepEqual(ran(11, "eng:lint").inputs, ["rustBuild"]);
  assert.equal(ran(11, "eng:smoke").cause, "not cacheable");

  // A journey file and a runtime input whose command prints differently are two inputs.
  assert.deepEqual(ran(12, "eng:test").inputs, ["e2eSource", "runtime:node scripts/recipe.cjs"]);
  assert.equal(ran(12, "eng:test").cause, "several inputs");
  assert.deepEqual(pr(12).moved, ["runtime:node scripts/recipe.cjs"]);
  // lint's key holds neither, so it replayed, and that replay is no diff-forced key.
  assert.deepEqual(
    pr(12).jobs[0].replayed.map((row) => [row.task, row.diffMatched]),
    [["eng:lint", false]],
  );

  // Nothing a key holds moved, so the cause is the run's own record: a key run 110 stored.
  assert.deepEqual(ran(13, "eng:lint").inputs, []);
  assert.equal(ran(13, "eng:lint").cause, "missed a key run 110 (#11 test-linux) stored");
  assert.equal(pr(13).release, true);

  // A pull request with no log naming its checkout is reported unread, not dropped.
  assert.equal(pr(14).unread, "no job log names the merge it checked out");

  // The totals split forced-alone, forced-together and not-forced, per job.
  assert.deepEqual(result.totals.alone.nonRelease.rustBuild["test-linux"], {
    tasks: 2,
    minutes: 11,
  });
  assert.deepEqual(result.totals.several.nonRelease["several inputs together"]["test-macos"], {
    tasks: 1,
    minutes: 8,
  });
  assert.equal(
    result.totals.unforced.release["missed a key an earlier run stored"]["test-linux"].tasks,
    1,
  );
  // The weights: each task's median executed minutes, less a build it did.
  assert.equal(result.weights.tasks["eng:test"].linux.minutes, 9);
  assert.equal(result.weights.build.linux.minutes, 1);
});

test("the observed replay's Markdown tables the inputs and lists each pull request's tasks", () => {
  const out = join(scratch, "observed.md");
  const run = observedRun(out);
  assert.equal(run.status, 0, run.stderr);
  const md = readFileSync(out, "utf8");
  assert.match(md, /### Ran: forced by one input alone, non-release pull requests/);
  assert.match(md, /\| rustBuild \| - \| 2 \/ 11\.0 \| - \| - \|/);
  assert.match(md, /ran `eng:test` \(8\.0 min\): e2eSource \+ runtime:node scripts\/recipe\.cjs/);
  assert.match(
    md,
    /#### #14 — run 140 — fix: a run whose log is gone\n\nUnread: no job log names the merge it checked out\./,
  );
});

test("--observed without a clone is refused with what to make", () => {
  const run = spawnSync(
    process.execPath,
    [replayScript, "--observed", evidence, "--out", join(scratch, "x")],
    {
      encoding: "utf8",
    },
  );
  assert.equal(run.status, 1);
  assert.match(run.stderr, /--observed needs --clone/);
  assert.match(run.stderr, /^next: git clone/m);
});

test("today's tiers, generated from the workspace and replayed over the observed changes, run what Nx ran", () => {
  const mapping = join(scratch, "tiers.json");
  const gen = spawnSync(
    process.execPath,
    [
      tierScript,
      "--clone",
      repo,
      "--changes",
      observedJson,
      "--out",
      mapping,
      "--commit",
      commits.base,
    ],
    { encoding: "utf8" },
  );
  assert.equal(gen.status, 0, gen.stderr);
  const tiers = JSON.parse(readFileSync(mapping, "utf8"));
  // smoke is never cached, so it is no suite a key could skip.
  assert.deepEqual(Object.keys(tiers.suites).sort(), ["eng:lint", "eng:test"]);
  assert.deepEqual(tiers.suites["eng:test"].crates, ["onepipeline"]);
  assert.ok(tiers.suites["eng:test"].paths.includes("tests/e2e/**/*"));
  assert.ok(tiers.suites["eng:test"].paths.includes("runtime:node scripts/recipe.cjs"));
  const out = join(scratch, "tiers-replay.json");
  const run = spawnSync(
    process.execPath,
    [replayScript, "--mapping", mapping, "--changes", observedJson, "--out", out, "--json"],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
  const result = JSON.parse(readFileSync(out, "utf8"));
  // #11 touched the source: both; #12 a journey and the recipe: test only.
  assert.deepEqual(
    result.perChange.map((change) => [change.commit, change.affected.sort()]),
    [
      ["#11", ["eng:lint", "eng:test"]],
      ["#12", ["eng:test"]],
    ],
  );
  assert.equal(result.summary.platforms.linux.suiteMinutesSkipped, 1);
  // --release replays the release pull requests instead: the changelog moves no key.
  const release = spawnSync(
    process.execPath,
    [
      replayScript,
      "--mapping",
      mapping,
      "--changes",
      observedJson,
      "--release",
      "--out",
      out,
      "--json",
    ],
    { encoding: "utf8" },
  );
  assert.equal(release.status, 0, release.stderr);
  assert.deepEqual(JSON.parse(readFileSync(out, "utf8")).perChange[0].affected, []);
});

// ---- per-platform weights and the build a split adds ------------------------------

test("a split is weighed per platform: skipped suites, the shared build only when every archive suite skips, less what the split adds", () => {
  const dir = join(scratch, "weights");
  const changes = join(dir, "observed.json");
  write(dir, {
    "observed.json": JSON.stringify({
      prs: [
        { pr: 1, title: "docs only", release: false, changed: ["docs/a.md"], moved: [] },
        { pr: 2, title: "a journey", release: false, changed: ["tests/one.rs"], moved: [] },
        { pr: 3, title: "the lock", release: false, changed: [], moved: ["runtime:cargo"] },
      ],
      weights: {
        tasks: {
          "e:one": { linux: { minutes: 10 } },
          "e:two": { linux: { minutes: 20 }, win32: { minutes: 40 } },
        },
        modules: {},
        build: { linux: { minutes: 5 }, win32: { minutes: 7 } },
      },
    }),
    "mapping.json": JSON.stringify({
      shared: ["runtime:cargo"],
      crates: { core: { paths: ["src/**"] } },
      suites: {
        one: { paths: ["tests/one.rs"], crates: ["core"], tasks: ["e:one"], archive: true },
        two: { paths: ["tests/two.rs"], crates: ["core"], tasks: ["e:two"], archive: true },
      },
    }),
  });
  const run = spawnSync(
    process.execPath,
    [
      replayScript,
      "--mapping",
      join(dir, "mapping.json"),
      "--changes",
      changes,
      "--build-delta",
      "linux=1,win32=2",
      "--out",
      join(dir, "out.json"),
      "--json",
    ],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
  assert.match(run.stdout, /net minutes saved per change: linux 17\.7, darwin 0\.0, win32 27\.7$/m);
  const { linux, win32 } = JSON.parse(readFileSync(join(dir, "out.json"), "utf8")).summary
    .platforms;
  // #1 skips both (30 + one build of 5); #2 skips two (20) and still builds (+1 charged).
  assert.deepEqual(
    {
      skipped: linux.suiteMinutesSkipped,
      builds: linux.archiveBuildsSkipped,
      delta: linux.buildDeltaCharged,
      net: linux.netMinutesSaved,
    },
    { skipped: 50, builds: 1, delta: 2, net: 50 + 5 - 2 },
  );
  assert.equal(win32.netMinutesSaved, 40 + 40 + 7 - 4);
  const bad = spawnSync(
    process.execPath,
    [
      replayScript,
      "--mapping",
      join(dir, "mapping.json"),
      "--changes",
      changes,
      "--build-delta",
      "beos=1",
      "--out",
      join(dir, "x"),
    ],
    { encoding: "utf8" },
  );
  assert.equal(bad.status, 1);
  assert.match(bad.stderr, /--build-delta beos=1 is not platform=minutes pairs/);
  const notObserved = spawnSync(
    process.execPath,
    [
      replayScript,
      "--mapping",
      join(dir, "mapping.json"),
      "--changes",
      join(dir, "mapping.json"),
      "--out",
      join(dir, "x"),
    ],
    { encoding: "utf8" },
  );
  assert.equal(notObserved.status, 1);
  assert.match(notObserved.stderr, /is not an --observed replay's JSON/);
});

// ---- the module graph ---------------------------------------------------------------

test("the module graph puts a cycle in one component and leaves the modules outside it their own", () => {
  const crate = join(scratch, "crate");
  write(crate, {
    "src/lib.rs":
      "pub mod a;\npub mod b;\npub mod c;\npub mod d;\npub use d::{Leaf, Other as Renamed};\n",
    "src/main.rs": "fn main() { onepipeline::c::go(); }\n",
    // a and b reach each other, one through a grouped import; c reaches a and the
    // re-exported item of d; d names `crate::a` only in a comment and a string.
    "src/a.rs": "use crate::{b::B, Leaf};\npub struct A;\n",
    "src/b.rs": "pub struct B(crate::a::A);\n",
    "src/c.rs": "pub fn go() { let _ = crate::a::A; let _ = crate::Renamed; }\n",
    "src/d.rs":
      '// crate::a is prose\npub struct Leaf;\npub struct Other;\nconst S: &str = "crate::b";\n',
    "src/d/inner.rs": "pub fn x() {}\n",
  });
  const out = join(scratch, "graph.json");
  const run = spawnSync(process.execPath, [graphScript, "--repo", crate, "--out", out], {
    encoding: "utf8",
  });
  assert.equal(run.status, 0, run.stderr);
  assert.equal(run.stdout.trim(), "module-graph: 4 modules in 3 components; the largest holds 2");
  const result = JSON.parse(readFileSync(out, "utf8"));
  const byModules = Object.fromEntries(
    Object.values(result.crates).map((c) => [c.modules.join("+"), c]),
  );
  assert.deepEqual(Object.keys(byModules).sort(), ["a+b", "c", "d"]);
  assert.deepEqual(byModules.d.deps, []);
  assert.deepEqual(byModules.d.paths, ["src/d.rs", "src/d/**"]);
  assert.equal(byModules.c.deps.length, 2);
});
