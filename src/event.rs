//! The merged event stream's envelope.
//!
//! `onepipeline` merges the three libraries' streams into one, so it both
//! *relays* envelopes produced by `oneagentgraph` and `onevcs` and *emits* its
//! own. The shape is the stack's one NDJSON envelope, duplicated here on purpose
//! — there is deliberately no shared util crate, so each producer owns its copy
//! and the contract fixtures hold them together.
//!
//! Nothing here emits, orders, merges, truncates, or redacts anything: this is
//! the wire shape and its documented bounds, not the machinery that honours
//! them.

// llmlint: ignore-file[invalid_states_unrepresentable, boundary_inputs_validated] two
// things here are deliberately not narrowed at the interface-only stage (see AGENTS.md).
// `EventKind` is the wire string because this crate relays another library's kinds as
// well as its own and `docs/contract.md` enumerates neither set — an enum here would
// invent the interface rather than compile it, and would reject a kind a sibling already
// emits. And the envelope's semantic checks — that `v` is 1, that `ts` is
// millisecond-precision UTC RFC 3339, that a text field was truncated at
// `MAX_PAYLOAD_TEXT_BYTES` — belong to the reader seam that parses a stream, which is
// exactly what this stage does not implement. The structural boundary *is* enforced: an
// unknown `source`, a `seq` that is not a `u64`, or a missing field is rejected by serde
// and asserted in `tests/contract.rs`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The envelope version this crate produces and understands.
pub const ENVELOPE_VERSION: u32 = 1;

/// The byte bound on a payload text field, past which it is truncated and the
/// payload carries `truncated: true`.
pub const MAX_PAYLOAD_TEXT_BYTES: usize = 4096;

/// One NDJSON event.
///
/// A stream is merged in its own [`seq`](Self::seq) — the producer's statement
/// of the order it wrote things in, and the only ordering promise an envelope
/// carries — and the streams interleave with each other by [`ts`](Self::ts). A
/// consumer detects loss through per-stream `seq` gaps; there are no
/// cross-stream ordering promises beyond the timestamps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Envelope version; [`ENVELOPE_VERSION`] for anything this crate writes.
    pub v: u32,
    /// RFC 3339 timestamp, millisecond precision, UTC.
    pub ts: String,
    /// Unique id of the producing process.
    pub stream: String,
    /// Monotonic per [`stream`](Self::stream).
    pub seq: u64,
    /// Which of the three libraries produced the event.
    pub source: Source,
    /// What happened, as the producing library named it.
    pub kind: EventKind,
    /// Where in the run the producer stamped the event.
    #[serde(default)]
    pub labels: Labels,
    /// Kind-specific detail. Text fields are bounded by
    /// [`MAX_PAYLOAD_TEXT_BYTES`]; large evidence is an [`ArtifactRef`] instead.
    #[serde(default)]
    pub payload: Map<String, Value>,
    /// Evidence stored by the producing library and referenced by id.
    #[serde(default)]
    pub artifacts: Vec<ArtifactRef>,
}

/// The library that produced an event — one per merged stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// `oneagentgraph`.
    Agentgraph,
    /// `onevcs`.
    Vcs,
    /// This crate.
    Pipeline,
}

/// What an [`Envelope`] reports, as its producer named it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventKind(pub String);

/// Where in the run an event happened.
///
/// The five reserved keys are the ones `docs/contract.md` names on a
/// [`DispatchRequest`](crate::executor::DispatchRequest); anything else a
/// producer stamps rides in [`extra`](Self::extra). Enrichers never rewrite what
/// is already there.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Labels {
    /// The run this event belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The round within the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u64>,
    /// The graph node being executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// The step within a node that runs several in sequence on one branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// The persona the dispatch is running under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    /// Free-form extras beyond the reserved keys above.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Every event kind this library emits, and exactly those.
///
/// Enumerated because they are `onepipeline`'s own vocabulary: a kind this crate
/// writes cannot be a typo, and a reader folds a closed set rather than matching
/// strings. The kinds a *sibling* produces stay [`EventKind`]'s wire string —
/// this crate relays those unchanged, and an enum there would reject a kind a
/// newer sibling already emits. `docs/contract.md` lists exactly these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum PipelineKind {
    /// The run was launched.
    RunStarted,
    /// A launch deliberately proceeded beside live repository holders.
    ConcurrentAcknowledged,
    /// A round began executing.
    RoundStarted,
    /// A round stopped executing, with the state it settled in.
    RoundFinished,
    /// A node's dispatch was started.
    NodeDispatched,
    /// A node reached a terminal status for this round.
    NodeSettled,
    /// A live edit was accepted and applied to the desired graph.
    EditCommitted,
    /// A live edit was refused, with the reason its submitter was told.
    EditRejected,
    /// A surface was *sent*. Delivery is a separate fact.
    PlannerSurfaceQueued,
    /// A surface was *consumed* by the planner. This is what resets the pacemaker.
    PlannerSurfaced,
    /// The planner answered a consumed surface.
    PlannerReplied,
    /// A human action was attested.
    HumanAttested,
    /// A fresh driver was attached to an intact ledger.
    DriverAdopted,
    /// The run was ended by `stop`.
    RunStopped,
    /// An in-flight dispatch recorded nothing past the stall threshold.
    QuietWorker,
    /// A round exceeded its budget and cooperatively cancelled its workers.
    RoundBudgetExceeded,
    /// A request a round boundary depends on was retried.
    BoundaryRetried,
    /// A cross-DAG edge resolved, with how far its upstream had got when it did.
    CrossDagSatisfied,
    /// A cross-DAG upstream advanced after its consumer recorded it.
    UpstreamModified,
    /// The planner requested completion, independently of graph mutation.
    CompletionRequested,
}

impl PipelineKind {
    /// The kind as it appears on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RunStarted => "run-started",
            Self::ConcurrentAcknowledged => "concurrent-acknowledged",
            Self::RoundStarted => "round-started",
            Self::RoundFinished => "round-finished",
            Self::NodeDispatched => "node-dispatched",
            Self::NodeSettled => "node-settled",
            Self::EditCommitted => "edit-committed",
            Self::EditRejected => "edit-rejected",
            Self::PlannerSurfaceQueued => "planner-surface-queued",
            Self::PlannerSurfaced => "planner-surfaced",
            Self::PlannerReplied => "planner-replied",
            Self::HumanAttested => "human-attested",
            Self::DriverAdopted => "driver-adopted",
            Self::RunStopped => "run-stopped",
            Self::QuietWorker => "quiet-worker",
            Self::RoundBudgetExceeded => "round-budget-exceeded",
            Self::BoundaryRetried => "boundary-retried",
            Self::CrossDagSatisfied => "cross-dag-satisfied",
            Self::UpstreamModified => "upstream-modified",
            Self::CompletionRequested => "completion-requested",
        }
    }

    /// The kind an envelope carries, when it is one of this library's own.
    ///
    /// `None` for anything else, which is every kind a sibling produced: the
    /// merged store holds all three vocabularies and only this one is closed.
    pub fn from_wire(kind: &EventKind) -> Option<Self> {
        PIPELINE_KINDS
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == kind.0)
    }
}

impl std::fmt::Display for PipelineKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<PipelineKind> for EventKind {
    fn from(kind: PipelineKind) -> Self {
        Self(kind.as_str().to_string())
    }
}

/// Every kind, for the lookup above and for the contract's own list.
pub const PIPELINE_KINDS: &[PipelineKind] = &[
    PipelineKind::RunStarted,
    PipelineKind::ConcurrentAcknowledged,
    PipelineKind::RoundStarted,
    PipelineKind::RoundFinished,
    PipelineKind::NodeDispatched,
    PipelineKind::NodeSettled,
    PipelineKind::EditCommitted,
    PipelineKind::EditRejected,
    PipelineKind::PlannerSurfaceQueued,
    PipelineKind::PlannerSurfaced,
    PipelineKind::PlannerReplied,
    PipelineKind::HumanAttested,
    PipelineKind::DriverAdopted,
    PipelineKind::RunStopped,
    PipelineKind::QuietWorker,
    PipelineKind::RoundBudgetExceeded,
    PipelineKind::BoundaryRetried,
    PipelineKind::CrossDagSatisfied,
    PipelineKind::UpstreamModified,
    PipelineKind::CompletionRequested,
];

/// A reference to evidence stored beside the stream rather than inside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// The id the producing library's CLI fetches this artifact by.
    pub id: ArtifactId,
    /// What the artifact is, e.g. `log`.
    pub kind: String,
    /// Its size in bytes.
    pub bytes: u64,
}

/// The id of a stored artifact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArtifactId(pub String);
