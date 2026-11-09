import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = fileURLToPath(new URL("../../", import.meta.url));
const command = (exe, args) => execFileSync(exe, args, { cwd: root, encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
function list(filter) {
  const doc = JSON.parse(command("cargo", ["nextest", "list", "--locked", "--message-format", "json", "-E", filter]));
  return Object.entries(doc["rust-suites"]).flatMap(([binary, suite]) => Object.entries(suite.testcases).filter(([, t]) => t["filter-match"].status === "matches").map(([name]) => `${binary}::${name}`)).sort();
}
// A budget tier added after the pinned justfile, excluded from `rest-tier` by name and
// run by its own project, so the pinned filters would otherwise still claim it.
const ownTierSincePin = "binary(status_read_workload)";
const withoutOwnTier = (filter) => `(${filter}) and not ${ownTierSincePin}`;
// A binary given its own tier after the base justfile was written leaves `rest` with it. Its `harness::` tests are the
// e2e binary's own module compiled in again, so its tier leaves them to `e2e`'s; every other test it lists stays selected.
const splitOut = [["onepipeline::narrow_write", "narrow-write"]];
const inSplit = (name) => splitOut.some(([binary]) => name.startsWith(`${binary}::`));
const asE2e = (name) => name.replace(/^onepipeline::[a-z_]+::/, "onepipeline::e2e::");
test("real nextest listings preserve every test when version readers move to their targets", { timeout: 600000 }, () => {
  const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "test-selection-"));
  try {
    const base = join(dir, "justfile");
    writeFileSync(base, command("git", ["show", "96c876ab:justfile"]));
    for (const [original, moved] of [["rest", "package"], ["e2e", "e2e"], ["contract", "contract"]]) {
      const beforeFilter = command("just", ["--justfile", base, "--evaluate", `${original}-tier`]).trim();
      const afterFilter = command("just", ["--evaluate", `${original}-tier`]).trim();
      const movedFilter = command("just", ["--evaluate", `version-${moved}-tier`]).trim();
      const before = list(withoutOwnTier(beforeFilter)).filter((name) => !inSplit(name));
      const after = list(afterFilter);
      const version = list(movedFilter);
      assert.equal(version.length, 1, `${moved}'s version target must contain exactly its one moved test`);
      assert.deepEqual([...after, ...version].sort(), before, `${original}'s selected tests changed`);
      assert.equal(after.some((name) => version.includes(name)), false, "a moved test ran in both targets");
    }
    const e2e = list(command("just", ["--evaluate", "e2e-tier"]).trim());
    for (const [binary, tier] of splitOut) {
      const own = list(command("just", ["--evaluate", `${tier}-tier`]).trim());
      assert.notEqual(own.length, 0, `${tier}'s tier selects none of ${binary}'s tests`);
      for (const name of list(`binary_id(${binary})`).filter((test) => !own.includes(test))) {
        assert.match(name, /^onepipeline::[a-z_]+::harness::/, `${name} is selected by no tier`);
        assert.ok(e2e.includes(asE2e(name)), `${name}'s module is not run by e2e's tier`);
      }
    }
    const before = command("just", ["--justfile", base, "--evaluate", "offline-tiers"]).trim();
    const after = command("just", ["--evaluate", "offline-tiers"]).trim();
    const leftToE2e = (name) => inSplit(name) && /::harness::/.test(name) && e2e.includes(asE2e(name));
    assert.deepEqual(list(after), list(withoutOwnTier(before)).filter((name) => !leftToE2e(name)));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
