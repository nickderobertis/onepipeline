//! A node whose prerequisites a private source could not be asked about, held until it
//! can be.
//!
//! The plan store answers a task's dependency read without failing it when a source it had
//! to ask cannot be asked — its credential absent or refused, it unreachable or rate-limited,
//! or the tracker an opaque edge names configured nowhere here — and names it beside the
//! edges the other sources hold. A task's read asks only the sources its own record and its
//! edges name, so a node is held only on the sources its own read reported. A reader without
//! an organization's credential still reads the plan, which is the point. What it cannot read
//! is whether that source records a prerequisite of the task that no other source holds, and
//! a node dispatched on the edges it could read would be dispatched as if it had no such
//! prerequisite.
//!
//! So the plan loads and such a node is **held** under a `source` reason rather than
//! dispatched: the launch says so on its first records, and the driver re-reads the node's
//! edges off the loop's thread, because a hosted source can take its time not answering.
//!
//! **When a source is read again is the source's, not the node's.** A source that answered
//! rate limited and named its reset is read again one second after that instant, which the
//! hold records as `retry_at_ms`; any other is read again after a backoff that starts at
//! [`POLL_ENV`]'s interval, doubles after each re-read it still does not answer, stops
//! doubling at [`BACKOFF_CAP_INTERVALS`] intervals, and starts again from one interval once
//! it answers. One pass re-reads every node held on the sources then due, and stops asking a
//! source the moment it refuses again, so a pass costs a source that still cannot answer one
//! request however many nodes wait on it.
//!
//! The hold clears the pass a re-read asks every source and finds no prerequisite the run's
//! graph does not already give the node; one it does find keeps the node held, naming it,
//! until an edit gives the node that edge. While held, the wait is surfaced non-blocking on
//! the cadence a release wait is, naming the source, why it did not answer, when it is read
//! again, and — where something other than waiting would let it answer — what would. Nothing
//! times the hold out. A node whose read asked every source loads and dispatches as it always
//! did.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use serde_json::{json, Value};

use crate::channel::Surface;
use crate::error::Result;
use crate::journal::Journal;
use crate::ledger::RunPaths;
use crate::plan::Node;
use crate::taskgraph::{Cause, QualifiedId, Reread, SourceCause, Store, Unanswered, Unasked};

/// The kind a held node's wait is surfaced under.
pub const WAIT_SURFACE_KIND: &str = "source-wait";

/// The word a `node-held` entry carries for this hold.
pub(crate) const HOLD_KIND: &str = "source";

/// The environment variable setting the interval a source's backoff starts at.
pub const POLL_ENV: &str = "ONEPIPELINE_SOURCE_POLL_SECONDS";

/// The interval a source's backoff starts at when nothing overrides it: a minute, the
/// interval every other paced read of the loop ships with. A host wanting its store asked
/// less often lengthens [`POLL_ENV`]; a value that is not a positive whole number of seconds
/// is no setting at all, and this stands.
pub const DEFAULT_POLL_SECONDS: u64 = 60;

/// How many intervals a source's backoff grows to before it stops doubling: half an hour at
/// the default interval.
pub(crate) const BACKOFF_CAP_INTERVALS: u64 = 30;

/// How long after a provider's reset its source is read again, so the read lands after the
/// reset rather than on it.
const AFTER_RESET_MS: u64 = 1_000;

/// How soon the loop looks again for a re-read it has under way.
const TICK: Duration = Duration::from_millis(200);

/// What a `source` hold carries: each source the node's last dependency read could not ask
/// and why, and each prerequisite that read found which the run's graph does not give the
/// node.
///
/// Never neither: a hold names what holds the node, so the one constructor, [`new`](Self::new),
/// answers no hold at all for a read that asked every source and found nothing missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceHold {
    /// The sources not asked, in name order; empty only where `missing` is not.
    unanswered: Vec<Unanswered>,
    /// The node ids the read resolved a prerequisite to that the node does not depend on in
    /// the run's graph.
    missing: Vec<String>,
}

impl SourceHold {
    /// The hold these sources and prerequisites stand for, or `None` where both are empty
    /// and nothing holds the node.
    pub(crate) fn new(unanswered: Vec<Unanswered>, missing: Vec<String>) -> Option<Self> {
        (!unanswered.is_empty() || !missing.is_empty()).then_some(Self {
            unanswered,
            missing,
        })
    }

    pub(crate) fn unanswered(&self) -> &[Unanswered] {
        &self.unanswered
    }

    /// The `node-held` entry: `missing` only where the read found one, and on each source
    /// its `cause` and, only where the provider gave a reset, `retry_at_ms`.
    pub(crate) fn payload(&self) -> Value {
        let mut entry = json!({
            "kind": HOLD_KIND,
            "unanswered": self
                .unanswered
                .iter()
                .map(|unasked| {
                    let mut said = json!({"source": unasked.name(), "reason": unasked.reason});
                    if let Some(cause) = unasked.cause() {
                        said["cause"] = json!(cause.as_str());
                    }
                    if let Some(at) = unasked.retry_at_ms() {
                        said["retry_at_ms"] = json!(at);
                    }
                    said
                })
                .collect::<Vec<_>>(),
        });
        if !self.missing.is_empty() {
            entry["missing"] = json!(self.missing);
        }
        entry
    }

    /// One `node-held` entry read back, or `None` for an entry that is not this hold or that
    /// this build cannot read whole — a `cause` it has no word for among them. An entry a
    /// build before `cause` wrote carries neither field, and reads as it was written.
    pub(crate) fn of_payload(entry: &Value) -> Option<Self> {
        if entry.get("kind")?.as_str()? != HOLD_KIND {
            return None;
        }
        let unanswered = entry
            .get("unanswered")?
            .as_array()?
            .iter()
            .map(|unasked| {
                let named = unasked.get("source")?.as_str()?;
                let retry_at_ms = match unasked.get("retry_at_ms") {
                    None => None,
                    Some(at) => Some(at.as_u64()?),
                };
                // A cause word or a reset on another cause is what only a later build could
                // journal; such an entry is not read, and the engine treats the hold as one it
                // does not know, as it does any reason a later build adds.
                // llmlint: ignore-block[changed_behavior_has_e2e] no build writes either shape, so a journey could only reach them through a journal edited by hand, which `tests_mirror_real_usage` refuses; `unanswered::tests::the_hold_entry_round_trips_and_omits_what_it_does_not_hold` drives this reader with each, and what the engine does with an entry it cannot read is `HoldReason::of_payload`'s existing path for every hold kind.
                let cause = match unasked.get("cause") {
                    None => None,
                    Some(word) => Some(Cause::of_word(word.as_str()?)?),
                };
                if retry_at_ms.is_some() && cause != Some(Cause::RateLimited) {
                    return None;
                }
                // llmlint: ignore-end[changed_behavior_has_e2e]
                let name = || onetaskgraph_plugin_api::SourceName::new(named).ok();
                let source = |cause| {
                    Some(Unasked::Source {
                        source: name()?,
                        cause,
                    })
                };
                let who = match cause {
                    None => Unasked::Recorded(name()?),
                    Some(Cause::RateLimited) => source(SourceCause::RateLimited { retry_at_ms })?,
                    Some(Cause::Auth) => source(SourceCause::Auth)?,
                    Some(Cause::Unavailable) => source(SourceCause::Unavailable)?,
                    Some(Cause::Unconfigured) => Unasked::Tracker(
                        onetaskgraph_plugin_api::Designator::try_from(named.to_owned()).ok()?,
                    ),
                };
                Some(Unanswered {
                    who,
                    reason: unasked.get("reason")?.as_str()?.to_owned(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let missing = match entry.get("missing") {
            None => Vec::new(),
            Some(missing) => missing
                .as_array()?
                .iter()
                .map(|id| id.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()?,
        };
        Self::new(unanswered, missing)
    }

    /// What holds the node, on one line, as a view says it: each source, why it could not
    /// answer, and when it is read again — and that nothing times the hold out.
    pub(crate) fn describe(&self) -> String {
        self.describe_scheduled(None)
    }

    /// [`describe`](Self::describe), saying of a backing-off source the next read `schedule`
    /// — the driver's [`SCHEDULE_FILE`] — gives it, where it gives one.
    pub(crate) fn describe_scheduled(&self, schedule: Option<&BTreeMap<String, Due>>) -> String {
        let said = self.said(&|unasked| Next::of(schedule, unasked));
        if self
            .unanswered
            .iter()
            .any(|unasked| unasked.cause().is_none())
        {
            // A hold journalled before the cause was recorded reads as it always did.
            return said;
        }
        format!("{said}; nothing times this out")
    }

    /// What holds the node, saying of each source the backoff wait `wait` gives it.
    fn said(&self, wait: &dyn Fn(&Unanswered) -> Next) -> String {
        let mut said: Vec<String> = self
            .unanswered
            .iter()
            .map(|unasked| match unasked.cause() {
                None => format!(
                    "source '{}', declared private, could not be asked what it records of \
                     its task: {}",
                    unasked.name(),
                    unasked.reason
                ),
                Some(cause) => format!(
                    "{}: {}; {}",
                    what(&unasked.name(), cause),
                    unasked.reason,
                    when(unasked, wait(unasked), "it")
                ),
            })
            .collect();
        if !self.missing.is_empty() {
            said.push(format!(
                "the store records it depending on {}, which this run's graph does not",
                quoted(&self.missing)
            ));
        }
        said.join("; ")
    }

    /// The sources it names, as a list a person reads.
    pub(crate) fn sources(&self) -> String {
        quoted(
            &self
                .unanswered
                .iter()
                .map(Unanswered::name)
                .collect::<Vec<_>>(),
        )
    }
}

/// What a source could not do, as every view and the surface say it.
fn what(source: &str, cause: Cause) -> String {
    match cause {
        Cause::Unconfigured => format!(
            "tracker '{source}', which no source configured here is, could not be asked about \
             the item this task's edge names there"
        ),
        Cause::RateLimited => format!(
            "source '{source}', declared private, is rate limited, so it could not be asked \
             what it records of this task"
        ),
        Cause::Auth => format!(
            "source '{source}', declared private, could not authenticate, so it could not be \
             asked what it records of this task"
        ),
        Cause::Unavailable => format!(
            "source '{source}', declared private, could not be reached, so it could not be \
             asked what it records of this task"
        ),
    }
}

/// What the one saying when a source is read again knows of its backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// Nothing: the backoff's first interval and its cap are what it can say.
    Unknown,
    /// The wait its schedule last chose, in seconds from when it last did not answer.
    In(u64),
    /// That wait, and the instant it ends at.
    At { wait: u64, at_ms: u64 },
}

impl Next {
    /// What `schedule` says of `unasked`'s source.
    fn of(schedule: Option<&BTreeMap<String, Due>>, unasked: &Unanswered) -> Self {
        match schedule.and_then(|schedule| schedule.get(&unasked.name())) {
            Some(Due {
                at_ms,
                wait: Some(wait),
                ..
            }) => Self::At {
                wait: *wait,
                at_ms: *at_ms,
            },
            _ => Self::Unknown,
        }
    }
}

/// When `unasked` is read again: one second after the reset its provider gave, or on its
/// backoff as `next` knows it. `subject` is what the sentence calls the source: `it`, or its
/// quoted name beside another's.
fn when(unasked: &Unanswered, next: Next, subject: &str) -> String {
    let poll = poll_seconds();
    let cap = poll.saturating_mul(BACKOFF_CAP_INTERVALS);
    if let Some(at) = unasked.retry_at_ms() {
        return format!(
            "{subject} is read again at {}, the reset the provider gave",
            utc(at)
        );
    }
    let given = if unasked.cause() == Some(Cause::RateLimited) {
        "no reset time was given, so "
    } else {
        ""
    };
    match next {
        Next::In(wait) => format!(
            "{given}{subject} is read again in {wait}s, the wait doubling each time it still \
             does not answer, up to {cap}s ({BACKOFF_CAP_INTERVALS} poll intervals)"
        ),
        Next::At { wait, at_ms } => format!(
            "{given}{subject} is read again at {}, {wait}s after it last did not answer, the \
             wait doubling each time it still does not answer, up to {cap}s \
             ({BACKOFF_CAP_INTERVALS} poll intervals)",
            utc(at_ms)
        ),
        Next::Unknown => format!(
            "{given}{subject} is read again on a backoff starting at {poll}s, the wait doubling \
             each time it still does not answer, up to {cap}s ({BACKOFF_CAP_INTERVALS} poll \
             intervals)"
        ),
    }
}

fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// An instant as a person reads it: `2026-05-04 11:50:00 UTC`.
fn utc(at_ms: u64) -> String {
    let stamp = crate::sys::rfc3339_from_millis(at_ms);
    format!(
        "{} UTC",
        stamp.get(..19).unwrap_or(&stamp).replace('T', " ")
    )
}

/// The holds a launch's read of `plan` stands for: each node of it whose dependency read
/// could not ask a source, held for those sources.
pub(crate) fn launched(
    plan: &crate::plan::Plan,
    unanswered: &BTreeMap<String, Vec<Unanswered>>,
) -> BTreeMap<String, SourceHold> {
    plan.tasks
        .iter()
        .filter_map(|node| {
            let unasked = unanswered.get(&node.id)?;
            Some((
                node.id.clone(),
                SourceHold::new(unasked.clone(), Vec::new())?,
            ))
        })
        .collect()
}

/// What a launch, and a `plan check`, say on stderr about the nodes `held` holds: one line
/// per source, naming it, why it did not answer, the nodes it holds, and when it is read
/// again. `None` where nothing is held.
pub(crate) fn notice(held: &BTreeMap<String, SourceHold>) -> Option<String> {
    let mut by_source: BTreeMap<String, (Unanswered, Vec<String>)> = BTreeMap::new();
    for (node, hold) in held {
        for unasked in hold.unanswered() {
            by_source
                .entry(unasked.name())
                .or_insert_with(|| (unasked.clone(), Vec::new()))
                .1
                .push(node.clone());
        }
    }
    if by_source.is_empty() {
        return None;
    }
    // A launch's first re-read of a source that named no reset is one interval out.
    let first = Next::In(poll_seconds());
    Some(
        by_source
            .into_values()
            .map(|(unasked, nodes)| {
                let cause = unasked.cause().unwrap_or(Cause::Unavailable);
                let who = match cause {
                    Cause::Unconfigured => format!(
                        "tracker '{}', which no source configured here is,",
                        unasked.name()
                    ),
                    _ => format!("source '{}', declared private,", unasked.name()),
                };
                let why = match cause {
                    Cause::RateLimited => "is rate limited and could not be asked",
                    Cause::Auth => "could not authenticate and could not be asked",
                    Cause::Unavailable => "could not be reached and could not be asked",
                    Cause::Unconfigured => "could not be asked",
                };
                format!(
                    "onepipeline: {who} {why} about the tasks whose reads name it, {}: {} — the \
                     plan loads, and each of those nodes is held, never dispatched; {}. Nothing \
                     times this out.",
                    quoted(&nodes),
                    unasked.reason,
                    when(&unasked, first, "it")
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// What holds a node a re-read of its edges answered, or `None` where nothing does: every
/// source was asked, and every prerequisite the read found is one of `deps`, the node's
/// dependencies in the run's graph.
pub(crate) fn hold_after(deps: &[String], reread: &Reread) -> Option<SourceHold> {
    let mut missing: Vec<String> = reread
        .deps
        .iter()
        .filter(|dep| !deps.contains(dep))
        .cloned()
        .collect();
    missing.sort();
    missing.dedup();
    SourceHold::new(reread.unanswered.clone(), missing)
}

/// When one source is next read, and how far its backoff has grown: one entry of
/// [`SCHEDULE_FILE`], by these names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Due {
    /// The epoch milliseconds it is next read at.
    #[serde(rename = "next_read_ms")]
    at_ms: u64,
    /// How many re-reads in a row it has not answered with no reset named, which is how many
    /// times its backoff has doubled.
    backoffs: u32,
    /// The wait its schedule last chose, in seconds; `None` where a reset decided it.
    #[serde(rename = "wait_seconds")]
    wait: Option<u64>,
}

/// The file, in a run's directory, the driver keeps each held source's schedule in: what a
/// view in another process says a source's next read by, since the journal records a hold's
/// reset but not its backoff. Telemetry beside the record, not a field of it (entry 131).
pub(crate) const SCHEDULE_FILE: &str = "source-schedule.json";

/// The schema [`SCHEDULE_FILE`] is written at; a file in any other is not read.
const SCHEDULE_SCHEMA_VERSION: u32 = 1;

/// [`SCHEDULE_FILE`] as it is written.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ScheduleFile {
    schema_version: u32,
    sources: BTreeMap<String, Due>,
}

/// Each held source's schedule the driver of the run in `run_dir` last wrote, or `None` for a
/// run no driver of this build has scheduled, and for a file that does not read whole.
pub(crate) fn schedule_of(run_dir: &std::path::Path) -> Option<BTreeMap<String, Due>> {
    let text = std::fs::read_to_string(run_dir.join(SCHEDULE_FILE)).ok()?;
    let file: ScheduleFile = serde_json::from_str(&text).ok()?;
    (file.schema_version == SCHEDULE_SCHEMA_VERSION).then_some(file.sources)
}

impl Due {
    /// The schedule after a refusal answered at `now_ms` naming the reset `retry_at_ms`: that
    /// reset plus a second, or — where it named none — the backoff's next wait, which then
    /// doubles.
    fn after(self, retry_at_ms: Option<u64>, now_ms: u64, poll: u64) -> Self {
        if let Some(at) = retry_at_ms {
            return Self {
                at_ms: at.saturating_add(AFTER_RESET_MS),
                wait: None,
                ..self
            };
        }
        let wait = backoff(self.backoffs, poll);
        Self {
            at_ms: now_ms.saturating_add(wait.saturating_mul(1_000)),
            backoffs: self.backoffs.saturating_add(1),
            wait: Some(wait),
        }
    }

    /// A source nothing has scheduled yet: its backoff at its first interval.
    fn fresh(now_ms: u64) -> Self {
        Self {
            at_ms: now_ms,
            backoffs: 0,
            wait: None,
        }
    }
}

/// The wait, in seconds, after `backoffs` re-reads in a row a source did not answer: the
/// interval doubled that many times, and never more than [`BACKOFF_CAP_INTERVALS`] of it.
pub(crate) fn backoff(backoffs: u32, poll: u64) -> u64 {
    let cap = poll.saturating_mul(BACKOFF_CAP_INTERVALS);
    1u64.checked_shl(backoffs)
        .map_or(cap, |doubled| poll.saturating_mul(doubled))
        .min(cap)
}

/// A re-read under way, on its own thread, and the sources it set out to ask.
struct Reading {
    answer: Receiver<Result<BTreeMap<String, Reread>>>,
    /// The sources due when it started.
    due: BTreeSet<String>,
    /// Each node it asked about, and the sources it was held on.
    asked: BTreeMap<String, BTreeSet<String>>,
}

/// What the loop knows about the nodes it holds for a source that did not answer.
///
/// One value for the whole run, threaded through the pass the way the workspace holds are:
/// the nodes held and why, when each source is next read, the re-read under way, and when
/// each held node's wait was last surfaced.
pub(crate) struct SourceHolds {
    held: BTreeMap<String, SourceHold>,
    /// The store the launch read the plan through, and the project: `None` only for a launch
    /// record naming no qualified project, which nothing re-reads.
    store: Option<(Store, QualifiedId)>,
    reading: Option<Reading>,
    /// When each source a node is held on is next read.
    schedule: BTreeMap<String, Due>,
    /// When a node held only for a prerequisite its read found is next read: such a hold
    /// names no source to pace it, so it is read on the backoff's first interval.
    missing_at: u64,
    poll: u64,
    surfaced: crate::release::Resurfacing,
    /// The run directory [`SCHEDULE_FILE`] is kept in, and the schedule last written there:
    /// `None` for holds no view reads, which a unit test makes.
    recorded: Option<(std::path::PathBuf, Option<BTreeMap<String, Due>>)>,
}

impl SourceHolds {
    /// The holds the run's record says are standing, taken up by a driver that re-reads
    /// them through the store configured from `dir` for `project`.
    ///
    /// Seeded from the record, unlike a workspace hold: what decided this one is the
    /// launch's read, which no driver repeats, so a fresh driver that started empty would
    /// dispatch every node the launch held. A source whose hold records a reset is read
    /// again a second after it, whichever driver journalled it; any other one interval from
    /// now, its backoff doubling from the re-reads after that one.
    pub(crate) fn of_run(
        holds: &BTreeMap<String, Vec<Value>>,
        dir: &std::path::Path,
        project: &str,
    ) -> Self {
        let held: BTreeMap<String, SourceHold> = holds
            .iter()
            .filter_map(|(node, reasons)| {
                reasons
                    .iter()
                    .find_map(SourceHold::of_payload)
                    .map(|hold| (node.clone(), hold))
            })
            .collect();
        let store = project
            .parse::<QualifiedId>()
            .ok()
            .map(|project| (Store::at(dir), project));
        let poll = poll_seconds();
        let now = crate::sys::now_millis();
        let mut schedule = BTreeMap::new();
        for unasked in held.values().flat_map(SourceHold::unanswered) {
            schedule
                .entry(unasked.name())
                .or_insert_with(|| match unasked.retry_at_ms() {
                    Some(at) => Due::fresh(now).after(Some(at), now, poll),
                    None => Due {
                        at_ms: now.saturating_add(poll.saturating_mul(1_000)),
                        backoffs: 0,
                        wait: Some(poll),
                    },
                });
        }
        Self {
            held,
            store,
            reading: None,
            schedule,
            missing_at: now.saturating_add(poll.saturating_mul(1_000)),
            poll,
            surfaced: crate::release::Resurfacing::new(),
            recorded: None,
        }
    }

    /// These holds, keeping their schedule in `run_dir`'s [`SCHEDULE_FILE`] from now on.
    pub(crate) fn recorded_in(mut self, run_dir: &std::path::Path) -> Self {
        self.recorded = Some((run_dir.to_path_buf(), None));
        self.record();
        self
    }

    /// Write the schedule where a view reads it, where it differs from what was last written:
    /// whole, staged beside the file and renamed over it, so a reader never meets half of
    /// one. A write that fails is left to the next change, and a view falls back to the
    /// backoff's first interval meanwhile.
    fn record(&mut self) {
        let Some((dir, written)) = &mut self.recorded else {
            return;
        };
        if written.as_ref() == Some(&self.schedule) {
            return;
        }
        let file = ScheduleFile {
            schema_version: SCHEDULE_SCHEMA_VERSION,
            sources: self.schedule.clone(),
        };
        let Ok(text) = serde_json::to_string(&file) else {
            return;
        };
        let staged = dir.join(format!(".{SCHEDULE_FILE}.{}", std::process::id()));
        let landed = std::fs::write(&staged, text)
            .and_then(|()| std::fs::rename(&staged, dir.join(SCHEDULE_FILE)));
        if landed.is_ok() {
            *written = Some(self.schedule.clone());
        } else {
            let _ = std::fs::remove_file(&staged);
        }
    }

    /// Forget every node no longer waiting to start: a node an edit dropped, and one that
    /// started or settled, holds nothing.
    pub(crate) fn retain(&mut self, waiting: impl Fn(&str) -> bool) {
        self.held.retain(|node, _| waiting(node));
        let named: BTreeSet<String> = self
            .held
            .values()
            .flat_map(SourceHold::unanswered)
            .map(Unanswered::name)
            .collect();
        self.schedule.retain(|source, _| named.contains(source));
        self.record();
    }

    /// Take up what a finished re-read answered, and say whether it changed a hold.
    ///
    /// `deps` is a node's dependencies in the run's graph as they stand now, so an edit that
    /// gave a held node the prerequisite a read found is what the next read clears it on.
    pub(crate) fn take_up(&mut self, deps: impl Fn(&str) -> Option<Vec<String>>) -> bool {
        let Some(reading) = &self.reading else {
            return false;
        };
        let answered = match reading.answer.try_recv() {
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err(crate::error::Error::Sibling {
                tool: "onetaskgraph",
                message: "the re-read ended without an answer".to_owned(),
            }),
            Ok(answered) => answered,
        };
        let Reading { due, asked, .. } = self.reading.take().expect("a re-read was under way");
        let now = crate::sys::now_millis();
        let reread = match answered {
            Ok(reread) => reread,
            Err(why) => {
                // The store refused the read outright, which says nothing about whether the
                // private source would answer: the holds stand as they are, and each source
                // due is read again on its backoff, as one that did not answer is.
                eprintln!(
                    "onepipeline: cannot read again the dependencies of {}, so they stay held: \
                     {why}",
                    quoted(&asked.keys().cloned().collect::<Vec<_>>())
                );
                for source in &due {
                    if let Some(scheduled) = self.schedule.get_mut(source) {
                        *scheduled = scheduled.after(None, now, self.poll);
                    }
                }
                self.missing_at = now.saturating_add(self.poll.saturating_mul(1_000));
                self.record();
                return false;
            }
        };
        // What the pass learned of each source: whether any read it asked answered, and the
        // first refusal it gave.
        let mut answered_by: BTreeSet<String> = BTreeSet::new();
        let mut refused_by: BTreeMap<String, Unanswered> = BTreeMap::new();
        for (node, read) in &reread {
            for unasked in &read.unanswered {
                refused_by
                    .entry(unasked.name())
                    .or_insert_with(|| unasked.clone());
            }
            for source in asked.get(node).into_iter().flatten() {
                if !read
                    .unanswered
                    .iter()
                    .any(|unasked| &unasked.name() == source)
                {
                    answered_by.insert(source.clone());
                }
            }
        }
        // Each source once: one due and refused again is scheduled by that refusal alone.
        let learned: BTreeSet<&String> = due.iter().chain(refused_by.keys()).collect();
        for source in learned {
            let mut scheduled = self
                .schedule
                .get(source)
                .copied()
                .unwrap_or_else(|| Due::fresh(now));
            if answered_by.contains(source) {
                scheduled.backoffs = 0;
            }
            scheduled = match refused_by.get(source) {
                Some(refusal) => scheduled.after(refusal.retry_at_ms(), now, self.poll),
                // Due and not asked: every node held on it was held on a source that refused
                // first. It is read with them, an interval from now.
                None if !answered_by.contains(source) => Due {
                    at_ms: now.saturating_add(self.poll.saturating_mul(1_000)),
                    ..scheduled
                },
                None => scheduled,
            };
            self.schedule.insert(source.clone(), scheduled);
        }
        if asked.values().any(BTreeSet::is_empty) {
            self.missing_at = now.saturating_add(self.poll.saturating_mul(1_000));
        }
        let mut changed = false;
        // A node the pass passed over is held on a source that refused it again: its hold
        // says what that source said this time, so every node held on it says the same.
        let passed_over: Vec<String> = asked
            .keys()
            .filter(|node| !reread.contains_key(*node))
            .cloned()
            .collect();
        for node in passed_over {
            let Some(hold) = self.held.get(&node) else {
                continue;
            };
            let refreshed = SourceHold {
                unanswered: hold
                    .unanswered
                    .iter()
                    .map(|unasked| {
                        refused_by
                            .get(&unasked.name())
                            .cloned()
                            .unwrap_or_else(|| unasked.clone())
                    })
                    .collect(),
                missing: hold.missing.clone(),
            };
            if &refreshed != hold {
                self.held.insert(node, refreshed);
                changed = true;
            }
        }
        for (node, read) in reread {
            if !self.held.contains_key(&node) {
                continue;
            }
            let Some(deps) = deps(&node) else {
                continue;
            };
            match hold_after(&deps, &read) {
                None => {
                    self.held.remove(&node);
                    changed = true;
                }
                Some(hold) => {
                    if self.held.get(&node) != Some(&hold) {
                        self.held.insert(node, hold);
                        changed = true;
                    }
                }
            }
        }
        self.retain(|_| true);
        changed
    }

    /// The sources due now, and the nodes a pass over them asks about: each node every source
    /// it is held on is due, and each node held only for a prerequisite once its interval is.
    /// A node held on two sources waits for the later of them, which
    /// `store::a_node_held_on_two_sources_is_read_again_once_both_are_due` drives.
    fn due(&self, now: u64) -> (BTreeSet<String>, BTreeMap<String, BTreeSet<String>>) {
        let due: BTreeSet<String> = self
            .schedule
            .iter()
            .filter(|(_, scheduled)| scheduled.at_ms <= now)
            .map(|(source, _)| source.clone())
            .collect();
        let asked = self
            .held
            .iter()
            .filter_map(|(node, hold)| {
                let held_on: BTreeSet<String> =
                    hold.unanswered.iter().map(Unanswered::name).collect();
                let ready = if held_on.is_empty() {
                    self.missing_at <= now
                } else {
                    held_on.is_subset(&due)
                };
                ready.then(|| (node.clone(), held_on))
            })
            .collect();
        (due, asked)
    }

    /// Start a re-read of every held node a due source holds, where one is due and none is
    /// under way.
    ///
    /// `nodes` is the run's graph: a held node's prerequisites are resolved against every
    /// node of it.
    pub(crate) fn read_again(&mut self, nodes: impl FnOnce() -> Vec<Node>) {
        if self.held.is_empty() || self.reading.is_some() {
            return;
        }
        let Some((store, project)) = self.store.clone() else {
            return;
        };
        let now = crate::sys::now_millis();
        let (due, asked) = self.due(now);
        if asked.is_empty() {
            // Each source due here holds only nodes another source holds too, which is not
            // due yet: it is read with that one, when the latest of them falls due — as
            // `store::a_node_held_on_two_sources_is_read_again_once_both_are_due` drives.
            for source in &due {
                let with = self
                    .held
                    .values()
                    .filter(|hold| {
                        hold.unanswered
                            .iter()
                            .any(|unasked| &unasked.name() == source)
                    })
                    .flat_map(|hold| hold.unanswered.iter())
                    .filter_map(|unasked| self.schedule.get(&unasked.name()))
                    .map(|scheduled| scheduled.at_ms)
                    .max()
                    .unwrap_or(now);
                if let Some(scheduled) = self.schedule.get_mut(source) {
                    scheduled.at_ms = with.max(now.saturating_add(1));
                }
            }
            self.record();
            return;
        }
        let nodes = nodes();
        let (answer, reading) = mpsc::channel();
        let sent = asked.clone();
        std::thread::spawn(move || {
            // A loop that has gone has nobody to answer, which is what a failed send says.
            let _ = answer.send(store.reread(&project, &nodes, &sent));
        });
        self.reading = Some(Reading {
            answer: reading,
            due,
            asked,
        });
    }

    pub(crate) fn held(&self) -> &BTreeMap<String, SourceHold> {
        &self.held
    }

    /// How long the loop may wait before it next has a re-read to start or take up.
    ///
    /// `Duration::MAX` where nothing is held, so a run whose plan was read whole waits on
    /// the channel alone — and where there is no store to read, which only a launch record
    /// naming no qualified project leaves.
    pub(crate) fn next_read(&self) -> Duration {
        if self.held.is_empty() || self.store.is_none() {
            return Duration::MAX;
        }
        if self.reading.is_some() {
            return TICK;
        }
        let now = crate::sys::now_millis();
        let missing_only = self.held.values().any(|hold| hold.unanswered.is_empty());
        self.schedule
            .values()
            .map(|scheduled| scheduled.at_ms)
            .chain(missing_only.then_some(self.missing_at))
            .min()
            .map_or(Duration::MAX, |at| {
                Duration::from_millis(at.saturating_sub(now))
            })
    }

    /// The wait `unasked`'s source's backoff last chose, as the surface says it.
    fn wait(&self, unasked: &Unanswered) -> Next {
        Next::In(
            self.schedule
                .get(&unasked.name())
                .and_then(|scheduled| scheduled.wait)
                .unwrap_or(self.poll),
        )
    }

    /// Surface every held node's wait that is due, and forget the nodes no longer held so a
    /// wait that ends is not repeated after it.
    ///
    /// Non-blocking, on the cadence a release wait is surfaced at: queued when the hold
    /// begins and when what it says changes, and an unchanged one again only on
    /// [`crate::release::Resurfacing`]'s doubling interval. A report rather than a decision:
    /// what clears the hold is the source answering, which nobody answers a surface to bring
    /// about.
    pub(crate) fn surface_waits(&mut self, paths: &RunPaths, journal: &mut Journal) -> Result<()> {
        for (node, hold) in &self.held {
            let surface = wait_surface(node, hold, &|unasked| self.wait(unasked));
            if !self.surfaced.due(paths, node, &surface.message) {
                continue;
            }
            let content = surface.message.clone();
            let queued = crate::engine::raised(paths, journal, surface)?;
            self.surfaced.queued(node, content, &queued);
        }
        let held = &self.held;
        self.surfaced.retain(|node| held.contains_key(node));
        Ok(())
    }
}

/// The surface a held node's wait raises, saying of each source the backoff wait `wait`
/// gives it.
fn wait_surface(node: &str, hold: &SourceHold, wait: &dyn Fn(&Unanswered) -> Next) -> Surface {
    let message = if hold
        .unanswered
        .iter()
        .any(|unasked| unasked.cause().is_none())
    {
        legacy_message(node, hold)
    } else {
        message(node, hold, wait)
    };
    Surface {
        id: 0,
        kind: WAIT_SURFACE_KIND.to_owned(),
        message,
        source: crate::channel::source::PROPOSAL.to_owned(),
        blocking: false,
        queued_at: crate::sys::now_millis(),
        abandoned: false,
        asker: None,
        workstream: Some(node.to_owned()),
        correlation: None,
    }
}

/// The wait a hold says, its remedy chosen by each source's cause: a rate limit is waited
/// out and asks nobody for anything; a credential refused asks for the credential; a source
/// unreachable asks that it be reachable; a tracker configured nowhere here asks that it be.
/// Each is driven through `next` by `tests/e2e/store.rs`:
/// `the_launch_status_and_source_wait_name_the_source_its_cause_and_its_next_read` (rate limit
/// and credential), `a_source_that_cannot_be_reached_is_said_so_and_its_node_released_once_it_is`
/// and `an_edge_into_a_tracker_configured_nowhere_holds_its_node_until_it_is_configured`.
fn message(node: &str, hold: &SourceHold, wait: &dyn Fn(&Unanswered) -> Next) -> String {
    if hold.unanswered.is_empty() {
        return format!(
            "node '{node}' is held: {}, so starting it now would start it before that \
             prerequisite. Give it those dependencies with an edit, or drop it, and it \
             dispatches the pass a read finds nothing missing; the edges are read again every \
             {}s. Nothing times this out and nothing will fail the node; or stop the run.",
            hold.said(wait),
            poll_seconds()
        );
    }
    let mut remedies: Vec<String> = Vec::new();
    let of = |cause: Cause| -> Vec<String> {
        hold.unanswered
            .iter()
            .filter(|unasked| unasked.cause() == Some(cause))
            .map(Unanswered::name)
            .collect()
    };
    let refused = of(Cause::Auth);
    if !refused.is_empty() {
        remedies.push(format!(
            "Supply the credential {} needs in the secrets file the store reads, or correct its \
             configuration — both are read again for every read, while a variable exported now \
             never reaches a driver already running.",
            quoted(&refused)
        ));
    }
    let unreached = of(Cause::Unavailable);
    if !unreached.is_empty() {
        remedies.push(format!(
            "Check that {} can be reached, or correct its configuration.",
            quoted(&unreached)
        ));
    }
    let unconfigured = of(Cause::Unconfigured);
    if !unconfigured.is_empty() {
        remedies.push(format!(
            "Configure the tracker {} names on this host — its plugin and, for a backend with \
             no identifier of its own, the public_alias every host declares for it.",
            quoted(&unconfigured)
        ));
    }
    let many = hold.unanswered.len() > 1;
    let what: Vec<String> = hold
        .unanswered
        .iter()
        .map(|unasked| {
            let cause = unasked.cause().unwrap_or(Cause::Unavailable);
            format!("{}: {}", self::what(&unasked.name(), cause), unasked.reason)
        })
        .collect();
    let next: Vec<String> = hold
        .unanswered
        .iter()
        .map(|unasked| {
            let subject = if many {
                format!("'{}'", unasked.name())
            } else {
                "it".to_owned()
            };
            when(unasked, wait(unasked), &subject)
        })
        .collect();
    let missing = if hold.missing.is_empty() {
        String::new()
    } else {
        format!(
            "; and the store records it depending on {}, which this run's graph does not",
            quoted(&hold.missing)
        )
    };
    let remedy = if remedies.is_empty() {
        String::new()
    } else {
        format!("{} ", remedies.join(" "))
    };
    format!(
        "node '{node}' is held: {}{missing}, so it may have a prerequisite this run cannot see. \
         {remedy}{}; it dispatches the pass a read of its edges is answered and finds nothing \
         missing. Nothing times this out and nothing will fail the node; or stop the run.",
        what.join("; "),
        sentence(&next.join("; "))
    )
}

/// The wait a hold a build before `cause` journalled says, as that build said it: an adopted
/// run's surface reads as it did until its first re-read records the cause.
fn legacy_message(node: &str, hold: &SourceHold) -> String {
    format!(
        "node '{node}' is held: {}, so it may have a prerequisite this run cannot see. Supply \
         the credential {} needs in the secrets file the store reads, or correct its \
         configuration — both are read again for every read, while a variable exported now \
         never reaches a driver already running — and it dispatches the pass a read of its \
         edges asks every source and finds nothing missing; they are read again every {}s. \
         Nothing times this out and nothing will fail the node; or stop the run.",
        hold.describe(),
        hold.sources(),
        poll_seconds()
    )
}

/// Ids as a person reads a list of them.
fn quoted(ids: &[String]) -> String {
    ids.iter()
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The interval a source's backoff starts at.
// llmlint: ignore[changed_behavior_has_e2e] the journeys set the variable to a usable value to drive the re-read (`tests/e2e/store.rs`'s private-source journeys read again on a one-second interval); an unset or unusable one selects the shipped minute, which no journey can wait out, so its parsing is held by `unanswered::tests::the_pace_is_a_positive_whole_number_of_seconds_or_the_default`, as `pool.rs` holds its own.
fn poll_seconds() -> u64 {
    std::env::var(POLL_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_POLL_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(reason: &str) -> Unanswered {
        Unanswered {
            who: Unasked::Source {
                source: onetaskgraph_plugin_api::SourceName::new("vault").expect("a source name"),
                cause: SourceCause::Auth,
            },
            reason: reason.to_owned(),
        }
    }

    fn limited(retry_at_ms: Option<u64>) -> Unanswered {
        Unanswered {
            who: Unasked::Source {
                source: onetaskgraph_plugin_api::SourceName::new("vault").expect("a source name"),
                cause: SourceCause::RateLimited { retry_at_ms },
            },
            reason: "rate limited".to_owned(),
        }
    }

    /// The hold's entry reads back whole: `cause` on every source, `retry_at_ms` only where a
    /// reset was given, and `missing` left out where the read found none.
    #[test]
    fn the_hold_entry_round_trips_and_omits_what_it_does_not_hold() {
        let unasked = SourceHold::new(vec![vault("auth: no token")], Vec::new()).expect("a hold");
        let entry = unasked.payload();
        assert_eq!(
            entry,
            json!({"kind": "source", "unanswered": [
                {"source": "vault", "reason": "auth: no token", "cause": "auth"}
            ]})
        );
        assert_eq!(SourceHold::of_payload(&entry), Some(unasked));

        let reset =
            SourceHold::new(vec![limited(Some(1_777_895_400_000))], Vec::new()).expect("a hold");
        assert_eq!(
            reset.payload()["unanswered"][0]["retry_at_ms"],
            json!(1_777_895_400_000_u64)
        );
        assert_eq!(SourceHold::of_payload(&reset.payload()), Some(reset));

        let found = SourceHold::new(Vec::new(), vec!["design".to_owned()]).expect("a hold");
        assert_eq!(SourceHold::of_payload(&found.payload()), Some(found));
        // Another hold, and one this build cannot read whole, are not this one.
        assert_eq!(
            SourceHold::of_payload(&json!({"kind": "release", "awaiting": []})),
            None
        );
        for unreadable in [
            json!({"kind": "source", "unanswered": [{"source": 7}]}),
            json!({"kind": "source", "unanswered": [
                {"source": "vault", "reason": "r", "cause": "weather"}
            ]}),
            json!({"kind": "source", "unanswered": [
                {"source": "vault", "reason": "r", "cause": "rate-limited", "retry_at_ms": "soon"}
            ]}),
            // A reset rides a rate limit alone, and a tracker is named by its designator.
            json!({"kind": "source", "unanswered": [
                {"source": "vault", "reason": "r", "cause": "auth", "retry_at_ms": 1}
            ]}),
            json!({"kind": "source", "unanswered": [
                {"source": "vault", "reason": "r", "retry_at_ms": 1}
            ]}),
            json!({"kind": "source", "unanswered": [
                {"source": "vault", "reason": "r", "cause": "unconfigured"}
            ]}),
        ] {
            assert_eq!(SourceHold::of_payload(&unreadable), None, "{unreadable}");
        }
        // Nor is one naming nothing that holds the node.
        assert_eq!(
            SourceHold::of_payload(&json!({"kind": "source", "unanswered": []})),
            None
        );
    }

    /// A hold a build before `cause` journalled reads back as it was written, and says what
    /// it said then.
    #[test]
    fn a_hold_journalled_before_its_cause_reads_and_says_what_it_did() {
        let written = json!({"kind": "source", "unanswered": [
            {"source": "vault", "reason": "authentication for this source failed: no token"}
        ]});
        let hold = SourceHold::of_payload(&written).expect("an older hold reads");
        assert_eq!(hold.payload(), written);
        assert_eq!(
            hold.describe(),
            "source 'vault', declared private, could not be asked what it records of its task: \
             authentication for this source failed: no token"
        );
    }

    /// The hold's entries, the surface kind and the pacing are what divergence entry 131
    /// records, and each entry reads back whole.
    #[test]
    fn the_hold_is_what_the_divergence_record_fixes_and_round_trips() {
        let record = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
        )
        .expect("the divergence record reads");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with("131."))
            .expect("the divergence record carries entry 131");
        let block: Value = serde_json::from_str(
            entry
                .split("```json")
                .nth(1)
                .and_then(|rest| rest.split("```").next())
                .expect("entry 131 carries the json block this test drives"),
        )
        .expect("entry 131's block is JSON");
        for key in [
            "hold",
            "hold_with_reset",
            "hold_unconfigured",
            "hold_with_missing",
            "hold_before_cause",
        ] {
            let written = &block[key];
            let hold = SourceHold::of_payload(written)
                .unwrap_or_else(|| panic!("entry 131's {key} reads"));
            assert_eq!(&hold.payload(), written, "{key} does not round-trip");
        }
        let causes: Vec<&str> = Cause::ALL.iter().map(|cause| cause.as_str()).collect();
        assert_eq!(block["causes"], json!(causes));
        assert_eq!(block["surface_kind"], WAIT_SURFACE_KIND);
        assert_eq!(block["poll_env"], POLL_ENV);
        assert_eq!(block["default_poll_seconds"], DEFAULT_POLL_SECONDS);
        assert_eq!(block["backoff_cap_intervals"], BACKOFF_CAP_INTERVALS);
        assert_eq!(block["after_reset_ms"], AFTER_RESET_MS);
        assert_eq!(block["surface_cadence_env"], crate::release::SURFACE_ENV);
        let surface = wait_surface(
            "build",
            &SourceHold::of_payload(&block["hold"]).expect("reads"),
            &|_| Next::Unknown,
        );
        assert_eq!(surface.kind, WAIT_SURFACE_KIND);
        assert_eq!(json!(surface.blocking), block["surface"]["blocking"]);
        assert_eq!(json!(surface.source), block["surface"]["source"]);
        assert_eq!(block["surface"]["workstream"], "the held node");
        assert_eq!(surface.workstream.as_deref(), Some("build"));
    }

    /// A re-read clears the hold only where it asked every source and found no prerequisite
    /// the graph does not give the node; otherwise it holds, saying which.
    #[test]
    fn a_reread_clears_the_hold_only_when_every_source_answered_and_nothing_is_missing() {
        let deps = vec!["design".to_owned()];
        let answered = Reread {
            deps: vec!["design".to_owned()],
            unanswered: Vec::new(),
        };
        assert_eq!(hold_after(&deps, &answered), None);

        let still = Reread {
            deps: vec!["design".to_owned()],
            unanswered: vec![vault("unavailable: down")],
        };
        assert_eq!(
            hold_after(&deps, &still),
            SourceHold::new(vec![vault("unavailable: down")], Vec::new())
        );

        let found = Reread {
            deps: vec![
                "review".to_owned(),
                "design".to_owned(),
                "review".to_owned(),
            ],
            unanswered: Vec::new(),
        };
        let held = hold_after(&deps, &found).expect("a prerequisite the graph lacks holds");
        assert_eq!(held.missing, ["review"]);
        assert!(held.describe().contains("'review'"), "{}", held.describe());
    }

    /// The backoff starts at one interval, doubles after each re-read a source still does not
    /// answer, and stops doubling at thirty; a reset decides the next read outright and
    /// leaves the backoff where it was.
    #[test]
    fn a_source_is_read_again_at_its_reset_or_on_a_doubling_capped_backoff() {
        let poll = 60;
        let waits: Vec<u64> = (0..8).map(|backoffs| backoff(backoffs, poll)).collect();
        assert_eq!(waits, [60, 120, 240, 480, 960, 1_800, 1_800, 1_800]);
        assert_eq!(backoff(u32::MAX, poll), 1_800);

        let now = 1_000_000;
        let refused = Due::fresh(now).after(None, now, poll);
        assert_eq!((refused.at_ms, refused.backoffs), (now + 60_000, 1));
        let again = refused.after(None, now, poll);
        assert_eq!((again.at_ms, again.backoffs), (now + 120_000, 2));
        let reset = again.after(Some(now + 1_800_000), now, poll);
        assert_eq!(
            (reset.at_ms, reset.backoffs, reset.wait),
            (now + 1_801_000, 2, None)
        );
    }

    /// A pass a source refuses again advances its schedule once — however it was due and
    /// whatever it refused — and a node it passed over takes up the same refusal; a pass in
    /// which the source answered one node starts its backoff again from one interval.
    #[test]
    fn a_pass_advances_each_source_once_and_an_answer_restarts_its_backoff() {
        let _held = crate::vcs::scratch_home_held();
        std::env::set_var(POLL_ENV, "10");
        let hold = || SourceHold::new(vec![limited(None)], Vec::new()).expect("a hold");
        let payloads: BTreeMap<String, Vec<Value>> = ["alpha", "beta"]
            .into_iter()
            .map(|node| (node.to_owned(), vec![hold().payload()]))
            .collect();
        let mut holds = SourceHolds::of_run(&payloads, std::path::Path::new("."), "plans:p");
        assert_eq!(
            (
                holds.schedule["vault"].backoffs,
                holds.schedule["vault"].wait
            ),
            (0, Some(10)),
            "a hold taken up is read again one interval out"
        );
        let pass = |holds: &mut SourceHolds, read: BTreeMap<String, Reread>| {
            let (answer, reading) = mpsc::channel();
            let asked: BTreeMap<String, BTreeSet<String>> = holds
                .held
                .keys()
                .map(|node| (node.clone(), BTreeSet::from(["vault".to_owned()])))
                .collect();
            holds.reading = Some(Reading {
                answer: reading,
                due: BTreeSet::from(["vault".to_owned()]),
                asked,
            });
            answer.send(Ok(read)).expect("the pass answers");
            holds.take_up(|_| Some(Vec::new()))
        };
        let refused = |reason: &str| Reread {
            deps: Vec::new(),
            unanswered: vec![Unanswered {
                reason: reason.to_owned(),
                ..limited(None)
            }],
        };
        // alpha refused again, twice; beta passed over each time.
        assert!(pass(
            &mut holds,
            [("alpha".to_owned(), refused("again"))].into()
        ));
        assert_eq!(holds.schedule["vault"].backoffs, 1);
        let before = crate::sys::now_millis();
        assert!(pass(
            &mut holds,
            [("alpha".to_owned(), refused("still"))].into()
        ));
        let scheduled = holds.schedule["vault"];
        assert_eq!((scheduled.backoffs, scheduled.wait), (2, Some(20)));
        assert!(scheduled.at_ms >= before + 20_000, "{scheduled:?}");
        assert_eq!(holds.held["beta"].unanswered[0].reason, "still");
        // alpha answered, beta refused: alpha is released and the backoff starts again.
        let answered = Reread {
            deps: Vec::new(),
            unanswered: Vec::new(),
        };
        assert!(pass(
            &mut holds,
            [
                ("alpha".to_owned(), answered),
                ("beta".to_owned(), refused("yet"))
            ]
            .into()
        ));
        assert!(!holds.held.contains_key("alpha"));
        let scheduled = holds.schedule["vault"];
        assert_eq!((scheduled.backoffs, scheduled.wait), (1, Some(10)));
        std::env::remove_var(POLL_ENV);
    }

    /// A pass the store failed outright, and one whose worker ended without an answer, leave
    /// every hold as it stood and put each source the pass set out to ask on its next backoff,
    /// rather than asking it again at once.
    ///
    /// Held here rather than end to end for the second: only a panic inside the store's read
    /// ends the worker without an answer, and no input a journey can give the linked store
    /// raises one. The first is driven end to end — the hold and the driver's line — by
    /// `store::a_private_source_that_cannot_authenticate_holds_each_node_until_it_answers`.
    #[test]
    fn a_pass_that_failed_or_never_answered_keeps_the_holds_and_backs_off() {
        let _held = crate::vcs::scratch_home_held();
        std::env::set_var(POLL_ENV, "10");
        let payloads: BTreeMap<String, Vec<Value>> = [(
            "alpha".to_owned(),
            vec![SourceHold::new(vec![limited(None)], Vec::new())
                .expect("a hold")
                .payload()],
        )]
        .into();
        for outcome in ["failed", "ended"] {
            let mut holds = SourceHolds::of_run(&payloads, std::path::Path::new("."), "plans:p");
            let standing = holds.held.clone();
            let (answer, reading) = mpsc::channel();
            holds.reading = Some(Reading {
                answer: reading,
                due: BTreeSet::from(["vault".to_owned()]),
                asked: BTreeMap::from([("alpha".to_owned(), BTreeSet::from(["vault".to_owned()]))]),
            });
            match outcome {
                "failed" => answer
                    .send(Err(crate::error::Error::Sibling {
                        tool: "onetaskgraph",
                        message: "the store refused this read".to_owned(),
                    }))
                    .expect("the pass answers"),
                _ => drop(answer),
            }
            let before = crate::sys::now_millis();
            assert!(!holds.take_up(|_| Some(Vec::new())), "{outcome}");
            assert!(
                holds.reading.is_none(),
                "{outcome}: the pass is still under way"
            );
            assert_eq!(holds.held, standing, "{outcome}: a hold moved");
            let scheduled = holds.schedule["vault"];
            assert_eq!(
                (scheduled.backoffs, scheduled.wait),
                (1, Some(10)),
                "{outcome}"
            );
            assert!(
                scheduled.at_ms >= before + 10_000,
                "{outcome}: {scheduled:?}"
            );
        }
        std::env::remove_var(POLL_ENV);
    }

    /// The driver keeps each source's schedule where a view in another process reads it,
    /// rewritten as it changes; the held line then names a backing-off source's next read and
    /// the wait it ends, and a file in another schema, or none, leaves the backoff's first
    /// interval to say.
    #[test]
    fn a_held_line_names_the_next_read_the_driver_recorded() {
        let _held = crate::vcs::scratch_home_held();
        std::env::set_var(POLL_ENV, "10");
        let dir = std::env::temp_dir().join(format!("source-schedule-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a run directory");
        let hold = SourceHold::new(vec![limited(None)], Vec::new()).expect("a hold");
        let payloads: BTreeMap<String, Vec<Value>> =
            [("alpha".to_owned(), vec![hold.payload()])].into();
        let mut holds =
            SourceHolds::of_run(&payloads, std::path::Path::new("."), "plans:p").recorded_in(&dir);
        assert_eq!(schedule_of(&dir).as_ref(), Some(&holds.schedule));
        holds.schedule.insert(
            "vault".to_owned(),
            Due {
                at_ms: 1_777_895_400_000,
                backoffs: 3,
                wait: Some(80),
            },
        );
        holds.record();
        let schedule = schedule_of(&dir).expect("the schedule reads back");
        assert_eq!(
            hold.describe_scheduled(Some(&schedule)),
            "source 'vault', declared private, is rate limited, so it could not be asked what \
             it records of this task: rate limited; no reset time was given, so it is read \
             again at 2026-05-04 11:50:00 UTC, 80s after it last did not answer, the wait \
             doubling each time it still does not answer, up to 300s (30 poll intervals); \
             nothing times this out"
        );
        std::fs::write(
            dir.join(SCHEDULE_FILE),
            json!({"schema_version": 2, "sources": {}}).to_string(),
        )
        .expect("a file of another schema");
        assert_eq!(schedule_of(&dir), None);
        assert_eq!(
            hold.describe_scheduled(schedule_of(&dir).as_ref()),
            hold.describe()
        );
        assert!(hold.describe().contains("on a backoff starting at 10s"));
        std::fs::remove_dir_all(&dir).expect("the run directory is removed");
        std::env::remove_var(POLL_ENV);
    }

    /// The launch notice names the source, its cause and its next read, in one line with no
    /// run of spaces in it; a rate-limited hold asks for no credential, and an
    /// authentication one does.
    #[test]
    fn the_notice_and_surface_say_the_cause_and_the_next_read() {
        let _held = crate::vcs::scratch_home_held();
        std::env::remove_var(POLL_ENV);
        let held: BTreeMap<String, SourceHold> = [(
            "changelog".to_owned(),
            SourceHold::new(vec![limited(Some(1_777_895_400_000))], Vec::new()).expect("a hold"),
        )]
        .into();
        assert_eq!(
            notice(&held).expect("a notice"),
            "onepipeline: source 'vault', declared private, is rate limited and could not be \
             asked about the tasks whose reads name it, 'changelog': rate limited — the plan \
             loads, and each of those nodes is held, never dispatched; it is read again at \
             2026-05-04 11:50:00 UTC, the reset the provider gave. Nothing times this out."
        );
        let limited_surface = wait_surface("changelog", &held["changelog"], &|_| Next::In(60));
        assert!(
            !limited_surface.message.contains("credential"),
            "{}",
            limited_surface.message
        );
        let auth = SourceHold::new(vec![vault("no token")], Vec::new()).expect("a hold");
        let auth_surface = wait_surface("changelog", &auth, &|_| Next::In(120));
        assert!(
            auth_surface
                .message
                .contains("Supply the credential 'vault' needs")
                && auth_surface.message.contains("It is read again in 120s"),
            "{}",
            auth_surface.message
        );
        assert!(
            !auth_surface.message.contains("  "),
            "{}",
            auth_surface.message
        );
    }

    /// The pace is the variable where it is a positive whole number of seconds, and the
    /// default for anything else, unset included.
    #[test]
    fn the_pace_is_a_positive_whole_number_of_seconds_or_the_default() {
        let _held = crate::vcs::scratch_home_held();
        for unusable in ["", "0", "-5", "soon", "1.5"] {
            std::env::set_var(POLL_ENV, unusable);
            assert_eq!(
                poll_seconds(),
                DEFAULT_POLL_SECONDS,
                "{POLL_ENV}={unusable:?}"
            );
        }
        std::env::set_var(POLL_ENV, "7200");
        assert_eq!(poll_seconds(), 7200);
        std::env::remove_var(POLL_ENV);
        assert_eq!(poll_seconds(), DEFAULT_POLL_SECONDS);
    }
}
