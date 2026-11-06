// Nx runtime input of github-workflows:test-ci-shape: one digest of the path and
// bytes of every tracked file, so a replayed proof is bound to the whole workspace
// its hash and test-selection journeys read.
//
// It is a runtime input rather than `{workspaceRoot}/**/*` because Nx's affected
// selection treats a target's `{workspaceRoot}/` file input as "this project is
// touched when that file changes", which would select the proof for every change
// in the repository. Files the version-free inputs already answer for (manifests,
// lockfiles) and changelogs are left out, so a release's version edit moves nothing.
//
// Nx hashes a failed runtime command's output as an ordinary input, so a failure
// prints a value unique to this attempt on both streams: it cannot be mistaken for
// a digest, and no two attempts, on this tree or another, can share a replayable key.
import { execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { lstatSync, readFileSync, readlinkSync } from "node:fs";
import { basename } from "node:path";

const VERSION_FREE_ELSEWHERE = new Set([
  "Cargo.toml",
  "Cargo.lock",
  "package.json",
  "package-lock.json",
  "CHANGELOG.md",
]);

function isHashed(path) {
  if (path === "pyproject.toml" || VERSION_FREE_ELSEWHERE.has(basename(path))) return false;
  return !path.split("/").includes("node_modules");
}

// The kind and the length frame each entry, so a deleted file, a link and a file
// that merely holds such bytes can never hash alike.
function entryOf(path) {
  let stat;
  try {
    stat = lstatSync(path);
  } catch (error) {
    if (error.code === "ENOENT") return ["deleted", ""];
    throw error;
  }
  if (stat.isSymbolicLink()) return ["link", readlinkSync(path)];
  return stat.isFile() ? ["file", readFileSync(path)] : ["other", ""];
}

function workspaceDigest() {
  const digest = createHash("sha256");
  const tracked = execFileSync("git", ["ls-files", "-z"], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  });
  for (const path of tracked.split("\0").filter(isHashed).filter(Boolean).sort()) {
    const [kind, body] = entryOf(path);
    digest.update(`${path}\0${kind}\0${Buffer.byteLength(body)}\0`).update(body);
  }
  return digest.digest("hex");
}

try {
  console.log(workspaceDigest());
} catch (error) {
  const unreplayable = `workspace-proof-input-failed:${randomUUID()}`;
  console.log(unreplayable);
  console.error(`${unreplayable}: ${error.message}`);
  console.error("ACTION: run this from a git checkout whose tracked files are all readable");
  process.exitCode = 1;
}
