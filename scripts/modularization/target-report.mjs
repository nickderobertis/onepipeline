#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL, fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { commandAppendix } from "./command-report.mjs";
import { compileMapping, affectedSuites, touchedCrates } from "../crate-split-replay.mjs";
import { targetGraph } from "./target-graph.mjs";
import { commandMapping } from "./command-replay.mjs";
import { commandPaths, cliArgumentOwners } from "./source-projection.mjs";
import { expandCatalog } from "./catalog.mjs";
import { moduleGraph } from "./graph.mjs";
import { loadAttributions } from "./attribution.mjs";
import { functions } from "./rust-syntax.mjs";
import { compareGlobInputs } from "./glob-impact.mjs";
import { testFakes } from "./test-fakes.mjs";

export const TARGET_MARKER = "<!-- BEGIN TARGET LAYOUT -->";
const projectRoot = fileURLToPath(new URL("../../", import.meta.url));
const reviewed = (path) => readFileSync(`${projectRoot}${path}`, "utf8");
function stable(value) {
  if (Array.isArray(value)) return value.map(stable);
  return value && typeof value === "object"
    ? Object.fromEntries(
        Object.keys(value)
          .sort()
          .map((key) => [key, stable(value[key])]),
      )
    : value;
}
const digest = (value) =>
  createHash("sha256")
    .update(JSON.stringify(stable(value)))
    .digest("hex");

const pr = (row) =>
  `${/#(\d+)\)/.exec(row.subject)?.[0]?.replace(")", "") ?? row.commit.slice(0, 8)} / ${row.commit.slice(0, 8)}`;

/**
 * Linux test-seconds each crate forces across the 144 PRs. "Forced" charges a PR's whole reach to every crate it
 * touches (overlapping); "sole cause" counts only suites no other touched crate, own path or shared input selected,
 * which is what splitting that crate alone could ever return.
 */
export function forcedTimeLines(result, reach, mixedEngineRows) {
  const seconds = result.platforms.linux.suiteSeconds;
  const total = Object.values(seconds).reduce((a, b) => a + b, 0);
  const ran = result.rows.reduce(
    (sum, row) => sum + row.affected.reduce((s, name) => s + seconds[name], 0),
    0,
  );
  const crates = Object.keys(reach.crates)
    .filter((name) => !name.startsWith("runner-") && name !== "residual")
    .map((name) => ({
      name,
      reached: Object.keys(reach.suites).filter((suite) => reach.suites[suite].crates.has(name)),
      touches: 0,
      forced: 0,
      sole: 0,
    }));
  const byName = new Map(crates.map((crate) => [crate.name, crate]));
  const causes = { shared: 0, ownOnly: 0, ownAndCrate: 0, crateOnly: 0 };
  for (const row of result.rows) {
    const { touched } = touchedCrates(reach, row.effectiveFiles);
    const shared = row.effectiveFiles.some(reach.shared);
    for (const name of touched) {
      const crate = byName.get(name);
      if (!crate) continue;
      crate.touches++;
      crate.forced += crate.reached.reduce((sum, suite) => sum + seconds[suite], 0);
    }
    for (const suite of row.affected) {
      if (shared) {
        causes.shared += seconds[suite];
        continue;
      }
      const own = row.effectiveFiles.some(reach.suites[suite].own);
      const by = [...touched].filter((name) => reach.suites[suite].crates.has(name));
      causes[own ? (by.length ? "ownAndCrate" : "ownOnly") : "crateOnly"] += seconds[suite];
      if (!own && by.length === 1 && byName.has(by[0])) byName.get(by[0]).sole += seconds[suite];
    }
  }
  const ranked = crates.filter((crate) => crate.touches).sort((a, b) => b.forced - a.forced);
  const suites = Object.keys(seconds)
    .map((name) => ({
      name,
      share: seconds[name] / total,
      skip: result.rows.filter((row) => !row.affected.includes(name)).length / result.rows.length,
    }))
    .sort((a, b) => b.share - a.share);
  const heavy = [];
  for (const suite of suites) {
    heavy.push(suite);
    if (heavy.reduce((sum, entry) => sum + entry.share, 0) >= 0.8) break;
  }
  const mean = (list, key) => list.reduce((sum, entry) => sum + entry[key], 0) / list.length;
  const rest = suites.slice(heavy.length);
  const weighted = suites.reduce((sum, suite) => sum + suite.share * suite.skip, 0);
  const pct = (value) => `${(value * 100).toFixed(2)}%`;
  const resolvedDriver = result.rows.filter(
    (row) =>
      row.files.includes("src/driver.rs") &&
      !row.effectiveFiles.includes("src/driver.rs") &&
      row.sourceProjection?.some((entry) => entry.file === "src/driver.rs"),
  );
  return [
    "### Test time each crate forces",
    "",
    `Linux module-seconds per suite run are allocated as in the replay table above (${total.toFixed(0)} s per full run; the 144 PRs ran ${ran.toFixed(0)} suite-seconds against ${(total * 144).toFixed(0)} for running everything). **Forced** charges each PR's whole reach to every crate it touches, so PRs touching several crates count once per crate. **Sole cause** counts only suites that no other touched crate, suite-owned path or shared input also selected: it is the most splitting that crate alone could return.`,
    "",
    `Each suite-second run has exactly one of these causes: a shared input (lockfile, toolchain, configuration or the docs/scripts globs) ${pct(causes.shared / ran)}; only the suite's own paths ${pct(causes.ownOnly / ran)}; its own paths and a crate together ${pct(causes.ownAndCrate / ran)}; only crates ${pct(causes.crateOnly / ran)}. No crate split can return more than the last share, and splitting the five highest-forcing crates perfectly would return at most ${ranked
      .slice(0, 5)
      .reduce((sum, crate) => sum + crate.sole, 0)
      .toFixed(
        0,
      )} s, ${pct(ranked.slice(0, 5).reduce((sum, crate) => sum + crate.sole, 0) / ran)} of what ran: the PRs touching them also change the same suites' own test, harness or contract-document inputs. Those crates are therefore not split further here; the levers are the own-path inputs and the heaviest suites below.`,
    "",
    "| Rank | Crate | Suites reached | Linux s per change | PRs touching | Forced Linux s (144 PRs) | Sole-cause Linux s |",
    "| ---: | --- | ---: | ---: | ---: | ---: | ---: |",
    ...ranked
      .slice(0, 15)
      .map(
        (crate, index) =>
          `| ${index + 1} | ${crate.name} | ${crate.reached.length} | ${crate.reached.reduce((sum, suite) => sum + seconds[suite], 0).toFixed(0)} | ${crate.touches} | ${crate.forced.toFixed(0)} | ${crate.sole.toFixed(0)} |`,
      ),
    "",
    "### Why fewer seconds are skipped than suite runs",
    "",
    `Suite-run skipping is the unweighted mean of per-suite skip rates; seconds skipped weights each suite by its share of a full run (${pct(weighted)} on Linux). The two differ because skip rate and weight are inversely related. ${heavy.length} of ${suites.length} suites (${heavy.map((suite) => suite.name).join(", ")}) hold ${pct(heavy.reduce((sum, entry) => sum + entry.share, 0))} of the seconds and are skipped on ${pct(mean(heavy, "skip"))} of PRs on average; the other ${rest.length} hold the rest and are skipped on ${pct(mean(rest, "skip"))}. ${suites[0].name} alone holds ${pct(suites[0].share)} and is skipped on ${pct(suites[0].skip)} of PRs. Moving seconds-skipped toward suite-runs-skipped therefore needs the heaviest suites split by the inputs their tests actually read, rather than more isolation of already-cheap command suites.`,
    "",
    "| Suite | Share of Linux seconds | PRs skipping it |",
    "| --- | ---: | ---: |",
    ...suites.map((suite) => `| ${suite.name} | ${pct(suite.share)} | ${pct(suite.skip)} |`),
    "",
    "### Driver hunks outside owned items",
    "",
    `A src/driver.rs hunk outside every owned declaration resolves in its own snapshot: a private helper to its production callers, a method or nested item to its container, an impl block to its type, a doc comment or attribute to the item it precedes, a use statement to the declarations naming what it imports, and a unit test (with its test-only fixtures) to the production items it references; a blank line changes nothing. ${resolvedDriver.length} PRs touching driver.rs now project every driver hunk to owners. ${mixedEngineRows.length} still retain the whole-file input, each for one of two reasons the lexer cannot settle: the hunk is in a crate-visible item whose other callers are in other modules (src/driver.rs::settled, for instance), or a private item's name is also a local binding or field in an unresolved caller, so its true callers need Rust scope resolution. Both stay conservatively whole-file.`,
    "",
    "| PR / revision still whole-file |",
    "| --- |",
    ...mixedEngineRows.map((row) => `| ${pr(row)} |`),
  ];
}

/** The reviewed test-fake split: one crate per fake process or protocol, and the suites that link each. */
export function testFakeLines(result, reach) {
  const fakes = testFakes(projectRoot.replace(/\/$/, ""));
  const suites = Object.keys(reach.suites);
  const rows = Object.entries(fakes.crates).map(([name, crate]) => {
    const users = suites.filter((suite) => reach.suites[suite].crates.has(name));
    const moved = [
      ...crate.files,
      ...(crate.items.length ? [`lib.rs::{${crate.items.join(", ")}}`] : []),
      ...crate.modules.map((module) => `lib.rs mod ${module}`),
    ];
    const reason = crate.selection.everySuite
      ? `every suite: ${crate.selection.everySuite}`
      : [
          ...(crate.selection.doubles ?? []).map((double) => `tests starting ${double}`),
          ...(crate.selection.capabilities ?? []).map((cap) => `closures reaching ${cap}`),
          ...(crate.selection.helpers ?? []).map((helper) => `tests calling ${helper}`),
          ...(crate.selection.via ? [crate.selection.via] : []),
        ].join("; ");
    return `| ${name} | ${cell(moved.join("; "))} | ${crate.deps.join(", ") || "none"} | ${crate.external.join(", ") || "std only"} | ${cell(reason)} | ${users.length} | ${users.length && users.length <= 12 ? users.join(", ") : users.length ? "see count" : "none"} |`;
  });
  const touched = result.rows.filter((row) =>
    row.files.some((file) => file.startsWith(`${fakes.package}/`)),
  );
  const selected = (row) =>
    row.affected.filter((suite) =>
      [...reach.suites[suite].crates].some((crate) => Object.hasOwn(fakes.crates, crate)),
    ).length;
  return [
    "## Test-fake crates",
    "",
    `The planner's ruling splits crates/testfakes into one crate per fake process or fake protocol; only test-fake-codec, the stable primitive codec (script directory, recorded invocations, argv flags, rendezvous, YAML helper), is shared by the fakes. scripts/modularization/test-fakes.json is the reviewed ownership: every tracked file, every public library item and module, every binary (declared or auto-discovered) and every manifest dependency must be owned, and every double("…") a test names must be a declared binary, or the replay refuses. Dependencies between these crates and their external roots are derived from the code each owns. A suite links a fake when one of its traced test phases starts that fake by name, when a world wires the fake in and the suite's production closure reaches the capability that executes it (wired sites below), or when its tests call the helper that configures it; test-profile, test-containment and the codec are linked by the harness itself and so reach every suite. Each crate keeps the real compiled subprocess and its typed sibling protocols; no fake becomes a mock. double() in the harness builds the package of the fake it is asked for, rather than the whole aggregate.`,
    "",
    `Historical changes are re-attributed: a library hunk goes to the crate owning its item (private helpers and comments resolve as driver hunks do, and an unresolved line charges every crate with library items), a binary or module file to its crate, a manifest dependency change to the crates linking that dependency, a [[bin]] change to its crate, and any other manifest change to all of them. ${touched.length} of the 144 PRs change ${fakes.package}; on those, ${(touched.reduce((sum, row) => sum + selected(row), 0) / Math.max(touched.length, 1)).toFixed(1)} of ${suites.length} suites are selected on average.`,
    "",
    "| Crate | Moves into it | Depends on | External roots | Selected by | Suites | Suites depending |",
    "| --- | --- | --- | --- | --- | ---: | --- |",
    ...rows,
    "",
    "| Wired site | Fakes | Why selection uses capabilities instead |",
    "| --- | --- | --- |",
    ...fakes.wired.map(
      (site) => `| ${site.helper} | ${site.doubles.join(", ")} | ${cell(site.reason)} |`,
    ),
    ...fakes.historical.map(
      (entry) => `| historical ${entry.file} | ${entry.crate} | ${cell(entry.reason)} |`,
    ),
    "",
  ];
}

/** Called in the producer workspace, independently of a renderer's output directory. */
export function currentDigests() {
  const moves = JSON.parse(reviewed("scripts/modularization/target-moves.json"));
  const layout = targetGraph(JSON.parse(reviewed("scripts/modularization/layout.json")), moves);
  const catalog = expandCatalog(
    JSON.parse(reviewed("scripts/modularization/command-tests-target.json")),
  );
  return { layout: digest(layout), mapping: digest(commandMapping(layout, catalog)) };
}
const cell = (value) => String(value).replaceAll("|", "\\|").replaceAll("\n", " ");
const sourceTests = new Map();
export function intentFor(id) {
  if (typeof id !== "string" || !/^[a-z][a-z0-9_]*::[A-Za-z_]\w*$/.test(id))
    throw new Error("invalid test intent id");
  const [module, name] = id.split("::");
  if (!sourceTests.has(module)) {
    const text = reviewed(`tests/e2e/${module}.rs`);
    sourceTests.set(
      module,
      new Map(
        functions(text)
          .filter((fn) => fn.test)
          .map((fn) => {
            const lines = text.slice(0, fn.start).split("\n").reverse();
            const docs = [];
            for (const line of lines) {
              const trimmed = line.trim();
              if (trimmed.startsWith("///")) docs.unshift(trimmed.slice(3).trim());
              else if (docs.length) break;
              else if (trimmed && !trimmed.startsWith("#") && !trimmed.startsWith("//")) break;
            }
            return [
              fn.name,
              docs
                .filter((line) => !line.includes("llmlint:"))
                .join(" ")
                .replace(/\s+/g, " ")
                .slice(0, 600),
            ];
          }),
      ),
    );
  }
  const intent = sourceTests.get(module).get(name);
  if (intent === undefined) throw new Error(`missing test intent source ${id}`);
  return intent || name.replaceAll("_", " ");
}

function validateMigration(steps, crates) {
  if (!Array.isArray(steps) || steps.length !== Object.keys(crates).length)
    throw new Error("invalid target migration: expected one step per crate");
  const landed = new Set();
  let previous = 0;
  for (const step of steps) {
    if (
      !step ||
      typeof step !== "object" ||
      Array.isArray(step) ||
      Object.keys(step).some((key) => !["crate", "suiteSkip"].includes(key)) ||
      typeof step.crate !== "string" ||
      !Object.hasOwn(crates, step.crate) ||
      landed.has(step.crate) ||
      !Number.isFinite(step.suiteSkip) ||
      step.suiteSkip < previous ||
      step.suiteSkip > 1
    )
      throw new Error("invalid target migration step: crate, order or skipped fraction");
    if (
      !Array.isArray(crates[step.crate].deps) ||
      crates[step.crate].deps.some((name) => !landed.has(name))
    )
      throw new Error(`invalid target migration dependency order at ${step.crate}`);
    landed.add(step.crate);
    previous = step.suiteSkip;
  }
}

/** Render the reviewed graph and every original test from the replay's own inputs. */
export function targetAppendix({ layout, moves, catalog, result }) {
  if (
    result?.schema !== 2 ||
    !Array.isArray(result.rows) ||
    result.rows.length !== 144 ||
    !Number.isInteger(result.tests) ||
    !Number.isInteger(result.suites) ||
    !result.mapping ||
    !catalog?.dispositions ||
    !layout?.crates ||
    !moves?.extractions
  )
    throw new Error("invalid target replay evidence");
  if (
    result.originalTests !== Object.keys(catalog.dispositions).length ||
    result.tests !== Object.keys(catalog.tests).length
  )
    throw new Error("target test/phase count mismatch");
  if (
    JSON.stringify(moves) !==
      JSON.stringify(JSON.parse(reviewed("scripts/modularization/target-moves.json"))) ||
    JSON.stringify(catalog) !==
      JSON.stringify(
        expandCatalog(JSON.parse(reviewed("scripts/modularization/command-tests-target.json"))),
      )
  )
    throw new Error("target evidence differs from the current reviewed moves/catalog");
  validateMigration(result.migration, {
    ...layout.crates,
    ...testFakes(projectRoot.replace(/\/$/, "")).crates,
  });
  const current = JSON.parse(
    execFileSync(
      process.execPath,
      [
        "--input-type=module",
        "--eval",
        'process.argv[1]="target-evidence-validation"; const {currentDigests}=await import("./scripts/modularization/target-report.mjs"); process.stdout.write(JSON.stringify(currentDigests()));',
      ],
      {
        cwd: projectRoot,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
        maxBuffer: 1024 * 1024,
      },
    ),
  );
  if (digest(layout) !== current.layout || digest(result.mapping) !== current.mapping)
    throw new Error("target layout/selection differs from the current source-derived proposal");
  const reach = compileMapping(result.mapping.selection);
  if (result.suites !== Object.keys(result.mapping.suites).length)
    throw new Error("invalid target suite count");
  for (const row of result.rows) {
    if (
      typeof row.commit !== "string" ||
      !/^[a-f0-9]{40}$/.test(row.commit) ||
      typeof row.subject !== "string" ||
      [row.files, row.effectiveFiles, row.affected, row.reasons].some(
        (entries) =>
          !Array.isArray(entries) || !entries.every((entry) => typeof entry === "string"),
      )
    )
      throw new Error("invalid target replay row");
    if (
      JSON.stringify(affectedSuites(reach, row.effectiveFiles).affected) !==
      JSON.stringify(row.affected)
    )
      throw new Error("target row selection differs from its graph inputs");
  }
  const suiteSkip =
    result.rows.reduce((sum, row) => sum + result.suites - row.affected.length, 0) /
    (144 * result.suites);
  if (
    !result.platforms ||
    JSON.stringify(Object.keys(result.platforms).sort()) !==
      JSON.stringify(["linux", "local", "macos", "windows"])
  )
    throw new Error("invalid target platforms");
  for (const platform of Object.values(result.platforms))
    if (
      platform.suiteSkip !== suiteSkip ||
      !Number.isFinite(platform.secondsSkip) ||
      platform.secondsSkip < 0 ||
      platform.secondsSkip > 1 ||
      !Number.isFinite(platform.secondsPerRun) ||
      platform.secondsPerRun <= 0 ||
      !platform.suiteSeconds ||
      typeof platform.suiteSeconds !== "object" ||
      Object.values(platform.suiteSeconds).some(
        (seconds) => typeof seconds !== "number" || !Number.isFinite(seconds) || seconds < 0,
      ) ||
      JSON.stringify(Object.keys(platform.suiteSeconds).sort()) !==
        JSON.stringify(Object.keys(result.mapping.suites).sort()) ||
      Math.abs(
        Object.values(platform.suiteSeconds).reduce((a, b) => a + b, 0) - platform.secondsPerRun,
      ) >
        1e-6 * platform.secondsPerRun ||
      !Array.isArray(platform.missingModules) ||
      !platform.missingModules.every((name) => typeof name === "string")
    )
      throw new Error("invalid target platform data");
  // The published seconds fraction must be the one the validated rows and suite weights imply.
  for (const platform of Object.values(result.platforms)) {
    const skipped = result.rows.reduce(
      (sum, row) =>
        sum +
        Object.entries(platform.suiteSeconds)
          .filter(([suite]) => !row.affected.includes(suite))
          .reduce((total, [, seconds]) => total + seconds, 0),
      0,
    );
    if (
      Math.abs(skipped / (platform.secondsPerRun * result.rows.length) - platform.secondsSkip) >
      1e-9
    )
      throw new Error(
        "invalid target platform data: seconds skipped differs from its rows and weights",
      );
  }
  if (result.migration.at(-1).suiteSkip !== suiteSkip)
    throw new Error("invalid target migration terminal skipped fraction");
  const baseline = JSON.parse(reviewed("scripts/modularization/command-tests.json"));
  const baselineMetrics = [
    ...reviewed("docs/modularization-command-replay.md").matchAll(
      /^\| (local|linux|macos|windows) \| ([\d.]+)% \| ([\d.]+)% \|/gm,
    ),
  ];
  if (baselineMetrics.length !== 4 || new Set(baselineMetrics.map((match) => match[1])).size !== 4)
    throw new Error("baseline comparison requires four unique platform rows");
  const baselineByPlatform = new Map(baselineMetrics.map((match) => [match[1], match]));
  const comparison = [
    "## Baseline beside the target",
    "",
    "The 6.61% unsplit-test baseline is committed at bffb01b3. The previous target at ebe2ba6e skipped 8.30% of suite runs (Linux 5.00%, macOS 4.95%, Windows 4.97%, local 5.09% estimated seconds); the regenerated target below replaces it. Suite denominators differ (31 baseline versus the target count below); both seconds columns allocate the same observed module totals and are estimates, not new timings.",
    "",
    "| Platform | Baseline suite runs skipped | Baseline estimated seconds skipped | Target suite runs skipped | Target estimated seconds skipped |",
    "| --- | ---: | ---: | ---: | ---: |",
    ...Object.entries(result.platforms).map(
      ([name, value]) =>
        `| ${name} | ${baselineByPlatform.get(name)[2]}% | ${baselineByPlatform.get(name)[3]}% | ${(value.suiteSkip * 100).toFixed(2)}% | ${(value.secondsSkip * 100).toFixed(2)}% |`,
    ),
    "",
  ];
  const engineCrates = new Set([
    "journal",
    "journal-reader",
    "checkpoint",
    "cross-run-query",
    "run-summary",
    "run-result",
    "edit-reconciler",
    "readjust-state",
    "readjustment",
    "review-discovery",
    "review-replies",
    "event-emission",
    "channel-admission",
    "dispatch-cancellation",
    "publication-query",
    "agent-session",
    "run-locks",
    "run-paths",
    "planner-channel",
    "checkpoint",
    "plan-edits",
    "scheduler",
    "run-control",
    "agent-execution",
    "task-store",
    "run-record",
    "run-projection",
    "publication",
  ]);
  const engineFiles = new Set(
    Object.entries(layout.crates)
      .filter(([name]) => engineCrates.has(name))
      .flatMap(([, crate]) => crate.files),
  );
  const engineRows = result.rows
    .map((row) => ({
      ...row,
      engineFiles: row.files.filter(
        (file) =>
          engineFiles.has(file) &&
          (file !== "src/driver.rs" ||
            row.effectiveFiles.some((path) =>
              [...engineCrates].some((name) => path === `moved/${name}/${file}`),
            )),
      ),
    }))
    .filter((row) => row.engineFiles.length);
  const mixedEngineRows = result.rows.filter(
    (row) => row.files.includes("src/driver.rs") && row.effectiveFiles.includes("src/driver.rs"),
  );
  const engineLines = [
    "## PRs changing genuinely shared engine source",
    "",
    `${engineRows.length}/144 PRs (${((100 * engineRows.length) / 144).toFixed(2)}%) change the shared engine source files listed below. This is a separate file-touch statistic, not the number forced broad by lockfiles, configuration or CLI arguments. Command-only driver hunks are excluded; driver.rs counts only when a classified hunk moves to a shared engine owner. ${mixedEngineRows.length} PRs retain an ambiguous whole-file driver input and are conservatively selected separately. A file-level touch can still cover both shared and command-local code, so this statistic is evidence of shared subsystem work, not a claim that every hunk must fan out to all commands.`,
    "",
    "| PR / revision | Shared engine files |",
    "| --- | --- |",
    ...engineRows.map(
      (row) =>
        `| ${/#(\d+)\)/.exec(row.subject)?.[0]?.replace(")", "") ?? row.commit.slice(0, 8)} / ${row.commit.slice(0, 8)} | ${row.engineFiles.join(", ")} |`,
    ),
    "",
  ];
  const subsystemLines = [
    "## Engine subsystem boundaries (included in this replay)",
    "",
    "engine-subsystems.json schema 1 replaces aggregate dependencies with the consumer-owned interfaces below. The resulting layout is schema 3; source item moves override earlier extraction ownership. These are proposed real implementations and adapters, not test doubles. The compatibility facade aggregates their original public paths and strict wire types.",
    "",
    "| Subsystem / command | Source files and extracted items | Consumer-owned interface / preserved behavior |",
    "| --- | --- | --- |",
    ...Object.entries(layout.engineSubsystems.subsystems)
      .filter(([name]) => !name.startsWith("cmd-"))
      .map(
        ([name, entry]) =>
          `| ${name} | ${cell([...entry.files, ...entry.items.map((item) => `${item.file}::{${item.symbols.join(", ")}}`)].join("; ") || "remaining primary items / adapter")} | ${cell(entry.interface)} |`,
      ),
    "",
    "### Corrected source-cycle classification for review replies",
    "",
    "src/reply.rs is host review reply drafting/posting. It is shared review-replies, not the CLI reply command. This table supersedes the original command classification for every incident source-cycle edge; the original edge inventory remains unchanged.",
    "",
    "| Original edge | Classification | Fix |",
    "| --- | --- | --- |",
    ...moduleGraph()
      .edges.filter((edge) => edge.from === "reply" || edge.to === "reply")
      .map(
        (edge) =>
          `| ${edge.from} → ${edge.to} | ${edge.to === "engine" ? "new seam" : "genuinely shared concept"} | ${edge.to === "engine" ? "review-replies owns DeclineValidation; edit-reconciler wraps Staged. Shared emission helpers move to event-emission." : edge.to === "review" ? "Move review::key to review-replies; discovery calls the same idempotence key without a reverse dependency." : "Move host reply behavior to review-replies and retain the named subsystem implementation dependencies in the graph below."} |`,
      ),
    "",
    "### Aggregate edges replaced by subsystem interfaces",
    "",
    "Every removed aggregate edge is listed here. The depending subsystem owns the port or input DTO described in its row above; the command or scheduler composition root supplies the real implementation. Pure query code never receives a write-capable manager merely to obtain facts. This preserves execution paths while reversing compile dependencies.",
    "",
    "| Depending crate | Former dependency | New seam / operation move |",
    "| --- | --- | --- |",
    ...layout.targetFixes
      .filter((fix) => fix.replacement === "consumer-owned subsystem port")
      .map((fix) => `| ${fix.from} | ${fix.to} | ${cell(fix.reason)} |`),
    "",
    "### Remaining crate fan-out and the next boundary",
    "",
    "Counts include explicit cross-command, public-API and shipped-dispatch integration suites. These edges have not been erased from the replay. The next split narrows implementation changes; a change to a common wire grammar or atomic primitive still must test all its real consumers. No claim below treats moving a runtime dependency behind a callback as permission to omit the real callback implementation from a driving command's suite.",
    "",
    "| Crate reaching more than half the suites | Suites | Further split |",
    "| --- | ---: | --- |",
  ];
  const furtherSplits = {
    "run-views":
      "Separate snapshot assembly, activity/history queries, liveness refresh and upstream/publication refresh; each command runner retains only the refreshes it actually invokes. Full snapshot integrations retain all facets.",
    "edit-reconciler":
      "Separate envelope validation, staged operation application and orphan queue recovery; inline submitters still compose their actual apply/recovery effects.",
    "readjust-state":
      "Separate detection evidence, lineage placement and readjust progress records; consumers retain only the state facets they read.",
    readjustment:
      "Separate host detection from retarget/restack execution and post-edit refresh; only operations invoking each effect retain it.",
    "review-replies":
      "Separate review idempotence keys, decline validation, drafting and host posting; the editor needs validation and actual triggered effects.",
    "review-discovery":
      "Separate review discovery/query from scheduled pass effects and reply triggering.",
    "event-emission":
      "Separate payload encoding, bounded formatting and channel delivery; each emitter retains its actual writer implementation.",
    "channel-admission":
      "Separate ownership handover, queue admission and reply wait policy; preserve takeover recovery ordering.",
    "run-control":
      "Separate lifecycle start/adoption from stop/shutdown and continuation; retain real ownership and process operations for each entrypoint.",
    publication:
      "Separate landing validation, publish execution, restack and retarget operations; preserve atomic publication boundaries.",
    "release-watch":
      "Separate stated-release evidence/acknowledgement from upstream polling and scheduler wakeup.",
    "task-store":
      "Separate graph edits, dispatch facts and publication state commits while retaining transactional consistency.",
    "plan-edits":
      "Separate operation schema and validation from operation-specific application; retain full envelope validation integrations.",
    "agent-session":
      "Separate session identity/lookup from author graph creation and process attachment.",
    "planner-notes": "Separate note envelope validation, delivery and reply receipt handling.",
    "run-state":
      "Separate node status and frontier records from activity, refusal and driver-claim records; status readers and activity views then link only their facet. The derived-status cache stays with the status facet.",
    "plan-validation":
      "Separate the graph value types run state records (NodeStatus, NodeRef, Landing, Settled) from graph and destination validation; run-state would then link the value types only, and validation reach the suites that validate.",
    "plan-criteria":
      "Already the task-document grammar alone; plan-model renders tasks through it, so a grammar change legitimately reaches every plan reader. Splitting it further returns nothing until rendering moves out of plan-model.",
    "plan-compat":
      "Reaches the plan-validation suites because validation gates on schema versions; separating the read-side version gate from the write-side retired-field refusal would leave writers alone linking the refusal.",
    "plan-preservation":
      "Reaches exactly plan-validation's suites, its only consumer; it narrows further only when plan-validation's own reach does (see that row).",
    "render-cost":
      "Separate read-byte accounting, elapsed timers and display formatting; only consumers of the changed counter or formatter depend on it.",
    processes:
      "Separate host identity, process liveness, subprocess launch and handle detachment; replace generic sys imports with those capability crates.",
    "event-vocabulary":
      "Separate scheduler, channel, publication and agent kind registries; assemble complete validation only in the journal registry integration.",
    diagnostics:
      "Move domain-specific refusal constructors/formatters to the owning subsystem; retain only the stable error envelope shared by all commands.",
    "plan-model":
      "Separate node/edge schema, executor configuration, publication criteria and plan-loading IO; field-specific readers use the corresponding schema facet. Common strict schema validation remains shared.",
    "event-payloads":
      "Separate scheduler, edit, publication and channel payload schemas from the generic bus envelope. Full history validators still compose all registries; a registry grammar change legitimately reaches them.",
    "run-record":
      "Separate launch identity, agent configuration and invocation provenance; retain a facade LaunchRecord with identical serialization. Whole-record strict decoding remains a shared grammar input.",
    "run-projection":
      "Separate scheduler frontier, branch/publication, channel debt and activity folds; views request their required facets. Full-state checkpoint compatibility retains a full-fold integration.",
    "planner-channel":
      "Separate envelope schema/allowlist, transport queue, delivery acknowledgements and document-layout validation. Reply transport does not compile scheduler-op validation.",
    "run-files":
      "Separate atomic replacement/durability, append-tail healing, JSON codec and range reads; read-only facets cease importing append/lock implementations. Atomic replacement fixes still run all writers.",
    "branch-template":
      "Separate branch-template parsing from expansion; commands validating plans need parsing, launch composition needs expansion.",
    "yaml-codec":
      "Keep generic strict codec stable; move plan, channel-layout and executor document adapters into their owning crates. A generic strict-decoder fix remains a real shared input.",
    "run-paths":
      "Separate run-id validation/root selection from checkpoint/channel/report/dispatch path builders, colocating each builder with its subsystem. Common root semantics remain shared.",
    "run-labels":
      "Separate label record codec/query from update/atomic commit; display commands depend only on the query and schema.",
    "journal-reader":
      "Separate envelope decoding/version checks, merge ordering, loss accounting and byte-range iteration. Full-history readers still compose the same strict checks.",
    "publication-query":
      "Separate landing evidence reread, branch status and debt classification; status needs landing evidence, debt commands need the corresponding debt facet.",
    "run-result":
      "Separate terminal settlement wire facts from per-node publication details and presentation; full results retains all facts, liveness readers need only settlement.",
    checkpoint:
      "Separate coverage/seal validation, bounded replay and derived-cache publication. Cache-writing readers still test the real cache writer.",
    "run-summary":
      "Separate summary record/query from append-driven Maintainer; host/runs listing need query, writers compose maintenance.",
    "append-stamp":
      "Separate stamp reading/comparison from publication; a changed shared stamp protocol remains an actual input of every stamp consumer.",
    "run-report":
      "Separate terminal result record reading from report atomic publication and report presentation; preserve the existing result wire shape.",
    "cross-run-query":
      "Separate reference parsing, upstream status snapshot IO and resolution-cache tracking; only the views consuming upstream state retain those facets.",
    "member-liveness":
      "Separate heartbeat wire decoding, freshness classification and heartbeat publication; observer views do not import publisher implementation.",
    "event-filter":
      "Separate selector AST, matcher and YAML document adapter; inline selectors need no file parser.",
  };
  for (const name of Object.keys(layout.crates)) {
    const count = Object.values(reach.suites).filter((suite) => suite.crates.has(name)).length;
    if (count <= result.suites / 2) continue;
    if (!furtherSplits[name]) throw new Error(`missing further split for broad crate ${name}`);
    subsystemLines.push(`| ${name} | ${count} | ${furtherSplits[name]} |`);
  }
  subsystemLines.push(
    "",
    ...forcedTimeLines(result, reach, mixedEngineRows),
    "",
    ...testFakeLines(result, reach),
  );
  const glob = compareGlobInputs(result.mapping.selection, result.rows);
  const globLines = [
    "## What narrowing the docs/scripts globs frees",
    "",
    `Holding this proposed ${glob.suites}-suite graph fixed, ${glob.globPRs} of the 144 PRs edit docs/ or scripts/. Retaining blanket docs/** and scripts/** inputs selects every suite for those edits. Replacing those blanket inputs with each suite's actual fixture/script inputs frees at least one suite on **${glob.freed.length} PRs**; **${glob.fullyFreed}** of those can skip all suites. It removes **${glob.suiteRunsSaved} suite runs** across the cohort. Suite-run skipping is ${(glob.wideSkip * 100).toFixed(2)}% with blanket inputs and ${(glob.narrowSkip * 100).toFixed(2)}% with the proposed inputs. This isolates the glob effect on the same graph; it is not a claim that today's single binary already has the proposed isolation. Required real fixture inputs remain.`,
    "",
    "| PR / revision freed by narrower inputs | Changed docs/scripts inputs | Blanket selected | Proposed selected |",
    "| --- | --- | ---: | ---: |",
    ...glob.freed.map(
      (row) =>
        `| ${/#(\d+)\)/.exec(row.subject)?.[0]?.replace(")", "") ?? row.commit.slice(0, 8)} / ${row.commit.slice(0, 8)} | ${cell(row.files.join(", "))} | ${row.before.length} | ${row.after.length} |`,
    ),
    "",
  ];
  const partition = {};
  for (const [id, disposition] of Object.entries(catalog.dispositions))
    if (baseline.tests[id].suite === "cross-command")
      partition[disposition.kind] = (partition[disposition.kind] ?? 0) + 1;
  const lines = [
    TARGET_MARKER,
    "",
    "## Target replay",
    "",
    `The original 1,273 cross-command assignments partition as ${Object.entries(partition)
      .map(([kind, count]) => `${kind}: ${count}`)
      .join(
        "; ",
      )}. All ${Object.keys(catalog.dispositions).length} original tests remain represented below.`,
    "",
    ...comparison,
    commandAppendix(
      {
        ...result,
        rows: result.rows.map((row) => ({
          ...row,
          reasons: row.reasons.map((reason) =>
            reason.replace(
              "resolved dependencies/toolchain:",
              "Cargo.lock / compiler projection consumers:",
            ),
          ),
        })),
      },
      layout,
      catalog,
    )
      .replace("<!-- BEGIN COMMAND REPLAY -->\n", "")
      .replace(
        "Nx/recipe/config changes remain global.",
        "Nx/package-manager changes remain global; just recipes and Cargo/nextest sections are projected only to test targets which consume them.",
      )
      .replaceAll(
        "Compiler change: no; other inputs: only after their stated extraction.",
        "No: uniform compiler changes reach all Rust crates; other listed inputs are already projected.",
      )
      .replaceAll(
        "Only command fragments/borrowed helper attribution can narrow; actual shared behavior remains.",
        "The subsystem table names further capability splits for broad crates. Common protocol/compiler changes retain actual consumers; ambiguous historical hunks conservatively overselect.",
      )
      .replaceAll(
        "scripts/modularization/command-tests.json",
        "scripts/modularization/command-tests-target.json",
      )
      .replace(
        "`Cargo.toml` remains conservative until manifests are extracted: splitting package metadata and per-crate requirements removes unrelated manifest invalidation.",
        "Manifest edits are projected by each crate's actual external roots, with shared edition/profile/resolver/features affecting their real consumers.",
      )
      .replace(
        "Exact test names and individual integration reasons",
        "Expanded phase assignments and individual integration reasons",
      )
      .replaceAll(
        "Unrelated manifest edits: yes, split manifests; shared requirement/source changes: no.",
        "Unrelated manifest edits are already projected; actual shared requirements/source changes: no.",
      ),
    ...globLines,
    ...engineLines,
    ...subsystemLines,
    "## CLI argument ownership",
    "",
    "Each argument type and enum variant below moves to its named command crate. Cli and registration enums stay in the compatibility routing facade; cli-schema retains only genuinely shared argument value types. The old public paths re-export these identical types. A command flag hunk selects its command, while routing registration changes select the shipped-dispatch integration. The primary whole-file ownership table is overridden by these item moves.",
    "",
    "| Argument item / registration variant | Destination |",
    "| --- | --- |",
    ...Array.from(
      cliArgumentOwners(
        reviewed("src/cli.rs"),
        commandPaths(JSON.parse(reviewed("scripts/modularization/cli-commands.json"))),
      ),
    ).map(
      ([symbol, owner]) =>
        `| ${symbol} | ${owner instanceof Set ? `${[...owner].join(", ")} (duplicate argument definition, preserving flags)` : owner} |`,
    ),
    "",
    "## Crates, direct edges and primary source ownership",
    "",
    "Every Rust source file has exactly one primary owner after layout.json and engine-subsystems.json are applied. Symbol extractions below override the primary owner for the named items, including their private helpers; source wrappers become re-exports. engine-subsystems.json replaces the former whole-module unions with explicit subsystem interfaces; remaining edges are real proposed capabilities. This graph proves modeled reachability and acyclicity, not a compiled refactor.",
    "",
    "| Crate | Primary files | Direct dependencies |",
    "| --- | --- | --- |",
  ];
  for (const [name, crate] of Object.entries(layout.crates))
    lines.push(
      `| ${name} | ${crate.files.join(", ") || "extracted items / command adapter"} | ${crate.deps.join(", ")} |`,
    );
  lines.push(
    "",
    "## Named symbol moves",
    "",
    "| Source | Symbols | Destination | Preservation / reason |",
    "| --- | --- | --- | --- |",
  );
  for (const move of [
    ...moves.extractions,
    ...moves.edgeMoves,
    ...(moves.wholeModules ?? []),
    moves.recordFiles,
    ...(moves.sharedSlices ?? []),
    ...(moves.commandSlices ?? []),
  ])
    lines.push(
      `| ${move.file} | ${move.symbols.join(", ")} | ${move.shared ?? move.replacement ?? move.crate} | ${cell(move.reason)} |`,
    );
  lines.push(
    "",
    "## Every CLI route and its test dependencies",
    "",
    "Command phases depend on their own command and its shared concepts. The retained journey column names every integration or parser/routing exception that actually invokes the command, including commands exercised through legacy-named helpers. The exception runner closures are listed immediately below; these are the only exceptions to command isolation.",
    "",
    "| CLI route | Command crate | Command phase IDs | Retained integration / routing test IDs | Command runner closure |",
    "| --- | --- | --- | --- | --- |",
  );
  const commands = commandPaths(JSON.parse(reviewed("scripts/modularization/cli-commands.json")));
  for (const [path, crate] of Object.entries(commands)) {
    const tests = Object.entries(catalog.tests)
      .filter(([, t]) => t.suite === crate)
      .map(([id]) => id);
    const retained = Object.entries(catalog.tests)
      .filter(
        ([, test]) =>
          ["cross-command", "full-dispatch"].includes(test.suite) && test.commands.includes(crate),
      )
      .map(([id, test]) => `${id} (${test.suite})`);
    lines.push(
      `| ${path} | ${crate} | ${tests.join(", ") || "none"} | ${retained.join(", ") || "none"} | ${[...(reach.suites[crate]?.crates ?? [])].sort().join(", ")} |`,
    );
  }
  lines.push("", "| Exception suite | Complete runner crate closure |", "| --- | --- |");
  for (const suite of ["cross-command", "full-dispatch"])
    lines.push(`| ${suite} | ${[...(reach.suites[suite]?.crates ?? [])].sort().join(", ")} |`);
  const audit = loadAttributions(commands, projectRoot);
  lines.push(
    "",
    "## Source-backed argv and boundary audit",
    "",
    "Every exception below is checked against both its complete function definition and its source module SHA-256. Changes to helper argv, constants or assertions refuse regeneration until reviewed again. Unresolved argv is never a dispatcher contract. The former 499 dispatch dispositions (336 within the original cross-command bucket) at bc07e51b are all re-evaluated; only the explicit parser/routing cases remain full-dispatch.",
    "",
    "| Source function | Commands / boundary | Reviewed evidence | Function SHA-256 |",
    "| --- | --- | --- | --- |",
  );
  for (const [id, entry] of Object.entries(audit))
    lines.push(
      `| ${id} | ${entry.commands.join(", ") || entry.kind || entry.boundary || "forward calling argv"}${entry.forward ? `; argument ${entry.forward.argument}, prefix ${entry.forward.prefix.join(" ") || "none"}` : ""} | ${cell(entry.reason)} | ${entry.sha256} |`,
    );
  lines.push(
    "",
    "## Every original test and retained integration",
    "",
    "Each row preserves all assertions. Decomposed rows move setup exit/output assertions to the producer's phase and consumer assertions to the consumer's phase over recorded fixture preconditions. Phase-local std/serde builders prepare those files; the real consumer validates them. Assertions about producer output, durability or ordering remain in the producer phase or a complete integration. Retained integrations exercise the listed commands in the original sequence, with the live ordering in the reason. full-dispatch retains parser/routing assertions against the shipped binary; public-api retains facade identity/parity assertions. Shared helper imports and fixture paths are explicit runner inputs in the replay JSON.",
    "",
    "| Original test | Disposition | Destination command phases / integration participants | Assertion/integration intent from its source | Why |",
    "| --- | --- | --- | --- | --- |",
  );
  for (const [id, entry] of Object.entries(catalog.dispositions)) {
    const pieces = Object.entries(catalog.tests).filter(([, test]) => test.original === id);
    lines.push(
      `| ${id} | ${entry.kind} | ${entry.originalCommands.join(", ") || pieces.map(([, t]) => t.suite).join(", ")} | ${cell(intentFor(id))} | ${cell(pieces[0][1].reason)} |`,
    );
  }
  lines.push(
    "",
    "## Unit tests crossing an extracted item",
    "",
    "These test bodies reference an extracted symbol. Move each with its sole command/concept owner, or into a test-only integration project for the listed multiple owners; replace crate-relative imports with owner-crate imports and keep every assertion. Other unit tests follow their primary source owner. This list is conservative lexical reference detection, including same-name references; Rust compilation decides the precise imports during implementation.",
    "",
    "| Existing source test | Referenced destination owners |",
    "| --- | --- |",
  );
  const subsystemSites = Object.entries(layout.engineSubsystems.subsystems).flatMap(
    ([crate, entry]) => entry.items.map((item) => ({ ...item, crate })),
  );
  const subsystemOwner = new Map(
    subsystemSites.flatMap((site) =>
      site.symbols.map((symbol) => [`${site.file}::${symbol}`, site.crate]),
    ),
  );
  const sites = [
    ...moves.extractions,
    ...moves.edgeMoves,
    ...(moves.wholeModules ?? []),
    moves.recordFiles,
    ...(moves.sharedSlices ?? []),
    ...(moves.commandSlices ?? []),
  ]
    .map((site) => ({
      ...site,
      symbols: site.symbols.filter(
        (symbol) =>
          !subsystemOwner.has(`${site.file}::${symbol}`) ||
          subsystemOwner.get(`${site.file}::${symbol}`) ===
            (site.shared ?? site.replacement ?? site.crate),
      ),
    }))
    .concat(subsystemSites);
  for (const file of [...new Set(sites.map((site) => site.file))])
    for (const test of functions(reviewed(file)).filter((entry) => entry.test)) {
      const owners = [
        ...new Set(
          sites
            .filter(
              (site) =>
                site.file === file &&
                ((moves.wholeModules ?? []).some((move) => move.file === file) ||
                  site.symbols.some((symbol) => new RegExp(`\\b${symbol}\\b`).test(test.code))),
            )
            .map((site) => site.shared ?? site.replacement ?? site.crate),
        ),
      ];
      if (owners.length)
        lines.push(
          `| ${file}::${test.name} | ${owners.join(", ")}${(moves.wholeModules ?? []).some((move) => move.file === file) ? "; test-only integration keeps all real cross-document/lease loader calls" : ""} |`,
        );
    }
  return `${lines.join("\n")}\n`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length !== 3) throw new Error("usage: target-report.mjs REPLAY.json");
    const doc = "docs/modularization-target-layout.md";
    const prefix = readFileSync(doc, "utf8").split(TARGET_MARKER);
    if (prefix.length !== 2) throw new Error("expected one target layout marker");
    const evidence = JSON.parse(readFileSync(process.argv[2], "utf8"));
    if (
      !evidence ||
      Object.keys(evidence).some((key) => !["layout", "moves", "catalog", "result"].includes(key))
    )
      throw new Error("unknown target evidence field");
    writeFileSync(doc, `${prefix[0]}${targetAppendix(evidence)}`);
    console.log(
      `target layout: rendered ${evidence.result.originalTests} original tests in ${evidence.result.tests} phases`,
    );
  } catch (error) {
    const next =
      error.code && /^(?:EACCES|EPERM|ENOENT|EISDIR|ENOTDIR|EEXIST|ENOSPC|EROFS)$/.test(error.code)
        ? "check the named path's type, existence, permissions and disk space, and that the output parent is writable; rerun the same target-report.mjs invocation"
        : "generate a target replay and restore exactly one target layout marker";
    console.error(`${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}
