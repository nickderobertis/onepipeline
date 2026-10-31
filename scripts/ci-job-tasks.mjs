// The task ids of an Nx `--graph=stdout` document, sorted and space-separated —
// what `scripts/ci-job.sh` selects, read as a parsed graph whose task map must
// be there. A document of another shape exits 1 naming what it lacked, rather
// than reading as an empty selection, which would pass a job that checked
// nothing.
//
// Usage: <nx ... --graph=stdout> | node scripts/ci-job-tasks.mjs
import { readFileSync } from "node:fs";

const text = readFileSync(0, "utf8");
let graph;
try {
  graph = JSON.parse(text.slice(text.indexOf("{")));
} catch (error) {
  process.stderr.write(
    `Nx answered no JSON task graph: ${error.message}\n` +
      "ACTION: run the same Nx command with --graph=stdout and fix the error it prints before the graph\n",
  );
  process.exit(1);
}
const tasks = graph?.tasks?.tasks;
if (!tasks || typeof tasks !== "object" || Array.isArray(tasks)) {
  process.stderr.write(
    "Nx answered a graph with no task map\n" +
      "ACTION: check that the Nx command was given -t <targets>, which is what makes --graph=stdout list tasks\n",
  );
  process.exit(1);
}
process.stdout.write(Object.keys(tasks).sort().join(" "));
