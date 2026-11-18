// The process sampler behind `just gate-profile`: reads /proc at a fixed interval for every process
// of one profiled run, and the host's 1-minute load average, until its stdin closes.
//
//   node gate-profile/sampler.mjs <out.json> <run id> <top task hash> <interval ms>
//
// A process belongs to the run when its environment carries the run's id or its parent belongs.
// Each one's own CPU and its reaped children's CPU are kept as last seen, and when a sampled
// process loses children between two samples, the jump in its reaped-children CPU over that
// interval is kept with them: that jump is exactly what the children and everything beneath them
// used, which no sample of their own can see once they have exited. gate-profile/fold.mjs turns
// these into each Nx task's CPU. Prints `ready` once the first sample is taken; writes <out.json>
// and exits 0 when stdin ends. Exit 2 for a usage error, 3 when /proc cannot be read or the samples cannot be written.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { availableParallelism } from "node:os";

const given = process.argv.slice(2);
const [out, runId, topHash, intervalText] = given;
const interval = Number(intervalText);
if (given.length !== 4 || !out || !runId || !Number.isInteger(interval) || interval < 10) {
  console.error(
    "gate-profile: usage: sampler.mjs <out.json> <run id> <top task hash> <interval ms>",
  );
  process.exit(2);
}

const MARKER = `ONEPIPELINE_GATE_PROFILE_RUN=${runId}`;
const WANTED = [
  "NX_TASK_TARGET_PROJECT",
  "NX_TASK_TARGET_TARGET",
  "NX_TASK_HASH",
  "ONEPIPELINE_GATE_PROFILE_INVOCATION",
];

// /proc/<pid>/stat counts CPU in USER_HZ, which the kernel fixes at 100 for user space on every
// architecture this runs on, whatever the kernel's own tick rate.
const CLOCK_TICKS_PER_SECOND = 100;

/** /proc/<pid>/stat's fields after the command name, which may itself hold spaces or parens. */
function readStat(pid) {
  let text;
  try {
    text = readFileSync(`/proc/${pid}/stat`, "utf8");
  } catch {
    return null;
  }
  const open = text.indexOf("(");
  const close = text.lastIndexOf(")");
  if (open < 0 || close < open) return null;
  const fields = text.slice(close + 2).split(" ");
  // fields[0] is field 3 (state); utime is field 14, starttime field 22.
  const numbers = [1, 11, 12, 13, 14, 19].map((i) => Number(fields[i]));
  // A process being torn down can read short; it is skipped this sample, not misread.
  if (fields.length < 20 || !numbers.every((n) => Number.isInteger(n) && n >= 0)) return null;
  const [ppid, utime, stime, cutime, cstime, starttime] = numbers;
  return {
    comm: text.slice(open + 1, close),
    ppid,
    self: utime + stime,
    children: cutime + cstime,
    starttime: String(starttime),
  };
}

function readEnvironment(pid) {
  let text;
  try {
    text = readFileSync(`/proc/${pid}/environ`, "latin1");
  } catch {
    return null;
  }
  const found = { marked: false };
  for (const entry of text.split("\0")) {
    if (entry === MARKER) found.marked = true;
    const equals = entry.indexOf("=");
    const name = entry.slice(0, equals);
    if (WANTED.includes(name)) found[name] = entry.slice(equals + 1);
  }
  return found;
}

// Samples are stamped on the monotonic clock, set against the wall clock once: a wall clock the
// host adjusts mid-run (this one has stepped back 14ms between two samples) would put them out of
// order. The shims' and Nx's wall-clock stamps stay comparable to within that adjustment.
const origin = Date.now() - performance.now();
const clock = () => Math.round(origin + performance.now());
const started = clock();
const ticks = [];
const loads = [];
const processes = new Map(); // key "pid:starttime" → record
const live = new Map(); // pid → key, for the run's processes seen in the last sample
const ignored = new Map(); // pid → starttime of a process that is not the run's
const reaps = [];

function taskOf(environment) {
  const hash = environment?.NX_TASK_HASH;
  // The Nx task the run itself was started inside is not one of the run's tasks.
  if (!hash || hash === topHash || !environment.NX_TASK_TARGET_PROJECT) return null;
  return {
    project: environment.NX_TASK_TARGET_PROJECT,
    target: environment.NX_TASK_TARGET_TARGET ?? "",
    hash,
    invocation: environment.ONEPIPELINE_GATE_PROFILE_INVOCATION ?? "",
  };
}

function sample() {
  const tick = ticks.length;
  const now = clock();
  ticks.push(now);
  // The /proc refusals below are untested: staging one takes a mount namespace without /proc,
  // which this host's unprivileged tests cannot make. Each prints and exits 3 exactly as the
  // samples-write refusal does, which the sampler's journey in gate-profile.test.mjs holds.
  let load;
  try {
    load = Number(readFileSync("/proc/loadavg", "utf8").split(" ")[0]);
  } catch (error) {
    console.error(`gate-profile: cannot read /proc/loadavg: ${error.message}`);
    process.exit(3);
  }
  if (!Number.isFinite(load) || load < 0) {
    console.error(
      `gate-profile: /proc/loadavg does not start with a load average; is /proc mounted?`,
    );
    process.exit(3);
  }
  loads.push([now, load]);
  const before = new Map(live);
  live.clear();
  let pids;
  try {
    pids = readdirSync("/proc").filter((name) => /^\d+$/.test(name));
  } catch (error) {
    console.error(`gate-profile: cannot list /proc: ${error.message}`);
    process.exit(3);
  }
  // Ascending pids put a parent before the children it forked since the last sample.
  pids.sort((a, b) => a - b);
  for (const pid of pids.map(Number)) {
    const previous = before.get(pid);
    // A pid once found not to be the run's is not read again, so a run process reusing it is
    // missed: Linux recycles a pid only after pid_max (4194304 here) others, so staging that
    // costs millions of forks, and nothing in a gate run comes near it.
    if (!previous && ignored.has(pid)) continue;
    const stat = readStat(pid);
    if (!stat) continue;
    const key = `${pid}:${stat.starttime}`;
    let record = processes.get(key);
    if (!record) {
      if (ignored.get(pid) === stat.starttime) continue;
      const environment = readEnvironment(pid);
      const parentKey = live.get(stat.ppid) ?? before.get(stat.ppid);
      if (!environment?.marked && !parentKey) {
        ignored.set(pid, stat.starttime);
        continue;
      }
      record = {
        key,
        pid,
        parent: parentKey ?? null,
        comm: stat.comm,
        task: taskOf(environment),
        first_tick: tick,
        last_tick: tick,
        self_ticks: 0,
        children_ticks: 0,
      };
      processes.set(key, record);
    }
    if (record.task === null && record.comm !== stat.comm) {
      // A process sampled between its fork and its exec still showed its parent's environment.
      // Untested: hitting that microsecond window takes a race no test can stage on demand; the
      // fixture journey's tasks are each attributed, which a miss here would break only rarely.
      record.comm = stat.comm;
      if (!record.parent || processes.get(record.parent).task === null)
        record.task = taskOf(readEnvironment(pid));
    }
    record.reaper_ticks_before = record.last_tick === tick - 1 ? record.children_ticks : null;
    record.last_tick = tick;
    record.self_ticks = stat.self;
    record.children_ticks = stat.children;
    record.ppid = stat.ppid;
    live.set(pid, key);
  }
  // Children gone since the last sample, grouped by the parent that reaped them.
  const gone = new Map();
  for (const [pid, key] of before) {
    if (live.get(pid) === key) continue;
    const child = processes.get(key);
    const reaper = live.get(child.ppid) ?? null;
    if (!reaper) continue;
    if (!gone.has(reaper)) gone.set(reaper, []);
    gone.get(reaper).push(key);
  }
  for (const [reaper, children] of gone) {
    const record = processes.get(reaper);
    if (record.reaper_ticks_before === null) continue;
    reaps.push({
      reaper,
      tick,
      ticks: record.children_ticks - record.reaper_ticks_before,
      children,
    });
  }
}

let timer;
function finish() {
  clearTimeout(timer);
  sample();
  for (const record of processes.values()) {
    delete record.reaper_ticks_before;
    delete record.ppid;
  }
  try {
    writeFileSync(
      out,
      `${JSON.stringify({
        run: runId,
        clock_ticks_per_second: CLOCK_TICKS_PER_SECOND,
        interval_ms: interval,
        nproc: availableParallelism(),
        started,
        ended: clock(),
        ticks,
        loads,
        processes: [...processes.values()],
        reaps,
      })}\n`,
    );
  } catch (error) {
    console.error(`gate-profile: cannot write the samples to ${out}: ${error.message}`);
    process.exit(3);
  }
  process.exit(0);
}

function loop() {
  sample();
  timer = setTimeout(loop, interval);
}

sample();
console.log("ready");
timer = setTimeout(loop, interval);
process.stdin.on("data", () => {});
process.stdin.on("end", finish);
// An interrupted run still has its samples written: the gate under it is stopped by the same signal.
process.on("SIGINT", finish);
