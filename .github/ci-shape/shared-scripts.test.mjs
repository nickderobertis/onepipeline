// Upstream-owned copies: the cache reader at dero-skills
// 27a6b4950bd7582363eebdfbb8b93a0daf85fc21; the cache prune at
// 270bea5d287f6fe6a844fd6b882922eacadf6ecf; version-free inputs at
// 63e071b72420bd6737226a50cb8a7a7760cc73a4; figures, version 13, at
// afc6829981dc31ad4221fc7cf706916d078ada35.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const upstream = {
  "scripts/actions-budget-figures.sh": "f4dbf47e81ce3baf9acf92d4e92f284614bf9c6a3774a76f18ed89893ce7d3b0",
  "scripts/actions-cache-prune.mjs": "4c7bfd986ddb7e59b395a00eb3bb74892b44b76234d0c2a61d63ddf7a49b37a7",
  "scripts/nx-remote-cache.mjs": "21a6ba436f59d7bd21d1659bf928b0123a6ac0003d5993747e9f216b020d3526",
  "scripts/nx-version-free-inputs.mjs": "fdd4f59fc98a0073dbefdbaa0f08f4b58432bbf8e55d315865f951e0151b19b2",
};
for (const [path, digest] of Object.entries(upstream)) {
  test(`${path} is byte-identical to the pinned upstream commit`, () => {
    const bytes = readFileSync(new URL(`../../${path}`, import.meta.url));
    assert.equal(createHash("sha256").update(bytes).digest("hex"), digest);
    const lines = bytes.toString().split("\n");
    const header = lines.findIndex((line) => /sha256: [a-f0-9]{64}$/.test(line));
    assert.ok(header > 0);
    assert.equal(
      createHash("sha256").update(lines.slice(header + 1).join("\n")).digest("hex"),
      lines[header].split(": ").at(-1),
    );
  });
}

test("formatter and linter exclusions add only the digest-guarded upstream copies", async () => {
  const { execFileSync } = await import("node:child_process");
  const { parse } = await import("yaml");
  const base = (path) => execFileSync("git", ["show", `96c876ab:${path}`], { encoding: "utf8" });
  const current = (path) => readFileSync(new URL(`../../${path}`, import.meta.url), "utf8");
  const exact = ["scripts/actions-cache-prune.mjs", "scripts/nx-remote-cache.mjs", "scripts/nx-version-free-inputs.mjs"];
  const before = parse(base("llmlint.yml"));
  const after = parse(current("llmlint.yml"));
  assert.deepEqual(after.files.exclude.filter((p) => !before.files.exclude.includes(p)), exact);
  after.files.exclude = after.files.exclude.filter((p) => !exact.includes(p));
  assert.deepEqual(after, before, "no other lint rule or setting may be loosened");
  const biome = JSON.parse(current("biome.jsonc").split("\n").filter((line) => !line.trim().startsWith("//")).join("\n"));
  assert.deepEqual(biome.files.includes.filter((p) => p.startsWith("!!")), exact.map((p) => `!!${p}`));
  biome.files.includes = biome.files.includes.filter((p) => !p.startsWith("!!"));
  assert.deepEqual(biome, JSON.parse(base("biome.json")));
  assert.match(current("llmlint.yml"), /Upstream-owned copies/);
  assert.match(current("biome.jsonc"), /Upstream-owned copies/);
});
