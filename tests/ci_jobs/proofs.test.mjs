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
// The capture's clone of HEAD is the one thing past them proved here, against a
// stand-in `docker` that only records how it was run.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
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
import { commit, git, root, run, scratchWorkspace } from "./support.mjs";

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

// Where each install builds, with `cargo` the one stand-in: it records the target
// directory it was handed and whether that directory existed, and "installs" a
// `onepipeline` that cannot start, so each proof runs on through its smoke, which fails,
// after the build this asks about and before anything it would time.
describe("where the install proofs build", () => {
  const CARGO = `#!/usr/bin/env bash
printf '%s\\n' "\${CARGO_TARGET_DIR:-unset}" >> "$CARGO_RECORD"
if [ -d "\${CARGO_TARGET_DIR:-}" ]; then echo exists >> "$CARGO_RECORD"; else echo absent >> "$CARGO_RECORD"; fi
printf '%s\\n' "$*" >> "$CARGO_RECORD"
into="\${CARGO_INSTALL_ROOT:-}"
while [ "$#" -gt 0 ]; do [ "$1" = --root ] && into="$2"; shift; done
mkdir -p "$into/bin"
printf '#!/usr/bin/env bash\\necho "the recording cargo built nothing" >&2\\nexit 1\\n' > "$into/bin/onepipeline"
chmod +x "$into/bin/onepipeline"
printf '"onepipeline 0.0.0 (git+https://example.invalid/onepipeline#%s)" = ["onepipeline"]\\n' "$CARGO_INSTALLS" > "$into/.crates.toml"
`;

  function proof(t, which, env = {}) {
    const dir = scratchWorkspace(t);
    // The documented install reads its smoke at the revision it installed from origin.
    git(dir, "remote", "add", "origin", dir);
    const scratch = mkdtempSync(join(tmpdir(), "install-proof-cargo-"));
    t.after(() => rmSync(scratch, { recursive: true, force: true }));
    const bin = join(scratch, "bin");
    const temp = join(scratch, "runner-temp");
    mkdirSync(bin);
    mkdirSync(temp);
    writeFileSync(join(bin, "cargo"), CARGO, { mode: 0o755 });
    const record = join(scratch, "cargo.record");
    const result = run(dir, "bash", ["scripts/install-proof.sh", which], {
      PATH: `${bin}:${process.env.PATH}`,
      CARGO_RECORD: record,
      CARGO_INSTALLS: git(dir, "rev-parse", "HEAD"),
      RUNNER_TEMP: temp,
      ...env,
    });
    const [targetDir, existed, argv] = existsSync(record)
      ? readFileSync(record, "utf8").trimEnd().split("\n")
      : [];
    return { dir, temp, targetDir, existed, argv, ...result };
  }

  it("builds the documented install in a directory of its own, outside target/, and removes it", (t) => {
    const { dir, temp, targetDir, existed, argv, status, log } = proof(t, "documented");
    const readme = readFileSync(join(dir, "README.md"), "utf8");
    assert.equal(`cargo ${argv}`, /^cargo install --git .*$/m.exec(readme)[0], log);
    assert.ok(targetDir.startsWith(`${temp}/install-documented.`), `built in ${targetDir}`);
    assert.ok(!targetDir.startsWith(join(dir, "target")), `built in the job's ${targetDir}`);
    assert.equal(existed, "exists", log);
    assert.ok(!existsSync(targetDir), `${targetDir} outlived the proof`);
    // It went on to smoke what it installed, with the installed revision's assertions.
    assert.equal(status, 1, log);
    assert.match(log, /cargo install --git on /);
    assert.match(log, /'--version' did not run/);
  });

  it("builds this revision's install in the job's one target directory", (t) => {
    const { dir, targetDir, argv, status, log } = proof(t, "path");
    assert.equal(targetDir, join(dir, "target"), log);
    assert.equal(
      argv,
      `install --path . --locked --root ${join(dir, "target/install-proof/path")}`,
    );
    assert.equal(status, 1, log);
    assert.match(log, /cargo install on /);
    assert.match(log, /'--version' did not run/);
  });

  it("refuses to build the documented install where it cannot make its directory", (t) => {
    const { temp, targetDir, status, log } = proof(t, "documented", {
      RUNNER_TEMP: "/nonexistent/install-proof-temp",
    });
    assert.equal(status, 1, log);
    assert.match(
      log,
      /could not create the documented install's build directory under \/nonexistent\/install-proof-temp/,
    );
    assert.match(log, /^ACTION: point RUNNER_TEMP or TMPDIR at a writable directory/m);
    assert.equal(targetDir, undefined, "cargo ran without a build directory");
    assert.ok(existsSync(temp));
  });
});

// The order test-linux asks for when it runs both installs (scripts/ci-job.sh): the
// two targets run through the real Nx scheduler side by side, and the documented
// install's build starts only after install-smoke's host-view-time reads end. The
// stand-ins each log when they ran: `cargo` (the path build takes three seconds, so
// an unordered documented build overlaps it), the `host-view-budget` recipe behind a
// `just` that passes every other recipe through, and a smoke script committed in the
// scratch copy, which the documented install reads at the revision it installed.
describe("the order of the two installs", () => {
  const realJust = spawnSync("bash", ["-c", "command -v just"], { encoding: "utf8" }).stdout.trim();
  const CARGO = `#!/usr/bin/env bash
case " $* " in *" --path "*) kind=path ;; *) kind=documented ;; esac
echo "$(date +%s%N) cargo-$kind" >> "$ORDER_LOG"
[ "$kind" = documented ] || sleep 3
into="\${CARGO_INSTALL_ROOT:-}"
while [ "$#" -gt 0 ]; do [ "$1" = --root ] && into="$2"; shift; done
mkdir -p "$into/bin"
printf '#!/usr/bin/env bash\\n' > "$into/bin/onepipeline"
chmod +x "$into/bin/onepipeline"
printf '"onepipeline 0.0.0 (git+https://example.invalid/onepipeline#%s)" = ["onepipeline"]\\n' "$CARGO_INSTALLS" > "$into/.crates.toml"
`;
  const JUST = `#!/usr/bin/env bash
if [ "\${1:-}" = host-view-budget ]; then
  echo "$(date +%s%N) host-view-start" >> "$ORDER_LOG"
  sleep 2
  echo "$(date +%s%N) host-view-end" >> "$ORDER_LOG"
  exit 0
fi
exec '${realJust}' "$@"
`;

  // `ordered` takes the assignment from `install-proof.sh order`, as ci-job.sh does,
  // after seeding an earlier run's marker, which that call must clear.
  function installs(t, ordered) {
    const dir = scratchWorkspace(t);
    const env = {};
    if (ordered) {
      mkdirSync(join(dir, "target/install-proof"), { recursive: true });
      writeFileSync(join(dir, "target/install-proof/path.finished"), "");
      const order = run(dir, "bash", ["scripts/install-proof.sh", "order"]);
      assert.equal(order.status, 0, order.log);
      // Cleared before Nx starts: install-smoke clears it too, but only once it runs,
      // which can be after the documented install has looked.
      assert.ok(
        !existsSync(join(dir, "target/install-proof/path.finished")),
        "an earlier run's marker would release the wait",
      );
      const [name, value] = order.log.trim().split("=");
      env[name] = value;
    }
    commit(dir, "scripts/smoke-published.sh", 'echo "smoke stand-in passed"\n');
    git(dir, "remote", "add", "origin", dir);
    const scratch = mkdtempSync(join(tmpdir(), "install-order-"));
    t.after(() => rmSync(scratch, { recursive: true, force: true }));
    const bin = join(scratch, "bin");
    mkdirSync(bin);
    mkdirSync(join(scratch, "temp"));
    writeFileSync(join(bin, "cargo"), CARGO, { mode: 0o755 });
    writeFileSync(join(bin, "just"), JUST, { mode: 0o755 });
    const log = join(scratch, "order.log");
    const result = run(
      dir,
      "bash",
      [
        "scripts/nx.sh",
        "run-many",
        "-t",
        "install-smoke,install-documented",
        "-p",
        "onepipeline",
        "--parallel=4",
      ],
      {
        PATH: `${bin}:${process.env.PATH}`,
        ORDER_LOG: log,
        CARGO_INSTALLS: git(dir, "rev-parse", "HEAD"),
        RUNNER_TEMP: join(scratch, "temp"),
        NX_DAEMON: "false",
        ONEPIPELINE_NX_SHOW_OUTPUT: "1",
        ...env,
      },
    );
    assert.equal(result.status, 0, result.log);
    const at = Object.fromEntries(
      readFileSync(log, "utf8")
        .trimEnd()
        .split("\n")
        .map((line) => line.split(" ").reverse()),
    );
    for (const event of ["cargo-path", "cargo-documented", "host-view-start", "host-view-end"])
      assert.ok(at[event], `${event} never happened:\n${result.log}`);
    return { at: (event) => BigInt(at[event]) };
  }

  it("starts the documented build only after install-smoke's host-view-time reads, when asked to", (t) => {
    const { at } = installs(t, true);
    assert.ok(
      at("cargo-documented") > at("host-view-end"),
      "the documented build overlapped the host-view-time reads",
    );
  });

  it("runs the two side by side otherwise, as Windows, macOS and `timed` do", (t) => {
    const { at } = installs(t, false);
    assert.ok(
      at("cargo-documented") < at("host-view-start"),
      "the documented build waited without being asked",
    );
  });

  it("refuses an order it does not know before building anything", () => {
    const { status, log } = run(root, "bash", ["scripts/install-proof.sh", "documented"], {
      ONEPIPELINE_INSTALL_ORDER: "documented-first",
    });
    assert.equal(status, 2, log);
    assert.match(log, /ONEPIPELINE_INSTALL_ORDER='documented-first' is not an order this knows/);
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

  for (const temp of ["relative/temp", "/runner/../escape", "/runner/./temp", "/runner temp"]) {
    it(`refuses to clone into a RUNNER_TEMP of ${temp}`, () => {
      const { status, log } = run(root, "bash", ["screenshots/ci-capture.sh"], {
        SHOTS_OUT: "shots/current/x86_64",
        RUNNER_TEMP: temp,
      });
      assert.equal(status, 2, log);
      assert.match(log, /RUNNER_TEMP='.*' is not an absolute directory/);
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
      RUNNER_TEMP: "",
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

  // A stand-in `docker` on PATH that records how it was run: the capture's own
  // container run builds release binaries, which `lint` runs for real.
  function standInDocker(t) {
    const bin = mkdtempSync(join(tmpdir(), "ci-capture-docker-"));
    t.after(() => rmSync(bin, { recursive: true, force: true }));
    writeFileSync(join(bin, "docker"), '#!/bin/sh\nprintf "%s\\n" "$@" > "$DOCKER_ARGS"\n');
    chmodSync(join(bin, "docker"), 0o755);
    return { PATH: `${bin}:${process.env.PATH}`, DOCKER_ARGS: join(bin, "args") };
  }

  it("clones HEAD outside target/ in CI, where rust-cache cleans", (t) => {
    const dir = scratchWorkspace(t);
    const temp = mkdtempSync(join(tmpdir(), "ci-capture-runner-temp-"));
    t.after(() => rmSync(temp, { recursive: true, force: true }));
    const docker = standInDocker(t);
    const { status, log } = run(dir, "bash", ["screenshots/ci-capture.sh"], {
      SHOTS_OUT: "shots/current/x86_64",
      RUNNER_TEMP: temp,
      ...docker,
    });
    assert.equal(status, 0, log);
    const copy = join(temp, "ci-capture");
    assert.equal(git(copy, "rev-parse", "--show-toplevel"), copy);
    assert.equal(git(copy, "rev-parse", "HEAD"), git(dir, "rev-parse", "HEAD"));
    assert.ok(existsSync(join(copy, "rust-toolchain.toml")));
    assert.ok(!existsSync(join(dir, "target", "ci-capture")));
    const args = readFileSync(docker.DOCKER_ARGS, "utf8").split("\n");
    assert.ok(args.includes(`${copy}:/__w/onepipeline/onepipeline`), args.join(" "));
  });

  it("re-clones a copy that rust-cache's clean left without its own .git", (t) => {
    const dir = scratchWorkspace(t);
    // What the clean leaves of a clone: the directories it keeps, no `.git`, so
    // `git -C` there answers with the enclosing checkout's HEAD.
    const copy = join(dir, "target", "ci-capture");
    mkdirSync(join(copy, "deps"), { recursive: true });
    const docker = standInDocker(t);
    const { status, log } = run(dir, "bash", ["screenshots/ci-capture.sh"], {
      SHOTS_OUT: "shots/current/x86_64",
      RUNNER_TEMP: "",
      ...docker,
    });
    assert.equal(status, 0, log);
    assert.equal(git(copy, "rev-parse", "--show-toplevel"), copy);
    assert.ok(existsSync(join(copy, "rust-toolchain.toml")));
    assert.ok(!existsSync(join(copy, "deps")));
  });

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
    // show-env exports no RUSTFLAGS of its own, whatever it is run under, and a coverage
    // tier's nested cargo asks under `-D warnings`: lost, the warm line builds units no
    // journey asks for, and the first journey compiles the doubles again.
    assert.match(
      recipe,
      /instrumented\) eval "\$\(cargo llvm-cov show-env --export-prefix\)"; export RUSTFLAGS="-D warnings";/,
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

  // A coverage tier asks for the doubles under `-D warnings`; a warm build that lost those
  // flags saved units no journey asks for, and 83 of them were stale at the tier's first
  // journey. Over a scratch crate: what the recipe's own instrumented arm builds is fresh
  // under the tier's flags, and the arm's earlier form, which lost them, is not.
  it("are the units a coverage tier's flags ask for", () => {
    const arm = recipe.match(/instrumented\) (.*?); target=/)?.[1];
    assert.ok(arm, "the recipe has no instrumented arm");
    const dir = mkdtempSync(
      join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "instrumented-"),
    );
    try {
      mkdirSync(join(dir, "src"));
      writeFileSync(
        join(dir, "Cargo.toml"),
        '[package]\nname = "fixture"\nversion = "0.1.0"\nedition = "2021"\n',
      );
      writeFileSync(join(dir, "src/lib.rs"), "pub fn fixture() {}\n");
      const build = (flags) => {
        const script = `set -eu; unset RUSTFLAGS; ${flags}; cargo build --offline --target-dir out --message-format=json`;
        const done = spawnSync("bash", ["-c", script], { cwd: dir, encoding: "utf8" });
        assert.equal(done.status, 0, done.stderr);
        return done.stdout
          .split("\n")
          .filter((line) => line.startsWith('{"reason":"compiler-artifact"'))
          .map((line) => JSON.parse(line).fresh);
      };
      const tier =
        'eval "$(cargo llvm-cov show-env --export-prefix 2>/dev/null)"; export RUSTFLAGS="-D warnings"';
      assert.deepEqual(build(arm), [false], "the warm build compiled nothing");
      assert.deepEqual(build(tier), [true], "the tier's flags found the warm build stale");
      rmSync(join(dir, "out"), { recursive: true });
      build(
        `eval "$(RUSTFLAGS='-D warnings' cargo llvm-cov show-env --export-prefix 2>/dev/null)"`,
      );
      assert.deepEqual(
        build(tier),
        [false],
        "the arm's earlier form lost -D warnings, and this no longer tells them apart",
      );
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
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
