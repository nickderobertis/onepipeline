#!/usr/bin/env node
import { baselineHash, expandCatalog } from "./catalog.mjs";
import { functions } from "./rust-syntax.mjs";
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import {
  cargoManifest,
  dependencyDefinitions,
  manifestChanges,
  toolchainFingerprint,
} from "./manifest-projection.mjs";
import { historicalConfigChanges } from "./config-projection.mjs";
import { phaseInputs } from "./phase-inputs.mjs";
import { cliPaths, declarations, commandPaths } from "./source-projection.mjs";
import { codeOnly, moduleGraph, validate } from "./graph.mjs";
import { suiteInputs } from "./suite-inputs.mjs";
import { COMMAND_MARKER, commandAppendix } from "./command-report.mjs";
import { evidence } from "./replay.mjs";
import { affectedSuites, compileMapping, mergedChanges } from "../crate-split-replay.mjs";

const read = (path) => readFileSync(path, "utf8");
const sorted = (items) => [...new Set(items)].sort();
const git = (...args) =>
  execFileSync("git", args, { encoding: "utf8", maxBuffer: 32 * 1024 * 1024 });

export { functions } from "./rust-syntax.mjs";

/** Explicit extraction inventory, used for review; never silently regenerate ownership at replay time. */
export function inventory(layout) {
  const modules = [...read("tests/e2e/main.rs").matchAll(/^mod (\w+);/gm)].map((m) => m[1]);
  const sources = Object.fromEntries(modules.map((m) => [m, read(`tests/e2e/${m}.rs`)]));
  const parsed = Object.fromEntries(modules.map((m) => [m, functions(sources[m])]));
  const commands = Object.keys(layout.crates).filter((name) => name.startsWith("cmd-"));
  const configured = {
    paths: commandPaths(JSON.parse(read("scripts/modularization/cli-commands.json"))),
  };
  cliPaths(read("src/cli.rs"), configured.paths);
  const aliases = {};
  for (const [path, owner] of Object.entries(configured.paths)) {
    if (!commands.includes(owner)) throw new Error(`unmapped CLI command crate: ${path}`);
    const parent = path.split(" ")[0];
    aliases[parent] ??= commands.includes(`cmd-${parent}`) ? `cmd-${parent}` : owner;
  }
  // A family token alone is conservative; literal child tokens narrow it below.
  const detect = (body) => {
    const roots = new Set();
    for (const match of body.matchAll(
      /(?:run|cmd|args|run_with_stdin|run_from|run_in|run_on_agentgraph|run_recording_renders|run_with_file_ceiling)\s*\(\s*&?\[\s*"([a-z][a-z-]*)"(?:\s*,\s*"([a-z-]+)")?/g,
    )) {
      const child = configured.paths[`${match[1]} ${match[2]}`];
      if (child) roots.add(child);
      else if (aliases[match[1]]) roots.add(aliases[match[1]]);
    }
    return [...roots];
  };
  const tests = {};
  for (const module of modules)
    for (const fn of parsed[module].filter((f) => f.test)) {
      const reached = new Map([[`${module}::${fn.name}`, fn]]);
      for (const [key, held] of reached) {
        const owner = key.split("::")[0];
        for (const helper of parsed[owner].filter((f) => !f.test))
          if (new RegExp(`\\b${helper.name}\\s*\\(`).test(held.code))
            reached.set(`${owner}::${helper.name}`, helper);
        for (const match of held.code.matchAll(/crate::(\w+)::(\w+)\s*\(/g)) {
          const helper = parsed[match[1]]?.find((f) => f.name === match[2]);
          if (helper && !helper.test) reached.set(`${match[1]}::${helper.name}`, helper);
        }
      }
      let roots = sorted([...reached.values()].flatMap((f) => detect(f.body)));
      // A borrowed helper is attributed only when the test actually calls it; its full module remains a file input.
      const borrowed = [...sources[module].matchAll(/use crate::(?!harness\b)(\w+)::/g)].map(
        (m) => m[1],
      );
      for (const other of borrowed)
        if (parsed[other]) {
          for (const helper of parsed[other].filter((f) => !f.test))
            if (
              [...reached.values()].some((f) => new RegExp(`\\b${helper.name}\\s*\\(`).test(f.code))
            )
              roots = sorted([...roots, ...detect(helper.body)]);
        }
      const old = layout.suites[`e2e-${module}`];
      if (!roots.length) roots = old?.crates.filter((c) => c.startsWith("cmd-")) ?? [];
      const suite = roots.length === 1 ? roots[0] : "cross-command";
      tests[`${module}::${fn.name}`] = {
        suite,
        commands: roots,
        reason:
          roots.length === 1
            ? `Exercises ${roots[0]} through the production entry point; retain assertions and failure paths.`
            : `Retain the existing integration between ${roots.join(", ") || "the public facade and runtime fixtures"}; no fixture assertion is replaced.`,
      };
    }
  return { schema: 1, tests };
}

/** Full package records, with version-qualified dependency closure (both sides of a lock diff). */
export function lockPackages(text) {
  const records = cargoManifest(text).package ?? [];
  if (!Array.isArray(records)) throw new Error("lockfile packages must be an array");
  const packages = text
    .split(/^\[\[package\]\]\s*$/m)
    .slice(1)
    .map((block, index) => {
      const record = records[index];
      const name = record?.name;
      const version = record?.version;
      const source = record?.source ?? "";
      if (
        typeof name !== "string" ||
        !name.trim() ||
        typeof version !== "string" ||
        !version.trim()
      )
        throw new Error("lock package needs name and version");
      if (typeof source !== "string") throw new Error("lock package source must be text");
      const deps = record.dependencies ?? [];
      if (!Array.isArray(deps) || !deps.every((dep) => typeof dep === "string" && dep.trim()))
        throw new Error("lock package dependencies must be a string array");
      return {
        name,
        version,
        key: `${name} ${version} ${source}`,
        source,
        block: block.trim(),
        deps,
      };
    });
  if (!packages.length) throw new Error("lockfile contains no packages");
  const result = new Map(packages.map((p) => [p.key, p]));
  if (result.size !== packages.length) throw new Error("duplicate lock package identity");
  for (const p of packages)
    p.deps = p.deps.flatMap((dep) => {
      const [name, version] = dep.split(" ");
      const source = /^[^ ]+ [^ ]+ \((.*)\)$/.exec(dep)?.[1];
      const found = packages.filter(
        (candidate) =>
          candidate.name === name &&
          (!version || candidate.version === version) &&
          (!source || candidate.source === source),
      );
      if (!found.length) throw new Error(`unresolved lock dependency ${dep}`);
      return found.map((item) => item.key);
    });
  return result;
}

export function resolution(packages, roots) {
  for (const root of roots)
    if (![...packages.values()].some((pkg) => pkg.name === root))
      throw new Error(`unresolved external root ${root}`);
  const reached = new Set(
    [...packages.values()].filter((p) => roots.includes(p.name)).map((p) => p.key),
  );
  for (const key of reached) for (const dep of packages.get(key).deps) reached.add(dep);
  return [...reached]
    .sort()
    .map((key) => packages.get(key).block)
    .join("\n");
}

/** Compute changed owned input paths; the Nx preparation step must materialize them. */
export function resolvedChanges(crates, before, after, toolBefore, toolAfter) {
  const a = lockPackages(before);
  const b = lockPackages(after);
  const toolChanged = toolchainFingerprint(toolBefore) !== toolchainFingerprint(toolAfter);
  return Object.entries(crates).flatMap(([name, crate]) => {
    for (const root of crate.external)
      if (![...a.values(), ...b.values()].some((pkg) => pkg.name === root))
        throw new Error(`unresolved external root ${name}:${root}`);
    const changed = [];
    const present = (packages, roots) =>
      roots.filter((root) => [...packages.values()].some((pkg) => pkg.name === root));
    if (
      resolution(a, present(a, crate.beforeExternal ?? crate.external)) !==
      resolution(b, present(b, crate.afterExternal ?? crate.external))
    )
      changed.push(`resolved/${name}/lock.json`);
    if (toolChanged && crate.toolchain) changed.push(`resolved/${name}/toolchain.json`);
    return changed;
  });
}

/** Project modern crate roots through a PR's own package aliases and resolved sets. */
export function historicalCrates(crates, current, oldManifest, newManifest, before, after) {
  const definitions = dependencyDefinitions(cargoManifest(current));
  const sides = [
    [oldManifest, before],
    [newManifest, after],
  ].map(([manifest, lock]) => ({
    definitions: dependencyDefinitions(cargoManifest(manifest)),
    known: new Set([...lockPackages(lock).values()].map((pkg) => pkg.name)),
  }));
  return Object.fromEntries(
    Object.entries(crates).map(([name, crate]) => {
      const roots = sides.map((side) =>
        sorted(
          crate.external.flatMap((root) => {
            const aliases = definitions
              .filter((row) => row.packageName === root)
              .map((row) => row.alias);
            if (!aliases.length) return side.known.has(root) ? [root] : [];
            const byAlias = side.definitions.filter((row) => aliases.includes(row.alias));
            return (
              byAlias.length ? byAlias : side.definitions.filter((row) => row.packageName === root)
            )
              .map((row) => row.packageName)
              .filter((pkg) => side.known.has(pkg));
          }),
        ),
      );
      return [
        name,
        {
          ...crate,
          external: sorted(roots.flat()),
          beforeExternal: roots[0],
          afterExternal: roots[1],
        },
      ];
    }),
  );
}

export function commandMapping(layout, catalog) {
  if (![1, 4].includes(catalog.schema))
    throw new Error("command test catalog must use schema 1 or 4");
  if (
    Object.keys(catalog).some(
      (key) =>
        !(
          catalog.schema === 4
            ? ["schema", "baselineSha256", "tests", "dispositions"]
            : ["schema", "tests"]
        ).includes(key),
    )
  )
    throw new Error("unknown command catalog field");
  if (!catalog.tests || Array.isArray(catalog.tests) || typeof catalog.tests !== "object")
    throw new Error("catalog.tests must be an object");
  for (const [name, entry] of Object.entries(catalog.tests))
    if (
      !entry ||
      Object.keys(entry).some(
        (key) =>
          !(
            catalog.schema === 4
              ? ["original", "suite", "commands", "reason", "share"]
              : ["suite", "commands", "reason"]
          ).includes(key),
      )
    )
      throw new Error(`unknown test catalog field: ${name}`);
  const expected = inventory(layout);
  if (catalog.schema === 1) {
    if (JSON.stringify(Object.keys(expected.tests)) !== JSON.stringify(Object.keys(catalog.tests)))
      throw new Error("test inventory drift: regenerate and review the command test catalog");
    for (const [name, entry] of Object.entries(expected.tests))
      if (
        !Array.isArray(catalog.tests[name].commands) ||
        entry.commands.some((command) => !catalog.tests[name].commands.includes(command))
      )
        throw new Error(`test command attribution drift: ${name}`);
  } else {
    const normalized = expandCatalog({
      schema: 4,
      baselineSha256: catalog.baselineSha256,
      dispositions: catalog.dispositions,
    });
    if (JSON.stringify(normalized.tests) !== JSON.stringify(catalog.tests))
      throw new Error("decomposition entries differ from their dispositions");
    if (
      catalog.baselineSha256 !==
      baselineHash(JSON.parse(read("scripts/modularization/command-tests.json")))
    )
      throw new Error("decomposition baseline fingerprint differs");
    const originals = new Map();
    for (const [name, entry] of Object.entries(catalog.tests)) {
      if (
        !expected.tests[entry.original] ||
        !Number.isFinite(entry.share) ||
        entry.share <= 0 ||
        entry.share > 1
      )
        throw new Error(`invalid original test/share: ${name}`);
      originals.set(entry.original, (originals.get(entry.original) ?? 0) + entry.share);
    }
    if (
      originals.size !== Object.keys(expected.tests).length ||
      [...originals.values()].some((share) => Math.abs(share - 1) > 1e-9)
    )
      throw new Error("decomposition loses or duplicates an original test's weight");
    for (const id of Object.keys(expected.tests)) {
      const pieces = Object.values(catalog.tests).filter((entry) => entry.original === id);
      const roots = new Set(pieces.flatMap((entry) => entry.commands));
      const disposition = catalog.dispositions?.[id];
      if (!disposition) throw new Error(`missing disposition: ${id}`);
      if (disposition.originalCommands.some((command) => !roots.has(command)))
        throw new Error(`decomposition loses command assertions: ${id}`);
    }
  }
  const manifest = cargoManifest(read("Cargo.toml"));
  const externalDefinitions = dependencyDefinitions(manifest);
  const external = sorted(externalDefinitions.map((entry) => entry.alias));
  const packageName = (alias) =>
    externalDefinitions.find((entry) => entry.alias === alias)?.packageName ?? alias;
  const phases =
    catalog.schema === 4
      ? Object.fromEntries(
          Object.entries(catalog.tests).map(([name, entry]) => [
            name,
            phaseInputs(
              entry,
              catalog.dispositions[entry.original],
              commandPaths(JSON.parse(read("scripts/modularization/cli-commands.json"))),
            ),
          ]),
        )
      : {};
  const testCode = new Map();
  const moduleCode = (module) => {
    if (!testCode.has(module)) testCode.set(module, codeOnly(read(`tests/e2e/${module}.rs`)));
    return testCode.get(module);
  };
  const crates = {};
  for (const [name, crate] of Object.entries(layout.crates)) {
    const code = [
      ...crate.files.map((file) => codeOnly(read(file))),
      ...(layout.symbolSources?.[name] !== undefined
        ? layout.symbolSources[name].map(({ file, symbols }) => {
            const text = read(file);
            const decls = declarations(text);
            const selected = new Set(symbols);
            for (const symbol of selected)
              for (const decl of decls.filter((entry) => entry.name === symbol))
                for (const helper of decls)
                  if (
                    helper.kind === "fn" &&
                    helper.private &&
                    new RegExp(`\\b${helper.name}\\b`).test(decl.code)
                  )
                    selected.add(helper.name);
            return [
              ...decls.filter((entry) => selected.has(entry.name)).map((entry) => entry.code),
              ...codeOnly(text).matchAll(/(?:^|\n)use\s+[^;]+;/g),
            ].join("\n");
          })
        : (layout.historicalPaths?.[name] ?? []).map((file) => codeOnly(read(file)))),
    ].join("\n");
    crates[name] = {
      paths: [
        ...crate.files,
        ...(layout.assets?.[name] ?? []),
        `moved/${name}/**`,
        `resolved/${name}/**`,
      ],
      deps: crate.deps,
      external: external.filter((dep) =>
        new RegExp(`\\b${dep.replaceAll("-", "_")}\\s*::`).test(code),
      ),
      toolchain: true,
    };
  }
  const suites = Object.fromEntries(
    Object.keys(layout.crates)
      .filter((name) => name.startsWith("cmd-"))
      .map((name) => [name, { paths: [], crates: [name], modules: [], tests: [] }]),
  );
  suites["cross-command"] = {
    paths: [],
    crates: ["cli-dispatcher", "onepipeline"],
    modules: [],
    tests: [],
  };
  if (catalog.schema === 4) {
    suites["test-support"] = { paths: [], crates: [], modules: [], tests: [] };
    suites["public-api"] = { paths: [], crates: ["onepipeline"], modules: [], tests: [] };
    suites["full-dispatch"] = {
      paths: [],
      crates: ["cli-dispatcher", "onepipeline"],
      modules: [],
      tests: [],
    };
  }
  for (const [name, test] of Object.entries(catalog.tests)) {
    if (
      !suites[test.suite] ||
      !Array.isArray(test.commands) ||
      !test.reason ||
      test.commands.some((c) => !c.startsWith("cmd-") || !crates[c])
    )
      throw new Error(`invalid test owner: ${name}`);
    if (
      !["cross-command", "test-support", "public-api", "full-dispatch"].includes(test.suite) &&
      (test.commands.length !== 1 || test.commands[0] !== test.suite)
    )
      throw new Error(`command suite has foreign root: ${name}`);
    const suite = suites[test.suite];
    suite.tests.push(name);
    suite.modules.push(name.split("::")[0]);
    suite.crates.push(...test.commands);
  }
  for (const [name, suite] of Object.entries(suites)) {
    if (!suite.tests.length) {
      delete suites[name];
      continue;
    }
    // Phase-local fixture builders write documents with std/serde; they do not link
    // production IO/process implementations merely to prepare a directory.
    // Assertions calling the public library retain the explicit public-api suite.
    suite.crates = sorted(suite.crates);
    suite.modules = sorted(suite.modules);
    suite.paths =
      catalog.schema === 4
        ? sorted(suite.tests.flatMap((id) => phases[id].paths))
        : [...suiteInputs(suite.modules), `migrated-tests/${name}/**`];
    // Migrated helpers are linked per command; source attribution remains conservative.
    suite.paths = suite.paths.filter((path) => !path.startsWith("src/"));
    if (catalog.schema === 4) {
      const reaches = (crate, target, seen = new Set()) => {
        if (crate === target) return true;
        if (seen.has(crate)) return false;
        seen.add(crate);
        return (crates[crate]?.deps ?? []).some((dep) => reaches(dep, target, seen));
      };
      if (
        suite.crates.some(
          (crate) => reaches(crate, "agent-execution") || reaches(crate, "agent-session"),
        )
      ) {
        suite.paths.push("crates/testfakes/src/bin/fake-oneagentgraph.rs");
        if (crates["test-agent-process"]) suite.crates.push("test-agent-process");
      }
      if (suite.crates.some((crate) => reaches(crate, "publication"))) {
        suite.paths.push("crates/testfakes/src/bin/fake-gh.rs");
        if (crates["test-host-process"]) suite.crates.push("test-host-process");
      }
      if (crates["test-scenario-schema"]) suite.crates.push("test-scenario-schema");
      suite.paths.push("crates/testfakes/src/profile.rs", "crates/testfakes/src/profile/**");
    }
    const phaseExternal = new Set(
      catalog.schema === 4 ? suite.tests.flatMap((id) => phases[id].external) : [],
    );
    const usesProcessDouble =
      catalog.schema === 4 &&
      (phaseExternal.has("onepipeline_testfakes") ||
        suite.paths.some((path) => path.startsWith("crates/testfakes/src/bin/")));
    if (usesProcessDouble)
      suite.paths.push("crates/testfakes/Cargo.toml", "crates/testfakes/src/lib.rs");
    crates[`runner-${name}`] = {
      paths: [`resolved/runner-${name}/**`],
      deps: suite.crates,
      external: sorted([
        "serde",
        "serde_json",
        ...(catalog.schema === 1
          ? ["onepipeline-testfakes"]
          : [
              ...(usesProcessDouble && catalog.schema !== 4 ? ["onepipeline-testfakes"] : []),
              ...(name === "test-support" ? ["oneagentgraph", "oneharness-core", "onejudge"] : []),
            ]),
        ...external.filter(
          (dep) =>
            (catalog.schema === 1 || dep !== "onepipeline-testfakes") &&
            (catalog.schema === 4
              ? phaseExternal.has(dep.replaceAll("-", "_"))
              : suite.modules.some((module) =>
                  new RegExp(`\\b${dep.replaceAll("-", "_")}\\s*::`).test(moduleCode(module)),
                )),
        ),
      ]),
      toolchain: true,
    };
    suite.crates = [`runner-${name}`];
  }
  for (const [name, crate] of Object.entries(crates))
    crate.external = sorted((layout.externalRoots?.[name] ?? crate.external).map(packageName));
  const selection = {
    shared: [
      "nx.json",
      "package.json",
      "package-lock.json",
      ...(catalog.schema === 1 ? ["justfile", ".cargo/config.toml", ".config/nextest.toml"] : []),
      ...(catalog.schema === 1 ? ["tests/e2e/harness.rs", "crates/testfakes/**"] : []),
    ],
    sources: ["src/**"],
    fallback: "onepipeline",
    crates: Object.fromEntries(
      Object.entries(crates).map(([name, c]) => [name, { paths: c.paths, deps: c.deps }]),
    ),
    suites: Object.fromEntries(
      Object.entries(suites).map(([name, s]) => [
        name,
        { paths: s.paths, crates: s.crates, modules: [] },
      ]),
    ),
  };
  compileMapping(selection);
  return {
    selection,
    crates,
    suites,
    ...(catalog.schema === 4
      ? {
          helperConsumers: Object.fromEntries(
            Object.entries(suites).map(([name, suite]) => [
              name,
              sorted(suite.tests.flatMap((id) => phases[id].helpers)),
            ]),
          ),
        }
      : {}),
  };
}

export function run(layout, catalog, weights, changes, projector) {
  const mapping = commandMapping(layout, catalog);
  const compiled = compileMapping(mapping.selection);
  const moduleCounts = {};
  const originals = new Set(
    Object.entries(catalog.tests).map(([name, entry]) => entry.original ?? name),
  );
  for (const test of originals) {
    const module = test.split("::")[0];
    moduleCounts[module] = (moduleCounts[module] ?? 0) + 1;
  }
  const rows = changes.map((change) => {
    const sourceProjection = projector ? projector(change) : { files: change.files };
    let files = sourceProjection.files.filter(
      (file) => !["Cargo.lock", "rust-toolchain.toml"].includes(file),
    );
    const reasons = [];
    if (change.files.includes("Cargo.lock") || change.files.includes("rust-toolchain.toml")) {
      const get = (revision, file) => git("show", `${revision}:${file}`);
      const before = get(`${change.commit}^`, "Cargo.lock");
      const after = get(change.commit, "Cargo.lock");
      const known = new Set(
        [...lockPackages(before).values(), ...lockPackages(after).values()].map((pkg) => pkg.name),
      );
      // Schema 1 reproduces the committed historical baseline; schema 4 is the proposed design.
      const uncertain =
        catalog.schema === 1
          ? Object.entries(mapping.crates)
              .filter(([, crate]) => crate.external.some((root) => !known.has(root)))
              .map(([name]) => `resolved/${name}/unresolved.json`)
          : [];
      const historical =
        catalog.schema === 1
          ? Object.fromEntries(
              Object.entries(mapping.crates).map(([name, crate]) => [
                name,
                { ...crate, external: crate.external.filter((root) => known.has(root)) },
              ]),
            )
          : historicalCrates(
              mapping.crates,
              read("Cargo.toml"),
              get(`${change.commit}^`, "Cargo.toml"),
              get(change.commit, "Cargo.toml"),
              before,
              after,
            );
      const resolved = [
        ...resolvedChanges(
          historical,
          before,
          after,
          get(`${change.commit}^`, "rust-toolchain.toml"),
          get(change.commit, "rust-toolchain.toml"),
        ),
        ...uncertain,
      ];
      if (uncertain.length)
        reasons.push(
          "historically unresolvable modern dependency roots: conservatively select their consumers",
        );
      files = [...files, ...resolved];
      if (resolved.length)
        reasons.push(
          `resolved dependencies/toolchain: ${sorted(resolved.map((p) => p.split("/")[1])).join(", ")}`,
        );
    }
    if (catalog.schema === 4) {
      for (const file of ["justfile", ".cargo/config.toml", ".config/nextest.toml"]) {
        if (!files.includes(file)) continue;
        const get = (rev) => {
          try {
            return git("show", `${rev}:${file}`);
          } catch (error) {
            if (/does not exist|exists on disk, but not in/.test(String(error.stderr))) return "";
            throw error;
          }
        };
        files = files.filter((path) => path !== file);
        const projected = historicalConfigChanges(
          file,
          get(`${change.commit}^`),
          get(change.commit),
          Object.keys(mapping.suites),
        );
        files.push(...projected);
        if (projected.length) reasons.push(`test target configuration: ${file}`);
      }
    }
    if (files.includes("Cargo.toml")) {
      if (catalog.schema === 4)
        files.push(
          ...manifestChanges(
            mapping.crates,
            git("show", `${change.commit}^:Cargo.toml`),
            git("show", `${change.commit}:Cargo.toml`),
          ),
        );
      else
        files.push(...Object.keys(mapping.crates).map((name) => `resolved/${name}/manifest.json`));
    }
    const selected = affectedSuites(compiled, files);
    return {
      ...change,
      effectiveFiles: files,
      ...(sourceProjection.projected?.length
        ? { sourceProjection: sourceProjection.projected }
        : {}),
      ...selected,
      reasons: [
        ...reasons,
        ...files
          .filter(
            (file) =>
              !file.startsWith("resolved/") &&
              affectedSuites(compiled, [file]).affected.length >
                Object.keys(mapping.suites).length / 2,
          )
          .map((file) => `shared source/input: ${file}`),
      ],
    };
  });
  const platforms = Object.fromEntries(
    Object.entries(weights).map(([platform, seconds]) => {
      const suiteWeights = Object.fromEntries(
        Object.entries(mapping.suites).map(([name, suite]) => [
          name,
          suite.tests.reduce((sum, test) => {
            const module = test.split("::")[0];
            return (
              sum +
              ((seconds.get(module) ?? 0) / moduleCounts[module]) * (catalog.tests[test].share ?? 1)
            );
          }, 0),
        ]),
      );
      const perRun = Object.values(suiteWeights).reduce((a, b) => a + b, 0);
      const skipped = rows.reduce(
        (sum, row) =>
          sum +
          Object.keys(mapping.suites)
            .filter((name) => !row.affected.includes(name))
            .reduce((s, name) => s + suiteWeights[name], 0),
        0,
      );
      return [
        platform,
        {
          suiteSkip:
            rows.reduce(
              (sum, row) => sum + Object.keys(mapping.suites).length - row.affected.length,
              0,
            ) /
            (rows.length * Object.keys(mapping.suites).length),
          secondsSkip: skipped / (perRun * rows.length),
          secondsPerRun: perRun,
          suiteSeconds: suiteWeights,
          missingModules: Object.keys(moduleCounts).filter((module) => !seconds.has(module)),
        },
      ];
    }),
  );
  const migration = [];
  const extracted = new Set();
  while (extracted.size < Object.keys(layout.crates).length) {
    const next = Object.keys(layout.crates).find(
      (name) => !extracted.has(name) && layout.crates[name].deps.every((dep) => extracted.has(dep)),
    );
    if (!next) throw new Error("migration dependency cycle");
    extracted.add(next);
    const ready = (name) =>
      extracted.has(name) ||
      (name.startsWith("runner-") && mapping.crates[name].deps.every((dep) => extracted.has(dep)));
    const stageCrates = Object.fromEntries(
      Object.entries(mapping.selection.crates).filter(([name]) => ready(name)),
    );
    stageCrates.residual = {
      paths: Object.entries(mapping.selection.crates)
        .filter(([name]) => !ready(name))
        .flatMap(([, c]) => c.paths),
      deps: Object.keys(stageCrates),
    };
    const stage = compileMapping({
      ...mapping.selection,
      fallback: "residual",
      crates: stageCrates,
      suites: Object.fromEntries(
        Object.entries(mapping.selection.suites).map(([name, suite]) => [
          name,
          { ...suite, crates: suite.crates.map((crate) => (ready(crate) ? crate : "residual")) },
        ]),
      ),
    });
    const skipped = rows.reduce(
      (sum, row) =>
        sum +
        Object.keys(mapping.suites).length -
        affectedSuites(stage, row.effectiveFiles).affected.length,
      0,
    );
    migration.push({
      crate: next,
      suiteSkip: skipped / (rows.length * Object.keys(mapping.suites).length),
    });
  }
  return {
    schema: catalog.schema === 4 ? 2 : 1,
    migration,
    suites: Object.keys(mapping.suites).length,
    tests: Object.keys(catalog.tests).length,
    ...(catalog.schema === 4 ? { originalTests: originals.size } : {}),
    platforms,
    rows,
    mapping,
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (["--resolve", "--resolve-history"].includes(process.argv[2])) {
      const history = process.argv[2] === "--resolve-history";
      if (process.argv.length !== (history ? 12 : 9))
        throw new Error(
          "usage: command-replay.mjs --resolve CRATES.json BEFORE.lock AFTER.lock BEFORE-tool AFTER-tool OUTPUT.json [CURRENT.toml BEFORE.toml AFTER.toml for --resolve-history]",
        );
      const crates = JSON.parse(read(process.argv[3]));
      if (!crates || typeof crates !== "object" || Array.isArray(crates))
        throw new Error("resolution configuration must be an object");
      for (const [name, crate] of Object.entries(crates))
        if (
          !/^[a-z][a-z0-9-]*$/.test(name) ||
          !Array.isArray(crate.external) ||
          !crate.external.every((dep) => typeof dep === "string" && /^[A-Za-z0-9_-]+$/.test(dep)) ||
          typeof crate.toolchain !== "boolean" ||
          Object.keys(crate).some((key) => !["external", "toolchain"].includes(key))
        )
          throw new Error(`invalid resolution inputs for ${name}`);
      writeFileSync(
        process.argv[8],
        JSON.stringify(
          resolvedChanges(
            history
              ? historicalCrates(
                  crates,
                  ...process.argv.slice(9, 12).map(read),
                  read(process.argv[4]),
                  read(process.argv[5]),
                )
              : crates,
            ...process.argv.slice(4, 8).map(read),
          ),
        ),
      );
      process.exit(0);
    }
    if (
      process.argv[2] === "--inventory"
        ? process.argv.length !== 3
        : !process.argv[2] ||
          process.argv[2].startsWith("--") ||
          process.argv.length > 4 ||
          (process.argv[3] && process.argv[3] !== "--write-doc")
    )
      throw new Error("usage: command-replay.mjs OUTPUT.json [--write-doc] | --inventory");
    const doc = "docs/modularization-command-replay.md";
    const prefix = process.argv[3] === "--write-doc" ? read(doc).split(COMMAND_MARKER) : undefined;
    if (prefix && prefix.length !== 2) throw new Error("expected one command replay marker");
    const file = "scripts/modularization/command-tests.json";
    if (process.argv[2] === "--inventory") {
      const layout = JSON.parse(read("scripts/modularization/layout.json"));
      validate(layout, moduleGraph());
      writeFileSync(file, `${JSON.stringify(inventory(layout), null, 2)}\n`);
    } else {
      const { layout } = evidence();
      const { profileTable } = await import("./replay.mjs");
      const changes = mergedChanges(".", {
        ref: "origin/main",
        since: "2026-09-15T00:00:00Z",
        until: "2026-10-16T00:00:00Z",
        exclude: "^chore: release",
      });
      if (changes.length !== 144) throw new Error(`expected 144 PRs, got ${changes.length}`);
      const result = run(
        layout,
        JSON.parse(read(file)),
        profileTable(read("docs/test-profile-report.md")),
        changes,
      );
      if (!process.argv[2])
        throw new Error(
          "usage: node scripts/modularization/command-replay.mjs OUTPUT.json | --inventory",
        );
      writeFileSync(process.argv[2], `${JSON.stringify(result, null, 2)}\n`);
      if (prefix)
        writeFileSync(
          doc,
          `${prefix[0]}${commandAppendix(result, layout, JSON.parse(read(file)))}`,
        );
      console.log(
        `command replay: ${result.rows.length} PRs; ${result.tests} test pieces; ${(result.platforms.linux.suiteSkip * 100).toFixed(2)}% suite runs skipped; wrote ${process.argv[2]}`,
      );
    }
  } catch (error) {
    const mode = ["--resolve", "--resolve-history", "--inventory"].includes(process.argv[2])
      ? process.argv[2]
      : "replay";
    const next =
      error.code && /^(?:EACCES|EPERM|ENOENT|EISDIR|ENOTDIR|ENOSPC|EROFS)$/.test(error.code)
        ? `check the named path's type, existence and permissions, available disk space, and that the output parent is writable; rerun the same ${mode} invocation`
        : mode.startsWith("--resolve")
          ? `correct the named resolution configuration, lockfile, manifest or toolchain input; rerun the same ${mode} invocation with all input and output paths`
          : `review the named catalog/inventory input and 144-PR history; rerun the same ${mode} invocation with its output path`;
    console.error(`${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}
