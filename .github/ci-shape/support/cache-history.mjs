import { execFileSync } from "node:child_process";
import { readFileSync, statSync } from "node:fs";
import { join } from "node:path";

export function zip(directory, name = "artifact.zip") {
  const path = join(directory, "..", name);
  execFileSync("zip", ["-qr", path, "."], { cwd: directory });
  return readFileSync(path);
}
export function sourceMetadata(api, ws, report, { id, pr, attempt = 1, expired = false }) {
  const repository = pr.base.repo;
  const repo = repository.full_name;
  const execution = JSON.parse(readFileSync(join(report, "executions.json")));
  const run = { id, run_attempt: attempt, head_sha: pr.head.sha, head_branch: pr.head.ref, event: "pull_request", head_repository: repository,
    pull_requests: [structuredClone(pr)], status: "completed", conclusion: "success", actor: pr.user,
    run_started_at: execution.tasks.map((t) => t.started_at).sort()[0], updated_at: new Date().toISOString() };
  const artifact = { id: id * 100 + attempt, name: "nx-cache-entries-test-linux", expired, created_at: statSync(join(report, "executions.json")).mtime.toISOString(),
    workflow_run: { id, head_sha: pr.head.sha, head_repository_id: repository.id } };
  run.updated_at = artifact.created_at;
  api.metadata.set(`/repos/${repo}/commits/${pr.merge_commit_sha}/pulls`, [pr]);
  api.metadata.set(`/repos/${repo}/pulls/${pr.number}`, pr);
  const listing = api.metadata.get(`/repos/${repo}/actions/workflows/ci.yml/runs`) ?? { workflow_runs: [] };
  listing.workflow_runs.push(run);
  api.metadata.set(`/repos/${repo}/actions/workflows/ci.yml/runs`, listing);
  api.metadata.set(`/repos/${repo}/actions/runs/${id}/attempts/${attempt}`, run);
  api.metadata.set(`/repos/${repo}/actions/runs/${id}/attempts/${attempt}/jobs`, { jobs: [{ name: "test-linux", conclusion: "success", labels: ["ubuntu-latest"] }] });
  api.metadata.set(`/repos/${repo}/actions/runs/${id}/artifacts`, { artifacts: [artifact] });
  api.metadata.set(`/repos/${repo}/actions/artifacts/${artifact.id}/zip`, zip(report, `run-${id}-${attempt}.zip`));
  return { run, artifact };
}
