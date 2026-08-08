//! The executor seam.
//!
//! An [`Executor`] is *where* a node's dispatch runs. v1 ships [`LocalExecutor`]
//! only — it supports both workspace variants — while the trait and the
//! [rules grammar](crate::rules) are shaped so a dispatch-server executor over a
//! WebSocket, and a Kubernetes one, drop in behind the same interface.
//!
//! Two of the request's fields are a sibling library's types, so this seam is
//! also where the cross-repo wiring is proven at compile time: the agent-graph
//! config comes from `oneagentgraph` and the repository session from `onevcs`.
//! The contract names those types `ResolvedGraphRef` and `SessionSpec`; neither
//! sibling exports a type by that name, and
//! [`docs/contract-divergences.md`](../../../docs/contract-divergences.md)
//! records what is used instead and why.
//!
//! Nothing here dispatches, probes capacity, relays a stream, waits, or cancels.

use std::path::PathBuf;

use oneagentgraph::config::ConfigRef;
use onevcs::SessionRequest;

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
/// `docs/contract.md` names the type on the request but specifies no operations
/// for it, so this is the handle and nothing more; raising and observing it
/// lands with the first executor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CancellationToken;

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
/// `docs/contract.md` names this as `wait`'s success value but specifies no
/// fields for it; what it carries is a gap
/// [recorded for the planner](../../../docs/contract-divergences.md) rather than
/// filled in here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DispatchOutcome;

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
        // both workspace variants.
        Capabilities { vcs_sessions: true }
    }

    fn capacity(&self) -> CapacityReport {
        // Nothing is probed yet, and a report that guessed would let the rules
        // file select an executor on numbers nobody measured. Zero free slots is
        // the honest interface-only answer; the probe lands with the dispatch.
        CapacityReport::default()
    }

    fn dispatch(&self, _req: DispatchRequest) -> Result<Box<dyn DispatchHandle>> {
        Err(Error::NotImplemented("LocalExecutor::dispatch"))
    }
}
