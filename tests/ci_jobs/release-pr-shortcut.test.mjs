// The release pull request's shortcut (`just release-pr-shortcut`), driven over a
// scratch repository: a bare `origin` whose `main` holds a small crate, and a
// head commit on top of it carrying release-plz's edits, or more than them.
//
// GitHub is the one thing stood in for: a `gh` on PATH that answers the three
// read-only API paths the script asks with the verdicts each case gives `main`,
// and records every call so a case can say what was asked. Git, the diff and
// the script are the real ones.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";
import { git, root } from "./support.mjs";

const REPO = "owner/crate";

const MANIFEST = (version, serde = "1") =>
  `[package]\nname = "crate"\nversion = "${version}"\nedition = "2021"\n\n[dependencies.serde]\nversion = "${serde}"\n`;
const LOCK = (version) =>
  `version = 4\n\n[[package]]\nname = "crate"\nversion = "${version}"\n\n[[package]]\nname = "serde"\nversion = "1.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n`;
const CHANGELOG = "# Changelog\n\n## [0.1.0]\n\n- the first release\n";

// The stand-in `gh`: answers from $FAKE_GH_DIR/<sanitized path>.json, or fails
// as an unreachable API does, and appends each path it was asked to calls.log.
const FAKE_GH = `#!/usr/bin/env bash
set -euo pipefail
[ "$1" = api ] && [ "$2" = --method ] && [ "$3" = GET ] || { echo "unexpected gh call: $*" >&2; exit 3; }
echo "$4" >> "$FAKE_GH_DIR/calls.log"
key="$(printf '%s' "$4" | tr -c 'A-Za-z0-9' '_')"
[ -f "$FAKE_GH_DIR/$key.json" ] || { echo "gh: HTTP 404: $4" >&2; exit 1; }
cat "$FAKE_GH_DIR/$key.json"
`;

function scratch(t, files = {}) {
  const dir = mkdtempSync(join(tmpdir(), "release-pr-shortcut-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const origin = join(dir, "origin.git");
  const work = join(dir, "work");
  const fake = join(dir, "gh");
  mkdirSync(join(fake, "bin"), { recursive: true });
  writeFileSync(join(fake, "bin", "gh"), FAKE_GH);
  chmodSync(join(fake, "bin", "gh"), 0o755);
  writeFileSync(join(fake, "calls.log"), "");
  git(dir, "init", "--quiet", "--bare", "--initial-branch=main", origin);
  git(dir, "init", "--quiet", "--initial-branch=main", work);
  write(work, {
    "Cargo.toml": MANIFEST("0.1.0"),
    "Cargo.lock": LOCK("0.1.0"),
    "CHANGELOG.md": CHANGELOG,
    "src/lib.rs": "pub fn answer() -> u8 { 42 }\n",
    ...files,
  });
  git(work, "add", "--all");
  git(work, "commit", "--quiet", "--message", "feat: the crate");
  git(work, "remote", "add", "origin", origin);
  git(work, "push", "--quiet", "origin", "main");
  git(work, "checkout", "--quiet", "-b", "release-plz-2026-10-31");
  return { work, fake, base: git(work, "rev-parse", "HEAD") };
}

function write(dir, files) {
  for (const [path, content] of Object.entries(files)) {
    mkdirSync(join(dir, path, ".."), { recursive: true });
    writeFileSync(join(dir, path), content);
  }
}

function commitAll(work, files, message) {
  write(work, files);
  git(work, "add", "--all");
  git(work, "commit", "--quiet", "--message", message);
}

// release-plz's edits: the package version in both files, a changelog section.
const RELEASE = {
  "Cargo.toml": MANIFEST("0.2.0"),
  "Cargo.lock": LOCK("0.2.0"),
  "CHANGELOG.md": `# Changelog\n\n## [0.2.0]\n\n- a second release\n\n## [0.1.0]\n\n- the first release\n`,
};

function answers(fake, responses) {
  for (const [path, body] of Object.entries(responses)) {
    writeFileSync(join(fake, `${path.replace(/[^A-Za-z0-9]/g, "_")}.json`), JSON.stringify(body));
  }
}

// main's push run of CI succeeded on the base commit.
const pushRunPassed = (base) => ({
  [`repos/${REPO}/actions/runs?head_sha=${base}&status=success&per_page=100`]: {
    workflow_runs: [
      { path: ".github/workflows/ci.yml", conclusion: "success", head_sha: base, event: "push" },
    ],
  },
});
const noRuns = (base) => ({
  [`repos/${REPO}/actions/runs?head_sha=${base}&status=success&per_page=100`]: {
    workflow_runs: [],
  },
});

function shortcut({ work, fake }, args = ["--repo", REPO, "--base-ref", "main"], env = {}) {
  const result = spawnSync("node", [join(root, "scripts/release-pr-shortcut.mjs"), ...args], {
    cwd: work,
    encoding: "utf8",
    env: {
      ...process.env,
      CI: "",
      PATH: `${join(fake, "bin")}:${process.env.PATH}`,
      FAKE_GH_DIR: fake,
      ...env,
    },
  });
  assert.equal(result.error, undefined);
  return {
    status: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
    calls: readFileSync(join(fake, "calls.log"), "utf8").split("\n").filter(Boolean),
  };
}

describe("the release pull request's shortcut", () => {
  it("skips the tests for a tree main passed plus only release-plz's edits", (t) => {
    const repo = scratch(t);
    commitAll(repo.work, RELEASE, "chore: release v0.2.0");
    answers(repo.fake, pushRunPassed(repo.base));
    const result = shortcut(repo);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "skip=true\n", result.stderr);
    assert.match(result.stderr, /CI passed on it/);
  });

  it("skips them when the merged pull request's head had the tree and passed the test jobs", (t) => {
    const repo = scratch(t);
    commitAll(repo.work, RELEASE, "chore: release v0.2.0");
    const tree = git(repo.work, "rev-parse", `${repo.base}^{tree}`);
    answers(repo.fake, {
      ...noRuns(repo.base),
      [`repos/${REPO}/commits/${repo.base}/pulls`]: [
        { number: 7, merge_commit_sha: repo.base, head: { sha: "f".repeat(40) } },
      ],
      [`repos/${REPO}/git/commits/${"f".repeat(40)}`]: { tree: { sha: tree } },
      [`repos/${REPO}/commits/${"f".repeat(40)}/check-runs?per_page=100`]: {
        check_runs: ["lint", "test-linux", "test-windows", "test-macos"].map((name) => ({
          name,
          conclusion: "success",
        })),
      },
    });
    const result = shortcut(repo);
    assert.equal(result.stdout, "skip=true\n", result.stderr);
    assert.match(result.stderr, /#7 passed/);
  });

  it("runs them when the head adds any other change", (t) => {
    const repo = scratch(t);
    commitAll(
      repo.work,
      { ...RELEASE, "src/lib.rs": "pub fn answer() -> u8 { 41 }\n" },
      "chore: release v0.2.0",
    );
    answers(repo.fake, pushRunPassed(repo.base));
    const result = shortcut(repo);
    assert.equal(result.stdout, "skip=false\n", result.stderr);
    assert.match(result.stderr, /src\/lib\.rs \(M\) is not one of release-plz's/);
    assert.deepEqual(result.calls, [], "it asked GitHub about a diff it had already refused");
  });

  it("runs them when a release file carries more than a version line", (t) => {
    const repo = scratch(t);
    commitAll(
      repo.work,
      { ...RELEASE, "Cargo.toml": MANIFEST("0.2.0", "2") },
      "chore: release v0.2.0",
    );
    answers(repo.fake, pushRunPassed(repo.base));
    const result = shortcut(repo);
    assert.equal(result.stdout, "skip=false\n", result.stderr);
    assert.match(result.stderr, /Cargo\.toml line \d+ is a version under \[dependencies\.serde\]/);
  });

  it("runs them when the changelog rewrites a released entry", (t) => {
    const repo = scratch(t);
    commitAll(
      repo.work,
      { ...RELEASE, "CHANGELOG.md": "# Changelog\n\n## [0.1.0]\n\n- a rewritten release\n" },
      "chore: release v0.2.0",
    );
    answers(repo.fake, pushRunPassed(repo.base));
    const result = shortcut(repo);
    assert.equal(result.stdout, "skip=false\n", result.stderr);
    assert.match(result.stderr, /CHANGELOG\.md rewrites existing lines/);
  });

  it("runs them when main never passed the base tree", (t) => {
    const repo = scratch(t);
    commitAll(repo.work, RELEASE, "chore: release v0.2.0");
    answers(repo.fake, {
      ...noRuns(repo.base),
      [`repos/${REPO}/commits/${repo.base}/pulls`]: [],
    });
    const result = shortcut(repo);
    assert.equal(result.stdout, "skip=false\n", result.stderr);
    assert.match(result.stderr, /no record that main passed/);
  });

  it("runs them when a merged head with the tree did not pass every test job", (t) => {
    const repo = scratch(t);
    commitAll(repo.work, RELEASE, "chore: release v0.2.0");
    const tree = git(repo.work, "rev-parse", `${repo.base}^{tree}`);
    answers(repo.fake, {
      ...noRuns(repo.base),
      [`repos/${REPO}/commits/${repo.base}/pulls`]: [
        { number: 8, merge_commit_sha: repo.base, head: { sha: "e".repeat(40) } },
      ],
      [`repos/${REPO}/git/commits/${"e".repeat(40)}`]: { tree: { sha: tree } },
      [`repos/${REPO}/commits/${"e".repeat(40)}/check-runs?per_page=100`]: {
        check_runs: [
          { name: "test-linux", conclusion: "success" },
          { name: "test-windows", conclusion: "failure" },
          { name: "test-macos", conclusion: "success" },
        ],
      },
    });
    const result = shortcut(repo);
    assert.equal(result.stdout, "skip=false\n", result.stderr);
  });

  it("runs them when GitHub does not answer", (t) => {
    const repo = scratch(t);
    commitAll(repo.work, RELEASE, "chore: release v0.2.0");
    const result = shortcut(repo);
    assert.equal(result.status, 0);
    assert.equal(result.stdout, "skip=false\n", result.stderr);
    assert.match(result.stderr, /GitHub did not answer/);
  });

  it("skips them for a release that versions the workspace's shared package version", (t) => {
    const workspace = (version) =>
      `[workspace]\nmembers = ["."]\n\n[workspace.package]\nversion = "${version}"\n\n[package]\nname = "crate"\nversion.workspace = true\nedition = "2021"\n`;
    const repo = scratch(t, { "Cargo.toml": workspace("0.1.0") });
    commitAll(repo.work, { ...RELEASE, "Cargo.toml": workspace("0.2.0") }, "chore: release v0.2.0");
    answers(repo.fake, pushRunPassed(repo.base));
    const result = shortcut(repo);
    assert.equal(result.stdout, "skip=true\n", result.stderr);
  });

  it("refreshes the base branch from origin in CI, and answers from what it has when it cannot", (t) => {
    const repo = scratch(t);
    commitAll(repo.work, RELEASE, "chore: release v0.2.0");
    answers(repo.fake, pushRunPassed(repo.base));
    git(repo.work, "update-ref", "-d", "refs/remotes/origin/main");
    const args = ["--repo", REPO, "--base-ref", "main"];
    const local = shortcut(repo, args);
    assert.equal(local.stdout, "skip=false\n", local.stderr);
    assert.match(local.stderr, /no merge base with origin\/main/);
    const fetched = shortcut(repo, args, { CI: "true" });
    assert.equal(fetched.stdout, "skip=true\n", fetched.stderr);
    git(repo.work, "remote", "set-url", "origin", join(repo.work, "..", "no-such-origin.git"));
    const unreachable = shortcut(repo, args, { CI: "true" });
    assert.equal(unreachable.stdout, "skip=true\n", unreachable.stderr);
  });

  // Each a release pull request carrying one edit release-plz never makes, with
  // main's verdict a pass, so only the diff can be why the tests run.
  const NOT_RELEASE_PLZ = {
    "a file added beside the release": [
      { ...RELEASE, "NOTES.md": "a note\n" },
      /NOTES\.md \(A\) is not one of release-plz's/,
    ],
    "a manifest line that is not a version": [
      {
        ...RELEASE,
        "Cargo.toml": MANIFEST("0.2.0").replace('edition = "2021"', 'edition = "2024"'),
      },
      /Cargo\.toml changes more than a version line/,
    ],
    "a registry dependency's lock entry re-versioned": [
      { ...RELEASE, "Cargo.lock": LOCK("0.2.0").replace('"1.0.0"', '"1.0.1"') },
      /Cargo\.lock line \d+ is a version under \[package\], not this workspace's own package/,
    ],
    "a changelog line added below the released entries": [
      { ...RELEASE, "CHANGELOG.md": `${RELEASE["CHANGELOG.md"]}- slipped in later\n` },
      /CHANGELOG\.md adds lines other than one new section above its releases/,
    ],
  };
  for (const [what, [files, reason]] of Object.entries(NOT_RELEASE_PLZ)) {
    it(`runs them for ${what}`, (t) => {
      const repo = scratch(t);
      commitAll(repo.work, files, "chore: release v0.2.0");
      answers(repo.fake, pushRunPassed(repo.base));
      const result = shortcut(repo);
      assert.equal(result.stdout, "skip=false\n", result.stderr);
      assert.match(result.stderr, reason);
    });
  }

  it("runs them for a release file deleted", (t) => {
    const repo = scratch(t);
    commitAll(repo.work, RELEASE, "chore: release v0.2.0");
    git(repo.work, "rm", "--quiet", "CHANGELOG.md");
    git(repo.work, "commit", "--quiet", "--message", "chore: drop the changelog");
    answers(repo.fake, pushRunPassed(repo.base));
    const result = shortcut(repo);
    assert.equal(result.stdout, "skip=false\n", result.stderr);
    assert.match(result.stderr, /CHANGELOG\.md \(D\) is not one of release-plz's/);
  });

  it("runs them when the merged pull request is another commit's, or its head had another tree", (t) => {
    const repo = scratch(t);
    commitAll(repo.work, RELEASE, "chore: release v0.2.0");
    const green = (sha) => ({
      [`repos/${REPO}/commits/${sha}/check-runs?per_page=100`]: {
        check_runs: ["test-linux", "test-windows", "test-macos"].map((name) => ({
          name,
          conclusion: "success",
        })),
      },
    });
    answers(repo.fake, {
      ...noRuns(repo.base),
      [`repos/${REPO}/commits/${repo.base}/pulls`]: [
        { number: 9, merge_commit_sha: "a".repeat(40), head: { sha: "b".repeat(40) } },
        { number: 10, merge_commit_sha: repo.base, head: { sha: "c".repeat(40) } },
        { number: 11, merge_commit_sha: repo.base, head: {} },
      ],
      [`repos/${REPO}/git/commits/${"c".repeat(40)}`]: { tree: { sha: "d".repeat(40) } },
      ...green("b".repeat(40)),
      ...green("c".repeat(40)),
    });
    const result = shortcut(repo);
    assert.equal(result.stdout, "skip=false\n", result.stderr);
    assert.ok(
      !result.calls.some((call) => call.includes("b".repeat(40))),
      "it read the checks of a pull request that did not merge the base",
    );
  });

  it("runs them when GitHub answers in a shape it does not read", (t) => {
    const repo = scratch(t);
    commitAll(repo.work, RELEASE, "chore: release v0.2.0");
    answers(repo.fake, {
      [`repos/${REPO}/actions/runs?head_sha=${repo.base}&status=success&per_page=100`]: {
        workflow_runs: "not a list",
      },
    });
    const result = shortcut(repo);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "skip=false\n", result.stderr);
    assert.match(result.stderr, /in a shape this does not read/);
  });

  it("runs them, asking nothing, with no repository to ask", (t) => {
    const repo = scratch(t);
    commitAll(repo.work, RELEASE, "chore: release v0.2.0");
    const result = shortcut(repo, ["--base-ref", "main"]);
    assert.equal(result.stdout, "skip=false\n", result.stderr);
    assert.match(result.stderr, /no repository to ask/);
    assert.deepEqual(result.calls, []);
  });

  it("runs them when the head does not resolve, or shares no base with the branch", (t) => {
    const repo = scratch(t);
    const unresolved = shortcut(repo, ["--repo", REPO, "--head", "no-such-branch"]);
    assert.equal(unresolved.stdout, "skip=false\n", unresolved.stderr);
    assert.match(unresolved.stderr, /does not resolve to a commit/);
    git(repo.work, "checkout", "--quiet", "--orphan", "unrelated");
    commitAll(repo.work, { "other.txt": "unrelated\n" }, "an unrelated root");
    const unrelated = shortcut(repo);
    assert.equal(unrelated.stdout, "skip=false\n", unrelated.stderr);
    assert.match(unrelated.stderr, /no merge base with origin\/main/);
  });

  for (const [args, refusal] of [
    [["--repo"], /--repo needs a value/],
    [["--bogus", "x"], /unknown argument '--bogus'/],
    [["--base-ref", "-not-a-branch"], /is not a branch name/],
    [["--head", "a b"], /is not a revision/],
  ]) {
    it(`refuses ${args.join(" ")}`, () => {
      const result = spawnSync("node", [join(root, "scripts/release-pr-shortcut.mjs"), ...args], {
        encoding: "utf8",
      });
      assert.equal(result.status, 2, result.stderr);
      assert.equal(result.stdout, "");
      assert.match(result.stderr, refusal);
    });
  }

  it("refuses a repository that is not an owner/name", () => {
    const result = spawnSync(
      "node",
      [join(root, "scripts/release-pr-shortcut.mjs"), "--repo", "not a repo"],
      { encoding: "utf8" },
    );
    assert.equal(result.status, 2);
    assert.match(result.stderr, /is not an owner\/name/);
  });
});
