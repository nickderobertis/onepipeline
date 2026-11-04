import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";
import { evaluate } from "../../workflow-graph.mjs";
import { OP_NX_COMMIT } from "../../../scripts/nx-remote-cache.mjs";

export const root = fileURLToPath(new URL("../../../", import.meta.url));
export function command(cwd, cmd, args, env) {
  return new Promise((resolve, reject) => {
    // Fixture Nx runs must not inherit the enclosing CI job's sidecar or
    // telemetry identity. Cache journeys supply their own explicit context.
    const inherited = { ...process.env };
    for (const key of Object.keys(inherited)) if (key.startsWith("GITHUB_") || key.startsWith("ACTIONS_") || key.startsWith("NX_SELF_HOSTED_REMOTE_CACHE_") || key === "NX_BASE" || key === "NX_HEAD") delete inherited[key];
    const child = spawn(cmd, args, { cwd, env: { ...inherited, ...env }, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "", stderr = "";
    child.stdout.on("data", (data) => { stdout += data; });
    child.stderr.on("data", (data) => { stderr += data; });
    child.on("error", reject);
    child.on("close", (code) => resolve({ code, stdout, stderr }));
  });
}
export function workspace(revision) {
  const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "cache-journey-"));
  if (revision) {
    const archive = execFileSync("git", ["archive", revision], { cwd: root, maxBuffer: 128 * 1024 * 1024 });
    execFileSync("tar", ["-xf", "-", "-C", dir], { input: archive });
  } else for (const path of execFileSync("git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z"], { cwd: root, encoding: "utf8" }).split("\0").filter(Boolean)) {
    if (!existsSync(join(root, path))) continue;
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    cpSync(join(root, path), join(dir, path), { dereference: false });
  }
  symlinkSync(resolve(root, "node_modules"), join(dir, "node_modules"), "junction");
  const git = (...args) => execFileSync("git", args, { cwd: dir, encoding: "utf8" }).trim();
  git("init", "-q"); git("add", "-A");
  git("-c", "user.name=Cache journey", "-c", "user.email=cache@example.invalid", "commit", "-qm", "fixture base");
  const base = git("rev-parse", "HEAD");
  writeFileSync(join(dir, "pool-maintenance/README.md"), "cache journey input\n");
  writeFileSync(join(dir, "README.md"), readFileSync(join(dir, "README.md"), "utf8") + "\nCache fixture change.\n");
  git("add", "-A");
  git("-c", "user.name=Cache journey", "-c", "user.email=cache@example.invalid", "commit", "-qm", "fixture head");
  return { dir, base, head: git("rev-parse", "HEAD"), git, close: () => rmSync(dir, { recursive: true, force: true }) };
}
export const outputs = (path) => existsSync(path) ? Object.fromEntries(readFileSync(path, "utf8").split("\n").filter((l) => l.includes("=")).map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)])) : {};
export async function sidecar(ws, api, event, steps, { runId = 1, attempt = 1, scope = `refs/pull/${event.pull_request.number}/merge`, eventName = "pull_request", baseScope = "refs/heads/main", beforeTrust } = {}) {
  const temp = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "cache-run-"));
  const payload = join(temp, "event.json"); writeFileSync(payload, JSON.stringify(event));
  const repo = event.pull_request?.base.repo.full_name ?? event.repository.full_name;
  const context = { github: { repository: repo, token: "fixture-token", workspace: ws.dir, event_name: eventName, ref: scope, event }, steps: {} };
  const env = {
    GITHUB_ACTIONS: "true", GITHUB_EVENT_NAME: eventName, GITHUB_SERVER_URL: "https://github.com", GITHUB_REPOSITORY: repo,
    GITHUB_REF: scope, GITHUB_REF_NAME: scope.replace(/^refs\/heads\//, ""), GITHUB_SHA: ws.head, GITHUB_EVENT_PATH: payload,
    GITHUB_JOB: "test-linux", GITHUB_RUN_ID: String(runId), GITHUB_RUN_ATTEMPT: String(attempt), GITHUB_WORKSPACE: ws.dir,
    GITHUB_TOKEN: "fixture-token", GH_TOKEN: "fixture-token", GITHUB_API_URL: api.url.replace(/\/$/, ""), ACTIONS_RUNTIME_TOKEN: scope,
    ACTIONS_CACHE_SERVICE_V2: "true", ACTIONS_RESULTS_URL: api.url, RUNNER_TEMP: temp, GITHUB_ENV: join(temp, "env"),
    GITHUB_OUTPUT: join(temp, "outputs"), GITHUB_STEP_SUMMARY: join(temp, "summary"), NX_DAEMON: "false", NX_ISOLATE_PLUGINS: "false",
    NX_BASE: event.pull_request?.base.sha ?? "", NX_HEAD: event.pull_request?.head.sha ?? "",
  };
  const render = (text) => String(text).replace(/\$\{\{(.*?)\}\}/g, (_, expression) => String(evaluate(expression, context) ?? ""));
  if (beforeTrust) await beforeTrust({ env, temp, context, render });
  const trust = steps.find((s) => s.id === "nx-cache-trust");
  const decided = await command(ws.dir, "bash", ["-eo", "pipefail", "-c", trust.run], env);
  assert.equal(decided.code, 0, decided.stderr);
  context.steps[trust.id] = { outputs: outputs(env.GITHUB_OUTPUT) };
  const server = steps.find((s) => s.id === "nx-cache");
  assert.equal(server.uses, `op-nx/github-cache/start-cache-server@${OP_NX_COMMIT}`);
  for (const [key, value] of Object.entries(server.env)) env[key] = render(value);
  // Rebind only the service address, so this journey can run inside a job
  // already using the workflow's port 3000. Trust and read-only stay unmodified.
  const socket = createServer(); await new Promise((resolve) => socket.listen(0, "127.0.0.1", resolve));
  env.INPUT_PORT = String(socket.address().port); await new Promise((resolve) => socket.close(resolve));
  env.NX_SELF_HOSTED_REMOTE_CACHE_SERVER = `http://127.0.0.1:${env.INPUT_PORT}`;
  api.bases.set(scope, baseScope);
  const fixture = new URL("../fixtures/", import.meta.url);
  const manifest = JSON.parse(readFileSync(new URL("op-nx-server.json", fixture)));
  const bundle = gunzipSync(readFileSync(new URL("op-nx-server.cjs.gz", fixture)));
  assert.equal(manifest.commit, OP_NX_COMMIT);
  assert.equal(createHash("sha256").update(bundle).digest("hex"), manifest.sha256);
  const executable = join(temp, "op-nx-server.cjs"); writeFileSync(executable, bundle);
  const sidecarProcess = spawn(globalThis.process.execPath, [executable], { cwd: ws.dir, env: { ...globalThis.process.env, ...env }, stdio: ["ignore", "pipe", "pipe"] });
  let log = ""; sidecarProcess.stdout.on("data", (data) => { log += data; }); sidecarProcess.stderr.on("data", (data) => { log += data; });
  const closed = new Promise((resolve) => sidecarProcess.on("close", resolve));
  let stopped = false;
  const stop = async () => {
    if (stopped) return;
    stopped = true;
    // report is the shared proxy's public teardown, including for a push job
    // with no task executions. Keep the fixture's detached child inside its test.
    await command(ws.dir, globalThis.process.execPath, ["scripts/nx-remote-cache.mjs", "report", "--job", "test-linux", "--out", join(temp, "cleanup-report")], env);
    sidecarProcess.kill("SIGTERM"); await closed;
  };
  try {
    const ready = steps.find((s) => s.run?.includes("nx-remote-cache.mjs ready"));
    const started = await command(ws.dir, "bash", ["-eo", "pipefail", "-c", ready.run], env);
    assert.equal(started.code, 0, started.stderr);
    assert.ok(outputs(env.GITHUB_ENV).NX_SELF_HOSTED_REMOTE_CACHE_SERVER, started.stderr + log);
    Object.assign(env, outputs(env.GITHUB_ENV));
    assert.match(env.NX_SELF_HOSTED_REMOTE_CACHE_SERVER, /127\.0\.0\.1/);
    return { env, temp, context, trust: context.steps[trust.id].outputs, log: () => log, render,
      close: stop };
  } catch (error) { await stop(); throw error; }
}
