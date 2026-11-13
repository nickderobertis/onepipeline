// The reuse spike's trust boundary: every argument its commands take and every document
// they read (GitHub's answers, the evidence they saved, each other's results) is checked
// here before anything is computed from it, and refused with what to do next.
import { Refusal } from "../test-profile-report.mjs";

export { Refusal };

/** Refuse unless `ok`, naming what is wrong and the next action. */
export function need(ok, message, next) {
  if (!ok) throw new Refusal(message, next);
}

const isObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
const isCount = (v) => Number.isSafeInteger(v) && v >= 0;
const isText = (v) => typeof v === "string" && v !== "";
const isTime = (v) => typeof v === "string" && Number.isFinite(Date.parse(v));
const isSha = (v) => typeof v === "string" && /^[0-9a-f]{40}$/.test(v);

/** A whole number given on the command line. */
export function wholeNumber(text, name, next) {
  need(
    typeof text === "string" && /^\d{1,15}$/.test(text),
    `${name} ${text} is not a whole number`,
    next,
  );
  const n = Number(text);
  need(Number.isSafeInteger(n), `${name} ${text} is too large`, next);
  return n;
}

/** An Actions run id given on the command line. */
export function runId(text, name = "the run id") {
  need(
    typeof text === "string" && /^[1-9]\d{0,19}$/.test(text),
    `${name} ${text} is not a run id`,
    "give the numeric id from the run's URL",
  );
  return text;
}

/** OWNER/NAME. */
export function repository(text) {
  need(
    typeof text === "string" && /^[\w.-]+\/[\w.-]+$/.test(text),
    `--repo ${text} is not OWNER/NAME`,
    "give --repo as owner/name, e.g. nickderobertis/onepipeline",
  );
  return text;
}

/**
 * Parse `--name value` pairs (and bare `--flag`s in `flags`), refusing an unknown name,
 * a repeated one, a missing value or a value that is itself an option.
 */
export function parseOptions(argv, names, { flags = [], usage }) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const name = argv[i]?.startsWith("--") ? argv[i].slice(2) : null;
    need(
      name !== null && (names.includes(name) || flags.includes(name)),
      `${argv[i]} is not an option this reads`,
      usage,
    );
    need(!(name in out), `--${name} is given twice`, usage);
    if (flags.includes(name)) {
      out[name] = true;
      continue;
    }
    const value = argv[i + 1];
    need(value !== undefined && !value.startsWith("--"), `--${name} needs a value`, usage);
    out[name] = value;
    i++;
  }
  return out;
}

const FETCH_NEXT =
  "re-run the same command; a read GitHub answered oddly is fetched again only if its file is removed";

/** `gh api rate_limit --jq '.resources.<r> | {remaining, reset}'`'s answer. */
export function rateLimit(doc, resource) {
  need(
    isObject(doc) && isCount(doc.remaining) && isCount(doc.reset) && doc.reset > 0,
    `GitHub's ${resource} rate limit answer is not {remaining, reset}`,
    "run `gh api rate_limit` and check the login",
  );
  return doc;
}

/** `gh pr list --json number,title,mergedAt,...`'s answer. */
export function mergedList(list) {
  need(
    Array.isArray(list) &&
      list.every(
        (pr) =>
          isObject(pr) && isCount(pr.number) && typeof pr.title === "string" && isTime(pr.mergedAt),
      ),
    "the merged pull request list is not a list of {number, title, mergedAt}",
    FETCH_NEXT,
  );
  return list;
}

/** A saved population.json. */
export function population(doc) {
  const prs = (list) =>
    Array.isArray(list) &&
    list.every(
      (pr) =>
        isObject(pr) && isCount(pr.number) && typeof pr.title === "string" && isTime(pr.mergedAt),
    );
  need(
    isObject(doc) && prs(doc.non_release) && prs(doc.release),
    "population.json is not {non_release, release} lists of pull requests",
    "remove it and re-run fetch-evidence.mjs",
  );
  return doc;
}

/** The rollup GraphQL answer for one pull request: its last commit's check suites. */
export function rollup(doc, pr) {
  const commit = doc?.data?.repository?.pullRequest?.commits?.nodes?.[0]?.commit;
  const again = `remove rollup/${pr}.json and re-run`;
  need(
    !doc?.errors &&
      isObject(commit) &&
      isSha(commit.oid) &&
      Array.isArray(commit.checkSuites?.nodes),
    `the check suites read for #${pr} hold no last commit`,
    `remove rollup/${pr}.json and re-run fetch-evidence.mjs`,
  );
  const timeOrNull = (v) => v === null || v === undefined || isTime(v);
  for (const suite of commit.checkSuites.nodes) {
    need(
      isObject(suite) && Array.isArray(suite.checkRuns?.nodes),
      `a check suite read for #${pr} has no check runs list`,
      again,
    );
    if (suite.workflowRun) {
      need(
        isCount(suite.workflowRun.databaseId) && isTime(suite.workflowRun.createdAt),
        `a workflow run read for #${pr} has no id or creation time`,
        again,
      );
    }
    for (const run of suite.checkRuns.nodes) {
      need(
        isText(run.name) &&
          isCount(run.databaseId) &&
          timeOrNull(run.startedAt) &&
          timeOrNull(run.completedAt),
        `a check run read for #${pr} has no name or id, or a time that is not one`,
        again,
      );
    }
  }
  return commit;
}

/** A job the replay weighs must have run: both its times, the second not before the first. */
export function finishedJob(job, pr) {
  need(
    isTime(job.started_at ?? job.startedAt) &&
      isTime(job.completed_at ?? job.completedAt) &&
      Date.parse(job.started_at ?? job.startedAt) <=
        Date.parse(job.completed_at ?? job.completedAt),
    `#${pr}'s ${job.name} job has no start and end, or ends before it starts`,
    "re-run fetch-evidence.mjs once the run has finished",
  );
  return job;
}

/** A run's artifact listing. */
export function artifacts(doc, run) {
  need(
    Array.isArray(doc?.artifacts) &&
      doc.artifacts.every(
        (a) =>
          isCount(a.id) && isText(a.name) && typeof a.expired === "boolean" && isTime(a.created_at),
      ),
    `the artifact listing of run ${run} is not a list of {id, name, expired, created_at}`,
    `remove artifacts/${run}.json and re-run`,
  );
  return doc.artifacts;
}

/** One page of the Actions cache listing. */
export function cachePage(doc, page) {
  need(
    Array.isArray(doc?.actions_caches) &&
      doc.actions_caches.every(
        (e) =>
          isCount(e.id) &&
          isText(e.key) &&
          isText(e.ref) &&
          isCount(e.size_in_bytes) &&
          isTime(e.created_at),
      ),
    `cache page ${page} is not a list of {id, key, ref, size_in_bytes, created_at}`,
    `remove caches/page-${page}.json and re-run`,
  );
  return doc.actions_caches;
}

/**
 * An `nx-cache-entries-<job>/executions.json`, as `nx-remote-cache.mjs report` writes it,
 * or the `{absent}` stand-in fetch-evidence writes when the artifact is gone.
 */
const RESULTS = ["remote_hit", "local_hit", "executed", "failed"];
const REMOTE_READS = ["hit", "miss", "failed", "not_attempted"];

export function executions(doc, where) {
  if (isObject(doc) && isText(doc.absent)) return doc;
  const again = `remove ${where} and re-run fetch-evidence.mjs`;
  need(
    isObject(doc) && Array.isArray(doc.tasks) && Array.isArray(doc.store_attempts ?? []),
    `${where} holds no tasks`,
    again,
  );
  for (const t of doc.tasks) {
    need(
      isText(t.task) &&
        t.task.includes(":") &&
        typeof t.hash === "string" &&
        /^[0-9a-f]{1,512}$/.test(t.hash),
      `${where} has a task with no project:target or hash`,
      again,
    );
    need(
      typeof t.cacheable === "boolean" && RESULTS.includes(t.result),
      `${where}: ${t.task} has no cacheability or a result this reads`,
      again,
    );
    // A record written before the reader recorded remote reads has none.
    need(
      t.remote_read === undefined || REMOTE_READS.includes(t.remote_read),
      `${where}: ${t.task}'s remote_read is not one this reads`,
      again,
    );
    need(
      isTime(t.started_at) &&
        isTime(t.finished_at) &&
        Date.parse(t.started_at) <= Date.parse(t.finished_at),
      `${where}: ${t.task} has no start and finish, or finishes before it starts`,
      again,
    );
  }
  for (const a of doc.store_attempts ?? []) {
    need(
      isText(a.task) && typeof a.hash === "string" && isText(a.status) && isTime(a.attempted_at),
      `${where} has a store attempt with no task, hash, status or time`,
      again,
    );
  }
  return doc;
}

/** A hash-plan.cjs result, read back from the plans cache. */
export function plan(doc, where) {
  const again = `remove ${where}; the next replay reads the commit's plan again`;
  need(
    isObject(doc) &&
      isObject(doc.plans) &&
      Array.isArray(doc.missing) &&
      doc.missing.every(isText) &&
      isObject(doc.runtime) &&
      typeof doc.external === "string",
    `${where} is not a hash plan`,
    again,
  );
  for (const [task, p] of Object.entries(doc.plans)) {
    need(
      isObject(p) &&
        Array.isArray(p.plan) &&
        p.plan.every(isText) &&
        typeof p.config === "string" &&
        (p.inputs === null || Array.isArray(p.inputs)) &&
        typeof p.root === "string",
      `${where}: ${task}'s plan is not a list of inputs with its configuration digest and root`,
      again,
    );
  }
  for (const [command, value] of Object.entries(doc.runtime)) {
    need(
      isObject(value) && typeof value.ok === "boolean" && typeof value.out === "string",
      `${where}: the runtime result of ${command} is not {ok, out}`,
      again,
    );
  }
  return doc;
}

/** `nx.json`'s namedInputs: names to lists of inputs. */
export function namedInputs(doc, where) {
  need(
    isObject(doc) && Object.values(doc).every((list) => Array.isArray(list)),
    `${where}'s namedInputs is not an object of input lists`,
    "replay a commit whose nx.json Nx itself reads",
  );
  return doc;
}

/** A GitHub jobs listing saved by a demonstration's reader. */
export function jobsListing(doc, run) {
  const again = `read the run's jobs again: gh api repos/OWNER/NAME/actions/runs/${run}/jobs > ${run}.jobs.json`;
  need(
    Array.isArray(doc?.jobs) &&
      doc.jobs.every(
        (j) => isCount(j.id) && isText(j.name) && isText(j.status) && Array.isArray(j.steps),
      ),
    `run ${run}'s jobs listing is not a list of {id, name, status, steps}`,
    again,
  );
  for (const j of doc.jobs) {
    for (const s of j.steps) {
      need(
        isText(s.name) &&
          (s.completed_at == null || (isTime(s.completed_at) && isTime(s.started_at))) &&
          (s.started_at == null || isTime(s.started_at)),
        `run ${run}'s job ${j.id} has a step with no name, or a finished step with no start`,
        again,
      );
    }
  }
  return doc.jobs;
}

/** An --observed replay's JSON, as option replays and the report read it. */
export function observedResult(doc, where) {
  const again = "give the file an --observed replay wrote with --json";
  need(
    isObject(doc) && Array.isArray(doc.prs) && isObject(doc.weights),
    `${where} is not an --observed replay's JSON`,
    again,
  );
  need(
    doc.totals === undefined ||
      (isObject(doc.totals) &&
        ["alone", "several", "unforced", "replayed"].every((t) =>
          ["nonRelease", "release"].every(
            (scope) =>
              isObject(doc.totals[t]?.[scope]) &&
              Object.values(doc.totals[t][scope]).every(
                (byJob) =>
                  isObject(byJob) &&
                  Object.values(byJob).every(
                    (x) => isCount(x?.tasks) && Number.isFinite(x.minutes) && x.minutes >= 0,
                  ),
              ),
          ),
        )),
    `${where}'s totals are not the alone, several, unforced and replayed tables of each scope`,
    again,
  );
  const forcedRow = (r) =>
    isText(r.task) &&
    isText(r.cause) &&
    (!["one input", "several inputs"].includes(r.cause) ||
      (Array.isArray(r.inputs) && r.inputs.every(isText)));
  for (const pr of doc.prs) {
    need(
      isObject(pr) &&
        isCount(pr.pr) &&
        typeof pr.release === "boolean" &&
        typeof pr.title === "string",
      `${where} has a pull request with no number, title or release flag`,
      again,
    );
    if (pr.unread) continue;
    need(
      pr.base === undefined || isSha(pr.base),
      `${where}: #${pr.pr}'s base is not a commit`,
      again,
    );
    need(
      Array.isArray(pr.changed) &&
        pr.changed.every((f) => typeof f === "string") &&
        (pr.moved ?? []).every?.((m) => typeof m === "string") &&
        Array.isArray(pr.jobs ?? []),
      `${where}: #${pr.pr} has no changed files, moved inputs or jobs`,
      again,
    );
    for (const job of pr.jobs ?? []) {
      need(
        isText(job.job) &&
          Array.isArray(job.ran) &&
          Array.isArray(job.replayed) &&
          job.ran.every(forcedRow) &&
          job.replayed.every((r) => isText(r.task)),
        `${where}: #${pr.pr} has a job whose tasks are not rows of a task, a cause and the inputs that forced it`,
        again,
      );
    }
  }
  const weight = (w) => Number.isFinite(w?.minutes) && w.minutes >= 0;
  for (const table of ["tasks", "modules"]) {
    for (const [unit, byPlatform] of Object.entries(doc.weights[table] ?? {})) {
      for (const [platform, w] of Object.entries(byPlatform)) {
        need(
          weight(w),
          `${where}: the ${table} weight of ${unit} on ${platform} is not a nonnegative number of minutes`,
          again,
        );
      }
    }
  }
  for (const [platform, w] of Object.entries(doc.weights.build ?? {})) {
    need(
      weight(w),
      `${where}: the archive build's weight on ${platform} is not a nonnegative number of minutes`,
      again,
    );
  }
  return doc;
}

/** A cargo `--timings` unit. */
export function timingUnit(u) {
  need(
    isObject(u) &&
      isText(u.name) &&
      typeof u.target === "string" &&
      Number.isFinite(u.duration) &&
      u.duration >= 0,
    "a cargo timings unit has no name, target or nonnegative duration",
    "re-run the build; cargo's timings report was not one this reads",
  );
  return u;
}
