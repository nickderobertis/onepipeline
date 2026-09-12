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
// emits. And the envelope's semantic checks — that `ts` is millisecond-precision UTC
// RFC 3339, that a text field was truncated at `MAX_PAYLOAD_TEXT_BYTES` — belong to the
// reader seam that parses a stream, which is exactly what this stage does not implement.
// The structural boundary *is* enforced: an unknown `source`, a `seq` that is not a
// `u64`, or a missing field is rejected by serde and asserted in `tests/contract.rs`.
// `v` is deliberately **not** refused here either: a runs root holds journals from every
// build that ever wrote into it, so a version this build does not read is a record to
// report rather than a line to reject — `Envelope::written_at_a_known_version` is the
// question, and `src/projection.rs`'s fold is what answers it.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The envelope version this crate stamps on everything it writes.
///
/// **2** since the journal's record shapes moved: at `1` an `edit-committed` may
/// be an accepted command that changed nothing; at `2` it means something changed,
/// an accepted command that did not is [`PipelineKind::CommandAccepted`], and both
/// kinds carry `operation_kinds` so a reader keys on what happened without
/// deserializing the command. A runs root outlives the build that wrote into it,
/// which is why the number is the question a reader of one has. Entry 65 of
/// `docs/contract-divergences.md` proposes the move.
///
/// A **relayed** envelope keeps its producer's own number, exactly as it keeps
/// that producer's `stream`, `seq`, `source` and kind: the version says which
/// build wrote the envelope, and a sibling's is that library's to declare. So one
/// run's journal carries both, and that is not a disagreement.
pub const ENVELOPE_VERSION: u32 = 2;

/// Every envelope version this build reads, newest first.
///
/// The number an envelope declares is the schema its author wrote it against;
/// what a reader asks is whether this build knows that schema. A runs root holds
/// journals from every build that ever wrote into it, so reading the older one is
/// not a courtesy — it is the ordinary case, and
/// [`Envelope::written_at_a_known_version`] is where it is asked.
///
/// Version `1` is read whole: nothing was removed from the envelope or from a
/// record's payload, so a `1` folds exactly as it always did. What `2` adds is
/// what a v1 record cannot promise, which is why the number is worth carrying at
/// all.
pub const ENVELOPE_VERSIONS_READ: &[u32] = &[ENVELOPE_VERSION, 1];

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
    /// Envelope version: [`ENVELOPE_VERSION`] for anything this crate writes, and
    /// the producer's own for anything it relayed.
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
    /// Which part of a change's life the event belongs to, as its producer
    /// classified it.
    ///
    /// Stamped by the producer and never derived here: one kind's phase is not a
    /// fact about the kind — `onevcs` classifies a push of the session's own
    /// branch and a push of the base it landed on differently, and only the
    /// thing that made the push knows which it was — so this is relayed exactly
    /// as it arrived.
    ///
    /// `None` for a producer that stamps none, which is every `oneagentgraph`
    /// envelope, everything this crate emits, and every `onevcs` record written
    /// before that library stamped one. Omitted from the wire when absent, so a
    /// store written before this field round-trips as its writer wrote it. See
    /// `docs/contract-divergences.md` entry 40.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,
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

impl Envelope {
    /// Whether this build knows the envelope schema this record was written at.
    ///
    /// Asked of **this library's own** records and never of a relayed one: a
    /// sibling's version is that library's own vocabulary, and judging it by this
    /// crate's table would refuse a producer for moving at its own pace.
    ///
    /// A reader that meets `false` has met a record a *newer* build wrote, whose
    /// kinds or payload may mean something this build would read wrongly. What
    /// the fold does with that is report rather than guess — it marks the run as
    /// one it could not read whole, and a driver says so before it converges —
    /// because a record mis-folded silently is worse than a run said to be
    /// incompletely understood.
    #[must_use]
    pub fn written_at_a_known_version(&self) -> bool {
        ENVELOPE_VERSIONS_READ.contains(&self.v)
    }
}

/// Which part of a change's life an event belongs to.
///
/// `onevcs`'s own four, relayed as that library stamps them: the work is made
/// ([`Development`](Self::Development)), it is brought together with the base it
/// is going onto ([`Integrate`](Self::Integrate)), it is proposed and ruled on
/// ([`Review`](Self::Review)), and what carries it is released
/// ([`Release`](Self::Release)).
///
/// A closed set here where [`EventKind`] is a wire string, and the difference is
/// which side owns the vocabulary: a kind is one of three libraries' and this
/// crate relays all three, while a phase is `onevcs`'s alone and `src/vcs.rs`
/// converts it arm by arm — so a phase that library adds fails to compile here
/// rather than arriving as a string nothing folds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// The work is being made.
    Development,
    /// The work is being brought together with the base.
    Integrate,
    /// The change request is open and being ruled on.
    Review,
    /// What carries the landed change is being released.
    Release,
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
/// The reserved keys are the ones `docs/contract.md` names on a
/// [`DispatchRequest`](crate::executor::DispatchRequest); anything else a
/// producer stamps rides in [`extra`](Self::extra). Enrichers never rewrite what
/// is already there.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Labels {
    /// The run this event belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The round within the run. **Deprecated and never stamped:** execution is
    /// continuous, so there is no round to name.
    ///
    /// The field survives because this envelope is duplicated across the three
    /// libraries and the siblings still declare it — dropping it here would make
    /// one copy of a shared wire shape reject what another one writes. It is
    /// read and re-serialized as it arrives and is `None` on everything this
    /// crate produces.
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
    /// Every dependency of a node has settled `done`, so it may dispatch now.
    NodeReady,
    /// A node's dispatch was started.
    NodeDispatched,
    /// A node reached a terminal status.
    NodeSettled,
    /// A live edit was accepted, and committing it is what made the change it
    /// records.
    ///
    /// Emitted only where at least one operation the command compiled to
    /// **changed** something a reader folding this record moves —
    /// `Operation::commits_a_change` decides that, exhaustively over the
    /// operations. That is the desired graph *and* the node record derived beside
    /// it, because those are one durable document to such a reader: an
    /// attestation, a park and a settlement from evidence move a node's recorded
    /// state without moving the graph, and are read back off this record by name.
    /// "Something changed" is what a reader has always taken this kind to mean,
    /// and it is exactly what it still means. The kinds of the operations it
    /// committed ride on the record as `operation_kinds`, so a reader keys on
    /// what happened without parsing the command that produced it.
    EditCommitted,
    /// A command was accepted and committed nothing a reader folds.
    ///
    /// The other half of the split above: a `finding` and a `complete` are
    /// accepted, are answered `applied`, and change nothing — each is a
    /// **report**, whose own record is a planner surface and a
    /// `completion-requested` respectively — so journalling them as committed
    /// edits made one kind carry two meanings.
    CommandAccepted,
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
    /// The loop is not running a node it has not settled, and this is why.
    ///
    /// Written when a hold **begins**, again when what the node is held by
    /// **changes**, and never on a pass where it is held by what it was held by
    /// before. `reasons` carries one entry per reason holding it at once, so a
    /// node behind three running nodes and a node whose dependency has not
    /// settled and a node that is both are three answers a reader tells apart
    /// without joining another record.
    NodeHeld,
    /// That hold cleared, carrying the reasons that were holding it.
    NodeUnheld,
    /// A blocking surface began holding a subtree of dependents back.
    DecisionPending,
    /// That surface was cleared, and the subtree it held was released.
    DecisionCleared,
    /// A cross-DAG edge resolved, with how far its upstream had got when it did.
    CrossDagSatisfied,
    /// A cross-DAG upstream advanced after its consumer recorded it.
    UpstreamModified,
    /// The planner requested completion, independently of graph mutation.
    CompletionRequested,
    /// A node is held under `published` adoption, waiting on releases.
    ///
    /// Raised when the wait begins and again on its own interval, so a wait
    /// nobody has ended cannot go silent. Each awaited release names its style,
    /// so a wait on a machine and a wait on a person are tellable apart from the
    /// payload as well as from the surface beside it.
    ReleaseWait,
    /// One release a node was waiting on has happened.
    ReleaseArrived,
    /// A fast-adoption node was told the releases it was waiting on arrived, and
    /// whether the note reached a running turn or its next dispatch.
    ReleaseAdopted,
    /// One mechanically checkable acceptance criterion was compared against the
    /// branch its node settled on.
    ///
    /// Emitted for every criterion this build could parse into "this named file
    /// holds this literal", carrying the answer — `match`, `mismatch`, or
    /// `unread`, the check declining to answer a file it could not read. A
    /// criterion it could not parse is not recorded at all: the check says
    /// nothing about prose it has no business ruling on.
    CriterionChecked,
    /// A drafting dispatch ran for a change request's body and produced none.
    ///
    /// Only where one was *configured and attempted*: a launch that named no
    /// pr-author graph, and a node that carried its own `body`, both spend no
    /// dispatch and neither is a failure to report. The payload's `ending` says
    /// which of the three it was, because they need three different fixes.
    BodyNotDrafted,
    /// A party of a node's conversation was **shown** a manager's note.
    ///
    /// Written when the relayed stream shows the presentation happening — the
    /// worker's turn opening on the note, the supervisor's turn opening after
    /// it — and never at delivery, where the conversation has only said where
    /// it *will* route the note. `note-delivered` records what was confirmed
    /// the moment the conversation acknowledged the note and what it routed
    /// onward; this is the record for each routed presentation that then
    /// happened, so a conversation interrupted between the two leaves no claim
    /// that the second party saw anything. Carries `party`, the `turn` the
    /// presentation opened, and the note.
    NoteShown,
}

impl PipelineKind {
    /// The kind as it appears on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RunStarted => "run-started",
            Self::ConcurrentAcknowledged => "concurrent-acknowledged",
            Self::NodeReady => "node-ready",
            Self::NodeDispatched => "node-dispatched",
            Self::NodeSettled => "node-settled",
            Self::EditCommitted => "edit-committed",
            Self::CommandAccepted => "command-accepted",
            Self::EditRejected => "edit-rejected",
            Self::PlannerSurfaceQueued => "planner-surface-queued",
            Self::PlannerSurfaced => "planner-surfaced",
            Self::PlannerReplied => "planner-replied",
            Self::HumanAttested => "human-attested",
            Self::DriverAdopted => "driver-adopted",
            Self::RunStopped => "run-stopped",
            Self::QuietWorker => "quiet-worker",
            Self::NodeHeld => "node-held",
            Self::NodeUnheld => "node-unheld",
            Self::DecisionPending => "decision-pending",
            Self::DecisionCleared => "decision-cleared",
            Self::CrossDagSatisfied => "cross-dag-satisfied",
            Self::UpstreamModified => "upstream-modified",
            Self::CompletionRequested => "completion-requested",
            Self::ReleaseWait => "release-wait",
            Self::ReleaseArrived => "release-arrived",
            Self::ReleaseAdopted => "release-adopted",
            Self::CriterionChecked => "criterion-checked",
            Self::BodyNotDrafted => "body-not-drafted",
            Self::NoteShown => "note-shown",
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
    PipelineKind::NodeReady,
    PipelineKind::NodeDispatched,
    PipelineKind::NodeSettled,
    PipelineKind::EditCommitted,
    PipelineKind::CommandAccepted,
    PipelineKind::EditRejected,
    PipelineKind::PlannerSurfaceQueued,
    PipelineKind::PlannerSurfaced,
    PipelineKind::PlannerReplied,
    PipelineKind::HumanAttested,
    PipelineKind::DriverAdopted,
    PipelineKind::RunStopped,
    PipelineKind::QuietWorker,
    PipelineKind::NodeHeld,
    PipelineKind::NodeUnheld,
    PipelineKind::DecisionPending,
    PipelineKind::DecisionCleared,
    PipelineKind::CrossDagSatisfied,
    PipelineKind::UpstreamModified,
    PipelineKind::CompletionRequested,
    PipelineKind::ReleaseWait,
    PipelineKind::ReleaseArrived,
    PipelineKind::ReleaseAdopted,
    PipelineKind::CriterionChecked,
    PipelineKind::BodyNotDrafted,
    PipelineKind::NoteShown,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The envelope this build writes, as a document.
    const GOLDEN: &str = include_str!("../tests/golden/envelope-v2.json");

    /// One this build did **not** write, at the version before the bump.
    const GOLDEN_BEFORE: &str = include_str!("../tests/golden/envelope-v1.json");

    /// The shape this build stamps its own records with, held to a committed
    /// document.
    ///
    /// A runs root outlives the build that wrote into it and is read by things
    /// outside this repository, so the envelope is a published document rather
    /// than an internal struct: a field renamed, an optional one becoming an
    /// explicit null, or the version moving without anyone deciding to move it
    /// are all things a consumer finds out about by breaking.
    #[test]
    fn the_envelope_this_build_writes_is_the_committed_golden() {
        let golden: serde_json::Value = serde_json::from_str(GOLDEN).expect("the golden is JSON");
        assert_eq!(
            golden["v"],
            json!(ENVELOPE_VERSION),
            "the golden is not at the version this build writes. Bump ENVELOPE_VERSION and \
             add tests/golden/envelope-v<n>.json together, keeping the older file as the \
             read-compatibility reference"
        );

        let envelope: Envelope = serde_json::from_value(golden.clone()).expect("it parses");
        assert_eq!(envelope.v, ENVELOPE_VERSION);
        assert_eq!(envelope.source, Source::Pipeline);
        assert_eq!(envelope.kind, EventKind("edit-committed".into()));
        assert!(envelope.written_at_a_known_version());
        assert_eq!(
            serde_json::to_value(&envelope).expect("it serializes"),
            golden,
            "the envelope changed shape. Bump ENVELOPE_VERSION, add the golden for the new \
             one, and keep this file as what the older version looked like"
        );
    }

    /// And the one before it still reads, whole, and comes back out unchanged.
    ///
    /// The version bump is a statement about what a **new** record promises, not
    /// a line drawn under the old ones: a v1 journal is the ordinary contents of
    /// a runs root, and this build folds it. It is also not restamped — reading a
    /// record does not make it this build's — so a store round-trips as its
    /// writer wrote it.
    #[test]
    fn the_envelope_version_before_this_one_is_still_read_and_never_restamped() {
        let before: serde_json::Value =
            serde_json::from_str(GOLDEN_BEFORE).expect("the golden is JSON");
        assert_eq!(before["v"], json!(1));
        assert!(
            ENVELOPE_VERSIONS_READ.contains(&1),
            "this build no longer reads the version its committed fixture is written at"
        );

        let envelope: Envelope = serde_json::from_value(before.clone()).expect("it parses");
        assert!(envelope.written_at_a_known_version());
        assert_eq!(envelope.v, 1, "a version this build read was rewritten");
        assert_eq!(
            serde_json::to_value(&envelope).expect("it serializes"),
            before
        );

        // And a version nothing has published is not read, which is what makes
        // the set above a statement rather than a comment.
        let ahead: Envelope = serde_json::from_value(json!({
            "v": ENVELOPE_VERSION + 1,
            "ts": "2026-09-09T04:00:00.000Z",
            "stream": "onepipeline-7f3a",
            "seq": 43,
            "source": "pipeline",
            "kind": "edit-committed",
            "labels": {},
            "payload": {},
            "artifacts": []
        }))
        .expect("a newer build's record still parses structurally");
        assert!(!ahead.written_at_a_known_version());
    }

    /// The optional fields are optional in both directions: absent stays absent
    /// on the wire, and present survives the trip.
    ///
    /// `phase` is the one an envelope declares, and it is the field a store
    /// written before there was a phase depends on: a build that serialized it as
    /// an explicit null would rewrite every such record the first time it read
    /// one back.
    #[test]
    fn an_envelopes_optional_fields_round_trip_and_are_omitted_when_empty() {
        let bare = json!({
            "v": ENVELOPE_VERSION,
            "ts": "2026-09-09T04:00:00.000Z",
            "stream": "onepipeline-7f3a",
            "seq": 1,
            "source": "pipeline",
            "kind": "node-ready",
            "labels": {},
            "payload": {},
            "artifacts": []
        });
        let envelope: Envelope = serde_json::from_value(bare.clone()).expect("it parses");
        assert_eq!(envelope.phase, None);
        assert_eq!(serde_json::to_value(&envelope).expect("serializes"), bare);

        let mut with = bare.clone();
        with["phase"] = json!("release");
        let envelope: Envelope = serde_json::from_value(with.clone()).expect("it parses");
        assert_eq!(envelope.phase, Some(Phase::Release));
        assert_eq!(serde_json::to_value(&envelope).expect("serializes"), with);

        // The three defaulted containers are the same promise: a record that
        // omitted them reads, and comes back out omitting nothing it carried.
        let minimal = json!({
            "v": ENVELOPE_VERSION,
            "ts": "2026-09-09T04:00:00.000Z",
            "stream": "onepipeline-7f3a",
            "seq": 2,
            "source": "pipeline",
            "kind": "node-ready"
        });
        let envelope: Envelope = serde_json::from_value(minimal).expect("it parses");
        assert!(envelope.payload.is_empty() && envelope.artifacts.is_empty());
        assert_eq!(envelope.labels, Labels::default());
    }
}
