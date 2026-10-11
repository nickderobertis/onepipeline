// Ask the kernel which runs' journals one process opened, through `strace`.
//
// `scripts/host-view-measure.mjs` runs one untimed read per scaling size through
// this, so its record says from the tracer, not from the view's own account, how
// many ended-journal bytes that size's read opened and which runs' journals it
// opened. `tests/budgets.rs` drives it over real processes opening real journals.

import { spawnSync } from "node:child_process";
import { readFileSync, rmSync } from "node:fs";
import { basename, dirname, resolve } from "node:path";

const JOURNAL = "events.jsonl";
// `openat`'s directory, as `-y` annotates it, and its quoted path.
const OPENAT = /^openat\((?:AT_FDCWD|\d+)<([^>]*)>, "((?:[^"\\]|\\.)*)"/;
// A read's descriptor, as `-y` annotates it with the file it names.
const READ_FD = /^\w+\(\d+<([^>]*)>/;

/**
 * The run whose journal `path` names, relative to `dir` where it is not absolute,
 * or null where it names no journal. Throws for a journal path strace escaped,
 * which this cannot read back to the file it was.
 */
function runOf(path, dir = "/") {
  if (basename(path) !== JOURNAL) return null;
  const full = resolve(dir, path);
  if (full.includes("\\"))
    throw new Error(`a journal path strace escaped cannot be read back: ${full}`);
  return basename(dirname(full));
}

// `-f` follows every thread and child; a call one thread was in when another's
// was written is split into an `<unfinished ...>` line and a `<... resumed>` one,
// each starting with the thread's id, and is joined back here before it is read.
const UNFINISHED = /^(\d+) (.*) <unfinished \.\.\.>$/;
const RESUMED = /^(\d+) <\.\.\. (\w+) resumed>(.*)$/;
// A finished call's result: a count, or -1 and its errno name.
const RESULT = /\) += (-?\d+)(?:<[^>]*>)?(?: E[A-Z0-9]+ \([^)]*\))?$/;

/**
 * Run `command` with `args` under `strace`, writing the trace to `into`, and
 * answer its stdout with the journals it reached: `opened`, a Map of run to the
 * bytes read from that run's journal, and `refused`, the runs whose journal it
 * tried and failed to open. Throws where the tracer did not run, or the command
 * did not exit 0, or a traced call cannot be read: an observation nobody made is
 * not one of nothing.
 */
export function journalOpens(command, args, into, options = {}) {
  // A trace an earlier read left at `into` is not this read's.
  rmSync(into, { force: true });
  const done = spawnSync(
    "strace",
    [
      "-f",
      "--seccomp-bpf",
      "-qq",
      "-y",
      "-e",
      "trace=openat,read,pread64",
      "-e",
      "signal=none",
      "-o",
      into,
      command,
      ...args,
    ],
    { encoding: "utf8", ...options },
  );
  if (done.error) throw new Error(`\`strace\` could not run: ${done.error.message}`);
  if (done.status !== 0)
    throw new Error(
      `\`strace ${command} ${args.join(" ")}\` exited ${done.status ?? `on ${done.signal}`}: ${done.stderr}`,
    );
  let trace;
  try {
    trace = readFileSync(into, "utf8");
  } catch (error) {
    throw new Error(`the tracer left no trace (${error.message})`);
  }
  if (!/\bopenat\(/.test(trace)) throw new Error("the tracer recorded no open");

  const opened = new Map();
  const refused = new Set();
  const pending = new Map();
  for (const raw of trace.split("\n")) {
    if (raw === "") continue;
    let line = raw;
    const unfinished = raw.match(UNFINISHED);
    if (unfinished) {
      pending.set(unfinished[1], unfinished[2]);
      continue;
    }
    const resumed = raw.match(RESUMED);
    if (resumed) {
      const head = pending.get(resumed[1]);
      pending.delete(resumed[1]);
      if (head === undefined) throw new Error(`a call resumed that never began: ${raw}`);
      line = `${resumed[1]} ${head}${resumed[3]}`;
    }
    const body = line.slice(line.indexOf(" ") + 1);
    const isOpen = body.startsWith("openat(");
    const isRead = body.startsWith("read(") || body.startsWith("pread64(");
    if (!isOpen && !isRead) continue;
    const named = body.match(isOpen ? OPENAT : READ_FD);
    if (!named) {
      if (body.includes(JOURNAL))
        throw new Error(`a traced call names a journal unreadably: ${raw}`);
      continue;
    }
    const run = isOpen ? runOf(named[2], named[1]) : runOf(named[1]);
    if (run === null) continue;
    const result = body.match(RESULT);
    if (!result) throw new Error(`a traced call on ${run}'s journal has no result: ${raw}`);
    const count = Number(result[1]);
    if (isOpen && count < 0) refused.add(run);
    else opened.set(run, (opened.get(run) ?? 0) + (isRead && count > 0 ? count : 0));
  }
  const cut = [...pending.values()].filter((call) => call.includes(JOURNAL));
  if (cut.length > 0)
    throw new Error(`a traced call on a journal never finished: ${cut.join("; ")}`);
  return { stdout: done.stdout, opened, refused };
}

/**
 * Hold what one read reached to what a steady read may: no ended run's journal
 * opened or tried, and exactly the live runs' opened. Answers the record's two
 * figures, or throws naming what the read reached instead.
 */
export function steadyOpens({ opened, refused }, ended, live, label) {
  const endedSet = new Set(ended);
  const reached = [...new Set([...opened.keys(), ...refused])].filter((run) => endedSet.has(run));
  if (reached.length > 0)
    throw new Error(
      `${label}: the read opened or tried to open ended runs' journals: ${reached.sort().join(", ")}`,
    );
  const runsOpened = [...opened.keys()].sort();
  const want = [...live].sort();
  if (runsOpened.join() !== want.join())
    throw new Error(
      `${label}: the read opened the journals of ${runsOpened.join(", ") || "no run"}, not the live runs ${want.join(", ")}`,
    );
  return {
    ended_journal_bytes_opened: [...opened]
      .filter(([run]) => endedSet.has(run))
      .reduce((sum, [, bytes]) => sum + bytes, 0),
    runs_opened: runsOpened,
  };
}
