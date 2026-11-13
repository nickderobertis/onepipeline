#!/usr/bin/env node
// The reuse spike's one measurement, from fetched evidence to every option's replay.
//
//   node scripts/crate-split/reuse-spike.mjs --evidence DIR --clone DIR --out DIR
//        [--commit REF] [--build-delta-b linux=M,darwin=M,win32=M] [--build-delta-c ...]
//
// REF (default `refs/spike/main`) is the commit today's tiers are read at.
//
// It runs, in order, writing each result under --out and each step's own line to
// OUT/steps.txt:
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
//
// Exit 0: every step ran; one line gives the observed replay's headline and each
// option's net minutes per pull request on Linux / macOS / Windows over today's tiers.
// 1: an argument or a step was refused; the step's own reason and a `next:` action are
// on stderr.
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { sameBase } from "./observed.mjs";
import { VARIANTS } from "./tier-mapping.mjs";
import * as check from "./validate.mjs";

const USAGE =
  "node scripts/crate-split/reuse-spike.mjs --evidence DIR --clone DIR --out DIR [--commit REF] [--build-delta-b linux=M,darwin=M,win32=M] [--build-delta-c ...]";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..");

const steps = [];

function step(args) {
  const run = spawnSync(process.execPath, args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  if (run.error) {
    throw new check.Refusal(
      `${args[0].replace(root, ".")} could not start: ${run.error.message}`,
      "check the node on PATH and that the step's script exists",
    );
  }
  steps.push(...run.stdout.trim().split("\n").filter(Boolean));
  if (run.status !== 0) {
    process.stderr.write(run.stderr);
    throw new check.Refusal(
      `${args.map((a) => a.replace(root, ".")).join(" ")} exited ${run.status}`,
      "fix what that step reported above, then re-run the same command",
    );
  }
}

function main(argv) {
  const options = check.parseOptions(
    argv,
    ["evidence", "clone", "out", "commit", "build-delta-b", "build-delta-c"],
    { usage: USAGE },
  );
  for (const required of ["evidence", "clone", "out"])
    check.need(options[required], `--${required} is required`, USAGE);
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
  steps.push(
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
  writeFileSync(join(out, "steps.txt"), `${steps.join("\n")}\n`);
  console.log(summary(out, result, d2));
}

/** One line: the observed headline and each option's net minutes over today's tiers. */
function summary(out, observed, d2) {
  const net = (name) =>
    JSON.parse(readFileSync(join(out, `${name}.json`), "utf8")).summary.platforms;
  const today = net("today");
  const platforms = ["linux", "darwin", "win32"];
  const over = (name) => {
    const p = net(name);
    return platforms
      .map((x) => (p[x].netMinutesPerChange - today[x].netMinutesPerChange).toFixed(1))
      .join("/");
  };
  const own = (name) => platforms.map((x) => net(name)[x].netMinutesPerChange.toFixed(1)).join("/");
  const ran = observed.prs.flatMap((pr) => (pr.jobs ?? []).flatMap((job) => job.ran));
  const forced = ran.filter(
    (row) => row.cause === "one input" || row.cause === "several inputs",
  ).length;
  return (
    `reuse-spike: wrote ${out} (each step's line in steps.txt): ${observed.prs.filter((pr) => !pr.unread).length} of ${observed.prs.length} pull requests read, ` +
    `${ran.length} task runs ran and ${forced} of them the diff forced; net min per PR over today's tiers (linux/macOS/Windows): ` +
    `a-all ${over("a-all")}, b ${over("b")}, c ${over("c")}; d1 ${own("d-install-proofs")}; d2 ${platforms.map((x) => d2[x].minutesPerChange?.toFixed(1) ?? "-").join("/")} (estimate)`
  );
}

try {
  main(process.argv.slice(2));
} catch (error) {
  console.error(`reuse-spike: ${error.message.split("\n")[0]}`);
  console.error(`next: ${error instanceof check.Refusal ? error.next : USAGE}`);
  process.exitCode = 1;
}
