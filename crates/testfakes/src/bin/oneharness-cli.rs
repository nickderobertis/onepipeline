//! The **released** `oneharness` CLI, as `oneharness ARGS...`.
//!
//! **Not a double for anything**: it is the shell of the sibling's own entry
//! point — parse, dispatch, exit with the code — over the `oneharness` crate this
//! lock resolves, and so over the one `oneharness-core` the engine links. A
//! journey that names it at `ONEAGENTGRAPH_ONEHARNESS_BIN` meets every refusal
//! the real CLI makes, a config it cannot satisfy among them, and doubles only
//! the paid harness below it, at oneharness's own `ONEHARNESS_BIN_<ID>`.
//! `fake-oneharness` beside it stands in for oneharness itself, so it cannot be
//! the one a journey about oneharness's own refusals runs.

// llmlint: ignore-file[new_code_lands_in_a_project] `crates/testfakes` has no `project.json`
// of its own, like every double beside this file: its files are inputs of the
// `onepipeline-note-journeys` project (`noteJourneySource` lists `crates/**/*` in `nx.json`)
// and of the root project's `visualDocsSource`, and the crate is a member of the one Cargo
// workspace those projects build. It ships nowhere, and a project of its own would restate
// their targets over the same build unit.

fn main() {
    let cli = oneharness::Cli::parse_or_exit();
    std::process::exit(oneharness::dispatch(cli));
}
