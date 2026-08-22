// The judged tier's computation cache, driven rather than read.
//
// The judge is non-deterministic across the gap between what it judges — every
// file in the base-to-head diff — and what changed, so one tree judged against one
// base has to produce one verdict rather than a fresh sample per invocation. These
// journeys drive the real `just lint-llm-diff` recipe, the real `scripts/nx.sh`,
// the real Nx target definition, and the real fingerprint script inside a
// throwaway copy of this repository, and count how often the judge was actually
// asked.
//
// llmlint itself is the one thing replaced, and it is replaced by an executable on
// PATH — the same subprocess-double boundary `crates/testfakes` draws for the
// siblings. It is also the one boundary these journeys cannot use for real: the
// claim under test is that an unchanged tree answers the same twice, which a
// non-deterministic judge cannot demonstrate, and paying a model call per
// invocation would put a credential inside the offline gate. Everything the cache
// is made of — the recipe, Nx, git, the fingerprint, and the merged configuration
// it hashes — is real.

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  appendFileSync,
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

/// What the double reports, and what the recipe says about where a verdict came
/// from. A replayed run has to carry the first two verbatim, so they are what
/// separates a restored report from a fresh one.
const PASS_VERDICT = "fake-judge: 31 rules, 0 failed";
const FINDING = "fake-judge finding: robust_shell in scripts/llmlint-judge.sh";
const FAIL_VERDICT = "fake-judge: 30 rules, 1 failed";
const CACHE_HIT = "replayed the recorded verdict for base";
const CACHE_MISS = "judged this diff against base";

/// An `llmlint` that counts judge runs instead of paying for them.
///
/// `config` is answered from the files a real merge would read — this checkout's
/// `llmlint.yml` and every absolute plugin path it pins — so a rule change inside
/// or outside the tree reaches the fingerprint the way a real one would.
const FAKE_LLMLINT = `#!/usr/bin/env bash
set -euo pipefail
if [[ \${1:-} == "--version" ]]; then
  echo "llmlint \${FAKE_LLMLINT_VERSION:-0.0.0-e2e}"
  exit 0
fi
if [[ \${1:-} == "config" ]]; then
  [[ \${FAKE_LLMLINT_CONFIG_EXIT:-0} == 0 ]] || exit "$FAKE_LLMLINT_CONFIG_EXIT"
  # The one environment-resolved value a real \`llmlint config\` renders, so a
  # fingerprint that read the caller's copy of it would split this key too.
  echo "oneharness bin: \${LLMLINT_ONEHARNESS_BIN:-null}"
  cat llmlint.yml
  for plugin in $(sed -n 's/^ *- *"\\(\\/[^"]*\\)".*/\\1/p' llmlint.yml); do cat "$plugin"; done
  exit 0
fi
# One line per judge run: the arguments the tier actually asked for.
printf "%s\\n" "$*" >>"$FAKE_LLMLINT_LOG"
if [[ \${FAKE_LLMLINT_EXIT:-0} != 0 ]]; then
  echo "${FINDING}"
  echo "${FAIL_VERDICT}"
  exit "$FAKE_LLMLINT_EXIT"
fi
echo "${PASS_VERDICT}"
`;

/// An `llmlint` that can report a version and nothing else, for the journeys about
/// a caller's PATH: reaching this one to judge or to merge a config is a failure.
function versionOnlyLlmlint(version) {
  return `#!/usr/bin/env bash
set -euo pipefail
[[ \${1:-} == "--version" ]] || { echo "ambient llmlint reached $1" >&2; exit 2; }
echo "llmlint ${version}"
`;
}

function writeExecutable(path, body) {
  writeFileSync(path, body, "utf8");
  chmodSync(path, 0o755);
}

/// Install an `llmlint` in its own directory and hand back that directory.
function installLlmlint(directory, body) {
  mkdirSync(directory, { recursive: true });
  writeExecutable(join(directory, "llmlint"), body);
  return directory;
}

/// Copy exactly the files Nx would hash: everything git would commit from here.
///
/// Copying the checkout rather than judging this one is what lets a journey commit,
/// rewrite a rule, or advance a base without touching the tree it runs from. Nx
/// skips ignored state, so `node_modules` — which it needs and which is far too
/// large to duplicate — arrives as a symlink to this checkout's own install.
function copyCheckout(root) {
  mkdirSync(root, { recursive: true });
  execFileSync(
    "bash",
    [
      "-c",
      'git ls-files -z --cached --others --exclude-standard | tar --null -T - -cf - | tar -xf - -C "$1"',
      "--",
      root,
    ],
    { cwd: REPO_ROOT, encoding: "utf8" },
  );
  symlinkSync(join(REPO_ROOT, "node_modules"), join(root, "node_modules"));
}

/// A throwaway checkout wired to count judge runs instead of paying for them.
class Workspace {
  constructor(sandbox) {
    this.sandbox = sandbox;
    this.root = join(sandbox, "checkout");
    this.plugin = join(sandbox, "external-plugin.yml");
    this.judgeLog = join(sandbox, "judge-runs.log");

    copyCheckout(this.root);
    // A plugin outside the tree: no file input can see it, so only the judge
    // configuration fingerprint can notice when its rules change.
    writeFileSync(
      this.plugin,
      "version: 1\nrules:\n  - name: plugin_rule\n    description: The change documents every new operator entry point.\n",
      "utf8",
    );
    writeFileSync(
      join(this.root, "llmlint.yml"),
      `files:\n  exclude:\n    - "**/.git/**"\nplugins:\n  - "${this.plugin}"\n`,
      "utf8",
    );
    writeFileSync(this.judgeLog, "", "utf8");

    // The toolchain lives where `scripts/setup-llmlint.sh` installs it, because
    // that is the directory `scripts/llmlint-runtime-env.sh` puts first for both
    // ends of the tier. Reaching it is therefore a claim about the runtime
    // environment, not about the PATH this suite happens to run under.
    const home = join(sandbox, "home");
    installLlmlint(join(home, ".local", "bin"), FAKE_LLMLINT);

    this.env = { ...process.env };
    for (const inherited of [
      "LLMLINT_ONEHARNESS_BIN",
      "LLMLINT_DIFF_BASE_SHA",
      "NX_SKIP_NX_CACHE",
      "NX_DISABLE_NX_CACHE",
    ]) {
      delete this.env[inherited];
    }
    Object.assign(this.env, {
      HOME: home,
      XDG_CACHE_HOME: join(sandbox, "cache"),
      FAKE_LLMLINT_LOG: this.judgeLog,
    });

    this.git("init", "-q");
    this.commit("checkout under test");
  }

  /// Run the recipe an operator, the `gate` recipe, and CI all invoke.
  lint(base, { args = [], env = {} } = {}) {
    const environment = { ...this.env, ...env };
    for (const [name, value] of Object.entries(env)) {
      if (value === undefined) delete environment[name];
    }
    return spawnSync("just", ["lint-llm-diff", base, ...args], {
      cwd: this.root,
      encoding: "utf8",
      env: environment,
    });
  }

  /// Run the fingerprint the way an operator diagnosing a cache miss would.
  fingerprint({ env = {} } = {}) {
    return spawnSync("bash", ["scripts/llmlint-fingerprint.sh"], {
      cwd: this.root,
      encoding: "utf8",
      env: { ...this.env, ...env },
    });
  }

  /// How many times the judge was actually asked, across every run so far.
  judgeRuns() {
    return readFileSync(this.judgeLog, "utf8").split("\n").filter(Boolean);
  }

  /// Prepend an `llmlint` to the caller's PATH, as a shell that never ran setup has.
  onAmbientLlmlint(name, version) {
    const directory = installLlmlint(join(this.sandbox, name), versionOnlyLlmlint(version));
    return { PATH: `${directory}${delimiter}${this.env.PATH}` };
  }

  git(...args) {
    return execFileSync("git", ["-c", "user.name=e2e", "-c", "user.email=e2e@invalid", ...args], {
      cwd: this.root,
      encoding: "utf8",
    });
  }

  commit(message, { allowEmpty = false } = {}) {
    this.git("add", "-A");
    this.git("commit", "-q", "-m", message, ...(allowEmpty ? ["--allow-empty"] : []));
    return this.head();
  }

  head() {
    return this.git("rev-parse", "HEAD").trim();
  }
}

/// A fresh checkout, cache and all, removed when the test that asked for it ends.
function workspace(t) {
  const sandbox = mkdtempSync(join(tmpdir(), "onepipeline-llmlint-cache-"));
  t.after(() => rmSync(sandbox, { recursive: true, force: true }));
  return new Workspace(sandbox);
}

/// Both streams, which is where a run's report and its provenance line both are.
function report(result) {
  return `${result.stdout}${result.stderr}`;
}

describe("the judged tier's computation cache", () => {
  it("replays the first verdict for an unchanged tree and an unchanged base", (t) => {
    const ws = workspace(t);
    const base = ws.head();

    const first = ws.lint(base);
    const second = ws.lint(base);

    assert.equal(first.status, 0, report(first));
    assert.equal(second.status, 0, report(second));
    assert.deepEqual(ws.judgeRuns(), [`--diff --diff-base ${base}`], "the judge was asked twice");
    // The restored run says what the fresh one said: the report is the record.
    for (const result of [first, second]) assert.match(result.stdout, new RegExp(PASS_VERDICT));
    // "Green" is a claim about one base commit, so the provenance line names it:
    // a gate run and a CI run resolving different bases answer different questions.
    assert.match(first.stderr, new RegExp(`${CACHE_MISS} ${base}`));
    assert.match(second.stderr, new RegExp(`${CACHE_HIT} ${base}`));
  });

  it("judges again when the workspace changes", (t) => {
    const ws = workspace(t);
    const base = ws.head();
    ws.lint(base);

    appendFileSync(join(ws.root, "README.md"), "\nJudged again.\n", "utf8");
    const second = ws.lint(base);

    assert.equal(second.status, 0, report(second));
    assert.equal(ws.judgeRuns().length, 2);
    assert.match(second.stderr, new RegExp(CACHE_MISS));
  });

  it("judges again when the base commit advances, then replays per base", (t) => {
    const ws = workspace(t);
    const original = ws.head();
    ws.lint(original);

    // Identical tree, advanced base: only the comparison differs, so a hit here
    // would replay a verdict computed against a different question.
    const advanced = ws.commit("advance the base", { allowEmpty: true });
    assert.notEqual(advanced, original);
    const moved = ws.lint(advanced);
    const repeated = ws.lint(advanced);

    assert.equal(ws.judgeRuns().length, 2);
    assert.match(moved.stderr, new RegExp(CACHE_MISS));
    assert.match(repeated.stderr, new RegExp(CACHE_HIT));
  });

  it("judges again when a rule pinned outside the tree changes", (t) => {
    const ws = workspace(t);
    const base = ws.head();
    ws.lint(base);

    // The plugin lives outside the checkout, so the tree Nx hashes is
    // byte-identical: only the judge configuration fingerprint can see this.
    appendFileSync(ws.plugin, "    False when it adds an entry point silently.\n", "utf8");
    const second = ws.lint(base);

    assert.equal(second.status, 0, report(second));
    assert.equal(ws.judgeRuns().length, 2);
    assert.match(second.stderr, new RegExp(CACHE_MISS));
  });

  it("judges again when the installed llmlint version changes", (t) => {
    const ws = workspace(t);
    const base = ws.head();
    ws.lint(base, { env: { FAKE_LLMLINT_VERSION: "0.4.0" } });

    const second = ws.lint(base, { env: { FAKE_LLMLINT_VERSION: "0.5.0" } });

    assert.equal(second.status, 0, report(second));
    assert.equal(ws.judgeRuns().length, 2);
    assert.match(second.stderr, new RegExp(CACHE_MISS));
  });

  it("keys on the judge configuration the target runs with, not the caller's", (t) => {
    // A caller's `LLMLINT_ONEHARNESS_BIN` says where its harness binary lives, and
    // a real `llmlint config` renders it. Reading it would give one judged diff a
    // different key per dispatch — the split verdict this cache exists to end.
    const ws = workspace(t);
    const base = ws.head();

    const first = ws.lint(base, { env: { LLMLINT_ONEHARNESS_BIN: "/caller/one/oneharness" } });
    const second = ws.lint(base, { env: { LLMLINT_ONEHARNESS_BIN: "/caller/two/oneharness" } });

    assert.equal(first.status, 0, report(first));
    assert.equal(second.status, 0, report(second));
    assert.equal(ws.judgeRuns().length, 1);
    assert.match(second.stderr, new RegExp(CACHE_HIT));
  });

  it("resolves both ends of the key past an unrelated llmlint on the caller's PATH", (t) => {
    // A cache hit alone would not prove this: Nx scores a runtime input that exits
    // non-zero as *no contribution* rather than as an error, so a fingerprint the
    // caller's environment can break also produces one — both runs sharing a key
    // that no longer describes the judge. So the fingerprint is read directly too:
    // it has to resolve under each ambient llmlint, and to the same digest.
    const ws = workspace(t);
    const base = ws.head();
    const onFirst = { env: ws.onAmbientLlmlint("ambient-one", "1.0.0") };
    const onSecond = { env: ws.onAmbientLlmlint("ambient-two", "2.0.0") };

    const first = ws.lint(base, onFirst);
    const second = ws.lint(base, onSecond);
    const firstDigest = ws.fingerprint(onFirst);
    const secondDigest = ws.fingerprint(onSecond);

    assert.equal(first.status, 0, report(first));
    assert.equal(second.status, 0, report(second));
    assert.equal(ws.judgeRuns().length, 1);
    assert.match(second.stderr, new RegExp(CACHE_HIT));
    assert.equal(firstDigest.status, 0, report(firstDigest));
    assert.equal(secondDigest.status, 0, report(secondDigest));
    assert.notEqual(firstDigest.stdout.trim(), "");
    assert.equal(firstDigest.stdout.trim(), secondDigest.stdout.trim());
  });

  it("still invalidates on a rule change while that unrelated llmlint sits on PATH", (t) => {
    // The other half of the same claim, and the worse failure it guards: a
    // spurious miss only re-rolls the judge, but a key the fingerprint dropped out
    // of replays a verdict the judge configuration has since moved on from.
    const ws = workspace(t);
    const base = ws.head();
    const ambient = { env: ws.onAmbientLlmlint("ambient-judge", "1.0.0") };
    ws.lint(base, ambient);

    appendFileSync(ws.plugin, "    False when it adds an entry point silently.\n", "utf8");
    const second = ws.lint(base, ambient);

    assert.equal(second.status, 0, report(second));
    assert.equal(ws.judgeRuns().length, 2);
    assert.match(second.stderr, new RegExp(CACHE_MISS));
  });

  it("refuses to judge, or to replay, when the fingerprint cannot be produced", (t) => {
    const ws = workspace(t);
    const base = ws.head();
    ws.lint(base);

    const broken = ws.lint(base, { env: { FAKE_LLMLINT_CONFIG_EXIT: "3" } });

    assert.notEqual(broken.status, 0, report(broken));
    // The stored green from the first run is still there, and must not answer for
    // a judge configuration nothing could read.
    assert.doesNotMatch(broken.stderr, new RegExp(CACHE_HIT));
    assert.match(broken.stderr, /'llmlint config' failed/);
    assert.match(broken.stderr, /refusing to judge without the judge-configuration fingerprint/);
    assert.equal(ws.judgeRuns().length, 1);
  });

  it("fails the tier and judges again when the judge reports findings", (t) => {
    const ws = workspace(t);
    const base = ws.head();

    const first = ws.lint(base, { env: { FAKE_LLMLINT_EXIT: "1" } });
    const second = ws.lint(base, { env: { FAKE_LLMLINT_EXIT: "1" } });

    assert.equal(ws.judgeRuns().length, 2);
    for (const result of [first, second]) {
      assert.notEqual(result.status, 0, report(result));
      assert.match(report(result), new RegExp(FINDING));
      assert.match(report(result), new RegExp(FAIL_VERDICT));
      assert.match(result.stderr, new RegExp(CACHE_MISS));
    }
  });

  it("fails the tier and judges again when the toolchain never reaches a verdict", (t) => {
    const ws = workspace(t);
    const base = ws.head();

    const first = ws.lint(base, { env: { FAKE_LLMLINT_EXIT: "2" } });
    const second = ws.lint(base, { env: { FAKE_LLMLINT_EXIT: "2" } });

    assert.equal(ws.judgeRuns().length, 2);
    for (const result of [first, second]) {
      assert.notEqual(result.status, 0, report(result));
      assert.match(result.stderr, new RegExp(CACHE_MISS));
    }
  });

  it("caches the green that replaced a red", (t) => {
    // The path a worker actually walks: judge, clear the finding, judge again,
    // then settle without paying for a third roll.
    const ws = workspace(t);
    const base = ws.head();

    const red = ws.lint(base, { env: { FAKE_LLMLINT_EXIT: "1" } });
    appendFileSync(join(ws.root, "README.md"), "\nThe finding, cleared.\n", "utf8");
    const green = ws.lint(base);
    const settled = ws.lint(base);

    assert.notEqual(red.status, 0, report(red));
    assert.equal(green.status, 0, report(green));
    assert.equal(settled.status, 0, report(settled));
    assert.equal(ws.judgeRuns().length, 2);
    assert.match(green.stderr, new RegExp(CACHE_MISS));
    assert.match(settled.stderr, new RegExp(CACHE_HIT));
  });

  it("re-judges per invocation with --skip-nx-cache, and ignores an ambient global skip", (t) => {
    // The supported re-judge lever is per-invocation on purpose: an exported
    // global skip would re-roll a non-deterministic judge from every unrelated
    // command, and silently break the checks whose contract is cache replay.
    const ws = workspace(t);
    const base = ws.head();
    ws.lint(base);

    const forced = ws.lint(base, { args: ["--skip-nx-cache"], env: { NX_SKIP_NX_CACHE: "true" } });
    const ambient = ws.lint(base, { env: { NX_DISABLE_NX_CACHE: "true" } });

    assert.equal(forced.status, 0, report(forced));
    assert.equal(ambient.status, 0, report(ambient));
    assert.equal(ws.judgeRuns().length, 2, "the ambient skip re-rolled the judge");
    assert.match(forced.stderr, new RegExp(CACHE_MISS));
    assert.match(ambient.stderr, new RegExp(CACHE_HIT));
    for (const result of [forced, ambient]) {
      assert.match(result.stderr, /ignoring the ambient global Nx cache skip/);
      assert.match(result.stderr, new RegExp(`just lint-llm-diff ${base} --skip-nx-cache`));
    }
  });

  it("refuses a base it cannot resolve before the judge is paid", (t) => {
    const ws = workspace(t);

    const result = ws.lint("no-such-ref");

    assert.notEqual(result.status, 0, report(result));
    assert.match(result.stderr, /'no-such-ref' does not resolve to a commit/);
    assert.equal(ws.judgeRuns().length, 0);
  });
});

describe("the judged tier's toolchain directory", () => {
  // Two files name it and neither can reference the other: the setup script that
  // installs llmlint there, and the runtime environment that puts it first for
  // both ends of the cache key. A rename that missed one would leave the
  // fingerprint resolving a different binary than the one setup installed.
  it("is spelled the same way by the script that installs it and the one that resolves it", () => {
    const installer = readFileSync(join(REPO_ROOT, "scripts", "setup-llmlint.sh"), "utf8");
    const runtime = readFileSync(join(REPO_ROOT, "scripts", "llmlint-runtime-env.sh"), "utf8");
    for (const [name, text] of [
      ["setup-llmlint.sh", installer],
      ["llmlint-runtime-env.sh", runtime],
    ]) {
      assert.ok(
        text.includes('"$HOME/.local/bin"'),
        `${name} no longer names "$HOME/.local/bin" as the llmlint install directory`,
      );
    }
  });
});
