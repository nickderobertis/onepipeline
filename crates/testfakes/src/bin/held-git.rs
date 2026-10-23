//! The real `git`, held once inside a named checkout, as `held-git ARGS…` placed on a
//! `PATH` under the name `git`.
//!
//! **Not a double for anything in this stack**: every invocation runs the real `git`
//! that `HELD_GIT_REAL` names, with its argv, environment, directory and exit code
//! untouched once it runs. What it adds is one pause. The first invocation started with
//! `HELD_GIT_HOLD` set whose working directory is `HELD_GIT_CLONE` writes its pid to
//! `HELD_GIT_DIR/inside` and waits for `HELD_GIT_DIR/release` before running `git`, so
//! a journey can hold a real `onevcs` read with a `git` child inside a session's clone
//! for as long as it needs. A compiled program rather than a shell script, so the same
//! pause holds on Windows, where a `git` on `PATH` must be an executable.
//!
//! Exits 70 without running `git` when its own setup fails: no `HELD_GIT_REAL`, a hold
//! missing `HELD_GIT_CLONE` or `HELD_GIT_DIR`, a pause file it cannot write, or a `git`
//! that does not start. A `git` killed by a signal, which has no exit code, exits 1.

// llmlint: ignore-file[new_code_lands_in_a_project] `crates/testfakes` has no `project.json`
// of its own, like every double beside this file: its files are inputs of the
// `onepipeline-note-journeys` project (`noteJourneySource` lists `crates/**/*` in `nx.json`)
// and of the root project's `visualDocsSource`, and the crate is a member of the one Cargo
// workspace those projects build. It ships nowhere, and a project of its own would restate
// their targets over the same build unit.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let Some(real) = std::env::var_os("HELD_GIT_REAL") else {
        eprintln!("held-git: HELD_GIT_REAL names no git to run");
        return ExitCode::from(70);
    };
    if std::env::var_os("HELD_GIT_HOLD").is_some() {
        if let Err(error) = hold() {
            eprintln!("held-git: {error}");
            return ExitCode::from(70);
        }
    }
    match Command::new(real)
        .args(std::env::args_os().skip(1))
        .status()
    {
        Ok(status) => status
            .code()
            .and_then(|code| u8::try_from(code).ok())
            .map_or(ExitCode::FAILURE, ExitCode::from),
        Err(error) => {
            eprintln!("held-git: the real git did not start: {error}");
            ExitCode::from(70)
        }
    }
}

/// Wait for the release, when this is the first start inside the held checkout.
fn hold() -> std::io::Result<()> {
    let variable = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .ok_or_else(|| std::io::Error::other(format!("{name} is not set")))
    };
    let clone = variable("HELD_GIT_CLONE")?;
    let control = variable("HELD_GIT_DIR")?;
    if !same(&std::env::current_dir()?, &clone) {
        return Ok(());
    }
    // `create_new` makes the claim atomic, so of several starts only the first waits.
    let inside = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(control.join("inside.claim"))
    {
        Ok(_) => control.join("inside"),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error),
    };
    std::fs::write(&inside, std::process::id().to_string())?;
    let release = control.join("release");
    while !release.exists() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok(())
}

fn same(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}
