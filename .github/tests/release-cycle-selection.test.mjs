import { tmpdir } from "node:os";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { delimiter, join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("../../", import.meta.url));
const fixture = join(root, ".github/tests/fixtures/release-tail-v0.76.0");

test("recorded onepipeline v0.76.0 excludes CI-only PR #983 using the provisioned CLI without onevcs on PATH", {
  timeout: 60000,
}, () => {
  const dir = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "recorded-cycle-"),
  );
  try {
    const onevcs = join(root, ".github/node_modules/.onevcs-cli/node_modules/.bin/onevcs");
    assert.ok(existsSync(onevcs), "github-workflows:bootstrap must install the real onevcs CLI");
    const budgetspec = execFileSync("bash", ["-c", "command -v onebudgetspec"], {
      encoding: "utf8",
    }).trim();
    const path = process.env.PATH.split(delimiter)
      .filter((dir) => !existsSync(join(dir, "onevcs")))
      .join(delimiter);
    const env = { ...process.env, PATH: path };
    assert.equal(spawnSync("onevcs", ["--version"], { env }).error?.code, "ENOENT");
    const version = readFileSync(join(root, ".github/onevcs-cli-version"), "utf8").trim();
    assert.equal(
      execFileSync(onevcs, ["--version"], { env, encoding: "utf8" }).trim(),
      `onevcs ${version}`,
    );
    mkdirSync(join(dir, "scripts"));
    mkdirSync(join(dir, ".github"));
    mkdirSync(join(dir, "temp"));
    for (const path of [
      "budgets.yaml",
      "scripts/actions-budget-figures.sh",
      ".github/release-published-paths",
    ])
      copyFileSync(join(root, path), join(dir, path));
    execFileSync("git", ["init", "-q", dir]);
    const common = execFileSync("git", ["rev-parse", "--git-common-dir"], {
      cwd: root,
      encoding: "utf8",
    }).trim();
    writeFileSync(
      join(dir, ".git/objects/info/alternates"),
      `${resolve(root, common, "objects")}\n`,
    );
    const gh = join(dir, "gh-replay.mjs");
    writeFileSync(
      gh,
      `#!/usr/bin/env node
import {readFileSync,writeFileSync,appendFileSync} from 'node:fs';
const request=process.argv.at(-1).replace(/^https:\\/\\/api.github.com\\//,'');
const record=JSON.parse(readFileSync(process.env.RECORDED_GITHUB)).records.find(r=>r.endpoint===request);
if(!record){writeFileSync(2,'No recorded GitHub response for '+request);process.exit(1);}
appendFileSync(process.env.REPLAY_SERVED,request+'\\n');writeFileSync(1,record.response);
`,
      { mode: 0o755 },
    );
    const served = join(dir, "served");
    const started = Math.floor(Date.now() / 1000);
    const measured = spawnSync(
      budgetspec,
      ["check", "budgets.yaml", "--id", "onepipeline-release-cycle-time", "--json"],
      {
        cwd: dir,
        encoding: "utf8",
        env: {
          ...env,
          ONEVCS: onevcs,
          GH_TOKEN: "recorded-token",
          ONEVCS_GH: gh,
          RECORDED_GITHUB: join(fixture, "github.json"),
          REPLAY_SERVED: served,
          GITHUB_REPOSITORY: "nickderobertis/onepipeline",
          GITHUB_REF_NAME: "v0.76.0",
          GITHUB_REF: "refs/tags/v0.76.0",
          GITHUB_REF_TYPE: "tag",
          RUNNER_TEMP: join(dir, "temp"),
        },
      },
    );
    const ended = Math.ceil(Date.now() / 1000);
    assert.equal(measured.status, 1, measured.stdout + measured.stderr);
    const result = JSON.parse(measured.stdout).results[0];
    const expected = JSON.parse(readFileSync(join(fixture, "expected.json")));
    const ts = (value) => Date.parse(value) / 1000;
    const maximum = (now) =>
      Math.max(
        ...expected.counted.map(
          (pr) =>
            ts(pr.last_job_completed_at) - ts(pr.first_run_created_at) + now - ts(pr.merged_at),
        ),
      );
    assert.ok(
      result.actual >= maximum(started) && result.actual <= maximum(ended),
      JSON.stringify(result),
    );
    assert.equal(result.threshold, 5400);
    assert.equal(result.verdict, "over");
    assert.deepEqual(
      expected.counted.map((pr) => pr.number).sort((a, b) => a - b),
      [982, 985, 988, 992, 994, 1000, 1003],
    );
    assert.match(result.detail, /#983: left out, changes no published path/);
    for (const pr of expected.counted)
      assert.match(
        result.detail,
        new RegExp(`#${pr.number}: checks \\d+s \\+ merge to release \\d+s`),
      );
    const selection = JSON.parse(readFileSync(join(fixture, "selection.json")));
    const excluded = selection.changes.find((pr) => pr.number === 983);
    assert.doesNotMatch(readFileSync(served, "utf8"), new RegExp(excluded.head_sha));
    console.error(
      `Recorded v0.76.0 selection at dispatch tail: ${result.actual}s; threshold ${result.threshold}s; ${result.verdict}`,
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
