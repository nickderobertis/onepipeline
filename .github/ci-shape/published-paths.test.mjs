import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { parse } from "yaml";
const read = (path) => readFileSync(new URL(`../../${path}`, import.meta.url), "utf8");
const git = (...args) => execFileSync("git", args, { encoding: "utf8" }).trim().split("\n").filter(Boolean);
const specs = read(".github/release-published-paths").split("\n").filter((p) => p && !p.startsWith("#"));
const matched = new Set(git("ls-files", "--", ...specs));

test("published paths cover the real crate package and wheel and npm build inputs", { timeout: 120000 }, () => {
  const packed = execFileSync("cargo", ["package", "--list", "--allow-dirty", "--locked", "--offline"], { encoding: "utf8" }).trim().split("\n");
  const generated = new Set([".cargo_vcs_info.json", "Cargo.toml.orig"]);
  const tracked = new Set(git("ls-files"));
  for (const file of packed.filter((p) => !generated.has(p) && tracked.has(p))) assert.ok(matched.has(file), `published crate file omitted: ${file}`);
  const npm = JSON.parse(read("npm/onepipeline-cli/package.json"));
  for (const file of ["package.json", ...npm.files]) assert.ok(matched.has(`npm/onepipeline-cli/${file}`), `npm package file omitted: ${file}`);
  const wheel = read("pyproject.toml");
  const readme = wheel.match(/^readme = "([^"]+)"/m)[1];
  const manifest = wheel.match(/^manifest-path = "([^"]+)"/m)[1];
  for (const file of [readme, manifest, "pyproject.toml", "CHANGELOG.md", "scripts/build-linux-wheel.sh", "scripts/npm-build.mjs", "release/release.py", "rust-toolchain.toml", "Cargo.lock", ".cargo/config.toml", "justfile"])
    assert.ok(matched.has(file), `wheel or registry build input omitted: ${file}`);
  assert.ok(!new Set(git("ls-files", "--", ...specs.filter((p) => p !== "Cargo.toml"))).has("Cargo.toml"), "the coverage assertion must detect a removed manifest path");
});

test("publish fetches history and checks both registered release-tail commands", () => {
  const workflow = parse(read(".github/workflows/release.yml"));
  const steps = workflow.jobs.publish.steps;
  const checkout = steps.find((s) => s.uses?.startsWith("actions/checkout@"));
  assert.equal(checkout.with["fetch-depth"], 0);
  assert.equal(steps.at(-1).run.trim(), "onebudgetspec check budgets.yaml --label release-tail");
  const budgets = parse(read("budgets.yaml")).budgets;
  for (const [id, command] of [
    ["onepipeline-nx-cache-use", ["bash", "scripts/actions-budget-figures.sh", "cache-use"]],
    ["onepipeline-release-cycle-time", ["bash", "scripts/actions-budget-figures.sh", "release-cycle-time", "--published-paths", ".github/release-published-paths"]],
  ]) {
    const budget = budgets.find((b) => b.id === id);
    assert.ok(budget.labels.includes("release-tail"));
    assert.deepEqual(budget.command, command);
  }
});
