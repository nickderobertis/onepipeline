// Exit codes: 0 report produced, 2 invalid input (repair printed on stderr).
// Measurement-only engine-startup spike; the inventory owns trace validation and scope.
import { pathToFileURL } from "node:url";
import { foldJob, joinTrace, readJob } from "./launch-inventory.mjs";

function intervalUnion(intervals, from, to) {
  const clipped = intervals
    .map(([a, b]) => [Math.max(a, from), Math.min(b, to)])
    .filter(([a, b]) => b > a)
    .sort((a, b) => a[0] - b[0]);
  let total = 0;
  let end = from;
  for (const [a, b] of clipped) {
    if (b > end) {
      total += b - Math.max(a, end);
      end = b;
    }
  }
  return total;
}

function localPhases(label, dir) {
  const job = readJob(label, dir);
  const trace = joinTrace(job.trace);
  const lines = [`${label}: local trace intervals (not a hosted wall-clock estimate)`];
  for (const proc of trace.procs.filter(
    (p) => p.exe === "onepipeline" && ["start", "adopt"].includes(p.verb) && !p.parentStart,
  )) {
    const key = trace.key(proc.file, proc.pid, proc.instance);
    const end = trace.ends.get(key);
    if (end == null) {
      lines.push(`${proc.verb} pid ${proc.pid}: unfinished; partition not measured`);
      continue;
    }
    if (end < proc.t)
      throw new Error(
        "trace process clock moved backwards; record the journey again into a fresh directory",
      );
    const starts = trace.startsOf.get(key) ?? [];
    const creation = starts.filter((s) => s.spawn_ns != null).map((s) => [s.t, s.t + s.spawn_ns]);
    const operations = starts.filter((s) => s.dur != null).map((s) => [s.t, s.t + s.dur]);
    const span = end - proc.t;
    const creating = intervalUnion(creation, proc.t, end);
    const known = intervalUnion([...creation, ...operations], proc.t, end);
    const ms = (ns) => (ns / 1e6).toFixed(3);
    // llmlint: ignore-block[tool_output_is_signal] These requested never-land trace/CPU accounting rows must retain each bucket and uncertainty; a one-line mutation success message cannot carry the measured result (manager a1).
    lines.push(
      `${proc.verb} pid ${proc.pid}: trace window ${ms(span)} ms; ${starts.filter((s) => s.dur != null).length} timed / ${starts.length} observed child or opaque operations`,
    );
    lines.push(
      `partition: spawn-call ${ms(creating)} ms + operation-live outside spawn ${ms(known - creating)} ms + unallocated ${ms(span - known)} ms = trace window ${ms(span)} ms`,
    );
    const programs = new Map();
    for (const s of starts) {
      if (!programs.has(s.program))
        programs.set(s.program, { count: 0, timed: 0, ns: 0, spawn: 0 });
      const program = programs.get(s.program);
      program.count++;
      if (s.dur != null) {
        program.timed++;
        program.ns += intervalUnion([[s.t, s.t + s.dur]], proc.t, end);
      }
      if (s.spawn_ns != null)
        program.spawn += intervalUnion([[s.t, s.t + s.spawn_ns]], proc.t, end);
    }
    for (const [program, value] of programs)
      lines.push(
        `  ${program}: ${value.timed}/${value.count} timed; inclusive operation ${ms(value.ns)} ms; spawn-call ${ms(value.spawn)} ms (program rows can overlap)`,
      );
    const waits = trace.waits.filter((w) => trace.key(w.file, w.pid, w.instance) === key);
    lines.push(
      `recorded child-wait loops: ${ms(waits.reduce((sum, w) => sum + (w.wait_ns ?? 0), 0))} ms over ${waits.length} records (overlays, not an additive partition)`,
    );
    // llmlint: ignore-end[tool_output_is_signal]
  }
  lines.push(
    "Unallocated wall includes own CPU, filesystem latency, descheduling, non-child waits and unmeasured children, not separately measured. Operation-live means observed child/opaque-call presence, not proof the engine was waiting. Separately sampled engine CPU overlaps all timeline buckets and must not be added to them. Untimed operations remain uncertainty; clocks exclude creation of the engine, pre-first-trace initialization and teardown.",
  );
  return `${lines.join("\n")}\n`;
}

export function main(args) {
  if (!["fold", "phases"].includes(args[0]) || args.length < 2)
    throw new Error(
      "use: node scripts/engine-startup.mjs fold|phases LABEL=UNPACKED_JOB [...]; unzip each job's test-profile artifact and timestamped job.log together",
    );
  if (args[0] === "phases")
    return args
      .slice(1)
      .map((arg) => {
        const at = arg.indexOf("=");
        if (at < 1 || at === arg.length - 1)
          throw new Error("expected LABEL=UNPACKED_JOB; pass e.g. local=/path/to/unpacked/profile");
        return localPhases(arg.slice(0, at), arg.slice(at + 1));
      })
      .join("\n");
  const folds = [];
  const rows = args.slice(1).map((arg) => {
    const at = arg.indexOf("=");
    if (at < 1 || at === arg.length - 1)
      throw new Error(
        `expected LABEL=UNPACKED_JOB, got ${arg}; unzip a job artifact and pass e.g. before/1=/path/to/job`,
      );
    const f = foldJob(readJob(arg.slice(0, at), arg.slice(at + 1)));
    folds.push({ label: f.label, os: f.os, wallS: f.wallS, names: f.names });
    // llmlint: ignore-block[tool_output_is_signal] This never-land measurement report must print each job and side, with uncertainty beside the numbers; one success line would discard the deliverable (manager a1).
    const lines = [
      `${f.label} (${f.os})`,
      `runner: ${f.host?.cpu_model ?? "not recorded"}; cores: ${f.host?.cpus ?? "not recorded"}`,
      `e2e minutes: ${f.wallS == null ? "not measured" : (f.wallS / 60).toFixed(6)}`,
    ];
    for (const verb of ["start", "adopt"]) {
      const r = f.verbs.find((v) => v.key === `onepipeline ${verb}`);
      const n = r?.lives.length ?? 0;
      const mean = n ? r.lives.reduce((a, b) => a + b, 0) / n / 1e6 : null;
      lines.push(
        `${verb}: ${mean == null ? "not measured" : `${mean.toFixed(6)} ms mean`}; ${n} completed / ${r?.count ?? 0} observed`,
      );
    }
    lines.push(
      "clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded",
    );
    lines.push(
      `test profiles: ${f.testCount}; names digest: ${f.testsDigest}; scope: ${JSON.stringify(f.scope)}`,
    );
    return lines.join("\n");
    // llmlint: ignore-end[tool_output_is_signal]
  });
  const groups = new Map();
  for (const f of folds) {
    const side = f.label.split("/")[0];
    const key = `${side}/${f.os}`;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(f);
  }
  const mean = (values) => values.reduce((a, b) => a + b, 0) / values.length;
  // llmlint: ignore-block[tool_output_is_signal] These never-land comparison rows are the requested measurement report, not a mutation command success message (manager a1).
  for (const [key, members] of groups) {
    const minutes = members.filter((f) => f.wallS != null).map((f) => f.wallS / 60);
    if (!minutes.length) continue;
    rows.push(
      `${key}: e2e mean ${mean(minutes).toFixed(6)} min; range ${Math.min(...minutes).toFixed(6)}..${Math.max(...minutes).toFixed(6)}; n=${minutes.length}`,
    );
    const baseline = groups.get(`before/${members[0].os}`);
    if (!baseline || key.startsWith("before/")) continue;
    const baseMinutes = baseline.filter((f) => f.wallS != null).map((f) => f.wallS / 60);
    if (!baseMinutes.length) continue;
    const delta = mean(baseMinutes) - mean(minutes);
    const spread = (v) => Math.max(...v) - Math.min(...v);
    const decided = Math.abs(delta) > Math.max(spread(minutes), spread(baseMinutes));
    rows.push(
      `${key}: before-minus-after ${delta.toFixed(6)} min; ${decided ? "outside observed spreads" : "undecided (inside a side's spread)"}; typical PR estimate (27/30) ${((delta * 27) / 30).toFixed(6)} min${decided ? "" : " (undecided, not a saving)"}`,
    );
    const baselineNames = new Set(baseline.flatMap((f) => f.names));
    for (const f of members) {
      const present = new Set(f.names);
      const missing = [...baselineNames].filter((name) => !present.has(name));
      rows.push(
        `${f.label}: baseline test profiles missing ${JSON.stringify(missing)}; profile presence does not establish pass status (read nextest outcomes)`,
      );
    }
  }
  return `${rows.join("\n\n")}\n`;
  // llmlint: ignore-end[tool_output_is_signal]
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    process.stdout.write(main(process.argv.slice(2)));
  } catch (error) {
    console.error(`engine-startup: ${error.message}${error.next ? `; ${error.next}` : ""}`);
    process.exitCode = 2;
  }
}
