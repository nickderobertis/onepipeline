#!/usr/bin/env node
// Spike harness for `spike-onepipeline-test-cost` (throwaway; lands nowhere).
//
// One measurement, one command:
//
//   node spikes/test-cost/measure.mjs --cache DIR --side NAME=RUN[#ATTEMPT][@VARIANT][,...] [--side ...]
//
// For every hosted run attempt named it reads, through `gh api` and only once
// (each answer is kept under DIR, so a second invocation makes no request), the
// attempt, its jobs, its log archive and its profile-record artifacts. It prints,
// per side and per platform job: the job's queue and run minutes, the e2e tier's
// nextest minutes and counts, the Linux line coverage, the run's GITHUB_SHA, head
// SHA, executed workflow blob and Nx task selection, each cache restore's outcome
// and seconds with the job's own words about saving, every side's difference from
// the first side, the e2e tests a run ran that the first side's first run did not
// (and the reverse) with their seconds, and the engine's invocations per verb.
//
// A variant names the matrix job `<job> (<variant>)`. The platform jobs are the
// attempt's own `test-*` jobs; the artifact holding a job's records is the one the
// head's own workflow uploads for that job; a record is held to the repository's
// golden profile record, which `tests/e2e/profiling.rs` holds the writer to.
//
// Exit status: 0 when the report was printed; 2 when an argument, a kept answer, a
// log or a record does not have the shape this reads (the message names which); 1
// when a program it runs (`gh`, `unzip`, `git`) fails.

import { execFileSync } from "node:child_process";
import { existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";

const REPO = "nickderobertis/onepipeline";
const GOLDEN = resolve(dirname(fileURLToPath(import.meta.url)), "../../tests/golden/test-profile-record-v2.json");
const SHA = /^[0-9a-f]{40}$/;
// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] these are the values GitHub's REST API documents for a workflow run's status and a job's conclusion; their producer is GitHub, which nothing in this repository runs, so no gate here can be derived from it, and a value outside them is refused by name rather than read.
const STATUSES = new Set(["queued", "in_progress", "completed", "waiting", "requested", "pending"]);
const CONCLUSIONS = new Set(["success", "failure", "cancelled", "skipped", "timed_out", "neutral", "action_required", "stale", null]);
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
/// The lines `scripts/ci-job.sh` and `scripts/coverage-merge.mjs` print; the
/// harness's tests read them from lines made by each producer's own statement.
// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] the reconciliation is measure.test.mjs, which makes its fixture lines by running each producer's own print statement; it is run by hand like the rest of this throwaway harness, which lands nowhere and so has no quality-gate target to join or lifetime over which either producer could drift from it.
const TASKS_LINE = /^ci-job: \S+ runs (\d+) task\(s\): (.*)$/;
const COVERAGE_LINE = /^lines percent: (\d+(?:\.\d+)?)$/;
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

class Refusal extends Error {}

function expect(condition, message) {
  if (!condition) throw new Refusal(message);
}

const isTime = (value) => typeof value === "string" && !Number.isNaN(Date.parse(value));
const isCount = (value) => Number.isSafeInteger(value) && value >= 0;
const isSeconds = (value) => typeof value === "number" && Number.isFinite(value) && value >= 0;

function parseArgs(argv) {
  const options = { cache: null, sides: [] };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    expect(i + 1 < argv.length, `${arg} needs a value`);
    const given = argv[++i];
    if (arg === "--cache") options.cache = resolve(given);
    else if (arg === "--side") {
      const side = /^([A-Za-z0-9_-]+)=([^=]+)$/.exec(given);
      expect(side, `--side takes NAME=RUN[#ATTEMPT][@VARIANT][,...], not '${given}'`);
      expect(!options.sides.some((known) => known.name === side[1]), `side '${side[1]}' is named twice`);
      const runs = side[2].split(",").map((spec) => {
        const run = /^([1-9]\d{0,15})(?:#([1-9]\d{0,3}))?(?:@([A-Za-z0-9_-]+))?$/.exec(spec);
        expect(run, `'${spec}' is not RUN[#ATTEMPT][@VARIANT]: a run id, an attempt from 1 to 9999, a variant name`);
        return { run: run[1], attempt: run[2] ? Number(run[2]) : null, variant: run[3] ?? null };
      });
      options.sides.push({ name: side[1], runs });
    } else throw new Refusal(`unknown argument ${arg}`);
  }
  expect(options.cache, "name the directory its reads are kept in with --cache DIR");
  expect(options.sides.length, "name at least one --side NAME=RUN[#ATTEMPT][@VARIANT]");
  return options;
}

/// What a record says without its values, as `tests/e2e/profiling.rs` reduces it:
/// each object's keys and each leaf's JSON type, `programs` taken per entry.
function shape(value) {
  if (Array.isArray(value)) return "array";
  if (value === null) return "null";
  if (typeof value !== "object") return typeof value;
  const isMap = (field) => field !== null && typeof field === "object" && !Array.isArray(field);
  return Object.fromEntries(Object.entries(value).map(([key, field]) => [key, key === "programs" && isMap(field) ? "programs" : shape(field)]));
}

const golden = JSON.parse(readFileSync(GOLDEN, "utf8"));
const GOLDEN_SHAPE = shape(golden);
const PROGRAM_SHAPE = shape(Object.values(golden.programs)[0]);

function checkRecord(record, path) {
  expect(isDeepStrictEqual(shape(record), GOLDEN_SHAPE), `${path} does not have the golden profile record's shape (${GOLDEN})`);
  expect(record.schema === golden.schema, `${path} is schema ${record.schema}, not ${golden.schema}`);
  expect(isSeconds(record.wall_s), `${path}: wall_s is not a duration`);
  for (const [name, category] of Object.entries(record.categories)) {
    expect(isCount(category.count) && isSeconds(category.s), `${path}: category ${name} is not a count and a duration`);
  }
  for (const [name, entry] of Object.entries(record.programs)) {
    expect(isDeepStrictEqual(shape(entry), PROGRAM_SHAPE) && isCount(entry.count) && isSeconds(entry.s), `${path}: program ${name} is not a count and a duration`);
  }
}

function readRecords(dir) {
  const records = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (lstatSync(path).isDirectory()) records.push(...readRecords(path));
    else if (name.endsWith(".json")) {
      let record;
      try {
        record = JSON.parse(readFileSync(path, "utf8"));
      } catch {
        throw new Refusal(`${path} is not JSON`);
      }
      checkRecord(record, path);
      records.push(record);
    }
  }
  return records;
}

/// The e2e records of the run most of them share: a directory holding stray
/// records of an earlier run on the same runner keeps only this run's.
function summarize(records) {
  const e2e = records.filter((record) => record.binary === "e2e");
  const counts = new Map();
  for (const record of e2e) counts.set(record.run_id, (counts.get(record.run_id) ?? 0) + 1);
  const [run] = [...counts.entries()].sort((a, b) => b[1] - a[1])[0] ?? [];
  const tests = new Map();
  const verbs = new Map();
  const engine = { count: 0, s: 0 };
  let wall = 0;
  for (const record of e2e.filter((r) => r.run_id === run)) {
    wall += record.wall_s;
    tests.set(record.test, (tests.get(record.test) ?? 0) + record.wall_s);
    for (const [program, entry] of Object.entries(record.programs)) {
      if (program === "onepipeline") {
        engine.count += entry.count;
        engine.s += entry.s;
      } else if (program.startsWith("onepipeline:")) {
        const verb = program.slice("onepipeline:".length);
        const total = verbs.get(verb) ?? { count: 0, s: 0 };
        total.count += entry.count;
        total.s += entry.s;
        verbs.set(verb, total);
      }
    }
  }
  return { wall, tests, verbs, engine };
}

function gh(path, { binary = false } = {}) {
  return execFileSync("gh", ["api", path], { encoding: binary ? "buffer" : "utf8", maxBuffer: 1 << 30, stdio: ["ignore", "pipe", "pipe"] });
}

function kept(dir, name, produce) {
  const path = join(dir, name);
  if (!existsSync(path)) {
    mkdirSync(dir, { recursive: true });
    writeFileSync(path, produce());
  }
  return path;
}

function readJson(path) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    throw new Refusal(`${path} is not JSON; delete it to read it again`);
  }
}

/// Every entry named under the archive's own root, read from its name listing (one
/// name a line) before anything is extracted; links are refused once extracted.
function checkArchive(zip) {
  let names;
  try {
    names = execFileSync("unzip", ["-Z1", zip], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], maxBuffer: 1 << 28 });
  } catch (error) {
    rmSync(zip, { force: true });
    throw new Error(`${zip} is not an archive and is dropped; run again to fetch it again (${String(error.stderr ?? error.message).trim()})`);
  }
  for (const name of names.split("\n").filter(Boolean)) {
    expect(!name.startsWith("/") && !name.split(/[\\/]/).includes(".."), `${zip} holds ${name}, outside the directory it extracts to`);
  }
}

/// Nothing extracted is a link: a log or profile archive holds files and directories.
function checkExtracted(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    const kind = lstatSync(path);
    expect(!kind.isSymbolicLink(), `${path} is a link, which no log or profile archive holds`);
    if (kind.isDirectory()) checkExtracted(path);
  }
}

/// The archive extracted once; one that does not extract is dropped with what it
/// left, so the next invocation fetches it again.
function unzipped(zip, into) {
  if (!existsSync(join(into, ".done"))) {
    checkArchive(zip);
    mkdirSync(into, { recursive: true });
    try {
      execFileSync("unzip", ["-o", "-q", zip, "-d", into], { stdio: ["ignore", "ignore", "pipe"] });
    } catch (error) {
      rmSync(into, { recursive: true, force: true });
      rmSync(zip, { force: true });
      throw new Error(`${zip} did not extract and is dropped; run again to fetch it again (${String(error.stderr ?? error.message).trim()})`);
    }
    try {
      checkExtracted(into);
    } catch (refusal) {
      rmSync(into, { recursive: true, force: true });
      throw refusal;
    }
    writeFileSync(join(into, ".done"), "");
  }
  // Again on every reuse: what was kept may have been changed since it was checked.
  checkExtracted(into);
  return into;
}

function fetchAttempt(cache, spec) {
  const dir = join(cache, spec.run);
  const top = readJson(kept(dir, "run.json", () => gh(`repos/${REPO}/actions/runs/${spec.run}`)));
  expect(isCount(top?.run_attempt) && top.run_attempt >= 1, `run ${spec.run}: its record names no attempt`);
  const attempt = spec.attempt ?? top.run_attempt;
  const adir = join(dir, `attempt-${attempt}`);
  const meta = readJson(kept(adir, "attempt.json", () => gh(`repos/${REPO}/actions/runs/${spec.run}/attempts/${attempt}`)));
  expect(STATUSES.has(meta?.status) && typeof meta.head_sha === "string" && SHA.test(meta.head_sha) && isTime(meta.run_started_at), `run ${spec.run} attempt ${attempt}: no known status, head SHA or start time`);
  const label = `${spec.run}#${attempt}${spec.variant ? `@${spec.variant}` : ""}`;
  if (meta.status !== "completed") return { label, incomplete: meta.status };
  const jobs = readJson(kept(adir, "jobs.json", () => gh(`repos/${REPO}/actions/runs/${spec.run}/attempts/${attempt}/jobs?per_page=100`)))?.jobs;
  expect(
    Array.isArray(jobs) &&
      jobs.every(
        (job) =>
          isCount(job?.id) &&
          typeof job.name === "string" &&
          isTime(job.created_at) &&
          "conclusion" in job &&
          CONCLUSIONS.has(job.conclusion) &&
          (job.completed_at === undefined || job.completed_at === null || isTime(job.completed_at)),
      ),
    `run ${spec.run} attempt ${attempt}: its job listing is not a list of named jobs with known conclusions`,
  );
  const jobCount = readJson(join(adir, "jobs.json")).total_count;
  expect(jobCount === jobs.length, `run ${spec.run} attempt ${attempt}: its job listing holds ${jobs.length} of ${jobCount} jobs`);
  const logs = unzipped(kept(adir, "logs.zip", () => gh(`repos/${REPO}/actions/runs/${spec.run}/attempts/${attempt}/logs`, { binary: true })), join(adir, "logs"));
  // A re-run keeps the first attempt's artifacts beside its own under the same names.
  const listed = readJson(kept(adir, "artifacts.json", () => gh(`repos/${REPO}/actions/runs/${spec.run}/artifacts?per_page=100`)))?.artifacts;
  expect(
    Array.isArray(listed) && listed.every((a) => isCount(a?.id) && typeof a.name === "string" && isTime(a.created_at) && typeof a.expired === "boolean"),
    `run ${spec.run}: its artifact listing is not a list of artifacts`,
  );
  const artifactCount = readJson(join(adir, "artifacts.json")).total_count;
  expect(artifactCount === listed.length, `run ${spec.run}: its artifact listing holds ${listed.length} of ${artifactCount} artifacts`);
  // Its own artifacts were uploaded while its jobs ran: after it started and by the
  // time its last job ended, which a later attempt's never are.
  const started = Date.parse(meta.run_started_at);
  const ends = jobs.filter((job) => isTime(job.completed_at)).map((job) => Date.parse(job.completed_at));
  const ended = ends.length ? Math.max(...ends) : started;
  return {
    label,
    adir,
    meta,
    jobs,
    logs,
    variant: spec.variant,
    artifacts: listed.filter((a) => Date.parse(a.created_at) >= started && Date.parse(a.created_at) <= ended && !a.expired),
  };
}

/// The attempt's platform jobs: its `test-*` jobs of this variant.
function platformJobs(fetched) {
  const names = new Map();
  for (const job of fetched.jobs) {
    const match = /^(test-[a-z0-9-]+)(?: \((.+)\))?$/.exec(job.name);
    if (match && (match[2] ?? null) === fetched.variant) names.set(match[1], job);
  }
  return [...names.entries()].sort(([a], [b]) => a.localeCompare(b));
}

function git(args) {
  return execFileSync("git", args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
}

/// A fact about the head commit, kept beside the attempt's other reads: a
/// demonstration's head is unreachable once its branch is deleted.
function headFact(fetched, name, valid, read) {
  const path = join(fetched.adir, name);
  if (existsSync(path)) {
    const known = readFileSync(path, "utf8").trim();
    if (valid(known)) return known;
  }
  const answer = read();
  expect(answer !== null && valid(answer), `the head ${fetched.meta.head_sha}'s workflow names no ${name} this reads`);
  writeFileSync(path, `${answer}\n`);
  return answer;
}

function workflowBlob(fetched) {
  return headFact(fetched, "ci-yml-blob", (v) => SHA.test(v), () => git(["rev-parse", `${fetched.meta.head_sha}:.github/workflows/ci.yml`]).trim());
}

/// The profile artifact the head's own workflow uploads for this job: the upload
/// step's `name:` under that job naming `test-profile-…`, its expressions filled.
function profileArtifact(fetched, job) {
  const name = headFact(
    fetched,
    `profile-artifact-${job}`,
    (v) => /^test-profile-[A-Za-z0-9${}. _-]+$/.test(v),
    () => {
      const workflow = git(["show", `${fetched.meta.head_sha}:.github/workflows/ci.yml`]);
      const section = new RegExp(`\\n  ${job}:\\n([\\s\\S]*?)(?=\\n  [a-z][a-z0-9-]*:\\n|$)`).exec(workflow);
      const upload = section && /\n\s+name: (test-profile-[^\n]+)/.exec(section[1]);
      return upload ? upload[1].trim() : null;
    },
  );
  const filled = name.replaceAll("${{ matrix.variant }}", fetched.variant ?? "").replaceAll("${{ github.job }}", job);
  expect(!filled.includes("${{"), `the workflow names ${job}'s profile artifact '${name}' with an expression this does not fill`);
  return filled;
}

function records(fetched, job) {
  const name = profileArtifact(fetched, job);
  const artifact = fetched.artifacts.filter((a) => a.name === name).sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at))[0];
  if (!artifact) return null;
  const zip = kept(fetched.adir, `${name}.zip`, () => gh(`repos/${REPO}/actions/artifacts/${artifact.id}/zip`, { binary: true }));
  return readRecords(unzipped(zip, join(fetched.adir, "artifacts", name)));
}

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] from here to the save evidence, the formats read are GitHub's and its actions': the stamp the runner writes on every log line, the `<n>_<job>.txt` names its log archive gives each job, the `##[group]Run` header it opens each step with, the restore lines rust-cache, actions/cache and setup-uv print, the save lines those actions and the Nx sidecar print, and the TAP lines `node --test` prints, set apart so a test's own output is not read as the job's. None is produced in this repository, so no gate here can be derived from them; each was read from the 30 job logs of the two attempts this spike measured, and what none of them matches is refused, reported as `unknown`, or left out, never guessed.
const STAMP = /^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z) ?/;
const STAMPED = /^\d{4}-\d\d-\d\dT/;

function jobLog(fetched, job) {
  const name = fetched.variant ? `${job} (${fetched.variant})` : job;
  const file = readdirSync(fetched.logs).find((entry) => entry.endsWith(`_${name}.txt`));
  if (!file) return null;
  // Unstamped lines are the runner's echo of a step's multi-line inputs, which
  // carry no measurement; a line opening like a stamp has to be one.
  return readFileSync(join(fetched.logs, file), "utf8")
    .replace(/^\uFEFF/, "")
    .replace(/\x1b\[[0-9;]*m/g, "")
    .replace(/\r/g, "")
    .split("\n")
    .filter((line) => STAMPED.test(line))
    .map((line) => {
      const stamp = STAMP.exec(line);
      const at = stamp ? Date.parse(stamp[1]) : Number.NaN;
      expect(!Number.isNaN(at), `${file}: '${line.slice(0, 40)}' does not open with a time`);
      return { at, text: line.replace(STAMP, "") };
    });
}

/// Each step as the log opens it with `##[group]Run X`; its seconds run to the next.
function steps(lines) {
  const found = [];
  lines.forEach((line, index) => {
    const match = /^##\[group\]Run (.*)$/.exec(line.text);
    if (match) found.push({ name: match[1].trim(), index });
  });
  return found.map((step, i) => {
    const end = i + 1 < found.length ? found[i + 1].index : lines.length;
    const until = end < lines.length ? lines[end].at : lines[end - 1].at;
    expect(until >= lines[step.index].at, `the step '${step.name}' ends before it starts`);
    return {
      name: step.name,
      seconds: (until - lines[step.index].at) / 1000,
      text: lines
        .slice(step.index, end)
        .map((line) => line.text)
        .join("\n"),
    };
  });
}

const CACHE_STEPS = [
  ["rust-cache", "Swatinem/rust-cache"],
  ["actions/cache/restore", "actions/cache/restore"],
  ["setup-uv", "astral-sh/setup-uv"],
];

function cacheSteps(lines) {
  const out = [];
  for (const step of steps(lines)) {
    const kind = CACHE_STEPS.find(([, uses]) => step.name.startsWith(uses))?.[0];
    if (!kind) continue;
    let outcome = "unknown";
    if (/full match: true/.test(step.text)) outcome = "hit (full)";
    else if (/full match: false/.test(step.text)) outcome = "hit (partial key)";
    else if (/No cache found|Cache not found|No GitHub Actions cache found/.test(step.text)) outcome = "miss";
    else if (/Cache restored from key|uv cache restored|Cache restored successfully/.test(step.text)) outcome = "hit";
    out.push(`${kind}: ${outcome}, ${step.seconds.toFixed(1)} s`);
  }
  return out;
}

/// Every distinct line where the job itself says whether it saves, in its own words.
function saveEvidence(lines) {
  const said = lines
    .map((line) => line.text.trim())
    .filter((text) => /save-if|save-cache|Saving cache|Cache saved|not saving|Not saving|CACHE_READ_ONLY|READ-ONLY/.test(text))
    .filter((text) => !/# Subtest|^ok \d+ -|^not ok \d+ -/.test(text));
  return [...new Set(said)];
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// The first line `marker` finds, read by `pattern`: no such line is no
/// measurement, and a line that does not read is refused rather than taken for none.
function measurement(lines, marker, pattern, parse, valid, what) {
  const line = lines.find((entry) => marker.test(entry.text.trim()));
  if (!line) return null;
  const text = line.text.trim();
  const match = pattern.exec(text);
  expect(match, `${what} reads '${text}', which is not a measurement`);
  const value = parse(match);
  expect(valid(value), `${what} reads '${text}', which is not a measurement`);
  return value;
}

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] `> nx run <task>` is Nx's own task header and `Summary [...]` is cargo-nextest's run summary; both are third-party output this repository only prints through, so no gate here can be derived from them, and a summary line that does not read is refused by `measurement`.
function e2eTier(lines) {
  const start = lines.findIndex((line) => /^> nx run onepipeline-e2e:(test|test-quick)\b/.test(line.text));
  if (start < 0) return null;
  return measurement(
    lines.slice(start),
    /^Summary \[/,
    /^Summary \[\s*(\d+\.\d+)s\] (\d+)(?:\/\d+)? tests? run: (\d+) passed((?: \(\d+ (?:slow|flaky|leaky)\))*)((?:, \d+ (?:failed|skipped|timed out|exec failed|leaky|flaky|not run))*)$/,
    (m) => ({
      seconds: Number(m[1]),
      run: Number(m[2]),
      passed: Number(m[3]),
      // nextest counts timed-out and unstartable tests apart from its `failed`; none passed.
      failed: [...m[5].matchAll(/, (\d+) (?:failed|timed out|exec failed)\b/g)].reduce((total, hit) => total + Number(hit[1]), 0),
    }),
    (t) => isSeconds(t.seconds) && isCount(t.run) && isCount(t.passed) && isCount(t.failed) && t.passed + t.failed <= t.run,
    "the e2e tier's summary",
  );
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

function measureAttempt(cache, spec) {
  const fetched = fetchAttempt(cache, spec);
  if (fetched.incomplete) return fetched;
  const out = { label: fetched.label, head: fetched.meta.head_sha, blob: workflowBlob(fetched), jobs: new Map() };
  for (const [job, meta] of platformJobs(fetched)) {
    const lines = jobLog(fetched, job);
    if (!lines?.length) {
      out.jobs.set(job, { conclusion: meta.conclusion, absent: "no log" });
      continue;
    }
    expect(
      isTime(meta.started_at) &&
        isTime(meta.completed_at) &&
        Date.parse(meta.created_at) <= Date.parse(meta.started_at) &&
        Date.parse(meta.started_at) <= Date.parse(meta.completed_at),
      `${fetched.label} ${job}: its created, started and completed times are not in order`,
    );
    const text = lines.map((line) => line.text).join("\n");
    // llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] this is actions/checkout's own line for the merge commit a pull-request run checks out; that action is not in this repository, and a log without it reports no GITHUB_SHA rather than a guessed one.
    const head = /HEAD is now at ([0-9a-f]{7,40}) Merge ([0-9a-f]{40}) into ([0-9a-f]{40})/.exec(text);
    // llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
    const found = records(fetched, job);
    out.jobs.set(job, {
      conclusion: meta.conclusion,
      queue: (Date.parse(meta.started_at) - Date.parse(meta.created_at)) / 60000,
      minutes: (Date.parse(meta.completed_at) - Date.parse(meta.started_at)) / 60000,
      githubSha: head ? head[1] : null,
      tasks: measurement(
        lines,
        /^ci-job: \S+ runs /,
        TASKS_LINE,
        (m) => ({ count: Number(m[1]), list: m[2].trim() === "none" ? "" : m[2].trim() }),
        (t) => t.count === (t.list ? t.list.split(/\s+/).length : 0),
        "the Nx task selection",
      ),
      e2e: e2eTier(lines),
      coverage: measurement(lines, /^lines percent:/, COVERAGE_LINE, (m) => Number(m[1]), (v) => v >= 0 && v <= 100, "the line coverage"),
      caches: cacheSteps(lines),
      saves: saveEvidence(lines),
      records: found ? summarize(found) : null,
    });
  }
  return out;
}

const fmt = (value, digits = 1) => (typeof value === "number" ? value.toFixed(digits) : "–");
const mean = (values) => {
  const real = values.filter((value) => typeof value === "number");
  return real.length ? real.reduce((a, b) => a + b, 0) / real.length : null;
};

function report(sides) {
  const print = (line = "") => process.stdout.write(`${line}\n`);
  const done = (side) => side.measured.filter((attempt) => !attempt.incomplete);
  const jobs = [...new Set(sides.flatMap((side) => done(side).flatMap((attempt) => [...attempt.jobs.keys()])))].sort();
  const of = (side, job) =>
    done(side)
      .map((attempt) => attempt.jobs.get(job))
      .filter((j) => j && !j.absent);

  print("## Runs");
  print();
  print("| side | run | job | conclusion | GITHUB_SHA | head SHA | ci.yml blob | Nx tasks |");
  print("| --- | --- | --- | --- | --- | --- | --- | ---: |");
  for (const side of sides) {
    for (const attempt of side.measured) {
      if (attempt.incomplete) {
        print(`| ${side.name} | ${attempt.label} | – | ${attempt.incomplete} | – | – | – | – |`);
        continue;
      }
      for (const [job, j] of attempt.jobs) {
        print(
          `| ${side.name} | ${attempt.label} | ${job} | ${j.conclusion ?? "–"}${j.absent ? ` (${j.absent})` : ""} | ${j.githubSha ?? "–"} | ${attempt.head.slice(0, 12)} | ${attempt.blob.slice(0, 12)} | ${j.tasks?.count ?? "–"} |`,
        );
      }
    }
  }

  print();
  print("## Minutes per platform (each run, then the side's mean)");
  print();
  print("Job minutes run from the job's start to its end; the time it waited for a runner is the queue column.");
  print();
  print("| side | job | queue min | job min | e2e nextest min | e2e tests run/passed/failed | Linux line coverage % | mean job min | mean e2e min |");
  print("| --- | --- | --- | --- | --- | --- | --- | ---: | ---: |");
  const means = new Map();
  for (const side of sides) {
    for (const job of jobs) {
      const js = of(side, job);
      const jobMin = js.map((j) => j.minutes);
      const e2eMin = js.map((j) => (j.e2e ? j.e2e.seconds / 60 : null));
      const m = { job: mean(jobMin), e2e: mean(e2eMin), spread: jobMin.length ? `${fmt(Math.min(...jobMin))}–${fmt(Math.max(...jobMin))}` : "–" };
      means.set(`${side.name} ${job}`, m);
      print(
        `| ${side.name} | ${job} | ${js.map((j) => fmt(j.queue)).join(", ")} | ${jobMin.map((v) => fmt(v)).join(", ")} | ${e2eMin.map((v) => fmt(v)).join(", ")} | ${js.map((j) => (j.e2e ? `${j.e2e.run}/${j.e2e.passed}/${j.e2e.failed}` : "–")).join(", ")} | ${js.map((j) => fmt(j.coverage, 4)).join(", ")} | ${fmt(m.job)} | ${fmt(m.e2e)} |`,
      );
    }
  }

  const [base, ...others] = sides;
  if (others.length) {
    print();
    print(`## Against \`${base.name}\` (mean minus mean; negative saves)`);
    print();
    print("| side | job | job min | e2e min | job-minute spread of each side's runs |");
    print("| --- | --- | ---: | ---: | --- |");
    for (const side of others) {
      for (const job of jobs) {
        const a = means.get(`${base.name} ${job}`);
        const b = means.get(`${side.name} ${job}`);
        const delta = (key) => (a[key] !== null && b[key] !== null ? fmt(b[key] - a[key]) : "–");
        print(`| ${side.name} | ${job} | ${delta("job")} | ${delta("e2e")} | ${base.name} ${a.spread}; ${side.name} ${b.spread} |`);
      }
    }
  }

  print();
  print(`## E2E tests each run ran that \`${base.name}\`'s first run did not, and the reverse (from the records)`);
  print();
  print("| side | run | job | records | only here: tests, s | only in the first run: tests, s (there) |");
  print("| --- | --- | --- | ---: | --- | --- |");
  for (const job of jobs) {
    const first = done(base)
      .map((attempt) => attempt.jobs.get(job)?.records)
      .find(Boolean);
    for (const side of sides) {
      for (const attempt of done(side)) {
        const mine = attempt.jobs.get(job)?.records;
        if (!mine) {
          print(`| ${side.name} | ${attempt.label} | ${job} | – | no records | – |`);
          continue;
        }
        if (!first) {
          print(`| ${side.name} | ${attempt.label} | ${job} | ${mine.tests.size} | – | – |`);
          continue;
        }
        const only = [...mine.tests].filter(([test]) => !first.tests.has(test));
        const missing = [...first.tests].filter(([test]) => !mine.tests.has(test));
        const sum = (pairs) => pairs.reduce((total, [, s]) => total + s, 0);
        print(`| ${side.name} | ${attempt.label} | ${job} | ${mine.tests.size} | ${only.length}, ${fmt(sum(only))} | ${missing.length}, ${fmt(sum(missing))} |`);
      }
    }
  }

  print();
  print("## Engine invocations per verb (each side's first run)");
  for (const side of sides) {
    const attempt = done(side)[0];
    for (const job of jobs) {
      const r = attempt?.jobs.get(job)?.records;
      if (!r) continue;
      print();
      print(`### ${side.name} ${attempt.label} ${job}`);
      print();
      print(`engine invocations: ${r.engine.count}, ${r.engine.s.toFixed(1)} s of ${r.wall.toFixed(1)} e2e test-s`);
      if (!r.verbs.size) {
        print();
        print("(these records carry no per-verb entries)");
        continue;
      }
      print();
      print("| verb | count | s | mean ms | share of engine s |");
      print("| --- | ---: | ---: | ---: | ---: |");
      for (const [verb, total] of [...r.verbs].sort((a, b) => b[1].s - a[1].s).slice(0, 15)) {
        const share = r.engine.s > 0 ? `${((100 * total.s) / r.engine.s).toFixed(1)}%` : "–";
        print(`| ${verb} | ${total.count} | ${total.s.toFixed(1)} | ${((total.s / Math.max(total.count, 1)) * 1000).toFixed(0)} | ${share} |`);
      }
    }
  }

  print();
  print("## Nx task selection (each distinct list, and the runs that selected it)");
  for (const job of jobs) {
    const lists = new Map();
    for (const side of sides) {
      for (const attempt of done(side)) {
        const list = attempt.jobs.get(job)?.tasks?.list;
        if (list) lists.set(list, [...(lists.get(list) ?? []), `${side.name}/${attempt.label}`]);
      }
    }
    for (const [list, runs] of lists) {
      print();
      print(`- ${job}, ${list.split(/\s+/).length} tasks, selected by ${runs.length} run(s) (${runs.join(", ")}): ${list}`);
    }
  }

  print();
  print("## Cache restores and saves");
  print();
  print("| side | run | job | restores (kind: outcome, s) | the job's words about saving |");
  print("| --- | --- | --- | --- | --- |");
  for (const side of sides) {
    for (const attempt of done(side)) {
      for (const [job, j] of attempt.jobs) {
        if (j.absent) continue;
        print(`| ${side.name} | ${attempt.label} | ${job} | ${j.caches.join("; ")} | ${j.saves.join(" / ").replace(/\|/g, "\\|")} |`);
      }
    }
  }
}

try {
  const options = parseArgs(process.argv.slice(2));
  report(options.sides.map((side) => ({ name: side.name, measured: side.runs.map((spec) => measureAttempt(options.cache, spec)) })));
} catch (error) {
  process.stderr.write(`measure: ${error.message}\n`);
  process.exit(error instanceof Refusal ? 2 : 1);
}
