// The judged tier, driven the way CI, the gate and a worker drive it.
//
// Each journey runs the real `just lint-llm-diff` recipe in a throwaway copy of
// this repository, through the real `scripts/nx.sh`, the real Nx target and cache,
// and the real installed `llmlint` — whose `fingerprint` keys that cache. The paid
// model call is the one thing replaced: `LLMLINT_ONEHARNESS_BIN` names a stand-in
// oneharness that answers each judge call from `STANDIN_VERDICT` and records it,
// and hands everything else to the real oneharness installed beside llmlint.
//
// A replay is recognised by the stand-in recording no judge call and by Nx's own
// cache line, never by a task annotation, which differs between the copy that
// recorded a hit and a fresh one.

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  appendFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, "..", "..");
const STANDIN = join(HERE, "oneharness-standin.mjs");

const SUMMARY = /^\d+ rules: .*$/m;
const CACHE_HIT =
  /Nx read the output from the cache instead of running the command|Cache: 1\/1 hit/;

/// The installed llmlint and the oneharness `just setup-llmlint` installs beside it.
///
/// Missing is a failure, not a skip: this tier's proof is about the real llmlint.
function installedToolchain() {
  const found = spawnSync("bash", ["-c", "command -v llmlint"], { encoding: "utf8" });
  assert.equal(found.status, 0, "llmlint is not on PATH; run 'just setup-llmlint'");
  const llmlint = realpathSync(found.stdout.trim());
  return { oneharness: join(dirname(llmlint), "oneharness") };
}

const RULES = `files:
  include: ["notes/**"]
rules:
  - name: notes_are_kind
    description: Every note is written kindly.
  - name: notes_are_short
    description: Every note is at most three lines long.
`;

/// A throwaway copy of this repository, with a judge configuration of its own.
///
/// The copy is what git would commit from here, so Nx loads the same graph and
/// target; `node_modules`, which Nx needs and git ignores, is linked rather than
/// copied. Its `llmlint.yml` declares two rules over `notes/` and no plugins, so
/// no judge configuration is fetched and none of this checkout's files are judged.
class Workspace {
  constructor(t) {
    this.sandbox = mkdtempSync(join(tmpdir(), "onepipeline-lint-llm-diff-"));
    t.after(() => rmSync(this.sandbox, { recursive: true, force: true, maxRetries: 5 }));
    this.root = join(this.sandbox, "checkout");
    this.judgeLog = join(this.sandbox, "judge-calls.jsonl");
    mkdirSync(this.root);
    execFileSync(
      "bash",
      [
        "-c",
        'git ls-files -z --cached --others --exclude-standard | tar --null -T - -cf - | tar -xf - -C "$1"',
        "--",
        this.root,
      ],
      { cwd: REPO_ROOT },
    );
    symlinkSync(join(REPO_ROOT, "node_modules"), join(this.root, "node_modules"));
    writeFileSync(join(this.root, "llmlint.yml"), RULES);
    writeFileSync(this.judgeLog, "");

    const env = {};
    for (const [name, value] of Object.entries(process.env)) {
      // Whatever launched this suite — an outer Nx, a git hook, a CI runner, an
      // agent — says nothing about the run under test; a journey sets its own.
      if (/^(NX_|GIT_|LLMLINT_|ONEHARNESS_)/.test(name)) continue;
      if (["CI", "CLAUDECODE", "CODEX_THREAD_ID", "FORCE_COLOR", "NO_COLOR"].includes(name))
        continue;
      env[name] = value;
    }
    const home = join(this.sandbox, "home");
    mkdirSync(home);
    this.env = {
      ...env,
      HOME: home,
      XDG_CONFIG_HOME: join(home, ".config"),
      XDG_CACHE_HOME: join(home, ".cache"),
      XDG_STATE_HOME: join(home, ".local", "state"),
      GIT_AUTHOR_NAME: "journey",
      GIT_AUTHOR_EMAIL: "journey@example.invalid",
      GIT_COMMITTER_NAME: "journey",
      GIT_COMMITTER_EMAIL: "journey@example.invalid",
      LLMLINT_ONEHARNESS_BIN: STANDIN,
      STANDIN_REAL_ONEHARNESS: installedToolchain().oneharness,
      STANDIN_LOG: this.judgeLog,
      STANDIN_FILE: "notes/first.md",
      STANDIN_VERDICT: "pass",
    };

    this.git("init", "-q", "-b", "main");
    this.base = this.commit("the repository as the base");
    this.git("branch", "judged-base");
    this.write("notes/first.md", "A kind, short note.\n");
    this.commit("add a note");
  }

  git(...args) {
    return execFileSync("git", ["-c", "gc.auto=0", ...args], {
      cwd: this.root,
      encoding: "utf8",
      env: this.env,
    });
  }

  write(path, body) {
    mkdirSync(dirname(join(this.root, path)), { recursive: true });
    writeFileSync(join(this.root, path), body);
  }

  commit(message) {
    this.git("add", "-A");
    this.git("commit", "-q", "-m", message);
    return this.git("rev-parse", "HEAD").trim();
  }

  /// `just lint-llm-diff`, with both streams piped, so neither is a terminal.
  lint(base, env = {}) {
    const result = spawnSync("just", ["lint-llm-diff", base], {
      cwd: this.root,
      encoding: "utf8",
      env: { ...this.env, ...env },
    });
    assert.equal(result.error, undefined, String(result.error));
    return { ...result, output: `${result.stdout}${result.stderr}` };
  }

  /// The digest the target's `runtime` input reads, from the real llmlint.
  fingerprint(env = {}) {
    return execFileSync("llmlint", ["fingerprint"], {
      cwd: this.root,
      encoding: "utf8",
      env: { ...this.env, ...env },
    }).trim();
  }

  judgeCalls() {
    return readFileSync(this.judgeLog, "utf8").split("\n").filter(Boolean).length;
  }
}

/// Run once, expecting the judge to be asked; return the pass's summary line.
function judged(ws, base, env = {}) {
  const before = ws.judgeCalls();
  const run = ws.lint(base, env);
  assert.equal(run.status, 0, run.output);
  assert.ok(ws.judgeCalls() > before, `expected a judge call:\n${run.output}`);
  assert.doesNotMatch(run.output, CACHE_HIT, run.output);
  const summary = run.output.match(SUMMARY)?.[0];
  assert.ok(summary, `no llmlint summary line:\n${run.output}`);
  return summary;
}

/// Run once, expecting Nx to replay `summary` with no judge call.
function replayed(ws, base, summary, env = {}) {
  const before = ws.judgeCalls();
  const run = ws.lint(base, env);
  assert.equal(run.status, 0, run.output);
  assert.equal(ws.judgeCalls(), before, `expected no judge call:\n${run.output}`);
  assert.match(run.output, CACHE_HIT, run.output);
  assert.equal(run.output.match(SUMMARY)?.[0], summary, run.output);
}

describe("just lint-llm-diff", () => {
  it("judges a first pass against the resolved base, then replays it unchanged", (t) => {
    const ws = new Workspace(t);
    const first = ws.lint("judged-base");
    assert.equal(first.status, 0, first.output);
    assert.ok(first.output.includes(ws.base), `the resolved base is not printed:\n${first.output}`);
    assert.equal(ws.judgeCalls(), 1, first.output);
    const summary = first.output.match(SUMMARY)?.[0];
    assert.ok(summary, first.output);

    replayed(ws, "judged-base", summary);
  });

  it("judges a new base commit again, and replays each base's own pass", (t) => {
    const ws = new Workspace(t);
    judged(ws, "judged-base");
    const second = ws.git("rev-parse", "HEAD").trim();
    ws.write("notes/second.md", "Another kind note.\n");
    ws.commit("add a second note");

    const againstFirstAfter = judged(ws, "judged-base");
    const againstSecond = judged(ws, second);
    replayed(ws, "judged-base", againstFirstAfter);
    replayed(ws, second, againstSecond);
  });

  it("judges again when a rule in llmlint.yml changes", (t) => {
    const ws = new Workspace(t);
    const summary = judged(ws, "judged-base");
    replayed(ws, "judged-base", summary);

    const before = ws.fingerprint();
    ws.write("llmlint.yml", RULES.replace("at most three lines", "at most one line"));
    assert.notEqual(ws.fingerprint(), before, "the rule change left the digest unchanged");
    const changed = judged(ws, "judged-base");
    replayed(ws, "judged-base", changed);

    // A rule in a plugin outside the tree moves the digest and nothing else Nx
    // hashes, so only the digest can send it back to the judge.
    const plugin = join(ws.sandbox, "plugin.llmlint.yml");
    const pluginRule = (text) =>
      `version: 1\nrules:\n  - name: notes_are_dated\n    description: ${text}\n`;
    writeFileSync(plugin, pluginRule("Every note names the day it was written."));
    ws.write("llmlint.yml", `${RULES}plugins:\n  - "${plugin}"\n`);
    const withPlugin = judged(ws, "judged-base");
    const unedited = ws.fingerprint();
    writeFileSync(plugin, pluginRule("Every note names the year it was written."));
    assert.notEqual(ws.fingerprint(), unedited, "the plugin rule change left the digest unchanged");
    judged(ws, "judged-base");
    writeFileSync(plugin, pluginRule("Every note names the day it was written."));
    assert.equal(ws.fingerprint(), unedited);
    replayed(ws, "judged-base", withPlugin);
  });

  it("judges again when the harness configuration's model changes", (t) => {
    const ws = new Workspace(t);
    judged(ws, "judged-base");

    const before = ws.fingerprint();
    const harness = join(ws.root, "oneharness.toml");
    const config = readFileSync(harness, "utf8");
    assert.match(config, /model = "claude-opus-4-8"/);
    writeFileSync(
      harness,
      config.replace('model = "claude-opus-4-8"', 'model = "claude-opus-5-5"'),
    );
    assert.notEqual(ws.fingerprint(), before, "the model change left the digest unchanged");
    const changed = judged(ws, "judged-base");
    replayed(ws, "judged-base", changed);

    // The same move made only through oneharness's environment override leaves
    // the tree untouched, so the digest is all that can tell the two judges apart:
    // it re-judges under the override, and dropping it replays the pass above.
    const overridden = { ONEHARNESS_MODEL: "claude-haiku-5-5" };
    assert.notEqual(ws.fingerprint(overridden), ws.fingerprint());
    judged(ws, "judged-base", overridden);
    replayed(ws, "judged-base", changed);
  });

  it("judges again when a tracked file changes", (t) => {
    const ws = new Workspace(t);
    judged(ws, "judged-base");
    appendFileSync(join(ws.root, "notes", "first.md"), "And a second kind line.\n");
    const changed = judged(ws, "judged-base");
    replayed(ws, "judged-base", changed);
  });

  it("fails on a finding with llmlint's report in its output, and judges again", (t) => {
    const ws = new Workspace(t);
    // What CI, the pre-push hook and a dispatched worker look like to Nx and
    // llmlint: piped streams, `CI`, and an AI agent's marker.
    const failing = { STANDIN_VERDICT: "fail", CI: "true", CLAUDECODE: "1" };
    for (const attempt of [1, 2]) {
      const run = ws.lint("judged-base", failing);
      assert.notEqual(run.status, 0, run.output);
      assert.match(run.output, /FAIL notes_are_kind/, run.output);
      assert.match(run.output, /notes\/first\.md:1: stand-in violation/, run.output);
      assert.equal(ws.judgeCalls(), attempt, `attempt ${attempt} was not judged:\n${run.output}`);
    }
  });

  for (const [verdict, unanswered] of [
    ["none", ["notes_are_kind", "notes_are_short"]],
    ["some", ["notes_are_short"]],
  ]) {
    it(`fails a judge reply answering ${verdict} of the rules, naming the rest, and judges again`, (t) => {
      const ws = new Workspace(t);
      for (const attempt of [1, 2]) {
        const run = ws.lint("judged-base", { STANDIN_VERDICT: verdict });
        assert.notEqual(run.status, 0, run.output);
        for (const rule of unanswered) {
          assert.match(
            run.output,
            new RegExp(`rule ${rule}: the judge returned no verdict for it`),
            run.output,
          );
        }
        if (verdict === "some") {
          assert.doesNotMatch(run.output, /rule notes_are_kind: the judge returned no verdict/);
        }
        assert.equal(ws.judgeCalls(), attempt, run.output);
      }
    });
  }

  it("refuses a base that does not resolve, naming it, before any judge call", (t) => {
    const ws = new Workspace(t);
    const run = ws.lint("no-such-base");
    assert.notEqual(run.status, 0, run.output);
    assert.match(run.stderr, /'no-such-base' does not resolve to a commit/, run.output);
    assert.equal(ws.judgeCalls(), 0, run.output);
  });
});
