import assert from "node:assert/strict";
import { readFileSync, rmSync, mkdirSync, writeFileSync } from "node:fs";
import { delimiter, join } from "node:path";
import { parse } from "yaml";
import { evaluate } from "../../workflow-graph.mjs";
import { command, outputs, root, sidecar } from "./cache-workspace.mjs";
const ci = parse(readFileSync(join(root, ".github/workflows/ci.yml"), "utf8"));
const cd = parse(readFileSync(join(root, ".github/workflows/release-plz.yml"), "utf8"));
export const repo = { id: 1, full_name: "example/project" };
export const pull = (ws, number = 1) => ({ number, head: { sha: ws.head, ref: `change-${number}`, repo }, base: { sha: ws.base, ref: "main", repo },
  user: { login: "maintainer" }, merged_at: new Date().toISOString(), merge_commit_sha: ws.head });
export async function sourceRun(ws, api, pr, id, baseScope = "refs/heads/main", clearCache = true) {
  if (clearCache) rmSync(join(ws.dir, ".nx"), { recursive: true, force: true });
  const active = await sidecar(ws, api, { pull_request: pr }, ci.jobs["test-linux"].steps, { runId: id, baseScope });
  try {
    const task = await command(ws.dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", "run", "pool-maintenance:test", "--outputStyle=static"], active.env);
    assert.equal(task.code, 0, task.stdout + task.stderr + active.log());
    const report = ci.jobs["test-linux"].steps.find((s) => s.run?.includes("nx-remote-cache.mjs report"));
    const done = await command(ws.dir, "bash", ["-eo", "pipefail", "-c", report.run], active.env);
    assert.equal(done.code, 0, done.stderr);
    return { report: join(active.temp, "nx-cache-report"), temp: active.temp, env: active.env, output: task.stdout };
  } finally { await active.close(); }
}
export async function releaseJob(ws, api, scope = "refs/heads/main") {
  const event = { ref: scope, repository: { ...repo, default_branch: "main" }, head_commit: { message: "fix: cache journey" } };
  const steps = cd.jobs["release-plz"].steps;
  const active = await sidecar(ws, api, event, steps, { runId: 100, eventName: "push", scope,
    beforeTrust: async ({ env, temp, context, render }) => {
      context.env = cd.env;
      context.secrets = { RELEASE_PLZ_TOKEN: "fixture-token" };
      for (const step of steps.slice(0, steps.findIndex((s) => s.id === "nx-cache-trust"))) {
        if (step.if && !evaluate(String(step.if), context)) continue;
        if (!step.run) continue;
        const stepEnv = { ...env, GITHUB_OUTPUT: join(temp, "guard-output") };
        for (const [key, value] of Object.entries(step.env ?? {})) stepEnv[key] = render(value);
        const result = await command(ws.dir, "bash", ["-eo", "pipefail", "-c", step.run], stepEnv);
        assert.equal(result.code, 0, result.stderr);
        if (step.id) context.steps[step.id] = { outputs: outputs(stepEnv.GITHUB_OUTPUT) };
      }
    },
  });
  const bin = join(active.temp, "bin"); mkdirSync(bin);
  writeFileSync(join(bin, "release-plz"), '#!/usr/bin/env bash\nprintf "%s\\n" "$*" >> "$RELEASE_CALLS"\n', { mode: 0o755 });
  writeFileSync(join(bin, "cargo-semver-checks"), '#!/usr/bin/env bash\nexit 0\n', { mode: 0o755 });
  active.env.PATH = `${bin}${delimiter}${process.env.PATH}`;
  active.env.RELEASE_CALLS = join(active.temp, "release-calls");
  active.env.GITHUB_GRAPHQL_URL = `${api.url}graphql`;
  active.context.env = cd.env;
  active.context.secrets = { RELEASE_PLZ_TOKEN: "fixture-token" };
  api.metadata.set(`/repos/${repo.full_name}/pulls`, []);
  const ready = steps.findIndex((s) => s.run?.includes("nx-remote-cache.mjs ready"));
  try {
    for (const step of steps.slice(ready + 1)) {
      if (step.if && !evaluate(String(step.if), active.context)) continue;
      if (step.cancel) { await active.close(); continue; }
      if (!step.run) continue;
      const env = { ...active.env };
      for (const [key, value] of Object.entries(step.env ?? {})) env[key] = active.render(value);
      env.GITHUB_OUTPUT = join(active.temp, `output-${steps.indexOf(step)}`);
      const done = await command(ws.dir, "bash", ["-eo", "pipefail", "-c", step.run], env);
      assert.equal(done.code, 0, `${step.name}: ${done.stdout}${done.stderr}`);
      if (step.id) active.context.steps[step.id] = { outputs: outputs(env.GITHUB_OUTPUT) };
    }
    return active;
  } finally { await active.close(); }
}

