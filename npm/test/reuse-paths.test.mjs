// The reuse harness's less travelled paths, each driven through its command: which run
// and which artifact the fetch takes, what it does with an artifact that is gone or
// holds no record, an interrupted unpack, a page of the cache listing it already holds,
// and the refusals the replayer and the demonstration reader give.
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
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const script = (name) => join(root, "scripts", "crate-split", name);
const replay = join(root, "scripts", "crate-split-replay.mjs");

let scratch;
before(() => {
  scratch = mkdtempSync(join(tmpdir(), "reuse-paths-"));
});
after(() => rmSync(scratch, { recursive: true, force: true }));

function write(dir, files) {
  for (const [path, body] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), body);
  }
}

/** A zip holding `files`, made with the system's zip, for the double to serve. */
function zip(dir, name, files) {
  const at = join(dir, `${name}.src`);
  write(at, files);
  execFileSync("zip", ["-q", "-r", join(dir, `${name}.zip`), "."], { cwd: at });
  return join(dir, `${name}.zip`);
}

/**
 * A `gh` that answers from `state.json`: `routes` maps an endpoint's path to a file to
 * print (JSON or a zip), `rollups` a pull request number to its check suites, and every
 * call is appended to calls.log.
 */
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
const say = (body) => process.stdout.write(body);
if (args[1] === "rate_limit") {
  const resource = args[3].match(/resources\\.(\\w+)/)[1];
  const reads = (state.reads ?? {})[resource] ?? 0;
  const seq = state[resource] ?? [4000];
  state.reads = { ...state.reads, [resource]: reads + 1 };
  fs.writeFileSync(dir + "/state.json", JSON.stringify(state));
  say(JSON.stringify({ remaining: seq[Math.min(reads, seq.length - 1)], reset: Math.floor(Date.now() / 1000) - 29 }));
} else if (args[0] === "pr") say(JSON.stringify(state.merged));
else if (args[1] === "graphql") say(JSON.stringify(state.rollups[args.find((a) => a.startsWith("n=")).slice(2)]));
else {
  const path = args.at(-1).replace(/^repos\\/[^/]+\\/[^/]+\\//, "").replace(/\\?.*$/, "");
  const route = state.routes[path];
  if (route === undefined) {
    process.stderr.write("HTTP 404: Not Found (" + path + ")");
    process.exit(1);
  }
  process.stdout.write(fs.readFileSync(route));
}
`,
  );
  chmodSync(join(bin, "gh"), 0o755);
  return bin;
}

const at = (minutes) =>
  new Date(Date.parse("2026-11-01T00:00:00Z") + minutes * 60000).toISOString();
const checkRun = (name, id, from, to) => ({
  name,
  databaseId: id,
  conclusion: "SUCCESS",
  startedAt: at(from),
  completedAt: at(to),
});
const rollup = (oid, suites) => ({
  data: {
    repository: {
      pullRequest: { commits: { nodes: [{ commit: { oid, checkSuites: { nodes: suites } } }] } },
    },
  },
});
const suite = (run, created, name, runs) => ({
  workflowRun: { databaseId: run, workflow: { name }, createdAt: at(created) },
  conclusion: "SUCCESS",
  checkRuns: { nodes: runs },
});
const artifact = (id, name, created, expired = false) => ({
  id,
  name,
  expired,
  created_at: at(created),
});

test("the fetch takes each pull request's newest CI run, and each job's own attempt's record", () => {
  const dir = join(scratch, "fetch");
  mkdirSync(dir, { recursive: true });
  const record = (job) => ({
    "executions.json": JSON.stringify({
      tasks: [
        {
          task: "p:test",
          hash: job === "first" ? "1" : "2",
          cacheable: true,
          result: "executed",
          started_at: at(1),
          finished_at: at(2),
        },
      ],
      store_attempts: [],
    }),
  });
  const profile = zip(dir, "profile", { "e2e/one.json": "{}" });
  write(dir, {
    "list-7.json": JSON.stringify({
      artifacts: [
        // The first attempt's record, uploaded before the re-run attempt started.
        artifact(70, "nx-cache-entries-test-linux", 5),
        artifact(71, "nx-cache-entries-test-linux", 25),
        artifact(72, "nx-cache-entries-test-macos", 25, true),
        artifact(73, "nx-cache-entries-lint", 25),
      ],
    }),
    "list-9.json": JSON.stringify({ artifacts: [artifact(90, "test-profile-test-linux", 1)] }),
    "log.txt": "2026-11-01T00:00:00.0000000Z a log\n",
  });
  const routes = {
    "actions/runs/7/artifacts": join(dir, "list-7.json"),
    "actions/runs/9/artifacts": join(dir, "list-9.json"),
    "actions/artifacts/70/zip": zip(dir, "first", record("first")),
    "actions/artifacts/71/zip": zip(dir, "second", record("second")),
    "actions/artifacts/73/zip": zip(dir, "empty", { "manifest.json": "{}" }),
    "actions/artifacts/90/zip": profile,
  };
  for (const id of [701, 702, 703]) routes[`actions/jobs/${id}/logs`] = join(dir, "log.txt");
  const bin = ghDouble(dir, {
    merged: [
      { number: 2, title: "feat: two", mergedAt: at(60) },
      { number: 1, title: "fix: one, with no CI run", mergedAt: at(50) },
    ],
    rollups: {
      // An earlier CI run and the newest, re-run attempt of its test-linux job.
      2: rollup("b".repeat(40), [
        suite(6, 0, "CI", [checkRun("test-linux", 601, 0, 10)]),
        suite(7, 2, "CI", [
          checkRun("test-linux", 701, 20, 30),
          checkRun("test-macos", 702, 20, 30),
          checkRun("lint", 703, 20, 30),
        ]),
        suite(8, 3, "Visual docs", [checkRun("test-linux", 801, 20, 30)]),
      ]),
      1: rollup("a".repeat(40), [suite(5, 0, "Visual docs", [])]),
    },
    routes,
  });
  const out = join(dir, "out");
  // A run left this unpack half done: it is unpacked again, not taken as whole.
  write(out, { "profiles/9/test-linux.partial/stale.json": "{}" });
  const run = spawnSync(
    process.execPath,
    [script("fetch-evidence.mjs"), "--out", out, "--prs", "2", "--profiles", "9"],
    {
      encoding: "utf8",
      env: { ...process.env, PATH: `${bin}:${process.env.PATH}` },
    },
  );
  assert.equal(run.status, 0, run.stderr);
  // The newest CI run's jobs, and nothing of the earlier run or of another workflow.
  assert.ok(existsSync(join(out, "logs", "701.log")));
  assert.ok(!existsSync(join(out, "logs", "601.log")));
  assert.ok(!existsSync(join(out, "logs", "801.log")));
  // The record of the attempt the check run is, not the earlier attempt's.
  assert.equal(
    JSON.parse(readFileSync(join(out, "executions", "701.json"), "utf8")).tasks[0].hash,
    "2",
  );
  assert.deepEqual(JSON.parse(readFileSync(join(out, "executions", "702.json"), "utf8")), {
    absent: "expired",
  });
  assert.deepEqual(JSON.parse(readFileSync(join(out, "executions", "703.json"), "utf8")), {
    absent: "no executions.json in the artifact",
  });
  assert.ok(existsSync(join(out, "profiles", "9", "test-linux", "e2e", "one.json")));
  assert.ok(!existsSync(join(out, "profiles", "9", "test-linux.partial")));
  // #1 has no CI run, so no listing of it was read.
  const calls = readFileSync(join(dir, "calls.log"), "utf8");
  assert.doesNotMatch(calls, /actions\/runs\/5\//);
});

test("a read GitHub refuses stops the fetch with what to check, and keeps what it fetched", () => {
  const dir = join(scratch, "fetch-refused");
  const bin = ghDouble(dir, {
    merged: [{ number: 3, title: "feat: three", mergedAt: at(1) }],
    rollups: {
      3: rollup("c".repeat(40), [suite(31, 0, "CI", [checkRun("test-linux", 3101, 0, 10)])]),
    },
    routes: {},
  });
  const out = join(dir, "out");
  const run = spawnSync(
    process.execPath,
    [script("fetch-evidence.mjs"), "--out", out, "--prs", "1"],
    {
      encoding: "utf8",
      env: { ...process.env, PATH: `${bin}:${process.env.PATH}` },
    },
  );
  assert.equal(run.status, 1);
  assert.match(
    run.stderr,
    /gh api repos\/nickderobertis\/onepipeline\/actions\/runs\/31\/artifacts\?per_page=100 failed: HTTP 404/,
  );
  assert.match(
    run.stderr,
    /^next: check `gh auth status` and the network, then re-run; what was fetched is kept$/m,
  );
  assert.ok(existsSync(join(out, "rollup", "3.json")));
  assert.ok(!existsSync(join(out, "rollup", "3.json.partial")));
});

test("--profiles that is not a run id is refused before anything is read", () => {
  const dir = join(scratch, "fetch-bad-run");
  const bin = ghDouble(dir, { routes: {} });
  const run = spawnSync(
    process.execPath,
    [script("fetch-evidence.mjs"), "--out", join(dir, "out"), "--profiles", "../x"],
    {
      encoding: "utf8",
      env: { ...process.env, PATH: `${bin}:${process.env.PATH}` },
    },
  );
  assert.equal(run.status, 1);
  assert.match(run.stderr, /--profiles \.\.\/x is not a run id/);
  assert.ok(!existsSync(join(dir, "calls.log")));
});

test("the cache inventory reads no page it holds, and waits out a floor with --wait", () => {
  const dir = join(scratch, "inventory");
  const page = {
    actions_caches: [
      {
        id: 1,
        key: "nx-cache-123456789012",
        ref: "refs/heads/main",
        size_in_bytes: 5,
        created_at: at(1),
      },
    ],
  };
  write(dir, { "caches/page-1.json": JSON.stringify(page) });
  const held = spawnSync(process.execPath, [script("cache-inventory.mjs"), "--evidence", dir], {
    encoding: "utf8",
    env: { ...process.env, PATH: `${ghDouble(dir, { routes: {} })}:${process.env.PATH}` },
  });
  assert.equal(held.status, 0, held.stderr);
  assert.match(held.stdout, /^cache-inventory: 1 entries, 0\.00 GB, 0 pages read;/);
  assert.ok(!existsSync(join(dir, "calls.log")), "a held page is not read again");
  const fresh = join(scratch, "inventory-wait");
  write(fresh, { "page.json": JSON.stringify(page) });
  const bin = ghDouble(fresh, {
    core: [10, 4000],
    routes: { "actions/caches": join(fresh, "page.json") },
  });
  const waited = spawnSync(
    process.execPath,
    [script("cache-inventory.mjs"), "--evidence", fresh, "--wait"],
    {
      encoding: "utf8",
      env: { ...process.env, PATH: `${bin}:${process.env.PATH}` },
    },
  );
  assert.equal(waited.status, 0, waited.stderr);
  assert.match(waited.stdout, /1 pages read/);
});

test("the replayer refuses a suite's archive flag, an unreadable --changes and a malformed build delta", () => {
  const dir = join(scratch, "replayer");
  write(dir, {
    "bad-archive.json": JSON.stringify({
      crates: { c: { paths: ["src/**"] } },
      suites: { s: { paths: [], crates: ["c"], archive: "yes" } },
    }),
    "good.json": JSON.stringify({
      crates: { c: { paths: ["src/**"] } },
      suites: { s: { paths: [], crates: ["c"] } },
    }),
    "observed.json": JSON.stringify({ prs: [], weights: { tasks: {}, modules: {} } }),
    "broken.json": "{",
  });
  const call = (...args) =>
    spawnSync(process.execPath, [replay, ...args, "--out", join(dir, "x")], { encoding: "utf8" });
  let run = call(
    "--mapping",
    join(dir, "bad-archive.json"),
    "--changes",
    join(dir, "observed.json"),
  );
  assert.equal(run.status, 1);
  assert.match(run.stderr, /suites\.s\.archive is not true or false/);
  run = call("--mapping", join(dir, "good.json"), "--changes", join(dir, "broken.json"));
  assert.equal(run.status, 1);
  assert.match(run.stderr, /--changes \S+broken\.json cannot be read as JSON/);
  assert.match(
    run.stderr,
    /^next: give --changes the file an --observed replay wrote with --json$/m,
  );
  for (const delta of ["linux=1=2", "linux=abc", "linux=1,linux=2", "linux="]) {
    run = call(
      "--mapping",
      join(dir, "good.json"),
      "--changes",
      join(dir, "observed.json"),
      "--build-delta",
      delta,
    );
    assert.equal(run.status, 1, delta);
    assert.match(run.stderr, /is not platform=minutes pairs/, delta);
  }
});

test("a demonstration's log reads an exact Rust cache hit in either of its spellings", () => {
  const dir = join(scratch, "exact");
  const log = (line) =>
    `2026-11-13T07:42:49.0000000Z ##[group]Run Swatinem/rust-cache@v2.9.2\n2026-11-13T07:42:50.0000000Z ${line}\n2026-11-13T07:43:00.0000000Z ##[group]Run actions/setup-node@v4\n`;
  write(dir, {
    "5.jobs.json": JSON.stringify({
      jobs: [
        { name: "a", id: 1, status: "completed", steps: [] },
        { name: "b", id: 2, status: "completed", steps: [] },
      ],
    }),
    "1.log": log("Cache hit for: v0-rust-test-linux-x"),
    "2.log": log("Cache restored from key: v0-rust-test-macos-y"),
  });
  const run = spawnSync(
    process.execPath,
    [script("demo-runs.mjs"), "--dir", dir, "--out", join(dir, "t.md"), "5"],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
  const table = readFileSync(join(dir, "t.md"), "utf8");
  assert.match(
    table,
    /\| a \| 1 \| .* \| v0-rust-test-linux-x \| - \| 11 \| 0 \| no measurement \|/,
  );
  assert.match(
    table,
    /\| b \| 2 \| .* \| v0-rust-test-macos-y \| - \| 11 \| 0 \| no measurement \|/,
  );
  // With no run named, nothing is read and the usage is said.
  const none = spawnSync(
    process.execPath,
    [script("demo-runs.mjs"), "--dir", dir, "--out", join(dir, "t.md")],
    { encoding: "utf8" },
  );
  assert.equal(none.status, 1);
  assert.match(none.stderr, /name at least one run/);
});

const gitEnv = {
  ...process.env,
  GIT_AUTHOR_NAME: "reuse",
  GIT_AUTHOR_EMAIL: "reuse@example.invalid",
  GIT_COMMITTER_NAME: "reuse",
  GIT_COMMITTER_EMAIL: "reuse@example.invalid",
  GIT_AUTHOR_DATE: "2026-11-01T00:00:00Z",
  GIT_COMMITTER_DATE: "2026-11-01T00:00:00Z",
  GIT_CONFIG_GLOBAL: "/dev/null",
};
const git = (repo, args) =>
  execFileSync("git", ["-C", repo, ...args], { encoding: "utf8", env: gitEnv }).trim();

/** A small real Nx workspace in a git repository, and a branch per change. */
function workspace(dir) {
  mkdirSync(dir, { recursive: true });
  git(dir, ["init", "-q", "--initial-branch=main"]);
  const project = (extra = {}) =>
    JSON.stringify({
      name: "p",
      root: ".",
      targets: {
        test: {
          command: "true",
          cache: true,
          inputs: ["rustBuild", { runtime: "node scripts/probe.cjs" }],
        },
        lint: { command: "true", cache: true, inputs: ["rustBuild"] },
        "install-documented": { command: "true", cache: false, inputs: ["rustBuild"] },
        ...extra,
      },
    });
  write(dir, {
    ".gitignore": "node_modules\n.nx\n",
    "package.json": JSON.stringify({ name: "ws", private: true }),
    "nx.json": JSON.stringify({ namedInputs: { rustBuild: ["{workspaceRoot}/src/**/*"] } }),
    "project.json": project(),
    "scripts/probe.cjs": "console.log('ok')\n",
    "src/a.rs": "a\n",
    "src/b.rs": "b\n",
    "CHANGELOG.md": "0.1\n",
  });
  git(dir, ["add", "-A"]);
  git(dir, ["commit", "-qm", "seed"]);
  const base = git(dir, ["rev-parse", "HEAD"]);
  const branch = (name, change, from = base) => {
    git(dir, ["checkout", "-q", "-b", name, from]);
    change();
    git(dir, ["add", "-A"]);
    git(dir, ["commit", "-qm", name]);
    const head = git(dir, ["rev-parse", "HEAD"]);
    git(dir, ["checkout", "-q", "main"]);
    return head;
  };
  const heads = {
    base,
    deleted: branch("deleted", () => rmSync(join(dir, "src", "b.rs"))),
    probe: branch("probe", () => write(dir, { "scripts/probe.cjs": "process.exit(3)\n" })),
    newTask: branch("new-task", () =>
      write(dir, {
        "project.json": project({
          newtask: { command: "true", cache: true, inputs: ["rustBuild"] },
        }),
      }),
    ),
    changelog: branch("changelog", () => write(dir, { "CHANGELOG.md": "0.2\n" })),
    source: branch("source", () => write(dir, { "src/a.rs": "a-source\n" })),
    conflicting: branch("conflicting", () => write(dir, { "src/a.rs": "a-head\n" })),
  };
  heads.moved = branch("moved-main", () => write(dir, { "src/a.rs": "a-main\n" }));
  return heads;
}

test("the observed replay reads each kind of moved input, each miss's cause and each fallback", async () => {
  const repo = join(scratch, "ws");
  const h = workspace(repo);
  const evidence = join(scratch, "observed-evidence");
  const lines = (head, base, tasks) =>
    [
      `${at(0)} HEAD is now at 1234567 Merge ${head} into ${base}`,
      ...tasks.map((t, i) => `${at(i + 1)} ${t}`),
    ].join("\n");
  const task = (name, extra) => ({
    task: name,
    hash: "1",
    cacheable: true,
    result: "executed",
    remote_read: "miss",
    started_at: at(1),
    finished_at: at(3),
    ...extra,
  });
  const pr = (n, head, jobs, runId, release = false) => {
    write(evidence, {
      [`rollup/${n}.json`]: JSON.stringify(
        rollup(head, [
          suite(
            runId,
            0,
            "CI",
            jobs.map(([name, id]) => checkRun(name, id, 0, 60)),
          ),
        ]),
      ),
    });
    return {
      number: n,
      title: `${release ? "chore: release" : "feat"} ${n}`,
      mergedAt: at(n),
      release,
    };
  };
  const prs = [
    pr(21, h.deleted, [["test-linux", 2101]], 210),
    pr(22, h.probe, [["test-linux", 2201]], 220),
    pr(23, h.newTask, [["test-linux", 2301]], 230),
    pr(24, h.conflicting, [["test-linux", 2401]], 240),
    pr(
      26,
      h.changelog,
      [
        ["test-linux", 2601],
        ["test-macos", 2602],
      ],
      260,
    ),
    pr(27, h.source, [["test-linux", 2701]], 270),
    pr(28, h.changelog, [["test-linux", 2801]], 280, true),
  ];
  write(evidence, {
    "population.json": JSON.stringify({
      window_from: at(0),
      non_release: prs.filter((p) => !p.release).map(({ release, ...p }) => p),
      release: prs.filter((p) => p.release).map(({ release, ...p }) => p),
    }),
    "logs/2101.log": lines(h.deleted, h.base, ["> nx run p:test"]),
    "executions/2101.json": JSON.stringify({ tasks: [task("p:test")], store_attempts: [] }),
    "logs/2201.log": lines(h.probe, h.base, ["> nx run p:test", "> nx run p:lint  [remote cache]"]),
    "executions/2201.json": JSON.stringify({
      tasks: [task("p:test"), task("p:lint", { result: "remote_hit", remote_read: "hit" })],
      store_attempts: [],
    }),
    "logs/2301.log": lines(h.newTask, h.base, ["> nx run p:newtask", "> nx run p:lint"]),
    "executions/2301.json": JSON.stringify({
      tasks: [task("p:newtask"), task("p:lint")],
      store_attempts: [],
    }),
    "logs/2401.log": lines(h.conflicting, h.moved, ["> nx run p:test"]),
    // #26: nothing a key holds moved; each task's own record says why it ran anyway.
    "logs/2601.log": lines(h.changelog, h.base, [
      "> nx run p:test",
      "> nx run p:lint",
      "> nx run p:install-documented",
      "> nx run p:newtask  [local cache]",
    ]),
    "executions/2601.json": JSON.stringify({
      tasks: [
        task("p:test", { remote_read: "not_attempted" }),
        task("p:lint", { remote_read: undefined }),
        task("p:install-documented", { cacheable: false }),
        task("p:newtask", { result: "local_hit" }),
      ],
      store_attempts: [],
    }),
    // #26's macOS log is gone; its record still lists what ran.
    "executions/2602.json": JSON.stringify({
      tasks: [task("p:test", { remote_read: "failed" }), task("p:lint")],
      store_attempts: [],
    }),
    "logs/2701.log": lines(h.source, h.base, [
      "> nx run p:test  [remote cache]",
      "> nx run p:install-documented",
    ]),
    "executions/2701.json": JSON.stringify({
      tasks: [
        task("p:test", { result: "remote_hit", remote_read: "hit" }),
        task("p:install-documented", { cacheable: false }),
      ],
      store_attempts: [],
    }),
    "logs/2801.log": lines(h.changelog, h.base, ["> nx run p:install-documented"]),
    "executions/2801.json": JSON.stringify({
      tasks: [task("p:install-documented", { cacheable: false })],
      store_attempts: [],
    }),
  });
  const out = join(scratch, "observed.json");
  const observe = () =>
    spawnSync(
      process.execPath,
      [replay, "--observed", evidence, "--clone", repo, "--out", out, "--json"],
      { encoding: "utf8" },
    );
  const run = observe();
  assert.equal(run.status, 0, run.stderr);
  const result = JSON.parse(readFileSync(out, "utf8"));
  const prOf = (n) => result.prs.find((p) => p.pr === n);
  const row = (n, job, name) =>
    prOf(n)
      .jobs.find((j) => j.job === job)
      .ran.find((r) => r.task === name);
  // A deleted keyed file is in its base's plan, so it still counts.
  assert.deepEqual(row(21, "test-linux", "p:test").inputs, ["rustBuild"]);
  // A runtime input that fails moved, whatever it printed before.
  assert.deepEqual(row(22, "test-linux", "p:test").inputs, ["runtime:node scripts/probe.cjs"]);
  assert.deepEqual(prOf(22).moved, ["runtime:node scripts/probe.cjs"]);
  // A task the base had no plan of is new; the others' configuration moved.
  assert.deepEqual(row(23, "test-linux", "p:newtask").inputs, ["task new at this pull request"]);
  assert.deepEqual(row(23, "test-linux", "p:lint").inputs, ["project configuration"]);
  assert.ok(prOf(23).moved.includes("config:p"));
  assert.match(prOf(24).unread, /does not merge cleanly/);
  // Each unforced task's cause is its own record's.
  assert.equal(row(26, "test-linux", "p:test").cause, "no remote read");
  assert.equal(
    row(26, "test-linux", "p:lint").cause,
    "an executions record too old to say whether it read the remote cache",
  );
  assert.equal(row(26, "test-linux", "p:install-documented").cause, "not cacheable");
  assert.deepEqual(
    prOf(26)
      .jobs.find((j) => j.job === "test-linux")
      .replayed.map((r) => [r.task, r.log]),
    [["p:newtask", "local cache"]],
  );
  const mac = prOf(26).jobs.find((j) => j.job === "test-macos");
  assert.equal(mac.unread, "log not fetched");
  assert.deepEqual(
    mac.ran.map((r) => [r.task, r.cause]),
    [
      ["p:test", "remote read failed"],
      ["p:lint", "absent: no run in the evidence stored this key"],
    ],
  );
  // A replay of a key the diff moved came from the pull request's own earlier run.
  assert.deepEqual(
    prOf(27).jobs[0].replayed.map((r) => [r.task, r.diffMatched]),
    [["p:test", true]],
  );
  // A held plan is read again only when a task it lacks is asked for.
  const plan = join(evidence, "plans", `${h.base}.json`);
  const before = readFileSync(plan, "utf8");
  assert.equal(observe().status, 0);
  assert.equal(readFileSync(plan, "utf8"), before);
  const held = JSON.parse(before);
  delete held.plans["p:lint"];
  writeFileSync(plan, JSON.stringify(held));
  assert.equal(observe().status, 0);
  assert.ok(JSON.parse(readFileSync(plan, "utf8")).plans["p:lint"], "the plan was read again");
  // A plan that is not one is refused with what to do.
  writeFileSync(plan, JSON.stringify({ plans: [] }));
  const refused = observe();
  assert.equal(refused.status, 1);
  assert.match(refused.stderr, /is not a hash plan/);
  assert.match(
    refused.stderr,
    /^next: remove \S+; the next replay reads the commit's plan again$/m,
  );
  rmSync(plan);

  // The documented install keyed on its base: #27 replays #26's run on Linux, macOS ran
  // it nowhere, and the release pull request counts only among the releases.
  const { sameBase } = await import(pathToFileURL(script("observed.mjs")).href);
  result.weights.tasks["p:install-documented"] = { linux: { minutes: 6 } };
  const nonRelease = sameBase(result, "p:install-documented");
  assert.deepEqual(nonRelease.linux, { runs: 2, replays: 1, minutesPerChange: 3 });
  assert.deepEqual(nonRelease.darwin, { runs: 0, replays: 0, minutesPerChange: null });
  assert.deepEqual(sameBase(result, "p:install-documented", true).linux, {
    runs: 1,
    replays: 1,
    minutesPerChange: 6,
  });
});

test("a lockfile change moves every key that holds external dependencies, as Nx's default keys do, and no other", () => {
  const repo = join(scratch, "ws-external");
  mkdirSync(repo, { recursive: true });
  git(repo, ["init", "-q", "--initial-branch=main"]);
  const lock = (v) => ({
    "package.json": JSON.stringify({ name: "ws", private: true, dependencies: { "left-pad": v } }),
    "package-lock.json": JSON.stringify({
      name: "ws",
      lockfileVersion: 3,
      requires: true,
      packages: {
        "": { name: "ws", dependencies: { "left-pad": v } },
        "node_modules/left-pad": {
          version: v,
          resolved: `https://registry.npmjs.org/left-pad/-/left-pad-${v}.tgz`,
          integrity: "sha512-x",
        },
      },
    }),
  });
  write(repo, {
    ".gitignore": "node_modules\n.nx\n",
    "nx.json": JSON.stringify({ namedInputs: {} }),
    "project.json": JSON.stringify({
      name: "p",
      root: ".",
      targets: {
        uses: { command: "true", cache: true, inputs: [{ externalDependencies: ["left-pad"] }] },
        // Nx hashes every external dependency into a key that names none.
        plain: { command: "true", cache: true, inputs: ["{workspaceRoot}/src/**/*"] },
        none: {
          command: "true",
          cache: true,
          inputs: ["{workspaceRoot}/src/**/*", { externalDependencies: [] }],
        },
      },
    }),
    "src/a.rs": "a\n",
    ...lock("1.2.0"),
  });
  git(repo, ["add", "-A"]);
  git(repo, ["commit", "-qm", "seed"]);
  const base = git(repo, ["rev-parse", "HEAD"]);
  git(repo, ["checkout", "-q", "-b", "bump"]);
  write(repo, lock("1.3.0"));
  git(repo, ["add", "-A"]);
  git(repo, ["commit", "-qm", "bump"]);
  const head = git(repo, ["rev-parse", "HEAD"]);
  git(repo, ["checkout", "-q", "main"]);
  const evidence = join(scratch, "external-evidence");
  write(evidence, {
    "population.json": JSON.stringify({
      window_from: at(0),
      non_release: [{ number: 31, title: "fix(deps): bump", mergedAt: at(1) }],
      release: [],
    }),
    "rollup/31.json": JSON.stringify(
      rollup(head, [suite(310, 0, "CI", [checkRun("test-linux", 3101, 0, 10)])]),
    ),
    "logs/3101.log": [
      `${at(0)} HEAD is now at 1234567 Merge ${head} into ${base}`,
      `${at(1)} > nx run p:uses`,
      `${at(2)} > nx run p:plain`,
      `${at(3)} > nx run p:none`,
    ].join("\n"),
    "executions/3101.json": JSON.stringify({
      tasks: ["p:uses", "p:plain", "p:none"].map((t) => ({
        task: t,
        hash: "5",
        cacheable: true,
        result: "executed",
        remote_read: "miss",
        started_at: at(1),
        finished_at: at(2),
      })),
      store_attempts: [],
    }),
  });
  const out = join(scratch, "external.json");
  const run = spawnSync(
    process.execPath,
    [replay, "--observed", evidence, "--clone", repo, "--out", out, "--json"],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
  const result = JSON.parse(readFileSync(out, "utf8"));
  const ran = Object.fromEntries(result.prs[0].jobs[0].ran.map((r) => [r.task, r.inputs]));
  assert.deepEqual(ran["p:uses"], ["external dependencies"]);
  assert.deepEqual(ran["p:plain"], ["external dependencies"]);
  assert.ok(!ran["p:none"].includes("external dependencies"), JSON.stringify(ran["p:none"]));
  assert.ok(result.prs[0].moved.includes("external"));
});

test("each option (a) variant removes exactly the inputs it names, and only from the tiers it names", async () => {
  const { VARIANTS } = await import(pathToFileURL(script("tier-mapping.mjs")).href);
  const paths = [
    "src/**/*",
    "docs/**/*",
    "nx.json",
    "scripts/**/*",
    ".github/**/*",
    "tests/*.rs",
    "justfile",
    "runtime:node scripts/nx-version-free-inputs.mjs --ecosystem python",
  ];
  const removed = (variant, task) =>
    paths.filter((p) => !VARIANTS[variant](task, paths).includes(p));
  assert.deepEqual(removed("docs-out-of-e2e", "onepipeline-e2e:test"), ["docs/**/*"]);
  assert.deepEqual(removed("docs-out-of-e2e", "onepipeline-contract:test-quick"), ["docs/**/*"]);
  assert.deepEqual(removed("docs-out-of-e2e", "onepipeline:test-rest"), []);
  assert.deepEqual(removed("rest-own-tests", "onepipeline:test-rest"), [
    "docs/**/*",
    "scripts/**/*",
    ".github/**/*",
    "justfile",
    "runtime:node scripts/nx-version-free-inputs.mjs --ecosystem python",
  ]);
  assert.deepEqual(removed("rest-own-tests", "onepipeline-e2e:test"), []);
  assert.deepEqual(removed("nx-json-out", "onepipeline-npm:test"), ["nx.json"]);
});

test("the tier mapping finds a tier's archive recipe through the justfile, and refuses one just cannot read", () => {
  const repo = join(scratch, "ws");
  const branch = git(repo, ["rev-parse", "deleted"]);
  // Today's tiers at a commit whose justfile runs `test` from the archive and `lint` not.
  git(repo, ["checkout", "-q", "-b", "with-justfile", branch]);
  const project = JSON.parse(readFileSync(join(repo, "project.json"), "utf8"));
  project.targets.test.command = "just _p-test";
  project.targets.lint.command = "just _p-lint";
  write(repo, {
    "project.json": JSON.stringify(project),
    justfile: "_tier-test:\n    true\n_p-test: _tier-test\n_p-lint:\n    true\n",
  });
  git(repo, ["add", "-A"]);
  git(repo, ["commit", "-qm", "justfile"]);
  const good = git(repo, ["rev-parse", "HEAD"]);
  write(repo, { justfile: "_p-test: _nowhere\n" });
  git(repo, ["commit", "-qam", "a justfile just refuses"]);
  const bad = git(repo, ["rev-parse", "HEAD"]);
  git(repo, ["checkout", "-q", "main"]);
  const changes = join(scratch, "observed.json");
  const map = (commit) =>
    spawnSync(
      process.execPath,
      [
        script("tier-mapping.mjs"),
        "--clone",
        repo,
        "--changes",
        changes,
        "--out",
        join(scratch, "tiers.json"),
        "--commit",
        commit,
      ],
      {
        encoding: "utf8",
      },
    );
  let run = map(good);
  assert.equal(run.status, 0, run.stderr);
  const tiers = JSON.parse(readFileSync(join(scratch, "tiers.json"), "utf8"));
  assert.equal(tiers.suites["p:test"].archive, true);
  assert.equal(tiers.suites["p:lint"].archive, false);
  run = map(bad);
  assert.equal(run.status, 1);
  assert.match(run.stderr, /just cannot read \S+'s justfile/);
  assert.match(run.stderr, /^next: install the just the repository pins/m);
});

test("the module graph reads past raw and escaped strings and through every kind of re-export", () => {
  const crate = join(scratch, "graph-crate");
  write(crate, {
    "src/lib.rs":
      "pub mod a;\npub mod b;\npub mod c;\npub mod d;\npub mod e;\npub use d::Single;\npub use e::Inner as Aliased;\n",
    "src/main.rs": "fn main() {}\n",
    // Every crate:: below a, b's raw and escaped strings is text, not a reference.
    "src/a.rs":
      'pub const R: &str = r#"crate::b "quoted" crate::c"#;\npub const E: &str = "say \\"crate::c\\" here";\npub fn f() { let _ = crate::Single; }\n',
    "src/b.rs": "pub fn g() { let _ = crate::Aliased; }\n",
    "src/c.rs": "pub fn h() { crate::a::f(); }\n",
    "src/d.rs": "pub struct Single;\n",
    "src/e.rs": "pub struct Inner;\n",
  });
  const out = join(scratch, "graph.json");
  const run = spawnSync(
    process.execPath,
    [script("module-graph.mjs"), "--repo", crate, "--out", out],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
  const result = JSON.parse(readFileSync(out, "utf8"));
  const depsOf = (m) =>
    Object.values(result.crates)
      .find((c) => c.modules.includes(m))
      .deps.map((d) => result.crates[d].modules.join("+"));
  assert.deepEqual(depsOf("a"), ["d"]);
  assert.deepEqual(depsOf("b"), ["e"]);
  assert.deepEqual(depsOf("c"), ["a"]);
  assert.equal(result.components, 5);
});

test("option (c)'s unit suites run on a change to their crate or one it depends on, and on nothing else", () => {
  const dir = join(scratch, "option-c");
  const crate = join(scratch, "graph-crate");
  write(dir, {
    "base.json": JSON.stringify({
      sources: ["src/**"],
      fallback: "onepipeline",
      crates: { onepipeline: { paths: ["src/**"] } },
      suites: {
        "onepipeline:test-rest": {
          paths: ["tests/*.rs"],
          crates: ["onepipeline"],
          tasks: ["onepipeline:test-rest"],
          archive: true,
        },
      },
    }),
    "observed.json": JSON.stringify({
      prs: [
        { pr: 1, title: "d", release: false, changed: ["src/d.rs"], moved: [] },
        { pr: 2, title: "c", release: false, changed: ["src/c.rs"], moved: [] },
        { pr: 3, title: "a test", release: false, changed: ["tests/x.rs"], moved: [] },
      ],
      weights: { tasks: {}, modules: {} },
    }),
  });
  const mapping = join(dir, "c.json");
  let run = spawnSync(
    process.execPath,
    [
      script("module-graph.mjs"),
      "--repo",
      crate,
      "--out",
      join(dir, "g.json"),
      "--mapping",
      mapping,
      "--base",
      join(dir, "base.json"),
    ],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
  run = spawnSync(
    process.execPath,
    [
      replay,
      "--mapping",
      mapping,
      "--changes",
      join(dir, "observed.json"),
      "--out",
      join(dir, "r.json"),
      "--json",
    ],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
  const affected = Object.fromEntries(
    JSON.parse(readFileSync(join(dir, "r.json"), "utf8")).perChange.map((c) => [
      c.commit,
      c.affected
        .filter((s) => s.includes("."))
        .map((s) => s.split(".m-")[1])
        .sort(),
    ]),
  );
  // d is below a, and a below c: a change to d runs d's, a's and c's unit suites.
  assert.deepEqual(affected["#1"], ["a", "c", "d"]);
  assert.deepEqual(affected["#2"], ["c"]);
  // A test file is the tier's own, not any crate's.
  assert.deepEqual(affected["#3"], []);
  // A base the replayer would refuse is refused here too.
  write(dir, { "bad.json": JSON.stringify({ crates: {}, suites: {} }) });
  run = spawnSync(
    process.execPath,
    [
      script("module-graph.mjs"),
      "--repo",
      crate,
      "--out",
      join(dir, "g.json"),
      "--mapping",
      mapping,
      "--base",
      join(dir, "bad.json"),
    ],
    {
      encoding: "utf8",
    },
  );
  assert.equal(run.status, 1);
  assert.match(run.stderr, /the mapping names no crate/);
});

test("prototype (c) splits a crate along its module graph into crates cargo builds, unit tests and all", () => {
  const crate = join(scratch, "proto-c");
  write(crate, {
    "Cargo.toml":
      '[workspace]\nmembers = ["."]\n\n[package]\nname = "onepipeline"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n\n[dev-dependencies]\n',
    "src/lib.rs":
      "pub mod leaf;\npub mod core_a;\npub mod core_b;\npub mod top;\npub use leaf::Leaf;\n",
    "src/main.rs": "fn main() { onepipeline::top::run(); }\n",
    // leaf is below the cycle, core_a and core_b are the cycle, top is above it.
    "src/leaf.rs": "pub struct Leaf;\n#[cfg(test)]\npub const MEASURING: bool = true;\n",
    "src/core_a.rs":
      "pub(crate) fn a() -> i32 { crate::core_b::b() + 1 }\npub fn from_leaf() -> crate::Leaf { crate::leaf::Leaf }\n",
    "src/core_b.rs":
      "pub fn b() -> i32 { 1 }\npub fn back() -> i32 { crate::core_a::a() }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn uses_a_test_only_item_below() { assert!(crate::leaf::MEASURING); }\n}\n",
    "src/top.rs": "pub fn run() { let _ = crate::core_b::back(); }\n",
  });
  spawnSync("cargo", ["generate-lockfile", "--offline"], { cwd: crate });
  const run = spawnSync(process.execPath, [script("prototype-c.mjs"), "--repo", crate], {
    encoding: "utf8",
  });
  assert.equal(run.status, 0, run.stderr);
  assert.equal(run.stdout, "prototype-c: onepipeline-base 1 modules, onepipeline-core 2, root 1\n");
  const manifest = readFileSync(join(crate, "Cargo.toml"), "utf8");
  assert.match(
    manifest,
    /default-members = \[".", "crates\/onepipeline-base", "crates\/onepipeline-core"\]/,
  );
  assert.match(
    readFileSync(join(crate, "crates", "onepipeline-core", "Cargo.toml"), "utf8"),
    /onepipeline-base = \{ path = "\.\.\/onepipeline-base", features = \["test-support"\] \}/,
  );
  assert.match(
    readFileSync(join(crate, "src", "lib.rs"), "utf8"),
    /pub use onepipeline_core::core_a;/,
  );
  assert.doesNotMatch(readFileSync(join(crate, "src", "core_a.rs"), "utf8"), /pub\(crate\)/);
  // It builds, and the moved unit test runs against the test-only item below it.
  const check = spawnSync("cargo", ["test", "--offline", "--quiet"], {
    cwd: crate,
    encoding: "utf8",
    env: { ...process.env, CARGO_TARGET_DIR: join(crate, "target") },
  });
  assert.equal(check.status, 0, check.stderr);
  assert.match(check.stdout, /1 passed/);
});

test("demonstration c writes the split and refreshes the lock for its new members", () => {
  const crate = join(scratch, "proto-c-demo");
  rmSync(crate, { recursive: true, force: true });
  write(crate, {
    "Cargo.toml":
      '[workspace]\nmembers = ["."]\n\n[package]\nname = "onepipeline"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n',
    "src/lib.rs": "pub mod x;\npub mod y;\n",
    "src/main.rs": "fn main() {}\n",
    "src/x.rs": "pub fn x() { crate::y::y() }\n",
    "src/y.rs": "pub fn y() {}\npub fn back() { crate::x::x() }\n",
    ".github/workflows/visual-docs.yml":
      "name: Visual docs\non:\n  pull_request:\n    types: [closed]\njobs: {}\n",
  });
  spawnSync("cargo", ["generate-lockfile", "--offline"], { cwd: crate });
  const run = spawnSync(
    process.execPath,
    [script("demo-commit.mjs"), "--repo", crate, "--variant", "c"],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
  assert.match(readFileSync(join(crate, "Cargo.lock"), "utf8"), /name = "onepipeline-core"/);
  assert.match(
    readFileSync(join(crate, ".github", "workflows", "ci.yml"), "utf8"),
    /measure-archive\.mjs coverage/,
  );
});

test("each platform is weighed by its newest saved profile run, a run timing nothing weighs no module, and a rest tier with no records weighs whole", async () => {
  const { weigh } = await import(pathToFileURL(script("observed.mjs")).href);
  const golden = JSON.parse(
    readFileSync(join(root, "tests", "golden", "test-profile-record-v2.json"), "utf8"),
  );
  const dir = join(scratch, "profiles");
  const record = (run, job, i, binary, name, wall = golden.wall_s) =>
    write(dir, {
      [`${run}/${job}/${i}.json`]: JSON.stringify({
        ...golden,
        binary,
        test: name,
        ...(wall === 0 ? zeroed(golden) : {}),
      }),
    });
  const zeroed = (g) => ({
    wall_s: 0,
    unattributed_s: 0,
    categories: Object.fromEntries(
      Object.entries(g.categories).map(([k, v]) => [
        k,
        Object.fromEntries(Object.keys(v).map((f) => [f, 0])),
      ]),
    ),
  });
  // Linux: an older run times alpha alone; the newer one alpha once and beta three times.
  record("100", "test-linux", 0, "e2e", "alpha::one");
  record("200", "test-linux", 0, "e2e", "alpha::one");
  for (const i of [1, 2, 3]) record("200", "test-linux", i, "e2e", `beta::t${i}`);
  // macOS: one run whose records time nothing.
  record("200", "test-macos", 0, "e2e", "alpha::one", 0);
  const rows = (job, task, minutes) => ({
    jobs: [{ job, ran: [{ task, minutes, cause: "one input" }], replayed: [] }],
  });
  const weights = weigh(
    [
      rows("test-linux", "onepipeline-e2e:test", 8),
      rows("test-linux", "onepipeline:test-rest", 4),
      rows("test-macos", "onepipeline-e2e:test-quick", 6),
      rows("test-macos", "onepipeline:test-quick", 3),
    ],
    dir,
    new Set(["e2e"]),
  );
  assert.equal(weights.sources.linux.run, "200");
  assert.equal(weights.modules.alpha.linux.minutes, 2);
  assert.equal(weights.modules.beta.linux.minutes, 6);
  assert.equal(weights.sources.darwin.testSeconds, 0);
  assert.ok(!weights.modules.alpha.darwin, "a run timing nothing shares nothing out");
  // Neither platform's records hold a rest-tier test, so each rest tier weighs whole.
  assert.deepEqual(weights.modules["rest:onepipeline:test-rest"].linux, {
    minutes: 4,
    unsplit: true,
  });
});

test("the report prices a typical pull request, charging a split's build delta on it", () => {
  const spike = join(scratch, "typical-spike");
  const platforms = (perSuite, delta, builds) =>
    Object.fromEntries(
      ["linux", "darwin", "win32"].map((p) => [
        p,
        {
          minutesPerFullRun: 10,
          suiteMinutesSkipped: 0,
          archiveBuildsSkipped: 0,
          buildDeltaCharged: delta * builds,
          netMinutesSaved: 0,
          netMinutesPerChange: 0,
          perSuite,
        },
      ]),
    );
  const replay = (perChange, perSuite, delta = 0) => ({
    summary: {
      changes: perChange.length,
      suites: Object.keys(perSuite).length,
      skippedSuiteRuns: 1,
      platforms: platforms(perSuite, delta, perChange.length),
    },
    suites: Object.fromEntries(Object.keys(perSuite).map((s) => [s, {}])),
    perChange,
  });
  const changes = [
    { commit: "#1", affected: ["e2e"] },
    { commit: "#2", affected: ["e2e", "docs"] },
  ];
  write(spike, {
    "observed.json": JSON.stringify({
      prs: [
        {
          pr: 1,
          title: "typical",
          release: false,
          changed: ["src/a.rs", "tests/e2e/a.rs"],
          moved: [],
          jobs: [],
        },
        { pr: 2, title: "docs", release: false, changed: ["docs/a.md"], moved: [], jobs: [] },
      ],
      weights: { tasks: {}, modules: {} },
      totals: {
        alone: { nonRelease: {}, release: {} },
        several: { nonRelease: {}, release: {} },
        unforced: { nonRelease: {}, release: {} },
        replayed: { nonRelease: {}, release: {} },
      },
    }),
    "today.json": JSON.stringify(replay(changes, { e2e: 30, docs: 2 })),
    // (b) skips the typical pull request's docs suite (2 min) and charges 0.5 a build.
    "b.json": JSON.stringify(replay(changes, { e2e: 30, docs: 2 }, 0.5)),
    "d-install-documented.json": JSON.stringify(
      Object.fromEntries(
        ["linux", "darwin", "win32"].map((p) => [p, { runs: 2, replays: 1, minutesPerChange: 3 }]),
      ),
    ),
  });
  const evidence = join(scratch, "typical-evidence");
  mkdirSync(evidence, { recursive: true });
  const run = spawnSync(
    process.execPath,
    [
      script("report-tables.mjs"),
      "--spike",
      spike,
      "--evidence",
      evidence,
      "--out",
      join(spike, "t.md"),
    ],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
  const md = readFileSync(join(spike, "t.md"), "utf8");
  // One typical pull request: today's keys skip its docs suite, 2 min a platform.
  assert.match(
    md,
    /^\| today's tiers \(keys allow\) \| 1 of 4 \| 0\.0 \/ 0\.0 \/ 0\.0 \| 2\.0 \/ 2\.0 \/ 2\.0 \| - \|$/m,
  );
  // (b) the same, less its 0.5 build delta.
  assert.match(
    md,
    /^\| \(b\) e2e split into suites \| 1 of 4 \| 0\.0 \/ 0\.0 \/ 0\.0 \| 1\.5 \/ 1\.5 \/ 1\.5 \| 0\.0 \/ 0\.0 \/ 0\.0 \|$/m,
  );
  assert.match(
    md,
    /^\| \(d2\) .* \| 1 \/ 1 \/ 1 of 2 runs \| 3\.0 \/ 3\.0 \/ 3\.0 \| as all 30 \| - \|$/m,
  );
});

test("an artifact that is not a zip is refused rather than recorded as holding no record", () => {
  const dir = join(scratch, "fetch-corrupt");
  mkdirSync(dir, { recursive: true });
  write(dir, {
    "list.json": JSON.stringify({ artifacts: [artifact(40, "nx-cache-entries-test-linux", 1)] }),
    "not-a-zip": "this is not a zip archive",
    "log.txt": "a log\n",
  });
  const bin = ghDouble(dir, {
    merged: [{ number: 4, title: "feat: four", mergedAt: at(1) }],
    rollups: {
      4: rollup("d".repeat(40), [suite(41, 0, "CI", [checkRun("test-linux", 4101, 0, 10)])]),
    },
    routes: {
      "actions/runs/41/artifacts": join(dir, "list.json"),
      "actions/jobs/4101/logs": join(dir, "log.txt"),
      "actions/artifacts/40/zip": join(dir, "not-a-zip"),
    },
  });
  const out = join(dir, "out");
  const run = spawnSync(
    process.execPath,
    [script("fetch-evidence.mjs"), "--out", out, "--prs", "1"],
    {
      encoding: "utf8",
      env: { ...process.env, PATH: `${bin}:${process.env.PATH}` },
    },
  );
  assert.equal(run.status, 1);
  assert.match(
    run.stderr,
    /job 4101's nx-cache-entries artifact did not unpack \(unzip exit \d+\)/,
  );
  assert.match(run.stderr, /^next: re-run; the artifact is downloaded again$/m);
  assert.ok(!existsSync(join(out, "executions", "4101.json")));
});

test("a merge whose commit the clone lacks is refused with what to fetch, not read as a conflict", () => {
  const repo = join(scratch, "ws");
  const evidence = join(scratch, "missing-commit-evidence");
  const head = "e".repeat(40);
  write(evidence, {
    "population.json": JSON.stringify({
      window_from: at(0),
      non_release: [{ number: 51, title: "feat", mergedAt: at(1) }],
      release: [],
    }),
    "rollup/51.json": JSON.stringify(
      rollup(head, [suite(510, 0, "CI", [checkRun("test-linux", 5101, 0, 10)])]),
    ),
    "logs/5101.log": `${at(0)} HEAD is now at 1234567 Merge ${head} into ${git(repo, ["rev-parse", "main"])}\n`,
  });
  const run = spawnSync(
    process.execPath,
    [replay, "--observed", evidence, "--clone", repo, "--out", join(scratch, "m.json")],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 1);
  assert.match(run.stderr, /git merge-tree \S+ e{40} failed in /);
  assert.match(run.stderr, /^next: fetch the pull requests' heads into the clone/m);
});
