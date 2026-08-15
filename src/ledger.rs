//! The run ledger: where a run's durable state lives, and who may write it.
//!
//! One directory per run under the runs root. The launch record says who owns
//! the run and how to relaunch it; the ownership lock is what makes the driving
//! process a single writer; the journal beside them is the merged event store.
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
    /// on its first line of output — and it dies mid-run, leaving a run whose
    /// graph never settles and whose driver is gone.
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

    /// The run's recorded result, rewritten whenever a driver closes out.
    ///
    /// One document, at the run's own root: the frontier is continuous, so what
    /// the ledger records is where the whole graph has got to.
    pub fn result(&self) -> PathBuf {
        self.dir.join("result.json")
    }
}

/// A run root this build refused, and the reason it gave.
///
/// A rejection, never an absence. A reader who is told nothing is there acts on
/// "nothing is running"; a reader who is told which directory was refused and
/// why can fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// The directory that was refused.
    pub path: PathBuf,
    /// Why, in the words the reader needs to act on it.
    pub reason: String,
}

/// Every run a root records, and every run root under it that records none.
///
/// The two halves are returned together because dropping the second is what
/// made an unreadable root indistinguishable from an empty one: a host with
/// thirty run roots on it rendered as a host with nothing running.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunIndex {
    /// The runs the root records, in id order.
    pub runs: Vec<RunPaths>,
    /// The run roots it refused, in path order.
    pub skipped: Vec<Skipped>,
}

/// Every run the root records, in id order, and every run root it refused.
///
/// A directory under the runs root is a *claim* to be a run, and one this build
/// cannot read is a claim it is rejecting — so it comes back named. A plain file
/// beside the runs is not such a claim and is passed over silently: nothing ever
/// said it was a run.
pub fn all_runs(root: &Path) -> RunIndex {
    let mut index = RunIndex::default();
    // llmlint: ignore-block[changed_behavior_has_e2e] a runs root that exists and will not
    // open, and an entry the filesystem lists and then refuses to describe, are host
    // conditions no portable journey can set. The arm a user reaches — a run root with no
    // launch record — is driven in `tests/e2e/views.rs`.
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        // A root nobody has written to yet holds nothing to reject, which is the
        // one reading of an empty view that is honest. Anything else is a root
        // this process was pointed at and could not read, and reporting that as
        // "no runs recorded" is the lie this whole index exists to stop.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return index,
        Err(error) => {
            index.skipped.push(Skipped {
                path: root.to_path_buf(),
                reason: format!("the runs root cannot be read: {error}"),
            });
            return index;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                index.skipped.push(Skipped {
                    path: root.to_path_buf(),
                    reason: format!("an entry under the runs root cannot be read: {error}"),
                });
                continue;
            }
        };
        // llmlint: ignore-end[changed_behavior_has_e2e]
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // llmlint: ignore-block[changed_behavior_has_e2e] a directory name that is not text
        // is not portably creatable — Windows refuses one outright — and no run id this
        // crate mints is one. What lands here is a directory an operator left beside the
        // runs, which this arm names rather than drops.
        let Ok(name) = entry.file_name().into_string() else {
            index.skipped.push(Skipped {
                path,
                reason: "its name is not text this host can read, so no run id names it".into(),
            });
            continue;
        };
        // llmlint: ignore-end[changed_behavior_has_e2e]
        let paths = RunPaths::under(root, &name);
        let launch = paths.launch();
        if !launch.is_file() {
            index.skipped.push(Skipped {
                path: paths.dir,
                reason: format!(
                    "no {}: a run root records the launch that owns it",
                    launch
                        .file_name()
                        .map_or_else(|| "launch record".into(), |name| name.to_string_lossy())
                ),
            });
            continue;
        }
        index.runs.push(paths);
    }
    index.runs.sort_by(|a, b| a.run.cmp(&b.run));
    index.skipped.sort_by(|a, b| a.path.cmp(&b.path));
    index
}

/// Whether a path field carries nothing, for the records that omit it then.
fn is_unset(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

/// What `start` recorded about a run, and what `adopt` replays it from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRecord {
    /// The run id.
    pub run_id: String,
    /// The plan file the run was launched with.
    pub plan: PathBuf,
    /// The directory every member of this run works in.
    ///
    /// The launch directory, made absolute once — at `start`, by the process
    /// the operator ran — and replayed verbatim by `adopt`. It reaches
    /// `oneagentgraph` as the run's `dir`, which is the `--cwd` each harness is
    /// given, so a relative value would resolve against whichever process
    /// happened to spawn the graph rather than against the directory the
    /// operator launched from, and `start` and `adopt` would name two different
    /// places for one run.
    ///
    /// Empty only on a record written before this field existed; the launcher
    /// then falls back to its own working directory, which is what it did then.
    /// Omitted when empty, like every other field added to this record after
    /// it shipped, so a build that predates it still reads what it wrote.
    #[serde(default, skip_serializing_if = "is_unset")]
    pub dir: PathBuf,
    /// The dag-scope agent-graph config launched as this run's observer.
    ///
    /// Absent when the launch named none, which is the shipped default: no
    /// agent is required to execute a plan. Read it through
    /// [`observer_graph`](Self::observer_graph) rather than testing this field —
    /// the serialized shape omits an absent value, and the one place that turns
    /// "omitted" back into "there is none" is that accessor.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub graph: String,
    /// The `oneagentgraph` run this run's observer graph is, as that library
    /// minted it — **not** this run's id, which names something else entirely.
    ///
    /// Written after the launch that produced it, and rewritten by every
    /// `adopt`, because an adoption starts a fresh graph run with an id of its
    /// own. It is how a later `onepipeline next` — a different process, with no
    /// handle on the observer — addresses the run's pacemaker. Empty when no
    /// observer graph was launched.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub graph_run: String,
    /// The default node-scope agent-graph config every dispatch launches.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node_graph: String,
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
    /// The pacemaker interval, in seconds.
    pub heartbeat_interval: u64,
    /// Opaque overrides replayed on the dag-scope graph launch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dag_sets: Vec<String>,
    /// Opaque overrides replayed on every node-scope graph launch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub node_sets: Vec<String>,
    /// How many times a fresh driver has been attached by `adopt`.
    #[serde(default)]
    pub adoptions: u32,
}

impl LaunchRecord {
    /// The observer graph this run was launched with, when it was launched with
    /// one.
    ///
    /// The record is a serialized schema and an absent string field is written
    /// as no field at all, so the absence arrives back as an empty one. This is
    /// where that becomes an [`Option`] again, so no caller decides for itself
    /// what an empty graph reference means.
    pub fn observer_graph(&self) -> Option<&str> {
        (!self.graph.is_empty()).then_some(self.graph.as_str())
    }

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
    // and the driving process's own writer among them. A second appender landing between
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
    /// The holder's own process start token, as
    /// [`sys::process_start_token`] read it when the lock was taken.
    ///
    /// The pid beside it says *which* process; this says it is still that
    /// process. A pid is reused, so a lock left behind by a driver that died two
    /// days ago names a pid the host may since have handed to something else —
    /// and a view reading the pid alone renders that as a live dispatch. Compared
    /// for equality against a fresh reading and never parsed.
    ///
    /// Empty when this host would not say, and on a record written before the
    /// field existed. Omitted when empty, like every other field added to a
    /// record after it shipped. Empty is **not** a match: it leaves a reader
    /// unable to prove the holder either way, which is the answer it has.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub started: String,
}

/// The run's ownership lock, released when this value is dropped.
///
/// The process driving a run is the only writer of its graph and its journal's
/// graph records, and this is what makes that true across processes: the lock
/// file is created exclusively, so a second writer loses the race rather than
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
    /// that died mid-run must not leave its run unwritable forever, which is
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
            started: sys::process_start_token(sys::pid())
                .map(|token| token.recorded().to_string())
                .unwrap_or_default(),
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
    /// driver's stream, and the engine loop's own writer — so a record has to reach the
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

        let first = OwnershipLock::acquire(&paths, "start").expect("the first writer wins");
        let second = OwnershipLock::acquire(&paths, "adopt");
        match second {
            Err(Error::Locked { run, pid, verb, .. }) => {
                assert_eq!(run, "demo");
                assert_eq!(pid, sys::pid());
                assert_eq!(verb, "start");
            }
            other => panic!("a second writer was not refused: {other:?}"),
        }

        first.release();
        OwnershipLock::acquire(&paths, "adopt").expect("the lock was released");
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
                verb: "start".to_string(),
                started: String::new(),
            },
        )
        .expect("a stale lock");

        OwnershipLock::acquire(&paths, "start").expect("a dead holder's lock is reclaimed");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_unreadable_lock_is_still_a_claim() {
        let root = scratch("unreadable");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        fs::write(paths.lock(), "not json at all").expect("a corrupt lock");

        assert!(matches!(
            OwnershipLock::acquire(&paths, "start"),
            Err(Error::Locked { .. })
        ));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_unknown_launch_is_nobodys_run() {
        let record = LaunchRecord {
            run_id: "demo".into(),
            plan: PathBuf::from("plan.json"),
            dir: PathBuf::from("/tmp/launch"),
            graph: "graphs/dag-scope.yaml".into(),
            graph_run: String::new(),
            node_graph: String::new(),
            launcher: sys::UNKNOWN_LAUNCHER.into(),
            session: sys::UNKNOWN_LAUNCHER.into(),
            pid: 1,
            host: "h".into(),
            started_at: sys::now_rfc3339(),
            heartbeat_interval: 1,
            dag_sets: Vec::new(),
            node_sets: Vec::new(),
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
            dir: PathBuf::from("/tmp/launch"),
            graph: "graphs/dag-scope.yaml".into(),
            graph_run: String::new(),
            node_graph: String::new(),
            launcher: "claude-code".into(),
            session: "secret-session-id".into(),
            pid: 1,
            host: "h".into(),
            started_at: sys::now_rfc3339(),
            heartbeat_interval: 1,
            dag_sets: Vec::new(),
            node_sets: Vec::new(),
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
        // A scratch name of its own: `scratch` derives the directory from the
        // process, so two tests naming one share it — and these two run at once,
        // which made the concurrent-appender count above fail on this test's
        // writes rather than on a torn record.
        let root = scratch("append-order");
        let path = root.join("queue.jsonl");
        assert!(read_lines(&path).is_empty());
        append_line(&path, "first").expect("appended");
        append_line(&path, "").expect("appended");
        append_line(&path, "second").expect("appended");
        assert_eq!(read_lines(&path), vec!["first", "second"]);
        fs::remove_dir_all(&root).ok();
    }

    /// A directory that is not a run is a **rejection**, and it comes back named.
    ///
    /// The reading it replaces: the same root reported two runs and said nothing
    /// at all about the third directory, so a reader could not tell a root that
    /// holds nothing from one whose contents were refused.
    #[test]
    fn only_directories_with_a_launch_record_are_runs_and_the_rest_are_named() {
        let root = scratch("index");
        for name in ["b-run", "a-run"] {
            let paths = RunPaths::under(&root, name);
            paths.create().expect("a run directory");
            write_json(&paths.launch(), &serde_json::json!({})).expect("a launch record");
        }
        fs::create_dir_all(root.join("scratch")).expect("a directory that records no run");
        fs::write(root.join("notes.txt"), "not a run root").expect("a file beside the runs");

        let index = all_runs(&root);
        let ids: Vec<String> = index.runs.iter().map(|r| r.run.clone()).collect();
        assert_eq!(ids, vec!["a-run".to_string(), "b-run".to_string()]);
        // The directory is named with its reason; the file never claimed to be a
        // run, so nothing is claimed about it either.
        assert_eq!(index.skipped.len(), 1, "{:?}", index.skipped);
        assert_eq!(index.skipped[0].path, root.join("scratch"));
        assert!(
            index.skipped[0].reason.contains("launch.json"),
            "{:?}",
            index.skipped[0]
        );

        // A root nobody has written to holds nothing to reject.
        assert_eq!(all_runs(&root.join("missing")), RunIndex::default());
        fs::remove_dir_all(&root).ok();
    }

    /// The lock names the process *and* proves it is still that process.
    ///
    /// A pid alone is what a two-day-old lock has, and a pid the host has since
    /// reused answers a liveness probe as alive — which is how a dead run's
    /// dispatches were rendered as a live fleet.
    #[test]
    fn a_lock_records_the_holders_start_token_beside_its_pid() {
        let root = scratch("stamp");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");

        let held = OwnershipLock::acquire(&paths, "drive").expect("the lock is taken");
        let record: LockRecord = read_json(&paths.lock()).expect("the lock reads back");
        assert_eq!(record.pid, sys::pid());
        assert!(
            sys::process_start_token(sys::pid())
                .expect("this host says when a process started")
                .matches(&record.started),
            "the lock's stamp is not this process's own start"
        );
        assert!(!record.started.is_empty());
        held.release();
        fs::remove_dir_all(&root).ok();
    }

    /// A lock written before the stamp existed still reads, and still round-trips
    /// without gaining a field it never had.
    #[test]
    fn a_lock_without_a_start_token_reads_and_is_written_back_without_one() {
        let record: LockRecord = serde_json::from_str(
            r#"{"pid":1,"host":"h","acquired_at":"2026-01-01T00:00:00.000Z","verb":"drive"}"#,
        )
        .expect("a lock from a build that predates the stamp");
        assert!(record.started.is_empty());
        let written = serde_json::to_string(&record).expect("it serializes");
        assert!(!written.contains("started"), "{written}");
    }
}
