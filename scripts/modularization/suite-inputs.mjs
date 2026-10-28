#!/usr/bin/env node
import { readFileSync, writeFileSync, existsSync, statSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { moduleGraph, validate } from "./graph.mjs";
import { compileMapping, globToRegExp } from "../crate-split-replay.mjs";

/** Files compiled or read through static paths by a suite's transitive helpers. */
export function suiteInputs(modules, root = ".") {
  const main = readFileSync(join(root, "tests/e2e/main.rs"), "utf8");
  const declared = new Set([...main.matchAll(/^mod (\w+);/gm)].map((match) => match[1]));
  const reached = new Set(["harness", ...modules]);
  const paths = new Set();
  for (const module of reached) {
    if (!declared.has(module)) throw new Error(`undeclared test module: ${module}`);
    const file = `tests/e2e/${module}.rs`;
    paths.add(file);
    const source = readFileSync(join(root, file), "utf8");
    for (const match of source.matchAll(/crate::(\w+)::/g))
      if (declared.has(match[1])) reached.add(match[1]);
    const constants = new Map(
      [...source.matchAll(/const (\w+): &str =\s*"([^"]+)";/g)].map((match) => [
        match[1],
        match[2],
      ]),
    );
    const reads = [
      ...[...source.matchAll(/repo_file\(\s*"([^"]+)"/g)].map((match) => match[1]),
      ...[
        ...source.matchAll(/(?:repo_file\(|CARGO_MANIFEST_DIR"\)\)\s*\.join\()\s*(\w+)\s*\)/g),
      ].map((match) => constants.get(match[1])),
      ...[...source.matchAll(/CARGO_MANIFEST_DIR"\)\)\s*\.join\("([^"]+)"/g)].map(
        (match) => match[1],
      ),
      ...[...source.matchAll(/include_str!\("([^"]+)"/g)].map((match) =>
        join("tests/e2e", match[1]).replaceAll("\\", "/"),
      ),
    ];
    for (const path of reads.filter(Boolean)) {
      if (path === "." || path === "target" || path.startsWith("target/")) continue;
      const normalized = path.replace(/\/$/, "");
      paths.add(normalized);
      if (existsSync(join(root, normalized)) && statSync(join(root, normalized)).isDirectory())
        paths.add(`${normalized}/**`);
    }
  }
  return [...paths].sort();
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length !== 3)
      throw new Error("usage: node scripts/modularization/suite-inputs.mjs MAPPING.json");
    const file = process.argv[2];
    const mapping = JSON.parse(readFileSync(file, "utf8"));
    if (Object.hasOwn(mapping, "schema")) validate(mapping, moduleGraph());
    else compileMapping(mapping);
    for (const suite of Object.values(mapping.suites)) {
      const covered = (path) =>
        [
          ...suite.paths,
          ...(mapping.shared ?? []),
          ...Object.values(mapping.crates).flatMap((crate) => crate.paths ?? crate.files),
        ].some((glob) => globToRegExp(glob).test(path));
      for (const path of suiteInputs(suite.modules)) if (!covered(path)) suite.paths.push(path);
    }
    writeFileSync(file, `${JSON.stringify(mapping, null, 2)}\n`);
  } catch (error) {
    console.error(
      `${error.message}\nnext: correct the named mapping/module, then rerun node scripts/modularization/suite-inputs.mjs MAPPING.json`,
    );
    process.exitCode = 1;
  }
}
