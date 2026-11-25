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
    spike_mark_process();
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

/// SPIKE (spike-git-call-sites, never landed): name this process and its verb in the
/// vendored `onevcs`'s per-spawn log, so each git process it starts is placed under
/// the `onepipeline` invocation that started it.
fn spike_mark_process() {
    use std::io::Write as _;
    let Some(file) = std::env::var_os("ONEVCS_SPIKE_SPAWN_LOG").filter(|v| !v.is_empty()) else {
        return;
    };
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    // Lossy, so an argument that is not UTF-8 is logged rather than panicking the engine.
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let line = format!(
        "{}\n",
        // llmlint: ignore[contracts_have_one_source_or_a_drift_gate] this spike's instrumentation record, read only by spike/git-call-sites/attribute.py on a branch never landed and in no quality gate; that reader's SPAWN_LOG check refuses any drifted record by file and line at every measurement.
        serde_json::json!({"kind": "process", "pid": std::process::id(), "argv": argv, "t": t})
    );
    if let Ok(mut out) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        let _ = out.write_all(line.as_bytes());
    }
}
