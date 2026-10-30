// Maps the engine subsystem proposal onto the target layout and writes the resulting crate graph.
// Exit statuses follow cli.mjs.
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { engineGraph } from "./engine-graph.mjs";
import { targetGraph } from "./target-graph.mjs";
import { UsageError, exitStatus } from "./cli.mjs";

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length !== 5)
      throw new UsageError("usage: engine-graph-cli.mjs LAYOUT.json PROPOSAL.json OUTPUT.json");
    const layout = JSON.parse(readFileSync(process.argv[2], "utf8"));
    const canonical = targetGraph(
      JSON.parse(readFileSync("scripts/modularization/layout.json", "utf8")),
      JSON.parse(readFileSync("scripts/modularization/target-moves.json", "utf8")),
    );
    if (!isDeepStrictEqual(layout, canonical))
      throw new Error(
        "layout differs from the reviewed source-derived layout; regenerate it with target-graph.mjs OUTPUT.json",
      );
    const mapped = engineGraph(layout, JSON.parse(readFileSync(process.argv[3], "utf8")));
    writeFileSync(process.argv[4], JSON.stringify(mapped, null, 2));
    console.log(
      `engine graph: ${Object.keys(mapped.crates).length} crates; acyclic; commands isolated`,
    );
  } catch (error) {
    const next =
      error instanceof UsageError
        ? "pass the three paths the usage names"
        : error.code && /^(?:EACCES|EPERM|ENOENT|EISDIR|ENOTDIR|ENOSPC|EROFS)$/.test(error.code)
          ? "check the named path's type, existence, permissions and disk space, and that the output parent is writable"
          : "correct the named subsystem item or dependency in the proposal";
    console.error(
      `${error.message}\nnext: ${next}; rerun engine-graph-cli.mjs LAYOUT.json PROPOSAL.json OUTPUT.json`,
    );
    process.exitCode = exitStatus(error);
  }
}
