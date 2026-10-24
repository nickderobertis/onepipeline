//! The real `git`, held once inside a named checkout, as `held-git ARGS…` placed on a
//! `PATH` under the name `git`.
//!
//! **Not a double for anything in this stack**: every invocation runs the real `git`
//! that `HELD_GIT_REAL` names, with its argv, environment, directory and exit code
//! untouched once it runs. What it adds is one pause. The first invocation started with
//! `HELD_GIT_HOLD` set that works on the `HELD_GIT_CLONE` repository writes its pid to
//! `HELD_GIT_DIR/inside` and waits for `HELD_GIT_DIR/release` before running `git`, so
//! a journey can hold a real `onevcs` read at its `git` on a session's clone for as long
//! as it needs. A `git` works on the clone when its working directory is the clone, or
//! when its global options name the clone's `.git` with `--git-dir` from anywhere else:
//! `onevcs` 0.56.8 and later address a read of a session's repository that way, from
//! outside every run root, where earlier releases ran it inside the clone. A compiled
//! program rather than a shell script, so the same pause holds on Windows, where a `git`
//! on `PATH` must be an executable.
//!
//! Exits 70 without running `git` when its own setup fails: no `HELD_GIT_REAL`, a hold
//! missing `HELD_GIT_CLONE` or `HELD_GIT_DIR`, a held clone or working directory that
//! does not resolve, a pause file it cannot write, a hold whose `HELD_GIT_DIR` is removed
//! before its release, or a `git` that does not start. A
//! `git` killed by a signal, which has no exit code, exits 1.

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

/// Wait for the release, when this is the first start working on the held clone.
fn hold() -> std::io::Result<()> {
    let variable = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .ok_or_else(|| std::io::Error::other(format!("{name} is not set")))
    };
    let clone = variable("HELD_GIT_CLONE")?;
    let control = variable("HELD_GIT_DIR")?;
    if !works_on(&clone)? {
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
        // A journey that ends without releasing — a failed assertion, its world removed
        // — takes the control directory with it, and nothing will ever release this.
        if !control.is_dir() {
            return Err(std::io::Error::other(format!(
                "{} was removed before the release",
                control.display()
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok(())
}

/// Whether this `git` works on the held `clone`: started inside it, or naming its `.git`
/// with `--git-dir`. The clone or a working directory that cannot be resolved is an error
/// rather than a mismatch, so a hold set up wrongly refuses instead of quietly not holding.
/// A named git directory that does not resolve is the real `git`'s to refuse.
fn works_on(clone: &Path) -> std::io::Result<bool> {
    let resolved = |path: &Path| {
        path.canonicalize().map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("{} does not resolve: {error}", path.display()),
            )
        })
    };
    let clone = resolved(clone)?;
    if resolved(&std::env::current_dir()?)? == clone {
        return Ok(true);
    }
    Ok(named_git_dir().is_some_and(|named| {
        let named = named.canonicalize().ok();
        named.is_some() && named == clone.join(".git").canonicalize().ok()
    }))
}

/// The `--git-dir` among `git`'s global options: those before its subcommand, where a
/// subcommand's own `--git-dir` (`rev-parse --git-dir`, a question) is never read.
fn named_git_dir() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if text == "--git-dir" {
            return args.next().map(PathBuf::from);
        }
        if let Some(dir) = text.strip_prefix("--git-dir=") {
            return Some(PathBuf::from(dir));
        }
        if !text.starts_with('-') {
            return None;
        }
        // The global options that take their value as the next argument.
        if matches!(text.as_ref(), "-C" | "-c" | "--work-tree" | "--namespace") {
            args.next();
        }
    }
    None
}
