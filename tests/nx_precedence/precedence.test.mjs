// `scripts/nx-precedence.sh`, which the e2e tier's coverage run goes through now that it
// runs beside the Linux job's other tasks, driven inside real Nx runs: the leading task
// must keep its CPU priority, a task beside it must drop to the lowest, with what that
// task spawns afterwards and a task Nx starts later, and a `--keep` task must keep its
// own. A process carrying the same task's name under another run's root stands for
// another Nx run on the same host.
//
// This project's `test` runs alone (`parallelism: false`): beside the e2e tier its own
// processes would already be at the lowest priority, and the run it starts would
// inherit that, leaving nothing for the wrapper to lower.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  readlinkSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const script = join(root, "scripts", "nx-precedence.sh");
const linux = process.platform === "linux";
// biome-ignore lint/suspicious/noControlCharactersInRegex: the escape is what is stripped.
const plain = (text) => text.replace(/\u001b\[[0-9;]*[A-Za-z]/g, "");

// Each record is the niceness of a process the task starts at that moment, read by
// `nice.sh` from /proc rather than through `ps`, which one case below replaces with a
// stand-in that fails: a task's new process inherits what the task then has, which is
// how a lowered task's later work is lowered with it.
const NICE_SH = 'read -ra f < /proc/$$/stat; echo "$' + '{f[18]}"\n';
const record = (name) => `bash nice.sh > ${name}.tmp && mv ${name}.tmp ${name}.nice`;
const SIDE = `bash -c 'for i in $(seq 300); do [ -e done ] && break; ${record("side")}; if [ "$(cat side.nice)" = 19 ] && [ ! -e side.io ]; then /usr/bin/ionice -p $$ > side.io 2>&1; fi; sleep 0.2; done'`;
const KEPT = `bash -c 'for i in $(seq 300); do [ -e done ] && break; ${record("kept")}; sleep 0.2; done'`;
const LATE = `bash -c 'for i in $(seq 300); do [ -e done ] && break; ${record("late")}; sleep 0.2; done'`;
// The leading task waits until every other task has recorded once, then long enough for
// the wrapper's once-a-second pass to have reached them twice.
const LEAD = `bash ${script} --keep kept:work -- bash -c 'for i in $(seq 300); do [ -e side.nice ] && [ -e kept.nice ] && [ -e late.nice ] && break; sleep 0.1; done; sleep 3; bash nice.sh > lead.nice; touch done; exit 0'`;

function workspace(t, { churn = false } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "nx-precedence-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  writeFileSync(join(dir, "package.json"), '{"name":"precedence","private":true}\n');
  writeFileSync(join(dir, "nx.json"), "{}\n");
  symlinkSync(join(root, "node_modules"), join(dir, "node_modules"));
  writeFileSync(join(dir, "nice.sh"), NICE_SH);
  const targets = {
    lead: { command: LEAD },
    side: { command: SIDE },
    kept: { command: KEPT },
    // `late` starts only after `pause`, two seconds in, when the wrapper has already
    // passed over the run at least once.
    pause: { command: "sleep 2" },
    late: { command: LATE, dependsOn: [{ projects: ["pause"], target: "work" }] },
  };
  // A task that keeps starting processes that end at once, so the wrapper's lists keep
  // naming processes that are gone before it lowers them.
  if (churn) {
    targets.churn = {
      command: `bash -c 'for i in $(seq 3000); do [ -e done ] && break; env true & sleep 0.01; done; wait'`,
    };
  }
  for (const [name, target] of Object.entries(targets)) {
    mkdirSync(join(dir, name));
    writeFileSync(
      join(dir, name, "project.json"),
      JSON.stringify({ name, targets: { work: { ...target, cache: false } } }),
    );
  }
  return dir;
}

// A directory of stand-ins put ahead of PATH, each failing with the message given; one
// given `after` fails that many times and then runs the real tool.
function failing(t, tools, after = Number.POSITIVE_INFINITY, code = 1) {
  const dir = mkdtempSync(join(tmpdir(), "nx-precedence-stubs-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (const [tool, message] of Object.entries(tools)) {
    const real = spawnSync("bash", ["-c", `command -v ${tool}`], {
      encoding: "utf8",
    }).stdout.trim();
    const recover = Number.isFinite(after)
      ? `n=$(cat "$0.count" 2>/dev/null || echo 0); echo $((n + 1)) > "$0.count"\n[ "$n" -lt ${after} ] || exec ${real} "$@"\n`
      : "";
    writeFileSync(join(dir, tool), `#!/bin/sh\n${recover}echo "${message}" >&2\nexit ${code}\n`);
    chmodSync(join(dir, tool), 0o755);
  }
  return dir;
}

// A PATH holding only links to the tools named, so one left out is not found at all.
function only(t, tools) {
  const dir = mkdtempSync(join(tmpdir(), "nx-precedence-path-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (const tool of tools) {
    const real = spawnSync("bash", ["-c", `command -v ${tool}`], {
      encoding: "utf8",
    }).stdout.trim();
    assert.ok(real, `${tool} is not on this host's PATH`);
    symlinkSync(real, join(dir, tool));
  }
  return dir;
}

function nx(dir, { direct = "true", path = process.env.PATH } = {}) {
  // This suite's own run would otherwise be the inner one's root too.
  const env = {
    ...process.env,
    PATH: path,
    NX_DAEMON: "false",
    NX_NO_CLOUD: "true",
    NX_RUN_COMMANDS_DIRECTLY: direct,
    NX_TASK_TARGET_PROJECT: "",
    NX_TASK_TARGET_TARGET: "",
  };
  delete env.NX_INVOCATION_ROOT_PID;
  const { status, stdout, stderr } = spawnSync(
    join(root, "node_modules", ".bin", "nx"),
    [
      "run-many",
      "-t",
      "work",
      "--parallel=6",
      "--skip-nx-cache",
      "--outputStyle=stream-without-prefixes",
    ],
    {
      cwd: dir,
      encoding: "utf8",
      timeout: 120_000,
      env,
    },
  );
  return { status, log: plain(`${stdout}\n${stderr}`) };
}

const read = (dir, file) => readFileSync(join(dir, file), "utf8").trim();
const lines = (log) => log.split("\n").filter((line) => line.startsWith("nx-precedence:"));

// Every process still working in the run's directory: the wrapper's watcher would be
// one. A watcher ended mid-sleep leaves its `sleep 1` behind for at most that second.
function leftIn(dir) {
  const deadline = Date.now() + 3000;
  let left = [];
  do {
    left = readdirSync("/proc")
      .filter((pid) => /^\d+$/.test(pid))
      .filter((pid) => {
        try {
          return readlinkSync(`/proc/${pid}/cwd`) === dir;
        } catch {
          return false;
        }
      });
    if (left.length === 0) return left;
    spawnSync("sleep", ["0.2"]);
  } while (Date.now() < deadline);
  return left.map((pid) => readFileSync(`/proc/${pid}/cmdline`, "utf8").replaceAll("\0", " "));
}

describe("the e2e tier's precedence over the tasks beside it", () => {
  for (const direct of ["true", "false"]) {
    it(`lowers the run's other tasks, early and late, and nothing else (NX_RUN_COMMANDS_DIRECTLY=${direct})`, (t) => {
      const dir = workspace(t);
      const foreign = spawn("bash", ["-c", "sleep 60"], {
        env: {
          ...process.env,
          NX_TASK_TARGET_PROJECT: "side",
          NX_TASK_TARGET_TARGET: "work",
          NX_INVOCATION_ROOT_PID: "1",
        },
        stdio: "ignore",
      });
      t.after(() => foreign.kill());
      const { status, log } = nx(dir, { direct });
      assert.equal(status, 0, log);
      assert.ok(existsSync(join(dir, "done")), log);
      const foreignNice = spawnSync("ps", ["-o", "ni=", "-p", String(foreign.pid)], {
        encoding: "utf8",
      });
      assert.equal(foreignNice.stdout.trim(), "0", "a process of no task of this run was lowered");
      assert.equal(read(dir, "lead.nice"), "0", log);
      assert.equal(read(dir, "kept.nice"), "0", `the --keep task was lowered\n${log}`);
      if (linux) {
        assert.equal(read(dir, "side.nice"), "19", log);
        assert.equal(read(dir, "side.io"), "idle", "the lowered task kept its I/O class");
        assert.equal(
          read(dir, "late.nice"),
          "19",
          `a task that started later was not lowered\n${log}`,
        );
        assert.deepEqual(lines(log), [
          "nx-precedence: lead:work ran ahead of late:work pause:work side:work, keeping kept:work",
        ]);
        assert.deepEqual(leftIn(dir), [], "the wrapper's watcher outlived its command");
      } else {
        // Without /proc there is nothing to tell the run's tasks apart by, so the
        // command simply runs and nothing is lowered.
        assert.equal(read(dir, "side.nice"), "0", log);
        assert.deepEqual(lines(log), []);
      }
    });
  }

  const failures = [
    {
      // 1 and 2 are grep's own answers here (no match; a file it could not read), so
      // the stand-in fails as grep itself failing does.
      tool: "grep",
      code: 3,
      warning:
        "nx-precedence: cannot read which processes are this run's (grep exited 3), so the tasks beside lead:work keep their priority until it can",
      side: "0",
      summary: "nx-precedence: lead:work ran ahead of no other task, keeping kept:work",
    },
    {
      tool: "renice",
      warning: "nx-precedence: cannot lower the tasks beside lead:work (renice refused here)",
      side: "0",
      summary: "nx-precedence: lead:work ran ahead of no other task, keeping kept:work",
    },
    {
      tool: "ionice",
      warning:
        "nx-precedence: cannot move the tasks beside lead:work to the idle I/O class (ionice refused here)",
      side: "19",
      summary:
        "nx-precedence: lead:work ran ahead of late:work pause:work side:work, keeping kept:work",
    },
    {
      tool: "ps",
      warning:
        "nx-precedence: cannot list processes (ps refused here), so the tasks beside lead:work keep their priority until it can",
      side: "0",
      summary: "nx-precedence: lead:work ran ahead of no other task, keeping kept:work",
    },
  ];
  for (const { tool, code = 1, warning, side, summary } of failures) {
    it(`says once when ${tool} fails, and its command still runs to its end`, (t) => {
      const dir = workspace(t);
      const stubs = failing(t, { [tool]: `${tool} refused here` }, Number.POSITIVE_INFINITY, code);
      const { status, log } = nx(dir, { path: `${stubs}:${process.env.PATH}` });
      assert.equal(status, 0, log);
      assert.ok(existsSync(join(dir, "done")), log);
      assert.equal(read(dir, "lead.nice"), "0", log);
      if (!linux) {
        assert.deepEqual(lines(log), []);
        return;
      }
      assert.equal(read(dir, "side.nice"), side, log);
      assert.deepEqual(lines(log), [warning, summary]);
      assert.equal(log.split("\n").filter((line) => line.startsWith("ACTION: ")).length, 1, log);
    });
  }

  it("lowers the run once ps answers again after refusing", (t) => {
    const dir = workspace(t);
    const stubs = failing(t, { ps: "ps refused here" }, 1);
    const { status, log } = nx(dir, { path: `${stubs}:${process.env.PATH}` });
    assert.equal(status, 0, log);
    assert.equal(read(dir, "lead.nice"), "0", log);
    if (!linux) return assert.deepEqual(lines(log), []);
    assert.equal(read(dir, "side.nice"), "19", log);
    assert.equal(read(dir, "late.nice"), "19", log);
    const [warning, summary, ...rest] = lines(log);
    assert.match(warning, /cannot list processes \(ps refused here\)/);
    assert.match(
      summary,
      /^nx-precedence: lead:work ran ahead of .*late:work .*side:work, keeping kept:work$/,
    );
    assert.deepEqual(rest, []);
  });

  it("names no failure for processes that ended before it lowered them", (t) => {
    const dir = workspace(t, { churn: true });
    const { status, log } = nx(dir);
    assert.equal(status, 0, log);
    if (!linux) return assert.deepEqual(lines(log), []);
    assert.equal(read(dir, "side.nice"), "19", log);
    assert.deepEqual(lines(log), [
      "nx-precedence: lead:work ran ahead of churn:work late:work pause:work side:work, keeping kept:work",
    ]);
  });

  it("still lowers the CPU priority where there is no ionice, and says so once", (t) => {
    const dir = workspace(t);
    const tools = ["node", "bash", "sh", "env", "git", "ps", "grep", "cut", "renice", "mktemp"];
    tools.push("sort", "paste", "rm", "awk", "sleep", "seq", "cat", "mv", "touch", "true");
    const { status, log } = nx(dir, { path: only(t, tools) });
    assert.equal(status, 0, log);
    if (!linux) return assert.deepEqual(lines(log), []);
    assert.equal(read(dir, "side.nice"), "19", log);
    assert.notEqual(read(dir, "side.io"), "idle", log);
    assert.deepEqual(lines(log), [
      "nx-precedence: found no ionice, so the tasks beside lead:work keep their I/O priority",
      "nx-precedence: lead:work ran ahead of late:work pause:work side:work, keeping kept:work",
    ]);
  });

  it("exits with its command's status, outside an Nx task as inside one", () => {
    const env = { ...process.env, NX_TASK_TARGET_PROJECT: "", NX_TASK_TARGET_TARGET: "" };
    const outside = spawnSync("bash", [script, "--", "bash", "-c", "exit 4"], {
      encoding: "utf8",
      env,
    });
    assert.equal(outside.status, 4, outside.stderr);
    assert.equal(outside.stderr, "", "a command outside an Nx task was told about a run");
    const inside = spawnSync("bash", [script, "--", "bash", "-c", "echo ran; exit 3"], {
      encoding: "utf8",
      env: {
        ...env,
        NX_TASK_TARGET_PROJECT: "lead",
        NX_TASK_TARGET_TARGET: "work",
        NX_INVOCATION_ROOT_PID: String(process.pid),
      },
    });
    assert.equal(inside.status, 3, inside.stderr);
    assert.equal(inside.stdout, "ran\n");
    // A root that is not a pid names no run, so nothing could be scoped to it.
    const malformed = spawnSync("bash", [script, "--", "bash", "-c", "exit 5"], {
      encoding: "utf8",
      env: {
        ...env,
        NX_TASK_TARGET_PROJECT: "lead",
        NX_TASK_TARGET_TARGET: "work",
        NX_INVOCATION_ROOT_PID: ".*",
      },
    });
    assert.equal(malformed.status, 5, malformed.stderr);
    assert.equal(malformed.stderr, "", "a run with no valid root was treated as one");
  });

  for (const [args, message] of [
    [[], /name the command to run after --/],
    [
      ["--keep", "install-smoke", "--", "true"],
      /--keep needs one PROJECT:TARGET, not 'install-smoke'/,
    ],
    [["--keep", "a:b c:d", "--", "true"], /--keep needs one PROJECT:TARGET, not 'a:b c:d'/],
    [["--first", "--", "true"], /unknown argument '--first'/],
  ]) {
    it(`refuses ${JSON.stringify(args)} and runs nothing`, () => {
      const { status, stdout, stderr } = spawnSync("bash", [script, ...args], { encoding: "utf8" });
      assert.equal(status, 2, stderr);
      assert.equal(stdout, "");
      assert.match(stderr, message);
      assert.match(stderr, /^ACTION: /m);
    });
  }
});
