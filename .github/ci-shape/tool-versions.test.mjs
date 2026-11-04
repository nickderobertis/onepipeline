import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { parse } from "yaml";

const root = fileURLToPath(new URL("../../", import.meta.url));
const file = ".github/ci-tool-versions.json";
const reader = ".github/read-ci-tool-versions.mjs";
const ci = () => parse(readFileSync(join(root, ".github/workflows/ci.yml"), "utf8"));
const pins = () => JSON.parse(readFileSync(join(root, file), "utf8"));
function problems(workflow) {
  const found = [];
  for (const [name, job] of Object.entries(workflow.jobs)) {
    const read = job.steps.findIndex((step) => step.id === "ci-tools");
    if (read < 0 || job.steps[read].run !== `node ${reader} >> "$GITHUB_OUTPUT"`) found.push(`${name} does not read tracked tool versions`);
    for (const action of ["actions/setup-node@", "actions/setup-python@", "astral-sh/setup-uv@", "taiki-e/install-action@"]) {
      if (job.steps.filter((step) => step.uses?.startsWith(action)).length !== 1) found.push(`${name} is missing its single pinned ${action} install`);
    }
    for (const [index, step] of job.steps.entries()) {
      let key;
      if (step.uses?.startsWith("actions/setup-node@")) key = "node-version";
      else if (step.uses?.startsWith("actions/setup-python@")) key = "python-version";
      else if (step.uses?.startsWith("astral-sh/setup-uv@")) key = "version";
      else if (step.uses?.startsWith("taiki-e/install-action@")) key = "tool";
      if (!key) continue;
      const output = key === "node-version" ? "node" : key === "python-version" ? "python" : key === "version" ? "uv" : name === "lint" ? "lint_tools" : name === "test-linux" ? "linux_tools" : "quick_tools";
      if (index < read || step.with?.[key] !== ("$" + "{{ steps.ci-tools.outputs." + output + " }}")) found.push(`${name} installs ${step.uses} without the tracked version`);
    }
  }
  return found;
}

test("every CI tool install reads its exact tracked version", () => {
  assert.equal(pins().nextest, "0.9.150");
  assert.deepEqual(problems(ci()), []);
  assert.equal(ci().jobs.lint.steps.find((step) => step.name === "Install llmlint toolchain").run, "just setup-llmlint-ci");
  const steps = ci().jobs.lint.steps;
  const fingerprint = steps.findIndex((step) => step.name === "Verify the judged lint fingerprint");
  assert.match(steps[fingerprint].run, /^llmlint fingerprint\n.*fingerprint exited 0/);
  assert.ok(fingerprint > steps.findIndex((step) => step.name === "Install llmlint toolchain"));
  assert.ok(fingerprint < steps.findIndex((step) => step.name === "Lint changed files"));
  const broken = ci();
  broken.jobs["test-linux"].steps.find((step) => step.uses?.startsWith("taiki-e/install-action@")).with.tool = "just,nextest,cargo-llvm-cov";
  assert.match(problems(broken).join("\n"), /test-linux installs.*without the tracked version/);
  const unwired = ci();
  unwired.jobs.lint.steps.find((step) => step.id === "ci-tools").run = `node ${reader}`;
  assert.match(problems(unwired).join("\n"), /lint does not read tracked tool versions/);
});

test("the real tool-version reader rejects invalid pins and recovers", () => {
  const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "ci-tool-versions-"));
  try {
    mkdirSync(join(dir, ".github"));
    copyFileSync(join(root, reader), join(dir, reader));
    const invoke = (args = []) => spawnSync(process.execPath, [reader, ...args], { cwd: dir, encoding: "utf8" });
    for (const data of [undefined, "{", null, [], {}, { ...pins(), uv: "1.2.3\nbad=value" }, { ...pins(), nextest: "latest" }, { ...pins(), node: "22" }, { ...pins(), unexpected: "1.2.3" }]) {
      if (data === undefined) rmSync(join(dir, file), { force: true });
      else writeFileSync(join(dir, file), typeof data === "string" ? data : JSON.stringify(data));
      const failed = invoke();
      assert.equal(failed.status, 2, failed.stderr);
      assert.match(failed.stderr, /ACTION: restore exact tool versions/);
    }
    writeFileSync(join(dir, file), JSON.stringify(pins()));
    const valid = invoke();
    assert.equal(valid.status, 0, valid.stderr);
    const output = Object.fromEntries(valid.stdout.trim().split("\n").map((line) => line.split("=")));
    assert.equal(invoke(["unexpected"]).status, 2);
    for (const name of ["node", "python", "uv", "llmlint", "oneharness"]) assert.equal(output[name], pins()[name]);
    assert.equal(output.linux_tools, `just@${pins().just},nextest@0.9.150,cargo-llvm-cov@${pins()["cargo-llvm-cov"]}`);
    assert.equal(output.quick_tools, `just@${pins().just},nextest@0.9.150`);
    assert.equal(output.lint_tools, `just@${pins().just},cargo-deny@${pins()["cargo-deny"]},cargo-machete@${pins()["cargo-machete"]}`);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});


test("the CI lint recipe installs exact lint and harness tools at the UV boundary", () => {
  const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "ci-lint-install-"));
  try {
    mkdirSync(join(dir, ".github"));
    mkdirSync(join(dir, "bin"));
    for (const path of [file, reader, ".github/setup-llmlint.sh", "justfile", "Cargo.toml"]) copyFileSync(join(root, path), join(dir, path));
    const record = join(dir, "uv.jsonl"), bin = join(dir, "installed-bin"), githubPath = join(dir, "github-path");
    writeFileSync(join(dir, "bin/uv"), `#!/usr/bin/env node
const fs = require("node:fs");
fs.appendFileSync(process.env.UV_RECORD, JSON.stringify(process.argv.slice(2))+"\\n");
if (process.argv[2] === "tool" && process.argv[3] === "install") {
  if (process.env.UV_FAIL === "true") { console.error("fixture UV refused the install"); process.exit(42); }
} else if (process.argv.slice(2).join(" ") === "tool dir --bin") console.log(process.env.UV_BIN);
else process.exit(2);
`);
    chmodSync(join(dir, "bin/uv"), 0o755);
    const env = { ...process.env, PATH: `${join(dir, "bin")}:${process.env.PATH}`, UV_RECORD: record, UV_BIN: bin, GITHUB_PATH: githubPath, GITHUB_ACTIONS: "true" };
    const invoke = (extra) => spawnSync("just", ["setup-llmlint"], { cwd: dir, env: { ...env, ...extra }, encoding: "utf8" });
    const missingContext = invoke({ GITHUB_PATH: "" });
    assert.notEqual(missingContext.status, 0);
    assert.match(missingContext.stderr, /requires GITHUB_PATH/);
    const failed = invoke({ UV_FAIL: "true" });
    assert.notEqual(failed.status, 0);
    assert.match(failed.stderr, /fixture UV refused the install/);
    const recovered = invoke({ UV_FAIL: "false" });
    assert.equal(recovered.status, 0, recovered.stderr);
    const calls = readFileSync(record, "utf8").trim().split("\n").map((line) => JSON.parse(line));
    assert.deepEqual(calls[0], ["tool", "install", "--upgrade", `llmlint-cli==${pins().llmlint}`, "--with", `oneharness-cli==${pins().oneharness}`]);
    assert.deepEqual(calls[1], calls[0]);
    assert.deepEqual(calls[2], ["tool", "dir", "--bin"]);
    assert.equal(readFileSync(githubPath, "utf8"), `${bin}\n`);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
