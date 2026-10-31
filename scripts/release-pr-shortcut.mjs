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
const VERSIONED = new Set(["Cargo.toml", "Cargo.lock"]);
const CHANGELOG = "CHANGELOG.md";

function refuse(message) {
  process.stderr.write(`release-pr-shortcut: ${message}\n`);
  process.stderr.write(
    "ACTION: run 'node scripts/release-pr-shortcut.mjs [--base-ref <branch>] [--head <rev>] [--repo <owner/name>]'\n",
  );
  process.exit(2);
}

// llmlint: ignore-block[tool_output_is_signal] the answer is two streams, one
// line each: stdout is the machine answer `$GITHUB_OUTPUT` takes as it is, and
// stderr the one line a reader of the job's log needs to know why its tests ran
// or were skipped, which no other output of the job says.
function answer(skip, why) {
  process.stderr.write(`release-pr-shortcut: ${why}\n`);
  process.stdout.write(`skip=${skip}\n`);
  process.exit(0);
}
// llmlint: ignore-end[tool_output_is_signal]

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

function tryGit(...args) {
  try {
    return execFileSync("git", args, {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    }).trim();
  } catch {
    return null;
  }
}

// A read the answer depends on: a git that cannot give it leaves a diff nobody
// read, which is never one to skip the tests for.
function git(...args) {
  const out = tryGit(...args);
  if (out === null) answer(false, `git ${args.join(" ")} failed, so the tests run`);
  return out;
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
// In place, as text: a mode change leaves no hunk for the checks below to read,
// and a binary diff no lines, so either would pass them unread.
for (const line of git("diff", "--no-renames", "--raw", base, head).split("\n").filter(Boolean)) {
  const [modes, path] = line.split("\t");
  const [oldMode, newMode] = modes.slice(1).split(" ");
  if (oldMode !== "100644" || newMode !== "100644") {
    answer(false, `${path} changes mode (${oldMode} -> ${newMode}), which no release edit does`);
  }
}
for (const line of git("diff", "--no-renames", "--numstat", base, head)
  .split("\n")
  .filter(Boolean)) {
  const [added, removed, path] = line.split("\t");
  if (added === "-" || removed === "-")
    answer(false, `${path} changes as binary, which no release edit does`);
}

// Each hunk of one file's diff: where it sits in the base, and the lines it
// removes (with their base line numbers) and adds.
function hunksOf(path) {
  const diff = git("diff", "--unified=0", "--no-color", base, head, "--", path);
  const hunks = [];
  for (const line of diff.split("\n")) {
    const header = /^@@ -(\d+)(?:,(\d+))? \+\d+(?:,\d+)? @@/.exec(line);
    if (header) {
      hunks.push({
        oldStart: Number(header[1]),
        oldCount: Number(header[2] ?? 1),
        removed: [],
        added: [],
      });
      continue;
    }
    const hunk = hunks.at(-1);
    if (!hunk || line.startsWith("---") || line.startsWith("+++")) continue;
    if (line.startsWith("-")) {
      hunk.removed.push({ text: line.slice(1), at: hunk.oldStart + hunk.removed.length });
    } else if (line.startsWith("+")) hunk.added.push(line.slice(1));
  }
  return hunks;
}

const baseLines = (path) => git("show", `${base}:${path}`).split("\n");
// A Cargo version: SemVer 2.0's three numbers, then an optional pre-release of
// dot-separated identifiers (numeric ones without a leading zero) and optional
// build metadata.
const IDENTIFIER = "(?:0|[1-9]\\d*|\\d*[A-Za-z-][0-9A-Za-z-]*)";
const VERSION = new RegExp(
  `^version = "(0|[1-9]\\d*)\\.(0|[1-9]\\d*)\\.(0|[1-9]\\d*)` +
    `(?:-${IDENTIFIER}(?:\\.${IDENTIFIER})*)?(?:\\+[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*)?"$`,
);

// The table a line of the base's file sits in — its header and every line of
// it — so a dependency's own `version = ...` key (a `[dependencies.<name>]`
// table, a registry package's lock entry) is not taken for the package's.
function tableOf(path, lineNumber) {
  const lines = baseLines(path);
  const isHeader = (text) => /^\s*\[\[?[^\]]+\]\]?\s*$/.test(text);
  let start = lineNumber - 1;
  while (start >= 0 && !isHeader(lines[start])) start--;
  let end = lineNumber;
  while (end < lines.length && !isHeader(lines[end])) end++;
  const header = start >= 0 ? /^\s*\[\[?([^\]]+)\]\]?/.exec(lines[start])[1].trim() : "";
  return { header, body: lines.slice(start + 1, end) };
}

// The packages this tree's own manifests declare, by name: a lock entry
// release-plz bumps is one of these, never a registry, git or outside-path
// dependency.
const workspacePackages = new Set(
  git("ls-tree", "-r", "--name-only", base)
    .split("\n")
    .filter((path) => /(^|\/)Cargo\.toml$/.test(path))
    .map((path) => {
      const lines = baseLines(path);
      const start = lines.findIndex((line) => line.trim() === "[package]");
      if (start < 0) return null;
      for (const line of lines.slice(start + 1)) {
        if (/^\s*\[/.test(line)) break;
        const name = /^name = "([^"]+)"$/.exec(line.trim());
        if (name) return name[1];
      }
      return null;
    })
    .filter(Boolean),
);
function isWorkspacePackage({ header, body }) {
  const name = body.map((line) => /^name = "([^"]+)"$/.exec(line)?.[1]).find(Boolean);
  return (
    header === "package" &&
    !body.some((line) => /^source = /.test(line)) &&
    workspacePackages.has(name)
  );
}

for (const path of VERSIONED) {
  if (!changes.split("\n").some((line) => line.endsWith(`\t${path}`))) continue;
  for (const hunk of hunksOf(path)) {
    const replaced =
      hunk.removed.length === hunk.added.length &&
      hunk.removed.every((line) => VERSION.test(line.text)) &&
      hunk.added.every((line) => VERSION.test(line));
    if (!replaced) answer(false, `${path} changes more than a version line`);
    for (const line of hunk.removed) {
      const table = tableOf(path, line.at);
      const allowed =
        path === "Cargo.lock"
          ? isWorkspacePackage(table)
          : table.header === "package" || table.header === "workspace.package";
      if (!allowed) {
        answer(
          false,
          `${path} line ${line.at} is a version under [${table.header}], not this workspace's own package`,
        );
      }
    }
  }
}
// A release adds one section at the head of the changelog, ahead of every
// release already there, and touches nothing else in it.
if (changes.split("\n").some((line) => line.endsWith(`\t${CHANGELOG}`))) {
  const hunks = hunksOf(CHANGELOG);
  if (hunks.some((hunk) => hunk.removed.length > 0)) {
    answer(false, `${CHANGELOG} rewrites existing lines rather than adding a release's`);
  }
  const firstRelease = baseLines(CHANGELOG).findIndex((line) => line.startsWith("## ")) + 1;
  // A pure insertion's header names the base line it follows.
  if (hunks.length !== 1 || (firstRelease > 0 && hunks[0].oldStart >= firstRelease)) {
    answer(false, `${CHANGELOG} adds lines other than one new section above its releases`);
  }
}

// The base's own verdict, read-only. A GitHub that does not answer, or answers
// in a shape this does not read, is a base nobody can say passed.
if (options.repo === "") {
  answer(false, "no repository to ask whether main passed, so the tests run");
}
function gh(path, shaped) {
  let body;
  try {
    body = JSON.parse(
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
  if (!shaped(body))
    answer(false, `GitHub answered ${path} in a shape this does not read, so the tests run`);
  return body;
}
const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const listOf = (value, key) =>
  isObject(value) && Array.isArray(value[key]) && value[key].every(isObject);

const runs = gh(
  `repos/${options.repo}/actions/runs?head_sha=${base}&status=success&per_page=100`,
  (body) => listOf(body, "workflow_runs"),
);
if (
  runs.workflow_runs.some(
    (run) => run.path === CI_WORKFLOW && run.conclusion === "success" && run.head_sha === base,
  )
) {
  answer(
    true,
    `${head} is ${base} (CI passed on it) plus only release-plz's version and changelog edits`,
  );
}
const baseTree = git("rev-parse", `${base}^{tree}`);
const pulls = gh(
  `repos/${options.repo}/commits/${base}/pulls`,
  (body) => Array.isArray(body) && body.every(isObject),
);
for (const pull of pulls) {
  const tested = pull.head?.sha;
  if (
    pull.merge_commit_sha !== base ||
    typeof tested !== "string" ||
    !/^[0-9a-f]{40}$/.test(tested)
  ) {
    continue;
  }
  const commit = gh(`repos/${options.repo}/git/commits/${tested}`, (body) => isObject(body?.tree));
  if (commit.tree.sha !== baseTree) continue;
  const checks = gh(`repos/${options.repo}/commits/${tested}/check-runs?per_page=100`, (body) =>
    listOf(body, "check_runs"),
  ).check_runs;
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
