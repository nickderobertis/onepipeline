#!/usr/bin/env node
// Replays the 144 PRs against the crate layout and the test split with the start, reply and adopt
// tests re-homed onto the engine crates engine-tests.json names, and writes that variant's figures.
// `--parents` re-homes only the named parents' tests, which is the layout once those migration
// steps have landed.
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { ENGINE_PATH, PARENTS, checkEngineSplit, targetLayout } from "./engine-tests.mjs";
import { classify, splitEvidence } from "./split-replay.mjs";
import { targetCatalog } from "./test-split.mjs";
import { affectedSuites, compileMapping } from "../crate-split-replay.mjs";

/** The figures the engine-tests document quotes for one replayed variant. */
export function engineSummary(result, parents) {
  const linux = result.platforms.linux;
  const share = (affected) =>
    affected.reduce((sum, suite) => sum + linux.suiteSeconds[suite], 0) / linux.secondsPerRun;
  const compiled = compileMapping(result.mapping.selection);
  const round = (value) => Math.round(value * 1e6) / 1e6;
  const overHalf = result.rows
    .map((row) => {
      const kinds = {};
      for (const file of row.effectiveFiles) {
        const kind = classify(file, row)[0];
        kinds[kind] ??= [];
        kinds[kind].push(file);
      }
      const forcing = Object.keys(kinds)
        .filter((kind) => share(affectedSuites(compiled, kinds[kind]).affected) > 0.5)
        .sort();
      return {
        pr: /#(\d+)\)/.exec(row.subject)?.[1] ?? null,
        commit: row.commit.slice(0, 8),
        share: round(share(row.affected)),
        forcing: forcing.length ? forcing : ["several inputs together"],
      };
    })
    .filter((row) => row.share > 0.5);
  const engineSuites = Object.entries(result.mapping.suites)
    .filter(([, suite]) => suite.engine)
    .map(([name, suite]) => {
      const closure = new Set();
      const visit = (crate) => {
        if (closure.has(crate)) return;
        closure.add(crate);
        for (const dep of result.mapping.crates[crate]?.deps ?? []) visit(dep);
      };
      visit(suite.engine);
      return {
        suite: name,
        crate: suite.engine,
        tests: suite.tests.length,
        closure: closure.size,
        linuxShare: round(linux.suiteSeconds[name] / linux.secondsPerRun),
        skipped: round(result.rows.filter((row) => !row.affected.includes(name)).length / 144),
      };
    })
    .sort((a, b) => (a.suite < b.suite ? -1 : 1));
  // What a change to each production crate alone selects, and how many PRs changed it.
  const crateReach = Object.keys(result.mapping.crates)
    .filter((crate) => !crate.startsWith("runner-"))
    .map((crate) => {
      const affected = affectedSuites(compiled, [`moved/${crate}/change`]).affected;
      return {
        crate,
        suites: affected.length,
        linuxShare: round(share(affected)),
        prs: result.rows.filter((row) =>
          row.effectiveFiles.some(
            (file) => file.startsWith(`moved/${crate}/`) || file.startsWith(`resolved/${crate}/`),
          ),
        ).length,
      };
    })
    .filter((entry) => entry.suites > result.suites / 2)
    .sort((a, b) => b.prs - a.prs || (a.crate < b.crate ? -1 : 1));
  return {
    schema: 1,
    parents,
    prs: result.rows.length,
    suites: result.suites,
    platforms: Object.fromEntries(
      ["linux", "macos", "windows", "local"].map((platform) => [
        platform,
        {
          suiteSkip: round(result.platforms[platform].suiteSkip),
          secondsSkip: round(result.platforms[platform].secondsSkip),
        },
      ]),
    ),
    broad: result.causes.broad,
    causes: result.causes.prs,
    sole: result.causes.sole,
    lock: result.causes.lock,
    wall: Object.fromEntries(
      Object.entries(result.wall).map(([platform, data]) => [
        platform,
        platform === "local"
          ? {
              testPhase: data.testPhase,
              median: round(data.projected.median),
              max: round(data.projected.max),
            }
          : {
              today: round(data.today),
              residual: round(data.residual),
              median: round(data.projected.median),
              max: round(data.projected.max),
              over: data.projected.over,
              typicalShare: round(data.typicalShare),
            },
      ]),
    ),
    overHalf,
    crateReach,
    engineSuites,
  };
}

/** Replays the layout with the named parents' tests re-homed. */
export function engineEvidence(parents = PARENTS) {
  const engineOf = checkEngineSplit(
    readFileSync(ENGINE_PATH, "utf8"),
    targetCatalog(),
    targetLayout(),
  );
  const split = JSON.parse(readFileSync(ENGINE_PATH, "utf8"));
  for (const id of engineOf.keys())
    if (!parents.includes(split.moved[id].parent)) engineOf.delete(id);
  return engineSummary(splitEvidence({ engineOf }).result, parents);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const args = process.argv.slice(2);
    const at = args.indexOf("--parents");
    const parents = at >= 0 ? (args[at + 1] ?? "").split(",") : PARENTS;
    if (at >= 0) args.splice(at, 2);
    if (
      args.length !== 1 ||
      !parents.length ||
      parents.some((parent) => !PARENTS.includes(parent)) ||
      new Set(parents).size !== parents.length
    )
      throw new Error(
        `usage: engine-replay.mjs OUTPUT.json [--parents ${PARENTS.join(",")}] (a subset, each once)`,
      );
    const summary = engineEvidence(parents);
    writeFileSync(args[0], `${JSON.stringify(summary, null, 2)}\n`);
    console.log(
      `engine replay (${parents.join(", ")}): ${summary.prs} PRs; ${summary.suites} suites; ${(summary.platforms.linux.suiteSkip * 100).toFixed(2)}% suite runs and ${(summary.platforms.linux.secondsSkip * 100).toFixed(2)}% Linux seconds skipped; wrote ${args[0]}`,
    );
  } catch (error) {
    const next =
      error.code && /^(?:EACCES|EPERM|ENOENT|EISDIR|ENOTDIR|EEXIST|ENOSPC|EROFS)$/.test(error.code)
        ? "check the named path's type, existence, permissions and disk space, and that the output parent is writable; rerun the same engine-replay.mjs invocation"
        : "reconcile engine-tests.json, test-split.json, the target graph and the 144-PR history; rerun engine-replay.mjs OUTPUT.json";
    console.error(`${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}
