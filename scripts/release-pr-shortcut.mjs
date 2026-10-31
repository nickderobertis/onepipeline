// Whether a pull request's tests are already proven, so ci.yml's three test jobs
// may succeed without running them: its head's tree is the tree of a `main`
// commit that passed, plus only the edits release-plz makes when it cuts a
// release — the package version in `Cargo.toml` and `Cargo.lock`, and new lines
// at the head of `CHANGELOG.md`. `lint` never asks: it runs in full on every
// pull request, the release pull request included.
//
// Decided from the trees alone. Nothing here reads the pull request's title,
// author or branch name, so a pull request that only looks like a release one
// runs its tests, and so does a release pull request carrying anything else.
//
// Usage:
//   node scripts/release-pr-shortcut.mjs [--base-ref main] [--head HEAD] [--repo owner/name]
//
// Prints `skip=true` or `skip=false` on stdout — a line `$GITHUB_OUTPUT` takes
// as it is — and why on stderr. Every doubt answers `skip=false`: a base that
// cannot be found, a diff it cannot read, a `main` verdict GitHub does not give.
// Exit 2 only for a refused call; the answer is otherwise always exit 0.
//
// "Passed" is asked of GitHub (`gh api`, read-only): a successful `CI` run on
// the base commit itself, or the pull request that merged it, whose head had
// the same tree and whose `test-linux`, `test-windows` and `test-macos` checks
// all succeeded.
import { execFileSync } from "node:child_process";

const TEST_JOBS = ["test-linux", "test-windows", "test-macos"];
const CI_WORKFLOW = ".github/workflows/ci.yml";
// The files release-plz edits, and the only edit each may carry.
const VERSIONED = new Set(["Cargo.toml", "Cargo.lock"]);
const CHANGELOG = "CHANGELOG.md";

function refuse(message) {
  process.stderr.write(`release-pr-shortcut: ${message}\n`);
  process.stderr.write(
    "ACTION: run 'node scripts/release-pr-shortcut.mjs [--base-ref <branch>] [--head <rev>] [--repo <owner/name>]'\n",
  );
  process.exit(2);
}

function answer(skip, why) {
  process.stderr.write(`release-pr-shortcut: ${why}\n`);
  process.stdout.write(`skip=${skip}\n`);
  process.exit(0);
}

const options = { baseRef: "main", head: "HEAD", repo: process.env.GITHUB_REPOSITORY ?? "" };
const argv = process.argv.slice(2);
while (argv.length > 0) {
  const flag = argv.shift();
  const value = argv.shift();
  if (value === undefined || value.startsWith("--")) refuse(`${flag} needs a value`);
  if (flag === "--base-ref") options.baseRef = value;
  else if (flag === "--head") options.head = value;
  else if (flag === "--repo") options.repo = value;
  else refuse(`unknown argument '${flag}'`);
}
// Each reaches git or the GitHub API as an argument, so its shape is checked at
// this boundary rather than trusted.
if (!/^[A-Za-z0-9][A-Za-z0-9._/-]*$/.test(options.baseRef)) {
  refuse(`'${options.baseRef}' is not a branch name`);
}
if (!/^[A-Za-z0-9][A-Za-z0-9._/~^-]*$/.test(options.head)) {
  refuse(`'${options.head}' is not a revision`);
}
if (options.repo !== "" && !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(options.repo)) {
  refuse(`'${options.repo}' is not an owner/name`);
}

function git(...args) {
  return execFileSync("git", args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
}

function tryGit(...args) {
  try {
    return git(...args);
  } catch {
    return null;
  }
}

// The base is the commit the head forked from on the base branch: the tree the
// head's edits are judged against, and the commit whose verdict is asked.
if (process.env.CI) {
  tryGit(
    "fetch",
    "--no-tags",
    "--quiet",
    "origin",
    `+refs/heads/${options.baseRef}:refs/remotes/origin/${options.baseRef}`,
  );
}
const head = tryGit("rev-parse", "--verify", "--quiet", `${options.head}^{commit}`);
if (!head) answer(false, `'${options.head}' does not resolve to a commit, so the tests run`);
const base = tryGit("merge-base", `origin/${options.baseRef}`, head);
if (!base) {
  answer(false, `no merge base with origin/${options.baseRef}, so the tests run`);
}

// Only the edits release-plz makes: in-place modifications of its three files.
const changes = tryGit("diff", "--no-renames", "--name-status", base, head);
if (changes === null) answer(false, `could not diff ${base}..${head}, so the tests run`);
for (const line of changes.split("\n").filter(Boolean)) {
  const [status, path] = line.split("\t");
  if (status !== "M" || !(VERSIONED.has(path) || path === CHANGELOG)) {
    answer(false, `${path} (${status}) is not one of release-plz's version or changelog edits`);
  }
}

function hunkLines(path) {
  const diff = git("diff", "--unified=0", "--no-color", base, head, "--", path);
  const removed = [];
  const added = [];
  let oldAt = 0;
  for (const line of diff.split("\n")) {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(line);
    if (hunk) {
      oldAt = Number(hunk[1]);
      continue;
    }
    if (line.startsWith("---") || line.startsWith("+++")) continue;
    if (line.startsWith("-")) removed.push({ text: line.slice(1), at: oldAt++ });
    else if (line.startsWith("+")) added.push(line.slice(1));
  }
  return { removed, added };
}

const VERSION = /^version = "[^"\s]+"$/;
// The section a line of the base's manifest sits in, so a dependency's own
// `version = ...` key (a `[dependencies.<name>]` table) is not taken for the
// package's.
function sectionOf(path, lineNumber) {
  const lines = git("show", `${base}:${path}`)
    .split("\n")
    .slice(0, lineNumber - 1);
  for (let index = lines.length - 1; index >= 0; index--) {
    const header = /^\s*\[\[?([^\]]+)\]\]?\s*$/.exec(lines[index]);
    if (header) return header[1].trim();
  }
  return "";
}

for (const path of VERSIONED) {
  if (!changes.split("\n").some((line) => line.endsWith(`\t${path}`))) continue;
  const { removed, added } = hunkLines(path);
  const replaced =
    removed.length === added.length &&
    removed.every((line) => VERSION.test(line.text)) &&
    added.every((line) => VERSION.test(line));
  if (!replaced) answer(false, `${path} changes more than a version line`);
  for (const line of removed) {
    const section = sectionOf(path, line.at);
    const allowed =
      path === "Cargo.lock"
        ? section === "package"
        : section === "package" || section === "workspace.package";
    if (!allowed) {
      answer(
        false,
        `${path} line ${line.at} is a version under [${section}], not the package's own`,
      );
    }
  }
}
if (changes.split("\n").some((line) => line.endsWith(`\t${CHANGELOG}`))) {
  if (hunkLines(CHANGELOG).removed.length > 0) {
    answer(false, `${CHANGELOG} rewrites existing lines rather than adding a release's`);
  }
}

// The base's own verdict, read-only. A GitHub that does not answer is a base
// nobody can say passed.
if (options.repo === "")
  answer(false, "no repository to ask whether main passed, so the tests run");
function gh(path) {
  try {
    return JSON.parse(
      execFileSync("gh", ["api", "--method", "GET", path], {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
      }),
    );
  } catch (error) {
    answer(
      false,
      `GitHub did not answer ${path} (${error.message.split("\n")[0]}), so the tests run`,
    );
  }
}

const runs = gh(`repos/${options.repo}/actions/runs?head_sha=${base}&status=success&per_page=100`);
if (
  (runs.workflow_runs ?? []).some(
    (run) => run.path === CI_WORKFLOW && run.conclusion === "success" && run.head_sha === base,
  )
) {
  answer(
    true,
    `${head} is ${base} (CI passed on it) plus only release-plz's version and changelog edits`,
  );
}
const baseTree = git("rev-parse", `${base}^{tree}`);
for (const pull of gh(`repos/${options.repo}/commits/${base}/pulls`)) {
  if (pull.merge_commit_sha !== base || !pull.head?.sha) continue;
  const tested = pull.head.sha;
  const commit = gh(`repos/${options.repo}/git/commits/${tested}`);
  if (commit.tree?.sha !== baseTree) continue;
  const checks =
    gh(`repos/${options.repo}/commits/${tested}/check-runs?per_page=100`).check_runs ?? [];
  const green = TEST_JOBS.every((job) =>
    checks.some((check) => check.name === job && check.conclusion === "success"),
  );
  if (green) {
    answer(
      true,
      `${head} is the tree #${pull.number} passed ${TEST_JOBS.join(", ")} on, plus only release-plz's version and changelog edits`,
    );
  }
}
answer(false, `no record that main passed ${base}'s tree, so the tests run`);
