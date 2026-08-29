//! The executor seam.
//!
//! An [`Executor`] is *where* a node's dispatch runs. v1 ships [`LocalExecutor`]
//! only — it supports both workspace variants — while the trait and the
//! [rules grammar](crate::rules) are shaped so a dispatch-server executor over a
//! WebSocket, and a Kubernetes one, drop in behind the same interface. That is
//! what decouples where a dispatch runs from the caller that asked for it.
//!
//! Two of the request's fields are a sibling library's types, so this seam is
//! also where the cross-repo wiring is proven at compile time: the agent-graph
//! config comes from `oneagentgraph` and the repository session from `onevcs`.
//! The contract first named those types `ResolvedGraphRef` and `SessionSpec`,
//! which neither sibling exports; it now names `ConfigRef` and `SessionRequest`,
//! which they do. Divergences 1 and 2 in
//! [`docs/contract-divergences.md`](../../../docs/contract-divergences.md)
//! record the ruling.

// llmlint: ignore-file[invalid_states_unrepresentable] every shape in this module is the
// one `docs/contract.md` declares in its own Rust block, character for character, and
// narrowing any of them is interface drift. That covers `Executor::name -> &str` (an
// `ExecutorName` newtype is a public item the contract does not name; the rules file
// validates the name against the declared executors), `Capabilities.vcs_sessions: bool`
// (written as `{ vcs_sessions: bool, ... }`), and `CapacityReport.load1: f64` (written as
// `{ slots_free, load1, mem_free_bytes }`, where the probe already refuses a negative or
// NaN load by never producing one).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use oneagentgraph::config::ConfigRef;
use onevcs::SessionRequest;

use crate::agentgraph::{GraphOutput, GraphRun, Launch};
use crate::controls::{NodeControls, WORKER_MEMBER};
use crate::error::{Error, Result};
use crate::event::{Envelope, Labels};

/// Where a node's dispatch runs.
pub trait Executor {
    /// The name the [rules](crate::rules) file selects this executor by.
    fn name(&self) -> &str;
    /// What this executor can do.
    fn capabilities(&self) -> Capabilities;
    /// What it currently has free.
    fn capacity(&self) -> CapacityReport;
    /// Start one dispatch.
    fn dispatch(&self, req: DispatchRequest) -> Result<Box<dyn DispatchHandle>>;
}

/// What an [`Executor`] can do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Capabilities {
    /// Whether it can open a `onevcs` session — that is, whether it accepts
    /// [`WorkspaceSpec::VcsSession`] as well as [`WorkspaceSpec::Path`].
    pub vcs_sessions: bool,
}

/// What an [`Executor`] currently has free.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CapacityReport {
    /// How many more dispatches it will accept.
    pub slots_free: u32,
    /// Its one-minute load average.
    pub load1: f64,
    /// Its free memory, in bytes.
    pub mem_free_bytes: u64,
}

/// One dispatch, as an [`Executor`] is asked for it.
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchRequest {
    /// The content-addressed node-scope agent-graph config, an `oneagentgraph`
    /// type.
    pub graph: ConfigRef,
    /// The task prose.
    pub task: String,
    /// Where in the run this dispatch sits. The reserved keys are `run_id`,
    /// `node`, `step`, and `persona`.
    pub labels: Labels,
    /// The per-node controls this dispatch runs under.
    ///
    /// Carried on the request rather than on the labels: a label is what an
    /// envelope is stamped with and what a `node_label` rule selects on, while a
    /// control changes the agent graph's own effective configuration. `persona`
    /// is both, and is the label, which is why it is not here.
    pub controls: NodeControls,
    /// The workspace to run in.
    pub workspace: WorkspaceSpec,
    /// Raised to stop the dispatch cooperatively.
    pub cancel: CancellationToken,
}

/// The workspace a dispatch runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSpec {
    /// A directory that already exists on the machine running the dispatch.
    Path(PathBuf),
    /// A `onevcs` session the machine running the dispatch opens *there* — the
    /// clone, worktree, and branch are cut where the work happens, not shipped
    /// to it.
    VcsSession(SessionRequest),
}

/// The cooperative cancellation signal a [`DispatchRequest`] carries.
///
/// Shared rather than copied: the engine's loop raises it on one side while the
/// dispatch observes it on the other, which is what makes a `drop`, a `retry`,
/// or a `stop` end in-flight work without killing it.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// A signal nobody has raised.
    pub fn new() -> Self {
        Self::default()
    }

    /// Raise it.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether it has been raised.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

impl PartialEq for CancellationToken {
    fn eq(&self, other: &Self) -> bool {
        self.is_cancelled() == other.is_cancelled()
    }
}

/// A started dispatch.
pub trait DispatchHandle {
    /// The envelope NDJSON it produces, relayed from wherever it runs.
    fn events(&mut self) -> EventStream;
    /// Block until it settles.
    fn wait(&mut self) -> Result<DispatchOutcome>;
    /// Stop it.
    fn cancel(&self, mode: CancelMode);
}

/// A dispatch's relayed event stream.
///
/// A boxed iterator rather than a newtype: the contract names `EventStream` as
/// `events`' return type and nothing else about it, and a newtype would need
/// constructors and accessors the contract does not name.
pub type EventStream = Box<dyn Iterator<Item = Result<Envelope>> + Send>;

/// How a dispatch is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CancelMode {
    /// Raise the cancellation signal and let the dispatch preserve its work.
    Cooperative,
    /// Terminate it.
    Kill,
}

/// How a dispatch settled.
///
/// Everything a caller cannot recover from the relayed event stream: whether the
/// dispatch succeeded, and — because the machine running the dispatch is the one
/// that opened the session — the session it left open for its node to publish.
/// `docs/contract.md` declares these four; divergence 3 in
/// [the divergence record](../../../docs/contract-divergences.md) is the ruling
/// that put them there, and `#[non_exhaustive]` keeps a fifth additive.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct DispatchOutcome {
    /// Whether the dispatch completed successfully.
    pub succeeded: bool,
    /// What it said when it did not.
    pub detail: String,
    /// The `onevcs` session token, when the workspace was a session.
    pub session: Option<String>,
    /// The branch that session has checked out.
    pub branch: Option<String>,
}

/// The executor that runs a dispatch on this machine.
///
/// The only one v1 ships, and the only one that supports both
/// [`WorkspaceSpec`] variants.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct LocalExecutor;

impl Executor for LocalExecutor {
    fn name(&self) -> &str {
        "local"
    }

    fn capabilities(&self) -> Capabilities {
        // The one capability the contract states for this executor: it supports
        // both workspace variants, because the machine running the dispatch is
        // this one.
        Capabilities { vcs_sessions: true }
    }

    fn capacity(&self) -> CapacityReport {
        let load1 = load_average().unwrap_or(0.0);
        let cores = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1);
        // Every unreadable input resolves toward "has capacity": refusing to
        // dispatch on numbers nobody could measure would stall a healthy host.
        let busy = load1.ceil().max(0.0);
        let busy = if busy.is_finite() { busy as u64 } else { 0 };
        CapacityReport {
            slots_free: u32::try_from(u64::try_from(cores).unwrap_or(1).saturating_sub(busy))
                .unwrap_or(u32::MAX),
            load1,
            mem_free_bytes: available_memory().unwrap_or(u64::MAX),
        }
    }

    fn dispatch(&self, req: DispatchRequest) -> Result<Box<dyn DispatchHandle>> {
        // `WorkspaceSpec::VcsSession` means the machine running the dispatch
        // opens the session *there* — the clone, worktree, and branch are cut
        // where the work happens rather than shipped to it. This executor is
        // that machine, so it opens the session itself and runs in the worktree
        // `onevcs` hands back.
        let (dir, session) = match &req.workspace {
            WorkspaceSpec::Path(path) => (path.clone(), None),
            WorkspaceSpec::VcsSession(request) => {
                let session = crate::vcs::session_open(request)?;
                (session.worktree.clone(), Some(session))
            }
        };
        // Relayed: this dispatch is read turn by turn into the merged store.
        let node_sets = node_sets(&req.labels, &req.controls)?;
        // Every node-scope launch a run starts is one of that run's
        // `oneagentgraph` sources, so it carries the same source filter the
        // observer graph does. Read from the launch record beside the overrides
        // above, for the same reason: the labels are what identify the run, and
        // this is the last responsible moment.
        let filters = launched_with(&req.labels)?
            .map(|record| record.filters)
            .unwrap_or_default();
        let env = prepare_dispatch_env(&req.labels)?;
        let mut run = GraphRun::start(&Launch {
            graph: &req.graph.0,
            task: &req.task,
            dir: &dir,
            labels: &req.labels,
            env: &env,
            sets: &node_sets,
            filter: filters.agentgraph.as_ref(),
            output: GraphOutput::Relayed,
        })?;
        // The run's registry of what it is running, and where. Recorded here
        // because this is the layer that knows: the executor is *where a
        // dispatch runs*, so the process the work is in is its answer to give
        // and nobody else's — an executor that ran the dispatch on another
        // machine would have no local process to name, and would say so by
        // recording nothing.
        //
        // A dispatch this run cannot register does not run. The registry is the
        // only record of where the work is, so an unregistered dispatch is a
        // process no view will show and no `stop` will reach — work that can only
        // be found by a person reading a process table, on a run whose own
        // records say it has nothing running. So the graph that has just started
        // is taken back down and the failure is the caller's: a dispatch that
        // could not start is an outcome this seam already has, and the engine
        // retries it and settles the node saying so.
        let claim = match register_dispatch(&req.labels, run.process()) {
            Ok(claim) => claim,
            Err(refusal) => {
                // Ended and collected, not merely signalled: what this returns
                // to the caller is that the dispatch is not running, and a
                // process nobody has waited on is a zombie — which answers a
                // liveness probe as alive and would leave the very row an
                // operator would go looking for.
                run.cancel();
                let _ = run.wait();
                return Err(refusal);
            }
        };
        Ok(Box::new(LocalDispatch {
            run,
            cancel: req.cancel,
            labels: req.labels,
            session,
            _claim: claim,
        }))
    }
}

/// Where one dispatch may write whatever it likes.
///
/// An **absolute** path to a directory this crate created, that exists and is
/// writable before the dispatch's first turn, that is unique to that dispatch —
/// a retry, a requeue and a resumed pin of the same node each get their own — and
/// that nothing here removes while the dispatch is running. Nothing more is
/// promised: the spelling below is not a contract and no consumer may derive one
/// path from another.
///
/// Divergence 47 in
/// [the divergence record](../../../docs/contract-divergences.md) is why, and the
/// proposal this answers.
pub(crate) const NODE_SCRATCH_DIR_ENV: &str = "ONEPIPELINE_NODE_SCRATCH_DIR";

/// Compose what every dispatch this executor makes carries in its own
/// environment, **making** the scratch directory one of those pairs names.
///
/// The **run id** is what the operator's `ask-manager` wrapper addresses a
/// manager by, and a dispatch outside a run carries none for the same reason it
/// registers nothing. It is constant for the life of a driver, which is the case
/// [`export`](crate::agentgraph) allows. The **scratch directory** is per
/// dispatch, which that same note says a pair coming through here must never be;
/// divergence 47's closing paragraph is what carrying it anyway costs.
///
/// # Errors
///
/// [`Error::Ledger`] where the scratch directory cannot be made: a promised
/// directory that is not there would fail the agent's writes one at a time, and
/// those failures read as the agent's own work going wrong.
// llmlint: ignore-block[changed_behavior_has_e2e] the journeys in `tests/e2e/scratch.rs`
// and `dispatch::a_dispatchs_scratch_directory_reaches_the_turn_the_library_backend_runs`
// drive both backends. The one case with no journey is two *concurrent* library-backend
// dispatches, where what a test would assert is divergence 47's shortfall rather than
// anything this crate promises.
fn prepare_dispatch_env(labels: &Labels) -> Result<Vec<(String, String)>> {
    let mut env: Vec<(String, String)> = labels
        .run_id
        .iter()
        .map(|run| (crate::agentgraph::RUN_ID_ENV.to_string(), run.clone()))
        .collect();
    env.push((
        NODE_SCRATCH_DIR_ENV.to_string(),
        node_scratch_dir(labels)?.display().to_string(),
    ));
    Ok(env)
} // llmlint: ignore-end[changed_behavior_has_e2e]

/// Make this dispatch's own scratch directory, and answer where it is.
///
/// Under the run's own directory, so a run's scratch is thrown away with the run.
///
/// Uniqueness is the directory's *creation*, not its name: `create_dir` refuses
/// one that is already there, which a name minted from a pid and a counter would
/// not — a host reissues pids and a counter starts again in every process.
// llmlint: ignore-block[changed_behavior_has_e2e] no command reaches the two arms below:
// every dispatch a run makes carries its id, and a `scratch` that will not be created
// needs a run directory that exists holding a file by that name. Both are driven against
// the real filesystem by
// `tests::every_dispatch_is_given_a_directory_of_its_own_and_no_two_share_one`.
fn node_scratch_dir(labels: &Labels) -> Result<PathBuf> {
    /// Enough numbers that walking past every directory a run has already made is
    /// never the reason a dispatch fails, and few enough that a base directory
    /// nothing can be created in fails rather than spinning.
    const TRIES: u64 = 4096;
    static MINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let base = match labels.run_id.as_deref() {
        Some(run) => crate::ledger::RunPaths::under(&crate::ledger::runs_root(), run)
            .dir
            .join("scratch"),
        None => std::env::temp_dir().join("onepipeline-scratch"),
    };
    let ledger = |path: &Path| {
        let path = path.to_path_buf();
        move |source: std::io::Error| Error::Ledger { path, source }
    };
    std::fs::create_dir_all(&base).map_err(ledger(&base))?;
    let pid = crate::sys::pid();
    for _ in 0..TRIES {
        let at = base.join(format!(
            "{pid}-{}",
            MINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        match std::fs::create_dir(&at) {
            // Absolute, because the value is read by a program whose working
            // directory is its own business: a relative runs root — the default
            // is one — would name a different place from the workspace a
            // dispatch runs in.
            Ok(()) => return std::fs::canonicalize(&at).map_err(ledger(&at)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(ledger(&at)(error)),
        }
    }
    Err(Error::Ledger {
        path: base.clone(),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "no scratch directory under {} could be created",
                base.display()
            ),
        ),
    })
} // llmlint: ignore-end[changed_behavior_has_e2e]

/// Record this dispatch in its run's registry, and hold the entry open.
///
/// `process` is the graph run's own, where the graph is a process this crate
/// started; a graph running **in this process** is recorded as this process,
/// which is the true answer to where that dispatch's work is and the one a
/// teardown would have to aim at.
///
/// A dispatch outside a run records nothing and is not refused for it: the
/// contract's own example and the seam's tests carry no `run_id`, and there is no
/// registry for a run that does not exist. So is one whose node the labels do not
/// name — an entry that could not say which node it belonged to would be a pid an
/// operator could not act on. Every dispatch a *run* makes carries both.
fn register_dispatch(
    labels: &Labels,
    process: Option<u32>,
) -> Result<Option<crate::ledger::DispatchClaim>> {
    let (Some(run), Some(node)) = (labels.run_id.as_deref(), labels.node.as_deref()) else {
        return Ok(None);
    };
    let paths = crate::ledger::RunPaths::under(&crate::ledger::runs_root(), run);
    crate::ledger::claim_dispatch(&paths, node, process.unwrap_or_else(crate::sys::pid)).map(Some)
}

/// The overrides one dispatch's graph launch carries, in the order they apply.
///
/// The run's opaque node-scope overrides are read at the last responsible
/// moment — the labels already identify the launch ledger for every local
/// dispatch — and the node's own settings are applied *after* them: an operator's
/// `--node-set` is run-wide, and a control the plan wrote against one node is the
/// more specific of the two.
///
/// **None of them for the drafting dispatch.** `--node-set` is forwarded to every
/// *node-scope* launch, and the persona override names `members.worker`, which is
/// the member of the node-scope graph this crate composes: a run's pr-author
/// graph is the operator's whole statement about how a change request is
/// drafted, and it declares its own members under its own names. Composing
/// either onto it refuses the launch — `this graph has no worker` — which is a
/// drafting dispatch that could never start.
///
/// The **persona** is what tells the two apart, and it can be: `pr-author` is
/// this crate's own, and a plan naming it for a node or a step is refused where
/// the plan is read — see [`RESERVED_PERSONA`](crate::graph::RESERVED_PERSONA) —
/// so a dispatch arriving here under it is the drafting one and nothing else. A
/// second condition on the graph would not narrow that: an operator may point
/// `--pr-author-graph` at the same document a node dispatches under, and then
/// the graph says nothing about which dispatch this is.
fn node_sets(labels: &Labels, controls: &NodeControls) -> Result<Vec<String>> {
    if labels.persona.as_deref() == Some(crate::lifecycle::PR_AUTHOR_PERSONA) {
        return Ok(Vec::new());
    }
    let mut sets = launched_with(labels)?.map_or_else(Vec::new, |record| record.node_sets);
    if let Some(persona) = &labels.persona {
        sets.push(format!("members.{WORKER_MEMBER}.persona={persona}"));
    }
    // A control this build cannot apply refuses the launch here as well as at
    // validation, so no path composes a launch that drops one on the floor.
    sets.extend(controls.overrides().map_err(Error::Invalid)?);
    Ok(sets)
}

/// The launch record of the run this dispatch belongs to, when it belongs to one.
///
/// A dispatch built outside a run — the contract's own example, and the seam's
/// tests — carries no `run_id` and so has no launch to read: it takes the
/// defaults rather than being refused, because nothing about it is wrong.
fn launched_with(labels: &Labels) -> Result<Option<crate::ledger::LaunchRecord>> {
    let Some(run) = labels.run_id.as_deref() else {
        return Ok(None);
    };
    let paths = crate::ledger::RunPaths::under(&crate::ledger::runs_root(), run);
    crate::ledger::read_json::<crate::ledger::LaunchRecord>(&paths.launch()).map(Some)
}

/// One dispatch running on this machine.
#[derive(Debug)]
struct LocalDispatch {
    run: GraphRun,
    cancel: CancellationToken,
    labels: Labels,
    session: Option<onevcs::Session>,
    /// This dispatch's entry in the run's registry, held for exactly as long as
    /// the dispatch is: dropping the handle — settled, failed, cancelled,
    /// retried — takes the entry with it, so the registry holds live dispatches
    /// and nothing else.
    ///
    /// Underscored because nothing reads it and nothing should: what it does, it
    /// does by existing and then not.
    _claim: Option<crate::ledger::DispatchClaim>,
}

impl DispatchHandle for LocalDispatch {
    fn events(&mut self) -> EventStream {
        let opened = self.session.as_ref().map(|session| {
            // The opened session is `onevcs`'s own contribution to the merged
            // stream: without it a lifecycle node's branch would appear in the
            // ledger with nothing saying where it came from.
            Ok(crate::vcs::session_opened_event(session, &self.labels))
        });
        match opened {
            Some(event) => Box::new(std::iter::once(event).chain(self.run.events())),
            None => self.run.events(),
        }
    }

    fn wait(&mut self) -> Result<DispatchOutcome> {
        let settled = self.run.wait()?;
        Ok(DispatchOutcome {
            succeeded: settled.succeeded(),
            detail: settled.stderr.trim().to_string(),
            session: self.session.as_ref().map(|s| s.token.0.clone()),
            branch: self.session.as_ref().map(|s| s.branch.clone()),
        })
    }

    /// Stop this dispatch, as far as the mode asks.
    ///
    /// Both modes raise the cooperative signal, because both are the caller
    /// changing its mind. `Kill` additionally tears the graph run down —
    /// `GraphRun::cancel` acts on either backend and reaps the process tree —
    /// which is what a `Cooperative` stop deliberately does not do: the engine
    /// asks the live turn to commit and end first, and escalates to this only
    /// when the dispatch has not exited by its deadline.
    fn cancel(&self, mode: CancelMode) {
        self.cancel.cancel();
        if mode == CancelMode::Kill {
            self.run.cancel();
        }
    }
}

/// This host's one-minute load average, where it can be read.
fn load_average() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    text.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

/// This host's available memory in bytes, where it can be read.
fn available_memory() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kib = rest.split_whitespace().next()?.parse::<u64>().ok()?;
            return kib.checked_mul(1024);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_executor_is_named_and_capable_of_both_workspaces() {
        let executor = LocalExecutor;
        assert_eq!(executor.name(), "local");
        assert!(executor.capabilities().vcs_sessions);
    }

    #[test]
    fn the_capacity_probe_reports_finite_numbers_on_any_host() {
        let report = LocalExecutor.capacity();
        assert!(
            report.load1.is_finite() && report.load1 >= 0.0,
            "{report:?}"
        );
        assert!(report.mem_free_bytes > 0, "{report:?}");
    }

    #[test]
    fn a_cancellation_signal_is_shared_between_the_two_sides() {
        let token = CancellationToken::new();
        let observer = token.clone();
        assert!(!observer.is_cancelled());
        token.cancel();
        assert!(
            observer.is_cancelled(),
            "the signal did not reach the dispatch"
        );
        assert_eq!(token, observer);
        assert_ne!(CancellationToken::new(), observer);
    }

    #[test]
    fn a_dispatch_request_carries_both_siblings_types() {
        // The seam's whole point: this fails to compile if either sibling's
        // vocabulary drifts out from under it.
        let request = DispatchRequest {
            graph: ConfigRef("./graphs/node-scope.yaml".into()),
            task: "## What\ndo it".into(),
            labels: Labels::default(),
            controls: NodeControls::default(),
            workspace: WorkspaceSpec::VcsSession(SessionRequest {
                repo: "owner/repo".into(),
                branch: None,
                base: None,
                execution_checkout: None,
            }),
            cancel: CancellationToken::new(),
        };
        assert!(matches!(request.workspace, WorkspaceSpec::VcsSession(_)));
        assert_eq!(request.graph.0, "./graphs/node-scope.yaml");
    }

    /// The drafting dispatch takes the graph the launch named as it was written.
    ///
    /// Neither half of the node-scope composition is a statement about it: the
    /// persona override names a member only the node-scope graph has, and
    /// `--node-set` is forwarded to node-scope launches. Both are dropped on the
    /// persona alone, which is a name a plan may not claim — so this is the one
    /// dispatch that reaches it.
    #[test]
    fn the_drafting_dispatch_composes_nothing_onto_the_graph_the_launch_named() {
        let root = std::env::temp_dir().join(format!("onepipeline-drafting-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = crate::ledger::RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let record = r#"{"run_id":"demo","plan":"p.json","node_graph":"./node.yaml",
            "pr_author_graph":"./author.yaml","launcher":"l","session":"s","pid":1,
            "host":"h","started_at":"now","heartbeat_interval":1,
            "node_sets":["members.worker.model=m"]}"#;
        std::fs::write(paths.launch(), record).expect("the launch record is written");
        std::env::set_var(crate::ledger::RUNS_DIR_ENV, &root);

        let sets = |persona: &str| {
            node_sets(
                &Labels {
                    run_id: Some("demo".into()),
                    persona: Some(persona.into()),
                    ..Labels::default()
                },
                &NodeControls::default(),
            )
            .expect("the launch record is readable")
        };
        assert!(
            sets(crate::lifecycle::PR_AUTHOR_PERSONA).is_empty(),
            "the drafting dispatch was given a member this graph never declared"
        );
        // The node's own work, under the same run and the same record.
        assert_eq!(
            sets("engineer"),
            vec![
                "members.worker.model=m".to_string(),
                "members.worker.persona=engineer".to_string(),
            ]
        );
        std::env::remove_var(crate::ledger::RUNS_DIR_ENV);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every dispatch is given a directory of its own, and no two are given one.
    ///
    /// The end-to-end halves are `scratch::a_dispatch_is_given_an_absolute_writable_directory_of_its_own`
    /// and `scratch::two_dispatches_of_one_node_are_given_two_directories_and_neither_is_taken_away`,
    /// which read the value out of a real dispatch's own environment. What is held
    /// here is the promise itself, against the real filesystem: two dispatches of
    /// one node — the pair a retry produces, and the pair that agree on every name
    /// a path could have been derived from — and a run root that cannot hold a
    /// scratch directory at all.
    #[test]
    fn every_dispatch_is_given_a_directory_of_its_own_and_no_two_share_one() {
        let root = std::env::temp_dir().join(format!("onepipeline-scratch-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var(crate::ledger::RUNS_DIR_ENV, &root);
        let labels = Labels {
            run_id: Some("demo".into()),
            node: Some("build".into()),
            ..Labels::default()
        };

        let scratch = |labels: &Labels| {
            let env = prepare_dispatch_env(labels).expect("the dispatch's environment is composed");
            let (_, value) = env
                .iter()
                .find(|(key, _)| key == NODE_SCRATCH_DIR_ENV)
                .expect("every dispatch carries a scratch directory")
                .clone();
            PathBuf::from(value)
        };

        // The same node, twice, which is what a retry is.
        let first = scratch(&labels);
        let second = scratch(&labels);
        assert_ne!(
            first, second,
            "a node asked again was handed the directory its first attempt had"
        );
        for at in [&first, &second] {
            assert!(at.is_absolute(), "{} is not absolute", at.display());
            assert!(at.is_dir(), "{} was not created", at.display());
            std::fs::write(at.join("written"), "by the dispatch")
                .unwrap_or_else(|error| panic!("{} is not writable: {error}", at.display()));
        }
        // And the first is untouched by the second, which is the whole of what
        // "unique to that dispatch" buys.
        assert!(first.join("written").is_file());

        // A dispatch outside a run has no run directory to sit in and is given one
        // anyway: the contract's own example carries no `run_id`.
        assert!(scratch(&Labels::default()).is_dir());

        // A run root that is a file holds no scratch directory, and the dispatch is
        // refused rather than handed a path to nothing.
        let blocked = root.join("blocked");
        std::fs::write(&blocked, "not a directory").expect("the blocking file is written");
        std::env::set_var(crate::ledger::RUNS_DIR_ENV, &blocked);
        assert!(matches!(
            prepare_dispatch_env(&labels),
            Err(Error::Ledger { .. })
        ));

        std::env::remove_var(crate::ledger::RUNS_DIR_ENV);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dispatch_with_no_run_still_carries_its_nodes_own_controls() {
        // No `run_id`, so there is no launch record to read: the node's own
        // budget is what the launch must still carry, because a dispatch that
        // dropped it here would run to the base config's default instead.
        let sets = node_sets(
            &Labels {
                persona: Some("engineer".into()),
                ..Labels::default()
            },
            &NodeControls {
                max_turns: std::num::NonZeroU32::new(45),
            },
        )
        .expect("both are appliable");
        assert_eq!(
            sets,
            vec![
                "members.worker.persona=engineer".to_string(),
                "members.worker.max_turns=45".to_string(),
            ],
            "the node's own control must apply after the run-wide ones"
        );
    }
}
