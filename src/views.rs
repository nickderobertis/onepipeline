//! What the read-only views report.
//!
//! The views are the CLI's `runs`, `status`, `host`, `monitor`, `results`,
//! `goals`, `transcript`, and `telemetry`. The one distinction that has cost
//! real supervision time is named here as a type rather than left to prose:
//! whether a run is being driven, and if not, how it stopped being driven.
//!
//! The second is not a type but a rule: **a view never reports an unmeasured
//! thing as a measured nothing.** A dispatch that has named no tool says so; a
//! report this host cannot read says so; a bucket nothing produces is absent.
//!
//! Everything here **reads**. A view opens a run's ledger and its merged event
//! store, probes the recorded driver, and renders — and writes nothing back, so
//! rendering a run never counts as supervising it. Consuming a surface is the
//! channel's job, not a view's.

// llmlint: ignore-block[names_match_behavior] `Parked` reads as a deliberate idle, and
// for a *node* it is one — but this is the *run* liveness verdict, and `PARKED` is the
// word `docs/contract.md` fixes for it ("DRIVER DEAD vs PARKED vs UNDRIVEN"). Renaming it
// would make this crate's views disagree with the contract and with the operators who
// already read that word off them; the collision is exactly what the doc comment below
// exists to disarm. Raise it with the planner who owns the contract, not here.

/// Whether a run is being driven, and if not, why not.
///
/// The three are deliberately distinct. A run whose *driver* died is not lost —
/// its ledger is intact and `onepipeline adopt` attaches a fresh driver to it —
/// while a parked node is a planner's own deliberate idle and nothing to
/// intervene in. Reading one as the other is what this distinction exists to
/// prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DriverLiveness {
    /// A driver holds the run and this host has observed it working.
    Driving,
    /// This host has proved the recorded driver process is gone. Nothing is
    /// driving the run; `adopt` is the way back.
    DriverDead,
    /// The launch still holds its recorded pid, but nothing is happening — no
    /// child process, no surface, no ledger write. Alive and not working, so
    /// treat it as stopped and intervene.
    Parked,
    /// A *node* the ledger records as started that nothing is driving.
    /// Deliberately not [`Parked`](Self::Parked), which is the state a planner's
    /// own `cancel` produces.
    Undriven,
}
// llmlint: ignore-end[names_match_behavior]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::event::{Envelope, Source};
use crate::filter::EventFilter;
use crate::graph::{self, Landing, NodeStatus};
use crate::journal::PipelineKind;
use crate::ledger::{self, LaunchRecord};
use crate::projection::{self, MemberLabel, Refusal, RunState, Served};
use crate::report::{ToolText, Truncation};
use crate::sys;

/// A run root a view refused, and the reason it gave.
///
/// Re-exported where the views are, because a rejection is part of what a view
/// reports: a root that was skipped is named on the same output a run is.
pub use crate::ledger::Skipped;

/// Where one run's durable state lives.
///
/// Re-exported where the views are, because it is the type a consumer already
/// receives on [`RunView::paths`] and could not name — and because
/// [`report_for`](RunPaths::report_for) is how a reader of this run's store
/// resolves the copy [`report::retain`](crate::report::retain) wrote. What the
/// contract promises of it is `run`, `dir`, [`new`](RunPaths::new),
/// [`under`](RunPaths::under), [`reports_dir`](RunPaths::reports_dir), and
/// `report_for`; the segment sanitiser behind the last of those stays private,
/// so a report path is obtained by calling and never by restating.
// llmlint: ignore[invalid_states_unrepresentable] naming the type changes nothing about a run id: `run` is a `String` for the reason `src/ledger.rs`'s file-level suppression states, and `ledger::is_valid_run_id` remains the boundary every externally-supplied id crosses.
pub use crate::ledger::RunPaths;

/// How long a launch may hold its pid without doing anything before it is
/// reported [`Parked`](DriverLiveness::Parked).
///
/// The default planner-update interval: a run that has not written, surfaced,
/// or dispatched for a whole interval is not merely between turns.
pub const DEFAULT_PARKED_AFTER_SECONDS: u64 = 1_800;

/// The environment variable that moves that threshold.
pub const PARKED_AFTER_ENV: &str = "ONEPIPELINE_PARKED_AFTER_SECONDS";

/// How a node whose dispatch a `stop` ended is reported.
///
/// One phrasing, in the two views that report an in-flight node, because they
/// are read together and a run that says two things about one node is a run
/// nobody trusts. It says what happened to the *worker* rather than what the
/// node produced: a stop ends the run's whole dispatch tree, and the process
/// that would have settled the node was in that tree — so the last thing the
/// record holds for it is that it started, and a reader with nothing else to go
/// on takes that for a node that produced nothing.
const ENDED_BY_THE_STOP: &str = "worker ended when the run was stopped";

/// How the same node reads when nothing established what became of its worker.
///
/// It has to be a different sentence: the worker is very likely still running,
/// and "ended" there is the false completion `stop` itself refuses to report.
const OUTLIVED_THE_STOP: &str = "worker may still be running: the stop could not reach it";

/// Which of the two a stopped run's in-flight node gets.
fn became_of_the_worker(state: &crate::projection::RunState) -> &'static str {
    match state.stop {
        crate::projection::StopState::WorkersUndetermined => OUTLIVED_THE_STOP,
        _ => ENDED_BY_THE_STOP,
    }
}

/// Whether the run's **observer** graph is still watching, and if not, why not.
///
/// Beside [`DriverLiveness`] rather than inside it: the two are about different
/// processes and a run can be in any pairing of them, and a live driver
/// executing unwatched is the state this exists to report. Private, because
/// `docs/contract.md` names the driver tier and this is a rendering beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ObserverLiveness {
    /// The launch named an observer graph and nothing says its run has ended.
    Watching,
    /// The launch named one and this host can prove that graph run is over. The
    /// run is executing with nothing watching it.
    ObserverDead,
    /// The launch named no observer graph at all — the shipped default, since no
    /// agent is required to execute a plan. Nothing is watching this run either,
    /// and nothing ever was: a different fact, and a different fix.
    Unobserved,
}

impl ObserverLiveness {
    /// The word a view prints beside the driver tier, or nothing when the
    /// observer is doing its job.
    fn as_str(self) -> &'static str {
        match self {
            Self::Watching => "",
            Self::ObserverDead => "OBSERVER DEAD",
            Self::Unobserved => "NO OBSERVER",
        }
    }
}

/// Whether anything is watching this run.
///
/// The launch record answers the first half — whether there is an observer at
/// all — and names the graph run whose own liveness is the second, which
/// [`agentgraph::graph_run_ended`] decides and documents.
///
/// [`agentgraph::graph_run_ended`]: crate::agentgraph::graph_run_ended
fn observer_liveness(launch: &LaunchRecord) -> ObserverLiveness {
    if launch.observer_graph().is_none() {
        return ObserverLiveness::Unobserved;
    }
    if crate::agentgraph::graph_run_ended(&launch.graph_run, &launch.run_id) {
        return ObserverLiveness::ObserverDead;
    }
    ObserverLiveness::Watching
}

impl DriverLiveness {
    /// The word a view prints for this verdict.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Driving => "ACTIVE",
            Self::DriverDead => "DRIVER DEAD",
            Self::Parked => "PARKED",
            Self::Undriven => "UNDRIVEN",
        }
    }

    /// Whether this verdict means nothing is driving the run.
    ///
    /// `adopt` is the way back from both of the two that do.
    pub fn is_undriven(self) -> bool {
        matches!(self, Self::DriverDead | Self::Parked)
    }
}

/// How long a launch may be silent before it is reported parked.
pub fn parked_after_seconds() -> u64 {
    std::env::var(PARKED_AFTER_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_PARKED_AFTER_SECONDS)
}

/// Whether a **decision point** is outstanding, in either of the two forms one
/// takes: a ready human action nobody has attested, or a blocking surface nobody
/// has answered.
///
/// The one question every verdict about a stalled run asks, so the settlement and
/// the liveness verdict cannot disagree about the same run — a run reported
/// `PARKED` invites an `adopt` that may end its driver, and doing that to a run
/// whose next move is already sitting in a planner's queue costs the work it
/// holds for nothing.
pub fn decision_outstanding(state: &RunState, paths: &RunPaths) -> bool {
    state.awaiting_human_action() || blocking_surface(paths)
}

/// Whether a blocking surface is outstanding, read or not.
///
/// Unread counts. A question nobody has looked at is still a question the run is
/// waiting on, and a verdict that ignored it would report the run as abandoned —
/// sending an operator to intervene in a run whose next move is already sitting
/// in their own queue.
fn blocking_surface(paths: &RunPaths) -> bool {
    let queue = crate::channel::ChannelState::new(paths).queue();
    queue
        .waiting
        .iter()
        .chain(queue.pending.iter())
        .any(|surface| surface.blocking)
}

/// Whether a run is being driven, and if not, why not.
///
/// Every unreadable input resolves toward "still working", so a busy driver is
/// never misreported: one live process, one fresh surface, or one recent ledger
/// write is enough to keep it reported as running. A pid recorded on another
/// host is exactly such an unknown — a pid means nothing across machines — so a
/// run another driver is holding reads as the live work it is.
pub fn liveness(launch: &LaunchRecord, state: &RunState, paths: &RunPaths) -> DriverLiveness {
    if state.stop_recorded() {
        return DriverLiveness::DriverDead;
    }
    let ours = launch.host == sys::hostname();
    if ours && !sys::process_may_be_live(launch.pid) {
        return DriverLiveness::DriverDead;
    }
    // A live pid is ownership, not progress.
    let quiet_for = state
        .last_write_at
        .map(|last| sys::now_millis().saturating_sub(last) / 1_000);
    match quiet_for {
        // A run holding an outstanding decision point is *waiting*, not parked:
        // the loop that would be writing is deliberately holding a subtree back
        // until a person answers, and a driver reported dead there sends an
        // operator to intervene in work that is doing exactly what it should.
        Some(seconds)
            if seconds > parked_after_seconds() && !decision_outstanding(state, paths) =>
        {
            DriverLiveness::Parked
        }
        _ => DriverLiveness::Driving,
    }
}

/// Take the landings the run's **own settled report** recorded, where they say
/// more than its journal does.
///
/// A driver re-reads every unlanded change as it closes out — `engine`'s
/// `landings_after_asking_again` — and writes what it found there; without this
/// the report a consumer parses said `landed` while the view an operator opens
/// said `NOT landed`.
///
/// **Only ever `unlanded` → `landed`.** A report written at an earlier close-out
/// is older than the journal beside it, and a run still going has one — but a
/// base does not stop carrying what it carries, so taking the later answer in
/// that direction alone cannot un-land anything.
fn landings_the_run_re_read(state: &mut RunState, paths: &RunPaths) {
    // Read leniently: a report this build cannot parse — one a newer build
    // wrote, at a version this one refuses — leaves the view exactly as the
    // journal left it, which is the answer it always had.
    let Some(result) = ledger::read_json_opt::<crate::engine::RunResult>(&paths.result()) else {
        return;
    };
    for node in result.nodes {
        if node.landing == Some(Landing::Landed)
            && state.landings.get(&node.id) == Some(&Landing::Unlanded)
        {
            state.landings.insert(node.id, Landing::Landed);
        }
    }
}

/// Everything a view needs about one run, read once.
#[derive(Debug)]
pub struct RunView {
    /// Where the run's state lives.
    pub paths: RunPaths,
    /// Who launched it, and with what.
    pub launch: LaunchRecord,
    /// Its merged event store, in merge order.
    pub events: Vec<Envelope>,
    /// What the journal says about it.
    pub state: RunState,
}

impl RunView {
    /// Read one run, or report why it cannot be read.
    pub fn open(paths: &RunPaths) -> crate::Result<Self> {
        if !paths.exists() {
            return Err(crate::Error::NoSuchRun {
                run: paths.run.clone(),
                root: paths.dir.parent().unwrap_or(Path::new(".")).to_path_buf(),
            });
        }
        let launch: LaunchRecord = ledger::read_json(&paths.launch())?;
        let mut events = crate::journal::read(&paths.journal());
        crate::journal::merge_order(&mut events);
        let mut state = projection::fold(&events);
        landings_the_run_re_read(&mut state, paths);
        // A view resolves cross-DAG edges the same way the loop does, so a
        // consumer this run is about to dispatch is not reported blocked to the
        // person deciding whether to intervene. Reading only: rendering a run
        // records nothing about it.
        state.cross_dag = crate::crossdag::resolve_quietly(
            &paths
                .dir
                .parent()
                .map_or_else(ledger::runs_root, Path::to_path_buf),
            &state.graph,
        );
        Ok(Self {
            paths: paths.clone(),
            launch,
            events,
            state,
        })
    }

    /// How the run is being driven.
    pub fn liveness(&self) -> DriverLiveness {
        liveness(&self.launch, &self.state, &self.paths)
    }

    /// The surfaces nobody has read yet, and how stale the oldest is.
    ///
    /// These are the state a planner who never attached is blind to: the row
    /// above says only `ACTIVE`, and the delivery record they would look for is
    /// written on consumption, which has not happened.
    pub fn unread_surfaces(&self) -> (usize, Option<u64>) {
        let unread = self.unread();
        (unread.count, unread.oldest_seconds)
    }

    /// The same read, with what the queue is holding as well as how much.
    ///
    /// Crate-visible and behind the pair above: the count and the staleness are
    /// what the contract names, and which kinds are waiting is a rendering — see
    /// [`Unread`] for why the line carries it.
    fn unread(&self) -> Unread {
        Unread::of(&crate::channel::ChannelState::new(&self.paths).queue())
    }

    /// A one-line summary of where the run has got to.
    ///
    /// `n/n done` is the line a planner reads to decide a run is finished, and
    /// on its own it is the false completion this crate exists to stop
    /// reporting: every node can be done while every change is sitting in a pull
    /// request nobody merged. So the count of what has not landed rides the same
    /// line, and is absent — rather than a zero — when there is nothing to say.
    ///
    /// Dated, for the reason the per-node phrase is: it counts what each
    /// settlement observed, plus whatever the run's own close-out re-read
    /// afterwards — and a count that read as the state of things now would say a
    /// merged change had reached nobody. Divergence 33 in
    /// [the divergence record](../../../docs/contract-divergences.md) records
    /// what that re-read still cannot reach.
    pub fn summary(&self) -> String {
        let statuses = self.state.statuses();
        let done = statuses
            .values()
            .filter(|status| **status == NodeStatus::Done)
            .count();
        let unlanded = match unlanded_nodes(self).len() {
            0 => String::new(),
            count => format!(", {count} not landed as of settlement"),
        };
        // What is *missing* from the count above splits two ways, and only one
        // of them is work that was attempted: `n/n done` on its own left a
        // reader unable to tell a node the run tried and lost from one it never
        // asked at all. Absent rather than a zero, like the clause before it.
        let skipped = match statuses
            .values()
            .filter(|status| **status == NodeStatus::Skipped)
            .count()
        {
            0 => String::new(),
            count => format!(", {count} never attempted"),
        };
        format!("{done}/{} done{unlanded}{skipped}", statuses.len())
    }
}

/// What one run's unread surfaces are, as the one line reporting them needs them.
///
/// A blocking surface produces no other signal, and on a host holding thousands
/// of routine `monitor` updates against a handful of questions a bare count read
/// the same either way.
#[derive(Debug, Default)]
struct Unread {
    count: usize,
    /// Absent when nothing is waiting, rather than a zero that reads as a queue
    /// somebody has just emptied.
    oldest_seconds: Option<u64>,
    /// Blocking kinds first — that is what the run is held on — then rarest
    /// first, since a rare kind behind a common one is the burial this repairs;
    /// then by name, so the line is stable to read.
    kinds: Vec<(String, usize)>,
}

/// How many kinds a line names before it summarises the rest.
///
/// The remainder is counted out loud rather than dropped: a silently truncated
/// list reads as the whole answer.
const MAX_NAMED_KINDS: usize = 4;

impl Unread {
    fn of(queue: &crate::channel::Queue) -> Self {
        let mut counts: BTreeMap<String, (bool, usize)> = BTreeMap::new();
        for surface in &queue.waiting {
            // The kind is a stranger's: an observer's frame names it in that
            // persona's own vocabulary, so it goes through the same strip every
            // other borrowed value on these views does.
            let seen = counts.entry(one_line(&surface.kind)).or_insert((false, 0));
            seen.0 |= surface.blocking;
            seen.1 += 1;
        }
        let mut ordered: Vec<(bool, usize, String)> = counts
            .into_iter()
            .map(|(kind, (blocking, count))| (blocking, count, kind))
            .collect();
        ordered.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        Self {
            count: queue.waiting.len(),
            oldest_seconds: queue
                .waiting
                .iter()
                .map(|surface| sys::now_millis().saturating_sub(surface.queued_at) / 1_000)
                .max(),
            kinds: ordered
                .into_iter()
                .map(|(_, count, kind)| (kind, count))
                .collect(),
        }
    }

    /// The kinds, bounded: past [`MAX_NAMED_KINDS`] the line says how many it
    /// left out rather than ending where a reader cannot tell it was cut.
    fn phrase(&self) -> String {
        let named: Vec<String> = self
            .kinds
            .iter()
            .take(MAX_NAMED_KINDS)
            .map(|(kind, count)| format!("{count} {kind}"))
            .collect();
        let rest = match self.kinds.len().saturating_sub(named.len()) {
            0 => String::new(),
            more => format!(", and {more} other kind(s)"),
        };
        format!("{}{rest}", named.join(", "))
    }
}

/// What a view over a whole runs root read, and what it refused.
///
/// The second half is why this type exists. A view that opened every run it
/// could and silently dropped the rest reported a rejection as an *absence*: a
/// host with thirty run roots on it rendered as `no runs recorded`, which a
/// planner reads as "nothing is running". A refused root is a fact about the
/// root, and it is carried to the renderer rather than thrown away in the read.
#[derive(Debug)]
pub struct Survey {
    /// The runs root this survey read. Named on the output, because it is the
    /// scope of every claim made from it.
    pub root: PathBuf,
    /// The runs that read, oldest id first.
    pub views: Vec<RunView>,
    /// The run roots that did not, each with the reason it was refused.
    pub skipped: Vec<Skipped>,
}

impl Survey {
    /// Read every run under a root, keeping what could not be read.
    ///
    /// A root the ledger refused and a run whose launch record this build cannot
    /// accept are the same fact to a reader — one directory that claimed to be a
    /// run and is not being reported as one — so they arrive on one list.
    pub fn of(root: &Path) -> Self {
        let index = ledger::all_runs(root);
        let mut views = Vec::new();
        let mut skipped = index.skipped;
        for paths in index.runs {
            match RunView::open(&paths) {
                Ok(view) => views.push(view),
                // The refusal as `results` already words it: the file, the
                // offending field, and what was expected. Nothing is added to it
                // here — a second wording of one refusal is a second thing to
                // keep true.
                Err(error) => skipped.push(Skipped {
                    path: paths.dir,
                    reason: error.to_string(),
                }),
            }
        }
        skipped.sort_by(|a, b| a.path.cmp(&b.path));
        Self {
            root: root.to_path_buf(),
            views,
            skipped,
        }
    }

    /// The one run a caller named, surveyed on its own.
    ///
    /// It was opened by name, so a root it did not read is not this survey's to
    /// report: a caller who named a run that could not be read was refused
    /// outright rather than handed a view of it.
    pub fn of_one(view: RunView) -> Self {
        let root = view
            .paths
            .dir
            .parent()
            .map_or_else(ledger::runs_root, Path::to_path_buf);
        Self {
            root,
            views: vec![view],
            skipped: Vec::new(),
        }
    }
}

// llmlint: ignore-block[cli_output_contract] a refused run root is part of the answer, not
// a failure of the command: the empty case here *replaces* `no runs recorded`, so it cannot
// live on a stream other than the answer it replaces. The two driver sites that print these
// carry the exit code that goes with the same decision.
/// How many refused run roots a view names before it summarises the rest.
///
/// The remainder is counted out loud rather than dropped, exactly as
/// [`MAX_NAMED_KINDS`] counts the surface kinds it did not name: a silently
/// truncated list reads as the whole answer.
const MAX_NAMED_SKIPS: usize = 3;

/// What a view says about the run roots it refused, or nothing when it refused
/// none.
///
/// **One line: a count, and as many reasons as fit on it.** A count alone tells
/// a reader something is wrong and not which directory to look at, so a few
/// roots are named with the reason `results` prints for the same refusal — and a
/// line each is what buried the answer, because a third of the roots on a host
/// can be unreadable and the live verdict is what a reader opened the view
/// for.
fn skipped_lines(skipped: &[Skipped]) -> String {
    if skipped.is_empty() {
        return String::new();
    }
    // Every value on the line is a stranger's — a directory name on disk and a
    // refusal built from it — so both go through the same strip.
    let named: Vec<String> = skipped
        .iter()
        .take(MAX_NAMED_SKIPS)
        .map(|root| {
            format!(
                "{}: {}",
                one_line(&root.path.display().to_string()),
                one_line(&root.reason)
            )
        })
        .collect();
    let rest = match skipped.len().saturating_sub(named.len()) {
        0 => String::new(),
        more => format!(", and {more} more"),
    };
    format!(
        "{} run root(s) skipped: {}{rest}\n",
        skipped.len(),
        named.join("; ")
    )
}

/// What a whole-root view says when it has no run to report.
///
/// Two different facts, which is the whole reason this is a function: a root
/// with nothing in it and a root whose every run was refused both rendered as
/// `no runs recorded`, and only one of them means there is nothing running.
fn nothing_to_report(survey: &Survey) -> String {
    let mut out = if survey.views.is_empty() && !survey.skipped.is_empty() {
        format!(
            "no run under {} could be read\n",
            one_line(&survey.root.display().to_string())
        )
    } else {
        "no runs recorded\n".to_string()
    };
    out.push_str(&skipped_lines(&survey.skipped));
    out
}
// llmlint: ignore-end[cli_output_contract]

/// Where a run stands: what is left for a driver to do, and whether anything is
/// doing it.
///
/// One reading behind both halves of what a view says about a run — the word on
/// its row and the advice beneath it — so no combination of graph state and
/// driver state can make the two disagree. Read apart, they did: a run whose
/// graph had completed printed `SETTLED` and, directly under it, an invitation
/// to attach a fresh driver to finished work.
struct Standing {
    /// How the run is being driven.
    liveness: DriverLiveness,
    /// What remains for a driver or planner to do.
    work: WorkStanding,
    /// Whether the loop has anything left to converge.
    ///
    /// Carried beside [`work`](Self::work) rather than read off it, because the
    /// two answer different questions and one of the readings is not a function
    /// of the other: a converged run holding a ready human action is
    /// [`WorkStanding::Outstanding`] — an `attest` moves it — and it has still
    /// settled, its driver having written its result and gone.
    convergence: Convergence,
}

/// Whether every node of a run's graph reached a state the loop is finished
/// with, so no further pass is coming.
///
/// A named verdict rather than a bare flag, because it is what
/// [`has_settled`] answers and what the channel refuses a reply on: read as the
/// wrong polarity it queues a verdict where nothing drains it, or turns away the
/// one reply a live run was waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Convergence {
    /// Nothing is left to converge: the run has settled.
    Settled,
    /// Something can still move, with or without a driver attached to move it.
    Moving,
}

impl Convergence {
    /// The verdict for a graph whose nodes have all settled, or not.
    fn of(converged: bool) -> Self {
        if converged {
            Self::Settled
        } else {
            Self::Moving
        }
    }
}

/// One or more nodes an operator has to act on before a driver can move the run.
///
/// The invariant is the advice itself: a standing that says work is held names
/// the work it means, and a list that could be empty would put a prescription
/// naming nothing on an operator's screen.
struct HeldNodes(Vec<String>);

impl HeldNodes {
    /// The nodes, or `None` where there are none — and so no standing to report.
    fn of(nodes: Vec<String>) -> Option<Self> {
        (!nodes.is_empty()).then_some(Self(nodes))
    }

    fn named(&self) -> String {
        self.0.join(", ")
    }
}

/// The readings of the graph behind a run's word and advice.
enum WorkStanding {
    /// Every node completed successfully.
    Complete,
    /// The graph converged without completing and has no work an operator can
    /// return to the frontier, as for a failed run.
    Settled,
    /// Something is ready, running, waiting, or blocked and a fresh driver can
    /// pick it up now or when its gate opens.
    Outstanding,
    /// Work no driver moves on its own, which is what the run is told about.
    Held(HeldWork),
}

/// The work a converged run holds that no driver moves on its own.
///
/// Three inhabited cases and no fourth. A run can hold both kinds at once and
/// they are different facts about it — parked work returns to the frontier on a
/// `requeue`, and a node its judge rejected does not return on anything a driver
/// does — so both are carried, and [`ranked`](Self::ranked) decides which is
/// said rather than the reading throwing one away to decide.
enum HeldWork {
    /// Nodes a `requeue` returns to the frontier.
    Parked(HeldNodes),
    /// Nodes a judge rejected, which nothing dispatches as they stand.
    Rejected(HeldNodes),
    /// Both, on the one run.
    Both {
        /// The parked half, which is the half that has an answer a driver acts on.
        parked: HeldNodes,
        /// The judged half, still true of the run and still unanswered by that.
        rejected: HeldNodes,
    },
}

impl HeldWork {
    /// What a converged run holds, or `None` where it holds neither kind — which
    /// is also the run that has no held standing to report.
    fn of(parked: Vec<String>, rejected: Vec<String>) -> Option<Self> {
        match (HeldNodes::of(parked), HeldNodes::of(rejected)) {
            (Some(parked), Some(rejected)) => Some(Self::Both { parked, rejected }),
            (Some(parked), None) => Some(Self::Parked(parked)),
            (None, Some(rejected)) => Some(Self::Rejected(rejected)),
            (None, None) => None,
        }
    }

    /// Every prescription this held work carries, most urgent first.
    ///
    /// The order is the ranking and the head is the answer: a `requeue` is the
    /// one of the two an operator can act on now — it returns work to the
    /// frontier and a driver dispatches it — so a run holding both is told to
    /// requeue, and meets the judgement on the read after that work has moved.
    /// One run is given one answer, because [`Standing::intervention`] takes the
    /// head; what the ranking does not do is decide by discarding the other
    /// half.
    fn ranked(&self) -> impl Iterator<Item = Intervention<'_>> {
        let (parked, rejected) = match self {
            Self::Parked(parked) => (Some(parked), None),
            Self::Rejected(rejected) => (None, Some(rejected)),
            Self::Both { parked, rejected } => (Some(parked), Some(rejected)),
        };
        parked
            .map(Intervention::RequeueThenAdopt)
            .into_iter()
            .chain(rejected.map(Intervention::ReviewThenSupersede))
    }
}

/// The nodes of a converged run whose work a judge rejected.
///
/// **Two records, because either alone names the wrong nodes.** The outcome says
/// the dispatch ended on the *task's* own verdict rather than on the machinery
/// or on a publication, which is the settlement a judgement produces; the failed
/// verdict is the judgement itself, and without one a node that simply failed
/// its task would be reported as judged by a judge that never scored it. Both
/// are read off records these views already hold — the run's folded outcomes and
/// the verdicts `oneagentgraph` copies onto the settlement — so nothing is
/// opened to answer the question.
fn rejected_by_a_judge(view: &RunView, statuses: &BTreeMap<String, NodeStatus>) -> Vec<String> {
    statuses
        .iter()
        .filter(|(_, status)| **status == NodeStatus::Failed)
        .filter(|(id, _)| {
            matches!(
                view.state.outcomes.get(*id).map(String::as_str),
                Some(crate::engine::TASK_FAILED | crate::engine::TASK_FAILED_CHANGE_OPEN)
            )
        })
        .filter(|(id, _)| !crate::report::failed_verdicts(&view.events, id).is_empty())
        .map(|(id, _)| id.clone())
        .collect()
}

impl Standing {
    /// Read one run's standing, once.
    fn of(view: &RunView) -> Self {
        let statuses = view.state.statuses();
        // An empty status map is a run whose graph nothing has read, not a run
        // with nothing left to do: `is_terminal` and `state_of` both answer an
        // empty map with the settled reading, and taking it would report a run
        // that has recorded nothing as finished.
        let converged = !statuses.is_empty() && graph::is_terminal(&statuses);
        let parked: Vec<_> = if converged {
            statuses
                .iter()
                .filter(|(_, status)| **status == NodeStatus::Parked)
                .map(|(id, _)| id.clone())
                .collect()
        } else {
            Vec::new()
        };
        // Read for a converged run alone, and for the same reason the park above
        // is: a run with a node still ready, running, or pending has work a fresh
        // driver moves, whatever else a judge rejected, and `adopt` is the whole
        // of what it needs. This reading is for the frontier that cannot move.
        let rejected = if converged {
            rejected_by_a_judge(view, &statuses)
        } else {
            Vec::new()
        };
        let work = if converged && graph::state_of(&statuses) == graph::GraphState::Complete {
            WorkStanding::Complete
        } else if let Some(held) = HeldWork::of(parked, rejected) {
            WorkStanding::Held(held)
        } else if !converged
            || statuses
                .values()
                .any(|status| matches!(status, NodeStatus::Waiting | NodeStatus::Blocked))
        {
            WorkStanding::Outstanding
        } else {
            WorkStanding::Settled
        };
        Self {
            liveness: view.liveness(),
            work,
            convergence: Convergence::of(converged),
        }
    }

    /// The word a view prints for how the run is being driven.
    fn word(&self) -> &'static str {
        if matches!(&self.work, WorkStanding::Complete) {
            "SETTLED"
        } else {
            self.liveness.as_str()
        }
    }

    /// What this run needs before it can move again, where it needs anything.
    ///
    /// `None` is the third answer and the one the advice used not to have: a run
    /// whose work is over needs nothing, and a driver attached to it settles it
    /// again in no time at all having dispatched nothing.
    fn intervention(&self) -> Option<Intervention<'_>> {
        if !self.liveness.is_undriven() {
            return None;
        }
        match &self.work {
            WorkStanding::Outstanding => Some(Intervention::Adopt),
            // The head of the ranking, which held work always has: `HeldWork::of`
            // answers `None` rather than build one holding nothing.
            WorkStanding::Held(held) => held.ranked().next(),
            WorkStanding::Complete | WorkStanding::Settled => None,
        }
    }
}

/// What a run nothing is driving needs before it can move again.
enum Intervention<'a> {
    /// A fresh driver, and nothing else: work is waiting on the frontier for it.
    Adopt,
    /// The parked work returned to the frontier, and *then* a driver.
    RequeueThenAdopt(&'a HeldNodes),
    /// The judge's verdict read, and the node it rejected superseded. A driver
    /// is not the first step here and on its own is not a step at all.
    ReviewThenSupersede(&'a HeldNodes),
}

/// The prescription for a run nothing is driving whose unfinished work is
/// parked, phrased once for both views that give it.
///
/// The order is the whole content. A parked node is held out of every reconcile
/// pass until a `requeue`, so a driver adopted first derives an empty frontier
/// and returns at exit 0 having dispatched nothing — which is what an advice
/// line naming only `adopt` cost the operator who followed it, twice.
fn requeue_then_adopt(run: &str, parked: &HeldNodes) -> String {
    format!(
        "its unfinished work is parked, and no driver dispatches a parked node: return {} \
         to the frontier with a `requeue` on: onepipeline reply {run} — and only then \
         attach a fresh driver with: onepipeline adopt {run}",
        parked.named()
    )
}

/// The prescription for a run nothing is driving whose unfinished work is held
/// up by nodes a judge rejected, phrased once for both views that give it.
///
/// **The judgement is the content.** A rejection is deliberately outside the
/// publication attempt budget — asking again repeats the same work against the
/// same bar — so nothing dispatches the node as it stands, and a driver attached
/// here settles having moved nothing, which is what the `adopt` this replaces
/// cost an operator twice. What moves the run is the verdict, and then the
/// planner's own `amend` and `retry`.
fn review_then_supersede(run: &str, rejected: &HeldNodes) -> String {
    format!(
        "its unfinished work is held up by {}, whose work a judge rejected, and no driver \
         dispatches a rejected node as it stands: read the verdict with: onepipeline \
         results {run} — and decide from it, most likely amending the task and superseding \
         the node with an `amend` and a `retry` on: onepipeline reply {run}",
        rejected.named()
    )
}

/// The word a view prints for how a run is being driven.
///
/// A run whose graph completed is **settled**, not abandoned: its driver is
/// gone because there was nothing left for it to do. Reporting `DRIVER DEAD`
/// there would send a planner to intervene in finished work.
pub fn liveness_word(view: &RunView) -> &'static str {
    Standing::of(view).word()
}

/// Whether a run has **settled**: every node of its graph reached a state the
/// loop is finished with, so there is no pass left to read anything handed to it.
///
/// A fact about the run rather than about a process, and deliberately so.
/// [`DriverLiveness`] answers *who is driving*, which is a different question
/// with a different answer at the same instant: a driver that has written the
/// run's result and released its ownership lock is still in the process table
/// for as long as it takes to exit, and reads as `ACTIVE` throughout. Anything
/// deciding whether a run can still take work in — `reply` is the one that
/// costs, because a reply queued for a run nobody will drive again is delivered
/// to nothing — has to ask this instead, or its answer depends on the order in
/// which a driver exits.
///
/// A run whose graph nothing has recorded has **not** settled: an empty status
/// map is a run that has not started, and `Standing::of` states that reading
/// once for every reader of it.
///
/// Crate-visible: what the contract names here is what a view *renders*, and this
/// is a reading the channel's own refusal shares with them rather than a promise
/// to a consumer.
pub(crate) fn has_settled(view: &RunView) -> bool {
    Standing::of(view).convergence == Convergence::Settled
}

/// What a view prints about the graph **watching** the run, beside the word for
/// the one driving it.
///
/// Only for a run that is actually executing. A settled run needs no observer,
/// and a run nothing is driving has bigger news on the same line — reporting
/// either as unwatched would send an operator after a graph whose absence is not
/// the problem.
fn observer_word(view: &RunView, standing: &Standing) -> &'static str {
    if standing.word() != DriverLiveness::Driving.as_str() {
        return "";
    }
    observer_liveness(&view.launch).as_str()
}

fn observer_suffix(view: &RunView, standing: &Standing) -> String {
    match observer_word(view, standing) {
        "" => String::new(),
        word => format!("  {word}"),
    }
}

/// `onepipeline runs`.
///
/// A run root under this root that could not be read is named at the end rather
/// than dropped: an empty listing on a host that holds runs is the reading that
/// costs the most, because it is the one a planner acts on by starting more work.
pub fn runs(root: &Path, mine_only: bool, session: &str) -> String {
    let survey = Survey::of(root);
    let mut out = String::new();
    for view in &survey.views {
        let owned = view.launch.owned_by(session);
        if mine_only && !owned {
            continue;
        }
        let marker = if owned { '*' } else { ' ' };
        let standing = Standing::of(view);
        out.push_str(&format!(
            "{marker} {:<24} {:<24} {}  {}{}\n",
            view.paths.run,
            view.launch.owner_label(session),
            view.summary(),
            standing.word(),
            observer_suffix(view, &standing)
        ));
        // A run reported stopped keeps the line saying why it stopped rather
        // than an invitation to read updates nothing will follow up on. A run
        // that needs no intervention falls through to those updates instead:
        // its work is over, and what is left to say about it is what nobody has
        // read yet.
        if let Some(intervention) = standing.intervention() {
            out.push_str(&match intervention {
                Intervention::Adopt => format!(
                    "    {} — its ledger is intact; attach a fresh driver with: \
                     onepipeline adopt {}\n",
                    standing.word(),
                    view.paths.run
                ),
                Intervention::RequeueThenAdopt(parked) => format!(
                    "    {} — its ledger is intact; {}\n",
                    standing.word(),
                    requeue_then_adopt(&view.paths.run, parked)
                ),
                Intervention::ReviewThenSupersede(rejected) => format!(
                    "    {} — its ledger is intact; {}\n",
                    standing.word(),
                    review_then_supersede(&view.paths.run, rejected)
                ),
            });
            continue;
        }
        let unread = view.unread();
        if let (count, Some(stale)) = (unread.count, unread.oldest_seconds) {
            if count > 0 {
                out.push_str(&format!(
                    "    {count} planner update(s) waiting ({}), unread for {}; \
                     read them with: onepipeline next {}\n",
                    unread.phrase(),
                    crate::telemetry::duration(stale * 1_000),
                    view.paths.run
                ));
            }
        }
    }
    if out.is_empty() {
        return nothing_to_report(&survey);
    }
    out.push_str(&skipped_lines(&survey.skipped));
    out
}

/// `onepipeline status`.
pub fn status(survey: &Survey) -> String {
    let mut out = String::new();
    for view in &survey.views {
        let standing = Standing::of(view);
        out.push_str(&format!(
            "{}  {}{}  {}\n",
            view.paths.run,
            standing.word(),
            observer_suffix(view, &standing),
            view.summary()
        ));
        if let Some(intervention) = standing.intervention() {
            out.push_str(&match intervention {
                Intervention::Adopt => format!(
                    "  {}: nothing is driving this run; adopt it or stop it\n",
                    standing.word()
                ),
                Intervention::RequeueThenAdopt(parked) => format!(
                    "  {}: nothing is driving this run and {}\n",
                    standing.word(),
                    requeue_then_adopt(&view.paths.run, parked)
                ),
                Intervention::ReviewThenSupersede(rejected) => format!(
                    "  {}: nothing is driving this run and {}\n",
                    standing.word(),
                    review_then_supersede(&view.paths.run, rejected)
                ),
            });
        }
        if let Some(pending) = crate::channel::ChannelState::new(&view.paths).pending() {
            out.push_str(&format!(
                "  waiting for planner {}: {} — {}\n",
                if pending.blocking {
                    "decision"
                } else {
                    "reply"
                },
                pending.kind,
                pending.message
            ));
        }
        let unread = view.unread();
        if unread.count > 0 {
            out.push_str(&format!(
                "  {} planner update(s) waiting ({}), unread for {}\n",
                unread.count,
                unread.phrase(),
                crate::telemetry::duration(unread.oldest_seconds.unwrap_or(0) * 1_000)
            ));
        }
        let statuses = view.state.statuses();
        for (id, node_status) in &statuses {
            if *node_status != NodeStatus::Running {
                continue;
            }
            let age = view
                .state
                .dispatched_at
                .get(id)
                .map(|at| sys::now_millis().saturating_sub(*at));
            let age = crate::telemetry::duration(age.unwrap_or(0));
            if view.state.stop_recorded() {
                // What this node last did stays on the record and is
                // deliberately not repeated here — see [`ENDED_BY_THE_STOP`].
                let became = became_of_the_worker(&view.state);
                out.push_str(&format!("  {id}: {became}, {age} in\n"));
                continue;
            }
            out.push_str(&format!("  {id}: running for {age}"));
            match view.state.activity.get(id) {
                // A node the ledger records as running that no live dispatch is
                // driving is `UNDRIVEN`. Deliberately not `parked`: that word
                // means the opposite here — a node the planner idled with
                // `cancel`.
                None => out.push_str(&format!(" — {}", DriverLiveness::Undriven.as_str())),
                Some(activity) => out.push_str(&format!(" — {}", working(activity))),
            }
            out.push('\n');
        }
        // A node whose cancellation is still out there. Under its own word,
        // because neither of the two a reader has fits: `parked` is a planner's
        // own idle with nothing running, and `ready` is a node about to start.
        for (id, node_status) in &statuses {
            if *node_status != NodeStatus::Parked {
                continue;
            }
            let Some(pending) = cancelling_for(&view.state, id) else {
                continue;
            };
            out.push_str(&format!(
                "  {id}: cancelling — asked to stop {pending} ago and its dispatch has not \
                 settled; it still holds the node's workspace, so wait for it rather than \
                 requeueing the node\n"
            ));
        }
        // A node that is ready and has not started. On its own that reads as
        // "about to go", which is what a node waiting on an occupied workspace
        // looks like for as long as it waits — one sat `ready` for forty minutes
        // while a supervisor looked for a wedge that was not there. So the two
        // are separate lines: what it is waiting for, or that it is waiting for
        // nothing but a slot.
        for (id, node_status) in &statuses {
            if *node_status != NodeStatus::Ready {
                continue;
            }
            out.push_str(&format!(
                "  {id}: ready — {}\n",
                waiting_on(&view.state, id)
            ));
        }
        // What each amended node is currently judged against. Rendered whatever
        // that node's status is, because the reader this line is for is a
        // manager about to *replace* an amendment: `amend` replaces rather than
        // appends, so a ruling that is not readable before the replacement lands
        // is a ruling nobody can weigh against the one taking its place.
        for node in view.state.graph.iter() {
            if let Some(amendment) = &node.amendment {
                out.push_str(&format!(
                    "  {}: amended — {}\n",
                    node.id,
                    one_line(amendment)
                ));
            }
        }
        // What refused, for the nodes that failed. A failed node otherwise reads
        // the same whether its own gate failed or an identity chain ran out, and
        // the two call for opposite actions from whoever is reading this.
        //
        // Only a chain that ran out is reported under the node's failure. One
        // that fell through and was then served is reported as the recovery it
        // was, under its own word.
        for (id, node_status) in &statuses {
            if *node_status != NodeStatus::Failed {
                continue;
            }
            // A dispatch that died is not a node whose agent failed, and this
            // view is where somebody decides whether to re-run one. Said here as
            // well as in `results` for the reason the unlanded line below is:
            // deciding there is nothing left to do is a decision made from this
            // view, and a node reported as a plain failure over finished work is
            // what that decision used to be made on.
            let branch = view.state.branches.get(id).cloned().or_else(|| {
                view.state
                    .graph
                    .get(id)
                    .and_then(|node| node.branch.clone())
            });
            if let Some(died) = death_phrase(&view.state, id, branch.as_deref()) {
                out.push_str(&format!("  {id}: {died}\n"));
            }
            for record in chain_records(&view.state, id) {
                out.push_str(&format!(
                    "  {id}: {} — {}\n",
                    record.lead_in(),
                    chain_phrase(&record)
                ));
            }
        }
        // The nodes this run has deliberately stopped short of merging. Said
        // here because this is the view somebody reads when a run has gone quiet
        // and they are deciding whether it is stuck: a run holding one of these
        // is **waiting on a release**, not stalled and not finished, and nothing
        // else on this screen distinguishes the two. The line names what each is
        // waiting on, so the decision — keep waiting, or go and cut the release —
        // is made from here rather than from the store.
        let drafted = drafted_nodes(view);
        if !drafted.is_empty() {
            out.push_str(&format!(
                "  {} node(s) complete and held as a draft: the run is waiting on the \
                 release(s) each names, and is neither stalled nor finished\n",
                drafted.len()
            ));
            for (id, awaiting) in &drafted {
                let says = awaiting
                    .as_deref()
                    .map(|detail| format!(" — {detail}"))
                    .unwrap_or_default();
                out.push_str(&format!("  {id}: complete-but-draft{says}\n"));
            }
        }
        // The settled nodes whose work has not reached anyone. This view
        // otherwise reports only what is in flight, so a planner reading it saw
        // a run go quiet and took that for a run whose work had landed. Named
        // here rather than left to `results`, because deciding there is nothing
        // left to do is a decision made from this view.
        let unlanded = unlanded_nodes(view);
        if !unlanded.is_empty() {
            out.push_str(&format!(
                "  {} node(s) settled without landing: {} — as each settled, not as of now; \
                 `results {}` names the change to open\n",
                unlanded.len(),
                unlanded.join(", "),
                view.paths.run
            ));
        }
        out.push_str(&journal_loss_line(view));
        if let Some(health) = crate::agentgraph::health() {
            out.push_str(&format!("  providers: {health}\n"));
        }
    }
    if out.is_empty() {
        return nothing_to_report(survey);
    }
    out.push_str(&skipped_lines(&survey.skipped));
    out
}

/// What a node that settled [`DISPATCH_DIED`] or [`PROVIDER_FAILED`] says, in one
/// sentence.
///
/// Named once because `results` and `status` both say it, and a manager reading
/// one after the other must not meet two accounts of the same node.
///
/// The two words open the sentence differently and end it identically, because
/// the difference between them is *what* killed the dispatch and the half a
/// reader acts on is the same either way. `provider-failed` says the provider
/// killed it outright: `task-failed` used to send that reader looking for what
/// the work got wrong, when nothing was wrong with the work at all.
///
/// It names the producer's own classification first, because that is what decides
/// whether anything is worth re-running: a rate limit twenty seconds after the
/// final report and a workspace deleted underneath a live turn call for opposite
/// next moves. Then — where the node left a branch, and where `onevcs` recorded
/// what that branch is at — it says the branch may carry finished work and names
/// the commit, which is the half that otherwise has to be dug out of the run's
/// journal by hand.
///
/// It says **may**, deliberately. This crate cannot tell a worker's prose report
/// from a gate verdict, and a sentence that was sometimes wrong about "the gate
/// passed" would be worse than no sentence at all. So it points at the branch and
/// at the node's own transcript and lets a person read them.
///
/// `None` for every node that settled any other way, which is what keeps this out
/// of the line of a node whose agent really did fail its task.
///
/// [`DISPATCH_DIED`]: crate::engine::DISPATCH_DIED
/// [`PROVIDER_FAILED`]: crate::engine::PROVIDER_FAILED
fn death_phrase(state: &RunState, id: &str, branch: Option<&str>) -> Option<String> {
    let word = match state.outcomes.get(id).map(String::as_str) {
        Some(word @ (crate::engine::DISPATCH_DIED | crate::engine::PROVIDER_FAILED)) => word,
        _ => return None,
    };
    let classified = match state.causes.get(id) {
        Some(cause) => format!(" ({cause})"),
        // llmlint: ignore[changed_behavior_has_e2e] no invocation a user can type reaches
        // this arm: `engine::failed_task` settles this word only where it lifted a
        // classification, so a settlement carrying it without one is a journal an *older
        // build* wrote. Reaching it end to end means writing that journal by hand, which
        // would prove the fixture, and dropping the arm would put the run's own word behind
        // an unwrap. Held by this module's unit test instead, which folds that record.
        None => String::new(),
    };
    let where_the_work_is = match (branch, state.heads.get(id)) {
        (Some(branch), Some(head)) => {
            format!("{branch} may carry finished work, at {head}")
        }
        (Some(branch), None) => format!("{branch} may carry finished work"),
        // The workspace went with the dispatch. Said outright, because the
        // absence of a branch here is the difference between work to recover and
        // work to redo.
        (None, _) => "it left no branch, so nothing of it survived".to_owned(),
    };
    // The provider death says what it was rather than what it was not, because
    // that is the reading `task-failed` used to deny it: nothing was wrong with
    // the work, so "rather than failing its task" is not the contrast to draw.
    let how = if word == crate::engine::PROVIDER_FAILED {
        format!("the provider killed the dispatch{classified}, so nothing here is the work's fault")
    } else {
        format!("the dispatch died{classified} rather than failing its task")
    };
    Some(format!("{how}; {where_the_work_is}"))
}

/// How long a node's cancellation has been waiting on the dispatch it asked to
/// stop, when one is still out there — see [`Recorded::cancelling_since`].
///
/// [`Recorded::cancelling_since`]: crate::projection::Recorded::cancelling_since
fn cancelling_for(state: &RunState, id: &str) -> Option<String> {
    let since = state.recorded.get(id)?.cancelling_since()?;
    Some(crate::telemetry::duration(
        sys::now_millis().saturating_sub(since),
    ))
}

/// What became of one candidate a node's identity chain stepped past.
///
/// Three answers rather than a bool, because the third is a different fact and
/// reading it as either of the others is a claim nothing supports: a chain this
/// run's records cannot follow is **not** a chain that ran out of candidates,
/// and a view that called it one would send every reader at a subscription that
/// was never the problem.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Fallthrough {
    /// Another identity went on to run that side's invocation on that turn.
    Served(String),
    /// Nothing served it: the chain had no successful candidate.
    Refused,
    /// This run's records do not say. A single-sided member attributes nothing
    /// per side or per turn, so nothing it publishes can be paired with.
    Unrecorded,
}

/// One rendered line's worth of what a node's identity chains did.
///
/// The advance, what became of it, and how many records said the same thing —
/// the last collapsed **here** rather than in the fold, because two turns of one
/// chain can end differently and a record that had collapsed them could only be
/// rendered as one of the two.
struct ChainRecord<'a> {
    /// The candidate the chain stepped past.
    refusal: &'a Refusal,
    /// What became of that side's turn afterwards.
    became: Fallthrough,
    /// How many records carried this same side, identity, reason, and ending.
    ///
    /// Non-zero for the reason [`Refusal::records`] is: a line exists only by
    /// having been recorded at least once, and a rendering that could hold a
    /// zero would be one that could say a chain recorded nothing.
    records: std::num::NonZeroU64,
}

impl ChainRecord<'_> {
    /// The word this record is reported under.
    ///
    /// A chain that ran out is why the node failed; one that recovered is
    /// evidence beside it, and saying `failed` over it is exactly the confusion
    /// this exists to end.
    fn lead_in(&self) -> &'static str {
        match self.became {
            Fallthrough::Refused => "failed",
            Fallthrough::Served(_) | Fallthrough::Unrecorded => "fallback",
        }
    }
}

/// What one node's identity chains did, in arrival order and one entry per line
/// a view will render.
///
/// Records that agree on the side, the identity, the reason **and** the ending
/// are one fact recorded several times; records that differ in the ending are
/// two facts, and a run whose chain recovered on one turn and ran out on the
/// next says both.
fn chain_records<'a>(state: &'a RunState, node: &str) -> Vec<ChainRecord<'a>> {
    let mut records: Vec<ChainRecord<'a>> = Vec::new();
    for refusal in refusals_of(state, node) {
        let became = became_of(state, node, refusal);
        if let Some(same) = records.iter_mut().find(|seen| {
            seen.refusal.advanced.identity == refusal.advanced.identity
                && seen.refusal.advanced.role == refusal.advanced.role
                && seen.refusal.advanced.reason == refusal.advanced.reason
                && seen.refusal.member == refusal.member
                && seen.became == became
        }) {
            same.records = same.records.saturating_add(refusal.records.get());
            continue;
        }
        records.push(ChainRecord {
            refusal,
            became,
            records: refusal.records,
        });
    }
    records
}

/// What became of the turn one advance was recorded on.
///
/// Paired by **side and turn**, which is what the producer stamps on both
/// records: a two-party member runs one chain per side per turn, publishes an
/// advance per candidate that chain stepped past, and publishes the invocation
/// that actually ran beside them. An advance carrying neither — which is a
/// single-sided member's, the one kind that publishes no invocation at all — has
/// nothing to pair with, and is said to have nothing rather than assumed to have
/// run out.
fn became_of(state: &RunState, node: &str, refusal: &Refusal) -> Fallthrough {
    let (Some(role), Some(turn)) = (refusal.advanced.role, refusal.advanced.turn) else {
        return Fallthrough::Unrecorded;
    };
    match served_in(state, node, &refusal.member, role, turn) {
        Some(served) => Fallthrough::Served(served.session.identity.clone()),
        None => Fallthrough::Refused,
    }
}

/// The invocation that ran one member's side on one turn, if this run recorded
/// one.
///
/// The member is part of the key as well as the side and the turn: a dispatch
/// runs more than one member, each numbers its own turns, and pairing across two
/// of them would name an identity that served somebody else's chain.
fn served_in<'a>(
    state: &'a RunState,
    node: &str,
    member: &MemberLabel,
    role: oneagentgraph::event::Role,
    turn: u64,
) -> Option<&'a Served> {
    state.served.get(node)?.iter().find(|served| {
        served.member == *member && served.session.role == role && served.session.turn == turn
    })
}

/// Every provider refusal one node's dispatches recorded, in arrival order.
fn refusals_of<'a>(state: &'a RunState, node: &str) -> &'a [Refusal] {
    state.refusals.get(node).map_or(&[], Vec::as_slice)
}

/// How one candidate a chain stepped past reads on a rendered line.
///
/// The side first, because it is the half a reader most often gets wrong: the
/// two sides of a member prefer different identities, and an operator who
/// restored the wrong subscription spent a night watching the same failure.
///
/// A bare "refused" is reserved for a chain with **no** successful candidate.
/// Every other ending says the chain fell through and what happened next, so no
/// reader takes a recovery for the reason a node failed.
///
/// Every value on the line is a stranger's — an identity, a classification, a
/// role, and a member name, all read off a sibling's envelope, and for a
/// recovery a second identity read off a second one — so the whole phrase goes
/// through the same control strip the rest of this module uses.
fn chain_phrase(record: &ChainRecord) -> String {
    let refusal = record.refusal;
    // The role's own spelling, taken from the producing library's serialization
    // rather than matched into words of this crate's: the sides are that
    // library's vocabulary, and a second spelling of them here is a second thing
    // to keep true.
    let role = refusal
        .advanced
        .role
        .and_then(|role| serde_json::to_value(role).ok());
    let side = match (
        role.as_ref().and_then(serde_json::Value::as_str),
        &refusal.member,
    ) {
        (Some(role), _) => format!("the {role} side"),
        (None, MemberLabel::Named(member)) => format!("member '{member}'"),
        // Neither was stamped. The identity is still the thing to act on, and
        // naming a side the record does not carry would send the fix at a chain
        // nobody named — which is the failure this line exists to end.
        //
        // llmlint: ignore-block[changed_behavior_has_e2e] `oneagentgraph` labels a
        // member's envelopes with the member, so no producer reaches either arm; they are
        // written for one that stamps neither, or stamps something that is not a member
        // name. The arm a producer does reach is driven in `tests/e2e/views.rs`.
        (None, MemberLabel::Unstamped) => "a side the record does not name".to_string(),
        // Stamped, and not readable as a member. Saying the record names no
        // side would be denying a record that does name one.
        (None, MemberLabel::Unreadable) => "a side this build cannot read".to_string(),
        // llmlint: ignore-end[changed_behavior_has_e2e]
    };
    // llmlint: ignore-block[changed_behavior_has_e2e] `FallbackAdvanced::reason` is a
    // required `String`, so no producer reaches the empty half; it is written for a newer
    // sibling that relaxes the field. The half a producer does reach is driven end to end.
    let reason = if refusal.advanced.reason.is_empty() {
        "for a reason the record does not carry".to_string()
    } else {
        format!("({})", refusal.advanced.reason)
    };
    // llmlint: ignore-end[changed_behavior_has_e2e]
    // What was counted, said as what it is: records carrying this same side,
    // identity, reason, and ending. The producer stamps a turn on each advance
    // and nothing here counts them, so "on N turns" would be a measurement this
    // line never made.
    let again = if record.records.get() > 1 {
        format!(", recorded {} times", record.records)
    } else {
        String::new()
    };
    let identity = &refusal.advanced.identity;
    one_line(&match &record.became {
        Fallthrough::Refused => format!("{side}: identity '{identity}' refused {reason}{again}"),
        Fallthrough::Served(who) => {
            format!("{side} fell through '{identity}' {reason} → served by '{who}'{again}")
        }
        Fallthrough::Unrecorded => format!(
            "{side} fell through '{identity}' {reason}; nothing this run recorded names what \
             served that turn{again}"
        ),
    })
}

/// How one judge verdict that failed a node reads on a rendered line.
///
/// Both halves are a stranger's — the criterion a graph declared and the
/// sentence a judge wrote — so the phrase goes through the same control strip
/// every other relayed value on these views does.
fn verdict_phrase(verdict: &crate::report::FailedVerdict) -> String {
    // A record that named neither is still worth a line: it says the node failed
    // on its judge, which is the fact a provider line above it would otherwise
    // be read as.
    let criterion = match &verdict.criterion {
        Some(criterion) => format!("'{criterion}'"),
        None => "a criterion the record does not name".to_string(),
    };
    let reason = match &verdict.reason {
        Some(reason) => reason.clone(),
        None => "the record carries no reason".to_string(),
    };
    one_line(&format!("{criterion} failed — {reason}"))
}

/// How the dependencies that skipped a node read on that node's own line.
///
/// Each carries its own status: a `failed` cause is work attempted and lost and
/// a `skipped` one is another node never tried, so a reader following the chain
/// back knows whether the next hop is the end of it.
fn skipped_by_phrase(causes: &[(String, NodeStatus)]) -> String {
    if causes.is_empty() {
        // Unreachable by construction, and phrased anyway: rendering nothing
        // would read as a fact the view lost.
        return "a dependency this run can no longer name".to_string();
    }
    causes
        .iter()
        .map(|(dependency, status)| format!("{dependency} ({})", status.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether a person attested that a node this run **failed** had in fact landed.
///
/// Two records, because either alone says something else: an attestation is how
/// every human action completes, and the failure is what the status said before
/// anybody looked.
fn attested_after_failing(view: &RunView, node: &str) -> bool {
    view.state.attestations.contains(node)
        && view.events.iter().any(|event| {
            event.kind.0 == PipelineKind::NodeSettled.as_str()
                && event.labels.node.as_deref() == Some(node)
                && event
                    .payload
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    == Some(NodeStatus::Failed.as_str())
        })
}

/// The nodes whose change had not reached its base when they settled, in id
/// order.
///
/// Read off what each settlement observed, plus whatever the run's own settled
/// report re-read afterwards — never off a node's repository or its policy — so
/// a node absent from this list is one that either landed or had no change to
/// land, and never one nobody looked at.
///
/// It is deliberately not called what has *not landed now*: the re-read happens
/// when a driver closes the run out and cannot always answer, so a node here is
/// one no read has moved rather than one proved still open. Every line rendered
/// from this list says so — see the per-node phrase below, and divergence 33 in
/// [the divergence record](../../../docs/contract-divergences.md) for the half
/// that still cannot be reached.
fn unlanded_nodes(view: &RunView) -> Vec<String> {
    let statuses = view.state.statuses();
    view.state
        .landings
        .iter()
        .filter(|(_, landing)| **landing == Landing::Unlanded)
        // A node whose change is held as a draft has not landed and is not one of
        // these: this list is work an operator has to *decide* about — a change
        // sitting open that nobody merged — and a draft is a change this run is
        // deliberately holding and will lift itself. It has a line of its own,
        // which says what it is waiting on.
        .filter(|(node, _)| statuses.get(*node) != Some(&NodeStatus::CompleteDraft))
        .map(|(node, _)| node.clone())
        .collect()
}

/// The nodes whose work is complete and whose change is held back as a draft,
/// with what each is waiting on.
///
/// The reason is the settlement's own detail, which is the same sentence `results`
/// prints — so a reader meeting this line and a reader opening `results` are told
/// the same thing rather than two accounts of one node.
fn drafted_nodes(view: &RunView) -> Vec<(String, Option<String>)> {
    view.state
        .statuses()
        .into_iter()
        .filter(|(_, status)| *status == NodeStatus::CompleteDraft)
        .map(|(node, _)| {
            let awaiting = settled_detail(view, &node).map(|detail| one_line(&detail));
            (node, awaiting)
        })
        .collect()
}

/// The `detail` the newest `node-settled` for one node carried.
///
/// One read for the two views that print it, so a settlement cannot be quoted two
/// ways.
fn settled_detail(view: &RunView, node: &str) -> Option<String> {
    view.events
        .iter()
        .rev()
        .find(|event| {
            event.kind.0 == PipelineKind::NodeSettled.as_str()
                && event.labels.node.as_deref() == Some(node)
        })
        .and_then(|event| event.payload.get("detail"))
        .and_then(|detail| detail.as_str())
        .map(str::to_owned)
}

/// What a ready node is waiting on, as far as this host can tell.
///
/// A lifecycle node cannot start until it can open a `onevcs` session over its
/// repository, and a session is held under an occupancy lease — so a ready node
/// whose repository somebody is already in is waiting on that lease, and waits
/// silently. The commonest holder is the node's *own* previous dispatch, which
/// is the state right after a cancel: the node is back on the frontier and the
/// dispatch it cancelled has not let go. Reported so that node reads differently
/// from one waiting for nothing but a concurrency slot, because a supervisor
/// spent forty minutes looking for a wedge in the second when it was the first.
///
/// Three answers and not two. A workspace this host **could not ask about** is
/// neither held nor free, and saying "queued" there would report an unmeasured
/// thing as a measured nothing — the one rule every view here is written to. The
/// holders themselves are `onevcs`'s own verdict, liveness included, because a
/// pid alone cannot say whether a lease is real.
fn waiting_on(state: &RunState, id: &str) -> String {
    const QUEUED: &str = "queued for dispatch";
    // A node with no repository has no workspace to be held out of.
    let Some(repo) = state.graph.get(id).and_then(|node| node.repo.as_deref()) else {
        return QUEUED.to_string();
    };
    let holders = match crate::vcs::holders_of(repo) {
        Ok(holders) => holders,
        Err(why) => {
            return format!(
                "{QUEUED}, and this host cannot say whether the '{repo}' workspace is \
                 free: {}",
                one_line(&why)
            )
        }
    };
    let held: Vec<String> = holders
        .into_iter()
        .filter(|holder| {
            holder.state == onevcs::Lifecycle::Open && holder.liveness == onevcs::Liveness::Live
        })
        .map(|holder| {
            format!(
                "session '{}' (owner_pid {})",
                holder.token.0, holder.owner_pid
            )
        })
        .collect();
    if held.is_empty() {
        return QUEUED.to_string();
    }
    format!(
        "waiting for the '{repo}' workspace, held by {}",
        held.join(", ")
    )
}

/// What one in-flight dispatch is doing now, on the line that reports it.
///
/// Four facts, because one alone misleads: what it last did, how much it has
/// done, how long ago, and — separately — whether it is still alive. A dispatch
/// that has recorded plenty and nothing recently is the wedged one; a first turn
/// that has run for twenty minutes and is still recording is healthy, and has
/// twice been reported dead for want of this line.
///
/// The age is of the last thing the dispatch **did**, and the heartbeat is
/// reported beside it rather than inside it: an age over every envelope can
/// never be older than one beat for anything that has not died, so a wedged
/// dispatch and a working one read the same.
fn working(activity: &crate::projection::NodeActivity) -> String {
    let ago = |at: u64| crate::telemetry::duration(sys::now_millis().saturating_sub(at));
    let alive = activity
        .last_heartbeat_at
        .filter(|beat| activity.progress.is_none_or(|done| *beat > done.last_at()))
        .map(|beat| format!("; alive {} ago", ago(beat)))
        .unwrap_or_default();
    // A dispatch whose every envelope has been a heartbeat has *started* and
    // produced nothing, which is neither a dispatch nothing is driving nor one
    // that is doing something. Said in those words rather than as an age of
    // nothing.
    let Some(progress) = activity.progress else {
        return format!("nothing recorded yet{alive}");
    };
    let counted = format!(
        "{} event(s), {} ago{alive}",
        progress.events(),
        ago(progress.last_at())
    );
    match &activity.doing {
        Some(doing) => format!("now {doing} ({counted})"),
        // Absent rather than guessed: the dispatch has recorded something and
        // has not named a tool, so the count and the age are the whole of what
        // this line can claim.
        None => counted,
    }
}

/// Whether this host can prove a recorded dispatch is still running.
///
/// Three answers, because a row an operator acts on has to distinguish them.
/// `Stale` and `Held` are proofs in opposite directions; `Unproven` is the
/// answer this host does not have, and collapsing it into either is how a
/// registry row outlives the process it describes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Proof {
    /// The dispatch's own registry entry names a live pid, and that pid started
    /// when the entry says it did.
    ///
    /// Deliberately not named for liveness: what this establishes is that the
    /// evidence agrees, to the resolution the host reports a process start at —
    /// one second, where that is `ps`. A pid reused *inside* that resolution by
    /// a process that also started then is the one case this cannot separate,
    /// and it is why the word is about the record rather than about the work.
    Held,
    /// This host proved the process behind the row is gone.
    Stale(String),
    /// This host cannot decide: the dispatch was recorded elsewhere, the
    /// registry cannot be read, or it holds no entry for the node at all.
    Unproven(String),
}

/// What a run's own dispatch registry says about the processes its work is in.
///
/// The registry, and not the ownership lock: the lock names the **driver**, which
/// starts the dispatch and does not outlive it, and judging a row by it answered
/// about the wrong process — a run stopped and then adopted carries its stop for
/// ever, so its fresh dispatches all read as ended. [`ledger::DispatchRecord`] is
/// what the run writes for each process its work is in, and what a `stop` aims
/// at, so a row here and the teardown an operator runs after reading it are
/// decided from one record.
// llmlint: ignore-block[invalid_states_unrepresentable] the key is the node id
// `DispatchRecord::node` carries, and a node id is the plain `String` every identifier in
// this crate is, for the reason `src/error.rs`'s file-level suppression states. The value
// is not this reader's to check either: it comes back from `ledger::dispatches_of`, which
// is the boundary that refuses an entry nothing may be acted on.
enum Registry {
    /// What the registry holds, by the node each entry names.
    Read(BTreeMap<String, Vec<ledger::DispatchRecord>>),
    /// The registry could not be read, and this is why.
    ///
    /// Not an absence: a run whose registry this build cannot enumerate is one
    /// nobody can say is idle, which is the whole reading this view exists to
    /// stop being made.
    Unreadable(String),
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl Registry {
    /// Read one run's registry, keeping the refusal where there is one.
    fn of(paths: &RunPaths) -> Self {
        match ledger::dispatches_of(paths) {
            Ok(records) => {
                let mut by_node: BTreeMap<String, Vec<ledger::DispatchRecord>> = BTreeMap::new();
                for record in records {
                    by_node.entry(record.node.clone()).or_default().push(record);
                }
                Self::Read(by_node)
            }
            Err(error) => Self::Unreadable(error.to_string()),
        }
    }

    /// What this registry proves about one node's dispatch.
    ///
    /// **Most certain first, and a live entry wins.** One node can hold more than
    /// one entry — a dispatch that was retried inside the run leaves the ended
    /// attempt's record behind until the process it named is collected — so a
    /// stale entry beside a live one says the older attempt ended, never that the
    /// node has nothing running.
    fn proves(&self, node: &str) -> Proof {
        let by_node = match self {
            Self::Unreadable(why) => {
                return Proof::Unproven(format!(
                    "the run's dispatch registry cannot be read: {why}"
                ))
            }
            Self::Read(by_node) => by_node,
        };
        let records = by_node.get(node).map(Vec::as_slice).unwrap_or_default();
        let mut unproven = None;
        let mut stale = None;
        for record in records {
            match proof_of(record) {
                Proof::Held => return Proof::Held,
                Proof::Unproven(why) => unproven.get_or_insert(why),
                Proof::Stale(why) => stale.get_or_insert(why),
            };
        }
        match (unproven, stale) {
            (Some(why), _) => Proof::Unproven(why),
            (None, Some(why)) => Proof::Stale(why),
            // No entry at all: the run says a dispatch is in flight and no
            // process claims it. Deliberately not a proof of staleness — the
            // entry is written by the executor as the dispatch starts, so a
            // reader between those two writes would report live work as ended.
            (None, None) => Proof::Unproven(
                "the run's dispatch registry holds no entry for it, so nothing here says which \
                 process it is in"
                    .to_string(),
            ),
        }
    }
}

/// Whether this host can prove the process one registry entry names is still the
/// dispatch it was recorded for.
///
/// Its pid says which process, and its start token says the pid is still that
/// process — a pid alone is what a two-day-old entry has, and a reused one
/// answers a liveness probe as alive.
fn proof_of(record: &ledger::DispatchRecord) -> Proof {
    if record.host != sys::hostname() {
        return Proof::Unproven(format!(
            "its dispatch runs on {}, and a pid means nothing across machines",
            record.host
        ));
    }
    if !sys::process_may_be_live(record.pid) {
        return Proof::Stale(format!("its dispatch (pid {}) is gone", record.pid));
    }
    match sys::process_start_token(record.pid) {
        // llmlint: ignore-block[changed_behavior_has_e2e] the host declining to answer is a
        // property of the machine rather than of anything a user types. The answers it does
        // give, and the other unproven arms that resolve alike, are driven end to end.
        None => Proof::Unproven(format!(
            "this host will not say when pid {} started",
            record.pid
        )),
        // llmlint: ignore-end[changed_behavior_has_e2e]
        Some(token) if token.matches(&record.started) => Proof::Held,
        Some(_) => Proof::Stale(format!(
            "pid {} is a different process from the one its dispatch was recorded in",
            record.pid
        )),
    }
}

/// `onepipeline host` — every live dispatch on this host, across every planner.
///
/// A row here is a claim that a dispatch exists **now**, and it is acted on: an
/// operator leaves it alone, or ends it. So a row is rendered as live only where
/// this host can prove the process the dispatch is in is still that process —
/// the pid its own registry entry names, and the start token beside it. The run's
/// ownership lock is deliberately *not* what decides this — it names the driver,
/// and a driver is not the work; the reasoning is on the private reader that
/// replaced it. A row
/// proved to have nothing behind it is dropped and counted, and one this host
/// cannot decide either way is rendered saying so. Never a bare row that reads
/// as live work.
pub fn host(survey: &Survey) -> String {
    let mut out = format!("host {}\n", sys::hostname());
    // The scope of the claim. This scan has an under-reporting direction it
    // cannot see past — a run recorded under another runs root is a live
    // dispatch this view will never list — and a reader who does not know which
    // root was read cannot tell that absence from an idle host.
    out.push_str(&format!(
        "  reading {}\n",
        one_line(&survey.root.display().to_string())
    ));
    let mut rendered = false;
    let mut ignored: Vec<String> = Vec::new();
    for view in &survey.views {
        let registry = Registry::of(&view.paths);
        for (id, status) in &view.state.statuses() {
            if *status != NodeStatus::Running {
                continue;
            }
            let proof = registry.proves(id);
            if let Proof::Stale(why) = &proof {
                ignored.push(one_line(&format!("{}/{id}: {why}", view.paths.run)));
                continue;
            }
            let age = view
                .state
                .dispatched_at
                .get(id)
                .map(|at| sys::now_millis().saturating_sub(*at))
                .unwrap_or(0);
            rendered = true;
            out.push_str(&format!(
                "  {:<24} {:<20} {:<16} {}",
                view.paths.run,
                id,
                view.launch.launcher,
                crate::telemetry::duration(age)
            ));
            if let Proof::Unproven(why) = &proof {
                out.push_str(&format!("  UNPROVEN: {}", one_line(why)));
            }
            out.push('\n');
        }
    }
    if !rendered {
        out.push_str("  no live dispatches\n");
    }
    if !ignored.is_empty() {
        out.push_str(&format!(
            "  {} stale registry entr{} ignored: {}\n",
            ignored.len(),
            if ignored.len() == 1 { "y" } else { "ies" },
            ignored.join("; ")
        ));
    }
    out.push_str(&skipped_lines(&survey.skipped));
    out
}

/// One run's merged store as one reader is shown it.
///
/// **Read-time only.** The store is not touched and nothing is recorded: two
/// readers of the same run see it through different profiles and neither loses
/// an event the other keeps, which is the whole difference between this and the
/// source filters a launch passes through to `oneagentgraph` and `onevcs`.
///
/// Borrowed rather than cloned where nothing is dropped, because the common case
/// — `--all`, and the shipped `monitor` profile — is a filter that admits
/// everything.
pub fn shaped<'a>(view: &'a RunView, filter: &EventFilter) -> Vec<&'a Envelope> {
    view.events
        .iter()
        .filter(|event| filter.matches(event))
        .collect()
}

/// `onepipeline monitor` — one pass over the merged stream.
///
/// The first line is the contract, not a banner: every event line carries the
/// typed id a detail lookup resolves, and the monitor never tries to *be* the
/// detail.
pub fn monitor(view: &RunView, filter: &EventFilter) -> String {
    let mut out = String::from(
        "Concise graph events; ask the producing library for full detail by stream id.\n",
    );
    for event in shaped(view, filter) {
        let id = match event.source {
            Source::Pipeline => format!("graph:{}", event.labels.node.as_deref().unwrap_or("-")),
            Source::Agentgraph => format!("agent:{}", event.stream),
            Source::Vcs => format!("vcs:{}", event.stream),
        };
        out.push_str(&format!(
            "{}  {:<28} {}{}\n",
            event.ts,
            id,
            summarize(event),
            superseded_suffix(view, event)
        ));
    }
    // The run's own state has no node, so it has no graph id: it reaches the
    // reader as a trailer rather than as an event line.
    out.push_str(&format!(
        "-- {}  {}  {}  {}\n",
        view.paths.run,
        view.summary(),
        liveness_word(view),
        graph::state_of(&view.state.statuses()).as_str()
    ));
    out
}

/// What a line about a superseded node says beyond the event itself, or nothing
/// for every other line.
///
/// The stream keeps the `node-settled` that failed the node, and it is what the
/// run's own monitor persona reads — see
/// [`crate::projection::RunState::superseded`] for what that monitor did with an
/// unqualified one.
///
/// Only the records *about the node* carry it, which keeps it a qualification
/// rather than a banner: a relayed envelope of a sibling's is about the dispatch
/// that ran, and the supersession is this crate's own answer about the node.
fn superseded_suffix(view: &RunView, event: &Envelope) -> String {
    if event.source != Source::Pipeline {
        return String::new();
    }
    let Some(node) = event.labels.node.as_deref() else {
        return String::new();
    };
    match view.state.superseded.get(node) {
        Some(replacement) => format!("  — superseded, retried as {}", one_line(replacement)),
        None => String::new(),
    }
}

/// One control-stripped line derived from an event's recorded values.
fn summarize(event: &Envelope) -> String {
    const CAP: usize = 96;
    let mut detail = event.kind.0.clone();
    // `landing` beside `status`: a `node-settled` and a `published` both carry
    // it, and a monitor line that showed only `done` said the same thing about a
    // merge and about an open change request.
    for key in ["status", "landing", "outcome", "state", "message", "reason"] {
        if let Some(value) = event.payload.get(key).and_then(|v| v.as_str()) {
            detail.push_str(&format!(" {value}"));
        }
    }
    let stripped: String = detail
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if stripped.chars().count() <= CAP {
        return stripped;
    }
    stripped.chars().take(CAP).collect()
}

/// How a landing reads on a rendered line.
///
/// A phrase rather than the bare word, and it says **when** it was true, because
/// only one of the two answers stays true. A change observed on its base has
/// reached it and a base does not stop carrying what it carries; a change that
/// had not reached it is an observation of a moment, and the moment passes — a
/// node that settled `done (queued)` was still reporting the settlement's answer
/// hours after its change had merged, and a supervisor read that as work nobody
/// had landed.
///
/// So the unlanded phrase is dated, and says that no later read has moved it.
/// One does look: a driver asks again as the run closes out and records what it
/// found, which [`landings_the_run_re_read`] is what carries onto these lines. It
/// cannot always answer — see [`crate::vcs::proved_landed`] and divergence 33 in
/// [the divergence record](../../../docs/contract-divergences.md) — and where it
/// does not, this is the claim that stands: the settlement's own, dated, beside
/// the change's own URL. Asserting the state of things now would not be honest.
fn landed_phrase(landing: Landing, settled_at: Option<u64>) -> String {
    let ago = match settled_at {
        Some(at) => format!(
            " {} ago",
            crate::telemetry::duration(sys::now_millis().saturating_sub(at))
        ),
        // A settlement whose moment the ledger does not carry: the claim is
        // still the settlement's, and saying *when* would be inventing one.
        None => String::new(),
    };
    match landing {
        Landing::Landed => "landed on its base".to_string(),
        Landing::Unlanded => format!(
            "NOT landed: the change had not reached its base when this settled{ago}, and \
             no later read has said otherwise — open the change for where it is now"
        ),
    }
}

/// What a view says about the records the run's own store does not hold whole,
/// or nothing when it holds them all.
///
/// Every line either view prints is folded from that store, so a loss inside it
/// is the one fact that makes the rest of them unprovable — a `node-settled`
/// nobody can read renders as a node that never settled, and an `edit-committed`
/// nobody can read renders as a node the run never had. It is said here because
/// the only place it used to be said was the driver's stderr, which a detached
/// run writes to a log file nobody opens.
fn journal_loss_line(view: &RunView) -> String {
    let integrity = crate::journal::integrity(&view.paths.journal());
    if integrity.is_whole() {
        return String::new();
    }
    format!(
        "  journal: {} — this run's record of itself is incomplete\n",
        integrity.phrase()
    )
}

/// `onepipeline results` — per-node outcomes, with each node's own evidence.
pub fn results(view: &RunView) -> String {
    // The run and how its graph stands — deliberately not the node tally the
    // other views carry, because every line under this one is a node's own
    // status and a header that also said `done` would read as one of them.
    let mut out = format!(
        "{}  {}\n",
        view.paths.run,
        graph::state_of(&view.state.statuses()).as_str()
    );
    let statuses = view.state.statuses();
    for node in view.state.graph.iter() {
        let status = statuses
            .get(&node.id)
            .copied()
            .unwrap_or(NodeStatus::Pending);
        out.push_str(&format!("  {:<24} {}", node.id, status.as_str()));
        if let Some(outcome) = view.state.outcomes.get(&node.id) {
            out.push_str(&format!(" ({outcome})"));
        }
        // Beside the status word for the same reason `status` gives it a line:
        // `parked` on its own says the planner idled the node, and says nothing
        // about the dispatch still running for it.
        if let Some(pending) = cancelling_for(&view.state, &node.id) {
            out.push_str(&format!(" — cancelling, asked to stop {pending} ago"));
        }
        // Beside the status word, because it is the fact the status word does
        // not carry: `done` is the same for a change that merged and one still
        // sitting in a pull request. Rendered for both landings rather than only
        // the unlanded one — a reader who sees the qualifier where a change was
        // open and nothing where it merged is reading the absence, which is what
        // every other node's absence already means.
        if let Some(landing) = view.state.landings.get(&node.id) {
            let settled_at = view.state.settled_at.get(&node.id).copied();
            out.push_str(&format!(" — {}", landed_phrase(*landing, settled_at)));
        }
        // The attestation settles the node, so the status word alone would
        // report a dispatch that failed as one that succeeded. Both records
        // ride the line instead: what this run got, and what a person said
        // afterwards — which is also what released everything under it.
        if attested_after_failing(view, &node.id) {
            out.push_str(" — settled failed, attested as landed");
        }
        if status == NodeStatus::Running && view.state.stop_recorded() {
            out.push_str(&format!(" — {}", became_of_the_worker(&view.state)));
        }
        // What the dispatch reported, before what the plan asked for: an
        // unpinned lifecycle node's branch is named by the sibling that cut it,
        // so the plan does not know it and a reader looking for the work would
        // find nothing.
        let branch = view
            .state
            .branches
            .get(&node.id)
            .or(node.branch.as_ref())
            .cloned();
        if let (NodeStatus::Parked | NodeStatus::Failed | NodeStatus::Cancelled, Some(branch)) =
            (status, &branch)
        {
            out.push_str(&format!(" — preserved on {branch}"));
        }
        // Where the dispatch an adoption cleared had got to. The node itself is
        // dispatched again and settles under its own words, and none of them say
        // that an *earlier* dispatch committed work somewhere: the driver that
        // was running it exited without settling anything, so this line is the
        // only place that branch and that session are ever named.
        if let Some(session) = view.state.abandoned.get(&node.id) {
            out.push_str(&format!(
                " — a dispatch was abandoned when the run was adopted; its work is on {} \
                 (onevcs session {})",
                session.branch(),
                session.token().0
            ));
        }
        // The one piece of evidence a person actually opens.
        if let Some(url) = view.state.change_urls.get(&node.id) {
            out.push_str(&format!(" — {url}"));
        }
        out.push('\n');
        // Before the detail, because it is what the detail is evidence *for*: the
        // detail is the producer's own sentence about how the dispatch ended, and
        // this says what that means for the node — which used to be a thing a
        // manager derived by opening `events.jsonl` and counting commits.
        if let Some(died) = death_phrase(&view.state, &node.id, branch.as_deref()) {
            out.push_str(&format!("      died: {died}\n"));
        }
        if let Some(detail) = settled_detail(view, &node.id) {
            out.push_str(&format!("      detail: {}\n", one_line(&detail)));
        }
        // What this node is judged against beyond its own task prose. Under the
        // node rather than beside it, because it is prose rather than a word,
        // and rendered before the settlement's own reasons because it is what
        // those reasons were reached against.
        if let Some(amendment) = &node.amendment {
            out.push_str(&format!("      amendment: {}\n", one_line(amendment)));
        }
        // Why the judge failed it, then which chains ran out, then which
        // recovered — in that order, because that is the order they matter in.
        //
        // The verdict is first because it is the thing that actually failed the
        // node, and it used to be reachable only by opening the node's retained
        // report by hand while three provider lines sat above it pointing
        // somewhere else. A chain that ran out gets the `provider` line, which
        // is a retry aimed at a subscription; a chain that recovered gets its
        // own word, because a fix aimed at it changes nothing.
        if status == NodeStatus::Failed {
            for verdict in crate::report::failed_verdicts(&view.events, &node.id) {
                out.push_str(&format!("      verdict: {}\n", verdict_phrase(&verdict)));
            }
            let chains = chain_records(&view.state, &node.id);
            for record in chains
                .iter()
                .filter(|record| record.became == Fallthrough::Refused)
            {
                out.push_str(&format!("      provider: {}\n", chain_phrase(record)));
            }
            for record in chains
                .iter()
                .filter(|record| record.became != Fallthrough::Refused)
            {
                out.push_str(&format!("      fallback: {}\n", chain_phrase(record)));
            }
        }
        // Why the run never asked this node to do anything. `skipped` on its own
        // says a dependency of *some* kind went wrong and leaves a reader to
        // rebuild the graph by hand to find which — which is how a node stayed
        // skipped over work that had already merged. The dependency is known at
        // the moment the skip is derived, so it is named where the skip is
        // reported.
        if status == NodeStatus::Skipped {
            out.push_str(&format!(
                "      never attempted; skipped by: {}\n",
                skipped_by_phrase(&graph::skipped_by(&view.state.graph, &statuses, &node.id))
            ));
        }
        if status == NodeStatus::Waiting {
            if let Some(task) = &node.task {
                out.push_str(&format!("      action: {task}\n"));
            }
            let unblocks = graph::unblocks(&view.state.graph, &node.id);
            if !unblocks.is_empty() {
                out.push_str(&format!("      unblocks: {}\n", unblocks.join(", ")));
            }
        }
    }
    out.push_str(&superseded_lines(view));
    out.push_str(&journal_loss_line(view));
    out
}

/// What the run's results say about the nodes a `retry` replaced, or nothing
/// where it replaced none.
///
/// **Under the graph, because they are not in it** — every line above is a node
/// the run is still executing, and these vanished from the results altogether;
/// [`crate::projection::RunState::superseded`] is what that cost. The line says
/// what became of the node rather than what its last dispatch scored, and names
/// the replacement because that is where the work went: a supersession inherits
/// the branch, so the replacement's own line above is where the work is.
fn superseded_lines(view: &RunView) -> String {
    let mut out = String::new();
    for (node, replacement) in &view.state.superseded {
        out.push_str(&format!(
            "  {:<24} superseded — retried as {}\n",
            one_line(node),
            one_line(replacement)
        ));
    }
    out
}

/// `onepipeline transcript` — one dispatch's turns, its tools, and its words.
///
/// Two sources, because they answer at different times and neither is the whole
/// answer. The merged store carries every `turn-activity` as it arrives, so a
/// turn's tools are readable *while it runs*; the onejudge report a
/// `member-settled` retained carries the conversation itself, which is what a
/// reader asking why a turn did what it did needs, and which exists only once
/// the member has settled.
///
/// A report this run did not keep a copy of is said to be unretained rather
/// than passed over: an absent transcript and an unread one are different facts.
/// The path the producer named is printed and **never opened** — the only file
/// this verb reads is the run's own copy, made when the settlement was ingested.
pub fn transcript(view: &RunView, only: Option<&str>) -> String {
    let mut out = String::new();
    // Derived once for the whole run rather than per node.
    let settlements = crate::report::evidence(&view.paths, &view.events);
    for node in nodes_with_agent_records(view, only) {
        // Every value on a rendered line is a stranger's: a node label a
        // producer stamped, a member it named, a path it chose, a role its
        // report carried. One control character in any of them rewrites the
        // line around it, so they all go through the same strip.
        out.push_str(&format!("{}  {}\n", view.paths.run, one_line(&node)));
        for event in view
            .events
            .iter()
            .filter(|event| event.source == Source::Agentgraph)
            .filter(|event| event.labels.node.as_deref() == Some(node.as_str()))
        {
            let field = |key: &str| {
                event
                    .payload
                    .get(key)
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
            };
            match event.kind.0.as_str() {
                "turn-started" => out.push_str(&format!(
                    "  turn {}\n",
                    event
                        .payload
                        .get("turn")
                        .map_or_else(|| "-".to_string(), ToString::to_string)
                )),
                "turn-activity" => out.push_str(&format!(
                    "    {} {}  {}\n",
                    one_line(field("kind")),
                    one_line(field("name")),
                    // Read out of the payload the same way a retained report's
                    // event is: a result's text is under `output` and it carries
                    // no `detail` at all, so a third column read out of `detail`
                    // was blank on every observation a turn made.
                    tool_text(&ToolText::of(field("kind"), |key| event.payload.get(key)))
                )),
                _ => {}
            }
        }
        for settled in settlements
            .iter()
            .filter(|settled| settled.node.as_deref() == Some(node.as_str()))
        {
            // Named by the member that settled with it: a graph runs more than
            // one, and a reader looking at two reports has to know whose is
            // whose. The path is the producer's own, printed so a reader knows
            // what the settlement claimed — and it is not what is opened.
            out.push_str(&format!(
                "  report {} {}\n",
                one_line(settled.member.as_deref().unwrap_or("-")),
                one_line(&settled.named.display().to_string())
            ));
            let Some(document) = crate::report::read(&settled.kept) else {
                out.push_str(
                    "    not retained by this run, so it is not read: only this run's own \
                     copy of a report is ever opened\n",
                );
                continue;
            };
            let turns = crate::report::turns(&document);
            if turns.is_empty() {
                out.push_str("    it carries no transcript\n");
            }
            for turn in turns {
                out.push_str(&format!("    {}\n", one_line(&turn.role)));
                for line in turn.text.lines() {
                    out.push_str(&format!("      {}\n", one_line(line)));
                }
                for tool in turn.tools {
                    out.push_str(&format!(
                        "      {} {}  {}\n",
                        one_line(&tool.kind),
                        one_line(&tool.name),
                        tool_text(&tool.text)
                    ));
                }
            }
        }
    }
    if out.is_empty() {
        out.push_str("no dispatch has recorded a transcript\n");
    }
    out
}

/// The nodes this run's merged store carries an `oneagentgraph` record for, in
/// id order.
///
/// Any record, not only a settled turn: a node whose dispatch is still running
/// has a transcript worth reading, and that is most of what this verb is for.
///
/// Crate-visible: `docs/contract.md` names the views, not the parts one is
/// assembled from, and a public item the contract does not name is a promise
/// this crate did not make.
pub(crate) fn nodes_with_agent_records(view: &RunView, only: Option<&str>) -> Vec<String> {
    let mut nodes: Vec<String> = view
        .events
        .iter()
        .filter(|event| event.source == Source::Agentgraph)
        .filter_map(|event| event.labels.node.clone())
        .filter(|node| only.is_none_or(|wanted| wanted == node))
        .collect();
    nodes.sort_unstable();
    nodes.dedup();
    nodes
}

/// How much of a tool's own output one transcript line prints.
///
/// Explicit rather than incidental, because the two sources this verb reads are
/// bounded differently. A `turn-activity` summary is inside
/// [`MAX_PAYLOAD_TEXT_BYTES`](crate::event::MAX_PAYLOAD_TEXT_BYTES) by the time
/// it is in the store — this crate's own promise about its own envelope, cut and
/// marked at ingest — while a retained report's output is the harness's raw
/// bytes with nothing bounding them at all: reports on this host carry single
/// outputs past sixty kilobytes, and one of those splatted whole into a terminal
/// is the rendering this ceiling exists to prevent.
///
/// So it is that same bound, read as a **character** ceiling against a byte one:
/// a text inside 4096 bytes is inside 4096 characters, which is what makes this
/// unable to cut anything the run's own journal kept. What it does cut on the
/// report path it counts out loud.
const MAX_TOOL_OUTPUT_CHARS: usize = crate::event::MAX_PAYLOAD_TEXT_BYTES;

/// The third column of a tool's line: what a call acted on, or what a result
/// returned.
///
/// Whichever half the event is, because a `tool_result` carries its text under
/// `output` and carries no `detail` at all — reading only the latter rendered
/// every observation a turn made as a blank column, which is a run's own
/// evidence hidden by its reader. Either text is a stranger's, so either goes
/// through the same control-character strip every other borrowed value on these
/// views does.
///
/// **What is not printed is said rather than dropped.** An output the producer
/// had already cut short says so, and one this view cuts says how much of it a
/// reader is looking at — a truncated output rendered as though it were whole is
/// how a reader concludes a tool returned nothing further, which is the reading
/// this verb exists to correct. A result that returned nothing renders as the
/// empty column it is, and says nothing about what it left out, because it left
/// nothing out.
fn tool_text(text: &ToolText) -> String {
    let (output, truncated) = match text {
        ToolText::Acted(detail) => return one_line(detail),
        ToolText::Returned { output, truncated } => (output, *truncated),
    };
    if output.is_empty() {
        return String::new();
    }
    let stripped = one_line(output);
    let whole = stripped.chars().count();
    let mut text: String = stripped.chars().take(MAX_TOOL_OUTPUT_CHARS).collect();
    let mut notes: Vec<String> = Vec::new();
    if whole > MAX_TOOL_OUTPUT_CHARS {
        notes.push(format!("{MAX_TOOL_OUTPUT_CHARS} of {whole} characters"));
    }
    match truncated {
        Truncation::Whole => {}
        Truncation::Cut => notes.push("already cut short by the producer".to_string()),
        // Not silence, and not `false`: a flag this build cannot read leaves
        // whether the output is whole unanswered, and a line that said nothing
        // would be answering it.
        Truncation::Unreadable => {
            notes.push("the producer's truncation flag is unreadable".to_string());
        }
    }
    if !notes.is_empty() {
        text.push_str(&format!(" … [{}]", notes.join("; ")));
    }
    text
}

/// One control-stripped line, so a relayed value cannot rewrite the rendering
/// around it.
///
/// Shared with the settlements this crate *composes* out of a sibling's values —
/// `crate::lifecycle` names the ref a publication compared against — so the rule
/// is one rule rather than one per place a sibling's text reaches a line.
pub(crate) fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// `onepipeline goals` — what each run is for, and how far it has got.
pub fn goals(survey: &Survey) -> String {
    let mut out = String::new();
    for view in &survey.views {
        let goal = view
            .state
            .plan
            .as_ref()
            .and_then(|plan| plan.goal.as_ref())
            .map(|goal| goal.text.clone())
            .unwrap_or_else(|| crate::plan::NO_GOAL.to_string());
        out.push_str(&format!(
            "{}  {}\n  {}\n  {}\n",
            view.paths.run,
            liveness_word(view),
            goal,
            view.summary()
        ));
        // The repository identities this run holds, so two planners can see
        // whether they would share a checkout.
        let mut repos: Vec<&str> = view
            .state
            .graph
            .iter()
            .filter_map(|node| node.repo.as_deref())
            .collect();
        repos.sort_unstable();
        repos.dedup();
        if !repos.is_empty() {
            out.push_str(&format!("  identities: {}\n", repos.join(", ")));
        }
    }
    if out.is_empty() {
        return nothing_to_report(survey);
    }
    out.push_str(&skipped_lines(&survey.skipped));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, Labels, ENVELOPE_VERSION};
    use crate::filter::Filters;
    use crate::plan::{Node, Plan, PLAN_SCHEMA_VERSION};
    use serde_json::json;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("onepipeline-views-{name}-{}", sys::pid()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch root");
        dir
    }

    fn plan() -> Plan {
        Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            goal: Some(crate::plan::Goal {
                text: "close the coverage gap".into(),
            }),
            name: Some("demo".into()),
            concurrency: 4,
            tasks: vec![Node {
                id: "build".into(),
                persona: Some("engineer".into()),
                task: Some("## What\ndo it".into()),
                ..Node::default()
            }],
        }
    }

    fn launch(pid: u32) -> LaunchRecord {
        LaunchRecord {
            run_id: "demo".into(),
            project: "plans:demo".into(),
            dir: PathBuf::from("/tmp/launch"),
            graph: "graphs/dag-scope.yaml".into(),
            graph_run: String::new(),
            node_graph: String::new(),
            pr_author_graph: String::new(),
            node_validator: String::new(),
            envelope_reviewer: String::new(),
            launcher: "claude-code".into(),
            session: "session-a".into(),
            pid,
            host: sys::hostname(),
            started: sys::process_start_token(pid)
                .map(|token| token.recorded().to_string())
                .unwrap_or_default(),
            started_at: sys::now_rfc3339(),
            heartbeat_interval: 1_800,
            dag_sets: Vec::new(),
            node_sets: Vec::new(),
            adoptions: 0,
            filters: Filters::default(),
        }
    }

    fn write_run(root: &Path, run: &str, pid: u32, events: &[Envelope]) -> RunPaths {
        let paths = RunPaths::under(root, run);
        paths.create().expect("the run directory");
        let mut record = launch(pid);
        record.run_id = run.to_string();
        ledger::write_json(&paths.launch(), &record).expect("a launch record");
        for event in events {
            ledger::append_line(
                &paths.journal(),
                &serde_json::to_string(event).expect("an event"),
            )
            .expect("appended");
        }
        paths
    }

    /// One entry in a run's dispatch registry, as the executor writes it.
    ///
    /// Written by hand rather than through `claim_dispatch` because what these
    /// exercise is a reader meeting a record another process left behind — a pid
    /// that has gone, one on another machine, one a reused pid now answers for —
    /// and none of those is a state a claim this process takes can be in.
    fn register(paths: &RunPaths, record: &ledger::DispatchRecord) {
        std::fs::create_dir_all(paths.dispatches()).expect("the registry directory");
        ledger::write_json(&paths.dispatch(record.pid, 0), record).expect("a registry entry");
    }

    /// The entry a live dispatch of this process leaves: the pid *and* the start
    /// token that says the pid is still that process.
    fn dispatched_here(node: &str) -> ledger::DispatchRecord {
        ledger::DispatchRecord {
            node: node.to_string(),
            pid: sys::pid(),
            host: sys::hostname(),
            dispatched_at: sys::now_rfc3339(),
            started: sys::process_start_token(sys::pid())
                .map(|token| token.recorded().to_string())
                .unwrap_or_default(),
        }
    }

    fn event(
        kind: crate::journal::PipelineKind,
        node: Option<&str>,
        fields: &[(&str, serde_json::Value)],
    ) -> Envelope {
        relayed(
            EventKind(kind.as_str().into()),
            Source::Pipeline,
            node,
            fields,
        )
    }

    /// The same envelope, for a kind a *sibling* produced: those stay wire
    /// strings, which is the half of the merged store this crate does not close.
    fn relayed(
        kind: EventKind,
        source: Source,
        node: Option<&str>,
        fields: &[(&str, serde_json::Value)],
    ) -> Envelope {
        Envelope {
            v: ENVELOPE_VERSION,
            ts: sys::now_rfc3339(),
            stream: "s".into(),
            seq: 0,
            source,
            kind,
            phase: None,
            labels: Labels {
                run_id: Some("demo".into()),
                round: Some(1),
                node: node.map(str::to_string),
                ..Labels::default()
            },
            payload: crate::journal::payload(fields),
            artifacts: Vec::new(),
        }
    }

    fn dead_pid() -> u32 {
        sys::reaped_pid()
    }

    /// The one line a supervisor may not filter out says *what* is waiting.
    ///
    /// A blocking question behind a pile of routine updates is exactly the case
    /// a bare count hid: this host's own history holds a handful of questions
    /// against thousands of `monitor` surfaces, and both rendered as a number.
    /// So the kinds are named, the blocking one leads, and a queue of unrelated
    /// kinds is summarised out loud rather than silently cut.
    #[test]
    fn the_unread_line_names_the_kinds_waiting_and_leads_with_a_blocking_one() {
        let root = scratch("unread-kinds");
        let paths = write_run(
            &root,
            "demo",
            sys::pid(),
            &[event(
                crate::journal::PipelineKind::RunStarted,
                None,
                &[("plan", json!(plan()))],
            )],
        );
        let channel = crate::channel::ChannelState::new(&paths);
        let queue = |kind: &str, blocking: bool| {
            channel
                .push(crate::channel::Surface {
                    id: 0,
                    kind: kind.into(),
                    message: format!("something about {kind}"),
                    source: "proposal".into(),
                    blocking,
                    queued_at: sys::now_millis(),
                    workstream: None,
                })
                .expect("the surface queues");
        };
        for _ in 0..6 {
            queue("monitor", false);
        }
        queue("planner-question", true);
        for kind in ["edit-rejected", "quiet-worker", "check-in", "proposal"] {
            queue(kind, false);
        }

        let view = RunView::open(&paths).expect("the run reads");
        assert_eq!(view.unread_surfaces().0, 11);
        let unread = view.unread();
        // The blocking kind leads; then the rarest, so the common one that
        // buries it never comes first; then the count that stands for the rest.
        assert_eq!(
            unread.phrase(),
            "1 planner-question, 1 check-in, 1 edit-rejected, 1 proposal, and 2 other kind(s)"
        );

        let rendered = runs(&root, false, "session-a");
        assert!(
            rendered.contains("11 planner update(s) waiting (1 planner-question,"),
            "{rendered}"
        );
        assert!(
            status(&Survey::of(&root)).contains("1 planner-question,"),
            "{}",
            status(&Survey::of(&root))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A skip with no cause to name still says the node was never attempted.
    ///
    /// The empty list is unreachable from a plan this crate executes, so the
    /// phrase has no journey of its own — and it is held here rather than left
    /// untested, because what it guards against is `results` printing a bare
    /// `skipped by:` that a reader takes for a view that lost the fact.
    #[test]
    fn a_skip_with_no_cause_left_in_the_graph_is_still_phrased() {
        assert_eq!(
            skipped_by_phrase(&[]),
            "a dependency this run can no longer name"
        );
        assert_eq!(
            skipped_by_phrase(&[
                ("build".to_string(), NodeStatus::Failed),
                ("lint".to_string(), NodeStatus::Skipped),
            ]),
            "build (failed), lint (skipped)"
        );
    }

    #[test]
    fn a_driver_this_host_can_prove_is_gone_reads_as_driver_dead() {
        let root = scratch("dead");
        write_run(
            &root,
            "demo",
            dead_pid(),
            &[event(
                crate::journal::PipelineKind::RunStarted,
                None,
                &[("plan", json!(plan()))],
            )],
        );
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");
        assert_eq!(view.liveness(), DriverLiveness::DriverDead);
        assert!(view.liveness().is_undriven());
        assert!(runs(&root, false, "session-a").contains("DRIVER DEAD"));
        std::fs::remove_dir_all(&root).ok();
    }

    /// A run this quiet is parked — unless a decision is outstanding.
    ///
    /// Both halves of the same setup, so the difference between them is only the
    /// blocking surface: a live pid, and a last write old enough that silence
    /// alone would park it.
    fn quiet_run(root: &Path, run: &str) -> RunPaths {
        let mut stale = event(
            crate::journal::PipelineKind::RunStarted,
            None,
            &[("plan", json!(plan()))],
        );
        // Far older than `DEFAULT_PARKED_AFTER_SECONDS`, so the verdict does not
        // depend on the threshold's environment override.
        stale.ts = "2020-01-01T00:00:00Z".into();
        write_run(root, run, sys::pid(), &[stale])
    }

    #[test]
    fn a_live_driver_that_has_gone_quiet_with_nothing_outstanding_reads_as_parked() {
        let root = scratch("quiet-parked");
        let paths = quiet_run(&root, "demo");
        let view = RunView::open(&paths).expect("the run reads");
        assert_eq!(view.liveness(), DriverLiveness::Parked);
        std::fs::remove_dir_all(&root).ok();
    }

    /// The same silence, with a blocking surface nobody has answered.
    ///
    /// A decision point takes two forms, and `settlement_of` already reports this
    /// run `awaiting-planner`. A liveness verdict that read only the graph's human
    /// actions called the very same run `PARKED` — inviting an `adopt` that may
    /// end its driver while the answer sits unread in a planner's queue.
    #[test]
    fn a_live_driver_quiet_behind_a_blocking_surface_reads_as_active() {
        let root = scratch("quiet-blocking");
        let paths = quiet_run(&root, "demo");
        crate::channel::ChannelState::new(&paths)
            .push(crate::channel::Surface {
                id: 0,
                kind: "blocker".into(),
                message: "Node build needs a decision; proceed?".into(),
                source: "monitor".into(),
                blocking: true,
                queued_at: sys::now_millis(),
                workstream: Some("build".into()),
            })
            .expect("the surface queues");

        let view = RunView::open(&paths).expect("the run reads");
        assert_eq!(view.liveness(), DriverLiveness::Driving);
        assert!(!view.liveness().is_undriven());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_live_driver_that_is_writing_reads_as_active() {
        let root = scratch("live");
        write_run(
            &root,
            "demo",
            sys::pid(),
            &[event(
                crate::journal::PipelineKind::RunStarted,
                None,
                &[("plan", json!(plan()))],
            )],
        );
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");
        assert_eq!(view.liveness(), DriverLiveness::Driving);
        assert!(!view.liveness().is_undriven());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_pid_recorded_on_another_host_never_reads_as_dead() {
        let root = scratch("elsewhere");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let mut record = launch(dead_pid());
        record.host = "some-other-host".into();
        ledger::write_json(&paths.launch(), &record).expect("a launch record");
        ledger::append_line(
            &paths.journal(),
            &serde_json::to_string(&event(
                crate::journal::PipelineKind::RunStarted,
                None,
                &[("plan", json!(plan()))],
            ))
            .expect("an event"),
        )
        .expect("appended");

        let view = RunView::open(&paths).expect("the run reads");
        assert_eq!(
            view.liveness(),
            DriverLiveness::Driving,
            "a pid means nothing across machines"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_run_nobody_recorded_is_no_such_run() {
        let root = scratch("missing");
        let error = RunView::open(&RunPaths::under(&root, "nowhere")).unwrap_err();
        assert!(matches!(error, crate::Error::NoSuchRun { .. }));
        assert!(runs(&root, false, "session-a").contains("no runs recorded"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_the_reader_sees_mine_and_a_foreign_run_is_labelled_by_digest() {
        let root = scratch("owner");
        write_run(
            &root,
            "demo",
            sys::pid(),
            &[event(
                crate::journal::PipelineKind::RunStarted,
                None,
                &[("plan", json!(plan()))],
            )],
        );
        let listing = runs(&root, false, "session-a");
        assert!(listing.contains("[mine]"), "{listing}");

        let foreign = runs(&root, false, "session-b");
        assert!(!foreign.contains("[mine]"), "{foreign}");
        assert!(
            !foreign.contains("session-a"),
            "{foreign} leaks the session id"
        );
        assert!(runs(&root, true, "session-b").contains("no runs recorded"));
        std::fs::remove_dir_all(&root).ok();
    }

    /// A directory under the runs root that this build will not read is a
    /// **rejection**, and every whole-root view names it beside the runs that
    /// did read.
    ///
    /// The reading it replaces: the same root listed one run and said nothing
    /// about the other directory, so a planner could not tell a root holding one
    /// run from a root holding one run and one it could not open.
    ///
    /// **Unreadable** is what is named here, and it is not the same thing as
    /// unfamiliar: a launch record carrying a key this build does not know is
    /// read and listed, which is the run beside them below.
    #[test]
    fn a_run_root_this_build_refuses_is_named_rather_than_dropped() {
        let root = scratch("skipped");
        write_run(
            &root,
            "readable",
            sys::pid(),
            &[event(
                crate::journal::PipelineKind::RunStarted,
                None,
                &[("plan", json!(plan()))],
            )],
        );
        // A run whose launch record another build wrote, carrying a key this one
        // has never had. It reads, and the run is on the view.
        let newer = write_run(&root, "from-a-newer-build", sys::pid(), &[]);
        let mut written: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(newer.launch()).expect("the launch record this build wrote"),
        )
        .expect("a launch record");
        written["channel_id"] = json!("a field a later build removed");
        std::fs::write(newer.launch(), written.to_string()).expect("a launch record");
        std::fs::create_dir_all(root.join("no-launch")).expect("a directory with no launch");
        // And one whose launch record is not a launch record: a document with
        // none of what this build needs to say anything about the run.
        let empty = RunPaths::under(&root, "not-a-record");
        empty.create().expect("the run directory");
        std::fs::write(empty.launch(), json!({"oops": true}).to_string())
            .expect("a launch record this build cannot read");

        let survey = Survey::of(&root);
        assert_eq!(survey.views.len(), 2, "{:?}", survey.skipped);
        assert_eq!(survey.skipped.len(), 2, "{:?}", survey.skipped);

        for rendered in [
            runs(&root, false, "session-a"),
            status(&survey),
            goals(&survey),
        ] {
            assert!(rendered.contains("readable"), "{rendered}");
            assert!(rendered.contains("from-a-newer-build"), "{rendered}");
        }
        // `host` lists dispatches rather than runs, so the runs it read are not
        // on it — the roots it could not read still are.
        for rendered in [
            runs(&root, false, "session-a"),
            status(&survey),
            goals(&survey),
            host(&survey),
        ] {
            assert!(rendered.contains("2 run root(s) skipped"), "{rendered}");
            assert!(rendered.contains("no-launch"), "{rendered}");
            assert!(rendered.contains("launch.json"), "{rendered}");
            // What the record was missing, as the schema named it: a refusal
            // that only counted the roots would leave a reader nothing to act
            // on.
            assert!(rendered.contains("run_id"), "{rendered}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// A root whose every run was refused is not a root with nothing in it, and
    /// the two must not read alike: one means nothing is running and the other
    /// means this build cannot see what is.
    #[test]
    fn a_root_whose_every_run_is_refused_does_not_read_as_no_runs_recorded() {
        let root = scratch("all-refused");
        std::fs::create_dir_all(root.join("no-launch")).expect("a directory with no launch");

        let survey = Survey::of(&root);
        assert!(survey.views.is_empty());
        for rendered in [
            runs(&root, false, "session-a"),
            status(&survey),
            goals(&survey),
        ] {
            assert!(
                !rendered.contains("no runs recorded"),
                "a rejected root reported as an absence: {rendered}"
            );
            assert!(rendered.contains("no run under"), "{rendered}");
            assert!(rendered.contains("1 run root(s) skipped"), "{rendered}");
            assert!(rendered.contains("no-launch"), "{rendered}");
        }

        // An empty root is still the other fact, and still says so.
        let empty = scratch("all-refused-empty");
        assert_eq!(runs(&empty, false, "session-a"), "no runs recorded\n");
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&empty).ok();
    }

    /// A dispatch row nothing is driving is not a live dispatch.
    ///
    /// Measured on a real host: six rows aged 12h–52h rendered as a live fleet
    /// while nothing matching them was running. The row is dropped and counted
    /// rather than rendered, because an operator acts on this list — and the
    /// action it invites for work that does not exist is the one that ends work
    /// that does.
    #[test]
    fn a_host_row_whose_driver_is_gone_is_counted_rather_than_rendered_live() {
        let root = scratch("host-stale");
        let paths = write_run(
            &root,
            "ghosted",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
                event(
                    crate::journal::PipelineKind::NodeDispatched,
                    Some("build"),
                    &[],
                ),
            ],
        );
        // The registry entry a dispatch that died left behind: its pid is one
        // this host can prove is gone.
        register(
            &paths,
            &ledger::DispatchRecord {
                pid: dead_pid(),
                started: "a token from the process that died".into(),
                ..dispatched_here("build")
            },
        );

        let rendered = host(&Survey::of(&root));
        assert!(
            !rendered.contains("ghosted               "),
            "a dispatch nothing is driving was rendered as a live row: {rendered}"
        );
        assert!(rendered.contains("no live dispatches"), "{rendered}");
        assert!(
            rendered.contains("1 stale registry entry ignored"),
            "{rendered}"
        );
        assert!(rendered.contains("ghosted/build"), "{rendered}");
        assert!(rendered.contains("is gone"), "{rendered}");
        // And the scope of the claim is on the output, because the scan cannot
        // see a run recorded under another root.
        assert!(rendered.contains(&root.display().to_string()), "{rendered}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The same run with a live process actually running the dispatch: the row
    /// renders, and it renders as live.
    #[test]
    fn a_host_row_backed_by_a_live_registry_entry_renders_as_a_live_dispatch() {
        let root = scratch("host-live");
        let paths = write_run(
            &root,
            "driven",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
                event(
                    crate::journal::PipelineKind::NodeDispatched,
                    Some("build"),
                    &[],
                ),
            ],
        );
        register(&paths, &dispatched_here("build"));

        let rendered = host(&Survey::of(&root));
        assert!(rendered.contains("driven"), "{rendered}");
        assert!(rendered.contains("build"), "{rendered}");
        assert!(!rendered.contains("no live dispatches"), "{rendered}");
        assert!(!rendered.contains("stale registry"), "{rendered}");
        assert!(!rendered.contains("UNPROVEN"), "{rendered}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// A lock this host cannot check the pid against is neither proof. The row
    /// stays visible — dropping a dispatch that may be running is the other
    /// error — and it says outright that nothing backs it.
    #[test]
    fn a_host_row_this_host_cannot_prove_either_way_says_so_rather_than_reading_live() {
        let root = scratch("host-unproven");
        let paths = write_run(
            &root,
            "elsewhere",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
                event(
                    crate::journal::PipelineKind::NodeDispatched,
                    Some("build"),
                    &[],
                ),
            ],
        );
        register(
            &paths,
            &ledger::DispatchRecord {
                host: "some-other-host".into(),
                ..dispatched_here("build")
            },
        );

        let rendered = host(&Survey::of(&root));
        assert!(rendered.contains("elsewhere"), "{rendered}");
        assert!(rendered.contains("UNPROVEN"), "{rendered}");
        assert!(rendered.contains("some-other-host"), "{rendered}");
        assert!(!rendered.contains("stale registry"), "{rendered}");

        // An entry from a build that predates the stamp is the same answer for a
        // different reason: nothing says the pid is still the process the
        // dispatch was recorded in, and the registry refuses the whole read
        // rather than hand back a row nobody may act on.
        register(
            &paths,
            &ledger::DispatchRecord {
                started: String::new(),
                ..dispatched_here("build")
            },
        );
        let rendered = host(&Survey::of(&root));
        assert!(rendered.contains("UNPROVEN"), "{rendered}");
        assert!(rendered.contains("no start token"), "{rendered}");

        // And a live pid whose start token disagrees with the one recorded is a
        // *different* process wearing a reused pid: proved stale.
        register(
            &paths,
            &ledger::DispatchRecord {
                started: "the process it was recorded in, which was not this one".into(),
                ..dispatched_here("build")
            },
        );
        let rendered = host(&Survey::of(&root));
        assert!(
            rendered.contains("1 stale registry entry ignored"),
            "{rendered}"
        );
        assert!(rendered.contains("different process"), "{rendered}");

        // And with nothing in the registry at all, the run says a dispatch is in
        // flight and no process claims it — which is neither proof.
        std::fs::remove_dir_all(paths.dispatches()).expect("the registry is taken away");
        std::fs::create_dir_all(paths.dispatches()).expect("an empty registry");
        let rendered = host(&Survey::of(&root));
        assert!(rendered.contains("UNPROVEN"), "{rendered}");
        assert!(rendered.contains("holds no entry for it"), "{rendered}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// One advance a member's chain published, as the producer publishes it.
    fn advanced(role: Option<&str>, turn: Option<u64>, identity: &str, reason: &str) -> Envelope {
        advanced_for("worker", role, turn, identity, reason)
    }

    /// The same, for a named member: a dispatch runs more than one, and each
    /// numbers its own turns.
    fn advanced_for(
        member: &str,
        role: Option<&str>,
        turn: Option<u64>,
        identity: &str,
        reason: &str,
    ) -> Envelope {
        let mut fields = vec![("identity", json!(identity)), ("reason", json!(reason))];
        if let Some(role) = role {
            fields.push(("role", json!(role)));
        }
        if let Some(turn) = turn {
            fields.push(("turn", json!(turn)));
        }
        let mut envelope = relayed(
            EventKind("fallback-advanced".into()),
            Source::Agentgraph,
            Some("build"),
            &fields,
        );
        envelope.stream = "oneagentgraph-1".into();
        envelope.labels.extra.insert("member".into(), member.into());
        envelope
    }

    /// One invocation that ran, built through the producing library's own
    /// payload type so what is folded is what that library publishes.
    fn invocation(role: oneagentgraph::event::Role, turn: u64, identity: &str) -> Envelope {
        invocation_for("worker", role, turn, identity)
    }

    /// The same, for a named member.
    fn invocation_for(
        member: &str,
        role: oneagentgraph::event::Role,
        turn: u64,
        identity: &str,
    ) -> Envelope {
        let session = oneagentgraph::event::OneharnessSession {
            role,
            turn,
            identity: identity.to_string(),
            session_id: None,
            history_id: "record-1".into(),
            history_dir: "/store".into(),
            history_project: "project".into(),
            history_session: "record-1".into(),
        };
        let mut envelope = relayed(
            EventKind("oneharness-session".into()),
            Source::Agentgraph,
            Some("build"),
            &[],
        );
        envelope.stream = "oneagentgraph-1".into();
        envelope.payload = match serde_json::to_value(&session) {
            Ok(serde_json::Value::Object(payload)) => payload,
            other => panic!("a session is not an object: {other:?}"),
        };
        envelope.labels.extra.insert("member".into(), member.into());
        envelope
    }

    /// What a dispatch that died says about where its work is, in each of the
    /// three shapes a settlement of that kind comes in.
    ///
    /// The e2e journeys drive the two ends of this — a dispatch that left a
    /// branch and a commit, and one that left neither — because those are the two
    /// a run can actually reach. The middle one is a branch `onevcs` recorded no
    /// commit for, which is a settlement rather than a scenario: it is what a
    /// dispatch that opened a session and committed nothing leaves, and the
    /// sentence has to stay true of it. The fourth is a record an **older build**
    /// wrote, carrying the word and no classification, which no run this build
    /// drives can produce at all.
    #[test]
    fn a_dispatch_that_died_says_where_its_work_is_in_every_shape_a_settlement_has() {
        let root = scratch("died");
        let died = |node: &str, fields: &[(&str, serde_json::Value)]| {
            let mut all = vec![
                ("status", json!("failed")),
                ("outcome", json!(crate::engine::DISPATCH_DIED)),
            ];
            all.extend(fields.iter().cloned());
            event(crate::journal::PipelineKind::NodeSettled, Some(node), &all)
        };
        let mut plan = plan();
        for id in ["branchless", "uncommitted", "unclassified"] {
            plan.tasks.push(Node {
                id: id.into(),
                task: Some("## What\ndo it".into()),
                ..Node::default()
            });
        }
        write_run(
            &root,
            "died",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan))],
                ),
                died(
                    "build",
                    &[
                        ("cause", json!("rate_limit")),
                        ("branch", json!("b/one")),
                        ("head", json!("abc123")),
                    ],
                ),
                died("branchless", &[("cause", json!("spawn-error"))]),
                died(
                    "uncommitted",
                    &[("cause", json!("auth")), ("branch", json!("b/two"))],
                ),
                died("unclassified", &[("branch", json!("b/three"))]),
            ],
        );

        let survey = Survey::of(&root);
        let rendered = results(&survey.views[0]);
        for said in [
            "(rate_limit) rather than failing its task; b/one may carry finished work, at abc123",
            "(spawn-error) rather than failing its task; it left no branch",
            "(auth) rather than failing its task; b/two may carry finished work",
            // No classification at all: the word still says what happened, and
            // nothing here invents a reason the producer did not give.
            "the dispatch died rather than failing its task; b/three may carry finished work",
        ] {
            assert!(rendered.contains(said), "{said:?} is not in:{rendered}");
        }
        // And the same sentences reach the view a supervisor decides from.
        let standing = status(&survey);
        assert!(
            standing.contains("the dispatch died (rate_limit) rather than failing its task"),
            "{standing}"
        );
    }

    /// A node that failed says which chain **ran out** and which merely fell
    /// through and was served — and never the second under the first's word.
    ///
    /// Both sides, because they are the point: a two-party member runs one chain
    /// per side and they prefer different identities, so a fix aimed at the wrong
    /// one changes nothing and the run fails the same way again. And a fix aimed
    /// at a chain that recovered changes nothing at all.
    #[test]
    fn a_failed_node_tells_a_recovered_chain_from_one_that_ran_out() {
        let root = scratch("refusal");
        write_run(
            &root,
            "refused",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
                event(
                    crate::journal::PipelineKind::NodeDispatched,
                    Some("build"),
                    &[],
                ),
                advanced(Some("agent"), Some(1), "claude-code", "quota"),
                // The agent side's turn went on to run under the next candidate,
                // so its chain recovered and nothing about it failed this node.
                invocation(
                    oneagentgraph::event::Role::Agent,
                    1,
                    "claude-code:alternate",
                ),
                advanced(Some("judge"), Some(1), "codex", "rate_limit"),
                // The same side refusing the same way again is one fact
                // recorded twice, not two facts.
                advanced(Some("judge"), Some(1), "codex", "rate_limit"),
                event(
                    crate::journal::PipelineKind::NodeSettled,
                    Some("build"),
                    &[
                        ("status", json!("failed")),
                        ("outcome", json!("task-failed")),
                    ],
                ),
            ],
        );

        let survey = Survey::of(&root);
        let rendered = results(&survey.views[0]);
        assert!(
            rendered.contains(
                "fallback: the agent side fell through 'claude-code' (quota) → served by \
                 'claude-code:alternate'"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "provider: the judge side: identity 'codex' refused (rate_limit), recorded 2 times"
            ),
            "{rendered}"
        );
        assert!(
            !rendered.contains("provider: the agent side"),
            "a recovered chain was reported as a refusal:\n{rendered}"
        );

        // The same attribution on the view a planner reads first.
        let rendered = status(&survey);
        assert!(
            rendered.contains("build: failed — the judge side: identity 'codex' refused"),
            "{rendered}"
        );
        assert!(
            rendered.contains("build: fallback — the agent side fell through 'claude-code'"),
            "{rendered}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A record that names no side is rendered without one, and a chain that
    /// named no identity is not rendered at all: an attribution nobody can act
    /// on is what this whole line exists to replace.
    #[test]
    fn an_unattributed_refusal_is_never_given_a_side_it_did_not_carry() {
        let advance = |reason: &str| oneagentgraph::event::FallbackAdvanced {
            identity: "codex".into(),
            reason: reason.into(),
            role: None,
            turn: None,
        };
        let single = Refusal {
            advanced: advance("auth"),
            member: MemberLabel::Named("worker".into()),
            records: std::num::NonZeroU64::MIN,
        };
        // A record carrying neither side nor turn has nothing to pair with, so
        // it is neither a recovery nor a refusal — and saying "refused" over it
        // would be naming a subscription that was never the problem.
        assert_eq!(
            chain_phrase(&ChainRecord {
                refusal: &single,
                became: Fallthrough::Unrecorded,
                records: std::num::NonZeroU64::MIN,
            }),
            "member 'worker' fell through 'codex' (auth); nothing this run recorded names what \
             served that turn"
        );
        let bare = Refusal {
            advanced: advance(""),
            member: MemberLabel::Unstamped,
            records: std::num::NonZeroU64::MIN,
        };
        let phrase = chain_phrase(&ChainRecord {
            refusal: &bare,
            became: Fallthrough::Refused,
            records: std::num::NonZeroU64::MIN,
        });
        assert!(
            phrase.contains("a side the record does not name"),
            "{phrase}"
        );
        assert!(
            phrase.contains("for a reason the record does not carry"),
            "{phrase}"
        );
        let unreadable = Refusal {
            advanced: advance("auth"),
            member: MemberLabel::Unreadable,
            records: std::num::NonZeroU64::MIN,
        };
        let phrase = chain_phrase(&ChainRecord {
            refusal: &unreadable,
            became: Fallthrough::Served("codex:alternate".into()),
            records: std::num::NonZeroU64::new(2).expect("two records"),
        });
        assert_eq!(
            phrase,
            "a side this build cannot read fell through 'codex' (auth) → served by \
             'codex:alternate', recorded 2 times"
        );

        // An advance carrying no identity names nothing to act on. It is not an
        // advance the producing library's own type accepts, so nothing here
        // assembles an attribution out of what is left of it.
        let mut nameless = relayed(
            EventKind("fallback-advanced".into()),
            Source::Agentgraph,
            Some("build"),
            &[("reason", json!("quota"))],
        );
        nameless.stream = "oneagentgraph-1".into();
        assert!(projection::fold(&[nameless]).refusals.is_empty());
    }

    /// One chain, two turns, two endings: the recovered turn and the one that
    /// ran out are two facts, and each is rendered as itself.
    ///
    /// The fold keeps them apart *by turn* for exactly this — a record that had
    /// collapsed them could only ever be rendered as one of the two, and which
    /// one it picked would decide where a reader went.
    #[test]
    fn one_chain_that_recovers_and_then_runs_out_says_both() {
        let state = projection::fold(&[
            advanced(Some("agent"), Some(1), "claude-code", "quota"),
            invocation(
                oneagentgraph::event::Role::Agent,
                1,
                "claude-code:alternate",
            ),
            // A second turn that ended the same way. Two records of one fact, so
            // they are counted rather than repeated — the collapsing the fold
            // cannot do, because it does not yet know how either ended.
            advanced(Some("agent"), Some(2), "claude-code", "quota"),
            invocation(
                oneagentgraph::event::Role::Agent,
                2,
                "claude-code:alternate",
            ),
            // And a third that ran out of candidates, which is a different fact
            // about the same chain and says so on its own line.
            advanced(Some("agent"), Some(3), "claude-code", "quota"),
        ]);
        let records = chain_records(&state, "build");
        let phrases = records.iter().map(chain_phrase).collect::<Vec<_>>();
        assert_eq!(
            phrases,
            vec![
                "the agent side fell through 'claude-code' (quota) → served by \
                 'claude-code:alternate', recorded 2 times"
                    .to_string(),
                "the agent side: identity 'claude-code' refused (quota)".to_string(),
            ]
        );

        // An invocation of the *other* side, or of another member, never answers
        // for this one: each numbers its own turns, so pairing across either
        // would name an identity that served somebody else's chain. A dispatch
        // runs more than one member, and the double every journey here drives
        // labels every envelope with the one it runs — so the second member is
        // stated at this level, where a record can carry the label the producer
        // stamps on a member of its own.
        for crossing in [
            invocation(oneagentgraph::event::Role::Agent, 1, "claude-code"),
            invocation_for("reviewer", oneagentgraph::event::Role::Judge, 1, "codex-2"),
        ] {
            let crossed =
                projection::fold(&[advanced(Some("judge"), Some(1), "codex", "quota"), crossing]);
            assert_eq!(
                chain_records(&crossed, "build")
                    .iter()
                    .map(chain_phrase)
                    .collect::<Vec<_>>(),
                vec!["the judge side: identity 'codex' refused (quota)".to_string()]
            );
        }

        // And the member's *own* invocation still answers for it, so the
        // isolation above is a boundary rather than a chain nothing can pair.
        let paired = projection::fold(&[
            advanced_for("reviewer", Some("judge"), Some(1), "codex", "quota"),
            invocation_for("reviewer", oneagentgraph::event::Role::Judge, 1, "codex-2"),
        ]);
        assert_eq!(
            chain_records(&paired, "build")
                .iter()
                .map(chain_phrase)
                .collect::<Vec<_>>(),
            vec!["the judge side fell through 'codex' (quota) → served by 'codex-2'".to_string()]
        );
    }

    /// A judge verdict that failed a node is rendered from the settlement's own
    /// inline copy, so the reason reaches a reader without a file being opened.
    #[test]
    fn a_verdict_that_failed_a_node_names_its_criterion_and_its_reason() {
        let root = scratch("verdict");
        let settled = |verdicts: serde_json::Value| {
            let mut envelope = relayed(
                EventKind("member-settled".into()),
                Source::Agentgraph,
                Some("build"),
                &[("completed", json!(false)), ("verdict", verdicts)],
            );
            envelope.stream = "oneagentgraph-1".into();
            envelope
        };
        write_run(
            &root,
            "verdict",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
                event(
                    crate::journal::PipelineKind::NodeDispatched,
                    Some("build"),
                    &[],
                ),
                settled(json!([
                    // Passed, so it failed nothing and is not the reason.
                    {"criterion": "the branch is pushed", "kind": "boolean",
                     "verdict": {"value": true, "reason": "it is"}},
                    {"criterion": "the change builds", "kind": "boolean",
                     "verdict": {"value": false, "reason": "cargo build fails in src/views.rs"}},
                    // Numeric, which onejudge reports and fails nothing over.
                    {"criterion": "how readable it is", "kind": "numeric",
                     "verdict": {"value": 2.0, "reason": "dense"}},
                    // A record that names neither half. It still says the node
                    // failed on its judge, which is the fact a provider line
                    // above it would otherwise be read as — and an empty string
                    // is a criterion nobody wrote, not one worth a bare pair of
                    // quotes on a line.
                    {"criterion": "", "kind": "boolean", "verdict": {"value": false}},
                    // Not one of the producing library's verdicts at all: it
                    // names no kind. Dropped whole rather than mined for the
                    // fields it does carry — a sentence lifted out of a record
                    // this build cannot read would be attributed to a criterion
                    // nobody scored.
                    {"criterion": "the tests pass",
                     "verdict": {"value": false, "reason": "the suite is red"}},
                ])),
                event(
                    crate::journal::PipelineKind::NodeSettled,
                    Some("build"),
                    &[
                        ("status", json!("failed")),
                        ("outcome", json!("task-failed")),
                    ],
                ),
            ],
        );

        let rendered = results(&Survey::of(&root).views[0]);
        assert!(
            rendered.contains(
                "verdict: 'the change builds' failed — cargo build fails in src/views.rs"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "verdict: a criterion the record does not name failed — the record carries no \
                 reason"
            ),
            "{rendered}"
        );
        for absent in [
            "the branch is pushed",
            "how readable it is",
            "the suite is red",
        ] {
            assert!(
                !rendered.contains(absent),
                "a verdict that failed nothing, or that this build cannot read, was named as \
                 the failure:\n{rendered}"
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// The advice for a run nothing is driving names the nodes a judge
    /// **rejected**, and no other node that failed.
    ///
    /// Two records make a rejection, and this drives the run either one alone
    /// would misname: a node that failed its own task with no judge verdict
    /// against it is not a node a judge rejected, and reported as one it would
    /// send a planner to read a verdict nobody wrote. The journey the operator
    /// took is in `tests/e2e/views.rs`; what is held here is the discrimination.
    #[test]
    fn only_a_node_a_judge_rejected_is_named_as_one() {
        let root = scratch("rejected-advice");
        let failing = |node: &str, verdicts: Option<serde_json::Value>| {
            let mut events = vec![event(
                crate::journal::PipelineKind::NodeDispatched,
                Some(node),
                &[],
            )];
            if let Some(verdicts) = verdicts {
                let mut settled = relayed(
                    EventKind("member-settled".into()),
                    Source::Agentgraph,
                    Some(node),
                    &[("completed", json!(false)), ("verdict", verdicts)],
                );
                settled.stream = "oneagentgraph-1".into();
                events.push(settled);
            }
            events.push(event(
                crate::journal::PipelineKind::NodeSettled,
                Some(node),
                &[
                    ("status", json!("failed")),
                    ("outcome", json!(crate::engine::TASK_FAILED)),
                ],
            ));
            events
        };
        let rejection = json!([
            {"criterion": "the change builds", "kind": "boolean",
             "verdict": {"value": false, "reason": "cargo build fails"}},
        ]);
        let held_up = Plan {
            tasks: vec![
                Node {
                    id: "build".into(),
                    ..Node::default()
                },
                Node {
                    id: "later".into(),
                    deps: vec!["build".into()],
                    ..Node::default()
                },
            ],
            ..plan()
        };
        for (run, verdicts) in [("judged", Some(rejection)), ("brokeoff", None)] {
            let mut events = vec![event(
                crate::journal::PipelineKind::RunStarted,
                None,
                &[("plan", json!(held_up))],
            )];
            events.extend(failing("build", verdicts));
            write_run(&root, run, dead_pid(), &events);
        }

        let listing = runs(&root, false, "session-a");
        let status_of = |run: &str| {
            let paths = RunPaths::under(&root, run);
            status(&Survey {
                root: root.clone(),
                views: vec![RunView::open(&paths).expect("the run reads back")],
                skipped: Vec::new(),
            })
        };
        let judged = status_of("judged");
        for rendered in [&listing, &judged] {
            assert!(
                rendered.contains("build, whose work a judge rejected"),
                "the rejected node is not named as one a judge rejected:\n{rendered}"
            );
            assert!(
                rendered.contains("onepipeline results judged"),
                "the verdict a planner has to read is not named:\n{rendered}"
            );
            assert!(
                rendered.contains("superseding the node"),
                "the step that moves the run is not named:\n{rendered}"
            );
        }
        assert!(
            !judged.contains("adopt"),
            "a driver is prescribed for a frontier it cannot move:\n{judged}"
        );
        let broke = status_of("brokeoff");
        assert!(
            !broke.contains("a judge rejected"),
            "a node that failed its own task was reported as judged:\n{broke}"
        );
        assert!(
            !listing.contains("onepipeline results brokeoff"),
            "a run nothing judged was given the judgement's advice:\n{listing}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// Every value relayed from a sibling's record reaches a rendered line
    /// through the same strip: an identity that served a turn is a stranger's
    /// string exactly as the one that refused is.
    #[test]
    fn a_relayed_value_never_carries_a_control_character_onto_a_line() {
        let refusal = Refusal {
            advanced: oneagentgraph::event::FallbackAdvanced {
                identity: "codex".into(),
                reason: "quota".into(),
                role: Some(oneagentgraph::event::Role::Agent),
                turn: Some(1),
            },
            member: MemberLabel::Named("worker".into()),
            records: std::num::NonZeroU64::MIN,
        };
        let phrase = chain_phrase(&ChainRecord {
            refusal: &refusal,
            became: Fallthrough::Served("codex\r\nprovider: forged".into()),
            records: std::num::NonZeroU64::MIN,
        });
        assert!(!phrase.contains('\n') && !phrase.contains('\r'), "{phrase}");
        let phrase = verdict_phrase(&crate::report::FailedVerdict {
            criterion: Some("it builds".into()),
            reason: Some("no\nit does not".into()),
        });
        assert!(!phrase.contains('\n'), "{phrase}");
    }

    #[test]
    fn every_view_renders_from_the_merged_stream() {
        let root = scratch("render");
        let mut agent = relayed(
            EventKind("turn-finished".into()),
            Source::Agentgraph,
            Some("build"),
            &[("message", json!("ran the gate"))],
        );
        agent.stream = "oneagentgraph-1".into();
        let mut vcs = relayed(
            EventKind("session-opened".into()),
            Source::Vcs,
            Some("build"),
            &[("branch", json!("feature"))],
        );
        vcs.stream = "onevcs-tok".into();

        write_run(
            &root,
            "demo",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
                event(crate::journal::PipelineKind::NodeReady, Some("build"), &[]),
                event(
                    crate::journal::PipelineKind::NodeDispatched,
                    Some("build"),
                    &[],
                ),
                agent,
                vcs,
            ],
        );
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");

        let stream = monitor(&view, &EventFilter::default());
        assert!(stream.starts_with("Concise graph events;"), "{stream}");
        assert!(stream.contains("agent:oneagentgraph-1"), "{stream}");
        assert!(stream.contains("vcs:onevcs-tok"), "{stream}");
        assert!(stream.contains("graph:build"), "{stream}");
        // The run's own state has no node, so it has no typed id: it reaches the
        // reader as a trailer, naming the run it belongs to.
        assert!(stream.contains("-- demo  0/1 done"), "{stream}");
        assert!(
            !stream.contains("round"),
            "a round reached a view: {stream}"
        );

        // The dispatch's own registry entry is what makes it a live one to the
        // host view.
        let paths = RunPaths::under(&root, "demo");
        register(&paths, &dispatched_here("build"));
        let survey = Survey::of(&root);
        assert!(status(&survey).contains("build: running"));
        assert!(host(&survey).contains("build"));
        assert!(goals(&survey).contains("close the coverage gap"));
        assert!(results(&survey.views[0]).contains("build"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_node_the_ledger_calls_running_that_nothing_drives_is_undriven() {
        let root = scratch("undriven");
        write_run(
            &root,
            "demo",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
                event(
                    crate::journal::PipelineKind::NodeDispatched,
                    Some("build"),
                    &[],
                ),
            ],
        );
        let rendered = status(&Survey::of(&root));
        assert!(rendered.contains("UNDRIVEN"), "{rendered}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// What the line says while a node is in flight, which is the whole of what
    /// an operator has to decide between cancel, retry, and wait on.
    #[test]
    fn a_live_dispatch_reports_what_it_is_doing_now_with_a_count_and_an_age() {
        let root = scratch("activity");
        let mut turn = relayed(
            EventKind("turn-activity".into()),
            Source::Agentgraph,
            Some("build"),
            &[
                ("kind", json!("tool_call")),
                ("name", json!("Bash")),
                ("detail", json!("cargo llvm-cov --workspace")),
            ],
        );
        turn.stream = "oneagentgraph-1".into();
        write_run(
            &root,
            "demo",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
                event(
                    crate::journal::PipelineKind::NodeDispatched,
                    Some("build"),
                    &[],
                ),
                turn,
            ],
        );
        let rendered = status(&Survey::of(&root));
        assert!(
            rendered.contains("now Bash cargo llvm-cov --workspace"),
            "{rendered}"
        );
        assert!(rendered.contains("1 event(s)"), "{rendered}");
        assert!(rendered.contains("ago"), "{rendered}");
        assert!(
            !rendered.contains(DriverLiveness::Undriven.as_str()),
            "a dispatch that is recording was reported as driving nothing: {rendered}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// One node's progress record, built through the transitions the fold builds
    /// it through rather than assembled: `events` arrivals, the last of them at
    /// `last_at`.
    ///
    /// Started from nothing and advanced one arrival at a time, exactly as
    /// `fold_activity` advances it — so no arrivals is no record, which is the
    /// answer the fold gives too.
    fn recorded(events: u64, last_at: u64) -> Option<crate::projection::Progress> {
        (0..events).fold(None, |progress, _| match progress {
            None => crate::projection::Progress::first(Some(last_at)),
            Some(progress) => Some(progress.and(Some(last_at))),
        })
    }

    /// A dispatch that has recorded something without naming a tool claims the
    /// count and the age and nothing more.
    #[test]
    fn a_dispatch_that_has_named_no_tool_reports_its_count_rather_than_a_guess() {
        let rendered = working(&crate::projection::NodeActivity {
            doing: None,
            progress: recorded(3, sys::now_millis()),
            last_heartbeat_at: None,
        });
        assert_eq!(rendered, "3 event(s), 0s ago");
        assert!(!rendered.contains("now"), "{rendered}");
    }

    /// A dispatch heartbeating over work it did long ago reports both, and the
    /// age it reports is the work's.
    #[test]
    fn a_heartbeat_is_reported_beside_the_age_of_the_work_rather_than_as_work() {
        let now = sys::now_millis();
        let rendered = working(&crate::projection::NodeActivity {
            doing: Some("Bash red-green.sh".into()),
            progress: recorded(4, now - 600_000),
            last_heartbeat_at: Some(now),
        });
        assert!(
            rendered.contains("4 event(s), 10m00s ago"),
            "the age of the work was taken from the heartbeat: {rendered}"
        );
        assert!(
            rendered.contains("alive 0s ago"),
            "a dispatch that is alive and doing nothing is not reported as alive: {rendered}"
        );
    }

    /// A dispatch that has only ever heartbeated has produced nothing, and says
    /// so — rather than claiming an age for work it has not done, and rather
    /// than reading as a node nothing is driving.
    #[test]
    fn a_dispatch_that_has_only_heartbeated_reports_no_work_and_still_reads_as_alive() {
        let rendered = working(&crate::projection::NodeActivity {
            doing: None,
            progress: None,
            last_heartbeat_at: Some(sys::now_millis()),
        });
        assert_eq!(rendered, "nothing recorded yet; alive 0s ago");
    }

    /// Both halves of a transcript: the tools the store carries as the turn
    /// runs, and the words out of the report the member settled with.
    #[test]
    fn a_transcript_renders_the_turns_tools_and_the_report_it_settled_with() {
        let root = scratch("transcript");
        let paths = RunPaths::under(&root, "demo");
        // This run's own copy, at the name ingest gives it: the reader derives
        // that name from the settlement rather than following the path on it.
        let stored = paths.report_for("s", 0);
        std::fs::create_dir_all(paths.reports_dir()).expect("the run's report storage");
        std::fs::write(
            &stored,
            json!({
                "schema_version": 7,
                "transcript": {"messages": [
                    {"role": "assistant", "content": "Ran the gate.\nIt passed.", "events": [
                        {"kind": "tool_call", "name": "bash", "input": {"command": "just check"}},
                    ]},
                ]},
            })
            .to_string(),
        )
        .expect("a stored report");

        let mut started = relayed(
            EventKind("turn-started".into()),
            Source::Agentgraph,
            Some("build"),
            &[("turn", json!(1))],
        );
        started.stream = "oneagentgraph-1".into();
        let mut activity = relayed(
            EventKind("turn-activity".into()),
            Source::Agentgraph,
            Some("build"),
            &[
                ("kind", json!("tool_call")),
                ("name", json!("bash")),
                ("detail", json!("just check")),
            ],
        );
        activity.stream = "oneagentgraph-1".into();
        let settled = relayed(
            EventKind(crate::report::MEMBER_SETTLED.into()),
            Source::Agentgraph,
            Some("build"),
            &[(crate::report::REPORT_PATH, json!("/elsewhere/report.json"))],
        );

        write_run(
            &root,
            "demo",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
                started,
                activity,
                settled,
            ],
        );
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");
        let rendered = transcript(&view, None);
        assert!(rendered.contains("demo  build"), "{rendered}");
        assert!(rendered.contains("turn 1"), "{rendered}");
        assert!(
            rendered.contains("tool_call bash  just check"),
            "{rendered}"
        );
        assert!(rendered.contains("assistant"), "{rendered}");
        assert!(rendered.contains("Ran the gate."), "{rendered}");
        assert!(rendered.contains("It passed."), "{rendered}");

        // Scoped to a node that dispatched nothing, there is nothing to render.
        assert!(transcript(&view, Some("elsewhere")).contains("no dispatch"));
        std::fs::remove_dir_all(&root).ok();
    }

    /// Two records off a run this crate drove on 2026-08-22, verbatim: a
    /// `tool_call` and the `tool_result` that answered it.
    ///
    /// Recorded rather than composed here, because the payload under test is a
    /// producer's shape and not this crate's. A fixture written beside the
    /// renderer proves the renderer agrees with the fixture — which is exactly
    /// what the defect below had, since a `tool_result` carries its text under
    /// `output` and carries no `detail` at all.
    const RECORDED_ACTIVITY: &str = include_str!("../tests/recorded/turn-activity.jsonl");
    /// The `member-settled` a real dispatch relayed, and the report this run
    /// kept its own copy of. Same run, same seq: the reader derives the copy's
    /// name from the settlement.
    const RECORDED_SETTLEMENT: &str = include_str!("../tests/recorded/member-settled.json");
    const RECORDED_REPORT: &str = include_str!("../tests/recorded/settled-report.json");

    /// The evidence a run recorded, read back off the verb an operator is sent
    /// to when a settlement's evidence looks missing.
    ///
    /// The third column used to be `detail` for every activity, and a
    /// `tool_result`'s `detail` is the empty string — so every observation a
    /// dispatch made rendered blank while its own journal carried the whole
    /// text. A node was failed for want of a measurement its journal held.
    #[test]
    fn a_recorded_tool_results_own_output_is_what_the_transcript_renders() {
        let root = scratch("transcript-recorded-journal");
        let mut events = vec![event(
            crate::journal::PipelineKind::RunStarted,
            None,
            &[("plan", json!(plan()))],
        )];
        events.extend(RECORDED_ACTIVITY.lines().map(|line| {
            serde_json::from_str::<Envelope>(line).expect("a recorded envelope reads back")
        }));
        write_run(&root, "demo", sys::pid(), &events);
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");
        let rendered = transcript(&view, None);

        assert!(
            rendered.contains("tool_call Bash  gh issue view 28"),
            "{rendered}"
        );
        assert!(
            rendered.contains("**Accepted fix.** Four parts:"),
            "the recorded output is not on the line that answered for it:\n{rendered}"
        );
        // Not a prefix of it: the end of what the producer recorded is there
        // too, so a reader is not looking at a silently shortened output.
        assert!(
            rendered.contains("the gate then passed without it."),
            "the recorded output was cut before its end:\n{rendered}"
        );
        // And the producer had already cut it short, which the line says rather
        // than leaving a reader to conclude the tool returned nothing further.
        assert!(
            rendered.contains("… [already cut short by the producer]"),
            "an output the producer marked truncated is rendered as a whole \
             one:\n{rendered}"
        );
        assert!(
            !rendered.lines().any(|line| line.trim() == "tool_result"),
            "an observation still renders as an empty column:\n{rendered}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The same evidence out of the other source this verb reads: the report a
    /// settled member left behind, which is what a reader gets once the dispatch
    /// is over.
    ///
    /// A retained report's outputs are the harness's raw bytes and nothing
    /// bounds them, so this is also where the view's own ceiling is proven: the
    /// one output past it is cut, and the line counts out loud how much of it a
    /// reader is looking at.
    #[test]
    fn a_recorded_reports_tool_output_is_rendered_and_bounded() {
        let root = scratch("transcript-recorded-report");
        let settled: Envelope =
            serde_json::from_str(RECORDED_SETTLEMENT.trim()).expect("a recorded settlement");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        std::fs::create_dir_all(paths.reports_dir()).expect("the run's report storage");
        std::fs::write(
            paths.report_for(&settled.stream, settled.seq),
            RECORDED_REPORT,
        )
        .expect("this run's own copy of the report");
        write_run(
            &root,
            "demo",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
                settled,
            ],
        );
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");
        let rendered = transcript(&view, None);

        assert!(
            rendered.contains("tool_result   ///"),
            "the report's first observation renders as an empty column:\n{rendered}"
        );
        assert!(
            rendered.contains("// the second, because every family below it is the"),
            "the report's longest observation is not rendered at all:\n{rendered}"
        );
        assert!(
            rendered.contains("… [4096 of 4350 characters]"),
            "an unbounded output was printed whole, or cut without saying \
             so:\n{rendered}"
        );
        assert!(
            !rendered.contains("Verdict::Reclaim(lease) => reclaim(&mut report"),
            "the ceiling printed the tail of an output past it:\n{rendered}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A `tool_result` a producer recorded with **no output at all**, which is a
    /// shape the store really carries — two of the 525 results one run relayed.
    ///
    /// It renders with an empty third column, and that is the honest answer: the
    /// payload holds no text under any key, so there is nothing to show. Pinned
    /// because the blank line it produces looks exactly like the defect this
    /// renderer was fixed for, and the two have opposite fixes — one is the
    /// reader looking under the wrong key, this one is a producer that recorded
    /// nothing. A reader meeting it should not go looking for text that was
    /// never there, and nobody should invent any.
    const RECORDED_WITHOUT_OUTPUT: &str =
        include_str!("../tests/recorded/turn-activity-no-output.json");

    #[test]
    fn a_recorded_result_carrying_no_output_renders_empty_because_it_is_empty() {
        let root = scratch("transcript-recorded-outputless");
        let recorded: Envelope =
            serde_json::from_str(RECORDED_WITHOUT_OUTPUT.trim()).expect("a recorded envelope");
        // The premise, checked against the record rather than assumed: nothing in
        // this payload carries text, under the key the renderer reads or any
        // other.
        assert_eq!(recorded.payload.get("output"), None, "{recorded:?}");
        assert_eq!(
            recorded
                .payload
                .get("detail")
                .and_then(serde_json::Value::as_str),
            Some(""),
            "{recorded:?}"
        );

        write_run(
            &root,
            "demo",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
                recorded,
            ],
        );
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");
        let rendered = transcript(&view, None);
        assert!(
            rendered.lines().any(|line| line.trim() == "tool_result"),
            "{rendered}"
        );
        assert!(!rendered.contains('…'), "{rendered}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// A stranger's output cannot rewrite the line it is printed on, and the
    /// note about what was left out cannot be forged into looking like the
    /// view's own.
    #[test]
    fn a_control_character_in_an_output_is_stripped_like_every_other_value() {
        let rendered = tool_text(&ToolText::Returned {
            output: "first\r\nsecond\u{1b}[2K".to_string(),
            truncated: Truncation::Whole,
        });
        assert_eq!(rendered, "first  second [2K");
    }

    /// What a producer says about having cut an output short, in each of the
    /// three things it can say — including the one that is neither answer.
    ///
    /// A flag this build cannot read used to be read as `false`, which is a
    /// claim that the output is whole made on behalf of a producer that claimed
    /// nothing of the sort. It is the same claim this view was corrected for
    /// making, so it is said rather than assumed.
    #[test]
    fn a_truncation_flag_this_build_cannot_read_is_said_rather_than_assumed() {
        let text = |flag: Option<serde_json::Value>| {
            tool_text(&ToolText::Returned {
                output: "what it returned".to_string(),
                truncated: Truncation::of(flag.as_ref()),
            })
        };
        assert_eq!(text(None), "what it returned");
        assert_eq!(text(Some(json!(false))), "what it returned");
        assert_eq!(text(Some(json!(null))), "what it returned");
        assert_eq!(
            text(Some(json!(true))),
            "what it returned … [already cut short by the producer]"
        );
        for unreadable in [json!("true"), json!(1), json!({"cut": true})] {
            assert_eq!(
                text(Some(unreadable.clone())),
                "what it returned … [the producer's truncation flag is unreadable]",
                "{unreadable}"
            );
        }
    }

    /// A settlement whose report this run kept no copy of is said to be
    /// unretained, and the path it named is printed and not opened. An absent
    /// transcript and an unread one are different facts.
    #[test]
    fn a_report_this_run_did_not_keep_is_named_as_unretained_and_never_opened() {
        let root = scratch("transcript-unread");
        let settled = relayed(
            EventKind(crate::report::MEMBER_SETTLED.into()),
            Source::Agentgraph,
            Some("build"),
            &[(
                crate::report::REPORT_PATH,
                json!("/nowhere/onepipeline/report.json"),
            )],
        );
        write_run(
            &root,
            "demo",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
                settled,
            ],
        );
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");
        let rendered = transcript(&view, None);
        assert!(rendered.contains("not retained by this run"), "{rendered}");
        assert!(
            rendered.contains("/nowhere/onepipeline/report.json"),
            "the path that was not read is not named: {rendered}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A report that carries no transcript says so, rather than reading as a
    /// dispatch that did nothing.
    #[test]
    fn a_report_without_a_transcript_says_so() {
        let root = scratch("transcript-none");
        let paths = RunPaths::under(&root, "demo");
        std::fs::create_dir_all(paths.reports_dir()).expect("the run's report storage");
        std::fs::write(
            paths.report_for("s", 0),
            json!({"usage": {"input_tokens": 1}}).to_string(),
        )
        .expect("a stored report");
        let settled = relayed(
            EventKind(crate::report::MEMBER_SETTLED.into()),
            Source::Agentgraph,
            Some("build"),
            &[(crate::report::REPORT_PATH, json!("/elsewhere/report.json"))],
        );
        write_run(
            &root,
            "demo",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
                settled,
            ],
        );
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");
        assert!(transcript(&view, None).contains("carries no transcript"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_run_that_dispatched_nothing_has_no_transcript_to_render() {
        let root = scratch("transcript-empty");
        write_run(
            &root,
            "demo",
            sys::pid(),
            &[event(
                crate::journal::PipelineKind::RunStarted,
                None,
                &[("plan", json!(plan()))],
            )],
        );
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");
        assert_eq!(
            transcript(&view, None),
            "no dispatch has recorded a transcript\n"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_waiting_human_reports_its_action_and_what_it_unblocks() {
        let root = scratch("waiting");
        let mut waiting_plan = plan();
        waiting_plan.tasks = vec![
            Node {
                id: "approve".into(),
                kind: crate::plan::NodeKind::Human,
                task: Some("approve the release".into()),
                ..Node::default()
            },
            Node {
                id: "ship".into(),
                persona: Some("engineer".into()),
                task: Some("## What\nship".into()),
                deps: vec!["approve".into()],
                ..Node::default()
            },
        ];
        write_run(
            &root,
            "demo",
            sys::pid(),
            &[
                event(
                    crate::journal::PipelineKind::RunStarted,
                    None,
                    &[("plan", json!(waiting_plan))],
                ),
                event(
                    crate::journal::PipelineKind::NodeSettled,
                    Some("approve"),
                    &[("status", json!("waiting"))],
                ),
            ],
        );
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");
        let rendered = results(&view);
        assert!(rendered.contains("approve the release"), "{rendered}");
        assert!(rendered.contains("unblocks: ship"), "{rendered}");
        assert!(
            rendered.contains("ship") && rendered.contains("blocked"),
            "{rendered}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A manager about to replace an amendment can read the one they are
    /// replacing, from either view.
    ///
    /// `amend` replaces rather than appends, so a ruling that is not readable
    /// *before* the replacement lands is a ruling nobody can weigh against the
    /// one taking its place. Both views, because deciding to replace one is made
    /// from either.
    #[test]
    fn both_views_render_the_amendment_a_node_is_currently_judged_against() {
        let root = scratch("amendment");
        let mut amended = plan();
        amended.tasks[0].amendment =
            Some("The four comment lines are out of scope: leave them.".into());
        write_run(
            &root,
            "demo",
            sys::pid(),
            &[event(
                crate::journal::PipelineKind::RunStarted,
                None,
                &[("plan", json!(amended))],
            )],
        );
        let paths = RunPaths::under(&root, "demo");
        let view = RunView::open(&paths).expect("the run reads");
        let survey = Survey::of(&root);
        for (which, rendered) in [("status", status(&survey)), ("results", results(&view))] {
            assert!(
                rendered.contains("The four comment lines are out of scope: leave them."),
                "`{which}` does not say what `build` is judged against:\n{rendered}"
            );
            assert!(
                rendered.contains("amend"),
                "`{which}` renders the text without naming it an amendment:\n{rendered}"
            );
        }

        // A node carrying none says nothing, so the absence reads as the absence
        // rather than as a blank line somebody has to interpret.
        let plain = scratch("amendment-none");
        write_run(
            &plain,
            "demo",
            sys::pid(),
            &[event(
                crate::journal::PipelineKind::RunStarted,
                None,
                &[("plan", json!(plan()))],
            )],
        );
        let view = RunView::open(&RunPaths::under(&plain, "demo")).expect("the run reads");
        assert!(!results(&view).contains("amendment:"), "{}", results(&view));
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&plain).ok();
    }

    #[test]
    fn a_summary_line_is_capped_and_control_stripped() {
        let long = "x".repeat(500);
        let stripped = summarize(&relayed(
            EventKind("kind".into()),
            Source::Agentgraph,
            None,
            &[("message", json!(format!("a\nb{long}")))],
        ));
        assert!(!stripped.contains('\n'), "{stripped}");
        assert_eq!(stripped.chars().count(), 96);
    }

    #[test]
    fn the_parked_threshold_is_read_from_the_environment_or_defaults() {
        assert!(parked_after_seconds() > 0);
    }

    #[test]
    fn every_liveness_verdict_has_the_word_the_contract_fixes() {
        assert_eq!(DriverLiveness::Driving.as_str(), "ACTIVE");
        assert_eq!(DriverLiveness::DriverDead.as_str(), "DRIVER DEAD");
        assert_eq!(DriverLiveness::Parked.as_str(), "PARKED");
        assert_eq!(DriverLiveness::Undriven.as_str(), "UNDRIVEN");
    }
}
