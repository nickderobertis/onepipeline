import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

// What the budget report's new shape leans on, through the entry points that really run: each check
// `test-linux` runs saves its results; each budget labelled `not-measured-on-pr` keeps the check that
// held it; and a pull request's CI-time pair reads that pull request's own run. Each case is the real
// recipe or script and the real pinned checker over the real budgets files, in a scratch tree holding
// the repository's copies. What stands in is only the workload behind a recording: `cargo`, whose
// journey would write the record seeded here, or a measuring program that would take hours. GitHub
// stands in as a recording of a real run's jobs. Verdicts and selection are asserted; a figure is
// printed, and only ever compared with a record this file seeds.

const root = fileURLToPath(new URL("../../", import.meta.url));
const scratchRoot = mkdtempSync(
  join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR || tmpdir(), "budget-results-"),
);
after(() => rmSync(scratchRoot, { recursive: true, force: true }));
let trees = 0;

const LABEL = "not-measured-on-pr";
const read = (path, base = root) => readFileSync(join(base, path), "utf8");
const budgetsIn = (path, base = root) => parse(read(path, base)).budgets;
const budgetIn = (path, id) => budgetsIn(path).find((budget) => budget.id === id);
const threshold = (path, id) => budgetIn(path, id).threshold;
// A figure past this budget's line, whichever way the line faces and even where the line is zero.
const pastLine = ({ direction, threshold: limit }) =>
  direction === "min" ? limit / 10 : (limit || 1) * 10;

function tree(files = []) {
  const dir = join(scratchRoot, String(trees++));
  mkdirSync(dir, { recursive: true });
  for (const file of ["budgets.yaml", "justfile", "Cargo.toml", "package.json", ...files]) {
    mkdirSync(dirname(join(dir, file)), { recursive: true });
    copyFileSync(join(root, file), join(dir, file));
  }
  symlinkSync(join(root, "node_modules"), join(dir, "node_modules"), "dir");
  mkdirSync(join(dir, "target", "budget-records"), { recursive: true });
  symlinkSync(join(root, "target", "tools"), join(dir, "target", "tools"), "dir");
  mkdirSync(join(dir, "stand-ins"));
  // The journey's compile and run: the record it would write is already in place.
  stand(dir, "cargo", "exit 0");
  return dir;
}
function stand(dir, name, body) {
  const path = join(dir, "stand-ins", name);
  writeFileSync(path, `#!/usr/bin/env bash\n${body}\n`);
  chmodSync(path, 0o755);
}
function program(dir, path, body) {
  mkdirSync(dirname(join(dir, path)), { recursive: true });
  writeFileSync(join(dir, path), `#!/usr/bin/env bash\n${body}\n`);
  chmodSync(join(dir, path), 0o755);
}
function record(dir, name, value) {
  writeFileSync(join(dir, "target", "budget-records", name), JSON.stringify(value));
}
function run(dir, command, args, env = {}) {
  const clean = Object.fromEntries(
    Object.entries(process.env).filter(([key]) => !/^(GITHUB_|RUNNER_|ONEBUDGETSPEC_)/.test(key)),
  );
  const ran = spawnSync(command, args, {
    cwd: dir,
    encoding: "utf8",
    env: { ...clean, PATH: `${join(dir, "stand-ins")}:${process.env.PATH}`, ...env },
    timeout: 300_000,
  });
  return { status: ran.status, out: `${ran.stdout}${ran.stderr}` };
}
const just = (dir, ...recipe) => run(dir, "just", recipe);
const saved = (dir, name) =>
  JSON.parse(readFileSync(join(dir, "target", "budget-results", name), "utf8"));
const verdicts = (out) =>
  Object.fromEntries(
    [
      ...out.matchAll(
        /^budget (\S+): (?:actual \S+ \S+, budget \S+ \S+, .* — (within|over)|(error))/gm,
      ),
    ].map((match) => [match[1], match[2] ?? match[3]]),
  );
// Each `report.mjs` reads the whole of the record, so the record is written whole, its figure over.
const HUMAN_ITEMS = (seconds) => ({
  host: "scratch",
  warm_max_elapsed_s: seconds,
  warm: [
    {
      elapsed_s: seconds,
      phase: "warm",
      index_io_ms: 1,
      discovery_ms: 1,
      replay_ms: 1,
      blocker_ms: 1,
      render_ms: 1,
      capture_ms: 1,
    },
  ],
  cold: [],
  tail: [],
  workload: { runs: 1, events_per_run: 1, open_items: 1, plans: 1, steps: 1 },
});

test("`just human-items-budget` saves what it checked, for a figure within and one over", () => {
  const limit = threshold("tests/human_items/budgets.yaml", "human-items-read-seconds");
  for (const [seconds, verdict, code] of [
    [limit / 2, "within", 0],
    [limit * 10, "over", 1],
  ]) {
    const dir = tree(["tests/human_items/budgets.yaml", "tests/human_items/report.mjs"]);
    record(dir, "human-items.json", HUMAN_ITEMS(seconds));
    const ran = just(dir, "human-items-budget");
    assert.equal(ran.status === 0 ? 0 : 1, code, ran.out);
    const [result] = saved(dir, "human-items.json").results;
    assert.deepEqual(
      [result.id, result.actual, result.verdict],
      ["human-items-read-seconds", seconds, verdict],
    );
  }
});

test("`just budgets` saves all it checked, even when a budget's input is absent", () => {
  const dir = tree(["scripts/stacked-spikes-budget.mjs", "scripts/store-operations-budget.mjs"]);
  record(dir, "stacked-spikes-stage.json", { wall_ms: 1234, run_id: "run" });
  const ran = just(dir, "budgets");
  assert.notEqual(ran.status, 0, "this tree holds only one of the records: the rest are errors");
  const results = Object.fromEntries(
    saved(dir, "onepipeline.json").results.map((result) => [result.id, result]),
  );
  assert.equal(results["stacked-spikes-stage-time"].actual, 1.234);
  assert.equal(results["stacked-spikes-stage-time"].verdict, "within");
  const expected = budgetsIn("budgets.yaml").filter(
    ({ labels = [] }) =>
      !labels.some((label) => ["release-binary", "release-tail", "host-variable"].includes(label)),
  );
  assert.deepEqual(Object.keys(results).sort(), expected.map((budget) => budget.id).sort());
  // The label filter keeps some of the catalog and leaves some out, so neither an empty selection
  // nor one that ignores the labels can pass the comparison above.
  assert.ok(
    expected.length > 0 && expected.length < budgetsIn("budgets.yaml").length,
    expected.length,
  );
});

test("`just host-view-budget`, which the install proof runs, saves `host-view-time` alone", () => {
  const dir = tree();
  // The measurement and its record's strict reader are `host-view-time`'s own tests: here they stand
  // in for a run over a release binary, and what is under test is the recipe's check.
  mkdirSync(join(dir, "scripts"));
  writeFileSync(join(dir, "scripts/host-view-measure.mjs"), "");
  writeFileSync(
    join(dir, "scripts/host-view-budget.mjs"),
    `import { report } from "@onebudgetspec/sdk";\nreport(7.5, "recorded");\n`,
  );
  stand(dir, "strace", "exit 0");
  const ran = just(dir, "host-view-budget", "/stand-in/onepipeline", "a build");
  assert.equal(ran.status, 0, ran.out);
  const { results } = saved(dir, "host-view.json");
  assert.deepEqual(
    results.map((result) => [result.id, result.actual]),
    [["host-view-time", 7.5]],
  );
  assert.ok(read("scripts/install-proof.sh").includes("just host-view-budget "));
});

test("`.github/check-budgets.sh` and the report's own check script save their results", () => {
  const macos = tree([".github/check-budgets.sh", ".github/budgets.yaml"]);
  writeFileSync(
    join(macos, ".github/budgets.yaml"),
    `schema_version: 1\nbudgets:\n  - {id: stand-in, measure: reported, command: [node, -e, "require('node:fs').writeFileSync(process.env.ONEBUDGETSPEC_RESULT, JSON.stringify({value: 1}))"], unit: jobs, direction: max, threshold: 1, description: a stand-in for the policy reader this test does not drive}\n`,
  );
  const ran = run(macos, "bash", [".github/check-budgets.sh"]);
  assert.equal(ran.status, 0, ran.out);
  assert.equal(saved(macos, "github-workflows.json").results[0].id, "stand-in");

  const own = tree([".github/reporting/check-budgets.sh", ".github/reporting/budgets.yaml"]);
  program(
    own,
    ".github/reporting/.telemetry/tools/onebudgetspec-github-action",
    'echo \'{"value": 5}\' > "$ONEBUDGETSPEC_RESULT"',
  );
  const reported = run(own, "bash", [".github/reporting/check-budgets.sh"]);
  assert.equal(reported.status, 0, reported.out);
  const [result] = saved(own, "report-job.json").results;
  assert.deepEqual([result.id, result.actual], ["onepipeline-budget-report-command-seconds", 5]);
});

test("`just budgets` still says how to install a checker that is missing, and saves nothing", () => {
  const dir = tree();
  rmSync(join(dir, "target", "tools"));
  const ran = just(dir, "budgets");
  assert.notEqual(ran.status, 0);
  assert.match(ran.out, /onebudgetspec \S+ is not installed/);
  assert.equal(existsSync(join(dir, "target", "budget-results")), false);
});

test("each target saving results is uncached, or names the file in its outputs", () => {
  const targets = {
    onepipeline: { budgets: "onepipeline.json", "install-smoke": "host-view.json" },
    "github-workflows": { budgets: "github-workflows.json" },
    "report-job": { budgets: "report-job.json" },
    "human-items-e2e": { test: "human-items.json" },
  };
  const nx = join(root, "node_modules", ".bin", "nx");
  for (const [project, saving] of Object.entries(targets)) {
    const shown = JSON.parse(
      execFileSync(nx, ["show", "project", project, "--json"], {
        cwd: root,
        encoding: "utf8",
        env: { ...process.env, NX_DAEMON: "false" },
        maxBuffer: 64 * 1024 * 1024,
      }),
    );
    for (const [target, file] of Object.entries(saving)) {
      const { cache, outputs = [] } = shown.targets[target];
      if (cache)
        assert.ok(
          outputs.includes(`{workspaceRoot}/target/budget-results/${file}`),
          `${project}:${target} is cached: a replay would not restore ${file}`,
        );
    }
  }
});

const LABELLED = {
  "budgets.yaml": [
    "onepipeline-cd-job-time",
    "onepipeline-release-cycle-time",
    "onepipeline-cd-job-count",
    "onepipeline-release-read-requests",
    "onepipeline-pr-read-requests",
    "onepipeline-nx-cache-use",
    "onepipeline-gate-time",
  ],
  "pool-maintenance/budgets.yaml": ["onepipeline-pool-maintenance-seconds"],
  "tests/journal_workload/budgets.yaml": [
    "driver-event-journal-delay",
    "driver-envelope-outcome-latency",
    "driver-idle-cpu-share",
    "driver-peak-rss",
    "liveness-journal-bytes-per-member-hour",
  ],
  "tests/pool_workload/budgets.yaml": ["pool-maintenance-engine-seconds"],
  "tests/scratch_workload/budgets.yaml": ["run-ending-scratch-seconds", "scratch-discard-seconds"],
};

test("the label is on exactly those budgets, none of which any job of ci.yml's Nx target list reaches", () => {
  for (const [file, ids] of Object.entries(LABELLED))
    assert.deepEqual(
      budgetsIn(file)
        .filter(({ labels = [] }) => labels.includes(LABEL))
        .map((budget) => budget.id)
        .sort(),
      [...ids].sort(),
      file,
    );
  assert.equal(Object.values(LABELLED).flat().length, 16);
  // The checks a pull request's jobs run: no `workload` or `budgets-host` target, which hold these.
  const jobs = read("scripts/ci-job.sh")
    .match(/^ {2}test-linux\) targets="([^"]+)"/m)[1]
    .split(",");
  assert.ok(!jobs.includes("workload") && !jobs.includes("budgets-host"), jobs.join());
});

test("publish's tail still selects its labelled budgets and reports each over, failing nothing", () => {
  const dir = tree();
  // Recorded figures over every threshold, in place of the reads release-tail.test.mjs replays.
  const over = Object.fromEntries(
    budgetsIn("budgets.yaml")
      .filter(({ labels = [] }) => labels.includes("release-tail"))
      .map((budget) => [budget.id, pastLine(budget)]),
  );
  writeFileSync(join(dir, "figures.json"), JSON.stringify(over));
  program(
    dir,
    "scripts/actions-budget-figures.sh",
    `jq -c --arg id "$ONEBUDGETSPEC_BUDGET_ID" '{value: .[$id]}' figures.json > "$ONEBUDGETSPEC_RESULT"`,
  );
  const release = parse(read(".github/workflows/release.yml"));
  const tail = release.jobs.publish.steps.at(-1);
  assert.equal(tail.run.trim(), "onebudgetspec check budgets.yaml --label release-tail");
  const pinned = read("justfile").match(/^onebudgetspec-version := "([^"]+)"$/m)[1];
  const ran = run(dir, "bash", ["--noprofile", "--norc", "-eo", "pipefail", "-c", tail.run], {
    PATH: `${join(root, "target", "tools", `onebudgetspec-${pinned}`, "bin")}:${process.env.PATH}`,
  });
  assert.equal(ran.status, 1, ran.out);
  assert.deepEqual(
    verdicts(ran.out),
    Object.fromEntries(Object.keys(over).map((id) => [id, "over"])),
  );
  assert.equal(tail["continue-on-error"], undefined);
  assert.ok(!JSON.stringify(release).includes("needs.publish"), "nothing reads the tail's result");
});

test("`just gate`'s check still warns on onepipeline-gate-time over and passes", () => {
  const dir = tree(["scripts/gate-time.sh", "scripts/gate-time-budget.mjs"]);
  const seconds = pastLine(budgetIn("budgets.yaml", "onepipeline-gate-time"));
  const started = "2026-10-01T12:00:00Z";
  const ended = new Date(Date.parse(started) + seconds * 1000).toISOString().replace(".000Z", "Z");
  writeFileSync(
    join(dir, ".gate-time.json"),
    JSON.stringify({
      schema_version: 1,
      invocation: "seeded",
      started_at: started,
      ended_at: ended,
      exit: 0,
    }),
  );
  const ran = run(dir, "bash", ["scripts/gate-time.sh", "--check"]);
  assert.equal(ran.status, 0, ran.out);
  assert.deepEqual(verdicts(ran.out), { "onepipeline-gate-time": "over" });
  assert.match(ran.out, /warning: onepipeline-gate-time is over its budget/);
});

test("`just budgets-host` still fails on onepipeline-pool-maintenance-seconds over", () => {
  const dir = tree(["pool-maintenance/budgets.yaml"]);
  const limit = pastLine(
    budgetIn("pool-maintenance/budgets.yaml", "onepipeline-pool-maintenance-seconds"),
  );
  // `pool-maintenance-budget`, the budget's command, is this script: it builds this clone twice.
  program(
    dir,
    "pool-maintenance/measure.sh",
    `echo '{"value": ${limit}}' > "$ONEBUDGETSPEC_RESULT"`,
  );
  const ran = just(dir, "_pool-maintenance-budgets");
  assert.notEqual(ran.status, 0, ran.out);
  assert.deepEqual(verdicts(ran.out), { "onepipeline-pool-maintenance-seconds": "over" });
});

const percentiles = (value) => ({ n: 1, p50: value, p95: value, max: value });
function journalRecording(figure) {
  return {
    sizes: [
      {
        size_mb: 1,
        event_delay_ms: percentiles(figure("driver-event-journal-delay")),
        activity: { emitted: 1 },
        to_first_pass_flush_s: 1,
        pass: { count: 1, intervals: 1, interval_average_ms: percentiles(1) },
        envelope_latency_ms: percentiles(figure("driver-envelope-outcome-latency")),
        envelope_outcome_ms: percentiles(1),
        idle: {
          share: figure("driver-idle-cpu-share"),
          seconds: 1,
          pass: { cpu_s_per_pass: 1 },
        },
        peak_rss_mb: figure("driver-peak-rss"),
        liveness: {
          bytes_per_member_hour: figure("liveness-journal-bytes-per-member-hour"),
          heartbeats: 1,
          liveness_bytes: 1,
          kinds: {},
          other_bytes: 1,
          other_kinds: {},
          liveness_json_bytes: 1,
        },
      },
    ],
  };
}
const timing = (seconds) => ({ entries: 1, dirs: 1, bytes: 1, seconds });

for (const [name, recipe, file, reader, recordName, recording] of [
  [
    "journal",
    "_journal-workload-run",
    "tests/journal_workload/budgets.yaml",
    "tests/journal_workload/journal-budget.mjs",
    "journal-workload.json",
    (over) => journalRecording(over),
  ],
  [
    "pool",
    "_pool-workload-run",
    "tests/pool_workload/budgets.yaml",
    "tests/pool_workload/pool-maintenance-budget.mjs",
    "pool-maintenance.json",
    (over) => ({
      started_at: "2026-10-01T12:00:00Z",
      journalled_at: "2026-10-01T12:00:00Z",
      slot_duration_ms: [],
      results_seconds: over("pool-maintenance-engine-seconds"),
      identities: 1,
      slots: 1,
    }),
  ],
  [
    "scratch",
    "_scratch-workload-run",
    "tests/scratch_workload/budgets.yaml",
    "tests/scratch_workload/scratch-budget.mjs",
    "scratch-workload.json",
    (over) => ({
      failed: timing(over("run-ending-scratch-seconds")),
      success: timing(over("run-ending-scratch-seconds")),
      discard: {
        ...timing(1),
        state_seconds: 1,
        wall_seconds: over("scratch-discard-seconds"),
      },
    }),
  ],
])
  test(`the ${name} workload's target still fails on each of its budgets over`, () => {
    const dir = tree([file, reader]);
    const over = (id) => pastLine(budgetIn(file, id));
    record(dir, recordName, recording(over));
    const ran = just(dir, recipe);
    assert.notEqual(ran.status, 0, ran.out);
    assert.deepEqual(
      verdicts(ran.out),
      Object.fromEntries(LABELLED[file].map((id) => [id, "over"])),
    );
  });

const RECORDED_JOBS = join(root, ".github/tests/fixtures/own-run/jobs.json");
const RUN = "42237631071";

function ownRun({ gh = `cat "${RECORDED_JOBS}"`, event = "pull_request" } = {}) {
  const dir = tree(["scripts/actions-budget-figures.sh"]);
  stand(dir, "gh", `printf '%s\\n' "$*" >> gh.calls\n${gh}`);
  const pinned = read("justfile").match(/^onebudgetspec-version := "([^"]+)"$/m)[1];
  const checker = join(root, "target", "tools", `onebudgetspec-${pinned}`, "bin", "onebudgetspec");
  const ran = run(
    dir,
    checker,
    [
      "check",
      "budgets.yaml",
      "--id",
      "onepipeline-ci-check-time",
      "--id",
      "onepipeline-ci-job-count",
    ],
    {
      GITHUB_EVENT_NAME: event,
      GITHUB_RUN_ID: RUN,
      GITHUB_REPOSITORY: "nickderobertis/onepipeline",
      GH_TOKEN: "replay-token",
      ONEVCS: join(dir, "no-onevcs"),
    },
  );
  const calls = existsSync(join(dir, "gh.calls"))
    ? read("gh.calls", dir).split("\n").filter(Boolean)
    : [];
  return { ...ran, calls };
}

test("the CI-time pair reports this pull request's own run, the job in progress counted once", () => {
  const { jobs } = JSON.parse(readFileSync(RECORDED_JOBS, "utf8"));
  const finished = jobs.filter((job) => job.status === "completed");
  assert.equal(jobs.length, 5);
  assert.equal(finished.length, 4, "four jobs finished and the budgets job in progress");
  const longest = Math.max(
    ...finished.map((job) => (Date.parse(job.completed_at) - Date.parse(job.started_at)) / 1000),
  );
  const ran = ownRun();
  assert.equal(ran.status, 0, ran.out);
  assert.match(
    ran.out,
    new RegExp(
      `budget onepipeline-ci-check-time: actual ${longest} seconds, budget 10800 seconds.* within`,
    ),
  );
  assert.match(ran.out, /budget onepipeline-ci-job-count: actual 5 jobs, budget 5 jobs.* within/);
  assert.match(ran.out, /CI \/ test-linux: 6549s/);
  assert.deepEqual(ran.calls, [
    `api repos/nickderobertis/onepipeline/actions/runs/${RUN}/jobs?per_page=100&filter=latest`,
    `api repos/nickderobertis/onepipeline/actions/runs/${RUN}/jobs?per_page=100&filter=latest`,
  ]);
  console.error(`replayed own-run figures, from a recording of run ${RUN}:\n${ran.out}`);
});

test("the CI-time pair has no figure when its read is refused, and off a pull request reads no run", () => {
  const refused = ownRun({ gh: 'echo "HTTP 403: refused" >&2; exit 1' });
  assert.equal(refused.status, 3, refused.out);
  assert.deepEqual(verdicts(refused.out), {
    "onepipeline-ci-check-time": "error",
    "onepipeline-ci-job-count": "error",
  });
  assert.match(refused.out, /HTTP 403: refused/);
  const push = ownRun({ event: "release" });
  assert.equal(push.status, 3, push.out);
  assert.deepEqual(push.calls, []);
  assert.match(push.out, /onevcs is not on PATH/);
});
