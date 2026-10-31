// The refusals of the scripts ci.yml's jobs run around their recipes: the
// install proofs (`scripts/install-proof.sh`), the visual-docs capture
// (`screenshots/ci-capture.sh`) and the main-branch warm build
// (`scripts/warm-rust-cache.sh`), each driven as its job runs it, over a scratch
// copy of this workspace where a case needs to break what the script reads.
//
// Each refusal is asked before the script reaches what it would build or fetch
// — cargo, Docker, the network — so these cases run nothing but the script. What
// a proof does once it gets past them is what the jobs themselves run, on every
// pull request that reaches the crate.
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";
import { commit, root, run, scratchWorkspace } from "./support.mjs";

describe("the install proofs' refusals", () => {
  it("refuses a proof it does not have", () => {
    const { status, log } = run(root, "bash", ["scripts/install-proof.sh", "registry"]);
    assert.equal(status, 2, log);
    assert.match(log, /name the proof: path, documented or wheel/);
    assert.match(log, /^ACTION: /m);
  });

  it("refuses anything beside the one proof it is asked for", () => {
    const { status, log } = run(root, "bash", ["scripts/install-proof.sh", "path", "--release"]);
    assert.equal(status, 2, log);
    assert.match(log, /takes exactly one argument, the proof, not 2/);
  });

  it("refuses a README that no longer gives the documented one-liner", (t) => {
    const dir = scratchWorkspace(t);
    const readme = readFileSync(join(dir, "README.md"), "utf8");
    commit(
      dir,
      "README.md",
      readme.replace(/^cargo install --git .*$/m, "cargo install onepipeline"),
    );
    const { status, log } = run(dir, "bash", ["scripts/install-proof.sh", "documented"]);
    assert.equal(status, 1, log);
    assert.match(log, /no longer gives a 'cargo install --git' one-liner/);
    assert.doesNotMatch(log, /running README\.md's command/);
  });

  it("refuses a one-liner carrying characters it will not run", (t) => {
    const dir = scratchWorkspace(t);
    const readme = readFileSync(join(dir, "README.md"), "utf8");
    commit(dir, "README.md", readme.replace(/^(cargo install --git .*)$/m, "$1; touch hijacked"));
    const { status, log } = run(dir, "bash", ["scripts/install-proof.sh", "documented"]);
    assert.equal(status, 1, log);
    assert.match(log, /carries characters this will not run/);
    assert.doesNotMatch(log, /running README\.md's command/);
  });
});

describe("the visual-docs capture's refusals", () => {
  it("refuses to capture without being told where", () => {
    const { status, log } = run(root, "bash", ["screenshots/ci-capture.sh"], { SHOTS_OUT: "" });
    assert.equal(status, 2, log);
    assert.match(log, /SHOTS_OUT is not set/);
  });

  for (const out of ["../escape", "/tmp/shots", "shots/../escape", "shots", "current/x86_64"]) {
    it(`refuses to write its capture to ${out}`, () => {
      const { status, log } = run(root, "bash", ["screenshots/ci-capture.sh"], { SHOTS_OUT: out });
      assert.equal(status, 2, log);
      assert.match(log, /is not a directory under shots\//);
    });
  }

  it("fails the wait for a capture it started in the background when a lane fails", (t) => {
    const dir = mkdtempSync(join(tmpdir(), "ci-capture-lanes-"));
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    const started = run(root, "bash", ["screenshots/ci-capture-lanes.sh", "start", dir], {
      SHOTS_LANES: "../escape shots/verify/x86_64",
    });
    assert.equal(started.status, 0, started.log);
    assert.match(started.log, /capturing \.\.\/escape shots\/verify\/x86_64 in the background/);
    const waited = run(root, "bash", ["screenshots/ci-capture-lanes.sh", "wait", dir]);
    assert.equal(waited.status, 2, waited.log);
    assert.match(waited.log, /SHOTS_OUT='\.\.\/escape' is not a directory under shots\//);
    assert.match(waited.log, /the capture failed \(exit 2\)/);
    assert.equal(readFileSync(join(dir, "capture.exit"), "utf8").trim(), "2");
  });

  for (const [what, args, env, refusal] of [
    ["a wait for a capture never started", ["wait"], {}, /no capture was started/],
    ["a start that names no lane", ["start"], { SHOTS_LANES: "" }, /SHOTS_LANES names no lane/],
    ["a mode it does not have", ["resume"], {}, /is not a mode/],
  ]) {
    it(`refuses ${what}`, (t) => {
      const dir = mkdtempSync(join(tmpdir(), "ci-capture-lanes-"));
      t.after(() => rmSync(dir, { recursive: true, force: true }));
      const { status, log } = run(
        root,
        "bash",
        ["screenshots/ci-capture-lanes.sh", ...args, dir],
        env,
      );
      assert.equal(status, 2, log);
      assert.match(log, refusal);
    });
  }

  it("refuses a visual-docs workflow that gives no capture-command", (t) => {
    const dir = scratchWorkspace(t);
    const workflow = ".github/workflows/visual-docs.yml";
    const text = readFileSync(join(dir, workflow), "utf8");
    commit(dir, workflow, text.replace("capture-command: |", "capture-commands: |"));
    const { status, log } = run(dir, "bash", ["screenshots/ci-capture.sh"], {
      SHOTS_OUT: "shots/current/x86_64",
    });
    assert.equal(status, 1, log);
    assert.match(log, /gives the visual-docs job no capture-command/);
    assert.match(log, /^ACTION: restore jobs\.visual-docs\.with\.capture-command/m);
    assert.doesNotMatch(log, /running visual-docs\.yml's capture-command/);
  });
});

describe("the warm build's refusals", () => {
  it("refuses a job family it does not build for", () => {
    const { status, log } = run(root, "bash", ["scripts/warm-rust-cache.sh", "release"]);
    assert.equal(status, 2, log);
    assert.match(log, /name the job family: linux or cross/);
  });

  it("refuses anything beside the one job family it is asked for", () => {
    const { status, log } = run(root, "bash", ["scripts/warm-rust-cache.sh", "linux", "cross"]);
    assert.equal(status, 2, log);
    assert.match(log, /takes exactly one argument, the job family, not 2/);
  });
});
