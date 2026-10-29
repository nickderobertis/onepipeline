#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { commandAppendix } from "./command-report.mjs";
import { compileMapping } from "../crate-split-replay.mjs";
import { commandPaths } from "./source-projection.mjs";
import { expandCatalog } from "./catalog.mjs";
import { functions } from "./rust-syntax.mjs";

export const TARGET_MARKER = "<!-- BEGIN TARGET LAYOUT -->";
const cell = (value) => String(value).replaceAll("|", "\\|").replaceAll("\n", " ");
const sourceTests = new Map();
export function intentFor(id) {
  if (typeof id !== "string" || !/^[a-z][a-z0-9_]*::[A-Za-z_]\w*$/.test(id))
    throw new Error("invalid test intent id");
  const [module, name] = id.split("::");
  if (!sourceTests.has(module)) {
    const text = readFileSync(`tests/e2e/${module}.rs`, "utf8");
    sourceTests.set(
      module,
      new Map(
        functions(text)
          .filter((fn) => fn.test)
          .map((fn) => {
            const lines = text.slice(0, fn.start).split("\n").reverse();
            const docs = [];
            for (const line of lines) {
              const trimmed = line.trim();
              if (trimmed.startsWith("///")) docs.unshift(trimmed.slice(3).trim());
              else if (docs.length) break;
              else if (trimmed && !trimmed.startsWith("#") && !trimmed.startsWith("//")) break;
            }
            return [
              fn.name,
              docs
                .filter((line) => !line.includes("llmlint:"))
                .join(" ")
                .replace(/\s+/g, " ")
                .slice(0, 600),
            ];
          }),
      ),
    );
  }
  const intent = sourceTests.get(module).get(name);
  if (intent === undefined) throw new Error(`missing test intent source ${id}`);
  return intent || name.replaceAll("_", " ");
}

/** Render the reviewed graph and every original test from the replay's own inputs. */
export function targetAppendix({ layout, moves, catalog, result }) {
  if (
    result?.schema !== 2 ||
    !Array.isArray(result.rows) ||
    result.rows.length !== 144 ||
    !Number.isInteger(result.tests) ||
    !Number.isInteger(result.suites) ||
    !result.mapping ||
    !catalog?.dispositions ||
    !layout?.crates ||
    !moves?.extractions
  )
    throw new Error("invalid target replay evidence");
  if (
    result.originalTests !== Object.keys(catalog.dispositions).length ||
    result.tests !== Object.keys(catalog.tests).length
  )
    throw new Error("target test/phase count mismatch");
  if (
    JSON.stringify(moves) !==
      JSON.stringify(
        JSON.parse(readFileSync("scripts/modularization/target-moves.json", "utf8")),
      ) ||
    JSON.stringify(catalog) !==
      JSON.stringify(
        expandCatalog(
          JSON.parse(readFileSync("scripts/modularization/command-tests-target.json", "utf8")),
        ),
      )
  )
    throw new Error("target evidence differs from the current reviewed moves/catalog");
  const reach = compileMapping(result.mapping.selection);
  const baseline = JSON.parse(readFileSync("scripts/modularization/command-tests.json", "utf8"));
  const partition = {};
  for (const [id, disposition] of Object.entries(catalog.dispositions))
    if (baseline.tests[id].suite === "cross-command")
      partition[disposition.kind] = (partition[disposition.kind] ?? 0) + 1;
  const lines = [
    TARGET_MARKER,
    "",
    "## Target replay",
    "",
    `The original 1,273 cross-command assignments partition as ${Object.entries(partition)
      .map(([kind, count]) => `${kind}: ${count}`)
      .join(
        "; ",
      )}. All ${Object.keys(catalog.dispositions).length} original tests remain represented below.`,
    "",
    commandAppendix(
      {
        ...result,
        rows: result.rows.map((row) => ({
          ...row,
          reasons: row.reasons.map((reason) =>
            reason.replace(
              "resolved dependencies/toolchain:",
              "Cargo.lock / compiler projection consumers:",
            ),
          ),
        })),
      },
      layout,
      catalog,
    )
      .replace("<!-- BEGIN COMMAND REPLAY -->\n", "")
      .replaceAll(
        "Compiler change: no; other inputs: only after their stated extraction.",
        "No: uniform compiler changes reach all Rust crates; other listed inputs are already projected.",
      )
      .replaceAll(
        "Only command fragments/borrowed helper attribution can narrow; actual shared behavior remains.",
        "No further removal in this design: these shared inputs/union retain their consumers; ambiguous source/helper hunks conservatively overselect.",
      )
      .replaceAll(
        "scripts/modularization/command-tests.json",
        "scripts/modularization/command-tests-target.json",
      )
      .replace(
        "`Cargo.toml` remains conservative until manifests are extracted: splitting package metadata and per-crate requirements removes unrelated manifest invalidation.",
        "Manifest edits are projected by each crate's actual external roots, with shared edition/profile/resolver/features affecting their real consumers.",
      )
      .replace(
        "Exact test names and individual integration reasons",
        "Expanded phase assignments and individual integration reasons",
      )
      .replaceAll(
        "Unrelated manifest edits: yes, split manifests; shared requirement/source changes: no.",
        "Unrelated manifest edits are already projected; actual shared requirements/source changes: no.",
      ),
    "## Crates, direct edges and primary source ownership",
    "",
    "Every Rust source file has exactly one primary owner in layout.json. Symbol extractions below override the primary owner for the named items, including their private helpers; source wrappers become re-exports. Dependencies retained from a former whole module are conservative: this graph proves reachability and acyclicity, not a compiled refactor.",
    "",
    "| Crate | Primary files | Direct dependencies |",
    "| --- | --- | --- |",
  ];
  for (const [name, crate] of Object.entries(layout.crates))
    lines.push(
      `| ${name} | ${crate.files.join(", ") || "extracted items / command adapter"} | ${crate.deps.join(", ")} |`,
    );
  lines.push(
    "",
    "## Named symbol moves",
    "",
    "| Source | Symbols | Destination | Preservation / reason |",
    "| --- | --- | --- | --- |",
  );
  for (const move of [
    ...moves.extractions,
    ...moves.edgeMoves,
    ...(moves.wholeModules ?? []),
    moves.recordFiles,
    ...(moves.sharedSlices ?? []),
    ...(moves.commandSlices ?? []),
  ])
    lines.push(
      `| ${move.file} | ${move.symbols.join(", ")} | ${move.shared ?? move.replacement ?? move.crate} | ${cell(move.reason)} |`,
    );
  lines.push(
    "",
    "## Every CLI route and its test dependencies",
    "",
    "The command suite column lists its phases; integration coverage remains in the explicitly listed exception set. A route with no isolated phase keeps its existing assertions in that set or in full-dispatch. The machine-readable catalog gives exact original names, not a sample.",
    "",
    "| CLI route | Command crate | Isolated phase IDs | Runner closure |",
    "| --- | --- | --- | --- |",
  );
  const commands = commandPaths(
    JSON.parse(readFileSync("scripts/modularization/cli-commands.json", "utf8")),
  );
  for (const [path, crate] of Object.entries(commands)) {
    const tests = Object.entries(catalog.tests)
      .filter(([, t]) => t.suite === crate)
      .map(([id]) => id);
    lines.push(
      `| ${path} | ${crate} | ${tests.join(", ") || "none; see retained integration/dispatcher assignments"} | ${[...(reach.suites[crate]?.crates ?? [])].sort().join(", ")} |`,
    );
  }
  lines.push(
    "",
    "## Every original test and retained integration",
    "",
    "Each row preserves all assertions. Decomposed rows move setup exit/output assertions to the producer's phase and consumer assertions to the consumer's phase over records written by the real production writers. Failure fixtures use the same envelope/schema validators and durable writers. Retained integrations exercise the listed commands in the original sequence, with the live ordering in the reason. full-dispatch retains parser/dynamic-argv assertions against the shipped binary; public-api retains facade identity/parity assertions. Shared helper imports and fixture paths are explicit runner inputs in the replay JSON.",
    "",
    "| Original test | Disposition | Destination command phases / integration participants | Assertion/integration intent from its source | Why |",
    "| --- | --- | --- | --- | --- |",
  );
  for (const [id, entry] of Object.entries(catalog.dispositions)) {
    const pieces = Object.entries(catalog.tests).filter(([, test]) => test.original === id);
    lines.push(
      `| ${id} | ${entry.kind} | ${entry.originalCommands.join(", ") || pieces.map(([, t]) => t.suite).join(", ")} | ${cell(intentFor(id))} | ${cell(pieces[0][1].reason)} |`,
    );
  }
  lines.push(
    "",
    "## Unit tests crossing an extracted item",
    "",
    "These test bodies reference an extracted symbol. Move each with its sole command/concept owner, or into a test-only integration project for the listed multiple owners; replace crate-relative imports with owner-crate imports and keep every assertion. Other unit tests follow their primary source owner. This list is conservative lexical reference detection, including same-name references; Rust compilation decides the precise imports during implementation.",
    "",
    "| Existing source test | Referenced destination owners |",
    "| --- | --- |",
  );
  const sites = [
    ...moves.extractions,
    ...moves.edgeMoves,
    ...(moves.wholeModules ?? []),
    moves.recordFiles,
    ...(moves.sharedSlices ?? []),
    ...(moves.commandSlices ?? []),
  ];
  for (const file of [...new Set(sites.map((site) => site.file))])
    for (const test of functions(readFileSync(file, "utf8")).filter((entry) => entry.test)) {
      const owners = [
        ...new Set(
          sites
            .filter(
              (site) =>
                site.file === file &&
                ((moves.wholeModules ?? []).some((move) => move.file === file) ||
                  site.symbols.some((symbol) => new RegExp(`\\b${symbol}\\b`).test(test.code))),
            )
            .map((site) => site.shared ?? site.replacement ?? site.crate),
        ),
      ];
      if (owners.length)
        lines.push(
          `| ${file}::${test.name} | ${owners.join(", ")}${(moves.wholeModules ?? []).some((move) => move.file === file) ? "; test-only integration keeps all real cross-document/lease loader calls" : ""} |`,
        );
    }
  return `${lines.join("\n")}\n`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length !== 3) throw new Error("usage: target-report.mjs REPLAY.json");
    const doc = "docs/modularization-target-layout.md";
    const prefix = readFileSync(doc, "utf8").split(TARGET_MARKER);
    if (prefix.length !== 2) throw new Error("expected one target layout marker");
    const evidence = JSON.parse(readFileSync(process.argv[2], "utf8"));
    if (
      !evidence ||
      Object.keys(evidence).some((key) => !["layout", "moves", "catalog", "result"].includes(key))
    )
      throw new Error("unknown target evidence field");
    writeFileSync(doc, `${prefix[0]}${targetAppendix(evidence)}`);
    console.log(
      `target layout: rendered ${evidence.result.originalTests} original tests in ${evidence.result.tests} phases`,
    );
  } catch (error) {
    console.error(
      `${error.message}\nnext: generate a target replay and restore exactly one target layout marker`,
    );
    process.exitCode = 1;
  }
}
