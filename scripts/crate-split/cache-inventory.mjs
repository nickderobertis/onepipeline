#!/usr/bin/env node
// What a repository's Actions cache holds, by family and by Nx target, read-only.
//
//   node scripts/crate-split/cache-inventory.mjs --evidence DIR [--repo OWNER/NAME]
//        [--floor N] [--wait]
//
// It pages `GET /repos/{repo}/actions/caches` (100 entries a page, newest first) into
// DIR/caches/page-<n>.json, reading the core allowance before each page as
// `fetch-evidence.mjs` does (stop below --floor, default 2500, or with --wait pause
// until it resets), and resumes from the pages it holds. It then writes
// DIR/caches/inventory.json and prints one line: entries and bytes, by family — the key
// with every run of hex digits of 8 or more folded to `#`, and its scope (`main`, a
// pull request, or another ref) — and, for each family whose keys carry an Nx task
// hash, by the Nx task that hash belongs to in the evidence's executions records. It
// writes and deletes nothing in the cache.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

function gh(args) {
  return execFileSync("gh", args, { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
}

/** A key's family: hashes folded, so every entry of one cache shape counts together. */
export const family = (key) => key.replace(/[0-9a-f]{8,}/gi, "#").replace(/\d{6,}/g, "#");

/** A ref's scope: main, a pull request, or the ref itself. */
export const scope = (ref) =>
  ref === "refs/heads/main" ? "main" : /^refs\/pull\/\d+\/merge$/.test(ref) ? "pull request" : ref;

export function inventory(entries, hashes) {
  const families = {};
  for (const e of entries) {
    const f = `${family(e.key)} @ ${scope(e.ref)}`;
    families[f] ??= {
      entries: 0,
      bytes: 0,
      oldest: e.created_at,
      newest: e.created_at,
      targets: {},
    };
    const t = families[f];
    t.entries++;
    t.bytes += e.size_in_bytes;
    if (e.created_at < t.oldest) t.oldest = e.created_at;
    if (e.created_at > t.newest) t.newest = e.created_at;
    const hash = e.key.match(/\d{12,}|[0-9a-f]{16,}/i)?.[0];
    const task = hash && hashes.get(hash);
    if (task) {
      t.targets[task] ??= { entries: 0, bytes: 0 };
      t.targets[task].entries++;
      t.targets[task].bytes += e.size_in_bytes;
    }
  }
  return {
    entries: entries.length,
    bytes: entries.reduce((s, e) => s + e.size_in_bytes, 0),
    families,
  };
}

function main(argv) {
  const options = { repo: "nickderobertis/onepipeline", floor: "2500" };
  for (let i = 0; i < argv.length; i += 2) {
    const name = argv[i]?.replace(/^--/, "");
    if (name === "wait") {
      options.wait = true;
      i--;
      continue;
    }
    if (!["evidence", "repo", "floor"].includes(name) || argv[i + 1] === undefined) {
      throw new Error(`${argv[i]} is not an option this reads`);
    }
    options[name] = argv[i + 1];
  }
  if (!options.evidence) throw new Error("--evidence names no directory");
  const dir = join(options.evidence, "caches");
  mkdirSync(dir, { recursive: true });
  const floor = Number(options.floor);
  let spent = 0;
  for (let page = 1; ; page++) {
    const path = join(dir, `page-${page}.json`);
    if (!existsSync(path)) {
      for (;;) {
        const { remaining, reset } = JSON.parse(
          gh(["api", "rate_limit", "--jq", ".resources.core | {remaining, reset}"]),
        );
        if (remaining >= floor) break;
        if (!options.wait) {
          console.error(
            `cache-inventory: ${remaining} core remain, below the floor of ${floor}; ${page - 1} pages held`,
          );
          console.error("next: re-run once the allowance resets, or add --wait; it resumes");
          process.exitCode = 2;
          return;
        }
        Atomics.wait(
          new Int32Array(new SharedArrayBuffer(4)),
          0,
          0,
          Math.max(5, reset * 1000 - Date.now() + 30000),
        );
      }
      writeFileSync(
        path,
        gh([
          "api",
          `repos/${options.repo}/actions/caches?per_page=100&page=${page}&sort=created_at&direction=desc`,
        ]),
      );
      spent++;
    }
    if (JSON.parse(readFileSync(path, "utf8")).actions_caches.length < 100) break;
  }
  // The listing can shift while it is paged; an entry seen twice counts once.
  const byId = new Map();
  for (const f of readdirSync(dir).filter((f) => /^page-\d+\.json$/.test(f))) {
    for (const e of JSON.parse(readFileSync(join(dir, f), "utf8")).actions_caches)
      byId.set(e.id, e);
  }
  const entries = [...byId.values()];
  const hashes = new Map();
  const executions = join(options.evidence, "executions");
  for (const f of existsSync(executions) ? readdirSync(executions) : []) {
    const doc = JSON.parse(readFileSync(join(executions, f), "utf8"));
    for (const t of doc.tasks ?? []) hashes.set(t.hash, t.task);
  }
  const result = inventory(entries, hashes);
  writeFileSync(join(dir, "inventory.json"), `${JSON.stringify(result, null, 2)}\n`);
  const top = Object.entries(result.families)
    .sort((a, b) => b[1].entries - a[1].entries)
    .slice(0, 6)
    .map(([f, t]) => `${f} ${t.entries} (${(t.bytes / 1e9).toFixed(2)} GB)`);
  console.log(
    `cache-inventory: ${result.entries} entries, ${(result.bytes / 1e9).toFixed(2)} GB, ${spent} pages read; largest families: ${top.join("; ")}`,
  );
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`cache-inventory: ${error.message.split("\n")[0]}`);
    process.exitCode = 1;
  }
}
