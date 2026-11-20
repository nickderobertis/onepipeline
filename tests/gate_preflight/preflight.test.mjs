// `just gate`, `check`, `test` and `check-affected` as a developer runs them, over a scratch
// copy of the justfile and the scripts those recipes start, on a PATH that holds only what each
// case says is installed. The tools are present as executables at the paths and names the
// probes ask for; what the recipes would start to build or test — Nx through `node`, `npm ci`,
// `cargo` — is a counting stand-in that records each start and does nothing else.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const linux = process.platform === "linux";

// What the recipes and scripts under test run that is not under test: the shells, `just`, `git`
// and the coreutils their bodies call.
const HOST_TOOLS = [
  "bash",
  "cat",
  "chmod",
  "cut",
  "date",
  "dirname",
  "git",
  "grep",
  "just",
  "mkdir",
  "mv",
  "rm",
  "sed",
  "uname",
];
const STAND_INS = ["node", "npm", "cargo"];

// Each required tool: how a case takes it away, and the name and remedy its refusal must give.
// `linuxOnly` tools are asked for on Linux alone, where the journeys that need them compile.
const TOOLS = [
  { id: "node-modules", says: /the locked Node modules are not installed .*just bootstrap/ },
  { id: "cargo-nextest", says: /cargo-nextest is not installed .*just bootstrap/ },
  { id: "cargo-llvm-cov", says: /cargo-llvm-cov is not installed .*just bootstrap/ },
  { id: "strace", linuxOnly: true, says: /strace not installed .*apt-get install -y strace/ },
  {
    id: "unshare",
    linuxOnly: true,
    says: /unprivileged user namespaces are unavailable .*sysctl -w kernel\.apparmor_restrict_unprivileged_userns=0/,
  },
  { id: "onetaskgraph", says: /onetaskgraph \S+ is not installed .*just bootstrap/ },
  { id: "onebudgetspec", says: /onebudgetspec \S+ is not installed: run 'just bootstrap'/ },
  { id: "llmlint", says: /llmlint is not on PATH .*just bootstrap/ },
  { id: "uv", says: /uv is not installed .*docs\.astral\.sh\/uv/ },
];
const RECIPES = ["gate", "check", "test", "check-affected"];

function executable(path, body) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `#!/bin/sh\n${body}`);
  chmodSync(path, 0o755);
}

/**
 * The justfile and the scripts its entry points start, committed as `main` with `origin/main`
 * on it, so `check-affected` derives a base and `gate` judges an empty diff. Cargo.toml is the
 * one line the justfile reads at load.
 */
function workspace(t) {
  const dir = mkdtempSync(join(tmpdir(), "gate-preflight-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (const path of [
    "justfile",
    "scripts/nx.sh",
    "scripts/nx-affected.sh",
    "scripts/gate-time.sh",
  ]) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    copyFileSync(join(root, path), join(dir, path));
  }
  writeFileSync(join(dir, "Cargo.toml"), '[package]\nrust-version = "1.0"\n');
  writeFileSync(join(dir, ".gitignore"), "/bin/\n/target/\n/node_modules\n/.logs/\n/calls\n");
  const git = (...args) =>
    execFileSync("git", args, {
      cwd: dir,
      env: {
        ...process.env,
        GIT_AUTHOR_NAME: "gate preflight",
        GIT_AUTHOR_EMAIL: "gate-preflight@example.invalid",
        GIT_COMMITTER_NAME: "gate preflight",
        GIT_COMMITTER_EMAIL: "gate-preflight@example.invalid",
      },
    });
  git("init", "--quiet", "--initial-branch=main");
  git("add", "--all");
  git("commit", "--quiet", "--no-verify", "--message", "base");
  git("update-ref", "refs/remotes/origin/main", "HEAD");
  return dir;
}

/** Install every required tool except those named in `absent`, and the counting stand-ins. */
function provision(dir, absent = []) {
  for (const path of ["bin", "target", "node_modules", "calls", ".gate-time.json"])
    rmSync(join(dir, path), { recursive: true, force: true });
  const bin = join(dir, "bin");
  mkdirSync(bin);
  const hostPath = process.env.PATH.split(delimiter);
  for (const tool of HOST_TOOLS) {
    const found = hostPath.map((entry) => join(entry, tool)).find((path) => existsSync(path));
    assert.ok(found, `${tool} is not on this host's PATH`);
    symlinkSync(found, join(bin, tool));
  }
  for (const tool of STAND_INS)
    executable(join(bin, tool), `printf '%s\\n' "${tool} $*" >> "${join(dir, "calls")}"\n`);
  for (const tool of ["cargo-nextest", "cargo-llvm-cov", "strace", "unshare", "llmlint", "uv"])
    if (!absent.includes(tool)) executable(join(bin, tool), "exit 0\n");
  for (const tool of ["onetaskgraph", "onebudgetspec"]) {
    if (absent.includes(tool)) continue;
    const installed = execFileSync("just", ["--evaluate", `${tool}-root`], {
      cwd: dir,
      encoding: "utf8",
    });
    executable(join(installed, "bin", tool), "exit 0\n");
  }
  // The real locked tree: its `nx` is `#!/usr/bin/env node`, which reaches the stand-in.
  if (!absent.includes("node-modules"))
    symlinkSync(join(root, "node_modules"), join(dir, "node_modules"));
}

function run(dir, args) {
  const result = spawnSync("just", args, {
    cwd: dir,
    encoding: "utf8",
    env: { PATH: join(dir, "bin"), HOME: dir },
    // A socket on stdin, which is what Node's pipes are, makes a Debian bash source the host's
    // bashrc as though started by sshd; a terminal user's has none.
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.error) throw result.error;
  const calls = existsSync(join(dir, "calls"))
    ? readFileSync(join(dir, "calls"), "utf8").split("\n").filter(Boolean)
    : [];
  return { ...result, calls };
}

/** The Nx command lines the stand-in `node` was started with, its script path dropped. */
const nx = (calls) => calls.map((call) => call.replace(/^node \S*node_modules\/\.bin\/nx /, "nx "));

test("each entry point refuses before Nx starts when one tool is missing, naming it and its remedy", (t) => {
  const dir = workspace(t);
  for (const tool of TOOLS) {
    provision(dir, [tool.id]);
    for (const recipe of RECIPES) {
      const ran = run(dir, [recipe]);
      if (tool.linuxOnly && !linux) {
        assert.doesNotMatch(ran.stderr, tool.says, `${recipe} asked for ${tool.id} off Linux`);
        continue;
      }
      assert.notEqual(ran.status, 0, `${recipe} passed without ${tool.id}:\n${ran.stderr}`);
      assert.match(ran.stderr, tool.says, `${recipe} without ${tool.id}`);
      assert.match(ran.stderr, /refusing before any build or test: 1 required tool\(s\) above/);
      assert.deepEqual(ran.calls, [], `${recipe} started work without ${tool.id}`);
    }
  }
});

test("two missing tools are both named in one refusal", (t) => {
  const dir = workspace(t);
  const absent = TOOLS.filter((tool) => linux || !tool.linuxOnly).slice(-2);
  provision(
    dir,
    absent.map((tool) => tool.id),
  );
  for (const recipe of RECIPES) {
    const ran = run(dir, [recipe]);
    assert.notEqual(ran.status, 0, ran.stderr);
    for (const tool of absent) assert.match(ran.stderr, tool.says, `${recipe}: ${tool.id}`);
    assert.match(ran.stderr, /refusing before any build or test: 2 required tool\(s\) above/);
    assert.deepEqual(ran.calls, [], recipe);
  }
});

test("with every tool present the preflight is silent and each recipe hands off to Nx as before", (t) => {
  const dir = workspace(t);
  provision(dir);
  const preflight = run(dir, ["_preflight"]);
  assert.equal(preflight.status, 0, preflight.stderr);
  assert.equal(preflight.stdout, "");
  assert.equal(preflight.stderr, "");
  assert.deepEqual(preflight.calls, []);

  const base = execFileSync("git", ["rev-parse", "origin/main"], { cwd: dir, encoding: "utf8" });
  const each = ["format-check", "lint", "test", "doc"].map((target) => `nx run-many -t ${target}`);
  const expected = {
    test: ["nx run-many -t test"],
    check: [...each, "nx run-many -t check"],
    "check-affected": [`nx affected --base=${base.trim()} --head=HEAD -t check`],
    gate: [...each, "nx run-many -t check", "nx run onepipeline-lint-llm:lint-llm-diff"],
  };
  for (const recipe of RECIPES) {
    provision(dir);
    const ran = run(dir, [recipe]);
    assert.equal(ran.status, 0, `${recipe}:\n${ran.stderr}`);
    assert.deepEqual(nx(ran.calls), expected[recipe], recipe);
    if (recipe !== "test") assert.match(ran.stdout, new RegExp(`^${recipe}: ok$`, "m"));
    assert.doesNotMatch(ran.stderr, /not installed|not on PATH|unavailable|refusing/);
  }
});
