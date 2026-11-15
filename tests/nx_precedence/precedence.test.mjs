// `scripts/nx-precedence.sh`, which the e2e tier's coverage run goes through now that it
// runs beside the Linux job's other tasks: driven inside a real Nx run of three tasks
// started together, where the leading task must keep its CPU priority, one other task
// must drop to the lowest, and a `--keep` task must keep its own. A process outside the
// run, carrying the same task's name, stands for another Nx run on the same host.
//
// This project's `test` runs alone (`parallelism: false`): beside the e2e tier its own
// processes would already be at the lowest priority, and the run it starts would
// inherit that, leaving nothing for the wrapper to lower.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
// biome-ignore lint/suspicious/noControlCharactersInRegex: the escape is what is stripped.
const plain = (text) => text.replace(/\u001b\[[0-9;]*[A-Za-z]/g, "");

const script = join(root, "scripts", "nx-precedence.sh");

// Each side task records its own niceness every 0.2s until the leading task is done.
const recorder = (name) =>
  `bash -c 'for i in $(seq 300); do [ -e done ] && break; ps -o ni= -p $$ > ${name}.tmp && mv ${name}.tmp ${name}.nice; sleep 0.2; done'`;

// The leading task waits until both others have recorded once, then long enough for
// the wrapper's once-a-second pass to have reached them twice, and records its own.
const lead = `bash ${script} --keep kept:work -- bash -c 'for i in $(seq 300); do [ -e side.nice ] && [ -e kept.nice ] && break; sleep 0.1; done; sleep 3; ps -o ni= -p $$ > lead.nice; touch done'`;

function workspace(t) {
  const dir = mkdtempSync(join(tmpdir(), "nx-precedence-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  writeFileSync(join(dir, "package.json"), '{"name":"precedence","private":true}\n');
  writeFileSync(join(dir, "nx.json"), "{}\n");
  symlinkSync(join(root, "node_modules"), join(dir, "node_modules"));
  for (const [name, command] of [
    ["lead", lead],
    ["side", recorder("side")],
    ["kept", recorder("kept")],
  ]) {
    mkdirSync(join(dir, name));
    writeFileSync(
      join(dir, name, "project.json"),
      JSON.stringify({ name, targets: { work: { command, cache: false } } }),
    );
  }
  return dir;
}

const niceness = (dir, name) => readFileSync(join(dir, `${name}.nice`), "utf8").trim();

describe("the e2e tier's precedence over the tasks beside it", () => {
  for (const direct of ["true", "false"]) {
    it(`lowers the run's other tasks and nothing else (NX_RUN_COMMANDS_DIRECTLY=${direct})`, (t) => {
      const dir = workspace(t);
      const foreign = spawn("bash", ["-c", "sleep 30"], {
        env: { ...process.env, NX_TASK_TARGET_PROJECT: "side", NX_TASK_TARGET_TARGET: "work" },
        stdio: "ignore",
      });
      t.after(() => foreign.kill());
      const { status, stdout, stderr } = spawnSync(
        "node_modules/.bin/nx",
        [
          "run-many",
          "-t",
          "work",
          "--parallel=3",
          "--skip-nx-cache",
          "--outputStyle=stream-without-prefixes",
        ],
        {
          cwd: dir,
          encoding: "utf8",
          timeout: 120_000,
          env: {
            ...process.env,
            NX_DAEMON: "false",
            NX_NO_CLOUD: "true",
            NX_RUN_COMMANDS_DIRECTLY: direct,
            NX_TASK_TARGET_PROJECT: "",
            NX_TASK_TARGET_TARGET: "",
          },
        },
      );
      const log = plain(`${stdout}\n${stderr}`);
      assert.equal(status, 0, log);
      const foreignNice = spawnSync("ps", ["-o", "ni=", "-p", String(foreign.pid)], {
        encoding: "utf8",
      });
      assert.equal(foreignNice.stdout.trim(), "0", "a process of no task of this run was lowered");
      assert.equal(niceness(dir, "lead"), "0", log);
      assert.equal(niceness(dir, "kept"), "0", `the --keep task was lowered\n${log}`);
      if (process.platform === "linux") {
        assert.equal(niceness(dir, "side"), "19", log);
        assert.match(
          log,
          /nx-precedence: lead:work takes the CPU before the other tasks of its Nx run but kept:work/,
        );
        assert.match(log, /nx-precedence: side:work runs at the lowest priority beside lead:work/);
        assert.doesNotMatch(log, /kept:work runs at the lowest priority/);
      } else {
        // Without /proc there is nothing to tell the run's tasks apart by, so the
        // command simply runs and nothing is lowered.
        assert.equal(niceness(dir, "side"), "0", log);
        assert.doesNotMatch(log, /nx-precedence:/);
      }
      assert.ok(existsSync(join(dir, "done")), log);
    });
  }

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
      env: { ...env, NX_TASK_TARGET_PROJECT: "lead", NX_TASK_TARGET_TARGET: "work" },
    });
    assert.equal(inside.status, 3, inside.stderr);
    assert.equal(inside.stdout, "ran\n");
  });

  for (const [args, message] of [
    [[], /name the command to run after --/],
    [
      ["--keep", "install-smoke", "--", "true"],
      /--keep needs a PROJECT:TARGET, not 'install-smoke'/,
    ],
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
