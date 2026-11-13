#!/usr/bin/env node
// Today's Nx tiers as a `crate-split-replay.mjs` mapping, read from one commit, so a
// replay of it is what Nx's cache keys do now and an option is a variant of it.
//
//   node scripts/crate-split/tier-mapping.mjs --clone DIR --changes OBSERVED.json
//        --out FILE [--commit REF] [--variant NAME]...
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
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { planAt } from "./observed.mjs";

const TEST_JOBS = ["test-linux", "test-windows", "test-macos"];

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
  return execFileSync("git", ["-C", clone, ...args], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
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
  const plan = planAt(
    clone,
    sha,
    [...tasks].sort(),
    join(observed.evidence, "plans"),
    join(import.meta.dirname, "..", "..", "node_modules"),
  );
  const nxJson = JSON.parse(git(clone, ["show", `${sha}:nx.json`]));
  git(clone, ["checkout", "-q", "-f", "--detach", sha]);
  // A checkout with no justfile (or no `just`) has no recipe a task could run a tier from.
  let dump = {};
  try {
    dump = JSON.parse(
      execFileSync("just", ["--dump", "--dump-format", "json"], {
        cwd: clone,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
      }),
    ).recipes;
  } catch {
    dump = {};
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
      ...(at.plan.includes("AllExternalDependencies") ? ["external"] : []),
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

function main(argv) {
  const options = { commit: "refs/spike/main", variant: [] };
  for (let i = 0; i < argv.length; i += 2) {
    const name = argv[i]?.replace(/^--/, "");
    if (
      !["clone", "changes", "out", "commit", "variant"].includes(name) ||
      argv[i + 1] === undefined
    ) {
      throw new Error(`${argv[i]} is not an option this generator reads`);
    }
    if (name === "variant") {
      if (!Object.hasOwn(VARIANTS, argv[i + 1])) {
        throw new Error(`--variant ${argv[i + 1]} is none of ${Object.keys(VARIANTS).join(", ")}`);
      }
      options.variant.push(argv[i + 1]);
    } else options[name] = argv[i + 1];
  }
  for (const required of ["clone", "changes", "out"]) {
    if (!options[required]) throw new Error(`--${required} is required`);
  }
  const observed = JSON.parse(readFileSync(options.changes, "utf8"));
  const mapping = tierMapping({
    clone: options.clone,
    commit: options.commit,
    observed,
    variants: options.variant,
  });
  writeFileSync(options.out, `${JSON.stringify(mapping, null, 2)}\n`);
  console.log(`tier-mapping: wrote ${options.out}: ${Object.keys(mapping.suites).length} suites`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`tier-mapping: ${error.message.split("\n")[0]}`);
    process.exitCode = 1;
  }
}
