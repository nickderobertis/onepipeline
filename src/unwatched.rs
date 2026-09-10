//! `onepipeline unwatched` — which of this session's runs has nothing watching
//! it.
//!
//! Written for a **hook** rather than for a person. What it makes possible is
//! asking "is anything looking at the work I dispatched?" with a script instead of
//! with prose in a manager's instructions, at the end of every turn, where a
//! harness can refuse to let the turn end. Entry 66 of
//! `docs/contract-divergences.md` is the proposal it waits on and the whole
//! statement of the contract; neither is restated here.
//!
//! Everything on this path **reads**, exactly as [`crate::views`] does — and it
//! reads no run's merged event store at any point, including for a run whose
//! summary document is absent or stale. That is not an optimisation but the
//! difference between a verb that can run at the end of every turn and one that
//! cannot: the host this was written for holds around 490 run roots and 11 GB of
//! journals, and folding them took 74 seconds.
//!
//! Two costs, and they are deliberately different. **Discovery** is proportional
//! to the number of run roots, because ownership lives in each run's own launch
//! record and reading those is the only way to find the owned ones — nothing else
//! about a root is opened to decide it. Everything after discovery is
//! proportional to the runs the asked-about session owns.

use std::path::Path;

use crate::cli::UnwatchedArgs;
use crate::error::{Error, Result, EXIT_RUNS_UNWATCHED, EXIT_SUCCESS};
use crate::ledger::{self, LaunchRecord, RunPaths};
use crate::summary::RunSummary;
use crate::sys;
use crate::views;
use crate::watchers::Watchers;

/// What a reported run's line tells a caller to do about it.
const ARM_A_WATCH: &str = "watch it with: onepipeline watch";

/// `onepipeline unwatched`.
///
/// The two streams are the whole interface. One line per reported run on standard
/// output, nothing at all when there is nothing to report, and everything
/// unresolved on standard error — where it changes no status, because a run whose
/// evidence could not be read is neither an unwatched run nor a reason to refuse
/// the question that was asked about the others.
pub(crate) fn unwatched(args: &UnwatchedArgs) -> Result<i32> {
    let session = session(args)?;
    let root = ledger::runs_root();
    let mut reported: Vec<String> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    for paths in owned_by(&root, &session)? {
        match decide(&paths) {
            Decided::Settled => {}
            Decided::Undecidable(reason) => {
                unresolved.push(format!("{}: {reason}\n", paths.run));
            }
            Decided::Unsettled(summary) => {
                let watchers = Watchers::of(&paths);
                if watchers.any_live() {
                    continue;
                }
                for refused in &watchers.refused {
                    unresolved.push(format!(
                        "{}: a watcher record cannot be read, so it is not a live watch: {} — {}\n",
                        paths.run,
                        refused.path.display(),
                        refused.reason
                    ));
                }
                reported.push(format!(
                    "{:<24} {:<12} {} — {ARM_A_WATCH} {}\n",
                    paths.run,
                    views::summary_standing_word(&root, &summary),
                    watchers.why_not_watched(),
                    paths.run
                ));
            }
        }
    }
    // Sorted by run id, which is the order a listing renders its rows in: this
    // verb is read beside `runs`, and a caller scanning for an id scans one order.
    reported.sort();
    unresolved.sort();
    // llmlint: ignore-block[cli_output_contract] both streams are written here rather than
    // returned as one rendering, because the split *is* this verb's answer: the reported
    // runs are what a hook acts on and go on standard output, and everything unresolved is
    // named on standard error where it changes no exit status. `src/driver.rs` records why
    // a write to either is not an error this crate's exit codes can carry.
    eprint!("{}", unresolved.concat());
    print!("{}", reported.concat());
    // llmlint: ignore-end[cli_output_contract]
    if reported.is_empty() {
        return Ok(EXIT_SUCCESS);
    }
    Ok(EXIT_RUNS_UNWATCHED)
}

/// The session this verb is asking about.
///
/// Taken from the option as well as from the environment because the consumer is
/// a hook: it is handed the session it must ask about on standard input, and the
/// environment it runs in carries somebody else's. A session that resolves to
/// nothing is a question this verb cannot ask, and is refused rather than
/// answered as owning nothing — an operator in a plain shell is told why, and a
/// hook reads every status but the two answers as silence.
///
/// An option carrying a blank value is that same refusal rather than a
/// fall-through to the environment: `--session ""` is a caller that meant to name
/// a session and did not, and answering it out of the environment would report on
/// whichever session the hook happens to be running under — the one mistake the
/// option exists to make impossible.
fn session(args: &UnwatchedArgs) -> Result<String> {
    args.session
        .clone()
        .or_else(|| std::env::var(sys::LAUNCHER_SESSION_ENV).ok())
        .filter(|named| !named.trim().is_empty())
        .ok_or_else(|| {
            Error::Invalid(format!(
                "no session to ask about: name one with `--session`, or export {} in the \
                 environment this runs in",
                sys::LAUNCHER_SESSION_ENV
            ))
        })
}

/// Every run root under `root` whose launch record names `session`.
///
/// **Ownership is a positive claim.** A root whose launch record is absent or
/// unreadable, and one naming no session, belongs to nobody and is passed over in
/// silence: the host this was written for holds seventy run roots with no launch
/// record at all, and a verb that ran at the end of every turn and named them
/// would be noise on every turn.
///
/// A runs root that does not exist holds no runs and is not an error. One that
/// exists and cannot be read **is** — it is the question this verb cannot ask,
/// and answering it as "nothing is unwatched" is the silence the whole verb exists
/// to end.
fn owned_by(root: &Path, session: &str) -> Result<Vec<RunPaths>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(Error::Ledger {
                path: root.to_path_buf(),
                source: error,
            })
        }
    };
    let mut owned = Vec::new();
    for entry in entries.flatten() {
        let paths = RunPaths::under(root, &entry.file_name().to_string_lossy());
        // The launch record and nothing else. Read leniently, exactly as every
        // other reader of it: a record this build cannot parse names no session,
        // and a run belonging to nobody is passed over.
        let Some(launch) = ledger::read_json_opt::<LaunchRecord>(&paths.launch()) else {
            continue;
        };
        if launch.owned_by(session) {
            owned.push(paths);
        }
    }
    Ok(owned)
}

/// What this build could establish about one run's settlement.
enum Decided {
    /// It has stopped or its graph is complete, proved from a summary document
    /// whose stamp matches its journal as it stands. Not reported, however long it
    /// has been unwatched.
    Settled,
    /// It is still going, so far as its own document says.
    ///
    /// Boxed because the document is the largest thing here by two orders of
    /// magnitude, and this value is built once per owned run: an unboxed variant
    /// would make every settled and undecidable run pay for the one that is
    /// reported.
    Unsettled(Box<RunSummary>),
    /// Nothing established either, and watching it would not clear that.
    Undecidable(String),
}

/// Read one run's settlement out of its **summary document and nothing else**,
/// never folding to answer it.
///
/// The stamp is required **only for exclusion**, and that asymmetry is the whole
/// freshness rule. A run that has stopped writing has a current document, so
/// requiring the stamp costs a settled run nothing — while a document that is
/// behind its journal is exactly what a run *still recording* looks like, and
/// treating that as proof of settlement is how the one run this verb exists to
/// find would be dropped.
fn decide(paths: &RunPaths) -> Decided {
    let summary = match std::fs::read_to_string(paths.summary())
        .map_err(|error| format!("{error}"))
        .and_then(|text| {
            serde_json::from_str::<RunSummary>(&text).map_err(|error| format!("{error}"))
        }) {
        Ok(summary) => summary,
        // Most often an old settled run whose document is gone, and blocking on it
        // would never clear by watching it — so it is named and passed over rather
        // than reported.
        Err(reason) => {
            return Decided::Undecidable(format!(
                "its settlement cannot be decided: its summary document could not be read: \
                 {reason}"
            ))
        }
    };
    if summary.run_id != paths.run {
        return Decided::Undecidable(format!(
            "its settlement cannot be decided: its summary document is run '{}'",
            summary.run_id
        ));
    }
    if (summary.stop_recorded || summary.graph_complete) && stamped(paths, &summary) {
        return Decided::Settled;
    }
    Decided::Unsettled(Box::new(summary))
}

/// Whether a document accounts for the journal beside it as it stands now.
fn stamped(paths: &RunPaths, summary: &RunSummary) -> bool {
    let Ok(about) = std::fs::metadata(paths.journal()) else {
        // A journal that is not there stamps as `(0, 0)`, which is what the writer
        // records for a run whose first record has not landed — so a document
        // carrying it describes that run exactly.
        return (summary.journal_len, summary.journal_mtime_ms) == (0, 0);
    };
    let modified = about
        .modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    (summary.journal_len, summary.journal_mtime_ms) == (about.len(), modified)
}
