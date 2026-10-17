// The crate-split replay, run as its command over a real git repository with two
// crates and two suites, under two proposed mappings that differ only in whether one
// crate depends on the other.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { compileMapping, globToRegExp } from "../../scripts/crate-split-replay.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const golden = JSON.parse(
  readFileSync(join(root, "tests", "golden", "test-profile-record-v1.json"), "utf8"),
);
const categories = Object.fromEntries(
  Object.entries(golden.categories).map(([name, tally]) => [
    name,
    Object.fromEntries(Object.keys(tally).map((key) => [key, 0])),
  ]),
);
const script = join(root, "scripts", "crate-split-replay.mjs");

let scratch;
let repo;

function git(args, date) {
  return execFileSync("git", ["-C", repo, ...args], {
    encoding: "utf8",
    env: {
      ...process.env,
      GIT_AUTHOR_NAME: "split",
      GIT_AUTHOR_EMAIL: "split@example.invalid",
      GIT_COMMITTER_NAME: "split",
      GIT_COMMITTER_EMAIL: "split@example.invalid",
      GIT_AUTHOR_DATE: date,
      GIT_COMMITTER_DATE: date,
      GIT_CONFIG_GLOBAL: join(scratch, "gitconfig"),
    },
  });
}

function commit(subject, date, files) {
  for (const [path, body] of Object.entries(files)) {
    mkdirSync(dirname(join(repo, path)), { recursive: true });
    writeFileSync(join(repo, path), body);
  }
  git(["add", "-A"], date);
  git(["commit", "-qm", subject], date);
}

before(() => {
  scratch = mkdtempSync(join(tmpdir(), "crate-split-replay-"));
  writeFileSync(join(scratch, "gitconfig"), "");
  repo = join(scratch, "repo");
  mkdirSync(repo);
  git(["init", "-q", "--initial-branch=main"], "2026-09-01T00:00:00Z");
  // Before the window, so the first change in it has a parent to be diffed against.
  commit("chore: seed", "2026-09-01T00:00:00Z", {
    "Cargo.lock": "seed\n",
    "README.md": "seed\n",
    "crates/alpha/src/lib.rs": "seed\n",
    "crates/beta/src/lib.rs": "seed\n",
    "crates/beta/src/moved.rs": "a module that later moves crates\n",
    "tests/one.rs": "seed\n",
    "tests/two.rs": "seed\n",
    // Today's shape, for --today: one crate, every test in one suite.
    "nx.json": JSON.stringify({
      namedInputs: {
        sharedGlobals: ["{workspaceRoot}/nx.json"],
        rustBuild: ["{workspaceRoot}/crates/**/*", "{workspaceRoot}/Cargo.lock", "sharedGlobals"],
        e2eSource: ["rustBuild", "{workspaceRoot}/tests/**/*"],
      },
    }),
    "tests/e2e/main.rs": "mod harness;\nmod one;\nmod two;\n",
  });
  commit("feat: alpha", "2026-09-16T00:00:00Z", { "crates/alpha/src/lib.rs": "a\n" });
  commit("feat: beta", "2026-09-17T00:00:00Z", { "crates/beta/src/lib.rs": "b\n" });
  commit("test: suite one", "2026-09-18T00:00:00Z", { "tests/one.rs": "one\n" });
  commit("fix: the lock", "2026-09-19T00:00:00Z", { "Cargo.lock": "lock\n" });
  commit("docs: the readme", "2026-09-20T00:00:00Z", { "README.md": "readme\n" });
  commit("chore: release v0.2.0", "2026-09-21T00:00:00Z", { "CHANGELOG.md": "0.2.0\n" });
  commit("feat: a crate the mapping forgot", "2026-09-22T00:00:00Z", {
    "crates/gamma/src/lib.rs": "g\n",
  });
  // After the window.
  commit("feat: alpha again", "2026-10-20T00:00:00Z", { "crates/alpha/src/lib.rs": "a2\n" });
  // A file moved from one crate to the other, which git would report as a rename.
  rmSync(join(repo, "crates/beta/src/moved.rs"));
  commit("refactor: move a module between crates", "2026-10-25T00:00:00Z", {
    "crates/alpha/src/moved.rs": "a module that later moves crates\n",
  });
  // A name git quotes in its line-separated output.
  commit("feat: a file git quotes the name of", "2026-10-28T00:00:00Z", {
    "crates/beta/src/naïve.rs": "b\n",
  });

  const records = join(scratch, "records", "e2e");
  mkdirSync(records, { recursive: true });
  for (const [test, wall] of [
    ["one::first", 10],
    ["one::second", 5],
    ["two::only", 20],
    // A module no suite of either mapping names.
    ["three::stray", 1],
  ]) {
    writeFileSync(
      join(records, `${test.replace("::", ".")}.json`),
      // The golden record's shape, which is what the harness writes, at this
      // test's name and wall clock with nothing attributed.
      JSON.stringify({
        ...golden,
        test,
        run_id: "r",
        wall_s: wall,
        unattributed_s: wall,
        categories,
      }),
    );
  }
});

after(() => rmSync(scratch, { recursive: true, force: true }));

// The proposed split, held to the e2e binary it proposes to divide: every module
// `tests/e2e/main.rs` declares is in exactly one suite (or is a support file one
// suite lists), and every repository file a module's tests read by a literal path
// is an input of that module's suite. A module added, renamed or given a new
// runtime file without the proposal following fails here.
test("the proposed split names every e2e module once and every file its tests read", () => {
  const proposal = JSON.parse(
    readFileSync(join(root, "scripts", "crate-split", "proposed.json"), "utf8"),
  );
  const compiled = compileMapping(proposal);
  const main = readFileSync(join(root, "tests", "e2e", "main.rs"), "utf8");
  const declared = [...main.matchAll(/^mod (\w+);/gm)].map((m) => m[1]);
  const owner = new Map();
  for (const [name, suite] of Object.entries(proposal.suites)) {
    for (const module of suite.modules) {
      assert.ok(!owner.has(module), `${module} is in ${owner.get(module)} and ${name}`);
      owner.set(module, name);
    }
  }
  for (const module of declared) {
    const file = `tests/e2e/${module}.rs`;
    const source = readFileSync(join(root, file), "utf8");
    // A module with tests of its own is one a suite runs; one without is support a
    // suite compiles, so a suite listing its file is enough.
    if (/#\[test\]/.test(source))
      assert.ok(owner.has(module), `${module} is in no suite's modules`);
    const suite =
      owner.get(module) ??
      Object.keys(proposal.suites).find((name) => compiled.suites[name].own(file));
    assert.ok(suite, `${module} is in no suite of the proposal`);
    assert.ok(compiled.suites[suite].own(file), `${suite} does not list ${file}`);
    const reads = [
      ...[...source.matchAll(/repo_file\(\s*"([^"]+)"/g)].map((m) => m[1]),
      ...[...source.matchAll(/CARGO_MANIFEST_DIR"\)\)\s*\.join\("([^"]+)"/g)].map((m) => m[1]),
      ...[...source.matchAll(/include_str!\("([^"]+)"/g)].map((m) =>
        join("tests/e2e", m[1]).replaceAll("\\", "/"),
      ),
    ]
      // What a test writes rather than reads.
      .filter((path) => path !== "." && path !== "target" && !path.startsWith("target/"));
    for (const path of reads) {
      const read = path.replace(/\/$/, "");
      const covered =
        compiled.shared(read) ||
        compiled.suites[suite].own(read) ||
        compiled.suites[suite].own(`${read}/x`) ||
        Object.values(compiled.crates).some((claims) => claims(read));
      assert.ok(covered, `${module} reads ${read}, which is no input of ${suite}`);
    }
  }
});

function mapping(betaDependsOnAlpha) {
  return {
    shared: ["Cargo.lock"],
    sources: ["crates/**"],
    fallback: "alpha",
    crates: {
      alpha: { paths: ["crates/alpha/**"], deps: [] },
      beta: { paths: ["crates/beta/**"], deps: betaDependsOnAlpha ? ["alpha"] : [] },
    },
    suites: {
      one: { paths: ["tests/one.rs"], crates: ["alpha"], modules: ["one"] },
      two: { paths: ["tests/two.rs"], crates: ["beta"], modules: ["two"] },
    },
  };
}

function run(args) {
  return spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
}

const WINDOW = [
  "--repo",
  "",
  "--ref",
  "main",
  "--since",
  "2026-09-15T00:00:00Z",
  "--until",
  "2026-10-16T00:00:00Z",
];
const window = () => WINDOW.map((arg) => (arg === "" ? repo : arg));

let outputs = 0;

/** What the command wrote to --out; what it printed is one line. */
function written(args, { json = true } = {}) {
  const out = join(scratch, `out-${outputs++}`);
  const ran = run([...args, "--out", out, ...(json ? ["--json"] : [])]);
  assert.equal(ran.status, 0, ran.stderr);
  assert.equal(ran.stdout.trim().split("\n").length, 1, ran.stdout);
  const text = readFileSync(out, "utf8");
  return json ? JSON.parse(text) : text;
}

function mappingFile(name, proposal) {
  const file = join(scratch, `${name}.json`);
  writeFileSync(file, JSON.stringify(proposal));
  return file;
}

function replayed(name, proposal, extra = ["--records", join(scratch, "records")]) {
  return written(["--mapping", mappingFile(name, proposal), ...window(), ...extra]);
}

function refused(args, message) {
  const ran = run(args);
  assert.equal(ran.status, 1, ran.stdout);
  assert.match(ran.stderr, message);
  assert.match(ran.stderr, /\nnext: /);
}

const affectedBySubject = (result) =>
  Object.fromEntries(result.perChange.map((change) => [change.subject, change.affected]));

test("independent crates: a change runs only the suites that exercise what it touched", () => {
  const result = replayed("independent", mapping(false));
  assert.deepEqual(affectedBySubject(result), {
    "feat: alpha": ["one"],
    "feat: beta": ["two"],
    "test: suite one": ["one"],
    "fix: the lock": ["one", "two"],
    "docs: the readme": [],
    "feat: a crate the mapping forgot": ["one"],
  });
  assert.deepEqual(result.summary.unclaimedFiles, ["crates/gamma/src/lib.rs"]);
  assert.equal(result.suites.one.unaffected, 2);
  assert.equal(result.suites.two.unaffected, 4);
  assert.equal(result.summary.skippedSuiteRuns, 6);
  // `one` is 15 test-seconds and `two` 20: skipped twice and four times.
  assert.equal(result.summary.testSecondsPerRun, 35);
  assert.equal(result.summary.testSecondsSkipped, 2 * 15 + 4 * 20);
});

test("a dependency carries a change to the crates that depend on it", () => {
  const result = replayed("dependent", mapping(true));
  assert.deepEqual(affectedBySubject(result), {
    "feat: alpha": ["one", "two"],
    "feat: beta": ["two"],
    "test: suite one": ["one"],
    "fix: the lock": ["one", "two"],
    "docs: the readme": [],
    "feat: a crate the mapping forgot": ["one", "two"],
  });
  assert.equal(result.suites.two.unaffected, 2);
  assert.equal(result.summary.testSecondsSkipped, 2 * 15 + 2 * 20);
});

test("a dependency of a dependency carries a change too", () => {
  const proposal = mapping(false);
  proposal.crates.gamma = { paths: ["crates/gamma/**"], deps: [] };
  proposal.crates.alpha.deps = ["gamma"];
  proposal.crates.beta.deps = ["alpha"];
  proposal.fallback = "beta";
  const result = replayed("chain", proposal);
  assert.deepEqual(affectedBySubject(result)["feat: a crate the mapping forgot"], ["one", "two"]);
  assert.deepEqual(result.summary.unclaimedFiles, []);
});

test("without records the suites are counted and not weighed", () => {
  const result = replayed("unweighed", mapping(false), []);
  assert.equal(result.suites.one.testSeconds, null);
  assert.equal(result.summary.testSecondsPerRun, undefined);
  assert.equal(result.summary.skippedSuiteRuns, 6);
});

test("a test module no suite names is reported, not silently weighed as nothing", () => {
  assert.deepEqual(replayed("stray", mapping(false)).summary.modulesInNoSuite, ["three"]);
});

test("a window holding no merged change replays nothing", () => {
  const file = mappingFile("empty-window", mapping(false));
  const result = written([
    "--mapping",
    file,
    "--repo",
    repo,
    "--ref",
    "main",
    "--since",
    "2026-11-01T00:00:00Z",
    "--until",
    "2026-12-01T00:00:00Z",
  ]);
  assert.equal(result.summary.changes, 0);
  assert.equal(result.summary.skippedSuiteFraction, null);
});

test("the Markdown result tables each suite", () => {
  const file = mappingFile("markdown", mapping(false));
  const text = written(["--mapping", file, ...window(), "--records", join(scratch, "records")], {
    json: false,
  });
  assert.match(text, /^6 merged changes, 2 suites: 6 of 12 suite runs skipped \(50\.0%\)\./);
  assert.match(
    text,
    /Test-seconds: 35\.0 per full run; 110\.0 skipped over the window \(52\.4%\)\./,
  );
  assert.match(text, /\| one \| 4 \| 33\.3% \| 15\.0 \|/);
  assert.match(text, /Source files no crate claims: crates\/gamma\/src\/lib\.rs\./);
});

test("--today reads today's shape from the repository's own nx.json and e2e binary", () => {
  const result = written(["--today", ...window()]);
  assert.deepEqual(Object.keys(result.suites), ["e2e"]);
  // Every change in the window but the readme touches the crate, the lock or the tests.
  assert.equal(result.suites.e2e.affected, 5);
  assert.equal(result.suites.e2e.unaffected, 1);
});

test("a mapping that does not hold together is refused before any history is read", () => {
  const broken = (change) => {
    const proposal = mapping(false);
    change(proposal);
    return [
      "--mapping",
      mappingFile("broken", proposal),
      "--out",
      join(scratch, "x"),
      "--repo",
      join(scratch, "nowhere"),
    ];
  };
  refused(
    broken((p) => {
      p.suites.two.crates = ["delta"];
    }),
    /suite two exercises unknown crate delta/,
  );
  refused(
    broken((p) => {
      p.crates.beta.deps = ["delta"];
    }),
    /crate beta depends on unknown crate delta/,
  );
  refused(
    broken((p) => {
      p.suitez = p.suites;
    }),
    /the mapping has a key this replay does not read: suitez/,
  );
  refused(
    broken((p) => {
      p.suites.one.path = p.suites.one.paths;
    }),
    /suites\.one has a key this replay does not read: path/,
  );
  refused(
    broken((p) => {
      p.crates.alpha.paths = "crates/alpha/**";
    }),
    /crates\.alpha\.paths is not a list of nonempty strings/,
  );
  refused(
    broken((p) => {
      p.fallback = "delta";
    }),
    /fallback delta is not a crate/,
  );
  refused(
    broken((p) => {
      p.suites = {};
    }),
    /the mapping names no suite/,
  );
});

test("an invocation with no window, revision or output it can use is refused before git runs", () => {
  const file = mappingFile("ok", mapping(false));
  const base = ["--mapping", file, "--repo", repo, "--out", join(scratch, "x")];
  refused(["--mapping", file], /--out names no file for the result/);
  refused(["--out", join(scratch, "x")], /name the split to replay/);
  refused([...base, "--today"], /name the split to replay/);
  refused([...base, "--since", "yesterday"], /are not a window/);
  refused(
    [...base, "--since", "2026-10-16T00:00:00Z", "--until", "2026-09-15T00:00:00Z"],
    /are not a window/,
  );
  refused([...base, "--ref", "--output=/tmp/owned"], /is not a revision/);
  refused([...base, "--exclude", "("], /--exclude is not a pattern/);
  refused([...base, "--ref", "no-such-branch"], /the history of no-such-branch .* cannot be read/);
});

test("globs cross directories only at a double star", () => {
  assert.ok(globToRegExp("src/**").test("src/channel/layout.rs"));
  assert.ok(globToRegExp("tests/e2e/**/*").test("tests/e2e/harness.rs"));
  assert.ok(globToRegExp("**/project.json").test("project.json"));
  assert.ok(!globToRegExp("src/*.rs").test("src/channel/layout.rs"));
  assert.ok(globToRegExp("src/*.rs").test("src/plan.rs"));
});

test("--run weighs the suites by the run it names, and refuses one the records do not hold", () => {
  const records = join(scratch, "two-runs");
  mkdirSync(join(records, "e2e"), { recursive: true });
  for (const [run, wall] of [
    ["first", 10],
    ["second", 40],
  ]) {
    writeFileSync(
      join(records, "e2e", `${run}.one.json`),
      JSON.stringify({
        ...golden,
        test: "one::first",
        run_id: run,
        wall_s: wall,
        unattributed_s: wall,
        categories,
      }),
    );
  }
  const weighed = (run) =>
    replayed(`run-${run}`, mapping(false), ["--records", records, "--run", run]).suites.one
      .testSeconds;
  assert.equal(weighed("first"), 10);
  assert.equal(weighed("second"), 40);
  refused(
    [
      "--mapping",
      mappingFile("run-none", mapping(false)),
      ...window(),
      "--out",
      join(scratch, "x"),
      "--records",
      records,
      "--run",
      "third",
    ],
    /no record belongs to run third/,
  );
});

test("a mapping's containers and names are its own, not an array's or an object's inherited ones", () => {
  const broken = (change) => {
    const proposal = mapping(false);
    change(proposal);
    return [
      "--mapping",
      mappingFile("inherited", proposal),
      "--out",
      join(scratch, "x"),
      "--repo",
      repo,
    ];
  };
  refused(
    broken((p) => {
      p.suites = Object.values(p.suites);
    }),
    /suites is not an object of named entries/,
  );
  refused(
    broken((p) => {
      p.crates.beta.deps = ["toString"];
    }),
    /crate beta depends on unknown crate toString/,
  );
  refused(
    broken((p) => {
      p.fallback = "constructor";
    }),
    /fallback constructor is not a crate/,
  );
  refused(
    broken((p) => {
      p.fallback = ["alpha"];
    }),
    /fallback alpha is not a crate/,
  );
});

test("--today refuses an nx.json whose inputs cannot be expanded", () => {
  const dir = join(scratch, "cyclic");
  mkdirSync(join(dir, "tests", "e2e"), { recursive: true });
  writeFileSync(join(dir, "tests", "e2e", "main.rs"), "mod one;\n");
  const today = (namedInputs) => {
    writeFileSync(join(dir, "nx.json"), JSON.stringify({ namedInputs }));
    return ["--today", "--repo", dir, "--out", join(scratch, "x")];
  };
  refused(
    today({ rustBuild: ["a"], a: ["rustBuild"], e2eSource: [] }),
    /rustBuild -> a -> rustBuild is a cycle/,
  );
  refused(today({ rustBuild: "src/**" }), /rustBuild is not a list of inputs/);
  refused(
    today({ rustBuild: ["{workspaceRoot}/Cargo.lock"], e2eSource: [] }),
    /rustBuild names no source/,
  );
});

test("a result that cannot be written says where it was going and what to fix", () => {
  refused(
    [
      "--mapping",
      mappingFile("unwritable", mapping(false)),
      ...window(),
      "--out",
      join(scratch, "no-such-dir", "r.md"),
    ],
    /--out .*no-such-dir.*r\.md cannot be written/,
  );
});

test("only the e2e binary's records weigh an e2e suite", () => {
  const records = join(scratch, "two-binaries");
  mkdirSync(join(records, "e2e"), { recursive: true });
  mkdirSync(join(records, "note"), { recursive: true });
  const write = (binary, wall) =>
    writeFileSync(
      join(records, binary, "one.first.json"),
      JSON.stringify({
        ...golden,
        binary,
        test: "one::first",
        run_id: "r",
        wall_s: wall,
        unattributed_s: wall,
        categories,
      }),
    );
  write("e2e", 10);
  write("note", 1000);
  const result = replayed("two-binaries", mapping(false), ["--records", records]);
  assert.equal(result.suites.one.testSeconds, 10);
});

test("a crate or suite named like an object's own property is refused", () => {
  for (const [change, message] of [
    [
      (p) => {
        p.suites.one2 = p.suites.one;
      },
      /"__proto__" is not a suite name/,
    ],
    [
      (p) => {
        p.crates.constructor = { paths: ["x/**"] };
      },
      /"constructor" is not a crate name/,
    ],
    [
      (p) => {
        p.suites["one two"] = p.suites.one;
      },
      /"one two" is not a suite name/,
    ],
  ]) {
    const proposal = JSON.parse(JSON.stringify(mapping(false)));
    change(proposal);
    // Through JSON text, as a mapping file arrives: `__proto__` is then an own key.
    const file = join(scratch, "reserved.json");
    writeFileSync(file, JSON.stringify(proposal).replace('"one2":', '"__proto__":'));
    refused(["--mapping", file, "--out", join(scratch, "x"), "--repo", repo], message);
  }
});

test("the window holds a change committed at its start and not one committed at its end", () => {
  const file = mappingFile("edges", mapping(false));
  const result = written([
    "--mapping",
    file,
    "--repo",
    repo,
    "--ref",
    "main",
    "--since",
    "2026-09-16T00:00:00Z",
    "--until",
    "2026-09-18T00:00:00Z",
  ]);
  assert.deepEqual(
    result.perChange.map((change) => change.subject),
    ["feat: alpha", "feat: beta"],
  );
});

test("a file moved from one crate to another touches both", () => {
  const file = mappingFile("moved", mapping(false));
  const result = written([
    "--mapping",
    file,
    "--repo",
    repo,
    "--ref",
    "main",
    "--since",
    "2026-10-24T00:00:00Z",
    "--until",
    "2026-10-26T00:00:00Z",
  ]);
  assert.deepEqual(affectedBySubject(result), {
    "refactor: move a module between crates": ["one", "two"],
  });
});

test("a module named like an object's own property is weighed by its records and nothing else", () => {
  const records = join(scratch, "inherited-modules");
  mkdirSync(join(records, "e2e"), { recursive: true });
  writeFileSync(
    join(records, "e2e", "constructor.first.json"),
    JSON.stringify({
      ...golden,
      test: "constructor::first",
      run_id: "r",
      wall_s: 3,
      unattributed_s: 3,
      categories,
    }),
  );
  const proposal = mapping(false);
  proposal.suites.one.modules = ["constructor"];
  proposal.suites.two.modules = ["toString"];
  const result = replayed("inherited-modules", proposal, ["--records", records]);
  assert.equal(result.suites.one.testSeconds, 3);
  assert.equal(result.suites.two.testSeconds, 0);
  assert.deepEqual(result.summary.modulesInNoSuite, []);
});

test("a changed file whose name git would quote is matched by the name it has", () => {
  const file = mappingFile("quoted", mapping(false));
  const result = written([
    "--mapping",
    file,
    "--repo",
    repo,
    "--ref",
    "main",
    "--since",
    "2026-10-27T00:00:00Z",
    "--until",
    "2026-10-29T00:00:00Z",
  ]);
  assert.deepEqual(affectedBySubject(result), { "feat: a file git quotes the name of": ["two"] });
  assert.deepEqual(result.summary.unclaimedFiles, []);
});
