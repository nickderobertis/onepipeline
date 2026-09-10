//! `onepipeline unwatched` — which of this session's runs has nothing watching
//! it.
//!
//! What the verb promises, and why it exists at all, is entry 68 of
//! `docs/contract-divergences.md`: the proposal it waits on. It is not restated
//! here, on the terms [`crate::watch`] keeps beside its own entry.
//!
//! The one thing worth saying beside the code is the cost, because it is a
//! property of *this* module rather than of the contract: everything here
//! **reads**, and it reads no run's merged event store at any point — including
//! for a run whose summary document is absent or stale, which is where a reader
//! is tempted to fold. Discovery is proportional to the number of run roots,
//! because ownership lives in each run's own launch record; everything after it is
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
    let owned = discover_owned_runs(&root, &session)?;
    let mut unresolved: Vec<String> = owned.unresolved;
    for paths in owned.runs {
        match decide(&paths) {
            Decided::Settled => {}
            Decided::Undecidable(reason) => {
                unresolved.push(format!("{}: {reason}\n", paths.run));
            }
            Decided::NotProvenSettled(summary) => {
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
    // named on standard error where it changes no exit status. A verb whose answer is two
    // streams cannot hand one string to a caller that prints it.
    // llmlint: ignore-block[no_panics_on_recoverable_errors] how this binary writes a view
    // to a stream is one decision for all of them rather than this verb's, and
    // `src/driver.rs` is where it is recorded and why: the exit codes are spent — `0`/`1`/
    // `2` are `reply`'s verdicts and `3` is "nothing is driving the run" — so a write this
    // verb returned as an error would have to carry a code that already means something
    // else, and this one has two answers of its own on top of those. Making a closed pipe a
    // first-class outcome is a change to the whole command surface and to the contract's
    // exit codes, which belongs with the planner who owns them rather than in the one verb
    // a diff happens to add.
    eprint!("{}", unresolved.concat());
    print!("{}", reported.concat());
    // llmlint: ignore-end[no_panics_on_recoverable_errors]
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
// llmlint: ignore-block[invalid_states_unrepresentable] a launching session is a `String`
// wherever it exists in this crate — `sys::launching_session` answers one, `LaunchRecord`
// records one, and `ledger::owned_by` compares two — and `docs/contract.md` names no
// `Session` type, so a newtype here would be converted straight back at the one place this
// value is used. What can be made unrepresentable is what this function does: the absence
// this verb refuses is a `Result` rather than a blank string handed on, so no caller can
// ask about a session nobody named.
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
// llmlint: ignore-end[invalid_states_unrepresentable]

/// Walk the runs root, and answer with every run root under it whose launch record
/// names `session`.
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
fn discover_owned_runs(root: &Path, session: &str) -> Result<Discovered> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Discovered::default())
        }
        Err(error) => {
            return Err(Error::Ledger {
                path: root.to_path_buf(),
                source: error,
            })
        }
    };
    let mut owned = Discovered::default();
    for entry in entries {
        // llmlint: ignore-block[changed_behavior_has_e2e] an entry the filesystem lists and
        // then refuses to describe is a host condition no portable journey can set —
        // `src/ledger.rs`'s own listing carries the same suppression for the same arm. What
        // it must not do is what it did before this arm existed: a scan that dropped it
        // silently would report an *incomplete* look at the root as a complete one, which
        // for this verb is a run nobody is watching passed over without a word.
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                owned.unresolved.push(format!(
                    "{}: an entry under the runs root cannot be read, so this look at it is \
                     incomplete: {error}\n",
                    root.display()
                ));
                continue;
            }
        };
        // llmlint: ignore-end[changed_behavior_has_e2e]
        // Asked for as text rather than converted to it. A lossy conversion would
        // put a replacement character where a byte was and then read *that* path —
        // which is another directory, or none — where what a name that is not text
        // means is that this build cannot name the run. Neither it nor a name that
        // is text and is not a run id can be one this verb reports: the line would
        // name a run nobody could type at the verb it tells them to type it at. So
        // both are passed over with the roots that belong to nobody.
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        // And it has to be a run id, which is the boundary every externally
        // supplied one in this crate crosses: what is joined onto the runs root
        // here is a *stranger's* directory name, and a reported line names it back
        // to a caller that will type it at another verb.
        if !ledger::is_valid_run_id(&name) {
            continue;
        }
        let paths = RunPaths::under(root, &name);
        // The launch record and nothing else. Read leniently, exactly as every
        // other reader of it: a record this build cannot parse names no session,
        // and a run belonging to nobody is passed over.
        let Some(launch) = ledger::read_json_opt::<LaunchRecord>(&paths.launch()) else {
            continue;
        };
        if launch.owned_by(session) {
            owned.runs.push(paths);
        }
    }
    Ok(owned)
}

/// What discovery found: the runs the session owns, and what it could not resolve.
///
/// The second half is why this is a value rather than a list. Discovery is a walk
/// over somebody else's directory, and an entry it is refused is **not** the same
/// fact as a root that belongs to nobody: one is a run this verb decided about, and
/// the other is a run it never saw. A scan that dropped the second silently would
/// report an incomplete look at the root as a complete one — and for this verb, an
/// incomplete look is exactly how a run nobody is watching goes unmentioned.
#[derive(Default)]
struct Discovered {
    /// The run roots whose launch record names the resolved session.
    runs: Vec<RunPaths>,
    /// What could not be resolved, each already worded for standard error.
    unresolved: Vec<String>,
}

/// What this build could establish about one run's settlement.
enum Decided {
    /// It has stopped or its graph is complete, proved from a summary document
    /// whose stamp matches its journal as it stands. Not reported, however long it
    /// has been unwatched.
    Settled,
    /// Nothing **proved** it settled, which is not the same as proving it is
    /// still going and is deliberately not named as though it were: a document
    /// that says a run stopped while standing behind its journal is in here too,
    /// because a stale document is not proof of anything it says.
    ///
    /// Boxed because the document is the largest thing here by two orders of
    /// magnitude, and this value is built once per owned run: an unboxed variant
    /// would make every settled and undecidable run pay for the one that is
    /// reported.
    NotProvenSettled(Box<RunSummary>),
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
    Decided::NotProvenSettled(Box::new(summary))
}

/// Whether a document accounts for the journal beside it as it stands now.
fn stamped(paths: &RunPaths, summary: &RunSummary) -> bool {
    let about = match std::fs::metadata(paths.journal()) {
        Ok(about) => about,
        // A journal that is not there stamps as `(0, 0)`, which is what the writer
        // records for a run whose first record has not landed — so a document
        // carrying it describes that run exactly.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (summary.journal_len, summary.journal_mtime_ms) == (0, 0)
        }
        // Anything else is this host declining to say what the journal is, which is
        // **not** the same fact and must not be read as it: a stamp that cannot be
        // compared has not matched, so the run is not excluded and is reported if
        // nothing is watching it. That is where every unknown on this path resolves.
        Err(_) => return false,
    };
    // The same rule as the metadata above, and for the same reason: a modification
    // time this host will not give, or one it gives from before the epoch, is a
    // stamp that cannot be compared — and a stamp that cannot be compared has not
    // matched. Read as a `0` it would agree with a document carrying `0`, which is
    // what a run whose journal is *not there* carries, and would exclude a run on
    // the strength of a reading nobody took.
    let Some(modified) = about
        .modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| u64::try_from(since.as_millis()).ok())
    else {
        return false;
    };
    (summary.journal_len, summary.journal_mtime_ms) == (about.len(), modified)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::error::EXIT_REFUSED;
    use crate::watchers::{WatchStanding, WatcherRecord, WATCHER_SCHEMA_VERSION};
    use std::collections::BTreeSet;

    /// Entry 66 of the divergence record, which is where this verb and the record
    /// it reads are *proposed*.
    ///
    /// The tests below hold that proposal to what this build actually does, in
    /// **both** directions, on the terms entries 39, 41, 56 and 58 already hold
    /// their own: a field the record grows and the entry does not name is surface
    /// nobody ruled on, and a name the entry keeps after the code dropped it is a
    /// promise to a person deciding about something that is not there.
    pub(crate) fn divergence_entry() -> String {
        let record = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("docs")
                .join("contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split_once("\n## 68.")
            .expect("this verb is recorded under entry 68")
            .1
            .to_string();
        entry
            .split_once("\n## ")
            .map_or(entry.clone(), |(head, _)| head.to_string())
    }

    /// The block the entry states its inventory in, as a value.
    pub(crate) fn block() -> serde_json::Value {
        let entry = divergence_entry();
        let block = entry
            .split_once("```json")
            .expect("entry 68 carries the json block these tests drive")
            .1
            .split_once("```")
            .expect("the block is fenced")
            .0;
        serde_json::from_str(block).expect("entry 68's block is JSON")
    }

    /// Every standing this build reads a record as, exhaustively.
    ///
    /// The match is what makes it exhaustive: a variant added to the enum and left
    /// out of this list fails to compile here rather than quietly leaving the
    /// entry naming five of six answers.
    fn every_standing() -> [WatchStanding; 7] {
        let all = [
            WatchStanding::Live,
            WatchStanding::AnotherRun,
            WatchStanding::AnotherHost,
            WatchStanding::ProcessGone,
            WatchStanding::AwaitingItsParent,
            WatchStanding::NotThatProcess,
            WatchStanding::Unproven,
        ];
        for standing in all {
            match standing {
                WatchStanding::Live
                | WatchStanding::AnotherRun
                | WatchStanding::AnotherHost
                | WatchStanding::ProcessGone
                | WatchStanding::AwaitingItsParent
                | WatchStanding::NotThatProcess
                | WatchStanding::Unproven => {}
            }
        }
        all
    }

    /// The record's schema version and field inventory are the type's own.
    ///
    /// Built through the type rather than listed, exactly as the summary
    /// document's inventory is: a field added to `WatcherRecord` does not compile
    /// here until it is given a value, so what this answers is the whole inventory
    /// a consumer can read.
    #[test]
    fn the_divergence_entry_names_the_record_this_build_writes() {
        let block = block();
        assert_eq!(
            block["schema_version"].as_u64(),
            Some(u64::from(WATCHER_SCHEMA_VERSION)),
            "entry 68 states a schema version this build does not write"
        );
        let written = WatcherRecord {
            schema_version: WATCHER_SCHEMA_VERSION,
            run_id: "gated".into(),
            pid: std::num::NonZeroU32::new(4_242).expect("a pid"),
            host: "a-host".into(),
            started: "linux-proc-stat:1".into(),
            began_at: "2026-01-01T00:00:00.000Z".into(),
        };
        let fields: BTreeSet<String> = serde_json::to_value(&written)
            .expect("a record is an object")
            .as_object()
            .expect("a record is an object")
            .keys()
            .cloned()
            .collect();
        let named: BTreeSet<String> =
            serde_json::from_value(block["fields"].clone()).expect("entry 68 names the fields");
        assert_eq!(
            named, fields,
            "entry 68's inventory is not the record this build writes"
        );
        // And the record round-trips, which is what makes the inventory a
        // consumer's to read rather than one this build only writes.
        let read: WatcherRecord =
            serde_json::from_str(&serde_json::to_string(&written).expect("a record serializes"))
                .expect("a record this build wrote reads back");
        assert_eq!(read, written);
    }

    /// A record at any other version is refused rather than read as this one.
    #[test]
    fn a_record_at_another_schema_version_is_refused() {
        let mut document = serde_json::json!({
            "schema_version": WATCHER_SCHEMA_VERSION + 1,
            "run_id": "gated",
            "pid": 4_242,
            "host": "a-host",
            "started": "linux-proc-stat:1",
            "began_at": "2026-01-01T00:00:00.000Z",
        });
        let refusal = serde_json::from_value::<WatcherRecord>(document.clone())
            .expect_err("a version this build does not write is refused");
        assert!(
            refusal.to_string().contains("schema_version"),
            "the refusal does not say what it refused: {refusal}"
        );
        // And a key this build does not know is refused too, rather than dropped:
        // a record read half by this build's meaning and half by another's is what
        // the version exists to stop.
        document["schema_version"] = serde_json::json!(WATCHER_SCHEMA_VERSION);
        document["watching_since_tick"] = serde_json::json!(1);
        serde_json::from_value::<WatcherRecord>(document).expect_err("an unknown key is refused");
    }

    /// The entry names exactly the answers this build reads a record as.
    #[test]
    fn the_divergence_entry_names_every_standing_this_build_reads() {
        let named: Vec<String> =
            serde_json::from_value(block()["standings"].clone()).expect("entry 68 names them");
        let read: Vec<String> = every_standing()
            .iter()
            .map(|standing| standing.as_str().to_string())
            .collect();
        assert_eq!(
            named, read,
            "entry 68 names a different set of standings than this build reads"
        );
        // One of them is the live one, and it is the only one.
        let live = every_standing()
            .into_iter()
            .filter(|standing| standing.is_live())
            .count();
        assert_eq!(live, 1, "exactly one standing reads as watching");
    }

    /// The entry proposes a command **schema**, and clap is what a caller is
    /// actually given.
    #[test]
    fn the_divergence_entry_proposes_exactly_the_options_this_build_offers() {
        use clap::CommandFactory;

        let named: BTreeSet<String> =
            serde_json::from_value(block()["options"].clone()).expect("entry 68 names them");
        let offered: BTreeSet<String> = crate::cli::Cli::command()
            .get_subcommands()
            .find(|sub| sub.get_name() == "unwatched")
            .expect("the binary offers `unwatched`")
            .get_arguments()
            .filter_map(|arg| arg.get_long().map(str::to_string))
            .collect();
        assert_eq!(
            named, offered,
            "entry 68 proposes a different set of options than this build offers"
        );
        // And the entry's own prose spells the command a caller types, which is
        // the part an operator reads rather than the block.
        assert!(
            divergence_entry().contains("onepipeline unwatched [--session <ID>]"),
            "entry 68 does not spell the command it proposes"
        );
    }

    /// The three statuses the entry states are the three this build returns.
    ///
    /// Named against the constants rather than against the numbers alone: the
    /// whole reason `unwatched` has a status of its own is that a caller branches
    /// on it, and a mapping quietly swapped here would leave a hook reading "no run
    /// is unwatched" off the answer that says one is.
    #[test]
    fn the_divergence_entry_names_the_statuses_this_verb_returns() {
        let block = block();
        assert_eq!(
            block["exit_reported"].as_i64(),
            Some(i64::from(EXIT_RUNS_UNWATCHED))
        );
        assert_eq!(
            block["exit_none_reported"].as_i64(),
            Some(i64::from(EXIT_SUCCESS))
        );
        assert_eq!(
            block["exit_refused"].as_i64(),
            Some(i64::from(EXIT_REFUSED))
        );
        assert_ne!(
            EXIT_RUNS_UNWATCHED, EXIT_SUCCESS,
            "the answer that a run is unwatched cannot be told from the answer that none is"
        );
        assert_ne!(
            EXIT_RUNS_UNWATCHED, EXIT_REFUSED,
            "the answer that a run is unwatched cannot be told from a refusal"
        );
        // What a caller actually gets is asserted where the environment is this
        // journey's own to set: `tests/e2e/unwatched.rs` drives both refusals
        // through the compiled binary with nothing in its environment naming a
        // session. Asserting it here would be a claim about whatever session the
        // process running the tests happens to have been launched under.
    }
}
