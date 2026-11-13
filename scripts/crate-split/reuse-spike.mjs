#!/usr/bin/env node
// The reuse spike's one measurement, from fetched evidence to every option's replay.
//
//   node scripts/crate-split/reuse-spike.mjs --evidence DIR --clone DIR --out DIR
//        [--commit REF] [--build-delta-b linux=M,darwin=M,win32=M] [--build-delta-c ...]
//
// REF (default `refs/spike/main`) is the commit today's tiers are read at.
//
// It runs, in order, writing each result under --out and printing each step's own line:
//   1. the observed replay (`crate-split-replay.mjs --observed`): observed.json, .md;
//   2. today's tiers (`tier-mapping.mjs`) and the replay of them over the same pull
//      requests, the non-release ones and the release ones apart;
//   3. option (a): each of tier-mapping's VARIANTS alone, and all of them together;
//   4. option (b): today's tiers with the e2e tier split as proposed.json maps;
//   5. option (c): today's tiers over the engine's crates along its module graph
//      (`module-graph.mjs`);
//   6. option (d): the install proofs keyed on what they build (option-d-install-proofs.json),
//      and the documented install keyed on the commit it proves (`sameBase`, an estimate);
// each replayed with `--changes observed.json`, weighed per platform, and (b) and (c)
// charged the build delta a hosted demonstration measured, when one is given.
// It reads nothing from GitHub: `fetch-evidence.mjs` is what fills --evidence.
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { sameBase } from "./observed.mjs";
import { VARIANTS } from "./tier-mapping.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..");

function step(args) {
  const run = spawnSync(process.execPath, args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  process.stdout.write(run.stdout);
  if (run.status !== 0) {
    process.stderr.write(run.stderr);
    throw new Error(`${args.map((a) => a.replace(root, ".")).join(" ")} exited ${run.status}`);
  }
}

function main(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i += 2) {
    const name = argv[i]?.replace(/^--/, "");
    if (
      !["evidence", "clone", "out", "commit", "build-delta-b", "build-delta-c"].includes(name) ||
      !argv[i + 1]
    ) {
      throw new Error(`${argv[i]} is not an option this reads`);
    }
    options[name] = argv[i + 1];
  }
  for (const required of ["evidence", "clone", "out"]) {
    if (!options[required]) throw new Error(`--${required} is required`);
  }
  const out = options.out;
  mkdirSync(out, { recursive: true });
  const replay = join(root, "scripts", "crate-split-replay.mjs");
  const tiers = join(here, "tier-mapping.mjs");
  const observed = join(out, "observed.json");
  step([
    replay,
    "--observed",
    options.evidence,
    "--clone",
    options.clone,
    "--out",
    observed,
    "--json",
  ]);
  step([
    replay,
    "--observed",
    options.evidence,
    "--clone",
    options.clone,
    "--out",
    join(out, "observed.md"),
  ]);
  const base = [
    "--clone",
    options.clone,
    "--changes",
    observed,
    "--commit",
    options.commit ?? "refs/spike/main",
  ];
  const mapped = (name, extra) => {
    const file = join(out, `${name}.mapping.json`);
    step([tiers, ...base, "--out", file, ...extra]);
    return file;
  };
  const replayed = (name, mapping, extra = []) => {
    step([
      replay,
      "--mapping",
      mapping,
      "--changes",
      observed,
      "--out",
      join(out, `${name}.json`),
      "--json",
      ...extra,
    ]);
    step([
      replay,
      "--mapping",
      mapping,
      "--changes",
      observed,
      "--out",
      join(out, `${name}.md`),
      ...extra,
    ]);
  };
  const today = mapped("today", []);
  replayed("today", today);
  replayed("today-release", today, ["--release"]);
  for (const variant of Object.keys(VARIANTS))
    replayed(`a-${variant}`, mapped(`a-${variant}`, ["--variant", variant]));
  replayed(
    "a-all",
    mapped(
      "a-all",
      Object.keys(VARIANTS).flatMap((v) => ["--variant", v]),
    ),
  );
  const delta = (name) => (options[name] ? ["--build-delta", options[name]] : []);
  replayed(
    "b",
    mapped("b", ["--split-e2e", join(root, "scripts", "crate-split", "proposed.json")]),
    delta("build-delta-b"),
  );
  const c = join(out, "c.mapping.json");
  step([
    join(here, "module-graph.mjs"),
    "--repo",
    root,
    "--out",
    join(out, "module-graph.json"),
    "--mapping",
    c,
    "--base",
    today,
  ]);
  replayed("c", c, delta("build-delta-c"));
  replayed("d-install-proofs", join(here, "option-d-install-proofs.json"));
  const result = JSON.parse(readFileSync(observed, "utf8"));
  const d2 = sameBase(result, "onepipeline:install-documented");
  writeFileSync(join(out, "d-install-documented.json"), `${JSON.stringify(d2, null, 2)}\n`);
  console.log(
    `reuse-spike: d-install-documented (estimate, keyed on the run's base): ${[
      "linux",
      "darwin",
      "win32",
    ]
      .map(
        (p) =>
          `${p} ${d2[p].replays} of ${d2[p].runs} runs replay, ${d2[p].minutesPerChange?.toFixed(1) ?? "-"} min per change`,
      )
      .join("; ")}`,
  );
}

try {
  main(process.argv.slice(2));
} catch (error) {
  console.error(`reuse-spike: ${error.message.split("\n")[0]}`);
  process.exitCode = 1;
}
