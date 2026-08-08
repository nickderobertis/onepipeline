// One product, three registries. Each wants its own manifest, so the
// description and the keyword set are physically duplicated across Cargo.toml,
// pyproject.toml, and the npm launcher's package.json — none of the three
// formats can reference another. Cargo.toml is the source (it is already the
// source of the version, which release-plz maintains and the other two read),
// and this is the gate that keeps the copies honest: change the description in
// one place and the suite says which manifests disagree.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

/** The value of a top-level `key = "..."` in a TOML table, read line-wise. */
function tomlString(text, table, key) {
  const section = text.split(`[${table}]`)[1];
  assert.ok(section, `no [${table}] table`);
  const line = section
    .split(/^\[/m)[0]
    .split("\n")
    .find((candidate) => candidate.startsWith(`${key} = `));
  assert.ok(line, `[${table}] has no ${key}`);
  return JSON.parse(line.slice(`${key} = `.length));
}

/** The value of a top-level `key = [...]` in a TOML table, on one line. */
function tomlStringArray(text, table, key) {
  const section = text.split(`[${table}]`)[1];
  assert.ok(section, `no [${table}] table`);
  const line = section
    .split(/^\[/m)[0]
    .split("\n")
    .find((candidate) => candidate.startsWith(`${key} = [`));
  assert.ok(line, `[${table}] has no single-line ${key}`);
  return JSON.parse(line.slice(`${key} = `.length));
}

describe("distribution metadata", () => {
  const cargo = readFileSync(join(root, "Cargo.toml"), "utf8");
  const pyproject = readFileSync(join(root, "pyproject.toml"), "utf8");
  const npmPackage = JSON.parse(
    readFileSync(join(root, "npm", "onepipeline-cli", "package.json"), "utf8"),
  );

  const description = tomlString(cargo, "package", "description");
  const keywords = tomlStringArray(cargo, "package", "keywords");

  it("describes the same product on PyPI as on crates.io", () => {
    assert.equal(tomlString(pyproject, "project", "description"), description);
  });

  it("describes the same product on npm as on crates.io", () => {
    assert.equal(npmPackage.description, description);
  });

  it("carries one keyword set across all three registries", () => {
    assert.deepEqual(tomlStringArray(pyproject, "project", "keywords"), keywords);
    assert.deepEqual(npmPackage.keywords, keywords);
  });
});
