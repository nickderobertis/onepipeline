//! Cross-platform repository gate commands used by lifecycle journeys.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use onepipeline_testfakes::{fail, refuse, required};

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("wait-for") => wait_for(&args),
        Some("append-future-event") => append_future_event(),
        Some(command) => refuse(&format!("unknown fake-gate command {command:?}")),
        None => refuse("missing fake-gate command"),
    }
}

fn wait_for(args: &[String]) -> ExitCode {
    let path = match required(args, 1, "path") {
        Ok(path) => PathBuf::from(path),
        Err(code) => return code,
    };
    while !path.is_file() {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    ExitCode::SUCCESS
}

fn append_future_event() -> ExitCode {
    let home = std::env::var_os("ONEVCS_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| fail("ONEVCS_HOME is unset"));
    let worktree = std::env::current_dir().unwrap_or_else(|error| fail(&error.to_string()));
    let token = worktree
        .parent()
        .and_then(|path| path.file_name())
        .unwrap_or_else(|| fail("the gate did not run in a session worktree"));
    let stream = home.join("streams").join(token).with_extension("ndjson");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&stream)
        .unwrap_or_else(|error| fail(&format!("cannot open {}: {error}", stream.display())));
    writeln!(file, "{{\"from\":\"a newer onevcs\"}}")
        .unwrap_or_else(|error| fail(&format!("cannot append to {}: {error}", stream.display())));
    ExitCode::SUCCESS
}
