//! The run ledger: where a run's durable state lives, and who may write it.
//!
//! One directory per run under the runs root. The launch record says who owns
//! the run and how to relaunch it; the ownership lock is what makes the engine
//! verbs a single writer; the journal beside them is the merged event store.
//!
//! Writes that a reader could catch half-finished are atomic — written to a
//! temporary beside the target and renamed over it — because every view here
//! reads a live run's directory while its driver is writing to it.

// llmlint: ignore-file[invalid_states_unrepresentable] a run id, a host name, and a
// timestamp are `String`s in these records because each one is a *serialized* field: the
// launch record and the lock are JSON a human reads and another process parses, and every
// reader — including one written against an older build — has to accept what is there
// rather than what this build would mint. `docs/contract.md` names no `RunId`, so a
// newtype would also be a public vocabulary the contract did not ask for. What is
// enforced instead is the thing that matters: `owned_by` is the one place ownership is
// decided, and `unknown` is never anybody's.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::sys;

/// The environment variable that moves the runs root.
pub const RUNS_DIR_ENV: &str = "ONEPIPELINE_RUNS_DIR";

/// The runs root when the environment names none.
pub const DEFAULT_RUNS_DIR: &str = "runs";

/// The runs root this process reads and writes.
pub fn runs_root() -> PathBuf {
    std::env::var_os(RUNS_DIR_ENV)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_RUNS_DIR))
}

/// Where one run's durable state lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPaths {
    /// The run id, as every command and view names it.
    pub run: String,
    /// The run's own directory.
    pub dir: PathBuf,
}

/// Whether a run id names one directory under the runs root and nothing else.
///
/// A run id is external input on every verb that takes one, and — since
/// cross-DAG references carry one — inside plan files too. It is joined onto the
/// runs root, so a separator, a `..`, or an absolute path would read and write
/// *outside* the ledger this process was pointed at: `onepipeline status
/// ../../elsewhere` would render another root's run, and a plan naming
/// `run:../../elsewhere#node` would resolve its schedule against one.
///
/// One segment, and nothing that navigates. `mint_run_id` already produces only
/// this alphabet; this is the boundary for the ids that arrive from outside.
pub fn is_valid_run_id(run: &str) -> bool {
    !run.is_empty()
        && run != "."
        && run != ".."
        && !run.contains('/')
        && !run.contains('\\')
        && !Path::new(run).is_absolute()
        && Path::new(run).components().count() == 1
}

/// One producer-supplied name, as a single path segment.
///
/// Everything outside `[A-Za-z0-9._-]` becomes a `-`, and a name that is empty
/// or navigates gets one of its own: a segment built from a stranger's string
/// has to be a *name*, never a path, and `..` is the shortest path there is.
fn path_segment(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if mapped.is_empty() || mapped.chars().all(|c| c == '.') {
        return "unnamed".to_string();
    }
    mapped
}

impl RunPaths {
    /// The paths for `run` under the process's runs root.
    pub fn new(run: &str) -> Self {
        Self::under(&runs_root(), run)
    }

    /// The paths for `run` under an explicit root.
    pub fn under(root: &Path, run: &str) -> Self {
        Self {
            run: run.to_string(),
            dir: root.join(run),
        }
    }

    /// Whether this run has a directory at all.
    pub fn exists(&self) -> bool {
        self.dir.is_dir()
    }

    /// Create the run's directory and its channel subdirectory.
    pub fn create(&self) -> Result<()> {
        fs::create_dir_all(self.channel_dir()).map_err(|e| Error::Ledger {
            path: self.channel_dir(),
            source: e,
        })
    }

    /// The merged three-stream event store.
    pub fn journal(&self) -> PathBuf {
        self.dir.join("events.jsonl")
    }

    /// The launch record: who owns the run, and what to relaunch it with.
    pub fn launch(&self) -> PathBuf {
        self.dir.join("launch.json")
    }

    /// The plan the run was launched with.
    pub fn plan(&self) -> PathBuf {
        self.dir.join("plan.json")
    }

    /// The single-writer ownership lock the engine verbs hold.
    pub fn lock(&self) -> PathBuf {
        self.dir.join("owner.lock")
    }

    /// Where a **detached** driver's own output goes.
    ///
    /// A file rather than a pipe, because the process that would read the pipe
    /// is the launcher, and `--detach` means the launcher is about to exit. A
    /// driver holding the write end of a pipe nobody holds the read end of dies
    /// on its first line of output — and it dies mid-round, leaving a run whose
    /// round never closes and whose driver is gone.
    pub fn driver_log(&self) -> PathBuf {
        self.dir.join("driver.log")
    }

    /// The channel's transport state.
    pub fn channel_dir(&self) -> PathBuf {
        self.dir.join("channel")
    }

    /// Where this run keeps its own copy of the evidence its dispatches left.
    ///
    /// Run-**owned**: a sibling's report lives in that library's scratch, which
    /// is a directory this crate neither chooses nor can attest, and a reader
    /// that opened whatever a journal line pointed at would be an
    /// arbitrary-file reader driven by whatever wrote to the journal. So the
    /// evidence is copied here as it is ingested, and every reader afterwards
    /// opens only what is under this directory.
    pub fn reports_dir(&self) -> PathBuf {
        self.dir.join("reports")
    }

    /// This run's copy of one relayed settlement's report.
    ///
    /// Named from the producing stream and its sequence number, which identify
    /// the settlement and nothing else — so a reader derives the name rather
    /// than following a path, and both sides agree without either trusting one.
    /// The stream is written as a single sanitised segment: it is a producer's
    /// string, and joining one raw is how a name becomes a path.
    pub fn report_for(&self, stream: &str, seq: u64) -> PathBuf {
        self.reports_dir()
            .join(format!("{}-{seq}.json", path_segment(stream)))
    }

    /// A file within the channel's transport state.
    pub fn channel(&self, name: &str) -> PathBuf {
        self.channel_dir().join(name)
    }

    /// One round's directory.
    pub fn round_dir(&self, round: u64) -> PathBuf {
        self.dir.join(format!("round-{round:02}"))
    }

    /// A round's launch record — the graph it started with, never rewritten.
    pub fn round_plan(&self, round: u64) -> PathBuf {
        self.round_dir(round).join("plan.json")
    }

    /// A round's recorded result.
    pub fn round_result(&self, round: u64) -> PathBuf {
        self.round_dir(round).join("result.json")
    }
}

/// Every run the root records, in id order.
pub fn all_runs(root: &Path) -> Vec<RunPaths> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut runs: Vec<RunPaths> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| e.path().join("launch.json").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .map(|name| RunPaths::under(root, &name))
        .collect();
    runs.sort_by(|a, b| a.run.cmp(&b.run));
    runs
}

/// What `start` recorded about a run, and what `adopt` replays it from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRecord {
    /// The run id.
    pub run_id: String,
    /// The plan file the run was launched with.
    pub plan: PathBuf,
    /// The dag-scope agent-graph config the driver launches.
    pub graph: String,
    /// The launcher, as the environment reported it.
    pub launcher: String,
    /// The launching session. A view labels a foreign one by
    /// [`sys::session_digest`], never by this value.
    pub session: String,
    /// The driver process.
    pub pid: u32,
    /// The host that pid is meaningful on.
    pub host: String,
    /// When the run was launched.
    pub started_at: String,
    /// The round budget, in seconds.
    pub round_budget: u64,
    /// The pacemaker interval, in seconds.
    pub heartbeat_interval: u64,
    /// How many times a fresh driver has been attached by `adopt`.
    #[serde(default)]
    pub adoptions: u32,
}

impl LaunchRecord {
    /// Whether `session` is the session that launched this run.
    ///
    /// An `unknown` launch is nobody's, including the reader's — a
    /// provenance-less run never displays as the caller's, and never accepts a
    /// command that ownership guards.
    pub fn owned_by(&self, session: &str) -> bool {
        self.session != sys::UNKNOWN_LAUNCHER && self.session == session
    }

    /// How a view names this run's owner.
    pub fn owner_label(&self, session: &str) -> String {
        if self.session == sys::UNKNOWN_LAUNCHER {
            "[unknown]".to_string()
        } else if self.session == session {
            "[mine]".to_string()
        } else {
            format!("[{}:{}]", self.launcher, sys::session_digest(&self.session))
        }
    }
}

/// Read a JSON document, refusing anything the type does not accept.
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).map_err(|e| Error::Ledger {
        path: path.to_path_buf(),
        source: e,
    })?;
    serde_json::from_str(&text).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))
}

/// Read a JSON document, or `None` when it is absent or unreadable.
///
/// Used only where the contract says an unreadable input withholds a verdict
/// rather than ending the read: a view must still render the rest of a run.
pub fn read_json_opt<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

/// Write a JSON document so no reader can observe it half-written.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let body = serde_json::to_string_pretty(value)
        .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
    write_atomic(path, body.as_bytes())
}

/// Write bytes so no reader can observe them half-written.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let ledger = |e: io::Error| Error::Ledger {
        path: path.to_path_buf(),
        source: e,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(ledger)?;
    }
    // The temporary carries this process's pid so two writers racing the same
    // target cannot truncate each other's partial file before the rename.
    let temp = path.with_extension(format!("tmp.{}", sys::pid()));
    fs::write(&temp, bytes).map_err(ledger)?;
    fs::rename(&temp, path).map_err(ledger)
}

/// Append one line to a durable append-only file.
pub fn append_line(path: &Path, line: &str) -> Result<()> {
    use std::io::Write;

    let ledger = |e: io::Error| Error::Ledger {
        path: path.to_path_buf(),
        source: e,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(ledger)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(ledger)?;
    // One `write_all` of the record *and* its terminator, never `writeln!`:
    // that macro writes the text and the newline as two calls, and a run's
    // journal is appended by several processes at once — the launcher's relay
    // and the round's own writer among them. A second appender landing between
    // the two tears the record in half, and a torn line is silently skipped by
    // every reader here, so the event simply disappears.
    file.write_all(format!("{line}\n").as_bytes())
        .map_err(ledger)?;
    file.flush().map_err(ledger)
}

/// Every line of an append-only file, or nothing when it does not exist yet.
pub fn read_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Who holds a run's single-writer lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockRecord {
    /// The holding process.
    pub pid: u32,
    /// The host that pid is meaningful on.
    pub host: String,
    /// When it was taken.
    pub acquired_at: String,
    /// What the holder is doing, for the refusal message.
    pub verb: String,
}

/// The run's ownership lock, released when this value is dropped.
///
/// The engine verbs are the only writers of a run's graph, journal, and round
/// ledger, and this is what makes that true across processes: the lock file is
/// created exclusively, so a second writer loses the race rather than
/// interleaving with the first.
#[derive(Debug)]
pub struct OwnershipLock {
    path: PathBuf,
    /// Whether this value still holds the lock. A lock released by hand does
    /// not release again on drop.
    held: bool,
}

impl OwnershipLock {
    /// Take the run's lock, or report who holds it.
    ///
    /// A lock whose holder this host can prove is gone is reclaimed: a driver
    /// that died mid-round must not leave its run unwritable forever, which is
    /// the state `adopt` exists to recover from.
    pub fn acquire(paths: &RunPaths, verb: &str) -> Result<Self> {
        let path = paths.lock();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::Ledger {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let record = LockRecord {
            pid: sys::pid(),
            host: sys::hostname(),
            acquired_at: sys::now_rfc3339(),
            verb: verb.to_string(),
        };
        let body = serde_json::to_string(&record)
            .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;

        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(body.as_bytes()).map_err(|e| Error::Ledger {
                    path: path.clone(),
                    source: e,
                })?;
                Ok(Self { path, held: true })
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let held_by: Option<LockRecord> = read_json_opt(&path);
                match held_by {
                    // A holder on this host that this host can prove is gone
                    // leaves a lock nothing will release. Reclaim it.
                    Some(held)
                        if held.host == sys::hostname() && !sys::process_may_be_live(held.pid) =>
                    {
                        write_atomic(&path, body.as_bytes())?;
                        Ok(Self { path, held: true })
                    }
                    Some(held) => Err(Error::Locked {
                        run: paths.run.clone(),
                        pid: held.pid,
                        host: held.host,
                        verb: held.verb,
                    }),
                    // An unreadable lock is still a claim. Refusing is the safe
                    // reading: the alternative is a second writer on a run
                    // whose first writer cannot be identified.
                    None => Err(Error::Locked {
                        run: paths.run.clone(),
                        pid: 0,
                        host: sys::hostname(),
                        verb: "an unreadable lock".to_string(),
                    }),
                }
            }
            Err(e) => Err(Error::Ledger { path, source: e }),
        }
    }

    /// Release the lock now rather than at the end of the scope.
    pub fn release(mut self) {
        self.remove();
    }

    fn remove(&mut self) {
        if self.held {
            let _ = fs::remove_file(&self.path);
            self.held = false;
        }
    }
}

impl Drop for OwnershipLock {
    fn drop(&mut self) {
        self.remove();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("onepipeline-ledger-{name}-{}", sys::pid()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch root");
        dir
    }

    /// A run's journal has several appenders at once — the launcher relaying its
    /// driver's stream, and the round's own writer — so a record has to reach the
    /// file whole. Written as concurrent appenders because that is the only way
    /// the tearing shows: each opens its own descriptor, exactly as the separate
    /// processes do.
    #[test]
    fn concurrent_appenders_each_land_a_whole_line() {
        let root = scratch("append");
        let path = root.join("events.jsonl");
        const WRITERS: usize = 8;
        const EACH: usize = 60;

        std::thread::scope(|scope| {
            for writer in 0..WRITERS {
                let path = path.clone();
                scope.spawn(move || {
                    for n in 0..EACH {
                        let line = serde_json::json!({
                            "writer": writer,
                            "seq": n,
                            // Long enough that a torn write is not hidden by the
                            // kernel's small-write behaviour.
                            "payload": "x".repeat(512),
                        })
                        .to_string();
                        append_line(&path, &line).expect("the line is appended");
                    }
                });
            }
        });

        let lines = read_lines(&path);
        assert_eq!(lines.len(), WRITERS * EACH, "a record was torn or lost");
        for line in &lines {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|e| panic!("a torn record reached the file: {e}: {line}"));
        }
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_lock_refuses_a_second_writer_and_names_the_first() {
        let root = scratch("lock");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");

        let first = OwnershipLock::acquire(&paths, "round run").expect("the first writer wins");
        let second = OwnershipLock::acquire(&paths, "round next");
        match second {
            Err(Error::Locked { run, pid, verb, .. }) => {
                assert_eq!(run, "demo");
                assert_eq!(pid, sys::pid());
                assert_eq!(verb, "round run");
            }
            other => panic!("a second writer was not refused: {other:?}"),
        }

        first.release();
        OwnershipLock::acquire(&paths, "round next").expect("the lock was released");
        fs::remove_dir_all(&root).ok();
    }

    /// Every verb takes a run id, and a plan's cross-DAG reference carries one.
    /// It is joined onto the runs root, so anything that navigates reaches a
    /// ledger this process was never pointed at.
    #[test]
    fn a_run_id_names_one_directory_and_never_a_path() {
        for good in ["demo", "run-2", "a_b", "tracked-release", "R1"] {
            assert!(is_valid_run_id(good), "{good} was refused");
        }
        for bad in [
            "",
            ".",
            "..",
            "../elsewhere",
            "../../elsewhere",
            "a/b",
            "a\\b",
            "/absolute",
            "./here",
        ] {
            assert!(!is_valid_run_id(bad), "{bad:?} was accepted");
        }
    }

    #[test]
    fn a_lock_whose_holder_is_proved_gone_is_reclaimed() {
        let root = scratch("stale");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");

        let dead = sys::reaped_pid();

        write_json(
            &paths.lock(),
            &LockRecord {
                pid: dead,
                host: sys::hostname(),
                acquired_at: sys::now_rfc3339(),
                verb: "round run".to_string(),
            },
        )
        .expect("a stale lock");

        OwnershipLock::acquire(&paths, "round run").expect("a dead holder's lock is reclaimed");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_unreadable_lock_is_still_a_claim() {
        let root = scratch("unreadable");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        fs::write(paths.lock(), "not json at all").expect("a corrupt lock");

        assert!(matches!(
            OwnershipLock::acquire(&paths, "round run"),
            Err(Error::Locked { .. })
        ));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_unknown_launch_is_nobodys_run() {
        let record = LaunchRecord {
            run_id: "demo".into(),
            plan: PathBuf::from("plan.json"),
            graph: "graphs/dag-scope.yaml".into(),
            launcher: sys::UNKNOWN_LAUNCHER.into(),
            session: sys::UNKNOWN_LAUNCHER.into(),
            pid: 1,
            host: "h".into(),
            started_at: sys::now_rfc3339(),
            round_budget: 1,
            heartbeat_interval: 1,
            adoptions: 0,
        };
        assert!(!record.owned_by(sys::UNKNOWN_LAUNCHER));
        assert_eq!(record.owner_label("anyone"), "[unknown]");
    }

    #[test]
    fn a_foreign_owner_is_labelled_without_naming_the_session() {
        let record = LaunchRecord {
            run_id: "demo".into(),
            plan: PathBuf::from("plan.json"),
            graph: "graphs/dag-scope.yaml".into(),
            launcher: "claude-code".into(),
            session: "secret-session-id".into(),
            pid: 1,
            host: "h".into(),
            started_at: sys::now_rfc3339(),
            round_budget: 1,
            heartbeat_interval: 1,
            adoptions: 0,
        };
        let label = record.owner_label("mine");
        assert!(!label.contains("secret-session-id"), "{label} leaks the id");
        assert!(label.starts_with("[claude-code:"));
        assert_eq!(record.owner_label("secret-session-id"), "[mine]");
        assert!(record.owned_by("secret-session-id"));
    }

    #[test]
    fn an_atomic_write_leaves_no_temporary_behind() {
        let root = scratch("atomic");
        let target = root.join("nested").join("record.json");
        write_json(&target, &serde_json::json!({"ok": true})).expect("written");
        let value: serde_json::Value = read_json(&target).expect("read back");
        assert_eq!(value["ok"], serde_json::json!(true));
        let leftovers: Vec<_> = fs::read_dir(root.join("nested"))
            .expect("the directory")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "a temporary survived the rename");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn appended_lines_read_back_in_order_and_skip_blanks() {
        let root = scratch("append");
        let path = root.join("queue.jsonl");
        assert!(read_lines(&path).is_empty());
        append_line(&path, "first").expect("appended");
        append_line(&path, "").expect("appended");
        append_line(&path, "second").expect("appended");
        assert_eq!(read_lines(&path), vec!["first", "second"]);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_directories_with_a_launch_record_are_runs() {
        let root = scratch("index");
        for name in ["b-run", "a-run"] {
            let paths = RunPaths::under(&root, name);
            paths.create().expect("a run directory");
            write_json(&paths.launch(), &serde_json::json!({})).expect("a launch record");
        }
        fs::create_dir_all(root.join("scratch")).expect("a non-run directory");
        let ids: Vec<String> = all_runs(&root).into_iter().map(|r| r.run).collect();
        assert_eq!(ids, vec!["a-run".to_string(), "b-run".to_string()]);
        assert!(all_runs(&root.join("missing")).is_empty());
        fs::remove_dir_all(&root).ok();
    }
}
