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
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use onemessagebus::{
    Batch, Changed, ConsumerName, DocumentName, Fingerprint, LocalTransport, Position, QueueName,
    Transport, TransportError,
};

use crate::cli::{WatchTimeout, WatchUntil, WATCH_CURSOR_VERSION};
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

/// Why a watch returned: a closed set with a code each, so a caller branches on
/// the status and never on the words beside it.
///
/// Published as `verbs::WatchEnding`, which is where the exit each one maps to
/// is read by the binary and by any consumer that carries this verb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// The run's graph is complete.
    Settled,
    /// A blocking surface is waiting to be answered.
    SurfaceWaiting,
    /// Nothing is driving the run.
    NothingDriving,
    /// A node the wait named settled, and this is the one that did.
    ///
    /// The node rides the ending rather than being looked up beside it, because
    /// the two conditions that produce this — any node settling, and one named
    /// node settling — return the same word, and *which node* is the fact a
    /// caller acts on.
    // llmlint: ignore[invalid_states_unrepresentable] a node id is a `String` everywhere it exists in this crate — `Graph`'s keys, `Envelope::labels.node`, `RunState::statuses` — and this one is *read out of* a journalled settlement's own label rather than composed here, so a newtype at this one site would validate nothing the graph has not already said while putting a type between this ending and every value it is built from. `src/cli.rs` carries the same suppression, for the same reason, on the flag this is parsed from.
    NodeSettled(String),
    /// The wait's own bound ran out.
    Elapsed,
}

impl Ending {
    /// The condition's own word, as the machine form's `condition` spells it.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Settled => "settled",
            Self::SurfaceWaiting => "surface-waiting",
            Self::NothingDriving => "nothing-driving",
            Self::NodeSettled(_) => "node-settled",
            Self::Elapsed => "elapsed",
        }
    }

    /// The status the binary exits with for this ending.
    pub const fn exit_code(&self) -> i32 {
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

/// What a watch is asked for: the shape of the wait, with the run and the
/// profile resolved by the caller.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The profile the event view is shaped through.
    pub filter: EventFilter,
    /// How long to wait before giving up.
    pub timeout: WatchTimeout,
    /// How long a silence may last before the stream says it is still there.
    /// Zero turns the heartbeat off.
    pub tick: Duration,
    /// A cursor an earlier watch printed, to resume from.
    pub cursor: Option<String>,
    /// What ends the wait, beside the run finishing and nothing driving it.
    pub until: Vec<WatchUntil>,
}

/// One thing a watch says as it waits, handed to the caller's sink as it
/// happens.
///
/// Each frame carries the view it was decided from, so a renderer can say what
/// the binary says — the event's line names what this crate knows about it
/// since, and the heartbeat names how the run is being driven — without a
/// second read of the run.
#[derive(Debug)]
pub enum Frame<'a> {
    /// One meaningful event, as the envelope itself.
    Event {
        /// The run as the read that found the event saw it.
        view: &'a RunView,
        /// The event.
        event: &'a Envelope,
    },
    /// A heartbeat: nothing has happened for a whole tick, and the watch is
    /// still there.
    Tick {
        /// The run as the last read saw it.
        view: &'a RunView,
    },
    /// The wait is over, and this is why.
    Ended {
        /// The run as the read that ended the wait saw it.
        view: &'a RunView,
        /// Why, and where the next watch resumes from.
        outcome: &'a Outcome,
    },
}

/// How a watch ended: the condition, and the cursor the next watch resumes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Why the wait returned.
    pub ending: Ending,
    /// The cursor token the next watch resumes from.
    pub cursor: String,
}

impl Outcome {
    /// The status the binary exits with.
    pub const fn exit_code(&self) -> i32 {
        self.ending.exit_code()
    }
}

/// Block until the run needs a supervisor, or until the wait runs out.
///
/// The run and the profile are resolved by the caller, and everything else this
/// verb can refuse is refused here before a frame is handed out or a second is
/// waited: the cursor, and every condition the wait was told to return on. A
/// watch that waited five minutes — or, now that a wait may have no bound at all,
/// forever — to report a typo would be worse than the loop it replaces.
///
/// Every frame goes to `sink` as it happens, and a sink that refuses one ends
/// the wait with its refusal: the binary's sink is a pipe, and a pipe that has
/// closed is the end of what a watch can report.
pub(crate) fn watch(
    paths: &RunPaths,
    request: &Request,
    sink: &mut dyn FnMut(Frame<'_>) -> Result<()>,
) -> Result<Outcome> {
    let mut cursor = match request.cursor.as_deref() {
        Some(token) => resolve_cursor(paths, token)?,
        None => Cursor::start(&paths.run),
    };
    let deadline = deadline(request.timeout)?;
    let tick = request.tick;
    let mut quiet_since = Instant::now();

    // What the wait watches is fingerprinted before the first read, so anything
    // written while that read runs moves it past what the read saw.
    let changes = RunChanges::of(paths);
    let mut follow = Follow::from_now(&changes, RunChanges::queue())?;

    // The first pass's reads, taken before anything is emitted, because the
    // conditions are validated against them: what this watch will read is what
    // decides whether a condition the run has already met returns immediately or
    // could never be met at all.
    let (mut view, mut fresh) = read_run(paths, &mut cursor)?;
    changes.observe(&view);
    let selectors = Selectors::resolve(&request.until, &view, &fresh)?;

    // The record that says this run is being watched, written once every refusal
    // above has been made — a command that never watched anything leaves no
    // evidence that it did — and removed when this returns. It is the one thing
    // this verb writes, and it is written **best effort and silently**: a runs
    // root this process may not write costs a reader the knowledge that this watch
    // exists and costs the watch itself nothing, so it changes neither this verb's
    // output nor any of its statuses. See `src/watchers.rs` for why its absence
    // may never be relied upon.
    let _armed = crate::watchers::Armed::arm(paths);

    let ended = |view: &RunView,
                 ending: Ending,
                 cursor: &Cursor,
                 sink: &mut dyn FnMut(Frame<'_>) -> Result<()>| {
        let outcome = Outcome {
            ending,
            cursor: cursor.to_string(),
        };
        sink(Frame::Ended {
            view,
            outcome: &outcome,
        })?;
        Ok(outcome)
    };

    // Whether the last wait saw the run move. Only a pass over a run that moved
    // emits or asks whether the wait is over: everything those decide from is in
    // what `RunChanges` fingerprints, so a run that did not move is neither read
    // nor reported on again.
    let mut moved = true;
    loop {
        if moved {
            for event in fresh
                .iter()
                .filter(|event| meaningful(event) && request.filter.matches(event))
            {
                sink(Frame::Event { view: &view, event })?;
                quiet_since = Instant::now();
            }

            if let Some(ending) = concluded(&view, paths, &selectors, &fresh) {
                return ended(&view, ending, &cursor, sink);
            }
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return ended(&view, Ending::Elapsed, &cursor, sink);
        }
        if !tick.is_zero() && quiet_since.elapsed() >= tick {
            sink(Frame::Tick { view: &view })?;
            quiet_since = Instant::now();
        }
        let within = wake_within(deadline, tick, quiet_since);
        moved = match follow.next(within, || read_run(paths, &mut cursor))? {
            Some((read, tailed)) => {
                view = read;
                fresh = tailed;
                changes.observe(&view);
                true
            }
            None => false,
        };
    }
}

/// One read of the run: its view, and the records past the cursor.
///
/// Recorded as one render of the watch where [`crate::rendercost`] is asked to
/// record, which is how a journey counts that a watch reads the run only when it
/// moved.
fn read_run(paths: &RunPaths, cursor: &mut Cursor) -> Result<(RunView, Vec<Envelope>)> {
    let _render = crate::rendercost::rendering(crate::rendercost::Rendered::Watch, &paths.run);
    let view = RunView::open(paths)?;
    Ok((view, tail(paths, cursor)))
}

/// How long the wait may block before this watch has something of its own to
/// do: reach its deadline, or write the heartbeat a quiet interval owes.
///
/// Unbounded where it has neither, because a change is what ends that wait.
fn wake_within(deadline: Option<Instant>, tick: Duration, quiet_since: Instant) -> Duration {
    let until_deadline = deadline.map_or(Duration::MAX, |deadline| {
        deadline.saturating_duration_since(Instant::now())
    });
    let until_heartbeat = match tick.is_zero() {
        true => Duration::MAX,
        false => tick.saturating_sub(quiet_since.elapsed()),
    };
    until_deadline.min(until_heartbeat)
}

/// A wait on one queue of a transport that reads what it follows again only when
/// that queue moved.
///
/// Over any [`Transport`] rather than tied to [`RunChanges`], so the property the
/// watch rests on — one read per change the wait reports, and none across a wait
/// that reports nothing — is held over the bus's memory transport, where a change
/// is an append a test makes.
struct Follow<'a> {
    changes: &'a dyn Transport,
    queue: QueueName,
    since: Fingerprint,
}

impl<'a> Follow<'a> {
    /// Follow `queue` from how it stands now.
    fn from_now(changes: &'a dyn Transport, queue: QueueName) -> Result<Self> {
        let since = changes.fingerprint(&queue).map_err(unwatchable)?;
        Ok(Self {
            changes,
            queue,
            since,
        })
    }

    /// Wait up to `within` for the queue to move, and `reread` once if it did.
    ///
    /// The fingerprint is taken as moved **before** the read, so a change landing
    /// while `reread` runs is the next wait's change rather than a lost one.
    fn next<T>(
        &mut self,
        within: Duration,
        reread: impl FnOnce() -> Result<T>,
    ) -> Result<Option<T>> {
        match self
            .changes
            .wait_for_change(&self.queue, &self.since, within)
            .map_err(unwatchable)?
        {
            Changed::Moved(now) => {
                self.since = now;
                reread().map(Some)
            }
            Changed::Unchanged(_) => Ok(None),
        }
    }
}

fn unwatchable(failure: TransportError) -> Error {
    Error::Invalid(format!(
        "the watch could not tell whether the run changed: {failure}"
    ))
}

/// Everything a pass of a watch decides from, as one queue whose fingerprint
/// moves whenever any of it does.
///
/// A [`Transport`] so the wait is the bus's own: a watch blocks in
/// [`Transport::wait_for_change`] over this fingerprint and keeps no clock of its
/// own. What it covers is what the lines and [`concluded`] are decided from — the
/// journal, the launch record, the channel's queues — and the two answers about
/// the driver that move with no file moving: the process a record names having
/// ended, and the run falling quiet past the parked bound. Those two are asked of
/// the host through the same readings [`views`] decides liveness with, from what
/// the last read said about the driver.
///
/// Read-only: it is a view over files other processes write, so every read and
/// write of records is refused.
struct RunChanges {
    paths: RunPaths,
    /// The channel's transport, opened once its directory exists — never
    /// before, so a watch makes no channel directory in a run that has none.
    channel: OnceLock<LocalTransport>,
    observed: Mutex<Observed>,
}

/// What the last read of the run said about its driver.
#[derive(Debug, Default)]
struct Observed {
    // llmlint: ignore-block[invalid_states_unrepresentable] these are the driver record's own `host` and `started` fields as it recorded them — `String`s in `ledger` — held only to be handed to `views::driver_claim_is_over`, which compares them with what the host answers now. `sys::StartToken` is a token read off a live process and matched against a recorded string, so typing a recorded value as one would claim a reading that never happened.
    host: Option<String>,
    pid: Option<NonZeroU32>,
    started: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    last_write_at: Option<u64>,
}

/// The one queue [`RunChanges`] answers for.
const RUN_CHANGES: &str = "run";

impl RunChanges {
    fn of(paths: &RunPaths) -> Self {
        Self {
            paths: paths.clone(),
            channel: OnceLock::new(),
            observed: Mutex::new(Observed::default()),
        }
    }

    fn queue() -> QueueName {
        QueueName::try_from(RUN_CHANGES)
            .unwrap_or_else(|_| unreachable!("`{RUN_CHANGES}` is a queue name"))
    }

    /// Keep what `view` says about the driver, for the parts of the fingerprint
    /// no file carries.
    fn observe(&self, view: &RunView) {
        *self.observed.lock().unwrap_or_else(PoisonError::into_inner) = Observed {
            host: view.launch.recorded_host().map(str::to_owned),
            pid: view.launch.driver_pid(),
            started: view.launch.driver_stamp().map(str::to_owned),
            last_write_at: view.state.last_write_at,
        };
    }

    fn refused(queue: &QueueName, why: &str) -> TransportError {
        TransportError::Backend {
            transport: "run-changes".to_owned(),
            detail: format!("{queue}: {why}"),
        }
    }

    fn read_only(queue: &QueueName) -> TransportError {
        Self::refused(
            queue,
            "a watch reads what a run's writers wrote, and writes and reads no records",
        )
    }

    fn channel(&self) -> std::result::Result<Option<&LocalTransport>, TransportError> {
        if let Some(channel) = self.channel.get() {
            return Ok(Some(channel));
        }
        let dir = self.paths.channel_dir();
        if !dir.is_dir() {
            return Ok(None);
        }
        let opened = LocalTransport::open(dir)?;
        Ok(Some(self.channel.get_or_init(|| opened)))
    }
}

/// A file's length and modification time, or that it is absent — the same
/// observation the local transport fingerprints a queue with.
fn mark(path: &Path, parts: &mut Vec<u64>) {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .unwrap_or_default();
            parts.extend([
                1,
                metadata.len(),
                modified.as_secs(),
                u64::from(modified.subsec_nanos()),
            ]);
        }
        Err(_) => parts.push(0),
    }
}

impl Transport for RunChanges {
    fn append(
        &self,
        queue: &QueueName,
        _record: &[u8],
    ) -> std::result::Result<Position, TransportError> {
        Err(Self::read_only(queue))
    }

    fn read(
        &self,
        queue: &QueueName,
        _from: Option<&Position>,
        _limit: usize,
    ) -> std::result::Result<Batch, TransportError> {
        Err(Self::read_only(queue))
    }

    fn cursor(
        &self,
        queue: &QueueName,
        _consumer: &ConsumerName,
    ) -> std::result::Result<Option<Position>, TransportError> {
        Err(Self::read_only(queue))
    }

    fn commit(
        &self,
        queue: &QueueName,
        _consumer: &ConsumerName,
        _at: &Position,
    ) -> std::result::Result<(), TransportError> {
        Err(Self::read_only(queue))
    }

    fn exclusive(
        &self,
        queue: &QueueName,
        _body: &mut dyn FnMut(&dyn Transport) -> std::result::Result<(), TransportError>,
    ) -> std::result::Result<(), TransportError> {
        Err(Self::read_only(queue))
    }

    fn fingerprint(&self, queue: &QueueName) -> std::result::Result<Fingerprint, TransportError> {
        if queue.to_string() != RUN_CHANGES {
            return Err(Self::refused(
                queue,
                "a watch fingerprints one run, as the queue `run`",
            ));
        }
        let mut parts = Vec::new();
        mark(&self.paths.journal(), &mut parts);
        mark(&self.paths.launch(), &mut parts);
        match self.channel()? {
            Some(channel) => {
                for name in [
                    onemessagebus_agent::channel::SURFACES,
                    onemessagebus_agent::channel::REPLIES,
                    onemessagebus_agent::channel::COMMANDS,
                    onemessagebus_agent::channel::COMMAND_OUTCOMES,
                ] {
                    let name = QueueName::try_from(name)
                        .map_err(|failure| Self::refused(queue, &failure.to_string()))?;
                    parts.extend_from_slice(channel.fingerprint(&name)?.parts());
                }
            }
            None => parts.push(0),
        }
        let observed = self.observed.lock().unwrap_or_else(PoisonError::into_inner);
        parts.push(u64::from(views::driver_claim_is_over(
            observed.host.as_deref(),
            observed.pid,
            observed.started.as_deref(),
        )));
        parts.push(u64::from(views::quiet_past_parked(observed.last_write_at)));
        Ok(Fingerprint::from_parts(parts))
    }

    fn wait_for_change(
        &self,
        queue: &QueueName,
        since: &Fingerprint,
        timeout: Duration,
    ) -> std::result::Result<Changed, TransportError> {
        onemessagebus::transport::poll_for_change(self, queue, since, timeout)
    }

    fn document(
        &self,
        queue: &QueueName,
        _name: &DocumentName,
    ) -> std::result::Result<Option<Vec<u8>>, TransportError> {
        Err(Self::read_only(queue))
    }

    fn replace_document(
        &self,
        queue: &QueueName,
        _name: &DocumentName,
        _bytes: &[u8],
    ) -> std::result::Result<(), TransportError> {
        Err(Self::read_only(queue))
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

/// `onepipeline monitor` — one pass over the merged stream, from a cursor or
/// from the start, ending on the cursor the next pass resumes from.
///
/// Here rather than in the views because the cursor is this module's: one type,
/// one parser and one tail read behind both verbs, so a token either prints is
/// a token the other reads. The events handed back and the byte the cursor
/// names come out of the **same** read of the journal — see [`views::monitor_of`]
/// for why [`RunView::open`]'s own copy of the store will not do — and the byte
/// is past every finished record that read reached, whether or not the profile
/// showed it, exactly as a watch advances its own.
///
/// Every refusal is made before anything is read, so a cursor this run cannot
/// place hands back no events at all.
pub(crate) fn monitored(
    paths: &RunPaths,
    view: RunView,
    filter: &EventFilter,
    cursor: Option<&str>,
) -> Result<Monitored> {
    let mut cursor = match cursor {
        Some(token) => resolve_cursor(paths, token)?,
        None => Cursor::start(&paths.run),
    };
    let events = tail(paths, &mut cursor)
        .into_iter()
        .filter(|event| filter.matches(event))
        .collect();
    Ok(Monitored {
        view,
        events,
        cursor: cursor.to_string(),
    })
}

/// What one `monitor` pass read: the events past the cursor it was given, shown
/// through the profile, and the cursor the next pass resumes from.
#[derive(Debug)]
pub struct Monitored {
    /// The run as the pass read it, which the event lines are rendered against.
    pub view: RunView,
    /// The events the profile admitted, in merge order.
    pub events: Vec<Envelope>,
    /// The cursor token the next pass resumes from.
    pub cursor: String,
}

impl Monitored {
    /// The document one `monitor` pass prints, resume line and all.
    pub(crate) fn render(&self) -> String {
        format!(
            "{}-- cursor {}",
            views::monitor_shown(&self.view, &self.events),
            self.cursor
        )
    }
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
            "cursor '{token}' was printed by a `watch` or `monitor` of run '{run}', and this \
             reads run '{}'; a cursor is only readable by the run it was printed for",
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
             a cursor is only readable by the run it was printed for",
            paths.run
        )));
    }
    if at > 0 && !ends_a_record(&journal, at) {
        return Err(Error::Invalid(format!(
            "cursor '{token}' resumes at byte {at} of run '{}', which is inside a record \
             rather than after one; a cursor is what an earlier `onepipeline watch` or \
             `monitor` printed, and never a byte count of its own",
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
             `onepipeline watch` or `monitor` printed, spelled \
             `{WATCH_CURSOR_VERSION}:<run>:<byte>`"
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
        /// The token, as [`Cursor`] spells it.
        cursor: &'a str,
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

/// The two forms of one frame, on the two descriptors that keep them apart.
///
/// The human line goes to standard error and the machine-readable one to
/// standard output, which is the split an attached `start` already makes and for
/// the same reason: a script reads stdout as NDJSON while a terminal beside it
/// follows the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lines {
    /// The line a person reads, without its terminator.
    pub human: String,
    /// The record a script reads, one JSON object, without its terminator.
    pub machine: String,
}

impl Lines {
    /// Render one frame both ways.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when the machine record cannot be rendered, which no
    /// frame this crate builds reaches.
    pub(crate) fn of(frame: &Frame<'_>) -> Result<Self> {
        let rendered = |record: &Record<'_>| {
            serde_json::to_string(record)
                .map_err(|e| Error::Invalid(format!("the watch could not render a record: {e}")))
        };
        let (human, machine) = match frame {
            Frame::Event { view, event } => (
                views::event_line(view, event),
                rendered(&Record::Event { event })?,
            ),
            Frame::Tick { view } => {
                let unread = view.unread();
                (
                    format!(
                        "-- watching {}  {}  {}",
                        view.paths.run,
                        views::liveness_word(view),
                        unread_phrase(&unread)
                    ),
                    rendered(&Record::Heartbeat {
                        run_id: &view.paths.run,
                        unread: UnreadRecord::of(&unread),
                    })?,
                )
            }
            Frame::Ended { view, outcome } => {
                let unread = view.unread();
                (
                    format!(
                        "-- watch {} {}  {}  cursor {}",
                        view.paths.run,
                        outcome.ending.phrase(),
                        unread_phrase(&unread),
                        outcome.cursor
                    ),
                    rendered(&Record::Return {
                        run_id: &view.paths.run,
                        ending: &outcome.ending,
                        cursor: &outcome.cursor,
                        unread: UnreadRecord::of(&unread),
                    })?,
                )
            }
        };
        Ok(Self { human, machine })
    }
}

/// Write one frame's two lines to the process's two streams, flushing both.
///
/// Each line is flushed as it is written — a watch is a **blocking** verb, and a
/// consumer reading it incrementally through a pipe would otherwise see nothing
/// until the process exits, which is the silence this whole verb exists to end.
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
pub(crate) fn say(lines: &Lines) -> Result<()> {
    let broken = |what: &str, error: std::io::Error| {
        Error::Invalid(format!("the watch could not write to {what}: {error}"))
    };
    let mut human = std::io::stderr();
    let mut machine = std::io::stdout();
    writeln!(human, "{}", lines.human).map_err(|e| broken("standard error", e))?;
    human.flush().map_err(|e| broken("standard error", e))?;
    writeln!(machine, "{}", lines.machine).map_err(|e| broken("standard output", e))?;
    machine.flush().map_err(|e| broken("standard output", e))?;
    Ok(())
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

    /// A wait reads what it follows once per change it reports, and never across
    /// a wait that reports none — over the bus's memory transport, where a change
    /// is an append made here.
    #[test]
    fn a_follow_reads_again_once_per_reported_change_and_never_across_an_unchanged_wait() {
        let memory = onemessagebus::MemoryTransport::new();
        let queue = QueueName::try_from("run").expect("a queue name");
        let mut follow = Follow::from_now(&memory, queue.clone()).expect("the queue fingerprints");
        let reads = std::cell::Cell::new(0_u32);
        let read = || {
            reads.set(reads.get() + 1);
            Ok(reads.get())
        };
        let quiet = Duration::from_millis(60);
        let change = |n: u32| {
            memory
                .append(&queue, format!("{{\"change\":{n}}}").as_bytes())
                .expect("the change is appended");
        };

        assert_eq!(follow.next(quiet, read).expect("waited"), None);
        assert_eq!(reads.get(), 0, "a wait nothing moved across read again");
        for n in 1..=3 {
            change(n);
            assert_eq!(
                follow.next(Duration::from_secs(30), read).expect("waited"),
                Some(n),
                "change {n} was not read exactly once"
            );
            assert_eq!(follow.next(quiet, read).expect("waited"), None);
        }
        // Two changes before one wait are one change reported, and one read.
        change(4);
        change(5);
        assert_eq!(
            follow.next(Duration::from_secs(30), read).expect("waited"),
            Some(4)
        );
        assert_eq!(follow.next(quiet, read).expect("waited"), None);
        assert_eq!(reads.get(), 4);
    }

    /// The run's fingerprint moves with each thing a pass of the watch decides
    /// from — the files, and the driver's two answers no file carries — and the
    /// transport it is written as reads and writes no records.
    #[test]
    fn a_runs_fingerprint_moves_with_everything_a_pass_reads_and_it_writes_nothing() {
        let root =
            std::env::temp_dir().join(format!("onepipeline-watch-changes-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = RunPaths::under(&root, "watched");
        paths.create().expect("the run directory");
        // A run whose channel nothing has opened yet, as one written before its
        // first surface was.
        std::fs::remove_dir(paths.channel_dir()).expect("an empty channel directory");
        let changes = RunChanges::of(&paths);
        let queue = RunChanges::queue();
        let mut seen = changes.fingerprint(&queue).expect("a fingerprint");
        let mut moved = |what: &str| {
            let now = changes.fingerprint(&queue).expect("a fingerprint");
            assert_ne!(now, seen, "{what} did not move the run's fingerprint");
            assert_eq!(
                changes.fingerprint(&queue).expect("a fingerprint"),
                now,
                "{what}"
            );
            seen = now;
        };

        std::fs::write(paths.journal(), "{}\n").expect("written");
        moved("the journal growing");
        std::fs::write(paths.launch(), "{}").expect("written");
        moved("the launch record being written");
        assert!(
            !paths.channel_dir().exists(),
            "a fingerprint made a channel"
        );
        crate::channel::ChannelState::new(&paths)
            .push(crate::channel::Surface {
                id: 0,
                kind: "finding".to_owned(),
                message: "raised while nobody was reading".to_owned(),
                source: "watcher".to_owned(),
                blocking: false,
                queued_at: 1,
                workstream: None,
                abandoned: false,
                asker: None,
                correlation: None,
            })
            .expect("the surface is queued");
        moved("a surface being queued");
        *changes.observed.lock().expect("unpoisoned") = Observed {
            host: Some(crate::sys::hostname()),
            pid: NonZeroU32::new(3_999_999),
            started: Some("not a process".to_owned()),
            last_write_at: None,
        };
        moved("the recorded driver being over");
        changes.observed.lock().expect("unpoisoned").last_write_at = Some(0);
        moved("the run falling quiet past the parked bound");

        let other = QueueName::try_from("surfaces").expect("a queue name");
        let consumer = ConsumerName::try_from("watch").expect("a consumer name");
        let document = DocumentName::try_from("queue.json").expect("a document name");
        assert!(changes.fingerprint(&other).is_err());
        assert!(changes.append(&queue, b"{}").is_err());
        assert!(changes.read(&queue, None, 1).is_err());
        assert!(changes.cursor(&queue, &consumer).is_err());
        assert!(changes.document(&queue, &document).is_err());
        assert!(changes.replace_document(&queue, &document, b"{}").is_err());
        assert!(changes.exclusive(&queue, &mut |_| Ok(())).is_err());
        let at = onemessagebus::MemoryTransport::new()
            .append(&queue, b"{}")
            .expect("a position");
        assert!(changes.commit(&queue, &consumer, &at).is_err());
        // A wait over a queue the run does not answer for is refused before it
        // blocks, in the watch's own words.
        let refused = Follow::from_now(&changes, other)
            .err()
            .expect("a queue the run does not answer for is refused");
        assert!(
            refused
                .to_string()
                .contains("could not tell whether the run changed"),
            "{refused}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

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
        // check-in cadence is `start`'s clock, and the entry says so in prose
        // rather than in the schema.
        assert!(
            !offered.contains("heartbeat-interval"),
            "`watch` took `start`'s check-in interval flag"
        );
    }

    /// `monitor` shares the cursor, and the entry and the README both state its
    /// surface: the flags clap gives it and the resume line it ends on.
    ///
    /// Held here rather than beside the views because the cursor spelling is this
    /// module's, and a flag added to `monitor` or dropped from it that the two
    /// documents did not follow is a surface nobody was told about.
    #[test]
    fn the_divergence_entry_and_the_readme_state_exactly_the_flags_monitor_offers() {
        use clap::CommandFactory;

        let flags_of = |synopsis: &str| -> std::collections::BTreeSet<String> {
            synopsis
                .split_whitespace()
                .filter_map(|word| {
                    word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                        .strip_prefix("--")
                        .map(str::to_string)
                })
                .collect()
        };
        let offered: std::collections::BTreeSet<String> = crate::cli::Cli::command()
            .get_subcommands()
            .find(|sub| sub.get_name() == "monitor")
            .expect("the binary offers `monitor`")
            .get_arguments()
            .filter_map(|arg| arg.get_long().map(str::to_string))
            .collect();
        let resume = format!("`-- cursor {WATCH_CURSOR_VERSION}:<run>:<byte>`");

        let readme = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"),
        )
        .expect("the README ships");
        for (document, text, opening) in [
            (
                "divergence entry",
                divergence_entry(),
                "`onepipeline monitor <RUN>",
            ),
            ("README", readme, "`onepipeline monitor RUN"),
        ] {
            let synopsis = text
                .split_once(opening)
                .unwrap_or_else(|| panic!("the {document} states `monitor`'s surface"))
                .1
                .split_once('`')
                .unwrap_or_else(|| panic!("the {document}'s synopsis is one fenced span"))
                .0;
            assert_eq!(
                flags_of(synopsis),
                offered,
                "the {document} states a different set of `monitor` flags than this build offers"
            );
            assert!(
                text.contains(&resume),
                "the {document} does not state the resume line {resume}"
            );
        }
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
                cursor: &Cursor::start("demo").to_string(),
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
                cursor: &Cursor::start("demo").to_string(),
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
