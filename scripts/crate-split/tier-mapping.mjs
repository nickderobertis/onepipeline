#!/usr/bin/env node
// Today's Nx tiers as a `crate-split-replay.mjs` mapping, read from one commit, so a
// replay of it is what Nx's cache keys do now and an option is a variant of it.
//
//   node scripts/crate-split/tier-mapping.mjs --clone DIR --changes OBSERVED.json
//        --out FILE [--commit REF] [--variant NAME]... [--split-e2e PROPOSED.json]
//
// One suite per cacheable Nx task an --observed replay (OBSERVED.json) saw run in a
// test job, weighed by that task's minutes: its `paths` are the globs its configured
// inputs expand to through `nx.json`'s named inputs at REF (default `refs/spike/main`),
// with every `runtime:` command, `config:<project>` and `external` its Nx hash plan
// holds; a task keyed on `rustBuild` exercises the one crate, whose paths are
// `rustBuild`'s sources. A task whose justfile recipe runs a tier from the shared
// nextest archive (`_tier-test`, `_tier-test-quick`) is marked `archive`. Exclusions
// (`!glob`) are not modelled, so a replay of the mapping can only over-run.
//
// --variant rewrites the mapping into one option (a) names; each is listed in VARIANTS.
// --split-e2e replaces the e2e tier with the suites a proposal maps (option (b),
// `optionB`).
//
// Exit 0: the mapping is written; one line names it and its suite count. 1: an argument,
// a document or the checkout was refused, with a `next:` action on stderr.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { checkOutAndPlan, NX_JOBS } from "./observed.mjs";
import * as check from "./validate.mjs";

const TEST_JOBS = NX_JOBS.filter((job) => job.startsWith("test-"));
const USAGE =
  "node scripts/crate-split/tier-mapping.mjs --clone DIR --changes OBSERVED.json --out FILE [--commit REF] [--variant NAME]... [--split-e2e PROPOSED.json]";

function readJson(path, what, next) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    throw new check.Refusal(`${what} ${path} cannot be read as JSON: ${error.message}`, next);
  }
}

/** Option (a)'s variants: each a rewrite of one suite's key, given its task and paths. */
export const VARIANTS = {
  // Every e2e-family tier keyed on the docs it reads, not every file under docs/.
  "docs-out-of-e2e": (task, paths) =>
    /^onepipeline-(e2e|version-e2e|contract|version-contract):/.test(task)
      ? paths.filter((p) => p !== "docs/**/*")
      : paths,
  // test-rest and test-quick keyed on the test files their binaries compile and read
  // (`tests/*.rs`, its fixtures and goldens), not on every script, workflow and the
  // documents of the repository.
  "rest-own-tests": (task, paths) =>
    /^onepipeline:(test-rest|test-quick)$/.test(task)
      ? paths.filter(
          (p) =>
            !/^(scripts\/\*\*\/\*|\.github\/\*\*\/\*|docs\/\*\*\/\*|screenshots\/\*\*\/\*|npm\/onepipeline-cli\/\*\*\/\*|release\/release\.py|AGENTS\.md|CODEOWNERS|biome\.jsonc|budgets\.yaml|deny\.toml|gh-secrets\.json|llmlint\.yml|release-plz\.toml|release-targets\.toml|screencomp\.toml|\.editorconfig|\.gitattributes|\.gitignore|README\.md|justfile|project\.json)$/.test(
              p,
            ) && !/^runtime:node scripts\/nx-version-free-inputs\.mjs --ecosystem python$/.test(p),
        )
      : paths,
  // Every tier's key without nx.json, whose edits are almost never to the key itself.
  "nx-json-out": (_task, paths) => paths.filter((p) => p !== "nx.json"),
};

function git(clone, args) {
  try {
    return gitRaw(clone, args);
  } catch (error) {
    throw new check.Refusal(
      `git ${args.join(" ")} failed in ${clone}: ${
        String(error.stderr ?? error.message)
          .trim()
          .split("\n")[0]
      }`,
      "give --clone a clone of this repository that holds --commit",
    );
  }
}

function gitRaw(clone, args) {
  return execFileSync("git", ["-C", clone, ...args], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  });
}

/** The positive globs a target's inputs expand to, and whether rustBuild is among them. */
export function expandInputs(namedInputs, inputs, projectRoot) {
  const globs = new Set();
  const seen = new Set();
  let rust = false;
  const subst = (glob) =>
    glob
      .replace("{workspaceRoot}/", "")
      .replace("{projectRoot}", projectRoot || ".")
      .replace(/^\.\//, "");
  const walk = (list) => {
    for (const input of list ?? []) {
      if (typeof input !== "string" || input.startsWith("^") || input.startsWith("!")) continue;
      if (Object.hasOwn(namedInputs, input)) {
        if (seen.has(input)) continue;
        seen.add(input);
        if (input === "rustBuild") rust = true;
        walk(namedInputs[input]);
      } else globs.add(subst(input));
    }
  };
  walk(inputs);
  return { globs: [...globs], rust };
}

export function tierMapping({ clone, commit, observed, variants = [] }) {
  const sha = git(clone, ["rev-parse", commit]).trim();
  const tasks = new Set();
  for (const pr of observed.prs) {
    for (const job of pr.jobs ?? []) {
      if (!TEST_JOBS.includes(job.job)) continue;
      for (const row of job.ran) if (row.cacheable) tasks.add(row.task);
      for (const row of job.replayed) tasks.add(row.task);
    }
  }
  // nx.json first: named inputs this cannot read are refused before Nx is asked to plan.
  let nxJson;
  try {
    nxJson = JSON.parse(git(clone, ["show", `${sha}:nx.json`]));
  } catch (error) {
    if (error instanceof check.Refusal) throw error;
    throw new check.Refusal(
      `${sha}'s nx.json is not JSON: ${error.message}`,
      "give --commit a commit Nx itself reads",
    );
  }
  check.namedInputs(nxJson.namedInputs ?? {}, `${sha}'s nx.json`);
  const plan = checkOutAndPlan(
    clone,
    sha,
    [...tasks].sort(),
    join(observed.evidence, "plans"),
    join(import.meta.dirname, "..", "..", "node_modules"),
  );
  git(clone, ["checkout", "-q", "-f", "--detach", sha]);
  // A checkout with no justfile has no recipe a task could run a tier from; one with a
  // justfile `just` cannot read is refused rather than read as having none.
  let dump = {};
  if (existsSync(join(clone, "justfile"))) {
    try {
      dump = JSON.parse(
        execFileSync("just", ["--dump", "--dump-format", "json"], {
          cwd: clone,
          encoding: "utf8",
          stdio: ["ignore", "pipe", "pipe"],
        }),
      ).recipes;
    } catch (error) {
      throw new check.Refusal(
        `just cannot read ${sha}'s justfile: ${
          String(error.stderr ?? error.message)
            .trim()
            .split("\n")[0]
        }`,
        "install the just the repository pins (`just bootstrap` names it) and re-run",
      );
    }
    check.need(
      dump !== null &&
        typeof dump === "object" &&
        Object.values(dump).every(
          (r) =>
            Array.isArray(r?.dependencies) &&
            r.dependencies.every((d) => typeof d?.recipe === "string"),
        ),
      `${sha}'s justfile dump is not recipes with dependency lists`,
      "install the just the repository pins (`just bootstrap` names it) and re-run",
    );
  }
  const archiveRecipe = (name, seen = new Set()) => {
    if (seen.has(name) || !dump[name]) return false;
    seen.add(name);
    if (name === "_tier-test" || name === "_tier-test-quick") return true;
    return dump[name].dependencies.some((dep) => archiveRecipe(dep.recipe, seen));
  };
  const rustSources = expandInputs(nxJson.namedInputs, ["rustBuild"], "").globs.filter((g) =>
    /^(src|crates|schemas)\//.test(g),
  );
  const suites = {};
  for (const [task, at] of Object.entries(plan.plans)) {
    if (!tasks.has(task) || at.cache === false) continue;
    const { globs, rust } = expandInputs(nxJson.namedInputs, at.inputs ?? [], at.root);
    let paths = [
      ...globs.filter((g) => !(rust && rustSources.includes(g))),
      ...at.plan.filter((e) => e.startsWith("runtime:")),
      `config:${task.slice(0, task.indexOf(":"))}`,
      // Nx's plan holds the external dependencies whole, or by named npm package.
      ...(at.plan.some((e) => e === "AllExternalDependencies" || e.startsWith("npm:"))
        ? ["external"]
        : []),
    ];
    for (const name of variants) paths = VARIANTS[name](task, paths);
    const recipe = /^just (\S+)$/.exec(String(at.command ?? ""))?.[1];
    suites[task] = {
      paths: [...new Set(paths)].sort(),
      crates: rust ? ["onepipeline"] : [],
      tasks: [task],
      archive: recipe ? archiveRecipe(recipe) : false,
    };
  }
  return {
    "//": `Today's Nx tiers at ${sha}${variants.length ? `, under option (a) variant(s) ${variants.join(", ")}` : ""}: written by scripts/crate-split/tier-mapping.mjs.`,
    sources: ["src/**", "crates/**", "schemas/**"],
    fallback: "onepipeline",
    crates: { onepipeline: { paths: rustSources, deps: [] } },
    suites,
  };
}

const E2E_TIERS = ["onepipeline-e2e:test", "onepipeline-e2e:test-quick"];

/**
 * Option (b): `base` (today's tiers) with the e2e tier, on every platform, replaced by
 * the suites `proposed` (`proposed.json`) maps. Each suite is keyed on its own files
 * (`paths`) and on everything else the e2e tier's key held that is not an e2e test file
 * — the harness's inputs, the runtime and configuration inputs, the crate — and weighed
 * by its modules' share of the e2e tier per platform. It runs from the shared archive.
 */
export function optionB(base, proposed) {
  const next = "give --split-e2e a proposal in scripts/crate-split/proposed.json's shape";
  check.need(
    proposed !== null &&
      typeof proposed === "object" &&
      proposed.suites &&
      typeof proposed.suites === "object",
    "the proposal holds no suites",
    next,
  );
  for (const [name, suite] of Object.entries(proposed.suites)) {
    check.need(/^[A-Za-z0-9._-]+$/.test(name), `${JSON.stringify(name)} is not a suite name`, next);
    const strings = (v) => Array.isArray(v) && v.every((x) => typeof x === "string" && x !== "");
    check.need(
      strings(suite?.paths) && strings(suite?.modules),
      `suite ${name} has no paths or modules lists`,
      next,
    );
  }
  const tiers = E2E_TIERS.filter((t) => base.suites[t]);
  check.need(
    tiers.length > 0,
    "the base mapping has no e2e tier to split",
    "generate it from an --observed replay that saw the e2e tier run",
  );
  const shared = [
    ...new Set(
      tiers.flatMap((t) => base.suites[t].paths.filter((p) => !p.startsWith("tests/e2e/"))),
    ),
  ];
  const suites = Object.fromEntries(
    Object.entries(base.suites).filter(([name]) => !tiers.includes(name)),
  );
  for (const [name, suite] of Object.entries(proposed.suites)) {
    suites[name] = {
      paths: [...new Set([...suite.paths, ...shared])].sort(),
      crates: ["onepipeline"],
      modules: suite.modules,
      archive: true,
    };
  }
  return {
    ...base,
    "//": `Option (b): ${base["//"]} with the e2e tier split into proposed.json's ${Object.keys(proposed.suites).length} suites.`,
    suites,
  };
}

function main(argv) {
  const options = { commit: "refs/spike/main", variant: [] };
  const names = ["clone", "changes", "out", "commit", "variant", "split-e2e"];
  for (let i = 0; i < argv.length; i += 2) {
    const name = argv[i]?.startsWith("--") ? argv[i].slice(2) : null;
    check.need(names.includes(name), `${argv[i]} is not an option this generator reads`, USAGE);
    const value = argv[i + 1];
    check.need(value !== undefined && !value.startsWith("--"), `--${name} needs a value`, USAGE);
    if (name === "variant") {
      check.need(
        Object.hasOwn(VARIANTS, value),
        `--variant ${value} is none of ${Object.keys(VARIANTS).join(", ")}`,
        USAGE,
      );
      options.variant.push(value);
    } else {
      check.need(!(name in options) || name === "commit", `--${name} is given twice`, USAGE);
      options[name] = value;
    }
  }
  for (const required of ["clone", "changes", "out"]) {
    check.need(options[required], `--${required} is required`, USAGE);
  }
  const observed = check.observedResult(
    readJson(options.changes, "--changes", "give the file an --observed replay wrote with --json"),
    options.changes,
  );
  check.need(
    typeof observed.evidence === "string",
    `${options.changes} names no evidence directory`,
    "write it again with --observed --json",
  );
  let mapping = tierMapping({
    clone: options.clone,
    commit: options.commit,
    observed,
    variants: options.variant,
  });
  if (options["split-e2e"]) {
    mapping = optionB(
      mapping,
      readJson(options["split-e2e"], "--split-e2e", "give scripts/crate-split/proposed.json"),
    );
  }
  writeFileSync(options.out, `${JSON.stringify(mapping, null, 2)}\n`);
  console.log(`tier-mapping: wrote ${options.out}: ${Object.keys(mapping.suites).length} suites`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`tier-mapping: ${error.message.split("\n")[0]}`);
    console.error(`next: ${error instanceof check.Refusal ? error.next : USAGE}`);
    process.exitCode = 1;
  }
}
