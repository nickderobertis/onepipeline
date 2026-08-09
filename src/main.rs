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
    let cli = Cli::parse();
    match onepipeline::run(cli) {
        Ok(code) => exit(code),
        Err(error) => {
            eprintln!("onepipeline: {error}");
            exit(error.exit_code())
        }
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
