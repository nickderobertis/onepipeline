import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { root, run } from "./support.mjs";

test("both warm families build the staged launcher before their final doubles and stop on failure", (t) => {
  const bin = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "staged-warm-"),
  );
  t.after(() => rmSync(bin, { recursive: true, force: true }));
  writeFileSync(
    join(bin, "just"),
    '#!/bin/sh\nprintf "%s\\n" "$*" >> "$JUST_ASKED"\nif [ "$1" = _npm-launcher-build ] && [ "$FAIL_STAGED" = yes ]; then echo "staged build refused" >&2; exit 17; fi\n',
  );
  chmodSync(join(bin, "just"), 0o755);
  for (const family of ["linux", "cross"]) {
    for (const fail of [false, true]) {
      const asked = join(bin, `${family}-${fail}.log`);
      const { status, log } = run(root, "bash", ["scripts/warm-rust-cache.sh", family], {
        PATH: `${bin}:${process.env.PATH}`,
        JUST_ASKED: asked,
        FAIL_STAGED: fail ? "yes" : "no",
      });
      const calls = readFileSync(asked, "utf8").trim().split("\n");
      const staged = calls.indexOf("_npm-launcher-build");
      assert.ok(staged >= 0, `${family} never warms the staged build: ${calls}`);
      if (fail) {
        assert.equal(status, 17, log);
        assert.match(log, /staged build refused/);
        assert.ok(
          calls.every((call) => !call.startsWith("_e2e-doubles-build ")),
          calls,
        );
      } else {
        assert.equal(status, 0, log);
        assert.ok(
          calls.slice(staged + 1).every((call) => call.startsWith("_e2e-doubles-build ")),
          calls,
        );
        assert.ok(staged < calls.length - 1, calls);
      }
    }
  }
});
