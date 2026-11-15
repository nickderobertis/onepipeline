#!/usr/bin/env node
// Measurement only (spike-child-waits, never lands): fold one platform's hosted e2e jobs
// into the child-wait spike's before/after comparison.
//
//   node scripts/child-waits.mjs compare [--share F] before:NAME=DIR ... after:NAME=DIR ...
//
// Each DIR is one test job as `scripts/launch-inventory.mjs fold` reads it: the job's
// `test-profile-<job>` artifact unzipped under `test-profile/`, and its timestamped log as
// `job.log`. An optional `meta.json` beside them names the hosted run it came from
// (`{"run", "attempt", "job", "head", "github_sha"}`); without one the run is named
// by NAME alone.
//
// It prints, in Markdown: every run with its merge commit (GITHUB_SHA), head, processor,
// job log span minutes, the minutes before the e2e tier and the e2e tier's nextest minutes; each
// side's mean, range and spread of e2e minutes, their difference, and whether that
// difference is decided (non-overlapping ranges and larger than either spread) or undecided; the
// minutes a typical pull request would save (an estimate: the difference scaled by
// `--share`, the share of pull requests whose diff reaches `src/`, 27/30 by default);
// waits and polls at the sites the prototypes change, per e2e tier; CLI launches in each
// journey the re-launch prototype changes, from the test-profile records; and the e2e
// tests every before run passed that an after run did not run and pass, by name.
//
// Exit 0 once printed; 1 with the reason and a `next:` line when an input cannot be
// read; 2 for a usage error.
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { foldJob, jobFacts, readJob, Refusal } from "./launch-inventory.mjs";

// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] This is the manually audited source-analysis selection for the frozen cabae915 experiment, not a producer schema or a continuing interface. The report quotes that commit's changed functions; deriving this selection from trace observations would hide sites that ran zero times. Nothing here lands, and any later prototype revision requires a new report audit.
/** Every function whose child wait or poll a prototype changes, by prototype. */
export const CHANGED_SITES = {
  exit: [
    "onevcs::git::bounded",
    "onevcs::git::PipeCapture::start",
    "onevcs::git::PipeRead::await_readable",
    "onevcs::probe::read",
    "onevcs::pool::maintain::run",
    "onepipeline::hooks::run",
    "onepipeline::dispatchenv::run",
    "onepipeline::commandstep::run",
    "onepipeline::planedit::run",
    "onepipeline::stopguard::consult",
    "onepipeline::agentgraph::GraphRun::stop",
    "onepipeline::sys::gone_within",
  ],
  adaptive: [
    "onepipeline::driver::attach",
    "onepipeline::driver::submit_offered",
    "onepipeline::driver::confirm_driving",
    "onepipeline::vcs::follow",
    "harness::waited_on",
  ],
};

// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] The report audits every changed test body against 13eb521a and cabae915; this is that fixed experiment's explicit selection, including journeys absent on one platform. It is not a new independently versioned test registry, and this measurement branch never lands.
/** Every journey whose test body the re-launch prototype changes. */
export const CHANGED_JOURNEYS = [
  "adoption::a_fast_local_direct_node_is_held_unlanded_until_its_release_and_then_lands_once",
  "adoption::a_node_settled_at_a_change_request_reads_landed_in_every_surface",
  "adoption::a_node_settled_at_a_squash_merge_commit_reads_landed_in_every_surface",
  "adoption_check::every_live_edit_onevcs_says_can_never_complete_is_refused_whole_and_applied_repaired",
  "command_step::a_command_exiting_non_zero_fails_with_its_status_and_stderr_and_holds_its_dependents",
  "command_step::a_command_past_its_timeout_is_stopped_and_one_with_none_runs_to_its_end",
  "command_step::a_command_that_cannot_start_says_too_much_or_is_signalled_settles_command_failed",
  "command_step::a_schema_13_launch_and_a_plan_naming_none_of_the_primitives_run_as_before",
  "landing::the_counting_views_report_what_a_read_taken_now_says",
  "live_edit::a_run_wide_set_edit_is_replayed_when_a_driver_adopts",
  "readjust::deferred_republications_are_read_by_the_next_trigger_green_or_repaired_red",
  "readjust::every_deferred_verdict_word_is_recorded_and_only_pending_stays_outstanding",
  "shipped::status_on_an_adopted_run_describes_the_driver_driving_it_now",
  "shipped::status_says_an_adopter_that_died_recorded_no_exit",
  "unwatched::concurrent_watches_are_recorded_apart_and_hold_the_run_watched_until_the_last_returns",
  "views::a_run_whose_unfinished_work_is_parked_reads_ended_unfinished_and_is_given_no_advice",
  "views::a_run_with_work_a_fresh_driver_could_schedule_is_still_told_to_adopt",
];

// biome-ignore lint/suspicious/noControlCharactersInRegex: a job log carries ANSI colour codes.
const ANSI = /\u001b\[[0-9;]*m/g;
const OUTCOME =
  /^\s*([A-Z][A-Z-]*)\s+\[\s*\d+(?:\.\d+)?s\]\s+(?:\(\s*\d+\/\d+\)\s+)?onepipeline::e2e\s+(\S+)/;

/**
 * Every e2e test the log's nextest runs ended, in order: `{ t, outcome, test }`, so a
 * test failing in the tier and passing on the job's own re-run reads as both.
 */
export function outcomes(log) {
  const ended = [];
  for (const raw of log.replace(ANSI, "").split(/\r?\n/)) {
    const m = /^(\S+Z)\s(.*)$/.exec(raw);
    const t = m ? Date.parse(m[1]) / 1000 : Number.NaN;
    if (!Number.isFinite(t)) continue;
    const o = OUTCOME.exec(m[2]);
    if (o) ended.push({ t, outcome: o[1], test: o[2] });
  }
  return ended;
}

/** The function a folded wait's site names, its source position dropped. */
const functionOf = (site) => site.replace(/ \([^)]*\)$/, "");

/** One test job, reduced to what the comparison prints. */
export function runOf(side, name, dir) {
  const job = readJob(name, dir);
  if (!job.log)
    throw new Refusal(
      `${side}:${name}: ${dir} holds no job.log`,
      "save the job's log there with its timestamps (gh api .../actions/jobs/<id>/logs)",
    );
  const facts = jobFacts(job.log);
  const tier = facts.largest;
  if (!tier || tier.start == null)
    throw new Refusal(
      `${side}:${name}: the job log holds no completed e2e nextest run`,
      "fold a job whose e2e tier ran to its summary, or leave it out and say why",
    );
  const fold = foldJob(job);
  const metaFile = join(dir, "meta.json");
  const meta = existsSync(metaFile) ? JSON.parse(readFileSync(metaFile, "utf8")) : {};
  const numeric = new Set(["run", "attempt", "job"]);
  const textual = new Set(["head", "github_sha"]);
  if (
    meta === null ||
    typeof meta !== "object" ||
    Array.isArray(meta) ||
    ([...numeric].some((key) => Object.hasOwn(meta, key)) &&
      ![...numeric].every((key) => Object.hasOwn(meta, key))) ||
    Object.entries(meta).some(([key, value]) =>
      numeric.has(key)
        ? !Number.isSafeInteger(value) || value <= 0
        : !textual.has(key) || typeof value !== "string" || !/^[0-9a-f]{1,40}$/.test(value),
    )
  )
    throw new Refusal(
      `${metaFile}: invalid run metadata`,
      "use only positive integer run, attempt and job fields together, and hexadecimal head and github_sha strings",
    );
  const waits = new Map();
  for (const w of fold.waits) {
    const key = `${functionOf(w.site)}\u0000${w.kind}`;
    const e = waits.get(key) ?? { count: 0, polls: 0, ns: 0 };
    e.count += w.count;
    e.polls += w.polls;
    e.ns += w.ns;
    waits.set(key, e);
  }
  // The harness's `profile::polled` waits are all `waited_on`'s: one poll site, every
  // `World::until` and its siblings.
  for (const h of fold.harness) {
    if (h.kind !== "poll" || !/harness\.rs:\d+$/.test(h.site)) continue;
    const key = "harness::waited_on\u0000poll";
    const e = waits.get(key) ?? { count: 0, polls: 0, ns: 0 };
    e.count += h.count;
    e.ns += h.s * 1e9;
    waits.set(key, e);
  }
  const launches = new Map();
  const passed = new Set();
  const completed = new Set();
  for (const r of job.tests) {
    if (r.run_id !== tier.id || r.ended === "unfinished") continue;
    launches.set(r.test, (launches.get(r.test) ?? 0) + (r.programs?.onepipeline?.count ?? 0));
    completed.add(r.test);
  }
  const ended = outcomes(job.log);
  const inTier = (e) => e.t >= tier.start - 1 && e.t <= tier.end;
  for (const e of ended) if (inTier(e) && e.outcome === "PASS") passed.add(e.test);
  const failedInTier = new Set(
    ended.filter((e) => inTier(e) && e.outcome !== "PASS").map((e) => e.test),
  );
  // A completed profile proves execution, not success. When every failure the
  // summary counts is named, the other completed names passed even in a quiet
  // Linux log. Otherwise only explicit PASS lines establish a named success.
  if (failedInTier.size === tier.run - tier.passed) {
    for (const test of completed) if (!failedInTier.has(test)) passed.add(test);
  }
  const passedLater = new Set(
    ended.filter((e) => e.t > tier.end && e.outcome === "PASS").map((e) => e.test),
  );
  for (const test of failedInTier) passed.delete(test);
  return {
    side,
    name,
    meta,
    checkout: facts.checkout,
    cpu: job.host
      ? `${String(job.host.cpu_model ?? "?").trim()} (${job.host.cpus})`
      : "not recorded",
    jobMinutes: (facts.job.end - facts.job.start) / 60,
    preMinutes: (tier.start - facts.job.start) / 60,
    e2eMinutes: tier.seconds / 60,
    ran: tier.run,
    tierPassed: tier.passed,
    waits,
    launches,
    passed,
    retried: [...failedInTier].filter((t) => passedLater.has(t)).sort(),
    failed: [...failedInTier].filter((t) => !passedLater.has(t)).sort(),
  };
}

const mean = (xs) => xs.reduce((a, b) => a + b, 0) / xs.length;
const f2 = (x) => x.toFixed(2);
const f1 = (x) => x.toFixed(1);

/** The two sides' e2e minutes, their difference, and whether it is decided. */
export function verdict(before, after, share) {
  const b = before.map((r) => r.e2eMinutes);
  const a = after.map((r) => r.e2eMinutes);
  const diff = mean(a) - mean(b);
  const disjoint = Math.max(...a) < Math.min(...b) || Math.min(...a) > Math.max(...b);
  const decided =
    disjoint &&
    Math.abs(diff) > Math.max(...b) - Math.min(...b) &&
    Math.abs(diff) > Math.max(...a) - Math.min(...a);
  return {
    before: { mean: mean(b), min: Math.min(...b), max: Math.max(...b) },
    after: { mean: mean(a), min: Math.min(...a), max: Math.max(...a) },
    diff,
    decided,
    typical: diff * share,
  };
}

function table(head, rows) {
  return [
    `| ${head.join(" | ")} |`,
    `| ${head.map((_, i) => (i ? "---:" : "---")).join(" | ")} |`,
    ...rows.map((r) => `| ${r.join(" | ")} |`),
  ].join("\n");
}

export function render(before, after, share) {
  const runs = [...before, ...after];
  const out = [
    `## ${before.length} before run(s) against ${after.length} after run(s)\n`,
    "### Runs\n",
  ];
  out.push(
    table(
      [
        "side",
        "run",
        "GITHUB_SHA",
        "head",
        "processor (logical cores)",
        "job log span min",
        "pre-e2e min (compile, earlier tiers)",
        "e2e nextest min",
        "e2e passed/ran",
      ],
      runs.map((r) => [
        r.side,
        r.meta.run ? `${r.meta.run}/${r.meta.attempt}/${r.meta.job} (${r.name})` : r.name,
        r.meta.github_sha ?? r.checkout?.merge ?? "-",
        r.meta.head ?? r.checkout?.head ?? "-",
        r.cpu,
        f2(r.jobMinutes),
        f2(r.preMinutes),
        f2(r.e2eMinutes),
        `${r.tierPassed}/${r.ran}`,
      ]),
    ),
  );
  const v = verdict(before, after, share);
  out.push(
    "\n### E2e tier minutes\n",
    table(
      ["side", "mean", "range", "spread"],
      [
        [
          "before",
          f2(v.before.mean),
          `${f2(v.before.min)}–${f2(v.before.max)}`,
          f2(v.before.max - v.before.min),
        ],
        [
          "after",
          f2(v.after.mean),
          `${f2(v.after.min)}–${f2(v.after.max)}`,
          f2(v.after.max - v.after.min),
        ],
      ],
    ),
    "",
    `Difference, after − before: ${f2(v.diff)} min — ${v.decided ? "decided: non-overlapping ranges and mean difference outside both spreads" : "undecided: inside the spread of the runs"}.`,
    `Typical pull request (estimate, the difference × ${share.toFixed(3)} of pull requests reaching src/): ${f2(v.typical)} min.`,
  );
  const keys = new Set();
  for (const r of runs) for (const k of r.waits.keys()) keys.add(k);
  const changed = new Set([...CHANGED_SITES.exit, ...CHANGED_SITES.adaptive]);
  const sideMean = (side, k, field) => {
    const rs = runs.filter((r) => r.side === side);
    return mean(rs.map((r) => r.waits.get(k)?.[field] ?? 0));
  };
  const rows = [...keys]
    .filter((k) => changed.has(k.split("\u0000")[0]))
    .sort()
    .map((k) => {
      const [fn, kind] = k.split("\u0000");
      return [
        fn,
        kind,
        f1(sideMean("before", k, "count")),
        f1(sideMean("before", k, "polls")),
        f1(sideMean("before", k, "ns") / 1e9),
        f1(sideMean("after", k, "count")),
        f1(sideMean("after", k, "polls")),
        f1(sideMean("after", k, "ns") / 1e9),
      ];
    });
  out.push(
    "\n### Waits at the changed sites, mean per e2e tier\n",
    rows.length
      ? table(
          [
            "function",
            "kind",
            "before count",
            "before polls",
            "before s",
            "after count",
            "after polls",
            "after s",
          ],
          rows,
        )
      : "No wait at a changed site was recorded.",
  );
  const launchRows = CHANGED_JOURNEYS.map((t) => {
    const per = (side) => {
      const rs = runs.filter((r) => r.side === side && r.launches.has(t));
      return rs.length ? f1(mean(rs.map((r) => r.launches.get(t)))) : "not recorded";
    };
    return [t, per("before"), per("after")];
  }).filter((r) => r[1] !== "not recorded" || r[2] !== "not recorded");
  out.push(
    "\n### CLI launches per changed journey, mean per recorded run (test-profile records)\n",
    launchRows.length
      ? table(["journey", "before", "after"], launchRows)
      : "No CLI launch count for a changed journey was recorded.",
  );
  const baseline = new Set();
  for (const r of before) for (const t of r.passed) baseline.add(t);
  out.push("\n### Tests\n", `${baseline.size} e2e tests passed in a before run.`);
  for (const r of runs) {
    const missing = r.side === "after" ? [...baseline].filter((t) => !r.passed.has(t)).sort() : [];
    out.push(
      `- ${r.side} ${r.name}: ${r.passed.size} passed in the tier` +
        (r.side === "after"
          ? `; of the before runs' passes, not run and passed here: ${missing.length ? missing.join(", ") : "none"}`
          : "") +
        `; failed in the tier, passed on the job's re-run: ${r.retried.length ? r.retried.join(", ") : "none"}` +
        `; failed and not passed again: ${r.failed.length ? r.failed.join(", ") : "none"}.`,
    );
  }
  return `${out.join("\n")}\n`;
}

function usage(message) {
  return new Refusal(
    message,
    "run 'node scripts/child-waits.mjs compare [--share F] before:NAME=DIR ... after:NAME=DIR ...'",
    2,
  );
}

export function main(argv) {
  const [verb, ...rest] = argv;
  if (verb !== "compare") throw usage(verb ? `unknown verb '${verb}'` : "name the verb: compare");
  let share = 27 / 30;
  const before = [];
  const after = [];
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i];
    if (arg === "--share") {
      share = Number(rest[++i]);
      if (!(share > 0 && share <= 1)) throw usage("--share takes a fraction in (0, 1]");
      continue;
    }
    const m = /^(before|after):([^=]+)=(.+)$/.exec(arg);
    if (!m) throw usage(`'${arg}' is not before:NAME=DIR or after:NAME=DIR`);
    (m[1] === "before" ? before : after).push(runOf(m[1], m[2], m[3]));
  }
  if (!before.length || !after.length) throw usage("name at least one before and one after run");
  // llmlint: ignore[tool_output_is_signal] compare is a report command: the Markdown it prints on success is the whole of what it is for, and it never lands: this branch is preserved as the measurement record and nothing on it merges.
  return render(before, after, share);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    process.stdout.write(main(process.argv.slice(2)));
  } catch (error) {
    const refusal =
      error instanceof Refusal
        ? error
        : new Refusal(
            `could not compare the input: ${error.message}`,
            "check that each DIR is one job's unzipped test-profile artifact beside its job.log",
          );
    process.stderr.write(`child-waits: ${refusal.message}\nnext: ${refusal.next}\n`);
    process.exitCode = refusal.code;
  }
}
