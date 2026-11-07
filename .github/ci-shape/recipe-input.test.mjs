// What scripts/nx-recipe-input.mjs covers, through real `just`: each thing a recipe reaches
// moves its digest when it changes, and what the recipe does not reach leaves it alone.
// nx-inputs.test.mjs holds that real Nx hashes this digest into the tasks that name it.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { after, describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../../scripts/nx-recipe-input.mjs", import.meta.url));
const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "recipe-input-"));
after(() => rmSync(dir, { recursive: true, force: true }));
const JUSTFILE = `set positional-arguments

export SHARED := "exported"
base := "base"
derived := base + "-derived"
argument := "argument"
fallback := "fallback"
unused := "unused"

# what main depends on
dependency value:
    @echo dependency "$1"

main param=fallback: (dependency argument)
    @echo {{derived}}
    @echo "a message naming 'just elsewhere' does not run it"
    @just nested
    @bash helper.sh

nested:
    @echo nested

through-a-script:
    @echo through a script

through-two-scripts:
    @echo through two scripts

elsewhere:
    @echo elsewhere
`;
const HELPER = `#!/usr/bin/env bash
# just elsewhere is a comment here, not a run
echo "and 'just elsewhere' is a message"
OPTION=1 just through-a-script
bash deeper.sh
`;
// It names the first script back, so a walk that did not remember where it had been
// would never finish.
const DEEPER = `#!/usr/bin/env bash
just through-two-scripts
bash helper.sh
`;
const digest = (args, { justfile = JUSTFILE, helper = HELPER, env = {} } = {}) => {
  writeFileSync(join(dir, "justfile"), justfile);
  writeFileSync(join(dir, "helper.sh"), helper);
  writeFileSync(join(dir, "deeper.sh"), DEEPER);
  return spawnSync(process.execPath, [script, ...args], { cwd: dir, encoding: "utf8", env: { ...process.env, ...env } });
};
const main = (options) => digest(["main"], options).stdout.trim();
const replaced = (from, to) => {
  assert.ok(JUSTFILE.includes(from), from);
  return { justfile: JUSTFILE.replace(from, to) };
};

describe("a recipe's digest", () => {
  const base = main();
  it("is a digest, and the same on a second reading", () => {
    assert.match(base, /^[0-9a-f]{64}$/);
    assert.equal(main(), base);
  });
  for (const [what, options] of [
    ["the recipe's own body", replaced("@echo {{derived}}", "@echo changed {{derived}}")],
    ["a recipe it depends on", replaced('@echo dependency "$1"', '@echo changed "$1"')],
    ["a variable a dependency's argument uses", replaced('argument := "argument"', 'argument := "changed"')],
    ["a variable its parameter defaults to", replaced('fallback := "fallback"', 'fallback := "changed"')],
    ["a variable its body uses", replaced('derived := base + "-derived"', 'derived := base + "-changed"')],
    ["a variable that variable uses", replaced('base := "base"', 'base := "changed"')],
    ["an exported variable", replaced('export SHARED := "exported"', 'export SHARED := "changed"')],
    ["a setting", replaced("set positional-arguments\n", "set positional-arguments\nset fallback\n")],
    ["a recipe a `just` line in its body runs", replaced("@echo nested", "@echo changed")],
    ["a recipe a `just` line in a script it names runs", replaced("@echo through a script", "@echo changed")],
    ["a recipe a script that script names runs, in a cycle of scripts", replaced("@echo through two scripts", "@echo changed")],
  ]) {
    it(`moves with ${what}`, () => assert.notEqual(main(options), base));
  }
  for (const [what, options] of [
    ["a recipe it does not reach, however messages and comments name it", replaced("@echo elsewhere", "@echo changed")],
    ["a variable it does not use", replaced('unused := "unused"', 'unused := "changed"')],
    ["a comment", replaced("# what main depends on", "# a different comment")],
  ]) {
    it(`stays with ${what}`, () => assert.equal(main(options), base));
  }
  it("can be taken over a variable alone", () => {
    const alone = (options) => digest(["--variable", "unused"], options);
    assert.match(alone().stdout.trim(), /^[0-9a-f]{64}$/);
    assert.notEqual(alone(replaced('unused := "unused"', 'unused := "changed"')).stdout, alone().stdout);
    assert.equal(alone(replaced("@echo {{derived}}", "@echo changed {{derived}}")).stdout, alone().stdout);
  });
  it("covers a variable the task reads some other way, when named", () => {
    const edit = replaced('unused := "unused"', 'unused := "changed"');
    const named = (options) => digest(["main", "--variable", "unused"], options).stdout.trim();
    assert.notEqual(named(edit), named());
  });
});

describe("a recipe digest that cannot be taken", () => {
  const fails = (args, options) => {
    const [first, second] = [digest(args, options), digest(args, options)];
    for (const failed of [first, second]) {
      assert.equal(failed.status, 1, failed.stdout);
      assert.doesNotMatch(failed.stdout, /^[0-9a-f]{64}$/m);
      assert.match(failed.stderr, /ACTION: /);
    }
    assert.notEqual(first.stdout, second.stdout, "two failures shared a key");
    return first.stderr;
  };
  it("refuses a recipe the justfile does not define", () => assert.match(fails(["no-such-recipe"]), /no recipe 'no-such-recipe'/));
  it("refuses a variable the justfile does not define", () => assert.match(fails(["main", "--variable", "no-such"]), /no variable 'no-such'/));
  it("refuses --variable with no name", () => assert.match(fails(["main", "--variable"]), /--variable names no variable/));
  it("refuses being asked for nothing", () => assert.match(fails([]), /name a recipe/));
  it("refuses a justfile just cannot parse", () => {
    fails(["main"], { justfile: "main:\n    @echo {{nope}}\n" });
  });
  for (const [what, answer, reason] of [
    ["no recipes", '{"assignments":{},"settings":{}}', /carries no recipes/],
    ["a recipe of an unknown shape", '{"recipes":{"main":{"body":"text"}},"assignments":{},"settings":{}}', /recipe 'main' an unknown shape/],
    ["a variable of an unknown shape", '{"recipes":{},"assignments":{"x":{}},"settings":{}}', /variable 'x' an unknown shape/],
    ["a body expression of an unknown shape", '{"recipes":{"main":{"body":[[{"kind":"variable","name":"x"}]],"dependencies":[],"parameters":[]}},"assignments":{},"settings":{}}', /unknown expression/],
    ["an expression form it does not know", '{"recipes":{"main":{"body":[[["new-form","x"]]],"dependencies":[],"parameters":[]}},"assignments":{},"settings":{}}', /unknown expression/],
    ["a use naming no variable", '{"recipes":{"main":{"body":[[["variable"]]],"dependencies":[],"parameters":[]}},"assignments":{},"settings":{}}', /unknown expression/],
    ["a variable's value of an unknown shape", '{"recipes":{"main":{"body":[[["variable","x"]]],"dependencies":[],"parameters":[]}},"assignments":{"x":{"export":false,"value":{"kind":"new"}}},"settings":{}}', /unknown expression/],
  ]) {
    it(`refuses a dump with ${what}`, () => {
      const bin = join(dir, "fake-just");
      mkdirSync(bin, { recursive: true });
      writeFileSync(join(bin, "just"), `#!/usr/bin/env bash\nprintf '%s' '${answer}'\n`);
      chmodSync(join(bin, "just"), 0o755);
      assert.match(fails(["main"], { env: { PATH: `${bin}${delimiter}${process.env.PATH}` } }), reason);
    });
  }
});
