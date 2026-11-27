import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
  existsSync,
  chmodSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, delimiter } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");

for (const scenario of ["e2e", "ordinary", "helper failure"]) {
  test(`the real tier recipe composes scheduling and preparation: ${scenario}`, (t) => {
    const dir = mkdtempSync(
      join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "tier-recipe-"),
    );
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    mkdirSync(join(dir, "scripts"));
    mkdirSync(join(dir, "bin"));
    cpSync(join(root, "justfile"), join(dir, "justfile"));
    cpSync(join(root, "Cargo.toml"), join(dir, "Cargo.toml"));
    for (const name of ["nx-precedence.sh", "e2e-prepare.sh", "ensure-test-archive.sh"]) {
      cpSync(join(root, "scripts", name), join(dir, "scripts", name));
    }
    writeFileSync(join(dir, "package.json"), '{"name":"tier-recipe","private":true}');
    writeFileSync(join(dir, "nx.json"), "{}");
    symlinkSync(join(root, "node_modules"), join(dir, "node_modules"));
    writeFileSync(
      join(dir, "bin", "cargo"),
      `#!/usr/bin/env bash
set -eu
if [ "$1" = metadata ]; then
  echo '{"packages":[{"name":"onepipeline-testfakes","targets":[{"kind":["bin"]}]}]}'
elif [ "$1" = llvm-cov ]; then
  echo "export CARGO_TARGET_DIR='$PWD/target'"
elif [ "$1" = build ]; then
  echo "$*" >> builds
  [ "$SCENARIO" != 'helper failure' ] || exit 43
else
  exit 44
fi
`,
    );
    chmodSync(join(dir, "bin", "cargo"), 0o755);
    writeFileSync(
      join(dir, "scripts", "coverage-unit.sh"),
      `#!/usr/bin/env bash
set -eu
printf '%s\\n' "$@" > coverage-args
printf '%s\\n' "$RUSTFLAGS" "$COVERAGE_UNIT_IGNORE_REGEX" > coverage-env
for i in $(seq 100); do
  [ -f side.nice ] && break
  sleep 0.1
done
if [ "$SCENARIO" = e2e ] && [ -r /proc/self/stat ]; then
  for i in $(seq 100); do
    [ "$(cat side.nice)" = 19 ] && break
    sleep 0.1
  done
else
  sleep 2
fi
`,
    );
    writeFileSync(
      join(dir, "lead.sh"),
      `#!/usr/bin/env bash
status=0
just _tier-test "$UNIT" 'test(=a_representative_journey)' || status=$?
touch done
exit "$status"
`,
    );
    writeFileSync(
      join(dir, "side.mjs"),
      `import {writeFileSync, existsSync} from 'node:fs';
import {getPriority} from 'node:os';
const timer = setInterval(() => {
  writeFileSync('side.nice', String(getPriority()));
  if (existsSync('done')) clearInterval(timer);
}, 100);
setTimeout(() => { clearInterval(timer); }, 20000).unref();
`,
    );
    for (const [name, command] of [
      ["lead", "bash lead.sh"],
      ["side", "node side.mjs"],
    ]) {
      mkdirSync(join(dir, name));
      writeFileSync(
        join(dir, name, "project.json"),
        JSON.stringify({ name, targets: { work: { command, cache: false } } }),
      );
    }
    const env = {
      ...process.env,
      PATH: `${join(dir, "bin")}${delimiter}${process.env.PATH}`,
      SCENARIO: scenario,
      UNIT: scenario === "ordinary" ? "onepipeline-note-journeys" : "onepipeline-e2e",
      ONEPIPELINE_TEST_ARCHIVE: "",
      NX_DAEMON: "false",
      NX_NO_CLOUD: "true",
      NX_RUN_COMMANDS_DIRECTLY: "true",
    };
    for (const key of [
      "NX_INVOCATION_ROOT_PID",
      "NX_TASK_TARGET_PROJECT",
      "NX_TASK_TARGET_TARGET",
      "NX_WORKSPACE_ROOT_PATH",
      "COVERAGE_UNIT_IGNORE_REGEX",
    ])
      delete env[key];
    const result = spawnSync(
      join(root, "node_modules", ".bin", "nx"),
      [
        "run-many",
        "-t",
        "work",
        "--parallel=2",
        "--skip-nx-cache",
        "--outputStyle=stream-without-prefixes",
      ],
      { cwd: dir, env, encoding: "utf8", timeout: 40000 },
    );
    const output = `${result.stdout}\n${result.stderr}`;
    const linux = process.platform === "linux";
    if (scenario === "helper failure" && linux) {
      assert.notEqual(result.status, 0, output);
      assert.match(output, /ACTION: resolve the Cargo build failure/);
      assert.equal(existsSync(join(dir, "coverage-args")), false);
    } else {
      assert.equal(result.status, 0, output);
      assert.deepEqual(readFileSync(join(dir, "coverage-args"), "utf8").trim().split("\n"), [
        env.UNIT,
        "--",
        "nextest",
        "--locked",
        "-E",
        "test(=a_representative_journey)",
        "--final-status-level",
        "fail",
      ]);
      assert.equal(
        readFileSync(join(dir, "coverage-env"), "utf8"),
        "-D warnings\n/crates/testfakes/\n",
      );
      assert.equal(
        readFileSync(join(dir, "side.nice"), "utf8"),
        linux && scenario === "e2e" ? "19" : "0",
      );
    }
    if (linux && scenario !== "ordinary") {
      assert.equal(
        readFileSync(join(dir, "builds"), "utf8").trim().split("\n").length,
        scenario === "e2e" ? 4 : 1,
      );
    } else {
      assert.equal(existsSync(join(dir, "builds")), false);
      assert.doesNotMatch(output, /helpers ready/);
    }
  });
}
