// `scripts/coverage-tiers.mjs` over a copy of this repository's project graph: what it names,
// and the malformed project files it refuses rather than reading past.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { TIERS, fixtureEnvironment, git, root } from "./support.mjs";

const PROJECTS = git(root, "ls-files", "project.json", "*/project.json").split("\n");

/** The justfile, the script and every tracked project file, in a fresh repository. */
function graph(t) {
  const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "tiers-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (const path of ["justfile", "scripts/coverage-tiers.mjs", ...PROJECTS]) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    cpSync(join(root, path), join(dir, path));
  }
  git(dir, "init", "--quiet");
  git(dir, "add", "-A");
  return dir;
}

const tiers = (dir) =>
  spawnSync("node", ["scripts/coverage-tiers.mjs"], {
    cwd: dir,
    encoding: "utf8",
    env: fixtureEnvironment(),
  });

const edit = (dir, path, change) => {
  const project = JSON.parse(readFileSync(join(dir, path), "utf8"));
  change(project);
  writeFileSync(join(dir, path), JSON.stringify(project));
};

test("names every coverage tier this graph holds", (t) => {
  const run = tiers(graph(t));
  assert.equal(run.status, 0, run.stderr);
  assert.deepEqual(run.stdout.trim().split(" "), [...TIERS].sort());
});

const MALFORMED = [
  [
    "a project with no name",
    "tests/note/project.json",
    (project) => {
      delete project.name;
    },
    /tests\/note\/project\.json has no project name/,
  ],
  [
    "a command that is not a string",
    "tests/note/project.json",
    (project) => {
      project.targets.test.command = ["just", "_note-test"];
    },
    /onepipeline-note-journeys:test has a command that is not a string/,
  ],
  [
    "an output that is not a path",
    "tests/note/project.json",
    (project) => {
      project.targets.test.outputs = [{ path: "target/coverage-units/x.lcov" }];
    },
    /onepipeline-note-journeys:test has an output that is not a path string/,
  ],
  [
    "a dependency without a target name",
    "project.json",
    (project) => {
      project.targets.test.dependsOn.push({ projects: ["onepipeline-note-journeys"] });
    },
    /onepipeline:test has a dependsOn entry without a target name/,
  ],
  [
    "a dependency on a project that is not a name",
    "project.json",
    (project) => {
      project.targets.test.dependsOn.push({ target: "test", projects: [7] });
    },
    /onepipeline:test has a dependsOn entry without a target name and a list of project names/,
  ],
];

for (const [what, path, change, message] of MALFORMED) {
  test(`refuses ${what}, naming it`, (t) => {
    const dir = graph(t);
    edit(dir, path, change);
    const run = tiers(dir);
    assert.equal(run.status, 2, run.stdout + run.stderr);
    assert.match(run.stderr, message);
    assert.equal(run.stdout, "", "a refused graph names no tier");
  });
}
