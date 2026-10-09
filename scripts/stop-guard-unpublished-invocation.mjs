#!/usr/bin/env node
// Mark the stop-verdict records as being regenerated before the producer starts, so a
// producer that fails or is skipped leaves nothing a budget reads as current.
import { randomBytes } from "node:crypto";
import { spawnSync } from "node:child_process";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const directory = fileURLToPath(new URL("../target/budget-records/", import.meta.url));
const run_id = randomBytes(32).toString("hex");
const manifest = (state) =>
  writeFileSync(
    `${directory}/stop-guard-unpublished-invocation.json`,
    JSON.stringify({ state, run_id }),
  );
const retract = () => rmSync(`${directory}/stop-guard-unpublished.json`, { force: true });
const failed = (reason) => {
  console.error(
    `stop-verdict producer: ${reason}\nnext: run 'just stop-verdict-journeys' to regenerate complete current-build records`,
  );
  try {
    manifest("failed");
    retract();
  } catch (error) {
    console.error(
      `stop-verdict producer: could not mark ${directory} failed (${error.message}); remove it before reading budgets`,
    );
  }
};
try {
  mkdirSync(directory, { recursive: true });
  manifest("started");
  retract();
} catch (error) {
  failed(`could not reset the records under ${directory}: ${error.message}`);
  process.exit(1);
}
const [program, ...args] = process.argv.slice(2);
if (!program) {
  failed("the producer command is missing");
  process.exitCode = 2;
} else {
  const result = spawnSync(program, args, {
    stdio: "inherit",
    env: { ...process.env, ONEPIPELINE_BUDGET_INVOCATION: run_id },
  });
  if (result.error || result.status !== 0) {
    failed(
      result.error?.message ??
        `${program} ${args.join(" ")} failed with ${result.signal ? `signal ${result.signal}` : `exit ${result.status}`}`,
    );
  }
  process.exitCode = result.status ?? 1;
}
