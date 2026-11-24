//! SPIKE (spike-git-call-sites, never landed): the engine's own `git` command, logged.
//!
//! Every git process the engine starts itself, rather than through `onevcs`, is made
//! here. With `ONEVCS_SPIKE_SPAWN_LOG` naming a file, each command gets an id in
//! `ONEVCS_SPIKE_SPAWN` that git writes into its trace2 events (through
//! `GIT_TRACE2_ENV_VARS`, as the vendored `onevcs`'s `run_fed` does), and one line in
//! that file carries the id with the backtrace frames of the call site. The argv,
//! exit and children are joined from the trace2 events by that id. Unset, the command
//! is exactly `Command::new("git")`.
//!
//! Proven by the harness's measured run rather than a test of its own: the branch is
//! never landed, and the run's report counts every engine-started git with no id.

use std::io::Write as _;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// `git`, with its call site logged when the spike log is on.
pub(crate) fn command() -> Command {
    let mut command = Command::new("git");
    let Some(file) = std::env::var_os("ONEVCS_SPIKE_SPAWN_LOG").filter(|v| !v.is_empty()) else {
        return command;
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = format!(
        "op-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    command
        .env("ONEVCS_SPIKE_SPAWN", &id)
        .env("GIT_TRACE2_ENV_VARS", "ONEVCS_SPIKE_SPAWN");
    let frames: Vec<String> = std::backtrace::Backtrace::force_capture()
        .to_string()
        .lines()
        .filter_map(|line| {
            let (number, name) = line.trim_start().split_once(": ")?;
            number.parse::<u32>().ok()?;
            Some(name.trim().to_owned())
        })
        .filter(|name| {
            let bare = name.trim_start_matches('<');
            bare.starts_with("onepipeline") || bare.starts_with("onevcs")
        })
        .collect();
    let current = std::thread::current();
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let line = serde_json::json!({
        "kind": "engine-spawn",
        "id": id,
        "pid": std::process::id(),
        "thread": format!("{:?}:{}", current.id(), current.name().unwrap_or("")),
        "t0": t,
        "frames": frames,
    });
    if let Ok(mut out) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        let _ = out.write_all(format!("{line}\n").as_bytes());
    }
    command
}
