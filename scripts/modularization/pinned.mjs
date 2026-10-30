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

/** Builds the pinned clone in `into`, returning the snapshot it pinned and the design files it copied; the caller removes the clone. */
export function pin(root, into) {
  const { commit, design } = readSnapshot(root);
  const git = (cwd, ...args) =>
    execFileSync(
      "git",
      ["-c", "user.name=modularization", "-c", "user.email=modularization@localhost", ...args],
      { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
    ).trim();
  try {
    git(root, "rev-parse", "--verify", "--quiet", `${commit}^{commit}`);
  } catch (error) {
    // `rev-parse --verify --quiet` exits 1, saying nothing, exactly when no such commit exists;
    // anything else is git itself failing, and keeps its own cause.
    if (error.status === 1 && !String(error.stderr ?? "").trim())
      throw new Error(
        `design snapshot commit ${commit} is not in this repository (read from ${SNAPSHOT})`,
      );
    const failure = new Error(
      `git could not read ${root}: ${String(error.stderr ?? "").trim() || error.message}`,
    );
    failure.git = true;
    throw failure;
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
  const parent = process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir();
  let into;
  let stage = "scratch";
  let status = 1;
  try {
    into = mkdtempSync(join(parent, "pinned-"));
    stage = "pin";
    const { commit } = pin(root, into);
    console.error(`pinned: running in ${into} (reviewed source ${commit.slice(0, 12)})`);
    stage = "command";
    const run = spawnSync(command[0], command.slice(1), { cwd: into, stdio: "inherit" });
    if (run.error) throw run.error;
    if (run.signal) {
      console.error(
        `pinned: ${command.join(" ")} ended on signal ${run.signal} before it exited\nnext: rerun it if the signal was an interruption; otherwise check what stopped it (memory, a timeout or a kill) before rerunning the same pinned.mjs invocation`,
      );
      status = 1;
    } else status = run.status;
    stage = "write-back";
    if (back && status === 0) writeBack(root, into, readSnapshot(root).design);
  } catch (error) {
    const next = {
      scratch: `make ${parent} an existing, writable directory (or point ONEPIPELINE_NODE_SCRATCH_DIR at one)`,
      pin:
        error.code === "ENOENT" && error.path === "git"
          ? "install git and put it on PATH"
          : error.git
            ? `check that git is installed and ${root} is a readable git repository`
            : error.code
              ? `check that ${root} and ${parent} are readable and writable and the disk has space`
              : `fetch the snapshot commit or correct ${SNAPSHOT}`,
      command:
        error.code === "ENOENT"
          ? `install ${command[0]} or correct the command given after --`
          : `check that ${command[0]} can be executed`,
      "write-back": `check that the design paths under ${root} are writable; the clone's output was not copied back`,
    }[stage];
    console.error(`${error.message}\nnext: ${next}; rerun the same pinned.mjs invocation`);
    status = 1;
  } finally {
    if (into) rmSync(into, { recursive: true, force: true });
  }
  process.exit(status);
}
