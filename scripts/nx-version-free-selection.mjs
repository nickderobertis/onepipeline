// The projects `scripts/nx-affected.sh` drops from Nx's affected answer: those tagged
// `selection:version-free` whose only selecting change is one Nx cannot read finer than
// a file.
//
//   node scripts/nx-version-free-selection.mjs --base SHA
//
// Nx's affected selection is file-level, so `cargoSelection` selects a project for a
// release's version bump and `justfileSelection` for an edit to any recipe. A tagged
// project is dropped when, between SHA and HEAD:
//   - every other changed file leaves it unselected, asked of Nx itself (`--files`);
//   - the Cargo manifests and lock are unchanged once their own versions are blanked,
//     by the digest its tasks' runtime input `nx-version-free-inputs.mjs --ecosystem
//     cargo` computes; and
//   - the justfile recipes its inputs name (`nx-recipe-input.mjs RECIPE...`) digest the
//     same, through what they reach.
// Both digests are taken of each commit's own files in a scratch checkout, by HEAD's
// scripts, so the two sides are read alike.
//
// Stdout is the dropped projects, comma-separated, or nothing. Exit 0 on an answer, 2
// on a usage error, 1 when it cannot answer; the caller then keeps Nx's answer, so a
// failure here can only run more, never less.
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const TAG = "selection:version-free";
const MANIFESTS = new Set(["Cargo.toml", "Cargo.lock"]);
const RECIPE_INPUT = /^node scripts\/nx-recipe-input\.mjs (.+)$/;

function usage(message) {
  console.error(`nx-version-free-selection: ${message}`);
  console.error("ACTION: run 'node scripts/nx-version-free-selection.mjs --base <commit sha>'");
  process.exit(2);
}

const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== "--base") usage("pass exactly --base <commit sha>");
const base = args[1];
if (!/^[0-9a-fA-F]{7,64}$/.test(base)) usage(`'${base}' is not a commit SHA`);

const git = (argv, options = {}) =>
  execFileSync("git", argv, {
    cwd: ROOT,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
    ...options,
  });

const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);

// Every string a project's inputs hold, wherever Nx reads one: its named inputs and
// each target's inputs. A shape Nx would not read as inputs is refused, since a recipe
// list missed here would compare nothing and drop the project.
function inputStrings(project, path) {
  const refuse = (what) => {
    throw new Error(`${path}: ${what}`);
  };
  if (project.namedInputs !== undefined && !isObject(project.namedInputs))
    refuse("namedInputs is not an object");
  if (project.targets !== undefined && !isObject(project.targets))
    refuse("targets is not an object");
  const lists = [
    ...Object.values(project.namedInputs ?? {}),
    ...Object.values(project.targets ?? {}).map((target) =>
      isObject(target) ? (target.inputs ?? []) : refuse("a target is not an object"),
    ),
  ];
  return lists.flatMap((list) => {
    if (!Array.isArray(list)) refuse("an input list is not an array");
    return list.map((input) => {
      if (typeof input === "string") return input;
      if (!isObject(input)) refuse(`an input is ${JSON.stringify(input)}`);
      if (input.runtime !== undefined && typeof input.runtime !== "string")
        refuse("a runtime input is not a string");
      return input.runtime ?? "";
    });
  });
}

function taggedProjects() {
  return git(["ls-files", "-z", "--", "*project.json"])
    .split("\0")
    .filter((path) => basename(path) === "project.json")
    .flatMap((path) => {
      const project = JSON.parse(readFileSync(join(ROOT, path), "utf8"));
      if (!isObject(project)) throw new Error(`${path} is not a project definition`);
      if (
        project.tags !== undefined &&
        !(Array.isArray(project.tags) && project.tags.every((tag) => typeof tag === "string"))
      )
        throw new Error(`${path}: tags is not a list of strings`);
      if (!project.tags?.includes(TAG)) return [];
      if (typeof project.name !== "string" || !/^[A-Za-z0-9@][A-Za-z0-9@._/-]*$/.test(project.name))
        throw new Error(`${path}: a ${TAG} project needs a name Nx can --exclude`);
      const recipes = inputStrings(project, path)
        .map((input) => input.match(RECIPE_INPUT)?.[1])
        .filter(Boolean);
      return [{ name: project.name, recipes }];
    });
}

// One commit's tree in a scratch git repository, so a script that reads tracked files
// reads that commit's.
const checkouts = new Map();
function checkout(revision) {
  if (checkouts.has(revision)) return checkouts.get(revision);
  const dir = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "version-free-selection-"),
  );
  checkouts.set(revision, dir);
  const archive = git(["archive", revision], { encoding: "buffer" });
  execFileSync("tar", ["-xf", "-", "-C", dir], { input: archive, stdio: ["pipe", "pipe", "pipe"] });
  const local = (argv) =>
    execFileSync("git", argv, { cwd: dir, stdio: ["ignore", "pipe", "pipe"] });
  local(["init", "-q"]);
  local(["add", "-A"]);
  return dir;
}

// A runtime input's digest of one commit. Either script prints a fallback rather than
// failing — raw bytes, or a value unique to the attempt — and says so on stderr; both
// are refusals here, since raw bytes carry the version this comparison ignores.
function digestAt(revision, script, argv) {
  const run = spawnSync(process.execPath, [join(ROOT, script), ...argv], {
    cwd: checkout(revision),
    encoding: "utf8",
  });
  const digest = (run.stdout ?? "").trim();
  if (run.status !== 0 || run.stderr.trim() || !/^[0-9a-f]{64}$/.test(digest))
    throw new Error(`${script} ${argv.join(" ")} at ${revision}: ${run.stderr.trim() || digest}`);
  return digest;
}

const moved = (script, argv) => digestAt(base, script, argv) !== digestAt("HEAD", script, argv);

function selectedByNx(files) {
  if (files.some((file) => file.includes(",")))
    throw new Error("a changed path holds a comma, which Nx's --files cannot carry");
  const output = execFileSync(
    "bash",
    ["scripts/nx.sh", "show", "projects", "--affected", `--files=${files.join(",")}`, "--json"],
    {
      cwd: ROOT,
      encoding: "utf8",
      env: { ...process.env, ONEPIPELINE_NX_SHOW_OUTPUT: "1" },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  const projects = JSON.parse(output.slice(output.indexOf("[")));
  if (!Array.isArray(projects) || !projects.every((name) => typeof name === "string"))
    throw new Error(`Nx's affected projects are not a list of names: ${output.trim()}`);
  return new Set(projects);
}

try {
  git(["rev-parse", "--quiet", "--verify", `${base}^{commit}`]);
  const changed = git(["diff", "--name-only", "--no-renames", "-z", base, "HEAD"])
    .split("\0")
    .filter(Boolean);
  const manifests = changed.filter((path) => MANIFESTS.has(basename(path)));
  const justfile = changed.includes("justfile");
  const others = changed.filter((path) => !MANIFESTS.has(basename(path)) && path !== "justfile");
  const dropped = [];
  if (manifests.length || justfile) {
    const projects = taggedProjects();
    const nx = others.length && projects.length ? selectedByNx(others) : new Set();
    const cargo =
      manifests.length && projects.length
        ? moved("scripts/nx-version-free-inputs.mjs", ["--ecosystem", "cargo"])
        : false;
    for (const project of projects) {
      if (nx.has(project.name) || cargo) continue;
      if (justfile) {
        if (!project.recipes.length) continue;
        if (
          project.recipes.some((recipes) =>
            moved("scripts/nx-recipe-input.mjs", recipes.split(" ")),
          )
        )
          continue;
      }
      dropped.push(project.name);
    }
  }
  process.stdout.write(dropped.join(","));
} catch (error) {
  console.error(`nx-version-free-selection: cannot answer against ${base}: ${error.message}`);
  console.error(
    "ACTION: check the base commit is fetched and `git archive`, `just` and Nx run here; until then Nx's own selection stands",
  );
  process.exitCode = 1;
} finally {
  for (const dir of checkouts.values()) rmSync(dir, { recursive: true, force: true });
}
