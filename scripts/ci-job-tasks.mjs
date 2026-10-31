// The task ids of an Nx `--graph=stdout` document, sorted and space-separated —
// what `scripts/ci-job.sh` selects, read as a parsed graph whose task map must
// be there. A document of another shape exits 1 naming what it lacked, rather
// than reading as an empty selection, which would pass a job that checked
// nothing.
//
// Usage: <nx ... --graph=stdout> | node scripts/ci-job-tasks.mjs
import { readFileSync } from "node:fs";

let text;
try {
  text = readFileSync(0, "utf8");
} catch (error) {
  process.stderr.write(
    `could not read the task graph from standard input: ${error.message}\n` +
      "ACTION: pipe an Nx command's --graph=stdout output into this script\n",
  );
  process.exit(1);
}
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
// Each id is `<project>:<target>[:<configuration>]`, naming the task its entry
// describes: the ids are printed space-separated and read back as words, so one
// of any other shape would be read as some other selection.
const ID = /^[A-Za-z0-9@._/-]+:[A-Za-z0-9._-]+(?::[A-Za-z0-9._-]+)?$/;
for (const [id, task] of Object.entries(tasks)) {
  const target = task?.target;
  if (
    !ID.test(id) ||
    typeof target?.project !== "string" ||
    typeof target?.target !== "string" ||
    id !==
      [target.project, target.target, target.configuration]
        .filter((part) => part !== undefined)
        .join(":")
  ) {
    process.stderr.write(
      `Nx answered a task map whose entry ${JSON.stringify(id)} is not a project:target task\n` +
        "ACTION: run the same Nx command with --graph=stdout and check its tasks.tasks entries\n",
    );
    process.exit(1);
  }
}
process.stdout.write(Object.keys(tasks).sort().join(" "));
