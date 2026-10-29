#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { expandCatalog } from "./catalog.mjs";
import { sourceOwners, projectSourceChange } from "./source-projection.mjs";
import { targetGraph } from "./target-graph.mjs";
import { evidence, profileTable } from "./replay.mjs";
import { commandMapping, run } from "./command-replay.mjs";
import { mergedChanges } from "../crate-split-replay.mjs";

export function targetEvidence() {
  const { layout: base } = evidence();
  const moves = JSON.parse(readFileSync("scripts/modularization/target-moves.json", "utf8"));
  const layout = targetGraph(base, moves);
  const catalog = expandCatalog(
    JSON.parse(readFileSync("scripts/modularization/command-tests-target.json", "utf8")),
  );
  const changes = mergedChanges(".", {
    ref: "origin/main",
    since: "2026-09-15T00:00:00Z",
    until: "2026-10-16T00:00:00Z",
    exclude: "^chore: release",
  });
  if (changes.length !== 144) throw new Error(`expected 144 PRs, got ${changes.length}`);
  const owners = sourceOwners(layout, moves, catalog);
  const phaseMap = commandMapping(layout, catalog);
  for (const [suite, helpers] of Object.entries(phaseMap.helperConsumers))
    for (const helper of helpers) {
      const [module, rawName] = helper.split("::");
      const name = rawName.replace(/^\$/, "");
      const file = `tests/e2e/${module}.rs`;
      owners[file] ??= new Map();
      const users = owners[file].get(name) ?? new Set();
      users.add(`suite:${suite}`);
      owners[file].set(name, users);
    }
  const result = run(
    layout,
    catalog,
    profileTable(readFileSync("docs/test-profile-report.md", "utf8")),
    changes,
    (change) => projectSourceChange(change, owners, layout),
  );
  return { layout, moves, catalog, result };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length !== 3) throw new Error("usage: target-replay.mjs OUTPUT.json");
    const output = targetEvidence();
    writeFileSync(process.argv[2], `${JSON.stringify(output, null, 2)}\n`);
    console.log(
      `target replay: ${output.result.rows.length} PRs; ${output.result.suites} suites; ${(output.result.platforms.linux.suiteSkip * 100).toFixed(2)}% suite runs skipped; wrote ${process.argv[2]}`,
    );
  } catch (error) {
    const next =
      error.code && /^(?:EACCES|EPERM|ENOENT|EISDIR|ENOTDIR|EEXIST|ENOSPC|EROFS)$/.test(error.code)
        ? "check the named path's type, existence, permissions and disk space, and that the output parent is writable; rerun the same target-replay.mjs invocation"
        : "reconcile the target graph/test catalog with the source and 144-PR history; rerun target-replay.mjs OUTPUT.json";
    console.error(`${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}
