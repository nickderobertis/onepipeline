import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { test } from "node:test";
import { hostTarget, REPO_ROOT, STAGED_TARGET_DIR } from "./install-support.mjs";
import { spawnable } from "./spawn-support.mjs";

test("the staged warm recipe builds the launcher's target and flags and propagates a failed build", (t) => {
  const launcher = readFileSync(join(REPO_ROOT, "npm/test/launcher-process.test.mjs"), "utf8");
  const call = launcher.match(/run\("cargo",\s*(\[[^\]]+\])\);/);
  assert.ok(call, "the launcher's Cargo build must have readable arguments");
  assert.match(launcher, /const target = hostTarget\(\);/);
  const expectedArgs = JSON.parse(
    call[1].replace(/\btarget(?=\s*\])/, JSON.stringify(hostTarget())),
  );
  const bin = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "staged-cargo-"),
  );
  t.after(() => rmSync(bin, { recursive: true, force: true }));
  const asked = join(bin, "asked");
  writeFileSync(
    join(bin, "cargo"),
    '#!/bin/sh\nprintf "%s|%s|%s|%s\\n" "$PWD" "$RUSTFLAGS" "$CARGO_TARGET_DIR" "$*" >> "$CARGO_ASKED"\nif [ "$FAIL_STAGED" = yes ]; then echo "staged compile refused" >&2; exit 17; fi\n',
  );
  chmodSync(join(bin, "cargo"), 0o755);
  for (const fail of [false, true]) {
    const result = spawnSync(
      ...spawnable("just", ["_npm-launcher-build"], {
        cwd: REPO_ROOT,
        encoding: "utf8",
        env: {
          ...process.env,
          PATH: `${bin}${delimiter}${process.env.PATH}`,
          CARGO_ASKED: asked,
          CARGO_TARGET_DIR: join(bin, "wrong-tree"),
          RUSTFLAGS: "wrong flags",
          FAIL_STAGED: fail ? "yes" : "no",
        },
      }),
    );
    const log = `${result.stdout}\n${result.stderr}`;
    if (fail) {
      assert.notEqual(result.status, 0, log);
      assert.match(log, /staged compile refused/);
    } else {
      assert.equal(result.status, 0, log);
    }
    const calls = readFileSync(asked, "utf8").trim().split("\n");
    assert.equal(calls.length, fail ? 2 : 1, calls);
    const [cwd, flags, targetDir, args] = calls.at(-1).split("|");
    assert.equal(cwd, REPO_ROOT);
    assert.equal(flags, "-D warnings");
    assert.equal(targetDir, STAGED_TARGET_DIR);
    assert.equal(args, expectedArgs.join(" "));
  }
});
