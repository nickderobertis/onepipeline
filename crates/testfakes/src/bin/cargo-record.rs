//! Record Cargo starts while forwarding every argument to the real Cargo.
//! The e2e harness's build-cache journey uses this to observe builds without
//! substituting the compiler, its freshness checks, or its refusal paths.

use std::io::Write;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("cargo-record: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let real = std::env::var_os("ONEPIPELINE_REAL_CARGO").ok_or("real Cargo is required")?;
    let log = std::env::var_os("ONEPIPELINE_CARGO_STARTS").ok_or("a starts log is required")?;
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?;
    writeln!(log, "build")?;
    let status = Command::new(real)
        .args(std::env::args_os().skip(1))
        .status()?;
    Ok(ExitCode::from(
        status
            .code()
            .and_then(|code| u8::try_from(code).ok())
            .unwrap_or(1),
    ))
}
