#!/usr/bin/env node
// Report the stop-verdict budgets from the full-workload journeys' validated
// telemetry. This command launches nothing, builds no fixture and times nothing.
import { report } from "@onebudgetspec/sdk";
import { journeyCommand, readRecord, slowest } from "./stop-guard-unpublished-record.mjs";

try {
  const args = process.argv.slice(2).join(" ");
  // `ONEPIPELINE_BUDGET_RECORDS` points the reader at another directory, which its
  // tests use; the budgets read the producing tier's `target/budget-records/`.
  const record = readRecord(process.env.ONEPIPELINE_BUDGET_RECORDS || undefined);
  let value;
  let description;
  if (args === "--journey-time") {
    value = record.total_ms / 1000;
    description =
      `empty-root preparation of both workloads ${record.preparation_ms / 1000}s of ` +
      `${record.total_ms / 1000}s total, load1 ${record.load1} at its start`;
  } else if (args === "" || args === "--cold" || args === "--scale 10") {
    const scale = args === "--scale 10" ? 10 : 1;
    const mode = args === "--cold" ? "cold" : "warm";
    const worst = slowest(record, scale, mode);
    value = worst.max / 1e6;
    const sample = worst.scenario[mode];
    description =
      `${mode} stop-guard --unpublished, scale ${scale}: slowest scenario ` +
      `${worst.scenario.scenario}, max of 10 calls (median ${sample.median_us / 1e6}s) at load1 ` +
      `${sample.load1}; ${worst.scenario[`${mode}_git`]} git execution(s) in a separate call`;
  } else {
    throw new Error("expected no arguments, --cold, --scale 10, or --journey-time");
  }
  if (!report(value, `invocation ${record.build.run_id}: ${description}`)) {
    throw new Error("ONEBUDGETSPEC_RESULT is missing; invoke through 'just budgets'");
  }
} catch (error) {
  console.error(
    `stop-verdict telemetry: ${error.message}\nnext: run '${journeyCommand}' to regenerate ` +
      "complete current-build records, then 'just budgets'",
  );
  process.exitCode = 1;
}
