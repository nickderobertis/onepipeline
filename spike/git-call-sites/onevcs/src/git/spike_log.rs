//! SPIKE (onepipeline `spike-git-call-sites`, never released): one JSON line per git
//! process [`super::run_fed`] starts, naming who asked for it.
//!
//! Off unless `ONEVCS_SPIKE_SPAWN_LOG` names a file. When it does, each process is
//! given an id in `ONEVCS_SPIKE_SPAWN`, and `GIT_TRACE2_ENV_VARS` names that variable,
//! so the process — and every git process it starts in turn (`git-upload-pack` and
//! `sh` under a local-transport fetch, push or `ls-remote`) — writes the id into its
//! own trace2 events. That is how a child git is joined to the call that caused it.
//!
//! Each line carries the argv, the environment `run_fed` added, the directory and the
//! repository (git common directory) it resolves to, the thread, the in-call memo's
//! scope, whether the on-disk proof cache (`recovery_cache`) was on, the start and end
//! times, the exit status, a hash of what git answered, and the frames of a backtrace
//! captured at the spawn (std, core and alloc frames dropped): the analysis takes the
//! outermost `onevcs` frame as the public function called and the frames above it as
//! the caller. A write onevcs makes in process (`export`'s libgit2 commit and branch)
//! is logged as a `write` line, since no git process shows it.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const LOG_ENV: &str = "ONEVCS_SPIKE_SPAWN_LOG";
const ID_ENV: &str = "ONEVCS_SPIKE_SPAWN";

thread_local! {
    /// The outermost read scope this thread is inside — the lifetime of the in-call
    /// memo (`reads::enter` / `reads::leave`) — or 0 outside every one.
    static SCOPE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A new outermost read scope begins on this thread.
pub(super) fn open_scope() {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    SCOPE.with(|scope| scope.set(NEXT.fetch_add(1, Ordering::Relaxed)));
}

/// This thread's outermost read scope ends.
pub(super) fn close_scope() {
    SCOPE.with(|scope| scope.set(0));
}

/// One spawn being logged.
pub(super) struct Spawn {
    file: PathBuf,
    id: String,
    began: f64,
    argv: Vec<String>,
    cwd: Option<String>,
    env: Vec<(String, String)>,
    repository: Option<String>,
    frames: Vec<String>,
    thread: String,
    scope: u64,
    proofs: bool,
}

/// The git common directory `dir` belongs to — a worktree's main repository, a
/// checkout's `.git`, or a bare repository itself — read from the files git keeps,
/// so that every worktree of one repository names the same one. `None` when `dir`
/// holds none of those (git would refuse the command, or the directory is gone).
fn common_dir(dir: &Path) -> Option<String> {
    let resolve = |git_dir: PathBuf| -> PathBuf {
        match std::fs::read_to_string(git_dir.join("commondir")) {
            Ok(common) => git_dir.join(common.trim()),
            Err(_) => git_dir,
        }
    };
    let mut at = Some(dir);
    while let Some(here) = at {
        let dot_git = here.join(".git");
        let found = if dot_git.is_dir() {
            Some(resolve(dot_git))
        } else if let Ok(pointer) = std::fs::read_to_string(&dot_git) {
            pointer
                .trim()
                .strip_prefix("gitdir: ")
                .map(|target| resolve(here.join(target)))
        } else if here.join("HEAD").is_file() && here.join("objects").is_dir() {
            Some(resolve(here.to_path_buf()))
        } else {
            None
        };
        if let Some(found) = found {
            let found = std::fs::canonicalize(&found).unwrap_or(found);
            return Some(found.display().to_string());
        }
        at = here.parent();
    }
    None
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The frames of a backtrace captured here, innermost first, with the standard
/// library's and the capture's own dropped.
fn frames() -> Vec<String> {
    let trace = std::backtrace::Backtrace::force_capture().to_string();
    trace
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (number, name) = line.split_once(": ")?;
            number.parse::<u32>().ok()?;
            Some(name.trim().to_owned())
        })
        .filter(|name| {
            let bare = name.trim_start_matches('<');
            !(bare.starts_with("std::")
                || bare.starts_with("core::")
                || bare.starts_with("alloc::")
                || bare.starts_with("onevcs::git::spike_log::")
                || bare.starts_with("__")
                || bare.starts_with("start_thread")
                || bare.starts_with("clone")
                || bare.starts_with("main")
                || bare.starts_with("_start")
                || bare.starts_with("<unknown>"))
        })
        .collect()
}

/// Begin logging one spawn, when the log is on, and tag `command` with its id.
pub(super) fn begin(
    command: &mut Command,
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(String, String)],
) -> Option<Spawn> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let file = PathBuf::from(std::env::var_os(LOG_ENV).filter(|v| !v.is_empty())?);
    let id = format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    command.env(ID_ENV, &id).env("GIT_TRACE2_ENV_VARS", ID_ENV);
    let current = std::thread::current();
    Some(Spawn {
        file,
        id,
        began: now(),
        argv: args.iter().map(|a| (*a).to_owned()).collect(),
        cwd: cwd.map(|c| c.display().to_string()),
        env: env.to_vec(),
        repository: cwd.and_then(common_dir),
        frames: frames(),
        thread: format!("{:?}:{}", current.id(), current.name().unwrap_or("")),
        scope: SCOPE.with(std::cell::Cell::get),
        proofs: crate::recovery_cache::enabled(),
    })
}

/// Write the spawn's line, with how it ended.
///
/// `answer` is git's status, standard output and whether it wrote nothing to standard
/// error — which, for a fetch, is whether it updated nothing.
pub(super) fn end(spawn: Spawn, answer: Option<(i32, &str, bool)>) {
    let (status, hash, quiet) = match answer {
        Some((status, stdout, quiet)) => {
            let mut hasher = DefaultHasher::new();
            status.hash(&mut hasher);
            stdout.hash(&mut hasher);
            (
                Some(status),
                Some(format!("{:016x}", hasher.finish())),
                Some(quiet),
            )
        }
        None => (None, None, None),
    };
    let line = serde_json::json!({
        "kind": "spawn",
        "id": spawn.id,
        "pid": std::process::id(),
        "thread": spawn.thread,
        "scope": spawn.scope,
        "t0": spawn.began,
        "t1": now(),
        "argv": spawn.argv,
        "cwd": spawn.cwd,
        "env": spawn.env,
        "repository": spawn.repository,
        "proofs": spawn.proofs,
        "status": status,
        "answer": hash,
        "quiet": quiet,
        "frames": spawn.frames,
    });
    let mut text = line.to_string();
    text.push('\n');
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&spawn.file)
    {
        let _ = file.write_all(text.as_bytes());
    }
}

/// Log a write onevcs made in process, to the repository at `dir`.
pub(crate) fn wrote(dir: &Path, what: &str) {
    let Some(file) = std::env::var_os(LOG_ENV).filter(|v| !v.is_empty()) else {
        return;
    };
    let t = now();
    let line = serde_json::json!({
        "kind": "write",
        "pid": std::process::id(),
        "t0": t,
        "t1": t,
        "what": what,
        "repository": common_dir(dir),
    });
    if let Ok(mut out) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        let _ = out.write_all(format!("{line}\n").as_bytes());
    }
}

/// Append a trace2 event file a command was pointed at in place of this process's
/// own `GIT_TRACE2_EVENT` to that one, when the spike log is on.
pub(super) fn forward_events(from: &Path) {
    if std::env::var_os(LOG_ENV)
        .filter(|v| !v.is_empty())
        .is_none()
    {
        return;
    }
    let Some(to) = std::env::var_os("GIT_TRACE2_EVENT").filter(|v| !v.is_empty()) else {
        return;
    };
    let Ok(events) = std::fs::read(from) else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(to)
    {
        let _ = file.write_all(&events);
    }
}
