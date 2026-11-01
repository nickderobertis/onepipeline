import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";

// `just gate`'s timing wrapper (scripts/gate-time.sh) and onepipeline-gate-time's reporter
// (scripts/gate-time-budget.mjs), driven as a contributor's `just gate` drives them: the real
// wrapper around a short real body, and the real onebudgetspec binary the justfile pins (the
// one `just bootstrap` installs under target/tools) over the real budgets.yaml. Each case runs in
// a scratch tree holding the repository's own copies of those files and its justfile beside its
// node_modules and installed tools, so the record it writes is the scratch tree's and never a
// contributor's. Exit statuses, warnings and record contents are asserted; durations never are.

const root = fileURLToPath(new URL("../../", import.meta.url));
const scratchRoot = mkdtempSync(
  join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR || tmpdir(), "gate-time-"),
);
after(() => rmSync(scratchRoot, { recursive: true, force: true }));
let trees = 0;

function tree({ tools = true } = {}) {
  const dir = join(scratchRoot, String(trees++));
  mkdirSync(join(dir, "scripts"), { recursive: true });
  for (const file of [
    "budgets.yaml",
    "justfile",
    "package.json",
    "scripts/gate-time.sh",
    "scripts/gate-time-budget.mjs",
  ])
    copyFileSync(join(root, file), join(dir, file));
  symlinkSync(join(root, "node_modules"), join(dir, "node_modules"), "dir");
  mkdirSync(join(dir, "target"));
  if (tools) symlinkSync(join(root, "target", "tools"), join(dir, "target", "tools"), "dir");
  return dir;
}
const recordPath = (dir) => join(dir, ".gate-time.json");
const readRecord = (dir) => JSON.parse(readFileSync(recordPath(dir), "utf8"));
function seed(dir, seconds, exit = 0) {
  const started = new Date(Date.UTC(2026, 9, 1, 12, 0, 0));
  const ended = new Date(started.getTime() + seconds * 1000);
  const iso = (date) => date.toISOString().replace(/\.000Z$/, "Z");
  writeFileSync(
    recordPath(dir),
    JSON.stringify({
      schema_version: 1,
      invocation: "seeded",
      started_at: iso(started),
      ended_at: iso(ended),
      exit,
    }),
  );
}
function gate(dir, ...args) {
  const ran = spawnSync("bash", ["scripts/gate-time.sh", ...args], { cwd: dir, encoding: "utf8" });
  if (ran.error) throw ran.error;
  return { status: ran.status, out: ran.stdout + ran.stderr };
}
const WARNING = /gate-time: warning: onepipeline-gate-time is over its budget/;

test("the wrapper records this invocation over a stale record and checks it", () => {
  const dir = tree();
  seed(dir, 5000);
  const before = new Date(Date.now() - 1000).toISOString().replace(/\.\d+Z$/, "Z");
  const ran = gate(dir, "bash", "-c", "echo the gate body ran");
  assert.equal(ran.status, 0, ran.out);
  assert.match(ran.out, /the gate body ran/);
  assert.match(ran.out, /budget onepipeline-gate-time: .* within/);
  assert.doesNotMatch(ran.out, WARNING);
  const record = readRecord(dir);
  assert.equal(record.schema_version, 1);
  assert.equal(record.exit, 0);
  assert.notEqual(record.invocation, "seeded");
  assert.ok(record.started_at >= before, `a fresh start, not the stale one: ${record.started_at}`);
  assert.ok(record.ended_at >= record.started_at);
});

test("a failing body fails the gate with its status, records it and checks no budget", () => {
  const dir = tree();
  seed(dir, 10);
  const ran = gate(dir, "bash", "-c", "exit 7");
  assert.equal(ran.status, 7, ran.out);
  assert.match(ran.out, /the gate failed \(exit 7\), .*run `just gate` again/);
  assert.doesNotMatch(ran.out, /budget onepipeline-gate-time/);
  const record = readRecord(dir);
  assert.equal(record.exit, 7);
  assert.notEqual(record.invocation, "seeded");
});

test("a record the wrapper cannot write fails the gate, and no earlier record stands in", () => {
  const dir = tree();
  // A directory where the record goes: removing the stale record fails, and so would the write.
  mkdirSync(recordPath(dir));
  writeFileSync(join(recordPath(dir), "stale"), "");
  const ran = gate(dir, "bash", "-c", "echo the gate body ran");
  assert.equal(ran.status, 2, ran.out);
  assert.match(ran.out, /cannot remove the earlier record .*: \S/);
  assert.doesNotMatch(ran.out, /the gate body ran/);
  assert.doesNotMatch(ran.out, /budget onepipeline-gate-time/);
});

// The body runs as the wrapper's child, so `$PPID` there is the wrapper's own pid, which names
// the temporary record it writes beside .gate-time.json.
test("a record the wrapper cannot write after the body fails the gate, checking no budget", () => {
  const dir = tree();
  const ran = gate(dir, "bash", "-c", 'mkdir ".gate-time.json.$PPID"');
  assert.equal(ran.status, 2, ran.out);
  assert.match(ran.out, /cannot write the record beside .*: .*[Ii]s a directory/);
  assert.doesNotMatch(ran.out, /budget onepipeline-gate-time/);
  assert.equal(existsSync(recordPath(dir)), false);
});

test("a record the wrapper cannot put in place fails the gate and leaves no temporary file", () => {
  const dir = tree();
  // A directory nobody may write into, where the record goes, made by the body after the
  // wrapper removed the earlier record: the record cannot be moved into place.
  const ran = gate(dir, "bash", "-c", "mkdir .gate-time.json && chmod 555 .gate-time.json");
  try {
    assert.equal(ran.status, 2, ran.out);
    assert.match(ran.out, /cannot replace .*: .*[Pp]ermission denied/);
    assert.doesNotMatch(ran.out, /budget onepipeline-gate-time/);
    assert.deepEqual(
      readdirSync(dir).filter((name) => name.startsWith(".gate-time.json.")),
      [],
    );
  } finally {
    chmodSync(recordPath(dir), 0o755);
  }
});

test("a recorded gate within the threshold passes", () => {
  const dir = tree();
  seed(dir, 1200);
  const ran = gate(dir, "--check");
  assert.equal(ran.status, 0, ran.out);
  assert.match(
    ran.out,
    /budget onepipeline-gate-time: actual 1200 seconds, budget 1800 seconds.* within/,
  );
  assert.doesNotMatch(ran.out, WARNING);
});

test("a recorded gate over the threshold warns naming the budget and still passes", () => {
  const dir = tree();
  seed(dir, 2400);
  const ran = gate(dir, "--check");
  assert.equal(ran.status, 0, ran.out);
  assert.match(ran.out, /budget onepipeline-gate-time: actual 2400 seconds.* over/);
  assert.match(ran.out, WARNING);
});

test("a gate with no pinned checker installed fails, naming how to install it", () => {
  const dir = tree({ tools: false });
  seed(dir, 100);
  const ran = gate(dir, "--check");
  assert.equal(ran.status, 3, ran.out);
  assert.match(ran.out, /the repository-pinned onebudgetspec is missing.*run `just bootstrap`/);
  assert.doesNotMatch(ran.out, /budget onepipeline-gate-time/);
});

test("a missing record fails the gate", () => {
  const dir = tree();
  assert.equal(existsSync(recordPath(dir)), false);
  const ran = gate(dir, "--check");
  assert.equal(ran.status, 3, ran.out);
  assert.match(ran.out, /no gate record at \.gate-time\.json/);
  assert.match(ran.out, /checking onepipeline-gate-time failed \(onebudgetspec exit 3\)/);
});

test("a record that cannot be read fails the check naming why", () => {
  const dir = tree();
  mkdirSync(recordPath(dir));
  const ran = gate(dir, "--check");
  assert.equal(ran.status, 3, ran.out);
  assert.match(ran.out, /cannot read \.gate-time\.json \(EISDIR/);
});

test("a recorded failed gate is no figure, and fails the check", () => {
  const dir = tree();
  seed(dir, 100, 1);
  const ran = gate(dir, "--check");
  assert.equal(ran.status, 3, ran.out);
  assert.match(ran.out, /the recorded gate exited 1, .*run `just gate` again/);
});

test("a malformed record is no figure, and fails the check naming how to recover", () => {
  const good = {
    schema_version: 1,
    invocation: "seeded",
    started_at: "2026-10-01T12:00:00Z",
    ended_at: "2026-10-01T12:10:00Z",
    exit: 0,
  };
  for (const [what, text] of [
    ["not JSON", "{"],
    ["another schema", JSON.stringify({ ...good, schema_version: 2 })],
    ["an unreadable start", JSON.stringify({ ...good, started_at: "yesterday" })],
    ["an end before its start", JSON.stringify({ ...good, ended_at: "2026-10-01T11:00:00Z" })],
    ["an exit that is no integer", JSON.stringify({ ...good, exit: "0" })],
    ["a start in another format", JSON.stringify({ ...good, started_at: "2026-10-01 12:00" })],
    ["no invocation", JSON.stringify({ ...good, invocation: undefined })],
    ["a day that does not exist", JSON.stringify({ ...good, started_at: "2026-02-30T12:00:00Z" })],
  ]) {
    const dir = tree();
    writeFileSync(recordPath(dir), text);
    const ran = gate(dir, "--check");
    assert.equal(ran.status, 3, `${what}: ${ran.out}`);
    assert.match(ran.out, /delete it and run `just gate`/, what);
  }
});

test("an invalid budgets.yaml fails the gate", () => {
  const dir = tree();
  seed(dir, 100);
  writeFileSync(join(dir, "budgets.yaml"), "schema_version: 1\nbudgets: not-a-list\n");
  const ran = gate(dir, "--check");
  assert.equal(ran.status, 2, ran.out);
  assert.match(ran.out, /checking onepipeline-gate-time failed \(onebudgetspec exit 2\)/);
});

// The recipe, as `just` itself reads it: `gate` must depend on nothing and run exactly one line,
// the wrapper around `just _gate-body`, so every recipe the gate runs is inside the timing.
export function recipeProblems(recipes) {
  const problems = [];
  const gateRecipe = recipes.gate;
  if (!gateRecipe) return ["the justfile has no gate recipe"];
  if (gateRecipe.dependencies.length)
    problems.push(
      `gate depends on ${gateRecipe.dependencies.map((d) => d.recipe).join(", ")}, which run outside the timing wrapper`,
    );
  const lines = gateRecipe.body.map((line) =>
    line.map((part) => (typeof part === "string" ? part : "{{…}}")).join(""),
  );
  if (lines.length !== 1 || lines[0] !== '@bash scripts/gate-time.sh just _gate-body "{{…}}"')
    problems.push(
      `gate's body is not the timing wrapper around just _gate-body alone: ${JSON.stringify(lines)}`,
    );
  const body = recipes["_gate-body"];
  if (!body?.dependencies.length) problems.push("_gate-body runs nothing");
  return problems;
}
const dumped = () =>
  JSON.parse(
    execFileSync("just", ["--dump", "--dump-format", "json"], { cwd: root, encoding: "utf8" }),
  ).recipes;

test("the gate recipe runs its whole body inside the timing wrapper", () => {
  const recipes = dumped();
  assert.deepEqual(recipeProblems(recipes), []);
  const body = recipes["_gate-body"];
  assert.deepEqual(
    body.dependencies.map((d) => d.recipe),
    ["check"],
  );
  assert.ok(
    body.body.some((line) => line[0] === '@just lint-llm-diff "'),
    "_gate-body does not run the judged lint over the diff",
  );
});

test("a gate that stops running its whole body inside the wrapper is refused", () => {
  const cases = [
    (r) => r.gate.dependencies.push({ recipe: "check", arguments: [] }),
    (r) => {
      r.gate.body = [["@just _gate-body"]];
    },
    (r) => r.gate.body.push(["@just lint-llm-diff"]),
    (r) => {
      r["_gate-body"].dependencies = [];
    },
  ];
  for (const change of cases) {
    const recipes = dumped();
    change(recipes);
    assert.notDeepEqual(recipeProblems(recipes), [], String(change));
  }
});
