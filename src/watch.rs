//! `onepipeline watch` — a bounded, resumable wait on one run.
//!
//! Watching dispatched work is the one supervisory duty this crate had no verb
//! behind, so every supervisor wrote a shell loop around `monitor` and `status`
//! and each one went silent differently. The two failures that cost real runs
//! were both prose matching: one watch matched a word inside an embedded
//! host-health report and called a healthy dispatch dead, and another filtered
//! out the one line saying a question was waiting and went quiet while
//! twenty-six updates queued behind a question asked three times.
//!
//! So this verb blocks, emits a line per event a supervisor acts on, says
//! something on an interval when nothing has happened, and returns a **status**
//! rather than a sentence:
//!
//! | the run settled `complete` | [`EXIT_SUCCESS`] |
//! | nothing is driving it | [`EXIT_NOTHING_DRIVING`] |
//! | a blocking surface is waiting | [`EXIT_SURFACE_WAITING`] |
//! | the wait elapsed, run still live | [`EXIT_WATCH_ELAPSED`] |
//!
//! Every heartbeat states how many planner surfaces are unread and of which
//! kinds. That is the property the verb is worth having for: a caller that
//! matches only on event lines cannot lose the signal that a question is
//! waiting, because the signal rides the line that is emitted when nothing is
//! happening.
//!
//! Everything here **reads**, exactly as [`crate::views`] does. A watch takes no
//! lock a writer needs, consumes no surface, and records nothing: watching a run
//! is not supervising it.

use std::io::Write;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cli::{WatchArgs, WatchUntil, WATCH_CURSOR_VERSION};
use crate::error::{
    Error, Result, EXIT_NOTHING_DRIVING, EXIT_SUCCESS, EXIT_SURFACE_WAITING, EXIT_WATCH_ELAPSED,
};
use crate::event::{Envelope, PipelineKind};
use crate::filter::EventFilter;
use crate::graph::{self, GraphState};
use crate::journal;
use crate::ledger::RunPaths;
use crate::views::{self, RunView, Unread};

/// How often the wait re-reads the run.
///
/// A supervisory latency rather than an interactive one: a second is below any
/// interval a heartbeat is worth stating and far below the time it takes to act
/// on what a line says, and each pass costs a read of the run's ledger — which a
/// watch left open for an hour pays three and a half thousand times.
const POLL: Duration = Duration::from_secs(1);

/// The events a supervisor acts on.
///
/// This crate's own kinds and a chosen few of them, which is what makes "one
/// line per meaningful event" a stream a person reads rather than the whole
/// store: the siblings' token-by-token detail is what `monitor --all` is for.
///
/// Every class the run this verb comes from lost is here. A **graph edit**,
/// whichever author issued it — four destructive edits matched no wake condition
/// on that run and what eventually surfaced them was the run dying — and a
/// refused one beside it, because an edit that did not land is as much a thing
/// to act on as one that did. A **node settling**, at any outcome. A **surface
/// being raised**, before anybody has read it. And the three facts that end a
/// supervisor's assumptions about the run as a whole: a decision beginning to
/// hold a subtree, that hold clearing, and the run being stopped.
const MEANINGFUL: [PipelineKind; 8] = [
    PipelineKind::EditCommitted,
    PipelineKind::EditRejected,
    PipelineKind::NodeSettled,
    PipelineKind::PlannerSurfaceQueued,
    PipelineKind::DecisionPending,
    PipelineKind::DecisionCleared,
    PipelineKind::CompletionRequested,
    PipelineKind::RunStopped,
];

/// Why a watch returned.
///
/// A closed set with a code each, so a caller branches on the status and never
/// on the words beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    Settled,
    SurfaceWaiting,
    NothingDriving,
    Elapsed,
}

impl Ending {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Settled => "settled",
            Self::SurfaceWaiting => "surface-waiting",
            Self::NothingDriving => "nothing-driving",
            Self::Elapsed => "elapsed",
        }
    }

    const fn exit_code(self) -> i32 {
        match self {
            Self::Settled => EXIT_SUCCESS,
            Self::SurfaceWaiting => EXIT_SURFACE_WAITING,
            Self::NothingDriving => EXIT_NOTHING_DRIVING,
            Self::Elapsed => EXIT_WATCH_ELAPSED,
        }
    }
}

/// Block until the run needs a supervisor, or until the wait runs out.
///
/// The run and the profile are resolved by the caller, so a run that does not
/// exist or a profile this run does not have refuses **before** anything blocks:
/// a watch that waited five minutes to report a typo would be worse than the
/// loop it replaces.
pub(crate) fn watch(args: &WatchArgs, paths: &RunPaths, filter: &EventFilter) -> Result<i32> {
    let mut cursor = match args.cursor.as_deref() {
        Some(token) => resolve_cursor(paths, token)?,
        None => 0,
    };
    // Checked, because the seconds are a caller's: `Instant` addition panics on
    // overflow, and a wait longer than this host's clock can hold is a value to
    // refuse rather than a reason to abort the process.
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(args.timeout))
        .ok_or_else(|| {
            Error::Invalid(format!(
                "a wait of {} seconds is further ahead than this host's clock can name; \
                 give `--timeout` a value it can reach",
                args.timeout
            ))
        })?;
    let tick = Duration::from_secs(args.tick_interval);
    let mut quiet_since = Instant::now();
    let mut out = Emitter::new();

    loop {
        let view = RunView::open(paths)?;
        let (mut fresh, at) = journal::finished_after(&paths.journal(), cursor);
        cursor = at;
        journal::merge_order(&mut fresh);
        for event in fresh
            .iter()
            .filter(|event| meaningful(event) && filter.matches(event))
        {
            out.event(&view, event)?;
            quiet_since = Instant::now();
        }

        if let Some(ending) = concluded(&view, paths, args.until) {
            return out.returned(&view, ending, cursor);
        }
        if Instant::now() >= deadline {
            return out.returned(&view, Ending::Elapsed, cursor);
        }
        if !tick.is_zero() && quiet_since.elapsed() >= tick {
            out.heartbeat(&view)?;
            quiet_since = Instant::now();
        }
        std::thread::sleep(POLL);
    }
}

/// Whether this is an event a supervisor acts on.
///
/// Asked of the kind rather than of the source, so a sibling that one day emits
/// a kind spelled like one of these is not folded into this crate's vocabulary
/// by accident: [`PipelineKind::from_wire`] answers `None` for everything that
/// is not this library's own.
fn meaningful(event: &Envelope) -> bool {
    PipelineKind::from_wire(&event.kind).is_some_and(|kind| MEANINGFUL.contains(&kind))
}

/// The terminal condition this pass reached, if it reached one.
///
/// **Settled here is the graph being `complete`**, which is the reading an
/// attached `start` already returns on and deliberately not "the loop has
/// nothing left to do": a run whose one node failed has converged, and reporting
/// that as a run that settled would hand a supervisor exit `0` over work nobody
/// finished. Such a run reaches the caller as [`Ending::NothingDriving`] — the
/// state to intervene in — exactly as it does through `start`.
///
/// The order after it is what a supervisor does about each, hardest fact first.
/// Nothing driving outranks a waiting surface for the same reason `reply`
/// refuses one: an answer handed to a run nobody will drive again is delivered
/// to nothing, and `adopt` comes first.
fn concluded(view: &RunView, paths: &RunPaths, until: WatchUntil) -> Option<Ending> {
    let statuses = view.state.statuses();
    if !statuses.is_empty() && graph::state_of(&statuses) == GraphState::Complete {
        return Some(Ending::Settled);
    }
    if view.liveness().is_undriven() {
        return Some(Ending::NothingDriving);
    }
    if until == WatchUntil::Surface && views::blocking_surface(paths) {
        return Some(Ending::SurfaceWaiting);
    }
    None
}

fn render_cursor(at: u64) -> String {
    format!("{WATCH_CURSOR_VERSION}:{at}")
}

/// The byte a cursor token names **in this run's journal**, or a refusal.
///
/// Two questions, and the second is the one that costs. A token that parses is
/// only a number; whether that number is a place in *this* store is a fact about
/// the store, and a watch handed a cursor from another run — or from this one
/// before its journal was healed — would read past the end of the file, find
/// nothing, and report a run where nothing was happening. It is refused instead,
/// naming both numbers.
///
/// A boundary as well as a length: every record this crate appends ends in a
/// newline, so a cursor that does not sit just past one is pointing into the
/// middle of a record, and resuming there would hand the caller a fragment as
/// though it were an event.
fn resolve_cursor(paths: &RunPaths, token: &str) -> Result<u64> {
    let at = parse_cursor(token)?;
    let journal = paths.journal();
    let held = std::fs::metadata(&journal).map_or(0, |file| file.len());
    if at > held {
        return Err(Error::Invalid(format!(
            "cursor '{token}' resumes at byte {at} of run '{}', whose store holds {held}; \
             a cursor is only readable by the run the watch that printed it was watching",
            paths.run
        )));
    }
    if at > 0 && !ends_a_record(&journal, at) {
        return Err(Error::Invalid(format!(
            "cursor '{token}' resumes at byte {at} of run '{}', which is inside a record \
             rather than after one; a cursor is what an earlier `onepipeline watch` \
             printed, and never a byte count of its own",
            paths.run
        )));
    }
    Ok(at)
}

/// Whether the byte before `at` ends a record.
fn ends_a_record(journal: &std::path::Path, at: u64) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(journal) else {
        return false;
    };
    if file.seek(SeekFrom::Start(at - 1)).is_err() {
        return false;
    }
    let mut last = [0u8; 1];
    file.read_exact(&mut last).is_ok() && last[0] == b'\n'
}

/// The byte a cursor token names, or a refusal saying what was read.
///
/// External input like any other: a token is typed at a command line, and one
/// this build cannot place is refused by name rather than resumed from as though
/// its digits meant a byte count.
fn parse_cursor(token: &str) -> Result<u64> {
    let refusal = || {
        Error::Invalid(format!(
            "'{token}' is not a cursor this build reads; a cursor is what an earlier \
             `onepipeline watch` printed, spelled `{WATCH_CURSOR_VERSION}:<byte>`"
        ))
    };
    let (version, at) = token.split_once(':').ok_or_else(refusal)?;
    if version != WATCH_CURSOR_VERSION {
        return Err(refusal());
    }
    at.parse().map_err(|_| refusal())
}

/// The two forms a watch writes, on the two descriptors that keep them apart.
///
/// The human stream goes to standard error and the machine-readable one to
/// standard output, which is the split an attached `start` already makes and for
/// the same reason: a script reads stdout as NDJSON while a terminal beside it
/// follows the run. Each line is flushed as it is written — a watch is a
/// **blocking** verb, and a consumer reading it incrementally through a pipe
/// would otherwise see nothing until the process exits, which is the silence
/// this whole verb exists to end.
struct Emitter {
    machine: std::io::Stdout,
    human: std::io::Stderr,
}

impl Emitter {
    fn new() -> Self {
        Self {
            machine: std::io::stdout(),
            human: std::io::stderr(),
        }
    }

    /// One meaningful event, as a line and as the envelope itself.
    ///
    /// The machine form carries the whole envelope rather than a rendering of
    /// it, exactly as `next` hands its caller the events it read: a consumer
    /// that needs a field this crate does not put on the line should not have to
    /// go back to the store for it.
    fn event(&mut self, view: &RunView, event: &Envelope) -> Result<()> {
        self.say(
            &views::event_line(view, event),
            &json!({"watch": "event", "event": event}),
        )
    }

    /// One heartbeat: that the watch is alive, and what is waiting unread.
    fn heartbeat(&mut self, view: &RunView) -> Result<()> {
        let unread = view.unread();
        self.say(
            &format!(
                "-- watching {}  {}  {}",
                view.paths.run,
                views::liveness_word(view),
                unread_phrase(&unread)
            ),
            &json!({
                "watch": "heartbeat",
                "run_id": view.paths.run,
                "unread": unread_json(&unread),
            }),
        )
    }

    /// The last thing a watch says: why it returned, what is unread, and the
    /// cursor the next one resumes from.
    fn returned(&mut self, view: &RunView, ending: Ending, cursor: u64) -> Result<i32> {
        let unread = view.unread();
        let cursor = render_cursor(cursor);
        self.say(
            &format!(
                "-- watch {} {}  {}  cursor {cursor}",
                view.paths.run,
                ending.as_str(),
                unread_phrase(&unread)
            ),
            &json!({
                "watch": "return",
                "run_id": view.paths.run,
                "condition": ending.as_str(),
                "exit": ending.exit_code(),
                "cursor": cursor,
                "unread": unread_json(&unread),
            }),
        )?;
        Ok(ending.exit_code())
    }

    /// Write one line of each form, flushing both.
    ///
    /// A write that fails is the caller's pipe closing, which is not this run's
    /// failure — but it is the end of what this watch can report, so it refuses
    /// rather than going on emitting into a descriptor nobody is reading.
    fn say(&mut self, human: &str, machine: &Value) -> Result<()> {
        let broken = |what: &str, error: std::io::Error| {
            Error::Invalid(format!("the watch could not write to {what}: {error}"))
        };
        writeln!(self.machine, "{machine}").map_err(|e| broken("standard output", e))?;
        self.machine
            .flush()
            .map_err(|e| broken("standard output", e))?;
        writeln!(self.human, "{human}").map_err(|e| broken("standard error", e))?;
        self.human
            .flush()
            .map_err(|e| broken("standard error", e))?;
        Ok(())
    }
}

/// How many planner surfaces are unread and of which kinds, as one clause.
///
/// A zero is said out loud rather than left out. "Nothing is waiting" and "this
/// line does not mention what is waiting" read identically to a script and to a
/// tired person, and telling them apart is the whole point of putting the count
/// on the quiet line.
fn unread_phrase(unread: &Unread) -> String {
    match unread.count {
        0 => "0 unread planner surfaces".to_string(),
        count => format!("{count} unread planner surface(s): {}", unread.phrase()),
    }
}

/// The same answer, as the machine form carries it.
fn unread_json(unread: &Unread) -> Value {
    json!({
        "count": unread.count,
        "oldest_seconds": unread.oldest_seconds,
        "kinds": unread
            .kinds
            .iter()
            .map(|(kind, count)| json!({"kind": kind, "count": count}))
            .collect::<Vec<Value>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_terminal_condition_returns_a_status_of_its_own() {
        let endings = [
            Ending::Settled,
            Ending::SurfaceWaiting,
            Ending::NothingDriving,
            Ending::Elapsed,
        ];
        let codes: std::collections::BTreeSet<i32> =
            endings.iter().map(|end| end.exit_code()).collect();
        assert_eq!(codes.len(), endings.len(), "two endings share a status");
        // And the one this crate already assigns elsewhere is reused rather than
        // given a fresh number.
        assert_eq!(Ending::NothingDriving.exit_code(), EXIT_NOTHING_DRIVING);
        assert_eq!(Ending::Settled.exit_code(), EXIT_SUCCESS);
    }

    #[test]
    fn a_cursor_round_trips_and_anything_else_is_refused() {
        assert_eq!(parse_cursor(&render_cursor(4096)).expect("reads"), 4096);
        for token in ["", "4096", "2:4096", "1:", "1:x", "1:-1", "1:4096:0"] {
            let refused = parse_cursor(token).expect_err("refused");
            assert!(
                refused
                    .to_string()
                    .contains("is not a cursor this build reads"),
                "{token:?}: {refused}"
            );
        }
    }

    /// The divergence record is where this verb's surface is *proposed*, and the
    /// meaningful set is the part of it a reader has to trust: an entry naming a
    /// kind this build does not emit, or silent about one it does, is a proposal
    /// for something nobody built. The two are held together here rather than by
    /// a reader noticing.
    #[test]
    fn the_divergence_entry_names_exactly_the_kinds_this_build_calls_meaningful() {
        let record = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("docs")
                .join("contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split_once("\n## 58.")
            .expect("this verb is recorded under entry 58")
            .1;
        let entry = entry.split_once("\n## ").map_or(entry, |(head, _)| head);

        for kind in crate::event::PIPELINE_KINDS {
            let named = entry.contains(&format!("`{}`", kind.as_str()));
            assert_eq!(
                named,
                MEANINGFUL.contains(kind),
                "the entry and this build disagree about whether `{}` is a kind a watch \
                 emits",
                kind.as_str()
            );
        }
    }

    #[test]
    fn a_wait_longer_than_the_clock_can_name_is_refused_rather_than_panicking() {
        assert!(Instant::now()
            .checked_add(Duration::from_secs(u64::MAX))
            .is_none());
    }

    #[test]
    fn an_empty_queue_says_so_rather_than_saying_nothing() {
        let quiet = unread_phrase(&Unread::default());
        assert!(quiet.contains('0'), "{quiet}");
        assert_eq!(unread_json(&Unread::default())["count"], json!(0));
    }
}
