// Folds what one `just gate-profile` run recorded into report.json and report.md beside it.
//
//   node gate-profile/fold.mjs <run dir>     fold a run again (gate-profile/run.mjs folds each run)
//
// The run directory holds meta.json (run.mjs), run.json and phases.jsonl (the `just` shim),
// cargo.jsonl (the `cargo` shim), nx/<invocation>.{invocation,profile,graph}.json (nx-hook.sh,
// Nx's NX_PROFILE, `nx --graph`), processes.json (sampler.mjs) and cargo's own timings pages.
// REPORT_SCHEMA below is the one declaration of report.json's shape; every report is checked
// against it before it is written. Exit 0 when both reports are written, 2 for a usage error,
// 3 when a record is missing or malformed.
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { basename, join, relative } from "node:path";
import { pathToFileURL } from "node:url";

export const SCHEMA_VERSION = 1;

const num = { type: "number" };
const text = { type: "string" };
const when = { type: "string", format: "date-time" };
const nullableText = { type: ["string", "null"] };
const load = { type: "number", minimum: 0 };
const strict = (properties, optional = []) => ({
  type: "object",
  additionalProperties: false,
  required: Object.keys(properties).filter((name) => !optional.includes(name)),
  properties,
});
const list = (items) => ({ type: "array", items });

/**
 * report.json, version 1. Times are ISO-8601 UTC; durations and CPU are seconds; every `load` is
 * the host's mean 1-minute load average over that row's own interval, to be read against
 * `host.nproc`. CPU is user plus system time of the row's whole process tree; `delegated_cpu_s` is
 * CPU outside every tree of the run that a daemon spent on a task's behalf (sccache's compilers). A unit's compile
 * time has no CPU figure: cargo's timings give each unit's wall time only.
 */
export const REPORT_SCHEMA = strict({
  schema_version: { type: "integer", enum: [SCHEMA_VERSION] },
  recipe: text,
  commit: strict({ sha: nullableText, dirty: { type: "boolean" } }),
  host: strict({
    nproc: { type: "integer", minimum: 1 },
    clock_ticks_per_second: { type: "integer", minimum: 1 },
    sample_interval_ms: { type: "integer", minimum: 1 },
  }),
  run: strict({
    start: when,
    end: when,
    wall_s: num,
    cpu_s: num,
    user_s: num,
    system_s: num,
    load,
    load_min: load,
    load_max: load,
    exit: { type: "integer" },
    delegated_cpu_s: num,
    outside_phases: strict({ wall_s: num, cpu_s: num }),
  }),
  phases: list(
    strict({
      id: text,
      recipe: text,
      within: list(text),
      start: when,
      end: when,
      wall_s: num,
      cpu_s: num,
      user_s: num,
      system_s: num,
      load,
      exit: { type: "integer" },
      invocations: list(text),
    }),
  ),
  invocations: list(
    strict({
      id: text,
      phase: nullableText,
      args: list(text),
      task_graph: { type: "boolean" },
    }),
  ),
  tasks: list(
    strict({
      invocation: text,
      phase: nullableText,
      task: text,
      status: text,
      start: when,
      end: when,
      wall_s: num,
      cpu_s: num,
      cpu_basis: { type: "string", enum: ["reaped", "sampled", "none"] },
      delegated_cpu_s: num,
      load,
    }),
  ),
  critical_path: strict({
    wall_s: num,
    run_wall_s: num,
    complete: { type: "boolean" },
    tasks: list(
      strict({ invocation: text, task: text, start: when, end: when, wall_s: num, load }),
    ),
  }),
  cargo: list(
    strict({
      command: list(text),
      cwd: text,
      task: nullableText,
      invocation: nullableText,
      phase: nullableText,
      start: when,
      end: when,
      wall_s: num,
      cpu_s: num,
      user_s: num,
      system_s: num,
      load,
      exit: { type: "integer" },
      timings: list(text),
      units: list(
        strict({
          unit: text,
          name: text,
          version: text,
          target: text,
          mode: text,
          start: when,
          end: when,
          wall_s: num,
          load,
        }),
      ),
    }),
  ),
  notes: list(text),
});

const TYPES = {
  object: (v) => v !== null && typeof v === "object" && !Array.isArray(v),
  array: Array.isArray,
  string: (v) => typeof v === "string",
  number: (v) => typeof v === "number" && Number.isFinite(v),
  integer: Number.isInteger,
  boolean: (v) => typeof v === "boolean",
  null: (v) => v === null,
};

/** Every place `value` departs from `schema` (the subset REPORT_SCHEMA uses), as path: problem. */
export function validateReport(value, schema = REPORT_SCHEMA, path = "$") {
  const types = [schema.type].flat();
  if (!types.some((type) => TYPES[type](value)))
    return [`${path}: expected ${types.join(" or ")}, got ${JSON.stringify(value)}`];
  const errors = [];
  if (schema.enum && !schema.enum.includes(value))
    errors.push(`${path}: ${JSON.stringify(value)} is not one of ${JSON.stringify(schema.enum)}`);
  if (schema.minimum !== undefined && typeof value === "number" && value < schema.minimum)
    errors.push(`${path}: ${value} is below ${schema.minimum}`);
  if (schema.format === "date-time" && typeof value === "string" && Number.isNaN(Date.parse(value)))
    errors.push(`${path}: ${value} is not a date-time`);
  if (TYPES.array(value) && schema.minItems !== undefined && value.length < schema.minItems)
    errors.push(`${path}: has ${value.length} items, fewer than ${schema.minItems}`);
  if (TYPES.array(value) && schema.maxItems !== undefined && value.length > schema.maxItems)
    errors.push(`${path}: has ${value.length} items, more than ${schema.maxItems}`);
  if (TYPES.array(value) && schema.items)
    value.forEach((item, i) => {
      errors.push(...validateReport(item, schema.items, `${path}[${i}]`));
    });
  if (TYPES.object(value) && (schema.properties || schema.additionalProperties)) {
    for (const name of schema.required ?? [])
      if (!(name in value)) errors.push(`${path}: missing ${name}`);
    for (const [name, item] of Object.entries(value)) {
      if (schema.properties?.[name])
        errors.push(...validateReport(item, schema.properties[name], `${path}.${name}`));
      else if (schema.additionalProperties === false) errors.push(`${path}: unexpected ${name}`);
      else if (typeof schema.additionalProperties === "object")
        errors.push(...validateReport(item, schema.additionalProperties, `${path}.${name}`));
    }
  }
  return errors;
}

class RecordError extends Error {}

// What each record the run left must hold before the fold reads it. The writers are this
// directory's own scripts and Nx, but a run cut short or a newer Nx can leave anything.
const open = (properties) => ({
  type: "object",
  required: Object.keys(properties),
  properties,
});
const count = { type: "integer", minimum: 0 };
const epoch = { type: "number", minimum: 0 };
const cpuPair = { type: "array", items: count, minItems: 2, maxItems: 2 };
const timed = { start: epoch, end: epoch, cpu_ms_before: cpuPair, cpu_ms_after: cpuPair };
export const RECORD_SCHEMAS = {
  meta: open({
    recipe: text,
    root: text,
    commit: nullableText,
    dirty: { type: "boolean" },
    run: text,
  }),
  run: open({ recipe: text, ...timed, exit: { type: "integer" } }),
  phase: open({ id: text, parent: text, recipe: text, ...timed, exit: { type: "integer" } }),
  cargo: open({
    ...timed,
    exit: { type: "integer" },
    argv: list(text),
    cwd: text,
    target_dir: text,
    llvm_cov_target_dir: text,
    invocation: text,
    phase: text,
    task: text,
  }),
  invocation: open({ id: text, phase: text, args: list(text) }),
  profile: list({ type: "object", required: ["ph"], properties: { ph: text } }),
  profileTask: open({
    name: text,
    ts: num,
    dur: { type: "number", minimum: 0 },
    args: open({ status: text }),
  }),
  timingUnits: list(
    open({
      name: text,
      version: text,
      target: text,
      mode: text,
      start: { type: "number", minimum: 0 },
      duration: { type: "number", minimum: 0 },
    }),
  ),
  graph: open({
    tasks: open({ dependencies: { type: "object", additionalProperties: list(text) } }),
  }),
  processes: open({
    clock_ticks_per_second: { type: "integer", minimum: 1 },
    interval_ms: { type: "integer", minimum: 1 },
    nproc: { type: "integer", minimum: 1 },
    loads: list({ type: "array", items: { type: "number", minimum: 0 }, minItems: 2, maxItems: 2 }),
    processes: list(
      open({
        key: text,
        pid: count,
        parent: nullableText,
        comm: text,
        task: {
          type: ["object", "null"],
          required: ["project", "target", "hash", "invocation"],
          properties: { project: text, target: text, hash: text, invocation: text },
        },
        first_tick: count,
        last_tick: count,
        self_ticks: count,
        children_ticks: count,
      }),
    ),
    reaps: list(
      open({ reaper: text, tick: count, ticks: { type: "integer" }, children: list(text) }),
    ),
  }),
};

function checked(value, schema, what) {
  const errors = validateReport(value, schema);
  // A timed record's end and CPU counters cannot run backwards.
  if (!errors.length && schema.properties?.cpu_ms_before) {
    if (value.end < value.start) errors.push(`$: ends at ${value.end}, before it starts`);
    if (value.cpu_ms_after.some((after, i) => after < value.cpu_ms_before[i]))
      errors.push("$: its CPU counters fall between start and end");
  }
  need(
    !errors.length,
    `${what} is not as recorded: ${errors.slice(0, 5).join("; ")}${errors.length > 5 ? "; …" : ""}`,
  );
  return value;
}

function readJson(path, what, schema) {
  let raw;
  try {
    raw = readFileSync(path, "utf8");
  } catch (error) {
    throw new RecordError(`cannot read ${what} (${path}): ${error.message}`);
  }
  let value;
  try {
    value = JSON.parse(raw);
  } catch (error) {
    throw new RecordError(`${what} (${path}) is not JSON: ${error.message}`);
  }
  return checked(value, schema, `${what} (${path})`);
}

function readLines(path, what, schema) {
  if (!existsSync(path)) return [];
  return readFileSync(path, "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line, i) => {
      let value;
      try {
        value = JSON.parse(line);
      } catch (error) {
        throw new RecordError(`${what} line ${i + 1} (${path}) is not JSON: ${error.message}`);
      }
      return checked(value, schema, `${what} line ${i + 1} (${path})`);
    });
}

function need(condition, message) {
  if (!condition) throw new RecordError(message);
}

const seconds = (ms) => Math.round(ms) / 1000;
const iso = (ms) => new Date(ms).toISOString();
const round2 = (value) => Math.round(value * 100) / 100;
const cpuMs = ([user, system]) => ({ user, system });

/** Mean of the 1-minute load samples inside [from, to], or the samples' line at its midpoint. */
export function loadOver(loads, from, to) {
  if (!loads.length) return 0;
  const inside = loads.filter(([t]) => t >= from && t <= to);
  if (inside.length) return round2(inside.reduce((sum, [, l]) => sum + l, 0) / inside.length);
  const middle = (from + to) / 2;
  const after = loads.findIndex(([t]) => t > middle);
  if (after === -1) return loads[loads.length - 1][1];
  if (after === 0) return loads[0][1];
  const [t0, l0] = loads[after - 1];
  const [t1, l1] = loads[after];
  return round2(l0 + ((l1 - l0) * (middle - t0)) / (t1 - t0));
}

/**
 * Each Nx task's CPU, in clock ticks, from the sampler's processes and reaps. A process belongs to
 * the task its nearest ancestor already belongs to, and otherwise to the task its own environment
 * names, so a nested Nx run's tasks stay inside the outer task that started them. A task's CPU is
 * the CPU of the process trees it is the root of. A tree whose exit its parent was sampled across
 * gets exactly what that parent reaped (split by each tree's sampled share when several exited in
 * one interval); any other tree gets what its samples saw, which can miss what it used after its
 * last sample. A tree no process of the run started is the task's delegated CPU, kept apart.
 *
 * gate-profile.test.mjs proves the reaped case end to end. Two trees exiting in one interval, a
 * tree outliving its samples' parent and a daemon's compilers cannot be staged on demand by real
 * processes without timing races or a daemon in the test, so fold.test.mjs drives this function
 * with records in the sampler's own shape instead, each case asserting its exact figure.
 */
export function taskCpu(sampled) {
  const byKey = new Map(sampled.processes.map((p) => [p.key, p]));
  need(byKey.size === sampled.processes.length, "processes.json names a process twice");
  for (const p of sampled.processes) {
    const lineage = new Set();
    for (let up = p; up; up = up.parent ? byKey.get(up.parent) : undefined) {
      need(!lineage.has(up.key), `processes.json's parents loop through ${up.key}`);
      lineage.add(up.key);
    }
  }
  const kids = new Map();
  for (const p of sampled.processes)
    if (p.parent && byKey.has(p.parent)) {
      if (!kids.has(p.parent)) kids.set(p.parent, []);
      kids.get(p.parent).push(p);
    }
  const owner = new Map();
  const ownerOf = (p) => {
    if (owner.has(p.key)) return owner.get(p.key);
    const parent = p.parent ? byKey.get(p.parent) : undefined;
    const inherited = parent ? ownerOf(parent) : null;
    const own = p.task ? `${p.task.invocation}|${p.task.project}:${p.task.target}` : null;
    const result = inherited ?? own;
    owner.set(p.key, result);
    return result;
  };
  const sampledTotal = new Map();
  const seenTotal = (p) => {
    if (sampledTotal.has(p.key)) return sampledTotal.get(p.key);
    // Children still running at this process's last sample are not yet in its reaped CPU.
    let total = p.self_ticks + p.children_ticks;
    for (const child of kids.get(p.key) ?? [])
      if (child.last_tick >= p.last_tick) total += best(child);
    sampledTotal.set(p.key, total);
    return total;
  };
  const reaped = new Map();
  for (const reap of sampled.reaps) {
    const trees = reap.children.map((key) => byKey.get(key)).filter(Boolean);
    const seen = trees.map(seenTotal);
    const sum = seen.reduce((a, b) => a + b, 0);
    trees.forEach((tree, i) => {
      const share = sum > 0 ? seen[i] / sum : 1 / trees.length;
      reaped.set(tree.key, Math.max(seen[i], reap.ticks * share));
    });
  }
  function best(p) {
    return reaped.has(p.key) ? reaped.get(p.key) : seenTotal(p);
  }
  const tasks = new Map();
  for (const p of sampled.processes) {
    const task = ownerOf(p);
    if (!task) continue;
    const parent = p.parent ? byKey.get(p.parent) : undefined;
    if (parent && ownerOf(parent) === task) continue;
    const entry = tasks.get(task) ?? { ticks: 0, basis: "reaped", delegated_ticks: 0 };
    if (!parent) {
      // Started outside the run's process tree yet carrying its environment: work a daemon did
      // for the task (sccache's server runs the compilers cargo asks it for). Real CPU, but no
      // phase's or the run's own figure can hold it, so it is kept apart from theirs.
      entry.delegated_ticks += best(p);
    } else {
      entry.ticks += best(p);
      if (!reaped.has(p.key)) entry.basis = "sampled";
    }
    tasks.set(task, entry);
  }
  return tasks;
}

/** The longest chain of `durations` through `dependencies`, as task ids in run order. */
export function longestPath(durations, dependencies) {
  const finish = new Map();
  const via = new Map();
  const visit = (id, trail = new Set()) => {
    if (finish.has(id)) return finish.get(id);
    if (trail.has(id)) return 0;
    trail.add(id);
    let before = 0;
    let from = null;
    for (const dep of dependencies[id] ?? []) {
      if (!durations.has(dep)) continue;
      const at = visit(dep, trail);
      // A cache replay takes no time but is still the link its dependents waited on.
      if (from === null || at > before) [before, from] = [at, dep];
    }
    trail.delete(id);
    finish.set(id, before + durations.get(id));
    via.set(id, from);
    return finish.get(id);
  };
  let end = null;
  for (const id of durations.keys()) {
    const at = visit(id);
    // Ties go to the later task (ids come in start order): a no-op that ends the chain is on it.
    if (end === null || at >= finish.get(end)) end = id;
  }
  const path = [];
  for (let id = end; id !== null && id !== undefined; id = via.get(id)) path.unshift(id);
  return path;
}

const BUILD_START = /<td>Build start:<\/td><td>(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(\.\d+)?Z<\/td>/;
const UNIT_DATA = /const UNIT_DATA = (\[[\s\S]*?\n\]);/;

/** One cargo timings page: when its build started and each unit it compiled. */
export function readTimings(path) {
  const page = readFileSync(path, "utf8");
  const start = page.match(BUILD_START);
  const units = page.match(UNIT_DATA);
  need(start && units, `${path} has no build start or unit data; is it a cargo --timings page?`);
  const fraction = (start[2] ?? ".0").slice(1, 4).padEnd(3, "0");
  const built = Date.parse(`${start[1]}.${fraction}Z`);
  need(Number.isFinite(built), `${path}'s build start ${start[1]} is not a date`);
  let data;
  try {
    data = JSON.parse(units[1]);
  } catch (error) {
    throw new RecordError(`${path}'s unit data is not JSON: ${error.message}`);
  }
  return {
    built,
    written: statSync(path).mtimeMs,
    units: checked(data, RECORD_SCHEMAS.timingUnits, `${path}'s unit data`),
  };
}

/** Every cargo timings page written since `since` in a `cargo-timings` directory cargo could use. */
function timingsPages(root, cargoRuns, since) {
  const dirs = new Set();
  const consider = (dir) => {
    if (dir && existsSync(join(dir, "cargo-timings"))) dirs.add(join(dir, "cargo-timings"));
  };
  const skip = new Set(["node_modules", ".git", ".nx"]);
  const entries = (dir) => {
    try {
      return readdirSync(dir, { withFileTypes: true }).filter(
        (e) => e.isDirectory() && !skip.has(e.name),
      );
    } catch {
      return [];
    }
  };
  for (const first of entries(root)) {
    consider(join(root, first.name));
    for (const second of entries(join(root, first.name)))
      consider(join(root, first.name, second.name));
  }
  for (const run of cargoRuns) {
    consider(run.target_dir);
    consider(run.llvm_cov_target_dir);
    if (run.target_dir) consider(join(run.target_dir, "llvm-cov-target"));
  }
  const pages = [];
  for (const dir of dirs)
    for (const name of readdirSync(dir))
      if (/^cargo-timing-.+\.html$/.test(name) && statSync(join(dir, name)).mtimeMs >= since)
        pages.push(join(dir, name));
  return pages.sort();
}

/** Fold the run directory `dir` into its report object. */
export function foldRun(dir) {
  const meta = readJson(join(dir, "meta.json"), "the run's meta.json", RECORD_SCHEMAS.meta);
  const run = readJson(join(dir, "run.json"), "the whole run's record", RECORD_SCHEMAS.run);
  const sampled = readJson(
    join(dir, "processes.json"),
    "the sampler's processes.json",
    RECORD_SCHEMAS.processes,
  );
  const phaseRecords = readLines(join(dir, "phases.jsonl"), "phases.jsonl", RECORD_SCHEMAS.phase);
  const cargoRuns = readLines(join(dir, "cargo.jsonl"), "cargo.jsonl", RECORD_SCHEMAS.cargo);
  const loads = sampled.loads;
  const notes = [];
  const ms = (epochSeconds) => Math.round(Number(epochSeconds) * 1000);

  const runStart = ms(run.start);
  const runEnd = ms(run.end);
  const runCpu = cpuMs(run.cpu_ms_after);
  const runCpuBefore = cpuMs(run.cpu_ms_before);
  const runUser = runCpu.user - runCpuBefore.user;
  const runSystem = runCpu.system - runCpuBefore.system;

  // Invocations, each with its Nx task profile and, where `nx --graph` could write it, its graph.
  const nxDir = join(dir, "nx");
  const invocations = [];
  for (const name of existsSync(nxDir) ? readdirSync(nxDir).sort() : []) {
    if (!name.endsWith(".invocation.json")) continue;
    const id = name.slice(0, -".invocation.json".length);
    const record = readJson(join(nxDir, name), `invocation ${id}`, RECORD_SCHEMAS.invocation);
    const profilePath = join(nxDir, `${id}.profile.json`);
    const graphPath = join(nxDir, `${id}.graph.json`);
    const profile = existsSync(profilePath)
      ? readJson(profilePath, `Nx's profile of ${id}`, RECORD_SCHEMAS.profile)
      : null;
    if (!profile)
      notes.push(`Nx wrote no task profile for invocation ${id} (${record.args.join(" ")}).`);
    const graph = existsSync(graphPath)
      ? readJson(graphPath, `the task graph of ${id}`, RECORD_SCHEMAS.graph)
      : null;
    if (profile && !graph)
      notes.push(`No task graph for invocation ${id}: its critical path is its longest task.`);
    invocations.push({
      id,
      phase: record.phase || null,
      args: record.args,
      profiled: profile !== null,
      entries: (profile ?? [])
        .filter((e) => e.ph === "X")
        .map((e, i) => checked(e, RECORD_SCHEMAS.profileTask, `task ${i + 1} of ${id}'s profile`)),
      dependencies: graph?.tasks?.dependencies ?? null,
    });
  }

  const cpuByTask = taskCpu(sampled);
  const ticksPerSecond = sampled.clock_ticks_per_second;
  const tasks = [];
  for (const invocation of invocations)
    for (const entry of invocation.entries) {
      const start = entry.ts / 1000;
      const end = start + entry.dur / 1000;
      const cpu = cpuByTask.get(`${invocation.id}|${entry.name}`);
      tasks.push({
        invocation: invocation.id,
        phase: invocation.phase,
        task: entry.name,
        status: entry.args.status,
        start: iso(start),
        end: iso(end),
        wall_s: seconds(end - start),
        cpu_s: cpu ? round2(cpu.ticks / ticksPerSecond) : 0,
        cpu_basis: cpu && cpu.ticks > 0 ? cpu.basis : "none",
        delegated_cpu_s: cpu ? round2(cpu.delegated_ticks / ticksPerSecond) : 0,
        load: loadOver(loads, start, end),
        startMs: start,
        endMs: end,
      });
    }
  invocations.sort((a, b) => firstStart(a, tasks) - firstStart(b, tasks));
  tasks.sort((a, b) => a.startMs - b.startMs);

  // Phases: the recipes that did the run's work, i.e. those no other phase ran inside.
  const byId = new Map(phaseRecords.map((p) => [p.id, p]));
  need(byId.size === phaseRecords.length, "phases.jsonl names a phase twice");
  for (const p of phaseRecords) {
    const lineage = new Set();
    for (let up = p; up; up = byId.get(up.parent)) {
      need(!lineage.has(up.id), `phases.jsonl's parents loop through ${up.id}`);
      lineage.add(up.id);
    }
  }
  const parents = new Set(phaseRecords.map((p) => p.parent).filter(Boolean));
  const within = (p) => {
    const chain = [];
    for (let up = byId.get(p.parent); up; up = byId.get(up.parent)) chain.unshift(up.recipe);
    return chain;
  };
  const phases = phaseRecords
    .filter((p) => !parents.has(p.id))
    .map((p) => {
      const start = ms(p.start);
      const end = ms(p.end);
      const before = cpuMs(p.cpu_ms_before);
      const after = cpuMs(p.cpu_ms_after);
      return {
        id: p.id,
        recipe: p.recipe,
        within: within(p),
        start: iso(start),
        end: iso(end),
        wall_s: seconds(end - start),
        cpu_s: seconds(after.user - before.user + after.system - before.system),
        user_s: seconds(after.user - before.user),
        system_s: seconds(after.system - before.system),
        load: loadOver(loads, start, end),
        exit: p.exit,
        invocations: invocations.filter((i) => i.phase === p.id).map((i) => i.id),
        startMs: start,
      };
    })
    .sort((a, b) => a.startMs - b.startMs);

  // The run's critical path: its invocations run one after another, so it is each one's longest
  // dependency chain, in turn.
  const pathTasks = [];
  // Complete only when every invocation left both its task profile and its task graph.
  let complete = invocations.length > 0;
  for (const invocation of invocations) {
    if (!invocation.profiled || !invocation.dependencies) complete = false;
    const mine = tasks.filter((t) => t.invocation === invocation.id);
    if (!mine.length) continue;
    const durations = new Map(mine.map((t) => [t.task, t.endMs - t.startMs]));
    for (const id of longestPath(durations, invocation.dependencies ?? {})) {
      const t = mine.find((m) => m.task === id);
      pathTasks.push({
        invocation: invocation.id,
        task: id,
        start: t.start,
        end: t.end,
        wall_s: t.wall_s,
        load: t.load,
      });
    }
  }

  // Cargo invocations, each with the timings pages written inside its interval.
  const cargo = cargoRuns
    .map((c) => {
      const start = ms(c.start);
      const end = ms(c.end);
      const before = cpuMs(c.cpu_ms_before);
      const after = cpuMs(c.cpu_ms_after);
      return {
        command: ["cargo", ...c.argv],
        cwd: c.cwd,
        task: c.task || null,
        invocation: c.invocation || null,
        phase: c.phase || null,
        start: iso(start),
        end: iso(end),
        wall_s: seconds(end - start),
        cpu_s: seconds(after.user - before.user + after.system - before.system),
        user_s: seconds(after.user - before.user),
        system_s: seconds(after.system - before.system),
        load: loadOver(loads, start, end),
        exit: c.exit,
        timings: [],
        units: [],
        startMs: start,
        endMs: end,
      };
    })
    .sort((a, b) => a.startMs - b.startMs);
  const slack = 5;
  for (const page of timingsPages(meta.root, cargoRuns, runStart - 1000)) {
    const timing = readTimings(page);
    const owner = cargo
      .filter((c) => c.startMs <= timing.built + slack && c.endMs + slack >= timing.written)
      .sort((a, b) => b.startMs - a.startMs)[0];
    if (!owner) {
      notes.push(
        `${relative(meta.root, page)} was written during the run by no recorded cargo invocation.`,
      );
      continue;
    }
    owner.timings.push(relative(meta.root, page));
    for (const unit of timing.units) {
      const from = timing.built + unit.start * 1000;
      const to = from + unit.duration * 1000;
      owner.units.push({
        unit: `${unit.name} v${unit.version} ${unit.target.trim()}`.trim(),
        name: unit.name,
        version: unit.version,
        target: unit.target.trim(),
        mode: unit.mode,
        start: iso(from),
        end: iso(to),
        wall_s: round2(unit.duration),
        load: loadOver(loads, from, to),
      });
    }
  }
  for (const c of cargo) {
    c.units.sort((a, b) => b.wall_s - a.wall_s);
    if (!c.timings.length) notes.push(`\`${c.command.join(" ")}\` left no timings page.`);
  }

  const phaseWall = phases.reduce((sum, p) => sum + p.wall_s, 0);
  const phaseCpu = phases.reduce((sum, p) => sum + p.cpu_s, 0);
  const runLoads = loads.filter(([t]) => t >= runStart && t <= runEnd).map(([, l]) => l);
  const report = {
    schema_version: SCHEMA_VERSION,
    recipe: meta.recipe,
    commit: { sha: meta.commit ?? null, dirty: Boolean(meta.dirty) },
    host: {
      nproc: sampled.nproc,
      clock_ticks_per_second: ticksPerSecond,
      sample_interval_ms: sampled.interval_ms,
    },
    run: {
      start: iso(runStart),
      end: iso(runEnd),
      wall_s: seconds(runEnd - runStart),
      cpu_s: seconds(runUser + runSystem),
      user_s: seconds(runUser),
      system_s: seconds(runSystem),
      load: loadOver(loads, runStart, runEnd),
      load_min: runLoads.length ? Math.min(...runLoads) : loadOver(loads, runStart, runEnd),
      load_max: runLoads.length ? Math.max(...runLoads) : loadOver(loads, runStart, runEnd),
      exit: run.exit,
      delegated_cpu_s: round2(tasks.reduce((sum, t) => sum + t.delegated_cpu_s, 0)),
      outside_phases: {
        wall_s: seconds(Math.max(0, runEnd - runStart - phaseWall * 1000)),
        cpu_s: seconds(Math.max(0, runUser + runSystem - phaseCpu * 1000)),
      },
    },
    phases: phases.map(({ startMs, ...p }) => p),
    invocations: invocations.map((i) => ({
      id: i.id,
      phase: i.phase,
      args: i.args,
      task_graph: i.dependencies !== null,
    })),
    tasks: tasks.map(({ startMs, endMs, ...t }) => t),
    critical_path: {
      wall_s: seconds(pathTasks.reduce((sum, t) => sum + t.wall_s * 1000, 0)),
      run_wall_s: seconds(runEnd - runStart),
      complete,
      tasks: pathTasks,
    },
    cargo: cargo.map(({ startMs, endMs, ...c }) => c),
    notes,
  };
  const errors = validateReport(report);
  need(!errors.length, `the folded report departs from REPORT_SCHEMA:\n  ${errors.join("\n  ")}`);
  return report;
}

function firstStart(invocation, tasks) {
  const mine = tasks.filter((t) => t.invocation === invocation.id);
  return mine.length ? Math.min(...mine.map((t) => t.startMs)) : Number.POSITIVE_INFINITY;
}

const s = (value) => `${value.toFixed(value >= 100 ? 0 : value >= 10 ? 1 : 2)}s`;
const ratio = (cpu, wall) => (wall > 0 ? `${(cpu / wall).toFixed(1)}×` : "–");
const at = (report, time) => `+${s((Date.parse(time) - Date.parse(report.run.start)) / 1000)}`;
const cell = (value) => String(value).replaceAll("|", "\\|").replaceAll("\n", " ");
const table = (head, rows) =>
  [
    `| ${head.join(" | ")} |`,
    `| ${head.map(() => "---").join(" | ")} |`,
    ...rows.map((row) => `| ${row.map(cell).join(" | ")} |`),
  ].join("\n");

/** report.md: the same figures as report.json, laid out for a reader. */
export function renderMarkdown(report) {
  const { run, host } = report;
  const phaseOf = new Map(report.phases.map((p) => [p.id, p.recipe]));
  const out = [
    `# Gate profile: \`just ${report.recipe}\``,
    "",
    `Commit \`${report.commit.sha ?? "unknown"}\`${report.commit.dirty ? " with uncommitted changes" : ""}, profiled ${run.start}. ` +
      `Host: ${host.nproc} CPUs. Every load is the mean 1-minute load average over that row's own interval; ` +
      `CPU is user + system time of the row's whole process tree.`,
    "",
    "## Whole run",
    "",
    table(
      ["wall", "CPU", "user", "system", "CPU/wall", "load (min–max)", "exit"],
      [
        [
          s(run.wall_s),
          s(run.cpu_s),
          s(run.user_s),
          s(run.system_s),
          ratio(run.cpu_s, run.wall_s),
          `${run.load} (${run.load_min}–${run.load_max})`,
          run.exit,
        ],
      ],
    ),
    "",
    `Outside every phase (the wrappers around them): ${s(run.outside_phases.wall_s)} wall, ${s(run.outside_phases.cpu_s)} CPU.`,
    "",
    `Delegated: ${s(run.delegated_cpu_s)} more CPU was spent for the run's tasks outside its process tree, by a daemon such as sccache's server, and is in none of the CPU figures above or below; each task's share is its delegated CPU.`,
    "",
    "## Phases",
    "",
    table(
      ["#", "recipe", "within", "start", "wall", "CPU", "CPU/wall", "load", "Nx runs", "exit"],
      report.phases.map((p, i) => [
        i + 1,
        p.recipe,
        p.within.join(" › ") || "–",
        at(report, p.start),
        s(p.wall_s),
        s(p.cpu_s),
        ratio(p.cpu_s, p.wall_s),
        p.load,
        p.invocations.length,
        p.exit,
      ]),
    ),
    "",
    "## Critical path",
    "",
    `The chain of dependent Nx tasks that sets the total: ${s(report.critical_path.wall_s)} of the run's ${s(report.critical_path.run_wall_s)} wall` +
      `${report.critical_path.complete ? "" : " (incomplete: an invocation without its task graph counts only its longest task, and one without its task profile counts none; see the notes)"}.`,
    "",
    table(
      ["#", "task", "phase", "start", "wall", "load"],
      report.critical_path.tasks.map((t, i) => {
        const phase = report.invocations.find((v) => v.id === t.invocation)?.phase;
        return [i + 1, t.task, phaseOf.get(phase) ?? "–", at(report, t.start), s(t.wall_s), t.load];
      }),
    ),
    "",
    "## Nx tasks, slowest first",
    "",
    table(
      [
        "task",
        "phase",
        "status",
        "start",
        "end",
        "wall",
        "CPU",
        "CPU basis",
        "delegated CPU",
        "load",
      ],
      [...report.tasks]
        .sort((a, b) => b.wall_s - a.wall_s)
        .map((t) => [
          t.task,
          phaseOf.get(t.phase) ?? "–",
          t.status,
          at(report, t.start),
          at(report, t.end),
          s(t.wall_s),
          s(t.cpu_s),
          t.cpu_basis,
          s(t.delegated_cpu_s),
          t.load,
        ]),
    ),
    "",
    "CPU basis: `reaped` is exactly what the task's process trees used; `sampled` is what samples saw, which can miss a tree's last moments; `none` means no process of it was sampled (a cache replay, or one shorter than the sampling interval). Delegated CPU is what a daemon outside the run's process tree spent on the task.",
    "",
    "## Cargo invocations",
    "",
    table(
      ["#", "command", "task", "start", "wall", "CPU", "CPU/wall", "load", "units", "exit"],
      report.cargo.map((c, i) => [
        i + 1,
        `\`${c.command.join(" ")}\``,
        c.task ?? phaseOf.get(c.phase) ?? "–",
        at(report, c.start),
        s(c.wall_s),
        s(c.cpu_s),
        ratio(c.cpu_s, c.wall_s),
        c.load,
        c.units.length,
        c.exit,
      ]),
    ),
  ];
  report.cargo.forEach((c, i) => {
    if (!c.units.length) return;
    out.push("", `### Compile units of cargo invocation ${i + 1}, slowest first`, "");
    out.push(
      table(
        ["unit", "mode", "start", "wall", "load"],
        c.units.map((u) => [u.unit, u.mode, at(report, u.start), s(u.wall_s), u.load]),
      ),
    );
  });
  if (report.notes.length) out.push("", "## Notes", "", ...report.notes.map((n) => `- ${n}`));
  return `${out.join("\n")}\n`;
}

/** Fold `dir` and write report.json and report.md into it. */
export function writeReports(dir) {
  const report = foldRun(dir);
  writeFileSync(join(dir, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
  writeFileSync(join(dir, "report.md"), renderMarkdown(report));
  return report;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const [dir] = process.argv.slice(2);
  if (!dir || !existsSync(join(dir, "meta.json"))) {
    console.error(
      "gate-profile: usage: fold.mjs <run dir>, a target/gate-profile/<timestamp> directory",
    );
    process.exit(2);
  }
  try {
    writeReports(dir);
    console.log(`gate-profile: folded ${join(dir, "report.md")}`);
  } catch (error) {
    // A record the checks above let through can still fail the fold; that is exit 3 as well.
    console.error(`gate-profile: ${error instanceof RecordError ? error.message : error.stack}`);
    console.error(
      `gate-profile: the run in ${basename(dir)} is incomplete; profile again with \`just gate-profile\``,
    );
    process.exit(3);
  }
}

export { RecordError };
