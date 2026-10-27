// What every SBOM onepipeline ships is held to, wherever it ships: the shape
// the wheel's `.dist-info/sboms/` document has and a consumer's pin gate reads.
// `launcher.test.mjs` holds the staged npm package to it and
// `tests/release_journeys/release-linux.journey.mjs` the package npm serves.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

/// Every version Cargo.lock resolves, by crate name.
export function lockVersions() {
  const lock = readFileSync(join(REPO_ROOT, "Cargo.lock"), "utf8");
  const versions = new Map();
  for (const [, name, version] of lock.matchAll(
    /^\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"/gm,
  )) {
    versions.set(name, [...(versions.get(name) ?? []), version]);
  }
  return versions;
}

/// The engines a consumer's pin gate reads out of the SBOM.
export const ENGINES = [
  "onevcs",
  "oneagentgraph",
  "onejudge",
  "oneharness-core",
  "onemessagebus",
  "onetaskgraph-core",
];

/// Hold `text` to the shape the wheel's SBOM has and a consumer reads: the
/// onepipeline crate as the subject, one component per linked crate at the
/// version Cargo.lock resolves, and dependency edges between their bom-refs.
export function assertSbomShape(text, version) {
  const bom = JSON.parse(text);
  assert.equal(bom.bomFormat, "CycloneDX");
  const subject = bom.metadata?.component;
  assert.equal(subject?.name, "onepipeline");
  assert.equal(subject.version, version);
  assert.equal(typeof subject["bom-ref"], "string");
  const lock = lockVersions();
  const refs = new Set([subject["bom-ref"]]);
  assert.ok(Array.isArray(bom.components) && bom.components.length > 0, "components[] is listed");
  for (const component of bom.components) {
    for (const field of ["name", "version", "bom-ref"]) {
      assert.equal(typeof component[field], "string", `${component.name} has a ${field}`);
    }
    assert.ok(
      lock.get(component.name)?.includes(component.version),
      `${component.name}@${component.version} is a version Cargo.lock resolves`,
    );
    refs.add(component["bom-ref"]);
  }
  for (const engine of ENGINES) {
    const listed = bom.components.filter((c) => c.name === engine).map((c) => c.version);
    assert.deepEqual(listed, lock.get(engine), `${engine} is listed once, at the lock's version`);
  }
  assert.ok(Array.isArray(bom.dependencies) && bom.dependencies.length > 0, "dependencies[]");
  const root = bom.dependencies.find((edge) => edge.ref === subject["bom-ref"]);
  for (const engine of ["onevcs", "oneagentgraph", "onejudge", "oneharness-core"]) {
    const ref = bom.components.find((c) => c.name === engine)["bom-ref"];
    assert.ok(root?.dependsOn.includes(ref), `onepipeline depends on ${engine}`);
  }
  for (const edge of bom.dependencies) {
    assert.ok(refs.has(edge.ref), `the edge from ${edge.ref} names a component`);
    for (const ref of edge.dependsOn ?? []) assert.ok(refs.has(ref), `${ref} names a component`);
  }
}
