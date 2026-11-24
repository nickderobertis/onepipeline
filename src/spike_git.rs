//! SPIKE (spike-git-call-sites, never landed): the engine's own `git` command, logged.
//!
//! Every git process the engine starts itself, rather than through `onevcs`, is made
//! here. With `ONEVCS_SPIKE_SPAWN_LOG` naming a file, each command gets an id in
//! `ONEVCS_SPIKE_SPAWN` that git writes into its trace2 events (through
//! `GIT_TRACE2_ENV_VARS`, as the vendored `onevcs`'s `run_fed` does), and one line in
//! that file carries the id with the backtrace frames of the call site. When the
//! command ends, [`finished`] writes a second line with its directory, the variables
//! it set, its exit status and a hash of what it answered, hashed as `run_fed`'s log
//! hashes them, so a repeat is found across both. Unset, the command is exactly
//! `Command::new("git")`.
//!
//! [`tag`] gives a non-git program the engine starts (the agent driver) an id of its
//! own, so every git that program starts in turn is joined to the engine call site
//! that started it.
//!
//! Proven by the harness's measured run rather than a test of its own: the branch is
//! never landed, and the run's report counts every engine-started git with no id.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write as _;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const LOG_ENV: &str = "ONEVCS_SPIKE_SPAWN_LOG";
const ID_ENV: &str = "ONEVCS_SPIKE_SPAWN";

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn append(line: &serde_json::Value) {
    let Some(file) = std::env::var_os(LOG_ENV).filter(|v| !v.is_empty()) else {
        return;
    };
    if let Ok(mut out) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        let _ = out.write_all(format!("{line}\n").as_bytes());
    }
}

/// Give `command` an id and log it with the frames of its call site, as `kind`.
fn log_start(command: &mut Command, kind: &str, what: &str) {
    if std::env::var_os(LOG_ENV)
        .filter(|v| !v.is_empty())
        .is_none()
    {
        return;
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = format!(
        "op-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    command.env(ID_ENV, &id).env("GIT_TRACE2_ENV_VARS", ID_ENV);
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
    append(&serde_json::json!({
        "kind": kind,
        "what": what,
        "id": id,
        "pid": std::process::id(),
        "thread": format!("{:?}:{}", current.id(), current.name().unwrap_or("")),
        "t0": now(),
        "frames": frames,
    }));
}

/// `git`, with its call site logged when the spike log is on.
pub(crate) fn command() -> Command {
    let mut command = Command::new("git");
    log_start(&mut command, "engine-spawn", "git");
    command
}

/// Tag a program the engine starts, so the git it starts in turn is attributed.
pub(crate) fn tag(command: &mut Command, what: &str) {
    log_start(command, "tool-spawn", what);
}

/// How a command [`command`] made ended: logged only for those, by their id.
pub(crate) fn finished(command: &Command, output: &std::io::Result<std::process::Output>) {
    let mut id = None;
    let mut env = Vec::new();
    for (key, value) in command.get_envs() {
        let (key, value) = (key.to_string_lossy(), value.map(|v| v.to_string_lossy()));
        if key == ID_ENV {
            id = value.map(|v| v.into_owned());
        } else if key != "GIT_TRACE2_ENV_VARS" {
            env.push((
                key.into_owned(),
                value.map(|v| v.into_owned()).unwrap_or_default(),
            ));
        }
    }
    let Some(id) = id.filter(|id| id.starts_with("op-")) else {
        return;
    };
    let cwd = command
        .get_current_dir()
        .map(std::path::Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok());
    let (status, answer, quiet) = match output {
        Ok(output) => {
            let status = output.status.code().unwrap_or(-1);
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let mut hasher = DefaultHasher::new();
            status.hash(&mut hasher);
            stdout.as_str().hash(&mut hasher);
            (
                Some(status),
                Some(format!("{:016x}", hasher.finish())),
                Some(output.stderr.is_empty()),
            )
        }
        Err(_) => (None, None, None),
    };
    append(&serde_json::json!({
        "kind": "engine-done",
        "id": id,
        "t1": now(),
        "argv": command.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "cwd": cwd.map(|c| c.display().to_string()),
        "env": env,
        "status": status,
        "answer": answer,
        "quiet": quiet,
    }));
}
