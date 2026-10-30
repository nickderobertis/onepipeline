#!/usr/bin/env node
// Renders the split replay's evidence into docs/modularization-test-split.md, after its marker.
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { affectedSuites, compileMapping } from "../crate-split-replay.mjs";
import {
  causeAnalysis,
  classify,
  readWall,
  selectedShares,
  wallProjection,
} from "./split-replay.mjs";

export const SPLIT_MARKER = "<!-- BEGIN TEST SPLIT -->";
const DOC = "docs/modularization-test-split.md";
const PLATFORMS = ["linux", "macos", "windows", "local"];
const pct = (value) => `${(value * 100).toFixed(2)}%`;
const secs = (value) => `${Math.round(value)} s (${(value / 60).toFixed(1)} min)`;
const cell = (value) => String(value).replaceAll("|", "\\|").replaceAll("\n", " ");
const pr = (row) => /#(\d+)\)/.exec(row.subject)?.[0]?.replace(")", "") ?? row.commit.slice(0, 8);

/** The crate-only target's figures, as the target layout's generated table states them. */
export function crateOnly(targetDoc) {
  const rows = {};
  for (const match of targetDoc.matchAll(
    /^\| (local|linux|macos|windows) \| ([\d.]+)% \| ([\d.]+)% \| ([\d.]+)% \| ([\d.]+)% \|$/gm,
  ))
    rows[match[1]] = {
      baselineSuites: match[2],
      baselineSeconds: match[3],
      suites: match[4],
      seconds: match[5],
    };
  if (PLATFORMS.some((platform) => !rows[platform]))
    throw new Error("the target layout's baseline table names no figure for every platform");
  return rows;
}

const fraction = (value) =>
  typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 1;
const finite = (value) => typeof value === "number" && Number.isFinite(value) && value >= 0;
const close = (a, b) => Math.abs(a - b) < 1e-9;
/** A count the document publishes: a whole number from zero up to what it counts out of. */
const upTo = (value, most) => Number.isInteger(value) && value >= 0 && value <= most;

/**
 * Refuses evidence whose figures do not follow from its own rows and selection: every row's suites
 * are recomputed from its inputs, every platform's skip shares from the rows and suite weights,
 * and the migration, wall projection and decision summary are checked against them.
 */
export function validateEvidence(evidence) {
  if (!evidence || typeof evidence !== "object" || Array.isArray(evidence))
    throw new Error("split evidence is not an object");
  if (Object.keys(evidence).some((key) => !["split", "result"].includes(key)))
    throw new Error("unknown split evidence field");
  const { split, result } = evidence;
  if (
    !split?.suites ||
    !result?.mapping?.selection ||
    !Array.isArray(result.rows) ||
    !result.rows.length
  )
    throw new Error("split evidence needs a split, a selection and replayed rows");
  const suites = Object.keys(result.mapping.selection.suites);
  if (result.suites !== suites.length)
    throw new Error("split evidence suite count differs from its selection");
  const count = (value) => Number.isInteger(value) && value > 0;
  if (!count(result.tests) || !count(result.originalTests) || result.originalTests > result.tests)
    throw new Error("split evidence needs positive phase and original-test counts");
  const named = (value) => typeof value === "string" && value.trim() !== "";
  const listed = new Set();
  for (const [name, suite] of Object.entries(split.suites)) {
    if (
      !suites.includes(name) ||
      !named(suite.parent) ||
      !name.startsWith(`${suite.parent}--`) ||
      !Array.isArray(suite.tests) ||
      !suite.tests.length ||
      new Set(suite.tests).size !== suite.tests.length ||
      suite.tests.some((id) => !named(id) || listed.has(id))
    )
      throw new Error(`split suite ${name} is not a replayed suite with its own tests`);
    for (const id of suite.tests) listed.add(id);
    if (!result.mapping.crates?.[`runner-${name}`])
      throw new Error(`split suite ${name} has no runner`);
  }
  const compiled = compileMapping(result.mapping.selection);
  for (const row of result.rows) {
    if (!Array.isArray(row.effectiveFiles) || !Array.isArray(row.affected))
      throw new Error("split evidence row needs its inputs and selection");
    const affected = affectedSuites(compiled, row.effectiveFiles).affected;
    if (JSON.stringify([...affected].sort()) !== JSON.stringify([...row.affected].sort()))
      throw new Error(`split evidence row selection differs from its inputs: ${row.commit}`);
  }
  for (const platform of PLATFORMS) {
    const data = result.platforms?.[platform];
    if (
      !data ||
      !fraction(data.suiteSkip) ||
      !fraction(data.secondsSkip) ||
      !finite(data.secondsPerRun) ||
      data.secondsPerRun === 0 ||
      suites.some((suite) => !finite(data.suiteSeconds?.[suite]))
    )
      throw new Error(`invalid split platform data: ${platform}`);
    // The denominator is the suites' own weights, so a larger one cannot overstate the savings.
    const weighed = suites.reduce((sum, suite) => sum + data.suiteSeconds[suite], 0);
    if (Math.abs(weighed - data.secondsPerRun) > 1e-6 * data.secondsPerRun)
      throw new Error(`split seconds per run differs from its suite weights: ${platform}`);
    const ran = result.rows.reduce(
      (sum, row) => sum + row.affected.reduce((s, suite) => s + data.suiteSeconds[suite], 0),
      0,
    );
    if (!close(1 - ran / (data.secondsPerRun * result.rows.length), data.secondsSkip))
      throw new Error(`split seconds skipped differs from its rows and weights: ${platform}`);
    const skipped = result.rows.reduce((sum, row) => sum + suites.length - row.affected.length, 0);
    if (!close(skipped / (suites.length * result.rows.length), data.suiteSkip))
      throw new Error(`split suite runs skipped differs from its rows: ${platform}`);
    const last = Array.isArray(result.splitMigration) ? result.splitMigration.at(-1) : undefined;
    if (!last || !(Math.abs(last.secondsSkip?.[platform] - data.secondsSkip) < 1e-9))
      throw new Error(`split migration does not end at the replayed figure: ${platform}`);
  }
  for (const step of result.splitMigration)
    if (
      !fraction(step.suiteSkip) ||
      !upTo(step.suites, result.suites) ||
      PLATFORMS.some((platform) => !fraction(step.secondsSkip?.[platform]))
    )
      throw new Error("invalid split migration step");
  for (const platform of ["linux", "macos", "windows"]) {
    const wall = result.wall?.[platform];
    const ordered = (summary) =>
      summary &&
      [summary.median, summary.p90, summary.max].every(finite) &&
      summary.median <= summary.p90 &&
      summary.p90 <= summary.max &&
      upTo(summary.over, result.rows.length);
    if (
      !wall ||
      ![wall.residual, wall.e2e, wall.today].every(finite) ||
      !close(wall.residual + wall.e2e, wall.today) ||
      !ordered(wall.projected) ||
      !ordered(wall.withCompileStress) ||
      wall.projected.max > wall.today + 1e-9 ||
      !fraction(wall.typicalShare) ||
      !fraction(wall.worstShare)
    )
      throw new Error(`invalid wall projection: ${platform}`);
  }
  const local = result.wall?.local;
  if (
    !local ||
    !finite(local.testPhase) ||
    ![local.projected?.median, local.projected?.p90, local.projected?.max].every(finite) ||
    local.projected.median > local.projected.p90 ||
    local.projected.p90 > local.projected.max ||
    local.projected.max > local.testPhase + 1e-9
  )
    throw new Error("invalid wall projection: local");
  // The projection must be the one the rows' selected shares and the measured jobs give.
  const projected = wallProjection(
    selectedShares(result.rows, result.platforms),
    readWall(readFileSync(new URL("./ci-wall.json", import.meta.url), "utf8")),
  );
  for (const platform of ["linux", "macos", "windows", "local"])
    if (JSON.stringify(projected[platform]) !== JSON.stringify(result.wall[platform]))
      throw new Error(`invalid wall projection: ${platform}`);
  if (["linux", "macos", "windows"].some((platform) => !result.wallBreakdown?.[platform]))
    throw new Error("split evidence needs a residual breakdown for every job");
  for (const [platform, breakdown] of Object.entries(result.wallBreakdown)) {
    const legs = breakdown?.legs ?? [{ segments: breakdown?.segments }];
    if (
      !named(breakdown?.how) ||
      legs.some(
        (leg) =>
          !Array.isArray(leg.segments) ||
          !leg.segments.length ||
          leg.segments.some((segment) => !named(segment?.part) || !finite(segment.seconds)),
      ) ||
      (breakdown.overlapping ?? []).some((task) => !named(task?.part) || !finite(task.seconds))
    )
      throw new Error(`invalid residual breakdown: ${platform}`);
  }
  const linux = result.platforms.linux;
  const broad = result.rows.filter(
    (row) =>
      row.affected.reduce((sum, suite) => sum + linux.suiteSeconds[suite], 0) /
        linux.secondsPerRun >=
      0.9,
  ).length;
  const { causes } = result;
  if (
    !causes ||
    causes.broad !== broad ||
    Object.values(causes.prs ?? {}).some(
      (count) => !Number.isInteger(count) || count < 0 || count > broad,
    ) ||
    ["sibling", "thirdParty", "both"].some((key) => !upTo(causes.lock?.[key], broad)) ||
    causes.lock.sibling + causes.lock.thirdParty + causes.lock.both !==
      (causes.prs["lockfile resolution"] ?? 0) ||
    Object.entries(causes.sole ?? {}).some(
      ([kind, value]) => !Number.isInteger(value) || value < 0 || value > (causes.prs[kind] ?? 0),
    ) ||
    !Array.isArray(causes.freed) ||
    causes.freed.some(
      (entry) =>
        !fraction(entry.suiteSkip) ||
        !fraction(entry.secondsSkip) ||
        !upTo(entry.broad, result.rows.length),
    )
  )
    throw new Error("split decision summary differs from its rows");
  // Every count and counterfactual but the lockfile split, which needs the lockfiles, follows
  // from the rows themselves.
  const derived = causeAnalysis(result, {});
  if (
    ["broad", "prs", "sole", "freed"].some(
      (key) => JSON.stringify(derived[key]) !== JSON.stringify(causes[key]),
    )
  )
    throw new Error("split decision summary differs from its rows");
  const sib = result.siblings;
  if (
    !sib ||
    !Array.isArray(sib.prs) ||
    sib.prs.some(
      (entry) => !Array.isArray(entry.moved) || !fraction(entry.lockShare) || entry.lockShare < 0.9,
    ) ||
    !fraction(sib.adapters?.suiteSkip) ||
    !fraction(sib.adapters?.secondsSkip) ||
    sib.adapters.secondsSkip + 1e-9 < result.platforms.linux.secondsSkip ||
    !upTo(sib.adapters.broad, result.rows.length) ||
    !upTo(sib.adapters.stillBroad, sib.prs.length) ||
    Object.values(sib.roles ?? {}).some(
      (entry) =>
        !upTo(entry.prs, sib.prs.length) ||
        !finite(entry.seconds) ||
        !finite(entry.only) ||
        entry.only > entry.seconds + 1e-9,
    ) ||
    sib.prs.some(
      (entry) =>
        !named(entry.commit) ||
        !upTo(entry.crates, Object.keys(result.mapping.crates).length) ||
        Object.values(entry.byRole ?? {}).some((value) => !upTo(value, entry.crates)),
    ) ||
    Object.values(sib.crates ?? {}).some(
      (entry) =>
        !named(entry.role) ||
        !upTo(entry.prs, sib.prs.length) ||
        !Array.isArray(entry.reach) ||
        !entry.reach.every(named) ||
        !Array.isArray(entry.roots) ||
        !entry.roots.every(named),
    )
  )
    throw new Error("split sibling-engine paths differ from their rows");
  // Each listed PR is a replayed row, with the resolved lock inputs and lock-only share it has.
  for (const entry of sib.prs) {
    const row = result.rows.find((candidate) => candidate.commit === entry.commit);
    const lockFiles = (row?.effectiveFiles ?? []).filter(
      (file) => file.startsWith("resolved/") && file.endsWith("/lock.json"),
    );
    const lockShare =
      affectedSuites(compiled, lockFiles).affected.reduce(
        (sum, suite) => sum + linux.suiteSeconds[suite],
        0,
      ) / linux.secondsPerRun;
    if (
      !row ||
      entry.crates !== lockFiles.length ||
      entry.lockShare !== lockShare ||
      Object.values(entry.byRole).reduce((sum, value) => sum + value, 0) !== entry.crates
    )
      throw new Error("split sibling-engine paths differ from their rows");
  }
  for (const [key, entry] of Object.entries(result.scopes ?? {}))
    if (!key.startsWith(`${entry?.path}#`) || !Array.isArray(entry.needles))
      throw new Error(`invalid scoped input ${key}`);
  return evidence;
}

const ROLE_LABELS = {
  "a shared engine crate":
    "a shared engine crate the suite reaches transitively, through its command's dependencies",
  "a command crate": "the suite's own command crate",
  "the suite's own test code": "the suite's own test code (its runner's imports)",
  "the shared test-fake crates": "the test-fake crates the suite starts or links",
  "the dispatcher or facade every e2e suite runs": "the dispatcher or facade",
};

/** The decision summary's answer to why a bump of only sibling engines reaches most suites. */
function siblingSection(result) {
  const { siblings } = result;
  const total = result.platforms.linux.secondsPerRun;
  const count = siblings.prs.length;
  const roles = Object.entries(siblings.roles).sort((a, b) => b[1].seconds - a[1].seconds);
  const shared = Object.entries(siblings.crates).filter(
    ([, entry]) => entry.role === "a shared engine crate",
  );
  const reach = {};
  for (const entry of Object.values(siblings.crates))
    for (const kind of entry.reach) reach[kind] = (reach[kind] ?? 0) + 1;
  const { adapters } = siblings;
  return [
    `**Projected** (replay model) for the ${count} broad PRs whose lockfile change moved only sibling-engine packages and whose lockfile resolution alone selects at least 90% of Linux seconds. This workspace's own \`onepipeline*\` packages are not counted as a moved dependency. A crate's resolved set changes when a package reachable from its own external roots moved; a suite is selected when any crate in its runner's closure changed. The seconds columns are each path's Linux suite-seconds, summed over the ${count} PRs as a share of their full runs: "alone" is what that path's crates select by themselves, "only through this path" is what no other path also selects.`,
    "",
    "| Path to the moved engine | PRs | Linux seconds it selects alone | Linux seconds only through this path |",
    "| --- | ---: | ---: | ---: |",
    ...roles.map(
      ([role, entry]) =>
        `| ${ROLE_LABELS[role] ?? role} | ${entry.prs} | ${pct(entry.seconds / (total * count))} | ${pct(entry.only / (total * count))} |`,
    ),
    "",
    `Of the ${Object.keys(siblings.crates).length} crates whose resolved set changed in any of these PRs, ${reach["names the moved engine"] ?? 0} name a moved engine in their own production code (or, for a runner, in the test code its phases reach), ${reach["names a sibling engine that depends on the moved one"] ?? 0} also reach it through another sibling that depends on it (a crate may do both), and ${reach["through a third-party package that depends on it"] ?? 0} reach it only through a third-party package. No path in these PRs runs through a third-party package the bump also moved: by selection, these lockfile changes moved sibling packages only.`,
    "",
    "The shared engine crates every one of them reaches, with the sibling engines their production code names:",
    "",
    "| Shared engine crate | PRs in which its resolved set changed | Sibling engines it names |",
    "| --- | ---: | --- |",
    ...shared
      .slice(0, 15)
      .map(([name, entry]) => `| ${name} | ${entry.prs} | ${entry.roots.join(", ")} |`),
    "",
    "| PR / commit | Moved sibling packages | Shared engine crates changed | Command crates | Runners | Test fakes | Dispatcher or facade | Linux seconds the lockfile alone selects |",
    "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...siblings.prs.map(
      (entry) =>
        `| ${entry.pr ? `#${entry.pr}` : "—"} / ${entry.commit.slice(0, 8)} | ${entry.moved.join(", ")} | ${entry.byRole["a shared engine crate"] ?? 0} | ${entry.byRole["a command crate"] ?? 0} | ${entry.byRole["the suite's own test code"] ?? 0} | ${entry.byRole["the shared test-fake crates"] ?? 0} | ${entry.byRole["the dispatcher or facade every e2e suite runs"] ?? 0} | ${pct(entry.lockShare)} |`,
    ),
    "",
    `**The design change, projected as an upper bound:** shared engine crates hold their own value types where today they embed a sibling's, and reach the engines only behind adapter crates the command crates wire, so a sibling bump changes only the crates that call that engine. Keeping sibling resolution only in command crates, runners, test fakes and the dispatcher, the replay reaches ${pct(adapters.suiteSkip)} of suite runs and ${pct(adapters.secondsSkip)} of Linux seconds skipped (from ${pct(result.platforms.linux.suiteSkip)} and ${pct(result.platforms.linux.secondsSkip)}). ${adapters.stillBroad} of these ${count} PRs and ${adapters.broad} of all ${result.rows.length} would still select at least 90% of the seconds, because the suites' own test code and their fakes call the engines themselves: the harness registers every checkout through \`onevcs\` in-process, reads \`oneagentgraph\`'s task-token schema version and drives task stores through \`onetaskgraph-core\`, and the agent, host and harness fakes link the engines they act out. Those are real dependencies of realistic tests, which this design does not replace.`,
    "",
  ];
}

export function splitAppendix({ split, result }, targetDoc) {
  const before = crateOnly(targetDoc);
  const compiled = compileMapping(result.mapping.selection);
  const linux = result.platforms.linux;
  const total = linux.secondsPerRun * result.rows.length;
  const lines = [
    SPLIT_MARKER,
    "",
    "## Replay of crates and test split together",
    "",
    `**Projected** (replay model): ${result.rows.length} PRs, ${result.tests} test phases of ${result.originalTests} existing e2e tests, ${result.suites} suites. Seconds are the profile's measured module seconds divided evenly among each module's tests, as in the target layout. The baseline and crate-only columns are the target layout's projections.`,
    "",
    "| Platform | Baseline suite runs skipped | Baseline seconds skipped | Crates only: suite runs | Crates only: seconds | Crates and test split: suite runs | Crates and test split: seconds |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...PLATFORMS.map(
      (platform) =>
        `| ${platform} | ${before[platform].baselineSuites}% | ${before[platform].baselineSeconds}% | ${before[platform].suites}% | ${before[platform].seconds}% | ${pct(result.platforms[platform].suiteSkip)} | ${pct(result.platforms[platform].secondsSkip)} |`,
    ),
    "",
    "## Test-split migration steps",
    "",
    "**Projected** (replay model): each row is the skip share once that step has landed on top of the complete crate layout. Suite-run shares use that step's own suite count as their denominator.",
    "",
    "| Step | Suites | Suite runs skipped | Linux seconds | macOS seconds | Windows seconds | Local seconds |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...result.splitMigration.map(
      (step, index) =>
        `| ${index + 1}. ${step.step} | ${step.suites} | ${pct(step.suiteSkip)} | ${pct(step.secondsSkip.linux)} | ${pct(step.secondsSkip.macos)} | ${pct(step.secondsSkip.windows)} | ${pct(step.secondsSkip.local)} |`,
    ),
    "",
    "## Projected job wall clock against the 30-minute line",
    "",
    "The first three columns are **measured** on run 40947970908; the rest are **projected**. Typical is the median PR, worst is the PR selecting the most estimated seconds. A projection holds the measured job less its e2e tier fixed and scales the e2e tier by the PR's selected share; the stress columns add the target layout's all-runner compile stress. Windows is one job whose e2e tier is both of today's legs added: a second leg would be sharding, which does not count toward the line.",
    "",
    "| Job | Today, full sweep | Fixed residual | e2e tier | Typical selected share | Typical projected | Worst projected | PRs over 30 min | Typical with compile stress | Worst with compile stress |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...["linux", "macos", "windows"].map((platform) => {
      const wall = result.wall[platform];
      return `| ${platform} | ${secs(wall.today)} | ${secs(wall.residual)} | ${secs(wall.e2e)} | ${pct(wall.typicalShare)} | ${secs(wall.projected.median)} | ${secs(wall.projected.max)} | ${wall.projected.over}/${result.rows.length} | ${secs(wall.withCompileStress.median)} | ${secs(wall.withCompileStress.max)} |`;
    }),
    "",
    `Local gate test phase: ${secs(result.wall.local.testPhase)} **measured** (the profile's run F); **projected** typical ${secs(result.wall.local.projected.median)}, worst ${secs(result.wall.local.projected.max)}. This scales only the test phase; the gate's compilation and other tiers are not modelled.`,
    "",
  ];
  const { causes } = result;
  const freedBy = new Map(causes.freed.map((entry) => [entry.kinds.join(" + "), entry]));
  const anyRow = (kind) =>
    result.rows.find((row) => row.effectiveFiles.some((file) => classify(file, row)[0] === kind));
  const removal = (kind) => {
    const row = anyRow(kind);
    const file = row?.effectiveFiles.find((path) => classify(path, row)[0] === kind);
    return file ? classify(file, row)[1] : "no";
  };
  lines.push(
    "## Decision summary",
    "",
    `**Projected** (replay model): ${causes.broad} of ${result.rows.length} PRs still select at least 90% of a full run's estimated Linux seconds. A PR counts under every input kind whose own inputs alone select at least 90%; one with no such kind counts under "several inputs together". The skip shares treat that kind as selecting nothing: an upper bound where a further split could free it, and only a measure of its weight where no design change removes it.`,
    "",
    "| Forcing input kind | PRs it forces (of the broad PRs) | PRs it alone forces | Further design change that would free it | Suite runs skipped if freed | Linux seconds skipped if freed | PRs still at 90% or more |",
    "| --- | ---: | ---: | --- | ---: | ---: | ---: |",
    ...Object.entries(causes.prs)
      .sort((a, b) => b[1] - a[1])
      .map(([kind, count]) => {
        const freed = freedBy.get(kind);
        return `| ${kind} | ${count} | ${causes.sole[kind] ?? 0} | ${cell(kind === "several inputs together" ? "none: no single kind forces these" : removal(kind))} | ${freed ? pct(freed.suiteSkip) : "n/a"} | ${freed ? pct(freed.secondsSkip) : "n/a"} | ${freed ? freed.broad : "n/a"} |`;
      }),
    (() => {
      const all = causes.freed.at(-1);
      return `| every kind a further split could free (${all.kinds.join("; ")}) | | | all three splits together | ${pct(all.suiteSkip)} | ${pct(all.secondsSkip)} | ${all.broad} |`;
    })(),
    "",
    `**Measured** (each PR's own parent and merge lockfiles) for the ${causes.lock.sibling + causes.lock.thirdParty + causes.lock.both} broad PRs a lockfile resolution forces; sibling engines are the \`one*\` packages this repository links:`,
    "",
    "| Lockfile change | PRs |",
    "| --- | ---: |",
    `| sibling-engine packages only | ${causes.lock.sibling} |`,
    `| third-party packages only | ${causes.lock.thirdParty} |`,
    `| both | ${causes.lock.both} |`,
    "",
    "### Why a sibling-only lockfile change selects nearly everything",
    "",
    ...siblingSection(result),
    "## Where each job's residual goes",
    "",
    "**Measured** on run 40947970908. Segments add up to the job; Linux's quality-gate tasks run concurrently, so its task durations overlap and are listed apart.",
    "",
  );
  for (const [platform, breakdown] of Object.entries(result.wallBreakdown)) {
    const legs = breakdown.legs ?? [{ segments: breakdown.segments }];
    for (const [index, leg] of legs.entries()) {
      lines.push(
        `**${platform}${legs.length > 1 ? `, leg ${index + 1}` : ""}** (${breakdown.how}):`,
        "",
        "| Segment | Seconds |",
        "| --- | ---: |",
        ...leg.segments.map(
          (segment) => `| ${cell(segment.part)} | ${segment.seconds.toFixed(1)} |`,
        ),
        "",
      );
    }
    if (breakdown.overlapping)
      lines.push(
        `Task durations the Linux job log reports, overlapping the segments above: ${breakdown.overlapping.map((task) => `${task.part} ${Math.round(task.seconds)} s`).join(", ")}. Not reported in the job log: ${breakdown.unreported}.`,
        "",
      );
  }
  // Every selected suite-second charged evenly to the inputs that selected that suite.
  const charged = {};
  const removals = new Map([["unattributed", "no"]]);
  for (const row of result.rows) {
    const causes = {};
    for (const file of row.effectiveFiles)
      for (const suite of affectedSuites(compiled, [file]).affected) {
        causes[suite] ??= new Set();
        causes[suite].add(classify(file, row)[0]);
      }
    for (const file of row.effectiveFiles) removals.set(...classify(file, row));
    for (const suite of row.affected) {
      const kinds = [...(causes[suite] ?? ["unattributed"])];
      for (const kind of kinds) {
        charged[kind] ??= { seconds: 0, prs: new Set(), removal: removals.get(kind) };
        charged[kind].seconds += linux.suiteSeconds[suite] / kinds.length;
        charged[kind].prs.add(row.commit);
      }
    }
  }
  lines.push(
    "## What still selects the seconds",
    "",
    "**Projected** (replay model): each selected suite-second is charged evenly to the kinds of input that selected that suite in that PR.",
    "",
    "| Input kind | Share of all Linux suite-seconds the 144 PRs would run | PRs | Removed by a design change |",
    "| --- | ---: | ---: | --- |",
    ...Object.entries(charged)
      .sort((a, b) => b[1].seconds - a[1].seconds)
      .map(
        ([kind, entry]) =>
          `| ${kind} | ${pct(entry.seconds / total)} | ${entry.prs.size} | ${cell(entry.removal)} |`,
      ),
    "",
    "## Every PR still running more than half the suites",
    "",
    "**Projected** (replay model): the forcing inputs are the PR's inputs that each select more than half the suites.",
    "",
    "| PR / commit | Suites selected | Linux seconds selected | Forcing inputs | Input kinds | Removed by a design change |",
    "| --- | ---: | ---: | --- | --- | --- |",
  );
  for (const row of result.rows.filter((entry) => entry.affected.length > result.suites / 2)) {
    const forcing = row.effectiveFiles.filter(
      (file) => affectedSuites(compiled, [file]).affected.length > result.suites / 2,
    );
    const share =
      row.affected.reduce((sum, suite) => sum + linux.suiteSeconds[suite], 0) / linux.secondsPerRun;
    const kinds = [...new Set(forcing.map((file) => classify(file, row)[0]))];
    const removable = [...new Set(forcing.map((file) => classify(file, row)[1]))];
    // A raw src/ path is a hunk outside every attributed item, so its whole file stays an input.
    const named = [...new Set(forcing.map((file) => file.replace(/^moved\/[^/]+\//, "")))].map(
      (file) => (forcing.includes(file) && file.startsWith("src/") ? `${file} (whole file)` : file),
    );
    lines.push(
      `| ${pr(row)} / ${row.commit.slice(0, 8)} | ${row.affected.length} | ${pct(share)} | ${cell(named.join(", ") || "several narrower inputs together")} | ${cell(kinds.join("; ") || "none alone")} | ${cell(removable.join("; ") || "no single input selects more than half")} |`,
    );
  }
  const scopes = Object.entries(result.scopes);
  const whole = {};
  for (const [name, suite] of Object.entries(result.mapping.selection.suites))
    for (const path of suite.paths)
      if (/^docs\/(contract|contract-divergences|stop-guard)\.md$/.test(path)) {
        whole[path] ??= [];
        whole[path].push(name);
      }
  lines.push(
    "",
    "## Contract-document reads scoped to sections",
    "",
    "Derived from the reviewed source's reading code: a scoped read is selected only when the heading-bounded section its needles name changes, or when its needles stop naming exactly one section.",
    "",
    "| Scoped input | Needles | Suites reading it |",
    "| --- | --- | --- |",
    ...scopes.map(
      ([key, entry]) =>
        `| ${cell(key)} | ${cell(entry.needles.map((needle) => JSON.stringify(needle)).join(", "))} | ${Object.entries(
          result.mapping.selection.suites,
        )
          .filter(([, suite]) => suite.paths.includes(key))
          .map(([name]) => name)
          .join(", ")} |`,
    ),
    "",
    "Reads still keeping a whole document, because no needle in the reading code names one section (a module-level `include_str!` constant, or a passage found by a needle the document repeats):",
    "",
    "| Document | Suites reading it whole |",
    "| --- | --- |",
    ...Object.entries(whole).map(([path, names]) => `| ${path} | ${names.join(", ")} |`),
    "",
    "## Every split suite and the tests it runs",
    "",
    "Linux seconds per run are **projected** (the profile's module seconds divided evenly). Each part runs as a nextest filter over its parent's runner binary, so no runner is added. Runner crates are the crates the part's tests reach; a cross-command part links only the commands its journeys drive, and keeps the dispatcher and facade where a phase uses the public API or has unresolved argv.",
    "",
    "| Suite | Parent | Tests | Linux s per run | Runner crates | Tests it runs |",
    "| --- | --- | ---: | ---: | ---: | --- |",
  );
  for (const [name, suite] of Object.entries(split.suites))
    lines.push(
      `| ${name} | ${suite.parent} | ${suite.tests.length} | ${linux.suiteSeconds[name].toFixed(1)} | ${result.mapping.crates[`runner-${name}`].deps.length} | ${suite.tests.join(", ")} |`,
    );
  return `${lines.join("\n")}\n`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length !== 3) throw new Error("usage: split-report.mjs SPLIT-REPLAY.json");
    const parts = readFileSync(DOC, "utf8").split(SPLIT_MARKER);
    if (parts.length !== 2) throw new Error("expected one test split marker");
    const evidence = validateEvidence(JSON.parse(readFileSync(process.argv[2], "utf8")));
    writeFileSync(
      DOC,
      `${parts[0]}${splitAppendix(evidence, readFileSync("docs/modularization-target-layout.md", "utf8"))}`,
    );
    console.log(`test split: rendered ${evidence.result.suites} suites`);
  } catch (error) {
    const next =
      error.code && /^(?:EACCES|EPERM|ENOENT|EISDIR|ENOTDIR|ENOSPC|EROFS)$/.test(error.code)
        ? "check the named path's type, existence, permissions and disk space; rerun the same split-report.mjs invocation"
        : "generate a split replay and restore exactly one test split marker";
    console.error(`${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}
