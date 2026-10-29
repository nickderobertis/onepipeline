#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { codeOnly } from "./graph.mjs";
import { suiteInputs } from "./suite-inputs.mjs";
import { COMMAND_MARKER, commandAppendix } from "./command-report.mjs";
import { evidence } from "./replay.mjs";
import { affectedSuites, compileMapping, mergedChanges } from "../crate-split-replay.mjs";

const read = (path) => readFileSync(path, "utf8");
const sorted = (items) => [...new Set(items)].sort();
const git = (...args) =>
  execFileSync("git", args, { encoding: "utf8", maxBuffer: 32 * 1024 * 1024 });

/** Offsets come from comment/literal-erased Rust; bodies retain their original bytes. */
export function functions(source) {
  const code = codeOnly(source);
  return [...code.matchAll(/\bfn\s+(\w+)\s*(?:<[^{}]*>)?\s*\(/g)].flatMap((match) => {
    const start = code.indexOf("{", match.index);
    if (start < 0) return [];
    let end = start + 1;
    let depth = 1;
    while (end < code.length && depth) {
      if (code[end] === "{") depth++;
      if (code[end] === "}") depth--;
      end++;
    }
    const preceding = code.slice(code.lastIndexOf("}", match.index) + 1, match.index);
    return [
      {
        name: match[1],
        test: /#\s*\[\s*test\s*\]/.test(preceding),
        body: source.slice(start, end),
        code: code.slice(start, end),
      },
    ];
  });
}

/** Explicit extraction inventory, used for review; never silently regenerate ownership at replay time. */
export function inventory(layout) {
  const modules = [...read("tests/e2e/main.rs").matchAll(/^mod (\w+);/gm)].map((m) => m[1]);
  const sources = Object.fromEntries(modules.map((m) => [m, read(`tests/e2e/${m}.rs`)]));
  const parsed = Object.fromEntries(modules.map((m) => [m, functions(sources[m])]));
  const commands = Object.keys(layout.crates).filter((name) => name.startsWith("cmd-"));
  const aliases = Object.fromEntries(commands.map((name) => [name.slice(4), name]));
  aliases.monitor = "cmd-observer-view";
  aliases.plan = "cmd-plan-check";
  aliases.boundary = "cmd-boundary-guidance";
  aliases.channel = "cmd-channel-queue";
  aliases.scratch = "cmd-scratch-discard";
  // A family token alone is conservative; literal child tokens narrow it below.
  const detect = (body) => {
    const roots = new Set();
    for (const match of body.matchAll(
      /(?:run|cmd|args|run_with_stdin|run_from|run_in|run_on_agentgraph|run_recording_renders|run_with_file_ceiling)\s*\(\s*&?\[\s*"([a-z][a-z-]*)"(?:\s*,\s*"([a-z-]+)")?/g,
    )) {
      const child = `cmd-${match[1]}-${match[2]}`;
      if (commands.includes(child)) roots.add(child);
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
        // Follow named helpers in this module and explicitly qualified borrowed helpers.
        for (const helper of parsed[owner].filter((f) => !f.test))
          if (new RegExp(`\\b${helper.name}\\s*\\(`).test(held.code))
            reached.set(`${owner}::${helper.name}`, helper);
        for (const match of held.code.matchAll(/crate::(\w+)::(\w+)\s*\(/g)) {
          const helper = parsed[match[1]]?.find((f) => f.name === match[2]);
          if (helper && !helper.test) reached.set(`${match[1]}::${helper.name}`, helper);
        }
      }
      let roots = sorted([...reached.values()].flatMap((f) => detect(f.body)));
      // Imported helper functions may launch commands; retain their module's existing union until split.
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
  const packages = text
    .split(/^\[\[package\]\]\s*$/m)
    .slice(1)
    .map((block) => {
      const field = (name) =>
        JSON.parse(new RegExp(`^${name} = (".*")$`, "m").exec(block)?.[1] ?? '""');
      const name = field("name");
      const version = field("version");
      if (!name || !version) throw new Error("lock package needs name and version");
      const deps = [
        ...(/dependencies = \[([\s\S]*?)\]/.exec(block)?.[1] ?? "").matchAll(/"([^"]+)"/g),
      ].map((m) => m[1]);
      return { name, version, key: `${name} ${version}`, block: block.trim(), deps };
    });
  if (!packages.length) throw new Error("lockfile contains no packages");
  const result = new Map(packages.map((p) => [p.key, p]));
  for (const p of packages)
    p.deps = p.deps.flatMap((dep) => {
      const [name, version] = dep.split(" ");
      const found = packages.filter(
        (candidate) => candidate.name === name && (!version || candidate.version === version),
      );
      if (!found.length) throw new Error(`unresolved lock dependency ${dep}`);
      return found.map((item) => item.key);
    });
  return result;
}

export function resolution(packages, roots) {
  const reached = new Set(
    [...packages.values()].filter((p) => roots.includes(p.name)).map((p) => p.key),
  );
  for (const key of reached) for (const dep of packages.get(key).deps) reached.add(dep);
  return [...reached]
    .sort()
    .map((key) => packages.get(key).block)
    .join("\n");
}

/** Materialize resolved inputs as owned Nx files, not workspace-global lockfile edges. */
export function resolvedChanges(crates, before, after, toolBefore, toolAfter) {
  const a = lockPackages(before);
  const b = lockPackages(after);
  return Object.entries(crates).flatMap(([name, crate]) => {
    const changed = [];
    if (resolution(a, crate.external) !== resolution(b, crate.external))
      changed.push(`resolved/${name}/lock.json`);
    if (toolBefore !== toolAfter && crate.toolchain)
      changed.push(`resolved/${name}/toolchain.json`);
    return changed;
  });
}

export function commandMapping(layout, catalog) {
  if (catalog.schema !== 1) throw new Error("command test catalog must use schema 1");
  if (Object.keys(catalog).some((key) => !["schema", "tests"].includes(key)))
    throw new Error("unknown command catalog field");
  if (!catalog.tests || Array.isArray(catalog.tests) || typeof catalog.tests !== "object")
    throw new Error("catalog.tests must be an object");
  for (const [name, entry] of Object.entries(catalog.tests))
    if (!entry || Object.keys(entry).some((key) => !["suite", "commands", "reason"].includes(key)))
      throw new Error(`unknown test catalog field: ${name}`);
  const expected = inventory(layout);
  if (JSON.stringify(Object.keys(expected.tests)) !== JSON.stringify(Object.keys(catalog.tests)))
    throw new Error("test inventory drift: regenerate and review the command test catalog");
  for (const [name, entry] of Object.entries(expected.tests)) {
    if (
      !Array.isArray(catalog.tests[name].commands) ||
      entry.commands.some((command) => !catalog.tests[name].commands.includes(command))
    )
      throw new Error(`test command attribution drift: ${name}`);
  }
  const manifest = read("Cargo.toml");
  const external = sorted([...manifest.matchAll(/^([\w-]+)\s*=\s*(?:"|\{)/gm)].map((m) => m[1]));
  const testCode = new Map();
  const moduleCode = (module) => {
    if (!testCode.has(module)) testCode.set(module, codeOnly(read(`tests/e2e/${module}.rs`)));
    return testCode.get(module);
  };
  const crates = {};
  for (const [name, crate] of Object.entries(layout.crates)) {
    const code = crate.files.map((file) => codeOnly(read(file))).join("\n");
    crates[name] = {
      paths: [...crate.files, ...(layout.assets?.[name] ?? []), `resolved/${name}/**`],
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
  for (const [name, test] of Object.entries(catalog.tests)) {
    if (
      !suites[test.suite] ||
      !Array.isArray(test.commands) ||
      !test.reason ||
      test.commands.some((c) => !c.startsWith("cmd-") || !crates[c])
    )
      throw new Error(`invalid test owner: ${name}`);
    if (
      test.suite !== "cross-command" &&
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
    suite.crates = sorted(suite.crates);
    suite.modules = sorted(suite.modules);
    suite.paths = suiteInputs(suite.modules);
    // Migrated helpers are linked per command; source attribution remains conservative.
    suite.paths = suite.paths.filter((path) => !path.startsWith("src/"));
    crates[`runner-${name}`] = {
      paths: [`resolved/runner-${name}/**`],
      deps: suite.crates,
      external: sorted([
        "serde",
        "serde_json",
        "onepipeline-testfakes",
        ...external.filter((dep) =>
          suite.modules.some((module) =>
            new RegExp(`\\b${dep.replaceAll("-", "_")}\\s*::`).test(moduleCode(module)),
          ),
        ),
      ]),
      toolchain: true,
    };
    suite.crates = [`runner-${name}`];
  }
  const selection = {
    shared: [
      "nx.json",
      "package.json",
      "package-lock.json",
      "justfile",
      ".cargo/config.toml",
      ".config/nextest.toml",
      "tests/e2e/harness.rs",
      "crates/testfakes/**",
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
  return { selection, crates, suites };
}

export function run(layout, catalog, weights, changes) {
  const mapping = commandMapping(layout, catalog);
  const compiled = compileMapping(mapping.selection);
  const moduleCounts = {};
  for (const test of Object.keys(catalog.tests)) {
    const module = test.split("::")[0];
    moduleCounts[module] = (moduleCounts[module] ?? 0) + 1;
  }
  const rows = changes.map((change) => {
    let files = change.files.filter(
      (file) => !["Cargo.lock", "rust-toolchain.toml"].includes(file),
    );
    const reasons = [];
    if (change.files.includes("Cargo.lock") || change.files.includes("rust-toolchain.toml")) {
      const get = (revision, file) => git("show", `${revision}:${file}`);
      const resolved = resolvedChanges(
        mapping.crates,
        get(`${change.commit}^`, "Cargo.lock"),
        get(change.commit, "Cargo.lock"),
        get(`${change.commit}^`, "rust-toolchain.toml"),
        get(change.commit, "rust-toolchain.toml"),
      );
      files = [...files, ...resolved];
      if (resolved.length)
        reasons.push(
          `resolved dependencies/toolchain: ${sorted(resolved.map((p) => p.split("/")[1])).join(", ")}`,
        );
    }
    // The workspace manifest contains package metadata too; until manifests are split, keep it conservative.
    if (files.includes("Cargo.toml"))
      files.push(...Object.keys(mapping.crates).map((name) => `resolved/${name}/manifest.json`));
    const selected = affectedSuites(compiled, files);
    return {
      ...change,
      effectiveFiles: files,
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
            return sum + (seconds.get(module) ?? 0) / moduleCounts[module];
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
    schema: 1,
    migration,
    suites: Object.keys(mapping.suites).length,
    tests: Object.keys(catalog.tests).length,
    platforms,
    rows,
    mapping,
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv[2] === "--resolve") {
      if (process.argv.length !== 9)
        throw new Error(
          "usage: command-replay.mjs --resolve CRATES.json BEFORE.lock AFTER.lock BEFORE-tool AFTER-tool OUTPUT.json",
        );
      const crates = JSON.parse(read(process.argv[3]));
      for (const [name, crate] of Object.entries(crates))
        if (
          !/^[a-z][a-z0-9-]*$/.test(name) ||
          !Array.isArray(crate.external) ||
          !crate.external.every((dep) => typeof dep === "string") ||
          typeof crate.toolchain !== "boolean"
        )
          throw new Error(`invalid resolution inputs for ${name}`);
      writeFileSync(
        process.argv[8],
        JSON.stringify(resolvedChanges(crates, ...process.argv.slice(4, 8).map(read))),
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
    const { layout } = evidence();
    const file = "scripts/modularization/command-tests.json";
    if (process.argv[2] === "--inventory")
      writeFileSync(file, `${JSON.stringify(inventory(layout), null, 2)}\n`);
    else {
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
      if (process.argv[3] === "--write-doc") {
        const doc = "docs/modularization-command-replay.md";
        const prefix = read(doc).split(COMMAND_MARKER);
        if (prefix.length !== 2) throw new Error("expected one command replay marker");
        writeFileSync(
          doc,
          `${prefix[0].trimEnd()}\n\n${commandAppendix(result, layout, JSON.parse(read(file)))}`,
        );
      }
      console.log(
        JSON.stringify(
          { suites: result.suites, tests: result.tests, platforms: result.platforms },
          null,
          2,
        ),
      );
    }
  } catch (error) {
    console.error(
      `${error.message}\nnext: review command-tests.json against the test inventory, lock inputs and the 144-PR history; rerun with OUTPUT.json`,
    );
    process.exitCode = 1;
  }
}
