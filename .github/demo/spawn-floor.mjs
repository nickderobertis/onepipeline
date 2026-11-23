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
import { mkdtempSync, rmSync, writeFileSync, readdirSync } from "node:fs";
import { tmpdir, cpus } from "node:os";
import { join } from "node:path";

const [out, binary, n = "100"] = process.argv.slice(2);
const count = Number(n);
const root = mkdtempSync(join(tmpdir(), "spawn-floor-"));
const runs = join(root, "runs");
const spans = join(root, "spans");
const base = { ...process.env, ONEPIPELINE_RUNS_DIR: runs };
delete base.ONEPIPELINE_PROFILE_SPANS;
const cases = [
  ["git --version", "git", ["--version"], base],
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
rmSync(root, { recursive: true, force: true });
writeFileSync(out, JSON.stringify(result, null, 2));
console.log(JSON.stringify(result, null, 2));
