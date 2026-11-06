#!/usr/bin/env node
import { readFileSync } from "node:fs";

const file = new URL("./ci-tool-versions.json", import.meta.url);
const names = [
  "node",
  "python",
  "uv",
  "just",
  "nextest",
  "cargo-llvm-cov",
  "cargo-deny",
  "cargo-machete",
  "llmlint",
  "oneharness",
];
try {
  if (process.argv.length !== 2) throw new Error("this reader takes no arguments");
  const versions = JSON.parse(readFileSync(file, "utf8"));
  if (
    !versions ||
    Array.isArray(versions) ||
    typeof versions !== "object" ||
    Object.keys(versions).length !== names.length ||
    Object.keys(versions).some((key) => !names.includes(key))
  )
    throw new Error("the tool-version object must name exactly the CI tools");
  for (const name of names) {
    if (
      typeof versions[name] !== "string" ||
      !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(versions[name])
    )
      throw new Error(`${name} must be an exact stable version`);
  }
  const tools = (list) => list.map((name) => `${name}@${versions[name]}`).join(",");
  for (const name of ["node", "python", "uv", "llmlint", "oneharness"])
    console.log(`${name}=${versions[name]}`);
  console.log(`lint_tools=${tools(["just", "cargo-deny", "cargo-machete"])}`);
  console.log(`linux_tools=${tools(["just", "nextest", "cargo-llvm-cov"])}`);
  console.log(`quick_tools=${tools(["just", "nextest"])}`);
} catch (error) {
  console.error(`cannot read CI tool pins: ${error.message}`);
  console.error(
    "ACTION: restore exact tool versions in .github/ci-tool-versions.json, then rerun CI.",
  );
  process.exitCode = 2;
}
