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

use std::path::Path;

use crate::event::{Envelope, Source};
use crate::graph::{self, NodeStatus};
use crate::journal::PipelineKind;
use crate::ledger::{self, LaunchRecord, RunPaths};
use crate::projection::{self, RunState};
use crate::sys;

/// How long a launch may hold its pid without doing anything before it is
/// reported [`Parked`](DriverLiveness::Parked).
///
/// The default planner-update interval: a run that has not written, surfaced,
/// or dispatched for a whole interval is not merely between turns.
pub const DEFAULT_PARKED_AFTER_SECONDS: u64 = 1_800;

/// The environment variable that moves that threshold.
pub const PARKED_AFTER_ENV: &str = "ONEPIPELINE_PARKED_AFTER_SECONDS";

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

/// Whether a run is being driven, and if not, why not.
///
/// Every unreadable input resolves toward "still working", so a busy driver is
/// never misreported: one live process, one fresh surface, or one recent ledger
/// write is enough to keep it reported as running. A pid recorded on another
/// host is exactly such an unknown — a pid means nothing across machines — so a
/// run another driver is holding reads as the live work it is.
pub fn liveness(launch: &LaunchRecord, state: &RunState) -> DriverLiveness {
    if state.stopped {
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
        Some(seconds) if seconds > parked_after_seconds() && !state.round_open => {
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
        // A view resolves cross-DAG edges the same way the round does, so a
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

    /// Every readable run under a root, oldest id first.
    ///
    /// A run this host cannot read at all drops out, having supported no claim
    /// either way.
    pub fn all(root: &Path) -> Vec<Self> {
        ledger::all_runs(root)
            .iter()
            .filter_map(|paths| Self::open(paths).ok())
            .collect()
    }

    /// How the run is being driven.
    pub fn liveness(&self) -> DriverLiveness {
        liveness(&self.launch, &self.state)
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
    pub fn summary(&self) -> String {
        let statuses = self.state.statuses();
        let done = statuses
            .values()
            .filter(|status| **status == NodeStatus::Done)
            .count();
        format!(
            "round-{:02}  ({done}/{} done)",
            self.state.round,
            statuses.len()
        )
    }
}

/// The word a view prints for how a run is being driven.
///
/// A run whose graph completed is **settled**, not abandoned: its driver is
/// gone because there was nothing left for it to do. Reporting `DRIVER DEAD`
/// there would send a planner to intervene in finished work.
pub fn liveness_word(view: &RunView) -> &'static str {
    let statuses = view.state.statuses();
    if !view.state.round_open
        && !statuses.is_empty()
        && graph::state_of(&statuses) == graph::GraphState::Complete
    {
        return "SETTLED";
    }
    view.liveness().as_str()
}

/// `onepipeline runs`.
pub fn runs(root: &Path, mine_only: bool, session: &str) -> String {
    let mut out = String::new();
    for view in RunView::all(root) {
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
            liveness_word(&view)
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
        out.push_str("no runs recorded\n");
    }
    out
}

/// `onepipeline status`.
pub fn status(views: &[RunView]) -> String {
    let mut out = String::new();
    for view in views {
        out.push_str(&format!(
            "{}  {}  round-{:02}\n",
            view.paths.run,
            liveness_word(view),
            view.state.round
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
            out.push_str(&format!(
                "  {id}: running for {}",
                crate::telemetry::duration(age.unwrap_or(0)),
            ));
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
        if let Some(health) = crate::agentgraph::health() {
            out.push_str(&format!("  providers: {health}\n"));
        }
    }
    if out.is_empty() {
        out.push_str("no runs recorded\n");
    }
    out
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

/// `onepipeline host` — every live dispatch on this host, across every planner.
pub fn host(views: &[RunView]) -> String {
    let mut out = format!("host {}\n", sys::hostname());
    let mut any = false;
    for view in views {
        let statuses = view.state.statuses();
        for (id, status) in &statuses {
            if *status != NodeStatus::Running {
                continue;
            }
            any = true;
            let age = view
                .state
                .dispatched_at
                .get(id)
                .map(|at| sys::now_millis().saturating_sub(*at))
                .unwrap_or(0);
            out.push_str(&format!(
                "  {:<24} {:<20} {:<16} {}\n",
                view.paths.run,
                id,
                view.launch.launcher,
                crate::telemetry::duration(age)
            ));
        }
    }
    if !any {
        out.push_str("  no live dispatches\n");
    }
    out
}

/// `onepipeline monitor` — one pass over the merged stream.
///
/// The first line is the contract, not a banner: every event line carries the
/// typed id a detail lookup resolves, and the monitor never tries to *be* the
/// detail.
pub fn monitor(view: &RunView) -> String {
    let mut out = String::from(
        "Concise graph events; ask the producing library for full detail by stream id.\n",
    );
    for event in &view.events {
        let id = match event.source {
            Source::Pipeline => format!("graph:{}", event.labels.node.as_deref().unwrap_or("-")),
            Source::Agentgraph => format!("agent:{}", event.stream),
            Source::Vcs => format!("vcs:{}", event.stream),
        };
        out.push_str(&format!("{}  {:<28} {}\n", event.ts, id, summarize(event)));
    }
    // A round transition has no node, so it has no graph id: it reaches the
    // reader as run state rather than as an event line.
    out.push_str(&format!(
        "-- {}  round-{:02}  {}  {}\n",
        view.paths.run,
        view.state.round,
        liveness_word(view),
        graph::state_of(&view.state.statuses()).as_str()
    ));
    out
}

/// One control-stripped line derived from an event's recorded values.
fn summarize(event: &Envelope) -> String {
    const CAP: usize = 96;
    let mut detail = event.kind.0.clone();
    for key in ["status", "outcome", "state", "message", "reason"] {
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

/// `onepipeline results` — per-node outcomes, with each node's own evidence.
pub fn results(view: &RunView) -> String {
    let mut out = format!("{}  round-{:02}\n", view.paths.run, view.state.round);
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
pub fn goals(views: &[RunView]) -> String {
    let mut out = String::new();
    for view in views {
        let goal = view
            .state
            .plan
            .as_ref()
            .and_then(|plan| plan.goal.as_ref())
            .map(|goal| goal.text.clone())
            .unwrap_or_else(|| "(no goal stated)".to_string());
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
        out.push_str("no runs recorded\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, Labels, ENVELOPE_VERSION};
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
            launcher: "claude-code".into(),
            session: "session-a".into(),
            pid,
            host: sys::hostname(),
            started_at: sys::now_rfc3339(),
            round_budget: 14_400,
            heartbeat_interval: 1_800,
            dag_sets: Vec::new(),
            node_sets: Vec::new(),
            adoptions: 0,
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
                event(
                    crate::journal::PipelineKind::RoundStarted,
                    None,
                    &[("plan", json!(plan()))],
                ),
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

        let stream = monitor(&view);
        assert!(stream.starts_with("Concise graph events;"), "{stream}");
        assert!(stream.contains("agent:oneagentgraph-1"), "{stream}");
        assert!(stream.contains("vcs:onevcs-tok"), "{stream}");
        assert!(stream.contains("graph:build"), "{stream}");
        // A round transition has no node, so it has no typed id: it reaches the
        // reader as run state, naming the run it belongs to.
        assert!(stream.contains("-- demo  round-01"), "{stream}");

        let views = vec![view];
        assert!(status(&views).contains("build: running"));
        assert!(host(&views).contains("build"));
        assert!(goals(&views).contains("close the coverage gap"));
        assert!(results(&views[0]).contains("build"));
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
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");
        let rendered = status(std::slice::from_ref(&view));
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
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");
        let rendered = status(std::slice::from_ref(&view));
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
