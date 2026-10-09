// The read-only stop-verdict reader: it accepts a complete current-gate record and
// refuses every other one with the command that regenerates it.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { inputFiles, sourceFingerprint } from "./stop-guard-unpublished-build.mjs";
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
    "an invocation with a field it does not state",
    (_dir, _doc, inv) => {
      inv.extra = 1;
    },
    /states fields/,
  ],
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
    "a foreign version",
    (_dir, doc) => {
      doc.version = 2;
    },
    /foreign telemetry version/,
  ],
  [
    "a missing binary",
    (dir) => rmSync(join(dir, schema["x-binary"])),
    /producing binary is missing/,
  ],
  [
    "no preparation time",
    (_dir, doc) => {
      doc.preparation_ms = 0;
    },
    /preparation\/total timing/,
  ],
  [
    "a total shorter than its preparation",
    (_dir, doc) => {
      doc.total_ms = 1;
    },
    /preparation\/total timing/,
  ],
  [
    "no own runs",
    (_dir, doc) => {
      doc.workloads[0].own_runs = 0;
    },
    /measured no own runs/,
  ],
  [
    "no verdict digest",
    (_dir, doc) => {
      doc.workloads[0].scenarios[0].verdict_sha256 = "x";
    },
    /carries no verdict/,
  ],
  [
    "a cold sample at scale 10",
    (_dir, doc) => {
      doc.workloads[1].scenarios[0].cold = sample(1);
    },
    /cold sample no budget reads/,
  ],
  [
    "a call that took no time",
    (_dir, doc) => {
      doc.workloads[0].scenarios[0].warm.calls_us[0] = 0;
    },
    /a call took no time/,
  ],
  [
    "a wrong median",
    (_dir, doc) => {
      doc.workloads[0].scenarios[0].warm.median_us += 1;
    },
    /median or maximum/,
  ],
  [
    "a wrong maximum",
    (_dir, doc) => {
      doc.workloads[0].scenarios[0].warm.max_us += 1;
    },
    /median or maximum/,
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

test("the budget command refuses arguments it does not take, and a result it cannot report", () => {
  const dir = directory();
  for (const [args, env, said] of [
    [
      ["--warm"],
      { ONEBUDGETSPEC_RESULT: join(dir, "r.json") },
      /expected no arguments, --cold, --scale 10, or --journey-time/,
    ],
    [["--scale", "2"], { ONEBUDGETSPEC_RESULT: join(dir, "r.json") }, /expected no arguments/],
    [[], {}, /ONEBUDGETSPEC_RESULT is missing/],
  ]) {
    const environment = { ...process.env, ONEPIPELINE_BUDGET_RECORDS: dir, ...env };
    if (!("ONEBUDGETSPEC_RESULT" in env)) delete environment.ONEBUDGETSPEC_RESULT;
    const ran = spawnSync(process.execPath, [command, ...args], {
      env: environment,
      encoding: "utf8",
    });
    assert.equal(ran.status, 1, ran.stderr);
    assert.match(ran.stderr, said);
    assert.match(ran.stderr, /next: run 'just stop-verdict-journeys'/);
  }
  rmSync(dir, { recursive: true });
});

test("an invocation that cannot reset or mark its records says so, naming the journey", () => {
  // A records directory that is a file: nothing can be reset under it.
  const scratch = mkdtempSync(join(tmpdir(), "stop-verdict-reset-"));
  const blocked = join(scratch, "not-a-directory");
  writeFileSync(blocked, "a file");
  const reset = invoke(blocked, process.execPath, "-e", "process.exit(0)");
  assert.equal(reset.status, 1, reset.stderr);
  assert.match(reset.stderr, /could not reset the records/);
  assert.match(reset.stderr, /next: run 'just stop-verdict-journeys'/);
  // A producer that fails and leaves the manifest unwritable: the marking fails too,
  // and the operator is told to remove what is there.
  const dir = directory();
  const manifest = join(dir, schema["x-invocation"]);
  const locking = invoke(
    dir,
    process.execPath,
    "-e",
    "require('node:fs').chmodSync(process.argv[1], 0o444); process.exit(4)",
    manifest,
  );
  spawnSync("chmod", ["644", manifest]);
  assert.equal(locking.status, 4, locking.stderr);
  assert.match(locking.stderr, /could not mark .* failed/);
  assert.match(locking.stderr, /remove it before reading budgets/);
  rmSync(dir, { recursive: true });
  rmSync(scratch, { recursive: true });
});

test("the reader refuses a producing binary it cannot read, keeping the cause", () => {
  const dir = directory();
  const binary = join(dir, schema["x-binary"]);
  spawnSync("chmod", ["000", binary]);
  assert.throws(
    () => readRecord(dir),
    /producing binary beside its record is unreadable: .*EACCES/,
  );
  spawnSync("chmod", ["644", binary]);
  rmSync(dir, { recursive: true });
});

/** A scratch root carrying one build-input manifest and the files it names. */
function manifestRoot(manifest) {
  const root = mkdtempSync(join(tmpdir(), "stop-verdict-inputs-"));
  mkdirSync(join(root, "scripts"), { recursive: true });
  mkdirSync(join(root, "src"), { recursive: true });
  writeFileSync(join(root, "src", "lib.rs"), "fn main() {}\n");
  writeFileSync(join(root, "Cargo.toml"), "[package]\n");
  writeFileSync(
    join(root, "scripts", "stop-guard-unpublished-build-inputs.json"),
    JSON.stringify(manifest),
  );
  return root;
}

test("the build-input manifest is refused unless every entry is a repository-relative path", () => {
  const good = {
    version: 1,
    files: ["Cargo.toml"],
    directories: ["src"],
    prefixes: [{ directory: "scripts", prefix: "stop-guard-unpublished" }],
  };
  const root = manifestRoot(good);
  assert.deepEqual(inputFiles(root), [
    "Cargo.toml",
    "scripts/stop-guard-unpublished-build-inputs.json",
    "src/lib.rs",
  ]);
  rmSync(root, { recursive: true });
  for (const [spoil, said] of [
    [{ version: 2 }, /unsupported build-input manifest version/],
    [{ extra: true }, /unknown fields extra/],
    [{ files: "Cargo.toml" }, /files is not a list/],
    [{ files: ["/etc/passwd"] }, /not a repository-relative path/],
    [{ directories: ["src/../.."] }, /not a repository-relative path/],
    [
      { prefixes: [{ directory: "scripts", prefix: "a/b" }] },
      /needs exactly a directory and a non-empty prefix/,
    ],
    [
      { prefixes: [{ directory: "scripts", prefix: "x", more: 1 }] },
      /needs exactly a directory and a non-empty prefix/,
    ],
  ]) {
    const root = manifestRoot({ ...good, ...spoil });
    assert.throws(() => inputFiles(root), said, JSON.stringify(spoil));
    rmSync(root, { recursive: true });
  }
  const linked = manifestRoot(good);
  symlinkSync("/etc", join(linked, "src", "linked"));
  assert.throws(() => inputFiles(linked), /unsupported build input src\/linked/);
  rmSync(linked, { recursive: true });
});

test("the fingerprint and the producing tier's Nx inputs name one set of files", () => {
  const root = fileURLToPath(new URL("../", import.meta.url));
  const nx = JSON.parse(readFileSync(join(root, "nx.json"), "utf8"));
  const tier = JSON.parse(readFileSync(join(root, "tests/stop_verdict/project.json"), "utf8"));
  const expand = (input) =>
    nx.namedInputs[input] ? nx.namedInputs[input].flatMap(expand) : [input];
  const declared = new Set(
    tier.targets.test.inputs
      .flatMap(expand)
      .map((input) => input.replace("{workspaceRoot}/", "").replace(/\/\*\*\/\*$/, "")),
  );
  const manifest = JSON.parse(
    readFileSync(join(root, "scripts/stop-guard-unpublished-build-inputs.json"), "utf8"),
  );
  const fingerprinted = new Set([
    ...manifest.files,
    ...manifest.directories,
    ...manifest.prefixes.map(({ directory, prefix }) => `${directory}/${prefix}*`),
  ]);
  assert.deepEqual([...declared].sort(), [...fingerprinted].sort());
});
