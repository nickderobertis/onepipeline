// DEMONSTRATION ONLY: the cost of starting a process on this runner, measured by
// starting the same one again and again from Node and timing each start to exit.
//
//   node .github/demo/spawn-floor.mjs OUT.json BINARY [N]
//
// Cases: `git --version`; the binary's `--version` (argument parsing exits before
// its `run`, so no span is recorded); its `runs` over an empty runs root, without and
// with ONEPIPELINE_PROFILE_SPANS, so their difference is what recording a span costs a
// process. Each case runs N times (default 100) after 5 unmeasured warm-ups.
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync, readdirSync, readFileSync, existsSync } from "node:fs";
import { tmpdir, cpus } from "node:os";
import { join } from "node:path";

const [out, binary, n = "100"] = process.argv.slice(2);
const count = Number(n);
const root = mkdtempSync(join(tmpdir(), "spawn-floor-"));
const runs = join(root, "runs");
const spans = join(root, "spans");
const base = { ...process.env, ONEPIPELINE_RUNS_DIR: runs };
delete base.ONEPIPELINE_PROFILE_SPANS;
delete base.GIT_TRACE2_EVENT;
// A real repository, so `git rev-parse` does what the engine's reads do: find and open it.
const repo = join(root, "repo");
spawnSync("git", ["init", "-q", repo]);
const trace = join(root, "trace2.jsonl");
const cases = [
  ["git --version", "git", ["--version"], base],
  ["git rev-parse --git-dir", "git", ["-C", repo, "rev-parse", "--git-dir"], base],
  ["git rev-parse --git-dir, trace2", "git", ["-C", repo, "rev-parse", "--git-dir"], { ...base, GIT_TRACE2_EVENT: trace }],
  ["onepipeline --version", binary, ["--version"], base],
  ["onepipeline runs", binary, ["runs"], base],
  ["onepipeline runs, spans recorded", binary, ["runs"], { ...base, ONEPIPELINE_PROFILE_SPANS: spans }],
];
const result = { platform: process.platform, cpus: cpus().length, cpu: cpus()[0]?.model, count, cases: [] };
for (const [name, program, args, env] of cases) {
  const ms = [];
  for (let i = 0; i < count + 5; i++) {
    const began = process.hrtime.bigint();
    const ran = spawnSync(program, args, { env, stdio: ["ignore", "pipe", "pipe"] });
    const took = Number(process.hrtime.bigint() - began) / 1e6;
    if (ran.status !== 0) throw new Error(`${name} exited ${ran.status}: ${ran.stderr}`);
    if (i >= 5) ms.push(took);
  }
  ms.sort((a, b) => a - b);
  const mean = ms.reduce((a, b) => a + b, 0) / ms.length;
  result.cases.push({ name, mean_ms: mean, median_ms: ms[Math.floor(ms.length / 2)], p10_ms: ms[Math.floor(ms.length / 10)], p90_ms: ms[Math.floor((ms.length * 9) / 10)] });
}
result.span_records = readdirSync(spans).length;
// What git says the traced case spent inside itself, start to exit, against what Node
// measured from outside: the difference is the process's start-up and exit.
if (existsSync(trace)) {
  const abs = readFileSync(trace, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l))
    .filter((e) => e.event === "atexit").map((e) => e.t_abs * 1000);
  abs.sort((a, b) => a - b);
  result.git_trace2_atexit = { n: abs.length, mean_ms: abs.reduce((a, b) => a + b, 0) / abs.length, median_ms: abs[Math.floor(abs.length / 2)] };
}
rmSync(root, { recursive: true, force: true });
writeFileSync(out, JSON.stringify(result, null, 2));
console.log(JSON.stringify(result, null, 2));
