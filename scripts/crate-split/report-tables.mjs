#!/usr/bin/env node
// The reuse spike report's tables, from what `reuse-spike.mjs` wrote and the evidence.
//
//   node scripts/crate-split/report-tables.mjs --spike OUT --evidence DIR --out FILE [--clone DIR]
//
// With --clone, a pull request is "before #1493" when its run's base does not hold
// #1493's merge (the change that made the Linux tiers cacheable; --caching-change names
// another commit to stand for it); without it, none is.
//
// Writes Markdown to FILE: the population with each pull request's CI run and changed-file
// class; each platform job's Nx tasks replayed and run, the ran ones grouped by the key
// inputs their diff matched (or the cause their run's record shows); the totals per
// input; the e2e tier's runs by cause; every option's replay over the population and
// over its typical pull requests; how far today's tier mapping agrees with Nx's own
// per-commit hash plans; and the cache inventory with the test of whether a missed key
// was evicted. It reads only local files, each checked before it is read from.
//
// Exit 0: FILE is written; one line names it and its sections. 1: an argument or a file
// was refused, with a `next:` action on stderr.
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { JOB_PLATFORM, NX_JOBS } from "./observed.mjs";
import * as check from "./validate.mjs";

const USAGE =
  "node scripts/crate-split/report-tables.mjs --spike OUT --evidence DIR --out FILE [--clone DIR [--caching-change REF]]";

function readJson(path, next) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    throw new check.Refusal(`${path} cannot be read as JSON: ${error.message}`, next);
  }
}

// The harness's one table of CI's Nx jobs and their platforms.
const PLATFORM = Object.fromEntries(Object.entries(JOB_PLATFORM).filter(([job]) => job !== "lint"));
const JOBS = NX_JOBS;
// #1493's squash merge, "ci: cache each test tier's coverage run": a run whose base
// holds it ran the Linux tiers cacheable; one whose base does not, uncacheable.
const CACHING_CHANGE = "9f7e895194c30b5c1177150f00908465e0e2a746";
let afterCaching = () => true;

/** The changed-file class a pull request falls in: what defines a typical one. */
export function changeClass(files) {
  const src = files.some((f) => /^(src|crates|schemas)\//.test(f));
  const tests = files.some((f) => f.startsWith("tests/"));
  const cargo = (f) => /(^|\/)Cargo\.(toml|lock)$/.test(f);
  if (src && tests) return "engine source and tests";
  if (src) return "engine source, no tests";
  if (tests) return "tests, no engine source";
  if (
    files.some(cargo) &&
    files.every((f) => cargo(f) || /^(justfile|CHANGELOG\.md|docs\/)/.test(f))
  ) {
    return "dependency bump";
  }
  return "CI, scripts or docs only";
}

const short = (task) => task.replace(/^onepipeline-/, "").replace(/^onepipeline:/, ":");
const input = (i) =>
  i
    .replace("runtime:node scripts/nx-version-free-inputs.mjs --ecosystem ", "versionFree:")
    .replace(/runtime:node scripts\/nx-recipe-input\.mjs (\S+).*/, "recipe:$1")
    .replace(/^runtime:/, "rt:");
const esc = (s) => String(s).replace(/\|/g, "\\|");
const min = (x) => (x === null || x === undefined ? "-" : x.toFixed(1));

function load(spike, name) {
  const path = join(spike, `${name}.json`);
  const doc = readJson(path, "run scripts/crate-split/reuse-spike.mjs into --spike first");
  if (name === "observed") {
    check.observedResult(doc, path);
    check.need(doc.totals, `${path} holds no totals`, "write it again with --observed --json");
    return doc;
  }
  const platforms = ["linux", "darwin", "win32"];
  const again = "run scripts/crate-split/reuse-spike.mjs into --spike again";
  if (name === "d-install-documented") {
    check.need(
      platforms.every((p) => {
        const d = doc?.[p];
        return (
          Number.isSafeInteger(d?.runs) &&
          Number.isSafeInteger(d.replays) &&
          d.replays >= 0 &&
          d.replays <= d.runs &&
          (d.minutesPerChange === null ||
            (Number.isFinite(d.minutesPerChange) && d.minutesPerChange >= 0))
        );
      }),
      `${path} is not a same-base replay's JSON`,
      again,
    );
    return doc;
  }
  check.weighedReplay(doc, path);
  return doc;
}

function population(observed) {
  const out = [
    "| PR | kind | class | files | CI run | lint | test-linux | test-macos | test-windows |",
    "| --- | --- | --- | ---: | --- | --- | --- | --- | --- |",
  ];
  for (const pr of observed.prs) {
    const kind = pr.release ? "release" : afterCaching(pr) ? "non-release" : "before #1493";
    const job = (name) => {
      const j = (pr.jobs ?? []).find((x) => x.job === name);
      return j
        ? `${j.id} (${j.replayed.length} of ${j.replayed.length + j.ran.length} replayed)`
        : "-";
    };
    out.push(
      `| #${pr.pr} | ${kind} | ${pr.release ? "release" : changeClass(pr.changed ?? [])} | ${(pr.changed ?? []).length} | [${pr.run}](https://github.com/nickderobertis/onepipeline/actions/runs/${pr.run}) | ${JOBS.map(job).join(" | ")} |`,
    );
  }
  return out.join("\n");
}

function perJob(observed) {
  const out = [];
  for (const pr of observed.prs) {
    out.push(`**#${pr.pr}** (run ${pr.run}, base \`${pr.base?.slice(0, 8)}\`)`);
    for (const job of JOBS) {
      const j = (pr.jobs ?? []).find((x) => x.job === job);
      if (!j) continue;
      const groups = new Map();
      for (const row of j.ran) {
        const why =
          row.cause === "one input" || row.cause === "several inputs"
            ? row.inputs.map(input).join(" + ")
            : row.cause.replace(/ \(#\d+ [\w-]+\) stored$/, " stored");
        if (!groups.has(why)) groups.set(why, []);
        groups.get(why).push(short(row.task));
      }
      const ran = [...groups].map(([why, tasks]) => `[${esc(why)}] ${tasks.join(", ")}`).join("; ");
      out.push(
        `- ${job} (job ${j.id}): replayed ${j.replayed.length}: ${j.replayed.map((r) => short(r.task)).join(", ") || "none"}. Ran ${j.ran.length}: ${ran || "none"}.`,
      );
    }
    out.push("");
  }
  return out.join("\n");
}

function totals(observed) {
  const out = [];
  const table = (title, rows) => {
    out.push(
      `**${title}** (tasks / summed task minutes; Nx runs tasks in parallel, so minutes overlap and do not add to wall time)`,
      "",
    );
    out.push(
      `| input or cause | ${JOBS.join(" | ")} |`,
      `| --- |${JOBS.map(() => " ---: |").join("")}`,
    );
    const sorted = Object.entries(rows).sort(
      (a, b) =>
        JOBS.reduce((s, j) => s + (b[1][j]?.minutes ?? 0), 0) -
        JOBS.reduce((s, j) => s + (a[1][j]?.minutes ?? 0), 0),
    );
    for (const [key, byJob] of sorted) {
      out.push(
        `| ${esc(input(key.trim()))} | ${JOBS.map((j) => (byJob[j] ? `${byJob[j].tasks} / ${byJob[j].minutes.toFixed(0)}` : "-")).join(" | ")} |`,
      );
    }
    out.push("");
  };
  for (const scope of ["nonRelease", "release"]) {
    const name = scope === "release" ? "release" : "non-release";
    table(`Forced by one input alone, ${name}`, observed.totals.alone[scope]);
    table(
      `Forced by several inputs together, ${name}`,
      Object.fromEntries(
        Object.entries(observed.totals.several[scope]).filter(([k]) => !k.startsWith(" ")),
      ),
    );
    table(`Ran, not forced by the diff, ${name}`, observed.totals.unforced[scope]);
  }
  return out.join("\n");
}

function e2e(observed) {
  const out = [
    "| population | job | replayed | forced alone (input) | forced by several | not cacheable | absent key | missed a stored key | record too old |",
    "| --- | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: |",
  ];
  for (const [label, pick] of [
    ["non-release before #1493", (p) => !p.release && !afterCaching(p)],
    ["non-release from #1493", (p) => !p.release && afterCaching(p)],
    ["release", (p) => p.release],
  ]) {
    for (const job of ["test-linux", "test-macos", "test-windows"]) {
      const c = {
        replayed: 0,
        alone: {},
        several: 0,
        nc: 0,
        absent: 0,
        missed: 0,
        old: 0,
        mins: 0,
      };
      for (const pr of observed.prs.filter(pick)) {
        const j = (pr.jobs ?? []).find((x) => x.job === job);
        if (!j) continue;
        if (j.replayed.some((r) => /^onepipeline-e2e:test/.test(r.task))) c.replayed++;
        for (const r of j.ran.filter((r) => /^onepipeline-e2e:test/.test(r.task))) {
          if (r.cause === "one input")
            c.alone[input(r.inputs[0])] = (c.alone[input(r.inputs[0])] ?? 0) + 1;
          else if (r.cause === "several inputs") c.several++;
          else if (r.cause === "not cacheable") c.nc++;
          else if (r.cause.startsWith("absent")) c.absent++;
          else if (r.cause.startsWith("missed")) c.missed++;
          else c.old++;
        }
      }
      const alone =
        Object.entries(c.alone)
          .map(([k, v]) => `${v} ${esc(k)}`)
          .join(", ") || "0";
      out.push(
        `| ${label} | ${job} | ${c.replayed} | ${alone} | ${c.several} | ${c.nc} | ${c.absent} | ${c.missed} | ${c.old} |`,
      );
    }
  }
  return out.join("\n");
}

function options(spike, observed) {
  const runs = [
    ["today's tiers (keys allow)", "today"],
    ["(a) docs out of the e2e keys", "a-docs-out-of-e2e"],
    ["(a) rest tiers on their own tests", "a-rest-own-tests"],
    ["(a) nx.json out of every key", "a-nx-json-out"],
    ["(a) all three", "a-all"],
    ["(b) e2e split into suites", "b"],
    ["(c) engine split into crates", "c"],
    ["(d1) install proofs keyed on what they build", "d-install-proofs"],
  ].filter(([, name]) => existsSync(join(spike, `${name}.json`)));
  const typical = new Set(
    observed.prs
      .filter((p) => !p.release && changeClass(p.changed ?? []) === "engine source and tests")
      .map((p) => `#${p.pr}`),
  );
  const out = [
    `| option | suite runs skipped (all 30) | net min per PR, all 30: linux / macOS / Windows | net min per typical PR (${typical.size}): linux / macOS / Windows | over today, all 30 |`,
    "| --- | ---: | --- | --- | --- |",
  ];
  const today = load(spike, "today").summary.platforms;
  for (const [label, name] of runs) {
    const r = load(spike, name);
    const p = r.summary.platforms;
    const typicalNet = ["linux", "darwin", "win32"].map((platform) => {
      const per = p[platform].perSuite;
      let saved = 0;
      const changes = r.perChange.filter((c) => typical.has(c.commit));
      for (const c of changes) {
        for (const [suite, w] of Object.entries(per)) if (!c.affected.includes(suite)) saved += w;
      }
      // A typical pull request always builds the archive, so the split's delta is charged.
      const builds = r.perChange.length - p[platform].archiveBuildsSkipped;
      const delta = builds ? p[platform].buildDeltaCharged / builds : 0;
      return changes.length ? saved / changes.length - delta : null;
    });
    const over =
      name === "today"
        ? "-"
        : name.startsWith("d-")
          ? `${["linux", "darwin", "win32"].map((x) => min(p[x].netMinutesPerChange)).join(" / ")} (tasks today's tiers leave uncached, so all of it)`
          : ["linux", "darwin", "win32"]
              .map((x) => (p[x].netMinutesPerChange - today[x].netMinutesPerChange).toFixed(1))
              .join(" / ");
    out.push(
      `| ${label} | ${r.summary.skippedSuiteRuns} of ${r.summary.changes * r.summary.suites} | ${["linux", "darwin", "win32"].map((x) => min(p[x].netMinutesPerChange)).join(" / ")} | ${typicalNet.map(min).join(" / ")} | ${over} |`,
    );
  }
  if (existsSync(join(spike, "d-install-documented.json"))) {
    const d2 = load(spike, "d-install-documented");
    out.push(
      `| (d2) documented install keyed on the commit it proves (estimate) | ${["linux", "darwin", "win32"].map((x) => d2[x].replays).join(" / ")} of ${d2.linux.runs} runs | ${["linux", "darwin", "win32"].map((x) => min(d2[x].minutesPerChange)).join(" / ")} | as all 30 | - |`,
    );
  }
  return out.join("\n");
}

/**
 * What the cache delivered: each replayed task weighed by its median executed minutes
 * on its platform, per pull request, beside what today's keys allow measured from the
 * base. A replay the base's key would not allow is one of the pull request's own
 * earlier pushes, from its own scope.
 */
function delivered(spike, observed) {
  const today = load(spike, "today");
  const out = [
    "| pull requests | delivered min per PR: linux / macOS / Windows | replays the base's key would not allow, linux / macOS / Windows | keys allow from the base (today's tiers replayed), linux / macOS / Windows |",
    "| --- | --- | --- | --- |",
  ];
  const typical = (p) => changeClass(p.changed ?? []) === "engine source and tests";
  for (const [label, pick, release] of [
    ["the non-release", (p) => !p.release, false],
    ["the typical (engine source and tests)", (p) => !p.release && typical(p), false],
    ["the release", (p) => p.release, true],
  ]) {
    const prs = observed.prs.filter((p) => !p.unread && pick(p));
    const per = ["linux", "darwin", "win32"].map((platform) => {
      let saved = 0;
      let own = 0;
      for (const p of prs) {
        const j = p.jobs.find((x) => PLATFORM[x.job] === platform);
        for (const r of j?.replayed ?? []) {
          saved += observed.weights.tasks[r.task]?.[platform]?.minutes ?? 0;
          if (r.diffMatched) own++;
        }
      }
      return { saved: prs.length ? saved / prs.length : null, own };
    });
    const allowed = ["linux", "darwin", "win32"].map((platform) => {
      const changes = today.perChange.filter((c) => prs.some((p) => `#${p.pr}` === c.commit));
      // today.json replays the non-release pull requests; the release ones are apart.
      if (!changes.length || release) return null;
      const w = today.summary.platforms[platform].perSuite;
      let skipped = 0;
      for (const c of changes)
        for (const [suite, m] of Object.entries(w)) if (!c.affected.includes(suite)) skipped += m;
      return skipped / changes.length;
    });
    out.push(
      `| ${label} (${prs.length}) | ${per.map((x) => min(x.saved)).join(" / ")} | ${per.map((x) => x.own).join(" / ")} | ${allowed.map(min).join(" / ")} |`,
    );
  }
  return out.join("\n");
}

function agreement(spike, observed) {
  const today = load(spike, "today");
  let n = 0;
  let agree = 0;
  let over = 0;
  let under = 0;
  for (const change of today.perChange) {
    const pr = observed.prs.find((p) => `#${p.pr}` === change.commit);
    check.need(
      pr && !pr.unread,
      `today.json replays ${change.commit}, which the observed replay did not read`,
      "run reuse-spike.mjs into --spike again, so both come from one observed replay",
    );
    for (const suite of change.affected) {
      check.need(
        suite in today.suites,
        `today.json's ${change.commit} affects ${suite}, which is none of its suites`,
        "run reuse-spike.mjs into --spike again",
      );
    }
    for (const j of pr.jobs) {
      if (!PLATFORM[j.job]) continue;
      for (const r of [...j.ran.map((x) => ({ ...x, ran: true })), ...j.replayed]) {
        if (!(r.task in today.suites)) continue;
        n++;
        const forced = r.ran
          ? r.cause === "one input" || r.cause === "several inputs"
          : r.diffMatched;
        const affected = change.affected.includes(r.task);
        if (affected === forced) agree++;
        else if (affected) over++;
        else under++;
      }
    }
  }
  return { n, agree, over, under };
}

function caches(evidence, observed) {
  const dir = join(evidence, "caches");
  if (!existsSync(join(dir, "inventory.json")))
    return { table: "(no inventory read)", eviction: "" };
  const inv = readJson(
    join(dir, "inventory.json"),
    "run scripts/crate-split/cache-inventory.mjs --evidence DIR again",
  );
  const count = (v) => Number.isSafeInteger(v) && v >= 0;
  check.need(
    count(inv?.entries) &&
      Number.isFinite(inv.bytes) &&
      inv.bytes >= 0 &&
      inv.families &&
      Object.values(inv.families).every(
        (t) =>
          count(t.entries) &&
          Number.isFinite(t.bytes) &&
          t.bytes >= 0 &&
          check.isTimestamp(t.oldest) &&
          check.isTimestamp(t.newest) &&
          t.oldest <= t.newest &&
          t.targets &&
          Object.values(t.targets).every(
            (x) => count(x.entries) && Number.isFinite(x.bytes) && x.bytes >= 0,
          ),
      ),
    `${join(dir, "inventory.json")} is not a cache inventory`,
    "run scripts/crate-split/cache-inventory.mjs --evidence DIR again",
  );
  const out = [
    `${inv.entries} entries, ${(inv.bytes / 1e9).toFixed(2)} GB.`,
    "",
    "| family @ scope | entries | GB | oldest | newest |",
    "| --- | ---: | ---: | --- | --- |",
  ];
  for (const [f, t] of Object.entries(inv.families)
    .sort((a, b) => b[1].entries - a[1].entries)
    .slice(0, 16)) {
    out.push(
      `| ${esc(f)} | ${t.entries} | ${(t.bytes / 1e9).toFixed(3)} | ${t.oldest.slice(0, 16)} | ${t.newest.slice(0, 16)} |`,
    );
  }
  const rest = Object.values(inv.families)
    .sort((a, b) => b.entries - a.entries)
    .slice(16);
  out.push(
    `| ${rest.length} other families | ${rest.reduce((s, t) => s + t.entries, 0)} | ${(rest.reduce((s, t) => s + t.bytes, 0) / 1e9).toFixed(3)} | | |`,
  );
  out.push("", "Nx entries by target (entries whose task hash an executions record names):", "");
  out.push(
    "| scope | entries | named | lint | format-check | test-quick | test | build | other |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
  );
  for (const f of ["nx-cache-# @ pull request", "nx-cache-# @ main"]) {
    const t = inv.families[f];
    if (!t) continue;
    const by = {};
    for (const [task, v] of Object.entries(t.targets)) {
      const target = task.split(":")[1];
      by[target] = (by[target] ?? 0) + v.entries;
    }
    const named = Object.values(by).reduce((a, b) => a + b, 0);
    const main = ["lint", "format-check", "test-quick", "test", "build"];
    const other = named - main.reduce((s, k) => s + (by[k] ?? 0), 0);
    out.push(
      `| ${f.replace("nx-cache-# @ ", "")} | ${t.entries} | ${named} | ${main.map((k) => by[k] ?? 0).join(" | ")} | ${other} |`,
    );
  }
  // Was a key a run missed one an earlier run stored, and is it still held?
  const held = new Map();
  for (const f of readdirSync(dir).filter((f) => /^page-\d+\.json$/.test(f))) {
    const page = Number(f.match(/\d+/)[0]);
    for (const e of check.cachePage(
      readJson(join(dir, f), `remove ${f} and re-run cache-inventory.mjs`),
      page,
    )) {
      if (!held.has(e.key)) held.set(e.key, []);
      held.get(e.key).push(e);
    }
  }
  let missed = 0;
  let laterOnMain = 0;
  let gone = 0;
  for (const pr of observed.prs) {
    for (const j of pr.jobs ?? []) {
      for (const r of j.ran.filter((x) => x.evictedFrom)) {
        missed++;
        const where = join(evidence, "executions", `${j.id}.json`);
        const exec = check.executions(
          readJson(where, "remove it and re-run fetch-evidence.mjs"),
          where,
        );
        const t = exec.tasks.find((x) => x.task === r.task);
        const main = (held.get(`nx-cache-${t.hash}`) ?? []).find(
          (e) => e.ref === "refs/heads/main",
        );
        if (!main) gone++;
        else if (main.created_at > t.started_at) laterOnMain++;
      }
    }
  }
  return {
    table: out.join("\n"),
    eviction: { missed, laterOnMain, gone },
  };
}

function main(argv) {
  const options_ = check.parseOptions(
    argv,
    ["spike", "evidence", "out", "clone", "caching-change"],
    {
      usage: USAGE,
    },
  );
  const caching = options_["caching-change"] ?? CACHING_CHANGE;
  for (const k of ["spike", "evidence", "out"])
    check.need(options_[k], `--${k} is required`, USAGE);
  const observed = load(options_.spike, "observed");
  if (options_.clone) {
    afterCaching = (pr) => {
      if (!pr.base) return false;
      // --is-ancestor exits 0 for yes and 1 for no; anything else is no answer.
      const asked = spawnSync(
        "git",
        ["-C", options_.clone, "merge-base", "--is-ancestor", caching, pr.base],
        { encoding: "utf8" },
      );
      if (asked.status === 0 || asked.status === 1) return asked.status === 0;
      throw new check.Refusal(
        `git cannot say whether #${pr.pr}'s base holds #1493: ${
          String(asked.stderr ?? asked.error?.message)
            .trim()
            .split("\n")[0]
        }`,
        "give --clone a clone holding main's history and the pull requests' bases",
      );
    };
  }
  const a = agreement(options_.spike, observed);
  const c = caches(options_.evidence, observed);
  const sections = [
    ["Population", population(observed)],
    ["Totals per input", totals(observed)],
    ["The e2e tier", e2e(observed)],
    ["What the cache delivered", delivered(options_.spike, observed)],
    ["Options replayed", options(options_.spike, observed)],
    [
      "Today's tier mapping against Nx's own hash plans",
      `${a.agree} of ${a.n} pull-request task runs agree (${((100 * a.agree) / a.n).toFixed(1)}%); the mapping runs ${a.over} that Nx's plan at the commit did not key on the diff, and skips ${a.under} it did.`,
    ],
    ["Cache inventory", c.table],
    [
      "Eviction",
      c.eviction
        ? `${c.eviction.missed} task runs missed a key an earlier run in the evidence stored; ${c.eviction.laterOnMain} of them read it before its main-scope copy was created, and ${c.eviction.gone} are no longer in the listing.`
        : "Undecided: no cache listing was read (run scripts/crate-split/cache-inventory.mjs --evidence DIR), so a missed key cannot be told evicted or not.",
    ],
    ["Tasks replayed and run, per pull request and job", perJob(observed)],
  ];
  writeFileSync(
    options_.out,
    sections.map(([title, body]) => `### ${title}\n\n${body}\n`).join("\n"),
  );
  console.log(
    `report-tables: wrote ${options_.out}: ${sections.map(([title]) => title).join("; ")}`,
  );
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`report-tables: ${error.message.split("\n")[0]}`);
    console.error(`next: ${error instanceof check.Refusal ? error.next : USAGE}`);
    process.exitCode = 1;
  }
}
