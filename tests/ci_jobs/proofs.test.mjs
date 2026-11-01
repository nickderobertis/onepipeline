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
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
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

  it("captures every lane in order in the background and waits for them to succeed", (t) => {
    // The lanes' orchestration is what is under test; each lane's capture is a
    // stand-in that records it, because a real one builds release binaries in
    // a container, which `lint` runs on every pull request.
    const dir = scratchWorkspace(t);
    commit(
      dir,
      "screenshots/ci-capture.sh",
      'printf "%s\\n" "$SHOTS_OUT" >> "$CAPTURED"\necho "captured $SHOTS_OUT"\n',
    );
    const out = join(dir, "target");
    run(dir, "mkdir", ["-p", out]);
    const captured = join(out, "captured");
    const lanes = { SHOTS_LANES: "shots/current/x86_64 shots/verify/x86_64", CAPTURED: captured };
    const started = run(dir, "bash", ["screenshots/ci-capture-lanes.sh", "start", out], lanes);
    assert.equal(started.status, 0, started.log);
    const waited = run(dir, "bash", ["screenshots/ci-capture-lanes.sh", "wait", out]);
    assert.equal(waited.status, 0, waited.log);
    assert.match(waited.log, /captured shots\/current\/x86_64\ncaptured shots\/verify\/x86_64/);
    assert.equal(readFileSync(captured, "utf8"), "shots/current/x86_64\nshots/verify/x86_64\n");
  });

  it("re-clones a restored clone whose repository files are gone", (t) => {
    // main's warm run captures, so the `target/` rust-cache saves holds
    // `target/ci-capture` with its files pruned: inside that skeleton git finds
    // the checkout's own repository, whose HEAD matches, and a capture that took
    // the skeleton for the clone ran with no `rust-toolchain.toml` (run
    // 41313240282's `lint`). Docker is the one stand-in: it records whether the
    // tree it is handed holds the toolchain file, and runs nothing.
    const dir = scratchWorkspace(t);
    for (const sub of [".git/refs/heads", ".git/objects/pack", "src", "screenshots"]) {
      mkdirSync(join(dir, "target", "ci-capture", sub), { recursive: true });
    }
    const bin = join(dir, "target", "fake-bin");
    mkdirSync(bin, { recursive: true });
    const seen = join(dir, "target", "docker-saw");
    writeFileSync(
      join(bin, "docker"),
      '#!/usr/bin/env bash\nwhile [ "$#" -gt 0 ]; do\n  case "$1" in -v) src="$(printf %s "$2" | cut -d: -f1)"; [ -f "$src/rust-toolchain.toml" ] && echo "toolchain in $src" >> "$SEEN"; shift 2 ;; *) shift ;; esac\ndone\n',
    );
    chmodSync(join(bin, "docker"), 0o755);
    const { status, log } = run(dir, "bash", ["screenshots/ci-capture.sh"], {
      SHOTS_OUT: "shots/current/x86_64",
      SEEN: seen,
      PATH: `${bin}:${process.env.PATH}`,
    });
    assert.equal(status, 0, log);
    assert.equal(
      existsSync(seen) && readFileSync(seen, "utf8"),
      `toolchain in ${join(dir, "target", "ci-capture")}\n`,
    );
  });

  it("refuses a status file that holds no exit status", (t) => {
    const dir = mkdtempSync(join(tmpdir(), "ci-capture-lanes-"));
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    run(dir, "bash", ["-c", 'echo said > capture.log && echo "-1x" > capture.exit']);
    const { status, log } = run(root, "bash", ["screenshots/ci-capture-lanes.sh", "wait", dir]);
    assert.equal(status, 1, log);
    assert.match(log, /holds '-1x', not an exit status/);
  });

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
    [
      "a start whose lanes are only spaces",
      ["start"],
      { SHOTS_LANES: "   " },
      /SHOTS_LANES names no lane/,
    ],
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

  it("refuses a visual-docs workflow that does not parse", (t) => {
    const dir = scratchWorkspace(t);
    commit(dir, ".github/workflows/visual-docs.yml", "jobs: [unclosed\n");
    const { status, log } = run(dir, "bash", ["screenshots/ci-capture.sh"], {
      SHOTS_OUT: "shots/current/x86_64",
    });
    assert.equal(status, 1, log);
    assert.match(log, /visual-docs\.yml could not be read as YAML/);
    assert.match(log, /^ACTION: restore \.github\/workflows\/visual-docs\.yml/m);
  });

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

  it("refuses a doubles build it does not have", () => {
    const { status, log } = run(root, "just", ["_e2e-doubles-build", "release"]);
    assert.equal(status, 2, log);
    assert.match(log, /name the build: instrumented, plain or plain-deny-warnings, not 'release'/);
  });
});

describe("the warm build's test doubles", () => {
  // What the harness asks cargo for, read out of tests/e2e/harness.rs: every
  // `build(&[...])` selection, so the recipe the warm build runs cannot build
  // other doubles than the suite does and leave main's cache without them.
  const harness = readFileSync(join(root, "tests/e2e/harness.rs"), "utf8");
  const asked = [...harness.matchAll(/\bbuild\(&\[([^\]]*)\]\)/g)]
    .map((m) => [...m[1].matchAll(/"([^"]+)"/g)].map((arg) => arg[1]).join(" "))
    .sort();
  const justfile = readFileSync(join(root, "justfile"), "utf8");
  const recipe = justfile.split("\n_e2e-doubles-build mode:\n")[1]?.split("\n\n")[0] ?? "";

  it("are every selection the e2e harness builds, and no other", () => {
    assert.equal(asked.length, 4, harness.match(/\bbuild\(&\[[^\]]*\]\)/g)?.join("\n"));
    const built = [...recipe.matchAll(/^\s+"(--package [^"]+)"/gm)].map((m) => m[1]).sort();
    assert.deepEqual(built, asked);
    assert.match(recipe, /cargo build --offline \$selection --target-dir "\$target"/);
  });

  it("are built with the harness's own cargo arguments, into its target directory", () => {
    // The harness builds into the target directory its test binary sits in:
    // `target` for the plain build, llvm-cov's for the instrumented one, which
    // is the directory each of the recipe's two modes names.
    const fn = harness.split("\nfn build_package(")[1]?.split("\n}\n")[0] ?? "";
    assert.match(
      fn,
      /\.args\(\["build", "--offline"\]\)\s*\.args\(selection\)\s*\.arg\("--target-dir"\)/,
    );
    assert.match(fn, /let target = debug\s*\.parent\(\)/);
    assert.match(
      recipe,
      /instrumented\).*cargo llvm-cov show-env.*target="\{\{llvm-cov-target-dir\}\}"/,
    );
    assert.match(
      recipe,
      /plain\) unset RUSTFLAGS; target="\{\{justfile_directory\(\)\}\}\/target"/,
    );
    assert.match(
      recipe,
      /plain-deny-warnings\) export RUSTFLAGS="-D warnings"; target="\{\{justfile_directory\(\)\}\}\/target"/,
    );
  });

  // The doubles inherit RUSTFLAGS from the test that asks for them, and cargo
  // fingerprints RUSTFLAGS, so each uninstrumented journey's own flags name the
  // build it needs warm: release-compat's denied warnings, smoke's none (in
  // run 41298787095 each compiled its doubles cold: 111.8 s and 116.0 s).
  const project = (path) => JSON.parse(readFileSync(join(root, path), "utf8"));
  const bodyOf = (name) => {
    const body = justfile.split(new RegExp(`\n${name}(?: [^:\n]*)?:\n`))[1]?.split("\n\n")[0];
    assert.ok(body, `the justfile has no ${name} recipe`);
    return body;
  };
  const modeOf = (path, target) => {
    const name = /^just (_[\w-]+)$/.exec(project(path).targets[target]?.command ?? "")?.[1];
    assert.ok(name, `${path}'s ${target} runs no recipe this can name`);
    const body = bodyOf(name);
    assert.doesNotMatch(body, /llvm-cov/, `${name} is not an uninstrumented journey`);
    return /RUSTFLAGS="-D warnings"/.test(body) ? "plain-deny-warnings" : "plain";
  };
  const doublesOf = (family) => {
    const warm = readFileSync(join(root, "scripts/warm-rust-cache.sh"), "utf8");
    const arm = warm.split(`\n  ${family})\n`)[1]?.split(";;")[0] ?? "";
    const lines = arm
      .trim()
      .split("\n")
      .map((line) => line.trim());
    const first = lines.findIndex((line) => line.startsWith("just _e2e-doubles-build "));
    assert.ok(first >= 0, `the ${family} warm build builds no doubles`);
    return { lines, tail: lines.slice(first) };
  };

  it("are built, on Linux, in each variant test-linux's journeys build them in", () => {
    const owed = new Set([
      "instrumented",
      modeOf("tests/release_channel/project.json", "release-compat"),
      modeOf("tests/smoke/project.json", "smoke"),
    ]);
    assert.deepEqual([...owed].sort(), ["instrumented", "plain", "plain-deny-warnings"]);
    const built = doublesOf("linux").tail.map((line) => line.split(" ").at(-1));
    assert.deepEqual([...built].sort(), [...owed].sort());
  });

  it("are built, for Windows and macOS, in the variant their test-quick tiers build them in", () => {
    assert.doesNotMatch(bodyOf("_tier-test-quick"), /RUSTFLAGS|llvm-cov/);
    assert.deepEqual(doublesOf("cross").tail, ["just _e2e-doubles-build plain"]);
  });

  it("are built by both families of the warm build, last", () => {
    // Last, because a test build after them leaves them stale: built ahead of
    // the instrumented test build, testfakes and onevcs compiled again inside
    // the first journey (90 s and 31 s on a 20-core host); built after it, each
    // journey's build of them took under a second.
    for (const family of ["linux", "cross"]) {
      const { lines, tail } = doublesOf(family);
      assert.ok(
        tail.every((line) => line.startsWith("just _e2e-doubles-build ")),
        `the ${family} warm build runs another build after its doubles: ${tail.join("; ")}`,
      );
      assert.ok(tail.length < lines.length, family);
    }
  });
});
