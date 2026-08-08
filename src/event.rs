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
/// Merge order across streams is `(ts, stream, seq)`. A consumer detects loss
/// through per-stream [`seq`](Self::seq) gaps; there are no cross-stream
/// ordering promises beyond the timestamps.
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
