#!/usr/bin/env node
// Read a compile-cost demonstration's run from its saved jobs listing and job logs.
//
//   node scripts/crate-split/demo-runs.mjs --dir DIR --out FILE RUN_ID...
//
// DIR holds `<run>.jobs.json` (the run's jobs, read once) and `<job id>.log` (each
// job's log, read once). For each job it prints the run's `GITHUB_SHA`, the pull
// request head and the tree it measured, the executed workflow's blob id, the Rust
// cache restore (the key it matched, exactly or by restore key, or none; its size; the
// step's seconds), whether any line of the log saves a cache, and what
// `measure-archive.mjs` measured. The measurement workflow runs no Nx task, so the Nx
// selection is none. It reads only DIR, and writes the table to FILE.
//
// Exit 0: FILE is written; one line counts the jobs read and the cache-save lines seen.
// 1: an argument or a saved listing was refused, with a `next:` action on stderr.
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import * as check from "./validate.mjs";

// biome-ignore lint/suspicious/noControlCharactersInRegex: an Actions log's colour codes are ESC sequences.
const ANSI = /\x1b\[[0-9;]*[A-Za-z]/g;
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] a supplementary signal over the save messages the cache actions print (actions/cache "Cache saved", Swatinem/rust-cache "Saving cache", the Nx sidecar's PUT); the proof that a run wrote no cache is the read-only listing of the caches its ref holds, which a producer whose message changed cannot hide from.
const SAVES =
  /(Saving cache|Cache saved|Uploading cache|Successfully saved cache|nx-remote-cache|PUT \/v1\/cache)/;

/** A step's seconds from the log: its `##[group]Run` line to the next step's. */
export function logStep(text, start) {
  const lines = text
    .split("\n")
    .map((l) => l.match(/^(\d{4}-\d\d-\d\dT[\d:.]+Z) (.*)$/))
    .filter(Boolean);
  const at = lines.findIndex((m) => start.test(m[2]));
  if (at < 0) return null;
  const next = lines.slice(at + 1).find((m) => m[2].startsWith("##[group]Run "));
  if (!next) return null;
  const seconds = (Date.parse(next[1]) - Date.parse(lines[at][1])) / 1000;
  // A stamp that is no time, or steps out of order, measure nothing.
  return Number.isFinite(seconds) && seconds >= 0 ? Math.round(seconds) : null;
}

/** The seconds of cargo's `Finished `test` profile ... in [Mm ]S[.s]s` line. */
export function cargoBuild(text) {
  const m = text.match(
    /Finished `test` profile \[[^\]]*\] target\(s\) in (?:(\d+)m )?(\d+(?:\.\d+)?)s/,
  );
  return m ? Number(m[1] ?? 0) * 60 + Number(m[2]) : null;
}

export function readJob(job, log) {
  const text = log.replace(ANSI, "");
  const tree = text.match(/GITHUB_SHA=(\w+) head=(\w+) tree=(\w+)/);
  const restore =
    text.match(/Cache restored from key: (\S+)/)?.[1] ??
    text.match(/Cache hit for: (\S+)/)?.[1] ??
    (text.match(/Cache hit for restore-key: (\S+)/)
      ? `restore-key ${text.match(/Cache hit for restore-key: (\S+)/)[1]}`
      : null);
  const size = text.match(/Cache Size: ~(\d+) MB/)?.[1];
  const step = job.steps.find((s) => /rust-cache/.test(s.name));
  const measured = text.match(
    /measure-archive: mode=(\S+)( archive=failed)? wall_s=(\d+(?:\.\d+)?) units=(\d+) unit_s=(\d+(?:\.\d+)?) workspace_units=(\d+) workspace_unit_s=(\d+(?:\.\d+)?) dependency_unit_s=(-?\d+(?:\.\d+)?)/,
  );
  return {
    job: job.name,
    id: job.id,
    conclusion: job.conclusion,
    sha: tree?.[1] ?? null,
    head: tree?.[2] ?? null,
    tree: tree?.[3] ?? null,
    workflowBlob: text.match(/workflow blob=(\w+)/)?.[1] ?? null,
    restore: restore ?? (/No cache found|Cache not found/.test(text) ? "miss" : "unknown"),
    restoreMB: size ? Number(size) : null,
    restoreSeconds: step?.completed_at
      ? (Date.parse(step.completed_at) - Date.parse(step.started_at)) / 1000
      : logStep(text, /^##\[group\]Run Swatinem\/rust-cache/),
    saves: text.split("\n").filter((l) => SAVES.test(l)).length,
    // cargo's own build time: the one figure every layout reports, whatever nextest
    // then does with what was built.
    cargoSeconds: cargoBuild(text),
    mode: measured?.[1] ?? null,
    archived: measured ? !measured[2] : null,
    wallSeconds: measured ? Number(measured[3]) : null,
    unitSeconds: measured ? Number(measured[5]) : null,
    workspaceUnitSeconds: measured ? Number(measured[7]) : null,
    dependencyUnitSeconds: measured ? Number(measured[8]) : null,
  };
}

const USAGE = "node scripts/crate-split/demo-runs.mjs --dir DIR --out FILE RUN_ID...";

function main(argv) {
  const options = {};
  const runs = [];
  for (let i = 0; i < argv.length; i++) {
    if (!argv[i].startsWith("--")) {
      runs.push(argv[i]);
      continue;
    }
    const name = argv[i].slice(2);
    check.need(["dir", "out"].includes(name), `${argv[i]} is not an option this reads`, USAGE);
    check.need(!(name in options), `--${name} is given twice`, USAGE);
    check.need(
      argv[i + 1] !== undefined && !argv[i + 1].startsWith("--"),
      `--${name} needs a value`,
      USAGE,
    );
    options[name] = argv[++i];
  }
  check.need(options.dir, "--dir names the directory the run's jobs and logs were saved in", USAGE);
  check.need(options.out, "--out names the file the table is written to", USAGE);
  const { dir, out } = options;
  check.need(runs.length > 0, "name at least one run", USAGE);
  for (const run of runs) check.runId(run);
  const lines = [
    "| run | job | job id | GITHUB_SHA | head | workflow blob | Rust cache restore | MB | restore s | cache saves | archive | cargo build s | wall s | unit s (workspace / deps) |",
    "| --- | --- | --- | --- | --- | --- | --- | ---: | ---: | ---: | --- | ---: | ---: | --- |",
  ];
  let read = 0;
  let unread = 0;
  let saves = 0;
  for (const run of runs) {
    const listing = join(dir, `${run}.jobs.json`);
    let doc;
    try {
      doc = JSON.parse(readFileSync(listing, "utf8"));
    } catch (error) {
      throw new check.Refusal(
        `${listing} cannot be read as JSON: ${error.message}`,
        `save the run's jobs: gh api repos/OWNER/NAME/actions/runs/${run}/jobs > ${listing}`,
      );
    }
    for (const job of check.jobsListing(doc, run).sort((a, b) => a.name.localeCompare(b.name))) {
      const path = join(dir, `${job.id}.log`);
      if (!existsSync(path)) {
        unread++;
        lines.push(
          `| ${run} | ${job.name} | ${job.id} | not read: ${job.status} when the jobs were listed | | | | | | | | | | |`,
        );
        continue;
      }
      read++;
      const r = readJob(job, readFileSync(path, "utf8"));
      saves += r.saves;
      const code = (v) => (v ? `\`${v.slice(0, 12)}\`` : "-");
      // When nextest refuses the archive after cargo finished, the row's one figure is
      // cargo's compile and link of the test binaries: it measures no archive.
      const refused = "archive refused: compile-only figure";
      const archive =
        (r.mode && (r.archived ? r.mode : `${r.mode}, ${refused}`)) ??
        (r.cargoSeconds === null ? "no measurement" : refused);
      lines.push(
        `| ${run} | ${r.job} | ${r.id} | ${code(r.sha)} | ${code(r.head)} | ${code(r.workflowBlob)} | ${r.restore} | ${r.restoreMB ?? "-"} | ${r.restoreSeconds ?? "-"} | ${r.saves} | ${archive} | ${r.cargoSeconds ?? "-"} | ${r.wallSeconds ?? "-"} | ${r.workspaceUnitSeconds ?? "-"} / ${r.dependencyUnitSeconds ?? "-"} |`,
      );
    }
  }
  writeFileSync(out, `${lines.join("\n")}\n`);
  console.log(
    `demo-runs: wrote ${out}: ${read} jobs read and ${unread} not, of ${runs.length} runs; ${saves} cache-save lines`,
  );
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`demo-runs: ${error.message.split("\n")[0]}`);
    console.error(`next: ${error instanceof check.Refusal ? error.next : USAGE}`);
    process.exitCode = 1;
  }
}
