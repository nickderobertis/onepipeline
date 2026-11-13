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
const spikeScript = join(root, "scripts", "crate-split", "reuse-spike.mjs");
const cacheScript = join(root, "scripts", "crate-split", "cache-inventory.mjs");

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
} else if (args.at(-1).includes("/actions/caches?")) {
  const page = Number(args.at(-1).match(/[?&]page=(\\d+)/)[1]);
  process.stdout.write(JSON.stringify({ actions_caches: state.caches[page - 1] ?? [] }));
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
  assert.match(run.stderr, /--floor lots is not a whole number/);
  assert.deepEqual(calls, []);
});

test("the cache inventory pages the listing read-only and counts entries by family, scope and Nx target", () => {
  const dir = join(scratch, "caches");
  const entry = (id, key, ref, bytes) => ({
    id,
    key,
    ref,
    size_in_bytes: bytes,
    created_at: `2026-11-0${(id % 9) + 1}T00:00:00Z`,
  });
  const full = Array.from({ length: 100 }, (_, i) =>
    entry(i + 1, `nx-cache-1234567890123${String(i).padStart(3, "0")}`, "refs/heads/main", 10),
  );
  const bin = ghDouble(dir, {
    core: [4000],
    graphql: [4000],
    caches: [
      full,
      [
        entry(100, full[99].key, "refs/heads/main", 10),
        entry(201, "v0-rust-test-linux-Linux-x64-0123abcd-deadbeef", "refs/heads/main", 1000),
        entry(202, "v0-rust-test-linux-Linux-x64-0123abcd-feedface", "refs/pull/7/merge", 2000),
      ],
    ],
  });
  write(dir, {
    "executions/1.json": JSON.stringify({
      tasks: [{ task: "onepipeline-e2e:test", hash: "1234567890123007" }],
    }),
  });
  const run = spawnSync(process.execPath, [cacheScript, "--evidence", dir], {
    encoding: "utf8",
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}` },
  });
  assert.equal(run.status, 0, run.stderr);
  // The entry the second page repeats is one entry.
  assert.match(
    run.stdout,
    /^cache-inventory: 102 entries, 0\.00 GB, 2 pages read; largest families: nx-cache-# @ main 100/,
  );
  const inv = JSON.parse(readFileSync(join(dir, "caches", "inventory.json"), "utf8"));
  assert.equal(inv.families["v0-rust-test-linux-Linux-x64-#-# @ pull request"].bytes, 2000);
  assert.deepEqual(inv.families["nx-cache-# @ main"].targets, {
    "onepipeline-e2e:test": { entries: 1, bytes: 10 },
  });
  // Every call was a read: the rate limit, and GETs of the listing.
  const calls = readFileSync(join(dir, "calls.log"), "utf8")
    .trim()
    .split("\n")
    .map((l) => JSON.parse(l));
  assert.ok(
    calls.every((args) => args[0] === "api" && !args.includes("--method") && !args.includes("-X")),
  );
  // Below the floor it reads no page and says so.
  const low = ghDouble(join(scratch, "caches-low"), { core: [10], graphql: [4000], caches: [] });
  const stopped = spawnSync(
    process.execPath,
    [cacheScript, "--evidence", join(scratch, "caches-low")],
    {
      encoding: "utf8",
      env: { ...process.env, PATH: `${low}:${process.env.PATH}` },
    },
  );
  assert.equal(stopped.status, 2);
  assert.match(stopped.stderr, /10 core remain, below the floor of 2500; 0 pages held/);
});


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
      name: "onepipeline",
      root: ".",
      targets: {
        test: {
          command: "true",
          cache: true,
          inputs: ["e2eSource", "docsSource", { runtime: "node scripts/recipe.cjs" }],
        },
        lint: { command: "true", cache: true, inputs: ["rustBuild"] },
        "test-rest": { command: "true", cache: true, inputs: ["rustBuild"] },
        smoke: { command: "true", cache: false, inputs: ["rustBuild"] },
      },
    }),
    "tests/e2e/project.json": JSON.stringify({
      name: "onepipeline-e2e",
      root: "tests/e2e",
      targets: { test: { command: "true", cache: true, inputs: ["e2eSource"] } },
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
      "##[group]> nx run onepipeline:test",
      "     Starting 3 tests across 1 binary",
      "     Summary [  120.000s] 3 tests run: 3 passed",
      "> nx run onepipeline:lint",
      "> nx run onepipeline:smoke",
      "> nx run onepipeline-e2e:test",
      "> nx run onepipeline:test-rest",
    ]),
    "executions/1101.json": JSON.stringify({
      tasks: [
        {
          task: "onepipeline:test",
          hash: "111",
          cacheable: true,
          result: "executed",
          remote_read: "miss",
          started_at: at(1),
          finished_at: at(11),
        },
        {
          task: "onepipeline:lint",
          hash: "222",
          cacheable: true,
          result: "executed",
          remote_read: "miss",
          started_at: at(11),
          finished_at: at(12),
        },
        {
          task: "onepipeline:smoke",
          hash: "333",
          cacheable: false,
          result: "executed",
          remote_read: "not_attempted",
          started_at: at(12),
          finished_at: at(15),
        },
        {
          task: "onepipeline-e2e:test",
          hash: "666",
          cacheable: true,
          result: "executed",
          remote_read: "miss",
          started_at: at(15),
          finished_at: at(35),
        },
        {
          task: "onepipeline:test-rest",
          hash: "777",
          cacheable: true,
          result: "executed",
          remote_read: "miss",
          started_at: at(35),
          finished_at: at(39),
        },
      ],
      store_attempts: [
        { task: "onepipeline:lint", hash: "999", status: "stored", attempted_at: at(12) },
      ],
    }),
    // #12: a journey and the recipe a runtime input prints both moved; lint replayed.
    "logs/1201.log": log(commits.several, [
      "> nx run onepipeline:test",
      "> nx run onepipeline:lint  [remote cache]",
    ]),
    "executions/1201.json": JSON.stringify({
      tasks: [
        {
          task: "onepipeline:test",
          hash: "444",
          cacheable: true,
          result: "executed",
          remote_read: "miss",
          started_at: at(20),
          finished_at: at(28),
        },
        {
          task: "onepipeline:lint",
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
      "> nx run onepipeline:test  [remote cache]",
      "> nx run onepipeline:lint",
    ]),
    "executions/1301.json": JSON.stringify({
      tasks: [
        {
          task: "onepipeline:test",
          hash: "555",
          cacheable: true,
          result: "remote_hit",
          remote_read: "hit",
          started_at: at(40),
          finished_at: at(40.1),
        },
        {
          task: "onepipeline:lint",
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
  // The newest profile records #11's test-linux job uploaded: two e2e modules, one
  // three times the other, and the rest tier's library unit tests of one module beside
  // an integration binary of its own.
  const golden = JSON.parse(
    readFileSync(join(root, "tests", "golden", "test-profile-record-v2.json"), "utf8"),
  );
  const records = [
    ["e2e", "alpha::one"],
    ["e2e", "beta::one"],
    ["e2e", "beta::two"],
    ["e2e", "beta::three"],
    ["onepipeline", "plan::tests::parses"],
    ["linked_engines", "the_lock_links_one_release"],
  ];
  for (const [i, [binary, name]] of records.entries()) {
    write(evidence, {
      [`profiles/110/test-linux/${i}.json`]: JSON.stringify({ ...golden, binary, test: name }),
    });
  }

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
    `crate-split-replay: wrote ${observedJson}: 3 of 4 pull requests read, 2 task runs replayed and 7 ran; 5 of those the diff forced, 2 it did not`,
  );
  const result = JSON.parse(readFileSync(observedJson, "utf8"));
  const pr = (n) => result.prs.find((p) => p.pr === n);
  const ran = (n, task) => pr(n).jobs[0].ran.find((row) => row.task === task);

  // A source file is rustBuild's, however many named inputs reach it.
  assert.deepEqual(pr(11).changed, ["src/a.rs"]);
  assert.deepEqual(ran(11, "onepipeline:test").inputs, ["rustBuild"]);
  assert.equal(ran(11, "onepipeline:test").cause, "one input");
  assert.equal(ran(11, "onepipeline:test").minutes, 10);
  // The log's nextest lines split the task into its build and its tests.
  assert.equal(ran(11, "onepipeline:test").buildMinutes, 1);
  assert.equal(ran(11, "onepipeline:test").testMinutes, 2);
  assert.deepEqual(ran(11, "onepipeline:lint").inputs, ["rustBuild"]);
  assert.equal(ran(11, "onepipeline:smoke").cause, "not cacheable");

  // A journey file and a runtime input whose command prints differently are two inputs.
  assert.deepEqual(ran(12, "onepipeline:test").inputs, [
    "e2eSource",
    "runtime:node scripts/recipe.cjs",
  ]);
  assert.equal(ran(12, "onepipeline:test").cause, "several inputs");
  assert.deepEqual(pr(12).moved, ["runtime:node scripts/recipe.cjs"]);
  // lint's key holds neither, so it replayed, and that replay is no diff-forced key.
  assert.deepEqual(
    pr(12).jobs[0].replayed.map((row) => [row.task, row.diffMatched]),
    [["onepipeline:lint", false]],
  );

  // Nothing a key holds moved, so the cause is the run's own record: a key run 110 stored.
  assert.deepEqual(ran(13, "onepipeline:lint").inputs, []);
  assert.equal(ran(13, "onepipeline:lint").cause, "missed a key run 110 (#11 test-linux) stored");
  assert.equal(pr(13).release, true);

  // A pull request with no log naming its checkout is reported unread, not dropped.
  assert.equal(pr(14).unread, "no job log names the merge it checked out");

  // The totals split forced-alone, forced-together and not-forced, per job.
  assert.deepEqual(result.totals.alone.nonRelease.rustBuild["test-linux"], {
    tasks: 4,
    minutes: 35,
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
  assert.equal(result.weights.tasks["onepipeline:test"].linux.minutes, 9);
  assert.equal(result.weights.build.linux.minutes, 1);
  // Each e2e module weighs its share of the e2e tier's minutes on its platform, and the
  // rest tier splits into its library's unit tests by module and the rest of it.
  const minutesOf = (name) => result.weights.modules[name].linux.minutes;
  assert.equal(minutesOf("alpha"), 5);
  assert.equal(minutesOf("beta"), 15);
  assert.equal(minutesOf("unit:onepipeline:test-rest:plan"), 2);
  assert.equal(minutesOf("rest:onepipeline:test-rest"), 2);
  assert.equal(result.weights.sources.linux.restUnitRecords, 1);
});

test("the observed replay's Markdown tables the inputs and lists each pull request's tasks", () => {
  const out = join(scratch, "observed.md");
  const run = observedRun(out);
  assert.equal(run.status, 0, run.stderr);
  const md = readFileSync(out, "utf8");
  assert.match(md, /### Ran: forced by one input alone, non-release pull requests/);
  assert.match(md, /\| rustBuild \| - \| 4 \/ 35\.0 \| - \| - \|/);
  assert.match(
    md,
    /ran `onepipeline:test` \(8\.0 min\): e2eSource \+ runtime:node scripts\/recipe\.cjs/,
  );
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
  assert.deepEqual(Object.keys(tiers.suites).sort(), [
    "onepipeline-e2e:test",
    "onepipeline:lint",
    "onepipeline:test",
    "onepipeline:test-rest",
  ]);
  assert.deepEqual(tiers.suites["onepipeline:test"].crates, ["onepipeline"]);
  assert.ok(tiers.suites["onepipeline:test"].paths.includes("tests/e2e/**/*"));
  assert.ok(tiers.suites["onepipeline:test"].paths.includes("runtime:node scripts/recipe.cjs"));
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
      [
        "#11",
        ["onepipeline-e2e:test", "onepipeline:lint", "onepipeline:test", "onepipeline:test-rest"],
      ],
      ["#12", ["onepipeline-e2e:test", "onepipeline:test"]],
    ],
  );
  // #12 skips lint (1 min) and test-rest (4) on Linux.
  assert.equal(result.summary.platforms.linux.suiteMinutesSkipped, 5);
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

test("the reuse spike's one command replays today's tiers and every option over the same pull requests", () => {
  const out = join(scratch, "spike");
  const run = spawnSync(
    process.execPath,
    [
      spikeScript,
      "--evidence",
      evidence,
      "--clone",
      repo,
      "--out",
      out,
      "--build-delta-b",
      "linux=1",
      "--commit",
      commits.base,
    ],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
  // One result line, and each step's own line in steps.txt.
  assert.equal(run.stdout.trim().split("\n").length, 1, run.stdout);
  assert.match(
    run.stdout.trim(),
    /^reuse-spike: wrote \S+ \(each step's line in steps\.txt\): 3 of 4 pull requests read, 7 task runs ran and 5 of them the diff forced; net min per PR over today's tiers \(linux\/macOS\/Windows\): a-all -?\d+\.\d\/-?\d+\.\d\/-?\d+\.\d, b \S+, c \S+; d1 \S+; d2 -\/-\/- \(estimate\)$/,
  );
  const stepsText = readFileSync(join(out, "steps.txt"), "utf8");
  const lines = stepsText.trim().split("\n");
  assert.match(lines[0], /observed\.json: 3 of 4 pull requests read/);
  for (const name of [
    "today",
    "today-release",
    "a-docs-out-of-e2e",
    "a-rest-own-tests",
    "a-nx-json-out",
    "a-all",
    "b",
    "c",
    "d-install-proofs",
  ]) {
    assert.ok(
      lines.some((line) => line.includes(`${join(out, `${name}.md`)}:`)),
      `${name} was not replayed:\n${stepsText}`,
    );
  }
  // (d2) is said to be an estimate, and replays nothing no run here made.
  assert.match(
    stepsText,
    /^reuse-spike: d-install-documented \(estimate, keyed on the run's base\): linux 0 of 0 runs replay, - min per change/m,
  );
  // The report's tables render from what the one command wrote.
  const tables = spawnSync(
    process.execPath,
    [
      join(root, "scripts", "crate-split", "report-tables.mjs"),
      "--spike",
      out,
      "--evidence",
      evidence,
      "--out",
      join(out, "tables.md"),
    ],
    { encoding: "utf8" },
  );
  assert.equal(tables.status, 0, tables.stderr);
  assert.match(tables.stdout, /^report-tables: wrote \S+tables\.md: Population; /);
  const tablesText = readFileSync(join(out, "tables.md"), "utf8");
  for (const section of [
    "Population",
    "Totals per input",
    "The e2e tier",
    "Options replayed",
    "Eviction",
  ]) {
    assert.match(tablesText, new RegExp(`^### ${section}$`, "m"));
  }
  // #11 changed engine source only: its class, its run and its jobs' replays. Without
  // --clone nothing is told apart as run before the caching change.
  assert.match(
    tablesText,
    /^\| #11 \| non-release \| engine source, no tests \| 1 \| \[110\]\(https:\/\/github\.com\/nickderobertis\/onepipeline\/actions\/runs\/110\) \| - \| 1101 \(0 of 5 replayed\) \| - \| - \|$/m,
  );
  assert.match(
    tablesText,
    /^- test-linux \(job 1101\): replayed 0: none\. Ran 5: \[rustBuild\] :test, :lint, e2e:test, :test-rest; \[not cacheable\] :smoke\.$/m,
  );
  // (b): the e2e tier became proposed.json's suites, each weighed by its modules.
  const b = JSON.parse(readFileSync(join(out, "b.mapping.json"), "utf8"));
  const proposed = JSON.parse(
    readFileSync(join(root, "scripts", "crate-split", "proposed.json"), "utf8"),
  );
  assert.ok(!("onepipeline-e2e:test" in b.suites));
  for (const suite of Object.keys(proposed.suites)) assert.equal(b.suites[suite].archive, true);
  // The build delta is charged on each change that still builds the archive: both.
  const bResult = JSON.parse(readFileSync(join(out, "b.json"), "utf8"));
  assert.equal(bResult.summary.platforms.linux.buildDeltaCharged, 2);
  // (c): the rest tier keeps its integration binaries' share and splits its unit tests.
  const c = JSON.parse(readFileSync(join(out, "c.mapping.json"), "utf8"));
  assert.deepEqual(c.suites["onepipeline:test-rest"].modules, ["rest:onepipeline:test-rest"]);
  assert.ok(Object.keys(c.suites).some((name) => name.startsWith("onepipeline:test-rest.")));
});


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


test("the module graph puts a cycle in one component and leaves the modules outside it their own", () => {
  const crate = join(scratch, "crate");
  write(crate, {
    "src/lib.rs":
      "pub mod a;\npub mod b;\npub mod c;\npub mod d;\npub use d::{Leaf, Other as Renamed};\n",
    "src/main.rs": "fn main() { onepipeline::c::go(); }\n",
    // a and b reach each other, one through a grouped import; c reaches a and the
    // re-exported item of d; d names `crate::a` only in a comment and a string, and
    // `crate::b` only inside a nested block comment.
    "src/a.rs": "use crate::{b::B, Leaf};\npub struct A;\n",
    "src/b.rs": "pub struct B(crate::a::A);\n",
    // A char literal holding a quote opens no string: the references after it count.
    "src/c.rs":
      "pub fn go<'a>() { let _q = '\"'; let _ = crate::a::A; let _ = crate::Renamed; }\nconst M: &str = \"m\";\n",
    "src/d.rs":
      '/* outer /* crate::b */ still a comment crate::b */\n// crate::a is prose\npub struct Leaf;\npub struct Other;\nconst S: &str = "crate::b";\n',
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
