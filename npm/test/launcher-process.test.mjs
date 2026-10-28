// The process tree the npm-installed `onepipeline` gives its caller.
//
// On Linux and macOS the launcher replaces itself with the engine, so the PID a
// caller holds is the engine's, and a Node.js that cannot do that is refused.
// Windows has no exec: there the engine runs under a keeper the launcher starts,
// and the launcher has to pass the console's interrupts and the engine's status
// through faithfully and never leave the engine running past itself.
//
// Everything here is real: the compiled engine, packed into the platform package
// and installed beside the packed launcher by `npm install`. The engine's
// long-running journey is `flow run`, which supervises a program, relays
// SIGINT/SIGTERM/SIGHUP to it and exits with its status; that program
// (`PROGRAM` below) records which process started it and which signal reached it.
// CI runs this file on Windows (`launcher.yml`) and macOS (`ci.yml`'s Mac lane);
// Linux runs it in the gate with the rest of the project's tests.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readlinkSync, realpathSync } from "node:fs";
import { chmodSync, closeSync, mkdirSync, openSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, afterEach, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { STAGED_TARGET_DIR, hostTarget, installInto, pack, run } from "./install-support.mjs";
import { spawnable } from "./spawn-support.mjs";

const WINDOWS = process.platform === "win32";
const EXE = WINDOWS ? "onepipeline.exe" : "onepipeline";

/// What Windows ends a console process with when Ctrl+C or Ctrl+Break reaches it
/// and nothing handles it — what the engine, which installs no console handler,
/// ends with there.
const STATUS_CONTROL_C_EXIT = 0xc000013a;

/// The program `flow run` supervises. It records, atomically, its own PID, its
/// parent's (the engine's), and every signal that reaches it; echoes its stdin;
/// exits on end of input with `PROGRAM_EXIT`, and on a signal with that
/// signal's own code, so the status the caller sees names what arrived. Under
/// `PROGRAM_HOLD` it ignores its input and runs until a signal ends it, so that
/// nothing but the launcher can end the engine.
const PROGRAM = `
const fs = require("node:fs");
const ready = process.argv[2];
const seen = [];
const record = () => {
  fs.writeFileSync(ready + ".tmp", JSON.stringify({ pid: process.pid, ppid: process.ppid, seen }));
  fs.renameSync(ready + ".tmp", ready);
};
for (const [signal, code] of [["SIGINT", 7], ["SIGTERM", 8], ["SIGBREAK", 9], ["SIGHUP", 10]]) {
  process.on(signal, () => { seen.push(signal); record(); process.exit(code); });
}
if (process.env.PROGRAM_HOLD) {
  setInterval(() => {}, 1 << 30);
} else {
  process.stdin.on("data", (chunk) => process.stdout.write(chunk));
  process.stdin.on("end", () => process.exit(Number(process.env.PROGRAM_EXIT ?? 0)));
}
record();
`;

/// Node reporting Windows to the launcher it is preloaded into, and to nothing
/// that launcher starts.
const AS_WINDOWS = `Object.defineProperty(process, "platform", { value: "win32" });
Object.defineProperty(process, "arch", { value: "x64" });
`;

/// The launcher ended outright the instant its first child — the keeper — has
/// been started, before anything else of the launcher's runs. The keeper's PID
/// is left for the test in `KEEPER_PID_FILE`.
const DIES_AFTER_SPAWN = `const childProcess = require("node:child_process");
const spawn = childProcess.spawn;
childProcess.spawn = function (...args) {
  const child = spawn.apply(this, args);
  require("node:fs").writeFileSync(process.env.KEEPER_PID_FILE, String(child.pid));
  process.kill(process.pid, "SIGKILL");
  return child;
};
`;

/// The caller's environment without this host's own onepipeline settings, and
/// with a runs root of the test's own: `flow run` with no session registers
/// nothing, but it must not be able to reach a real one.
/// `NODE_OPTIONS` preloading `file`. Node reads a backslash inside its quotes as
/// an escape, so a Windows path is quoted as JSON is, not as it is written.
const preloading = (file) => `--require ${JSON.stringify(file)}`;

function engineEnv(work, extra = {}) {
  const env = Object.fromEntries(
    Object.entries(process.env).filter(([key]) => !key.startsWith("ONEPIPELINE_")),
  );
  return { ...env, ONEPIPELINE_RUNS_DIR: join(work, "runs"), ...extra };
}

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

async function waitFor(what, check, ms = 30_000) {
  const deadline = Date.now() + ms;
  for (;;) {
    const found = check();
    if (found) return found;
    assert.ok(Date.now() < deadline, `timed out waiting for ${what}`);
    await sleep(50);
  }
}

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === "EPERM";
  }
}

/// Every process a test started, with the record its program keeps, so that a
/// test that fails part-way ends what it started instead of holding the run open.
const launched = [];

function track(child, ready) {
  launched.push({ child, ready });
  return child;
}

afterEach(() => {
  for (const { child, ready } of launched.splice(0)) {
    const record = readRecord(ready);
    for (const pid of [child.pid, record?.ppid, record?.pid]) {
      if (pid && alive(pid)) process.kill(pid, "SIGKILL");
    }
    for (const stream of [child.stdin, child.stdout, child.stderr]) stream?.destroy();
  }
});

/// The parent `pid` was started by, from the process table.
function parentOf(pid) {
  const asked = WINDOWS
    ? spawnSync(
        "powershell.exe",
        [
          "-NoProfile",
          "-Command",
          `(Get-CimInstance Win32_Process -Filter "ProcessId=${pid}").ParentProcessId`,
        ],
        { encoding: "utf8" },
      )
    : spawnSync("ps", ["-o", "ppid=", "-p", String(pid)], { encoding: "utf8" });
  const parent = Number(asked.stdout.trim());
  assert.ok(parent > 0, `no parent read for ${pid}: ${asked.stderr}`);
  return parent;
}

/// The running engines whose `flow run` supervises the program recording to
/// `ready`, from the process table: an engine that never got to start its
/// program is found as surely as one that did.
function enginesFor(ready) {
  const listed = WINDOWS
    ? spawnSync(
        "powershell.exe",
        [
          "-NoProfile",
          "-Command",
          'Get-CimInstance Win32_Process | ForEach-Object { "$($_.ProcessId) $($_.CommandLine)" }',
        ],
        { encoding: "utf8" },
      )
    : spawnSync("ps", ["-eo", "pid=,args="], { encoding: "utf8" });
  assert.equal(listed.status, 0, listed.stderr);
  return listed.stdout
    .split(/\r?\n/)
    .filter((line) => line.includes(" flow run ") && line.includes(ready))
    .map((line) => Number(line.trim().split(/\s+/)[0]));
}

function readRecord(ready) {
  return existsSync(ready) ? JSON.parse(readFileSync(ready, "utf8")) : null;
}

/// A started process's exit, with everything it wrote.
function finished(child) {
  let stdout = "";
  let stderr = "";
  child.stdout?.on("data", (chunk) => {
    stdout += chunk;
  });
  child.stderr?.on("data", (chunk) => {
    stderr += chunk;
  });
  return new Promise((done) =>
    child.on("close", (code, signal) => done({ code, signal, stdout, stderr })),
  );
}

/// A started process's own exit, without waiting for the pipes it handed down:
/// a launcher ended under its engine leaves them held by what outlived it.
const exited = (child) => new Promise((done) => child.on("exit", done));

/// The file the kernel is running as `pid`, read from the process table rather
/// than from anything the process says about itself.
function executableOf(pid) {
  if (process.platform === "linux") return realpathSync(readlinkSync(`/proc/${pid}/exe`));
  // macOS has no /proc; lsof's `txt` descriptors are the mapped executables, the
  // program's own image first.
  const listed = spawnSync("lsof", ["-a", "-p", String(pid), "-d", "txt", "-Fn"], {
    encoding: "utf8",
  });
  const image = listed.stdout.split("\n").find((line) => line.startsWith("n"));
  assert.ok(image, `lsof named no executable for ${pid}: ${listed.stderr}`);
  return realpathSync(image.slice(1));
}

/// Start a command line in a new, hidden console, wait for its supervised
/// program to be up, deliver a console control event to that console, and
/// record the command's exit status. Windows offers no other way to raise a
/// console's Ctrl+C from outside it.
const CONSOLE_EVENT = `
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class ConsoleEvent {
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  struct STARTUPINFO {
    public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
    public int dwX; public int dwY; public int dwXSize; public int dwYSize;
    public int dwXCountChars; public int dwYCountChars; public int dwFillAttribute;
    public int dwFlags; public short wShowWindow; public short cbReserved2;
    public IntPtr lpReserved2; public IntPtr hStdInput; public IntPtr hStdOutput; public IntPtr hStdError;
  }
  [StructLayout(LayoutKind.Sequential)]
  struct PROCESS_INFORMATION { public IntPtr hProcess; public IntPtr hThread; public int dwProcessId; public int dwThreadId; }
  delegate bool Handler(uint type);
  [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
  static extern bool CreateProcessW(string app, StringBuilder cmd, IntPtr pa, IntPtr ta, bool inherit,
    uint flags, IntPtr env, string cwd, ref STARTUPINFO si, out PROCESS_INFORMATION pi);
  [DllImport("kernel32.dll", SetLastError = true)] static extern bool FreeConsole();
  [DllImport("kernel32.dll", SetLastError = true)] static extern bool AttachConsole(uint pid);
  [DllImport("kernel32.dll", SetLastError = true)] static extern bool SetConsoleCtrlHandler(Handler handler, bool add);
  [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "SetConsoleCtrlHandler")]
  static extern bool SetConsoleCtrlIgnored(IntPtr none, bool ignored);
  [DllImport("kernel32.dll", SetLastError = true)] static extern bool GenerateConsoleCtrlEvent(uint ev, uint group);
  [DllImport("kernel32.dll", SetLastError = true)] static extern uint WaitForSingleObject(IntPtr h, uint ms);
  [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetExitCodeProcess(IntPtr h, out uint code);
  static readonly Handler Ignore = type => true;
  static IntPtr process;
  public static int Start(string commandLine) {
    var si = new STARTUPINFO();
    si.cb = Marshal.SizeOf(typeof(STARTUPINFO));
    si.dwFlags = 1; // STARTF_USESHOWWINDOW
    si.wShowWindow = 0; // SW_HIDE
    PROCESS_INFORMATION pi;
    // Whether Ctrl+C is ignored is inherited, and a CI runner starts its steps
    // ignoring it: the launcher is given the handling a terminal gives instead.
    if (!SetConsoleCtrlIgnored(IntPtr.Zero, false))
      throw new Exception("SetConsoleCtrlHandler failed: " + Marshal.GetLastWin32Error());
    // CREATE_NEW_CONSOLE
    if (!CreateProcessW(null, new StringBuilder(commandLine), IntPtr.Zero, IntPtr.Zero, false, 0x10,
        IntPtr.Zero, null, ref si, out pi))
      throw new Exception("CreateProcess failed: " + Marshal.GetLastWin32Error());
    process = pi.hProcess;
    return pi.dwProcessId;
  }
  public static void Send(int pid, uint ev) {
    FreeConsole();
    if (!AttachConsole((uint)pid)) throw new Exception("AttachConsole failed: " + Marshal.GetLastWin32Error());
    // The event reaches every process on the console, this one included.
    SetConsoleCtrlHandler(Ignore, true);
    if (!GenerateConsoleCtrlEvent(ev, 0))
      throw new Exception("GenerateConsoleCtrlEvent failed: " + Marshal.GetLastWin32Error());
  }
  public static long Wait() {
    if (WaitForSingleObject(process, 60000) != 0) throw new Exception("the launcher did not exit within 60s");
    uint code;
    GetExitCodeProcess(process, out code);
    FreeConsole();
    return code;
  }
}
'@
$launcher = [ConsoleEvent]::Start($env:CONSOLE_COMMAND_LINE)
$deadline = (Get-Date).AddSeconds(60)
while (-not (Test-Path -LiteralPath $env:CONSOLE_READY)) {
  if ((Get-Date) -gt $deadline) { throw "the supervised program never started" }
  Start-Sleep -Milliseconds 100
}
[ConsoleEvent]::Send($launcher, [uint32]$env:CONSOLE_EVENT)
$exit = [ConsoleEvent]::Wait()
Set-Content -LiteralPath $env:CONSOLE_OUT -Value (ConvertTo-Json @{ launcher = $launcher; exit = $exit })
`;

describe("the process the npm-installed command runs as", () => {
  let work;
  let project;
  let program;
  let packages;

  before(() => {
    work = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "launcher-"));
    const target = hostTarget();
    // The staged builds' own directory, for the reason `install-support.mjs` gives.
    run("cargo", ["build", "--locked", "--bin", "onepipeline", "--target", target]);
    const binary = join(STAGED_TARGET_DIR, target, "debug", EXE);
    // The packaging step validates only that the SBOM is CycloneDX and names this
    // crate; what it lists is the release's concern, not this file's.
    const sbom = join(work, "sbom.json");
    writeFileSync(
      sbom,
      JSON.stringify({ bomFormat: "CycloneDX", metadata: { component: { name: "onepipeline" } } }),
    );
    const dist = join(work, "dist");
    const platformDir = run("node", [
      ...["scripts/npm-build.mjs", "platform", "--target", target],
      ...["--binary", binary, "--sbom", sbom, "--out", dist],
    ]).trim();
    const launcherDir = run("node", ["scripts/npm-build.mjs", "launcher", "--out", dist]).trim();
    project = join(work, "app");
    packages = [pack(launcherDir, work), pack(platformDir, work)];
    installInto(project, packages);
    program = join(work, "program.cjs");
    writeFileSync(program, PROGRAM);
  });

  after(() => {
    if (work) rmSync(work, { recursive: true, force: true });
  });

  const installedEngine = () =>
    realpathSync(
      join(
        project,
        "node_modules",
        `onepipeline-cli-${process.platform}-${process.arch}`,
        "bin",
        EXE,
      ),
    );
  const launcherScript = () =>
    join(project, "node_modules", "onepipeline-cli", "bin", "onepipeline.js");
  const flowRun = (ready) => ["flow", "run", "--", process.execPath, program, ready];

  if (!WINDOWS) {
    describe("on Linux and macOS", () => {
      // The command npm put on PATH, started the way any caller starts it.
      const start = (ready, env) =>
        track(
          spawn(join(project, "node_modules", ".bin", "onepipeline"), flowRun(ready), {
            cwd: project,
            env: engineEnv(work, env),
          }),
          ready,
        );

      it("is the engine itself: the caller's PID runs the engine binary and parents what it starts", async () => {
        const ready = join(work, "pid.json");
        const child = start(ready);
        const done = finished(child);
        const started = await waitFor("the supervised program", () => readRecord(ready));

        assert.equal(
          executableOf(child.pid),
          installedEngine(),
          "the caller's PID is not the engine",
        );
        assert.equal(
          started.ppid,
          child.pid,
          "what the engine starts must see the caller's PID as its parent",
        );
        if (process.platform === "linux") {
          // Node ignores SIGXFSZ, and an ignored signal survives exec: the engine
          // must not inherit that from the launcher it replaced.
          const ignored = readFileSync(`/proc/${child.pid}/status`, "utf8").match(
            /^SigIgn:\s*([0-9a-f]+)$/m,
          )[1];
          assert.equal(
            BigInt(`0x${ignored}`) & (1n << 24n),
            0n,
            "the engine starts with SIGXFSZ ignored",
          );
        }

        child.stdin.end("through the engine\n");
        const ended = await done;
        assert.equal(ended.code, 0, ended.stderr);
        assert.equal(ended.stdout, "through the engine\n");
        assert.match(ended.stderr, /runs as no flow/);
        assert.doesNotMatch(ended.stderr, /Warning/);
      });

      it("hands a signal sent to the caller's PID to the engine, and exits with the engine's status", async () => {
        const ready = join(work, "signal.json");
        const child = start(ready);
        const done = finished(child);
        await waitFor("the supervised program", () => readRecord(ready));

        child.kill("SIGTERM");
        const ended = await done;
        // The engine relayed the SIGTERM to its program, which exited 8 for it,
        // and the engine exited with that status.
        assert.deepEqual(readRecord(ready).seen, ["SIGTERM"]);
        assert.equal(ended.code, 8, ended.stderr);
      });

      it("refuses a Node.js that cannot exec, rather than running the engine as its child", () => {
        const withoutExec = join(work, "without-execve.cjs");
        writeFileSync(withoutExec, "delete process.execve;\n");
        const refused = spawnSync(
          join(project, "node_modules", ".bin", "onepipeline"),
          ["--version"],
          {
            cwd: project,
            encoding: "utf8",
            env: engineEnv(work, { NODE_OPTIONS: preloading(withoutExec) }),
          },
        );
        assert.equal(refused.status, 1, refused.stderr);
        assert.equal(refused.stdout, "", "the engine ran");
        assert.match(refused.stderr, /cannot run onepipeline from npm on \w+/);
        const { engines } = JSON.parse(
          readFileSync(join(project, "node_modules", "onepipeline-cli", "package.json"), "utf8"),
        );
        assert.ok(
          refused.stderr.includes(`process.execve (Node.js ${engines.node})`),
          refused.stderr,
        );
        assert.match(refused.stderr, /pip install onepipeline-cli/);
      });

      // On Node.js 22 a failed exec aborts Node, so the launcher checks the
      // binary before it execs; both refusals come from that check.
      it("refuses, and exits 1, when the installed binary is missing or not a file", () => {
        const damaged = join(work, "damaged");
        installInto(damaged, packages);
        const engine = join(
          damaged,
          "node_modules",
          `onepipeline-cli-${process.platform}-${process.arch}`,
          "bin",
          EXE,
        );
        const refuse = () =>
          spawnSync(join(damaged, "node_modules", ".bin", "onepipeline"), ["--version"], {
            cwd: damaged,
            encoding: "utf8",
            env: engineEnv(work),
          });

        rmSync(engine);
        const missing = refuse();
        assert.equal(missing.status, 1, missing.stderr);
        assert.equal(missing.stdout, "");
        assert.match(
          missing.stderr,
          /^onepipeline: failed to launch the onepipeline binary: ENOENT/,
        );

        mkdirSync(engine);
        const directory = refuse();
        assert.equal(directory.status, 1, directory.stderr);
        assert.equal(directory.stdout, "");
        // The launcher names the resolved path: on macOS the temp directory
        // sits under `/var`, a symlink to `/private/var`.
        assert.equal(
          directory.stderr,
          `onepipeline: failed to launch the onepipeline binary: ${realpathSync(engine)} is not a file\n`,
        );
      });
    });
  }

  // Windows has no exec, so there the engine runs under the launcher's keeper.
  // Linux and macOS reach that same code by Node reporting `win32` to the
  // launcher alone (`AS_WINDOWS`), beside a `win32-x64` platform package that
  // carries this host's engine; the console events stay Windows'.
  describe("under the keeper Windows runs it with", () => {
    let asWindows = [];
    const keeperFixture = (dir, mode = 0o755) => {
      if (WINDOWS) return;
      const pkg = join(dir, "node_modules", "onepipeline-cli-win32-x64");
      mkdirSync(join(pkg, "bin"), { recursive: true });
      writeFileSync(
        join(pkg, "package.json"),
        JSON.stringify({ name: "onepipeline-cli-win32-x64" }),
      );
      writeFileSync(join(pkg, "bin", "onepipeline.exe"), readFileSync(installedEngine()), { mode });
    };
    before(() => {
      if (WINDOWS) return;
      const shim = join(work, "as-windows.cjs");
      writeFileSync(shim, AS_WINDOWS);
      asWindows = ["--require", shim];
      keeperFixture(project);
    });
    const start = (ready, options = {}, preload = []) =>
      track(
        spawn(process.execPath, [...asWindows, ...preload, launcherScript(), ...flowRun(ready)], {
          cwd: project,
          ...options,
          env: engineEnv(work, options.env),
        }),
        ready,
      );

    it("passes stdin, stdout and the engine's exit status through, and leaves no keeper", async () => {
      const ready = join(work, "status.json");
      const child = start(ready, { env: { PROGRAM_EXIT: "5" } });
      const done = finished(child);
      const started = await waitFor("the supervised program", () => readRecord(ready));
      const keeper = parentOf(started.ppid);
      assert.notEqual(keeper, child.pid, "the engine should run under the keeper here");
      assert.equal(parentOf(keeper), child.pid, "the keeper should be the launcher's child");

      child.stdin.end("through the launcher\n");
      const ended = await done;
      assert.equal(ended.code, 5, ended.stderr);
      assert.equal(ended.stdout, "through the launcher\n");
      await waitFor("the keeper to exit", () => !alive(keeper), 15_000);
    });

    it("ends the engine and says so when the keeper dies before reporting", async () => {
      const ready = join(work, "keeper-killed.json");
      const child = start(ready, { stdio: ["ignore", "pipe", "pipe"], env: { PROGRAM_HOLD: "1" } });
      let stderr = "";
      child.stderr.on("data", (chunk) => {
        stderr += chunk;
      });
      const done = exited(child);
      const started = await waitFor("the supervised program", () => readRecord(ready));
      const engine = started.ppid;

      process.kill(parentOf(engine), "SIGKILL");
      assert.equal(await done, 1);
      await waitFor("the engine to be ended", () => !alive(engine), 15_000);
      assert.match(stderr, /the process keeping the onepipeline binary ended without reporting/);
    });

    it("exits 1 when the engine is killed outright", async () => {
      const ready = join(work, "engine-killed.json");
      const child = start(ready, { stdio: "ignore", env: { PROGRAM_HOLD: "1" } });
      const done = exited(child);
      const started = await waitFor("the supervised program", () => readRecord(ready));

      // SIGKILL leaves no exit code; on Windows TerminateProcess leaves 1.
      process.kill(started.ppid, "SIGKILL");
      assert.equal(await done, 1);
    });

    it("leaves no engine running when the launcher is killed outright", async () => {
      const ready = join(work, "killed.json");
      // Held and given no pipes: the engine has no reason to end but the launcher.
      const child = start(ready, { stdio: "ignore", env: { PROGRAM_HOLD: "1" } });
      const done = exited(child);
      const started = await waitFor("the supervised program", () => readRecord(ready));
      const engine = started.ppid;
      assert.ok(alive(engine));
      // SIGKILL; on Windows, TerminateProcess. No handler of the launcher runs.
      child.kill("SIGKILL");
      await done;
      await waitFor("the orphaned engine to be ended", () => !alive(engine), 15_000);
    });

    // Each envelope's field of the wrong type, a status exit would wrap to 0
    // (2^32), a key the keeper never sends, and a status beside a field it never
    // sends: none may be read as an envelope.
    for (const [index, [name, message]] of [
      ["a status not a number", { status: "done" }],
      ["a status out of range", { status: 4294967296 }],
      ["an engine PID not a number", { engine: "1" }],
      ["an error not a string", { error: 5 }],
      ["a message it does not know", { done: true }],
      ["a status with a field beside it", { status: 0, extra: 1 }],
    ].entries()) {
      it(`ends the engine and refuses when the keeper reports ${name}`, async () => {
        const ready = join(work, `unreadable-${index}.json`);
        // Only the keeper has an IPC channel, so only it sends: once the engine's
        // program is up, a message the launcher must not act on.
        const lies = join(work, `keeper-lies-${index}.cjs`);
        writeFileSync(
          lies,
          `if (process.send) {
  const fs = require("node:fs");
  const wait = setInterval(() => {
    if (!fs.existsSync(process.env.LIE_AFTER)) return;
    clearInterval(wait);
    process.send(${JSON.stringify(message)});
  }, 50);
}
`,
        );
        // A file rather than a pipe: what outlives the launcher would hold a pipe open.
        const log = join(work, `unreadable-${index}.stderr`);
        const stderrFile = openSync(log, "w");
        const child = start(ready, {
          stdio: ["ignore", "ignore", stderrFile],
          env: { PROGRAM_HOLD: "1", LIE_AFTER: ready, NODE_OPTIONS: preloading(lies) },
        });
        closeSync(stderrFile);
        const code = await exited(child);
        const stderr = readFileSync(log, "utf8");
        const started = readRecord(ready);
        assert.ok(started, "the engine's program never started");
        assert.equal(code, 1, stderr);
        assert.ok(
          stderr.includes(
            `the process keeping the onepipeline binary sent an unreadable message: ${JSON.stringify(message)}`,
          ),
          stderr,
        );
        await waitFor("the engine to be ended", () => !alive(started.ppid), 15_000);
      });
    }

    it("leaves no engine running when the launcher is asked to end with SIGTERM", async () => {
      const ready = join(work, "terminated.json");
      const child = start(ready, { stdio: "ignore", env: { PROGRAM_HOLD: "1" } });
      const done = exited(child);
      const started = await waitFor("the supervised program", () => readRecord(ready));
      const engine = started.ppid;
      const keeper = parentOf(engine);
      assert.ok(alive(engine));
      // Sent to the launcher alone, as a caller holding its PID sends it; on
      // Windows Node delivers it as TerminateProcess.
      child.kill("SIGTERM");
      await done;
      await waitFor("the engine to be ended", () => !alive(engine), 15_000);
      await waitFor("the keeper to exit", () => !alive(keeper), 15_000);
    });

    it("leaves no engine running when the launcher is killed the moment the keeper starts", async () => {
      const ready = join(work, "startup.json");
      const keeperPid = join(work, "startup-keeper.pid");
      const dies = join(work, "dies-after-spawn.cjs");
      writeFileSync(dies, DIES_AFTER_SPAWN);
      const child = start(
        ready,
        { stdio: "ignore", env: { PROGRAM_HOLD: "1", KEEPER_PID_FILE: keeperPid } },
        ["--require", dies],
      );
      await exited(child);
      const keeper = Number(readFileSync(keeperPid, "utf8"));
      // A keeper that started the engine and missed the launcher's death would
      // still be waiting on it.
      await waitFor("the keeper to exit", () => !alive(keeper), 15_000);
      // The keeper starts the engine before it can learn of the launcher's death,
      // so by the time it has exited, any engine it started is in the table.
      const survivors = enginesFor(ready);
      for (const pid of survivors) process.kill(pid, "SIGKILL");
      assert.deepEqual(survivors, [], "the engine outlived the launcher");
    });

    it("refuses to start the engine, through the installed command, when it cannot start the keeper", () => {
      const preload = join(work, "no-keeper.cjs");
      writeFileSync(
        preload,
        `${WINDOWS ? "" : AS_WINDOWS}process.execPath = ${JSON.stringify(join(work, "no-such-node"))};\n`,
      );
      const bin = join(
        project,
        "node_modules",
        ".bin",
        WINDOWS ? "onepipeline.cmd" : "onepipeline",
      );
      const refused = spawnSync(
        ...spawnable(bin, ["--version"], {
          cwd: project,
          encoding: "utf8",
          env: engineEnv(work, { NODE_OPTIONS: preloading(preload) }),
        }),
      );
      assert.equal(refused.status, 1, refused.stderr);
      assert.equal(refused.stdout, "", "the engine ran without a keeper");
      assert.match(
        refused.stderr,
        /could not start the process that keeps the onepipeline binary from outliving this command, so the binary was not started: /,
      );
    });

    it("says it could not start the engine, and exits 1, when the installed binary will not run", () => {
      const broken = join(work, "broken");
      installInto(broken, packages);
      const engine = join(
        broken,
        "node_modules",
        `onepipeline-cli-${process.platform}-${process.arch}`,
        "bin",
        EXE,
      );
      // Not executable outside Windows; not a program at all on it.
      if (WINDOWS) writeFileSync(engine, "not a program\n");
      else chmodSync(engine, 0o644);
      keeperFixture(broken, 0o644);
      const launcher = join(broken, "node_modules", "onepipeline-cli", "bin", "onepipeline.js");
      // The keeper's way, and on Linux and macOS the exec's too.
      for (const prefix of WINDOWS ? [[]] : [asWindows, []]) {
        const refused = spawnSync(process.execPath, [...prefix, launcher, "--version"], {
          cwd: broken,
          encoding: "utf8",
          env: engineEnv(work),
        });
        assert.equal(refused.status, 1, refused.stderr);
        assert.equal(refused.stdout, "");
        assert.match(refused.stderr, /^onepipeline: failed to launch the onepipeline binary: /);
      }
    });

    if (WINDOWS) {
      // A console's Ctrl+C and Ctrl+Break, delivered the way a terminal delivers
      // them: to every process attached to the launcher's console. The launcher
      // gets a console of its own so the event reaches nothing of the test's.
      for (const [name, event, seen] of [
        ["Ctrl+C", 0, "SIGINT"],
        ["Ctrl+Break", 1, "SIGBREAK"],
      ]) {
        it(`leaves the engine the console's ${name} and exits with the status it ends with`, async () => {
          const ready = join(work, `console-${event}.json`);
          const out = join(work, `console-${event}.out.json`);
          const helper = join(work, "console-event.ps1");
          writeFileSync(helper, CONSOLE_EVENT);
          const quote = (arg) => `"${arg}"`;
          const sent = spawnSync(
            "powershell.exe",
            ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", helper],
            {
              cwd: project,
              encoding: "utf8",
              timeout: 180_000,
              env: engineEnv(work, {
                CONSOLE_COMMAND_LINE: [process.execPath, launcherScript(), ...flowRun(ready)]
                  .map(quote)
                  .join(" "),
                CONSOLE_READY: ready,
                CONSOLE_EVENT: String(event),
                CONSOLE_OUT: out,
              }),
            },
          );
          assert.equal(sent.status, 0, `${sent.stdout}\n${sent.stderr}`);
          const result = JSON.parse(readFileSync(out, "utf8"));
          const started = readRecord(ready);
          assert.deepEqual(started.seen, [seen]);
          assert.equal(result.exit, STATUS_CONTROL_C_EXIT);
          await waitFor("the engine to end", () => !alive(started.ppid), 15_000);
        });
      }
    }
  });
});
