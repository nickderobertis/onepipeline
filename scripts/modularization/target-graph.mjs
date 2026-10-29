#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { codeOnly, moduleGraph, validate } from "./graph.mjs";
import { engineGraph } from "./engine-graph.mjs";
import { compileMapping } from "../crate-split-replay.mjs";
import { commandPaths, cliPaths, cliArgumentOwners } from "./source-projection.mjs";

/** Apply symbol extractions to the source-derived graph; retain surplus edges conservatively. */
export function targetGraph(base, moves, root = ".") {
  if (
    ![1, 2, 3].includes(moves.schema) ||
    Object.keys(moves).some(
      (key) =>
        !(
          moves.schema === 3
            ? [
                "schema",
                "extractions",
                "edgeMoves",
                "recordFiles",
                "commandSlices",
                "sharedSlices",
                "commandDeps",
                "wholeModules",
                "conceptLinks",
                "renames",
              ]
            : moves.schema === 2
              ? ["schema", "extractions", "edgeMoves", "recordFiles"]
              : ["schema", "extractions", "edgeMoves"]
        ).includes(key),
    )
  )
    throw new Error("invalid target graph moves");
  for (const key of ["extractions", "edgeMoves", "sharedSlices", "commandSlices", "wholeModules"])
    if (
      (["extractions", "edgeMoves"].includes(key) || moves[key] !== undefined) &&
      !Array.isArray(moves[key])
    )
      throw new Error(`invalid target field ${key}`);
  for (const key of ["commandDeps", "conceptLinks", "renames"])
    if (
      moves[key] !== undefined &&
      (!moves[key] || typeof moves[key] !== "object" || Array.isArray(moves[key]))
    )
      throw new Error(`invalid target field ${key}`);
  const sourceGraph = moduleGraph(root);
  const derived = validate(base, sourceGraph, root);
  const crates = structuredClone(base.crates);
  for (const [name, entry] of Object.entries(derived.transformedCrates))
    crates[name].deps = [...entry.deps];
  const redirects = new Map();
  const attribution = {};
  const assets = structuredClone(base.assets ?? {});
  const verify = (entry) => {
    const allowed =
      entry.command === undefined
        ? ["from", "to", "replacement", "file", "symbols", "reason"]
        : ["command", "shared", "file", "symbols", "reason", "assets"];
    if (
      Object.keys(entry).some((key) => !allowed.includes(key)) ||
      typeof entry.reason !== "string" ||
      !entry.reason.trim() ||
      !Array.isArray(entry.symbols) ||
      !entry.symbols.length ||
      !entry.symbols.every((symbol) => typeof symbol === "string" && /^[A-Za-z_]\w*$/.test(symbol))
    )
      throw new Error("extraction needs symbols and a reason");
    if (!Object.hasOwn(sourceGraph.files, entry.file))
      throw new Error(`unknown extraction source ${entry.file}`);
    const source = codeOnly(readFileSync(`${root}/${entry.file}`, "utf8"));
    for (const symbol of entry.symbols)
      if (!new RegExp(`\\b(?:fn|struct|enum|type|const)\\s+(?:fn\\s+)?${symbol}\\b`).test(source))
        throw new Error(`missing extraction symbol ${entry.file}::${symbol}`);
  };
  const externalRoots = {};
  if (moves.schema >= 2) {
    const io = moves.recordFiles;
    if (
      !io ||
      Object.keys(io).some(
        (key) => !["crate", "file", "symbols", "deps", "external", "reason"].includes(key),
      ) ||
      !/^[a-z][a-z0-9-]*$/.test(io.crate) ||
      Object.hasOwn(crates, io.crate) ||
      !Array.isArray(io.deps) ||
      io.deps.some((dep) => !Object.hasOwn(crates, dep)) ||
      !Array.isArray(io.external) ||
      !io.external.every((root) => typeof root === "string" && /^[A-Za-z0-9_-]+$/.test(root))
    )
      throw new Error("invalid recordFiles extraction");
    verify({
      from: "run-record",
      to: io.crate,
      replacement: io.crate,
      file: io.file,
      symbols: io.symbols,
      reason: io.reason,
    });
    crates[io.crate] = { files: [], deps: io.deps, extract: io.reason };
    crates["run-record"].deps.push(io.crate);
    attribution[io.crate] = [io.file];
    externalRoots[io.crate] = io.external;
  }
  for (const entry of moves.extractions) {
    if (
      !crates[entry.command] ||
      !entry.command.startsWith("cmd-") ||
      !/^[a-z][a-z0-9-]*$/.test(entry.shared) ||
      redirects.has(entry.command)
    )
      throw new Error(`invalid extraction ${entry.command}`);
    verify(entry);
    if (
      entry.assets !== undefined &&
      (!Array.isArray(entry.assets) ||
        entry.assets.some(
          (asset) => typeof asset !== "string" || !(assets[entry.command] ?? []).includes(asset),
        ))
    )
      throw new Error(`invalid extracted asset ${entry.command}`);
    for (const asset of entry.assets ?? []) {
      assets[entry.command] = assets[entry.command].filter((path) => path !== asset);
      assets[entry.shared] = [...(assets[entry.shared] ?? []), asset];
    }
    redirects.set(entry.command, entry.shared);
    if (!crates[entry.shared])
      crates[entry.shared] = {
        files: [],
        deps: [...crates[entry.command].deps],
        extract: entry.reason,
      };
    attribution[entry.shared] = [...(attribution[entry.shared] ?? []), entry.file];
  }
  const composition = new Set(["onepipeline", base.dispatcher.crate]);
  const fixes = [];
  for (const [name, crate] of Object.entries(crates)) {
    crate.deps = [
      ...new Set(
        crate.deps
          .map((dep) => {
            if (!composition.has(name) && redirects.has(dep)) {
              fixes.push({
                from: name,
                to: dep,
                replacement: redirects.get(dep),
                reason: moves.extractions.find((e) => e.command === dep).reason,
              });
              return redirects.get(dep);
            }
            return dep;
          })
          .filter((dep) => dep !== name),
      ),
    ];
  }
  for (const entry of moves.extractions)
    if (!crates[entry.command].deps.includes(entry.shared))
      crates[entry.command].deps.push(entry.shared);
  for (const entry of moves.edgeMoves) {
    verify(entry);
    if (!crates[entry.from]?.deps.includes(entry.to) || !crates[entry.replacement])
      throw new Error(`unknown edge move ${entry.from}->${entry.to}`);
    crates[entry.from].deps = [
      ...new Set(
        crates[entry.from].deps.map((dep) => (dep === entry.to ? entry.replacement : dep)),
      ),
    ].filter((dep) => dep !== entry.from);
    attribution[entry.replacement] = [...(attribution[entry.replacement] ?? []), entry.file];
    fixes.push(entry);
  }
  for (const entry of [...(moves.sharedSlices ?? []), ...(moves.commandSlices ?? [])]) {
    if (
      Object.keys(entry).some(
        (key) => !["file", "crate", "symbols", "deps", "reason"].includes(key),
      )
    )
      throw new Error("unknown slice field");
    verify({
      from: entry.crate,
      to: entry.crate,
      replacement: entry.crate,
      file: entry.file,
      symbols: entry.symbols,
      reason: entry.reason,
    });
    if (!/^[a-z][a-z0-9-]*$/.test(entry.crate) || (entry.deps ?? []).some((dep) => !crates[dep]))
      throw new Error("invalid slice crate/dependency");
    crates[entry.crate] ??= { files: [], deps: [], extract: entry.reason };
    crates[entry.crate].deps = [...new Set([...crates[entry.crate].deps, ...(entry.deps ?? [])])];
    attribution[entry.crate] = [...new Set([...(attribution[entry.crate] ?? []), entry.file])];
  }
  for (const [command, deps] of Object.entries(moves.commandDeps ?? {})) {
    if (
      !command.startsWith("cmd-") ||
      !crates[command] ||
      !Array.isArray(deps) ||
      deps.some((dep) => !crates[dep])
    )
      throw new Error("invalid commandDeps");
    crates[command].deps = [...new Set([...crates[command].deps, ...deps])];
  }
  for (const entry of moves.wholeModules ?? []) {
    if (
      Object.keys(entry).some(
        (key) => !["file", "from", "crate", "symbols", "external", "reason", "deps"].includes(key),
      ) ||
      !crates[entry.from]?.files.includes(entry.file) ||
      !/^[a-z][a-z0-9-]*$/.test(entry.crate) ||
      (entry.external !== undefined &&
        (!Array.isArray(entry.external) ||
          !entry.external.every(
            (name) => typeof name === "string" && /^[a-z][a-z0-9-]*$/.test(name),
          ))) ||
      !Array.isArray(entry.deps ?? []) ||
      (entry.deps ?? []).some((dep) => !crates[dep])
    )
      throw new Error("invalid whole module move");
    verify({
      from: entry.from,
      to: entry.crate,
      replacement: entry.crate,
      file: entry.file,
      symbols: entry.symbols,
      reason: entry.reason,
    });
    const production = codeOnly(readFileSync(`${root}/${entry.file}`, "utf8")).split(
      /#\s*\[\s*cfg\(test\)\s*\]/,
    )[0];
    for (const match of production.matchAll(/\bcrate::([a-z][a-z0-9_]*)/g)) {
      const owner = Object.entries(base.crates).find(([, crate]) =>
        crate.files.some((file) => sourceGraph.files[file] === match[1]),
      )?.[0];
      if (!owner || !(entry.deps ?? []).includes(owner))
        throw new Error("whole codec module has an unmodeled production dependency");
    }
    crates[entry.from].files = crates[entry.from].files.filter((file) => file !== entry.file);
    crates[entry.crate] ??= { files: [], deps: [], extract: entry.reason };
    crates[entry.crate].files.push(entry.file);
    crates[entry.crate].deps = [...new Set([...crates[entry.crate].deps, ...(entry.deps ?? [])])];
    crates[entry.from].deps.push(entry.crate);
    if (entry.external) externalRoots[entry.crate] = entry.external;
    const module = sourceGraph.files[entry.file];
    for (const edge of sourceGraph.edges.filter((edge) => edge.to === module))
      for (const [name, crate] of Object.entries(crates))
        if (
          name !== entry.crate &&
          derived.transformedEdges.some(
            (moved) =>
              moved.source === `${edge.from}->${edge.to}` &&
              moved.from === name &&
              moved.to === entry.from,
          ) &&
          [...crate.files, ...(attribution[name] ?? [])].some(
            (file) => sourceGraph.files[file] === edge.from,
          )
        )
          crate.deps = [...new Set([...crate.deps, entry.crate])];
  }
  for (const [name, deps] of Object.entries(moves.conceptLinks ?? {})) {
    if (!crates[name] || !Array.isArray(deps) || deps.some((dep) => !crates[dep]))
      throw new Error("invalid conceptLinks");
    crates[name].deps = [...new Set([...crates[name].deps, ...deps])];
  }
  for (const [from, to] of Object.entries(moves.renames ?? {})) {
    if (
      !crates[from] ||
      typeof to !== "string" ||
      !/^[a-z][a-z0-9-]*$/.test(to) ||
      from === to ||
      Object.hasOwn(moves.renames, to)
    )
      throw new Error("invalid concept rename");
    crates[to] ??= { files: [], deps: [] };
    crates[to].files = [...new Set([...crates[to].files, ...crates[from].files])];
    crates[to].deps = [...new Set([...crates[to].deps, ...crates[from].deps])];
    attribution[to] = [...new Set([...(attribution[to] ?? []), ...(attribution[from] ?? [])])];
    delete attribution[from];
    delete crates[from];
  }
  for (const [name, crate] of Object.entries(crates))
    crate.deps = [...new Set(crate.deps.map((dep) => moves.renames?.[dep] ?? dep))].filter(
      (dep) => dep !== name,
    );
  const commands = commandPaths(
    JSON.parse(readFileSync(`${root}/scripts/modularization/cli-commands.json`, "utf8")),
  );
  cliPaths(readFileSync(`${root}/src/cli.rs`, "utf8"), commands);
  if (
    Object.values(commands).some((owner) => !crates[owner]) ||
    Object.keys(crates).some(
      (name) => name.startsWith("cmd-") && !Object.values(commands).includes(name),
    )
  )
    throw new Error("command crates must match actual CLI routes exactly");
  const compiled = compileMapping({
    crates: Object.fromEntries(
      Object.entries(crates).map(([name, c]) => [name, { paths: c.files, deps: c.deps }]),
    ),
    suites: Object.fromEntries(
      Object.keys(crates)
        .filter((name) => name.startsWith("cmd-"))
        .map((name) => [name, { paths: [], crates: [name] }]),
    ),
  });
  for (const [name, suite] of Object.entries(compiled.suites)) {
    const foreign = [...suite.crates].filter((dep) => dep.startsWith("cmd-") && dep !== name);
    if (foreign.length)
      throw new Error(`command isolation failed: ${name} reaches ${foreign.join(", ")}`);
  }
  return engineGraph(
    {
      ...base,
      crates,
      commands: Object.fromEntries(
        Object.entries(base.commands)
          .filter(([, owner]) => !moves.renames?.[owner] || moves.renames[owner].startsWith("cmd-"))
          .map(([file, owner]) => [file, moves.renames?.[owner] ?? owner]),
      ),
      suites: Object.fromEntries(
        Object.entries(base.suites).map(([name, suite]) => [
          name,
          { ...suite, crates: suite.crates.map((crate) => moves.renames?.[crate] ?? crate) },
        ]),
      ),
      auxiliarySuites: Object.fromEntries(
        Object.entries(base.auxiliarySuites).map(([name, suite]) => [
          name,
          { ...suite, crates: suite.crates.map((crate) => moves.renames?.[crate] ?? crate) },
        ]),
      ),
      historicalPaths: attribution,
      ...(Object.keys(assets).length ? { assets } : {}),
      ...(Object.keys(externalRoots).length ? { externalRoots } : {}),
      symbolSources: Object.fromEntries(
        Object.keys(crates).map((name) => [
          name,
          [
            ...moves.extractions,
            ...moves.edgeMoves,
            ...(moves.recordFiles ? [moves.recordFiles] : []),
            ...(moves.sharedSlices ?? []),
            ...(moves.commandSlices ?? []),
          ]
            .filter((entry) => (entry.shared ?? entry.replacement ?? entry.crate) === name)
            .map(({ file, symbols }) => ({ file, symbols }))
            .concat(
              [
                {
                  file: "src/cli.rs",
                  symbols: [
                    ...cliArgumentOwners(readFileSync(`${root}/src/cli.rs`, "utf8"), commands),
                  ]
                    .filter(
                      ([symbol, owner]) =>
                        (owner instanceof Set ? owner.has(name) : owner === name) &&
                        !symbol.startsWith("__"),
                    )
                    .map(([symbol]) => symbol),
                },
              ].filter((entry) => entry.symbols.length),
            ),
        ]),
      ),
      targetFixes: fixes,
      sourceProof: derived,
    },
    JSON.parse(readFileSync(`${root}/scripts/modularization/engine-subsystems.json`, "utf8")),
    root,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length > 3) throw new Error("usage: target-graph.mjs [OUTPUT.json]");
    const base = JSON.parse(readFileSync("scripts/modularization/layout.json", "utf8"));
    const moves = JSON.parse(readFileSync("scripts/modularization/target-moves.json", "utf8"));
    const result = targetGraph(base, moves);
    if (process.argv[2]) writeFileSync(process.argv[2], `${JSON.stringify(result, null, 2)}\n`);
    console.log(
      `target graph: ${Object.keys(result.crates).length} crates; acyclic; every command isolated; thin dispatcher verified from source`,
    );
  } catch (error) {
    console.error(
      `${error.message}\nnext: correct target-moves.json's named symbol or dependency; rerun target-graph.mjs [OUTPUT.json]`,
    );
    process.exitCode = 1;
  }
}
