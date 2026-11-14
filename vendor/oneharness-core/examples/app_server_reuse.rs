//! The shared Codex app-server's sustained-reuse workload, entered through
//! oneharness-core's own pool: `live/control/app-server-reuse.sh` runs it.
//!
//! ```text
//! app_server_reuse --codex BIN --out PATH [--leases N] [--hold-ms MS]
//!                  [--settle-ms MS] [--cwd DIR]
//! ```
//!
//! Exits 2 on a usage error, 1 when the workload ran and failed.
//!
//! It takes N sequential leases on one pooled server through the same
//! [`PooledSession`] a `--control` dispatch's lease, connection and release go
//! through: each lease `initialize`s, starts a thread (`thread/start`), runs
//! one `thread/shellCommand` of `true` on it, resumes it once
//! (`thread/resume`), holds it `--hold-ms`, and releases it — never a
//! `turn/start`, so no model turn is spent. One idle lease of its own (no
//! thread) is held from before the first lease to after the last, so the
//! server cannot linger out in a gap between two leases and every lease lands
//! on the one server the readings measure. The shell turn is there because
//! codex-cli 0.159.2 writes a thread's rollout only after a turn and refuses to
//! resume a thread without one; a shell turn makes no model call. After the first and the last
//! release — once the server no longer holds the threads released so far
//! (`thread/loaded/list`, polled for at most `--settle-ms`) — it
//! reads the server's whole-tree resident memory (the pid in the
//! pool entry's `server.json` and every descendant, summed from one `ps`
//! listing's `rss` column), and writes the readings to `--out` as the JSON
//! `live/control/app_server_rss_budget.py` reads. A released thread still
//! loaded at the bound fails the workload, naming it beside both readings.
//!
//! Codex unloads a thread only once it has had no subscriber and no activity
//! for `thread_unload_delay_secs`: 60 s by default on codex-cli 0.159.2, and
//! 1800 s (30 minutes) by default on codex-cli 0.163.0, which keeps every
//! released thread loaded that long whatever the client sends. So the
//! workload's server is launched with that delay set to
//! [`UNLOAD_DELAY_SECS`]: the release it checks is the same — a thread whose
//! release leaves it subscribed is never unloaded, whatever the delay — but
//! Codex's unload of a released thread lands within the settle bound. A
//! dispatch's own server keeps Codex's default, so on 0.163.0 it holds each
//! thread a lease released for 30 minutes: retention bounded by that delay,
//! not by the server's life.

use std::path::{Path, PathBuf};
use std::time::Duration;

use oneharness_core::io::app_server::{serve_sweeper_if_requested, PooledDispatch, PooledSession};
use oneharness_core::io::server_pool::ThreadId;
use serde_json::{json, Value};

struct Args {
    codex: String,
    out: PathBuf,
    leases: usize,
    hold: Duration,
    settle: Duration,
    cwd: PathBuf,
}

/// The longest `--hold-ms` or `--settle-ms`, as app-server-reuse.sh bounds
/// its own setting: under 1 000 000 ms.
const MAX_MS: u64 = 999_999;

/// `raw` as a duration in whole milliseconds, at most [`MAX_MS`].
fn millis(flag: &str, raw: &str) -> Result<Duration, String> {
    match raw.parse::<u64>() {
        Ok(ms) if ms <= MAX_MS => Ok(Duration::from_millis(ms)),
        _ => Err(format!(
            "{flag} must be whole milliseconds from 0 to {MAX_MS}, got `{raw}`"
        )),
    }
}

fn args() -> Result<Args, String> {
    let mut codex = None;
    let mut out = None;
    let mut leases = 5;
    let mut hold = Duration::from_millis(1000);
    let mut settle = Duration::from_millis(90_000);
    let mut cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut raw = std::env::args().skip(1);
    while let Some(flag) = raw.next() {
        let mut value = || raw.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--codex" => codex = Some(value()?),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--leases" => leases = value()?.parse().map_err(|e| format!("--leases: {e}"))?,
            "--hold-ms" => hold = millis("--hold-ms", &value()?)?,
            "--settle-ms" => settle = millis("--settle-ms", &value()?)?,
            "--cwd" => cwd = PathBuf::from(value()?),
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    if leases == 0 {
        return Err("--leases must be at least 1".to_string());
    }
    Ok(Args {
        codex: codex.ok_or("--codex is required")?,
        out: out.ok_or("--out is required")?,
        leases,
        hold,
        settle,
        cwd: std::fs::canonicalize(&cwd).map_err(|e| format!("--cwd {}: {e}", cwd.display()))?,
    })
}

/// `root` and every pid descending from it through `parents` (pid, parent
/// pid), sorted.
fn descendants(root: u32, parents: &[(u32, u32)]) -> Vec<u32> {
    let mut tree = vec![root];
    let mut index = 0;
    while index < tree.len() {
        let parent = tree[index];
        tree.extend(
            parents
                .iter()
                .filter(|(_, ppid)| *ppid == parent)
                .map(|(pid, _)| *pid),
        );
        index += 1;
    }
    tree.sort_unstable();
    tree
}

/// The whole-tree reading of the server `pid`, from one `ps` listing of
/// every process's pid, parent and resident size — a single snapshot, so no
/// process can exit between being found and being read. `rss` is in KiB
/// (1024-byte units in ps(1) on Linux and macOS alike) and 0 for a zombie,
/// which holds none.
fn reading(pid: u32) -> Result<Value, String> {
    const LISTING: [&str; 3] = ["-A", "-o", "pid=,ppid=,rss="];
    let out = std::process::Command::new("ps")
        .args(LISTING)
        .output()
        .map_err(|e| format!("cannot run `ps`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`ps {}` failed ({}): {}",
            LISTING.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let listing = String::from_utf8_lossy(&out.stdout);
    let mut parents = Vec::new();
    let mut sizes = std::collections::HashMap::new();
    for row in listing.lines().filter(|row| !row.trim().is_empty()) {
        let fields: Vec<u64> = row
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|e| format!("`ps` listed an unreadable row {row:?}: {e}"))?;
        let [pid, ppid, kib] = fields[..] else {
            return Err(format!("`ps` listed {row:?}, not a pid, parent and rss"));
        };
        let (Ok(pid), Ok(ppid)) = (u32::try_from(pid), u32::try_from(ppid)) else {
            return Err(format!("`ps` listed a pid out of range: {row:?}"));
        };
        let bytes = kib
            .checked_mul(1024)
            .ok_or_else(|| format!("`ps` listed an rss out of range: {row:?}"))?;
        parents.push((pid, ppid));
        sizes.insert(pid, bytes);
    }
    if !sizes.contains_key(&pid) {
        return Err(format!("the server (pid {pid}) is not running"));
    }
    let pids = descendants(pid, &parents);
    let bytes = pids
        .iter()
        .filter_map(|pid| sizes.get(pid))
        .try_fold(0u64, |total, bytes| total.checked_add(*bytes))
        .ok_or_else(|| format!("the server tree's rss over pids {pids:?} is out of range"))?;
    Ok(json!({"rss_bytes": bytes, "pids": pids}))
}

fn version(codex: &str) -> Result<String, String> {
    let out = std::process::Command::new(codex)
        .arg("--version")
        .output()
        .map_err(|e| format!("cannot run `{codex} --version`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`{codex} --version` failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The `thread_unload_delay_secs` the workload's server is launched with:
/// short, so a released thread is unloaded within the settle bound, and not
/// zero, so the unload still waits on Codex's own delay as a dispatch's does.
const UNLOAD_DELAY_SECS: u64 = 1;

/// How far past the settle bound a listing begun before it may run.
const LISTING_GRACE: Duration = Duration::from_secs(10);

/// Wait until the server holds none of `released` loaded, polling through a
/// short-lived lease of its own (released again before the caller reads), for
/// at most `bound`. Answers the released threads still loaded at the bound.
fn settle(
    dispatch: PooledDispatch,
    released: &[ThreadId],
    bound: Duration,
) -> Result<Vec<ThreadId>, String> {
    let mut observer = PooledSession::open(dispatch)?;
    let until = std::time::Instant::now()
        .checked_add(bound)
        .ok_or("the settle bound is past what the clock can count")?;
    let still = loop {
        // The last poll starts at the bound at the latest, and its listing
        // must finish within LISTING_GRACE of it.
        let loaded = observer.loaded_threads(until + LISTING_GRACE)?;
        let still: Vec<ThreadId> = released
            .iter()
            .filter(|t| loaded.contains(t))
            .cloned()
            .collect();
        if still.is_empty() || std::time::Instant::now() >= until {
            break still;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    observer.release();
    Ok(still)
}

fn run(args: &Args) -> Result<Value, String> {
    // The launch a default controlled dispatch would key its server by, plus
    // the short unload delay that lets a released thread's unload be seen.
    let unload_delay = format!("thread_unload_delay_secs={UNLOAD_DELAY_SECS}");
    let argv: Vec<String> = [
        args.codex.as_str(),
        "app-server",
        "-c",
        "features.plugins=false",
        "-c",
        "features.apps=false",
        "-c",
        "analytics.enabled=false",
        "-c",
        unload_delay.as_str(),
    ]
    .iter()
    .map(|arg| (*arg).to_string())
    .collect();
    let mut leases = Vec::new();
    let mut after_first = Value::Null;
    let mut after_last = Value::Null;
    let mut released: Vec<ThreadId> = Vec::new();
    let mut retained: Vec<ThreadId> = Vec::new();
    let keeper = PooledSession::open(
        PooledDispatch::plan(&argv, &[], &[], &args.cwd, false)
            .ok_or("the launch is not a codex app-server launch")?,
    )?;
    for lease in 1..=args.leases {
        let dispatch = PooledDispatch::plan(&argv, &[], &[], &args.cwd, false)
            .ok_or("the launch is not a codex app-server launch")?;
        let mut session = PooledSession::open(dispatch)?;
        let pid = session.record().pid.get();
        let started = session.open_thread(&args.cwd, None)?;
        let thread = started
            .pointer("/result/thread/id")
            .and_then(Value::as_str)
            .and_then(ThreadId::new)
            .ok_or("thread/start named no usable thread id")?;
        session.shell_turn("true")?;
        session.open_thread(&args.cwd, Some(&thread))?;
        std::thread::sleep(args.hold);
        session.release();
        released.push(thread.clone());
        leases.push(json!({"lease": lease, "server_pid": pid, "thread": thread}));
        if lease == 1 || lease == args.leases {
            let observe = PooledDispatch::plan(&argv, &[], &[], &args.cwd, false)
                .ok_or("the launch is not a codex app-server launch")?;
            for still in settle(observe, &released, args.settle)? {
                if !retained.contains(&still) {
                    retained.push(still);
                }
            }
            let taken = reading(pid)?;
            if lease == 1 {
                after_first = taken.clone();
            }
            if lease == args.leases {
                after_last = taken;
            }
        }
    }
    keeper.release();
    Ok(json!({
        "retained": retained,
        "codex_version": version(&args.codex)?,
        "platform": std::env::consts::OS,
        "lease_count": leases.len(),
        "leases": leases,
        "readings": {"after_first": after_first, "after_last": after_last},
    }))
}

fn write(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, format!("{value:#}\n")).map_err(|e| format!("{}: {e}", path.display()))
}

fn main() {
    serve_sweeper_if_requested();
    let args = args().unwrap_or_else(|error| {
        eprintln!("app_server_reuse: {error}");
        std::process::exit(2);
    });
    let outcome = (|| {
        let value = run(&args)?;
        write(&args.out, &value)?;
        // Retention is its own failure, not a ratio that happens to read high:
        // a released thread the server still holds at the bound is named,
        // beside both readings.
        match value["retained"].as_array().filter(|threads| !threads.is_empty()) {
            Some(threads) => Err(format!(
                "released thread(s) {threads:?} still loaded {}s after their release; after the last of {} leases: {} B over pids {}; after the first: {} B over pids {}",
                args.settle.as_secs(),
                value["lease_count"],
                value["readings"]["after_last"]["rss_bytes"],
                value["readings"]["after_last"]["pids"],
                value["readings"]["after_first"]["rss_bytes"],
                value["readings"]["after_first"]["pids"],
            )),
            None => Ok(()),
        }
    })();
    if let Err(error) = outcome {
        eprintln!("app_server_reuse: {error}");
        std::process::exit(1);
    }
}
