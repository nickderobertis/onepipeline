import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { gunzipSync } from "node:zlib";

const directory = new URL("../fixtures/github-cache-api/", import.meta.url);
const provenance = JSON.parse(readFileSync(new URL("provenance.json", directory)));
const responses = new Map(provenance.recordings.map((record) => {
  const bytes = gunzipSync(readFileSync(new URL(record.path, directory)));
  if (createHash("sha256").update(bytes).digest("hex") !== record.sha256) throw new Error(`GitHub recording changed: ${record.path}`);
  return [record.path, JSON.parse(bytes)];
}));

// Test-only identity variants of the unmodified, provenance-bearing responses.
// All other fields retain GitHub's recorded shape, including compact run repos.
export function associationFor(pr) {
  const association = structuredClone(responses.get("run.json.gz").pull_requests[0]);
  association.number = pr.number;
  for (const side of ["head", "base"]) {
    association[side].sha = pr[side].sha;
    association[side].ref = pr[side].ref;
    const repository = pr[side].repo;
    Object.assign(association[side].repo, { id: repository.id, name: repository.full_name.split("/")[1], url: `https://api.github.com/repos/${repository.full_name}` });
  }
  return association;
}
export function membershipFor(pr) {
  const member = structuredClone(responses.get("membership.json.gz")[0]);
  Object.assign(member, pr, {
    head: { ...member.head, ...pr.head, repo: { ...member.head.repo, ...pr.head.repo } },
    base: { ...member.base, ...pr.base, repo: { ...member.base.repo, ...pr.base.repo } },
    user: { ...member.user, ...pr.user },
  });
  return [member];
}
export function comparisonFor(identical) {
  return structuredClone(responses.get(identical ? "comparison-identical.json.gz" : "comparison-ahead.json.gz"));
}
