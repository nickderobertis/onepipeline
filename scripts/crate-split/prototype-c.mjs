#!/usr/bin/env node
// Option (c)'s throwaway prototype: the engine's `src/` split into crates along its
// module dependency graph as it stands, so a hosted build can time what the split
// costs to compile and link against today's one crate.
//
//   node scripts/crate-split/prototype-c.mjs --repo DIR
//
// The layering is the graph's own (`module-graph.mjs`): `onepipeline-core` is the one
// large cycle, `onepipeline-base` everything that cycle reaches outside itself, and the
// root crate keeps every other module, `lib.rs` and `main.rs`. Finer crates are possible
// for the modules outside the cycle but change nothing a build of the cycle pays. Each
// new crate loads its modules from where they are, through one inline `mod` whose
// `#[path]` is `src/`, so nothing moves; it
// carries the `pub use` re-exports `lib.rs` makes of its modules and `VERSION`, and
// re-exports the crates below it under their module names, so `crate::<m>::` paths
// resolve unchanged. `pub(crate)` becomes `pub` throughout `src/`, which a crate
// boundary needs, and the new crates and the root allow the lints that change raises
// (missing docs, unreachable pub, dead code): a prototype's allowances, not a proposal.
import { readdirSync, readFileSync, statSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { split } from "./module-graph.mjs";

const MOD = /^(?:pub(?:\(crate\))?\s+)?mod\s+(\w+);/;

/** `lib.rs` as blocks: each `mod` line with the attribute and doc lines before it. */
export function libBlocks(text) {
  const lines = text.split("\n");
  const blocks = [];
  let pending = [];
  for (const line of lines) {
    const m = line.match(MOD);
    if (m) {
      blocks.push({ module: m[1], lines: [...pending, line], pub: line.startsWith("pub") });
      pending = [];
    } else if (/^\s*(#\[|\/\/\/)/.test(line)) pending.push(line);
    else {
      for (const held of pending) blocks.push({ module: null, lines: [held] });
      pending = [];
      blocks.push({ module: null, lines: [line] });
    }
  }
  for (const held of pending) blocks.push({ module: null, lines: [held] });
  return blocks;
}

function rsFiles(dir) {
  const out = [];
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) out.push(...rsFiles(path));
    else if (entry.endsWith(".rs")) out.push(path);
  }
  return out;
}

/** The root manifest's dependency sections, with their path dependencies re-rooted. */
export function dependencySections(manifest) {
  const sections = [];
  let current = null;
  for (const line of manifest.split("\n")) {
    // An array-of-tables header (`[[test]]`) ends a section too.
    const header = line.match(/^\[\[?([^\]]+)\]\]?\s*$/);
    if (header) {
      current = /^(dependencies|dev-dependencies|target\..*\.dependencies)$/.test(header[1])
        ? [line]
        : null;
      if (current) sections.push(current);
    } else if (current) current.push(line.replace(/path = "(?!\.\.\/\.\.\/)/g, 'path = "../../'));
  }
  return sections.map((lines) => lines.join("\n")).join("\n");
}

export function prototypeC(repo) {
  const graph = split(repo);
  const [coreName, core] = Object.entries(graph.crates).sort(
    (a, b) => b[1].modules.length - a[1].modules.length,
  )[0];
  const below = new Set();
  const reach = (name) => {
    for (const dep of graph.crates[name].deps) {
      if (!below.has(dep)) {
        below.add(dep);
        reach(dep);
      }
    }
  };
  reach(coreName);
  const layers = {
    "onepipeline-base": [...below].flatMap((name) => graph.crates[name].modules).sort(),
    "onepipeline-core": core.modules,
  };
  const libPath = join(repo, "src", "lib.rs");
  const lib = readFileSync(libPath, "utf8");
  const blocks = libBlocks(lib);
  const manifestPath = join(repo, "Cargo.toml");
  const manifest = readFileSync(manifestPath, "utf8");
  const version = manifest.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)[1];
  const edition = manifest.match(/^\[package\][\s\S]*?^edition\s*=\s*"([^"]+)"/m)?.[1] ?? "2021";
  const deps = dependencySections(manifest);
  const allow = "#![allow(missing_docs, unreachable_pub, dead_code, unused, clippy::all)]";
  // A dependency's `#[cfg(test)]` items are not built for a dependent's tests, so a
  // moved module's test-only item another crate's tests use is gated on this feature
  // too, which each dependent's dev-dependencies turn on.
  const TEST_CFG = '#[cfg(any(test, feature = "test-support"))]';
  const reexports = (modules) =>
    lib.split("\n").filter((line) => {
      const m = line.match(/^pub use (\w+)::/);
      return m && modules.includes(m[1]);
    });
  let lower = [];
  const lowerCrates = [];
  for (const [crate, modules] of Object.entries(layers)) {
    const dir = join(repo, "crates", crate);
    mkdirSync(join(dir, "src"), { recursive: true });
    const body = [
      `//! Option (c)'s prototype crate ${crate}: written by scripts/crate-split/prototype-c.mjs.`,
      allow,
      `pub const VERSION: &str = "${version}";`,
      ...lowerCrates.map((c) => `pub use ${c.replace(/-/g, "_")}::*;`),
    ];
    // One inline module whose path is `src/`, so each module and the modules nested
    // under `src/<m>/` resolve by Rust's ordinary rules, re-exported at the root.
    body.push('#[path = "../../../src"]', "mod src {");
    const names = [];
    for (const block of blocks.filter((b) => modules.includes(b.module))) {
      const attrs = block.lines.slice(0, -1).filter((l) => !l.trimStart().startsWith("///"));
      const gated = attrs;
      body.push(...gated.map((l) => `    ${l}`), `    pub mod ${block.module};`);
      names.push({ attrs: gated, name: block.module });
    }
    body.push("}");
    for (const { attrs, name } of names) body.push(...attrs, `pub use src::${name};`);
    body.push(...reexports([...lower, ...modules]), "");
    writeFileSync(join(dir, "src", "lib.rs"), body.join("\n"));
    const own = lowerCrates.map((c) => `${c} = { path = "../${c}" }`).join("\n");
    const ownTest = lowerCrates
      .map((c) => `${c} = { path = "../${c}", features = ["test-support"] }`)
      .join("\n");
    writeFileSync(
      join(dir, "Cargo.toml"),
      `[package]\nname = "${crate}"\nversion = "${version}"\nedition = "${edition}"\npublish = false\n\n[features]\ntest-support = []\n\n[lib]\npath = "src/lib.rs"\ndoctest = false\n\n${deps
        .replace(/^\[dependencies\]$/m, `[dependencies]\n${own}`)
        .replace(/^\[dev-dependencies\]$/m, `[dev-dependencies]\n${ownTest}`)}\n`,
    );
    lower = [...lower, ...modules];
    lowerCrates.push(crate);
  }
  // The root: each moved module's declaration becomes a re-export of the crate it is in.
  const moved = new Map(Object.entries(layers).flatMap(([crate, ms]) => ms.map((m) => [m, crate])));
  const root = [allow];
  for (const block of blocks) {
    if (block.module && moved.has(block.module)) {
      const attrs = block.lines.slice(0, -1).filter((l) => !l.trimStart().startsWith("///"));
      // A test-only module is its own crate's: the root's tests do not reach it.
      if (attrs.some((l) => l.includes("#[cfg(test)]"))) continue;
      root.push(
        ...attrs,
        `pub use ${moved.get(block.module).replace(/-/g, "_")}::${block.module};`,
      );
    } else root.push(...block.lines);
  }
  writeFileSync(libPath, root.join("\n").replace(/^#!\[warn\(missing_docs\)\]$/m, ""));
  for (const file of rsFiles(join(repo, "src"))) {
    let text = readFileSync(file, "utf8").replaceAll("pub(crate)", "pub");
    const top = file
      .slice(join(repo, "src").length + 1)
      .split("/")[0]
      .replace(/\.rs$/, "");
    if (moved.get(top) === "onepipeline-base") {
      // Its test-only items, but never a test module, which needs dev-dependencies.
      text = text.replace(
        /^(\s*)#\[cfg\(test\)\]\n(?!(?:\s*#\[[^\n]*\]\n)*\s*(?:pub\s+)?mod\s)/gm,
        `$1${TEST_CFG}\n`,
      );
    }
    writeFileSync(file, text);
  }
  const members = manifest.match(/^members = \[([^\]]*)\]/m);
  let next = manifest.replace(
    members[0],
    // Default members too: an archive built without --workspace (as the tiers' is)
    // builds the default members, which must hold the unit tests that moved.
    `members = [${members[1]}, "crates/onepipeline-base", "crates/onepipeline-core"]\ndefault-members = [".", "crates/onepipeline-base", "crates/onepipeline-core"]`,
  );
  next = next.replace(
    /^\[dev-dependencies\]$/m,
    '[dev-dependencies]\nonepipeline-core = { path = "crates/onepipeline-core", features = ["test-support"] }\nonepipeline-base = { path = "crates/onepipeline-base", features = ["test-support"] }',
  );
  next = next.replace(
    /^\[dependencies\]$/m,
    '[dependencies]\nonepipeline-core = { path = "crates/onepipeline-core" }\nonepipeline-base = { path = "crates/onepipeline-base" }',
  );
  writeFileSync(manifestPath, next);
  return {
    base: layers["onepipeline-base"].length,
    core: layers["onepipeline-core"].length,
    root: graph.modules - layers["onepipeline-base"].length - layers["onepipeline-core"].length,
  };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const at = process.argv.indexOf("--repo");
  if (at < 0 || !process.argv[at + 1]) {
    console.error("prototype-c: --repo DIR names the checkout to write the prototype into");
    process.exitCode = 1;
  } else {
    const n = prototypeC(process.argv[at + 1]);
    console.log(
      `prototype-c: onepipeline-base ${n.base} modules, onepipeline-core ${n.core}, root ${n.root}`,
    );
  }
}
