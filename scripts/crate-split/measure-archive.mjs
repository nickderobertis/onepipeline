#!/usr/bin/env node
// Build the test archive the CI tiers run from, through the very recipe they build it
// with, and say what it cost: the wall seconds, and from cargo's own `--timings` report
// the seconds of every unit it compiled, workspace crates apart from their dependencies.
// A hosted demonstration of a crate or suite split runs it on each layout, so the
// difference is what the split costs to compile and link.
//
//   node scripts/crate-split/measure-archive.mjs coverage|quick
//
// `coverage` is `_crate-test-archive` (as test-linux builds it) and `quick` is
// `_crate-test-quick-archive` (as test-windows and test-macos build theirs). The
// command is the recipe's own line, read with `just --dry-run`, with `--timings`
// appended, so it cannot drift from the recipe. The archive goes to `$RUNNER_TEMP` (or
// the system temp directory), and every unit's seconds to
// `<target dir>/measure-archive.json`.
//
// Exit 0: built; one `measure-archive:` line of figures. Otherwise the build's own
// status (or 1), with the reason and a `next:` action on stderr; when cargo finished the
// build and only nextest refused to archive it, the figures line still prints, marked
// `archive=failed`. 2: no archive mode was named.
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as check from "./validate.mjs";

const RECIPE = { coverage: "_crate-test-archive", quick: "_crate-test-quick-archive" };

/** The units of a `cargo --timings` HTML report: name, target and seconds each. */
export function timingUnits(html) {
  const at = html.indexOf("UNIT_DATA = ");
  check.need(
    at >= 0,
    "the timings report holds no UNIT_DATA",
    "re-run the build; cargo's timings report was not one this reads",
  );
  const start = html.indexOf("[", at);
  let depth = 0;
  let end = start;
  for (; end < html.length; end++) {
    if (html[end] === "[") depth++;
    else if (html[end] === "]" && --depth === 0) break;
  }
  let units;
  try {
    units = JSON.parse(html.slice(start, end + 1));
  } catch (error) {
    throw new check.Refusal(
      `the timings report's UNIT_DATA is not JSON: ${error.message}`,
      "re-run the build",
    );
  }
  check.need(
    Array.isArray(units),
    "the timings report's UNIT_DATA is not a list",
    "re-run the build",
  );
  return units
    .map(check.timingUnit)
    .map((u) => ({ name: u.name, target: u.target, seconds: u.duration }));
}

/** The recipe's own command line, as `just --dry-run` prints it, with `--timings`. */
export function recipeCommand(mode, archive) {
  // just echoes a dry run's commands on stderr.
  const run = spawnSync("just", ["--dry-run", RECIPE[mode]], {
    encoding: "utf8",
    env: { ...process.env, ONEPIPELINE_TEST_ARCHIVE: archive },
  });
  check.need(
    run.status === 0,
    `just --dry-run ${RECIPE[mode]} failed: ${
      String(run.stderr ?? run.error?.message)
        .trim()
        .split("\n")[0]
    }`,
    "run from the repository's checkout with the just its CI installs",
  );
  const line = `${run.stdout}\n${run.stderr}`
    .trim()
    .split("\n")
    .filter((l) => /cargo .*archive/.test(l))
    .at(-1);
  check.need(
    line,
    `${RECIPE[mode]} prints no cargo archive command`,
    "point measure-archive.mjs at the recipe that builds the archive",
  );
  return `${line} --timings`;
}

function metadata() {
  const run = spawnSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
    encoding: "utf8",
  });
  check.need(
    run.status === 0,
    `cargo metadata failed: ${String(run.stderr).trim().split("\n").at(-1)}`,
    "run from a cargo workspace whose manifest cargo reads",
  );
  let doc;
  try {
    doc = JSON.parse(run.stdout);
  } catch (error) {
    throw new check.Refusal(
      `cargo metadata printed no JSON: ${error.message}`,
      "check the cargo on PATH",
    );
  }
  check.need(
    Array.isArray(doc?.packages) &&
      doc.packages.every((p) => typeof p.name === "string") &&
      typeof doc.target_directory === "string",
    "cargo metadata holds no packages or target directory",
    "check the cargo on PATH",
  );
  return doc;
}

function main([mode, ...rest]) {
  if (!Object.hasOwn(RECIPE, mode ?? "") || rest.length) {
    console.error("measure-archive: name the archive: coverage or quick");
    console.error("next: node scripts/crate-split/measure-archive.mjs coverage|quick");
    process.exitCode = 2;
    return;
  }
  const archive = join(process.env.RUNNER_TEMP || tmpdir(), "tests.tar.zst");
  const command = recipeCommand(mode, archive);
  const meta = metadata();
  const workspace = new Set(meta.packages.map((p) => p.name));
  const begun = Date.now();
  const build = spawnSync("bash", ["-c", command], {
    stdio: "inherit",
    env: { ...process.env, ONEPIPELINE_TEST_ARCHIVE: archive },
  });
  const wall = (Date.now() - begun) / 1000;
  if (build.error) {
    throw new check.Refusal(
      `the ${mode} archive build could not start: ${build.error.message}`,
      "check that bash and cargo are on PATH",
    );
  }
  if (build.status !== 0) {
    console.error(`measure-archive: the ${mode} archive build failed after ${wall.toFixed(1)} s`);
    console.error(
      `next: fix what cargo or nextest reported above, then re-run node scripts/crate-split/measure-archive.mjs ${mode}`,
    );
    process.exitCode = build.status ?? 1;
    // cargo may have finished and written its report before nextest refused to archive;
    // its units are still what the build cost.
  }
  // cargo-llvm-cov builds into its own directory under the target directory.
  const reports = [
    join(meta.target_directory, "cargo-timings"),
    join(meta.target_directory, "llvm-cov-target", "cargo-timings"),
  ].flatMap((dir) =>
    existsSync(dir)
      ? readdirSync(dir)
          .filter((f) => /^cargo-timing-.*\.html$/.test(f))
          .map((f) => ({ path: join(dir, f), at: statSync(join(dir, f)).mtimeMs }))
      : [],
  );
  // A report older than this build is an earlier build's, and says nothing of this one.
  const newest = reports.filter((r) => r.at >= begun).sort((a, b) => b.at - a.at)[0];
  if (!newest) {
    if (build.status === 0) {
      console.error(
        `measure-archive: the build wrote no cargo timings report under ${meta.target_directory}`,
      );
      console.error("next: check that the recipe's cargo accepts --timings, and re-run");
      process.exitCode = 1;
    }
    return;
  }
  const units = timingUnits(readFileSync(newest.path, "utf8"));
  const sum = (list) => list.reduce((s, u) => s + u.seconds, 0);
  const own = units.filter((u) => workspace.has(u.name));
  const detail = join(meta.target_directory, "measure-archive.json");
  writeFileSync(
    detail,
    `${JSON.stringify({ mode, wall_s: wall, report: newest.path, units }, null, 1)}\n`,
  );
  console.log(
    `measure-archive: mode=${mode}${build.status === 0 ? "" : " archive=failed"} wall_s=${wall.toFixed(1)} units=${units.length} unit_s=${sum(units).toFixed(1)} ` +
      `workspace_units=${own.length} workspace_unit_s=${sum(own).toFixed(1)} ` +
      `dependency_unit_s=${(sum(units) - sum(own)).toFixed(1)} workspace=${[...workspace].sort().join(",")} units_in=${detail}`,
  );
}

if (
  import.meta.url === `file://${process.argv[1]}` ||
  process.argv[1]?.endsWith("measure-archive.mjs")
) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`measure-archive: ${error.message.split("\n")[0]}`);
    console.error(
      `next: ${error instanceof check.Refusal ? error.next : "re-run; report the error above if it persists"}`,
    );
    process.exitCode = 1;
  }
}
