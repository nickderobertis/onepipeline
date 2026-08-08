//! The planner channel's wire shapes.
//!
//! A reply is one JSON envelope: a legacy verdict, a version-1 list of graph
//! edits, or both. The edits' required fields and validation semantics are
//! `ai-orchestrator`'s live-edit protocol exactly, per `docs/contract.md`.
//!
//! These types are the wire shape and nothing else. Nothing here validates an
//! edit against a live frontier, queues it, reconciles it, or answers it — the
//! applied-or-rejected-with-reason promise is the reconciler's, and the
//! reconciler is what this stage does not implement.

// llmlint: ignore-file[invalid_states_unrepresentable] every node id, dependency
// reference, and human-action reference here is a `String` because a `NodeId`/`NodeRef`
// newtype is a public item `docs/contract.md` does not name, and minting one is the
// interface drift the interface-only stage forbids (see src/AGENTS.md). `version` and
// `completion` stay independent optionals for the same reason: the contract's envelope is
// "legacy verdicts *plus* a version-1 command list", so a reply may legally carry either,
// both, or a version this build does not know — and collapsing that into one enum would
// reject envelopes the protocol accepts. Narrow all of it when the reconciler lands with
// a graph to validate references against.

// llmlint: ignore-file[boundary_inputs_validated] a reply is external input and its
// *structural* boundary is enforced here — an unknown `op`, a missing required field, or
// an unknown key is rejected by serde and asserted in `tests/contract.rs`. The *semantic*
// validation the contract specifies (the target exists, is in the right state, and the
// resulting graph is still acyclic) is a judgement against the live frontier, which is
// the reconciler's and does not exist yet (see AGENTS.md).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::plan::Node;

/// The reply envelope version this crate reads and writes.
pub const REPLY_ENVELOPE_VERSION: u32 = 1;

/// One reply to a planner surface.
///
/// A command-only envelope gets a synthesized continuing verdict; commands can
/// instead accompany either legacy verdict.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    /// [`REPLY_ENVELOPE_VERSION`] when the envelope carries commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// The legacy verdict: whether the planner considers the run complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<bool>,
    /// The legacy verdict's message to the orchestrator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Why the planner reached that verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The graph edits, reconciled in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<Command>,
}

/// What happens to a dropped node's direct dependents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dependents {
    /// Recursively drop them too.
    Drop,
    /// Keep them, detached from the dropped node.
    Detach,
}

/// One graph edit.
///
/// The variants and their required fields are the live-edit protocol's table,
/// unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
#[non_exhaustive]
pub enum Command {
    /// Add a new node. Its `deps`, if any, must name graph nodes or valid
    /// cross-DAG references.
    Add {
        /// The full node mapping.
        node: Node,
    },
    /// Remove the node and recursively drop its dependents, or detach its direct
    /// dependents.
    Drop {
        /// The node to remove.
        id: String,
        /// The dependents' fate. Stating it is required.
        dependents: Dependents,
    },
    /// Replace an unstarted node's dependencies.
    Reparent {
        /// The node to reparent.
        id: String,
        /// Its new dependency references.
        deps: Vec<String>,
    },
    /// Supersede a running, failed, or cancelled node with a fresh lineage and
    /// redirect its direct dependents.
    Retry {
        /// The node to supersede.
        id: String,
        /// The full replacement node mapping, with a new id.
        node: Node,
    },
    /// Park a pending or running node: cancel its dispatch cooperatively and
    /// hold it out of every later round until a `requeue`.
    Cancel {
        /// The node to park.
        id: String,
    },
    /// Return a parked node to the desired frontier, optionally amending it.
    Requeue {
        /// The parked node.
        id: String,
        /// Partial node overrides, merged onto the node before it is
        /// redispatched. It may not rewrite `id` or `deps`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        amend: Option<Map<String, Value>>,
    },
    /// Complete a currently ready, waiting human action.
    Attest {
        /// The human action's reference.
        #[serde(rename = "ref")]
        reference: String,
    },
    /// Journal the planner's completion request, independently of graph
    /// mutation.
    Complete {
        /// Why the planner considers the run complete.
        reason: String,
    },
    /// Attach one planner note to the node's next dispatch, without cancelling
    /// or restarting anything.
    Context {
        /// The node the note is for.
        id: String,
        /// The note. It carries exactly one round.
        note: String,
    },
}

/// What a planner surface is asking about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum SurfaceKind {
    /// The durable planner-update pacemaker came due. Consuming one resets that
    /// clock through `oneagentgraph reset-timer RUN check-in`.
    CheckIn,
}
