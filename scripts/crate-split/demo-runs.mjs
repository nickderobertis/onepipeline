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
import { readFileSync } from "node:fs";
import { join } from "node:path";

// biome-ignore lint/suspicious/noControlCharactersInRegex: an Actions log's colour codes are ESC sequences.
const ANSI = /\x1b\[[0-9;]*[A-Za-z]/g;
const SAVES =
  /(Saving cache|Cache saved|Uploading cache|Successfully saved cache|nx-remote-cache|PUT \/v1\/cache)/;

export function readJob(job, log) {
  const text = log.replace(ANSI, "");
  const tree = text.match(/GITHUB_SHA=(\w+) head=(\w+) tree=(\w+)/);
  const restore =
    text.match(/Cache restored from key: (\S+)/)?.[1] ??
    (text.match(/Cache hit for restore-key: (\S+)/)
      ? `restore-key ${text.match(/Cache hit for restore-key: (\S+)/)[1]}`
      : null);
  const size = text.match(/Cache Size: ~(\d+) MB/)?.[1];
  const step = job.steps.find((s) => /rust-cache/.test(s.name));
  const measured = text.match(
    /measure-archive: mode=(\S+) wall_s=([\d.]+) units=(\d+) unit_s=([\d.]+) workspace_units=(\d+) workspace_unit_s=([\d.]+) dependency_unit_s=([\d.]+)/,
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
    restoreSeconds: step?.started_at
      ? (Date.parse(step.completed_at) - Date.parse(step.started_at)) / 1000
      : null,
    saves: text.split("\n").filter((l) => SAVES.test(l)).length,
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
    "| run | job | job id | GITHUB_SHA | head | workflow blob | Rust cache restore | MB | restore s | cache saves | archive | wall s | unit s (workspace / deps) |",
  );
  console.log(
    "| --- | --- | --- | --- | --- | --- | --- | ---: | ---: | ---: | --- | ---: | --- |",
  );
  for (const run of runs) {
    const { jobs } = JSON.parse(readFileSync(join(dir, `${run}.jobs.json`), "utf8"));
    for (const job of jobs.sort((a, b) => a.name.localeCompare(b.name))) {
      const r = readJob(job, readFileSync(join(dir, `${job.id}.log`), "utf8"));
      console.log(
        `| ${run} | ${r.job} | ${r.id} | \`${r.sha?.slice(0, 12)}\` | \`${r.head?.slice(0, 12)}\` | \`${r.workflowBlob?.slice(0, 12)}\` | ${r.restore} | ${r.restoreMB ?? "-"} | ${r.restoreSeconds ?? "-"} | ${r.saves} | ${r.mode} | ${r.wallSeconds} | ${r.workspaceUnitSeconds} / ${r.dependencyUnitSeconds} |`,
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
