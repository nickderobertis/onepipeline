#!/usr/bin/env node
// Measurement only (spike-git-reads, never lands): fold hosted demonstration jobs
// into this spike's comparison — each side's runs, their e2e minutes, the git
// starts the launch trace saw, what passed, and what each run read from or wrote
// to a cache.
//
//   node scripts/git-reads-compare.mjs compare baseline=DIR native=DIR ...
//
// Use `counts LABEL=DIR ...` for local instrumentation verification without elapsed comparisons.
// Exit 0: requested report produced; 1: unreadable or invalid evidence; 2: usage error.
// Each DIR holds one directory per hosted run attempt, each holding `test-linux`,
// `test-macos` and `test-windows` job directories with the unzipped test-profile artifact, timestamped `job.log` and `meta.json`. The first
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
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] This never-landing measurement labels the frozen inventory argv shapes; SHAs pin the observed callers and unrecognized shapes remain explicit, so no shipped interface or evolving ownership table is promised.
const SHAPES = [
  [
    /^var GIT_CONFIG_(SYSTEM|GLOBAL)$/,
    "var GIT_CONFIG_* (config paths)",
    "onevcs spike_reads::reuse::global_configs",
  ],
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

/** Diagnostic job-step reruns never replace the original tier's verdict. */
export function diagnosticReruns(log) {
  const results = {};
  for (const raw of (log ?? "").replace(ANSI, "").split(/\r?\n/)) {
    const line = raw.replace(/^\S+Z\s?/, "");
    const match =
      /^rerun-failed: re-run \d+ of \d+: onepipeline::e2e (\S+) (passed|FAILED|did not run)(?:.*)$/.exec(
        line,
      );
    if (!match) continue;
    const [, name, status] = match;
    results[name] ??= { passed: 0, failed: 0, unrun: 0 };
    results[name][status === "passed" ? "passed" : status === "FAILED" ? "failed" : "unrun"]++;
  }
  return results;
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
  const m = /lines percent: (\S+)/.exec((log ?? "").replace(ANSI, ""));
  if (!m) return null;
  const value = Number(m[1]);
  if (
    !/^(?:\d+(?:\.\d+)?|\.\d+)$/.test(m[1]) ||
    !Number.isFinite(value) ||
    value < 0 ||
    value > 100
  )
    throw new Refusal(
      "invalid coverage percentage in job log",
      "restore an unmodified coverage-merge log with a finite percentage from 0 to 100",
    );
  return value;
}

const minutes = (seconds) => seconds / 60;

/** GitHub's UTC metadata timestamps, including calendar validity. */
function timestamp(x) {
  return (
    typeof x === "string" &&
    /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?Z$/.test(x) &&
    Number.isFinite(Date.parse(x)) &&
    new Date(x).toISOString().slice(0, 19) === x.slice(0, 19)
  );
}

/** Validate optional metadata before it can change measured identity or timing. */
function metadata(path) {
  if (!existsSync(path)) return {};
  const value = JSON.parse(readFileSync(path, "utf8"));
  const fields = {
    run_id: (x) => Number.isSafeInteger(x) && x > 0,
    attempt: (x) => Number.isSafeInteger(x) && x > 0,
    job_id: (x) => Number.isSafeInteger(x) && x > 0,
    head_sha: (x) => typeof x === "string" && /^[0-9a-f]{40}$/.test(x),
    workflow_blob: (x) => typeof x === "string" && /^[0-9a-f]{40}$/.test(x),
    started_at: timestamp,
    completed_at: timestamp,
  };
  if (
    !value ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    Object.entries(value).some(([key, x]) => !Object.hasOwn(fields, key) || !fields[key](x)) ||
    (value.started_at &&
      value.completed_at &&
      Date.parse(value.completed_at) < Date.parse(value.started_at))
  ) {
    throw new Refusal(
      `${path}: invalid run metadata`,
      "supply positive integer ids, hexadecimal commit/blob ids and ordered ISO timestamps",
    );
  }
  return value;
}

/** Everything the comparison needs from one job directory. */
export function summarizeJob(label, dir) {
  const metaFile = join(dir, "meta.json");
  const meta = metadata(metaFile);
  const job = readJob(label, dir);
  const log = job.log ?? "";
  const hostPlatforms = { linux: "linux", darwin: "macos", win32: "windows" };
  if (job.host?.platform != null && !Object.hasOwn(hostPlatforms, job.host.platform)) {
    throw new Refusal(
      `${dir}: unsupported host platform ${JSON.stringify(job.host.platform)}`,
      "supply an artifact from Linux, macOS or Windows; preserve the producer's platform value",
    );
  }
  const facts = log ? jobFacts(log) : null;
  const run = largestRun(log);
  const { failed, retried, passedLines } = outcomes(log, run);
  const names = job.tests.map((t) => t.test).sort();
  const trace = joinTrace(job.trace);
  // Linux's default nextest profile suppresses individual PASS lines. An
  // all-passed summary proves a named test only with evidence it ran in that
  // invocation: a matching profile or a test process timestamped inside it.
  const ran = new Set(job.tests.filter((t) => t.run_id === run?.id).map((t) => t.test));
  if (run?.start != null && run?.end != null) {
    for (const p of trace.procs) {
      if (/^e2e-/.test(p.exe ?? "") && p.t / 1e9 >= run.start - 1 && p.t / 1e9 <= run.end + 1)
        ran.add(p.test);
    }
  }
  const allPassed = run != null && run.run === run.passed;
  const passed = names.filter(
    (n) => !failed.has(n) && (passedLines.has(n) || (allPassed && ran.has(n))),
  );
  const summarySupported = passed.filter((n) => !passedLines.has(n));
  const verbOf = new Map(
    trace.procs.map((r) => [
      trace.key(r.file, r.pid, r.instance),
      /^e2e-|^smoke-/.test(r.exe ?? "") ? "(test process)" : `${r.exe} ${r.verb ?? ""}`.trim(),
    ]),
  );
  const git = {
    starts: 0,
    seconds: 0,
    missingLifetimes: 0,
    failedSpawns: 0,
    failedSpawnSeconds: 0,
    byVerb: {},
    byShape: {},
    engineSites: {},
  };
  for (const r of trace.starts) {
    if (r.program !== "git") continue;
    const s = (r.exit_ns ?? 0) / 1e9;
    // Launch::spawned writes a null child when Command::spawn failed. That
    // attempt has a measured spawn-call cost, but no child lifetime to collect.
    const failedSpawn = r.spawn_ns != null && r.child === null;
    const missing = r.exit_ns == null && !failedSpawn ? 1 : 0;
    if (failedSpawn) {
      git.failedSpawns++;
      git.failedSpawnSeconds += r.spawn_ns / 1e9;
    }
    const verb = verbOf.get(trace.key(r.file, r.pid, r.instance)) ?? "(unknown)";
    const { shape, site } = classify(r.argv ?? []);
    const engineSite = /^onepipeline(?:::|_core::)/.test(r.function ?? "")
      ? ["destination::git_path", "vcs::worktree_head"].find((site) =>
          r.function.includes(`::${site}`),
        )
      : null;
    if (engineSite) {
      git.engineSites[engineSite] ??= { starts: 0, seconds: 0, missingLifetimes: 0 };
      git.engineSites[engineSite].starts++;
      git.engineSites[engineSite].seconds += s;
      git.engineSites[engineSite].missingLifetimes += missing;
    }
    git.starts++;
    git.seconds += s;
    git.missingLifetimes += missing;
    git.byVerb[verb] ??= { starts: 0, seconds: 0, missingLifetimes: 0 };
    const v = git.byVerb[verb];
    v.starts++;
    v.seconds += s;
    v.missingLifetimes += missing;
    git.byShape[shape] ??= { starts: 0, seconds: 0, missingLifetimes: 0, site };
    const k = git.byShape[shape];
    k.starts++;
    k.seconds += s;
    k.missingLifetimes += missing;
  }
  const jobStart = meta.started_at ? Date.parse(meta.started_at) / 1000 : facts?.job.start;
  const jobEnd = meta.completed_at ? Date.parse(meta.completed_at) / 1000 : facts?.job.end;
  return {
    label,
    platform:
      hostPlatforms[job.host?.platform] ??
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
    summarySupported,
    diagnosticReruns: diagnosticReruns(log),
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
      "download the side's test-profile artifacts, timestamped job logs and run metadata",
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
      "download the side's test-profile artifacts, timestamped job logs and run metadata",
    );
  return { label, jobs };
}

const fmt = (x, digits = 2) => (x == null || Number.isNaN(x) ? "-" : x.toFixed(digits));
const mean = (xs) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : null);
const gitTime = (seconds, missing) =>
  `${fmt(seconds, 1)}${missing > 0 ? ` (+${fmt(missing, 1)} unknown)` : ""}`;
const groupTime = (jobs, group) =>
  gitTime(
    mean(jobs.map((j) => group(j)?.seconds ?? 0)),
    mean(jobs.map((j) => group(j)?.missingLifetimes ?? 0)),
  );
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
  if (b.length < 2 || a.length < 2) return "undecided (fewer than two runs)";
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
        gitTime(j.git.seconds, j.git.missingLifetimes),
        String(j.passed.length),
        j.failed.length ? j.failed.join(", ") : "0",
        j.retried.length ? j.retried.join(", ") : "0",
        j.coverage == null ? "not produced" : `${j.coverage}%`,
      ]),
    ),
  );
  out.push("", "Nx selection, workflow blob, cache restores and cache writes:", "");
  for (const j of side.jobs) {
    out.push(
      `- ${j.label}: failed Git spawns: ${j.git.failedSpawns} (${fmt(j.git.failedSpawnSeconds, 6)} s in failed spawn calls)`,
    );
    out.push(`- ${j.label}: unmeasured Git lifetimes: ${j.git.missingLifetimes}`);
    const restores =
      j.cacheSteps.map((s) => `${s.name}: ${s.hit}, ${fmt(s.seconds, 1)} s`).join("; ") || "none";
    const writes =
      j.cacheSaves.length || j.nxStored?.entries
        ? `${j.cacheSaves.length} Actions-cache save lines; Nx stored ${j.nxStored?.entries ?? 0}`
        : j.nxStored == null
          ? "no Actions save lines; Nx write evidence missing"
          : "none";
    out.push(
      `- ${j.label}: workflow blob ${j.workflowBlob ?? "not recorded"}; ${j.selection ? `${j.selection.count} tasks (${j.selection.tasks.join(" ")})` : "selection not printed"}; restores: ${restores}; writes: ${writes}`,
    );
  }
  out.push(
    "",
    "Passed tests by name (profile plus nextest PASS, or invocation evidence plus all-passed summary):",
    "",
  );
  for (const j of side.jobs)
    out.push(
      `- ${j.label} (${j.summarySupported.length} summary-supported): ${j.passed.length ? j.passed.join(", ") : "none proven"}`,
    );
  out.push("", "Diagnostic job-step re-runs (the original tier verdict is retained):", "");
  for (const j of side.jobs) {
    const names = Object.entries(j.diagnosticReruns).map(
      ([name, counts]) =>
        `${name}: ${counts.passed} passed, ${counts.failed} failed, ${counts.unrun} unrun`,
    );
    out.push(`- ${j.label}: diagnostic re-runs: ${names.length ? names.join("; ") : "none"}`);
  }
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
      gitTime(gb, mean(b.map((j) => j.git.missingLifetimes))),
      gitTime(ga, mean(a.map((j) => j.git.missingLifetimes))),
    ]);
  }
  out.push(
    table(
      [
        "platform",
        "runs before/after",
        "e2e mean min before",
        "e2e mean min after",
        "mean difference min (before − after)",
        "difference",
        `PR-scaled mean difference (undecided is not a saving) (× ${fmt(TYPICAL_SHARE, 2)})`,
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
        const sec = (jobs) => groupTime(jobs, (j) => j.git.byShape[shape]);
        const site = [...b, ...a].map((j) => j.git.byShape[shape]?.site).find(Boolean) ?? "-";
        const [pb, pa] = [per(b), per(a)];
        const verdict =
          pa === 0 && pb > 0 ? "removed" : pa < pb ? "reduced" : pa > pb ? "added" : "left";
        return [shape, site, fmt(pb, 0), fmt(pa, 0), sec(b), sec(a), verdict];
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
        const sec = (jobs) => groupTime(jobs, (j) => j.git.byVerb[verb]);
        return [verb, fmt(per(b), 0), fmt(per(a), 0), sec(b), sec(a)];
      })
      .sort((x, y) => Number(y[1]) - Number(x[1]));
    out.push(table(["verb", "starts before", "starts after", "s before", "s after"], verbRows));
    out.push(
      "",
      `### ${platform}: engine-owned read sites (separate API adoption scope)`,
      "",
      table(
        ["site", "starts before", "starts after", "s before", "s after"],
        ["destination::git_path", "vcs::worktree_head"].map((site) => {
          const per = (jobs) => mean(jobs.map((j) => j.git.engineSites[site]?.starts ?? 0));
          const sec = (jobs) => groupTime(jobs, (j) => j.git.engineSites[site]);
          return [site, fmt(per(b), 0), fmt(per(a), 0), sec(b), sec(a)];
        }),
      ),
    );
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
  const note =
    "Git starts count the frozen inventory's launch attempts, including failed spawns (reported separately with their spawn-call cost). Git seconds sum recorded child lifetimes only; (+N unknown) counts launches without an exit duration after explicit failed spawns are removed, never zero-cost children.";
  return `${[note, ...sides.map(renderSide), ...afters.map((after) => renderComparison(before, after))].join("\n\n")}\n`;
}

/** Local instrumentation verification deliberately omits elapsed comparisons. */
export function renderCounts(sides) {
  const jobs = sides.flatMap((side) => side.jobs.map((job) => ({ side: side.label, job })));
  const out = [
    "## Launch counts",
    "",
    table(
      ["side", "platform", "job", "git starts", "passed", "failed"],
      jobs.map(({ side, job }) => [
        side,
        job.platform,
        job.label,
        String(job.git.starts),
        String(job.passed.length),
        String(job.failed.length),
      ]),
    ),
    "",
    "## By engine verb",
    "",
  ];
  out.push(
    table(
      ["side", "platform", "verb", "git starts"],
      jobs.flatMap(({ side, job }) =>
        Object.entries(job.git.byVerb).map(([verb, count]) => [
          side,
          job.platform,
          verb,
          String(count.starts),
        ]),
      ),
    ),
  );
  out.push(
    "",
    "## By read shape",
    "",
    table(
      ["side", "platform", "shape", "site", "git starts"],
      jobs.flatMap(({ side, job }) =>
        Object.entries(job.git.byShape).map(([shape, count]) => [
          side,
          job.platform,
          shape,
          count.site,
          String(count.starts),
        ]),
      ),
    ),
  );
  return `${out.join("\n")}\n`;
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
  if (verb !== "compare" && verb !== "counts")
    throw usage(verb ? `unknown verb '${verb}'` : "name a verb: compare or counts");
  if (rest.length < (verb === "counts" ? 1 : 2))
    throw usage("compare takes a before side and at least one after side, each LABEL=DIR");
  const sides = rest.map((arg) => {
    const at = arg.indexOf("=");
    if (at < 1) throw usage(`'${arg}' is not LABEL=DIR`);
    return readSide(arg.slice(0, at), arg.slice(at + 1));
  });
  return verb === "counts" ? renderCounts(sides) : render(sides);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    // llmlint: ignore[tool_output_is_signal] This never-landing comparison command is required to emit the complete Markdown measurement tables for the report; stdout is redirected to its evidence artifact, with refusals on stderr.
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
