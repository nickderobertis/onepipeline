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
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import {
  appendFileSync,
  chmodSync,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { describe, it } from "node:test";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

/// What the double reports, and what the recipe says about where a verdict came
/// from. A replayed run has to carry the first two verbatim, so they are what
/// separates a restored report from a fresh one.
const PASS_VERDICT = "31 rules: 11 passed, 0 failed";
const FINDING = "FAIL robust_shell in scripts/llmlint-judge.sh";
const FAIL_VERDICT = "31 rules: 10 passed, 1 failed";
//// The pointer llmlint prints to the run behind a verdict. A replayed verdict has
//// to carry it: it is where the report a one-line pass elides stays retrievable.
const POINTER = "llmlint history 20260823T-fake";
const CACHE_HIT = "replayed the recorded verdict for base";
const CACHE_MISS = "judged this diff against base";

/// An `llmlint` that counts judge runs instead of paying for them.
///
/// `config` is answered from the files a real merge would read — this checkout's
/// `llmlint.yml` and every absolute plugin path it pins — so a rule change inside
/// or outside the tree reaches the fingerprint the way a real one would.
const FAKE_LLMLINT = `#!/usr/bin/env bash
set -euo pipefail
if [[ \${NX_DAEMON:-} != "false" ]]; then
  echo "sandbox Nx daemon was not disabled" >&2
  exit 9
fi
if [[ \${1:-} == "--version" ]]; then
  [[ \${FAKE_LLMLINT_VERSION_EXIT:-0} == 0 ]] || exit "$FAKE_LLMLINT_VERSION_EXIT"
  [[ -z \${FAKE_LLMLINT_VERSION_EMPTY:-} ]] || exit 0
  echo "llmlint \${FAKE_LLMLINT_VERSION:-0.0.0-e2e}"
  exit 0
fi
if [[ \${1:-} == "config" ]]; then
  [[ \${FAKE_LLMLINT_CONFIG_EXIT:-0} == 0 ]] || exit "$FAKE_LLMLINT_CONFIG_EXIT"
  [[ -z \${FAKE_LLMLINT_CONFIG_EMPTY:-} ]] || exit 0
  # The one environment-resolved value a real \`llmlint config\` renders, so a
  # fingerprint that read the caller's copy of it would split this key too.
  echo "oneharness bin: \${LLMLINT_ONEHARNESS_BIN:-null}"
  # A real merged config names the checkout every config file was resolved in, and
  # so does this: the digest has to be about the rules, not about where they live.
  echo "config file: $PWD/llmlint.yml"
  cat llmlint.yml
  for plugin in $(sed -n 's/^ *- *"\\(\\/[^"]*\\)".*/\\1/p' llmlint.yml); do cat "$plugin"; done
  exit 0
fi
# One line per judge run: the arguments the tier actually asked for.
printf "%s\\n" "$*" >>"$FAKE_LLMLINT_LOG"
if [[ \${FAKE_LLMLINT_EXIT:-0} != 0 ]]; then
  if [[ -z \${FAKE_LLMLINT_SILENT:-} ]]; then
    echo "${FINDING}"
    echo "${FAIL_VERDICT}"
    echo 'See full results with \`${POINTER}\`'
  fi
  exit "$FAKE_LLMLINT_EXIT"
fi
if [[ -z \${FAKE_LLMLINT_NO_VERDICT:-} ]]; then
  echo "${PASS_VERDICT}\${FAKE_LLMLINT_NOTE:+ ($FAKE_LLMLINT_NOTE)}"
fi
if [[ -z \${FAKE_LLMLINT_NO_POINTER:-} ]]; then
  echo 'See full results with \`${POINTER}\`'
fi
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
      // Whether Nx colours its output decides which shape the recipe has to read
      // a verdict's provenance out of, so each journey states its own answer
      // rather than inheriting one — this suite itself runs inside an Nx task,
      // which exports `FORCE_COLOR` to everything it starts.
      "FORCE_COLOR",
      "NO_COLOR",
    ]) {
      delete this.env[inherited];
    }
    Object.assign(this.env, {
      HOME: home,
      XDG_CACHE_HOME: join(sandbox, "cache"),
      FAKE_LLMLINT_LOG: this.judgeLog,
      NX_DAEMON: "false",
    });

    this.git("init", "-q");
    this.commit("checkout under test");
  }

  /// The environment one invocation runs under; an `undefined` override unsets
  /// the name, which is how a journey says its subject is a *missing* variable.
  environment(overrides) {
    const merged = { ...this.env, ...overrides };
    for (const [name, value] of Object.entries(overrides)) {
      if (value === undefined) delete merged[name];
    }
    return merged;
  }

  /// The recipe an operator, the `gate` recipe, and CI all invoke.
  lint(base, { args = [], env = {} } = {}) {
    return spawnSync("just", ["lint-llm-diff", base, ...args], {
      cwd: this.root,
      encoding: "utf8",
      env: this.environment(env),
    });
  }

  /// The fingerprint alone, as an operator diagnosing a cache miss runs it.
  fingerprint({ env = {} } = {}) {
    return spawnSync("bash", ["scripts/llmlint-fingerprint.sh"], {
      cwd: this.root,
      encoding: "utf8",
      env: this.environment(env),
    });
  }

  judgeRuns() {
    return readFileSync(this.judgeLog, "utf8").split("\n").filter(Boolean);
  }

  /// Drive the tier's own script, for the states the recipe's default hides.
  driver(args = [], { env = {} } = {}) {
    return spawnSync("bash", ["scripts/llmlint-diff.sh", ...args], {
      cwd: this.root,
      encoding: "utf8",
      env: this.environment(env),
    });
  }

  /// Drive the judge itself, for the states only its own caller can arrange.
  judge({ env = {} } = {}) {
    return spawnSync("bash", ["scripts/llmlint-judge.sh"], {
      cwd: this.root,
      encoding: "utf8",
      env: this.environment(env),
    });
  }

  /// Run the judge with its streams reaching nobody, as a loaded host has had Nx
  /// do: what the judge recorded is then all the recipe can relay.
  judgeUnheard() {
    const judge = join(this.root, "scripts", "llmlint-judge.sh");
    renameSync(judge, join(this.root, "scripts", "llmlint-judge-unheard.sh"));
    writeExecutable(
      judge,
      "#!/usr/bin/env bash\nexec bash scripts/llmlint-judge-unheard.sh >/dev/null 2>&1\n",
    );
  }

  /// Leave llmlint only where the caller's PATH finds it, never where setup puts it.
  onInheritedLlmlintOnly() {
    const inherited = join(this.sandbox, "inherited-bin");
    installLlmlint(inherited, FAKE_LLMLINT);
    rmSync(join(this.sandbox, "home", ".local", "bin", "llmlint"));
    return { PATH: `${inherited}${delimiter}${this.env.PATH}` };
  }

  /// Drive the cached Nx target directly, as someone who skipped the recipe does.
  target({ env = {} } = {}) {
    return spawnSync("bash", ["scripts/nx.sh", "run", "onepipeline:lint-llm-diff"], {
      cwd: this.root,
      encoding: "utf8",
      env: this.environment(env),
    });
  }

  /// Run one of the tier's scripts from somewhere that is not a checkout root.
  ///
  /// Every caller gives these scripts the repository root as their working
  /// directory; this is what a hand-run from the wrong place looks like.
  fromElsewhere(script, args = []) {
    const elsewhere = join(this.sandbox, "elsewhere");
    mkdirSync(elsewhere, { recursive: true });
    return spawnSync("bash", [join(this.root, "scripts", script), ...args], {
      cwd: elsewhere,
      encoding: "utf8",
      env: this.environment({}),
    });
  }

  /// A PATH with just enough to run a shell script, plus whatever a journey names.
  ///
  /// No sha256 tool comes with it: which one is on the host is the subject of two
  /// journeys below, so neither may inherit the developer's answer.
  onBareToolchain(...tools) {
    const directory = join(this.sandbox, `bare-bin-${tools.join("-") || "none"}`);
    mkdirSync(directory, { recursive: true });
    for (const tool of ["bash", "env", "cat", "sed", "dirname", ...tools]) {
      const resolved = execFileSync("bash", ["-c", `command -v ${tool}`], { encoding: "utf8" });
      symlinkSync(resolved.trim(), join(directory, tool));
    }
    return { PATH: directory };
  }

  /// The same repository, checked out at a second path, sharing this one's rules.
  secondCheckout() {
    const root = join(this.sandbox, "second-checkout");
    copyCheckout(root);
    copyFileSync(join(this.root, "llmlint.yml"), join(root, "llmlint.yml"));
    return root;
  }

  /// Prepend an `llmlint` to the caller's PATH, as a shell that never ran setup has.
  onAmbientLlmlint(name, version) {
    const directory = installLlmlint(join(this.sandbox, name), versionOnlyLlmlint(version));
    return { PATH: `${directory}${delimiter}${this.env.PATH}` };
  }

  /// Every git command this sandbox runs, with nothing left running behind it.
  ///
  /// `git commit` ends by running `git gc --auto`, and `gc.autoDetach` — on by
  /// default — forks that collection into the background, where it outlives the
  /// call that started it and keeps writing `.git` after the test has ended. That
  /// is what failed the teardown's removal with `ENOTEMPTY` on CI, over the two
  /// files `git update-server-info` writes last. A throwaway checkout has nothing
  /// to gain from a collection, so `gc.auto=0` starts none; `gc.autoDetach=false`
  /// says that whatever does start one is waited for rather than detached.
  git(...args) {
    const settings = [
      "user.name=e2e",
      "user.email=e2e@invalid",
      "gc.auto=0",
      "gc.autoDetach=false",
    ];
    return execFileSync("git", [...settings.flatMap((setting) => ["-c", setting]), ...args], {
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

const SANDBOX_REMOVAL_ATTEMPTS = 6;
const SANDBOX_REMOVAL_RETRY_MS = 50;

/// Remove a sandbox, retrying briefly when a late writer races the removal.
///
/// Sandbox Nx runs disable their daemon, which prevents the known late writer at
/// its source. The bounded retry covers a subprocess already finishing when
/// teardown begins; exhaustion still diagnoses the remaining entries and running
/// processes, then rethrows the last removal error unchanged.
function removeSandbox(sandbox, write = (text) => process.stderr.write(text)) {
  let lastError;
  for (let attempt = 1; attempt <= SANDBOX_REMOVAL_ATTEMPTS; attempt += 1) {
    try {
      rmSync(sandbox, { recursive: true, force: true });
      return;
    } catch (error) {
      lastError = error;
      if (attempt < SANDBOX_REMOVAL_ATTEMPTS) {
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, SANDBOX_REMOVAL_RETRY_MS);
      }
    }
  }
  write(removalDiagnosis(sandbox, lastError));
  throw lastError;
}

/// Every entry still under `sandbox` — the sandbox itself first, as `.` — each with
/// its mtime, then the process table at the moment the removal failed.
///
/// An entry that cannot be read is reported in its place rather than ending the
/// report, and a symlink is listed without being followed: `node_modules` in a
/// checkout is one, into this repository's own install.
function removalDiagnosis(sandbox, error) {
  const lines = [
    `removing the sandbox ${sandbox} failed: ${error.message}`,
    "entries still under it, each with its mtime:",
  ];
  const visit = (path, entry) => {
    let stats;
    try {
      stats = lstatSync(path);
    } catch (unreadable) {
      lines.push(`  ${entry}: cannot be read: ${unreadable.message}`);
      return;
    }
    lines.push(`  ${stats.mtime.toISOString()}  ${entry}`);
    if (!stats.isDirectory()) return;
    let children;
    try {
      children = readdirSync(path).sort();
    } catch (unlistable) {
      lines.push(`  ${entry}: cannot be listed: ${unlistable.message}`);
      return;
    }
    for (const child of children) {
      visit(join(path, child), entry === "." ? child : `${entry}/${child}`);
    }
  };
  visit(sandbox, ".");

  lines.push("process table when it failed:");
  const table = spawnSync("ps", ["-ww", "-eo", "pid,ppid,lstart,args"], { encoding: "utf8" });
  if (table.error || table.status !== 0) {
    lines.push(`  ps could not report it: ${table.error?.message ?? table.stderr.trim()}`);
  } else {
    lines.push(table.stdout.trimEnd());
  }
  return `${lines.join("\n")}\n`;
}

/// A fresh checkout, cache and all, removed when the test that asked for it ends.
function workspace(t) {
  const sandbox = mkdtempSync(join(tmpdir(), "onepipeline-llmlint-cache-"));
  t.after(() => removeSandbox(sandbox));
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
    for (const result of [first, second]) {
      // A pass is one line: this tier's verdict and the pointer to the run behind
      // it, without the orchestration that produced either.
      assert.match(result.stderr, new RegExp(PASS_VERDICT), report(result));
      assert.match(result.stderr, new RegExp(POINTER), report(result));
      assert.equal(result.stdout, "", report(result));
      assert.doesNotMatch(result.stderr, /Successfully ran target/, report(result));
    }
    // The whole claim, stated as one equality: strike the clause that says where a
    // verdict came from, and the replayed run said exactly what the judged one did.
    const withoutProvenance = (result) =>
      result.stderr.replace(
        /(judged this diff against|replayed the recorded verdict for) base \w+ \(Nx cache (miss|hit)\)/,
        "<provenance>",
      );
    assert.equal(withoutProvenance(second), withoutProvenance(first));
    // "Green" is a claim about one base commit, so the provenance line names it:
    // a gate run and a CI run resolving different bases answer different questions.
    assert.match(first.stderr, new RegExp(`${CACHE_MISS} ${base}`), report(first));
    assert.match(second.stderr, new RegExp(`${CACHE_HIT} ${base}`), report(second));
  });

  it("reports a replay as a replay when Nx colours its output", (t) => {
    // Colour is not cosmetic to this tier: Nx wraps the cache annotation, and the
    // words inside it, in escape sequences whenever it thinks the terminal takes
    // colour — which includes every run nested inside another Nx task, such as
    // this suite. Reading the provenance off the coloured shape without allowing
    // for that reported every replay as a fresh judgement.
    const ws = workspace(t);
    const base = ws.head();
    const coloured = { env: { FORCE_COLOR: "true" } };

    const first = ws.lint(base, coloured);
    const second = ws.lint(base, coloured);

    assert.equal(first.status, 0, report(first));
    assert.equal(second.status, 0, report(second));
    assert.equal(ws.judgeRuns().length, 1, report(second));
    assert.match(first.stderr, new RegExp(CACHE_MISS), report(first));
    assert.match(second.stderr, new RegExp(CACHE_HIT), report(second));
  });

  it("judges with an llmlint the caller's PATH provides and setup's directory does not", (t) => {
    // The install directory goes first, not instead: a contributor who installed
    // llmlint somewhere else is still judged, and both ends of the key still agree
    // because both take that same order.
    const ws = workspace(t);
    const base = ws.head();
    const inherited = { env: ws.onInheritedLlmlintOnly() };

    const judged = ws.lint(base, inherited);
    const replayed = ws.lint(base, inherited);
    const digest = ws.fingerprint(inherited);

    assert.equal(judged.status, 0, report(judged));
    assert.equal(replayed.status, 0, report(replayed));
    assert.equal(ws.judgeRuns().length, 1, report(replayed));
    assert.match(judged.stderr, new RegExp(PASS_VERDICT), report(judged));
    assert.match(replayed.stderr, new RegExp(CACHE_HIT), report(replayed));
    assert.match(digest.stdout.trim(), /^[0-9a-f]{64}$/, report(digest));
  });

  it("reports a pass whose judge offered no pointer to the run behind it", (t) => {
    const ws = workspace(t);
    const base = ws.head();
    const noPointer = { env: { FAKE_LLMLINT_NO_POINTER: "1" } };

    const judged = ws.lint(base, noPointer);
    const replayed = ws.lint(base, noPointer);

    for (const result of [judged, replayed]) {
      assert.equal(result.status, 0, report(result));
      assert.match(result.stderr, new RegExp(PASS_VERDICT), report(result));
      assert.doesNotMatch(result.stderr, /full report:/, report(result));
    }
    assert.equal(ws.judgeRuns().length, 1, report(replayed));
    assert.match(replayed.stderr, new RegExp(CACHE_HIT), report(replayed));
  });

  it("reports the verdict the judge recorded when none of its output reaches the caller", (t) => {
    // What a task prints reaches the recipe only through Nx, which reads, stores,
    // and replays it on its own schedule — so a verdict read from there can still
    // be missing when the task has exited. A judge whose streams reached nobody
    // still certified this diff, and the replay of that run has to say so too.
    const ws = workspace(t);
    const base = ws.head();
    ws.judgeUnheard();

    const judged = ws.lint(base);
    const replayed = ws.lint(base);

    for (const result of [judged, replayed]) {
      assert.equal(result.status, 0, report(result));
      assert.match(result.stderr, new RegExp(PASS_VERDICT), report(result));
      assert.match(result.stderr, new RegExp(POINTER), report(result));
    }
    assert.equal(ws.judgeRuns().length, 1, report(replayed));
    assert.match(judged.stderr, new RegExp(`${CACHE_MISS} ${base}`), report(judged));
    assert.match(replayed.stderr, new RegExp(`${CACHE_HIT} ${base}`), report(replayed));
  });

  it("judges again when the workspace changes", (t) => {
    const ws = workspace(t);
    const base = ws.head();
    ws.lint(base);

    appendFileSync(join(ws.root, "README.md"), "\nJudged again.\n", "utf8");
    const second = ws.lint(base);

    assert.equal(second.status, 0, report(second));
    assert.equal(ws.judgeRuns().length, 2, "the judge was asked a different number of times");
    assert.match(second.stderr, new RegExp(CACHE_MISS), report(second));
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

    assert.equal(ws.judgeRuns().length, 2, "the judge was asked a different number of times");
    assert.match(moved.stderr, new RegExp(CACHE_MISS), report(moved));
    assert.match(repeated.stderr, new RegExp(CACHE_HIT), report(repeated));
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
    assert.equal(ws.judgeRuns().length, 2, "the judge was asked a different number of times");
    assert.match(second.stderr, new RegExp(CACHE_MISS), report(second));
  });

  it("judges again when the installed llmlint version changes", (t) => {
    const ws = workspace(t);
    const base = ws.head();
    ws.lint(base, { env: { FAKE_LLMLINT_VERSION: "0.4.0" } });

    const second = ws.lint(base, { env: { FAKE_LLMLINT_VERSION: "0.5.0" } });

    assert.equal(second.status, 0, report(second));
    assert.equal(ws.judgeRuns().length, 2, "the judge was asked a different number of times");
    assert.match(second.stderr, new RegExp(CACHE_MISS), report(second));
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
    assert.equal(ws.judgeRuns().length, 1, "the judge was asked a different number of times");
    assert.match(second.stderr, new RegExp(CACHE_HIT), report(second));
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
    assert.equal(ws.judgeRuns().length, 1, "the judge was asked a different number of times");
    assert.match(second.stderr, new RegExp(CACHE_HIT), report(second));
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
    assert.equal(ws.judgeRuns().length, 2, "the judge was asked a different number of times");
    assert.match(second.stderr, new RegExp(CACHE_MISS), report(second));
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
    assert.equal(ws.judgeRuns().length, 1, "the judge was asked a different number of times");
  });

  it("fails the tier and judges again when the judge reports findings", (t) => {
    const ws = workspace(t);
    const base = ws.head();

    const first = ws.lint(base, { env: { FAKE_LLMLINT_EXIT: "1" } });
    const second = ws.lint(base, { env: { FAKE_LLMLINT_EXIT: "1" } });

    assert.equal(ws.judgeRuns().length, 2, "the judge was asked a different number of times");
    for (const result of [first, second]) {
      assert.equal(result.status, 1, report(result));
      assert.match(report(result), new RegExp(FINDING));
      assert.match(report(result), new RegExp(FAIL_VERDICT));
      assert.match(report(result), /clear the findings above/, report(result));
      assert.match(result.stderr, new RegExp(CACHE_MISS), report(result));
    }
  });

  it("fails the tier and judges again when the toolchain never reaches a verdict", (t) => {
    const ws = workspace(t);
    const base = ws.head();

    const first = ws.lint(base, { env: { FAKE_LLMLINT_EXIT: "2" } });
    const second = ws.lint(base, { env: { FAKE_LLMLINT_EXIT: "2" } });

    assert.equal(ws.judgeRuns().length, 2, "the judge was asked a different number of times");
    for (const result of [first, second]) {
      // Nx collapses a failed task, so what separates this from findings is the
      // report above it — which is why a red is never stored and always re-judged.
      assert.notEqual(result.status, 0, report(result));
      // A judge that never ruled is told apart from one that ruled against the
      // diff by the only thing that differs for the operator: what to do next.
      assert.match(report(result), /without judging this diff/, report(result));
      assert.doesNotMatch(report(result), /clear the findings above/, report(result));
      assert.match(result.stderr, new RegExp(CACHE_MISS), report(result));
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
    assert.equal(ws.judgeRuns().length, 2, "the judge was asked a different number of times");
    assert.match(green.stderr, new RegExp(CACHE_MISS), report(green));
    assert.match(settled.stderr, new RegExp(CACHE_HIT), report(settled));
  });

  it("re-judges per invocation with --skip-nx-cache, and ignores an ambient global skip", (t) => {
    // The supported re-judge lever is per-invocation on purpose: an exported
    // global skip would re-roll a non-deterministic judge from every unrelated
    // command, and silently break the checks whose contract is cache replay.
    const ws = workspace(t);
    const base = ws.head();
    ws.lint(base);

    // The forced run is made to say something the stored one does not, so the run
    // that follows it says which entry answered.
    const forced = ws.lint(base, {
      args: ["--skip-nx-cache"],
      env: { NX_SKIP_NX_CACHE: "true", FAKE_LLMLINT_NOTE: "forced" },
    });
    const ambient = ws.lint(base, { env: { NX_DISABLE_NX_CACHE: "true" } });

    assert.equal(forced.status, 0, report(forced));
    assert.equal(ambient.status, 0, report(ambient));
    assert.equal(ws.judgeRuns().length, 2, "the ambient skip re-rolled the judge");
    assert.match(forced.stderr, new RegExp(CACHE_MISS), report(forced));
    assert.match(forced.stderr, /forced/);
    assert.match(ambient.stderr, new RegExp(CACHE_HIT), report(ambient));
    // Under this Nx the lever neither reads nor writes: the run after it replays
    // the entry the forced roll left in place, not the forced roll's own report.
    assert.doesNotMatch(ambient.stderr, /forced/);
    for (const result of [forced, ambient]) {
      assert.match(result.stderr, /ignoring the ambient global Nx cache skip/);
      assert.match(
        result.stderr,
        new RegExp(`just lint-llm-diff ${base} --skip-nx-cache`),
        report(result),
      );
    }
  });

  it("refuses a base it cannot resolve before the judge is paid", (t) => {
    const ws = workspace(t);

    const result = ws.lint("no-such-ref");

    assert.notEqual(result.status, 0, report(result));
    assert.match(result.stderr, /'no-such-ref' does not resolve to a commit/);
    assert.equal(ws.judgeRuns().length, 0, "the judge was asked a different number of times");
  });
});

describe("the judged tier's refusals", () => {
  // The recipe resolves the base itself, so these states are reachable only by
  // driving the cached target directly — which is the misuse the guard names.
  for (const [what, base, expected] of [
    ["nothing at all", undefined, "must be a resolved commit id"],
    ["a ref rather than a commit", "origin/main", "must be a resolved commit id"],
    ["a commit this checkout does not have", "0".repeat(40), "missing from this checkout"],
  ]) {
    it(`refuses ${what} as a base to judge against`, (t) => {
      const ws = workspace(t);

      const result = ws.target({ env: { LLMLINT_DIFF_BASE_SHA: base } });

      assert.notEqual(result.status, 0, report(result));
      assert.match(report(result), new RegExp(expected));
      assert.equal(ws.judgeRuns().length, 0, report(result));
    });
  }

  for (const [what, broken, expected] of [
    ["report its version", { FAKE_LLMLINT_VERSION_EXIT: "4" }, "'llmlint --version' failed"],
    ["resolve its config", { FAKE_LLMLINT_CONFIG_EXIT: "5" }, "'llmlint config' failed"],
  ]) {
    it(`names a judge toolchain that cannot ${what}`, (t) => {
      const ws = workspace(t);

      const result = ws.fingerprint({ env: broken });

      // Distinct from a checkout this cannot read: one says repair the toolchain,
      // the other says repair the checkout, and the exit code says which.
      assert.equal(result.status, 2, report(result));
      assert.match(result.stderr, new RegExp(expected));
      assert.equal(result.stdout.trim(), "", "a failed fingerprint must contribute nothing");
    });
  }

  for (const entrypoint of ["fingerprint", "target"]) {
    it(`says what to restore when the ${entrypoint} loses the shared runtime environment`, (t) => {
      const ws = workspace(t);
      const base = ws.head();
      rmSync(join(ws.root, "scripts", "llmlint-runtime-env.sh"));

      const result =
        entrypoint === "fingerprint"
          ? ws.fingerprint()
          : ws.target({ env: { LLMLINT_DIFF_BASE_SHA: base } });

      // 3 from the fingerprint, which the caller runs directly; through Nx the
      // target's own code is collapsed, and the driver is what restores it.
      if (entrypoint === "fingerprint") assert.equal(result.status, 3, report(result));
      else assert.notEqual(result.status, 0, report(result));
      assert.match(report(result), /could not load the shared runtime environment/);
      assert.match(report(result), /restore scripts\/llmlint-runtime-env\.sh and retry/);
      assert.equal(ws.judgeRuns().length, 0, report(result));
    });
  }

  it("says what to pass when it is handed no base at all", (t) => {
    // Unreachable through the recipe, which defaults the base — so this is what a
    // script, a hook, or a workflow calling the driver directly is told.
    const ws = workspace(t);

    const result = ws.driver();

    assert.equal(result.status, 2, report(result));
    assert.match(result.stderr, /pass the base to judge against/);
    assert.equal(ws.judgeRuns().length, 0, report(result));
  });

  it("names the missing tool when the host cannot hash the judge configuration", (t) => {
    // The fingerprint resolves its own toolchain, so it still finds llmlint with
    // nothing else on PATH — and then has nothing to hash with.
    const ws = workspace(t);

    const result = ws.fingerprint({ env: ws.onBareToolchain() });

    assert.equal(result.status, 2, report(result));
    assert.match(result.stderr, /no sha256 tool found/);
  });

  it("says what to free when it cannot open storage for the report", (t) => {
    const ws = workspace(t);

    const result = ws.lint(ws.head(), { env: { TMPDIR: join(ws.sandbox, "no-such-tmp") } });

    assert.equal(result.status, 3, report(result));
    assert.match(result.stderr, /could not open temporary storage for the judge report/);
    assert.equal(ws.judgeRuns().length, 0, report(result));
  });

  it("refuses to certify a judge that exited cleanly without a verdict", (t) => {
    // llmlint's status is not the whole answer: a clean exit that reached no
    // verdict would otherwise be stored as a pass and replayed for this tree.
    const ws = workspace(t);
    const base = ws.head();

    const first = ws.lint(base, { env: { FAKE_LLMLINT_NO_VERDICT: "1" } });
    const second = ws.lint(base, { env: { FAKE_LLMLINT_NO_VERDICT: "1" } });
    // The refusal reaches the recipe through Nx like the verdict does, and on a
    // loaded host has arrived without it. The judge records it beside the verdict,
    // and that record is what the recipe relays when Nx forwarded nothing — and
    // does not repeat when Nx forwarded everything.
    ws.judgeUnheard();
    const unheard = ws.lint(base, { env: { FAKE_LLMLINT_NO_VERDICT: "1" } });

    for (const result of [first, second, unheard]) {
      // The tier fails, as it does for any diff it could not certify; what the
      // report says is which of the two happened, exactly once.
      assert.notEqual(result.status, 0, report(result));
      assert.equal(report(result).split("without reporting a verdict").length, 2, report(result));
    }
    assert.match(
      readFileSync(join(ws.root, ".lint-llm-diff", "refusal"), "utf8"),
      /^lint-llm-diff: llmlint exited cleanly without reporting a verdict for this diff; .*\n$/,
    );
    // Never stored, so the next run asks again rather than replaying the silence.
    assert.equal(ws.judgeRuns().length, 3, report(unheard));
    assert.match(unheard.stderr, new RegExp(CACHE_MISS), report(unheard));
  });

  it("refuses to report a pass the cached target never put a verdict in", (t) => {
    // The target is a seam: whatever runs behind it has to leave the verdict the
    // driver reports, and a driver that invented one would certify silence.
    const ws = workspace(t);
    writeExecutable(join(ws.root, "scripts", "llmlint-judge.sh"), "#!/usr/bin/env bash\nexit 0\n");

    const result = ws.lint(ws.head());

    assert.equal(result.status, 2, report(result));
    assert.match(result.stderr, /reported no verdict for base/, report(result));
  });

  for (const [what, record] of [
    ["is not a verdict", "lint-llm-diff: certified\n"],
    [
      "carries more than one verdict",
      `lint-llm-diff: ${PASS_VERDICT}\nlint-llm-diff: ${FAIL_VERDICT}\n`,
    ],
  ]) {
    it(`refuses to report a pass from a verdict record that ${what}`, (t) => {
      // The record is restored from the cache as well as written by the judge, so
      // it is read as input: only one whole verdict line certifies anything.
      const ws = workspace(t);
      writeExecutable(
        join(ws.root, "scripts", "llmlint-judge.sh"),
        `#!/usr/bin/env bash\nmkdir -p .lint-llm-diff\nprintf '%s' '${record}' >.lint-llm-diff/verdict\n`,
      );

      const result = ws.lint(ws.head());

      assert.equal(result.status, 2, report(result));
      assert.match(result.stderr, /reported no verdict for base/, report(result));
      assert.doesNotMatch(result.stderr, new RegExp(CACHE_MISS), report(result));
    });
  }

  it("says what to free when the judge cannot record its verdict", (t) => {
    const ws = workspace(t);
    // A file where the record's directory belongs: the judge reaches a verdict and
    // has nowhere to put it, which must not pass as a clean run.
    writeFileSync(join(ws.root, ".lint-llm-diff"), "", "utf8");

    const result = ws.judge({ env: { LLMLINT_DIFF_BASE_SHA: ws.head() } });

    assert.equal(result.status, 3, report(result));
    assert.match(result.stderr, /could not record the verdict/, report(result));
    assert.doesNotMatch(result.stdout, new RegExp(PASS_VERDICT), report(result));
  });

  it("says what to free when the judge cannot record its refusal", (t) => {
    const ws = workspace(t);
    // The same file where the record's directory belongs, met by a judge that
    // reached no verdict: the checkout failing the run is exit 3, as it is for the
    // verdict's own write, and never the no-verdict 2 — while the refusal itself
    // is still said, since it is what the operator retries for.
    writeFileSync(join(ws.root, ".lint-llm-diff"), "", "utf8");

    const result = ws.judge({
      env: { LLMLINT_DIFF_BASE_SHA: ws.head(), FAKE_LLMLINT_NO_VERDICT: "1" },
    });

    assert.equal(result.status, 3, report(result));
    assert.match(result.stderr, /could not record that refusal/, report(result));
    assert.match(result.stderr, /without reporting a verdict for this diff/, report(result));
  });

  it("says what to repair when an earlier verdict record cannot be cleared", (t) => {
    // A record this run cannot remove could answer for it, so the judge is not
    // paid for a verdict the tier could not tell apart from the stale one.
    const ws = workspace(t);
    mkdirSync(join(ws.root, ".lint-llm-diff", "verdict", "stuck"), { recursive: true });

    const result = ws.lint(ws.head());

    assert.equal(result.status, 3, report(result));
    assert.match(result.stderr, /could not clear the previous verdict record/, report(result));
    assert.equal(ws.judgeRuns().length, 0, report(result));
  });

  it("says what to free when the judge cannot open storage for its report", (t) => {
    const ws = workspace(t);

    const result = ws.judge({
      env: { LLMLINT_DIFF_BASE_SHA: ws.head(), TMPDIR: join(ws.sandbox, "no-such-tmp") },
    });

    assert.equal(result.status, 3, report(result));
    assert.match(result.stderr, /could not open temporary storage for the judge's report/);
    assert.equal(ws.judgeRuns().length, 0, report(result));
  });

  it("names the run to look at when the judge fails without saying anything", (t) => {
    // Nothing to clear, so nothing is said about clearing: what an operator needs
    // is the command that shows what the judge actually did.
    const ws = workspace(t);

    const result = ws.lint(ws.head(), {
      env: { FAKE_LLMLINT_EXIT: "1", FAKE_LLMLINT_SILENT: "1" },
    });

    assert.notEqual(result.status, 0, report(result));
    assert.match(report(result), /exited 1 without reporting anything/, report(result));
    assert.doesNotMatch(report(result), /clear the findings above/, report(result));
  });

  it("refuses an option that is not one of this tier's own", (t) => {
    const ws = workspace(t);

    const result = ws.lint(ws.head(), { args: ["--parallel=8"] });

    assert.equal(result.status, 2, report(result));
    assert.match(result.stderr, /is not one of this tier's options/);
    assert.equal(ws.judgeRuns().length, 0, report(result));
  });

  for (const [what, broken, expected] of [
    ["version", { FAKE_LLMLINT_VERSION_EMPTY: "1" }, /'llmlint --version' answered nothing/],
    ["configuration", { FAKE_LLMLINT_CONFIG_EMPTY: "1" }, /'llmlint config' answered nothing/],
  ]) {
    it(`refuses to hash a judge that answers with no ${what}`, (t) => {
      // An empty answer would hash to a fingerprint that says nothing about the
      // judge configuration, which is the silent key-shrink this tier guards.
      const ws = workspace(t);

      const result = ws.fingerprint({ env: broken });

      assert.equal(result.status, 2, report(result));
      assert.match(result.stderr, expected);
      assert.equal(result.stdout.trim(), "", "a refused fingerprint must contribute nothing");
    });
  }

  it("hashes the same on a host whose sha256 tool is shasum", (t) => {
    // `sha256sum` is coreutils and `shasum` is perl; a contributor on macOS has the
    // second and not the first, and `just gate` reaches this tier on either.
    const ws = workspace(t);

    const coreutils = ws.fingerprint({ env: ws.onBareToolchain("sha256sum") });
    const perl = ws.fingerprint({ env: ws.onBareToolchain("shasum") });

    assert.equal(coreutils.status, 0, report(coreutils));
    assert.equal(perl.status, 0, report(perl));
    assert.notEqual(coreutils.stdout.trim(), "");
    assert.equal(coreutils.stdout.trim(), perl.stdout.trim(), "one hash, two tools");
  });

  it("hashes the same judge configuration from two checkouts of this repository", (t) => {
    // The merged configuration names the checkout it was resolved in, so a digest
    // that kept those paths would miss for every worktree and share nothing.
    const ws = workspace(t);

    const here = ws.fingerprint();
    const elsewhere = spawnSync("bash", ["scripts/llmlint-fingerprint.sh"], {
      cwd: ws.secondCheckout(),
      encoding: "utf8",
      env: ws.environment({}),
    });

    assert.equal(here.status, 0, report(here));
    assert.equal(elsewhere.status, 0, report(elsewhere));
    assert.notEqual(here.stdout.trim(), "");
    assert.equal(here.stdout.trim(), elsewhere.stdout.trim(), "the digest is path-shaped");
  });

  for (const [what, body] of [
    ["fails", "exit 7"],
    ["answers with something that is not a digest", "echo 'no digest here'"],
  ]) {
    it(`refuses a fingerprint when the host's sha256 tool ${what}`, (t) => {
      // Whatever the hash tool returns is cache-key material, so a run that keyed
      // a verdict on 'no digest here' would replay it for every other tree too.
      const ws = workspace(t);
      const bare = ws.onBareToolchain();
      writeExecutable(join(bare.PATH, "sha256sum"), `#!/usr/bin/env bash\n${body}\n`);

      const result = ws.fingerprint({ env: bare });

      assert.equal(result.status, 2, report(result));
      assert.match(result.stderr, /rather than a digest/);
      assert.equal(result.stdout.trim(), "", "a refused fingerprint must contribute nothing");
    });
  }

  for (const [script, wants] of [
    ["llmlint-diff.sh", "the Nx workspace it hands this tier to"],
    ["llmlint-judge.sh", "the judge configuration it lints under"],
    ["llmlint-fingerprint.sh", "the judge configuration it hashes"],
  ]) {
    it(`refuses to run ${script} from anywhere but the repository root`, (t) => {
      // Answering from the wrong tree is worse than not answering: a fingerprint
      // taken elsewhere would key this tree's verdict to another tree's rules.
      const ws = workspace(t);

      const result = ws.fromElsewhere(script, [ws.head()]);

      assert.equal(result.status, 3, report(result));
      assert.match(result.stderr, /run this from the repository root/);
      assert.match(result.stderr, new RegExp(wants));
      assert.equal(ws.judgeRuns().length, 0, report(result));
    });
  }

  it("says what HOME has to be when it is not an absolute path", (t) => {
    // Both ends of the key resolve their toolchain under HOME, so a relative one
    // would resolve it from wherever each end happened to be run.
    const ws = workspace(t);

    const result = ws.fingerprint({ env: { HOME: "relative/home" } });

    assert.notEqual(result.status, 0, report(result));
    assert.match(result.stderr, /HOME is 'relative\/home', which is not an absolute path/);
    assert.equal(result.stdout.trim(), "", "a refused fingerprint must contribute nothing");
  });

  it("says whose HOME to set when the toolchain cannot be located at all", (t) => {
    const ws = workspace(t);

    const result = ws.fingerprint({ env: { HOME: undefined } });

    assert.notEqual(result.status, 0, report(result));

    assert.match(result.stderr, /HOME is not set/);
    assert.match(result.stderr, /just setup-llmlint/);
  });

  it("refuses a base whose shape is not a git ref, before reaching git", (t) => {
    const ws = workspace(t);

    const result = ws.lint("origin/main; touch pwned");

    assert.equal(result.status, 2, report(result));
    assert.match(result.stderr, /is not a usable git ref/);
    assert.equal(ws.judgeRuns().length, 0, report(result));
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
        text.includes("$HOME/.local/bin"),
        `${name} no longer names $HOME/.local/bin as the llmlint install directory`,
      );
    }
  });
});

describe("the journeys' sandbox teardown", () => {
  const waitSync = (milliseconds) =>
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds);

  function populatedSandbox(t) {
    const sandbox = mkdtempSync(join(tmpdir(), "onepipeline-llmlint-teardown-"));
    t.after(() => removeSandbox(sandbox));
    mkdirSync(join(sandbox, "checkout"));
    writeFileSync(join(sandbox, "checkout", "llmlint.yml"), "version: 1\n", "utf8");
    writeFileSync(join(sandbox, "judge-runs.log"), "", "utf8");
    return sandbox;
  }

  /// An error's own fields and message, with the sandbox it names made generic, so
  /// the same failure on two sandboxes compares equal.
  function shape(error, sandbox) {
    const generic = (value) =>
      typeof value === "string" ? value.replaceAll(sandbox, "<sandbox>") : value;
    return {
      constructor: error.constructor,
      message: generic(error.message),
      fields: Object.fromEntries(
        Object.entries(error).map(([key, value]) => [key, generic(value)]),
      ),
    };
  }

  it("removes a sandbox and prints nothing when the removal succeeds", (t) => {
    const sandbox = populatedSandbox(t);
    const written = [];

    removeSandbox(sandbox, (text) => written.push(text));

    assert.equal(existsSync(sandbox), false, `${sandbox} is still there`);
    assert.deepEqual(written, []);
  });

  it("retries until a late writer releases the populated sandbox", async (t) => {
    const sandbox = populatedSandbox(t);
    const ready = join(sandbox, "writer-ready");
    const writer = spawn(
      process.execPath,
      [
        "-e",
        `const fs = require("node:fs");
const [sandbox, ready] = process.argv.slice(1);
if (!sandbox || !ready || !sandbox.startsWith("/") || !ready.startsWith(sandbox + "/")) {
  throw new Error("expected an absolute sandbox and a ready path inside it");
}
const held = sandbox + "/checkout/held";
fs.mkdirSync(held);
fs.writeFileSync(held + "/late", "still writing\\n");
fs.chmodSync(held, 0o000);
fs.writeFileSync(ready, "ready\\n");
const deadline = Date.now() + 140;
let sequence = 0;
while (Date.now() < deadline) {
  fs.writeFileSync(sandbox + "/late-" + sequence, "still writing\\n");
  sequence += 1;
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10);
}
fs.chmodSync(held, 0o700);`,
        sandbox,
        ready,
      ],
      { stdio: "inherit" },
    );
    const writerExited = once(writer, "exit");
    while (!existsSync(ready)) {
      assert.equal(writer.exitCode, null, "the late writer exited before becoming ready");
      waitSync(10);
    }

    const written = [];
    removeSandbox(sandbox, (text) => written.push(text));
    const [status] = await writerExited;

    assert.equal(status, 0, "the late writer failed");
    assert.equal(existsSync(sandbox), false, `${sandbox} is still there`);
    assert.deepEqual(written, []);
  });

  it("prints what a failed removal left and what was running, then rethrows its error", (t) => {
    // No rmdir accepts a path whose last component is `.`, and that holds for root
    // too, where a permission failure would not throw: this is a real removal failing.
    // How much it deleted before failing is the runtime's: Node 22 fails before it
    // deletes anything; Node 26 deletes the contents first when it can, so as root
    // only the sandbox itself is left to list.
    const sandbox = populatedSandbox(t);
    const twin = populatedSandbox(t);
    let direct;
    try {
      rmSync(`${twin}/.`, { recursive: true, force: true });
    } catch (error) {
      direct = error;
    }
    assert.ok(direct, "removing a path ending in `.` succeeded, so nothing here fails");

    const written = [];
    let rethrown;
    const started = Date.now();
    assert.throws(
      () => removeSandbox(`${sandbox}/.`, (text) => written.push(text)),
      (error) => {
        rethrown = error;
        return true;
      },
    );
    assert.ok(
      Date.now() - started >= (SANDBOX_REMOVAL_ATTEMPTS - 1) * SANDBOX_REMOVAL_RETRY_MS,
      "the permanently populated sandbox was not retried to the bound",
    );
    const diagnosis = written.join("");

    // The removal's own error, not a wrapping of it: the same failure `rmSync` raises
    // when nothing diagnoses it.
    assert.deepEqual(shape(rethrown, sandbox), shape(direct, twin));
    assert.ok(
      diagnosis.startsWith(`removing the sandbox ${sandbox}/. failed: ${rethrown.message}\n`),
      diagnosis,
    );
    const remaining = [".", ...readdirSync(sandbox, { recursive: true }).sort()];
    const listed = diagnosis.split("\n").filter((line) => /^ {2}\d{4}-\d\d-\d\dT/.test(line));
    assert.deepEqual(
      listed,
      remaining.map(
        (entry) => `  ${lstatSync(join(sandbox, entry)).mtime.toISOString()}  ${entry}`,
      ),
      diagnosis,
    );
    const table = diagnosis.split("process table when it failed:\n")[1] ?? "";
    assert.match(table, /^\s*PID\s+PPID\s/, diagnosis);
    assert.match(table, new RegExp(`^\\s*${process.pid}\\s+${process.ppid}\\s`, "m"), diagnosis);
  });

  /// Every path under a checkout's `.git`, with its mtime and size. Two readings
  /// that compare equal are a `.git` nothing wrote to in between; an entry that
  /// vanishes mid-reading is reported in its place, because that is a write too.
  function gitState(root) {
    const git = join(root, ".git");
    return readdirSync(git, { recursive: true })
      .sort()
      .map((entry) => {
        try {
          const stats = lstatSync(join(git, entry));
          return `${entry} ${stats.mtimeMs} ${stats.size}`;
        } catch (unreadable) {
          return `${entry} disappeared while being read: ${unreadable.message}`;
        }
      });
  }

  it("leaves no collection running in a sandbox whose last git command has returned", async (t) => {
    const ws = workspace(t);
    // CI reached this by accident: a copied checkout whose loose objects happened
    // to pass git's auto-gc estimate, which is taken from one of the 256 object
    // fan-out directories and so answers differently on every host. The journey
    // arms the threshold rather than hoping for it — the repository asks for a
    // collection on its next commit, and the helper is what must not leave one
    // running.
    ws.git("config", "gc.auto", "1");
    writeFileSync(join(ws.root, "collected.txt"), "one more object to collect\n", "utf8");

    ws.commit("a commit whose auto-gc would collect");

    // `git gc` holds this for as long as it runs, so a detached one is still here.
    const collecting = join(ws.root, ".git", "gc.pid");
    assert.equal(existsSync(collecting), false, `${collecting} says a collection is running`);
    const settled = gitState(ws.root);
    await sleep(2000);
    assert.deepEqual(
      gitState(ws.root),
      settled,
      "the sandbox's .git was written after its last git command returned",
    );
  });
});
