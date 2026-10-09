// The producer and the reader hash one checked input manifest, without git or a build.
import { createHash } from "node:crypto";
import { lstatSync, readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

export const repositoryRoot = fileURLToPath(new URL("../", import.meta.url));
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");

export function inputFiles(root = repositoryRoot) {
  const manifest = JSON.parse(
    readFileSync(`${root}/scripts/stop-guard-unpublished-build-inputs.json`, "utf8"),
  );
  if (manifest?.version !== 1) throw new Error("unsupported build-input manifest version");
  const unknown = Object.keys(manifest).filter(
    (key) => !["version", "files", "directories", "prefixes"].includes(key),
  );
  if (unknown.length > 0) throw new Error(`build-input manifest names unknown fields ${unknown}`);
  // Every path is repository-relative before it is joined onto the root: the rule the
  // producer's own reader of this file applies.
  const relative = (field, path) => {
    if (
      typeof path !== "string" ||
      path === "" ||
      path.startsWith("/") ||
      path.includes("\\") ||
      path.split("/").some((part) => part === "" || part === "." || part === "..")
    ) {
      throw new Error(
        `build-input manifest ${field} entry ${JSON.stringify(path)} is not a repository-relative path`,
      );
    }
    return path;
  };
  const list = (field) => {
    if (!Array.isArray(manifest[field]))
      throw new Error(`build-input manifest ${field} is not a list`);
    return manifest[field];
  };
  const files = new Set(list("files").map((path) => relative("files", path)));
  const directories = list("directories").map((path) => relative("directories", path));
  const prefixes = list("prefixes").map((entry) => {
    const keys = entry && typeof entry === "object" ? Object.keys(entry).sort().join(",") : "";
    if (
      keys !== "directory,prefix" ||
      typeof entry.prefix !== "string" ||
      entry.prefix === "" ||
      entry.prefix.includes("/")
    ) {
      throw new Error(
        `build-input manifest prefixes entry ${JSON.stringify(entry)} needs exactly a directory and a non-empty prefix without '/'`,
      );
    }
    return { directory: relative("prefixes", entry.directory), prefix: entry.prefix };
  });
  const walk = (path) => {
    const stat = lstatSync(`${root}/${path}`);
    if (stat.isDirectory()) {
      for (const entry of readdirSync(`${root}/${path}`)) walk(`${path}/${entry}`);
    } else if (stat.isFile()) {
      files.add(path);
    } else {
      throw new Error(`unsupported build input ${path}`);
    }
  };
  for (const directory of directories) walk(directory);
  for (const { directory, prefix } of prefixes) {
    for (const name of readdirSync(`${root}/${directory}`)) {
      if (name.startsWith(prefix)) walk(`${directory}/${name}`);
    }
  }
  // Sorted by UTF-8 bytes, as the producer's ordered set sorts them.
  return [...files].sort((a, b) => Buffer.compare(Buffer.from(a), Buffer.from(b)));
}

export function sourceFingerprint(root = repositoryRoot) {
  return sha(
    JSON.stringify(inputFiles(root).map((path) => [path, sha(readFileSync(`${root}/${path}`))])),
  );
}
