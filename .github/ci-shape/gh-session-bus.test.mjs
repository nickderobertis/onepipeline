import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { command, commandEnvironment, root } from "./support/cache-workspace.mjs";

// The cache journeys run the real gh through scripts/nx-remote-cache.mjs at a loopback
// GitHub, with a recording dbus-launch here in place of the host's, so nothing starts.
// gh looks for its token before it connects, so a closed port is all this needs, and
// its refused connection is what shows the call reached the real gh.
const CLOSED_PORT = "http://127.0.0.1:9";
const REACHED_GH = /Get "http:\/\/127\.0\.0\.1:9\/.*connection refused/;
const hasGh = () => !spawnSync("gh", ["--version"]).error;
const unavailable = process.platform !== "linux" || !hasGh();

test("the cache journeys' command environment keeps the real gh from asking for a session bus", {
  skip: unavailable && "no session bus can start here: this guard needs the real gh on Linux",
}, async (t) => {
  const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "gh-session-bus-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const record = join(dir, "bus-calls");
  writeFileSync(join(dir, "dbus-launch"), "#!/bin/sh\necho called >> \"$BUS_RECORD\"\nexit 1\n");
  chmodSync(join(dir, "dbus-launch"), 0o755);
  // One gh call, for the commit's pull requests; promotion skips once it fails.
  const promotion = [join(root, "scripts/nx-remote-cache.mjs"), "promote", "--sha", "0".repeat(40), "--repo", "example/project", "--workflow", "release-plz.yml"];
  const given = {
    PATH: `${dir}:${process.env.PATH}`,
    BUS_RECORD: record,
    GH_TOKEN: "fixture-token",
    GITHUB_API_URL: CLOSED_PORT,
    GITHUB_EVENT_NAME: "push",
    GITHUB_REF: "refs/heads/main",
    RUNNER_TEMP: dir,
  };
  const calls = () => (existsSync(record) ? readFileSync(record, "utf8").split("\n").filter(Boolean).length : 0);

  const without = commandEnvironment(given);
  delete without.DBUS_SESSION_BUS_ADDRESS;
  const control = spawnSync(process.execPath, promotion, { cwd: dir, env: without, encoding: "utf8" });
  assert.match(control.stdout + control.stderr, REACHED_GH, "promotion never reached the real gh, so this guard would prove nothing");
  if (calls() === 0) {
    t.skip("this gh asks for no session bus even without the variable, so none can leak");
    return;
  }

  rmSync(record);
  const guarded = await command(dir, process.execPath, promotion, given);
  assert.equal(guarded.code, 0, guarded.stdout + guarded.stderr);
  assert.match(guarded.stdout + guarded.stderr, REACHED_GH, "the guarded call never reached the real gh");
  assert.equal(calls(), 0, "the command environment lets gh ask for a session bus");
});
