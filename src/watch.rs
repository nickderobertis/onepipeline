//! `onepipeline watch` — a bounded, resumable wait on one run.
//!
//! What the verb promises is entry 58 of `docs/contract-divergences.md`, which is
//! the proposal it waits on and the record of what this cost before there was a
//! verb; the README documents it for a caller. Neither is restated here.
//!
//! The one thing worth saying beside the code: everything in this module
//! **reads**, exactly as [`crate::views`] does. A watch takes no lock a writer
//! needs, consumes no surface and records nothing, so any number of them may sit
//! on a live run at once — watching a run is not supervising it.

use std::io::Write;
use std::time::{Duration, Instant};

use crate::cli::{WatchArgs, WatchUntil, WATCH_CURSOR_VERSION};
use crate::error::{
    Error, Result, EXIT_NOTHING_DRIVING, EXIT_SUCCESS, EXIT_SURFACE_WAITING, EXIT_WATCH_ELAPSED,
};
use crate::event::{Envelope, PipelineKind, Source};
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

/// The events a supervisor acts on: a closed set of *this crate's* own kinds.
///
/// The siblings' token-by-token detail is what `monitor --all` is for. Divergence
/// entry 58 argues the selection; what matters here is that it is closed, and
/// that an edit is in it whichever author issued it and whether or not it landed.
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

/// The two wire fields a return record states about why it ended, written from
/// the one value the process exits with.
///
/// Hand-written rather than derived because the pair is the whole promise: a
/// record that took `condition` and `exit` as two fields could be given a word
/// and a status that disagree, which is the caller reading prose again.
impl serde::Serialize for Ending {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut record = serializer.serialize_map(Some(2))?;
        record.serialize_entry("condition", self.as_str())?;
        record.serialize_entry("exit", &self.exit_code())?;
        record.end()
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
        None => Cursor::start(&paths.run),
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
        let (mut fresh, at) = journal::finished_after(&paths.journal(), cursor.at);
        cursor.at = at;
        journal::merge_order(&mut fresh);
        for event in fresh
            .iter()
            .filter(|event| meaningful(event) && filter.matches(event))
        {
            out.event(&view, event)?;
            quiet_since = Instant::now();
        }

        if let Some(ending) = concluded(&view, paths, args.until) {
            return out.returned(&view, ending, &cursor);
        }
        if Instant::now() >= deadline {
            return out.returned(&view, Ending::Elapsed, &cursor);
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
/// Asked of the source **and** the kind, because either alone admits the other's
/// events. The kind is a wire string that no library owns: this crate's stream is
/// merged with two siblings' before it reaches here, and a sibling that one day
/// spells a kind the way this one does would be folded into this crate's
/// vocabulary by a kind test alone — emitting, as a node settling, something that
/// settled no node. [`PipelineKind::from_wire`] narrows the string to this
/// library's own words; [`Source::Pipeline`] is what says the record came from
/// this library.
fn meaningful(event: &Envelope) -> bool {
    event.source == Source::Pipeline
        && PipelineKind::from_wire(&event.kind).is_some_and(|kind| MEANINGFUL.contains(&kind))
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

/// A place in one run's journal, as a later invocation is handed it.
///
/// A type rather than a string, so the token a caller is given renders and parses
/// in exactly one spelling. It carries the run as well as the byte: a byte alone
/// is a place in every journal there is, so without the run a cursor pasted
/// against the wrong one resumes rather than being refused.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cursor {
    run: String,
    at: u64,
}

impl Cursor {
    fn start(run: &str) -> Self {
        Self {
            run: run.to_string(),
            at: 0,
        }
    }
}

impl std::fmt::Display for Cursor {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{WATCH_CURSOR_VERSION}:{}:{}", self.run, self.at)
    }
}

impl serde::Serialize for Cursor {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The byte a cursor token names **in this run's journal**, or a refusal.
///
/// Four checks, in order: the token is this build's spelling, it names *this*
/// run, its byte is within the journal, and that byte sits just past a newline.
/// The last is a boundary check and not a second length check — every record here
/// ends in a newline, so a byte in range but mid-record would resume by handing
/// the caller a fragment as though it were an event.
fn resolve_cursor(paths: &RunPaths, token: &str) -> Result<Cursor> {
    let Cursor { run, at } = parse_cursor(token)?;
    if run != paths.run {
        return Err(Error::Invalid(format!(
            "cursor '{token}' was printed by a watch of run '{run}', and this is a watch of              run '{}'; a cursor is only readable by the run it was printed for",
            paths.run
        )));
    }
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
    Ok(Cursor { run, at })
}

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

/// The run and the byte a cursor token names, or a refusal saying what was read.
///
/// External input like any other: a token is typed at a command line, and one
/// this build cannot place is refused by name rather than resumed from as though
/// its digits meant a byte count.
///
/// The byte is taken from the **last** separator rather than the second, so a run
/// whose id contains one is read back as the id it was printed as instead of
/// being refused for a colon nobody chose.
fn parse_cursor(token: &str) -> Result<Cursor> {
    let refusal = || {
        Error::Invalid(format!(
            "'{token}' is not a cursor this build reads; a cursor is what an earlier \
             `onepipeline watch` printed, spelled `{WATCH_CURSOR_VERSION}:<run>:<byte>`"
        ))
    };
    let (version, rest) = token.split_once(':').ok_or_else(refusal)?;
    if version != WATCH_CURSOR_VERSION {
        return Err(refusal());
    }
    let (run, at) = rest.rsplit_once(':').ok_or_else(refusal)?;
    if run.is_empty() {
        return Err(refusal());
    }
    Ok(Cursor {
        run: run.to_string(),
        at: at.parse().map_err(|_| refusal())?,
    })
}

/// One line of the machine-readable form.
///
/// A serialized type rather than an object built by hand at each site: the tag
/// and the fields are the wire contract a caller branches on, and three
/// `json!` literals would be three places for it to drift. Externally tagged on
/// `watch`, so the key that says which record this is arrives beside the fields
/// that only that record has.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "watch", rename_all = "kebab-case")]
enum Record<'a> {
    /// One meaningful event, as the envelope itself rather than as a rendering
    /// of it — exactly as `next` hands its caller the events it read, so a
    /// consumer needing a field this crate does not put on the line never has to
    /// go back to the store for it.
    Event { event: &'a Envelope },
    Heartbeat {
        run_id: &'a str,
        unread: UnreadRecord<'a>,
    },
    Return {
        run_id: &'a str,
        /// The word and the status, both written from the one [`Ending`] the
        /// process is about to exit with, so the record cannot name a condition
        /// its own exit code contradicts.
        #[serde(flatten)]
        ending: Ending,
        cursor: &'a Cursor,
        unread: UnreadRecord<'a>,
    },
}

#[derive(Debug, serde::Serialize)]
struct UnreadRecord<'a> {
    count: usize,
    /// Absent as `null` rather than as a zero, which would read as a queue
    /// somebody had just emptied.
    oldest_seconds: Option<u64>,
    kinds: Vec<UnreadKind<'a>>,
}

#[derive(Debug, serde::Serialize)]
struct UnreadKind<'a> {
    kind: &'a str,
    count: usize,
}

impl<'a> UnreadRecord<'a> {
    fn of(unread: &'a Unread) -> Self {
        Self {
            count: unread.count,
            oldest_seconds: unread.oldest_seconds,
            kinds: unread
                .kinds
                .iter()
                .map(|(kind, count)| UnreadKind {
                    kind,
                    count: *count,
                })
                .collect(),
        }
    }
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

    fn event(&mut self, view: &RunView, event: &Envelope) -> Result<()> {
        self.say(&views::event_line(view, event), &Record::Event { event })
    }

    fn heartbeat(&mut self, view: &RunView) -> Result<()> {
        let unread = view.unread();
        self.say(
            &format!(
                "-- watching {}  {}  {}",
                view.paths.run,
                views::liveness_word(view),
                unread_phrase(&unread)
            ),
            &Record::Heartbeat {
                run_id: &view.paths.run,
                unread: UnreadRecord::of(&unread),
            },
        )
    }

    /// The last thing a watch says: why it returned, what is unread, and the
    /// cursor the next one resumes from.
    fn returned(&mut self, view: &RunView, ending: Ending, cursor: &Cursor) -> Result<i32> {
        let unread = view.unread();
        self.say(
            &format!(
                "-- watch {} {}  {}  cursor {cursor}",
                view.paths.run,
                ending.as_str(),
                unread_phrase(&unread)
            ),
            &Record::Return {
                run_id: &view.paths.run,
                ending,
                cursor,
                unread: UnreadRecord::of(&unread),
            },
        )?;
        Ok(ending.exit_code())
    }

    /// Write one line of each form, flushing both.
    ///
    /// A write that fails is the caller's pipe closing, which is not this run's
    /// failure — but it is the end of what this watch can report, so it refuses
    /// rather than going on emitting into a descriptor nobody is reading.
    ///
    /// **The machine record goes last**, after its human counterpart is written
    /// and flushed, because a refusal here becomes [`EXIT_REFUSED`] and the last
    /// machine record is where a caller reads the exit it should have got. Were
    /// the order the other way, a stderr that broke after the return record was
    /// flushed would leave stdout declaring exit `0` on a process that exited
    /// `2` — a caller branching on the machine form, which is the one thing this
    /// verb promises, would read a settled run off a watch that refused.
    ///
    /// [`EXIT_REFUSED`]: crate::error::EXIT_REFUSED
    fn say(&mut self, human: &str, machine: &Record<'_>) -> Result<()> {
        let broken = |what: &str, error: std::io::Error| {
            Error::Invalid(format!("the watch could not write to {what}: {error}"))
        };
        let machine = serde_json::to_string(machine)
            .map_err(|e| Error::Invalid(format!("the watch could not render a record: {e}")))?;
        writeln!(self.human, "{human}").map_err(|e| broken("standard error", e))?;
        self.human
            .flush()
            .map_err(|e| broken("standard error", e))?;
        writeln!(self.machine, "{machine}").map_err(|e| broken("standard output", e))?;
        self.machine
            .flush()
            .map_err(|e| broken("standard output", e))?;
        Ok(())
    }
}

/// How many planner surfaces are unread and of which kinds, as one clause.
///
/// A zero is said out loud rather than left out: "nothing is waiting" and "this
/// line does not mention what is waiting" are otherwise the same line.
fn unread_phrase(unread: &Unread) -> String {
    match unread.count {
        0 => "0 unread planner surfaces".to_string(),
        count => format!("{count} unread planner surface(s): {}", unread.phrase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entry 58 of the divergence record, which is where this verb's surface is
    /// *proposed*.
    ///
    /// The tests below hold that proposal to what this build actually does: an
    /// entry naming a flag, a kind, a default or a status the code does not have
    /// is a proposal for something nobody built, put in front of the person who
    /// rules on it.
    fn divergence_entry() -> String {
        let record = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("docs")
                .join("contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split_once("\n## 58.")
            .expect("this verb is recorded under entry 58")
            .1
            .to_string();
        entry
            .split_once("\n## ")
            .map_or(entry.clone(), |(head, _)| head.to_string())
    }

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
        // Each mapping by name, not only their distinctness: the four constants
        // are the crate's public promise and this match is the only thing that
        // honours it, so a mapping quietly swapped here would leave every caller
        // branching on the wrong one.
        assert_eq!(Ending::Settled.exit_code(), EXIT_SUCCESS);
        assert_eq!(Ending::NothingDriving.exit_code(), EXIT_NOTHING_DRIVING);
        assert_eq!(Ending::SurfaceWaiting.exit_code(), EXIT_SURFACE_WAITING);
        assert_eq!(Ending::Elapsed.exit_code(), EXIT_WATCH_ELAPSED);

        // On the wire the word and the status are one value's two spellings, so
        // a record can never state a condition its own exit code contradicts.
        for ending in endings {
            let rendered = serde_json::to_value(ending).expect("an ending serializes");
            assert_eq!(rendered["condition"], serde_json::json!(ending.as_str()));
            assert_eq!(rendered["exit"], serde_json::json!(ending.exit_code()));
        }
    }

    #[test]
    fn a_cursor_round_trips_and_anything_else_is_refused() {
        let cursor = Cursor {
            run: "demo".to_string(),
            at: 4096,
        };
        assert_eq!(parse_cursor(&cursor.to_string()).expect("reads"), cursor);
        // The token a caller is handed and the token it renders on the wire are
        // the one spelling this build reads back.
        assert_eq!(
            serde_json::to_value(&cursor).expect("a cursor serializes"),
            serde_json::json!("1:demo:4096")
        );
        // A run id carrying the separator round-trips as itself, because the byte
        // is taken from the last one rather than the second.
        let colonised = Cursor {
            run: "demo:2".to_string(),
            at: 8,
        };
        assert_eq!(
            parse_cursor(&colonised.to_string()).expect("reads"),
            colonised
        );
        for token in [
            "",
            "4096",
            "1:4096",
            "2:demo:4096",
            "1:demo:",
            "1:demo:x",
            "1:demo:-1",
            "1::4096",
        ] {
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
        let entry = divergence_entry();

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

        // Everything else the entry states in this build's own numbers: the two
        // defaults a caller gets when it names neither, the cursor spelling a
        // later invocation is handed, and each terminal status. Read out of the
        // constants, so a value moved in the code and left in the proposal fails
        // here rather than misinforming the person ruling on it.
        for stated in [
            format!("(default {})", crate::cli::DEFAULT_WATCH_TIMEOUT_SECONDS),
            format!("(default {})", crate::cli::DEFAULT_WATCH_TICK_SECONDS),
            format!("`{WATCH_CURSOR_VERSION}:<run>:<byte>`"),
            format!("`{}`", Ending::Settled.exit_code()),
            format!("`{}`", Ending::NothingDriving.exit_code()),
            format!("`{}`", Ending::SurfaceWaiting.exit_code()),
            format!("`{}`", Ending::Elapsed.exit_code()),
        ] {
            assert!(
                entry.contains(&stated),
                "the entry no longer states {stated}, which this build does"
            );
        }
    }

    /// The entry proposes a command **schema**, and clap is what a caller is
    /// actually given.
    ///
    /// Both ways, because both drift the same distance: a flag the code grew and
    /// the proposal never mentioned is surface nobody ruled on, and a flag the
    /// proposal names and the code dropped is a promise to a person deciding
    /// about something that is not there.
    #[test]
    fn the_divergence_entry_proposes_exactly_the_flags_this_build_offers() {
        use clap::CommandFactory;

        let entry = divergence_entry();
        let schema = entry
            .split_once("add `onepipeline watch")
            .expect("the entry proposes the command")
            .1
            .split_once('`')
            .expect("the proposed command is one fenced span")
            .0;
        let proposed: std::collections::BTreeSet<String> = schema
            .split_whitespace()
            .filter_map(|word| {
                word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                    .strip_prefix("--")
                    .map(str::to_string)
            })
            .collect();
        let offered: std::collections::BTreeSet<String> = crate::cli::Cli::command()
            .get_subcommands()
            .find(|sub| sub.get_name() == "watch")
            .expect("the binary offers `watch`")
            .get_arguments()
            .filter_map(|arg| arg.get_long().map(str::to_string))
            .collect();
        assert_eq!(
            proposed, offered,
            "the entry proposes a different set of flags than this build offers"
        );
        // The one flag the entry mentions that this verb must never take: the
        // pacemaker's cadence is `start`'s clock, and the entry says so in prose
        // rather than in the schema.
        assert!(
            !offered.contains("heartbeat-interval"),
            "`watch` took `start`'s pacemaker flag"
        );
    }

    /// The entry describes the machine-readable form by naming its records, and
    /// the records are a serialized type: this is what keeps the two the same
    /// answer.
    ///
    /// Down to the **fields**, because the tag is the part a consumer finds and
    /// the fields are the part it reads. An entry that named every record and
    /// silently dropped a key would be a proposal to add a field this build does
    /// not write, or to leave one out that it does.
    #[test]
    fn the_divergence_entry_names_the_records_the_machine_form_actually_writes() {
        let entry = divergence_entry();

        let unread = Unread::default();
        let written = [
            Record::Heartbeat {
                run_id: "demo",
                unread: UnreadRecord::of(&unread),
            },
            Record::Return {
                run_id: "demo",
                ending: Ending::Settled,
                cursor: &Cursor::start("demo"),
                unread: UnreadRecord::of(&unread),
            },
        ];
        for shape in &written {
            let rendered = serde_json::to_value(shape).expect("the record serializes");
            let tag = rendered["watch"].as_str().expect("every record is tagged");
            assert!(
                entry.contains(&format!("\"watch\":\"{tag}\"")),
                "the entry describes no `{tag}` record, which this build writes"
            );
            // Read within *this* record's own fragment of the entry rather than
            // across the whole of it: every record here carries `run_id` and two
            // carry `unread`, so a whole-entry search would find a key dropped
            // from one record still standing in the next.
            let shown = entry
                .split_once(&format!("{{\"watch\":\"{tag}\""))
                .unwrap_or_else(|| panic!("the entry shows the `{tag}` record"))
                .1
                .split_once('}')
                .unwrap_or_else(|| panic!("the entry's `{tag}` record is closed"))
                .0;
            let written: std::collections::BTreeSet<&str> = rendered
                .as_object()
                .expect("a record is an object")
                .keys()
                .map(String::as_str)
                .filter(|key| *key != "watch")
                .collect();
            for key in &written {
                assert!(
                    shown.contains(&format!("\"{key}\":")),
                    "the entry's `{tag}` record does not carry `{key}`, which this build writes"
                );
            }
            // And the other way: a key the entry kept past the code would pass
            // every assertion above, and it is the worse drift — the entry is
            // read by the person ruling on this surface, so a field standing in
            // it that nothing writes is a proposal to approve something that does
            // not exist.
            for shown_key in shown.split('"').skip(1).step_by(2) {
                assert!(
                    shown_key == "watch" || written.contains(shown_key),
                    "the entry's `{tag}` record carries `{shown_key}`, which this build does \
                     not write"
                );
            }
        }
        // The one variant that borrows an envelope, asserted the same way: its
        // payload is the whole envelope, so the field is the only key to hold.
        assert!(
            entry.contains("\"watch\":\"event\"") && entry.contains("\"event\":"),
            "the entry describes no `event` record, which this build writes"
        );
    }

    /// The README's own passage about this verb, bounded by the heading that
    /// follows it, so a kind or a key named elsewhere in that document cannot
    /// satisfy an assertion about what this passage says.
    fn readme_watch_passage() -> String {
        let readme = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"),
        )
        .expect("the README ships");
        readme
            .split_once("`onepipeline watch RUN` is the bounded wait")
            .expect("the README documents this verb")
            .1
            .split_once("\n## ")
            .expect("that passage ends where the README's next heading begins")
            .0
            .to_string()
    }

    /// The README restates this verb's event set and its NDJSON records, because
    /// that passage is what a supervisor writes their script against — and a
    /// restatement with no gate is exactly where the two drift apart.
    ///
    /// `tests/contract.rs` reconciles the same passage's flags and terminal
    /// statuses; the meaningful set and the record schema are private to this
    /// module, so they are reconciled here rather than by widening them.
    ///
    /// The kinds are asserted **both ways**, as the divergence entry's are: a kind
    /// the README names and this build does not emit sends a script matching for a
    /// word that never arrives, and a kind this build emits and the README omits
    /// is a signal nobody was told to watch for — which is the failure this whole
    /// verb exists to end.
    #[test]
    fn the_readme_passage_names_every_meaningful_kind_and_every_record_this_verb_writes() {
        let passage = readme_watch_passage();

        for kind in crate::event::PIPELINE_KINDS {
            let named = passage.contains(&format!("`{}`", kind.as_str()));
            assert_eq!(
                named,
                MEANINGFUL.contains(kind),
                "the README's watch passage names `{}` ({named}), and this build calls it \
                 meaningful ({})",
                kind.as_str(),
                MEANINGFUL.contains(kind)
            );
        }

        // Every record this build writes, by its tag and by its own keys, read off
        // the serialized form rather than copied. These are what a caller branches
        // on, so a key renamed in the code and left standing in the README is a
        // script reading a field that is no longer there.
        //
        // Across the passage rather than per record, because the README describes
        // these in prose and two of them share most of their keys: what it holds
        // is that no key this build writes goes unmentioned. The per-record
        // reconciliation is the divergence entry's, below, where the records are
        // written as JSON fragments that can be told apart.
        let unread = Unread::default();
        for shape in [
            Record::Heartbeat {
                run_id: "demo",
                unread: UnreadRecord::of(&unread),
            },
            Record::Return {
                run_id: "demo",
                ending: Ending::Settled,
                cursor: &Cursor::start("demo"),
                unread: UnreadRecord::of(&unread),
            },
        ] {
            let rendered = serde_json::to_value(&shape).expect("the record serializes");
            let tag = rendered["watch"].as_str().expect("every record is tagged");
            assert!(
                passage.contains(&format!("`{tag}`")),
                "the README's watch passage describes no `{tag}` record, which this build writes"
            );
            for key in rendered
                .as_object()
                .expect("a record is an object")
                .keys()
                .filter(|key| *key != "watch")
            {
                assert!(
                    passage.contains(&format!("`{key}`")),
                    "the README's watch passage does not name `{key}`, which the `{tag}` \
                     record carries"
                );
            }
        }
        // The variant that borrows an envelope, asserted the same way: it cannot
        // be built here without a run to borrow one from, and its tag is what the
        // passage promises.
        assert!(
            passage.contains("`event`"),
            "the README's watch passage describes no `event` record, which this build writes"
        );
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
        let empty = Unread::default();
        let rendered =
            serde_json::to_value(UnreadRecord::of(&empty)).expect("the record serializes");
        assert_eq!(rendered["count"], serde_json::json!(0));
        assert_eq!(rendered["oldest_seconds"], serde_json::Value::Null);
    }
}
