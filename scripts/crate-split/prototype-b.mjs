#!/usr/bin/env node
// Option (b)'s throwaway prototype: the e2e binary split into the suites
// `scripts/crate-split/proposed.json` maps, each its own `[[test]]` target, so a hosted
// build can time what the split costs to compile and link against today's one binary.
//
//   node scripts/crate-split/prototype-b.mjs --repo DIR
//
// It writes `tests/e2e_split/<suite>.rs` for each suite, a `mod` for every
// `tests/e2e/*.rs` the suite's paths name (its modules and those they reach), loaded
// from where it is with `#[path]` so nothing moves, and appends one `[[test]]` per suite
// to `Cargo.toml`, leaving the `e2e` target in place: a build selects one layout or the
// other with `--test`. Each suite allows dead code and unused imports, which a subset of
// the shared harness would otherwise fail under `-D warnings`; that is a prototype's
// allowance, not a proposal. It prints the `--test` arguments of each layout.
import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export function prototypeB(repo) {
  const proposed = JSON.parse(
    readFileSync(join(repo, "scripts", "crate-split", "proposed.json"), "utf8"),
  );
  const main = readFileSync(join(repo, "tests", "e2e", "main.rs"), "utf8");
  const declared = new Set([...main.matchAll(/^mod (\w+);/gm)].map((m) => m[1]));
  mkdirSync(join(repo, "tests", "e2e_split"), { recursive: true });
  const targets = [];
  let cargo = "\n# Option (b)'s throwaway prototype: scripts/crate-split/prototype-b.mjs.\n";
  for (const [suite, spec] of Object.entries(proposed.suites)) {
    const mods = new Set();
    for (const path of spec.paths) {
      const m = /^tests\/e2e\/(\w+)\.rs$/.exec(path);
      if (m && declared.has(m[1])) mods.add(m[1]);
    }
    mods.add("harness");
    const name = suite.replace(/-/g, "_");
    const body = [
      `//! Option (b)'s prototype suite ${suite}: written by scripts/crate-split/prototype-b.mjs.`,
      "#![allow(dead_code, unused_imports, unused_macros)]",
      ...[...mods].sort().map((m) => `#[path = "../e2e/${m}.rs"]\nmod ${m};`),
      "",
    ].join("\n");
    writeFileSync(join(repo, "tests", "e2e_split", `${name}.rs`), body);
    cargo += `\n[[test]]\nname = "${name}"\npath = "tests/e2e_split/${name}.rs"\n`;
    targets.push(name);
  }
  appendFileSync(join(repo, "Cargo.toml"), cargo);
  return { today: ["--test", "e2e"], split: targets.flatMap((t) => ["--test", t]) };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const at = process.argv.indexOf("--repo");
  if (at < 0 || !process.argv[at + 1]) {
    console.error("prototype-b: --repo DIR names the checkout to write the prototype into");
    process.exitCode = 1;
  } else {
    const layouts = prototypeB(process.argv[at + 1]);
    console.log(`today: ${layouts.today.join(" ")}`);
    console.log(`split: ${layouts.split.join(" ")}`);
  }
}
