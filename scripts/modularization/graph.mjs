#!/usr/bin/env node
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { sourceAssets, auxiliaryInputs } from "./inventory.mjs";

/** All Rust source files, including nested modules and test-only modules. */
export function rustFiles(root = "src") {
  return readdirSync(root, { withFileTypes: true })
    .flatMap((entry) => {
      const path = join(root, entry.name);
      return entry.isDirectory() ? rustFiles(path) : path.endsWith(".rs") ? [path] : [];
    })
    .sort();
}

/** Erase comments and literals, preserving offsets for diagnostic line numbers. */
export function codeOnly(text) {
  const parts = [];
  let at = 0;
  while (at < text.length) {
    let end = at;
    if (text.startsWith("//", at)) {
      end = text.indexOf("\n", at);
      if (end < 0) end = text.length;
    } else if (text.startsWith("/*", at)) {
      end = at + 2;
      let depth = 1;
      while (end < text.length && depth) {
        if (text.startsWith("/*", end)) {
          depth++;
          end += 2;
        } else if (text.startsWith("*/", end)) {
          depth--;
          end += 2;
        } else end++;
      }
    } else {
      const raw =
        (at === 0 || !/[a-zA-Z0-9_]/.test(text[at - 1])) &&
        /^(?:b)?r(#{0,255})"/.exec(text.slice(at));
      if (raw) {
        const close = `"${raw[1]}`;
        const found = text.indexOf(close, at + raw[0].length);
        end = found < 0 ? text.length : found + close.length;
      } else if (text[at] === '"') {
        end = at + 1;
        while (end < text.length) {
          if (text[end] === "\\") end += 2;
          else if (text[end++] === '"') break;
        }
      } else if (text[at] === "'") {
        const literal = /^'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'/u.exec(
          text.slice(at),
        );
        if (literal) end = at + literal[0].length;
      }
    }
    if (end > at) {
      parts.push(text.slice(at, end).replace(/[^\n]/g, " "));
      at = end;
    } else parts.push(text[at++]);
  }
  return parts.join("");
}

/** Conservative syntactic graph: root module paths and grouped crate imports. */
export function moduleGraph(root = ".") {
  const files = rustFiles(join(root, "src"));
  const names = new Set(
    files.map(
      (file) =>
        file
          .slice(join(root, "src").length + 1)
          .replaceAll("\\", "/")
          .split(/[/.]/)[0],
    ),
  );
  const aliases = new Map();
  const lib = files.find((file) => file === join(root, "src/lib.rs"));
  if (lib) {
    const exports = codeOnly(readFileSync(lib, "utf8"));
    for (const match of exports.matchAll(/pub\s+use\s+(?:crate::)?(\w+)::\{([^}]+)\}/g))
      for (const item of match[2].split(",")) {
        const symbol = /^\s*(\w+)(?:\s+as\s+(\w+))?\s*$/.exec(item);
        if (symbol && names.has(match[1])) aliases.set(symbol[2] ?? symbol[1], match[1]);
      }
  }
  const edges = new Map();
  const owners = {};
  for (const file of files) {
    const relative = file.slice(root === "." ? 0 : root.length + 1).replaceAll("\\", "/");
    const from = relative.slice(4).split(/[/.]/)[0];
    owners[relative] = from;
    const code = codeOnly(readFileSync(file, "utf8"));
    const add = (to, offset) => {
      if (!names.has(to) || from === to || to === "lib" || to === "main") return;
      const key = `${from}->${to}`;
      const edge = edges.get(key) ?? { from, to, sites: [] };
      const site = `${relative}:${code.slice(0, offset).split("\n").length}`;
      if (!edge.sites.includes(site)) edge.sites.push(site);
      edges.set(key, edge);
    };
    if (from === "lib") {
      for (const match of code.matchAll(/^(?:pub\s+)?mod\s+([a-z][a-z0-9_]*);/gm))
        add(match[1], match.index);
    }
    for (const match of code.matchAll(/\b(?:crate|onepipeline)::([a-z][a-z0-9_]*)/g))
      add(match[1], match.index);
    for (const match of code.matchAll(/\b(?:crate|super)::\s*\{([\s\S]*?)\};/g)) {
      for (const item of match[1].matchAll(/\b([a-z][a-z0-9_]*)\b/g)) add(item[1], match.index);
    }
    for (const [alias, module] of aliases)
      for (const match of code.matchAll(
        new RegExp(`\\bcrate::${alias}\\b|\\buse\\s+crate::\\{[^;]*\\b${alias}\\b`, "g"),
      ))
        add(module, match.index);
  }
  return {
    files: owners,
    modules: [...names].sort(),
    edges: [...edges.values()].sort((a, b) =>
      `${a.from}->${a.to}`.localeCompare(`${b.from}->${b.to}`),
    ),
  };
}

/** Strongly connected components. Every internal edge lies on a cycle. */
export function components(nodes, edges) {
  const reach = (start) => {
    const seen = new Set();
    const walk = (node) => {
      if (seen.has(node)) return;
      seen.add(node);
      for (const edge of edges.filter((edge) => edge.from === node)) walk(edge.to);
    };
    walk(start);
    return seen;
  };
  const closures = new Map(nodes.map((node) => [node, reach(node)]));
  const remaining = new Set(nodes);
  const result = [];
  for (const node of nodes) {
    if (!remaining.has(node)) continue;
    const group = nodes.filter(
      (other) =>
        remaining.has(other) && closures.get(node).has(other) && closures.get(other).has(node),
    );
    for (const member of group) remaining.delete(member);
    result.push(group);
  }
  return result;
}

/** Refuse a cycle, naming its path rather than just reporting a boolean. */
export function assertAcyclic(crates) {
  const done = new Set();
  const visit = (node, path) => {
    if (path.includes(node))
      throw new Error(`crate cycle: ${[...path.slice(path.indexOf(node)), node].join(" -> ")}`);
    if (done.has(node)) return;
    if (!Object.hasOwn(crates, node)) throw new Error(`unknown crate: ${node}`);
    for (const dep of crates[node].deps) visit(dep, [...path, node]);
    done.add(node);
  };
  for (const node of Object.keys(crates)) visit(node, []);
}

/** Validate exact file ownership and account for every source edge across crates. */
export function validate(mapping, graph, root = ".") {
  const object = (value, keys, where) => {
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw new Error(`${where} must be an object`);
    for (const key of Object.keys(value))
      if (!keys.includes(key)) throw new Error(`unknown ${where} field: ${key}`);
  };
  const strings = (value, where) => {
    if (!Array.isArray(value) || value.some((item) => typeof item !== "string" || !item))
      throw new Error(`${where} must contain nonempty strings`);
  };
  object(
    mapping,
    [
      "schema",
      "evidenceRevision",
      "crates",
      "edges",
      "suites",
      "assets",
      "auxiliarySuites",
      "commands",
      "dispatcher",
      "generatedEdges",
    ],
    "layout",
  );
  if (![1, 2].includes(mapping.schema)) throw new Error("layout must use schema 1 or 2");
  if (mapping.evidenceRevision !== undefined && !/^[a-f0-9]{40}$/.test(mapping.evidenceRevision))
    throw new Error("invalid evidenceRevision");
  if (!mapping.crates || typeof mapping.crates !== "object" || Array.isArray(mapping.crates))
    throw new Error("crates must be an object");
  for (const [name, crate] of Object.entries(mapping.crates)) {
    if (!/^[a-z][a-z0-9-]*$/.test(name)) throw new Error(`invalid crate: ${name}`);
    object(crate, ["files", "deps", "extract"], `crate ${name}`);
    strings(crate.files, `${name}.files`);
    strings(crate.deps, `${name}.deps`);
    if (crate.extract !== undefined && typeof crate.extract !== "string")
      throw new Error(`${name}.extract must be text`);
  }
  if (!mapping.edges || typeof mapping.edges !== "object" || Array.isArray(mapping.edges))
    throw new Error("edges must be an object");
  for (const [name, edge] of Object.entries(mapping.edges)) {
    object(edge, ["kind", "fix", "interface", "movedTo"], `edge ${name}`);
    if (
      !["move", "shared", "seam"].includes(edge.kind) ||
      typeof edge.fix !== "string" ||
      !edge.fix.trim()
    )
      throw new Error(`invalid edge ${name}`);
    if (edge.interface !== undefined) {
      object(edge.interface, ["owner", "trait", "adapter"], `interface ${name}`);
      const port = edge.interface;
      if (
        ![port.owner, port.adapter].every(
          (value) => typeof value === "string" && Object.hasOwn(mapping.crates, value),
        ) ||
        typeof port.trait !== "string" ||
        !/^[A-Z][A-Za-z0-9]*$/.test(port.trait)
      )
        throw new Error(`invalid interface ${name}`);
    }
    if (
      edge.movedTo !== undefined &&
      (typeof edge.movedTo !== "string" || !Object.hasOwn(mapping.crates, edge.movedTo))
    )
      throw new Error(`invalid movedTo ${name}`);
  }
  if (mapping.schema === 2) {
    if (
      !mapping.commands ||
      typeof mapping.commands !== "object" ||
      Array.isArray(mapping.commands)
    )
      throw new Error("commands must be an object");
    for (const [file, name] of Object.entries(mapping.commands))
      if (!Object.hasOwn(graph.files, file) || typeof name !== "string")
        throw new Error(`invalid command owner: ${file}`);
    object(
      mapping.dispatcher,
      ["crate", "routingFiles", "routingFunctions", "functionMoves"],
      "dispatcher",
    );
    strings(mapping.dispatcher.routingFiles, "dispatcher.routingFiles");
    strings(mapping.dispatcher.routingFunctions, "dispatcher.routingFunctions");
    const moves =
      mapping.dispatcher.functionMoves === undefined ? {} : mapping.dispatcher.functionMoves;
    if (!moves || typeof moves !== "object" || Array.isArray(moves))
      throw new Error("invalid dispatcher functionMoves object");
    for (const file of mapping.crates[mapping.dispatcher.crate]?.files ?? [])
      if (!Object.hasOwn(graph.files, file)) throw new Error(`stale source file ${file}`);
    const functions = new Set(
      mapping.crates[mapping.dispatcher.crate]?.files.flatMap((file) =>
        [...codeOnly(readFileSync(join(root, file), "utf8")).matchAll(/\bfn\s+(\w+)\s*[(<]/g)].map(
          (match) => `${file}::${match[1]}`,
        ),
      ),
    );
    for (const [name, destination] of Object.entries(moves))
      if (
        !functions.has(name) ||
        typeof destination !== "string" ||
        !Object.hasOwn(mapping.crates, destination) ||
        destination === mapping.dispatcher.crate
      )
        throw new Error(`invalid dispatcher functionMoves entry: ${name}`);
    if (!Array.isArray(mapping.generatedEdges)) throw new Error("generatedEdges must be an array");
    for (const edge of mapping.generatedEdges) {
      object(edge, ["from", "to", "reason"], "generated edge");
      if (
        ![edge.from, edge.to, edge.reason].every(
          (value) => typeof value === "string" && value.trim(),
        )
      )
        throw new Error("generated edge must name endpoints and a reason");
    }
    const sourceEdges = new Set(graph.edges.map((edge) => `${edge.from}->${edge.to}`));
    for (const [name, edge] of Object.entries(mapping.edges)) {
      if (!sourceEdges.has(name)) throw new Error(`stale source edge ${name}`);
      if (edge.kind === "move" && typeof edge.movedTo !== "string")
        throw new Error(`edge ${name}: movedTo is required`);
    }
  }
  for (const field of ["suites", "auxiliarySuites"]) {
    const suites = mapping[field] ?? {};
    if (!suites || typeof suites !== "object" || Array.isArray(suites))
      throw new Error(`${field} must be an object`);
    const owned = new Set();
    for (const [name, suite] of Object.entries(suites)) {
      object(suite, ["paths", "crates", "modules", "reason"], `suite ${name}`);
      strings(suite.paths, `${name}.paths`);
      strings(suite.crates, `${name}.crates`);
      for (const crate of suite.crates)
        if (!Object.hasOwn(mapping.crates, crate))
          throw new Error(`suite ${name}: unknown crate ${crate}`);
      if (suite.reason !== undefined && typeof suite.reason !== "string")
        throw new Error(`${name}.reason must be text`);
      if (field === "suites" && suite.modules === undefined)
        throw new Error(`${name}.modules is required for a timed suite`);
      if (suite.modules !== undefined) {
        strings(suite.modules, `${name}.modules`);
        if (field === "suites" && suite.modules.length === 0)
          throw new Error(`${name}.modules must not be empty`);
        const declared =
          field === "suites"
            ? new Set(
                [
                  ...readFileSync(join(root, "tests/e2e/main.rs"), "utf8").matchAll(
                    /^mod (\w+);/gm,
                  ),
                ].map((match) => match[1]),
              )
            : null;
        for (const module of suite.modules) {
          if (!/^[a-z][a-z0-9_]*$/.test(module) || (declared && !declared.has(module)))
            throw new Error(`suite ${name}: unknown test module ${module}`);
          if (owned.has(module)) throw new Error(`duplicate suite module: ${module}`);
          owned.add(module);
        }
      }
    }
    if (field === "suites" && mapping.suites !== undefined) {
      const declared = [
        ...readFileSync(join(root, "tests/e2e/main.rs"), "utf8").matchAll(/^mod (\w+);/gm),
      ].map((match) => match[1]);
      const missing = declared.filter(
        (module) =>
          /#\s*\[\s*test\s*\]/.test(
            codeOnly(readFileSync(join(root, `tests/e2e/${module}.rs`), "utf8")),
          ) && !owned.has(module),
      );
      if (missing.length) throw new Error(`unowned test modules: ${missing.join(", ")}`);
    }
  }
  const assets = mapping.assets ?? {};
  if (!assets || typeof assets !== "object" || Array.isArray(assets))
    throw new Error("assets must be an object");
  for (const [name, paths] of Object.entries(assets)) {
    if (!Object.hasOwn(mapping.crates, name)) throw new Error(`asset owner is unknown: ${name}`);
    strings(paths, `assets.${name}`);
  }
  const actualAssets = sourceAssets(root);
  const mappedAssets = Object.values(assets)
    .flat()
    .filter((path) => path.startsWith("src/"));
  for (const path of actualAssets)
    if (mappedAssets.filter((mapped) => mapped === path).length !== 1)
      throw new Error(`source asset ${path}: expected one owner`);
  for (const path of mappedAssets)
    if (!actualAssets.includes(path)) throw new Error(`stale source asset: ${path}`);
  if (mapping.auxiliarySuites !== undefined) {
    const actual = auxiliaryInputs(root);
    if (
      Object.keys(actual).sort().join("\n") !==
      Object.keys(mapping.auxiliarySuites).sort().join("\n")
    )
      throw new Error("auxiliary suite membership differs from Rust journey projects");
    for (const [name, paths] of Object.entries(actual))
      if ([...mapping.auxiliarySuites[name].paths].sort().join("\n") !== paths.join("\n"))
        throw new Error(`auxiliary suite ${name}: inputs differ from its Nx project`);
  }
  assertAcyclic(mapping.crates);
  for (const file of Object.keys(graph.files)) {
    const owners = Object.entries(mapping.crates).filter(([, crate]) => crate.files.includes(file));
    if (owners.length !== 1) throw new Error(`${file}: expected one owner, found ${owners.length}`);
  }
  for (const [name, crate] of Object.entries(mapping.crates)) {
    for (const file of crate.files)
      if (!Object.hasOwn(graph.files, file)) throw new Error(`${name}: stale source ${file}`);
  }
  const owner = Object.fromEntries(
    Object.entries(mapping.crates).flatMap(([name, crate]) =>
      crate.files.map((file) => [graph.files[file], name]),
    ),
  );
  const transformed = Object.fromEntries(
    Object.keys(mapping.crates).map((name) => [name, { deps: [] }]),
  );
  const transformedEdges = [];
  const add = (from, to, source, via) => {
    if (!transformed[from] || !transformed[to])
      throw new Error(`unknown transformed owner ${from}->${to}`);
    transformedEdges.push({ from, to, source, ...(via ? { via } : {}) });
    if (from !== to && !transformed[from].deps.includes(to)) transformed[from].deps.push(to);
  };
  // A root module and its directory children must move together: root-level edges
  // otherwise have no unambiguous crate owner.
  for (const module of graph.modules) {
    const owners = new Set(
      Object.entries(graph.files)
        .filter(([, name]) => name === module)
        .map(([file]) =>
          Object.keys(mapping.crates).find((name) => mapping.crates[name].files.includes(file)),
        ),
    );
    if (owners.size !== 1) throw new Error(`module ${module}: split ownership`);
  }
  if (mapping.schema === 2) {
    for (const [file, expected] of Object.entries(mapping.commands ?? {})) {
      if (!expected.startsWith("cmd-") || !mapping.crates[expected]?.files.includes(file))
        throw new Error(`command module ${file}: expected command crate ${expected}`);
    }
    const dispatcher = mapping.dispatcher;
    if (!dispatcher || !mapping.crates[dispatcher.crate]) throw new Error("missing dispatcher");
    for (const file of mapping.crates[dispatcher.crate].files) {
      if (!dispatcher.routingFiles.includes(file))
        throw new Error(`dispatcher holds non-routing module ${file}`);
      const functions = [
        ...codeOnly(readFileSync(join(root, file), "utf8")).matchAll(/\bfn\s+(\w+)\s*[(<]/g),
      ].map((match) => match[1]);
      for (const name of functions) {
        const moved = dispatcher.functionMoves?.[`${file}::${name}`];
        if (!dispatcher.routingFunctions.includes(name) && !moved)
          throw new Error(`dispatcher holds non-routing function ${file}::${name}`);
        if (moved) add(dispatcher.crate, moved, `${file}::${name}`);
      }
    }
  }
  for (const edge of graph.edges) {
    const [from, to] = [owner[edge.from], owner[edge.to]];
    const key = `${edge.from}->${edge.to}`;
    const fix = mapping.edges[key];
    if (from !== to && (!fix?.fix || !["move", "shared", "seam"].includes(fix.kind)))
      throw new Error(`unclassified edge ${key}`);
    if (fix?.kind === "seam" && from !== to) {
      const port = fix.interface;
      if (
        mapping.schema === 2 &&
        (!port || port.owner !== from || !/^[A-Z][A-Za-z0-9]*$/.test(port.trait))
      )
        throw new Error(`edge ${key}: interface must name a trait owned by ${from}`);
      // The implementation import becomes a local trait reference. Composition
      // must still depend on both the trait owner and its real implementation.
      add(from, from, key, port?.trait ?? "LegacyProvider");
      if (port) {
        add(port.adapter, from, key, port.trait);
        add(port.adapter, to, key, port.trait);
      }
    } else if (fix?.kind === "move" && from !== to) {
      const destination = fix.movedTo ?? to;
      if (destination !== to) throw new Error(`edge ${key}: move must land in command owner ${to}`);
      add(destination, to, key);
      if (mapping.schema === 2)
        add(
          "onepipeline" in transformed ? "onepipeline" : mapping.dispatcher.crate,
          destination,
          key,
        );
    } else add(from, to, key);
  }
  if (mapping.schema === 2)
    for (const edge of mapping.generatedEdges ?? []) add(edge.from, edge.to, edge.reason);
  assertAcyclic(transformed);
  for (const [from, crate] of Object.entries(transformed)) {
    crate.deps.sort();
    for (const to of crate.deps)
      if (!mapping.crates[from].deps.includes(to))
        throw new Error(`missing dependency ${from}->${to}`);
  }
  return {
    files: Object.keys(graph.files).length,
    crates: Object.keys(mapping.crates).length,
    cyclicComponents: components(graph.modules, graph.edges).filter((group) => group.length > 1),
    edges: graph.edges.length,
    transformedCrates: transformed,
    transformedEdges,
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length > 4)
      throw new Error("usage: node scripts/modularization/graph.mjs [MAPPING.json [REPORT.json]]");
    const mapping = JSON.parse(
      readFileSync(process.argv[2] ?? "scripts/modularization/layout.json", "utf8"),
    );
    const result = validate(mapping, moduleGraph());
    if (process.argv[3]) writeFileSync(process.argv[3], `${JSON.stringify(result, null, 2)}\n`);
    console.log(
      `module graph: ${result.files} files, ${result.crates} crates, ${result.edges} source edges; crate graph acyclic${process.argv[3] ? `; wrote ${process.argv[3]}` : ""}`,
    );
  } catch (error) {
    const next =
      error.code && /^(?:EACCES|EPERM|ENOENT|EISDIR|ENOTDIR|ENOSPC|EROFS)$/.test(error.code)
        ? "check the named path's type, existence, permissions and disk space, and that the output parent is writable; rerun the same node scripts/modularization/graph.mjs MAPPING.json [REPORT.json] invocation"
        : "correct the named layout entry and rerun node scripts/modularization/graph.mjs MAPPING.json [REPORT.json]";
    console.error(`${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}
