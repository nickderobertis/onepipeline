// A small workspace for the gate-profile journeys: real just, Nx and cargo, this repository's own
// scripts/nx.sh, gate-profile/ and `gate-profile` recipe, and a gate shaped like the real one —
// `gate` calls `_gate-body`, whose dependency `build` is one Nx invocation and whose body is a
// second. Their tasks form one chain across the two: crate:build (a cargo build of a crate with a
// library, a binary, a build script and a path dependency) → work:marker (appends to a marker
// file on every real execution) → work:tree (a child that starts a grandchild doing CPU work and
// reporting its own getrusage CPU) → work:check.
import { execFileSync, spawnSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const stateRoot = () => process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir();

/** The CPU the grandchild burns before it reports, in microseconds: above any other task's. */
export const GRANDCHILD_CPU_US = 2_000_000;

const json = (value) => `${JSON.stringify(value, null, 2)}\n`;

const FILES = {
  ".gitignore": "/target\n/crate/target\n/.nx\n/.logs\n/.markers\n/node_modules\n",
  "package.json": json({ name: "gate-profile-fixture", private: true }),
  "nx.json": json({ cacheDirectory: ".nx/cache", plugins: [] }),
  "crate/project.json": json({
    name: "crate",
    targets: {
      build: {
        command: "just _cargo-build",
        cache: true,
        inputs: ["{projectRoot}/**/*"],
        outputs: [],
      },
    },
  }),
  "work/project.json": json({
    name: "work",
    targets: {
      marker: {
        command: "node work/marker.mjs",
        cache: true,
        inputs: ["{projectRoot}/**/*"],
        outputs: [],
        dependsOn: [{ projects: ["crate"], target: "build" }],
      },
      tree: {
        command: "node work/tree.mjs",
        cache: true,
        inputs: ["{projectRoot}/**/*"],
        outputs: [],
        dependsOn: ["marker"],
      },
      check: { executor: "nx:noop", dependsOn: ["tree"] },
    },
  }),
  "work/marker.mjs": `import { appendFileSync, mkdirSync } from "node:fs";
mkdirSync(".markers", { recursive: true });
appendFileSync(".markers/marker.log", \`\${process.pid}\\n\`);
const until = process.cpuUsage().user + 200_000;
while (process.cpuUsage().user < until);
`,
  "work/tree.mjs": `import { spawnSync } from "node:child_process";
const child = spawnSync(process.execPath, ["work/child.mjs"], { stdio: "inherit" });
process.exit(child.status ?? 1);
`,
  "work/child.mjs": `import { spawnSync } from "node:child_process";
const grandchild = spawnSync(process.execPath, ["work/grandchild.mjs"], { stdio: "inherit" });
process.exit(grandchild.status ?? 1);
`,
  "work/grandchild.mjs": `import { appendFileSync, mkdirSync } from "node:fs";
const cpu = () => {
  const usage = process.resourceUsage();
  return usage.userCPUTime + usage.systemCPUTime;
};
let spin = 0;
while (cpu() < ${GRANDCHILD_CPU_US}) spin++;
mkdirSync(".markers", { recursive: true });
appendFileSync(".markers/grandchild.jsonl", \`\${JSON.stringify({ at: Date.now(), cpu_us: cpu(), spin })}\\n\`);
`,
  "crate/Cargo.toml": `[workspace]
members = [".", "helper"]

[package]
name = "fixture_app"
version = "0.1.0"
edition = "2021"
build = "build.rs"

[lib]
path = "src/lib.rs"

[[bin]]
name = "fixture_app"
path = "src/main.rs"

[dependencies]
fixture_helper = { path = "helper" }
`,
  "crate/build.rs": 'fn main() {\n    println!("cargo:rerun-if-changed=build.rs");\n}\n',
  "crate/src/lib.rs": "pub fn answer() -> u32 {\n    fixture_helper::half() * 2\n}\n",
  "crate/src/main.rs": 'fn main() {\n    println!("{}", fixture_app::answer());\n}\n',
  "crate/helper/Cargo.toml":
    '[package]\nname = "fixture_helper"\nversion = "0.1.0"\nedition = "2021"\n\n[lib]\npath = "src/lib.rs"\n',
  "crate/helper/src/lib.rs": "pub fn half() -> u32 {\n    21\n}\n",
};

const JUSTFILE = `set shell := ["bash", "-eu", "-o", "pipefail", "-c"]
set positional-arguments := true

export CARGO_TERM_QUIET := "true"

gate:
    @just _gate-body

_gate-body: build
    @bash scripts/nx.sh run-many -t check

build:
    @bash scripts/nx.sh run-many -t build

_cargo-build:
    @cargo build --locked --quiet --manifest-path crate/Cargo.toml

broken:
    @echo "broken: failing on purpose" >&2; exit 7

slow:
    @sleep 60

# An Nx run that fails before it runs any task: no task profile, and no task graph to read back.
missing:
    @bash scripts/nx.sh run crate:nosuch

# Leaves an invocation record the fold cannot read.
garbled:
    @mkdir -p "$ONEPIPELINE_GATE_PROFILE_DIR/nx" && echo '{' > "$ONEPIPELINE_GATE_PROFILE_DIR/nx/x.invocation.json"
`;

/**
 * The environment the journeys run under: this one's, less anything of a profiled run, an
 * enclosing Nx run's cache switches, or a CI job this test may itself be running inside.
 */
export function fixtureEnvironment() {
  const env = { ...process.env, NX_DAEMON: "false" };
  for (const name of Object.keys(env))
    if (
      name.startsWith("ONEPIPELINE_GATE_PROFILE_") ||
      name.startsWith("GITHUB_") ||
      name.startsWith("NX_SELF_HOSTED_REMOTE_CACHE_") ||
      [
        "NX_PROFILE",
        "NX_CACHE_DIRECTORY",
        "NX_WORKSPACE_DATA_DIRECTORY",
        // An enclosing Nx run started with --skip-nx-cache hands its tasks these.
        "NX_SKIP_NX_CACHE",
        "NX_DISABLE_NX_CACHE",
        "NX_SKIP_REMOTE_CACHE",
      ].includes(name)
    )
      delete env[name];
  return env;
}

function git(cwd, ...args) {
  return execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    env: {
      ...fixtureEnvironment(),
      GIT_AUTHOR_NAME: "gate profile",
      GIT_AUTHOR_EMAIL: "gate-profile@example.invalid",
      GIT_COMMITTER_NAME: "gate profile",
      GIT_COMMITTER_EMAIL: "gate-profile@example.invalid",
    },
  }).trim();
}

/** Build the fixture in a fresh directory, committed, and return its path and commit. */
export function fixture(t) {
  const dir = mkdtempSync(join(stateRoot(), "gate-profile-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (const [path, content] of Object.entries(FILES)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), content);
  }
  // The recipe exactly as this repository's justfile declares it.
  const recipe = execFileSync("just", ["--show", "gate-profile"], { cwd: root, encoding: "utf8" });
  writeFileSync(join(dir, "justfile"), `${JUSTFILE}\n${recipe}`);
  mkdirSync(join(dir, "scripts"));
  cpSync(join(root, "scripts", "nx.sh"), join(dir, "scripts", "nx.sh"));
  cpSync(join(root, "rust-toolchain.toml"), join(dir, "rust-toolchain.toml"));
  // The command's own files, not this project's declaration or its tests.
  cpSync(join(root, "gate-profile"), join(dir, "gate-profile"), {
    recursive: true,
    filter: (source) => !/(\.test\.mjs|fixture\.mjs|project\.json)$/.test(source),
  });
  symlinkSync(join(root, "node_modules"), join(dir, "node_modules"));
  execFileSync("cargo", ["generate-lockfile", "--quiet", "--manifest-path", "crate/Cargo.toml"], {
    cwd: dir,
    env: fixtureEnvironment(),
  });
  git(dir, "init", "--quiet", "--initial-branch=main");
  git(dir, "add", "-A");
  git(dir, "commit", "--quiet", "--no-verify", "-m", "fixture");
  return { dir, commit: git(dir, "rev-parse", "HEAD") };
}

/** Run `just <args>` in the fixture, with `env` over the journeys' own, and return its output. */
export function just(dir, args, env = {}) {
  const result = spawnSync("just", args, {
    cwd: dir,
    encoding: "utf8",
    env: { ...fixtureEnvironment(), ...env },
    maxBuffer: 64 * 1024 * 1024,
  });
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

export const markerLines = (dir) =>
  existsSync(join(dir, ".markers/marker.log"))
    ? readFileSync(join(dir, ".markers/marker.log"), "utf8").trim().split("\n").length
    : 0;

export const grandchildReports = (dir) =>
  readFileSync(join(dir, ".markers/grandchild.jsonl"), "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));

/** Every file under `dir` (not following the node_modules link, nor into .git). */
export function filesUnder(dir, base = dir) {
  const found = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name === ".git" || entry.name === "node_modules") continue;
    const path = join(dir, entry.name);
    if (entry.isDirectory()) found.push(...filesUnder(path, base));
    else found.push(path.slice(base.length + 1));
  }
  return found;
}

/** The units cargo itself reports for the fixture crate: each package's built targets. */
export function cargoUnits(dir) {
  const out = execFileSync(
    "cargo",
    ["build", "--quiet", "--message-format=json", "--manifest-path", "crate/Cargo.toml"],
    { cwd: dir, encoding: "utf8", env: fixtureEnvironment() },
  );
  return out
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line))
    .filter((m) => m.reason === "compiler-artifact" || m.reason === "build-script-executed")
    .map((m) => ({
      reason: m.reason,
      name: m.package_id.match(/#?([\w-]+)@[\d.]+$/)?.[1] ?? m.package_id,
      kind: m.target?.kind?.[0] ?? "run-custom-build",
    }));
}
