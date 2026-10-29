// The rule `spawnable` starts the npm journeys' commands by. Windows' lookup is
// held here on every host, against files laid out as npm lays them out there;
// the round trips run a real installed command on whatever host this is, so on
// Windows (`launcher.yml`) they go through cmd.exe and a real batch file.

import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { delimiter, join, win32 } from "node:path";
import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { installInto, pack, run } from "./install-support.mjs";
import { preloading, resolveWindows, spawnable } from "./spawn-support.mjs";

const WINDOWS = process.platform === "win32";

let work;
before(() => {
  work = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "spawn-"));
});
after(() => {
  if (work) rmSync(work, { recursive: true, force: true });
});

function directory(name, files) {
  const dir = join(work, name);
  mkdirSync(dir, { recursive: true });
  for (const file of files) writeFileSync(join(dir, file), "");
  return dir;
}

describe("on Windows, a command is", () => {
  const COMSPEC = "C:\\Windows\\system32\\cmd.exe";

  it("found as the batch file a Node.js install puts npm on PATH as, and run under cmd.exe", () => {
    // What a Node.js install puts beside node.exe: the shell script Git Bash
    // runs, PowerShell's script, and cmd.exe's batch file — no npm.exe.
    const nodejs = directory("nodejs", ["npm", "npm.ps1", "npm.cmd"]);
    // A spread `process.env` on Windows carries `Path`; an entry may be quoted.
    const env = { Path: `${join(work, "absent")};"${nodejs}"`, ComSpec: COMSPEC };

    assert.equal(resolveWindows("npm", env), join(nodejs, "npm.cmd"));
    const [file, args, options] = spawnable("npm", ["pack", "--json"], { cwd: work, env }, "win32");
    assert.equal(file, COMSPEC);
    assert.deepEqual(args, [
      "/d",
      "/s",
      "/c",
      `"${win32.normalize(join(nodejs, "npm.cmd"))} ^"pack^" ^"--json^""`,
    ]);
    assert.deepEqual(options, { cwd: work, env, windowsVerbatimArguments: true });
  });

  it("looked up PATH entry by entry, each with PATHEXT's extensions in its order", () => {
    const first = directory("first", ["tool.cmd"]);
    const second = directory("second", ["tool.exe"]);
    const both = directory("both", ["tool.cmd", "tool.exe"]);
    const env = { PATH: `${first};${second}` };
    assert.equal(resolveWindows("tool", env), join(first, "tool.cmd"));
    assert.equal(resolveWindows("tool", { PATH: both }), join(both, "tool.exe"));
    assert.equal(
      resolveWindows("tool", { PATH: both, PATHEXT: ".CMD;.EXE" }),
      join(both, "tool.cmd"),
    );
    // A name already carrying an extension is taken as given.
    assert.equal(resolveWindows("tool.cmd", { PATH: both }), join(both, "tool.cmd"));
  });

  it("left to Node.js when it is a program rather than a batch file, or is not found", () => {
    const bin = directory("programs", ["cargo.exe"]);
    const env = { PATH: bin };
    assert.deepEqual(spawnable("cargo", ["build"], { env }, "win32"), [
      "cargo",
      ["build"],
      { env },
    ]);
    // Node.js then reports the ENOENT itself, naming the command asked for.
    assert.equal(resolveWindows("nowhere", env), undefined);
    assert.deepEqual(spawnable("nowhere", [], { env }, "win32"), ["nowhere", [], { env }]);
  });

  for (const command of ["onepipeline", "onebudgetspec", "nx"]) {
    it(`resolves an extensionless installed ${command} bin and preserves Windows paths`, () => {
      const bins = directory(join(command, "node_modules", ".bin"), [`${command}.cmd`]);
      const shim = join(bins, `${command}.cmd`);
      const path = "C:\\Users\\RUNNER~1\\AppData\\program.cjs";
      const [file, args] = spawnable(
        join(bins, command),
        [path, "a&b"],
        { env: { PATH: "" } },
        "win32",
      );
      assert.equal(file, "cmd.exe");
      assert.ok(args[3].endsWith(` ^^^"${path}^^^" ^^^"a^^^&b^^^""`), args[3]);
      assert.equal(resolveWindows(join(bins, command), { PATH: "" }), shim);
    });
  }

  it("given each argument quoted so the program behind the batch file reads it back intact", () => {
    const dir = directory("quoting", ["echo.cmd"]);
    const [, args] = spawnable(
      "echo",
      ["two words", 'say "hi"', "trailing\\", "a|b", ""],
      { env: { PATH: dir } },
      "win32",
    );
    assert.ok(
      args[3].endsWith(' ^"two^ words^" ^"say^ \\^"hi\\^"^" ^"trailing\\\\^" ^"a^|b^" ^"^""'),
      args[3],
    );
  });
});

describe("outside Windows, a command is", () => {
  it("started exactly as asked", () => {
    const options = { env: { PATH: directory("posix", ["npm.cmd"]) } };
    for (const platform of ["linux", "darwin"]) {
      assert.deepEqual(spawnable("npm", ["pack"], options, platform), ["npm", ["pack"], options]);
    }
  });
});

// Keep the fixture's echo mode observable on hosts without cmd.exe.
function windowsEchoEntry(layout, script) {
  const invoke = `"${process.execPath}" "${script}" %*`;
  const line =
    layout === "installed"
      ? `@${invoke}`
      : `@ECHO off\r\nsetlocal\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & ${invoke}`;
  return `${line}\r\n`;
}

it("the Windows journey fixture suppresses command echo before its compound invocation", () => {
  const batch = windowsEchoEntry(join("node_modules", ".bin"), "echo-args.cjs");
  assert.match(batch, /^@ECHO off\r\n/i);
  assert.match(batch, /endLocal & goto .* & ".*" "echo-args.cjs" %\*\r\n$/);
});

it("npm's Windows bin shims forward only engine stdout and its status through the launcher", async () => {
  const project = join(work, "windows-install");
  const out = join(work, "packages");
  const launcherDir = run("node", ["scripts/npm-build.mjs", "launcher", "--out", out]).trim();
  installInto(project, [pack(launcherDir, work)]);
  const launcher = join(project, "node_modules", "onepipeline-cli", "bin", "onepipeline.js");
  const entry = join(project, "node_modules", ".bin", "onepipeline");
  // Use the very writer bundled with the npm doing the install, including on
  // Linux where npm normally makes a symlink instead of Windows' three files.
  const npmEnvironment = run("npm", ["run", "env", "--silent"]);
  const npmCli = npmEnvironment.match(/^npm_execpath=(.+)\r?$/m)?.[1].trim();
  assert.ok(npmCli, "npm run env must identify the npm CLI doing the install");
  const npmRequire = createRequire(npmCli);
  const cmdShim = npmRequire("cmd-shim");
  await cmdShim(launcher, entry);
  const cmd = readFileSync(`${entry}.cmd`, "utf8");
  const sh = readFileSync(entry, "utf8");
  const ps = readFileSync(`${entry}.ps1`, "utf8");
  assert.match(cmd, /^@ECHO off\r\n/i);
  assert.match(
    cmd,
    /endLocal & goto #_undefined_# 2>NUL \|\| title %COMSPEC% & "%_prog%"\s+"%dp0%\\..\\onepipeline-cli\\bin\\onepipeline.js" %\*\r\n$/,
  );
  const target = '"$basedir/../onepipeline-cli/bin/onepipeline.js"';
  assert.deepEqual(
    sh
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line.startsWith("exec ")),
    [`exec "$basedir/node"  ${target} "$@"`, `exec node  ${target} "$@"`],
  );
  assert.deepEqual(
    ps
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line.includes(" | & ") || line.startsWith("& ")),
    [
      `$input | & "$basedir/node$exe"  ${target} $args`,
      `& "$basedir/node$exe"  ${target} $args`,
      `$input | & "node$exe"  ${target} $args`,
      `& "node$exe"  ${target} $args`,
    ],
  );
  assert.match(cmd, /SET "_prog=%dp0%\\node.exe"/);
  assert.match(cmd, /SET "_prog=node"/);
  assert.match(ps, /\$ret=\$LASTEXITCODE/);
  assert.match(ps, /exit \$ret\n$/);

  const platform = join(project, "node_modules", "onepipeline-cli-win32-x64");
  mkdirSync(join(platform, "bin"), { recursive: true });
  writeFileSync(join(platform, "package.json"), '{"name":"onepipeline-cli-win32-x64"}');
  // A real executable engine: Node emits JSON and exits with the requested code.
  copyFileSync(process.execPath, join(platform, "bin", "onepipeline.exe"));
  const preload = join(project, "windows.cjs");
  // Pose as the win32-x64 host that package serves, whatever this host's arch.
  writeFileSync(
    preload,
    'Object.defineProperty(process, "platform", {value: "win32"});\n' +
      'Object.defineProperty(process, "arch", {value: "x64"});\n',
  );
  const sent = ["two words", 'say "hi"', "a&b|c", "", "trailing\\"];
  for (const status of [0, 3]) {
    const args = [
      "-e",
      `process.stdout.write(JSON.stringify(process.argv.slice(1))); process.exit(${status});`,
      "--",
      ...sent,
    ];
    // The native shells are unavailable on Linux. Check their exact forwarding
    // commands above, then execute the Node command each reaches, through the
    // real Windows launcher/keeper and executable engine. Run sh itself too.
    for (const command of [entry, launcher]) {
      const ran = spawnSync(
        ...(command === entry
          ? spawnable(entry, args, {
              encoding: "utf8",
              env: { ...process.env, NODE_OPTIONS: preloading(preload) },
            })
          : [
              process.execPath,
              [launcher, ...args],
              { encoding: "utf8", env: { ...process.env, NODE_OPTIONS: preloading(preload) } },
            ]),
      );
      assert.equal(ran.error, undefined);
      assert.equal(ran.status, status, ran.stderr);
      assert.equal(ran.stdout, JSON.stringify(sent));
      assert.equal(ran.stderr, "");
    }
  }
});

describe("on this host, a command started through it", () => {
  it("is npm itself, found however this platform installs it", () => {
    const ran = spawnSync(...spawnable("npm", ["--version"], { encoding: "utf8" }));
    assert.equal(ran.error, undefined);
    assert.equal(ran.status, 0, ran.stderr);
    assert.match(ran.stdout, /^\d+\.\d+\.\d+/);
  });

  // A command installed the way npm installs one on this platform — a batch file
  // on Windows, an executable script elsewhere — that prints what reached it.
  for (const layout of ["installed", join("node_modules", ".bin")]) {
    it(`receives every argument intact through ${layout}, and its exit status comes back`, () => {
      const bin = join(work, layout);
      mkdirSync(bin, { recursive: true });
      writeFileSync(
        join(work, "echo-args.cjs"),
        "process.stdout.write(JSON.stringify(process.argv.slice(2))); process.exit(3);\n",
      );
      const script = join(work, "echo-args.cjs");
      if (WINDOWS) {
        writeFileSync(join(bin, "echo-args.cmd"), windowsEchoEntry(layout, script));
      } else {
        writeFileSync(
          join(bin, "echo-args"),
          `#!/bin/sh\nexec "${process.execPath}" "${script}" "$@"\n`,
          { mode: 0o755 },
        );
      }
      // Windows' environment names are case-insensitive: replace the one it has.
      const pathKey = Object.keys(process.env).find((k) => k.toUpperCase() === "PATH") ?? "PATH";
      const sent = [
        "two words",
        'say "hi"',
        "trailing\\",
        "a&b|c",
        "(parens)",
        "^caret",
        "",
        "C:\\Users\\RUNNER~1\\AppData\\program.cjs",
      ];
      const ran = spawnSync(
        ...spawnable("echo-args", sent, {
          encoding: "utf8",
          env: { ...process.env, [pathKey]: `${bin}${delimiter}${process.env[pathKey]}` },
        }),
      );
      assert.equal(ran.error, undefined);
      assert.equal(ran.status, 3, ran.stderr);
      assert.deepEqual(JSON.parse(ran.stdout), sent);
    });
  }

  // A Windows temporary directory's path: backslashes, and a short name such as
  // RUNNER~1. Elsewhere a backslash is a character of the file's name, so the
  // same path is one file there.
  it("preloads a file whose path holds backslashes and a short name, as NODE_OPTIONS names it", () => {
    const dir = WINDOWS ? join(work, "RUNNER~1", "AppData") : work;
    mkdirSync(dir, { recursive: true });
    const preload = join(dir, WINDOWS ? "preload.cjs" : "RUNNER~1\\AppData\\preload.cjs");
    writeFileSync(preload, 'process.stdout.write("preloaded");\n');
    const ran = spawnSync(process.execPath, ["-e", ""], {
      encoding: "utf8",
      env: { ...process.env, NODE_OPTIONS: preloading(preload) },
    });
    assert.equal(ran.status, 0, ran.stderr);
    assert.equal(ran.stdout, "preloaded");
  });
});
