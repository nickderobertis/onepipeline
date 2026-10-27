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
// itself by the PID that execs the engine depends on exactly that, so a Node.js
// without `process.execve` (before 22.15 / 23.11) is refused there rather than
// given a child process in the engine's place.
//
// Windows has no exec. There the engine runs under a keeper process this one
// starts (`supervise` below): this process exits with the engine's status, the
// Ctrl+C and Ctrl+Break a console delivers reach the engine as they are, and the
// engine is ended rather than orphaned when this process is ended — abruptly too.
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

// The interrupts a Windows console delivers to every process attached to it,
// the engine included. The engine already has each one, as itself, so this
// process and the keeper hold on through them and report what the engine does.
const CONSOLE_EVENTS = ["SIGINT", "SIGBREAK"];

// The keeper, a Node process this one starts with an IPC channel to it. It
// starts the engine as its own child, so it knows the engine the moment the
// engine exists, and tells this process the engine's status. The channel breaks
// however this process ends — TerminateProcess included, where no handler of
// this process runs — and the keeper then ends the engine: there is no moment
// at which the engine runs and nothing that would end it knows it, however
// early in its start this process is ended.
const KEEPER = `
const { spawn } = require("node:child_process");
const [binary, ...args] = JSON.parse(process.argv[1]);
for (const signal of ${JSON.stringify(CONSOLE_EVENTS)}) process.on(signal, () => {});
const engine = spawn(binary, args, { stdio: "inherit" });
if (engine.pid !== undefined) process.send({ engine: engine.pid });
process.on("disconnect", () => {
  engine.kill("SIGKILL");
  process.exit(0);
});
const report = (message) => process.send(message, () => process.exit(0));
engine.on("error", (error) => report({ error: error.message }));
engine.on("exit", (code) => report({ status: code ?? 1 }));
`;

function supervise(binary, args) {
  for (const signal of CONSOLE_EVENTS) process.on(signal, () => {});
  const keeper = spawn(process.execPath, ["-e", KEEPER, JSON.stringify([binary, ...args])], {
    stdio: ["inherit", "inherit", "inherit", "ipc"],
  });
  // Without a keeper nothing would end the engine if this process were ended,
  // so no engine is started: the command refuses instead.
  keeper.on("error", (error) =>
    fail(
      `could not start the process that keeps the onepipeline binary from outliving ` +
        `this command, so the binary was not started: ${error.message}`,
    ),
  );
  let engine = null;
  const endEngine = () => {
    if (engine !== null) {
      try {
        process.kill(engine, "SIGKILL");
      } catch {}
    }
  };
  // The keeper's messages cross a process boundary, so each is read as exactly
  // one of its three envelopes; anything else ends the engine and the command.
  const isCount = (value) => Number.isInteger(value) && value >= 0;
  // A Windows exit code is 32 bits; process.exit would wrap a larger one, 2^32 to 0.
  const isStatus = (value) => isCount(value) && value <= 0xffffffff;
  keeper.on("message", (message) => {
    const keys = message !== null && typeof message === "object" ? Object.keys(message) : [];
    const [key] = keys;
    if (keys.length === 1 && key === "engine" && isCount(message.engine) && message.engine > 0) {
      engine = message.engine;
      return;
    }
    if (keys.length === 1 && key === "error" && typeof message.error === "string") {
      fail(`failed to launch the onepipeline binary: ${message.error}`);
    }
    // The engine's exit code verbatim — the contract assigns meaning to 1 and 2,
    // and a caller reads them.
    if (keys.length === 1 && key === "status" && isStatus(message.status)) {
      process.exit(message.status);
    }
    endEngine();
    fail(
      `the process keeping the onepipeline binary sent an unreadable message: ` +
        JSON.stringify(message),
    );
  });
  // A keeper gone without the engine's status has left the engine with
  // nothing to end it, so this process ends it before saying so.
  keeper.on("disconnect", () => {
    endEngine();
    fail("the process keeping the onepipeline binary ended without reporting its status");
  });
}

const binary = binaryPath();
const args = process.argv.slice(2);

if (process.platform === "win32") {
  supervise(binary, args);
} else {
  if (typeof process.execve !== "function") {
    fail(
      `Node.js ${process.version} cannot run onepipeline from npm on ${process.platform}: ` +
        "the command has to become the onepipeline binary's own process, which needs " +
        `process.execve (Node.js ${require("../package.json").engines.node}). Upgrade Node.js, or ` +
        `${OTHER_INSTALLS.charAt(0).toLowerCase()}${OTHER_INSTALLS.slice(1)}`,
    );
  }
  // Node ignores SIGXFSZ, and an ignored signal survives exec while a handled
  // one returns to its default: handle it so the engine, and every program it
  // starts, begins with the disposition a shell would give it. (Node ignores
  // SIGPIPE too, but the engine's runtime ignores that itself and restores it
  // for what it starts.)
  process.on("SIGXFSZ", () => {});
  try {
    process.execve(binary, [binary, ...args], process.env);
  } catch (error) {
    fail(`failed to launch the onepipeline binary: ${error.message}`);
  }
}
