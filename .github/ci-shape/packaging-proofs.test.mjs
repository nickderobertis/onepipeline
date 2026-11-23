// The packaging proofs (`onepipeline-packaging`: the npm journeys that build, pack,
// install or publish the release artifacts, and the Docker-built wheel) run on a pull
// request exactly when it changes what the packages contain, and never replay a pass
// their inputs have moved past.
//
// Selection is driven through `scripts/ci-job.sh <job> --plan` over real commits in a
// scratch copy of this workspace, so it is Nx's affected answer with
// `scripts/nx-version-free-selection.mjs`'s drop applied, as CI asks it. The files that
// must select the proofs are read from their own `packagingSelection`, so that list
// and the selection cannot disagree without this failing. Hashing is Nx's own task
// hasher (`support/nx-hashes.mjs`) over the same kind of copy.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";
import { command, root, workspace } from "./support/cache-workspace.mjs";

const PROJECT = "onepipeline-packaging";
const PROOFS = [`${PROJECT}:test`, `${PROJECT}:wheel-smoke`];
// The commit this change was made on: what each job selected for a source-only change
// before the proofs had a project of their own.
const BEFORE = "55cc843c";
const declared = JSON.parse(readFileSync(join(root, "packaging/project.json"), "utf8"));
const helper = fileURLToPath(new URL("./support/nx-hashes.mjs", import.meta.url));
const ENV = { CI: "true", NX_DAEMON: "false", NX_ISOLATE_PLUGINS: "false", GITHUB_BASE_REF: "" };

const tracked = (dir) =>
  execFileSync("git", ["ls-files", "-z"], { cwd: dir, encoding: "utf8" }).split("\0").filter(Boolean);
// A tracked file under `prefix`, preferring one that is neither JSON, whose edit below
// is the more invasive, nor a Cargo manifest, which is a dependency edit of its own.
function under(dir, prefix) {
  const files = tracked(dir).filter((file) => file.startsWith(prefix));
  assert.ok(files.length, `nothing under ${prefix} is tracked`);
  return files.find((file) => !/(\.json|Cargo\.toml)$/.test(file)) ?? files[0];
}

function selectionFiles(dir) {
  const files = tracked(dir);
  return declared.namedInputs.packagingSelection.map((entry) => {
    const path = entry.replace("{workspaceRoot}/", "").replace("{projectRoot}/", "packaging/");
    if (path.endsWith("/**/*")) return under(dir, path.slice(0, -"**/*".length));
    assert.ok(files.includes(path), `packagingSelection names ${path}, which is not tracked`);
    return path;
  });
}

// An edit that leaves the file what it was to every reader but its bytes.
function touch(dir, path) {
  const file = join(dir, path);
  const text = readFileSync(file, "utf8");
  if (path.endsWith(".json")) {
    writeFileSync(file, `${JSON.stringify({ ...JSON.parse(text), "//journey": "edited" }, null, 2)}\n`);
  } else if (/\.(mjs|js|rs)$/.test(path)) writeFileSync(file, `${text}\n// edited by the packaging journey\n`);
  else writeFileSync(file, `${text}\n# edited by the packaging journey\n`);
}

const ownVersion = (dir) =>
  readFileSync(join(dir, "Cargo.toml"), "utf8").match(/\[package\][\s\S]*?\nversion = "([^"]+)"/)[1];

// What a release pull request does to the manifests: the crate's version, and nothing else.
function moveVersion(dir) {
  const old = ownVersion(dir);
  const [major, minor] = old.split(".").map(Number);
  const next = `${major}.${minor + 1}.0`;
  for (const [file, from, to] of [
    ["Cargo.toml", /(\[package\][\s\S]*?\nversion = ")[^"]+"/, `$1${next}"`],
    ["Cargo.lock", `name = "onepipeline"\nversion = "${old}"`, `name = "onepipeline"\nversion = "${next}"`],
  ]) {
    const path = join(dir, file);
    const text = readFileSync(path, "utf8");
    const edited = text.replace(from, to);
    assert.notEqual(edited, text, `the version edit found nothing to move in ${file}`);
    writeFileSync(path, edited);
  }
}

// A registry dependency's locked version moved, as `cargo update -p` moves one.
function moveDependency(dir) {
  const path = join(dir, "Cargo.lock");
  const text = readFileSync(path, "utf8");
  const edited = text.replace(
    /(\[\[package\]\]\nname = "[^"]+"\nversion = ")([^"]+)("\nsource = "registry)/,
    (_, head, version, tail) => `${head}${version.replace(/\d+$/, (n) => String(Number(n) + 1))}${tail}`,
  );
  assert.notEqual(edited, text, "Cargo.lock holds no registry dependency to move");
  writeFileSync(path, edited);
}

function addDependency(dir) {
  const path = join(dir, "Cargo.lock");
  writeFileSync(
    path,
    `${readFileSync(path, "utf8")}\n[[package]]\nname = "journey-added"\nversion = "1.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "${"0".repeat(64)}"\n`,
  );
}

function editRecipe(dir, header) {
  const path = join(dir, "justfile");
  const text = readFileSync(path, "utf8");
  assert.ok(text.includes(`\n${header}\n`), `the justfile has no recipe line '${header}'`);
  writeFileSync(path, text.replace(`\n${header}\n`, `\n${header}\n    @:\n`));
}
// A recipe `wheel-smoke` runs through scripts/install-proof.sh, and one no packaging
// recipe reaches.
const PACKAGING_RECIPE = 'wheel-linux target out="dist":';
const OTHER_RECIPE = "npm-launcher:";

// Commits `edit` on top of HEAD and returns what each job plans against the commit before.
// `env` reaches the planning run; `unanswered` expects the version-free check to fail.
async function plan(ws, label, edit, { jobs = ["test-linux"], env = {}, unanswered = false } = {}) {
  const base = commit(ws, label, edit);
  const planned = {};
  for (const job of jobs) {
    const result = await command(ws.dir, "bash", ["scripts/ci-job.sh", job, "--plan"], {
      ...ENV,
      ...env,
      ONEPIPELINE_NX_BASE_SHA: base,
    });
    assert.equal(result.code, 0, `${label}: ${result.stdout}${result.stderr}`);
    (unanswered ? assert.match : assert.doesNotMatch)(result.stderr, /could not answer/, `${label}: ${result.stderr}`);
    const line = result.stdout.split("\n").find((each) => each.startsWith(`ci-job: ${job} runs `));
    assert.ok(line, `${label}: ${job} printed no selection: ${result.stdout}`);
    const tasks = line.split("task(s): ")[1].trim();
    planned[job] = new Set(tasks === "none" ? [] : tasks.split(" "));
  }
  return planned;
}

function commit(ws, label, edit) {
  const base = ws.git("rev-parse", "HEAD");
  edit(ws.dir);
  ws.git("add", "-A");
  ws.git("-c", "user.name=Packaging journey", "-c", "user.email=packaging@example.invalid", "commit", "-qm", label);
  return base;
}

async function affectsWith(ws, label, edit, env = {}) {
  const base = commit(ws, label, edit);
  const result = await command(ws.dir, "bash", ["scripts/nx-affected.sh", "--affects", PROJECT], {
    ...ENV,
    ...env,
    ONEPIPELINE_NX_BASE_SHA: base,
  });
  assert.equal(result.code, 0, `${label}: ${result.stderr}`);
  return result;
}
const affects = async (...args) => (await affectsWith(...args)).stdout.trim();

// A `just` that cannot dump the justfile, so the recipe comparison cannot answer. It sits
// under `.git/`, so no commit carries it into the change being selected.
function brokenJust(ws) {
  const bin = join(ws.dir, ".git", "journey-bin");
  mkdirSync(bin, { recursive: true });
  writeFileSync(join(bin, "just"), "#!/usr/bin/env bash\necho 'just: broken for the journey' >&2\nexit 1\n", { mode: 0o755 });
  return { PATH: `${bin}:${process.env.PATH}` };
}

const both = (dir, ...edits) => edits.forEach((edit) => edit(dir));

const selectsBoth = (planned, label) => {
  for (const proof of PROOFS) assert.ok(planned["test-linux"].has(proof), `${label} does not select ${proof}`);
};
const selectsNeither = (planned, label) => {
  for (const [job, tasks] of Object.entries(planned))
    for (const proof of PROOFS) assert.ok(!tasks.has(proof), `${label} selects ${proof} in ${job}`);
};

describe("a pull request's packaging proofs", () => {
  it("are selected by each packaging-determining file, and by a dependency moved or added", { timeout: 600000 }, async () => {
    const ws = workspace();
    try {
      for (const path of selectionFiles(ws.dir))
        selectsBoth(await plan(ws, path, (dir) => touch(dir, path)), path);
      selectsBoth(await plan(ws, "a packaging recipe", (dir) => editRecipe(dir, PACKAGING_RECIPE)), "a packaging recipe");
      selectsBoth(await plan(ws, "a moved dependency", moveDependency), "a dependency moved in Cargo.lock");
      selectsBoth(await plan(ws, "an added dependency", addDependency), "a dependency added to Cargo.lock");
    } finally {
      ws.close();
    }
  });

  it("are not selected by the binary's sources, its tests, a version or another recipe", { timeout: 600000 }, async () => {
    const ws = workspace();
    try {
      for (const prefix of ["src/", "crates/", "tests/", "schemas/"]) {
        const file = under(ws.dir, prefix);
        selectsNeither(await plan(ws, file, (dir) => touch(dir, file)), file);
      }
      const version = await plan(ws, "a version-only release", moveVersion);
      selectsNeither(version, "a version-only move in Cargo.toml and Cargo.lock");
      selectsNeither(await plan(ws, "another recipe", (dir) => editRecipe(dir, OTHER_RECIPE)), "an edit to a recipe no package runs");
    } finally {
      ws.close();
    }
  });

  it("are still selected when a version or another recipe rides with a packaging file", { timeout: 600000 }, async () => {
    const ws = workspace();
    try {
      const packagingFile = (dir) => touch(dir, "scripts/npm-build.mjs");
      selectsBoth(await plan(ws, "version and packaging", (dir) => both(dir, moveVersion, packagingFile)), "a version move beside a packaging file");
      selectsBoth(
        await plan(ws, "recipe and packaging", (dir) => both(dir, (d) => editRecipe(d, OTHER_RECIPE), packagingFile)),
        "another recipe's edit beside a packaging file",
      );
    } finally {
      ws.close();
    }
  });

  it("keep Nx's selection when the version-free check cannot answer", { timeout: 600000 }, async () => {
    const ws = workspace();
    try {
      const planned = await plan(ws, "another recipe, unreadable", (dir) => editRecipe(dir, OTHER_RECIPE), {
        env: brokenJust(ws),
        unanswered: true,
      });
      selectsBoth(planned, "a recipe edit the check could not read");
    } finally {
      ws.close();
    }
  });

  it("are answered the same by nx-affected.sh --affects", { timeout: 600000 }, async () => {
    const ws = workspace();
    try {
      assert.equal(await affects(ws, "a version-only release", moveVersion), "false");
      assert.equal(
        await affects(ws, "version and packaging", (dir) => both(dir, moveVersion, (d) => touch(d, "scripts/npm-build.mjs"))),
        "true",
      );
      assert.equal(await affects(ws, "another recipe", (dir) => editRecipe(dir, OTHER_RECIPE)), "false");
      assert.equal(
        await affects(ws, "another recipe, unreadable", (dir) => editRecipe(dir, OTHER_RECIPE), brokenJust(ws)),
        "true",
      );
    } finally {
      ws.close();
    }
  });

  it("keep Nx's selection whenever the version-free check refuses what it reads", { timeout: 600000 }, async () => {
    const ws = workspace();
    try {
      const refused = async (label, edit) => {
        const result = await affectsWith(ws, label, edit);
        assert.equal(result.stdout.trim(), "true", `${label}: ${result.stderr}`);
        assert.match(result.stderr, /version-free check could not answer/, label);
        return result.stderr;
      };
      // A manifest the version-free digest cannot parse is hashed raw, version and all.
      assert.match(
        await refused("an unparsable manifest", (dir) => {
          moveVersion(dir);
          const path = join(dir, "Cargo.toml");
          writeFileSync(path, `${readFileSync(path, "utf8")}\n[package.metadata.journey]\nnote = """two\nlines"""\n`);
        }),
        /hashing raw tracked inputs|fell back|nx-version-free-inputs/,
      );
      assert.match(
        await refused("a path Nx's --files cannot carry", (dir) => {
          moveVersion(dir);
          writeFileSync(join(dir, "a,b.txt"), "a comma\n");
        }),
        /comma/,
      );
      // Nx refuses a malformed project before the check is reached, which fails closed too;
      // asked directly, the check refuses it, naming the field, and drops nothing.
      const base = commit(ws, "a malformed tagged project", (dir) => {
        moveVersion(dir);
        const path = join(dir, "packaging/project.json");
        const project = JSON.parse(readFileSync(path, "utf8"));
        writeFileSync(path, `${JSON.stringify({ ...project, tags: "selection:version-free" }, null, 2)}\n`);
      });
      const direct = spawnSync(process.execPath, ["scripts/nx-version-free-selection.mjs", "--base", base], {
        cwd: ws.dir,
        encoding: "utf8",
      });
      assert.equal(direct.status, 1, direct.stderr);
      assert.equal(direct.stdout, "");
      assert.match(direct.stderr, /packaging\/project\.json: tags is not a list of strings/);
    } finally {
      ws.close();
    }
  });

  it("are kept for a justfile edit when the project names no recipe to compare", { timeout: 600000 }, async () => {
    const ws = workspace();
    try {
      commit(ws, "no recipe input", (dir) => {
        const path = join(dir, "packaging/project.json");
        const project = JSON.parse(readFileSync(path, "utf8"));
        project.namedInputs.packagingHash = project.namedInputs.packagingHash.filter(
          (input) => !String(input.runtime ?? "").includes("nx-recipe-input.mjs"),
        );
        writeFileSync(path, `${JSON.stringify(project, null, 2)}\n`);
      });
      assert.equal(await affects(ws, "another recipe", (dir) => editRecipe(dir, OTHER_RECIPE)), "true");
    } finally {
      ws.close();
    }
  });

  it("leave every other check a source-only change selected before they moved", { timeout: 600000 }, async () => {
    const jobs = ["lint", "test-linux", "test-windows", "test-macos"];
    const before = workspace(BEFORE);
    const after = workspace();
    try {
      const change = (dir) => touch(dir, "src/lib.rs");
      const then = await plan(before, "src", change, { jobs });
      const now = await plan(after, "src", change, { jobs });
      selectsNeither(now, "a change only under src/");
      // `human-items-e2e:test` counts on the debug tree `onepipeline:build` makes, which
      // compiles its test binary, rather than after a build of its own. The install proofs
      // a source-only change no longer selects are held by install-proofs.test.mjs.
      const moved = ["onepipeline:wheel-smoke", "human-items-e2e:build"];
      const narrowed = (job, task) =>
        task === "onepipeline:install-documented" || (job !== "test-linux" && task === "onepipeline:install-smoke");
      for (const job of jobs)
        for (const task of then[job])
          if (!moved.includes(task) && !narrowed(job, task))
            assert.ok(now[job].has(task), `${job} selected ${task} for a src/ change before the packaging proofs moved, and no longer does`);
      assert.ok(then["test-linux"].has("onepipeline:wheel-smoke"), "the base this compares with did not run the wheel on a src/ change");
    } finally {
      before.close();
      after.close();
    }
  });
});

function capture(dir) {
  rmSync(join(dir, ".nx/workspace-data"), { recursive: true, force: true });
  mkdirSync(join(dir, ".nx"), { recursive: true });
  const out = join(dir, ".nx/hash-details.json");
  execFileSync(process.execPath, [helper, "capture", dir, ...PROOFS, "--out", out], {
    cwd: dir,
    env: { ...process.env, ...ENV },
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  return JSON.parse(readFileSync(out, "utf8")).tasks;
}

describe("a packaging proof's cache key", () => {
  it("moves with every input it reads, version and binary sources included; an uncached proof stays uncached", { timeout: 900000 }, () => {
    const ws = workspace();
    try {
      let previous = capture(ws.dir);
      const cached = PROOFS.filter((id) => previous[id].cacheable);
      assert.deepEqual(
        PROOFS.filter((id) => !previous[id].cacheable),
        [`${PROJECT}:wheel-smoke`],
        "a proof's caching changed; a cached one must be held to every input here",
      );
      assert.equal(declared.targets["wheel-smoke"].cache, false, "the wheel proof is replayable");
      assert.ok(cached.length, "no packaging proof is cached, so none is checked here");
      const categories = [
        ...["src/", "crates/", "schemas/"].map((prefix) => [prefix, (dir) => touch(dir, under(dir, prefix))]),
        ...selectionFiles(ws.dir).map((path) => [path, (dir) => touch(dir, path)]),
        ...["README.md", "LICENSE", "CHANGELOG.md"].map((path) => [path, (dir) => touch(dir, path)]),
        ["a packaging recipe", (dir) => editRecipe(dir, PACKAGING_RECIPE)],
        ["a dependency moved in Cargo.lock", moveDependency],
        ["a version-only move in Cargo.toml and Cargo.lock", moveVersion],
      ];
      for (const [what, edit] of categories) {
        edit(ws.dir);
        const next = capture(ws.dir);
        for (const id of cached)
          assert.notEqual(next[id].hash, previous[id].hash, `${id}'s hash stayed when ${what} changed`);
        previous = next;
      }
    } finally {
      ws.close();
    }
  });
});

// The cache rule selected no wheel cache (budgets.yaml, above onepipeline-cache-headroom):
// the wheel is built as the release builds it, from the checkout alone, and nothing
// saves or restores one.
describe("the Linux wheel build", () => {
  it("reads no cache its caller did not ask for, and takes no option asking for one", () => {
    const bin = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "wheel-docker-"));
    const out = `target/${bin.split("/").at(-1)}-out`;
    const target = "target/wheel-x86_64-unknown-linux-gnu";
    const leftover = !existsSync(join(root, target));
    try {
      const log = join(bin, "docker.log");
      writeFileSync(join(bin, "docker"), `#!/usr/bin/env bash\nprintf '%s\\n' "$@" > '${log}'\n`, { mode: 0o755 });
      const build = (args) =>
        spawnSync("bash", ["scripts/build-linux-wheel.sh", ...args], {
          cwd: root,
          encoding: "utf8",
          env: { ...process.env, PATH: `${bin}:${process.env.PATH}` },
        });
      const built = build(["--target", "x86_64-unknown-linux-gnu", "--out", out]);
      assert.equal(built.status, 0, built.stderr);
      const args = readFileSync(log, "utf8").trimEnd().split("\n");
      const values = (flag) => args.flatMap((arg, at) => (args[at - 1] === flag ? [arg] : []));
      const checkout = execFileSync("pwd", ["-P"], { cwd: root, encoding: "utf8" }).trim();
      assert.deepEqual(values("-v"), [
        `${checkout}:/io:ro`,
        `${checkout}/${target}:/io/${target}`,
        `${checkout}/${out}:/io/${out}`,
      ]);
      const names = values("-e").map((assignment) => assignment.split("=")[0]);
      for (const cache of ["CARGO_HOME", "RUSTC_WRAPPER", "SCCACHE_DIR", "CARGO_INCREMENTAL"])
        assert.ok(!names.includes(cache), `the container is handed ${cache}`);
      const asked = build(["--target", "x86_64-unknown-linux-gnu", "--cache", "target/wheel-cache"]);
      assert.equal(asked.status, 2, asked.stderr);
      assert.match(asked.stderr, /unknown argument: --cache/);
    } finally {
      rmSync(bin, { recursive: true, force: true });
      rmSync(join(root, out), { recursive: true, force: true });
      if (leftover) rmSync(join(root, target), { recursive: true, force: true });
    }
  });

  it("is saved and restored by no workflow", () => {
    const dir = join(root, ".github/workflows");
    for (const file of readdirSync(dir).filter((name) => /\.ya?ml$/.test(name))) {
      const workflow = parse(readFileSync(join(dir, file), "utf8"));
      for (const [name, job] of Object.entries(workflow.jobs ?? {}))
        for (const step of job.steps ?? []) {
          if (!/^actions\/cache(\/(save|restore))?@/.test(step.uses ?? "")) continue;
          const cached = `${step.with?.key ?? ""} ${step.with?.path ?? ""}`;
          assert.doesNotMatch(cached, /wheel-build|target\/wheel-/, `${file}'s ${name} caches the wheel build`);
        }
    }
  });
});

// onepipeline-packaging hashes the binary's sources by naming them to this digest.
describe("the workspace digest over named paths", () => {
  const script = join(root, ".github/workspace-proof-input.mjs");
  it("digests the tracked files under them alone, and refuses a path that names none", () => {
    const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "scoped-digest-"));
    try {
      const git = (...args) => execFileSync("git", args, { cwd: dir, encoding: "utf8" });
      git("init", "-q");
      mkdirSync(join(dir, "named"));
      mkdirSync(join(dir, "other"));
      writeFileSync(join(dir, "named", "file"), "named\n");
      writeFileSync(join(dir, "other", "file"), "other\n");
      git("add", "-A");
      const digest = (...paths) => spawnSync(process.execPath, [script, ...paths], { cwd: dir, encoding: "utf8" });
      const first = digest("named");
      assert.equal(first.status, 0, first.stderr);
      assert.match(first.stdout.trim(), /^[0-9a-f]{64}$/);
      writeFileSync(join(dir, "other", "file"), "other, edited\n");
      assert.equal(digest("named").stdout, first.stdout, "a file outside the named path moved the digest");
      writeFileSync(join(dir, "named", "file"), "named, edited\n");
      assert.notEqual(digest("named").stdout, first.stdout, "a file under the named path left the digest");
      for (const bad of ["missing", "--all"]) {
        const [one, two] = [digest("named", bad), digest("named", bad)];
        for (const run of [one, two]) {
          assert.equal(run.status, 1, `${bad} was not refused`);
          assert.match(run.stdout.trim(), /^workspace-proof-input-failed:[0-9a-f-]{36}$/);
          assert.match(run.stderr, new RegExp(`'${bad}' names no tracked file`));
        }
        assert.notEqual(one.stdout, two.stdout, `a refusal of ${bad} printed a replayable value`);
      }
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("the version-free check's own invocation", () => {
  const helper = join(root, "scripts/nx-version-free-selection.mjs");
  for (const [args, says] of [
    [[], /pass exactly --base/],
    [["--base"], /pass exactly --base/],
    [["--head", "abc1234"], /pass exactly --base/],
    [["--base", "main; rm -rf /"], /is not a commit SHA/],
  ]) {
    it(`refuses ${JSON.stringify(args)} with its usage, exit 2`, () => {
      const run = spawnSync(process.execPath, [helper, ...args], { cwd: root, encoding: "utf8" });
      assert.equal(run.status, 2, run.stderr);
      assert.equal(run.stdout, "");
      assert.match(run.stderr, says);
      assert.match(run.stderr, /ACTION: run 'node scripts\/nx-version-free-selection\.mjs --base <commit sha>'/);
    });
  }
});
