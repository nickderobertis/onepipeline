//! The post-launch verbs, as typed calls: what the binary does after a run is
//! launched, reachable without the binary.
//!
//! **The CLI is argument parsing over these calls.** One function per verb,
//! each answering the facts behind what `onepipeline <verb>` prints, and beside
//! each a renderer producing exactly the text the binary prints — so an
//! embedding program reads the facts and the binary reads the rendering, off
//! one call. A post-launch behaviour the binary has and one of these lacks is a
//! defect, and `tests/parity.rs` is the gate that holds the two to one another:
//! it drives the compiled binary and the call here over one run and holds
//! stdout and exit code byte for byte.
//!
//! Exit codes do not move. A typed result carries the outcome — a
//! [`Receipt`]'s state, [`Stopped::clean`], a [`WatchOutcome`] — and the binary
//! maps it to the code `docs/contract.md` assigns, through the `exit_code`
//! each result answers. What the binary reads from its environment — the
//! launching session, the runs root, the profile a reader named — is passed in
//! here rather than read inside, so a consumer that is not this binary is not
//! answered out of the binary's environment.
//!
//! The launch verbs — `start`, `plan check` and the hidden retained verbs — are
//! not here: a launch is what makes a run, and these are what a run is reached
//! with afterwards. The one exception is [`drive_run`], the body of the hidden
//! `drive-run` verb, published so an embedding binary can carry a hidden driver
//! verb of its own and be its own detached driver.

// llmlint: ignore-file[invalid_states_unrepresentable] a run id, a launching session and a
// project id are `String`s on every result here for the reason `src/ledger.rs` states of
// the records they are read off: each is a *serialized* field an older build wrote and a
// consumer parses, and `docs/contract.md` names no `RunId`, `Session` or `ProjectId` — the
// signatures it fixes for this module spell `&RunPaths`, `session: &str` and `run: String`,
// which the downstream nodes are written against, so a newtype here would be a public
// vocabulary the contract did not ask for. What is enforced is the boundary that matters:
// `resolved` is where an externally supplied run id is checked before it is joined onto
// anything, and `ledger::owned_by` is the one place ownership is decided. The shapes the
// contract fixes field by field — `Next`, `ChannelQueue`, `StopRequest`, `Stopped` — are
// answered at each, beside the field.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::agentgraph;
use crate::channel::{
    Author, ChannelState, Command, CommandOutcome, QueuedCommands, QueuedReply, Reply, Surface,
    SurfaceKind,
};
use crate::driver::{self, Submitted};
use crate::error::{Error, Result, EXIT_REFUSED, EXIT_SUCCESS};
use crate::event::Envelope;
use crate::filter::EventFilter;
use crate::journal::{self, Journal};
use crate::ledger::{self, RunPaths};
use crate::telemetry::RunTelemetry;
use crate::views::{self, Listing, Projects, RunView, Survey};

pub use crate::agents::{AgentRun, AgentScope, AgentSession, Agents};
pub use crate::driver::{Retained, Settlement};
pub use crate::journal::StopTeardown;
pub use crate::unwatched::{Unwatched, UnwatchedRun};
pub use crate::watch::{
    Ending as WatchEnding, Frame as WatchFrame, Lines as WatchLines, Monitored,
    Outcome as WatchOutcome, Request as WatchRequest,
};

/// The paths for a run that exists under `root`, or a refusal naming the root
/// searched.
///
/// A run id that navigates is refused before it is joined onto anything: it is
/// not a run this root holds, and reporting it as merely missing would leave a
/// caller believing the path they typed was looked for where they meant.
pub(crate) fn resolved(root: &Path, run: &str) -> Result<RunPaths> {
    if !ledger::is_valid_run_id(run) {
        return Err(Error::Invalid(format!(
            "'{run}' is not a run id: a run id names one directory under the runs root, \
             so it may not be a path"
        )));
    }
    let paths = RunPaths::under(root, run);
    if !paths.exists() {
        return Err(Error::NoSuchRun {
            run: run.to_string(),
            root: root.to_path_buf(),
        });
    }
    Ok(paths)
}

/// How a listing lays its runs out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grouping {
    /// By project, newest activity first, under a header line each. The
    /// default.
    Grouped,
    /// One row per run, by run id: `runs --flat`.
    Flat,
}

/// `onepipeline runs [--mine]`: every run under `root`, grouped by project.
///
/// `mine` narrows each group to the runs `session` owns; a group none of whose
/// runs it owns stays, empty, so a consumer can still tell a root holding other
/// sessions' work from one holding nothing — the listing says `no runs recorded`
/// for the first and names what it refused for the second. A bounded read: it
/// opens no run's merged event store for a run whose summary document is
/// current. [`Projects::flat`] is the ungrouped list.
pub fn runs(root: &Path, session: &str, mine: bool) -> Projects {
    let mut projects = Projects::of(&Listing::of(root));
    if mine {
        for group in &mut projects.groups {
            group
                .runs
                .retain(|summary| ledger::owned_by(&summary.session, session));
        }
    }
    projects
}

/// The text `onepipeline runs` prints, laid out either way, for a reader in
/// `session` — which is what decides the ownership marker on each row.
pub fn render_runs(projects: &Projects, grouping: Grouping, session: &str) -> String {
    views::runs_of(projects, grouping, session)
}

/// `onepipeline status [RUN]`: one run's detail, or every run's standing.
///
/// The two are two different reads, deliberately so. A named run is a
/// **detail** read that folds that run's merged store and reports what each of
/// its nodes is doing; no run named is a **listing**, which never folds — it
/// answers the run-level lines out of each run's bounded summary document.
#[allow(
    clippy::large_enum_variant,
    reason = "built once per call and matched on by a consumer, so the size of a folded \
              run costs nothing a box would save; boxing the variant would change the shape \
              of a public enum the contract fixes as `Status::Run(RunStatus)`"
)]
#[derive(Debug)]
pub enum Status {
    /// The run named, folded.
    Run(RunStatus),
    /// Every run under the root, grouped by project.
    Listing(Projects),
}

/// One run's folded detail.
#[derive(Debug)]
pub struct RunStatus {
    /// The run, read once.
    pub view: RunView,
}

/// `onepipeline status [RUN]`.
///
/// # Errors
///
/// A run named that is not under `root`, or one whose store cannot be read.
pub fn status(root: &Path, run: Option<&str>) -> Result<Status> {
    Ok(match run {
        Some(run) => Status::Run(RunStatus {
            view: RunView::open(&resolved(root, run)?)?,
        }),
        None => Status::Listing(Projects::of(&Listing::of(root))),
    })
}

/// The text `onepipeline status [RUN]` prints.
pub fn render_status(status: &Status) -> String {
    match status {
        Status::Run(detail) => views::status_of(&detail.view),
        Status::Listing(projects) => views::status_listed(projects),
    }
}

/// `onepipeline host`: every live dispatch on this host, read off every run
/// under the root.
#[derive(Debug)]
pub struct Host {
    /// Every run the root holds, folded, and the roots that refused.
    pub survey: Survey,
}

/// `onepipeline host`.
pub fn host(root: &Path) -> Host {
    Host {
        survey: Survey::of(root),
    }
}

/// The text `onepipeline host` prints.
pub fn render_host(host: &Host) -> String {
    views::host(&host.survey)
}

/// `onepipeline goals [RUN]`: what each run is for, and how far it has got.
///
/// The goal lines come off each run's projected plan, so the runs are folded;
/// given no run, the grouping says which run goes under which project's header
/// and in what order, and is read off the bounded listing beside the fold.
#[derive(Debug)]
pub struct Goals {
    /// The runs read, folded, and the roots that refused.
    pub survey: Survey,
    /// How a listing groups them — `None` when one run was named.
    pub projects: Option<Projects>,
}

/// `onepipeline goals [RUN]`.
///
/// # Errors
///
/// A run named that is not under `root`, or one whose store cannot be read.
pub fn goals(root: &Path, run: Option<&str>) -> Result<Goals> {
    Ok(match run {
        Some(run) => Goals {
            survey: Survey::of_one(RunView::open(&resolved(root, run)?)?),
            projects: None,
        },
        None => Goals {
            survey: Survey::of(root),
            projects: Some(Projects::of(&Listing::of(root))),
        },
    })
}

/// The text `onepipeline goals [RUN]` prints.
pub fn render_goals(goals: &Goals) -> String {
    match &goals.projects {
        Some(projects) => views::goals_grouped(&goals.survey, projects),
        None => views::goals(&goals.survey),
    }
}

/// `onepipeline results RUN`: per-node outcomes, with each node's own evidence.
#[derive(Debug)]
pub struct Results {
    /// The run, read once.
    pub view: RunView,
}

/// `onepipeline results RUN`.
///
/// # Errors
///
/// A run whose store cannot be read.
pub fn results(paths: &RunPaths) -> Result<Results> {
    Ok(Results {
        view: RunView::open(paths)?,
    })
}

/// The text `onepipeline results RUN` prints.
pub fn render_results(results: &Results) -> String {
    views::results(&results.view)
}

/// `onepipeline transcript RUN [NODE]`: a dispatched turn's tools and reasoning,
/// from the evidence it retained.
#[derive(Debug)]
pub struct Transcript {
    /// The run, read once.
    pub view: RunView,
    /// The one node asked for, or every node with a record.
    pub node: Option<String>,
}

/// `onepipeline transcript RUN [NODE]`.
///
/// # Errors
///
/// A node this run never dispatched is refused rather than answered with an
/// empty transcript: the two read alike, and only one of them means the reader
/// typed a name that is not in this run.
pub fn transcript(paths: &RunPaths, node: Option<&str>) -> Result<Transcript> {
    let view = RunView::open(paths)?;
    if let Some(node) = node {
        if views::nodes_with_agent_records(&view, Some(node)).is_empty() {
            let recorded = views::nodes_with_agent_records(&view, None);
            return Err(Error::Refused(format!(
                "run '{}' has recorded nothing for node '{node}'; it has records for: {}",
                paths.run,
                if recorded.is_empty() {
                    "nothing yet".to_string()
                } else {
                    recorded.join(", ")
                }
            )));
        }
    }
    Ok(Transcript {
        view,
        node: node.map(str::to_owned),
    })
}

/// The text `onepipeline transcript RUN [NODE]` prints.
pub fn render_transcript(transcript: &Transcript) -> String {
    views::transcript(&transcript.view, transcript.node.as_deref())
}

/// `onepipeline agents RUN [NODE]`: every oneharness session a run's launches
/// wrote — every one, or the ones a node's dispatches wrote — read off the run's
/// own pointer file and nothing else.
///
/// One entry per session, grouped by `history_session`, carrying the three
/// fields a reader opens it to its transcript through: `history_dir`,
/// `history_project`, `history_session`. See [`crate::agents`] for what is
/// stamped and why the store is never opened here.
///
/// # Errors
///
/// A pointer file that exists and cannot be read. A run with no pointer file —
/// one an earlier build launched, or one that has dispatched nothing yet — is
/// an empty list rather than an error.
pub fn agents(paths: &RunPaths, scope: AgentScope<'_>) -> Result<Agents> {
    Agents::of_run(paths, scope)
}

/// `onepipeline agents --project PROJECT`: the union of [`agents`] over every
/// run under `root` whose summary names `project`, in the listing's order.
///
/// # Errors
///
/// A project no run under `root` was launched from, naming the ones that were;
/// and a pointer file that exists and cannot be read.
pub fn project_agents(root: &Path, project: &str) -> Result<Agents> {
    let listing = Listing::of(root);
    let mut agents = Agents::default();
    let mut found = false;
    for summary in listing
        .summaries
        .iter()
        .filter(|summary| summary.project == project)
    {
        found = true;
        agents.absorb(
            &RunPaths::under(root, &summary.run_id).oneharness_sessions(),
            AgentScope::Run,
        )?;
    }
    if !found {
        let mut launched: Vec<&str> = listing
            .summaries
            .iter()
            .map(|summary| summary.project.as_str())
            .filter(|project| !project.is_empty())
            .collect();
        launched.sort_unstable();
        launched.dedup();
        return Err(Error::Refused(format!(
            "no run under {} was launched from project '{project}'; runs were launched from: {}",
            root.display(),
            if launched.is_empty() {
                "no project at all".to_string()
            } else {
                launched.join(", ")
            }
        )));
    }
    Ok(agents)
}

/// The text `onepipeline agents` prints.
pub fn render_agents(agents: &Agents) -> String {
    crate::agents::render(agents)
}

/// `onepipeline telemetry [RUN]`: each run's timing and usage, one document per
/// run — the run named, or every run under the root, oldest id first.
///
/// Through [`telemetry::of_run`](crate::telemetry::of_run), the fold over a
/// view the caller holds; this is that fold over the views this reads.
///
/// # Errors
///
/// A run named that is not under `root`, or one whose store cannot be read.
pub fn telemetry(root: &Path, run: Option<&str>) -> Result<Vec<RunTelemetry>> {
    let survey = match run {
        Some(run) => Survey::of_one(RunView::open(&resolved(root, run)?)?),
        None => Survey::of(root),
    };
    Ok(survey
        .views
        .iter()
        .map(|view| crate::telemetry::of_run(&view.paths, &view.events))
        .collect())
}

/// The text `onepipeline telemetry [RUN] [--breakdown]` prints: one JSON line
/// per run, or each run's breakdown.
///
/// # Errors
///
/// A document that cannot be serialised, which none this crate folds is.
pub fn render_telemetry(measured: &[RunTelemetry], breakdown: bool) -> Result<String> {
    let mut out = String::new();
    for telemetry in measured {
        if breakdown {
            out.push_str(&render_telemetry_breakdown(telemetry));
        } else {
            out.push_str(
                &serde_json::to_string(telemetry)
                    .map_err(|e| Error::Invalid(format!("telemetry: {e}")))?,
            );
            out.push('\n');
        }
    }
    Ok(out)
}

/// One run's breakdown, as `telemetry --breakdown` prints it.
pub fn render_telemetry_breakdown(telemetry: &RunTelemetry) -> String {
    crate::telemetry::render_breakdown(telemetry)
}

/// `onepipeline monitor RUN [--cursor C]`: one pass over the merged stream, from
/// the cursor or from the start, shown through `filter`.
///
/// # Errors
///
/// A run whose store cannot be read, or a cursor this run cannot place.
pub fn monitor(paths: &RunPaths, filter: &EventFilter, cursor: Option<&str>) -> Result<Monitored> {
    let view = RunView::open(paths)?;
    crate::watch::monitored(paths, view, filter, cursor)
}

/// The text `onepipeline monitor` prints: the event lines, the run's trailer,
/// and the resume line last.
pub fn render_monitor(monitored: &Monitored) -> String {
    monitored.render()
}

/// `onepipeline watch RUN`: block until the run needs a supervisor, or until
/// the wait runs out, handing every frame to `sink` as it happens.
///
/// It registers the calling process as the run's watcher for the length of the
/// wait and removes the record on a clean return, exactly as the verb does; a
/// sink that refuses a frame ends the wait with that refusal.
///
/// # Errors
///
/// A cursor this run cannot place, a condition it cannot return on, a run that
/// cannot be read, or the sink's own refusal.
pub fn watch(
    paths: &RunPaths,
    request: &WatchRequest,
    sink: &mut dyn FnMut(WatchFrame<'_>) -> Result<()>,
) -> Result<WatchOutcome> {
    crate::watch::watch(paths, request, sink)
}

/// The two lines the binary prints for one frame: the human one on standard
/// error, the machine one on standard output.
///
/// # Errors
///
/// A record that cannot be rendered, which none this crate builds is.
pub fn render_watch_frame(frame: &WatchFrame<'_>) -> Result<WatchLines> {
    WatchLines::of(frame)
}

/// `onepipeline unwatched --session ID`: which of `session`'s runs under `root`
/// has nothing watching it.
///
/// # Errors
///
/// A runs root that exists and cannot be read.
pub fn unwatched(root: &Path, session: &str) -> Result<Unwatched> {
    crate::unwatched::unwatched(root, session)
}

/// The text `onepipeline unwatched` prints on standard output: one line per
/// reported run, and nothing when there is nothing to report. What could not be
/// resolved is [`Unwatched::unresolved`], which the binary prints on standard
/// error.
pub fn render_unwatched(unwatched: &Unwatched) -> String {
    unwatched
        .reported
        .iter()
        .map(UnwatchedRun::line)
        .collect::<Vec<_>>()
        .concat()
}

/// What `onepipeline next` answered.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NextStatus {
    /// Nothing was waiting, and the run has settled.
    Finished,
    /// Nothing was waiting, and the run is still being driven.
    Running,
    /// A surface was claimed, and it is the one on the answer.
    Surface,
}

/// One read of the channel: the surface claimed, if one was, and the run's
/// events shown through the reader's profile.
// llmlint: ignore[invalid_states_unrepresentable] the three fields are the three keys the line has always carried — `{"status", "surface", "events"}` — and the shape the contract fixes as `Next { status, surface, events }`, which the downstream nodes read by name; the one place a `Next` is built is `next` below, where the status and the surface are decided by one `match` on the claim, so no caller assembles a `Surface` status beside no surface.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Next {
    /// Whether a surface was claimed, and if not, whether the run is over.
    pub status: NextStatus,
    /// The surface claimed, consumed by this read.
    pub surface: Option<Surface>,
    /// The run's merged store, shaped through the profile.
    pub events: Vec<Envelope>,
}

/// `onepipeline next RUN` — the channel's only consumer.
///
/// Rendering is not reading: `monitor` shows a pending surface without
/// consuming it, and this is what advances the queue, journals
/// `planner-surfaced`, and restarts the check-in clock of every member the run's
/// observer graph declares resettable.
///
/// # Errors
///
/// A run whose store cannot be read, or a channel that refuses the claim.
pub fn next(paths: &RunPaths, filter: &EventFilter) -> Result<Next> {
    let view = RunView::open(paths)?;
    let events: Vec<Envelope> = views::shaped(&view, filter).into_iter().cloned().collect();
    let channel = ChannelState::new(paths);

    let Some(surface) = channel.claim()? else {
        let settled = view.liveness().is_undriven();
        return Ok(Next {
            status: if settled {
                NextStatus::Finished
            } else {
                NextStatus::Running
            },
            surface: None,
            events,
        });
    };

    let mut journal = Journal::open(paths);
    journal.emit(
        journal::PipelineKind::PlannerSurfaced,
        journal::labels(&paths.run, surface.workstream.as_deref()),
        journal::payload(&[
            ("kind", json!(surface.kind)),
            ("message", json!(surface.message)),
            ("source", json!(surface.source)),
            ("blocking", json!(surface.blocking)),
            // When the text was true, beside the text: a surface is written in
            // the present tense and this record's own stamp is the reading.
            // Divergence 66 is why it is the queued instant and not an age.
            ("queued_at", json!(surface.queued_at)),
        ]),
    )?;

    // Consumption is what restarts the check-in clock — the whole reset
    // contract. Which clocks is the observer graph's own to say: every member it
    // declared `resettable`, and the engine names none. Addressed by the
    // **graph** run's id, which is what the sibling minted and the only id its
    // signals answer to; this run's id names a run `oneagentgraph` has never
    // heard of. A run that launched no observer graph has no clock to restart
    // and nothing to report. A failure to reach the sibling is reported and does
    // not fail the read: the planner has the surface either way.
    if view.launch.observer_graph().is_some() {
        if let Err(error) = agentgraph::recorded_graph_run(&view.launch.graph_run, &paths.run)
            .and_then(|graph_run| agentgraph::reset_resettable(&graph_run))
        {
            eprintln!("onepipeline: could not restart the check-in clock: {error}");
        }
    }

    // The surface is delivered whatever the profile said. A profile shapes the
    // **event view** and nothing else: which surfaces exist, and the unread
    // accounting over them, belong to the channel, so a blocking surface reaches
    // its planner under the narrowest profile a run has.
    Ok(Next {
        status: NextStatus::Surface,
        surface: Some(surface),
        events,
    })
}

/// The line `onepipeline next` prints: one JSON object.
pub fn render_next(next: &Next) -> String {
    json!({"status": next.status, "surface": next.surface, "events": next.events}).to_string()
}

/// The run's channel as it stands: every surface it has raised and where each
/// is, the replies the planner has written, and the edit envelopes on the
/// durable command queue with the reconciler's answers to them.
///
/// A reading, never a record: nothing here consumes, claims or answers. What
/// `next` hands out and what the views count as unread are decided from the same
/// queues this reads.
// llmlint: ignore[invalid_states_unrepresentable] the fields are the channel's own queues as the bus's layout keeps them — the surfaces log, its projection's `waiting` and its pending slot, and the three other logs — read as one snapshot by `channel` below, which is the one place a `ChannelQueue` is built; the shape is the one the planner ruled on for the downstream nodes (`ChannelQueue { surfaces, waiting, held, replies, commands, outcomes }`), and a reading of six logs that folded them into one structure would be a projection of this crate's own beside the bus's, which is what the channel paragraph of the contract forbids.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ChannelQueue {
    /// Every surface the run has raised, in the order it raised them.
    pub surfaces: Vec<Surface>,
    /// The surfaces nobody has read yet, oldest first.
    pub waiting: Vec<Surface>,
    /// Whatever the pending slot holds: the surface a planner consumed and has
    /// not answered, abandoned or not.
    pub held: Option<Surface>,
    /// Every reply the planner has written, in order.
    pub replies: Vec<QueuedReply>,
    /// The edit envelopes the reconciler has not claimed yet.
    pub commands: Vec<QueuedCommands>,
    /// The reconciler's answer to every envelope it has answered, in order.
    pub outcomes: Vec<CommandOutcome>,
}

impl ChannelQueue {
    /// The surface waiting for an answer, if one is: the held surface, unless
    /// nobody is waiting on it any more.
    pub fn pending(&self) -> Option<&Surface> {
        self.held.as_ref().filter(|surface| !surface.abandoned)
    }

    /// The held surface nobody is waiting on any more, if that is what the
    /// slot holds.
    pub fn abandoned(&self) -> Option<&Surface> {
        self.held.as_ref().filter(|surface| surface.abandoned)
    }

    /// The surfaces that have been read and answered: every one raised that is
    /// neither waiting nor held.
    pub fn answered(&self) -> Vec<&Surface> {
        self.surfaces
            .iter()
            .filter(|surface| {
                !self.waiting.iter().any(|waiting| waiting.id == surface.id)
                    && self.held.as_ref().is_none_or(|held| held.id != surface.id)
            })
            .collect()
    }
}

/// `onepipeline channel queue RUN`: read the run's channel.
///
/// # Errors
///
/// A run that is not there.
pub fn channel(paths: &RunPaths) -> Result<ChannelQueue> {
    if !paths.exists() {
        return Err(Error::NoSuchRun {
            run: paths.run.clone(),
            root: paths.dir.parent().unwrap_or(Path::new(".")).to_path_buf(),
        });
    }
    let channel = ChannelState::new(paths);
    let queue = channel.queue();
    Ok(ChannelQueue {
        surfaces: channel.every_surface(),
        waiting: queue.waiting,
        held: queue.pending,
        replies: channel.replies(),
        commands: channel.claimable_commands(),
        outcomes: channel.outcomes(),
    })
}

/// The line `onepipeline channel queue` prints: one JSON object.
///
/// # Errors
///
/// A record that cannot be serialised, which none the channel holds is.
pub fn render_channel(queue: &ChannelQueue) -> Result<String> {
    serde_json::to_string(queue).map_err(|e| Error::Invalid(format!("channel: {e}")))
}

/// What became of an envelope's verdict half.
///
/// **Two variants and not three**: what happens to a verdict that reaches a
/// receipt is not a variable — it was queued, on every path a submission can
/// take — so the only thing left to say is whether the envelope carried one. The
/// other thing that can happen to a verdict is a refusal, which is an error and
/// never a receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerdictHalf {
    /// The envelope carried none, so the receipt names none.
    NotCarried,
    /// Carried, and on the reply queue for whichever reader claims it.
    OnTheQueue,
}

impl VerdictHalf {
    /// The word the receipt writes, and nothing for the half it never carried.
    ///
    /// `delivered` for the same reason `state` has always spelled it so:
    /// **delivery on this channel is acceptance**, a planner writing when it has
    /// something to say with nothing obliged to be listening at that moment. So
    /// the two keys cannot disagree about one half. Which question it answers is
    /// bound as `ChannelState::answer` states and entry 63 of
    /// `docs/contract-divergences.md` records, and which listener then reads it
    /// is not something a receipt written at submission could answer.
    fn word(self) -> Option<&'static str> {
        match self {
            Self::NotCarried => None,
            Self::OnTheQueue => Some("delivered"),
        }
    }
}

/// What one submitted envelope became: one variant per outcome a submission can
/// reach, so the receipt cannot be built out of parts that contradict each other.
///
/// Which words `state` and `commands` spell, and whether there is a channel
/// identifier to name at all, are decided by the same choice rather than carried
/// as three fields a caller assembles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyOutcome {
    /// A commandless verdict, queued for whichever reader the run owes one.
    Answered {
        /// Its id in the channel.
        reply: u64,
        /// Whether the envelope carried a verdict at all: an envelope carrying
        /// neither half is answered here too, and names neither.
        verdict: VerdictHalf,
    },
    /// Every command applied by this process, which took the run's ownership
    /// lock because nothing was driving it — so there is no queue and no
    /// identifier to name.
    AppliedHere {
        /// The verdict half that rode along, if one did.
        verdict: VerdictHalf,
    },
    /// Every command applied by the run's own reconciler, over the durable
    /// queue.
    AppliedByRun {
        /// The envelope's id in the command queue.
        reply: u64,
        /// The verdict half that rode along, if one did.
        verdict: VerdictHalf,
    },
    /// Accepted and durable, and not reconciled within the reply timeout. Still
    /// queued: **not** an instruction to send it again.
    Queued {
        /// The envelope's id in the command queue.
        reply: u64,
        /// The verdict half that rode along, if one did.
        verdict: VerdictHalf,
    },
}

/// `onepipeline reply`'s answer, whose shape entry 64 of
/// `docs/contract-divergences.md` states.
///
/// The receipt is a transport's answer and stays exactly what it was; what
/// rides beside it — what a queued envelope is waiting for, and a landing
/// settled with no release stated for it — is [`advice`](Self::advice), which the
/// binary prints on standard error and a consumer shows however it shows advice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// What the envelope became.
    pub outcome: ReplyOutcome,
    /// What the person who sent it should know beside the receipt, each a
    /// sentence of its own, in the order the binary says them.
    pub advice: Vec<String>,
}

impl Receipt {
    /// The identifier in the channel. The wire spells the local apply's absence
    /// `0`, which is what this receipt has always answered there and is not a
    /// second identifier.
    pub fn reply(&self) -> u64 {
        match self.outcome {
            ReplyOutcome::Answered { reply, .. }
            | ReplyOutcome::AppliedByRun { reply, .. }
            | ReplyOutcome::Queued { reply, .. } => reply,
            ReplyOutcome::AppliedHere { .. } => 0,
        }
    }

    /// The word `state` has spelled since before the two halves were named.
    pub fn state(&self) -> &'static str {
        match self.outcome {
            ReplyOutcome::Answered { .. } => "delivered",
            ReplyOutcome::AppliedHere { .. } | ReplyOutcome::AppliedByRun { .. } => "applied",
            ReplyOutcome::Queued { .. } => "queued",
        }
    }

    /// The same word again where there were commands to have one, under a name
    /// saying whose it is, and nothing where the envelope carried none.
    pub fn commands(&self) -> Option<&'static str> {
        match self.outcome {
            ReplyOutcome::Answered { .. } => None,
            _ => Some(self.state()),
        }
    }

    /// The verdict half, whichever outcome the commands reached.
    pub fn verdict(&self) -> VerdictHalf {
        match self.outcome {
            ReplyOutcome::Answered { verdict, .. }
            | ReplyOutcome::AppliedHere { verdict }
            | ReplyOutcome::AppliedByRun { verdict, .. }
            | ReplyOutcome::Queued { verdict, .. } => verdict,
        }
    }

    /// The status the binary exits with: [`EXIT_SUCCESS`] for every receipt,
    /// a queued envelope included — entry 67 of `docs/contract-divergences.md`
    /// records why the two outcomes share it and are told apart by `state`. A
    /// refusal is an error and never a receipt, and exits [`EXIT_REFUSED`].
    pub const fn exit_code(&self) -> i32 {
        EXIT_SUCCESS
    }
}

/// Written by hand rather than derived, because every key is read off the one
/// variant and a derive would need them stored as fields that could disagree.
/// The advice is not on the wire: the receipt is the record entry 64 states.
impl serde::Serialize for Receipt {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut receipt = serializer.serialize_map(None)?;
        receipt.serialize_entry("reply", &self.reply())?;
        receipt.serialize_entry("state", self.state())?;
        if let Some(verdict) = self.verdict().word() {
            receipt.serialize_entry("verdict", verdict)?;
        }
        if let Some(commands) = self.commands() {
            receipt.serialize_entry("commands", commands)?;
        }
        receipt.end()
    }
}

/// The line `onepipeline reply` and `onepipeline attest` print: the receipt, as
/// entry 64 states it. The advice beside it is the caller's to say.
///
/// # Errors
///
/// A receipt that cannot be serialised, which none is.
pub fn render_receipt(receipt: &Receipt) -> Result<String> {
    serde_json::to_string(receipt).map_err(|e| Error::Invalid(format!("receipt: {e}")))
}

/// `onepipeline reply RUN [--correlation C]`, over the envelope's **bytes**.
///
/// Parsed and validated here, through the whole submission path the verb runs:
/// the declared author and its grants, the completion grant, the node validator
/// and the envelope reviewer the launch retained and the reviewer's bar, a local
/// apply when nothing drives the run and the durable queue with the reply
/// timeout otherwise. A refusal anywhere is an error, exiting [`EXIT_REFUSED`],
/// and never a receipt.
///
/// # Errors
///
/// A malformed envelope — read a second time, leniently, to say when a retired
/// plan field is why — and every refusal the channel, the validators and the
/// reconciler can make.
pub fn reply(
    paths: &RunPaths,
    correlation: Option<&onemessagebus::Correlation>,
    envelope_json: &str,
) -> Result<Receipt> {
    let text = envelope_json.trim();
    // A reply this schema refuses is read a second time, leniently, to see
    // whether a retired plan field is why — an `add` carrying one is the same
    // planner mistake as a task carrying one, and deserves the same answer.
    let envelope: Reply = serde_json::from_str(text).map_err(|e| {
        let why = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .as_ref()
            .and_then(crate::plan::retired_field_refusal)
            .unwrap_or_else(|| e.to_string());
        Error::Refused(format!("the reply is malformed: {why}"))
    })?;
    submit(paths, correlation, &envelope)
}

/// `onepipeline attest RUN REF` — the shorthand for a reply carrying one
/// `attest`.
///
/// # Errors
///
/// Every refusal [`reply`] can make of the envelope this builds.
pub fn attest(paths: &RunPaths, reference: &str) -> Result<Receipt> {
    submit(
        paths,
        None,
        &Reply {
            version: Some(crate::channel::REPLY_ENVELOPE_VERSION),
            // The person who took the action, through the planner's own channel:
            // `attest` is not an op an observer may issue at all.
            author: Author::planner(),
            commands: vec![Command::Attest {
                reference: reference.to_owned(),
            }],
            ..Reply::default()
        },
    )
}

/// Validate a reply, queue it, and report which of the four true things happened.
///
/// The object [`render_receipt`] prints is stated in entry **64** of
/// `docs/contract-divergences.md` and nowhere else;
/// `channel::the_reply_receipt_names_each_half_the_envelope_carried` gates this
/// code against it.
fn submit(
    paths: &RunPaths,
    correlation: Option<&onemessagebus::Correlation>,
    envelope: &Reply,
) -> Result<Receipt> {
    let verdict = if envelope.carries_verdict() {
        VerdictHalf::OnTheQueue
    } else {
        VerdictHalf::NotCarried
    };
    let mut advice = Vec::new();
    let outcome = match driver::submit_envelope(paths, correlation, envelope)? {
        Submitted::Answered { reply } => ReplyOutcome::Answered { reply, verdict },
        // This process applied them itself, so there is no queue and no id in it.
        Submitted::AppliedHere { .. } => ReplyOutcome::AppliedHere { verdict },
        Submitted::AppliedByRun { reply } => ReplyOutcome::AppliedByRun { reply, verdict },
        // llmlint: ignore-block[cli_output_contract] the two outcomes this status shares are
        // told apart on stdout, by the receipt's `state`; the status answers whether the
        // envelope was accepted, which sharing it is the whole point of. Divergence 67.
        Submitted::Queued { reply } => {
            // The status cannot say what is left to happen, so the words do.
            advice.push(format!(
                "onepipeline: the edits are on run '{}'s durable command queue and have \
                 not been reconciled yet, so something has to drive the run for them to \
                 take effect: they are applied by the driver holding it, or by \
                 `onepipeline adopt {}` if nothing is driving it. They are not to be sent \
                 again — a second copy is a second edit.",
                paths.run, paths.run
            ));
            ReplyOutcome::Queued { reply, verdict }
        } // llmlint: ignore-end[cli_output_contract]
    };
    // Beside the receipt rather than in it — the receipt is a transport's answer and
    // stays exactly what it was — and only once the settle has been applied: a
    // landing settled with no release stated for it, which no probe answer will ever
    // release, is said to the person who settled it now rather than discovered later
    // from a dependent that is still waiting.
    if matches!(
        outcome,
        ReplyOutcome::AppliedHere { .. } | ReplyOutcome::AppliedByRun { .. }
    ) && envelope.commands.iter().any(|command| {
        matches!(
            command,
            Command::Settle {
                landing: Some(_),
                release: None,
                ..
            }
        )
    }) {
        // The settle is applied whatever this read finds, so a run that cannot be
        // read back now costs the advice and never the exit status the receipt has.
        // llmlint: ignore-block[changed_behavior_has_e2e] a run whose store cannot be
        // read in the instant after this process applied an edit to it is not a state an
        // invocation can put a run into; the read that succeeds is driven end to end by
        // `tests/e2e/adoption.rs`.
        match RunView::open(paths) {
            Ok(view) => advice.extend(crate::release::hold_warnings_for_stated_landings(
                &view.state,
                &envelope.commands,
            )),
            Err(unread) => advice.push(format!(
                "onepipeline: the settle was applied, and whether its landing has a release \
                 baseline could not be asked, because the run could not be read back: {unread}"
            )),
        } // llmlint: ignore-end[changed_behavior_has_e2e]
    }
    Ok(Receipt { outcome, advice })
}

/// What `onepipeline surface` queued.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Surfaced {
    /// The surface's id in the channel.
    pub surface: u64,
}

/// `onepipeline surface RUN --kind KIND --message TEXT`: raise a surface to the
/// planner.
///
/// What it is about, and what raised it, are two facts: a check-in is the
/// scheduled member's own, and a finding raised here is advice like any other.
/// Neither is a request, so neither blocks a subtree.
///
/// # Errors
///
/// A message with nothing in it once trimmed, and a channel that refuses the
/// push.
pub fn surface(paths: &RunPaths, kind: SurfaceKind, message: String) -> Result<Surfaced> {
    let message = message.trim().to_owned();
    if message.is_empty() {
        return Err(Error::Refused(
            "a surface carries what it has to say and this one carried nothing".to_owned(),
        ));
    }
    let source = if kind.as_str() == SurfaceKind::CHECK_IN {
        crate::channel::source::CHECK_IN
    } else {
        crate::channel::source::PROPOSAL
    };
    let queued = ChannelState::new(paths).push(Surface {
        id: 0,
        kind: kind.as_str().to_string(),
        message,
        source: source.to_string(),
        // Neither is a request: a check-in update and a finding raised at this
        // verb are reports, and never hold a subtree back waiting for a
        // decision. A finding that means to stop one says so through the
        // envelope's `finding` op, which carries `blocking`.
        blocking: false,
        queued_at: crate::sys::now_millis(),
        abandoned: false,
        // Raised by a call that answers its caller and is gone, so there is no
        // listener to name and none to come back: nothing abandons this and
        // nothing adopts it.
        asker: None,
        workstream: None,
        correlation: None,
    })?;
    let mut journal = Journal::open(paths);
    journal.emit(
        journal::PipelineKind::PlannerSurfaceQueued,
        journal::labels(&paths.run, None),
        journal::payload(&[
            ("kind", json!(queued.kind)),
            ("message", json!(queued.message)),
            ("source", json!(queued.source)),
            ("blocking", json!(false)),
        ]),
    )?;
    Ok(Surfaced { surface: queued.id })
}

/// The line `onepipeline surface` prints.
pub fn render_surfaced(surfaced: &Surfaced) -> String {
    json!({"surface": surfaced.surface, "state": "queued"}).to_string()
}

/// Who is stopping a run, and whether they may stop one they do not own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopRequest<'a> {
    /// The session acting. Passed in, never read from the environment here:
    /// the binary reads `ONEPIPELINE_LAUNCHER_SESSION` and passes it.
    pub session: &'a str,
    /// Stop a run another session owns, naming the owner.
    // llmlint: ignore[invalid_states_unrepresentable] the flag the contract fixes for this
    // request — `StopRequest { session, force }`, which `ui-api` and `aio-adopt` are written
    // against — and the one the CLI parses (`--force`); the two answers it has are the two
    // the verb has, and a named enum over them would be that boolean under another name.
    pub force: bool,
}

/// What `onepipeline stop` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stopped {
    /// The run.
    pub run: String,
    /// Who owned it, as the stop journalled it.
    pub owner: String,
    /// Whether the stop overrode another session's ownership.
    pub forced: bool,
    /// What the teardown established, as journalled on `run-stopped`.
    pub teardown: StopTeardown,
    /// Whether the run is stopped: every process it named was reached, or none
    /// was left to reach. A teardown that was not clean is journalled and is
    /// **not** a stop — the run is still running — and [`refusal`](Self::refusal)
    /// says so.
    // llmlint: ignore[invalid_states_unrepresentable] derivable from `teardown`, and the
    // contract fixes it as a field — `Stopped { run, owner, forced, teardown, clean }` is
    // what the downstream nodes read — so it is written from the teardown in the one place a
    // `Stopped` is built, `driver::stop_run`, and nothing else assembles one. `forced` is
    // independent of both: it says whose run was stopped, not how the teardown went.
    pub clean: bool,
}

impl Stopped {
    /// The status the binary exits with: [`EXIT_SUCCESS`] for a clean stop and
    /// [`EXIT_REFUSED`] otherwise.
    pub const fn exit_code(&self) -> i32 {
        if self.clean {
            EXIT_SUCCESS
        } else {
            EXIT_REFUSED
        }
    }

    /// Why this was not a stop, for a teardown that was not clean, in the words
    /// the binary refuses with. `None` for a clean stop.
    ///
    /// Each teardown says something different because it leaves the operator in
    /// a different place: a host that gave no answer is a stop to run again, and
    /// a tree that refused this user's signal is one to end as the user that
    /// owns it.
    pub fn refusal(&self) -> Option<String> {
        let run = &self.run;
        match self.teardown {
            StopTeardown::NotAttempted => Some(format!(
                "run '{run}' was not stopped: this host gave no answer its tree could be \
                 read from — no process listing, or nothing that says whether a pid it \
                 recorded is still the process it named, each said above — so the \
                 processes the run started could not be found, and ending its driver \
                 alone would have orphaned them. The run is untouched — run \
                 `onepipeline stop {run}` again once this host answers"
            )),
            StopTeardown::PartlySignalled => Some(format!(
                "run '{run}' was only partly stopped: part of its process tree was \
                 signalled and at least one process in it is still running — one this \
                 session could not signal, or one that took the ask and stayed. Find it \
                 in this host's process list and end it as the user that owns it"
            )),
            StopTeardown::IdentityDeclined => Some(format!(
                "run '{run}' was not stopped: live processes were found, but every recorded \
                 identity disagreed with the process now holding its pid, so none was safe \
                 to signal. This is distinct from a run with nothing left to stop; inspect \
                 the declined claims above and retry only after correcting the run records"
            )),
            // llmlint: ignore-block[changed_behavior_has_e2e] this arm has no journey and
            // cannot have one: reaching it takes a run every process of which refuses this
            // user's signal, and a process this user may not signal is not a thing for a
            // suite to go and make — the same reason `sys::established` is a fold driven
            // from the answers a round of signalling gives rather than from signals. What the
            // arm is built from is proved there, at
            // `a_teardown_refused_by_everything_it_aimed_at_reports_no_signal_at_all` and
            // `a_stop_that_could_signal_nothing_it_aimed_at_says_so`; every other outcome
            // this match renders is driven end to end in `tests/e2e/driver.rs`.
            StopTeardown::Refused => Some(format!(
                "run '{run}' was not stopped: its process tree was found and every \
                 process in it refused this session's signal, so nothing was signalled \
                 and all of it is still running. Running `onepipeline stop {run}` again \
                 as this user will be refused the same way — find the tree in this \
                 host's process list and end it as the user that owns it"
            )), // llmlint: ignore-end[changed_behavior_has_e2e]
            StopTeardown::Signalled | StopTeardown::NothingToStop | StopTeardown::Elsewhere => None,
        }
    }
}

/// `onepipeline stop RUN [--force]`: end a run and its whole dispatch tree.
///
/// Refuses a run another session owns with [`Error::NotOwned`] unless forced,
/// and a forced stop journals the owner it overrode. Every teardown that reached
/// the run is journalled as `run-stopped`; only a clean one releases what the run
/// claimed and fires the run's stop hook.
///
/// # Errors
///
/// [`Error::NotOwned`] for a run another session owns, and [`Error::Refused`]
/// where this build cannot establish what the run is running — nothing is
/// signalled and nothing is recorded on that path.
pub fn stop(paths: &RunPaths, request: StopRequest<'_>) -> Result<Stopped> {
    driver::stop_run(paths, request.session, request.force)
}

/// The line `onepipeline stop` prints for a clean stop. `teardown` qualifies
/// `stopped`: the ledger record is what stops a run, and it is written either
/// way.
pub fn render_stopped(stopped: &Stopped) -> String {
    json!({
        "run_id": stopped.run,
        "stopped": true,
        "owner": stopped.owner,
        journal::STOP_TEARDOWN: stopped.teardown,
    })
    .to_string()
}

/// The program a detached adoption retains as the run's driver, and its
/// arguments, exactly as they will be spawned — nothing is appended.
///
/// The binary passes itself with `["drive-run", RUN, "--adopt"]`; an embedding
/// program carrying a hidden driver verb of its own passes that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retain {
    /// The executable.
    pub program: PathBuf,
    /// Its arguments, in order.
    pub args: Vec<String>,
}

/// How a run is adopted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Adopt {
    /// This process drives the run until it settles, as `adopt` does.
    Attached,
    /// A retained process drives it, as `adopt --detach` does: validated and
    /// displaced here, then `program args…` spawned in a process group of its
    /// own with stdin null and stdout and stderr on the run's driver log, and
    /// waited for until it has claimed the run.
    Detached(Retain),
}

/// What `onepipeline adopt` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Adopted {
    /// This process drove the run to where it settled.
    Attached {
        /// The run.
        run: String,
        /// How it settled.
        settlement: Settlement,
    },
    /// A retained driver has claimed the run.
    Detached {
        /// The run.
        run: String,
        /// The driver's pid.
        pid: u32,
    },
}

impl Adopted {
    /// The status the binary exits with: the settlement's for an attached
    /// adoption, [`EXIT_SUCCESS`] for a detached one that claimed the run.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Attached { settlement, .. } => settlement.exit_code(),
            Self::Detached { .. } => EXIT_SUCCESS,
        }
    }
}

/// `onepipeline adopt RUN [--attach|--detach]`: attach a fresh driver to a run
/// whose ledger is intact.
///
/// Both refuse for the same reasons and refuse here, before anything is
/// written: a run another session owns ([`Error::NotOwned`]), and a run
/// something is still driving. A run the liveness verdict has called undriven is
/// taken over, ending the parked driver politely first.
///
/// # Errors
///
/// Those refusals; a lock that cannot be taken; and, detached, a retained
/// driver that did not claim the run, with what it said in its log.
pub fn adopt(paths: &RunPaths, how: Adopt) -> Result<Adopted> {
    Ok(match how {
        Adopt::Attached => Adopted::Attached {
            run: paths.run.clone(),
            settlement: driver::adopt_attached(paths, &mut |_| {})?,
        },
        Adopt::Detached(retain) => Adopted::Detached {
            run: paths.run.clone(),
            pid: driver::adopt_detached(paths, &retain)?,
        },
    })
}

/// The line `onepipeline adopt` prints: the settlement of an attached
/// adoption, or the run, the driver's pid and the two verbs that reach it for a
/// detached one — one shape for both detaching verbs, so `--detach` means on
/// `adopt` exactly what it means on `start`.
pub fn render_adopted(adopted: &Adopted) -> String {
    match adopted {
        Adopted::Attached { run, settlement } => settlement.line(run),
        Adopted::Detached { run, pid } => driver::announce_launch(run, *pid),
    }
}

/// `onepipeline drive-run RUN [--adopt]` — the body of the hidden retained
/// driver verb, published so an embedding binary can be its own detached
/// driver.
///
/// The same loop an attached launch runs in-process: it takes the ownership
/// lock, claims the run in the launch record, launches the observer graph the
/// run declares inside its own process tree, and drives the run until it
/// settles, answering the code a driver exits with — `0` for a complete graph
/// and `1` for one that settled unfinished. [`Retained::Adopting`] records the
/// adoption under the lock first. Whoever carries the verb calls
/// `agentgraph::speaks_this_cli` on its own executable first, as the binary's
/// `run` does.
///
/// # Errors
///
/// A lock another driver holds, a ledger that cannot be read, and an observer
/// graph that refuses to start.
pub fn drive_run(paths: &RunPaths, retained: Retained) -> Result<i32> {
    driver::drive_run(paths, retained)
}
