#!/usr/bin/env node
// Measurement only (spike-git-reads, never lands): fold hosted demonstration jobs
// into this spike's comparison — each side's runs, their e2e minutes, the git
// starts the launch trace saw, what passed, and what each run read from or wrote
// to a cache.
//
//   node scripts/git-reads-compare.mjs compare baseline=DIR native=DIR ...
//
// Each DIR holds one directory per hosted run attempt, each holding `test-linux`,
// `test-macos` and `test-windows` job directories as `spikes/git-reads/fetch-run.sh`
// writes them: the unzipped test-profile artifact, `job.log` and `meta.json`. The first
// side is the before side every other side is compared with.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { jobFacts, joinTrace, largestRun, readJob, Refusal } from "./launch-inventory.mjs";

// biome-ignore lint/suspicious/noControlCharactersInRegex: job logs contain ANSI escapes.
const ANSI = /\x1b\[[0-9;]*m/g;
const PLATFORMS = ["linux", "macos", "windows"];
/** Share of non-release `main` commits whose changes include `src/` (the test-cost spike's P30: 27 of 30). */
export const TYPICAL_SHARE = 27 / 30;

/**
 * The onevcs or engine function that asks each read, by the shape of its arguments:
 * the call site a git start is attributed to, since every onevcs start is spawned at
 * one site (`git::bounded`). Unlisted shapes are named by their subcommand.
 */
const SHAPES = [
  [/^show-ref --verify --quiet /, "show-ref --verify", "onevcs git::ref_exists"],
  [/^symbolic-ref --short refs\//, "symbolic-ref --short", "onevcs git::tracked_head"],
  [
    /^symbolic-ref (-q|--quiet)/,
    "symbolic-ref -q",
    "onevcs git::carry_remote_refs/checked_out/detached_at/default_branch",
  ],
  [/^rev-parse --abbrev-ref HEAD$/, "rev-parse --abbrev-ref HEAD", "onevcs git::current_branch"],
  [/^rev-parse --is-inside-work-tree$/, "rev-parse --is-inside-work-tree", "onevcs git::is_repo"],
  [/^cat-file -e /, "cat-file -e", "onevcs git::has_commit"],
  [
    /^cat-file --batch-check/,
    "cat-file --batch-check (batch reader)",
    "onevcs spike_reads::batched::start",
  ],
  [
    /^rev-parse --verify (-q |--quiet )?\S+\^\{commit\}$/,
    "rev-parse --verify NAME^{commit}",
    "onevcs git::tip/commit_of/merge_in_progress; engine vcs::worktree_head",
  ],
  [
    /^rev-parse --verify \S+\^\{tree\}$/,
    "rev-parse --verify NAME^{tree}",
    "onevcs git::tree_of/already_integrates",
  ],
  [
    /^rev-parse --verify (-q |--quiet )?\S+$/,
    "rev-parse --verify NAME",
    "onevcs git (other --verify reads)",
  ],
  [
    /^rev-parse --git-path /,
    "rev-parse --git-path",
    "onevcs git::git_owned_path; engine destination::git_path",
  ],
  [/^rev-parse --git-common-dir$/, "rev-parse --git-common-dir", "onevcs git::common_dir"],
  [/^rev-parse HEAD$/, "rev-parse HEAD", "onevcs git::head_sha"],
  [/^remote get-url /, "remote get-url", "onevcs git::remote_url/has_remote"],
  [/^config --get /, "config --get", "onevcs git::configured_hooks/worktree_hooks"],
  [/^merge-base --is-ancestor /, "merge-base --is-ancestor", "onevcs git::is_ancestor"],
  [/^merge-base /, "merge-base", "onevcs git::merge_base"],
  [/^diff /, "diff", "onevcs git::changed_paths/line_change/trees_differ"],
  [/^log /, "log", "onevcs git::log_messages/first_parent_line"],
  [/^for-each-ref /, "for-each-ref", "onevcs git::branches/heads"],
];

/** A git start's arguments past any `--git-dir DIR` or `-c k=v` this crate put first. */
function gitArgs(argv) {
  let args = argv;
  while (args.length) {
    if (args[0] === "--git-dir" || args[0] === "-c" || args[0] === "-C") args = args.slice(2);
    else break;
  }
  return args;
}

/** `{ shape, site }` for one git start. */
export function classify(argv) {
  const text = gitArgs(argv).join(" ");
  for (const [pattern, shape, site] of SHAPES) if (pattern.test(text)) return { shape, site };
  const sub = gitArgs(argv)[0] ?? "(none)";
  return { shape: `${sub} (other)`, site: "-" };
}

/** Per-test outcomes of the e2e nextest run in `log`: final status lines, as nextest prints them. */
export function outcomes(log, run) {
  const failed = new Set();
  const retried = new Set();
  const passedLines = new Set();
  if (!log || !run) return { failed, retried, passedLines };
  let inside = false;
  for (const raw of log.replace(ANSI, "").split(/\r?\n/)) {
    const line = raw.replace(/^\S+Z\s?/, "");
    if (line.includes(`Nextest run ID ${run.id}`)) inside = true;
    if (!inside) continue;
    const m =
      /^\s*(TRY \d+ )?(PASS|FAIL|FLAKY|TIMEOUT|SIGSEGV|SIGABRT|SIGKILL|ABORT|LEAK-FAIL)\s+\[[^\]]*\]\s+(?:\([^)]*\)\s+)?onepipeline::e2e\s+(\S+)/.exec(
        line,
      );
    if (m) {
      const [, attempt, status, name] = m;
      if (status === "PASS") {
        passedLines.add(name);
        failed.delete(name);
      } else if (status === "FLAKY") {
        retried.add(name);
        passedLines.add(name);
        failed.delete(name);
      } else {
        if (attempt) retried.add(name);
        failed.add(name);
      }
    }
    if (/^\s*Summary \[/.test(line)) break;
  }
  for (const name of retried) if (passedLines.has(name)) failed.delete(name);
  return { failed, retried, passedLines };
}

/** The full commit id `actions/checkout` printed for the merge it checked out (GITHUB_SHA). */
export function githubSha(log) {
  const lines = (log ?? "").replace(ANSI, "").split(/\r?\n/);
  for (let i = 0; i < lines.length - 1; i++)
    if (/git(\.exe)?"? log -1 --format=%H\s*$/.test(lines[i])) {
      const sha = /\b([0-9a-f]{40})\s*$/.exec(lines[i + 1]);
      if (sha) return sha[1];
    }
  return null;
}

/** The merged line coverage `coverage-merge.mjs` printed, or null where it never ran. */
export function mergedCoverage(log) {
  const m = /lines percent: ([\d.]+)/.exec((log ?? "").replace(ANSI, ""));
  return m ? Number(m[1]) : null;
}

const minutes = (seconds) => seconds / 60;

/** Everything the comparison needs from one job directory. */
export function summarizeJob(label, dir) {
  const metaFile = join(dir, "meta.json");
  const meta = existsSync(metaFile) ? JSON.parse(readFileSync(metaFile, "utf8")) : {};
  const job = readJob(label, dir);
  const log = job.log ?? "";
  const facts = log ? jobFacts(log) : null;
  const run = largestRun(log);
  const { failed, retried, passedLines } = outcomes(log, run);
  const names = job.tests.map((t) => t.test).sort();
  const passed = names.filter((n) => passedLines.has(n) && !failed.has(n));
  const trace = joinTrace(job.trace);
  const verbOf = new Map(
    trace.procs.map((r) => [
      trace.key(r.file, r.pid, r.instance),
      /^e2e-|^smoke-/.test(r.exe ?? "") ? "(test process)" : `${r.exe} ${r.verb ?? ""}`.trim(),
    ]),
  );
  const git = { starts: 0, seconds: 0, byVerb: {}, byShape: {} };
  for (const r of trace.starts) {
    if (r.program !== "git") continue;
    const s = (r.exit_ns ?? 0) / 1e9;
    const verb = verbOf.get(trace.key(r.file, r.pid, r.instance)) ?? "(unknown)";
    const { shape, site } = classify(r.argv ?? []);
    git.starts++;
    git.seconds += s;
    git.byVerb[verb] ??= { starts: 0, seconds: 0 };
    const v = git.byVerb[verb];
    v.starts++;
    v.seconds += s;
    git.byShape[shape] ??= { starts: 0, seconds: 0, site };
    const k = git.byShape[shape];
    k.starts++;
    k.seconds += s;
  }
  const jobStart = meta.started_at ? Date.parse(meta.started_at) / 1000 : facts?.job.start;
  const jobEnd = meta.completed_at ? Date.parse(meta.completed_at) / 1000 : facts?.job.end;
  return {
    label,
    platform:
      job.host?.platform ??
      (/macos/.test(label) ? "macos" : /windows/.test(label) ? "windows" : "linux"),
    run: meta.run_id ?? null,
    attempt: meta.attempt ?? null,
    jobId: meta.job_id ?? null,
    headSha: meta.head_sha ?? facts?.checkout?.head ?? null,
    workflowBlob: meta.workflow_blob ?? null,
    githubSha: githubSha(log),
    cpu: job.host?.cpu_model?.trim() ?? null,
    cpus: job.host?.cpus ?? null,
    physicalCpus: job.host?.physical_cpus ?? null,
    jobMinutes: jobStart != null && jobEnd != null ? minutes(jobEnd - jobStart) : null,
    preE2eMinutes: run?.start != null && jobStart != null ? minutes(run.start - jobStart) : null,
    e2eMinutes: run ? minutes(run.seconds) : null,
    summary: run?.line ?? null,
    passed,
    failed: [...failed].sort(),
    retried: [...retried].sort(),
    coverage: mergedCoverage(log),
    selection: facts?.selection ?? null,
    cacheSteps: facts?.steps ?? [],
    cacheSaves: facts?.cacheSaves ?? [],
    nxStored: facts?.nxStored ?? null,
    git,
  };
}

/** Every job of one side, from its run-attempt directories. */
export function readSide(label, dir) {
  if (!existsSync(dir))
    throw new Refusal(
      `${label}: ${dir} does not exist`,
      "fetch the side's runs with spikes/git-reads/fetch-run.sh",
    );
  const jobs = [];
  for (const attempt of readdirSync(dir).sort()) {
    for (const platform of PLATFORMS) {
      const jobDir = join(dir, attempt, `test-${platform}`);
      if (existsSync(jobDir))
        jobs.push(summarizeJob(`${label}/${attempt}/test-${platform}`, jobDir));
    }
  }
  if (!jobs.length)
    throw new Refusal(
      `${label}: ${dir} holds no test-<platform> job directory`,
      "fetch the side's runs with spikes/git-reads/fetch-run.sh",
    );
  return { label, jobs };
}

const fmt = (x, digits = 2) => (x == null || Number.isNaN(x) ? "-" : x.toFixed(digits));
const mean = (xs) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : null);
const table = (head, rows) =>
  [
    `| ${head.join(" | ")} |`,
    `| ${head.map(() => "---").join(" | ")} |`,
    ...rows.map((r) => `| ${r.join(" | ")} |`),
  ].join("\n");

/** Whether two sides' e2e means differ by more than either side's own spread. */
export function decided(before, after) {
  const b = before.map((j) => j.e2eMinutes).filter((x) => x != null);
  const a = after.map((j) => j.e2eMinutes).filter((x) => x != null);
  if (!b.length || !a.length) return "not measured";
  const [mb, ma] = [mean(b), mean(a)];
  const spread = (xs) => Math.max(...xs) - Math.min(...xs);
  return Math.abs(mb - ma) <= Math.max(spread(b), spread(a))
    ? "undecided (inside a side's spread)"
    : "decided";
}

function renderSide(side) {
  const out = [`## ${side.label}`, ""];
  out.push(
    table(
      [
        "job",
        "run/attempt/job",
        "GITHUB_SHA",
        "head",
        "processor",
        "job min",
        "pre-e2e min (compile and earlier tiers)",
        "e2e min",
        "git starts",
        "git s",
        "passed",
        "failed",
        "re-run",
        "merged line coverage",
      ],
      side.jobs.map((j) => [
        j.label,
        `${j.run ?? "-"}/${j.attempt ?? "-"}/${j.jobId ?? "-"}`,
        j.githubSha ?? "-",
        j.headSha ?? "-",
        `${j.cpu ?? "not named"} (${j.cpus ?? "?"} logical${j.physicalCpus ? `, ${j.physicalCpus} physical` : ""})`,
        fmt(j.jobMinutes),
        fmt(j.preE2eMinutes),
        fmt(j.e2eMinutes, 3),
        String(j.git.starts),
        fmt(j.git.seconds, 1),
        String(j.passed.length),
        j.failed.length ? j.failed.join(", ") : "0",
        j.retried.length ? j.retried.join(", ") : "0",
        j.coverage == null ? "not produced" : `${j.coverage}%`,
      ]),
    ),
  );
  out.push("", "Nx selection, workflow blob, cache restores and cache writes:", "");
  for (const j of side.jobs) {
    const restores =
      j.cacheSteps.map((s) => `${s.name}: ${s.hit}, ${fmt(s.seconds, 1)} s`).join("; ") || "none";
    const writes =
      j.cacheSaves.length || j.nxStored?.entries
        ? `${j.cacheSaves.length} Actions-cache save lines; Nx stored ${j.nxStored?.entries ?? 0}`
        : "none";
    out.push(
      `- ${j.label}: workflow blob ${j.workflowBlob ?? "not recorded"}; ${j.selection ? `${j.selection.count} tasks (${j.selection.tasks.join(" ")})` : "selection not printed"}; restores: ${restores}; writes: ${writes}`,
    );
  }
  out.push("", "Passed tests by name (profile and nextest PASS both required):", "");
  for (const j of side.jobs)
    out.push(`- ${j.label}: ${j.passed.length ? j.passed.join(", ") : "none proven"}`);
  return out.join("\n");
}

function renderComparison(before, after) {
  const out = [`## ${after.label} against ${before.label}`, ""];
  const rows = [];
  for (const platform of PLATFORMS) {
    const b = before.jobs.filter((j) => j.platform === platform);
    const a = after.jobs.filter((j) => j.platform === platform);
    if (!b.length && !a.length) continue;
    const mb = mean(b.map((j) => j.e2eMinutes).filter((x) => x != null));
    const ma = mean(a.map((j) => j.e2eMinutes).filter((x) => x != null));
    const diff = mb != null && ma != null ? mb - ma : null;
    const sb = mean(b.map((j) => j.git.starts));
    const sa = mean(a.map((j) => j.git.starts));
    const gb = mean(b.map((j) => j.git.seconds));
    const ga = mean(a.map((j) => j.git.seconds));
    rows.push([
      platform,
      `${b.length}/${a.length}`,
      fmt(mb, 3),
      fmt(ma, 3),
      fmt(diff, 3),
      decided(b, a),
      diff == null ? "-" : fmt(diff * TYPICAL_SHARE, 2),
      fmt(sb, 0),
      fmt(sa, 0),
      fmt(gb, 1),
      fmt(ga, 1),
    ]);
  }
  out.push(
    table(
      [
        "platform",
        "runs before/after",
        "e2e mean min before",
        "e2e mean min after",
        "saved min (before − after)",
        "difference",
        `expected min saved per typical PR (× ${fmt(TYPICAL_SHARE, 2)})`,
        "git starts before",
        "git starts after",
        "git s before",
        "git s after",
      ],
      rows,
    ),
  );
  for (const platform of PLATFORMS) {
    const b = before.jobs.filter((j) => j.platform === platform);
    const a = after.jobs.filter((j) => j.platform === platform);
    if (!b.length || !a.length) continue;
    out.push(
      "",
      `### ${platform}: git starts by read shape (mean per e2e tier) and the call site asking it`,
      "",
    );
    const shapes = new Set([...b, ...a].flatMap((j) => Object.keys(j.git.byShape)));
    const shapeRows = [...shapes]
      .map((shape) => {
        const per = (jobs) => mean(jobs.map((j) => j.git.byShape[shape]?.starts ?? 0));
        const sec = (jobs) => mean(jobs.map((j) => j.git.byShape[shape]?.seconds ?? 0));
        const site = [...b, ...a].map((j) => j.git.byShape[shape]?.site).find(Boolean) ?? "-";
        const [pb, pa] = [per(b), per(a)];
        const verdict =
          pa === 0 && pb > 0 ? "removed" : pa < pb ? "reduced" : pa > pb ? "added" : "left";
        return [shape, site, fmt(pb, 0), fmt(pa, 0), fmt(sec(b), 1), fmt(sec(a), 1), verdict];
      })
      .sort((x, y) => Number(y[2]) - Number(x[2]));
    out.push(
      table(
        ["shape", "site", "starts before", "starts after", "s before", "s after", "after"],
        shapeRows,
      ),
    );
    out.push("", `### ${platform}: git starts by engine verb (mean per e2e tier)`, "");
    const verbs = new Set([...b, ...a].flatMap((j) => Object.keys(j.git.byVerb)));
    const verbRows = [...verbs]
      .map((verb) => {
        const per = (jobs) => mean(jobs.map((j) => j.git.byVerb[verb]?.starts ?? 0));
        const sec = (jobs) => mean(jobs.map((j) => j.git.byVerb[verb]?.seconds ?? 0));
        return [verb, fmt(per(b), 0), fmt(per(a), 0), fmt(sec(b), 1), fmt(sec(a), 1)];
      })
      .sort((x, y) => Number(y[1]) - Number(x[1]));
    out.push(table(["verb", "starts before", "starts after", "s before", "s after"], verbRows));
    const baselinePassed = new Set(b.flatMap((j) => j.passed));
    out.push("", `### ${platform}: tests the before side passed, on every after run`, "");
    for (const j of a) {
      const mine = new Set(j.passed);
      const missing = [...baselinePassed].filter((n) => !mine.has(n)).sort();
      out.push(
        `- ${j.label}: ${missing.length ? `${missing.length} not passed: ${missing.join(", ")}` : `all ${baselinePassed.size} passed`}; re-run on this side: ${j.retried.length ? j.retried.join(", ") : "none"}`,
      );
    }
    const beforeRetried = [...new Set(b.flatMap((j) => j.retried))].sort();
    out.push(
      `- re-run on the before side: ${beforeRetried.length ? beforeRetried.join(", ") : "none"}`,
    );
  }
  return out.join("\n");
}

/** The whole comparison, as Markdown. */
export function render(sides) {
  const [before, ...afters] = sides;
  return `${[...sides.map(renderSide), ...afters.map((after) => renderComparison(before, after))].join("\n\n")}\n`;
}

function usage(message) {
  return new Refusal(
    message,
    "run 'node scripts/git-reads-compare.mjs compare BEFORE=DIR AFTER=DIR ...'",
    2,
  );
}

export function main(argv) {
  const [verb, ...rest] = argv;
  if (verb !== "compare") throw usage(verb ? `unknown verb '${verb}'` : "name a verb: compare");
  if (rest.length < 2)
    throw usage("compare takes a before side and at least one after side, each LABEL=DIR");
  const sides = rest.map((arg) => {
    const at = arg.indexOf("=");
    if (at < 1) throw usage(`'${arg}' is not LABEL=DIR`);
    return readSide(arg.slice(0, at), arg.slice(at + 1));
  });
  return render(sides);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    process.stdout.write(main(process.argv.slice(2)));
  } catch (error) {
    const refusal =
      error instanceof Refusal
        ? error
        : new Refusal(
            `could not compare: ${error.message}`,
            "check each LABEL=DIR holds run-attempt directories of test-<platform> jobs",
          );
    process.stderr.write(`git-reads-compare: ${refusal.message}\nnext: ${refusal.next}\n`);
    process.exitCode = refusal.code;
  }
}
