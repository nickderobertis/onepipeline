#!/usr/bin/env node
// Take the `host-view-time` measurement against an installed release binary.
//
//   node scripts/host-view-measure.mjs BINARY
//
// CI's `install` job runs this, through `just host-view-budget`, on the binary its
// `cargo install --path .` has just put on PATH: the binary users run, already
// built, so the budget adds only what measuring costs. It generates the 100-run
// fixture with the generator the e2e journeys use (`tests/e2e/host_view_fixture.mjs`),
// reads it cold once — the read that leaves it steady, as a host reaches steady
// state — stamps the sparse run's summary as its writer keeps it, and then, with
// every ended run's journal unreadable to the reads, times five single steady
// reads and holds each to the fixture's answer. The same reads over the root grown
// to 400 and 800 run roots of ended history follow, as evidence that the cost does
// not grow with it. Everything goes to `target/budget-records/host-view.json`,
// which `scripts/host-view-budget.mjs` reports.
//
// That no read opens an ended journal is held by `tests/e2e/host_view.rs`, under
// `strace`; here the reads simply cannot, and are held to answering correctly.

import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  cold,
  generate,
  grow,
  HOST,
  readable,
  stampSparse,
  startToken,
} from "../tests/e2e/host_view_fixture.mjs";
import { BREAKDOWN, READS, validateRecord } from "./host-view-budget.mjs";

const REPO = join(dirname(fileURLToPath(import.meta.url)), "..");
const RECORD = join(REPO, "target", "budget-records", "host-view.json");

const binary = process.argv[2];
if (!binary) {
  console.error(
    "usage: node scripts/host-view-measure.mjs BINARY (an installed release build of onepipeline)",
  );
  process.exit(2);
}

const work = mkdtempSync(join(tmpdir(), "host-view-measure-"));
const runs = join(work, "runs");
// The read's whole environment: the root, the host name the fixture's records
// carry, and a state root of its own for the linked `onevcs`.
process.env.ONEPIPELINE_RUNS_DIR = runs;
process.env.HOSTNAME = HOST;
process.env.ONEVCS_HOME = join(work, "onevcs-home");
const children = [];

/** A real process standing in for a dispatch or a driver, and its start token. */
function sleeper() {
  const child = spawn("sleep", ["86400"], { stdio: "ignore" });
  children.push(child);
  return { pid: child.pid, token: startToken(child.pid) };
}

const load1 = () => Number(readFileSync("/proc/loadavg", "utf8").split(/\s+/)[0]);

/** One `host` read, timed from spawn to exit, with its phase line. */
function read(args, label) {
  const phases = join(work, `${label}.phases.jsonl`);
  rmSync(phases, { force: true });
  // The wall clock the read's own marks are stamped against, to a millisecond: the
  // start and exit figures are that precise, and no more.
  const spawnedMs = Date.now();
  const began = process.hrtime.bigint();
  const done = spawnSync(binary, ["host", ...args], {
    encoding: "utf8",
    env: { ...process.env, ONEPIPELINE_HOST_PHASES: phases },
  });
  const wallMs = Number(process.hrtime.bigint() - began) / 1e6;
  if (done.status !== 0)
    throw new Error(`${label}: \`host ${args.join(" ")}\` exited ${done.status}: ${done.stderr}`);
  let line = null;
  try {
    line = JSON.parse(readFileSync(phases, "utf8").trim().split("\n").pop());
  } catch {
    throw new Error(`${label}: the read recorded no phase line`);
  }
  const marks = line.marks_unix_ns ?? {};
  const breakdown = Object.fromEntries(
    BREAKDOWN.map(([key, name]) => [key, line.phases?.[name]?.wall_ms ?? null]),
  );
  Object.assign(breakdown, {
    process_start_ms: marks.run ? Math.max(0, marks.run / 1e6 - spawnedMs) : null,
    exit_ms: marks.flushed ? Math.max(0, spawnedMs + wallMs - marks.flushed / 1e6) : null,
    live_runs_folded: line.phases?.["live-fold"]?.count ?? 0,
    live_journal_bytes: line.quantities?.["live-journal-bytes"] ?? 0,
    cold_journal_bytes: line.quantities?.["cold-journal-bytes"] ?? null,
    read_bytes: line.read_bytes ?? null,
  });
  return { wallMs, stdout: done.stdout, breakdown };
}

const key = (pair) => pair.join("/");

/** Hold one `--json` reading to the fixture's answer; throw naming what differs. */
function holds(manifest, reading, label) {
  const problems = [];
  const { expected } = manifest;
  if (reading.schema_version !== 1 || reading.host !== HOST || reading.root_error !== null) {
    problems.push(`the reading's header is not version 1's over ${HOST}`);
  }
  const rows = new Map(reading.dispatches.map((row) => [key([row.run, row.node]), row]));
  for (const pair of expected.held) {
    if (rows.get(key(pair))?.proof !== "held") problems.push(`live ${key(pair)} is not a held row`);
  }
  for (const pair of expected.unproven) {
    const row = rows.get(key(pair));
    if (row?.proof !== "unproven" || !row.unproven)
      problems.push(`${key(pair)} is not an UNPROVEN row with its reason`);
  }
  const wantRows = new Set([...expected.held, ...expected.unproven].map(key));
  if (rows.size !== wantRows.size || [...rows.keys()].some((row) => !wantRows.has(row))) {
    problems.push(`rows ${[...rows.keys()].join(", ")} are not the fixture's`);
  }
  const gone = new Map(reading.gone.map((item) => [key([item.run, item.node]), item.reason]));
  const wantGone = new Set([...expected.stale, ...expected.stopped].map(key));
  if (gone.size !== wantGone.size || [...gone.keys()].some((item) => !wantGone.has(item))) {
    problems.push(`gone ${[...gone.keys()].join(", ")} is not the fixture's`);
  }
  for (const pair of expected.stopped) {
    const reason = gone.get(key(pair)) ?? "";
    if (!reason.startsWith("its run was stopped") || !reason.includes("its workers signalled")) {
      problems.push(`${key(pair)}'s reason is not a signalled stop's: ${reason}`);
    }
  }
  const skipped = reading.skipped.map((item) => item.path.split("/").pop()).sort();
  if (skipped.join() !== [...expected.skipped].sort().join())
    problems.push("the skipped roots are not the launch-less ones");
  if (problems.length > 0) throw new Error(`${label}: ${problems.join("; ")}`);
}

const median = (walls) => [...walls].sort((a, b) => a - b)[Math.floor(walls.length / 2)];

function steady(manifest, label) {
  readable(runs, manifest.ended, false);
  const walls = [];
  let last;
  const load = load1();
  for (let n = 1; n <= READS; n += 1) {
    last = read(["--json"], `${label}-${n}`);
    holds(manifest, JSON.parse(last.stdout), `${label} read ${n}`);
    walls.push(last.wallMs);
  }
  return { walls, breakdown: last.breakdown, load };
}

try {
  const processes = {
    drivers: [sleeper(), sleeper()],
    entries: [[sleeper(), sleeper()], [sleeper(), sleeper()], [sleeper()]],
  };
  const generating = process.hrtime.bigint();
  const manifest = generate(runs, processes);
  const generatedMs = Number(process.hrtime.bigint() - generating) / 1e6;

  cold(runs, manifest);
  const coldRead = read(["--json"], "the cold read");
  holds(manifest, JSON.parse(coldRead.stdout), "the cold read");
  stampSparse(runs, manifest, binary);

  const measured = steady(manifest, "steady");
  readable(runs, manifest.ended, false);
  const text = spawnSync(binary, ["host"], { encoding: "utf8" });
  if (text.status !== 0 || !text.stdout.includes("UNPROVEN"))
    throw new Error(`the text form did not render the reading: ${text.stderr}`);

  const sizes = [
    {
      run_roots: 100,
      ended_runs: manifest.ended.length,
      reads_ms: measured.walls,
      median_ms: median(measured.walls),
      load1: measured.load,
    },
  ];
  for (const total of [400, 800]) {
    grow(runs, manifest, total);
    const grown = steady(manifest, `scaling-${total}`);
    sizes.push({
      run_roots: total,
      ended_runs: manifest.ended.length,
      reads_ms: grown.walls,
      median_ms: median(grown.walls),
      load1: grown.load,
    });
  }
  const increments = sizes.slice(1).map((size, n) => ({
    from: sizes[n].run_roots,
    to: size.run_roots,
    ms_per_ended_run:
      (size.median_ms - sizes[n].median_ms) / (size.ended_runs - sizes[n].ended_runs),
  }));

  const record = {
    schema_version: 1,
    binary,
    runs_root: {
      run_roots: 100,
      live_dispatches: manifest.expected.held.length,
      stale_entries: manifest.expected.stale.length,
    },
    steady: {
      build: "release",
      reads_ms: measured.walls,
      median_ms: median(measured.walls),
      load1: measured.load,
      breakdown: measured.breakdown,
      ended_journals_unreadable: true,
    },
    cold: { read_ms: coldRead.wallMs, breakdown: coldRead.breakdown, generation_ms: generatedMs },
    scaling: { sizes, increments },
  };
  // The reporter's own check, so a figure this writes and that one reads by
  // another name fails here, where it was measured.
  validateRecord(record);
  mkdirSync(dirname(RECORD), { recursive: true });
  writeFileSync(RECORD, JSON.stringify(record, null, 2));
  console.log(
    `host-view-time: ${measured.walls.map((ms) => ms.toFixed(1)).join(", ")} ms; record at ${RECORD}`,
  );
} catch (error) {
  console.error(
    `host-view-measure: ${error.message}\nnext: pass an installed release build of this tree's onepipeline (\`cargo install --path .\`), on Linux, with \`npm ci\` done; a read that answered wrongly is a defect of the view, which \`just test-e2e 'test(/^host_view::/)'\` reproduces against this tree's own build`,
  );
  process.exitCode = 1;
} finally {
  for (const child of children) child.kill("SIGKILL");
  spawnSync("chmod", ["-R", "u+rwX", work]);
  rmSync(work, { recursive: true, force: true });
}
