#!/usr/bin/env node
// The runs root `onepipeline host` is held to: one generator, shared by the e2e
// journeys in `tests/e2e/host_view.rs` (which drive this tree's own build under
// `strace`) and by `scripts/host-view-measure.mjs` (which times the release binary
// CI's `install` job has just installed, for the `host-view-time` budget).
//
// A real host's root accumulates over weeks — hundreds of ended runs, long-dead
// drivers, a 3.6 GiB journal, engines older than the dispatch registry — which no
// test can produce through the engine in its lifetime. So it is generated, in the
// shapes the engine writes: every journal is the recorded run
// `tests/recorded/run-root/onemessagebus-repair-2` renamed and padded, to a size
// drawn from a real host's quantiles, with that store's own records in the kind
// mix a real host's journals carry by bytes, so a journal of a given size costs
// the view what a real one does.
//
// Live dispatches are real processes the caller starts and owns: their pids and
// procfs start tokens are handed in, so whoever started them is who ends them.
//
// llmlint: ignore-file[tests_mirror_real_usage] this file is the generator: what is under
// test is `onepipeline host` over a runs root a host accumulates over weeks — hundreds of
// ended runs, long-dead drivers, a 3.6 GiB journal, engines older than the dispatch registry
// — which no test can produce through the engine in its lifetime. Every document is in the
// shape the engine writes, every journal is a recorded run's own records, a summary stamp
// moved stands in for the writer that keeps it current, and every read of the root is the
// real binary's. `tests/e2e/host_view.rs` drives a real adoption through the engine.
//
// As a command:
//   host_view_fixture.mjs generate RUNS PROCESSES.json   writes the 100-run root, prints its manifest
//   host_view_fixture.mjs grow RUNS MANIFEST.json TOTAL    grows it with ended history, prints the manifest
//   host_view_fixture.mjs cold RUNS MANIFEST.json          forgets every ended run's derived documents
//   host_view_fixture.mjs stamp-sparse RUNS MANIFEST.json BINARY   the sparse run's summary, as its writer keeps it
//   host_view_fixture.mjs run RUNS SPEC.json               writes one run, shaped as SPEC says
//   host_view_fixture.mjs append RUNS SPEC.json            appends SPEC's ending to a run's journal

import { spawnSync } from "node:child_process";
import {
  appendFileSync,
  chmodSync,
  linkSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  truncateSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const REPO = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const SEED_RUN = "onemessagebus-repair-2";
const MIB = 1 << 20;
/** The size of the largest journal on the host the budget was measured on, a long-ended run's: generated sparse, so it costs no disk. */
export const SPARSE_BYTES = 3_838_692_260;
/** The host name every generated record and every read carries, so a launch record's driver and a registry entry's pid are this host's to prove. */
export const HOST = "host-view-fixture";
/** How long after a run records a dispatch its registry entry is stamped: the executor writes it once the dispatch has launched. */
export const ENTRY_AFTER_MS = 1_000;
/** What one byte of a real host's journals is, by record kind: the share each kind held of that host's 5.2 GiB of journals, measured 2026-10-10. */
const KIND_MIX = [
  ["turn-activity", 47.2],
  ["member-heartbeat", 17.05],
  ["planner-surface-queued", 5.85],
  ["planner-surfaced", 4.52],
  ["turn-started", 3.89],
  ["turn-message", 2.36],
  ["turn-completed", 1.32],
  ["oneharness-session", 0.61],
];

/** SplitMix64, so a fixture is the same fixture every time it is generated. */
export class Draw {
  constructor(seed) {
    this.state = BigInt.asUintN(64, BigInt(seed));
  }

  next() {
    this.state = BigInt.asUintN(64, this.state + 0x9e3779b97f4a7c15n);
    let z = this.state;
    z = BigInt.asUintN(64, (z ^ (z >> 30n)) * 0xbf58476d1ce4e5b9n);
    z = BigInt.asUintN(64, (z ^ (z >> 27n)) * 0x94d049bb133111ebn);
    return z ^ (z >> 31n);
  }

  unit() {
    return Number(this.next() >> 11n) / 2 ** 53;
  }

  between(low, high) {
    return low + (high - low) * this.unit();
  }

  below(n) {
    return Number(this.next() % BigInt(n));
  }
}

/** A journal stamp, `YYYY-MM-DDThh:mm:ss.sssZ`, for epoch milliseconds. */
export const stamp = (ms) => new Date(ms).toISOString();
const millis = (ts) => Date.parse(ts);

/** The recorded store, split into what a run starts with, the records its body is padded with, and how it settles. */
export function readSeed() {
  const dir = join(REPO, "tests", "recorded", "run-root", SEED_RUN);
  const records = readFileSync(join(dir, "events.jsonl"), "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line));
  const at = (kind) => {
    const index = records.findIndex((record) => record.kind === kind);
    if (index < 0) throw new Error(`the recorded run holds no ${kind}`);
    return index;
  };
  const head = records.slice(0, at("turn-started") + 1);
  const settledAt = at("member-settled");
  const settle = records.slice(settledAt, at("node-settled") + 1);
  const body = new Map();
  for (const [kind] of KIND_MIX) {
    const rendered = records
      .slice(head.length, settledAt)
      .filter((record) => record.kind === kind)
      .map((record) => {
        const template = { ...record, ts: "@@TS@@", seq: "@@SEQ@@" };
        if (record.source === "pipeline") template.stream = "@@STREAM@@";
        return JSON.stringify(template);
      });
    if (rendered.length === 0) throw new Error(`the recorded run holds no ${kind}`);
    body.set(kind, rendered);
  }
  return {
    head,
    settle,
    body,
    launch: JSON.parse(readFileSync(join(dir, "launch.json"), "utf8")),
    t0: millis(records[0].ts),
  };
}

/** The recorded plan, with a task per extra node beside its one `plan` task. */
function planWith(seed, nodes) {
  const plan = structuredClone(seed.head[0].payload.plan);
  for (const node of nodes) plan.tasks.push({ ...plan.tasks[0], id: node, title: node });
  return plan;
}

/**
 * One run's store: the recorded head, a body padded to `size` bytes, and the
 * ending — the recorded settlement where `settled`, then `tail`.
 */
export function journal(seed, draw, { run, size, startMs, settled, tail = [], nodes = [] }) {
  const shift = startMs - seed.t0;
  const lines = [];
  let length = 0;
  const seqs = new Map();
  let clock = startMs;
  const nextSeq = (stream) => {
    const seq = seqs.get(stream) ?? 0;
    seqs.set(stream, seq + 1);
    return seq;
  };
  const push = (line) => {
    lines.push(line);
    length += Buffer.byteLength(line) + 1;
  };
  const emit = (record, at) => {
    const renamed = JSON.parse(JSON.stringify(record).replaceAll(SEED_RUN, run));
    clock = Math.max(clock, at);
    renamed.ts = stamp(clock);
    renamed.seq = nextSeq(renamed.stream ?? "");
    push(JSON.stringify(renamed));
  };
  for (const record of seed.head) {
    const written =
      record.kind === "run-started" && nodes.length > 0
        ? { ...record, payload: { ...record.payload, plan: planWith(seed, nodes) } }
        : record;
    emit(written, millis(record.ts) + shift);
  }
  const ending = settled ? [...seed.settle, ...tail] : [...tail];
  const endingBytes = ending.reduce((sum, record) => sum + JSON.stringify(record).length + 1, 0) + 512;
  const target = size - endingBytes;
  const total = KIND_MIX.reduce((sum, [, share]) => sum + share, 0);
  let surfaces = 0;
  while (length < target) {
    let pick = draw.unit() * total;
    const kind = (KIND_MIX.find(([, share]) => (pick -= share) < 0) ?? KIND_MIX[0])[0];
    const choices = seed.body.get(kind);
    const template = choices[draw.below(choices.length)];
    clock += 5 + Number(draw.next() % 896n);
    let stream;
    if (template.includes("@@STREAM@@")) {
      // Each surface on a stream of its own, as the channel's producers write them.
      surfaces += 1;
      stream = `${HOST}-surface-${surfaces}`;
    } else {
      stream = template.split('"stream":"')[1]?.split('"')[0] ?? "";
    }
    push(
      template
        .replaceAll(SEED_RUN, run)
        .replace('"@@TS@@"', JSON.stringify(stamp(clock)))
        .replace('"@@SEQ@@"', String(nextSeq(stream)))
        .replace("@@STREAM@@", stream),
    );
  }
  for (const record of ending) emit(record, clock + 1_000);
  return `${lines.join("\n")}\n`;
}

/** A `pipeline` record of this host's. */
export function pipeline(run, kind, node, at, payload = {}) {
  return {
    v: 2,
    ts: stamp(at),
    stream: `${HOST}-4000002`,
    seq: 0,
    source: "pipeline",
    kind,
    labels: node ? { run_id: run, node } : { run_id: run },
    payload,
    artifacts: [],
  };
}

/** A run's stop, as an engine records it, on a stream of the stopping process's own; a `null` teardown is one engines before the field wrote. */
export function stopped(run, teardown, at) {
  const payload = { owner: "[mine]", forced: false };
  if (teardown !== null) payload.teardown = teardown;
  return { ...pipeline(run, "run-stopped", null, at, payload), stream: `${HOST}-4000001` };
}

/** A `driver-adopted` record, naming a driver that is gone. */
export function adopted(run, at) {
  return pipeline(run, "driver-adopted", null, at, { adoption: 1, pid: reapedPid() });
}

/** The two records a driver writes as it dispatches a node. */
export function dispatched(run, node, at) {
  return ["node-ready", "node-dispatched"].map((kind) => pipeline(run, kind, node, at));
}

/** A pid this host has just reaped, so a probe of it answers gone. */
export function reapedPid() {
  return spawnSync("true").pid;
}

/** The procfs start token of a live process, as the engine records one. */
export function startToken(pid) {
  const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
  return `linux-proc-stat:${stat.slice(stat.lastIndexOf(")") + 1).trim().split(/\s+/)[19]}`;
}

const GONE = { pid: 0, token: "linux-proc-stat:1" };
const writeJson = (path, value) => writeFileSync(path, JSON.stringify(value, null, 2));
const readJson = (path) => JSON.parse(readFileSync(path, "utf8"));

/** Write one run root: its launch record, plan, journal and, where given, its registry. */
export function writeRun(runs, seed, { run, journal: text, registry = null, driver = GONE, nodes = [] }) {
  const dir = join(runs, run);
  mkdirSync(dir, { recursive: true });
  writeJson(join(dir, "launch.json"), {
    ...seed.launch,
    run_id: run,
    project: `authoring:${run}`,
    host: HOST,
    pid: driver.pid || reapedPid(),
    started: driver.token,
  });
  writeJson(join(dir, "plan.json"), planWith(seed, nodes));
  writeFileSync(join(dir, "events.jsonl"), text);
  if (registry !== null) {
    mkdirSync(join(dir, "dispatches"));
    registry.forEach((entry, n) => writeJson(join(dir, "dispatches", `${entry.pid}-${n}.json`), entry));
  }
}

/** A registry entry, stamped just after the run recorded the dispatch it names. */
export const entry = (node, process, recorded) => ({
  node,
  pid: process.pid || reapedPid(),
  host: HOST,
  dispatched_at: stamp(recorded + ENTRY_AFTER_MS),
  started: process.token,
});

/**
 * The 100-run fixture: 9 roots with no launch record, 64 with a dispatch registry,
 * 91 journals (one of them the sparse 3.6 GiB one), 3 runs holding 5 live
 * dispatches in the caller's processes — the third run's driver dead, its dispatch
 * outliving it — 4 stale registry entries, and 2 registry-less ended runs
 * recording a node running, stopped `signalled` and `partly-signalled`, written as
 * an engine before the registry wrote them: `round-NN` directories and no
 * `dispatches/`.
 *
 * `processes` is `{drivers: [two], entries: [[two], [two], [one]]}`, each
 * `{pid, token}`. Answers the manifest every reader of the root works from.
 */
export function generate(runs, processes, now = Date.now()) {
  const draw = new Draw(100);
  const seed = readSeed();
  mkdirSync(runs, { recursive: true });
  const expected = { held: [], unproven: [], stale: [], stopped: [], skipped: [] };
  const manifest = { expected, live_runs: [], ended: [], settled: [], sparse_run: "huge-ended", sparse_records: 0 };
  const hoursAgo = now - 3 * 3_600_000;
  const mib = (value) => Math.floor(value * MIB);
  // Of every 90 journals, 45 at or below 0.2 MiB, 36 up to 2.6 MiB, 8 up to 28.7 MiB and one of 29.
  const small = Array.from({ length: 45 }, () => mib(draw.between(0.02, 0.2)));
  const rest = Array.from({ length: 36 }, () => mib(draw.between(0.2, 2.6)));
  for (let i = 0; i < 8; i += 1) rest.push(mib(draw.between(2.6, 28.7)));
  rest.push(29 * MIB);

  for (let i = 0; i < 9; i += 1) {
    const name = `no-launch-${String(i).padStart(2, "0")}`;
    mkdirSync(join(runs, name, "channel"), { recursive: true });
    expected.skipped.push(name);
  }

  [["plan", "build"], ["plan", "review"], ["plan"]].forEach((nodes, i) => {
    const run = `live-${i}`;
    const started = now - 600_000;
    const tail = [];
    const registry = nodes.map((node, k) => {
      let recorded = started;
      if (node !== "plan") {
        recorded = now - 60_000 * (k + 1);
        tail.push(...dispatched(run, node, recorded));
      }
      expected.held.push([run, node]);
      return entry(node, processes.entries[i][k], recorded);
    });
    const extra = nodes.filter((node) => node !== "plan");
    const text = journal(seed, draw, { run, size: small.pop(), startMs: started, settled: false, tail, nodes: extra });
    writeRun(runs, seed, { run, journal: text, registry, driver: processes.drivers[i] ?? GONE, nodes: extra });
    manifest.live_runs.push(run);
  });

  for (let i = 0; i < 4; i += 1) {
    const run = `stale-${i}`;
    const started = hoursAgo - i * 86_400_000;
    const text = journal(seed, draw, { run, size: rest.shift(), startMs: started, settled: false });
    writeRun(runs, seed, { run, journal: text, registry: [entry("plan", GONE, started)] });
    expected.stale.push([run, "plan"]);
    manifest.ended.push(run);
  }

  for (const [run, teardown] of [
    ["registryless-stopped", "signalled"],
    ["registryless-undecided", "partly-signalled"],
  ]) {
    const text = journal(seed, draw, { run, size: small.pop(), startMs: hoursAgo - 50 * 86_400_000, settled: false });
    writeRun(runs, seed, { run, journal: text + lines(ending(run, lastMillis(text), [{ stopped: teardown }])) });
    mkdirSync(join(runs, run, "round-01"));
    manifest.ended.push(run);
  }
  expected.stopped.push(["registryless-stopped", "plan"]);
  expected.unproven.push(["registryless-undecided", "plan"]);

  // The 3.6 GiB journal's run: ended and settled, its registry emptied as its
  // dispatches ended. Its records are generated small and the file grown sparse
  // past them, so its 3.6 GiB cost no disk.
  const sparse = manifest.sparse_run;
  const sparseText = journal(seed, draw, { run: sparse, size: MIB / 5, startMs: hoursAgo - 40 * 86_400_000, settled: true });
  writeRun(runs, seed, { run: sparse, journal: sparseText, registry: [] });
  manifest.sparse_records = Buffer.byteLength(sparseText);
  truncateSync(join(runs, sparse, "events.jsonl"), SPARSE_BYTES);
  manifest.ended.push(sparse);

  // The rest: ended, settled runs, to 91 journals and 64 registries.
  const sizes = [...small, ...rest];
  const withRegistry = 64 - 3 - 4 - 1;
  for (let i = 0; sizes.length > 0; i += 1) {
    const size = sizes.splice(draw.below(sizes.length), 1)[0];
    const run = `ended-${String(i).padStart(3, "0")}`;
    const text = journal(seed, draw, { run, size, startMs: hoursAgo - (i + 1) * 3_600_000, settled: true });
    writeRun(runs, seed, { run, journal: text, registry: i < withRegistry ? [] : null });
    manifest.ended.push(run);
    manifest.settled.push(run);
  }
  const roots = readdirSync(runs).length;
  if (roots !== 100) throw new Error(`the fixture is ${roots} run roots, not 100`);
  return manifest;
}

/**
 * Grow the root with ended, settled runs — and launch-less roots, at 9% — to
 * `total` run roots. Each added run is one of the 100-run fixture's own settled
 * runs with its journal **hardlinked**, and its summary and checkpoint rewritten to
 * its own id, so the disk it costs is a few KiB.
 */
export function grow(runs, manifest, total) {
  const draw = new Draw(total);
  for (let n = manifest.expected.skipped.length; n < Math.floor((total * 9) / 100); n += 1) {
    const name = `no-launch-${String(n).padStart(3, "0")}`;
    mkdirSync(join(runs, name, "channel"), { recursive: true });
    manifest.expected.skipped.push(name);
  }
  for (let i = 0; readdirSync(runs).length < total; i += 1) {
    const source = manifest.settled[draw.below(manifest.settled.length)];
    const run = `clone-${total}-${String(i).padStart(4, "0")}`;
    const [from, to] = [join(runs, source), join(runs, run)];
    mkdirSync(to);
    linkSync(join(from, "events.jsonl"), join(to, "events.jsonl"));
    linkSync(join(from, "plan.json"), join(to, "plan.json"));
    writeJson(join(to, "launch.json"), { ...readJson(join(from, "launch.json")), run_id: run, project: `authoring:${run}` });
    if (readdirSync(from).includes("dispatches")) mkdirSync(join(to, "dispatches"));
    for (const document of ["summary.json", "checkpoint.json"]) {
      writeJson(join(to, document), { ...readJson(join(from, document)), run_id: run });
    }
    manifest.ended.push(run);
  }
  return manifest;
}

/** Make each of `names`' journals readable to the view's process, or not. */
export function readable(runs, names, yes) {
  for (const name of names) chmodSync(join(runs, name, "events.jsonl"), yes ? 0o644 : 0o000);
}

/** The cold state: no derived document beside any ended journal, every ended journal readable but the sparse one, which no state makes readable. */
export function cold(runs, manifest) {
  for (const run of manifest.ended) {
    for (const document of ["summary.json", "checkpoint.json"]) rmSync(join(runs, run, document), { force: true });
  }
  readable(
    runs,
    manifest.ended.filter((run) => run !== manifest.sparse_run),
    true,
  );
  readable(runs, [manifest.sparse_run], false);
}

/**
 * The sparse run's writer-maintained summary, standing in for the journal writer
 * that keeps it current as it appends: written by a read of `binary` over the
 * journal's records alone — every other ended journal unreadable, so nothing else
 * is read — then stamped to the 3.6 GiB file. `binary` runs in this process's
 * environment, which names the runs root.
 */
export function stampSparse(runs, manifest, binary) {
  const journalPath = join(runs, manifest.sparse_run, "events.jsonl");
  readable(runs, manifest.ended, false);
  readable(runs, [manifest.sparse_run], true);
  truncateSync(journalPath, manifest.sparse_records);
  const read = spawnSync(binary, ["host"], { stdio: ["ignore", "ignore", "pipe"] });
  if (read.status !== 0) throw new Error(`${binary} host exited ${read.status}: ${read.stderr}`);
  truncateSync(journalPath, SPARSE_BYTES);
  readable(runs, [manifest.sparse_run], false);
  const about = statSync(journalPath);
  const summaryPath = join(runs, manifest.sparse_run, "summary.json");
  writeJson(summaryPath, {
    ...readJson(summaryPath),
    journal_len: about.size,
    journal_mtime_ms: Math.floor(about.mtimeMs),
  });
}

const lastMillis = (text) => millis(JSON.parse(text.trimEnd().split("\n").pop()).ts);

/**
 * The records an ending names, a minute apart after `last`: `{stopped: teardown}`
 * (`null` for a stop recording none), `{adopted: true}`, `{dispatched: node}`.
 */
function ending(run, last, items) {
  let at = last;
  return items.flatMap((item) => {
    at += 60_000;
    if ("stopped" in item) return [stopped(run, item.stopped, at)];
    if ("adopted" in item) return [adopted(run, at)];
    if ("dispatched" in item) return dispatched(run, item.dispatched, at);
    throw new Error(`an ending names no record: ${JSON.stringify(item)}`);
  });
}

const lines = (records) => records.map((record) => `${JSON.stringify(record)}\n`).join("");

/**
 * One run, shaped as `spec` says: `{run, started_ms, size?, settled?, nodes?,
 * registry?: null | [{node, pid?, token?, recorded_ms}], driver?: {pid, token},
 * ending?: [...], rounds?, grow_to?}`. A registry entry with no pid names a
 * process this host has reaped; `rounds` writes the `round-01` directory an engine
 * before the registry kept; `grow_to` grows the journal sparse past its records.
 */
export function writeOne(runs, spec) {
  const seed = readSeed();
  let text = journal(seed, new Draw(spec.draw ?? spec.run.length), {
    run: spec.run,
    size: spec.size ?? 40 * 1024,
    startMs: spec.started_ms,
    settled: spec.settled ?? false,
    nodes: spec.nodes ?? [],
  });
  text += lines(ending(spec.run, lastMillis(text), spec.ending ?? []));
  const registry =
    spec.registry === undefined || spec.registry === null
      ? null
      : spec.registry.map((item) => entry(item.node, { pid: item.pid ?? 0, token: item.token ?? GONE.token }, item.recorded_ms));
  writeRun(runs, seed, { run: spec.run, journal: text, registry, driver: spec.driver ?? GONE, nodes: spec.nodes ?? [] });
  if (spec.rounds) mkdirSync(join(runs, spec.run, "round-01"));
  if (spec.grow_to) truncateSync(join(runs, spec.run, "events.jsonl"), spec.grow_to);
}

/** Append `spec.ending` to `spec.run`'s journal, after its last record. */
export function append(runs, spec) {
  const path = join(runs, spec.run, "events.jsonl");
  appendFileSync(path, lines(ending(spec.run, lastMillis(readFileSync(path, "utf8")), spec.ending)));
}

// What the command is handed is checked before anything is written: a run's name
// becomes a path under the root, so it is a plain name and nothing else.
const RUN_NAME = /^[a-z0-9][a-z0-9-]*$/;

function refuse(message) {
  throw new Error(`refused: ${message}`);
}

function runName(value, what) {
  if (typeof value !== "string" || !RUN_NAME.test(value)) refuse(`${what} must be a run name, not ${JSON.stringify(value)}`);
  return value;
}

function count(value, what) {
  if (!Number.isSafeInteger(value) || value < 0) refuse(`${what} must be a nonnegative integer`);
  return value;
}

function root(value) {
  if (typeof value !== "string" || value === "") refuse("the runs root must be a path");
  return value;
}

function processOf(value, what) {
  if (typeof value !== "object" || value === null || !Number.isSafeInteger(value.pid) || value.pid <= 0 || typeof value.token !== "string") {
    refuse(`${what} must be {pid, token} of a live process`);
  }
  return value;
}

function processes(value) {
  if (!Array.isArray(value?.drivers) || value.drivers.length !== 2) refuse("processes.drivers must name two drivers");
  value.drivers.forEach((driver, n) => processOf(driver, `processes.drivers[${n}]`));
  const shape = [2, 2, 1];
  if (!Array.isArray(value.entries) || value.entries.length !== shape.length) refuse("processes.entries must name three runs' dispatches");
  value.entries.forEach((run, i) => {
    if (!Array.isArray(run) || run.length !== shape[i]) refuse(`processes.entries[${i}] must name ${shape[i]} dispatch(es)`);
    run.forEach((process, k) => processOf(process, `processes.entries[${i}][${k}]`));
  });
  return value;
}

function manifestOf(value) {
  const names = (list, what) => {
    if (!Array.isArray(list)) refuse(`${what} must be a list of run names`);
    for (const name of list) runName(name, what);
  };
  if (typeof value !== "object" || value === null || typeof value.expected !== "object") refuse("not a manifest this generator wrote");
  for (const key of ["held", "unproven", "stale", "stopped"]) {
    const pairs = value.expected[key];
    if (!Array.isArray(pairs) || pairs.some((pair) => !Array.isArray(pair) || pair.length !== 2)) refuse(`expected.${key} must be [run, node] pairs`);
    names(
      pairs.map((pair) => pair[0]),
      `expected.${key}`,
    );
  }
  for (const key of ["live_runs", "ended", "settled"]) names(value[key], key);
  names(value.expected.skipped, "expected.skipped");
  runName(value.sparse_run, "sparse_run");
  count(value.sparse_records, "sparse_records");
  return value;
}

function specOf(value) {
  if (typeof value !== "object" || value === null) refuse("a run's spec must be an object");
  runName(value.run, "the spec's run");
  if ("started_ms" in value) count(value.started_ms, "started_ms");
  if (value.ending !== undefined && !Array.isArray(value.ending)) refuse("ending must be a list");
  if (value.registry !== undefined && value.registry !== null && !Array.isArray(value.registry)) refuse("registry must be a list or null");
  if (value.driver !== undefined) processOf(value.driver, "driver");
  if (value.grow_to !== undefined) count(value.grow_to, "grow_to");
  if (value.size !== undefined) count(value.size, "size");
  return value;
}

function command(argv) {
  const [verb, ...args] = argv;
  const out = (value) => process.stdout.write(`${JSON.stringify(value)}\n`);
  switch (verb) {
    case "generate":
      return out(generate(root(args[0]), processes(readJson(args[1]))));
    case "grow": {
      const total = Number(args[2]);
      if (!Number.isSafeInteger(total) || total < 100) refuse(`grow wants a total of at least 100 run roots, not ${args[2]}`);
      return out(grow(root(args[0]), manifestOf(readJson(args[1])), total));
    }
    case "cold":
      return cold(root(args[0]), manifestOf(readJson(args[1])));
    case "stamp-sparse":
      if (!args[2]) refuse("stamp-sparse wants the binary that reads the run");
      return stampSparse(root(args[0]), manifestOf(readJson(args[1])), args[2]);
    case "run":
      return writeOne(root(args[0]), specOf(readJson(args[1])));
    case "append": {
      const spec = specOf(readJson(args[1]));
      if (!Array.isArray(spec.ending)) refuse("append wants an ending");
      return append(root(args[0]), spec);
    }
    default:
      throw new Error(`unknown verb ${verb}: generate, grow, cold, stamp-sparse, run or append`);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    command(process.argv.slice(2));
  } catch (error) {
    console.error(`host_view_fixture: ${error.message}`);
    process.exitCode = 1;
  }
}
