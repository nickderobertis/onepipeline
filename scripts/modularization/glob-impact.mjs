import { compileMapping, affectedSuites } from "../crate-split-replay.mjs";

/** Isolate blanket-input invalidation while keeping the proposed crate/suite graph fixed. */
export function compareGlobInputs(selection, rows) {
  if (
    !Array.isArray(rows) ||
    !rows.length ||
    rows.some(
      (row) =>
        !row ||
        typeof row.commit !== "string" ||
        typeof row.subject !== "string" ||
        [row.files, row.effectiveFiles].some(
          (paths) => !Array.isArray(paths) || !paths.every((path) => typeof path === "string"),
        ),
    )
  )
    throw new Error("glob comparison needs validated PR rows");
  const narrow = compileMapping(selection);
  const wide = compileMapping({
    ...selection,
    shared: [...(selection.shared ?? []), "docs/**", "scripts/**"],
  });
  const suites = Object.keys(selection.suites).length;
  const compared = rows.map((row) => ({
    commit: row.commit,
    subject: row.subject,
    files: row.files.filter((path) => /^(?:docs|scripts)\//.test(path)),
    before: affectedSuites(wide, row.effectiveFiles).affected,
    after: affectedSuites(narrow, row.effectiveFiles).affected,
  }));
  const freed = compared.filter((row) => row.before.length > row.after.length);
  const denominator = rows.length * suites;
  return {
    prs: rows.length,
    suites,
    globPRs: compared.filter((row) => row.files.length).length,
    freed,
    fullyFreed: freed.filter((row) => !row.after.length).length,
    suiteRunsSaved: freed.reduce((sum, row) => sum + row.before.length - row.after.length, 0),
    wideSkip: compared.reduce((sum, row) => sum + suites - row.before.length, 0) / denominator,
    narrowSkip: compared.reduce((sum, row) => sum + suites - row.after.length, 0) / denominator,
  };
}
