//! `onepipeline watch --flow` — a bounded, resumable wait on one flow and every
//! run launched in it.
//!
//! What it returns on, with which status, is the Flows section of
//! `docs/stop-guard.md` and entry 114 of `docs/contract-divergences.md`; neither
//! is restated here. What belongs beside the code is that this wait **polls**
//! where a run's waits on a fingerprint: what it watches is the flow's own
//! files, its holder's liveness — which moves with no file moving — and a set of
//! runs that grows while it waits. Each poll is a handful of small reads, made at
//! most every [`POLL`], and a run's whole journal is never read twice: each
//! member is tailed from the cursor the last poll left it at.
//!
//! Like a run's watch it takes no lock, consumes no surface, and records itself
//! as a lease beside what it watches — the flow's own `watchers/` — so
//! `unwatched` can ask whether anything is watching the flow.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::cli::{WatchTimeout, WatchUntil, WATCH_CURSOR_VERSION};
use crate::error::{
    Error, Result, EXIT_FLOW_FAILED, EXIT_NODE_SETTLED, EXIT_RUN_JOINED, EXIT_SUCCESS,
    EXIT_SURFACE_WAITING, EXIT_WATCH_ELAPSED,
};
use crate::flow::{Flow, Standing};
use crate::ledger::RunPaths;
use crate::watch::{Cursor, Lines};

/// The longest a flow watch goes between two looks at what it watches.
const POLL: Duration = crate::watch::DEADLINE_CHECK;

/// The conditions a flow watch returns on when it is told none: all four.
const DEFAULT_UNTIL: [WatchUntil; 4] = [
    WatchUntil::Surface,
    WatchUntil::NodeSettled,
    WatchUntil::RunJoined,
    WatchUntil::LandingProgress,
];

/// What a flow watch is asked for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Request {
    /// How long to wait before giving up.
    pub(crate) timeout: WatchTimeout,
    /// How long a silence may last before the stream says it is still there.
    /// Zero turns the heartbeat off.
    pub(crate) tick: Duration,
    /// A cursor an earlier flow watch printed, to resume from.
    // llmlint: ignore[invalid_states_unrepresentable] a cursor token is external input, placed against the flow and each member's journal by `resume`, which is the check no type can make — the same reason `watch::Request::cursor` gives.
    pub(crate) cursor: Option<String>,
    /// What ends the wait beside the flow's ending: empty is all four.
    pub(crate) until: Vec<WatchUntil>,
    /// The profile a landing's events are judged through before one wakes the
    /// watch: the shipped `planner` profile unless the caller named another.
    pub(crate) filter: crate::filter::EventFilter,
}

/// Why a flow watch returned.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ending {
    /// A planner surface is waiting on the flow's own channel.
    SurfaceOnFlow,
    /// A planner surface is waiting on a member run's channel.
    // llmlint: ignore[invalid_states_unrepresentable] a run id read off the flow's own membership, which `Flow::members` admits only as a run id; the crate spells one `String` everywhere.
    SurfaceOnRun(String),
    /// A member run's node settled.
    // llmlint: ignore[invalid_states_unrepresentable] the run and the node as a journalled settlement names them, for `watch::Ending::NodeSettled`'s reason.
    NodeSettled { run: String, node: String },
    /// A run joined the flow.
    // llmlint: ignore[invalid_states_unrepresentable] a run id read off the flow's own membership, as `SurfaceOnRun`'s is.
    RunJoined(String),
    /// One of the flow's landings recorded an event the watch's profile admits.
    LandingProgress(Box<Progress>),
    /// The flow's program ended `0`.
    Ended,
    /// The flow's program ended with this non-zero status.
    Failed(std::num::NonZeroI32),
    /// The flow's holder died with no ending recorded.
    Died,
    /// The wait's own bound ran out.
    Elapsed,
}

/// One landing event that woke a flow watch: which landing, and the event as it
/// was recorded.
#[derive(Debug, Clone, PartialEq)]
struct Progress {
    /// The landing whose journal holds it.
    // llmlint: ignore[invalid_states_unrepresentable] a landing id read off the flow's own `landings/`, which `Flow::landings` admits only as the id its request names.
    landing: String,
    /// The event.
    event: crate::event::Envelope,
    /// The branches the landing registered, which a relayed `onevcs` event does
    /// not name itself.
    subjects: Vec<crate::flow::LandingSubject>,
}

impl Eq for Progress {}

impl Progress {
    /// The subjects the event is about: a landing transition's own, or — for a
    /// relayed `onevcs` event, which names none — the landing's.
    fn subjects(&self) -> serde_json::Value {
        self.event
            .payload
            .get("subjects")
            .cloned()
            .unwrap_or_else(|| json!(self.subjects))
    }

    /// The significance the event's producer stamped, where it stamped one.
    fn significance(&self) -> Option<&str> {
        self.event
            .labels
            .extra
            .get("significance")
            .and_then(serde_json::Value::as_str)
    }
}

impl Ending {
    /// The condition's word, as the machine record's `condition` spells it.
    const fn as_str(&self) -> &'static str {
        match self {
            Self::SurfaceOnFlow | Self::SurfaceOnRun(_) => "surface",
            Self::NodeSettled { .. } => "node-settled",
            Self::RunJoined(_) => "run-joined",
            Self::LandingProgress(_) => "landing-progress",
            Self::Ended => "ended",
            Self::Failed(_) | Self::Died => "flow-failed",
            Self::Elapsed => "elapsed",
        }
    }

    /// The status the binary exits with for this ending.
    const fn exit_code(&self) -> i32 {
        match self {
            Self::SurfaceOnFlow | Self::SurfaceOnRun(_) => EXIT_SURFACE_WAITING,
            Self::NodeSettled { .. } => EXIT_NODE_SETTLED,
            Self::RunJoined(_) => EXIT_RUN_JOINED,
            Self::LandingProgress(_) | Self::Ended => EXIT_SUCCESS,
            Self::Failed(_) | Self::Died => EXIT_FLOW_FAILED,
            Self::Elapsed => EXIT_WATCH_ELAPSED,
        }
    }

    /// The human line's words for this ending, naming what it is about.
    fn phrase(&self, flow: &str) -> String {
        let close = format!(
            "close it with: onepipeline unwatched --acknowledge-flow {flow} --reason <TEXT>"
        );
        match self {
            Self::SurfaceOnFlow => format!("surface on flow {flow}"),
            Self::SurfaceOnRun(run) => format!("surface on run {run}"),
            Self::NodeSettled { run, node } => format!("node-settled {run} {node}"),
            Self::RunJoined(run) => format!("run-joined {run}"),
            Self::LandingProgress(progress) => {
                let named: Vec<String> = progress
                    .subjects()
                    .as_array()
                    .map(|subjects| {
                        subjects
                            .iter()
                            .filter_map(|subject| {
                                Some(format!(
                                    "{} [{}]",
                                    subject.get("branch")?.as_str()?,
                                    subject.get("identity")?.as_str()?
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                format!(
                    "landing-progress {} {}{}{}",
                    progress.landing,
                    progress.event.kind.0,
                    progress
                        .significance()
                        .map(|said| format!(" ({said})"))
                        .unwrap_or_default(),
                    match named.is_empty() {
                        true => String::new(),
                        false => format!(" {}", named.join(", ")),
                    }
                )
            }
            Self::Ended => "ended with status 0".to_owned(),
            Self::Failed(status) => {
                format!("flow-failed: it ended with status {status} — {close}")
            }
            Self::Died => format!(
                "flow-failed: it died — its holder is gone with no ending recorded — {close}"
            ),
            Self::Elapsed => "elapsed".to_owned(),
        }
    }

    /// The machine record's fields about this ending, beyond its word and code.
    fn fields(&self, record: &mut serde_json::Map<String, serde_json::Value>) {
        match self {
            Self::SurfaceOnRun(run) | Self::RunJoined(run) => {
                record.insert("run_id".to_owned(), json!(run));
            }
            Self::NodeSettled { run, node } => {
                record.insert("run_id".to_owned(), json!(run));
                record.insert("node".to_owned(), json!(node));
            }
            Self::Ended => {
                record.insert("status".to_owned(), json!(0));
            }
            Self::Failed(status) => {
                record.insert("status".to_owned(), json!(status));
            }
            Self::Died => {
                record.insert("died".to_owned(), json!(true));
            }
            Self::LandingProgress(progress) => {
                record.insert("landing_id".to_owned(), json!(progress.landing));
                record.insert("kind".to_owned(), json!(progress.event.kind.0));
                record.insert("source".to_owned(), json!(progress.event.source));
                record.insert("subjects".to_owned(), progress.subjects());
                record.insert("significance".to_owned(), json!(progress.significance()));
                record.insert(
                    "operation_id".to_owned(),
                    json!(progress
                        .event
                        .labels
                        .extra
                        .get("operation_id")
                        .and_then(serde_json::Value::as_str)),
                );
                record.insert("stream".to_owned(), json!(progress.event.stream));
                record.insert("seq".to_owned(), json!(progress.event.seq));
                if let Some(status) = progress.event.payload.get("status") {
                    record.insert("status".to_owned(), status.clone());
                }
            }
            Self::SurfaceOnFlow | Self::Elapsed => {}
        }
    }
}

/// The conditions a flow watch was told to return on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Selected {
    surface: bool,
    node_settled: bool,
    run_joined: bool,
    landing_progress: bool,
}

impl Selected {
    /// Read the conditions, refusing one a flow watch does not return on.
    fn resolve(until: &[WatchUntil]) -> Result<Self> {
        let mut chosen = Self::default();
        for condition in until {
            match condition {
                WatchUntil::Surface => chosen.surface = true,
                WatchUntil::NodeSettled => chosen.node_settled = true,
                WatchUntil::RunJoined => chosen.run_joined = true,
                WatchUntil::LandingProgress => chosen.landing_progress = true,
                other => {
                    return Err(Error::Invalid(format!(
                        "`--until {other}` is a run watch's condition: `watch --flow` returns on \
                         surface, node-settled, run-joined and landing-progress, and always on \
                         the flow's ending"
                    )))
                }
            }
        }
        Ok(chosen)
    }
}

/// The cursor-token prefix a flow watch prints and reads back.
const CURSOR_PREFIX: &str = "flow";

/// The version of a flow cursor that places each landing as well as each run:
/// `flow:2:<flow>:<landing>@<byte>/...:<run>@<byte>/...`. A flow and a landing
/// id carry no `:`, so the run places are everything after the third.
const LANDING_CURSOR_VERSION: &str = "2";

/// Where a flow watch has read to: each member run's journal, and each
/// landing's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Known {
    runs: BTreeMap<String, Cursor>,
    /// Each landing's journal, by byte.
    landings: BTreeMap<String, u64>,
    /// Where a landing the cursor does not name starts: its first record for a
    /// cursor that places landings — it was registered after the watch that
    /// printed it — and its end for one printed before landings existed, which
    /// never replays a stream it did not know of.
    unnamed_from_start: bool,
}

/// `watch --flow`: block until the flow or one of its runs needs a supervisor,
/// or until the wait runs out, and answer the status.
///
/// Every refusal — the conditions and the cursor — is made before the lease is
/// written or a second is waited. Every line goes to `say` as it happens, and a
/// `say` that refuses ends the wait with its refusal.
///
/// # Errors
///
/// A condition a flow watch does not return on, a cursor this flow cannot
/// place, a deadline past what the clock can name, and a `say` that refused.
pub(crate) fn watch(
    root: &std::path::Path,
    flow: &Flow,
    request: &Request,
    say: &mut dyn FnMut(&Lines) -> Result<()>,
) -> Result<i32> {
    let until: &[WatchUntil] = match request.until.is_empty() {
        true => &DEFAULT_UNTIL,
        false => &request.until,
    };
    let selected = Selected::resolve(until)?;
    let deadline = crate::watch::deadline(request.timeout)?;
    let mut known = match &request.cursor {
        Some(token) => resume(root, flow, token)?,
        None => armed_at(root, flow)?,
    };
    let filter = &request.filter;
    let _armed = crate::watchers::Armed::arm(&flow.paths, request.timeout, until);
    let mut quiet_since = Instant::now();
    let mut unread = 0_usize;
    let mut first = true;
    let ended = |ending: Ending,
                 known: &Known,
                 unread: usize,
                 say: &mut dyn FnMut(&Lines) -> Result<()>| {
        say(&ending_lines(
            flow.id(),
            &ending,
            &cursor_of(flow.id(), known),
            unread,
        )?)?;
        Ok(ending.exit_code())
    };
    loop {
        // Asked before the pass rather than only after it, so a wait whose
        // deadline came due while it slept does nothing past the deadline but
        // say how it ended.
        let due = || deadline.is_some_and(|deadline| Instant::now() >= deadline);
        if !first && due() {
            return ended(Ending::Elapsed, &known, unread, say);
        }
        first = false;
        let (ending, waiting) = pass(root, flow, selected, filter, &mut known)?;
        unread = waiting;
        if let Some(ending) = ending {
            return ended(ending, &known, unread, say);
        }
        if due() {
            return ended(Ending::Elapsed, &known, unread, say);
        }
        if !request.tick.is_zero() && quiet_since.elapsed() >= request.tick {
            say(&heartbeat_lines(flow, unread)?)?;
            quiet_since = Instant::now();
        }
        std::thread::sleep(
            crate::watch::wake_within(deadline, request.tick, quiet_since).min(POLL),
        );
    }
}

/// Every member run positioned at the end of its journal as it stands, which is
/// what a watch given no cursor waits past: what was journalled before it armed
/// is history, and answers no condition. A **live** landing is the exception,
/// read from its first recorded transition, so a registration or a start
/// recorded before the first watch armed is not lost; a landing that already
/// ended is history, as a run's settlement is.
fn armed_at(root: &std::path::Path, flow: &Flow) -> Result<Known> {
    let runs = flow
        .members()?
        .into_iter()
        .map(|run| {
            let mut cursor = Cursor::start(&run);
            crate::watch::tail(&RunPaths::under(root, &run), &mut cursor);
            (run, cursor)
        })
        .collect();
    let landings = landings_of(flow)?
        .into_iter()
        .map(|landing| {
            let at = match landing.is_live() {
                true => 0,
                false => end_of(&landing.journal()),
            };
            (landing.id().to_owned(), at)
        })
        .collect();
    Ok(Known {
        runs,
        landings,
        unnamed_from_start: true,
    })
}

/// The flow's landings, or the refusal that says their state cannot be read.
fn landings_of(flow: &Flow) -> Result<Vec<crate::flow::Landing>> {
    flow.landings().map_err(|why| {
        Error::Invalid(format!(
            "flow '{}': {why}; it is owed supervision until that is repaired",
            flow.id()
        ))
    })
}

/// The byte just past a journal's last finished record.
fn end_of(journal: &std::path::Path) -> u64 {
    crate::journal::finished_after(journal, 0).1
}

/// The member runs a cursor token names, each placed against its own journal.
///
/// A member the token does not name is past the cursor: it joined after the
/// watch that printed it, and the first pass reports it.
fn resume(root: &std::path::Path, flow: &Flow, token: &str) -> Result<Known> {
    let refusal = || {
        Error::Invalid(format!(
            "'{token}' is not a cursor this build reads for a flow; a flow's cursor is what an \
             earlier `onepipeline watch --flow` printed, spelled \
             `{CURSOR_PREFIX}:{LANDING_CURSOR_VERSION}:<flow>:<landing>@<byte>/...:<run>@<byte>/...`, \
             or before landings `{CURSOR_PREFIX}:{WATCH_CURSOR_VERSION}:<flow>:<run>@<byte>/...`"
        ))
    };
    let (rest, placed) =
        match token.strip_prefix(&format!("{CURSOR_PREFIX}:{LANDING_CURSOR_VERSION}:")) {
            Some(rest) => (rest, true),
            None => (
                token
                    .strip_prefix(&format!("{CURSOR_PREFIX}:{WATCH_CURSOR_VERSION}:"))
                    .ok_or_else(refusal)?,
                false,
            ),
        };
    // A flow id carries no `:`, so the first one ends it.
    let (named, rest) = rest.split_once(':').ok_or_else(refusal)?;
    // A landing id carries none either: in a cursor that places landings, the
    // next one ends their places, and the runs' are everything after it.
    let (landing_places, places) = match placed {
        true => rest.split_once(':').ok_or_else(refusal)?,
        false => ("", rest),
    };
    if named != flow.id() {
        return Err(Error::Invalid(format!(
            "cursor '{token}' was printed by a watch of flow '{named}', and this watches flow \
             '{}'; a cursor is only readable by the flow it was printed for",
            flow.id()
        )));
    }
    let members = flow.members()?;
    let mut known = BTreeMap::new();
    // No member is spelled as nothing at all; otherwise every place is one run,
    // named once.
    let places: Vec<&str> = match places {
        "" => Vec::new(),
        places => places.split('/').collect(),
    };
    for place in places {
        // The byte follows the last `@`, so a run id carrying one reads back.
        let (run, at) = place.rsplit_once('@').ok_or_else(refusal)?;
        if !members.iter().any(|member| member == run) {
            return Err(Error::Invalid(format!(
                "cursor '{token}' names run '{run}', which is not a run of flow '{}'",
                flow.id()
            )));
        }
        let cursor = crate::watch::resolve_cursor(
            &RunPaths::under(root, run),
            &format!("{WATCH_CURSOR_VERSION}:{run}:{at}"),
        )?;
        if known.insert(run.to_owned(), cursor).is_some() {
            return Err(Error::Invalid(format!(
                "cursor '{token}' names run '{run}' twice; a flow's cursor places each run once"
            )));
        }
    }
    let landings = landings_of(flow)?;
    let mut placed_landings = BTreeMap::new();
    let landing_places: Vec<&str> = match landing_places {
        "" => Vec::new(),
        places => places.split('/').collect(),
    };
    for place in landing_places {
        let (landing, at) = place.rsplit_once('@').ok_or_else(refusal)?;
        let Some(registered) = landings.iter().find(|known| known.id() == landing) else {
            return Err(Error::Invalid(format!(
                "cursor '{token}' names landing '{landing}', which is not a landing of flow '{}'",
                flow.id()
            )));
        };
        let at: u64 = at.parse().map_err(|_| refusal())?;
        if at > end_of(&registered.journal()) {
            return Err(Error::Invalid(format!(
                "cursor '{token}' places landing '{landing}' past the end of its journal; a \
                 cursor is only readable against the journal it was printed for"
            )));
        }
        if placed_landings.insert(landing.to_owned(), at).is_some() {
            return Err(Error::Invalid(format!(
                "cursor '{token}' names landing '{landing}' twice; a flow's cursor places each \
                 landing once"
            )));
        }
    }
    if !placed {
        // A cursor printed before landings places none, and replays none: every
        // landing it could not have known of starts where its journal stands.
        for landing in &landings {
            placed_landings.insert(landing.id().to_owned(), end_of(&landing.journal()));
        }
    }
    Ok(Known {
        runs: known,
        landings: placed_landings,
        unnamed_from_start: placed,
    })
}

/// The token a later flow watch resumes from: every member and landing this
/// watch knows, and how far into each one's journal it has read.
fn cursor_of(flow: &str, known: &Known) -> String {
    let runs: Vec<String> = known
        .runs
        .iter()
        .map(|(run, cursor)| format!("{run}@{}", cursor.at))
        .collect();
    let landings: Vec<String> = known
        .landings
        .iter()
        .map(|(landing, at)| format!("{landing}@{at}"))
        .collect();
    format!(
        "{CURSOR_PREFIX}:{LANDING_CURSOR_VERSION}:{flow}:{}:{}",
        landings.join("/"),
        runs.join("/")
    )
}

/// One look at the flow and every member, in the order the conditions are
/// answered: a joined run, a waiting surface, a settled node, and the flow's
/// own ending. Answers the ending this look reached, if any, and how many
/// planner surfaces are unread across the flow's channel and its members'.
///
/// # Errors
///
/// The flow's membership becoming unreadable while the watch waits.
fn pass(
    root: &std::path::Path,
    flow: &Flow,
    selected: Selected,
    filter: &crate::filter::EventFilter,
    known: &mut Known,
) -> Result<(Option<Ending>, usize)> {
    let landings = landings_of(flow)?;
    for landing in &landings {
        if !known.landings.contains_key(landing.id()) {
            let at = match known.unnamed_from_start {
                true => 0,
                false => end_of(&landing.journal()),
            };
            known.landings.insert(landing.id().to_owned(), at);
        }
    }
    let before = known.landings.clone();
    let progress = landing_progress(root, flow, &landings, selected, filter, known)?;
    let (ending, unread) = runs_pass(root, flow, selected, &mut known.runs, progress.is_some())?;
    // A joined run and a waiting surface are answered first, as they always
    // were; a landing's event comes before a settlement and the flow's ending,
    // so an operation's notice is never lost behind the flow it ends.
    Ok(match (ending, progress) {
        (
            Some(ending @ (Ending::RunJoined(_) | Ending::SurfaceOnFlow | Ending::SurfaceOnRun(_))),
            progress,
        ) => {
            // The landing's event is the next watch's to report: its place is
            // put back where this pass found it.
            if progress.is_some() {
                known.landings = before;
            }
            (Some(ending), unread)
        }
        (_, Some(progress)) => (Some(Ending::LandingProgress(Box::new(progress))), unread),
        (Some(ending @ (Ending::Ended | Ending::Failed(_) | Ending::Died)), None) => {
            // The flow ended or died after this pass read its landings: every
            // landing proved dead since has its death written before the flow's
            // ending is reported, so no reader is told of a death the record
            // does not hold.
            for landing in &landings {
                crate::land::settle_death(root, flow, landing)?;
            }
            (Some(ending), unread)
        }
        (ending, None) => (ending, unread),
    })
}

/// The first event past each landing's place that the profile admits, read one
/// at a time so the cursor ends just past what it reports and the next event is
/// the next watch's; every event the profile leaves out on the way is consumed.
///
/// A landing whose process is proved gone with no ending has its death written
/// first, by whichever reader proves it. A terminal event of a landing that
/// holds its flow is consumed without waking anybody: that flow's own ending,
/// which follows it, is what reports it.
fn landing_progress(
    root: &std::path::Path,
    flow: &Flow,
    landings: &[crate::flow::Landing],
    selected: Selected,
    filter: &crate::filter::EventFilter,
    known: &mut Known,
) -> Result<Option<Progress>> {
    for landing in landings {
        crate::land::settle_death(root, flow, landing)?;
        let at = known.landings.get(landing.id()).copied().unwrap_or(0);
        let journal = landing.journal();
        if !selected.landing_progress {
            known
                .landings
                .insert(landing.id().to_owned(), end_of(&journal));
            continue;
        }
        let holds = landing.holds(flow);
        let wakes = |event: &crate::event::Envelope| {
            filter.matches(event)
                && !(holds && crate::flow::is_terminal_landing_kind(&event.kind.0))
        };
        let (fresh, through) = crate::journal::finished_through_first(&journal, at, wakes);
        known.landings.insert(landing.id().to_owned(), through);
        if let Some(event) = fresh.into_iter().find(|event| wakes(event)) {
            return Ok(Some(Progress {
                landing: landing.id().to_owned(),
                event,
                subjects: landing.request.subjects.clone(),
            }));
        }
    }
    Ok(None)
}

/// [`pass`] over the flow's member runs and its own ending, as it was before
/// landings. With `progress` already found, a join or a surface still wins over
/// it, and nothing past them is read — so no settlement is consumed behind a
/// landing's notice.
fn runs_pass(
    root: &std::path::Path,
    flow: &Flow,
    selected: Selected,
    known: &mut BTreeMap<String, Cursor>,
    progress: bool,
) -> Result<(Option<Ending>, usize)> {
    let members = flow.members()?;
    // A run that joined is entered before anything else is asked. Told to return
    // on a join, the watch returns on the first one — one at a time, so a run
    // this watch has not reported joining stays out of its cursor and the next
    // watch reports it. Told not to, every run that joined is entered here, so
    // its surfaces and its settlements are this very pass's to answer rather
    // than lost behind a flow that ended in the same instant.
    let mut joined: Option<String> = None;
    for run in &members {
        if known.contains_key(run) {
            continue;
        }
        known.insert(run.clone(), Cursor::start(run));
        if selected.run_joined {
            joined = Some(run.clone());
            break;
        }
    }
    // Counted over every run of the flow — those this watch has entered and
    // those that joined and it has not reported yet — so the line a join ends on
    // counts what every member is holding.
    let every: std::collections::BTreeSet<&String> = members.iter().chain(known.keys()).collect();
    let unread = flow.channel().queue().waiting.len()
        + every
            .into_iter()
            .map(|run| {
                crate::channel::ChannelState::new(&RunPaths::under(root, run))
                    .queue()
                    .waiting
                    .len()
            })
            .sum::<usize>();
    if let Some(run) = joined {
        return Ok((Some(Ending::RunJoined(run)), unread));
    }
    if selected.surface {
        if crate::watch::surface_waiting(&flow.paths).is_some() {
            return Ok((Some(Ending::SurfaceOnFlow), unread));
        }
        if let Some(run) = known
            .keys()
            .find(|run| crate::watch::surface_waiting(&RunPaths::under(root, run)).is_some())
        {
            return Ok((Some(Ending::SurfaceOnRun(run.clone())), unread));
        }
    }
    if progress {
        return Ok((None, unread));
    }
    // One settlement at a time: each member is read up to and including its
    // first settlement and no further, and the members after it not at all, so
    // the cursor this ends on is past exactly what it reported and a second
    // settlement is the next watch's to report rather than lost behind it.
    for (run, cursor) in known.iter_mut() {
        let journal = RunPaths::under(root, run).journal();
        if !selected.node_settled {
            crate::watch::tail(&RunPaths::under(root, run), cursor);
            continue;
        }
        let (fresh, at) = crate::journal::finished_through_first(&journal, cursor.at, |event| {
            crate::watch::settlement_of(event).is_some()
        });
        cursor.at = at;
        if let Some(node) = fresh.iter().find_map(crate::watch::settlement_of) {
            return Ok((
                Some(Ending::NodeSettled {
                    run: run.clone(),
                    node: node.to_owned(),
                }),
                unread,
            ));
        }
    }
    let ending = match flow.standing() {
        Ok(Standing::Ended(status)) => {
            Some(std::num::NonZeroI32::new(status).map_or(Ending::Ended, Ending::Failed))
        }
        Ok(Standing::Died) => Some(Ending::Died),
        // An ending that cannot be read says neither that the flow ended nor that
        // it did not, and `unwatched` names it: the wait goes on.
        Ok(Standing::Live) | Err(_) => None,
    };
    Ok((ending, unread))
}

/// How many unread planner surfaces, as one clause, a zero said out loud.
fn unread_phrase(unread: usize) -> String {
    match unread {
        0 => "0 unread planner surfaces".to_owned(),
        count => format!("{count} unread planner surface(s)"),
    }
}

fn rendered(record: &serde_json::Value) -> Result<String> {
    serde_json::to_string(record)
        .map_err(|e| Error::Invalid(format!("the watch could not render a record: {e}")))
}

/// The two lines a flow watch ends on.
fn ending_lines(flow: &str, ending: &Ending, cursor: &str, unread: usize) -> Result<Lines> {
    let mut record = serde_json::Map::new();
    record.insert("watch".to_owned(), json!("return"));
    record.insert("flow_id".to_owned(), json!(flow));
    record.insert("condition".to_owned(), json!(ending.as_str()));
    record.insert("exit".to_owned(), json!(ending.exit_code()));
    ending.fields(&mut record);
    record.insert("cursor".to_owned(), json!(cursor));
    record.insert("unread".to_owned(), json!({"count": unread}));
    Ok(Lines {
        human: format!(
            "-- watch flow {flow} {}  {}  cursor {cursor}",
            ending.phrase(flow),
            unread_phrase(unread)
        ),
        machine: rendered(&serde_json::Value::Object(record))?,
    })
}

/// The two lines a flow watch says while nothing has happened for a tick.
fn heartbeat_lines(flow: &Flow, unread: usize) -> Result<Lines> {
    let standing = match flow.standing() {
        Ok(Standing::Live) => "LIVE",
        Ok(Standing::Ended(_)) => "ENDED",
        Ok(Standing::Died) => "DIED",
        Err(_) => "UNKNOWN",
    };
    Ok(Lines {
        human: format!(
            "-- watching flow {}  {standing}  {}",
            flow.id(),
            unread_phrase(unread)
        ),
        machine: rendered(&json!({
            "watch": "heartbeat",
            "flow_id": flow.id(),
            "unread": {"count": unread},
        }))?,
    })
}

/// Every event of every landing `flow` holds, in merge order, that `filter`
/// admits — each landing's death written first where it is proved. The event
/// view `next --flow` answers with and `monitor --flow` renders.
///
/// # Errors
///
/// Landing state that cannot be read, or a death that could not be written.
pub(crate) fn events(
    root: &std::path::Path,
    flow: &Flow,
    filter: &crate::filter::EventFilter,
    cursor: Option<&str>,
) -> Result<(Vec<crate::event::Envelope>, String)> {
    let mut known = match cursor {
        Some(token) => resume(root, flow, token)?,
        None => Known {
            unnamed_from_start: true,
            ..Known::default()
        },
    };
    let mut events = Vec::new();
    for landing in landings_of(flow)? {
        crate::land::settle_death(root, flow, &landing)?;
        let at = known.landings.get(landing.id()).copied().unwrap_or(0);
        let (fresh, through) = crate::journal::finished_after(&landing.journal(), at);
        known.landings.insert(landing.id().to_owned(), through);
        events.extend(fresh.into_iter().filter(|event| filter.matches(event)));
    }
    crate::journal::merge_order(&mut events);
    for run in flow.members()? {
        let mut place = Cursor::start(&run);
        crate::watch::tail(&RunPaths::under(root, &run), &mut place);
        known.runs.entry(run).or_insert(place);
    }
    Ok((events, cursor_of(flow.id(), &known)))
}

/// `onepipeline monitor --flow ID`: one line per landing event the profile
/// admits, the flow's own trailer, and the cursor a later `monitor --flow` or
/// `watch --flow` resumes from.
///
/// # Errors
///
/// See [`events`].
pub(crate) fn monitor(
    root: &std::path::Path,
    flow: &Flow,
    filter: &crate::filter::EventFilter,
    cursor: Option<&str>,
) -> Result<String> {
    let (events, cursor) = events(root, flow, filter, cursor)?;
    let mut out = String::from(
        "Flow landing events; every onevcs operation event is relayed whole into the landing's \
         journal.\n",
    );
    for event in &events {
        let landing = event
            .labels
            .extra
            .get("landing_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("-");
        let significance = event
            .labels
            .extra
            .get("significance")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("-");
        out.push_str(&format!(
            "{}  {}:{}  {:<22} {:<9} {}\n",
            event.ts,
            event.source.as_str(),
            landing,
            event.kind.0,
            significance,
            crate::views::one_line(
                event
                    .payload
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
            ),
        ));
    }
    let standing = match flow.standing() {
        Ok(Standing::Live) => "LIVE".to_owned(),
        Ok(Standing::Ended(status)) => format!("ENDED {status}"),
        Ok(Standing::Died) => "DIED".to_owned(),
        Err(_) => "UNKNOWN".to_owned(),
    };
    out.push_str(&format!(
        "-- flow {}  {standing}\n-- cursor {cursor}",
        flow.id()
    ));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cursor names the flow and every member's place, and reads back as what
    /// it printed; a flow's cursor is refused by a run watch's parser and the
    /// other way round, because each is spelled with its own prefix.
    #[test]
    fn a_flow_cursor_is_spelled_with_its_flow_and_every_members_place() {
        let mut known = Known::default();
        let mut a = Cursor::start("draft");
        a.at = 120;
        known.runs.insert("draft".to_owned(), a);
        known
            .runs
            .insert("spikes:x@y".to_owned(), Cursor::start("spikes:x@y"));
        known.landings.insert("landing-1".to_owned(), 40);
        known.landings.insert("landing-2".to_owned(), 0);
        assert_eq!(
            cursor_of("plan", &known),
            format!(
                "flow:{LANDING_CURSOR_VERSION}:plan:landing-1@40/landing-2@0:draft@120/spikes:x@y@0"
            )
        );
        assert_eq!(
            cursor_of("plan", &Known::default()),
            format!("flow:{LANDING_CURSOR_VERSION}:plan::")
        );
    }

    /// Each ending exits the status the contract gives it, and the record names
    /// what it is about.
    #[test]
    fn each_ending_exits_its_own_status_and_names_what_it_is_about() {
        for (ending, code, word) in [
            (Ending::SurfaceOnFlow, 4, "surface"),
            (Ending::SurfaceOnRun("r".into()), 4, "surface"),
            (
                Ending::NodeSettled {
                    run: "r".into(),
                    node: "n".into(),
                },
                6,
                "node-settled",
            ),
            (Ending::RunJoined("r".into()), 8, "run-joined"),
            (Ending::Ended, 0, "ended"),
            (
                Ending::Failed(std::num::NonZeroI32::new(3).expect("non-zero")),
                9,
                "flow-failed",
            ),
            (Ending::Died, 9, "flow-failed"),
            (Ending::Elapsed, 5, "elapsed"),
        ] {
            let lines = ending_lines("plan", &ending, "flow:1:plan:", 2).expect("lines");
            let record: serde_json::Value =
                serde_json::from_str(&lines.machine).expect("one JSON object");
            assert_eq!(record["exit"], code, "{lines:?}");
            assert_eq!(record["condition"], word, "{lines:?}");
            assert_eq!(record["cursor"], "flow:1:plan:", "{lines:?}");
            assert!(
                lines.human.ends_with("cursor flow:1:plan:"),
                "{}",
                lines.human
            );
        }
        let failed = ending_lines(
            "plan",
            &Ending::Failed(std::num::NonZeroI32::new(3).expect("non-zero")),
            "c",
            0,
        )
        .expect("lines");
        assert!(
            failed.human.contains("ended with status 3")
                && failed
                    .human
                    .contains("onepipeline unwatched --acknowledge-flow plan --reason <TEXT>"),
            "{}",
            failed.human
        );
    }

    /// The Flows section of `docs/stop-guard.md` is the one statement of this
    /// surface, so it is held to the code both ways: each synopsis names exactly
    /// the flags clap offers, the exit table exactly the endings and their codes,
    /// the return record exactly the keys a return can carry, and the wake reserve
    /// and the deadline check the constants the verbs use.
    #[test]
    fn the_flows_section_states_exactly_this_surface() {
        use clap::CommandFactory;
        use std::collections::BTreeSet;

        let page = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("docs")
                .join("stop-guard.md"),
        )
        .expect("the page ships");
        let section = page
            .split_once("\n## Flows\n")
            .expect("the page has a Flows section")
            .1
            .split_once("\n## ")
            .map_or(String::new(), |(section, _)| section.to_owned());
        let flags_of = |synopsis: &str| -> BTreeSet<String> {
            synopsis
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .filter_map(|word| word.strip_prefix("--"))
                .filter(|word| !word.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let synopsis = |opening: &str| -> String {
            section
                .lines()
                .find(|line| line.starts_with(opening))
                .unwrap_or_else(|| panic!("the section states `{opening}`"))
                .to_owned()
        };
        let command = crate::cli::Cli::command();
        let offered = |path: &[&str]| -> BTreeSet<String> {
            let mut verb = command.clone();
            for name in path {
                verb = verb
                    .find_subcommand(name)
                    .unwrap_or_else(|| panic!("the binary offers `{name}`"))
                    .clone();
            }
            verb.get_arguments()
                .filter_map(|arg| arg.get_long().map(str::to_owned))
                .collect()
        };
        assert_eq!(
            flags_of(&synopsis("onepipeline flow run ")),
            offered(&["flow", "run"])
        );
        // A run's own `--all` is what `--flow` refuses beside it.
        let mut watch = offered(&["watch"]);
        watch.remove("all");
        assert_eq!(flags_of(&synopsis("onepipeline watch --flow ")), watch);
        // And each landing verb's synopsis names exactly its own two flags beside
        // the sibling's, which the verb forwards unread.
        for (verb, own) in [
            ("publish-branch", vec!["pr-author-graph", "no-draft"]),
            ("repo-recover", vec!["pr-author-graph", "no-draft"]),
            ("integrate", vec!["push"]),
        ] {
            let stated = flags_of(&synopsis(&format!("onepipeline {verb} ")));
            let expected: BTreeSet<String> = own
                .into_iter()
                .chain((verb != "integrate").then_some("repo"))
                .map(str::to_owned)
                .collect();
            assert_eq!(stated, expected, "`{verb}`'s synopsis");
        }

        let progress = Ending::LandingProgress(Box::new(Progress {
            landing: "landing-1".into(),
            event: serde_json::from_value(json!({
                "v": 2, "ts": "2026-10-13T00:00:00.000Z", "stream": "s", "seq": 3,
                "source": "pipeline", "kind": "landing-finished",
                "labels": {"landing_id": "landing-1", "flow_id": "plan",
                           "operation_id": "o-1", "significance": "milestone"},
                "payload": {"landing_id": "landing-1", "flow_id": "plan",
                            "operation": "integrate", "message": "m", "status": 0,
                            "subjects": [{"identity": "/r", "branch": "a"}]},
            }))
            .expect("an envelope"),
            subjects: Vec::new(),
        }));
        let endings = [
            progress,
            Ending::SurfaceOnFlow,
            Ending::NodeSettled {
                run: "r".into(),
                node: "n".into(),
            },
            Ending::RunJoined("r".into()),
            Ending::Ended,
            Ending::Failed(std::num::NonZeroI32::new(3).expect("non-zero")),
            Ending::Elapsed,
        ];
        let stated: BTreeSet<(String, String)> = section
            .lines()
            .filter_map(|row| {
                let cells: Vec<&str> = row.split('|').map(str::trim).collect();
                let code = cells.get(1)?.strip_prefix('`')?.strip_suffix('`')?;
                let word = cells.get(2)?.strip_prefix('`')?.strip_suffix('`')?;
                code.parse::<i32>()
                    .ok()
                    .map(|_| (code.to_owned(), word.to_owned()))
            })
            .collect();
        let built: BTreeSet<(String, String)> = endings
            .iter()
            .map(|ending| (ending.exit_code().to_string(), ending.as_str().to_owned()))
            .collect();
        assert_eq!(stated, built, "the exit table is not this build's endings");

        // The return record as the section spells it: the keys every return
        // carries, as one object in one span, and the ones a condition adds,
        // named after it. Each emitted record is held to both, both ways.
        let (_, after) = section
            .split_once("`{\"watch\":\"return\"")
            .expect("the section states the return record");
        let (span, after) = after.split_once('`').expect("the record is one span");
        let spelled: serde_json::Value =
            serde_json::from_str(&format!("{{\"watch\":\"return\"{span}").replace('…', "0"))
                .expect("the documented record is an object once its values are filled in");
        let always: BTreeSet<String> = spelled
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect();
        let optional: BTreeSet<String> = after
            .split_once("with ")
            .and_then(|(_, rest)| rest.split_once(" where"))
            .expect("the section names the keys a condition adds")
            .0
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect();
        let mut added: BTreeSet<String> = BTreeSet::new();
        for ending in endings.iter().chain([&Ending::Died]) {
            let lines = ending_lines("plan", ending, "c", 0).expect("lines");
            let record: serde_json::Value = serde_json::from_str(&lines.machine).expect("JSON");
            let keys: BTreeSet<String> = record
                .as_object()
                .expect("an object")
                .keys()
                .cloned()
                .collect();
            assert!(
                keys.is_superset(&always),
                "{ending:?} returns without a key the section says every return carries: {record}"
            );
            let extra: BTreeSet<String> = keys.difference(&always).cloned().collect();
            assert!(
                extra.is_subset(&optional),
                "{ending:?} returns a key the section does not name: {record}"
            );
            added.extend(extra);
            assert_eq!(
                record["unread"]
                    .as_object()
                    .map(|unread| unread.keys().cloned().collect::<BTreeSet<_>>()),
                spelled["unread"]
                    .as_object()
                    .map(|unread| unread.keys().cloned().collect::<BTreeSet<_>>()),
                "{record}"
            );
        }
        assert_eq!(
            added, optional,
            "the section names a key no return carries, or misses one that does"
        );

        for stated in [
            format!(
                "**wake reserve of {} seconds**",
                crate::cli::WAKE_RESERVE_SECONDS
            ),
            format!("**{} milliseconds**", crate::cli::DEADLINE_CHECK_MILLIS),
            format!("`{}`", crate::cli::FLOW_ENV),
        ] {
            assert!(
                section.contains(&stated),
                "the section does not state {stated}"
            );
        }
    }

    /// A run watch's conditions are refused on a flow watch, naming the three it
    /// takes.
    #[test]
    fn a_run_watchs_condition_is_refused_on_a_flow() {
        let refused = Selected::resolve(&[WatchUntil::Settled]).expect_err("a run's condition");
        assert!(refused.to_string().contains("run-joined"), "{refused}");
        assert_eq!(
            Selected::resolve(&DEFAULT_UNTIL).expect("the default set"),
            Selected {
                surface: true,
                node_settled: true,
                run_joined: true,
                landing_progress: true,
            }
        );
    }
}
