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

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use oneagentgraph::config::ConfigRef;
use onevcs::SessionRequest;

use crate::agentgraph::{GraphOutput, GraphRun};
use crate::error::Result;
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
    /// `round`, `node`, `step`, and `persona`.
    pub labels: Labels,
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
/// Shared rather than copied: the round raises it on one side while the dispatch
/// observes it on the other, which is what makes a `drop`, a `retry`, or a
/// spent round budget stop in-flight work without killing it.
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
        let node_sets = node_sets(&req.labels)?;
        let run = GraphRun::start(
            &req.graph.0,
            &req.task,
            Some(&dir),
            &req.labels,
            &[],
            &node_sets,
            GraphOutput::Relayed,
        )?;
        Ok(Box::new(LocalDispatch {
            run,
            cancel: req.cancel,
            labels: req.labels,
            session,
        }))
    }
}

/// Read the run's opaque node-scope overrides at the last responsible moment.
/// The labels already identify the launch ledger for every local dispatch.
fn node_sets(labels: &Labels) -> Result<Vec<String>> {
    let Some(run) = labels.run_id.as_deref() else {
        return Ok(Vec::new());
    };
    let paths = crate::ledger::RunPaths::under(&crate::ledger::runs_root(), run);
    crate::ledger::read_json::<crate::ledger::LaunchRecord>(&paths.launch()).map(|record| {
        let mut sets = record.node_sets;
        if let Some(persona) = &labels.persona {
            sets.push(format!("members.worker.persona={persona}"));
        }
        sets
    })
}

/// One dispatch running on this machine.
#[derive(Debug)]
struct LocalDispatch {
    run: GraphRun,
    cancel: CancellationToken,
    labels: Labels,
    session: Option<onevcs::Session>,
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

    fn cancel(&self, mode: CancelMode) {
        self.cancel.cancel();
        // `cancel` takes `&self`, so the kill goes through the pid rather than
        // the child handle: the signal is what stops it, and the handle's owner
        // still collects the status.
        if mode == CancelMode::Kill {
            kill_pid(self.run.pid());
        }
    }
}

#[cfg(unix)]
fn kill_pid(pid: u32) {
    if let Ok(raw) = i32::try_from(pid) {
        // SAFETY: `kill` takes a pid and a signal number and touches no memory
        // this call owns. A pid that has already exited is an error we ignore.
        unsafe { libc::kill(raw, libc::SIGKILL) };
    }
}

#[cfg(windows)]
fn kill_pid(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F", "/T"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
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
}
