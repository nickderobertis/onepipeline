//! The `onepipeline` binary.
//!
//! Argument parsing, one call into the library, and an exit code. Every failure
//! carries the code `docs/contract.md` assigns it — `0` applied, `1` queued or
//! unfinished, `2` refused or malformed, `3` nothing is driving the run — so a
//! caller reads the outcome from the status rather than from the text.

use std::process::ExitCode;

use clap::Parser;
use onepipeline::cli::Cli;

fn main() -> ExitCode {
    reads_take_no_optional_locks();
    let cli = Cli::parse();
    match onepipeline::run(cli) {
        Ok(code) => exit(code),
        Err(error) => {
            eprintln!("onepipeline: {error}");
            exit(error.exit_code())
        }
    }
}

/// Have every git command this process starts skip git's optional locks.
///
/// A read `onevcs` makes in a checkout — `git status` above all — refreshes that
/// checkout's index as a side effect whenever it can take `index.lock`. A
/// manager's own checkout is shared: two landings started at once read and write
/// it together, and the one whose `git merge` meets the other's read finds
/// `index.lock` taken and fails. `GIT_OPTIONAL_LOCKS=0` is git's own answer for a
/// process reading a repository beside other work on it: every read is still
/// made, and none of them takes a lock it did not need. A value the operator set
/// is kept. Set here, before anything else runs, because it is the process's
/// environment and no thread has started yet to read it concurrently.
fn reads_take_no_optional_locks() {
    const OPTIONAL_LOCKS: &str = "GIT_OPTIONAL_LOCKS";
    if std::env::var_os(OPTIONAL_LOCKS).is_none() {
        std::env::set_var(OPTIONAL_LOCKS, "0");
    }
}

/// Narrow an exit code to the process's own type.
///
/// Every code this crate produces is one the contract names, all of which fit;
/// anything else would be a bug, and reporting it as a refusal is better than
/// truncating it into a code that means something else.
fn exit(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(onepipeline::error::EXIT_REFUSED as u8))
}
