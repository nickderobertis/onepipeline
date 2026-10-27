// The process tree the npm-installed `onepipeline` gives its caller.
//
// On Linux and macOS the launcher replaces itself with the engine, so the PID a
// caller holds is the engine's. Where it cannot (Windows, or a Node.js without
// `process.execve`), the engine is its child, and the launcher has to pass the
// caller's interrupts and the engine's status through faithfully and never leave
// the engine running past itself.
//
// Everything here is real: the compiled engine, packed into the platform package
// and installed beside the packed launcher by `npm install`. The engine's
// long-running journey is `flow run`, which supervises a program, relays
// SIGINT/SIGTERM/SIGHUP to it and exits with its status; that program
// (`PROGRAM` below) records which process started it and which signal reached it.
// CI runs this file on macOS and Windows (`ci.yml`'s `launcher` job); Linux runs
// it in the gate with the rest of the project's tests.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readlinkSync, realpathSync } from "node:fs";
import { rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, afterEach, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { STAGED_TARGET_DIR, hostTarget, installInto, pack, run } from "./install-support.mjs";

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

/// The caller's environment without this host's own onepipeline settings, and
/// with a runs root of the test's own: `flow run` with no session registers
/// nothing, but it must not be able to reach a real one.
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

function launch(child, ready) {
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
    installInto(project, [pack(launcherDir, work), pack(platformDir, work)]);
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
        launch(
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
    });
  }

  // The launcher as its own process: Windows always, and on Linux and macOS a
  // Node.js without `process.execve`, which this host stands in for by removing it.
  describe("where the engine runs as the launcher's child", () => {
    let withoutExec;
    before(() => {
      withoutExec = join(work, "without-execve.cjs");
      writeFileSync(withoutExec, "delete process.execve;\n");
    });
    const start = (ready, options = {}) =>
      launch(
        spawn(
          process.execPath,
          [...(WINDOWS ? [] : ["--require", withoutExec]), launcherScript(), ...flowRun(ready)],
          { cwd: project, ...options, env: engineEnv(work, options.env) },
        ),
        ready,
      );

    it("passes stdin, stdout and the engine's exit status through", async () => {
      const ready = join(work, "status.json");
      const child = start(ready, { env: { PROGRAM_EXIT: "5" } });
      const done = finished(child);
      const started = await waitFor("the supervised program", () => readRecord(ready));
      assert.notEqual(started.ppid, child.pid, "the engine should be the launcher's child here");

      child.stdin.end("through the launcher\n");
      const ended = await done;
      assert.equal(ended.code, 5, ended.stderr);
      assert.equal(ended.stdout, "through the launcher\n");
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

    if (!WINDOWS) {
      for (const [signal, code] of [
        ["SIGINT", 7],
        ["SIGTERM", 8],
      ]) {
        it(`passes ${signal} on to the engine as ${signal} and exits with its status`, async () => {
          const ready = join(work, `${signal}.json`);
          const child = start(ready);
          const done = finished(child);
          const started = await waitFor("the supervised program", () => readRecord(ready));

          child.kill(signal);
          const ended = await done;
          assert.deepEqual(readRecord(ready).seen, [signal]);
          assert.equal(ended.code, code, ended.stderr);
          assert.equal(alive(started.ppid), false, "the engine outlived the launcher");
        });
      }
    }

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
