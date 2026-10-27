#!/usr/bin/env node
// Launcher for the `onepipeline` command installed from the
// `onepipeline-cli` npm package.
//
// Like the PyPI wheels (maturin `bindings = "bin"`, see pyproject.toml), the npm
// distribution carries the *prebuilt* Rust binary — no Rust toolchain, no
// compile, no download at install time. The platform-specific binary ships
// inside a per-platform package (`onepipeline-cli-<platform>-<arch>`) declared
// in this package's `optionalDependencies`; npm installs only the one whose
// `os`/`cpu` match the host, and this shim resolves it and starts it with the
// caller's argv, stdin, stdout and stderr.
//
// How it starts it is the point of this file. On Linux and macOS it *replaces
// itself* with the binary (`process.execve`), so the process the caller started
// is the engine: the PID the caller holds is the engine's, a signal sent to it
// reaches the engine, and a process the engine starts sees the engine as its
// parent — the process tree a PyPI or cargo install gives. A caller that marks
// itself by the PID that execs the engine depends on exactly that.
//
// Windows has no exec, and neither does a Node.js older than 22.15 / 23.11; there
// the engine runs as this process's child (`supervise` below). This process
// waits for it, leaves the Ctrl+C and Ctrl+Break a console delivers to the
// engine as they are, passes a signal sent to this process alone on to the
// engine as itself (outside Windows), exits with the engine's status, and ends
// the engine rather than orphaning it when this process is ended — abruptly too.
//
// This file is committed source; the version and the optionalDependency versions
// are stamped from Cargo.toml at publish time by scripts/npm-build.mjs, which
// also generates the per-platform packages from the release binaries.

const path = require("node:path");
const { spawn } = require("node:child_process");

// process.platform-process.arch -> the platform package that carries the binary.
// The keys mirror the targets release/release.py builds, the TARGETS table in
// scripts/npm-build.mjs, and the optionalDependencies in package.json; keep the
// four in lockstep.
const PACKAGES = {
  "linux-x64": "onepipeline-cli-linux-x64",
  "linux-arm64": "onepipeline-cli-linux-arm64",
  "darwin-arm64": "onepipeline-cli-darwin-arm64",
  "win32-x64": "onepipeline-cli-win32-x64",
};

// Every failure here is a failed install, so say what to do about it rather than
// only what went wrong.
const OTHER_INSTALLS =
  "Install another way instead: 'pip install onepipeline-cli', or " +
  "'cargo install onepipeline --locked'.";

function fail(message) {
  process.stderr.write(`onepipeline: ${message}\n`);
  process.exit(1);
}

function binaryPath() {
  const key = `${process.platform}-${process.arch}`;
  // Intel macOS packages are no longer published, so say that rather than
  // listing the platforms that are, or failing on a package that is missing.
  if (key === "darwin-x64") {
    fail(
      "Intel macOS (darwin-x64) is no longer supported: prebuilt binaries ship " +
        "for Apple silicon macOS only. On Apple silicon, run an arm64 Node.js; on an " +
        "Intel Mac, build from source with 'cargo install onepipeline --locked'.",
    );
  }
  const pkg = PACKAGES[key];
  // llmlint: ignore-block[changed_behavior_has_e2e] reaching this branch means running
  // where no prebuilt package exists, and a test could only get there by lying to node
  // about process.platform — which would prove the lie. The sibling branches are driven
  // for real by npm/test/launcher.test.mjs, and npm's own os/cpu fields keep a user from
  // installing a platform package this map does not name.
  if (!pkg) {
    fail(
      `unsupported platform ${key}. Prebuilt binaries exist for: ` +
        `${Object.keys(PACKAGES).join(", ")}. ${OTHER_INSTALLS}`,
    );
  }
  // llmlint: ignore-end[changed_behavior_has_e2e]

  const binName = process.platform === "win32" ? "onepipeline.exe" : "onepipeline";
  try {
    // Resolve the platform package's manifest, then locate the binary beside it.
    // Resolving package.json (rather than the binary file directly) is portable
    // across Node resolution modes and does not require an `exports` entry for a
    // non-JS asset.
    const manifest = require.resolve(`${pkg}/package.json`);
    return path.join(path.dirname(manifest), "bin", binName);
  } catch (_err) {
    return fail(
      `the platform package ${pkg} is not installed. This usually means npm ` +
        "skipped optional dependencies (e.g. --no-optional / --omit=optional) " +
        "or the install was for a different platform. Reinstall with optional " +
        `dependencies enabled. ${OTHER_INSTALLS}`,
    );
  }
}

// The signals a caller ends or interrupts the engine with: the set the engine
// itself relays to a program it supervises, and Windows' Ctrl+Break, SIGBREAK.
const RELAYED = ["SIGINT", "SIGTERM", "SIGHUP", "SIGBREAK"];

// Run in a separate Node process for as long as the engine is this process's
// child. Its stdin is a pipe only this process holds, so it reads end-of-file
// the moment this process ends, however it ends — `TerminateProcess` and SIGKILL
// included, where no handler of this process runs. It is told the engine's PID,
// and is killed first when this process exits normally; end-of-file with a PID
// and no kill means this process was ended under the engine, so it ends the
// engine instead of leaving it running unowned.
const WATCHER = `
let told = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => { told += chunk; });
const orphaned = () => {
  const pid = Number.parseInt(told, 10);
  if (pid > 0) { try { process.kill(pid, "SIGKILL"); } catch {} }
  process.exit(0);
};
process.stdin.on("end", orphaned);
process.stdin.on("error", orphaned);
`;

// The engine as this process's child, for where exec is not available.
function supervise(binary, args) {
  // Detached, so neither a console's Ctrl+C nor a terminal's foreground-group
  // signal ends it alongside this process: it has to outlive this process to
  // do its one job.
  const watcher = spawn(process.execPath, ["-e", WATCHER], {
    stdio: ["pipe", "ignore", "ignore"],
    detached: true,
    windowsHide: true,
  });
  watcher.on("error", () => {});
  watcher.stdin.on("error", () => {});
  watcher.unref();

  const engine = spawn(binary, args, { stdio: "inherit" });
  engine.on("error", (error) => {
    watcher.kill();
    fail(`failed to launch the onepipeline binary: ${error.message}`);
  });
  if (engine.pid !== undefined) watcher.stdin.write(`${engine.pid}\n`);

  for (const signal of RELAYED) {
    // A console delivers Ctrl+C and Ctrl+Break to every process attached to it,
    // the engine included, so on Windows the engine already has the very same
    // interrupt and relaying it would only end the engine a second, harsher way;
    // this process stays to report what the engine does with it. Elsewhere the
    // signal reached this process alone — a `kill` of the PID the caller holds —
    // so it is passed on as itself.
    process.on(signal, () => {
      if (process.platform !== "win32") engine.kill(signal);
    });
  }
  // Whatever ends this process with the engine still running ends the engine
  // too, rather than leaving it to the watcher.
  process.on("exit", () => {
    if (engine.exitCode === null && engine.signalCode === null) engine.kill("SIGKILL");
  });

  engine.on("exit", (code, signal) => {
    watcher.kill();
    // Re-raise a terminating signal so callers observe the true cause; otherwise
    // propagate the engine's exit code verbatim — the contract assigns meaning to
    // 1 and 2, and a caller reads them.
    if (signal) {
      process.removeAllListeners(signal);
      process.kill(process.pid, signal);
    }
    process.exit(code === null ? 1 : code);
  });
}

const binary = binaryPath();
const args = process.argv.slice(2);

if (process.platform !== "win32" && typeof process.execve === "function") {
  // Node ignores SIGPIPE and SIGXFSZ, and an ignored signal survives exec while
  // a handled one returns to its default: handle both so the engine, and every
  // program it starts, begins with the dispositions a shell would give it.
  for (const signal of ["SIGPIPE", "SIGXFSZ"]) process.on(signal, () => {});
  try {
    process.execve(binary, [binary, ...args], process.env);
  } catch (error) {
    fail(`failed to launch the onepipeline binary: ${error.message}`);
  }
}
supervise(binary, args);
