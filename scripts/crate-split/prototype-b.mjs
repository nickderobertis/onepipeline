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
// allowance, not a proposal. It prints one line: the `--test` arguments of each layout.
//
// Exit 0: written. 1: --repo or its proposal was refused, with a `next:` action.
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import * as check from "./validate.mjs";

export function prototypeB(repo) {
  const path = join(repo, "scripts", "crate-split", "proposed.json");
  let proposed;
  try {
    proposed = JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    throw new check.Refusal(
      `${path} cannot be read as JSON: ${error.message}`,
      "give --repo a checkout that holds scripts/crate-split/proposed.json",
    );
  }
  const next = "keep proposed.json in the shape crate-split-replay.mjs's head gives";
  check.need(
    proposed?.suites && typeof proposed.suites === "object",
    `${path} holds no suites`,
    next,
  );
  for (const [suite, spec] of Object.entries(proposed.suites)) {
    // The name becomes a file name, a Rust comment and a Cargo target name.
    check.need(
      /^[A-Za-z0-9-]+$/.test(suite) && !(suite in Object.prototype),
      `${JSON.stringify(suite)} is not a suite name a target can take`,
      next,
    );
    check.need(
      Array.isArray(spec?.paths) && spec.paths.every((p) => typeof p === "string"),
      `suite ${suite} has no list of paths`,
      next,
    );
  }
  check.need(
    existsSync(join(repo, "tests", "e2e", "main.rs")),
    `${repo} holds no tests/e2e/main.rs`,
    "give --repo a checkout of this repository",
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
  const usage = "node scripts/crate-split/prototype-b.mjs --repo DIR";
  try {
    const options = check.parseOptions(process.argv.slice(2), ["repo"], { usage });
    check.need(options.repo, "--repo names the checkout to write the prototype into", usage);
    const layouts = prototypeB(options.repo);
    console.log(`prototype-b: today ${layouts.today.join(" ")}; split ${layouts.split.join(" ")}`);
  } catch (error) {
    console.error(`prototype-b: ${error.message.split("\n")[0]}`);
    console.error(`next: ${error instanceof check.Refusal ? error.next : usage}`);
    process.exitCode = 1;
  }
}
