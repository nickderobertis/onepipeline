//! `onepipeline unwatched` — which of this session's runs has nothing watching
//! it.
//!
//! What the verb promises, and why it exists at all, is entry 68 of
//! `docs/contract-divergences.md`: the proposal it waits on. The wake budget,
//! the closure rule and the acknowledgement are entry 98's. Neither is restated
//! here, on the terms [`crate::watch`] keeps beside its own entry.
//!
//! The one thing worth saying beside the code is the cost, because it is a
//! property of *this* module rather than of the contract: the question
//! **reads** — `--acknowledge` is the one write, beside the run and never to
//! its journal — and it reads no run's merged event store — including for a run
//! whose summary document is absent or stale, which is where a reader is tempted
//! to fold — with one exception, stated at [`decide`]: a document the previous
//! release wrote is folded once to bring it to this build's schema, and the
//! document that fold leaves is current, so the next question costs nothing
//! again. Discovery is proportional to the number of run roots, because ownership
//! lives in each run's own launch record; everything after it is proportional to
//! the runs the asked-about session owns.

use std::num::NonZeroU64;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cli::UnwatchedArgs;
use crate::error::{Error, Result, EXIT_RUNS_UNWATCHED, EXIT_SUCCESS};
use crate::ledger::{self, LaunchRecord, RunPaths};
use crate::summary::RunSummary;
use crate::sys;
use crate::views;
use crate::watchers::{Leases, Wake};

/// What a reported run's line tells a caller to do about it.
const ARM_A_WATCH: &str = "watch it with: onepipeline watch";

/// How long a session may go without being woken, and where that was said.
///
/// Where matters because it decides the command a reported line names: a
/// budget the environment sets is also `watch`'s own default `--timeout`, so a
/// bare `onepipeline watch RUN` meets it, while one given as a flag has to be
/// spelled on the watch too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeBudget {
    seconds: NonZeroU64,
    said: Said,
}

/// Where a [`WakeBudget`] was said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Said {
    /// As `--wake-budget`, which a watch has to be told as well.
    Flag,
    /// As `ONEPIPELINE_WAKE_BUDGET`, which is `watch`'s own default too.
    Environment,
}

impl WakeBudget {
    /// A budget given as `--wake-budget`.
    pub const fn given(seconds: NonZeroU64) -> Self {
        Self {
            seconds,
            said: Said::Flag,
        }
    }

    /// The budget `ONEPIPELINE_WAKE_BUDGET` names, or none where it is unset.
    ///
    /// # Errors
    ///
    /// The variable set to anything but a positive whole number of seconds,
    /// refused naming it.
    pub fn from_environment() -> Result<Option<Self>> {
        crate::cli::wake_budget_from_environment()
            .map(|named| {
                named.map(|seconds| Self {
                    seconds,
                    said: Said::Environment,
                })
            })
            .map_err(Error::Invalid)
    }

    /// The flag's budget, else the environment's, else none.
    ///
    /// # Errors
    ///
    /// A flag of `0`, which the binary's parser already refuses; otherwise see
    /// [`from_environment`](Self::from_environment), which is asked only where
    /// no flag was given.
    pub fn resolved(flag: Option<u64>) -> Result<Option<Self>> {
        match flag {
            Some(seconds) => NonZeroU64::new(seconds)
                .map(|seconds| Some(Self::given(seconds)))
                .ok_or_else(|| {
                    Error::Invalid(
                        "a wake budget of 0 seconds wakes nobody: `--wake-budget` is a positive \
                         whole number of seconds"
                            .to_owned(),
                    )
                }),
            None => Self::from_environment(),
        }
    }

    /// The budget, in seconds.
    pub const fn seconds(&self) -> u64 {
        self.seconds.get()
    }

    /// The `--wake-budget` a person asking by hand passes to meet this budget,
    /// leading space included, or nothing where the environment already says it.
    pub(crate) fn flag(budget: Option<Self>) -> String {
        match budget {
            Some(budget) if budget.said == Said::Flag => {
                format!(" --wake-budget {}", budget.seconds())
            }
            _ => String::new(),
        }
    }
}

/// The schema version of an acknowledgement, read closed as the watcher record
/// is.
pub const ACKNOWLEDGEMENT_SCHEMA_VERSION: u32 = 1;

/// Read the version, refusing an acknowledgement this build cannot honestly read.
fn this_version<'de, D: serde::Deserializer<'de>>(reader: D) -> std::result::Result<u32, D::Error> {
    let found = u32::deserialize(reader)?;
    if found != ACKNOWLEDGEMENT_SCHEMA_VERSION {
        return Err(serde::de::Error::custom(format!(
            "acknowledgement schema_version {found}, and this build reads \
             {ACKNOWLEDGEMENT_SCHEMA_VERSION}"
        )));
    }
    Ok(found)
}

/// Text that says something: not blank.
fn said<'de, D: serde::Deserializer<'de>>(reader: D) -> std::result::Result<String, D::Error> {
    let found = String::deserialize(reader)?;
    if found.trim().is_empty() {
        return Err(serde::de::Error::custom("a blank value says nothing"));
    }
    Ok(found)
}

/// An RFC 3339 instant.
fn an_instant<'de, D: serde::Deserializer<'de>>(
    reader: D,
) -> std::result::Result<String, D::Error> {
    let found = String::deserialize(reader)?;
    if crate::watchers::instant_millis(&found).is_none() {
        return Err(serde::de::Error::custom(format!(
            "at is '{found}', which is not an RFC 3339 instant"
        )));
    }
    Ok(found)
}

/// A session's deliberate word that one of its runs is owed nothing further,
/// and why: `runs/<run>/acknowledgements/<nonce>.json`, written by
/// `onepipeline unwatched --acknowledge` beside the run and never to its
/// journal, so it needs no driver and takes no lock.
///
/// It closes the run for the session it names, which has to be the run's own,
/// until a later `driver-adopted` or `edit-committed` re-opens it. Entry 98 of
/// `docs/contract-divergences.md` states the rule.
// llmlint: ignore-block[invalid_states_unrepresentable] the run id, the session and the
// instant are `String`s for the reason the watcher record's own block states — a record read
// back as another process wrote it, in a vocabulary the contract names no type for — and
// each is checked where it is read: the version, a session and a reason that say something,
// and `at` as an RFC 3339 instant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acknowledgement {
    /// The record's own version. See [`ACKNOWLEDGEMENT_SCHEMA_VERSION`].
    #[serde(deserialize_with = "this_version")]
    pub schema_version: u32,
    /// The run acknowledged.
    pub run_id: String,
    /// The session that acknowledged it, which owns it.
    #[serde(deserialize_with = "said")]
    pub session: String,
    /// Why it needs nothing further.
    #[serde(deserialize_with = "said")]
    pub reason: String,
    /// When, as an RFC 3339 instant.
    #[serde(deserialize_with = "an_instant")]
    pub at: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl Acknowledgement {
    /// `onepipeline unwatched --acknowledge RUN --reason TEXT`: record that
    /// `session` owes `run` under `root` nothing further.
    ///
    /// Every refusal is made before anything is written, so a refused
    /// acknowledgement leaves neither a record nor the directory one would have
    /// gone in.
    ///
    /// # Errors
    ///
    /// A blank reason, a run id that is not one, a run root that is not there, a
    /// run whose launch record cannot be read or names another session, or a
    /// record that could not be written.
    pub fn record(root: &Path, run: &str, session: &str, reason: &str) -> Result<Self> {
        if reason.trim().is_empty() {
            return Err(Error::Invalid(
                "an acknowledgement needs a reason that says something: `--reason` is blank, \
                 and nothing was recorded"
                    .to_owned(),
            ));
        }
        if !ledger::is_valid_run_id(run) {
            return Err(Error::Invalid(format!(
                "'{run}' is not a run id, so there is no run to acknowledge; nothing was recorded"
            )));
        }
        let paths = RunPaths::under(root, run);
        if !paths.exists() {
            return Err(Error::NoSuchRun {
                run: run.to_owned(),
                root: root.to_path_buf(),
            });
        }
        let launch: LaunchRecord = ledger::read_json(&paths.launch())?;
        if !launch.owned_by(session) {
            return Err(Error::NotOwned {
                run: run.to_owned(),
                owner: launch.owner_label(session),
            });
        }
        let acknowledged = Self {
            schema_version: ACKNOWLEDGEMENT_SCHEMA_VERSION,
            run_id: run.to_owned(),
            session: session.to_owned(),
            reason: reason.to_owned(),
            at: sys::now_rfc3339(),
        };
        ledger::write_json(
            &paths
                .acknowledgements()
                .join(format!("{}.json", crate::watchers::nonce())),
            &acknowledged,
        )?;
        Ok(acknowledged)
    }

    /// The line the binary prints for a recorded acknowledgement.
    pub fn line(&self) -> String {
        format!(
            "{}: acknowledged for session {} — {}\n",
            self.run_id, self.session, self.reason
        )
    }
}

/// What `onepipeline unwatched` found: the runs to report, and what it could
/// not resolve.
///
/// The two are the whole interface. One line per reported run on standard
/// output, nothing at all when there is nothing to report, and everything
/// unresolved on standard error — where it changes no status, because a run
/// whose evidence could not be read is neither an unwatched run nor a reason to
/// refuse the question that was asked about the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unwatched {
    /// The runs the session owns that are not proven settled and that nothing is
    /// watching, sorted by run id — the order a listing renders its rows in, so
    /// a caller scanning for an id scans one order.
    pub reported: Vec<UnwatchedRun>,
    /// What could not be resolved, each worded for standard error and sorted,
    /// with its own terminator.
    pub unresolved: Vec<String>,
}

impl Unwatched {
    /// The status the binary exits with: [`EXIT_RUNS_UNWATCHED`] where anything
    /// was reported, [`EXIT_SUCCESS`] otherwise.
    pub const fn exit_code(&self) -> i32 {
        if self.reported.is_empty() {
            EXIT_SUCCESS
        } else {
            EXIT_RUNS_UNWATCHED
        }
    }
}

/// One run nothing is watching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnwatchedRun {
    /// The run.
    // llmlint: ignore[invalid_states_unrepresentable] a run id is a `String` here for the reason `src/ledger.rs`'s file-level suppression states — it is read off a directory name `is_valid_run_id` has already admitted in `discover_owned_runs`, and `docs/contract.md` names no `RunId`.
    pub run: String,
    /// The word the listing prints for how it is being driven.
    // llmlint: ignore[invalid_states_unrepresentable] the listing's own word — `SETTLED`,
    // or the liveness verdict's — as `views::summary_standing_word` answers it, which is a
    // rendering of the private `Standing` and the contract names no type for it; this line
    // reproduces the listing's word beside the run's id, so it carries the word.
    pub standing: &'static str,
    /// Why nothing counts as watching it, in the watcher records' own words.
    pub why_not_watched: String,
    /// What would satisfy the question for it, spelled as the command a person
    /// types: a watch for a run that can still move, and the two ways to close
    /// one that has settled.
    remedy: String,
}

impl UnwatchedRun {
    /// The line the binary prints for this run, terminator included.
    pub(crate) fn line(&self) -> String {
        format!(
            "{:<24} {:<12} {} — {}\n",
            self.run, self.standing, self.why_not_watched, self.remedy
        )
    }
}

impl Unwatched {
    /// `onepipeline unwatched [--wake-budget SECONDS]`: which of `session`'s runs
    /// under `root` nothing will wake the session about within `budget` — or,
    /// with none, that nothing is watching at all, as [`crate::verbs::unwatched`]
    /// answers it.
    ///
    /// # Errors
    ///
    /// A runs root that exists and cannot be read.
    pub fn within(root: &Path, session: &str, budget: Option<WakeBudget>) -> Result<Self> {
        asked(root, session, budget).map(|answer| answer.unwatched)
    }
}

/// `onepipeline unwatched`: which of `session`'s runs under `root` has nothing
/// watching it.
///
/// # Errors
///
/// A runs root that exists and cannot be read — the question this verb cannot
/// ask, and answering it as "nothing is unwatched" would be the silence the
/// whole verb exists to end.
pub(crate) fn unwatched(root: &Path, session: &str) -> Result<Unwatched> {
    Unwatched::within(root, session, None)
}

/// What one question answered: the verb's own answer, and which of its
/// unresolved runs are **unknowns** — a run whose watch or closure this build
/// could not judge, which a guard warns on rather than passing in silence.
pub(crate) struct Answer {
    /// The verb's answer, every unknown among its unresolved lines.
    pub(crate) unwatched: Unwatched,
    /// The unknowns, worded as their unresolved lines are.
    pub(crate) unknown: Vec<String>,
}

/// Ask the question, keeping the unknowns apart.
///
/// # Errors
///
/// See [`unwatched`].
pub(crate) fn asked(root: &Path, session: &str, budget: Option<WakeBudget>) -> Result<Answer> {
    let mut reported: Vec<UnwatchedRun> = Vec::new();
    let owned = discover_owned_runs(root, session)?;
    let mut unresolved: Vec<String> = owned.unresolved;
    let mut unknown: Vec<String> = Vec::new();
    let now = i128::from(sys::now_millis());
    for paths in owned.runs {
        let summary = match decide(&paths, session) {
            Decided::Closed => continue,
            Decided::Undecidable(reason) => {
                unresolved.push(format!("{}: {reason}\n", paths.run));
                continue;
            }
            Decided::Unknown(reason) => {
                unknown.push(format!("{}: {reason}\n", paths.run));
                continue;
            }
            Decided::Owed(summary) => summary,
        };
        let leases = Leases::of(&paths);
        let watchers = &leases.watchers;
        let why_not_watched = match budget {
            // No budget: "watched" keeps the meaning it always had, any live lease.
            None if watchers.any_live() => continue,
            None => watchers.why_not_watched(),
            Some(budget) => {
                let wakes = leases.wakes(session, i128::from(budget.seconds()) * 1_000, now);
                if wakes.contains(&Wake::Within) {
                    continue;
                }
                let words = |pick: fn(&Wake) -> Option<&String>| -> Vec<String> {
                    wakes.iter().filter_map(pick).cloned().collect()
                };
                let unknowns = words(|wake| match wake {
                    Wake::Unknown(why) => Some(why),
                    _ => None,
                });
                // Only a positively determined failure is reported: one live lease
                // nothing here can judge leaves the run unknown, however the rest
                // fail.
                if !unknowns.is_empty() {
                    unknown.push(format!(
                        "{}: whether a live watch wakes this session within its wake budget of \
                         {}s cannot be said, so it is not reported: {}\n",
                        paths.run,
                        budget.seconds(),
                        unknowns.join("; ")
                    ));
                    continue;
                }
                let fails = words(|wake| match wake {
                    Wake::Fails(why) => Some(why),
                    _ => None,
                });
                if fails.is_empty() {
                    watchers.why_not_watched()
                } else {
                    format!(
                        "no live watch wakes this session within its wake budget of {}s: {}",
                        budget.seconds(),
                        fails.join("; ")
                    )
                }
            }
        };
        for refused in &watchers.refused {
            unresolved.push(format!(
                "{}: a watcher record cannot be read, so it is not a live watch: {} — {}\n",
                paths.run,
                refused.path.display(),
                refused.reason
            ));
        }
        reported.push(UnwatchedRun {
            run: paths.run.clone(),
            standing: views::summary_standing_word(root, &summary),
            why_not_watched,
            remedy: remedy(&paths.run, &summary, budget),
        });
    }
    reported.sort_by(|a, b| a.run.cmp(&b.run));
    unknown.sort();
    unresolved.extend(unknown.iter().cloned());
    unresolved.sort();
    Ok(Answer {
        unwatched: Unwatched {
            reported,
            unresolved,
        },
        unknown,
    })
}

/// The command that would satisfy the question for one reported run.
///
/// A run whose graph has settled can no longer move, so no watch of it wakes
/// anybody: what it needs is closing, and there are two ways to. Any other run
/// needs a watch meeting the budget — spelled with the budget where a flag gave
/// it, and bare where the environment did, because the environment's budget is
/// `watch`'s own default.
fn remedy(run: &str, summary: &RunSummary, budget: Option<WakeBudget>) -> String {
    if summary.graph_complete {
        return format!(
            "close it with a verdict whose `completion` is true: onepipeline reply {run}, or \
             acknowledge it: onepipeline unwatched --acknowledge {run} --reason <TEXT>"
        );
    }
    match budget {
        Some(budget) if budget.said == Said::Flag => {
            format!("{ARM_A_WATCH} {run} --timeout {}", budget.seconds())
        }
        _ => format!("{ARM_A_WATCH} {run}"),
    }
}

/// The session `onepipeline unwatched` asks about, from the option or the
/// environment.
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
pub(crate) fn session(args: &UnwatchedArgs) -> Result<String> {
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

/// What this build could establish about whether one run is still owed.
enum Decided {
    /// It is closed — for a run no driver of the closure rule's release drove,
    /// stopped or settled; for one such a driver drove, stopped, asked to
    /// complete, or acknowledged — proved from a summary document whose stamp
    /// matches its journal as it stands. Not reported, however long it has been
    /// unwatched.
    Closed,
    /// Nothing **proved** it closed, which is not the same as proving it is
    /// still going and is deliberately not named as though it were: a document
    /// that records **no** closure is in here whether its stamp is fresh or
    /// stale, because a run still recording is exactly what a document behind its
    /// journal recording no closure looks like. A document that **does** record
    /// closure while standing behind its journal is *not* here: its own record
    /// says it closed, so a stale stamp leaves that undecidable rather than
    /// reported — see [`decide`].
    ///
    /// Boxed because the document is the largest thing here by two orders of
    /// magnitude, and this value is built once per owned run: an unboxed variant
    /// would make every closed and undecidable run pay for the one that is
    /// reported.
    Owed(Box<RunSummary>),
    /// Nothing established either, and watching it would not clear that.
    Undecidable(String),
    /// Whether it is closed cannot be said, because evidence that might close it
    /// could not be read — an unknown, which a guard warns on.
    Unknown(String),
}

/// Read one run's settlement out of its **summary document**, on the rule entry
/// 68 states: excluded on a current document that records settlement, undecidable
/// on a stale one that does, reported on one that records none.
///
/// The one fold on this path is for a document at a schema this build has moved
/// **past** — the previous release's, current for its journal on every run that
/// release settled. It is refreshed through [`RunSummary::of`], the reader `runs`
/// refreshes it with, and then decided like any other: reporting it unread is
/// what the entry records as measured, and passing it over would silence a run
/// still being driven by that release's binary.
fn decide(paths: &RunPaths, session: &str) -> Decided {
    let text = match std::fs::read_to_string(paths.summary()) {
        Ok(text) => text,
        // Most often an old settled run whose document is gone, and blocking on it
        // would never clear by watching it — so it is named and passed over rather
        // than reported.
        Err(error) => {
            return Decided::Undecidable(format!(
                "its settlement cannot be decided: its summary document could not be read: \
                 {error}"
            ))
        }
    };
    let summary = match serde_json::from_str::<RunSummary>(&text) {
        Ok(summary) => summary,
        Err(refusal) => {
            // A schema this build has moved past is the previous release's
            // document, and it is refreshed rather than read: the one fold on this
            // path, paid once per run per schema bump — the reader that refreshes
            // it is the listing's own, so the document it leaves is the one `runs`
            // would have left, and a store that reader cannot read is decided
            // exactly as `runs` decides it.
            if crate::summary::version_this_build_moved_past(&text).is_some() {
                return match RunSummary::of(paths) {
                    Ok(refreshed) => decided_from(paths, refreshed, session),
                    // The one refusal that reader gives an owned run: its root, or
                    // the launch record discovery just read, gone between that read
                    // and this one. Named rather than dropped, because a run this
                    // verb never saw is not a run it decided.
                    // llmlint: ignore-block[changed_behavior_has_e2e] reachable only by a
                    // run root or launch record removed between two reads of one
                    // invocation, which no journey can stage without racing the verb it
                    // drives.
                    Err(error) => Decided::Undecidable(format!(
                        "its settlement cannot be decided: its summary document is at a \
                         schema this build has moved past, and refreshing it failed: {error}"
                    )),
                    // llmlint: ignore-end[changed_behavior_has_e2e]
                };
            }
            return Decided::Undecidable(format!(
                "its settlement cannot be decided: its summary document could not be read: \
                 {refusal}"
            ));
        }
    };
    decided_from(paths, summary, session)
}

/// Decide a run from a document this build read — stored, or refreshed.
///
/// **Which rule is the run's own.** A run no driver of the closure rule's
/// release has driven carries no `owed_until_closed`, and is decided as it always
/// was: stopped or settled is closed. One that such a driver drove is owed until
/// it is closed, whatever its standing — a settled graph alone no longer closes
/// it, because a failure hook's ending counts failed nodes as settled. Either
/// kind is closed by an acknowledgement its own session made since it last
/// re-opened.
fn decided_from(paths: &RunPaths, summary: RunSummary, session: &str) -> Decided {
    if summary.run_id != paths.run {
        return Decided::Undecidable(format!(
            "its settlement cannot be decided: its summary document is run '{}'",
            summary.run_id
        ));
    }
    // Its own record says it closed: excluded when its stamp is current,
    // undecidable when it is stale. The docstring on [`decide`] says why the stale
    // case is not reported.
    let settled = summary.stop_recorded || (!summary.owed_until_closed && summary.graph_complete);
    if settled || (summary.owed_until_closed && summary.completion_requested) {
        if stamped(paths, &summary) {
            return Decided::Closed;
        }
        return Decided::Undecidable(format!(
            "its settlement cannot be decided: its document records that it {} but is behind \
             its journal",
            if settled { "settled" } else { "was closed" }
        ));
    }
    match acknowledged(paths, session, summary.reopened_at) {
        Err(why) => Decided::Unknown(why),
        Ok(true) if stamped(paths, &summary) => Decided::Closed,
        Ok(true) => Decided::Undecidable(
            "its settlement cannot be decided: it was acknowledged, but its document is behind \
             its journal, so whether it re-opened since cannot be said"
                .to_string(),
        ),
        Ok(false) => Decided::Owed(Box::new(summary)),
    }
}

/// Whether `session` has acknowledged the run since it last re-opened.
///
/// An acknowledgement naming another session, or another run, closes nothing —
/// it is not this session's word about this run. One that cannot be read is an
/// unknown unless another closes the run anyway: it may be the word that would
/// have.
fn acknowledged(
    paths: &RunPaths,
    session: &str,
    reopened_at: Option<u64>,
) -> std::result::Result<bool, String> {
    let dir = paths.acknowledgements();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "whether it is closed cannot be said: its acknowledgements cannot be read: \
                 {}: {error}",
                dir.display()
            ))
        }
    };
    let after = reopened_at.map_or(i128::MIN, i128::from);
    let mut unreadable: Option<String> = None;
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            // llmlint: ignore-block[changed_behavior_has_e2e] an entry the filesystem lists
            // and then refuses to describe is a host condition no portable journey can set —
            // `src/ledger.rs`'s own listing carries the same suppression for the same arm.
            Err(error) => {
                unreadable = Some(format!("{}: {error}", dir.display()));
                continue;
            } // llmlint: ignore-end[changed_behavior_has_e2e]
        };
        // A writer's temporary sibling is not a record until it is renamed.
        if path.extension().and_then(|suffix| suffix.to_str()) != Some("json") {
            continue;
        }
        let read = std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                serde_json::from_str::<Acknowledgement>(&text).map_err(|error| error.to_string())
            });
        match read {
            Ok(acknowledged) => {
                if acknowledged.run_id == paths.run
                    && acknowledged.session == session
                    && crate::watchers::instant_millis(&acknowledged.at)
                        .is_some_and(|at| at > after)
                {
                    return Ok(true);
                }
            }
            Err(why) => unreadable = Some(format!("{}: {why}", path.display())),
        }
    }
    match unreadable {
        Some(why) => Err(format!(
            "whether it is closed cannot be said: an acknowledgement of it cannot be read: {why}"
        )),
        None => Ok(false),
    }
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

    /// Entry 68 of the divergence record, which is where this verb and the record
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

    /// Entry 98, which proposes the terms record, the wake budget, the closure
    /// rule and the acknowledgement, as its JSON block.
    pub(crate) fn budget_block() -> serde_json::Value {
        let record = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("docs")
                .join("contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split_once("\n## 98.")
            .expect("the wake budget is recorded under entry 98")
            .1;
        let entry = entry.split_once("\n## ").map_or(entry, |(head, _)| head);
        let block = entry
            .split_once("```json")
            .expect("entry 98 carries the json block these tests drive")
            .1
            .split_once("```")
            .expect("the block is fenced")
            .0;
        serde_json::from_str(block).expect("entry 98's block is JSON")
    }

    /// The keys one document serializes under.
    fn keys_of(document: &impl serde::Serialize) -> BTreeSet<String> {
        serde_json::to_value(document)
            .expect("a record is an object")
            .as_object()
            .expect("a record is an object")
            .keys()
            .cloned()
            .collect()
    }

    /// The terms record and the acknowledgement are the ones entry 98 names,
    /// both ways, and each round-trips.
    ///
    /// Built through the types, so a field either grows does not compile here
    /// until it is given a value, and every optional field is given one so it
    /// reaches the wire.
    #[test]
    fn the_divergence_entry_names_the_terms_and_the_acknowledgement_this_build_writes() {
        use crate::watchers::{WatchTerms, WATCH_TERMS_SCHEMA_VERSION};

        let block = budget_block();
        assert_eq!(
            block["terms_schema_version"].as_u64(),
            Some(u64::from(WATCH_TERMS_SCHEMA_VERSION))
        );
        let terms = WatchTerms {
            schema_version: WATCH_TERMS_SCHEMA_VERSION,
            run_id: "gated".into(),
            pid: std::num::NonZeroU32::new(4_242).expect("a pid"),
            started: "linux-proc-stat:1".into(),
            deadline: Some("2026-01-01T00:30:00.000Z".into()),
            until: vec!["surface".into(), "settled".into(), "nothing-driving".into()],
            session: Some("a-session".into()),
        };
        let named: BTreeSet<String> =
            serde_json::from_value(block["terms_fields"].clone()).expect("entry 98 names them");
        assert_eq!(
            named,
            keys_of(&terms),
            "entry 98's terms inventory is not the record this build writes"
        );
        let read: WatchTerms =
            serde_json::from_str(&serde_json::to_string(&terms).expect("terms serialize"))
                .expect("terms this build wrote read back");
        assert_eq!(read, terms);
        // Nullable fields are written as `null`, never omitted: the record holds
        // exactly the fields it names.
        let unbounded = WatchTerms {
            deadline: None,
            session: None,
            ..terms
        };
        assert_eq!(keys_of(&unbounded), named);

        assert_eq!(
            block["acknowledgement_schema_version"].as_u64(),
            Some(u64::from(ACKNOWLEDGEMENT_SCHEMA_VERSION))
        );
        let acknowledged = Acknowledgement {
            schema_version: ACKNOWLEDGEMENT_SCHEMA_VERSION,
            run_id: "gated".into(),
            session: "a-session".into(),
            reason: "the follow-up run carries it".into(),
            at: "2026-01-01T00:00:00.000Z".into(),
        };
        let named: BTreeSet<String> =
            serde_json::from_value(block["acknowledgement_fields"].clone())
                .expect("entry 98 names them");
        assert_eq!(
            named,
            keys_of(&acknowledged),
            "entry 98's acknowledgement inventory is not the record this build writes"
        );
        let read: Acknowledgement = serde_json::from_str(
            &serde_json::to_string(&acknowledged).expect("an acknowledgement serializes"),
        )
        .expect("an acknowledgement this build wrote reads back");
        assert_eq!(read, acknowledged);

        // Where each is written, and the names the rest of the rule is spelled in.
        let paths = RunPaths::under(Path::new("/runs"), "gated");
        let dir_name = |dir: std::path::PathBuf| {
            dir.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        };
        assert_eq!(
            dir_name(paths.watch_terms()).as_deref(),
            block["terms_directory"].as_str()
        );
        assert_eq!(
            dir_name(paths.acknowledgements()).as_deref(),
            block["acknowledgement_directory"].as_str()
        );
        assert_eq!(
            block["wake_budget_environment"].as_str(),
            Some(crate::cli::WAKE_BUDGET_ENV)
        );
        assert_eq!(
            block["migration_field"].as_str(),
            Some(crate::journal::OWED_UNTIL_CLOSED)
        );
        let summary: Vec<String> =
            serde_json::from_value(block["summary_fields"].clone()).expect("entry 98 names them");
        // Read off the checked-in golden, which pins every field present.
        let golden: RunSummary =
            serde_json::from_str(include_str!("../tests/golden/run-summary-v8.json"))
                .expect("the summary golden reads");
        let carried = keys_of(&golden);
        for field in &summary {
            assert!(
                carried.contains(field),
                "entry 98 names `{field}`, which the summary document does not carry"
            );
        }
    }

    /// A terms record or an acknowledgement this build did not write is refused
    /// rather than read: another version, a key it does not know, a field
    /// missing — `null` included where a field is nullable — and each value that
    /// is not what its field says.
    #[test]
    fn a_terms_record_or_an_acknowledgement_this_build_did_not_write_is_refused() {
        use crate::watchers::{WatchTerms, WATCH_TERMS_SCHEMA_VERSION};

        let terms = serde_json::json!({
            "schema_version": WATCH_TERMS_SCHEMA_VERSION,
            "run_id": "gated",
            "pid": 4_242,
            "started": "linux-proc-stat:1",
            "deadline": null,
            "until": ["surface", "settled", "nothing-driving"],
            "session": null,
        });
        serde_json::from_value::<WatchTerms>(terms.clone()).expect("terms an arming wrote");
        let refused = |edit: &dyn Fn(&mut serde_json::Value), why: &str| {
            let mut document = terms.clone();
            edit(&mut document);
            let refusal = serde_json::from_value::<WatchTerms>(document)
                .expect_err("terms this build did not write are refused");
            assert!(
                refusal.to_string().contains(why),
                "the refusal does not say what it refused ({why}): {refusal}"
            );
        };
        refused(
            &|t| t["schema_version"] = serde_json::json!(2),
            "schema_version",
        );
        refused(&|t| t["watching"] = serde_json::json!(true), "watching");
        refused(
            &|t| {
                t.as_object_mut().expect("an object").remove("deadline");
            },
            "deadline",
        );
        refused(
            &|t| {
                t.as_object_mut().expect("an object").remove("session");
            },
            "session",
        );
        refused(
            &|t| t["deadline"] = serde_json::json!("in half an hour"),
            "deadline",
        );
        refused(
            &|t| t["until"] = serde_json::json!(["surface", "lunch"]),
            "lunch",
        );
        // And the two every watch returns on, which no record this build writes
        // leaves out.
        refused(
            &|t| t["until"] = serde_json::json!(["surface", "settled"]),
            "nothing-driving",
        );
        refused(
            &|t| t["until"] = serde_json::json!(["surface", "nothing-driving"]),
            "settled",
        );
        refused(&|t| t["session"] = serde_json::json!("  "), "session");
        refused(&|t| t["pid"] = serde_json::json!(0), "");

        let acknowledgement = serde_json::json!({
            "schema_version": ACKNOWLEDGEMENT_SCHEMA_VERSION,
            "run_id": "gated",
            "session": "a-session",
            "reason": "the follow-up run carries it",
            "at": "2026-01-01T00:00:00.000Z",
        });
        serde_json::from_value::<Acknowledgement>(acknowledgement.clone())
            .expect("an acknowledgement the verb wrote");
        for (key, value) in [
            ("schema_version", serde_json::json!(2)),
            ("reason", serde_json::json!(" ")),
            ("session", serde_json::json!("")),
            ("at", serde_json::json!("yesterday")),
            ("until", serde_json::json!([])),
        ] {
            let mut document = acknowledgement.clone();
            document[key] = value;
            serde_json::from_value::<Acknowledgement>(document)
                .expect_err("an acknowledgement this build did not write is refused");
        }
    }

    /// A budget of no seconds is refused rather than read as none, and a given
    /// one is spelled back on the command a person asks it by hand with.
    #[test]
    fn a_wake_budget_of_nothing_is_refused_and_a_given_one_is_spelled_back() {
        let refused = WakeBudget::resolved(Some(0)).expect_err("0 seconds wakes nobody");
        assert!(refused.to_string().contains("--wake-budget"), "{refused}");
        let given = WakeBudget::resolved(Some(1_800))
            .expect("a positive budget")
            .expect("a given budget is one");
        assert_eq!(given.seconds(), 1_800);
        assert_eq!(WakeBudget::flag(Some(given)), " --wake-budget 1800");
        assert_eq!(WakeBudget::flag(None), "");
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
