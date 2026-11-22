#!/usr/bin/env node
// Report the stop-verdict budgets from the full-workload journeys' validated
// telemetry. This command launches nothing, builds no fixture and times nothing.
import { basename, dirname } from "node:path";
import { report } from "@onebudgetspec/sdk";
import {
  hostShaped,
  journeyCommand,
  readRecord,
  slowest,
} from "./stop-guard-unpublished-record.mjs";

try {
  // `--record FILE` names the journeys' main record, read with the siblings beside it, so a job
  // without that tier's records reads the budget as unavailable from its argv alone.
  // `ONEPIPELINE_BUDGET_RECORDS` points the reader at another directory, which its tests use.
  const argv = process.argv.slice(2);
  const explicit = argv[0] === "--record";
  const file = explicit ? argv.splice(0, 2)[1] : undefined;
  if (explicit && basename(file ?? "") !== "stop-guard-unpublished.json") {
    throw new Error(`--record names ${file}, not the journeys' stop-guard-unpublished.json`);
  }
  const named = explicit ? dirname(file) : undefined;
  const args = argv.join(" ");
  const record = readRecord(process.env.ONEPIPELINE_BUDGET_RECORDS || named || undefined);
  let value;
  let description;
  if (args === "--journey-time") {
    value = record.total_ms / 1000;
    description =
      `empty-root preparation of every workload ${record.preparation_ms / 1000}s of ` +
      `${record.total_ms / 1000}s total, load1 ${record.load1} at its start`;
  } else if (args === "" || args === "--cold" || args === "--scale 10") {
    const scale = args === "--scale 10" ? 10 : 1;
    const mode = args === "--cold" ? "cold" : "warm";
    const worst = slowest(record, scale, mode);
    value = worst.median / 1e6;
    const sample = worst.scenario[mode];
    description =
      `${mode} stop-guard --unpublished, scale ${scale}: slowest scenario ` +
      `${worst.scenario.scenario}, median of 10 calls (max ${sample.max_us / 1e6}s) at load1 ` +
      `${sample.load1}; ${worst.scenario[`${mode}_git`]} git execution(s) in a separate call`;
  } else if (/^--host-shaped (unpublished-host|acknowledge|stop-guard)$/.test(args)) {
    const name = args.split(" ")[1];
    const read = hostShaped(record, name);
    value = read.warm.median_us / 1e6;
    const phases = Object.entries(read.phases_ms)
      .map(([phase, ms]) => `${phase} ${(ms / 1000).toFixed(3)}s`)
      .join(", ");
    const cold =
      read.cold_us === null
        ? ""
        : `cold first read ${read.cold_us / 1e6}s (stated, not budgeted); `;
    description =
      `host-shaped ${read.read}, aged state restored and verified before each call: median of ` +
      `10 calls (slowest ${read.warm.max_us / 1e6}s) at load1 ${read.warm.load1}; ${cold}` +
      `traced call: ${phases}; ${read.rederived} branch(es) re-derived; ${read.git} git ` +
      "execution(s) in a separate counting call";
  } else {
    throw new Error(
      "expected no arguments, --cold, --scale 10, --journey-time, or --host-shaped " +
        "<unpublished-host|acknowledge|stop-guard>",
    );
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
