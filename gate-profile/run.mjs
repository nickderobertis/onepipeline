// `just gate-profile [recipe]`: runs one recipe of this repository (default `gate`) with profiling
// on, and folds what it recorded into target/gate-profile/<UTC timestamp>/report.{md,json}.
//
//   node gate-profile/run.mjs [recipe]
//
// For the run's length, gate-profile/shims is first on PATH, so its `just` times each recipe the
// run executes as a phase and its `cargo` adds `--timings` to every build; scripts/nx.sh gives
// each Nx invocation its own task profile and this run its own fresh Nx cache (nx-hook.sh); and
// sampler.mjs reads every process of the run and the host's load from /proc. Afterwards each
// invocation's task graph is read back with `nx --graph`, outside the timed run, and fold.mjs
// writes the reports.
//
// Exit 0 when the recipe passed and the reports are written; 1 when the recipe failed (the
// reports cover the run up to the failure); 2 for a usage error or a host without /proc; 3 when
// the profile could not be recorded or folded.
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { RECORD_SCHEMAS, RecordError, validateReport, writeReports } from "./fold.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = realpathSync(join(here, ".."));
const shims = join(here, "shims");
const SAMPLE_INTERVAL_MS = 100;

function refuse(status, message) {
  console.error(`gate-profile: ${message}`);
  process.exit(status);
}

const [recipe = "gate", ...extra] = process.argv.slice(2);
if (extra.length)
  refuse(
    2,
    `takes one recipe, got ${[recipe, ...extra].join(" ")}; run \`just gate-profile <recipe>\``,
  );
if (!/^[A-Za-z0-9_][A-Za-z0-9_-]*$/.test(recipe))
  refuse(2, `'${recipe}' is not a recipe name; \`just --list\` shows them`);
if (recipe === "gate-profile")
  refuse(2, "cannot profile itself; name the recipe to profile, e.g. `just gate-profile gate`");
if (process.platform !== "linux" || !existsSync("/proc/self/stat"))
  refuse(2, "profiles from /proc, which only Linux has; run it on a Linux host");

let recipes;
try {
  recipes = JSON.parse(
    execFileSync("just", ["--dump", "--dump-format", "json"], {
      cwd: root,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
    }),
  ).recipes;
} catch (error) {
  refuse(2, `cannot read the justfile through \`just --dump\`: ${error.message}`);
}
if (recipes === null || typeof recipes !== "object" || Array.isArray(recipes))
  refuse(2, "`just --dump --dump-format json` printed no `recipes` object; is `just` too old?");
if (typeof recipes[recipe] !== "object" || recipes[recipe] === null)
  refuse(2, `no recipe '${recipe}' in ${join(root, "justfile")}; \`just --list\` shows them`);

function git(...args) {
  const result = spawnSync("git", args, { cwd: root, encoding: "utf8" });
  return result.status === 0 ? result.stdout.trim() : null;
}

const stamp = new Date()
  .toISOString()
  .replace(/[-:]/g, "")
  .replace(/\.\d+Z$/, "Z");
let dir = join(root, "target", "gate-profile", stamp);
for (let n = 2; existsSync(dir); n++) dir = join(root, "target", "gate-profile", `${stamp}-${n}`);
mkdirSync(dir, { recursive: true });
const runId = randomBytes(8).toString("hex");
const top = process.env.NX_TASK_HASH ?? "";
const commit = git("rev-parse", "HEAD");
writeFileSync(
  join(dir, "meta.json"),
  `${JSON.stringify({ recipe, root, commit, dirty: commit !== null && git("status", "--porcelain") !== "", run: runId })}\n`,
);

const sampler = spawn(
  process.execPath,
  [join(here, "sampler.mjs"), join(dir, "processes.json"), runId, top, String(SAMPLE_INTERVAL_MS)],
  { stdio: ["pipe", "pipe", "inherit"] },
);
const samplerExit = new Promise((resolve) =>
  sampler.on("exit", (code, signal) => resolve(code ?? signal)),
);
const ready = await new Promise((resolve) => {
  sampler.stdout.once("data", (chunk) => resolve(String(chunk).startsWith("ready")));
  sampler.once("exit", () => resolve(false));
});
if (!ready)
  refuse(3, `the process sampler did not start (exit ${await samplerExit}); see its error above`);

// The run inherits nothing of a profiled run this one may itself be running inside.
const inherited = { ...process.env };
for (const name of Object.keys(inherited))
  if (name.startsWith("ONEPIPELINE_GATE_PROFILE_")) delete inherited[name];
for (const name of ["NX_PROFILE", "NX_CACHE_DIRECTORY", "NX_WORKSPACE_DATA_DIRECTORY"])
  delete inherited[name];
const environment = { ...inherited };
Object.assign(environment, {
  PATH: `${shims}:${process.env.PATH ?? ""}`,
  ONEPIPELINE_GATE_PROFILE_DIR: dir,
  ONEPIPELINE_GATE_PROFILE_ROOT: root,
  ONEPIPELINE_GATE_PROFILE_RUN: runId,
  ONEPIPELINE_GATE_PROFILE_TOP: top,
  ONEPIPELINE_GATE_PROFILE_LINGER_MS: String(3 * SAMPLE_INTERVAL_MS),
});
// The recipe under it gets the terminal's interrupt itself; this process stays to fold what ran.
process.on("SIGINT", () => {});
const recipeExit = await new Promise((resolve) => {
  const child = spawn(join(shims, "just"), [recipe], {
    cwd: root,
    env: environment,
    stdio: "inherit",
  });
  child.on("error", (error) => resolve({ code: null, error }));
  child.on("exit", (code, signal) => resolve({ code, signal }));
});
sampler.stdin.end();
const sampled = await samplerExit;
if (recipeExit.error) refuse(3, `could not start \`just ${recipe}\`: ${recipeExit.error.message}`);
if (sampled !== 0) refuse(3, `the process sampler failed (exit ${sampled}); see its error above`);
if (!existsSync(join(dir, "run.json")))
  refuse(
    3,
    `\`just ${recipe}\` ended (${recipeExit.signal ?? `exit ${recipeExit.code}`}) before its run was recorded; nothing to fold in ${relative(root, dir)}`,
  );

// Each invocation's task graph, for the critical path. Its own Nx run is outside the timed one,
// and reads this run's workspace data so that it neither recomputes the graph nor touches .nx.
const nxDir = join(dir, "nx");
for (const name of existsSync(nxDir) ? readdirSync(nxDir) : []) {
  if (!name.endsWith(".invocation.json")) continue;
  const id = name.slice(0, -".invocation.json".length);
  let args;
  try {
    const record = JSON.parse(readFileSync(join(nxDir, name), "utf8"));
    if (validateReport(record, RECORD_SCHEMAS.invocation).length) throw new Error("its shape");
    args = record.args;
  } catch (error) {
    // The fold refuses the same record, naming what is wrong with it.
    console.error(`gate-profile: invocation record ${name} is unreadable (${error.message})`);
    continue;
  }
  const graph = spawnSync(
    "bash",
    ["scripts/nx.sh", ...args, `--graph=${join(nxDir, `${id}.graph.json`)}`],
    {
      cwd: root,
      encoding: "utf8",
      env: {
        ...inherited,
        ONEPIPELINE_NX_SHOW_OUTPUT: "1",
        NX_CACHE_DIRECTORY: join(dir, "nx-state", "cache"),
        NX_WORKSPACE_DATA_DIRECTORY: join(dir, "nx-state", "data"),
      },
    },
  );
  if (graph.status !== 0)
    console.error(
      `gate-profile: could not read the task graph of \`nx ${args.join(" ")}\`, so its critical path is its longest task:\n${graph.stderr}`,
    );
}

try {
  writeReports(dir);
} catch (error) {
  refuse(
    3,
    `${error instanceof RecordError ? error.message : error.stack}\nthe raw records are in ${relative(root, dir)}; fix the cause and fold them again with \`node gate-profile/fold.mjs ${relative(root, dir)}\``,
  );
}
const report = join(relative(root, dir), "report.md");
if (recipeExit.code !== 0)
  refuse(
    1,
    `\`just ${recipe}\` failed (${recipeExit.signal ?? `exit ${recipeExit.code}`}); the profile of the run up to the failure is ${report}`,
  );
console.log(`gate-profile: profiled \`just ${recipe}\`; report: ${report}`);
