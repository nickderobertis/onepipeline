//! The **watcher record**: the evidence that a run is being watched.
//!
//! One document per live watch, under the watched run's own root, because
//! [`crate::watch`] takes no lock a writer needs and any number of watches may
//! sit on one run at once. Its whole purpose is to be read back by somebody
//! *else* — the question "is anything watching this run?" is asked by a process
//! that is not watching it — so the record carries what a second party needs to
//! decide that from the host as it stands: which process, on which machine, and
//! the start token that says the pid is still that process.
//!
//! # The asymmetry is inverted here, deliberately
//!
//! [`sys::process_may_be_live`] resolves every unknown toward "still working",
//! because for a *driver* the worse error is reporting live work as dead. For a
//! *watch* the worse error is the other one: a run reported watched while nothing
//! is watching it is precisely the silence this record exists to end, and it
//! costs hours — where the opposite error costs one re-armed watch. So every
//! unknown here resolves toward **not watched**: a record this build cannot read
//! is not a live watch, a host that will not report a start token leaves a record
//! that is never a live watch, and a pid this host will not describe is not a
//! watcher.
//!
//! # No heartbeat, no expiry, no cleanup step
//!
//! Nothing stands between a watcher dying and its run reading unwatched. The
//! deaths that matter have no clean exit — killed, crashed, out of memory,
//! terminal closed — so liveness is decided by reading the host at the moment of
//! the question and never by an interval or a sweep. Removing the record on a
//! clean exit is permitted ([`Armed`]) and is never relied upon; a writer arming
//! a watch also removes the records it has itself **proved** are not live, so a
//! long-watched run does not accumulate them without bound, and it removes
//! nothing else. The reading side removes nothing at all, exactly as everything
//! in [`crate::views`] reads.

use std::num::NonZeroU32;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ledger::{self, RunPaths, Skipped};
use crate::sys;

/// The schema version of the watcher record.
///
/// The same compatibility statement the summary document makes, and the reason
/// this document is read **closed** — `deny_unknown_fields` — while the launch
/// record beside it is read permissively: a version this build does not write is
/// refused, and a refused record is not a live watch. That resolves in the safe
/// direction here, which is what makes refusing it affordable at all: a build
/// that met a record it did not understand and served it anyway would report a
/// run as watched out of fields that mean something else.
pub const WATCHER_SCHEMA_VERSION: u32 = 1;

/// Read the version, refusing a record this build cannot honestly read.
fn this_version<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<u32, D::Error> {
    let found = u32::deserialize(reader)?;
    if found != WATCHER_SCHEMA_VERSION {
        return Err(serde::de::Error::custom(format!(
            "watcher schema_version {found}, and this build reads {WATCHER_SCHEMA_VERSION}"
        )));
    }
    Ok(found)
}

/// One live watch, as the process taking it wrote itself down.
///
/// Every field is required. There is no absence to be lenient about: a record
/// missing any of these could not be decided against the host at all, and a
/// record that cannot be decided is not a live watch — so accepting one would
/// only move the refusal from the reader to the verdict.
// llmlint: ignore[invalid_states_unrepresentable] the run id, the host and the two stamps
// are `String`s for the reason `src/ledger.rs`'s own file-level suppression states, and
// this is that file's records read back: `docs/contract.md` names no `RunId`, no `Host` and
// no timestamp type, so a newtype here would be a public vocabulary the contract did not
// ask for. What can be made unrepresentable is: `schema_version` is refused by the
// deserializer, `pid` is a `NonZeroU32` rather than a checked `u32`, and `started` is
// compared only through `sys::StartToken::matches`, which is what carries the rule that an
// empty one never matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatcherRecord {
    /// The record's own version, so a reader can refuse one it does not
    /// understand. See [`WATCHER_SCHEMA_VERSION`].
    #[serde(deserialize_with = "this_version")]
    pub schema_version: u32,
    /// The run this watch is of.
    ///
    /// Carried rather than inferred from where the file sits, and checked against
    /// it: a record whose `run_id` is not the run whose directory it is in is not
    /// a watch of that run, however it got there — a copied run root, a restored
    /// backup, a record written by a build pointed at another ledger.
    pub run_id: String,
    /// The watching process.
    pub pid: NonZeroU32,
    /// The host that pid is meaningful on, as [`sys::hostname`] reads it.
    ///
    /// A pid means nothing across machines, so a record naming another host is
    /// never a live watch here — which is the inverted asymmetry the module
    /// documentation states, and the opposite of what a *driver*'s liveness
    /// reading does with the same fact.
    pub host: String,
    /// That process's start token, as [`sys::process_start_token`] reads it, and
    /// **empty** where this host would not say.
    ///
    /// The half of the proof a pid cannot give: a pid is reused, so a record
    /// naming one is evidence about the process that took it only while that
    /// process still holds it. Empty never matches, so a host that will not
    /// report a token leaves records that are never live watches — every run on
    /// such a host reads unwatched, which is the honest answer rather than a
    /// defect and resolves in the safe direction.
    pub started: String,
    /// When the watch began, as an RFC 3339 string.
    ///
    /// For a person reading the directory. It decides nothing: an interval is
    /// exactly what liveness here is never read from.
    pub began_at: String,
}

/// Whether one recorded watch is watching its run **now**, and where it is not,
/// why not.
///
/// Six answers rather than a flag, because a reader that renders a run's watchers
/// has to say which of them it is: a record naming another host, one whose
/// process this host has proved is gone, and one whose pid has since been handed
/// to something else are three different things to tell an operator, and only one
/// of them means the watch was ever running here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WatchStanding {
    /// The process it names is watching the run.
    Live,
    /// Its `run_id` is not the run whose directory it sits in, so it is not a
    /// watch of this run at all.
    AnotherRun,
    /// It names another host, where its pid means nothing.
    AnotherHost,
    /// This host has proved the process is gone.
    ProcessGone,
    /// The process has terminated and is only waiting for its parent to reap it.
    ///
    /// Not folded into [`ProcessGone`](Self::ProcessGone), because it is the one
    /// death that answers every other reading as life — see
    /// [`sys::process_terminated_awaiting_parent`] — and it is the state a watch
    /// killed by a parent that goes on running sits in.
    AwaitingItsParent,
    /// A start token read now is not the one recorded: the pid has been handed to
    /// another process, the record carries no token at all, or this host will not
    /// say. All three are the same fact to a reader — nothing here proves this
    /// process is the one that wrote the record.
    NotThatProcess,
}

impl WatchStanding {
    /// Whether this standing is a watch that is watching.
    pub fn is_live(self) -> bool {
        matches!(self, Self::Live)
    }

    /// The phrase a view puts on the line, in the words a reader acts on.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "watching",
            Self::AnotherRun => "its record names another run",
            Self::AnotherHost => "its record names another host",
            Self::ProcessGone => "its process is gone",
            Self::AwaitingItsParent => "its process ended and is waiting to be reaped",
            Self::NotThatProcess => "its pid is not the process that recorded it",
        }
    }

    /// Whether this build **proved** the recorded process is not watching, on
    /// this host and for this run.
    ///
    /// What a writer may remove, and nothing else: a record naming another host
    /// or another run is not proved dead but merely not ours to judge, and an
    /// unreadable one is not judged at all.
    fn proved_gone(self) -> bool {
        matches!(
            self,
            Self::ProcessGone | Self::AwaitingItsParent | Self::NotThatProcess
        )
    }
}

/// One record under a run's watcher directory, and what this host makes of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    /// The record as it was written.
    pub record: WatcherRecord,
    /// Whether the process it names is watching the run now.
    pub standing: WatchStanding,
}

/// Every watch recorded on one run, and every record that could not be read.
///
/// The two halves are returned together for the reason a run listing keeps its
/// own [`Skipped`] beside its rows: a reader told only what
/// could be read cannot tell an incomplete answer from a negative one. Here that
/// distinction is the whole point of the surface — a record this build cannot
/// read is **not** a live watch, so a run whose watcher evidence is incomplete
/// reads unwatched, and the reader is owed the reason rather than left to infer
/// it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Watchers {
    /// The records that read, in file-name order.
    pub watches: Vec<Watch>,
    /// The records that did not, each with the reason it was refused.
    pub refused: Vec<Skipped>,
}

impl Watchers {
    /// Read every watcher record on one run, deciding each against this host now.
    pub fn of(paths: &RunPaths) -> Self {
        let mut held = Self::default();
        for (path, read) in records(paths) {
            match read {
                Ok(record) => {
                    let standing = standing_of(&record, &paths.run);
                    held.watches.push(Watch { record, standing });
                }
                Err(reason) => held.refused.push(Skipped { path, reason }),
            }
        }
        held
    }

    /// Whether anything is watching the run.
    pub fn any_live(&self) -> bool {
        self.watches.iter().any(|watch| watch.standing.is_live())
    }

    /// Why nothing is watching the run, in the words a view puts on its line.
    ///
    /// Composed from what was actually found rather than from a fixed phrase,
    /// because the three states a reader acts on differently all read as
    /// "unwatched": a run nobody ever armed a watch on, one whose watcher died,
    /// and one whose watcher evidence this build could not read at all.
    pub fn why_not_watched(&self) -> String {
        if self.watches.is_empty() && self.refused.is_empty() {
            return "nothing has recorded a watch on it".to_string();
        }
        let mut said: Vec<String> = self
            .watches
            .iter()
            .map(|watch| format!("pid {}: {}", watch.record.pid, watch.standing.as_str()))
            .collect();
        said.extend(
            self.refused
                .iter()
                .map(|refused| format!("a record that cannot be read: {}", refused.reason)),
        );
        said.join("; ")
    }
}

/// Every entry under one run's watcher directory, in file-name order, each read
/// as a record or refused with a reason.
///
/// One walk behind both the reader and the writer's sweep, so what a sweep may
/// remove is decided by exactly the reading a reader would have taken of it —
/// and so the file a record came from is in hand for the one caller that needs
/// it, without putting a path on the surface a consumer renders.
///
/// A directory that is not there holds no watches and is no refusal: a run
/// nothing has ever watched never had one written.
fn records(paths: &RunPaths) -> Vec<(PathBuf, Result<WatcherRecord, String>)> {
    let dir = paths.watchers();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        // llmlint: ignore-block[changed_behavior_has_e2e] a directory that exists and will
        // not open, and an entry the filesystem lists and then refuses to name, are host
        // conditions no portable journey can set — `src/ledger.rs`'s own listing carries
        // the same suppression for the same two arms. What a user reaches is a *record*
        // that cannot be read, and that is driven through the binary in
        // `tests/e2e/unwatched.rs`.
        Err(error) => {
            return vec![(
                dir,
                Err(format!("the watcher directory cannot be read: {error}")),
            )]
        }
    };
    let mut read: Vec<(PathBuf, Result<WatcherRecord, String>)> = entries
        .map(|entry| match entry {
            Ok(entry) => {
                let path = entry.path();
                let record = read_record(&path);
                (path, record)
            }
            Err(error) => (
                dir.clone(),
                Err(format!(
                    "an entry under the watcher directory cannot be read: {error}"
                )),
            ),
        })
        .collect();
    // llmlint: ignore-end[changed_behavior_has_e2e]
    read.sort_by(|a, b| a.0.cmp(&b.0));
    read
}

/// One record, or the reason it is not one.
///
/// Every failure is a *reason* rather than an absence, because a record that
/// cannot be read is not a live watch and the reader is owed the difference
/// between a directory holding nothing and a directory holding something this
/// build refused.
fn read_record(path: &std::path::Path) -> Result<WatcherRecord, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("{error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("{error}"))
}

/// What this host makes of one record, over the six conditions in order.
///
/// The order is not incidental. The run and the host come first because a pid
/// read on the wrong ledger or the wrong machine is not evidence at all; the
/// zombie reading comes after the pid check and before the token, because it is
/// the one state both of its neighbours answer as life.
fn standing_of(record: &WatcherRecord, run: &str) -> WatchStanding {
    if record.run_id != run {
        return WatchStanding::AnotherRun;
    }
    if record.host != sys::hostname() {
        return WatchStanding::AnotherHost;
    }
    let pid = record.pid.get();
    if !sys::process_may_be_live(pid) {
        return WatchStanding::ProcessGone;
    }
    if sys::process_terminated_awaiting_parent(pid) {
        return WatchStanding::AwaitingItsParent;
    }
    match sys::process_start_token(pid) {
        Some(token) if token.matches(&record.started) => WatchStanding::Live,
        _ => WatchStanding::NotThatProcess,
    }
}

/// The record one live watch keeps under the run it is watching, removed when the
/// watch returns.
///
/// A guard rather than two calls, so the one path that *can* clean up does — and
/// nothing depends on it: a watch killed, crashed, or left on a closed terminal
/// runs no destructor, which is exactly why liveness is decided by reading the
/// host rather than by the record's presence.
pub(crate) struct Armed {
    /// The document this watch wrote, or nothing where it could not write one.
    path: Option<PathBuf>,
}

impl Armed {
    /// Record that this process is watching `paths`, sweeping away the records
    /// this host can prove are not live watches.
    ///
    /// **Best effort, and silent.** A runs root this process may not write costs
    /// a reader the knowledge that this watch exists, and costs the watch itself
    /// nothing — so a failure here neither refuses the verb nor writes to a
    /// stream a supervisor is reading events on. The sweep is what keeps a run
    /// that has been watched a thousand times from holding a thousand records,
    /// and it removes only what [`WatchStanding::proved_gone`] admits.
    pub(crate) fn arm(paths: &RunPaths) -> Self {
        Self::sweep(paths);
        let pid = sys::pid();
        let record = WatcherRecord {
            schema_version: WATCHER_SCHEMA_VERSION,
            run_id: paths.run.clone(),
            // A pid of zero is not a process on any platform this builds for, and
            // this one is asking about itself. `1` keeps the type honest where a
            // host answers absurdly rather than putting a checked `u32` back on
            // the record.
            pid: NonZeroU32::new(pid).unwrap_or(NonZeroU32::MIN),
            host: sys::hostname(),
            started: sys::process_start_token(pid)
                .map(|token| token.recorded().to_string())
                .unwrap_or_default(),
            began_at: sys::now_rfc3339(),
        };
        let path = paths.watcher(pid, &nonce());
        Self {
            path: ledger::write_json(&path, &record).ok().map(|()| path),
        }
    }

    /// Remove the records under this run that this host can prove are not live
    /// watches, and nothing else.
    ///
    /// A record it could not read is left exactly where it is. This process has
    /// proved nothing about it, and a writer that swept what it could not read
    /// would be deleting another build's evidence to tidy its own directory.
    fn sweep(paths: &RunPaths) {
        for (path, read) in records(paths) {
            if read.is_ok_and(|record| standing_of(&record, &paths.run).proved_gone()) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

impl Drop for Armed {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A value unique to one watch, as at least eight hexadecimal characters.
///
/// Unique per **watch** rather than per process, and that is the whole reason it
/// exists: two concurrent watches on one run must not overwrite one another's
/// record, and a pid the kernel has handed round again must not be mistaken for
/// its predecessor by file name. Both halves are needed — the counter tells two
/// watches armed in the same nanosecond of one process apart, and the clock tells
/// this process's watches from those of whatever held the pid before it.
///
/// Staleness is never decided by a name. What this buys is only that two records
/// are two files.
fn nonce() -> String {
    static MINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = MINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        });
    format!("{nanos:016x}{seq:04x}")
}
