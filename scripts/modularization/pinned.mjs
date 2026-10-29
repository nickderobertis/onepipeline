#!/usr/bin/env node
// Runs a design command against the source commit the design was reviewed at.
//
// The design's evidence (module graph, test catalog, hash-pinned attributions, replay) describes
// one reviewed tree. Reading the live src/ and tests/ instead would make every later change to
// them refuse this project's checks, so the command runs in a scratch clone whose HEAD is that
// commit with the design's own files, as they stand in this working tree, committed on top.
import { execFileSync, spawnSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const SNAPSHOT = "scripts/modularization/snapshot.json";
const DEFAULT_COMMAND = ["node", "--test", "scripts/modularization/design.test.mjs"];

/** The reviewed commit and the repository-relative paths the design owns. */
export function readSnapshot(root) {
  const snapshot = JSON.parse(readFileSync(join(root, SNAPSHOT), "utf8"));
  if (
    !snapshot ||
    typeof snapshot !== "object" ||
    Array.isArray(snapshot) ||
    Object.keys(snapshot).some((key) => !["commit", "design"].includes(key)) ||
    typeof snapshot.commit !== "string" ||
    !/^[0-9a-f]{40}$/.test(snapshot.commit) ||
    !Array.isArray(snapshot.design) ||
    !snapshot.design.length ||
    snapshot.design.some(
      (path) =>
        typeof path !== "string" ||
        !/^[A-Za-z0-9._-]+(?:\/[A-Za-z0-9._-]+)*$/.test(path) ||
        path.split("/").some((part) => part === "." || part === ".."),
    )
  )
    throw new Error(`invalid design snapshot ${SNAPSHOT}`);
  return snapshot;
}

/** Tracked and untracked-but-not-ignored files under the design paths, as the working tree has them. */
function designFiles(root, design) {
  return execFileSync(
    "git",
    ["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", ...design],
    { cwd: root, encoding: "utf8" },
  )
    .split("\0")
    .filter((path) => path && existsSync(join(root, path)));
}

/** Builds the pinned clone in `into` and returns it; the caller removes it. */
export function pin(root, into) {
  const { commit, design } = readSnapshot(root);
  const git = (cwd, ...args) =>
    execFileSync(
      "git",
      ["-c", "user.name=modularization", "-c", "user.email=modularization@localhost", ...args],
      { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
    ).trim();
  try {
    git(root, "cat-file", "-e", `${commit}^{commit}`);
  } catch {
    throw new Error(
      `design snapshot commit ${commit} is not in this repository (read from ${SNAPSHOT})`,
    );
  }
  const files = designFiles(root, design);
  git(root, "clone", "--quiet", "--shared", "--no-checkout", root, into);
  git(into, "checkout", "--quiet", "--detach", commit);
  for (const path of design) rmSync(join(into, path), { recursive: true, force: true });
  for (const path of files) {
    mkdirSync(dirname(join(into, path)), { recursive: true });
    cpSync(join(root, path), join(into, path));
  }
  git(into, "add", "-A", "--", ...design);
  git(into, "commit", "--quiet", "--allow-empty", "-m", "design files over the reviewed source");
  // The replay reads the merged-PR history from origin/main, so the clone carries this checkout's.
  git(into, "update-ref", "refs/remotes/origin/main", git(root, "rev-parse", "origin/main"));
  if (existsSync(join(root, "node_modules")))
    symlinkSync(join(root, "node_modules"), join(into, "node_modules"), "junction");
  return { commit, design, files };
}

/** Copies the clone's design files, as a regenerating command left them, back to the working tree. */
function writeBack(root, into, design) {
  for (const path of designFiles(into, design)) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    cpSync(join(into, path), join(root, path));
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const root = resolve(fileURLToPath(new URL("../../", import.meta.url)));
  let args = process.argv.slice(2);
  const back = args[0] === "--write-back";
  if (back) args = args.slice(1);
  if (args.length && args[0] !== "--") {
    console.error(
      "usage: pinned.mjs [--write-back] [-- COMMAND ARGS...]\nnext: put the command to run in the pinned clone after --",
    );
    process.exit(2);
  }
  const command = args.length > 1 ? args.slice(1) : DEFAULT_COMMAND;
  const into = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "pinned-"));
  let status = 1;
  try {
    const { commit } = pin(root, into);
    console.error(`pinned: running in ${into} (reviewed source ${commit.slice(0, 12)})`);
    const run = spawnSync(command[0], command.slice(1), { cwd: into, stdio: "inherit" });
    if (run.error) throw run.error;
    status = run.status ?? 1;
    if (back && status === 0) writeBack(root, into, readSnapshot(root).design);
  } catch (error) {
    console.error(
      `${error.message}\nnext: fetch the snapshot commit or correct ${SNAPSHOT}; rerun the same pinned.mjs invocation`,
    );
    status = 1;
  } finally {
    rmSync(into, { recursive: true, force: true });
  }
  process.exit(status);
}
