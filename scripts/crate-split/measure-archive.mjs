#!/usr/bin/env node
// Build the test archive the CI tiers run from, exactly as `ensure-test-archive.sh`'s
// recipes do, and say what it cost: the wall seconds, and from cargo's own `--timings`
// report the seconds of every unit it compiled, workspace crates apart from their
// dependencies. A hosted demonstration of a crate or suite split runs it on each
// layout, so the difference is what the split costs to compile and link.
//
//   node scripts/crate-split/measure-archive.mjs coverage|quick
//
// `coverage` is `_crate-test-archive` (cargo-llvm-cov's instrumented archive under
// `RUSTFLAGS="-D warnings"`, as test-linux builds it); `quick` is
// `_crate-test-quick-archive` (the plain archive test-windows and test-macos build).
// The archive goes to `$RUNNER_TEMP` (or the system temp directory). It prints one
// `measure-archive:` line of figures, and exits with cargo's status when the build fails.
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

/** The units of a `cargo --timings` HTML report: name, target and seconds each. */
export function timingUnits(html) {
  const at = html.indexOf("UNIT_DATA = ");
  if (at < 0) throw new Error("the timings report holds no UNIT_DATA");
  const start = html.indexOf("[", at);
  let depth = 0;
  let end = start;
  for (; end < html.length; end++) {
    if (html[end] === "[") depth++;
    else if (html[end] === "]" && --depth === 0) break;
  }
  return JSON.parse(html.slice(start, end + 1)).map((u) => ({
    name: u.name,
    target: u.target,
    seconds: u.duration,
  }));
}

function main([mode]) {
  if (!["coverage", "quick"].includes(mode)) {
    console.error("measure-archive: name the archive: coverage or quick");
    process.exitCode = 2;
    return;
  }
  const archive = join(process.env.RUNNER_TEMP || tmpdir(), "tests.tar.zst");
  const [command, args, env] =
    mode === "coverage"
      ? [
          "cargo",
          [
            "llvm-cov",
            "--no-report",
            "nextest-archive",
            "--locked",
            "--archive-file",
            archive,
            "--timings",
          ],
          { RUSTFLAGS: "-D warnings" },
        ]
      : ["cargo", ["nextest", "archive", "--locked", "--archive-file", archive, "--timings"], {}];
  const metadata = JSON.parse(
    spawnSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], { encoding: "utf8" })
      .stdout,
  );
  const workspace = new Set(metadata.packages.map((p) => p.name));
  const begun = Date.now();
  const build = spawnSync(command, args, { stdio: "inherit", env: { ...process.env, ...env } });
  const wall = (Date.now() - begun) / 1000;
  if (build.status !== 0) {
    console.error(`measure-archive: the ${mode} archive build failed after ${wall.toFixed(1)} s`);
    process.exitCode = build.status ?? 1;
    // cargo may have finished and written its report before nextest refused to archive;
    // its units are still what the build cost.
  }
  // cargo-llvm-cov builds into its own directory under the target directory.
  const reports = [
    join(metadata.target_directory, "cargo-timings"),
    join(metadata.target_directory, "llvm-cov-target", "cargo-timings"),
  ].flatMap((dir) =>
    existsSync(dir)
      ? readdirSync(dir)
          .filter((f) => /^cargo-timing-.*\.html$/.test(f))
          .map((f) => ({ path: join(dir, f), at: statSync(join(dir, f)).mtimeMs }))
      : [],
  );
  const newest = reports.sort((a, b) => b.at - a.at)[0];
  if (!newest || newest.at < begun) {
    if (build.status === 0) {
      console.error(
        `measure-archive: the build wrote no cargo timings report under ${metadata.target_directory}`,
      );
      process.exitCode = 1;
    }
    return;
  }
  const units = timingUnits(readFileSync(newest.path, "utf8"));
  const sum = (list) => list.reduce((s, u) => s + u.seconds, 0);
  const own = units.filter((u) => workspace.has(u.name));
  console.log(
    `measure-archive: mode=${mode}${build.status === 0 ? "" : " archive=failed"} wall_s=${wall.toFixed(1)} units=${units.length} unit_s=${sum(units).toFixed(1)} ` +
      `workspace_units=${own.length} workspace_unit_s=${sum(own).toFixed(1)} ` +
      `dependency_unit_s=${(sum(units) - sum(own)).toFixed(1)} workspace=${[...workspace].sort().join(",")}`,
  );
  for (const u of own.sort((a, b) => b.seconds - a.seconds)) {
    console.log(`measure-archive: unit ${u.name} ${u.target} ${u.seconds.toFixed(1)}`);
  }
}

if (
  import.meta.url === `file://${process.argv[1]}` ||
  process.argv[1]?.endsWith("measure-archive.mjs")
) {
  main(process.argv.slice(2));
}
