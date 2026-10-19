//! The real `git`, counted: one line per start appended to the file
//! [`COUNTED_GIT_LOG_ENV`] names, and then the real program, which
//! [`COUNTED_GIT_REAL_ENV`] names, run in its place.
//!
//! **Not a double**: every command reaches the real git with its argv, its
//! environment and its directory untouched. What it adds is the one figure no
//! other record of a journey holds — how many processes the stack started to do
//! its work — which is what a journey's cost is on a host where starting one is
//! slow: on the Windows runner a `git` start costs some twenty times what it does
//! on Linux.
//!
//! On Unix the real program *replaces* this one, so a start counted is exactly
//! one process. Elsewhere it would have to be started and waited for, which is
//! two and would double what it measures on the host where that is dearest, so
//! there it refuses and the harness installs it on Unix alone.

// llmlint: ignore-file[new_code_lands_in_a_project] `crates/testfakes` has no `project.json`
// of its own, like every double beside this file: its files are inputs of the
// `onepipeline-note-journeys` project (`noteJourneySource` lists `crates/**/*` in `nx.json`)
// and of the root project's `visualDocsSource`, and the crate is a member of the one Cargo
// workspace those projects build. It ships nowhere, and a project of its own would restate
// their targets over the same build unit.

use std::process::{Command, ExitCode};

use onepipeline_testfakes::{COUNTED_GIT_LOG_ENV, COUNTED_GIT_REAL_ENV};

fn main() -> ExitCode {
    // As the OS handed them over, so the real git is run with exactly these; the
    // count's own line spells them lossily, which is all a count needs.
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let Some(real) = std::env::var_os(COUNTED_GIT_REAL_ENV) else {
        return onepipeline_testfakes::refuse(&format!(
            "{COUNTED_GIT_REAL_ENV} names no git to run"
        ));
    };
    let Some(log) = std::env::var_os(COUNTED_GIT_LOG_ENV) else {
        return onepipeline_testfakes::refuse(&format!(
            "{COUNTED_GIT_LOG_ENV} names no file to count into"
        ));
    };
    let spelled: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    onepipeline_testfakes::append(
        std::path::Path::new(&log),
        &serde_json::json!({ "args": spelled }).to_string(),
    );
    let mut real = Command::new(real);
    real.args(&args);
    run(real)
}

#[cfg(unix)]
fn run(mut real: Command) -> ExitCode {
    use std::os::unix::process::CommandExt;
    let error = real.exec();
    onepipeline_testfakes::refuse(&format!("cannot run the real git: {error}"))
}

#[cfg(not(unix))]
fn run(real: Command) -> ExitCode {
    let _ = real;
    onepipeline_testfakes::refuse("counts on Unix alone, where the real git replaces it")
}
