//! What the read-only views report.
//!
//! The views are the CLI's `runs`, `status`, `host`, `monitor`, `results`,
//! `goals`, and `telemetry`. Their semantics are ported one-to-one from
//! `ai-orchestrator`, and the one distinction that has cost real supervision
//! time is named here as a type rather than left to prose: whether a run is
//! being driven, and if not, how it stopped being driven.
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
            // A node the ledger records as running that no live dispatch is
            // driving is `UNDRIVEN`. Deliberately not `parked`: that word means
            // the opposite here — a node the planner idled with `cancel`.
            let driving = view.events.iter().any(|event| {
                event.source == Source::Agentgraph && event.labels.node.as_deref() == Some(id)
            });
            let age = view
                .state
                .dispatched_at
                .get(id)
                .map(|at| sys::now_millis().saturating_sub(*at));
            out.push_str(&format!(
                "  {id}: running for {}{}\n",
                crate::telemetry::duration(age.unwrap_or(0)),
                if driving {
                    String::new()
                } else {
                    format!(" — {}", DriverLiveness::Undriven.as_str())
                }
            ));
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
            graph: "graphs/dag-scope.yaml".into(),
            launcher: "claude-code".into(),
            session: "session-a".into(),
            pid,
            host: sys::hostname(),
            started_at: sys::now_rfc3339(),
            round_budget: 14_400,
            heartbeat_interval: 1_800,
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

    fn event(kind: &str, node: Option<&str>, fields: &[(&str, serde_json::Value)]) -> Envelope {
        Envelope {
            v: ENVELOPE_VERSION,
            ts: sys::now_rfc3339(),
            stream: "s".into(),
            seq: 0,
            source: Source::Pipeline,
            kind: EventKind(kind.into()),
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
                crate::journal::RUN_STARTED,
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
                crate::journal::RUN_STARTED,
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
                crate::journal::RUN_STARTED,
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
                crate::journal::RUN_STARTED,
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
        let mut agent = event(
            "turn-finished",
            Some("build"),
            &[("message", json!("ran the gate"))],
        );
        agent.source = Source::Agentgraph;
        agent.stream = "oneagentgraph-1".into();
        let mut vcs = event(
            "session-opened",
            Some("build"),
            &[("branch", json!("feature"))],
        );
        vcs.source = Source::Vcs;
        vcs.stream = "onevcs-tok".into();

        write_run(
            &root,
            "demo",
            sys::pid(),
            &[
                event(
                    crate::journal::RUN_STARTED,
                    None,
                    &[("plan", json!(plan()))],
                ),
                event(
                    crate::journal::ROUND_STARTED,
                    None,
                    &[("plan", json!(plan()))],
                ),
                event(crate::journal::NODE_DISPATCHED, Some("build"), &[]),
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
                    crate::journal::RUN_STARTED,
                    None,
                    &[("plan", json!(plan()))],
                ),
                event(crate::journal::NODE_DISPATCHED, Some("build"), &[]),
            ],
        );
        let view = RunView::open(&RunPaths::under(&root, "demo")).expect("the run reads");
        let rendered = status(std::slice::from_ref(&view));
        assert!(rendered.contains("UNDRIVEN"), "{rendered}");
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
                    crate::journal::RUN_STARTED,
                    None,
                    &[("plan", json!(waiting_plan))],
                ),
                event(
                    crate::journal::NODE_SETTLED,
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
        let stripped = summarize(&event(
            "kind",
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
