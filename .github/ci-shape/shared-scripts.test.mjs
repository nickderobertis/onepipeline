// Upstream-owned copies: cache reader at dero-skills 25025c5c7a26a438192299ce32e4dcdb24eb0d78;
// version-free inputs and figures at 63e071b72420bd6737226a50cb8a7a7760cc73a4.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const upstream = {
  "scripts/actions-budget-figures.sh": "a3606ff3d00be139d52a7cbb2905f45e2ade99383417be31cc74cc7ce6c39eba",
  "scripts/nx-remote-cache.mjs": "3c9fb8af9550d6497f6717ab22e14857c139ef8988f9896ebcf3a4bbde30bdaf",
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
  const exact = ["scripts/nx-remote-cache.mjs", "scripts/nx-version-free-inputs.mjs"];
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
