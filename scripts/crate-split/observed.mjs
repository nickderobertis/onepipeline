// The observed half of `crate-split-replay.mjs --observed`: for each pull request's last
// CI run, which Nx tasks each job replayed and ran, as its log shows, and for each task
// that ran, which input of its key the pull request's diff matched.
//
// The evidence directory is `fetch-evidence.mjs`'s. The run's tested tree is rebuilt in
// a clone of the repository (`--clone`): the merge of the head the checkout step names
// into the base it names, made with `git merge-tree`, which is the tree GitHub's merge
// ref holds for a clean merge. Nx's own hash plan of each task is read at that tree and
// at its base (`hash-plan.cjs`); a changed file is in a task's key when either plan
// lists it, a runtime input moved when its command prints differently at the two, and
// the project's configuration or the external dependencies moved when Nx's graph of
// them did. Each matched file is charged to the innermost named input of `nx.json`
// whose glob holds it, read at the tested tree, so `src/x.rs` is `rustBuild`'s and
// `tests/e2e/x.rs` is `e2eSourceWithoutDocs`'s.
//
// A task that ran with nothing matched is given the cause the run's own record shows:
// not cacheable, a remote read that missed a key another run in the evidence stored
// (evicted, or held where this run could not read it), a miss on a key no run in the
// evidence stored (absent), or no remote read at all.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, symlinkSync } from "node:fs";
import { join, resolve } from "node:path";
import { globToRegExp } from "../crate-split-replay.mjs";
import { loadRecords, moduleOf, Refusal, selectRun } from "../test-profile-report.mjs";

export const NX_JOBS = ["lint", "test-linux", "test-windows", "test-macos"];
const OS_OF = {
  lint: "linux",
  "test-linux": "linux",
  "test-windows": "win32",
  "test-macos": "darwin",
};

// biome-ignore lint/suspicious/noControlCharactersInRegex: an Actions log's colour codes are ESC sequences.
const ANSI = /\x1b\[[0-9;]*[A-Za-z]/g;
const TASK_LINE =
  /> nx run (\S+?:[^\s]+)(?:\s+\[(remote cache|local cache|existing outputs match the cache, left as is)\])?\s*$/;
const CHECKOUT_LINE = /HEAD is now at [0-9a-f]+ Merge ([0-9a-f]{40}) into ([0-9a-f]{40})/;
const SUMMARY_LINE =
  /Nx read the output from the cache instead of running the command for (\d+) out of (\d+) tasks/;

/**
 * One job's log: the merge it checked out and each Nx task line, in order. A task line
 * nested in another task's output is prefixed (`# `), and is that task's own business.
 */
export function parseLog(text) {
  const lines = text.replace(ANSI, "").split(/\r?\n/);
  let merge = null;
  const tasks = [];
  const summaries = [];
  let current = null;
  for (const raw of lines) {
    const stamp = raw.match(/^(\d{4}-\d\d-\d\dT[\d:.]+Z)\s?/);
    const line = stamp ? raw.slice(stamp[0].length) : raw;
    const checkout = line.match(CHECKOUT_LINE);
    if (checkout && !merge) merge = { head: checkout[1], base: checkout[2] };
    const body = line.replace(/^##\[group\][^>\s]*\s*/, "");
    const task = body.startsWith(">") ? body.match(TASK_LINE) : null;
    if (task) {
      current = {
        task: task[1],
        replayed: task[2] ?? null,
        at: stamp?.[1] ?? null,
        starting: null,
        summary: null,
      };
      tasks.push(current);
      continue;
    }
    const summary = line.match(SUMMARY_LINE);
    if (summary) summaries.push({ replayed: Number(summary[1]), of: Number(summary[2]) });
    // Within a task's own output: when nextest began running tests (the build before it
    // is the archive's), and the seconds its run took.
    if (current && current.starting === null && /^\s*Starting \d+ tests? across/.test(line)) {
      current.starting = stamp?.[1] ?? null;
    }
    const run = line.match(/^\s*Summary \[\s*([\d.]+)s\]/);
    if (current && run) current.summary = (current.summary ?? 0) + Number(run[1]);
  }
  return { merge, tasks, summaries };
}

function git(clone, args, env = {}) {
  return execFileSync("git", ["-C", clone, ...args], {
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    env: { ...process.env, ...env },
  });
}

/** The merge commit of `head` into `base`, made in the clone, or null on a conflict. */
export function mergeCommit(clone, base, head) {
  let tree;
  try {
    tree = git(clone, ["merge-tree", "--write-tree", base, head]).split("\n")[0].trim();
  } catch {
    return null;
  }
  const fixed = {
    GIT_AUTHOR_NAME: "replay",
    GIT_AUTHOR_EMAIL: "replay@example.invalid",
    GIT_COMMITTER_NAME: "replay",
    GIT_COMMITTER_EMAIL: "replay@example.invalid",
    GIT_AUTHOR_DATE: "2000-01-01T00:00:00Z",
    GIT_COMMITTER_DATE: "2000-01-01T00:00:00Z",
  };
  return git(
    clone,
    ["commit-tree", tree, "-p", base, "-p", head, "-m", "replay merge"],
    fixed,
  ).trim();
}

/** Nx's hash plan of `tasks` at `commit`, read once and kept under `cache`. */
export function planAt(clone, commit, tasks, cache, nodeModules) {
  const out = join(cache, `${commit}.json`);
  if (existsSync(out)) {
    const held = JSON.parse(readFileSync(out, "utf8"));
    const known = new Set([...Object.keys(held.plans), ...held.missing]);
    if (tasks.every((task) => known.has(task))) return held;
  }
  mkdirSync(cache, { recursive: true });
  git(clone, ["checkout", "-q", "-f", "--detach", commit]);
  git(clone, ["clean", "-q", "-fdx", "-e", "node_modules"]);
  if (!existsSync(join(clone, "node_modules")))
    symlinkSync(nodeModules, join(clone, "node_modules"));
  rmSync(join(clone, ".nx"), { recursive: true, force: true });
  execFileSync(process.execPath, [resolve(import.meta.dirname, "hash-plan.cjs"), out, ...tasks], {
    cwd: clone,
    stdio: ["ignore", "ignore", "pipe"],
    env: { ...process.env, NX_DAEMON: "false", NX_NO_CLOUD: "true", NX_TUI: "false" },
  });
  return JSON.parse(readFileSync(out, "utf8"));
}

/**
 * The named inputs of `nx.json` expanded to the globs each directly lists, so a file
 * can be charged to the innermost one holding it.
 */
export function attributor(namedInputs, inputs, projectRoot) {
  const direct = [];
  const seen = new Set();
  const subst = (glob) =>
    glob
      .replace("{workspaceRoot}/", "")
      .replace("{projectRoot}", projectRoot || ".")
      .replace(/^\.\//, "");
  const walk = (list, owner) => {
    for (const input of list ?? []) {
      if (typeof input !== "string") continue;
      if (input.startsWith("^")) continue;
      if (Object.hasOwn(namedInputs, input)) {
        if (seen.has(input)) continue;
        seen.add(input);
        walk(namedInputs[input], input);
      } else if (!input.startsWith("!")) {
        direct.push({ owner, pattern: globToRegExp(subst(input)) });
      }
    }
  };
  walk(inputs, "target inputs");
  return (file) => [...new Set(direct.filter((d) => d.pattern.test(file)).map((d) => d.owner))];
}

/** The key inputs a diff moved for one task, between its base and tested plans. */
export function matchedInputs(task, basePlan, mergePlan, diff, attribute) {
  const at = mergePlan.plans[task];
  const before = basePlan.plans[task];
  if (!at) return { inputs: ["task absent at the tested tree"], files: [] };
  if (!before) return { inputs: ["task new at this pull request"], files: [] };
  const keyed = new Set(
    [...at.plan, ...before.plan].filter((e) => e.startsWith("file:")).map((e) => e.slice(5)),
  );
  const files = diff.filter((file) => keyed.has(file));
  const inputs = new Set();
  for (const file of files) {
    const owners = attribute(file);
    for (const owner of owners.length ? owners : ["implicit file input"]) inputs.add(owner);
  }
  for (const entry of new Set([...at.plan, ...before.plan])) {
    if (!entry.startsWith("runtime:")) continue;
    const a = mergePlan.runtime[entry];
    const b = basePlan.runtime[entry];
    if (!a || !b || a.out !== b.out || !a.ok || !b.ok) inputs.add(entry);
  }
  if (at.config !== before.config) inputs.add("project configuration");
  if (mergePlan.external !== basePlan.external) inputs.add("external dependencies");
  return { inputs: [...inputs].sort(), files };
}

/**
 * What moved between two checkouts' plans other than files: each runtime command that
 * prints differently (`runtime:<command>`), each project whose configuration changed
 * (`config:<project>`), and the external dependencies (`external`). These are the
 * pseudo-paths an option's mapping names to say a suite is keyed on them.
 */
export function movedInputs(basePlan, mergePlan) {
  const moved = new Set();
  for (const entry of new Set([
    ...Object.keys(basePlan.runtime),
    ...Object.keys(mergePlan.runtime),
  ])) {
    const a = mergePlan.runtime[entry];
    const b = basePlan.runtime[entry];
    if (a && b && (a.out !== b.out || !a.ok || !b.ok)) moved.add(entry);
  }
  for (const [task, at] of Object.entries(mergePlan.plans)) {
    const before = basePlan.plans[task];
    if (before && before.config !== at.config)
      moved.add(`config:${task.slice(0, task.indexOf(":"))}`);
  }
  if (basePlan.external !== mergePlan.external) moved.add("external");
  return [...moved].sort();
}

const minutes = (t) => (Date.parse(t.finished_at) - Date.parse(t.started_at)) / 60000;

/** Read the evidence directory into one record per pull request and job. */
export function loadEvidence(dir) {
  const read = (path) => JSON.parse(readFileSync(join(dir, path), "utf8"));
  if (!existsSync(join(dir, "population.json"))) {
    throw new Refusal(
      `${dir} holds no population.json`,
      "run scripts/crate-split/fetch-evidence.mjs --out DIR first",
    );
  }
  const pop = read("population.json");
  const prs = [
    ...pop.non_release.map((pr) => ({ ...pr, release: false })),
    ...pop.release.map((pr) => ({ ...pr, release: true })),
  ];
  const out = [];
  for (const pr of prs) {
    if (!existsSync(join(dir, "rollup", `${pr.number}.json`))) {
      out.push({ pr, run: null, jobs: [], unread: "no check suites fetched" });
      continue;
    }
    const commit = read(join("rollup", `${pr.number}.json`)).data.repository.pullRequest.commits
      .nodes[0].commit;
    const suite = commit.checkSuites.nodes
      .filter((s) => s.workflowRun?.workflow?.name === "CI")
      .sort((a, b) => a.workflowRun.createdAt.localeCompare(b.workflowRun.createdAt))
      .at(-1);
    const jobs = [];
    for (const check of suite?.checkRuns.nodes ?? []) {
      if (!NX_JOBS.includes(check.name)) continue;
      const logPath = join(dir, "logs", `${check.databaseId}.log`);
      const execPath = join(dir, "executions", `${check.databaseId}.json`);
      jobs.push({
        name: check.name,
        id: check.databaseId,
        os: OS_OF[check.name],
        minutes: (Date.parse(check.completedAt) - Date.parse(check.startedAt)) / 60000,
        log: existsSync(logPath) ? parseLog(readFileSync(logPath, "utf8")) : null,
        executions: existsSync(execPath) ? JSON.parse(readFileSync(execPath, "utf8")) : null,
      });
    }
    out.push({ pr, run: suite?.workflowRun.databaseId ?? null, head: commit.oid, jobs });
  }
  return out;
}

/** The observed replay over the evidence: per pull request, per job, per task. */
export function observe(dir, clone, { nodeModules }) {
  const evidence = loadEvidence(dir);
  // Every key any run stored, so a later miss on it can be told from a key never made.
  const stored = new Map();
  for (const entry of evidence) {
    for (const job of entry.jobs) {
      for (const attempt of job.executions?.store_attempts ?? []) {
        if (attempt.status !== "stored") continue;
        const key = `${attempt.task}\0${job.os}\0${attempt.hash}`;
        if (!stored.has(key)) stored.set(key, []);
        stored
          .get(key)
          .push({ pr: entry.pr.number, run: entry.run, job: job.name, at: attempt.attempted_at });
      }
    }
  }
  const allTasks = new Set();
  for (const entry of evidence) {
    for (const job of entry.jobs) {
      for (const t of job.executions?.tasks ?? []) allTasks.add(t.task);
      for (const t of job.log?.tasks ?? []) allTasks.add(t.task);
    }
  }
  const tasks = [...allTasks].sort();
  const plans = join(dir, "plans");
  const perPr = [];
  for (const entry of evidence) {
    const merge = entry.jobs.map((j) => j.log?.merge).find(Boolean);
    const record = {
      pr: entry.pr.number,
      title: entry.pr.title,
      release: entry.pr.release,
      run: entry.run,
      jobs: [],
    };
    if (!merge) {
      record.unread = entry.unread ?? "no job log names the merge it checked out";
      perPr.push(record);
      continue;
    }
    const tested = mergeCommit(clone, merge.base, merge.head);
    if (!tested) {
      record.unread = `the head ${merge.head} does not merge cleanly into ${merge.base} in the clone`;
      perPr.push(record);
      continue;
    }
    record.base = merge.base;
    record.head = merge.head;
    const diff = git(clone, ["diff", "-z", "--name-only", "--no-renames", merge.base, tested])
      .split("\0")
      .filter(Boolean);
    record.changed = diff;
    const basePlan = planAt(clone, merge.base, tasks, plans, nodeModules);
    const mergePlan = planAt(clone, tested, tasks, plans, nodeModules);
    record.moved = movedInputs(basePlan, mergePlan);
    const nxJson = JSON.parse(git(clone, ["show", `${tested}:nx.json`]));
    for (const job of entry.jobs) {
      const out = {
        job: job.name,
        id: job.id,
        minutes: job.minutes,
        replayed: [],
        ran: [],
        unread: null,
      };
      if (!job.log) out.unread = "log not fetched";
      const exec = job.executions && !job.executions.absent ? job.executions : null;
      if (!exec) out.executionsUnread = job.executions?.absent ?? "executions record not fetched";
      const byTask = new Map((exec?.tasks ?? []).map((t) => [t.task, t]));
      const listed = job.log
        ? job.log.tasks
        : (exec?.tasks ?? []).map((t) => ({ task: t.task, replayed: null }));
      for (const line of listed) {
        const t = byTask.get(line.task);
        const replayed =
          line.replayed !== null || (t && (t.result === "remote_hit" || t.result === "local_hit"));
        const row = {
          task: line.task,
          log: line.replayed ?? "ran",
          minutes: t ? minutes(t) : null,
        };
        if (line.at && line.starting)
          row.buildMinutes = (Date.parse(line.starting) - Date.parse(line.at)) / 60000;
        if (line.summary !== null && line.summary !== undefined)
          row.testMinutes = line.summary / 60;
        if (replayed) {
          const m = matchedInputs(line.task, basePlan, mergePlan, diff, () => []);
          row.diffMatched = m.files.length > 0 || m.inputs.length > 0;
          out.replayed.push(row);
          continue;
        }
        const at = mergePlan.plans[line.task];
        const attribute = attributor({ ...nxJson.namedInputs }, at?.inputs ?? [], at?.root ?? "");
        const m = matchedInputs(line.task, basePlan, mergePlan, diff, attribute);
        row.inputs = m.inputs;
        row.files = m.files.slice(0, 5);
        row.fileCount = m.files.length;
        row.cacheable = t ? t.cacheable : at?.cache !== false;
        if (!row.cacheable) row.cause = "not cacheable";
        else if (m.inputs.length)
          row.cause = m.inputs.length === 1 ? "one input" : "several inputs";
        else if (!t) row.cause = "no executions record to read the key from";
        else if (t.remote_read === "miss") {
          const earlier = (stored.get(`${t.task}\0${job.os}\0${t.hash}`) ?? []).filter(
            (s) => s.at < t.started_at && s.run !== entry.run,
          );
          row.cause = earlier.length
            ? `missed a key run ${earlier[0].run} (#${earlier[0].pr} ${earlier[0].job}) stored`
            : "absent: no run in the evidence stored this key";
          row.evictedFrom = earlier[0] ?? null;
        } else if (t.remote_read === "not_attempted") row.cause = "no remote read";
        else row.cause = `remote read ${t.remote_read}`;
        out.ran.push(row);
      }
      record.jobs.push(out);
    }
    perPr.push(record);
  }
  return {
    evidence: dir,
    prs: perPr,
    totals: totals(perPr),
    weights: weigh(perPr, join(dir, "profiles")),
  };
}

const PLATFORM_OF = {
  "test-linux": "linux",
  "test-windows": "win32",
  "test-macos": "darwin",
  lint: "lint",
};
const REST_TASK = {
  linux: "onepipeline:test-rest",
  darwin: "onepipeline:test-quick",
  win32: "onepipeline:test-quick",
};
// The test binaries a tier of their own runs; every other binary is the rest tier's.
const OWN_TIER_BINARIES =
  /^(e2e|contract|pooled_slot|narrow_write|note|codex_server|smoke|release_channel|stop_verdict|package_bundle|\w+_workload)$/;
const E2E_TASK = {
  linux: "onepipeline-e2e:test",
  darwin: "onepipeline-e2e:test-quick",
  win32: "onepipeline-e2e:test-quick",
};
const median = (values) => {
  const sorted = [...values].sort((a, b) => a - b);
  if (sorted.length === 0) return null;
  const mid = sorted.length >> 1;
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};

/**
 * What each unit an option can skip costs today, per platform: each Nx task's median
 * executed minutes less the shared test archive's build when it was the task that built
 * it; that build's median per platform; and each e2e module's share of the e2e tier's
 * test minutes, in proportion to its test-seconds in the newest profile records each
 * platform's job uploaded (`DIR/profiles/<run>/<job>/`).
 */
export function weigh(perPr, profilesDir) {
  const samples = {};
  const builds = {};
  for (const pr of perPr) {
    for (const job of pr.jobs ?? []) {
      const platform = PLATFORM_OF[job.job];
      let built = 0;
      for (const row of job.ran) {
        if (row.minutes === null) continue;
        const build = row.buildMinutes > 0.5 ? row.buildMinutes : 0;
        built = Math.max(built, build);
        samples[row.task] ??= {};
        samples[row.task][platform] ??= [];
        samples[row.task][platform].push(row.minutes - build);
      }
      if (built > 0) {
        builds[platform] ??= [];
        builds[platform].push(built);
      }
    }
  }
  const tasks = {};
  for (const [task, byPlatform] of Object.entries(samples)) {
    tasks[task] = {};
    for (const [platform, values] of Object.entries(byPlatform)) {
      tasks[task][platform] = { minutes: median(values), n: values.length };
    }
  }
  const build = Object.fromEntries(
    Object.entries(builds).map(([platform, values]) => [
      platform,
      { minutes: median(values), n: values.length },
    ]),
  );
  const modules = {};
  const sources = {};
  for (const run of existsSync(profilesDir) ? readdirSync(profilesDir) : []) {
    for (const job of readdirSync(join(profilesDir, run))) {
      const platform = PLATFORM_OF[job];
      const e2e = tasks[E2E_TASK[platform]]?.[platform]?.minutes;
      if (!e2e) continue;
      const records = selectRun(loadRecords(join(profilesDir, run, job))).records.filter(
        (r) => r.binary === "e2e",
      );
      const seconds = new Map();
      for (const record of records) {
        const name = moduleOf(record.test);
        seconds.set(name, (seconds.get(name) ?? 0) + (record.wall_s ?? 0));
      }
      const total = [...seconds.values()].reduce((a, b) => a + b, 0);
      for (const [name, s] of seconds) {
        modules[name] ??= {};
        modules[name][platform] = { minutes: (e2e * s) / total, testSeconds: s };
      }
      // The rest tier: the library's unit tests by top-level module, apart from the
      // integration binaries the tier also runs (`rest:<task>`).
      const restTask = REST_TASK[platform];
      const rest = tasks[restTask]?.[platform]?.minutes;
      const all = selectRun(loadRecords(join(profilesDir, run, job))).records;
      const restRecords = all.filter((r) => !OWN_TIER_BINARIES.test(r.binary));
      const restTotal = restRecords.reduce((sum, r) => sum + (r.wall_s ?? 0), 0);
      if (rest && restTotal > 0) {
        let unitShare = 0;
        const unit = new Map();
        for (const r of restRecords.filter((r) => r.binary === "onepipeline")) {
          const name = moduleOf(r.test);
          unit.set(name, (unit.get(name) ?? 0) + (r.wall_s ?? 0));
        }
        for (const [name, s] of unit) {
          const key = `unit:${restTask}:${name}`;
          modules[key] ??= {};
          modules[key][platform] = { minutes: (rest * s) / restTotal, testSeconds: s };
          unitShare += s / restTotal;
        }
        modules[`rest:${restTask}`] ??= {};
        modules[`rest:${restTask}`][platform] = { minutes: rest * (1 - unitShare) };
      }
      sources[platform] = {
        run,
        job,
        records: records.length,
        testSeconds: total,
        e2eMinutes: e2e,
        restRecords: restRecords.length,
        restUnitRecords: restRecords.filter((r) => r.binary === "onepipeline").length,
      };
    }
  }
  return { tasks, build, modules, sources };
}

/** Per input: the tasks and minutes it alone forced, per job; the rest by cause. */
export function totals(perPr) {
  const add = (table, key, job, mins) => {
    table[key] ??= {};
    table[key][job] ??= { tasks: 0, minutes: 0 };
    table[key][job].tasks++;
    table[key][job].minutes += mins ?? 0;
  };
  const alone = {};
  const several = {};
  const unforced = {};
  const replayed = {};
  for (const scope of ["nonRelease", "release"]) {
    alone[scope] = {};
    several[scope] = {};
    unforced[scope] = {};
    replayed[scope] = {};
  }
  for (const pr of perPr) {
    const scope = pr.release ? "release" : "nonRelease";
    for (const job of pr.jobs ?? []) {
      for (const row of job.replayed) add(replayed[scope], "replayed", job.job, row.minutes);
      for (const row of job.ran) {
        if (row.cause === "one input") add(alone[scope], row.inputs[0], job.job, row.minutes);
        else if (row.cause === "several inputs") {
          add(several[scope], "several inputs together", job.job, row.minutes);
          for (const input of row.inputs)
            add(several[scope], `  incl. ${input}`, job.job, row.minutes);
        } else
          add(
            unforced[scope],
            row.cause.replace(/run \d+ \(#\d+ [\w-]+\)/, "an earlier run"),
            job.job,
            row.minutes,
          );
      }
    }
  }
  return { alone, several, unforced, replayed };
}

/** The Markdown rendering of an observed replay. */
export function renderObserved(result) {
  const fmt = (x) => (x === null || x === undefined ? "-" : x.toFixed(1));
  const lines = [];
  const table = (title, rows) => {
    lines.push(
      `### ${title}`,
      "",
      `| input or cause | ${NX_JOBS.map((j) => `${j} tasks / min`).join(" | ")} |`,
    );
    lines.push(`| --- |${NX_JOBS.map(() => " ---: |").join("")}`);
    for (const [key, byJob] of Object.entries(rows).sort()) {
      lines.push(
        `| ${key.replace(/\|/g, "\\|")} | ${NX_JOBS.map((j) => (byJob[j] ? `${byJob[j].tasks} / ${fmt(byJob[j].minutes)}` : "-")).join(" | ")} |`,
      );
    }
    lines.push("");
  };
  for (const scope of ["nonRelease", "release"]) {
    const name = scope === "release" ? "release pull requests" : "non-release pull requests";
    table(`Replayed tasks, ${name}`, result.totals.replayed[scope]);
    table(`Ran: forced by one input alone, ${name}`, result.totals.alone[scope]);
    table(`Ran: forced by several inputs together, ${name}`, result.totals.several[scope]);
    table(`Ran: not forced by the diff, ${name}`, result.totals.unforced[scope]);
  }
  lines.push("### Per pull request", "");
  for (const pr of result.prs) {
    lines.push(
      `#### #${pr.pr}${pr.release ? " (release)" : ""} — run ${pr.run ?? "-"} — ${pr.title.replace(/\|/g, "\\|")}`,
      "",
    );
    if (pr.unread) {
      lines.push(`Unread: ${pr.unread}.`, "");
      continue;
    }
    lines.push(
      `Base \`${pr.base.slice(0, 12)}\`, head \`${pr.head.slice(0, 12)}\`, ${pr.changed.length} files changed.`,
      "",
    );
    for (const job of pr.jobs) {
      lines.push(
        `- **${job.job}** (job ${job.id}, ${fmt(job.minutes)} min): replayed ${job.replayed.length} — ${job.replayed.map((r) => r.task).join(", ") || "none"}`,
      );
      if (job.executionsUnread) lines.push(`  - executions record: ${job.executionsUnread}`);
      for (const row of job.ran) {
        const why =
          row.cause === "one input" || row.cause === "several inputs"
            ? row.inputs.join(" + ")
            : row.cause;
        lines.push(
          `  - ran \`${row.task}\` (${fmt(row.minutes)} min): ${why.replace(/\|/g, "\\|")}`,
        );
      }
    }
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
}
