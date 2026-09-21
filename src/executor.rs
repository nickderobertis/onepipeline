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

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use oneagentgraph::config::ConfigRef;
use onevcs::SessionRequest;

use crate::agentgraph::{Ending, Environment, GraphOutput, GraphRun, Launch};
use crate::controls::{NodeControls, WORKER_MEMBER};
use crate::error::{Error, Result};
use crate::event::{Envelope, Labels};

/// The `onevcs` session label naming the run that opened a node's session.
///
/// Every session a node-scope dispatch opens carries this, [`SESSION_NODE_LABEL`]
/// and [`SESSION_LAUNCHER_LABEL`] on its session record — the same run, node and
/// launching session the dispatch's own events are stamped with — so a listing
/// of the host's sessions and preserved branches says whose each one is without
/// joining a run's journal. `docs/contract.md` states the three keys and
/// `tests/contract.rs` reconciles them against these constants.
pub const SESSION_RUN_LABEL: &str = "run";

/// The `onevcs` session label naming the node that opened the session.
pub const SESSION_NODE_LABEL: &str = "node";

/// The `onevcs` session label naming the launching session — the one
/// `ONEPIPELINE_LAUNCHER_SESSION` named at launch, which `runs --mine` and
/// `unwatched --session` are keyed on. Omitted, never stamped empty, for a launch
/// nothing attributed.
pub const SESSION_LAUNCHER_LABEL: &str = "launcher";

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
    /// Which attempt of the node this dispatch is, counting from one: the
    /// number that dispatch's `node-dispatched` records.
    ///
    /// On the request rather than inferred where the dispatch runs, because it
    /// is what the executor stamps the launch's history labels with — see
    /// [`crate::agents::ATTEMPT_LABEL`] — and an executor on another machine
    /// has no journal to read it off.
    pub attempt: NonZeroU32,
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
        // The run's launch record, read once at the last responsible moment: the
        // labels are what identify the run, and the overrides, the source filter
        // and the dispatch-env hook below are all what that launch decided.
        let launched = launched_with(&req.labels)?;
        // Relayed: this dispatch is read turn by turn into the merged store.
        let node_sets = node_sets(launched.as_ref(), &req.labels, &req.controls)?;
        // The dispatch-env hook, **before** anything of the launch begins — the
        // session below included, so a launch the hook refuses cuts nothing —
        // and never for a dispatch built outside a run, which has no launch to
        // name one. What it adds is this child's alone: it goes into `env` below
        // and nowhere else, and this process's own environment is untouched.
        let added = match (
            &launched,
            req.labels.run_id.as_deref(),
            req.labels.node.as_deref(),
        ) {
            (Some(record), Some(run), Some(node)) => {
                let paths = crate::ledger::RunPaths::under(&crate::ledger::runs_root(), run);
                crate::dispatchenv::run_and_check(&crate::dispatchenv::Launching {
                    paths: &paths,
                    record,
                    node,
                    graph: &req.graph,
                    sets: &node_sets,
                    own: &own_dispatch_variables(&req.workspace),
                })?
            }
            _ => Vec::new(),
        };
        // `WorkspaceSpec::VcsSession` means the machine running the dispatch
        // opens the session *there* — the clone, worktree, and branch are cut
        // where the work happens rather than shipped to it. This executor is
        // that machine, so it opens the session itself and runs in the worktree
        // `onevcs` hands back.
        let (dir, session) = match &req.workspace {
            WorkspaceSpec::Path(path) => (path.clone(), None),
            WorkspaceSpec::VcsSession(request) => {
                let request = SessionRequest {
                    labels: session_labels(&request.labels, &req.labels, launched.as_ref()),
                    ..request.clone()
                };
                let session = crate::vcs::session_open(&request)?;
                remember_worktree(&session);
                (session.worktree.clone(), Some(session))
            }
        };
        // The session this dispatch works in: the one just opened, or — for every
        // later dispatch of the same node, which names the worktree rather than
        // asking for a session of its own — the one this executor opened there.
        let token = session
            .as_ref()
            .map(|session| session.token.0.clone())
            .or_else(|| session_of_worktree(&dir));
        // Every node-scope launch a run starts is one of that run's
        // `oneagentgraph` sources, so it carries the same source filter the
        // observer graph does.
        let filters = launched
            .as_ref()
            .map(|record| record.filters.clone())
            .unwrap_or_default();
        // The hook's additions first and this crate's own keys after them, so a
        // hook cannot move where a dispatch keeps its scratch or which run it
        // belongs to: a later pair of the same name is the one the child gets.
        let mut env = added;
        env.extend(prepare_dispatch_env(&req.labels, token.as_deref())?);
        // And the run's history stamp after both, composed **from** them: the
        // hook's document is overlaid before the engine's own pairs, so a label
        // set it wrote is the inherited value the merge keeps every repository
        // key of. A dispatch built outside a run has no run root to point at,
        // and carries none of this.
        if let (Some(record), Some(run)) = (&launched, req.labels.run_id.as_deref()) {
            let paths = crate::ledger::RunPaths::under(&crate::ledger::runs_root(), run);
            let inherited = match env
                .iter()
                .rev()
                .find(|(key, _)| key == crate::agents::LABELS_ENV)
            {
                Some((_, value)) => Some(value.clone()),
                None => crate::agents::inherited_labels()?,
            };
            // Every dispatch inside a run is a node's: the engine composes the
            // labels of each, and a request naming a run and no node is one
            // nothing here built.
            let node = req.labels.node.as_deref().ok_or_else(|| {
                Error::Invalid(format!(
                    "a dispatch inside run '{run}' names no node, so it cannot be stamped"
                ))
            })?;
            let launched =
                if req.labels.persona.as_deref() == Some(crate::lifecycle::PR_AUTHOR_PERSONA) {
                    crate::agents::Launched::PrAuthor {
                        node,
                        attempt: req.attempt,
                    }
                } else {
                    crate::agents::Launched::Node {
                        node,
                        step: req.labels.step.as_deref(),
                        attempt: req.attempt,
                    }
                };
            env.extend(crate::agents::overlay(
                &paths,
                inherited.as_deref(),
                &crate::agents::Stamp {
                    run,
                    project: (!record.project.is_empty()).then_some(record.project.as_str()),
                    launched,
                },
            )?);
        }
        let mut run = GraphRun::start(&Launch {
            graph: &req.graph.0,
            task: &req.task,
            dir: &dir,
            labels: &req.labels,
            env: &env,
            // The scratch directory in `env` is this dispatch's and no other's,
            // so the launch is given a process to hold it in: the library
            // backend has nowhere per-launch to put a pair, and two dispatches
            // sharing one driver would read and overwrite each other's.
            environment: Environment::PerLaunch,
            sets: &node_sets,
            filter: filters.agentgraph.as_ref(),
            output: GraphOutput::Relayed,
            // The dispatch settles on the terminal envelope this launch relays,
            // which is the answer the graph gives before its own final teardown
            // — so the launch is held for the sibling's write of its record,
            // which its `history` lists the run by, and not for the reap after.
            ending: Ending::Announced,
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
/// Divergence 48 in
/// [the divergence record](../../../docs/contract-divergences.md) is why, and the
/// proposal this answers.
pub(crate) const NODE_SCRATCH_DIR_ENV: &str = "ONEPIPELINE_NODE_SCRATCH_DIR";

/// The sessions this executor opened, by the worktree each handed back.
///
/// A lifecycle node's first dispatch asks for a session and every later one —
/// its remaining steps and its drafting dispatch — names the worktree that
/// session opened, because a second session on the same branch would reclaim
/// the first. The token is not on that request: [`WorkspaceSpec::Path`] is a
/// directory and the contract fixes it as one. So the executor that opened the
/// session is what remembers which one, which is the same fact
/// [`WorkspaceSpec::VcsSession`] states — the machine running the dispatch is
/// the one that opened the session there. A path nothing here opened answers
/// nothing, which is every direct node's dispatch.
fn opened_worktrees() -> std::sync::MutexGuard<'static, std::collections::BTreeMap<PathBuf, String>>
{
    static OPENED: std::sync::Mutex<std::collections::BTreeMap<PathBuf, String>> =
        std::sync::Mutex::new(std::collections::BTreeMap::new());
    // A poisoned lock holds a map a panicking thread was mid-insert into, which
    // is still a map of sessions this process opened.
    OPENED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn remember_worktree(session: &onevcs::Session) {
    opened_worktrees().insert(session.worktree.clone(), session.token.0.clone());
}

fn session_of_worktree(worktree: &Path) -> Option<String> {
    opened_worktrees().get(worktree).cloned()
}

/// The names of the variables [`prepare_dispatch_env`] will set on a dispatch in
/// `workspace`, known before any of their values are.
///
/// What the dispatch-env hook's check is told is present by name: the pairs
/// themselves are composed only once the launch goes ahead — the scratch
/// directory is made, and a session's token exists once the session is open —
/// and a config that sources one of this crate's own variables through
/// `env_from` is a launch that will have it. The session is the one variable
/// not every dispatch carries, and whether this one will is decided here the
/// way [`dispatch`](LocalExecutor::dispatch) decides it: a session it opens, or
/// one already opened on the worktree it names.
fn own_dispatch_variables(workspace: &WorkspaceSpec) -> Vec<&'static str> {
    let mut names = vec![
        crate::agentgraph::RUN_ID_ENV,
        crate::agentgraph::RUNS_DIR_ENV,
        crate::channel::ASKER_ENV,
        NODE_SCRATCH_DIR_ENV,
    ];
    let in_session = match workspace {
        WorkspaceSpec::VcsSession(_) => true,
        WorkspaceSpec::Path(path) => session_of_worktree(path).is_some(),
    };
    if in_session {
        names.push(crate::agentgraph::SESSION_ENV);
    }
    names
}

/// Compose what every dispatch this executor makes carries in its own
/// environment, **making** the scratch directory one of those pairs names.
///
/// The **run id** is what the operator's `ask-manager` wrapper addresses a
/// manager by, and a dispatch outside a run carries none for the same reason it
/// registers nothing. The **runs root** goes beside it, absolute, so a dispatch
/// that runs `onepipeline transcript` reads this run's store from wherever it
/// is working. The **session** is the `onevcs` token whose worktree the dispatch
/// runs in, so a worker can address its own session; a dispatch in no session
/// carries none. The **scratch directory** is this dispatch's alone, which
/// is why the launch below declares [`Environment::PerLaunch`]: the pair has to
/// live somewhere no sibling dispatch can read or overwrite, and that is a
/// process rather than a map. The **asker** is that same uniqueness read as an
/// identity: the wrapper above asks through a succession of the host bus server
/// listeners, and this is what tells them they are serving one side that is
/// still waiting rather than a series of sides that have each gone.
///
/// # Errors
///
/// [`Error::Ledger`] where the scratch directory cannot be made: a promised
/// directory that is not there would fail the agent's writes one at a time, and
/// those failures read as the agent's own work going wrong. And where the runs
/// root cannot be resolved to an absolute path, for the reason stated at that
/// call.
fn prepare_dispatch_env(labels: &Labels, session: Option<&str>) -> Result<Vec<(String, String)>> {
    let mut env: Vec<(String, String)> = labels
        .run_id
        .iter()
        .map(|run| (crate::agentgraph::RUN_ID_ENV.to_string(), run.clone()))
        .collect();
    // Absolute, because the dispatch does not run where this process was
    // started: the default root is the relative `runs`, which from inside a
    // worktree names a directory that is not there. A resolution that failed —
    // this process's own working directory unreadable, which is what `absolute`
    // consults for a relative root — is **refused** rather than fallen back
    // from, because exporting the relative root anyway hands every dispatch a
    // path the contract promises is absolute and that resolves, in the worktree,
    // somewhere else or nowhere.
    let root = crate::ledger::runs_root();
    let root = std::path::absolute(&root).map_err(|source| Error::Ledger {
        path: root.clone(),
        source,
    })?;
    env.push((
        crate::agentgraph::RUNS_DIR_ENV.to_string(),
        root.display().to_string(),
    ));
    if let Some(token) = session {
        env.push((crate::agentgraph::SESSION_ENV.to_string(), token.to_owned()));
    }
    let scratch = make_node_scratch_dir(labels)?.display().to_string();
    // The asker's name is the scratch directory's own path rather than a second
    // thing minted beside it: what has to be true of it is that every session
    // this dispatch serves through carries the same value and no other dispatch
    // carries it, and that is exactly what the directory above already is. It is
    // read as an opaque word and never as a path — see `channel::ASKER_ENV`.
    env.push((crate::channel::ASKER_ENV.to_string(), scratch.clone()));
    env.push((NODE_SCRATCH_DIR_ENV.to_string(), scratch));
    Ok(env)
}

/// Make this dispatch's own scratch directory, and answer where it is.
///
/// Named for the making, because that is the whole of it: this mints a number no
/// caller has had, creates a directory under it, and answers the path — so two
/// calls with identical arguments answer differently, and neither answer existed
/// before the call.
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
fn make_node_scratch_dir(labels: &Labels) -> Result<PathBuf> {
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
fn node_sets(
    launched: Option<&crate::ledger::LaunchRecord>,
    labels: &Labels,
    controls: &NodeControls,
) -> Result<Vec<String>> {
    if labels.persona.as_deref() == Some(crate::lifecycle::PR_AUTHOR_PERSONA) {
        return Ok(Vec::new());
    }
    let mut sets = launched.map_or_else(Vec::new, |record| record.node_sets.clone());
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

/// The labels a node's `onevcs` session is opened with.
///
/// Whatever the request already carries, with the engine's own keys decided over
/// it: the run and the node off the dispatch's labels, and the launching session
/// off the run's launch record when that launch was attributed to one. A key the
/// engine has no value for is removed rather than left as the caller spelled it,
/// so a session never names a launcher its launch did not have.
// llmlint: ignore[changed_behavior_has_e2e] the merge with a caller's own labels is
// reachable only by a library caller building its own `DispatchRequest`: the binary's
// every request comes from `vcs::request_for`, which carries none, so no journey can
// supply one. `tests::a_sessions_labels_keep_the_callers_and_the_engine_decides_its_own`
// holds the merge; `tests/e2e/session_reuse.rs` holds the stamping through the binary.
fn session_labels(
    asked: &std::collections::BTreeMap<String, String>,
    labels: &Labels,
    launched: Option<&crate::ledger::LaunchRecord>,
) -> std::collections::BTreeMap<String, String> {
    let mut stamped = asked.clone();
    let launcher = launched
        .map(|record| record.session.as_str())
        .filter(|session| crate::ledger::attributed(session));
    for (key, value) in [
        (SESSION_RUN_LABEL, labels.run_id.as_deref()),
        (SESSION_NODE_LABEL, labels.node.as_deref()),
        (SESSION_LAUNCHER_LABEL, launcher),
    ] {
        match value {
            Some(value) => stamped.insert(key.to_owned(), value.to_owned()),
            None => stamped.remove(key),
        };
    }
    stamped
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
    if let Some(stated) = stated_load_average() {
        return Some(stated);
    }
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    text.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

/// The environment variable stating the one-minute load average this process
/// reads in place of the host's.
///
/// `/proc/loadavg` is the host's whole load, which is not this process's
/// measure everywhere it runs: a container reads its host's load beside its own
/// cgroup's parallelism, and a suite driving several runs at once on one machine
/// reads its own neighbours as a host with no room. Where the operator knows
/// better, this says so; a value that is not a finite, non-negative number is
/// ignored and the host is read.
pub const LOAD1_ENV: &str = "ONEPIPELINE_LOAD1";

/// The load average the environment states, where it states one this build
/// can read.
fn stated_load_average() -> Option<f64> {
    std::env::var(LOAD1_ENV)
        .ok()?
        .trim()
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
    fn a_sessions_labels_keep_the_callers_and_the_engine_decides_its_own() {
        let asked: std::collections::BTreeMap<String, String> = [
            ("owner", "ci"),
            (SESSION_RUN_LABEL, "stale-run"),
            (SESSION_LAUNCHER_LABEL, "stale-session"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
        let labels = Labels {
            run_id: Some("demo-1".into()),
            node: Some("service".into()),
            ..Labels::default()
        };
        let record = |session: &str| -> crate::ledger::LaunchRecord {
            serde_json::from_value(serde_json::json!({
                "run_id": "demo-1",
                "session": session,
            }))
            .expect("a launch record")
        };
        let spelled = |stamped: std::collections::BTreeMap<String, String>| {
            stamped
                .into_iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            spelled(session_labels(&asked, &labels, Some(&record("session-a")))),
            [
                "launcher=session-a",
                "node=service",
                "owner=ci",
                "run=demo-1"
            ]
        );
        // An unattributed launch names no launcher, whatever the caller said.
        for nobody in ["", crate::sys::UNKNOWN_LAUNCHER] {
            assert_eq!(
                spelled(session_labels(&asked, &labels, Some(&record(nobody)))),
                ["node=service", "owner=ci", "run=demo-1"],
                "{nobody:?}"
            );
        }
        assert_eq!(
            spelled(session_labels(&asked, &labels, None)),
            ["node=service", "owner=ci", "run=demo-1"]
        );
    }

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

    /// A stated load average stands in for the host's, and one this build
    /// cannot read is ignored rather than read as a host with no load.
    #[test]
    fn a_stated_load_average_stands_in_for_the_hosts_and_an_unreadable_one_is_ignored() {
        let _held = crate::vcs::scratch_home_held();
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        std::env::set_var(LOAD1_ENV, "0");
        let still = LocalExecutor.capacity();
        assert_eq!(still.load1, 0.0);
        assert_eq!(still.slots_free as usize, cores);
        std::env::set_var(LOAD1_ENV, cores.to_string());
        assert_eq!(LocalExecutor.capacity().slots_free, 0);
        for unreadable in ["", "busy", "-1", "NaN", "inf"] {
            std::env::set_var(LOAD1_ENV, unreadable);
            assert_eq!(stated_load_average(), None, "{LOAD1_ENV}={unreadable:?}");
            let host = LocalExecutor.capacity();
            assert!(host.load1.is_finite() && host.load1 >= 0.0, "{host:?}");
        }
        std::env::remove_var(LOAD1_ENV);
        assert_eq!(stated_load_average(), None);
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
                pool: None,
                overflow: None,
                labels: Default::default(),
            }),
            cancel: CancellationToken::new(),
            attempt: NonZeroU32::MIN,
        };
        assert!(matches!(request.workspace, WorkspaceSpec::VcsSession(_)));
        assert_eq!(request.graph.0, "./graphs/node-scope.yaml");
    }

    /// Serialises the tests below that set `RUNS_DIR_ENV`, which belongs to the
    /// whole process rather than to the test that set it.
    ///
    /// nextest gives each test its own process; plain `cargo test` runs a
    /// module's tests as *threads of one process*, where both tests below would
    /// otherwise read whichever value the other set last. The lock costs nothing
    /// under nextest and makes both runners say the same thing.
    static RUNS_DIR: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Held for the length of a test that sets `RUNS_DIR_ENV`. A poisoned lock
    /// is recovered rather than propagated: the test that panicked holding it
    /// has already failed, and refusing to run the next one would report a
    /// second failure belonging to nobody.
    fn runs_dir_lock() -> std::sync::MutexGuard<'static, ()> {
        RUNS_DIR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A temporary root nobody else has, keyed on the *test* rather than on the
    /// process it runs in. Two tests running as threads share a pid; the counter
    /// is what they do not share.
    fn scratch_root(what: &str) -> PathBuf {
        static NTH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "onepipeline-{what}-{}-{}",
            crate::sys::pid(),
            NTH.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        root
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
        let _runs_dir = runs_dir_lock();
        let root = scratch_root("drafting");
        let paths = crate::ledger::RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let record = r#"{"run_id":"demo","plan":"p.json","node_graph":"./node.yaml",
            "pr_author_graph":"./author.yaml","launcher":"l","session":"s","pid":1,
            "host":"h","started_at":"now","heartbeat_interval":1,
            "node_sets":["members.worker.model=m"]}"#;
        std::fs::write(paths.launch(), record).expect("the launch record is written");
        std::env::set_var(crate::ledger::RUNS_DIR_ENV, &root);

        let sets = |persona: &str| {
            let labels = Labels {
                run_id: Some("demo".into()),
                persona: Some(persona.into()),
                ..Labels::default()
            };
            let launched = launched_with(&labels).expect("the launch record is readable");
            node_sets(launched.as_ref(), &labels, &NodeControls::default())
                .expect("the overrides compose")
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
    /// and `scratch::every_dispatch_of_one_node_is_given_its_own_directory_and_none_is_taken_away`,
    /// which read the value out of a real dispatch's own environment. What is held
    /// here is the promise itself, against the real filesystem: two dispatches of
    /// one node — the pair a retry produces, and the pair that agree on every name
    /// a path could have been derived from — and a run root that cannot hold a
    /// scratch directory at all.
    #[test]
    fn every_dispatch_is_given_a_directory_of_its_own_and_no_two_share_one() {
        let _runs_dir = runs_dir_lock();
        let root = scratch_root("scratch");
        std::env::set_var(crate::ledger::RUNS_DIR_ENV, &root);
        let labels = Labels {
            run_id: Some("demo".into()),
            node: Some("build".into()),
            ..Labels::default()
        };

        let scratch = |labels: &Labels| {
            let env =
                prepare_dispatch_env(labels, None).expect("the dispatch's environment is composed");
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
            prepare_dispatch_env(&labels, None),
            Err(Error::Ledger { .. })
        ));

        std::env::remove_var(crate::ledger::RUNS_DIR_ENV);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A runs root that cannot be made absolute refuses the dispatch.
    ///
    /// `ONEPIPELINE_RUNS_DIR` is promised absolute, and a dispatch reads it from
    /// a worktree rather than from where this process started — so a relative
    /// root exported anyway names a different directory there, or none. The one
    /// way the resolution fails for a relative root is this process's own
    /// working directory being unreadable, which is what `std::path::absolute`
    /// consults; that is induced here by removing it.
    ///
    /// The working directory belongs to the whole process, so this holds the
    /// same lock the root-setting tests do and puts it back before releasing it.
    /// nextest gives each test its own process and pays nothing for either.
    #[test]
    #[cfg(unix)]
    fn a_runs_root_that_cannot_be_made_absolute_refuses_the_dispatch() {
        let _runs_dir = runs_dir_lock();
        let labels = Labels {
            run_id: Some("demo".into()),
            node: Some("build".into()),
            ..Labels::default()
        };
        // Relative, which is the only kind whose resolution consults the working
        // directory — and the shipped default is one.
        std::env::set_var(crate::ledger::RUNS_DIR_ENV, "runs");
        let here = std::env::current_dir().expect("this process has a working directory");
        let gone = scratch_root("cwd-gone");
        std::fs::create_dir_all(&gone).expect("the directory to stand in is made");
        std::env::set_current_dir(&gone).expect("this process can stand in it");
        std::fs::remove_dir(&gone).expect("and it can be taken away underneath");

        let refused = prepare_dispatch_env(&labels, None);

        std::env::set_current_dir(&here).expect("the working directory is put back");
        std::env::remove_var(crate::ledger::RUNS_DIR_ENV);

        match refused {
            Err(Error::Ledger { path, .. }) => assert_eq!(
                path,
                PathBuf::from("runs"),
                "the refusal does not name the root that could not be resolved"
            ),
            other => panic!(
                "a runs root that cannot be made absolute was accepted rather than refused: \
                 {other:?}"
            ),
        }
    }

    #[test]
    fn a_dispatch_with_no_run_still_carries_its_nodes_own_controls() {
        // No `run_id`, so there is no launch record to read: the node's own
        // budget is what the launch must still carry, because a dispatch that
        // dropped it here would run to the base config's default instead.
        let sets = node_sets(
            None,
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
