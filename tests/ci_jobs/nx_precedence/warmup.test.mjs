import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const script = join(root, "scripts/e2e-prepare.sh");

function fixture(t) {
  const dir = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "e2e-prepare-"),
  );
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const source = readFileSync(join(root, "justfile"), "utf8");
  const recipe = source.slice(
    source.indexOf("_e2e-doubles-build mode:"),
    source.indexOf("# Explicit uninstrumented archive verb"),
  );
  writeFileSync(
    join(dir, "justfile"),
    `set shell := ["bash", "-eu", "-o", "pipefail", "-c"]\nset positional-arguments\nllvm-cov-target-dir := env("PREPARE_TARGET")\n${recipe}`,
  );
  writeFileSync(
    join(dir, "cargo"),
    `#!/usr/bin/env bash
set -eu
if [ "$1" = metadata ]; then
  if [ -n "\${FAIL_METADATA:-}" ]; then echo 'invalid Cargo manifest' >&2; exit 43; fi
  printf '%s' "$PREPARE_METADATA"
  exit
fi
if [ "$1" = llvm-cov ]; then echo "export CARGO_TARGET_DIR='$PREPARE_TARGET'"; exit; fi
printf '%s|%s|%s\\n' "$*" "$RUSTFLAGS" "$CARGO_TARGET_DIR" >> calls
if [ -n "\${FAIL_BUILD:-}" ]; then exit 42; fi
`,
  );
  chmodSync(join(dir, "cargo"), 0o755);
  writeFileSync(join(dir, "uname"), "#!/usr/bin/env bash\nprintf '%s\\n' Linux\n");
  chmodSync(join(dir, "uname"), 0o755);
  writeFileSync(
    join(dir, "test-command"),
    `#!/usr/bin/env bash
set -eu
[ "$(wc -l < calls)" = 4 ]
printf '%s\\n' "$*" > tested
exit "\${TEST_EXIT:-0}"
`,
  );
  chmodSync(join(dir, "test-command"), 0o755);
  return {
    dir,
    run(extra = {}, args = [join(dir, "test-command"), "an argument with spaces"]) {
      return spawnSync("bash", [script, ...args], {
        cwd: dir,
        env: {
          ...process.env,
          PATH: `${dir}${delimiter}${process.env.PATH}`,
          PREPARE_METADATA: JSON.stringify({
            packages: [
              {
                name: "onepipeline-testfakes",
                dependencies: [{ name: "oneagentgraph" }],
                targets: [{ kind: ["bin"] }],
              },
            ],
          }),
          PREPARE_TARGET: `${dir.replaceAll("\\", "/")}/target/llvm-cov-target`,
          ...extra,
        },
        encoding: "utf8",
        timeout: 30_000,
      });
    },
  };
}

test("the real preparation script builds all instrumented helpers before starting the test command", (t) => {
  const { dir, run } = fixture(t);
  const result = run();
  assert.equal(result.status, 0, result.stderr);
  const calls = readFileSync(join(dir, "calls"), "utf8").trim().split("\n");
  assert.equal(calls.length, 4);
  for (const [index, selection] of [
    "onepipeline-testfakes",
    "oneagentgraph --bin oneagentgraph --locked",
    "oneharness --bin oneharness --locked",
    "onevcs --bin onevcs --locked",
  ].entries()) {
    assert.equal(
      calls[index],
      `build --offline --package ${selection} --target-dir ${dir.replaceAll("\\", "/")}/target/llvm-cov-target|-D warnings|${dir.replaceAll("\\", "/")}/target/llvm-cov-target`,
    );
  }
  assert.equal(readFileSync(join(dir, "tested"), "utf8"), "an argument with spaces\n");
  assert.match(result.stderr, /helpers ready in \d+ seconds/);
});

test("a helper build failure prevents test execution and reports recovery", (t) => {
  const { dir, run } = fixture(t);
  const result = run({ FAIL_BUILD: "1" });
  assert.notEqual(result.status, 0);
  assert.throws(() => readFileSync(join(dir, "tested")), { code: "ENOENT" });
  assert.match(result.stderr, /ACTION: resolve the Cargo build failure/);
});

test("a test failure retains its status after successful helper preparation", (t) => {
  const { run } = fixture(t);
  const result = run({ TEST_EXIT: "19" });
  assert.equal(result.status, 19, result.stderr);
});

test("a missing test command is rejected before building", (t) => {
  const { dir, run } = fixture(t);
  const result = run({}, []);
  assert.equal(result.status, 2);
  assert.throws(() => readFileSync(join(dir, "calls")), { code: "ENOENT" });
  assert.match(result.stderr, /ACTION: pass the coverage-unit command/);
});

test("non-Linux coverage runs keep their uninstrumented command without building helpers", (t) => {
  const { dir, run } = fixture(t);
  writeFileSync(join(dir, "uname"), "#!/usr/bin/env bash\nprintf '%s\\n' Darwin\n");
  chmodSync(join(dir, "uname"), 0o755);
  const result = run({}, ["bash", "-c", "printf '%s' native > tested; exit 19"]);
  assert.equal(result.status, 19, result.stderr);
  assert.equal(readFileSync(join(dir, "tested"), "utf8"), "native");
  assert.throws(() => readFileSync(join(dir, "calls")), { code: "ENOENT" });
  assert.doesNotMatch(result.stderr, /helpers ready/);
});

test("a workspace without helper binaries runs its tests without preparing absent helpers", (t) => {
  const { dir, run } = fixture(t);
  const result = run(
    {
      PREPARE_METADATA: JSON.stringify({
        packages: [
          { name: "onepipeline-testfakes", dependencies: [], targets: [{ kind: ["lib"] }] },
        ],
      }),
    },
    ["bash", "-c", "printf '%s' minimal > tested; exit 19"],
  );
  assert.equal(result.status, 19, result.stderr);
  assert.equal(readFileSync(join(dir, "tested"), "utf8"), "minimal");
  assert.throws(() => readFileSync(join(dir, "calls")), { code: "ENOENT" });
  assert.doesNotMatch(result.stderr, /helpers ready/);
});

test("failed Cargo metadata prevents both helper builds and tests", (t) => {
  const { dir, run } = fixture(t);
  const result = run({ FAIL_METADATA: "1" });
  assert.equal(result.status, 43, result.stderr);
  assert.throws(() => readFileSync(join(dir, "calls")), { code: "ENOENT" });
  assert.throws(() => readFileSync(join(dir, "tested")), { code: "ENOENT" });
  assert.match(result.stderr, /ACTION: resolve the Cargo metadata failure/);
});

test("malformed Cargo metadata is rejected before building or testing", (t) => {
  const { dir, run } = fixture(t);
  const result = run({ PREPARE_METADATA: "{}" });
  assert.notEqual(result.status, 0);
  assert.throws(() => readFileSync(join(dir, "calls")), { code: "ENOENT" });
  assert.throws(() => readFileSync(join(dir, "tested")), { code: "ENOENT" });
  assert.match(result.stderr, /ACTION: restore valid Cargo metadata/);
});

test("helper detection follows the canonical recipe when a selected dependency changes", (t) => {
  const { dir, run } = fixture(t);
  const path = join(dir, "justfile");
  writeFileSync(
    path,
    readFileSync(path, "utf8").replaceAll(
      "--package onepipeline-testfakes",
      "--package renamed-helpers",
    ),
  );
  const result = run({
    PREPARE_METADATA: JSON.stringify({
      packages: [{ name: "renamed-helpers", dependencies: [], targets: [{ kind: ["bin"] }] }],
    }),
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(readFileSync(join(dir, "calls"), "utf8"), /--package renamed-helpers --target-dir/);
  assert.equal(readFileSync(join(dir, "tested"), "utf8"), "an argument with spaces\n");
});
