// The install proofs (`onepipeline:install-smoke`, `cargo install --path .` of this
// revision, and `onepipeline:install-documented`, README.md's `cargo install --git`
// one-liner) run where they can catch a regression: on a pull request, Windows and
// macOS run `install-smoke`, and every test job `install-documented`, only when the
// change reaches what that install reads beyond the code; Linux runs `install-smoke`
// whenever the crate is affected, for the `host-view-time` budget it times; and every
// push to main that affects the crate runs the documented install in `timed`.
//
// Selection is driven through `scripts/ci-job.sh <job> --plan` over real commits in a
// scratch copy of this workspace: Nx's affected answer, `nx-version-free-selection.mjs`'s
// drop, and the per-job narrowing `ci-job.sh` applies, as CI asks it. The files that
// must select each proof are read from its selection project's own list, so that list
// and the selection cannot disagree without this failing.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";
import { parse } from "yaml";
import { runGraph, triggered } from "../workflow-graph.mjs";
import { command, root, workspace } from "./support/cache-workspace.mjs";

const SMOKE = "onepipeline:install-smoke";
const DOCUMENTED = "onepipeline:install-documented";
const PROOFS = [SMOKE, DOCUMENTED];
const JOBS = ["test-linux", "test-windows", "test-macos"];
const CROSS = ["test-windows", "test-macos"];
// The commit this change was made on: what each job selected before the proofs were
// narrowed.
const BEFORE = "0f7898d9";
const ENV = { CI: "true", NX_DAEMON: "false", NX_ISOLATE_PLUGINS: "false", GITHUB_BASE_REF: "" };

const selection = (path, input) =>
  JSON.parse(readFileSync(join(root, path), "utf8")).namedInputs[input].map((entry) => {
    assert.ok(entry.startsWith("{workspaceRoot}/"), `${path}'s ${input} holds ${entry}, which this cannot edit alone`);
    return entry.slice("{workspaceRoot}/".length);
  });
const SMOKE_FILES = selection("install/smoke/project.json", "installSmokeSelection");
const DOCUMENTED_FILES = selection("install/documented/project.json", "installDocumentedSelection");

const tracked = (dir) =>
  execFileSync("git", ["ls-files", "-z"], { cwd: dir, encoding: "utf8" }).split("\0").filter(Boolean);
// A file the crate owns under `prefix` (not a nested project's), preferring its code,
// and not a Cargo manifest, whose edit is a dependency edit of its own.
function under(dir, prefix) {
  const files = tracked(dir).filter((file) => file.startsWith(prefix) && !file.endsWith("Cargo.toml"));
  const owned = files.filter((file) => !tracked(dir).some((other) => other.endsWith("/project.json") && file.startsWith(other.slice(0, -"project.json".length))));
  assert.ok(owned.length, `the crate owns nothing tracked under ${prefix}`);
  return owned.find((file) => file.endsWith(".rs")) ?? owned.find((file) => !/\.(json|md)$/.test(file)) ?? owned[0];
}

// An edit that leaves the file what it was to every reader but its bytes. In the crate's
// project.json it is the install targets' own definition that is edited.
function touch(dir, path) {
  const file = join(dir, path);
  const text = readFileSync(file, "utf8");
  if (path === "project.json") {
    const project = JSON.parse(text);
    for (const target of ["install-smoke", "install-documented"]) project.targets[target]["//journey"] = "edited";
    writeFileSync(file, `${JSON.stringify(project, null, 2)}\n`);
  } else if (path.endsWith(".json")) {
    writeFileSync(file, `${JSON.stringify({ ...JSON.parse(text), "//journey": "edited" }, null, 2)}\n`);
  } else if (/\.(mjs|js|rs)$/.test(path)) writeFileSync(file, `${text}\n// edited by the install journey\n`);
  else writeFileSync(file, `${text}\n# edited by the install journey\n`);
}

function edit(dir, path, from, to) {
  const file = join(dir, path);
  const text = readFileSync(file, "utf8");
  const edited = text.replace(from, to);
  assert.notEqual(edited, text, `the edit found nothing to change in ${path}`);
  writeFileSync(file, edited);
}

// What a release pull request does to the manifests: the crate's version, and nothing else.
function moveVersion(dir) {
  const old = readFileSync(join(dir, "Cargo.toml"), "utf8").match(/\[package\][\s\S]*?\nversion = "([^"]+)"/)[1];
  const [major, minor] = old.split(".").map(Number);
  const next = `${major}.${minor + 1}.0`;
  edit(dir, "Cargo.toml", /(\[package\][\s\S]*?\nversion = ")[^"]+"/, `$1${next}"`);
  edit(dir, "Cargo.lock", `name = "onepipeline"\nversion = "${old}"`, `name = "onepipeline"\nversion = "${next}"`);
}

// A registry dependency's locked version moved, as `cargo update -p` moves one.
const moveDependency = (dir) =>
  edit(dir, "Cargo.lock", /(\[\[package\]\]\nname = "[^"]+"\nversion = ")([^"]+)("\nsource = "registry)/, (_, head, version, tail) =>
    `${head}${version.replace(/\d+$/, (n) => String(Number(n) + 1))}${tail}`);
const addDependency = (dir) => edit(dir, "Cargo.toml", /\n\[dependencies\]\n/, '\n[dependencies]\njourney-added = "1.0.0"\n');

function commit(ws, label, change) {
  const base = ws.git("rev-parse", "HEAD");
  change(ws.dir);
  ws.git("add", "-A");
  ws.git("-c", "user.name=Install journey", "-c", "user.email=install@example.invalid", "commit", "-qm", label);
  return base;
}

// What each job plans for the commit `change` makes on top of HEAD, against the commit before.
async function plan(ws, label, change, jobs = JOBS, env = { ONEPIPELINE_NX_BASE_SHA: commit(ws, label, change) }) {
  const planned = {};
  for (const job of jobs) {
    const result = await command(ws.dir, "bash", ["scripts/ci-job.sh", job, "--plan"], { ...ENV, ...env });
    assert.equal(result.code, 0, `${label}: ${result.stdout}${result.stderr}`);
    const line = result.stdout.split("\n").find((each) => each.startsWith(`ci-job: ${job} runs `));
    assert.ok(line, `${label}: ${job} printed no selection: ${result.stdout}`);
    const tasks = line.split("task(s): ")[1].trim();
    planned[job] = new Set(tasks === "none" ? [] : tasks.split(" "));
  }
  return planned;
}

const selects = (planned, jobs, proof, label) => {
  for (const job of jobs) assert.ok(planned[job].has(proof), `${label} does not select ${proof} in ${job}`);
};
const skips = (planned, jobs, proof, label) => {
  for (const job of jobs) assert.ok(!planned[job].has(proof), `${label} selects ${proof} in ${job}`);
};

describe("a pull request's install proofs", () => {
  it("are left off Windows and macOS for a source-only change, which leaves every other target as it was", { timeout: 600000 }, async () => {
    const before = workspace(BEFORE);
    const after = workspace();
    try {
      for (const prefix of ["src/", "crates/", "schemas/", "tests/"]) {
        const file = under(after.dir, prefix);
        const change = (dir) => touch(dir, file);
        const then = await plan(before, file, change);
        const now = await plan(after, file, change);
        skips(now, CROSS, SMOKE, file);
        skips(now, JOBS, DOCUMENTED, file);
        selects(now, ["test-linux"], SMOKE, file);
        for (const proof of PROOFS) assert.ok(then["test-windows"].has(proof), `the base this compares with did not run ${proof} on Windows for ${file}`);
        for (const job of JOBS)
          for (const task of then[job])
            if (!PROOFS.includes(task) || (job === "test-linux" && task === SMOKE))
              assert.ok(now[job].has(task), `${job} selected ${task} for ${file} before the install proofs were narrowed, and no longer does`);
      }
    } finally {
      before.close();
      after.close();
    }
  });

  it("run install-smoke on Windows and macOS for each file that decides the install beyond the code", { timeout: 600000 }, async () => {
    const ws = workspace();
    try {
      for (const path of SMOKE_FILES) selects(await plan(ws, path, (dir) => touch(dir, path), CROSS), CROSS, SMOKE, path);
      selects(await plan(ws, "a moved dependency", moveDependency, CROSS), CROSS, SMOKE, "a dependency moved in Cargo.lock");
      selects(await plan(ws, "an added dependency", addDependency, CROSS), CROSS, SMOKE, "a dependency added to Cargo.toml");
      for (const path of ["scripts/install-proof.sh", "scripts/smoke-published.sh", "rust-toolchain.toml", ".cargo/config.toml", "project.json"])
        assert.ok(SMOKE_FILES.includes(path), `installSmokeSelection no longer names ${path}`);
    } finally {
      ws.close();
    }
  });

  it("leave install-smoke off Windows and macOS for a version-only release", { timeout: 600000 }, async () => {
    const ws = workspace();
    try {
      const planned = await plan(ws, "a version-only release", moveVersion);
      skips(planned, CROSS, SMOKE, "a version-only move in Cargo.toml and Cargo.lock");
      skips(planned, JOBS, DOCUMENTED, "a version-only move in Cargo.toml and Cargo.lock");
    } finally {
      ws.close();
    }
  });

  it("run install-documented on every platform for each file the documented install reads", { timeout: 600000 }, async () => {
    const ws = workspace();
    try {
      for (const path of DOCUMENTED_FILES) selects(await plan(ws, path, (dir) => touch(dir, path)), JOBS, DOCUMENTED, path);
      for (const path of ["README.md", "scripts/install-proof.sh", "rust-toolchain.toml", "project.json"])
        assert.ok(DOCUMENTED_FILES.includes(path), `installDocumentedSelection no longer names ${path}`);
    } finally {
      ws.close();
    }
  });

  it("run both on every platform when no base can be derived", { timeout: 600000 }, async () => {
    const ws = workspace();
    try {
      commit(ws, "a source change", (dir) => touch(dir, "src/lib.rs"));
      const planned = await plan(ws, "no base", () => {}, JOBS, {});
      for (const proof of PROOFS) selects(planned, JOBS, proof, "a CI build with no base");
    } finally {
      ws.close();
    }
  });

  it("are uncached, so a narrowed selection can never replay an earlier pass", () => {
    const shown = execFileSync("bash", ["scripts/nx.sh", "show", "project", "onepipeline", "--json"], {
      cwd: root,
      encoding: "utf8",
      env: { ...process.env, ONEPIPELINE_NX_SHOW_OUTPUT: "1" },
      maxBuffer: 64 * 1024 * 1024,
    });
    const crate = JSON.parse(shown.slice(shown.indexOf("{")));
    for (const target of ["install-smoke", "install-documented"])
      assert.equal(crate.targets[target].cache, false, `onepipeline:${target} could replay an earlier pass`);
  });
});

describe("a push to main", () => {
  it("runs the documented install in `timed` when it affects the crate, never during a timing", { timeout: 600000 }, async () => {
    const ws = workspace();
    try {
      const planned = await plan(ws, "a merged source change", (dir) => touch(dir, "src/lib.rs"), ["timed"]);
      assert.ok(planned.timed.has(DOCUMENTED), [...planned.timed].join(" "));
      assert.ok(!planned.timed.has(SMOKE), "timed installs this revision too");
      const timings = [...planned.timed].filter((task) => task.endsWith(":timed"));
      assert.ok(timings.length, "a source change selects no timing beside the documented install");
      for (const task of timings) {
        const [project] = task.split(":");
        const shown = execFileSync("bash", ["scripts/nx.sh", "show", "project", project, "--json"], {
          cwd: ws.dir,
          encoding: "utf8",
          env: { ...process.env, ...ENV, ONEPIPELINE_NX_SHOW_OUTPUT: "1" },
          maxBuffer: 64 * 1024 * 1024,
        });
        const timed = JSON.parse(shown.slice(shown.indexOf("{"))).targets.timed;
        assert.equal(timed.parallelism, false, `${task} may run while the documented install builds`);
      }
    } finally {
      ws.close();
    }
  });

  it("runs `timed` on Linux and no Windows or macOS job", () => {
    const push = { eventName: "push", ref: "refs/heads/main", head: "0".repeat(40), event: { head_commit: { message: "feat: a change (#1)" } } };
    const workflow = parse(readFileSync(join(root, ".github/workflows/ci.yml"), "utf8"));
    assert.ok(triggered(workflow, push), "ci.yml does not run on a push to main");
    const ran = runGraph(workflow, push, root).filter((row) => row.selected);
    assert.deepEqual(ran.filter((row) => row.job === "timed").map((row) => row.runner), ["ubuntu-24.04"]);
    for (const row of ran) {
      assert.doesNotMatch(String(row.runner), /^(windows|macos)-/, `a push to main runs ${row.job} on ${row.runner}`);
      assert.doesNotMatch(row.job, /^test-(windows|macos)$/, `a push to main runs ${row.job}`);
    }
  });
});
