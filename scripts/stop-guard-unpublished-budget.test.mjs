// The read-only stop-verdict reader: it accepts a complete current-gate record and
// refuses every other one with the command that regenerates it.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { sourceFingerprint } from "./stop-guard-unpublished-build.mjs";
import { readRecord, slowest } from "./stop-guard-unpublished-record.mjs";

const schema = JSON.parse(
  readFileSync(new URL("./stop-guard-unpublished-budget.schema.json", import.meta.url), "utf8"),
);
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const runId = "e".repeat(64);
const binaryBytes = Buffer.from("a producing binary");

function sample(base) {
  const calls_us = Array.from({ length: 10 }, (_, index) => base + index);
  return {
    calls_us,
    median_us: Math.floor((calls_us[4] + calls_us[5]) / 2),
    max_us: calls_us[9],
    load1: 0.5,
  };
}

function record() {
  const classes = {
    "in-part": 1,
    landed: 1,
    live: 1,
    no: 1,
    retirable: 1,
    superseded: 1,
    unknown: 1,
  };
  const workload = (scale) => ({
    scale,
    shape: schema["x-workloads"][String(scale)],
    class_counts: classes,
    own_runs: 8,
    scenarios: schema["x-scenarios"].map((scenario, index) => ({
      scenario,
      verdict_sha256: "d".repeat(64),
      warm: sample(100_000 + index * 1000),
      cold: scale === 1 ? sample(300_000 + index * 1000) : null,
      warm_git: 3,
      cold_git: scale === 1 ? 40 : null,
    })),
  });
  return {
    version: 1,
    build: {
      run_id: runId,
      binary: schema["x-binary"],
      binary_sha256: sha(binaryBytes),
      profile: "release",
      source_sha256: sourceFingerprint(),
    },
    load_workers: 0,
    workloads: [workload(1), workload(10)],
    preparation_ms: 40_000,
    total_ms: 90_000,
    load1: 1.25,
  };
}

function directory(document = record(), invocation) {
  const dir = mkdtempSync(join(tmpdir(), "stop-verdict-records-"));
  writeFileSync(join(dir, schema["x-record"]), JSON.stringify(document));
  writeFileSync(
    join(dir, schema["x-invocation"]),
    JSON.stringify(
      invocation ?? {
        state: "complete",
        run_id: runId,
        binary: schema["x-binary"],
        binary_sha256: sha(binaryBytes),
        source_sha256: sourceFingerprint(),
      },
    ),
  );
  writeFileSync(join(dir, schema["x-binary"]), binaryBytes);
  return dir;
}

test("a complete current-gate record is read, and the slowest scenario is the one reported", () => {
  const dir = directory();
  const read = readRecord(dir);
  assert.equal(slowest(read, 1, "warm").scenario.scenario, "unreadable");
  assert.equal(slowest(read, 1, "warm").max, 102_009);
  assert.equal(slowest(read, 1, "cold").max, 302_009);
  rmSync(dir, { recursive: true });
});

const refusals = [
  ["a missing record", (dir) => rmSync(join(dir, schema["x-record"])), /is missing/],
  [
    "an unreadable record",
    (dir) => writeFileSync(join(dir, schema["x-record"]), "{"),
    /unreadable/,
  ],
  ["a missing invocation", (dir) => rmSync(join(dir, schema["x-invocation"])), /is missing/],
  [
    "an incomplete invocation",
    (dir) =>
      writeFileSync(
        join(dir, schema["x-invocation"]),
        JSON.stringify({ state: "started", run_id: runId }),
      ),
    /missing or incomplete/,
  ],
  [
    "an incomplete record",
    (_dir, doc) => {
      delete doc.workloads[0].scenarios[0].warm_git;
    },
    /incomplete telemetry/,
  ],
  [
    "a record with a field the schema does not name",
    (_dir, doc) => {
      doc.extra = 1;
    },
    /is not a field/,
  ],
  [
    "nine calls",
    (_dir, doc) => {
      doc.workloads[0].scenarios[1].warm.calls_us.pop();
    },
    /incomplete scale 1 owed warm/,
  ],
  [
    "a foreign run",
    (_dir, doc) => {
      doc.build.run_id = "f".repeat(64);
    },
    /stale or foreign invocation/,
  ],
  [
    "stale build inputs",
    (_dir, doc, inv) => {
      doc.build.source_sha256 = "0".repeat(64);
      inv.source_sha256 = doc.build.source_sha256;
    },
    /build inputs changed/,
  ],
  [
    "another binary",
    (dir) => writeFileSync(join(dir, schema["x-binary"]), "another"),
    /not the one that produced it/,
  ],
  [
    "a debug binary",
    (_dir, doc) => {
      doc.build.profile = "debug";
    },
    /non-release/,
  ],
  [
    "a load run",
    (_dir, doc) => {
      doc.load_workers = 16;
    },
    /load run/,
  ],
  [
    "a smaller workload",
    (_dir, doc) => {
      doc.workloads[1].shape = [20, 371, 41, 2308];
    },
    /foreign workload at scale 10/,
  ],
  [
    "one scale only",
    (_dir, doc) => {
      doc.workloads.pop();
    },
    /foreign workloads/,
  ],
  [
    "a missing class",
    (_dir, doc) => {
      delete doc.workloads[0].class_counts.live;
    },
    /missing a recovery class/,
  ],
  [
    "a missing scenario",
    (_dir, doc) => {
      doc.workloads[0].scenarios.pop();
    },
    /scenarios are none,owed/,
  ],
  [
    "no cold sample",
    (_dir, doc) => {
      doc.workloads[0].scenarios[0].cold = null;
    },
    /no cold sample/,
  ],
  [
    "no git counted",
    (_dir, doc) => {
      doc.workloads[0].scenarios[1].warm_git = 0;
      doc.workloads[0].scenarios[1].cold_git = 0;
    },
    /counted no git/,
  ],
];

for (const [name, spoil, expected] of refusals) {
  test(`the reader refuses ${name}`, () => {
    const doc = record();
    const invocation = {
      state: "complete",
      run_id: runId,
      binary: schema["x-binary"],
      binary_sha256: sha(binaryBytes),
      source_sha256: sourceFingerprint(),
    };
    const dir = directory(doc, invocation);
    spoil(dir, doc, invocation);
    if (!["a missing record", "an unreadable record"].includes(name)) {
      writeFileSync(join(dir, schema["x-record"]), JSON.stringify(doc));
    }
    if (!["a missing invocation", "an incomplete invocation"].includes(name)) {
      writeFileSync(join(dir, schema["x-invocation"]), JSON.stringify(invocation));
    }
    assert.throws(() => readRecord(dir), expected);
    rmSync(dir, { recursive: true });
  });
}

const command = fileURLToPath(new URL("./stop-guard-unpublished-budget.mjs", import.meta.url));

test("the budget command reports each mode through the SDK, and refuses naming the journey", () => {
  const dir = directory();
  for (const [args, value] of [
    [[], 0.102009],
    [["--cold"], 0.302009],
    [["--scale", "10"], 0.102009],
    [["--journey-time"], 90],
  ]) {
    const result = join(dir, "result.json");
    const ran = spawnSync(process.execPath, [command, ...args], {
      env: { ...process.env, ONEBUDGETSPEC_RESULT: result, ONEPIPELINE_BUDGET_RECORDS: dir },
      encoding: "utf8",
    });
    assert.equal(ran.status, 0, ran.stderr);
    const reported = JSON.parse(readFileSync(result, "utf8"));
    assert.equal(reported.value, value, JSON.stringify(reported));
    rmSync(result);
  }
  rmSync(join(dir, schema["x-record"]));
  const refused = spawnSync(process.execPath, [command], {
    env: {
      ...process.env,
      ONEBUDGETSPEC_RESULT: join(dir, "r.json"),
      ONEPIPELINE_BUDGET_RECORDS: dir,
    },
    encoding: "utf8",
  });
  assert.equal(refused.status, 1);
  assert.match(refused.stderr, /next: run 'just stop-verdict-journeys'/);
  rmSync(dir, { recursive: true });
});

const invocation = fileURLToPath(
  new URL("./stop-guard-unpublished-invocation.mjs", import.meta.url),
);

function invoke(dir, ...program) {
  return spawnSync(process.execPath, [invocation, ...program], {
    env: { ...process.env, ONEPIPELINE_BUDGET_RECORDS: dir },
    encoding: "utf8",
  });
}

test("a producer that fails, cannot start, or is not named leaves no record a budget reads", () => {
  for (const [program, status] of [
    [[process.execPath, "-e", "process.exit(3)"], 3],
    [["/nonexistent/stop-verdict-producer"], 1],
    [[], 2],
  ]) {
    const dir = directory();
    const ran = invoke(dir, ...program);
    assert.equal(ran.status, status, ran.stderr);
    assert.match(ran.stderr, /next: run 'just stop-verdict-journeys'/);
    const manifest = JSON.parse(readFileSync(join(dir, schema["x-invocation"]), "utf8"));
    assert.equal(manifest.state, "failed");
    assert.throws(() => readFileSync(join(dir, schema["x-record"])), /ENOENT/);
    assert.throws(() => readRecord(dir), /is missing|incomplete/);
    rmSync(dir, { recursive: true });
  }
});

test("a producer that succeeds hands it the invocation identity it marked started", () => {
  const dir = directory();
  const ran = invoke(
    dir,
    process.execPath,
    "-e",
    "const fs=require('node:fs');fs.writeFileSync(process.argv[1], process.env.ONEPIPELINE_BUDGET_INVOCATION)",
    join(dir, "seen"),
  );
  assert.equal(ran.status, 0, ran.stderr);
  const seen = readFileSync(join(dir, "seen"), "utf8");
  const manifest = JSON.parse(readFileSync(join(dir, schema["x-invocation"]), "utf8"));
  assert.match(seen, /^[a-f0-9]{64}$/);
  assert.equal(manifest.run_id, seen);
  assert.equal(manifest.state, "started", "only the journey itself completes the invocation");
  rmSync(dir, { recursive: true });
});
