#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { loadAttributions } from "./attribution.mjs";
import { baselineHash } from "./catalog.mjs";
import { phaseInputs, commandCalls } from "./phase-inputs.mjs";
import { commandPaths, cliPaths } from "./source-projection.mjs";
import { moduleGraph, validate } from "./graph.mjs";
import { functions } from "./rust-syntax.mjs";
export { REASONS, expandCatalog } from "./catalog.mjs";

/** Reviewable dispositions, retaining live ordering rather than replacing it with fixture state. */
export function decompose(baseline) {
  if (
    baseline?.schema !== 1 ||
    Object.keys(baseline).some((key) => !["schema", "tests"].includes(key)) ||
    !baseline.tests ||
    typeof baseline.tests !== "object" ||
    Array.isArray(baseline.tests)
  )
    throw new Error("invalid baseline test catalog");
  for (const [id, entry] of Object.entries(baseline.tests))
    if (
      !/^[a-z][a-z0-9_]*::[A-Za-z_]\w*$/.test(id) ||
      !entry ||
      Object.keys(entry).some((key) => !["suite", "commands", "reason"].includes(key)) ||
      !Array.isArray(entry.commands) ||
      !entry.commands.every(
        (name) => typeof name === "string" && /^cmd-[a-z][a-z0-9-]*$/.test(name),
      ) ||
      typeof entry.reason !== "string"
    )
      throw new Error(`invalid baseline test ${id}`);
  const source = new Map();
  const dispositions = {};
  const configured = commandPaths(
    JSON.parse(readFileSync("scripts/modularization/cli-commands.json", "utf8")),
  );
  const attributions = loadAttributions(configured);
  cliPaths(readFileSync("src/cli.rs", "utf8"), configured);
  const knownCommands = new Set(Object.values(baseline.tests).flatMap((entry) => entry.commands));
  if (Object.values(configured).some((owner) => !knownCommands.has(owner)))
    throw new Error("unmapped CLI command crate");
  const legacy = [...knownCommands].filter((name) => !Object.values(configured).includes(name));
  if (legacy.length) {
    const oldLayout = JSON.parse(readFileSync("scripts/modularization/layout.json", "utf8"));
    validate(oldLayout, moduleGraph());
    if (legacy.some((name) => !Object.hasOwn(oldLayout.crates, name)))
      throw new Error("unmapped baseline command crate");
  }
  const liveName =
    /while|wakes|wake_|outlast|killed|handover|hand_off|takes_over|interrupt|concurr|race|signal|in_flight|still_running|still_working|before.*(?:claim|reply|publish|release)|after.*(?:kill|die|death)|takeover|before.*(?:project|dispatch)|shutdown/;
  const controls = new Set([
    "cmd-adopt",
    "cmd-reply",
    "cmd-review",
    "cmd-readjust",
    "cmd-attest",
    "cmd-stop",
    "cmd-shutdown",
    "cmd-note",
  ]);
  for (const [id, test] of Object.entries(baseline.tests)) {
    const [module, name] = id.split("::");
    if (!source.has(module))
      source.set(
        module,
        new Map(
          functions(readFileSync(`tests/e2e/${module}.rs`, "utf8"))
            .filter((f) => f.test)
            .map((f) => [f.name, f.body]),
        ),
      );
    const body = source.get(module).get(name);
    if (!body) throw new Error(`missing test body ${id}`);
    const facts = phaseInputs(
      { original: id, suite: "cross-command" },
      { kind: "isolated", originalCommands: test.commands },
      configured,
    );
    const unresolvedArgv = facts.dynamicCli && !attributions[id];
    const commandRoots = attributions[id]?.commands ?? facts.commands;
    const controlAt = commandCalls(body, configured).find((call) => controls.has(call.command))?.at;
    const pollAfterControl =
      controlAt !== undefined &&
      /\.(?:until|until_within|until_run_file_holds)\s*\(/.test(body.slice(controlAt));
    const liveControl =
      commandRoots.some((command) => controls.has(command)) &&
      (pollAfterControl || /end_driver\s*\(|\.kill\s*\(/.test(body));
    const concurrentObserver =
      commandRoots.some((c) => ["cmd-watch", "cmd-observer-view", "cmd-next"].includes(c)) &&
      /\.spawn\s*\(|thread::spawn|\.try_wait\s*\(/.test(body);
    const liveEvidence =
      /\.(?:until|spawn|wait|try_wait|kill)\s*\(|end_driver|rendezvous|Instant|thread::|signal/.test(
        body,
      );
    const integration =
      commandRoots.length > 1 &&
      ((liveName.test(name) && liveEvidence) || liveControl || concurrentObserver);
    const kind =
      attributions[id]?.kind ??
      (unresolvedArgv
        ? "dispatch"
        : !commandRoots.length
          ? facts.pipeline
            ? "dispatch"
            : facts.publicApi
              ? "library"
              : "support"
          : integration
            ? "integration"
            : commandRoots.length > 1
              ? "decompose"
              : "isolated");
    if (kind === "dispatch" && attributions[id]?.kind !== "dispatch")
      throw new Error(
        `unreviewed dispatcher attribution ${id}; resolve argv or explicitly prove parser/routing assertions`,
      );
    const reason =
      kind === "dispatch"
        ? "dispatch-contract"
        : kind === "integration"
          ? liveControl
            ? "live-control"
            : concurrentObserver
              ? "live-observer"
              : "live-ownership"
          : kind === "support"
            ? "support-boundary"
            : kind === "library"
              ? "public-api"
              : kind === "dispatch"
                ? "dispatch-contract"
                : kind === "decompose"
                  ? "command-phases"
                  : "isolated-command";
    dispositions[id] = {
      kind,
      originalCommands: commandRoots,
      reason,
      ...(attributions[id] ? { rationale: attributions[id].reason } : {}),
    };
  }
  return { schema: 4, baselineSha256: baselineHash(baseline), dispositions };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length !== 2) throw new Error("usage: decompose.mjs");
    const result = decompose(
      JSON.parse(readFileSync("scripts/modularization/command-tests.json", "utf8")),
    );
    writeFileSync(
      "scripts/modularization/command-tests-target.json",
      `${JSON.stringify(result, null, 2)}\n`,
    );
    console.log(
      `decomposition: ${Object.keys(result.dispositions).length} original tests classified`,
    );
  } catch (error) {
    console.error(
      `${error.message}\nnext: reconcile the baseline catalog with the current e2e source, then rerun decompose.mjs`,
    );
    process.exitCode = 1;
  }
}
