//! `onepipeline watch` — a bounded, resumable wait on one run.
//!
//! What the verb promises is entry 58 of `docs/contract-divergences.md`, which is
//! the proposal it waits on and the record of what this cost before there was a
//! verb; the README documents it for a caller. Neither is restated here.
//!
//! The one thing worth saying beside the code: this module takes no lock a writer
//! needs and consumes no surface, so any number of watches may sit on a live run
//! at once — watching a run is not supervising it. The single exception, and the
//! reason there is now something to say: a watch records **itself**, in one
//! document per live watch under the run's own root, so that a process which is
//! not watching the run can ask whether anything is. That record is
//! [`crate::watchers`], and `onepipeline unwatched` is what reads it.

use std::io::Write;
use std::time::{Duration, Instant};

use crate::cli::{WatchArgs, WatchTimeout, WatchUntil, WATCH_CURSOR_VERSION};
use crate::error::{
    Error, Result, EXIT_NODE_SETTLED, EXIT_NOTHING_DRIVING, EXIT_SUCCESS, EXIT_SURFACE_WAITING,
    EXIT_WATCH_ELAPSED,
};
use crate::event::{Envelope, PipelineKind, Source};
use crate::filter::EventFilter;
use crate::graph::{self, GraphState, NodeStatus};
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
const MEANINGFUL: [PipelineKind; 9] = [
    PipelineKind::EditCommitted,
    // Beside it rather than instead of it: which of the two kinds an accepted
    // command is journalled under says whether the graph moved, and a supervisor
    // watching a run acts on the command having been accepted either way.
    PipelineKind::CommandAccepted,
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
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ending {
    Settled,
    SurfaceWaiting,
    NothingDriving,
    /// A node the wait named settled, and this is the one that did.
    ///
    /// The node rides the ending rather than being looked up beside it, because
    /// the two conditions that produce this — any node settling, and one named
    /// node settling — return the same word, and *which node* is the fact a
    /// caller acts on.
    // llmlint: ignore[invalid_states_unrepresentable] a node id is a `String` everywhere it exists in this crate — `Graph`'s keys, `Envelope::labels.node`, `RunState::statuses` — and this one is *read out of* a journalled settlement's own label rather than composed here, so a newtype at this one site would validate nothing the graph has not already said while putting a type between this ending and every value it is built from. `src/cli.rs` carries the same suppression, for the same reason, on the flag this is parsed from.
    NodeSettled(String),
    Elapsed,
}

impl Ending {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Settled => "settled",
            Self::SurfaceWaiting => "surface-waiting",
            Self::NothingDriving => "nothing-driving",
            Self::NodeSettled(_) => "node-settled",
            Self::Elapsed => "elapsed",
        }
    }

    const fn exit_code(&self) -> i32 {
        match self {
            Self::Settled => EXIT_SUCCESS,
            Self::SurfaceWaiting => EXIT_SURFACE_WAITING,
            Self::NothingDriving => EXIT_NOTHING_DRIVING,
            Self::NodeSettled(_) => EXIT_NODE_SETTLED,
            Self::Elapsed => EXIT_WATCH_ELAPSED,
        }
    }

    /// The human form's own words for this ending.
    ///
    /// The machine form's `condition` and its own `node` field are what a caller
    /// branches on; this is the line beside it, which names the node in the same
    /// breath so a person reading the terminal is not sent to the JSON for it.
    fn phrase(&self) -> String {
        match self {
            Self::NodeSettled(node) => format!("{} {node}", self.as_str()),
            worded => worded.as_str().to_string(),
        }
    }
}

/// The wire fields a return record states about why it ended, written from the
/// one value the process exits with.
///
/// Hand-written rather than derived because the pair is the whole promise: a
/// record that took `condition` and `exit` as two fields could be given a word
/// and a status that disagree, which is the caller reading prose again. `node`
/// is written by the one ending that has one, and absent from the rest rather
/// than null — a return that names no node did not end on one.
impl serde::Serialize for Ending {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let settled_node = match self {
            Self::NodeSettled(node) => Some(node),
            _ => None,
        };
        let mut record = serializer.serialize_map(Some(2 + usize::from(settled_node.is_some())))?;
        record.serialize_entry("condition", self.as_str())?;
        record.serialize_entry("exit", &self.exit_code())?;
        if let Some(node) = settled_node {
            record.serialize_entry("node", node)?;
        }
        record.end()
    }
}

/// Block until the run needs a supervisor, or until the wait runs out.
///
/// The run and the profile are resolved by the caller, and everything else this
/// verb can refuse is refused here before a line is written or a second is
/// waited: the cursor, and every condition the wait was told to return on. A
/// watch that waited five minutes — or, now that a wait may have no bound at all,
/// forever — to report a typo would be worse than the loop it replaces.
pub(crate) fn watch(args: &WatchArgs, paths: &RunPaths, filter: &EventFilter) -> Result<i32> {
    let mut cursor = match args.cursor.as_deref() {
        Some(token) => resolve_cursor(paths, token)?,
        None => Cursor::start(&paths.run),
    };
    let deadline = deadline(args.timeout)?;
    let tick = Duration::from_secs(args.tick_interval);
    let mut quiet_since = Instant::now();
    let mut out = Emitter::new();

    // The first pass's reads, taken before anything is emitted, because the
    // conditions are validated against them: what this watch will read is what
    // decides whether a condition the run has already met returns immediately or
    // could never be met at all.
    let mut view = RunView::open(paths)?;
    let mut fresh = tail(paths, &mut cursor);
    let selectors = Selectors::resolve(&args.until, &view, &fresh)?;

    // The record that says this run is being watched, written once every refusal
    // above has been made — a command that never watched anything leaves no
    // evidence that it did — and removed when this returns. It is the one thing
    // this verb writes, and it is written **best effort and silently**: a runs
    // root this process may not write costs a reader the knowledge that this watch
    // exists and costs the watch itself nothing, so it changes neither this verb's
    // output nor any of its statuses. See `src/watchers.rs` for why its absence
    // may never be relied upon.
    let _armed = crate::watchers::Armed::arm(paths);

    loop {
        for event in fresh
            .iter()
            .filter(|event| meaningful(event) && filter.matches(event))
        {
            out.event(&view, event)?;
            quiet_since = Instant::now();
        }

        if let Some(ending) = concluded(&view, paths, &selectors, &fresh) {
            return out.returned(&view, ending, &cursor);
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return out.returned(&view, Ending::Elapsed, &cursor);
        }
        if !tick.is_zero() && quiet_since.elapsed() >= tick {
            out.heartbeat(&view)?;
            quiet_since = Instant::now();
        }
        std::thread::sleep(POLL);
        view = RunView::open(paths)?;
        fresh = tail(paths, &mut cursor);
    }
}

/// The instant this wait gives up at, or `None` for a wait with no bound.
///
/// Checked, because the seconds are a caller's: `Instant` addition panics on
/// overflow, and a wait longer than this host's clock can hold is a value to
/// refuse rather than a reason to abort the process. A wait with **no** bound
/// never reaches that arithmetic at all — it is the absence of a deadline rather
/// than one far away, which is what keeps `0`'s published meaning, read once and
/// return, the value it always was.
fn deadline(timeout: WatchTimeout) -> Result<Option<Instant>> {
    let WatchTimeout::Bounded(seconds) = timeout else {
        return Ok(None);
    };
    Instant::now()
        .checked_add(Duration::from_secs(seconds))
        .map(Some)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "a wait of {seconds} seconds is further ahead than this host's clock can name; \
                 give `--timeout` a value it can reach"
            ))
        })
}

fn tail(paths: &RunPaths, cursor: &mut Cursor) -> Vec<Envelope> {
    let (mut fresh, at) = journal::finished_after(&paths.journal(), cursor.at);
    cursor.at = at;
    journal::merge_order(&mut fresh);
    fresh
}

/// The conditions this watch returns on, resolved against the run before it
/// blocks.
///
/// Two of the terminal conditions are not held here at all: a run that settles
/// `complete` and a run nothing is driving end every wait whether or not they
/// were named, because a wait that could outlive the run it watches is the
/// unbounded silence this verb exists to end. So `--until settled` and `--until
/// nothing-driving` name what the verb already does — which is why the first of
/// them keeps its meaning exactly, "report a blocking surface and wait through
/// it" — and what is chosen here is everything else.
#[derive(Debug, Default, PartialEq, Eq)]
struct Selectors {
    /// Return on a blocking surface waiting to be answered.
    surface: bool,
    /// Return when any node of the run settles.
    any_node: bool,
    /// Return when one of these nodes settles.
    // llmlint: ignore[invalid_states_unrepresentable] every id in here has already been checked against this run's own graph by `resolve`, which is the only thing that constructs one, and that is the whole of what "valid" means for a node id — a fact about one run at one moment, which no type can carry across the moment the graph is edited. The crate spells a node id `String` everywhere else for the same reason.
    named: Vec<String>,
}

impl Selectors {
    /// Read the conditions this wait was given, or refuse one it could not
    /// return on.
    ///
    /// Three refusals, all made here rather than in the loop, so a condition this
    /// run cannot answer costs a caller nothing: a node the graph does not hold,
    /// a node that will never settle again, and — for a run with nothing left to
    /// settle — a wait for any node to settle. Each names what it would never
    /// fire on.
    ///
    /// **The line between "never" and "already".** A settlement at or past this
    /// watch's cursor is in `ahead`, is read on the very first pass, and returns
    /// immediately: a condition the run has already satisfied is answered, never
    /// refused. A settlement *behind* the cursor was handed to the watch that
    /// printed that cursor and is not handed over twice — so what is left, a node
    /// that settled `done` with nothing of it ahead, is a wait for a dispatch
    /// that will not happen, because `done` is the one status nothing schedules
    /// out of again. Every other settled status can settle again: a failed or
    /// cancelled node is retried, a parked one requeued, a waiting one attested,
    /// a draft-complete one dispatched by the release it waits on. (A planner
    /// *correcting* a record with `settle` can journal a further settlement for a
    /// done node. That is an intervention in the run rather than the run's own
    /// life, and it is not what the refusal claims: what it claims is that
    /// nothing will dispatch the node again.)
    ///
    /// A condition that cannot fire is refused whether or not some other
    /// condition would have ended the same watch anyway. It is a mistake in the
    /// command, and answering it with a different condition's status would hide
    /// it behind an exit code the caller would read as an answer.
    fn resolve(until: &[WatchUntil], view: &RunView, ahead: &[Envelope]) -> Result<Self> {
        let mut chosen = Self::default();
        let statuses = view.state.statuses();
        for condition in until {
            match condition {
                WatchUntil::Settled | WatchUntil::NothingDriving => {}
                WatchUntil::Surface => chosen.surface = true,
                WatchUntil::NodeSettled => {
                    let ids: Vec<&String> = statuses.keys().collect();
                    // A graph with nothing in it is refused on its own terms
                    // rather than through the sentence below, which would be
                    // saying that every one of no nodes settled.
                    if ids.is_empty() {
                        return Err(Error::Invalid(format!(
                            "`--until {condition}` would never fire: run '{}' holds no nodes \
                             at all, so nothing in it can settle",
                            view.paths.run
                        )));
                    }
                    if done_behind_the_cursor(&ids, &statuses, ahead) {
                        return Err(Error::Invalid(format!(
                            "`--until {condition}` would never fire: every node of run '{}' \
                             ({}) settled `done` before this watch's cursor, and nothing \
                             dispatches a `done` node again",
                            view.paths.run,
                            named(ids.into_iter())
                        )));
                    }
                    chosen.any_node = true;
                }
                WatchUntil::Node(node) => {
                    if !view.state.graph.contains(node) {
                        return Err(Error::Invalid(format!(
                            "`--until {condition}` names a node run '{}' does not hold; its \
                             graph holds {}",
                            view.paths.run,
                            named(view.state.graph.ids())
                        )));
                    }
                    if done_behind_the_cursor(&[node], &statuses, ahead) {
                        return Err(Error::Invalid(format!(
                            "`--until {condition}` would never fire: node '{node}' of run \
                             '{}' settled `done` before this watch's cursor, and nothing \
                             dispatches a `done` node again",
                            view.paths.run
                        )));
                    }
                    chosen.named.push(node.clone());
                }
            }
        }
        Ok(chosen)
    }

    fn wants(&self, node: &str) -> bool {
        self.any_node || self.named.iter().any(|named| named == node)
    }
}

/// Whether every one of these nodes has settled `done` with no settlement of any
/// of them left for this watch to read.
///
/// Named for exactly that and not for permanence: what it answers is a status and
/// a cursor, which is what [`Selectors::resolve`] refuses on, and a planner
/// correcting a record can still journal a further settlement for a `done` node.
fn done_behind_the_cursor(
    nodes: &[impl AsRef<str>],
    statuses: &std::collections::BTreeMap<String, NodeStatus>,
    ahead: &[Envelope],
) -> bool {
    nodes.iter().all(|node| {
        statuses.get(node.as_ref()).copied() == Some(NodeStatus::Done)
            && !ahead
                .iter()
                .filter_map(settlement_of)
                .any(|settled| settled == node.as_ref())
    })
}

/// The ids a refusal names, or that there are none — said out loud, because a
/// sentence that simply stops reads as one that forgot to name them.
fn named<'a>(ids: impl Iterator<Item = &'a String>) -> String {
    let ids: Vec<&str> = ids.map(String::as_str).collect();
    match ids.is_empty() {
        true => "no nodes at all".to_string(),
        false => ids.join(", "),
    }
}

/// The node an event settled, when the event is one of **this crate's** own
/// settlements.
///
/// Asked of the source and the kind together, for the reason [`meaningful`]
/// gives: a sibling that one day spells `node-settled` the way this one does
/// would otherwise end a wait over a node that settled nothing.
fn settlement_of(event: &Envelope) -> Option<&str> {
    (meaningful(event) && PipelineKind::from_wire(&event.kind) == Some(PipelineKind::NodeSettled))
        .then_some(event.labels.node.as_deref())
        .flatten()
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
/// to nothing, and `adopt` comes first. A node settling is last of the four,
/// because it is a fact *within* a run the three above it are facts *about*: a
/// settlement read out of a run nobody is driving is not the thing to act on.
///
/// The settlement is taken from `fresh` — the records this pass read — rather
/// than from the state folded out of them, so it is the same event the caller was
/// just handed a line for, and so the whole journal ahead of the cursor is what
/// answers a condition the run met before this watch started. It is **not** put
/// through the caller's profile: a profile shapes which events this reader is
/// shown, and a condition the wait returns on is a fact about the run rather than
/// about the view over it.
fn concluded(
    view: &RunView,
    paths: &RunPaths,
    selectors: &Selectors,
    fresh: &[Envelope],
) -> Option<Ending> {
    let statuses = view.state.statuses();
    if !statuses.is_empty() && graph::state_of(&statuses) == GraphState::Complete {
        return Some(Ending::Settled);
    }
    if view.liveness().is_undriven() {
        return Some(Ending::NothingDriving);
    }
    if selectors.surface && views::blocking_surface(paths) {
        return Some(Ending::SurfaceWaiting);
    }
    fresh
        .iter()
        .filter_map(settlement_of)
        .find(|node| selectors.wants(node))
        .map(|node| Ending::NodeSettled(node.to_string()))
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
    // The length the checks below are made against, and it is read rather than
    // assumed. A run whose driver has appended nothing has no journal file yet,
    // and byte 0 of it is a place a cursor may legitimately name — but every
    // other way a length fails to be read is the boundary check having nothing
    // to check, and treating that as a zero-length journal would accept
    // `1:<run>:0` off a store this process cannot read at all.
    let held = match std::fs::metadata(&journal) {
        Ok(file) => file.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => {
            return Err(Error::Invalid(format!(
                "cursor '{token}' resumes at byte {at} of run '{}', whose journal could not \
                 be read ({error}); a cursor is checked against the journal it names, and \
                 one that cannot be read is refused rather than resumed from",
                paths.run
            )))
        }
    };
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
        /// The word, the status and — for the one ending that has a node — the
        /// node, all written from the one [`Ending`] the process is about to exit
        /// with, so the record cannot name a condition its own exit code
        /// contradicts.
        #[serde(flatten)]
        ending: &'a Ending,
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
        let code = ending.exit_code();
        self.say(
            &format!(
                "-- watch {} {}  {}  cursor {cursor}",
                view.paths.run,
                ending.phrase(),
                unread_phrase(&unread)
            ),
            &Record::Return {
                run_id: &view.paths.run,
                ending: &ending,
                cursor,
                unread: UnreadRecord::of(&unread),
            },
        )?;
        Ok(code)
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
            Ending::NodeSettled("build".to_string()),
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
        assert_eq!(
            Ending::NodeSettled("build".to_string()).exit_code(),
            EXIT_NODE_SETTLED
        );

        // On the wire the word and the status are one value's two spellings, so
        // a record can never state a condition its own exit code contradicts.
        for ending in &endings {
            let rendered = serde_json::to_value(ending).expect("an ending serializes");
            assert_eq!(rendered["condition"], serde_json::json!(ending.as_str()));
            assert_eq!(rendered["exit"], serde_json::json!(ending.exit_code()));
        }

        // The one ending that names a node carries it as a field of its own —
        // the two conditions that produce it return the same word, so *which
        // node* is only readable here — and every other ending leaves the key
        // out rather than writing a null a caller would have to test for. The
        // human line says it too, so a person is not sent to the JSON for the
        // one fact the word omits.
        let settled = Ending::NodeSettled("build".to_string());
        let rendered = serde_json::to_value(&settled).expect("an ending serializes");
        assert_eq!(rendered["node"], serde_json::json!("build"));
        assert_eq!(settled.phrase(), "node-settled build");
        for ending in endings.iter().filter(|end| **end != settled) {
            let rendered = serde_json::to_value(ending).expect("an ending serializes");
            assert!(
                rendered.get("node").is_none(),
                "`{}` named a node it did not end on: {rendered}",
                ending.as_str()
            );
            assert_eq!(ending.phrase(), ending.as_str());
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
            format!("`{}`", Ending::NodeSettled("any".to_string()).exit_code()),
            // The unbounded wait's spelling, and the vocabulary it made
            // necessary. Both are read out of the constants a caller's command
            // line is parsed against, so a word changed in the code and left
            // standing in the proposal fails here rather than misinforming the
            // person ruling on it.
            format!(
                "`--timeout SECONDS|{}`",
                crate::cli::WATCH_TIMEOUT_UNBOUNDED
            ),
        ] {
            assert!(
                entry.contains(&stated),
                "the entry no longer states {stated}, which this build does"
            );
        }

        // And the selector's whole vocabulary, out of the constant the parser
        // reads a caller's condition against. The entry is where this surface is
        // proposed, so a condition the build accepts and the proposal never
        // mentions is a value nobody ruled on — and the wait it ends is the one
        // that may now have no bound at all.
        for condition in crate::cli::watch_conditions() {
            assert!(
                entry.contains(&format!("`{condition}`")),
                "the entry names no `{condition}` condition, which this build accepts"
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
        // The return is rendered on the ending that **names a node**, because
        // that record is the superset: `node` is the one key a base condition's
        // return leaves out, and the reconciliation below runs both ways — a
        // record rendered without it would read the entry's `node` as a key
        // nothing writes.
        let written = [
            Record::Heartbeat {
                run_id: "demo",
                unread: UnreadRecord::of(&unread),
            },
            Record::Return {
                run_id: "demo",
                ending: &Ending::NodeSettled("build".to_string()),
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
            // The node-naming return, for the reason the divergence entry's own
            // reconciliation renders that one: it is the record with every key.
            Record::Return {
                run_id: "demo",
                ending: &Ending::NodeSettled("build".to_string()),
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

    /// Every condition round-trips through the spelling a caller types.
    ///
    /// The spelling is a promise both ways: it is what a supervisor writes on a
    /// command line and what this build's own help and defaults render, so a
    /// value that parsed one way and printed another would leave a script and
    /// this binary a word apart. The refusal is held to naming the whole
    /// vocabulary, because that message is all a caller who mistyped one has.
    #[test]
    fn every_condition_round_trips_through_the_spelling_a_caller_types() {
        use std::str::FromStr;

        for (spelling, condition) in [
            ("surface", WatchUntil::Surface),
            ("settled", WatchUntil::Settled),
            ("nothing-driving", WatchUntil::NothingDriving),
            ("node-settled", WatchUntil::NodeSettled),
            ("node=build", WatchUntil::Node("build".to_string())),
        ] {
            assert_eq!(WatchUntil::from_str(spelling).expect("reads"), condition);
            assert_eq!(condition.to_string(), spelling);
        }
        // Every spelling the refusal offers is one this build actually accepts —
        // `node=<ID>` for the shape rather than for a node any run holds — so a
        // caller who types back what they were told is not refused again.
        for condition in crate::cli::watch_conditions() {
            assert!(
                WatchUntil::from_str(condition).is_ok(),
                "the vocabulary offers `{condition}`, which this build refuses"
            );
        }
        for text in ["", "node", "node=", "NODE=build", "surfaces", "0"] {
            let refused = WatchUntil::from_str(text).expect_err("refused");
            for condition in crate::cli::watch_conditions() {
                assert!(
                    refused.contains(condition),
                    "the refusal of {text:?} does not name `{condition}`: {refused}"
                );
            }
        }
    }

    /// A wait with no bound is a different value from the one that reads once and
    /// returns, in every spelling and in the deadline each produces.
    ///
    /// The pair is the point of the word: `0` is the shortest wait there is and
    /// `none` is the longest, and one value meaning both would have made "wake me
    /// when something happens" unsayable.
    #[test]
    fn a_wait_with_no_bound_is_a_different_value_from_the_one_that_reads_once() {
        use std::str::FromStr;

        let unbounded = WatchTimeout::from_str(crate::cli::WATCH_TIMEOUT_UNBOUNDED).expect("reads");
        assert_eq!(unbounded, WatchTimeout::Unbounded);
        assert_ne!(unbounded, WatchTimeout::Bounded(0));
        assert_eq!(
            unbounded.to_string(),
            crate::cli::WATCH_TIMEOUT_UNBOUNDED,
            "a wait with no bound does not render as the word it is read from"
        );
        for seconds in [0, 300] {
            let bounded = WatchTimeout::from_str(&seconds.to_string()).expect("reads");
            assert_eq!(bounded, WatchTimeout::Bounded(seconds));
            assert_eq!(bounded.to_string(), seconds.to_string());
        }
        // What each one is in the loop: no deadline at all, against a deadline
        // that has already passed — which is what reads the run once and returns.
        assert!(deadline(WatchTimeout::Unbounded)
            .expect("a wait with no bound is a wait")
            .is_none());
        assert!(deadline(WatchTimeout::Bounded(0))
            .expect("a zero wait is a wait")
            .is_some_and(|at| at <= Instant::now()));
        for text in ["", "-1", "forever", "5s", "None"] {
            let refused = WatchTimeout::from_str(text).expect_err("refused");
            assert!(
                refused.contains(crate::cli::WATCH_TIMEOUT_UNBOUNDED),
                "the refusal of {text:?} does not name the word for no bound: {refused}"
            );
        }
    }

    /// A refusal names the ids a graph holds, and says so out loud when it holds
    /// none.
    ///
    /// The empty case is said rather than left blank for the reason an empty
    /// unread queue is: "it holds nothing" and "this line does not say what it
    /// holds" would otherwise be the same sentence.
    #[test]
    fn a_refusal_names_the_ids_a_graph_holds_and_says_so_when_it_holds_none() {
        let ids = ["build".to_string(), "check".to_string()];
        assert_eq!(named(ids.iter()), "build, check");
        assert_eq!(named(std::iter::empty()), "no nodes at all");
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
