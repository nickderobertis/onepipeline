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

use std::path::{Path, PathBuf};

use crate::event::{Envelope, Source};
use crate::filter::EventFilter;
use crate::graph::{self, Landing, NodeStatus};
use crate::journal::PipelineKind;
use crate::ledger::{self, LaunchRecord, LockRecord, RunPaths};
use crate::projection::{self, MemberLabel, Refusal, RunState};
use crate::sys;

/// A run root a view refused, and the reason it gave.
///
/// Re-exported where the views are, because a rejection is part of what a view
/// reports: a root that was skipped is named on the same output a run is.
pub use crate::ledger::Skipped;

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
        let queue = crate::channel::ChannelState::new(&self.paths).queue();
        let oldest = queue
            .waiting
            .iter()
            .map(|surface| sys::now_millis().saturating_sub(surface.queued_at) / 1_000)
            .max();
        (queue.waiting.len(), oldest)
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
    /// settlement observed, and nothing has looked since — a count that read as
    /// the state of things now would say a merged change had reached nobody.
    /// Divergence 33 in
    /// [the divergence record](../../../docs/contract-divergences.md) is why
    /// nothing can look.
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
        format!("{done}/{} done{unlanded}", statuses.len())
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
/// What a view says about the run roots it refused, or nothing when it refused
/// none.
///
/// Counted **and** named. A count alone tells a reader something is wrong and
/// not which directory to look at, and the reason is what `results` already
/// prints for exactly this refusal.
fn skipped_lines(skipped: &[Skipped]) -> String {
    if skipped.is_empty() {
        return String::new();
    }
    let mut out = format!("{} run root(s) skipped:\n", skipped.len());
    for root in skipped {
        // Every value on the line is a stranger's — a directory name on disk and
        // a refusal built from it — so both go through the same strip.
        out.push_str(&format!(
            "  {}: {}\n",
            one_line(&root.path.display().to_string()),
            one_line(&root.reason)
        ));
    }
    out
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

/// The word a view prints for how a run is being driven.
///
/// A run whose graph completed is **settled**, not abandoned: its driver is
/// gone because there was nothing left for it to do. Reporting `DRIVER DEAD`
/// there would send a planner to intervene in finished work.
pub fn liveness_word(view: &RunView) -> &'static str {
    let statuses = view.state.statuses();
    if !statuses.is_empty() && graph::state_of(&statuses) == graph::GraphState::Complete {
        return "SETTLED";
    }
    view.liveness().as_str()
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
        out.push_str(&format!(
            "{marker} {:<24} {:<24} {}  {}\n",
            view.paths.run,
            view.launch.owner_label(session),
            view.summary(),
            liveness_word(view)
        ));
        // A run reported stopped keeps the line saying why it stopped rather
        // than an invitation to read updates nothing will follow up on.
        if view.liveness().is_undriven() {
            out.push_str(&format!(
                "    {} — its ledger is intact; attach a fresh driver with: \
                 onepipeline adopt {}\n",
                view.liveness().as_str(),
                view.paths.run
            ));
            continue;
        }
        if let (count, Some(stale)) = view.unread_surfaces() {
            if count > 0 {
                out.push_str(&format!(
                    "    {count} planner update(s) waiting, unread for {}; \
                     read them with: onepipeline next {}\n",
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
        out.push_str(&format!(
            "{}  {}  {}\n",
            view.paths.run,
            liveness_word(view),
            view.summary()
        ));
        if view.liveness().is_undriven() {
            out.push_str(&format!(
                "  {}: nothing is driving this run; adopt it or stop it\n",
                view.liveness().as_str()
            ));
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
        let (unread, stale) = view.unread_surfaces();
        if unread > 0 {
            out.push_str(&format!(
                "  {unread} planner update(s) waiting, unread for {}\n",
                crate::telemetry::duration(stale.unwrap_or(0) * 1_000)
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
        // What refused, for the nodes that failed. A failed node otherwise reads
        // the same whether its own gate failed or an identity chain ran out, and
        // the two call for opposite actions from whoever is reading this.
        for (id, node_status) in &statuses {
            if *node_status != NodeStatus::Failed {
                continue;
            }
            for refusal in refusals_of(&view.state, id) {
                out.push_str(&format!("  {id}: failed — {}\n", refusal_phrase(refusal)));
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

/// Every provider refusal one node's dispatches recorded, in arrival order.
fn refusals_of<'a>(state: &'a RunState, node: &str) -> &'a [Refusal] {
    state.refusals.get(node).map_or(&[], Vec::as_slice)
}

/// How one provider refusal reads on a rendered line.
///
/// The side first, because it is the half a reader most often gets wrong: the
/// two sides of a member prefer different identities, and an operator who
/// restored the wrong subscription spent a night watching the same failure.
///
/// Every value on the line is a stranger's — an identity, a classification, a
/// role, and a member name, all read off a sibling's envelope — so the whole
/// phrase goes through the same control strip the rest of this module uses.
fn refusal_phrase(refusal: &Refusal) -> String {
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
    // identity, and reason. The producer stamps a turn on each advance and
    // nothing here reads it, so "on N turns" would be a measurement this line
    // never made.
    let again = if refusal.records.get() > 1 {
        format!(", recorded {} times", refusal.records)
    } else {
        String::new()
    };
    one_line(&format!(
        "{side}: identity '{}' refused {reason}{again}",
        refusal.advanced.identity
    ))
}

/// The nodes whose change had not reached its base when they settled, in id
/// order.
///
/// Read off what each settlement observed — never off a node's repository or its
/// policy — so a node absent from this list is one that either landed or had no
/// change to land, and never one nobody looked at.
///
/// It is deliberately not called what has *not landed now*: nothing here has
/// looked since, and every line rendered from this list says so — see the
/// per-node phrase below, and divergence 33 in
/// [the divergence record](../../../docs/contract-divergences.md) for why
/// nothing can look.
fn unlanded_nodes(view: &RunView) -> Vec<String> {
    view.state
        .landings
        .iter()
        .filter(|(_, landing)| **landing == Landing::Unlanded)
        .map(|(node, _)| node.clone())
        .collect()
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
/// Three facts, because one alone misleads: what it last did, how much it has
/// done, and how long ago. A dispatch that has recorded plenty and nothing
/// recently is the wedged one; a first turn that has run for twenty minutes and
/// is still recording is healthy, and has twice been reported dead for want of
/// this line.
fn working(activity: &crate::projection::NodeActivity) -> String {
    let counted = format!(
        "{} event(s), {} ago",
        activity.events,
        crate::telemetry::duration(
            activity
                .last_at
                .map_or(0, |at| sys::now_millis().saturating_sub(at))
        )
    );
    match &activity.doing {
        Some(doing) => format!("now {doing} ({counted})"),
        // Absent rather than guessed: the dispatch has recorded something and
        // has not named a tool, so the count and the age are the whole of what
        // this line can claim.
        None => counted,
    }
}

/// Whether this host can prove the run behind a recorded dispatch is still being
/// driven.
///
/// Three answers, because a row an operator acts on has to distinguish them.
/// `Stale` and `Live` are proofs in opposite directions; `Unproven` is the
/// answer this host does not have, and collapsing it into either is how a
/// registry row outlives the process it describes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Proof {
    /// The run's lock is held, and the pid holding it started when the record
    /// says it did.
    ///
    /// Deliberately not named for liveness: what this establishes is that the
    /// evidence agrees, to the resolution the host reports a process start at —
    /// one second, where that is `ps`. A pid reused *inside* that resolution by
    /// a process that also started then is the one case this cannot separate,
    /// and it is why the word is about the lock rather than about the work.
    Held,
    /// This host proved nothing is driving the run behind the row.
    Stale(String),
    /// This host cannot decide: the lock was taken elsewhere, cannot be read, or
    /// carries no stamp to check the pid against.
    Unproven(String),
}

/// Whether the dispatches a run records are backed by a driver this host can
/// prove is running them.
///
/// The **ownership lock**, not the launch record: the lock is created by the
/// process that drives the run and removed when it lets go, so its presence is a
/// claim made now rather than one made at launch. Its pid says which process,
/// and its start token says the pid is still that process — a pid alone is what a
/// two-day-old lock has, and a reused one answers a liveness probe as alive.
fn dispatch_proof(view: &RunView) -> Proof {
    if view.state.stop_recorded() {
        return Proof::Stale("the run was stopped".to_string());
    }
    let path = view.paths.lock();
    // Asked for, rather than tested with `is_file`: that helper answers `false`
    // for a lock that is not there *and* for one this host would not describe,
    // and only the first is a proof. Reading the second as absence would turn a
    // question into a verdict that nothing is driving the run.
    match std::fs::metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Proof::Stale(
                "nothing holds the run's ownership lock, so no driver is running it".to_string(),
            )
        }
        // llmlint: ignore-block[changed_behavior_has_e2e] a lock this host will not describe
        // at all is a host condition no portable journey can set; the answer beside it — a
        // lock that is there and is not a file — is driven in `tests/e2e/views.rs`.
        Err(error) => {
            return Proof::Unproven(format!(
                "the run's ownership lock cannot be described: {error}"
            ))
        }
        // llmlint: ignore-end[changed_behavior_has_e2e]
        Ok(about) if !about.is_file() => {
            return Proof::Unproven(
                "the run's ownership lock is not a file, so nothing here holds it".to_string(),
            )
        }
        Ok(_) => {}
    }
    let Some(held) = ledger::read_json_opt::<LockRecord>(&path) else {
        // A claim this build cannot read is still a claim — it is what stops a
        // second writer — but it proves nothing about a *dispatch*, and a row is
        // a claim that one exists.
        return Proof::Unproven("the run's ownership lock cannot be read".to_string());
    };
    if held.host != sys::hostname() {
        return Proof::Unproven(format!(
            "its driver holds the lock on {}, and a pid means nothing across machines",
            held.host
        ));
    }
    if !sys::process_may_be_live(held.pid) {
        return Proof::Stale(format!("its driver (pid {}) is gone", held.pid));
    }
    if held.started.is_empty() {
        return Proof::Unproven(format!(
            "the run's lock carries no start token for pid {}, so nothing says it is still \
             the process that took it",
            held.pid
        ));
    }
    match sys::process_start_token(held.pid) {
        // llmlint: ignore-block[changed_behavior_has_e2e] the host declining to answer is a
        // property of the machine rather than of anything a user types. The answers it does
        // give, and three other unproven arms that resolve alike, are driven end to end.
        None => Proof::Unproven(format!(
            "this host will not say when pid {} started",
            held.pid
        )),
        // llmlint: ignore-end[changed_behavior_has_e2e]
        Some(token) if token.matches(&held.started) => Proof::Held,
        Some(_) => Proof::Stale(format!(
            "pid {} is a different process from the one that took the run's lock",
            held.pid
        )),
    }
}

/// `onepipeline host` — every live dispatch on this host, across every planner.
///
/// A row here is a claim that a dispatch exists **now**, and it is acted on: an
/// operator leaves it alone, or ends it. So a row is rendered as live only where
/// this host can prove the run behind it is still being driven — its ownership
/// lock's pid, and the start token that says the pid is still the process that
/// took it. A row proved to have nothing behind it is dropped and counted, and
/// one this host cannot decide either way is rendered saying so. Never a bare
/// row that reads as live work.
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
        let proof = dispatch_proof(view);
        for (id, status) in &view.state.statuses() {
            if *status != NodeStatus::Running {
                continue;
            }
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
        out.push_str(&format!("{}  {:<28} {}\n", event.ts, id, summarize(event)));
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
/// So the unlanded phrase is dated, and says nothing has looked since. Nothing
/// here *can* look: a change request lives on the repository's host, `onevcs`
/// owns every route to one, and the read that would answer this is not on that
/// library's surface — recorded as a proposal to it in
/// [`docs/contract-divergences.md`](../../../docs/contract-divergences.md).
/// Until it is, a dated claim beside the change's own URL is the honest answer,
/// and asserting the state of things now would not be.
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
             nothing has re-read it since — open the change for where it is now"
        ),
    }
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
        // The one piece of evidence a person actually opens.
        if let Some(url) = view.state.change_urls.get(&node.id) {
            out.push_str(&format!(" — {url}"));
        }
        out.push('\n');
        if let Some(detail) = view
            .events
            .iter()
            .rev()
            .find(|event| {
                event.kind.0 == PipelineKind::NodeSettled.as_str()
                    && event.labels.node.as_deref() == Some(node.id.as_str())
            })
            .and_then(|event| event.payload.get("detail"))
            .and_then(|detail| detail.as_str())
        {
            out.push_str(&format!("      detail: {}\n", one_line(detail)));
        }
        // Which side asked, and which identity refused. A failed node's own
        // detail says what the dispatch reported; this says who would not serve
        // it, which is the fact a retry aimed at the wrong chain does not change.
        if status == NodeStatus::Failed {
            for refusal in refusals_of(&view.state, &node.id) {
                out.push_str(&format!("      provider: {}\n", refusal_phrase(refusal)));
            }
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
                    one_line(field("detail"))
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
                        one_line(&tool.detail)
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

/// One control-stripped line, so a relayed value cannot rewrite the rendering
/// around it.
fn one_line(text: &str) -> String {
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
            plan: PathBuf::from("plan.json"),
            dir: PathBuf::from("/tmp/launch"),
            graph: "graphs/dag-scope.yaml".into(),
            graph_run: String::new(),
            node_graph: String::new(),
            pr_author_graph: String::new(),
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

    /// The lock a driving process leaves behind it, as this process would take
    /// it: the pid *and* the start token that says the pid is still that
    /// process.
    fn hold_lock(paths: &RunPaths) {
        ledger::write_json(
            &paths.lock(),
            &LockRecord {
                pid: sys::pid(),
                host: sys::hostname(),
                acquired_at: sys::now_rfc3339(),
                verb: "drive".into(),
                started: sys::process_start_token(sys::pid())
                    .map(|token| token.recorded().to_string())
                    .unwrap_or_default(),
            },
        )
        .expect("a held lock");
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
        // A directory that records no launch at all.
        std::fs::create_dir_all(root.join("no-launch")).expect("a directory with no launch");
        // And one whose launch record carries a field this build does not accept
        // — the refusal `results` already words, naming the file and the field.
        let typo = RunPaths::under(&root, "typo");
        typo.create().expect("the run directory");
        std::fs::write(typo.launch(), json!({"oops": true}).to_string()).expect("a launch record");

        let survey = Survey::of(&root);
        assert_eq!(survey.views.len(), 1, "{:?}", survey.skipped);
        assert_eq!(survey.skipped.len(), 2, "{:?}", survey.skipped);

        // The run that read is still listed, beside the two that did not.
        for rendered in [
            runs(&root, false, "session-a"),
            status(&survey),
            goals(&survey),
        ] {
            assert!(rendered.contains("readable"), "{rendered}");
        }
        // `host` lists dispatches rather than runs, so the run it read is not on
        // it — the roots it could not read still are.
        for rendered in [
            runs(&root, false, "session-a"),
            status(&survey),
            goals(&survey),
            host(&survey),
        ] {
            assert!(rendered.contains("2 run root(s) skipped"), "{rendered}");
            assert!(rendered.contains("no-launch"), "{rendered}");
            assert!(rendered.contains("launch.json"), "{rendered}");
            // The offending field, as the schema named it.
            assert!(rendered.contains("oops"), "{rendered}");
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
        // The lock a driver that died left behind: its pid is one this host can
        // prove is gone.
        ledger::write_json(
            &paths.lock(),
            &LockRecord {
                pid: dead_pid(),
                host: sys::hostname(),
                acquired_at: sys::now_rfc3339(),
                verb: "drive".into(),
                started: "a token from the process that died".into(),
            },
        )
        .expect("a stale lock");

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

    /// The same run with a driver actually holding it: the row renders, and it
    /// renders as live.
    #[test]
    fn a_host_row_backed_by_a_held_lock_renders_as_a_live_dispatch() {
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
        hold_lock(&paths);

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
        ledger::write_json(
            &paths.lock(),
            &LockRecord {
                pid: sys::pid(),
                host: "some-other-host".into(),
                acquired_at: sys::now_rfc3339(),
                verb: "drive".into(),
                started: String::new(),
            },
        )
        .expect("a lock taken elsewhere");

        let rendered = host(&Survey::of(&root));
        assert!(rendered.contains("elsewhere"), "{rendered}");
        assert!(rendered.contains("UNPROVEN"), "{rendered}");
        assert!(rendered.contains("some-other-host"), "{rendered}");
        assert!(!rendered.contains("stale registry"), "{rendered}");

        // A lock held by a live process on this host that carries no start token
        // is the same answer for a different reason: nothing says the pid is
        // still the process that took it.
        ledger::write_json(
            &paths.lock(),
            &LockRecord {
                pid: sys::pid(),
                host: sys::hostname(),
                acquired_at: sys::now_rfc3339(),
                verb: "drive".into(),
                started: String::new(),
            },
        )
        .expect("a lock from a build that predates the stamp");
        let rendered = host(&Survey::of(&root));
        assert!(rendered.contains("UNPROVEN"), "{rendered}");
        assert!(rendered.contains("no start token"), "{rendered}");

        // And a live pid whose start token disagrees with the one recorded is a
        // *different* process wearing a reused pid: proved stale.
        ledger::write_json(
            &paths.lock(),
            &LockRecord {
                pid: sys::pid(),
                host: sys::hostname(),
                acquired_at: sys::now_rfc3339(),
                verb: "drive".into(),
                started: "the process that took it, which was not this one".into(),
            },
        )
        .expect("a lock a reused pid now answers for");
        let rendered = host(&Survey::of(&root));
        assert!(
            rendered.contains("1 stale registry entry ignored"),
            "{rendered}"
        );
        assert!(rendered.contains("different process"), "{rendered}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// A node that failed because an identity chain ran out says which side
    /// asked and which identity refused.
    ///
    /// Both sides, because they are the point: a two-party member runs one chain
    /// per side and they prefer different identities, so a fix aimed at the wrong
    /// one changes nothing and the run fails the same way again.
    #[test]
    fn a_failed_node_names_the_side_and_the_identity_that_refused() {
        let root = scratch("refusal");
        let refused = |role: Option<&str>, identity: &str, reason: &str| {
            let mut fields = vec![("identity", json!(identity)), ("reason", json!(reason))];
            if let Some(role) = role {
                fields.push(("role", json!(role)));
            }
            let mut envelope = relayed(
                EventKind("fallback-advanced".into()),
                Source::Agentgraph,
                Some("build"),
                &fields,
            );
            envelope.stream = "oneagentgraph-1".into();
            envelope
                .labels
                .extra
                .insert("member".into(), "worker".into());
            envelope
        };
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
                refused(Some("agent"), "claude-code", "quota"),
                refused(Some("judge"), "codex", "rate_limit"),
                // The same side refusing the same way again is one fact
                // recorded twice, not two facts.
                refused(Some("judge"), "codex", "rate_limit"),
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
        assert!(rendered.contains("the agent side"), "{rendered}");
        assert!(rendered.contains("claude-code"), "{rendered}");
        assert!(rendered.contains("(quota)"), "{rendered}");
        assert!(rendered.contains("the judge side"), "{rendered}");
        assert!(rendered.contains("codex"), "{rendered}");
        assert!(rendered.contains("recorded 2 times"), "{rendered}");

        // The same attribution on the view a planner reads first.
        let rendered = status(&survey);
        assert!(rendered.contains("build: failed —"), "{rendered}");
        assert!(rendered.contains("the judge side"), "{rendered}");
        assert!(rendered.contains("codex"), "{rendered}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// A record that names no side is rendered without one, and a chain that
    /// named no identity is not rendered at all: an attribution nobody can act
    /// on is what this whole line exists to replace.
    #[test]
    fn an_unattributed_refusal_is_never_given_a_side_it_did_not_carry() {
        let advanced = |reason: &str| oneagentgraph::event::FallbackAdvanced {
            identity: "codex".into(),
            reason: reason.into(),
            role: None,
            turn: None,
        };
        let single = Refusal {
            advanced: advanced("auth"),
            member: MemberLabel::Named("worker".into()),
            records: std::num::NonZeroU64::MIN,
        };
        assert_eq!(
            refusal_phrase(&single),
            "member 'worker': identity 'codex' refused (auth)"
        );
        let bare = Refusal {
            advanced: advanced(""),
            member: MemberLabel::Unstamped,
            records: std::num::NonZeroU64::MIN,
        };
        let phrase = refusal_phrase(&bare);
        assert!(
            phrase.contains("a side the record does not name"),
            "{phrase}"
        );
        assert!(
            phrase.contains("for a reason the record does not carry"),
            "{phrase}"
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

        // A driver holds the run, which is what makes its dispatch a live one to
        // the host view.
        hold_lock(&RunPaths::under(&root, "demo"));
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

    /// A dispatch that has recorded something without naming a tool claims the
    /// count and the age and nothing more.
    #[test]
    fn a_dispatch_that_has_named_no_tool_reports_its_count_rather_than_a_guess() {
        let rendered = working(&crate::projection::NodeActivity {
            doing: None,
            events: 3,
            last_at: Some(sys::now_millis()),
        });
        assert_eq!(rendered, "3 event(s), 0s ago");
        assert!(!rendered.contains("now"), "{rendered}");
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
