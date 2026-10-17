// release-plz.yml's one job, run step by step over a scratch repository.
//
// Each step's own `run:` is executed by bash, in the workflow's order, under its
// own `if:` — evaluated by the Actions expression engine the rest of these
// tests use — with the environment its `env:` names. The `uses:` steps are what
// the scratch repository already is: a full-history checkout, with the real
// `release-plz` on PATH.
//
// The one stand-in is where release-plz would write to the world. `release` runs
// as `release-plz release --dry-run` against a local GitHub, so it plans the tag
// and the Release and creates neither; `release-pr` runs as `release-plz update`,
// the same version bump and changelog written to the checkout instead of pushed
// to a branch and opened as a PR. So no registry, remote or forge is written.
//
// Outside the deterministic gate because it needs the `release-plz` binary;
// `just release-journeys` runs it.

import { spawn, spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { evaluate, workflows } from "../../.github/workflow-graph.mjs";
import { GitHubDouble } from "../../.github/tests/support/release-doubles.mjs";

const ROOT = process.cwd();
const workflow = workflows(ROOT)["release-plz.yml"];
const job = workflow.jobs["release-plz"];
const real = spawnSync("bash", ["-c", "command -v release-plz"], {
  encoding: "utf8",
}).stdout.trim();

function git(cwd, ...args) {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

function bash(script, { cwd, env }) {
  return new Promise((resolve, reject) => {
    const child = spawn("bash", ["-eo", "pipefail", "-c", script], { cwd, env });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (code) => resolve({ code, stdout, stderr }));
  });
}

/// A crate with one released version and one feature commit after it.
function scratchRepository(base) {
  const repo = join(base, "repo");
  mkdirSync(join(repo, "src"), { recursive: true });
  mkdirSync(join(repo, "scripts"));
  cpSync(
    join(ROOT, "scripts", "release-pr-auto-merge.sh"),
    join(repo, "scripts", "release-pr-auto-merge.sh"),
  );
  writeFileSync(
    join(repo, "Cargo.toml"),
    '[package]\nname = "scratchpkg"\nversion = "0.1.0"\nedition = "2021"\nlicense = "MIT"\ndescription = "scratch"\n',
  );
  writeFileSync(join(repo, "src", "main.rs"), "fn main() {}\n");
  // This repository's release-plz.toml, less the API check: the scratch crate
  // has no published baseline for cargo-semver-checks to read.
  writeFileSync(
    join(repo, "release-plz.toml"),
    readFileSync(join(ROOT, "release-plz.toml"), "utf8").replace(
      /^semver_check = true$/m,
      "semver_check = false",
    ),
  );
  git(repo, "init", "-q", "-b", "main");
  git(repo, "config", "user.email", "release@example.invalid");
  git(repo, "config", "user.name", "release");
  git(repo, "add", "-A");
  git(repo, "commit", "-qm", "feat: the first release");
  spawnSync("cargo", ["generate-lockfile", "--offline"], { cwd: repo });
  git(repo, "add", "-A");
  git(repo, "commit", "-qm", "chore: lock");
  git(repo, "tag", "v0.1.0");
  writeFileSync(join(repo, "src", "main.rs"), 'fn main() {\n    println!("hello");\n}\n');
  git(repo, "commit", "-qam", "feat: greet whoever runs it");
  return repo;
}

/// The stand-in for release-plz's writes to the world: the real binary, asked
/// to plan rather than do.
function releasePlzShim(bin, github, log) {
  mkdirSync(bin, { recursive: true });
  writeFileSync(
    join(bin, "release-plz"),
    `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >> ${JSON.stringify(log)}
command="$1"; shift
args=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --git-token) shift 2 ;;
    *) args+=("$1"); shift ;;
  esac
done
case "$command" in
  release) exec ${JSON.stringify(real)} release --dry-run --forge github \\
      --repo-url ${JSON.stringify(`${github.url}/${github.repository}`)} --git-token not-a-real-token "\${args[@]}" ;;
  release-pr) exec ${JSON.stringify(real)} update "\${args[@]}" ;;
  *) exec ${JSON.stringify(real)} "$command" "\${args[@]}" ;;
esac
`,
    { mode: 0o755 },
  );
}

/// Run every step of the job as Actions would, returning what each did.
async function runJob({ repo, base, github, token, headCommit }) {
  const bin = join(base, "bin");
  const log = join(base, "release-plz.log");
  writeFileSync(log, "");
  releasePlzShim(bin, github, log);
  const runnerTemp = mkdtempSync(join(base, "runner-temp-"));
  const context = {
    github: { event: { head_commit: { message: headCommit } }, repository: github.repository },
    env: { ...workflow.env },
    secrets: { RELEASE_PLZ_TOKEN: token },
    steps: {},
  };
  const ran = [];
  for (const [index, step] of job.steps.entries()) {
    const name = step.name ?? step.uses;
    if (step.if !== undefined && !evaluate(String(step.if), context)) {
      ran.push({ name, skipped: true });
      continue;
    }
    if (step.uses) {
      // The scratch repository is the checkout, with full history, and the tools
      // the install step names are the ones on PATH.
      if (step.uses.startsWith("actions/checkout")) assert.equal(step.with["fetch-depth"], 0);
      if (step.uses.startsWith("taiki-e/install-action")) {
        assert.match(step.with.tool, /release-plz@\$\{\{ env\.RELEASE_PLZ_VERSION \}\}/);
      }
      ran.push({ name, skipped: false });
      continue;
    }
    const output = join(runnerTemp, `output-${index}`);
    writeFileSync(output, "");
    const env = {
      PATH: `${bin}:${process.env.PATH}`,
      HOME: process.env.HOME,
      CARGO_HOME: process.env.CARGO_HOME ?? join(process.env.HOME, ".cargo"),
      RUSTUP_HOME: process.env.RUSTUP_HOME ?? join(process.env.HOME, ".rustup"),
      ...github.env(),
      RUNNER_TEMP: runnerTemp,
      GITHUB_OUTPUT: output,
    };
    for (const [key, value] of Object.entries(step.env ?? {})) {
      env[key] = String(value).replace(/\$\{\{(.*?)\}\}/g, (_, e) =>
        String(evaluate(e, context) ?? ""),
      );
    }
    const result = await bash(step.run, { cwd: repo, env });
    const outputs = Object.fromEntries(
      readFileSync(output, "utf8")
        .split("\n")
        .filter(Boolean)
        .map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]),
    );
    if (step.id)
      context.steps[step.id] = { outputs, outcome: result.code === 0 ? "success" : "failure" };
    ran.push({ name, skipped: false, ...result });
    assert.equal(result.code, 0, `step '${name}' failed:\n${result.stdout}\n${result.stderr}`);
  }
  const calls = readFileSync(log, "utf8").split("\n").filter(Boolean);
  return { ran, calls };
}

describe("release-plz.yml's job", () => {
  let base;
  let github;
  before(async () => {
    assert.ok(
      real,
      "release-plz is not on PATH: install it (`cargo install release-plz --locked`) to run these journeys",
    );
    base = mkdtempSync(
      join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "release-plz-job-"),
    );
    github = await new GitHubDouble().start();
  });
  after(async () => {
    await github.stop();
    rmSync(base, { recursive: true, force: true });
  });

  it("is one job whose first step is the guard every other step is conditioned on", () => {
    assert.deepEqual(Object.keys(workflow.jobs), ["release-plz"]);
    assert.equal(job.steps[0].id, "guard");
    for (const step of job.steps.slice(1)) {
      assert.match(
        String(step.if),
        /steps\.guard\.outputs\.enabled == 'true'/,
        step.name ?? step.uses,
      );
    }
  });

  // The cases release-plz.yml's `guard` job decided: the token is set, or it
  // is not.
  it("without RELEASE_PLZ_TOKEN, says how to enable releases and runs nothing else", async () => {
    const repo = scratchRepository(mkdtempSync(join(base, "no-token-")));
    const { ran, calls } = await runJob({
      repo,
      base,
      github,
      token: "",
      headCommit: "feat: greet whoever runs it",
    });
    assert.equal(ran[0].code, 0);
    assert.match(ran[0].stderr, /RELEASE_PLZ_TOKEN not set; skipping release automation\./);
    assert.deepEqual(
      ran
        .slice(1)
        .filter((step) => !step.skipped)
        .map((step) => step.name),
      [],
      "every step after the guard is skipped",
    );
    assert.deepEqual(calls, [], "release-plz never ran");
    assert.deepEqual(github.writes(), []);
  });

  it("on an eligible push, prepares the version bump and the changelog, and asks to auto-merge", async () => {
    const repo = scratchRepository(mkdtempSync(join(base, "push-")));
    // The release PR release-plz keeps open, beside an unrelated one.
    github.addPull(41, "feat/unrelated");
    github.addPull(42, "release-plz-2026-10-17T00-00-00Z");
    const before = github.requests.length;
    const { ran, calls } = await runJob({
      repo,
      base,
      github,
      token: "a-token",
      headCommit: "feat: greet whoever runs it",
    });
    assert.ok(
      ran.every((step) => !step.skipped),
      `every step ran: ${JSON.stringify(ran.map((s) => [s.name, s.skipped]))}`,
    );
    assert.equal(
      calls[0].split(" ")[0],
      "release",
      "release runs first, for a bump merged earlier",
    );
    assert.match(
      calls[1],
      /^release-pr --registry-manifest-path \S+\/release-baseline\/Cargo\.toml --git-token a-token$/,
    );
    const manifest = readFileSync(join(repo, "Cargo.toml"), "utf8");
    assert.match(manifest, /^version = "0\.(1\.1|2\.0)"$/m, "the version is bumped");
    const changelog = readFileSync(join(repo, "CHANGELOG.md"), "utf8");
    assert.match(changelog, /greet whoever runs it/);
    assert.equal(git(repo, "tag", "--list"), "v0.1.0", "no tag is created");
    assert.equal(git(repo, "remote"), "", "no remote exists, so none was written");
    assert.ok(github.requests.slice(before).some((r) => r.path.endsWith("/pulls")));
    assert.equal(github.graphql.length, 1, "one request to merge");
    assert.match(github.graphql[0].query, /enablePullRequestAutoMerge/);
    assert.equal(github.graphql[0].variables.id, "PR_node_42");
    assert.deepEqual(
      github.pulls.map((p) => [p.number, p.autoMerge ?? null]),
      [
        [41, null],
        [42, "SQUASH"],
      ],
    );
    assert.deepEqual(
      github.writes().filter((r) => r.path !== "/graphql"),
      [],
      "nothing else was written",
    );
  });

  it("on the release PR's merge, plans the tag and the GitHub Release and opens no new PR", async () => {
    const repo = scratchRepository(mkdtempSync(join(base, "merge-")));
    // The release PR, merged: what `release-pr` prepared, committed under the
    // subject release-plz gives it.
    const baseline = join(repo, "..", "baseline");
    git(repo, "worktree", "add", "-q", "--detach", baseline, "v0.1.0");
    const updated = spawnSync(
      real,
      ["update", "--registry-manifest-path", join(baseline, "Cargo.toml")],
      {
        cwd: repo,
        encoding: "utf8",
      },
    );
    assert.equal(updated.status, 0, updated.stderr);
    git(repo, "worktree", "remove", "--force", baseline);
    const version = readFileSync(join(repo, "Cargo.toml"), "utf8").match(
      /^version = "([^"]+)"$/m,
    )[1];
    assert.notEqual(version, "0.1.0");
    git(repo, "add", "-A");
    git(repo, "commit", "-qm", `chore: release v${version}`);
    const before = github.requests.length;
    const { ran, calls } = await runJob({
      repo,
      base,
      github,
      token: "a-token",
      headCommit: `chore: release v${version}`,
    });
    const release = ran.find((step) => step.name === "Tag and publish a merged version bump");
    assert.ok(release && !release.skipped);
    assert.ok(
      (release.stdout + release.stderr).includes(
        `due to dry, skipping the following: ["creation of tag 'v${version}'", "creation of git release"]`,
      ),
      `release-plz planned the tag and the Release:\n${release.stdout}\n${release.stderr}`,
    );
    for (const name of [
      "Open or update the release PR",
      "Merge the release PR automatically once its required checks pass",
    ]) {
      assert.equal(
        ran.find((step) => step.name === name)?.skipped,
        true,
        `${name} is skipped on a release commit`,
      );
    }
    assert.deepEqual(
      calls.map((c) => c.split(" ")[0]),
      ["release"],
    );
    assert.equal(git(repo, "tag", "--list"), "v0.1.0", "the tag is planned, not created");
    assert.deepEqual(
      github.requests.slice(before).filter((r) => r.method !== "GET"),
      [],
      "the forge was only read",
    );
  });
});
