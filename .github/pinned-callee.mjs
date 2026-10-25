import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { parse } from "yaml";
function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object")
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, canonical(value[key])]),
    );
  return value;
}
export function pinnedCallee(reference, root) {
  const lock = JSON.parse(
    readFileSync(join(root, ".github/fixtures/visual-docs-reusable.origin.json"), "utf8"),
  );
  if (
    !lock ||
    typeof lock !== "object" ||
    Array.isArray(lock) ||
    lock.schema_version !== 1 ||
    lock.reference !== reference ||
    !/^[a-f0-9]{40}$/.test(lock.blob_sha) ||
    typeof lock.source_base64 !== "string" ||
    Object.keys(lock).sort().join() !==
      ["blob_sha", "reference", "schema_version", "source_base64"].join()
  )
    throw new Error("invalid pinned callee origin");
  const source = Buffer.from(lock.source_base64, "base64");
  if (source.toString("base64") !== lock.source_base64)
    throw new Error("invalid pinned callee encoding");
  const blob = createHash("sha1").update(`blob ${source.length}\0`).update(source).digest("hex");
  if (blob !== lock.blob_sha) throw new Error("pinned callee Git blob mismatch");
  const original = parse(source.toString("utf8"));
  const fixture = parse(
    readFileSync(join(root, ".github/fixtures/visual-docs-reusable.yml"), "utf8"),
  );
  if (JSON.stringify(canonical(original)) !== JSON.stringify(canonical(fixture)))
    throw new Error("callee fixture differs from locked upstream Git blob");
  return fixture;
}
