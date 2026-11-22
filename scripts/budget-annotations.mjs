#!/usr/bin/env node
// One line per budget a `timed` target's saved check did not pass, read from the onebudgetspec
// results it saved: the budget, its figure and its threshold, or why it has no figure. Under
// GitHub Actions each line is an `::error` annotation, which is what the push-to-main `timed`
// job's red status shows. A results file that is missing is reported the same way, because the
// target that owed it stopped before its check.
//
// Usage: node scripts/budget-annotations.mjs <results.json>...
// Exit 0 once every file is read, whatever it holds: the job's verdict is the target's own.
// Exit 2 for no file named, or a file that is not a onebudgetspec results document.
import { readFileSync } from "node:fs";

const files = process.argv.slice(2);
if (files.length === 0) {
  console.error("budget-annotations: name the saved results files to read");
  console.error(
    "ACTION: run 'node scripts/budget-annotations.mjs target/budget-results/<project>.json'",
  );
  process.exit(2);
}

const annotate = process.env.GITHUB_ACTIONS === "true";
// A workflow command's data escapes `%`, CR and LF, and a property `:` and `,` as well.
const data = (text) => text.replaceAll("%", "%25").replaceAll("\r", "%0D").replaceAll("\n", "%0A");
const property = (text) => data(text).replaceAll(":", "%3A").replaceAll(",", "%2C");
const say = (title, message) =>
  console.log(
    annotate ? `::error title=${property(title)}::${data(message)}` : `${title}: ${message}`,
  );
const figure = (value, unit) => `${value} ${unit}`;

for (const file of files) {
  let report;
  try {
    report = JSON.parse(readFileSync(file, "utf8"));
  } catch (error) {
    if (error.code === "ENOENT") {
      say("timed budgets unchecked", `${file} is missing: its target stopped before its check`);
      continue;
    }
    console.error(`budget-annotations: ${file} is unreadable: ${error.message}`);
    console.error("ACTION: re-run the timed target that saves it, then read it again");
    process.exit(2);
  }
  const results = report?.results;
  const valid =
    report?.schema_version === 1 &&
    Array.isArray(results) &&
    results.every(
      (result) => typeof result?.id === "string" && typeof result?.verdict === "string",
    );
  if (!valid) {
    console.error(`budget-annotations: ${file} is not a onebudgetspec results document`);
    console.error("ACTION: save it with 'onebudgetspec check --save-results', then read it again");
    process.exit(2);
  }
  for (const result of results) {
    if (result.verdict === "over") {
      say(
        `budget ${result.id} over`,
        `${result.id} measured ${figure(result.actual, result.unit)}, over its threshold of ` +
          `${figure(result.threshold, result.unit)}${result.detail ? ` (${result.detail})` : ""}`,
      );
    } else if (result.verdict === "error") {
      say(
        `budget ${result.id} unmeasured`,
        `${result.id} has no figure against its threshold of ` +
          `${figure(result.threshold, result.unit)}: ${result.error ?? "no reason given"}`,
      );
    }
  }
}
