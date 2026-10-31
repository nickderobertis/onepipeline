// A scratch copy of this workspace as a git repository whose `origin/main` is
// the copy itself, so a journey can commit a change and drive a real recipe
// against it with the real Nx — nothing in the tree under test is stood in for.
import { execFileSync, spawnSync } from "node:child_process";
import {
  appendFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  symlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

// A fixed author, so a commit needs nothing of the host's git configuration.
const GIT_ENV = {
  GIT_AUTHOR_NAME: "ci-jobs journey",
  GIT_AUTHOR_EMAIL: "ci-jobs@example.invalid",
  GIT_COMMITTER_NAME: "ci-jobs journey",
  GIT_COMMITTER_EMAIL: "ci-jobs@example.invalid",
};

export function git(cwd, ...args) {
  return execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    env: { ...process.env, ...GIT_ENV },
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

/**
 * Copy the working tree's files (tracked, and untracked but not ignored) into a
 * fresh repository, committed once as `main` and pointed at by `origin/main`.
 * The installed Node packages are linked rather than copied: they are what the
 * recipes run, and nothing here writes to them.
 */
export function scratchWorkspace(t) {
  const dir = mkdtempSync(join(tmpdir(), "ci-jobs-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const files = execFileSync(
    "git",
    ["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
    { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  )
    .split("\0")
    .filter((path) => path && existsSync(join(root, path)));
  for (const path of files) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    cpSync(join(root, path), join(dir, path), { verbatimSymlinks: true });
  }
  for (const modules of ["node_modules", ".github/node_modules"]) {
    if (existsSync(join(root, modules))) symlinkSync(join(root, modules), join(dir, modules));
  }
  git(dir, "init", "--quiet", "--initial-branch=main");
  // A link is not the directory `.gitignore`'s `node_modules/` names, so it is
  // excluded by name: an untracked file is a change Nx counts as affecting the
  // project that owns it, which for these is the crate's.
  appendFileSync(join(dir, ".git", "info", "exclude"), "/node_modules\n/.github/node_modules\n");
  git(dir, "add", "--all");
  git(dir, "commit", "--quiet", "--no-verify", "--message", "base");
  git(dir, "update-ref", "refs/remotes/origin/main", "HEAD");
  return dir;
}

/** Commit one file's new content on top of the copy's HEAD. */
export function commit(dir, path, content, message = `change ${path}`) {
  mkdirSync(dirname(join(dir, path)), { recursive: true });
  execFileSync("bash", ["-c", 'printf "%s" "$1" > "$2"', "_", content, join(dir, path)]);
  git(dir, "add", "--", path);
  git(dir, "commit", "--quiet", "--no-verify", "--message", message);
}

/** Strip the terminal colours Nx writes even to a pipe. */
export function plain(text) {
  // biome-ignore lint/suspicious/noControlCharactersInRegex: the escape is what is stripped.
  return text.replace(/\u001b\[[0-9;]*[A-Za-z]/g, "");
}

/**
 * Run a command in the copy with the environment a pull-request job scopes by:
 * a base branch rather than CI's own variables, which would make the scripts
 * fetch from a remote the copy does not have.
 */
export function run(dir, command, args, env = {}) {
  const inherited = { ...process.env, ...GIT_ENV, ...env };
  // `NODE_TEST_CONTEXT` is this runner's: a `node --test` a recipe starts would
  // read it as a nested call and skip every file it was given.
  for (const name of [
    "CI",
    "GITHUB_BASE_REF",
    "ONEPIPELINE_NX_BASE_SHA",
    "NX_TASKS_RUNNER",
    "NODE_TEST_CONTEXT",
  ]) {
    if (!(name in env)) delete inherited[name];
  }
  const result = spawnSync(command, args, {
    cwd: dir,
    encoding: "utf8",
    env: inherited,
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  return { status: result.status, log: plain(`${result.stdout}\n${result.stderr}`) };
}
