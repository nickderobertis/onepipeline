// `just gate-profile` as a user runs it, over the fixture workspace (fixture.mjs) with real just,
// Nx and cargo: the unprofiled gate is untouched by it, and a profiled run reports every phase,
// task, cargo build and compile unit with its CPU and load, and its critical path.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { validateReport } from "./fold.mjs";
import {
  cargoUnits,
  filesUnder,
  fixture,
  fixtureEnvironment,
  GRANDCHILD_CPU_US,
  grandchildReports,
  just,
  markerLines,
} from "./fixture.mjs";

const runDirs = (dir) =>
  existsSync(join(dir, "target/gate-profile")) ? readdirSync(join(dir, "target/gate-profile")) : [];

const readReport = (dir, run) =>
  JSON.parse(readFileSync(join(dir, "target/gate-profile", run, "report.json"), "utf8"));

const sum = (rows) => rows.reduce((total, row) => total + row.cpu_s, 0);

test("gate-profile profiles the gate run it makes, and the gate without it records nothing", async (t) => {
  const { dir, commit } = fixture(t);

  await t.test("the gate without the command writes no profile of any kind", () => {
    const gate = just(dir, ["gate"]);
    assert.equal(gate.status, 0, gate.stderr);
    assert.equal(markerLines(dir), 1);
    assert.deepEqual(runDirs(dir), []);
    const written = filesUnder(dir);
    assert.deepEqual(
      written.filter((path) =>
        /cargo-timing|\.profile\.json$|gate-profile\/.*\.jsonl?$/.test(path),
      ),
      [],
    );
    assert.doesNotMatch(readFileSync(join(dir, ".logs/nx.log"), "utf8"), /Performance Profile/);
    // The marker task is now in the Nx cache: running it again replays it and appends nothing.
    const replay = spawnSync("bash", ["scripts/nx.sh", "run", "work:marker"], {
      cwd: dir,
      encoding: "utf8",
      env: fixtureEnvironment(),
    });
    assert.equal(replay.status, 0, replay.stderr);
    assert.equal(markerLines(dir), 1);
  });

  // So that cargo compiles every unit again rather than finding them fresh.
  rmSync(join(dir, "crate/target"), { recursive: true, force: true });

  // Started inside an Nx run that skips its cache, as `nx run --skip-nx-cache` hands its tasks:
  // the profiled run still replays within itself (the replayed build below).
  const profiled = just(dir, ["gate-profile"], {
    NX_SKIP_NX_CACHE: "true",
    NX_DISABLE_NX_CACHE: "true",
  });
  const [run] = runDirs(dir);
  const report = run ? readReport(dir, run) : null;

  await t.test("the command runs the gate and writes both reports where it says", () => {
    assert.equal(profiled.status, 0, profiled.stderr);
    assert.equal(runDirs(dir).length, 1);
    assert.match(run, /^\d{8}T\d{6}Z$/);
    assert.match(profiled.stdout, new RegExp(`target/gate-profile/${run}/report\\.md`));
    assert.ok(existsSync(join(dir, "target/gate-profile", run, "report.md")));
    assert.deepEqual(validateReport(report), []);
    assert.equal(report.recipe, "gate");
    assert.deepEqual(report.commit, { sha: commit, dirty: false });
    const markdown = readFileSync(join(dir, "target/gate-profile", run, "report.md"), "utf8");
    for (const heading of ["Whole run", "Phases", "Critical path", "Nx tasks", "Cargo invocations"])
      assert.match(markdown, new RegExp(`## ${heading}`));
    assert.match(markdown, new RegExp(commit));
  });

  await t.test("the cache is skipped: the cached marker task ran again and is charged CPU", () => {
    assert.equal(markerLines(dir), 2);
    const marker = report.tasks.find((task) => task.task === "work:marker");
    assert.equal(marker.status, "success");
    assert.ok(marker.cpu_s >= 0.15, `work:marker charged ${marker.cpu_s}s`);
  });

  await t.test("phases are the recipes in the order they ran, one after another", () => {
    assert.deepEqual(
      report.phases.map((phase) => phase.recipe),
      ["build", "_gate-body"],
    );
    for (let i = 1; i < report.phases.length; i++)
      assert.ok(
        Date.parse(report.phases[i].start) >= Date.parse(report.phases[i - 1].end),
        `${report.phases[i].recipe} starts before ${report.phases[i - 1].recipe} ends`,
      );
    for (const phase of report.phases) assert.equal(phase.invocations.length, 1);
  });

  await t.test("CPU nests: the run holds its phases, each phase its tasks", () => {
    assert.ok(report.run.cpu_s > 0 && report.run.wall_s > 0);
    assert.ok(report.run.cpu_s >= sum(report.phases), JSON.stringify(report.run));
    for (const phase of report.phases) {
      const tasks = report.tasks.filter((task) => task.phase === phase.id);
      assert.ok(tasks.length > 0, `no tasks in ${phase.recipe}`);
      assert.ok(phase.cpu_s >= sum(tasks), `${phase.recipe}: ${phase.cpu_s}s < its tasks'`);
    }
  });

  await t.test("the task whose grandchild did the work is charged it, and no other task is", () => {
    const reports = grandchildReports(dir).filter(
      (r) => r.at >= Date.parse(report.run.start) && r.at <= Date.parse(report.run.end),
    );
    assert.equal(reports.length, 1);
    const grandchild = reports[0].cpu_us / 1e6;
    assert.ok(grandchild >= GRANDCHILD_CPU_US / 1e6);
    const tree = report.tasks.find((task) => task.task === "work:tree");
    assert.ok(tree.cpu_s >= grandchild, `work:tree charged ${tree.cpu_s}s of ${grandchild}s`);
    for (const other of report.tasks.filter((task) => task.task !== "work:tree"))
      assert.ok(other.cpu_s < grandchild, `${other.task} charged ${other.cpu_s}s`);
  });

  await t.test("the critical path is the whole run's chain, across both invocations", () => {
    const path = report.critical_path;
    const order = report.phases.flatMap((phase) => phase.invocations);
    assert.deepEqual([...new Set(path.tasks.map((task) => task.invocation))], order);
    // The second invocation's chain starts at the build the first one ran, replayed from this
    // run's own cache rather than built twice.
    assert.deepEqual(
      path.tasks.map((task) => [order.indexOf(task.invocation), task.task]),
      [
        [0, "crate:build"],
        [1, "crate:build"],
        [1, "work:marker"],
        [1, "work:tree"],
        [1, "work:check"],
      ],
    );
    const replay = report.tasks.find(
      (task) => task.invocation === order[1] && task.task === "crate:build",
    );
    assert.match(replay.status, /cache/);
    assert.equal(path.complete, true);
    assert.ok(path.wall_s <= path.run_wall_s, `${path.wall_s}s > ${path.run_wall_s}s`);
    assert.equal(path.run_wall_s, report.run.wall_s);
  });

  await t.test("every unit of the crate appears with its compile time and load", () => {
    assert.equal(report.cargo.length, 1);
    const [build] = report.cargo;
    assert.equal(build.task, "crate:build");
    assert.equal(build.exit, 0);
    assert.ok(build.cpu_s > 0);
    const expected = cargoUnits(dir);
    assert.ok(expected.length >= 5, JSON.stringify(expected));
    for (const unit of expected) {
      const found = build.units.find(
        (u) =>
          u.name === unit.name &&
          (unit.reason === "build-script-executed"
            ? u.mode === "run-custom-build"
            : u.mode !== "run-custom-build" &&
              (unit.kind === "custom-build"
                ? u.target === "build-script"
                : unit.kind === "bin"
                  ? u.target.includes("bin")
                  : u.target === "")),
      );
      assert.ok(found, `no unit for ${JSON.stringify(unit)} in ${JSON.stringify(build.units)}`);
      assert.equal(typeof found.wall_s, "number");
      assert.equal(typeof found.load, "number");
    }
    const walls = build.units.map((u) => u.wall_s);
    assert.deepEqual(
      walls,
      [...walls].sort((a, b) => b - a),
    );
  });

  await t.test("every row carries the load it was taken under", () => {
    assert.equal(typeof report.host.nproc, "number");
    for (const row of [report.run, ...report.phases, ...report.tasks, ...report.cargo])
      assert.equal(typeof row.load, "number", JSON.stringify(row));
  });
});

test("gate-profile refuses an unknown recipe, and reports a failing one up to its failure", async (t) => {
  const { dir } = fixture(t);

  const unknown = just(dir, ["gate-profile", "nosuch"]);
  assert.equal(unknown.status, 2);
  assert.match(unknown.stderr, /no recipe 'nosuch'/);
  assert.deepEqual(runDirs(dir), []);

  // An uncommitted change is named with the commit it sits on.
  writeFileSync(join(dir, "uncommitted.txt"), "a change\n");
  // Settings of a profiled run this one is started inside reach none of this run's records.
  const broken = just(dir, ["gate-profile", "broken"], {
    ONEPIPELINE_GATE_PROFILE_DIR: join(dir, "not-this-run"),
    ONEPIPELINE_GATE_PROFILE_PHASE: "an-enclosing-phase",
    NX_PROFILE: "an-enclosing-profile.json",
  });
  assert.equal(broken.status, 1, broken.stderr);
  assert.match(broken.stderr, /`just broken` failed \(exit 7\)/);
  const [run] = runDirs(dir);
  const report = readReport(dir, run);
  assert.deepEqual(validateReport(report), []);
  assert.equal(report.run.exit, 7);
  assert.deepEqual(
    report.phases.map((phase) => [phase.recipe, phase.exit]),
    [["broken", 7]],
  );
  assert.deepEqual(report.tasks, []);
  assert.deepEqual(report.phases[0].within, []);
  assert.equal(report.commit.dirty, true);
  rmSync(join(dir, "uncommitted.txt"));

  const missing = just(dir, ["gate-profile", "missing"]);
  assert.equal(missing.status, 1, missing.stderr);
  assert.match(missing.stderr, /could not read the task graph of `nx run crate:nosuch`/);
  const after = readReport(
    dir,
    runDirs(dir).find((r) => r !== run),
  );
  assert.deepEqual(validateReport(after), []);
  assert.equal(after.invocations.length, 1);
  assert.equal(after.critical_path.complete, false);
  assert.match(after.notes.join("\n"), /Nx wrote no task profile for invocation/);
});

test("an interrupted run still writes its profile of what ran", async (t) => {
  const { dir } = fixture(t);
  const child = spawn("just", ["gate-profile", "slow"], {
    cwd: dir,
    env: fixtureEnvironment(),
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });
  const exited = new Promise((resolve) => child.on("exit", resolve));
  // Interrupt once the recipe is under way: its run directory exists and the phase has started.
  while (
    !runDirs(dir).some((run) => existsSync(join(dir, "target/gate-profile", run, "meta.json")))
  )
    await new Promise((resolve) => setTimeout(resolve, 100));
  await new Promise((resolve) => setTimeout(resolve, 1500));
  // The terminal's Ctrl-C: the whole foreground group, this test's own child and its descendants.
  process.kill(-child.pid, "SIGINT");
  assert.equal(await exited, 1, stderr);
  assert.match(stderr, /`just slow` failed \(exit 130\)/);
  const report = readReport(dir, runDirs(dir)[0]);
  assert.deepEqual(validateReport(report), []);
  assert.equal(report.run.exit, 130);
  assert.deepEqual(
    report.phases.map((phase) => [phase.recipe, phase.exit]),
    [["slow", 130]],
  );
});

test("gate-profile exits 3, saying why, when it cannot record or fold the run", async (t) => {
  const { dir } = fixture(t);
  const own = (...path) => join(dir, "gate-profile", ...path);
  const swap = async (path, content, body) => {
    const kept = readFileSync(path);
    writeFileSync(path, content);
    try {
      await body();
    } finally {
      writeFileSync(path, kept);
    }
  };
  const refused = (pattern, args = ["gate-profile", "broken"]) => {
    const result = just(dir, args);
    assert.equal(result.status, 3, result.stderr);
    assert.match(result.stderr, pattern);
  };

  await t.test("a sampler that never starts", () =>
    swap(own("sampler.mjs"), "process.exit(4);\n", () =>
      refused(/the process sampler did not start \(exit 4\)/),
    ),
  );
  await t.test("a sampler that fails", () =>
    swap(
      own("sampler.mjs"),
      'console.log("ready");\nprocess.stdin.on("data", () => {});\nprocess.stdin.on("end", () => process.exit(3));\n',
      () => refused(/the process sampler failed \(exit 3\)/),
    ),
  );
  await t.test("a run that records nothing", () =>
    swap(own("shims", "just"), "#!/bin/sh\nexit 0\n", () =>
      refused(/`just broken` ended \(exit 0\) before its run was recorded/),
    ),
  );
  await t.test("a recipe that cannot be started", () => {
    chmodSync(own("shims", "just"), 0o644);
    try {
      refused(/could not start `just broken`/);
    } finally {
      chmodSync(own("shims", "just"), 0o755);
    }
  });
  await t.test("a record the fold cannot read, named with how to fold it again", () => {
    const result = just(dir, ["gate-profile", "garbled"]);
    assert.equal(result.status, 3, result.stderr);
    assert.match(result.stderr, /invocation record x\.invocation\.json is unreadable/);
    assert.match(result.stderr, /invocation x .* is not JSON/);
    assert.match(
      result.stderr,
      /fold them again with `node gate-profile\/fold\.mjs target\/gate-profile\//,
    );
  });
});

test("a workspace outside git is profiled with no commit to name", (t) => {
  const { dir } = fixture(t);
  rmSync(join(dir, ".git"), { recursive: true, force: true });
  // Nor may git find a repository around the scratch directory the fixture sits in.
  const result = just(dir, ["gate-profile", "broken"], { GIT_CEILING_DIRECTORIES: dirname(dir) });
  assert.equal(result.status, 1, result.stderr);
  assert.deepEqual(readReport(dir, runDirs(dir)[0]).commit, { sha: null, dirty: false });
  assert.match(
    readFileSync(join(dir, "target/gate-profile", runDirs(dir)[0], "report.md"), "utf8"),
    /Commit `unknown`/,
  );
});

test("a second run within the same second gets a directory of its own", (t) => {
  const { dir } = fixture(t);
  // Every stamp the next run could take is taken already.
  const now = Date.now();
  const taken = [];
  for (let s = -1; s < 30; s++) {
    const stamp = new Date(now + s * 1000)
      .toISOString()
      .replace(/[-:]/g, "")
      .replace(/\.\d+Z$/, "Z");
    mkdirSync(join(dir, "target/gate-profile", stamp), { recursive: true });
    taken.push(stamp);
  }
  const result = just(dir, ["gate-profile", "broken"]);
  assert.equal(result.status, 1, result.stderr);
  const fresh = runDirs(dir).filter((run) => !taken.includes(run));
  assert.equal(fresh.length, 1);
  assert.match(fresh[0], /^\d{8}T\d{6}Z-2$/);
  assert.ok(taken.includes(fresh[0].slice(0, -2)));
  for (const stamp of taken)
    assert.deepEqual(readdirSync(join(dir, "target/gate-profile", stamp)), []);
  assert.equal(readReport(dir, fresh[0]).run.exit, 7);
});

test("the sampler refuses a malformed invocation, and a run whose samples it cannot write", async (t) => {
  const { dir } = fixture(t);
  const sampler = join(dir, "gate-profile", "sampler.mjs");
  for (const args of [[], ["out.json", "run", "", "100", "extra"], ["out.json", "run", "", "5"]]) {
    const usage = spawnSync(process.execPath, [sampler, ...args], { encoding: "utf8" });
    assert.equal(usage.status, 2, args.join(" "));
    assert.match(usage.stderr, /usage: sampler\.mjs/);
  }
  const unwritable = join(dir, "no", "such", "dir", "processes.json");
  const child = spawn(process.execPath, [sampler, unwritable, "run", "", "100"], {
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });
  await new Promise((resolve) => child.stdout.once("data", resolve));
  const exited = new Promise((resolve) => child.on("exit", resolve));
  child.stdin.end();
  assert.equal(await exited, 3);
  assert.match(stderr, /cannot write the samples to .*processes\.json/);
});
