import { readFileSync } from "node:fs";
import { declarations } from "./source-projection.mjs";
import { compileMapping } from "../crate-split-replay.mjs";

/** Apply the reviewed subsystem ports, not the former aggregate-module dependency unions. */
export function engineGraph(layout, proposal, root = ".") {
  if (
    proposal?.schema !== 1 ||
    Object.keys(proposal).some((key) => !["schema", "subsystems"].includes(key)) ||
    !proposal.subsystems ||
    Array.isArray(proposal.subsystems) ||
    typeof proposal.subsystems !== "object"
  )
    throw new Error("invalid engine subsystem proposal");
  const result = structuredClone(layout);
  const entries = Object.entries(proposal.subsystems);
  const sourceFiles = new Set(Object.values(layout.crates).flatMap((crate) => crate.files));
  const names = new Set([...Object.keys(result.crates), ...entries.map(([name]) => name)]);
  const movedFiles = new Set();
  const itemOwners = new Map();
  for (const [name, entry] of entries) {
    if (
      !entry ||
      typeof entry !== "object" ||
      Array.isArray(entry) ||
      !/^[a-z][a-z0-9-]*$/.test(name) ||
      name in Object.prototype ||
      (name.startsWith("cmd-") && !Object.hasOwn(layout.crates, name)) ||
      Object.keys(entry).some(
        (key) => !["files", "items", "deps", "interface", "external"].includes(key),
      ) ||
      !Array.isArray(entry.files) ||
      !Array.isArray(entry.items) ||
      !Array.isArray(entry.deps) ||
      entry.deps.some((dep) => !names.has(dep) || dep === name) ||
      typeof entry.interface !== "string" ||
      !entry.interface.trim()
    )
      throw new Error(`invalid subsystem ${name}`);
    result.crates[name] ??= { files: [], deps: [] };
    if (entry.external !== undefined) {
      if (
        !Array.isArray(entry.external) ||
        entry.external.some((root) => typeof root !== "string" || !/^[a-z][a-z0-9_-]*$/.test(root))
      )
        throw new Error(`invalid subsystem external roots ${name}`);
      result.externalRoots[name] = entry.external;
    }

    for (const file of entry.files) {
      if (movedFiles.has(file)) throw new Error(`duplicate subsystem file ${file}`);
      movedFiles.add(file);
      const owners = Object.values(result.crates).filter((crate) => crate.files.includes(file));
      if (owners.length !== 1) throw new Error(`unknown or multiply owned subsystem file ${file}`);
      owners[0].files = owners[0].files.filter((path) => path !== file);
      result.crates[name].files.push(file);
      if (result.commands[file] && !name.startsWith("cmd-")) delete result.commands[file];
    }
    for (const item of entry.items) {
      if (
        !item ||
        typeof item !== "object" ||
        Array.isArray(item) ||
        Object.keys(item).some((key) => !["file", "symbols"].includes(key)) ||
        !sourceFiles.has(item.file) ||
        !Array.isArray(item.symbols) ||
        !item.symbols.length ||
        item.symbols.some((symbol) => typeof symbol !== "string" || !/^[A-Za-z_]\w*$/.test(symbol))
      )
        throw new Error(`invalid subsystem items ${name}`);
      const source = declarations(readFileSync(`${root}/${item.file}`, "utf8"));
      for (const symbol of item.symbols) {
        const key = `${item.file}::${symbol}`;
        if (!source.some((decl) => decl.name === symbol))
          throw new Error(`missing subsystem item ${key}`);
        if (itemOwners.has(key)) throw new Error(`duplicate subsystem item ${key}`);
        itemOwners.set(key, name);
      }
      result.symbolSources[name] ??= [];
      const existing = result.symbolSources[name].find((source) => source.file === item.file);
      if (existing) existing.symbols = [...new Set([...existing.symbols, ...item.symbols])];
      else result.symbolSources[name].push(structuredClone(item));
      result.historicalPaths[name] = [
        ...new Set([...(result.historicalPaths[name] ?? []), item.file]),
      ];
    }
    const before = result.crates[name].deps;
    result.crates[name].deps = [...new Set(entry.deps)];
    for (const dep of before.filter((dep) => !entry.deps.includes(dep)))
      result.targetFixes.push({
        from: name,
        to: dep,
        replacement: "consumer-owned subsystem port",
        reason: entry.interface,
      });
  }
  // The published facade preserves every original public path after the moves.
  result.crates.onepipeline.deps = [
    ...new Set([
      ...result.crates.onepipeline.deps,
      ...entries
        .map(([name]) => name)
        .filter((name) => !name.startsWith("cmd-") && !name.startsWith("test-")),
    ]),
  ];
  // An item's primary owner no longer owns that item: the facade re-exports it.
  for (const [name, sources] of Object.entries(result.symbolSources))
    result.symbolSources[name] = sources
      .map((item) => ({
        ...item,
        symbols: item.symbols.filter(
          (symbol) =>
            !itemOwners.has(`${item.file}::${symbol}`) ||
            itemOwners.get(`${item.file}::${symbol}`) === name,
        ),
      }))
      .filter((item) => item.symbols.length);
  const originalFiles = Object.values(layout.crates)
    .flatMap((crate) => crate.files)
    .sort();
  const finalFiles = Object.values(result.crates)
    .flatMap((crate) => crate.files)
    .sort();
  if (JSON.stringify(originalFiles) !== JSON.stringify(finalFiles))
    throw new Error("subsystem module partition changed source inventory");
  const mapping = compileMapping({
    crates: Object.fromEntries(
      Object.entries(result.crates).map(([name, crate]) => [
        name,
        { paths: crate.files, deps: crate.deps },
      ]),
    ),
    suites: Object.fromEntries(
      Object.keys(result.crates)
        .filter((name) => name.startsWith("cmd-"))
        .map((name) => [name, { paths: [], crates: [name] }]),
    ),
  });
  for (const [name, suite] of Object.entries(mapping.suites))
    if ([...suite.crates].some((dep) => dep.startsWith("cmd-") && dep !== name))
      throw new Error(`subsystem command isolation failed: ${name}`);
  const ownerOf = (file, symbol) =>
    itemOwners.get(`${file}::${symbol}`) ??
    Object.entries(result.crates).find(([, crate]) => crate.files.includes(file))?.[0];
  // A state-context port cannot erase the public Command payload's canonical Node type.
  const channel = declarations(readFileSync(`${root}/src/channel.rs`, "utf8")).find(
    (decl) => decl.name === "Command",
  );
  if (channel && /\bnode\s*:\s*Node\b/.test(channel.code)) {
    const from = ownerOf("src/channel.rs", "Command");
    const to = ownerOf("src/plan.rs", "Node");
    const reaches = (name, seen = new Set()) => {
      if (name === to) return true;
      if (seen.has(name)) return false;
      seen.add(name);
      return (result.crates[name]?.deps ?? []).some((dep) => reaches(dep, seen));
    };
    if (!from || !to || !reaches(from))
      throw new Error("channel Command must retain its canonical plan Node dependency");
  }
  // Every writer runner must wire the real cache maintainer, even with an injected observer.
  const journal = declarations(readFileSync(`${root}/src/journal.rs`, "utf8")).find(
    (decl) => decl.name === "Journal",
  );
  if (journal && /\bsummary\s*::\s*Maintainer\b/.test(journal.code)) {
    const writer = ownerOf("src/journal.rs", "Journal");
    const maintainer = ownerOf("src/summary.rs", "Maintainer");
    if (!result.crates[writer]?.deps.includes(maintainer))
      throw new Error("journal writer must retain real summary maintenance dependency");
    for (const [command, suite] of Object.entries(mapping.suites))
      if (suite.crates.has(writer) && !suite.crates.has(maintainer))
        throw new Error(`journal writer must retain real summary maintenance: ${command}`);
  }
  // Source-backed runtime paths remain linked even when driver admission is separated.
  const submit = declarations(readFileSync(`${root}/src/driver.rs`, "utf8")).find(
    (decl) => decl.name === "submit_envelope",
  );
  if (submit && /reconcile_queued/.test(submit.code))
    for (const command of ["cmd-reply", "cmd-attest"])
      if (!mapping.suites[command]?.crates.has(ownerOf("src/driver.rs", "submit_envelope")))
        throw new Error(`inline edit application missing: ${command}`);
  const trigger = readFileSync(`${root}/src/trigger.rs`, "utf8");
  if (/finish_undriven/.test(trigger))
    for (const command of ["cmd-review", "cmd-readjust"])
      if (!mapping.suites[command]?.crates.has(ownerOf("src/readjust.rs", "finish_undriven")))
        throw new Error(`undriven execution continuation missing: ${command}`);
  result.schema = 3;
  result.engineSubsystems = proposal;
  return result;
}
