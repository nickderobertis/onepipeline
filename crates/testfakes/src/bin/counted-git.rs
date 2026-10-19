//! The real `git`, counted: one line per start appended to the file
//! `ONEPIPELINE_COUNTED_GIT_LOG` names, and then the real program, which
//! `ONEPIPELINE_COUNTED_GIT_REAL` names, run in its place.
//!
//! **Not a double**: every command reaches the real git with its argv, its
//! environment and its directory untouched. What it adds is the one figure no
//! other record of a journey holds — how many processes the stack started to do
//! its work — which is what a journey's cost is on a host where starting one is
//! slow: on the Windows runner a `git` start costs some twenty times what it does
//! on Linux.
//!
//! On Unix the real program *replaces* this one, so a start counted is exactly
//! one process. Elsewhere it is started and waited for, which is two, and is why
//! the harness installs it on Unix alone.

// llmlint: ignore-file[new_code_lands_in_a_project] `crates/testfakes` has no `project.json`
// of its own, like every double beside this file: its files are inputs of the
// `onepipeline-note-journeys` project (`noteJourneySource` lists `crates/**/*` in `nx.json`)
// and of the root project's `visualDocsSource`, and the crate is a member of the one Cargo
// workspace those projects build. It ships nowhere, and a project of its own would restate
// their targets over the same build unit.

use std::process::{Command, ExitCode};

/// The file each start is counted into.
const LOG_ENV: &str = "ONEPIPELINE_COUNTED_GIT_LOG";

/// The real `git` each start runs.
const REAL_ENV: &str = "ONEPIPELINE_COUNTED_GIT_REAL";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(real) = std::env::var_os(REAL_ENV) else {
        return onepipeline_testfakes::refuse(&format!("{REAL_ENV} names no git to run"));
    };
    let Some(log) = std::env::var_os(LOG_ENV) else {
        return onepipeline_testfakes::refuse(&format!("{LOG_ENV} names no file to count into"));
    };
    onepipeline_testfakes::append(
        std::path::Path::new(&log),
        &serde_json::json!({ "args": args }).to_string(),
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
fn run(mut real: Command) -> ExitCode {
    match real.status() {
        Ok(status) => ExitCode::from(status.code().map_or(1, |code| code as u8)),
        Err(error) => onepipeline_testfakes::refuse(&format!("cannot run the real git: {error}")),
    }
}
