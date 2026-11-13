#!/usr/bin/env node
// The engine's module dependency graph as it stands, and the finest crate split it
// allows: option (c) of the reuse spike.
//
//   node scripts/crate-split/module-graph.mjs --repo DIR --out FILE [--mapping FILE]
//        [--base MAPPING]
//
// Each top-level module of `src/` (a `src/<m>.rs` and anything under `src/<m>/`)
// depends on every other one its text names as `crate::<m>`, inside a grouped
// `crate::{...}` import, or through an item `src/lib.rs` re-exports (`pub use <m>::X`
// named as `crate::X`). Cargo refuses a cycle among crates, so modules in one strongly
// connected component must share a crate: the components, and the acyclic graph
// between them, are the finest split there is. `lib.rs` and `main.rs` are the root and
// the binary and are left out of the graph; test-only modules are kept.
//
// FILE gets the components and their edges. With --mapping it also writes option (c):
// --base's mapping (today's tiers) with the one crate replaced by one crate per
// component, `onepipeline:test-rest` and `onepipeline:test-quick` split into a unit
// suite per component weighed by its modules' test-seconds share (`unit:` modules) and
// the rest of the tier, and every e2e-family suite still exercising every crate, since
// it drives the one binary that links them all.
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";

function files(dir) {
  const out = [];
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) out.push(...files(path));
    else if (entry.endsWith(".rs")) out.push(path);
  }
  return out;
}

/**
 * The code of a Rust source with its comments, string literals and char literals
 * blanked, so a word in prose or in a message is not a reference. A small lexer rather
 * than a pattern: a char literal such as `'"'` or a lifetime opens no string, block
 * comments nest, and a raw string ends only at its own number of `#`.
 */
export function code(text) {
  let out = "";
  let i = 0;
  const n = text.length;
  while (i < n) {
    const c = text[i];
    const next = text[i + 1];
    if (c === "/" && next === "/") {
      while (i < n && text[i] !== "\n") i++;
      out += " ";
    } else if (c === "/" && next === "*") {
      let depth = 0;
      do {
        if (text[i] === "/" && text[i + 1] === "*") {
          depth++;
          i += 2;
        } else if (text[i] === "*" && text[i + 1] === "/") {
          depth--;
          i += 2;
        } else i++;
      } while (i < n && depth > 0);
      out += " ";
    } else if (
      (c === "r" || (c === "b" && next === "r")) &&
      /^b?r#*"/.test(text.slice(i, i + 260)) &&
      !/\w/.test(text[i - 1] ?? "")
    ) {
      const head = text.slice(i).match(/^b?r(#*)"/);
      const close = `"${head[1]}`;
      const end = text.indexOf(close, i + head[0].length);
      i = end < 0 ? n : end + close.length;
      out += '""';
    } else if (c === '"' || (c === "b" && next === '"' && !/\w/.test(text[i - 1] ?? ""))) {
      i += c === "b" ? 2 : 1;
      while (i < n && text[i] !== '"') i += text[i] === "\\" ? 2 : 1;
      i++;
      out += '""';
    } else if (c === "'") {
      // A char literal is one char or one escape and a closing quote; anything else
      // is a lifetime or a label, which is code.
      const literal = text
        .slice(i)
        .match(/^'(?:\\(?:u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^\\'\n])'/);
      if (literal) {
        i += literal[0].length;
        out += "' '";
      } else {
        out += c;
        i++;
      }
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

/** The top-level names a module's code reaches through `crate::`. */
export function references(text) {
  const names = new Set();
  const body = code(text);
  for (const m of body.matchAll(/\bcrate::(\w+)/g)) names.add(m[1]);
  for (const m of body.matchAll(/\bcrate::\{/g)) {
    let depth = 1;
    let i = m.index + m[0].length;
    let token = "";
    let atTop = true;
    while (i < body.length && depth > 0) {
      const c = body[i++];
      if (/\w/.test(c)) {
        token += c;
        continue;
      }
      // A name ends: keep it when it is the first segment of a top-level item.
      if (token && atTop && depth === 1) names.add(token);
      token = "";
      if (c === "{") {
        depth++;
        atTop = false;
      } else if (c === "}") depth--;
      else if (c === ",") atTop = depth === 1 || atTop;
      else if (c === ":") atTop = false;
    }
  }
  return names;
}

export function moduleGraph(repo) {
  const src = join(repo, "src");
  const modules = new Map();
  for (const path of files(src)) {
    const rel = path.slice(src.length + 1);
    const top = rel.split("/")[0].replace(/\.rs$/, "");
    if (top === "lib" || top === "main") continue;
    if (!modules.has(top)) modules.set(top, []);
    modules.get(top).push(rel);
  }
  // Items lib.rs re-exports, so `crate::Item` resolves to the module that defines it.
  const lib = code(readFileSync(join(src, "lib.rs"), "utf8"));
  const reexport = new Map();
  for (const m of lib.matchAll(/pub use (\w+)::\{([^}]*)\}/g)) {
    for (const item of m[2].split(",").map((s) =>
      s
        .trim()
        .split(/\s+as\s+/)
        .at(-1),
    )) {
      if (item) reexport.set(item, m[1]);
    }
  }
  for (const m of lib.matchAll(/pub use (\w+)::(\w+)(?:\s+as\s+(\w+))?;/g))
    reexport.set(m[3] ?? m[2], m[1]);
  const edges = new Map();
  for (const [name, paths] of modules) {
    const deps = new Set();
    for (const rel of paths) {
      for (const ref of references(readFileSync(join(src, rel), "utf8"))) {
        const target = modules.has(ref) ? ref : reexport.get(ref);
        if (target && target !== name && modules.has(target)) deps.add(target);
      }
    }
    edges.set(name, [...deps].sort());
  }
  return { modules, edges };
}

/** Tarjan's strongly connected components, each sorted, in reverse topological order. */
export function components(edges) {
  let index = 0;
  const stack = [];
  const on = new Set();
  const idx = new Map();
  const low = new Map();
  const out = [];
  const visit = (v) => {
    idx.set(v, index);
    low.set(v, index++);
    stack.push(v);
    on.add(v);
    for (const w of edges.get(v) ?? []) {
      if (!idx.has(w)) {
        visit(w);
        low.set(v, Math.min(low.get(v), low.get(w)));
      } else if (on.has(w)) low.set(v, Math.min(low.get(v), idx.get(w)));
    }
    if (low.get(v) === idx.get(v)) {
      const scc = [];
      let w;
      do {
        w = stack.pop();
        on.delete(w);
        scc.push(w);
      } while (w !== v);
      out.push(scc.sort());
    }
  };
  for (const v of [...edges.keys()].sort()) if (!idx.has(v)) visit(v);
  return out;
}

export function split(repo) {
  const { modules, edges } = moduleGraph(repo);
  const sccs = components(edges);
  const crateOf = new Map();
  const crates = {};
  sccs.forEach((scc, i) => {
    const name = scc.length > 1 ? `core-${i}` : `m-${scc[0]}`;
    for (const m of scc) crateOf.set(m, name);
    crates[name] = { modules: scc, deps: new Set() };
  });
  for (const [m, deps] of edges) {
    for (const d of deps)
      if (crateOf.get(d) !== crateOf.get(m)) crates[crateOf.get(m)].deps.add(crateOf.get(d));
  }
  const paths = (scc) =>
    scc
      .flatMap((m) => [`src/${m}.rs`, `src/${m}/**`])
      .filter((p) => modules.has(p.replace(/^src\/|\.rs$|\/\*\*$/g, "")));
  return {
    modules: modules.size,
    components: sccs.length,
    largest: Math.max(...sccs.map((s) => s.length)),
    crates: Object.fromEntries(
      Object.entries(crates).map(([n, c]) => [
        n,
        { modules: c.modules, deps: [...c.deps].sort(), paths: paths(c.modules) },
      ]),
    ),
  };
}

/** Option (c): --base's tiers over one crate per component. */
export function optionC(result, base) {
  const crates = {
    root: {
      paths: ["src/lib.rs", "src/main.rs", "schemas/**", "crates/**"],
      deps: Object.keys(result.crates),
    },
  };
  for (const [name, c] of Object.entries(result.crates))
    crates[name] = { paths: c.paths, deps: c.deps };
  const suites = {};
  for (const [name, suite] of Object.entries(base.suites)) {
    const crate = suite.crates.length ? ["root"] : [];
    if (/^onepipeline:(test-rest|test-quick)$/.test(name)) {
      // The tier's integration binaries and the unit tests of the root still need it all.
      suites[name] = { ...suite, crates: crate };
      for (const [cname, c] of Object.entries(result.crates)) {
        suites[`${name}@${cname}`] = {
          paths: suite.paths.filter((p) => !p.startsWith("tests/")),
          crates: [cname],
          modules: c.modules.map((m) => `unit:${name}:${m}`),
          archive: suite.archive,
        };
      }
    } else suites[name] = { ...suite, crates: crate };
  }
  return {
    "//": `Option (c): ${result.components} crates along the module graph of ${result.modules} modules, the largest holding ${result.largest}; written by scripts/crate-split/module-graph.mjs.`,
    sources: base.sources,
    fallback: "root",
    crates,
    suites,
  };
}

function main(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i += 2) {
    const name = argv[i]?.replace(/^--/, "");
    if (!["repo", "out", "mapping", "base"].includes(name) || argv[i + 1] === undefined) {
      throw new Error(`${argv[i]} is not an option this reads`);
    }
    options[name] = argv[i + 1];
  }
  if (!options.repo || !options.out) throw new Error("--repo and --out are required");
  const result = split(options.repo);
  writeFileSync(options.out, `${JSON.stringify(result, null, 2)}\n`);
  if (options.mapping) {
    if (!options.base) throw new Error("--mapping needs --base, today's tier mapping");
    const base = JSON.parse(readFileSync(options.base, "utf8"));
    writeFileSync(options.mapping, `${JSON.stringify(optionC(result, base), null, 2)}\n`);
  }
  console.log(
    `module-graph: ${result.modules} modules in ${result.components} components; the largest holds ${result.largest}`,
  );
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`module-graph: ${error.message.split("\n")[0]}`);
    process.exitCode = 1;
  }
}
