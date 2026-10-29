import { compileMapping } from "../crate-split-replay.mjs";

export const COMMAND_MARKER = "<!-- BEGIN COMMAND REPLAY -->";
const pct = (value) => `${(value * 100).toFixed(2)}%`;

/** The catalog, not inferred test names in prose, owns individual journey assignments. */
export function commandAppendix(result, layout, catalog) {
  const reach = compileMapping(result.mapping.selection);
  const lines = [
    COMMAND_MARKER,
    "",
    "## Command replay output",
    "",
    `${result.tests} existing tests, ${result.suites} nonempty suites, 144 historical PRs. Empty command runners are excluded from the savings denominator. Test-second figures below are estimates obtained by dividing each measured module's seconds equally among its tests; they are not per-test measurements. Missing timings are excluded, never zero-duration observations.`,
    "",
    "| Platform | Suite runs skipped | Estimated observed test-seconds skipped | Observed module-seconds/full run |",
    "| --- | ---: | ---: | ---: |",
  ];
  for (const [name, value] of Object.entries(result.platforms))
    lines.push(
      `| ${name} | ${pct(value.suiteSkip)} | ${pct(value.secondsSkip)} | ${value.secondsPerRun.toFixed(0)} |`,
    );
  const cold = 574 + ((795 - 574) / 8) * (result.suites - 1);
  const incremental = 193 + ((264 - 193) / 8) * (result.suites - 1);
  const multiplier = cold / 574;
  lines.push(
    "",
    "Estimated compile stress scenario (not a benchmark or a confidence interval):",
    "",
    "| Job | Baseline | All-runner stress estimate | Delta |",
    "| --- | ---: | ---: | ---: |",
    `| Linux | 574 cold CPU s | ${cold.toFixed(1)} CPU s | +${(cold - 574).toFixed(1)} CPU s |`,
    `| macOS | 233 cached wall s | ${(233 * multiplier).toFixed(1)} wall s | +${(233 * (multiplier - 1)).toFixed(1)} wall s |`,
    `| Windows, each leg | 106–164 cached wall s | ${(106 * multiplier).toFixed(1)}–${(164 * multiplier).toFixed(1)} wall s | +${(106 * (multiplier - 1)).toFixed(1)}–${(164 * (multiplier - 1)).toFixed(1)} wall s |`,
    "",
    `Linux incremental all-source CPU stress estimate: 193 → ${incremental.toFixed(1)} s. Local test-phase scenario: 10,293.5 × ${pct(result.platforms.local.secondsSkip)} = ${(10293.5 * result.platforms.local.secondsSkip).toFixed(1)} wall seconds avoided on average if concurrency and scheduling stayed proportional; this excludes compilation and every other gate phase. There is no measured local gate speedup.`,
  );
  lines.push(
    "",
    "## Every command's test boundary",
    "",
    "Exact test names and individual integration reasons are in `scripts/modularization/command-tests.json`. The `cross-command` key there explicitly enumerates the exception set; the direct command column below counts only isolated tests. Dependencies include the generated runner's test-only external imports as well as the production closure. A zero means the command currently has only integration coverage, not that tests disappeared.",
    "",
    "| Command crate | Isolated tests | Cross-command tests | Production dependencies for isolated runner |",
    "| --- | ---: | ---: | --- |",
  );
  for (const name of Object.keys(layout.crates).filter((n) => n.startsWith("cmd-"))) {
    const direct = Object.values(catalog.tests).filter((test) => test.suite === name).length;
    const cross = Object.values(catalog.tests).filter(
      (test) => test.suite === "cross-command" && test.commands.includes(name),
    ).length;
    lines.push(
      `| ${name} | ${direct} | ${cross} | ${
        reach.suites[name]
          ? [...reach.suites[name].crates]
              .filter((n) => !n.startsWith("runner-"))
              .sort()
              .join(", ")
          : "integration suite only; no isolated runner counted"
      } |`,
    );
  }
  lines.push(
    "",
    "## Every PR still selecting more than half",
    "",
    "The named inputs below are the actual changed inputs whose union forces selection. `Cargo.toml` remains conservative until manifests are extracted: splitting package metadata and per-crate requirements removes unrelated manifest invalidation. The current single compiler pin really changes all Rust consumers; modularization alone cannot remove that dependency. Shared source edits can be narrowed only by moving the referenced DTO/helper or command fragment out of that source file; changing an actually shared wire type still reaches its consumers. Splitting borrowed test helpers removes whole-module attribution, but none is silently dropped here. Nx/recipe/config changes remain global. For rows without an individually broad input, the union of the listed narrower edits crosses half; no single-input design change eliminates that union.",
    "",
    "| PR / revision | Selected / total | Shared input or union | Design change removes it? |",
    "| --- | ---: | --- | --- |",
  );
  for (const row of result.rows.filter((r) => r.affected.length > result.suites / 2)) {
    const inputs = row.reasons.length ? row.reasons : row.files.map((p) => `union: ${p}`);
    const pr = /#(\d+)\)/.exec(row.subject)?.[1];
    const reasons = inputs
      .map((reason) =>
        reason.startsWith("resolved dependencies/toolchain:")
          ? "per-crate resolved dependencies/toolchain (exact consumers in replay JSON)"
          : reason,
      )
      .join("; ");
    const removal = row.files.includes("rust-toolchain.toml")
      ? "Compiler change: no; other inputs: only after their stated extraction."
      : row.files.includes("Cargo.toml")
        ? "Unrelated manifest edits: yes, split manifests; shared requirement/source changes: no."
        : "Only command fragments/borrowed helper attribution can narrow; actual shared behavior remains.";
    lines.push(
      `| ${pr ? `#${pr}` : row.commit.slice(0, 8)} / ${row.commit.slice(0, 8)} | ${row.affected.length}/${result.suites} | ${reasons} | ${removal} |`,
    );
  }
  lines.push(
    "",
    "## Command-suite migration replay",
    "",
    "Every extraction preserves the facade's public paths and type identities through re-exports, keeps the shipped dispatcher routing to the production command entry point, and moves the listed tests with unchanged assertions. Until a runner and all its dependency crates land, its tests still link the residual package. The compatibility facade and cross-command suite remain on the complete graph. Lock projections are not usable in Nx until the materialization hook and explicit projection-project dependencies land; these rows assume that prerequisite, described above.",
    "",
    "| Extract | Suite runs skipped after landing |",
    "| --- | ---: |",
  );
  for (const stage of result.migration) lines.push(`| ${stage.crate} | ${pct(stage.suiteSkip)} |`);
  return `${lines.join("\n")}\n`;
}
