import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";
import { digest } from "./workflow-graph.mjs";
try {
  const args = process.argv.slice(2);
  if (
    args.length !== 4 ||
    args[0] !== "--metric" ||
    args[2] !== "--telemetry" ||
    !["jobs-per-run", "runs-per-change"].includes(args[1])
  )
    throw new Error("usage: --metric jobs-per-run|runs-per-change --telemetry JSON");
  const telemetry = JSON.parse(readFileSync(args[3], "utf8"));
  if (
    telemetry.schema_version !== 1 ||
    telemetry.source_digest !== digest() ||
    !telemetry.scenarios?.length ||
    !telemetry.lifecycles?.length
  )
    throw new Error("missing or stale telemetry; run github-workflows:test");
  const candidates =
    args[1] === "jobs-per-run"
      ? telemetry.scenarios.flatMap((s) =>
          s.runs.map((r) => ({
            value: r.jobs.filter((j) => j.selected && j.macos).length,
            detail: { scenario: s.id, ...r, jobs: r.jobs.filter((j) => j.selected && j.macos) },
          })),
        )
      : telemetry.lifecycles.map((l) => ({
          value: l.runs.filter((r) => r.jobs.some((j) => j.selected && j.macos)).length,
          detail: {
            ...l,
            runs: l.runs
              .filter((r) => r.jobs.some((j) => j.selected && j.macos))
              .map((r) => ({ ...r, jobs: r.jobs.filter((j) => j.selected && j.macos) })),
          },
        }));
  const highest = Math.max(...candidates.map((c) => c.value));
  const detail = JSON.stringify(candidates.filter((c) => c.value === highest).map((c) => c.detail));
  if (!report(highest, detail))
    throw new Error("ONEBUDGETSPEC_RESULT missing; invoke through onebudgetspec");
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
