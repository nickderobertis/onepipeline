#!/usr/bin/env node
// Read a compile-cost demonstration's run from its saved jobs listing and job logs.
//
//   node scripts/crate-split/demo-runs.mjs --dir DIR RUN_ID...
//
// DIR holds `<run>.jobs.json` (the run's jobs, read once) and `<job id>.log` (each
// job's log, read once). For each job it prints the run's `GITHUB_SHA`, the pull
// request head and the tree it measured, the executed workflow's blob id, the Rust
// cache restore (the key it matched, exactly or by restore key, or none; its size; the
// step's seconds), whether any line of the log saves a cache, and what
// `measure-archive.mjs` measured. The measurement workflow runs no Nx task, so the Nx
// selection is none. It reads only DIR.
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

// biome-ignore lint/suspicious/noControlCharactersInRegex: an Actions log's colour codes are ESC sequences.
const ANSI = /\x1b\[[0-9;]*[A-Za-z]/g;
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
  return next ? Math.round((Date.parse(next[1]) - Date.parse(lines[at][1])) / 1000) : null;
}

/** The seconds of cargo's `Finished `test` profile ... in [Mm ]S[.s]s` line. */
export function cargoBuild(text) {
  const m = text.match(/Finished `test` profile \[[^\]]*\] target\(s\) in (?:(\d+)m )?([\d.]+)s/);
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
    /measure-archive: mode=(\S+)(?: archive=failed)? wall_s=([\d.]+) units=(\d+) unit_s=([\d.]+) workspace_units=(\d+) workspace_unit_s=([\d.]+) dependency_unit_s=([\d.]+)/,
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
    wallSeconds: measured ? Number(measured[2]) : null,
    unitSeconds: measured ? Number(measured[4]) : null,
    workspaceUnitSeconds: measured ? Number(measured[6]) : null,
    dependencyUnitSeconds: measured ? Number(measured[7]) : null,
  };
}

function main(argv) {
  const at = argv.indexOf("--dir");
  if (at < 0 || !argv[at + 1])
    throw new Error("--dir names the directory the run's jobs and logs were saved in");
  const dir = argv[at + 1];
  const runs = argv.filter((_, i) => i !== at && i !== at + 1);
  console.log(
    "| run | job | job id | GITHUB_SHA | head | workflow blob | Rust cache restore | MB | restore s | cache saves | archive | cargo build s | wall s | unit s (workspace / deps) |",
  );
  console.log(
    "| --- | --- | --- | --- | --- | --- | --- | ---: | ---: | ---: | --- | ---: | ---: | --- |",
  );
  for (const run of runs) {
    const { jobs } = JSON.parse(readFileSync(join(dir, `${run}.jobs.json`), "utf8"));
    for (const job of jobs.sort((a, b) => a.name.localeCompare(b.name))) {
      const path = join(dir, `${job.id}.log`);
      if (!existsSync(path)) {
        console.log(
          `| ${run} | ${job.name} | ${job.id} | not read: ${job.status} when the jobs were listed | | | | | | | | | | |`,
        );
        continue;
      }
      const r = readJob(job, readFileSync(path, "utf8"));
      console.log(
        `| ${run} | ${r.job} | ${r.id} | \`${r.sha?.slice(0, 12)}\` | \`${r.head?.slice(0, 12)}\` | \`${r.workflowBlob?.slice(0, 12)}\` | ${r.restore} | ${r.restoreMB ?? "-"} | ${r.restoreSeconds ?? "-"} | ${r.saves} | ${r.mode ?? "failed after the build"} | ${r.cargoSeconds ?? "-"} | ${r.wallSeconds ?? "-"} | ${r.workspaceUnitSeconds ?? "-"} / ${r.dependencyUnitSeconds ?? "-"} |`,
      );
    }
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`demo-runs: ${error.message.split("\n")[0]}`);
    process.exitCode = 1;
  }
}
