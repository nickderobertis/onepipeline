// Measurement only: exercise the comparison command with the shared recorded trace.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";

const root = resolve(import.meta.dirname, "../..");
const recorded = join(root, "npm/test/golden/launch-inventory");
const script = join(root, "scripts/git-reads-compare.mjs");
const names = [
  "destination::a_hook_git_would_skip_and_a_hook_that_cannot_start_are_told_apart",
  "destination::a_rules_check_this_build_cannot_read_a_policy_off_leaves_the_node_unchecked",
];
const run = (...args) => spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });

function withSides(body) {
  const scratch = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "compare-test-"),
  );
  try {
    const side = (label, seconds = [60], status = "PASS", platform = "linux") => {
      for (const [i, duration] of seconds.entries()) {
        const dir = join(scratch, label, `attempt-${i}`, `test-${platform}`);
        mkdirSync(dir, { recursive: true });
        cpSync(recorded, dir, { recursive: true });
        const hostPath = join(dir, "test-profile/launch-host.json");
        const host = JSON.parse(readFileSync(hostPath, "utf8"));
        host.platform = { linux: "linux", macos: "darwin", windows: "win32" }[platform];
        writeFileSync(hostPath, JSON.stringify(host));
        const log = readFileSync(join(dir, "job.log"), "utf8")
          .replace(
            /(^.*Summary \[ {2}60\.000s\].*$)/m,
            `${names.map((n) => `2026-11-14T09:00:59Z ${status} [ 1.000s] onepipeline::e2e ${n}`).join("\n")}\n$1`,
          )
          .replace("Summary [  60.000s]", `Summary [  ${duration.toFixed(3)}s]`);
        const finalLog =
          status === "FAIL"
            ? log.replace("2 passed", "0 passed, 2 failed")
            : status === "SLOW"
              ? log.replace("2 passed", "1 passed, 1 failed")
              : log;
        writeFileSync(join(dir, "job.log"), finalLog);
        if (status === "SILENT") {
          for (const file of readdirSync(join(dir, "test-profile/e2e"))) {
            const path = join(dir, "test-profile/e2e", file);
            const profile = JSON.parse(readFileSync(path, "utf8"));
            profile.run_id = "63400e03-501d-4561-9341-259ad855a180";
            writeFileSync(path, JSON.stringify(profile));
          }
        }
      }
      return `${label}=${join(scratch, label)}`;
    };
    body(side);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

test("the command folds recorded launches, measured means, verbs and passed-name evidence", () => {
  withSides((side) => {
    const result = run("compare", side("before"), side("after"));
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /\| linux \| 1\/1 \| 1\.000 \| 1\.000 \| 0\.000 \| undecided/);
    assert.match(result.stdout, /\| rev-parse --git-path \| .* \| 9 \| 9 \|/);
    assert.match(result.stdout, /\| onepipeline plan \| 24 \| 24 \|/);
    assert.match(result.stdout, /all 2 passed/);
    for (const name of names) assert.ok(result.stdout.includes(name), name);
    assert.match(result.stdout, /workflow blob/);
  });
});

test("a missing or failed outcome is never counted as passed just because its profile exists", () => {
  withSides((side) => {
    const result = run("compare", side("before"), side("after", [60], "FAIL"));
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /2 not passed:/);
    const unknown = run("compare", side("before"), side("unknown", [60], "SLOW"));
    assert.equal(unknown.status, 0, unknown.stderr);
    assert.match(unknown.stdout, /2 not passed:/);
  });
});

test("silent passes are proven by matching invocation records and an all-passed summary", () => {
  withSides((side) => {
    const result = run("compare", side("before"), side("silent", [60], "SILENT"));
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /all 2 passed/);
    assert.match(result.stdout, /2 summary-supported/);
    const unmatched = run("compare", side("before"), side("unmatched", [60], "UNMATCHED"));
    assert.equal(unmatched.status, 0, unmatched.stderr);
    assert.match(unmatched.stdout, /2 not passed:/);
  });
});

test("a mean difference smaller than either side's spread stays undecided", () => {
  withSides((side) => {
    const result = run("compare", side("before", [6000, 6600]), side("after", [6540, 7140]));
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /\| -9\.000 \| undecided/);
  });
});

test("bad usage and absent artifact directories give an actionable refusal", () => {
  assert.equal(run("compare").status, 2);
  const result = run("compare", "before=/does-not-exist", "after=/does-not-exist");
  assert.equal(result.status, 1);
  assert.match(result.stderr, /does not exist\nnext:/);
});

const jobOf = (side) => join(side.slice(side.indexOf("=") + 1), "attempt-0/test-linux");

test("metadata, checkout SHA, coverage and retry evidence reach the command's report", () => {
  withSides((side) => {
    const before = side("before");
    const after = side("after");
    const dir = jobOf(after);
    writeFileSync(
      join(dir, "meta.json"),
      JSON.stringify({
        run_id: 12,
        attempt: 3,
        job_id: 45,
        head_sha: "a".repeat(40),
        workflow_blob: "b".repeat(40),
        started_at: "2026-11-14T08:30:00Z",
        completed_at: "2026-11-14T09:30:00Z",
      }),
    );
    const path = join(dir, "job.log");
    let log = readFileSync(path, "utf8");
    log = log.replace(
      `PASS [ 1.000s] onepipeline::e2e ${names[0]}`,
      `TRY 1 FAIL [ 1.000s] onepipeline::e2e ${names[0]}\n2026-11-14T09:00:59Z FLAKY [ 1.000s] onepipeline::e2e ${names[0]}`,
    );
    writeFileSync(
      path,
      `2026-11-14T08:30:00Z /usr/bin/git log -1 --format=%H\n2026-11-14T08:30:01Z ${"c".repeat(40)}\n${log}\n2026-11-14T09:30:00Z lines percent: 95.74\n`,
    );
    const result = run("compare", before, after);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /12\/3\/45/);
    assert.ok(result.stdout.includes("c".repeat(40)));
    assert.ok(result.stdout.includes(`workflow blob ${"b".repeat(40)}`));
    assert.match(result.stdout, /60\.00 \| 30\.00/);
    assert.match(result.stdout, /95\.74%/);
    assert.match(result.stdout, /re-run on this side: destination::a_hook/);
    assert.match(result.stdout, /all 2 passed/);
    assert.match(result.stdout, /partial \(restore key\), 69\.3 s/);
  });
});

test("malformed metadata is refused instead of becoming timing or identity evidence", () => {
  withSides((side) => {
    const before = side("before");
    const after = side("after");
    for (const value of [
      null,
      [],
      { run_id: "12" },
      { workflow_blob: "bad" },
      { started_at: "yesterday" },
      { unexpected: true },
      { started_at: "2026-11-15T00:00:00Z", completed_at: "2026-11-14T00:00:00Z" },
    ]) {
      writeFileSync(join(jobOf(after), "meta.json"), JSON.stringify(value));
      const result = run("compare", before, after);
      assert.equal(result.status, 1, result.stderr);
      assert.match(result.stderr, /invalid run metadata\nnext:/);
    }
  });
});

test("removed and added reads are printed from changed recorded argv", () => {
  withSides((side) => {
    const before = side("before");
    const after = side("after");
    const dir = join(jobOf(after), "test-profile/launch");
    for (const file of readdirSync(dir).filter((f) => f.endsWith(".jsonl"))) {
      const path = join(dir, file);
      const lines = readFileSync(path, "utf8")
        .trim()
        .split("\n")
        .map((line) => {
          const r = JSON.parse(line);
          if (r.kind === "start" && r.program === "git" && r.argv[0] === "show-ref") {
            r.argv = ["cat-file", "-e", "HEAD^{commit}"];
            r.sub = "cat-file";
          }
          return JSON.stringify(r);
        });
      writeFileSync(path, `${lines.join("\n")}\n`);
    }
    const result = run("compare", before, after);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /\| show-ref --verify \| .* \| 8 \| 0 \| .* \| removed \|/);
    assert.match(result.stdout, /\| cat-file -e \| .* \| 0 \| 8 \| .* \| added \|/);
  });
});

test("the vendored-test wrapper refuses missing and unowned destinations without deleting them", () => {
  const wrapper = join(root, "spikes/git-reads/test-vendored-onevcs.sh");
  const invoke = (...args) =>
    spawnSync("bash", [wrapper, ...args], {
      encoding: "utf8",
      env: { ...process.env, ONEVCS_SOURCE: root },
    });
  assert.equal(invoke().status, 2);
  const source = invoke(root);
  assert.equal(source.status, 1);
  assert.match(source.stderr, /inside the source tree/);
  withSides((side) => {
    const dir = join(jobOf(side("before")), "unowned");
    mkdirSync(join(dir, "onevcs"), { recursive: true });
    const keep = join(dir, "onevcs/keep");
    writeFileSync(keep, "keep this checkout");
    const refused = invoke(dir);
    assert.equal(refused.status, 1);
    assert.match(refused.stderr, /unowned existing destination/);
    assert.equal(readFileSync(keep, "utf8"), "keep this checkout");
  });
});

test("macOS and Windows producer platforms are normalized into their comparison rows", () => {
  withSides((side) => {
    for (const platform of ["macos", "windows"]) {
      const result = run(
        "compare",
        side(`before-${platform}`, [60, 60, 60], "PASS", platform),
        side(`after-${platform}`, [30, 30, 30], "PASS", platform),
      );
      assert.equal(result.status, 0, result.stderr);
      assert.ok(result.stdout.includes(`| ${platform} | 3/3 | 1.000 | 0.500 | 0.500 | decided |`));
    }
    const before = side("before");
    const after = side("after");
    const path = join(jobOf(after), "test-profile/launch-host.json");
    const host = JSON.parse(readFileSync(path, "utf8"));
    host.platform = "unknown-platform";
    writeFileSync(path, JSON.stringify(host));
    const refused = run("compare", before, after);
    assert.equal(refused.status, 1);
    assert.match(refused.stderr, /unsupported host platform/);
  });
});

test("the local collector refuses old output and points at a real cargo failure's log", () => {
  const collector = join(root, "spikes/git-reads/local-slice.sh");
  withSides((side) => {
    const dir = jobOf(side("collector"));
    const old = spawnSync("bash", [collector, dir], { encoding: "utf8" });
    assert.equal(old.status, 1);
    assert.match(old.stderr, /next: choose a new output directory/);
    const failed = spawnSync("bash", [collector, join(dir, "fresh")], {
      cwd: "/",
      encoding: "utf8",
    });
    assert.equal(failed.status, 1);
    assert.match(failed.stderr, /command failed; evidence: .*job.log/);
    assert.match(failed.stderr, /next: read that log/);
    assert.match(
      readFileSync(join(dir, "fresh/stock/local/test-linux/job.log"), "utf8"),
      /could not find.*Cargo.toml/,
    );
  });
});

test("malformed coverage evidence is refused and job diagnostic re-runs are named separately", () => {
  withSides((side) => {
    const before = side("before");
    const after = side("after");
    const path = join(jobOf(after), "job.log");
    const log = readFileSync(path, "utf8");
    for (const percent of ["1..2", "101", "-1", "NaN"]) {
      writeFileSync(path, `${log}\n2026-11-14T09:30:00Z lines percent: ${percent}\n`);
      const result = run("compare", before, after);
      assert.equal(result.status, 1, percent);
      assert.match(result.stderr, /invalid coverage percentage/);
    }
    writeFileSync(
      path,
      `${log}\n2026-11-14T09:30:00Z rerun-failed: re-run 1 of 3: onepipeline::e2e ${names[0]} passed\n2026-11-14T09:30:01Z rerun-failed: re-run 2 of 3: onepipeline::e2e ${names[0]} FAILED\n`,
    );
    const result = run("compare", before, after);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /diagnostic re-runs: .*1 passed, 1 failed/);
    assert.match(result.stdout, /all 2 passed/);
  });
});

test("local verification prints counts without comparing shared-host elapsed times", () => {
  withSides((side) => {
    const result = run("counts", side("before"), side("after"));
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /\| before \| linux \| .* \| 39 \| 2 \| 0 \|/);
    assert.match(result.stdout, /\| onepipeline plan \| 24 \|/);
    assert.doesNotMatch(result.stdout, /minutes|seconds|saved|mean difference|git s\s*\|/);
  });
});

test("engine-owned reads are separated from onevcs starts for adoption pricing", () => {
  withSides((side) => {
    const before = side("before");
    const after = side("after");
    const dir = join(jobOf(after), "test-profile/launch");
    for (const file of readdirSync(dir).filter((f) => f.endsWith(".jsonl"))) {
      const path = join(dir, file);
      const records = readFileSync(path, "utf8")
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line));
      const removed = new Set(
        records
          .filter((r) => r.kind === "start" && r.function === "onepipeline::destination::git_path")
          .map((r) => `${r.pid}:${r.seq}`),
      );
      writeFileSync(
        path,
        `${records
          .filter((r) => !removed.has(`${r.pid}:${r.seq}`))
          .map((record) => JSON.stringify(record))
          .join("\n")}\n`,
      );
    }
    const result = run("compare", before, after);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /\| destination::git_path \| 6 \| 0 \|/);
    assert.match(result.stdout, /\| vcs::worktree_head \| 0 \| 0 \|/);
  });
});

test("a Git start without an exit remains counted with an explicitly unknown lifetime", () => {
  withSides((side) => {
    const before = side("before");
    const after = side("after");
    const dir = join(jobOf(after), "test-profile/launch");
    const path = join(
      dir,
      readdirSync(dir).find((f) => f.endsWith(".jsonl")),
    );
    const records = readFileSync(path, "utf8")
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
    const first = records.find((r) => r.kind === "start" && r.program === "git");
    const kept = records.filter(
      (r) => !(r.kind === "exit" && r.pid === first.pid && r.seq === first.seq),
    );
    assert.ok(kept.length < records.length, "the recorded Git start had a paired exit to remove");
    writeFileSync(path, `${kept.map((r) => JSON.stringify(r)).join("\n")}\n`);
    const result = run("compare", before, after);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /after\/attempt-0\/test-linux: unmeasured Git lifetimes: 1/);
    assert.match(result.stdout, /\(\+1\.0 unknown\)/);
    assert.match(result.stdout, /\| linux \| 1\/1 .* \| 39 \| 39 \|/);
  });
});
