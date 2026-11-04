// Upstream-owned copies from dero-skills 63e071b72420bd6737226a50cb8a7a7760cc73a4.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const upstream = {
  "scripts/nx-remote-cache.mjs": "4445c0d8d5f6045bb2b14b19cd03f9600b6b98f88dc689421d98d3ef996664b6",
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
