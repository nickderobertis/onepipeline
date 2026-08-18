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
use crate::filter::Filters;
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

    /// Create the run's directory and the two subdirectories a run always has.
    ///
    /// The dispatch registry among them, and empty is the answer it is created to
    /// be able to give: a reader that meets no registry at all cannot tell a run
    /// with nothing running from one whose record of what it is running has gone,
    /// and refuses. So a run has one from the moment it exists, and its absence
    /// afterwards means something took it away.
    pub fn create(&self) -> Result<()> {
        for dir in [self.channel_dir(), self.dispatches()] {
            fs::create_dir_all(&dir).map_err(|e| Error::Ledger {
                path: dir,
                source: e,
            })?;
        }
        Ok(())
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

    /// The dispatch ownership registry: one record per process this run has
    /// work running in.
    ///
    /// A directory of small records rather than one document, because its
    /// writers are the run's dispatch threads and they start and finish
    /// independently: a single file would be read, edited, and rewritten by
    /// several of them at once, and a lost update there is a live dispatch no
    /// later stop can find.
    pub fn dispatches(&self) -> PathBuf {
        self.dir.join("dispatches")
    }

    /// One dispatch's record, named by the process it runs in and the claim that
    /// wrote it.
    ///
    /// Named from a pid because a pid is always a safe file name and a node id is
    /// not: an id is plan text, required to be non-empty and unique and nothing
    /// else, so joining one raw is how a name becomes a path — and sanitising it
    /// would map two distinct nodes onto one record.
    ///
    /// A pid alone is **not** an identity, which is the other half. A run's
    /// dispatches can share one process: that is what the library backend is —
    /// several nodes running concurrently inside the driver — so two live
    /// dispatches would write one entry, the second would overwrite the first,
    /// and the first to end would take the survivor's registration with it,
    /// leaving a live dispatch nothing could find. `claim` is what tells them
    /// apart, and it is unique for the life of the process that mints it.
    pub fn dispatch(&self, pid: u32, claim: u64) -> PathBuf {
        self.dispatches().join(format!("{pid}-{claim}.json"))
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
        let path = entry.path();
        // Asked for, rather than tested with `is_dir`: that helper answers
        // `false` both for "not a directory" and for "this host would not say",
        // and reading the second as the first is the collapse this whole index
        // exists to undo — the entry would be dropped as though it had never
        // claimed to be a run.
        let about = match fs::metadata(&path) {
            Ok(about) => about,
            // Gone between the listing and the look. A run swept while this scan
            // was running is not a root to make any claim about.
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                index.skipped.push(Skipped {
                    path,
                    reason: format!("this host will not describe it: {error}"),
                });
                continue;
            }
        };
        // llmlint: ignore-end[changed_behavior_has_e2e]
        if !about.is_dir() {
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
        let named = launch
            .file_name()
            .map_or_else(|| "launch record".into(), |name| name.to_string_lossy());
        // Every answer kept apart, for the same reason as above: absent, present
        // as something that is not a record, and unreadable are three different
        // things to tell a reader, and `is_file` says `false` to all three.
        let refused = match fs::metadata(&launch) {
            Ok(about) if about.is_file() => None,
            Ok(_) => Some(format!(
                "its {named} is not a file, so it records no launch"
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Some(format!(
                "no {named}: a run root records the launch that owns it"
            )),
            // llmlint: ignore-block[changed_behavior_has_e2e] a launch record this host
            // lists and will not describe is a host condition no portable journey can set.
            // The two answers a user reaches are both driven in `tests/e2e/views.rs`.
            Err(error) => Some(format!("its {named} cannot be read: {error}")),
            // llmlint: ignore-end[changed_behavior_has_e2e]
        };
        if let Some(reason) = refused {
            index.skipped.push(Skipped {
                path: paths.dir,
                reason,
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
    /// The agent-graph config a lifecycle node's change request body is drafted
    /// by, when the launch named one.
    ///
    /// Absent when it named none, which is the shipped default: this crate ships
    /// the flag and not the document, so a launch that says nothing drafts
    /// nothing. Read it through [`pr_author_graph`](Self::pr_author_graph)
    /// rather than testing this field, for the reason [`graph`](Self::graph) is
    /// read through its own accessor. Omitted when empty, like every other field
    /// added to this record after it shipped, so a build that predates it still
    /// reads what it wrote.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pr_author_graph: String,
    /// The launcher, as the environment reported it.
    pub launcher: String,
    /// The launching session. A view labels a foreign one by
    /// [`sys::session_digest`], never by this value.
    pub session: String,
    /// The driver process.
    pub pid: u32,
    /// The host that pid is meaningful on.
    pub host: String,
    /// That driver's own process start token, as [`sys::process_start_token`]
    /// read it when it claimed the run.
    ///
    /// The same proof, and for the same reason, as the ownership lock's and the
    /// registry's: the pid says *which* process and this says it is still that
    /// one. This record outlives every driver it names — a driver that died
    /// leaves its pid sitting here until something adopts the run — so by the
    /// time a `stop` reads it the host may have handed that pid to a stranger,
    /// and a teardown aimed at it would end work this run never started.
    ///
    /// Written only by [`driven_by_this_process`](Self::driven_by_this_process),
    /// which writes all three fields together: a pid recorded without the stamp
    /// beside it is a pid no later reader may act on.
    ///
    /// Empty when this host would not say, and on a record written before the
    /// field existed. Omitted when empty, like every other field added to this
    /// record after it shipped, so a build that predates it still reads what it
    /// wrote. Empty is **not** a match — see [`sys::StartToken::matches`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub started: String,
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
    /// What this launch said about its run's events: the two source filters it
    /// passes through, and the read-time profiles it defines.
    ///
    /// Retained here rather than derived per command because both halves outlive
    /// the launching process: `adopt` replays the source filters onto the graphs
    /// it restarts, and every later `next` and `monitor` — different processes,
    /// with no handle on the launch — reads through the profiles this run was
    /// given. Omitted when empty, like every other field added to this record
    /// after it shipped, so a build that predates it still reads what it wrote.
    #[serde(default, skip_serializing_if = "Filters::is_empty")]
    pub filters: Filters,
}

impl LaunchRecord {
    /// Record that **this** process is now the run's driver.
    ///
    /// The one writer of the three fields that name a driver, because they are
    /// one fact and a record carrying two of them is a pid nothing can act on:
    /// a `stop` reading a pid with no stamp beside it cannot tell the driver it
    /// was written for from whatever the host has since given that pid to. Every
    /// path that claims a run — the launch, the driver a detached launch
    /// retains, and each adoption — goes through here.
    pub fn driven_by_this_process(&mut self) {
        self.pid = sys::pid();
        self.host = sys::hostname();
        self.started = sys::process_start_token(self.pid)
            .map(|token| token.recorded().to_string())
            .unwrap_or_default();
    }

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

    /// The graph this run drafts change request bodies with, when it was
    /// launched with one.
    ///
    /// The same reading [`observer_graph`](Self::observer_graph) has, for the
    /// same reason: an absent string field is written as no field at all, and
    /// this is the one place that absence becomes "there is none" again.
    pub fn pr_author_graph(&self) -> Option<&str> {
        (!self.pr_author_graph.is_empty()).then_some(self.pr_author_graph.as_str())
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

/// One live dispatch's claim on the process it is running in.
///
/// The registry answers a question neither the launch record nor the ownership
/// lock can: *what is this run actually running, and where*. Both of those name
/// a **driver**, and a driver is not the work — it starts the work, and when it
/// dies the work it started is reparented away and outlives it, findable by
/// nothing that descends from a pid either record holds. That is a live dispatch
/// a stop cannot reach and an operator is told is over.
///
/// Written by the machine running the dispatch, which is the one that knows
/// which process the work is in, and removed when that dispatch ends. Every
/// field is required, the stamp included: a record that cannot prove its own pid
/// is not a weaker entry but an unusable one, and the type is what stops one
/// being written or read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchRecord {
    /// The node this dispatch is running.
    pub node: String,
    /// The process the work is running in.
    pub pid: u32,
    /// The host that pid is meaningful on.
    pub host: String,
    /// When the dispatch was recorded.
    pub dispatched_at: String,
    /// That process's own start token, as [`sys::process_start_token`] read it
    /// when the dispatch started.
    ///
    /// The same proof, and for the same reason, as the ownership lock's: the pid
    /// says *which* process and this says it is still that one. A record outlives
    /// the driver that wrote it — that is the case it exists for — so by the time
    /// anything reads it the host may have handed the pid to a stranger, and a
    /// teardown aimed at that would end work this run never started.
    pub started: String,
}

impl DispatchRecord {
    /// Whether this entry is one a reader may act on.
    ///
    /// An empty stamp parses and proves nothing, which is the one state the
    /// field's type cannot rule out. A registry holding one cannot say whether
    /// the pid beside it is still this run's work, and *cannot say* is the answer
    /// this registry exists to stop being read as *nothing is running*.
    fn is_usable(&self) -> bool {
        !self.started.trim().is_empty()
    }
}

/// A dispatch's entry in the registry, removed when this value is dropped.
///
/// RAII for the same reason [`OwnershipLock`] is: a dispatch ends in more ways
/// than it settles, and every one of them drops this. The single ending that
/// leaves the entry behind is the process itself dying, which is exactly when a
/// stop needs it.
#[derive(Debug)]
pub struct DispatchClaim {
    path: PathBuf,
    /// What this claim recorded, so it removes its **own** entry and never a
    /// later dispatch's: the host reissues pids, and a record keyed by one is
    /// only this dispatch's while the process behind it is.
    started: String,
}

impl Drop for DispatchClaim {
    fn drop(&mut self) {
        let ours = read_json_opt::<DispatchRecord>(&self.path)
            .is_some_and(|held| held.started == self.started);
        if ours {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Record that this run is running `node` in `pid`, on this host, or refuse.
///
/// A **trust boundary**, not bookkeeping. The registry is the only record of
/// where a run's work actually is, so a dispatch this run cannot register is a
/// process nothing will ever find: not the operator reading a view, and not the
/// `stop` they run when they need the work to end. Continuing anyway would buy
/// one dispatch at the price of the guarantee every later stop rests on — so the
/// caller is given the failure and ends the dispatch with it.
///
/// Two ways to fail, and both are refusals rather than empty entries. A host that
/// will not say when `pid` started leaves nothing that could prove the pid is
/// still this process, and an entry a reader cannot act on is one that would make
/// a later stop refuse instead. A write that did not land — or landed as
/// something other than what was written — is the same absence with a file in the
/// way, so what was written is read back before the claim is handed over.
pub fn claim_dispatch(paths: &RunPaths, node: &str, pid: u32) -> Result<DispatchClaim> {
    // Unique for the life of this process, which is what separates two dispatches
    // running inside it. Across processes the pid separates them, and across a
    // pid this host has reissued the stamp does.
    static CLAIMED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let claim = CLAIMED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let Some(started) = sys::process_start_token(pid) else {
        return Err(Error::Refused(format!(
            "run '{}': node '{node}': this host will not say when pid {pid} started, so its \
             dispatch cannot be recorded as running there and nothing could prove that pid is \
             still this run's work",
            paths.run
        )));
    };
    let record = DispatchRecord {
        node: node.to_string(),
        pid,
        host: sys::hostname(),
        dispatched_at: sys::now_rfc3339(),
        started: started.recorded().to_string(),
    };
    let path = paths.dispatch(pid, claim);
    write_dispatch(paths, &path, claim, &record)?;
    // Read back through the same reader a stop uses, because what this promises
    // its caller is not that a write returned but that the registry now holds an
    // entry that reader will act on.
    //
    // llmlint: ignore-block[changed_behavior_has_e2e] a write that lands as something other
    // than what was written is a filesystem lying to a process, not anything a user can type
    // or a suite can arrange: every fault a journey *can* set — no directory, a file where
    // one has to be, an entry rewritten afterwards — fails earlier, in branches
    // `a_dispatch_this_run_cannot_record_is_refused_and_does_not_run` and
    // `stopping_a_run_whose_registry_cannot_be_read_refuses_and_leaves_the_run_retryable`
    // drive end to end. What this arm adds is that the promise is checked rather than
    // assumed, and `a_dispatch_the_registry_cannot_record_is_refused` holds the refusal it
    // produces against the real filesystem.
    match read_json::<DispatchRecord>(&path) {
        Ok(held) if held == record => Ok(DispatchClaim {
            path,
            started: record.started,
        }),
        Ok(_) | Err(_) => Err(Error::Refused(format!(
            "run '{}': node '{node}': its dispatch in pid {pid} was written to {} and did not \
             read back as itself, so the run cannot say where that work is",
            paths.run,
            path.display()
        ))),
    } // llmlint: ignore-end[changed_behavior_has_e2e]
}

/// Write one registry entry so no reader can catch it half-written.
///
/// Renamed into the registry from a temporary **outside** it, which is the whole
/// difference from [`write_atomic`]: every file in that directory is an entry a
/// reader acts on, and a reader is now entitled to fail on one it cannot read. A
/// temporary written beside its target would be a half-written entry in the set,
/// and a `stop` racing a dispatch would refuse over this crate's own scratch.
fn write_dispatch(
    paths: &RunPaths,
    path: &Path,
    claim: u64,
    record: &DispatchRecord,
) -> Result<()> {
    let body = serde_json::to_string_pretty(record)
        .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
    let ledger = |at: &Path| {
        let at = at.to_path_buf();
        move |source: io::Error| Error::Ledger { path: at, source }
    };
    fs::create_dir_all(paths.dispatches()).map_err(ledger(&paths.dispatches()))?;
    // Named from the claim as well as the process, for the reason
    // [`RunPaths::dispatch`] gives: two dispatches inside one process would
    // otherwise write one another's temporary, and a reader is entitled to fail
    // on an entry it cannot parse.
    let temp = paths
        .dir
        .join(format!("dispatch-{}-{claim}.tmp", record.pid));
    fs::write(&temp, body.as_bytes()).map_err(ledger(&temp))?;
    fs::rename(&temp, path).map_err(ledger(path))
}

/// Every dispatch this run has recorded, in pid order — or why this build cannot
/// say.
///
/// Errors are **preserved**, never flattened into an empty registry, and that is
/// this reader's whole job. "Nothing is registered" and "what is registered
/// cannot be read" are opposite answers for the caller that acts on them: the
/// first says a run has no work running, and the second says nobody knows — and a
/// stop that read the second as the first would report a run ended over work it
/// never looked for. So a registry that is not there, a directory this host will
/// not enumerate, an entry that cannot be read, one carrying a field this build
/// does not know, and one whose stamp proves nothing are all failures with the
/// path that caused them.
///
/// Ordered because a caller acts on them — a teardown signals what they name —
/// and a directory listing comes in whatever order the host gives.
pub fn dispatches_of(paths: &RunPaths) -> Result<Vec<DispatchRecord>> {
    let registry = paths.dispatches();
    let listed = fs::read_dir(&registry).map_err(|source| Error::Ledger {
        path: registry.clone(),
        source,
    })?;
    let mut found = Vec::new();
    for entry in listed {
        // llmlint: ignore-block[changed_behavior_has_e2e] an enumeration that fails *part way* is
        // the host withdrawing a directory it has already begun to list — a condition no
        // portable journey can set, and one this reader answers exactly as it answers the
        // directory it could not open at all, which
        // `stopping_a_run_whose_registry_cannot_be_read_refuses_and_leaves_the_run_retryable`
        // drives end to end for both the missing registry and the entry it cannot read.
        let entry = entry.map_err(|source| Error::Ledger {
            path: registry.clone(),
            source,
        })?; // llmlint: ignore-end[changed_behavior_has_e2e]
        let held: DispatchRecord = read_json(&entry.path())?;
        if !held.is_usable() {
            return Err(Error::Invalid(format!(
                "{}: the dispatch it records carries no start token, so nothing says pid {} is \
                 still this run's work",
                entry.path().display(),
                held.pid
            )));
        }
        found.push(held);
    }
    found.sort_by_key(|held| held.pid);
    Ok(found)
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

    /// The record a launch that declared no filters writes is the record every
    /// build before the block existed wrote.
    ///
    /// Checked at the wire rather than through the types: `Filters::default()`
    /// and an explicit `"filters": {}` are the same value in Rust whatever the
    /// serializer does, but writing the empty block out would make a record this
    /// build wrote unreadable to a build that predates the field — which is the
    /// one thing an added field must not do — and would have every reader
    /// branching on a key that is always present and usually meaningless.
    #[test]
    fn a_launch_that_declared_no_filters_writes_the_record_it_always_wrote() {
        let record = LaunchRecord {
            filters: Filters::default(),
            ..a_record()
        };
        let text = serde_json::to_string(&record).expect("it serialises");
        assert!(
            !text.contains("filters"),
            "an empty filters block reached the record: {text}"
        );
        assert_eq!(
            serde_json::from_str::<LaunchRecord>(&text).expect("it re-parses"),
            record
        );

        // Which is the same thing as saying a record written *before* the field
        // existed still reads, as the launch it was: nothing filtered, and the
        // shipped profiles to read through. Spelled as its own document rather
        // than inferred from the omission above, because that is the file on
        // disk this build has to keep opening.
        let older = serde_json::json!({
            "run_id": "demo",
            "plan": "plan.json",
            "node_graph": "graphs/node-scope.yaml",
            "launcher": "claude-code",
            "session": "a-session",
            "pid": 1,
            "host": "h",
            "started_at": "2026-08-15T00:00:00.000Z",
            "heartbeat_interval": 1800,
        });
        let read: LaunchRecord =
            serde_json::from_value(older).expect("a record predating the field still reads");
        assert!(read.filters.is_empty());
    }

    /// A record that *did* declare filters carries every one of them back.
    #[test]
    fn a_launchs_filters_survive_the_record_they_are_retained_in() {
        let declared = Filters {
            agentgraph: Some(
                crate::filter::EventFilter::parse(r#"{"exclude": [{"kind": "turn-*"}]}"#)
                    .expect("a filter"),
            ),
            vcs: Some(
                crate::filter::EventFilter::parse(r#"{"include": [{"kind": "gate-*"}]}"#)
                    .expect("a filter"),
            ),
            profiles: [(
                "planner".to_string(),
                crate::filter::EventFilter::parse(r#"{"include": [{"source": "pipeline"}]}"#)
                    .expect("a filter"),
            )]
            .into_iter()
            .collect(),
        };
        let record = LaunchRecord {
            filters: declared.clone(),
            ..a_record()
        };
        let text = serde_json::to_string(&record).expect("it serialises");
        let read: LaunchRecord = serde_json::from_str(&text).expect("it re-parses");
        assert_eq!(read.filters, declared);
        assert_eq!(read, record);
    }

    /// A launch record with nothing said about its events.
    fn a_record() -> LaunchRecord {
        LaunchRecord {
            run_id: "demo".into(),
            plan: PathBuf::from("plan.json"),
            dir: PathBuf::from("/tmp/launch"),
            graph: String::new(),
            graph_run: String::new(),
            node_graph: "graphs/node-scope.yaml".into(),
            pr_author_graph: String::new(),
            launcher: "claude-code".into(),
            session: "a-session".into(),
            pid: 1,
            host: "h".into(),
            started: "Fri Aug 15 00:00:00 2026".into(),
            started_at: sys::now_rfc3339(),
            heartbeat_interval: 1_800,
            dag_sets: Vec::new(),
            node_sets: Vec::new(),
            adoptions: 0,
            filters: Filters::default(),
        }
    }

    /// A driver is claimed as three facts at once, and a record that predates
    /// the stamp is read as proving nothing rather than as proving its pid.
    ///
    /// The pid and the stamp are one claim: a `stop` reading a pid with nothing
    /// beside it cannot tell the driver the record was written for from whatever
    /// the host has since handed that pid to, so the writer that records one
    /// records both. The compatibility half is the same promise every field
    /// added to this record makes — an older document still reads — and the
    /// answer it must give is the *empty* stamp, which never matches.
    #[test]
    fn claiming_a_run_records_the_stamp_that_proves_its_pid_and_an_older_record_carries_none() {
        let mut record = a_record();
        record.driven_by_this_process();
        assert_eq!(record.pid, sys::pid());
        assert_eq!(record.host, sys::hostname());
        assert!(
            sys::process_start_token(sys::pid())
                .expect("this host says when a process started")
                .matches(&record.started),
            "a run claimed by this process recorded a stamp that does not prove it"
        );
        let text = serde_json::to_string(&record).expect("it serialises");
        assert_eq!(
            serde_json::from_str::<LaunchRecord>(&text).expect("it re-parses"),
            record
        );

        // A record written before the field existed, which is the file on disk
        // this build has to keep opening.
        let older = serde_json::json!({
            "run_id": "demo",
            "plan": "plan.json",
            "node_graph": "graphs/node-scope.yaml",
            "launcher": "claude-code",
            "session": "a-session",
            "pid": sys::pid(),
            "host": sys::hostname(),
            "started_at": "2026-08-15T00:00:00.000Z",
            "heartbeat_interval": 1800,
        });
        let read: LaunchRecord =
            serde_json::from_value(older).expect("a record predating the stamp still reads");
        assert!(read.started.is_empty());
        assert!(
            !sys::process_start_token(sys::pid())
                .expect("this host says when a process started")
                .matches(&read.started),
            "a record carrying no stamp proved a live pid"
        );

        // And a record that carries none writes none, so a build that predates
        // the field still reads what this one wrote.
        let text = serde_json::to_string(&LaunchRecord {
            started: String::new(),
            ..a_record()
        })
        .expect("it serialises");
        assert!(
            !text.contains("started\""),
            "an empty stamp reached the record: {text}"
        );
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
            pr_author_graph: String::new(),
            launcher: sys::UNKNOWN_LAUNCHER.into(),
            session: sys::UNKNOWN_LAUNCHER.into(),
            pid: 1,
            host: "h".into(),
            started: String::new(),
            started_at: sys::now_rfc3339(),
            heartbeat_interval: 1,
            dag_sets: Vec::new(),
            node_sets: Vec::new(),
            adoptions: 0,
            filters: Filters::default(),
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
            pr_author_graph: String::new(),
            launcher: "claude-code".into(),
            session: "secret-session-id".into(),
            pid: 1,
            host: "h".into(),
            started: String::new(),
            started_at: sys::now_rfc3339(),
            heartbeat_interval: 1,
            dag_sets: Vec::new(),
            node_sets: Vec::new(),
            adoptions: 0,
            filters: Filters::default(),
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
        // A launch record that is there and is not a record: absent and "present
        // as something else" are different things to tell a reader.
        fs::create_dir_all(RunPaths::under(&root, "impostor").launch())
            .expect("a launch record that is a directory");

        let index = all_runs(&root);
        let ids: Vec<String> = index.runs.iter().map(|r| r.run.clone()).collect();
        assert_eq!(ids, vec!["a-run".to_string(), "b-run".to_string()]);
        // Each directory is named with its own reason; the file never claimed to
        // be a run, so nothing is claimed about it either.
        let refused: Vec<(PathBuf, String)> = index
            .skipped
            .iter()
            .map(|root| (root.path.clone(), root.reason.clone()))
            .collect();
        assert_eq!(refused.len(), 2, "{refused:?}");
        assert_eq!(refused[0].0, root.join("impostor"));
        assert!(refused[0].1.contains("is not a file"), "{refused:?}");
        assert_eq!(refused[1].0, root.join("scratch"));
        assert!(refused[1].1.contains("no launch.json"), "{refused:?}");

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

    /// A dispatch claims the process its work is in, and gives the claim up when
    /// the dispatch ends.
    ///
    /// The registry's whole contract in one place: while a dispatch is running
    /// the run says which process it is running in and proves that pid is still
    /// that process, and the moment the dispatch ends — however it ends, because
    /// the claim is given up by being dropped — the run stops saying so. A
    /// registry that kept the entry would send a later stop at whatever the host
    /// had handed the pid to next.
    #[test]
    fn a_dispatch_claims_the_process_it_runs_in_and_gives_it_up_when_it_ends() {
        let root = scratch("dispatches");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        assert!(
            dispatches_of(&paths)
                .expect("a run that has dispatched nothing has an empty registry")
                .is_empty(),
            "a run that has dispatched nothing claimed a process"
        );

        let claim = claim_dispatch(&paths, "build", sys::pid()).expect("the dispatch is recorded");
        let recorded = dispatches_of(&paths).expect("the registry reads");
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        assert_eq!(recorded[0].node, "build");
        assert_eq!(recorded[0].pid, sys::pid());
        assert_eq!(recorded[0].host, sys::hostname());
        assert!(
            sys::process_start_token(sys::pid())
                .expect("this host says when a process started")
                .matches(&recorded[0].started),
            "the entry's stamp is not this process's own start: {recorded:?}"
        );

        drop(claim);
        assert!(
            dispatches_of(&paths)
                .expect("the registry reads")
                .is_empty(),
            "a dispatch that ended left the run claiming its process"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// Two dispatches inside one process are two entries, and each ends alone.
    ///
    /// The library backend runs a node's dispatch **in the driver**, so a run at
    /// any concurrency above one has several live dispatches sharing a pid. Keyed
    /// by pid alone they were one entry: the second overwrote the first, and
    /// whichever ended first took the survivor's registration with it — leaving a
    /// live dispatch nothing could find, which is the failure this registry
    /// exists to make impossible.
    #[test]
    fn two_dispatches_in_one_process_are_two_entries_and_each_ends_alone() {
        let root = scratch("dispatches-shared-process");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");

        let first = claim_dispatch(&paths, "first", sys::pid()).expect("the first is recorded");
        let second = claim_dispatch(&paths, "second", sys::pid()).expect("the second is recorded");
        let nodes = |paths: &RunPaths| {
            let mut named: Vec<String> = dispatches_of(paths)
                .expect("the registry reads")
                .into_iter()
                .map(|held| held.node)
                .collect();
            named.sort();
            named
        };
        assert_eq!(
            nodes(&paths),
            vec!["first".to_string(), "second".to_string()],
            "two dispatches in one process did not record two entries"
        );

        drop(first);
        assert_eq!(
            nodes(&paths),
            vec!["second".to_string()],
            "a dispatch that ended took a live one's registration with it"
        );
        drop(second);
        assert!(dispatches_of(&paths)
            .expect("the registry reads")
            .is_empty());
        fs::remove_dir_all(&root).ok();
    }

    /// A claim removes its **own** entry and never the one that replaced it.
    ///
    /// The host reissues pids, so the entry under one is this dispatch's only
    /// while the process behind it is. A claim that removed the file by name
    /// would, on the ordering that matters — a dispatch ending just as a later
    /// one starts in a pid the host has recycled — delete a live dispatch's
    /// entry and leave that process findable by nothing.
    #[test]
    fn a_claim_that_ends_leaves_a_later_dispatchs_entry_alone() {
        let root = scratch("dispatches-reused");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");

        let first = claim_dispatch(&paths, "build", sys::pid()).expect("the dispatch is recorded");
        // The same pid, claimed again by what stands in here for a later process
        // wearing it: the entry is rewritten with a start this one does not have.
        write_json(
            &paths.dispatch(sys::pid(), 0),
            &DispatchRecord {
                started: "the process that took it, which is not the first one".into(),
                ..dispatches_of(&paths).expect("the registry reads")[0].clone()
            },
        )
        .expect("the entry is rewritten");

        drop(first);
        let recorded = dispatches_of(&paths).expect("the registry reads");
        assert_eq!(
            recorded.len(),
            1,
            "a dispatch that ended removed an entry it did not write: {recorded:?}"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// Every entry, in pid order.
    ///
    /// Ordered because a teardown acts on them, and a directory listing comes in
    /// whatever order the host gives — so a run's stop would reach its processes
    /// in a different order each time it was asked.
    #[test]
    fn the_registry_reads_in_pid_order() {
        let root = scratch("dispatches-order");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        for (node, pid) in [("later", 900_u32), ("earlier", 90), ("middle", 300)] {
            write_json(
                &paths.dispatch(pid, 0),
                &DispatchRecord {
                    node: node.to_string(),
                    pid,
                    host: sys::hostname(),
                    dispatched_at: sys::now_rfc3339(),
                    started: "a start this host once reported".into(),
                },
            )
            .expect("an entry");
        }

        let read: Vec<u32> = dispatches_of(&paths)
            .expect("the registry reads")
            .iter()
            .map(|held| held.pid)
            .collect();
        assert_eq!(read, vec![90, 300, 900]);
        fs::remove_dir_all(&root).ok();
    }

    /// Every way a registry can fail to be read is a failure, and none of them
    /// is an empty registry.
    ///
    /// "Nothing is registered" and "what is registered cannot be read" are
    /// opposite answers for the caller that acts on them, and this reader's whole
    /// job is to keep them apart: the first says a run has no work running, the
    /// second says nobody knows. Each of these was once the same empty vector.
    #[test]
    fn a_registry_this_build_cannot_read_is_reported_and_never_read_as_an_empty_one() {
        let root = scratch("dispatches-unreadable");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let usable = DispatchRecord {
            node: "build".into(),
            pid: 4_242,
            host: sys::hostname(),
            dispatched_at: sys::now_rfc3339(),
            started: "a start this host once reported".into(),
        };

        for (what, entry) in [
            (
                "a record that is not JSON at all",
                "not an entry".to_string(),
            ),
            (
                "a record carrying a field this build does not know",
                serde_json::to_string(&serde_json::json!({
                    "node": usable.node,
                    "pid": usable.pid,
                    "host": usable.host,
                    "dispatched_at": usable.dispatched_at,
                    "started": usable.started,
                    "reaped_by": "a build that came later",
                }))
                .expect("an entry from a newer writer"),
            ),
            (
                "a record missing the stamp entirely",
                serde_json::to_string(&serde_json::json!({
                    "node": usable.node,
                    "pid": usable.pid,
                    "host": usable.host,
                    "dispatched_at": usable.dispatched_at,
                }))
                .expect("an entry from a writer that recorded no stamp"),
            ),
            (
                "a record whose stamp proves nothing",
                serde_json::to_string(&DispatchRecord {
                    started: String::new(),
                    ..usable.clone()
                })
                .expect("an unstamped entry"),
            ),
        ] {
            fs::write(paths.dispatch(usable.pid, 0), entry).expect("an entry");
            let refused =
                dispatches_of(&paths).expect_err(&format!("{what} was read as a registry"));
            assert!(
                refused.to_string().contains(&usable.pid.to_string()),
                "the refusal over {what} does not name what caused it: {refused}"
            );
        }

        // And the registry that is not there at all. Every run this build creates
        // has one, so its absence is something having taken it away — which is
        // not the same fact as a run that has dispatched nothing, and answering
        // it the same way is what this reader refuses to do.
        fs::remove_dir_all(paths.dispatches()).expect("the registry is taken away");
        let refused = dispatches_of(&paths)
            .expect_err("a registry that is not there was read as a run with nothing running");
        assert!(
            refused
                .to_string()
                .contains(&paths.dispatches().display().to_string()),
            "the refusal does not name the registry it could not read: {refused}"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// A dispatch this run cannot record is a dispatch this run does not run.
    ///
    /// The claim is the trust boundary, not bookkeeping around one: an entry
    /// that was not written is a process no view will show and no stop will
    /// reach, on a run whose own records say it has nothing running. So the
    /// caller is handed the failure — both ways it can happen — and ends the
    /// dispatch with it rather than running work nothing can find.
    #[test]
    fn a_dispatch_the_registry_cannot_record_is_refused() {
        let root = scratch("dispatches-unwritable");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");

        // A host that will not say when the process started: nothing could prove
        // that pid is still this run's work, so there is no entry to write.
        let reaped = sys::reaped_pid();
        let refused = claim_dispatch(&paths, "build", reaped)
            .expect_err("a dispatch nothing can stamp was recorded anyway");
        assert!(
            refused.to_string().contains(&reaped.to_string()),
            "the refusal does not name the process it could not stamp: {refused}"
        );

        // And a registry that cannot be written at all, with a file where its
        // directory has to go — a host that is otherwise perfectly healthy.
        fs::remove_dir_all(paths.dispatches()).expect("the registry is taken away");
        fs::write(paths.dispatches(), "not a directory").expect("something in the way");
        let refused = claim_dispatch(&paths, "build", sys::pid())
            .expect_err("a claim that could not be written was reported as held");
        assert!(
            refused
                .to_string()
                .contains(&paths.dispatches().display().to_string()),
            "the refusal does not name what it could not write: {refused}"
        );
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
