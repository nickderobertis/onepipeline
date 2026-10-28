#!/usr/bin/env node
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

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
  return text.replace(
    /\/\*[\s\S]*?\*\/|\/\/[^\n]*|r(#+)?"[\s\S]*?"\1|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])'/g,
    (value) => value.replace(/[^\n]/g, " "),
  );
}

/** Conservative syntactic graph: root module paths and grouped crate imports. */
export function moduleGraph(root = ".") {
  const files = rustFiles(join(root, "src"));
  const names = new Set(
    files.map((file) => file.slice(join(root, "src").length + 1).split(/[/.]/)[0]),
  );
  const edges = new Map();
  const owners = {};
  for (const file of files) {
    const relative = file.slice(root === "." ? 0 : root.length + 1);
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
    for (const match of code.matchAll(/\b(?:crate|onepipeline)::([a-z][a-z0-9_]*)/g))
      add(match[1], match.index);
    for (const match of code.matchAll(/\b(?:crate|super)::\s*\{([\s\S]*?)\};/g)) {
      for (const item of match[1].matchAll(/\b([a-z][a-z0-9_]*)\b/g)) add(item[1], match.index);
    }
    // The root's Error/Result aliases otherwise hide every module's error edge.
    for (const match of code.matchAll(
      /\bcrate::(?:Error|Result)\b|\buse\s+crate::\{[^;]*\b(?:Error|Result)\b/g,
    ))
      add("error", match.index);
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
export function validate(mapping, graph) {
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
  for (const edge of graph.edges) {
    const [from, to] = [owner[edge.from], owner[edge.to]];
    if (from === to) continue;
    const fix = mapping.edges[`${edge.from}->${edge.to}`];
    if (!fix?.fix || !["move", "shared", "seam"].includes(fix.kind))
      throw new Error(`unclassified edge ${edge.from}->${edge.to}`);
    if (fix.kind !== "seam" && !mapping.crates[from].deps.includes(to))
      throw new Error(`missing dependency ${from}->${to}`);
  }
  return {
    files: Object.keys(graph.files).length,
    crates: Object.keys(mapping.crates).length,
    cyclicComponents: components(graph.modules, graph.edges).filter((group) => group.length > 1),
    edges: graph.edges.length,
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const mapping = JSON.parse(
      readFileSync(process.argv[2] ?? "scripts/modularization/layout.json", "utf8"),
    );
    console.log(JSON.stringify(validate(mapping, moduleGraph()), null, 2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
