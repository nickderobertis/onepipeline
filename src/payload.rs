//! The payload each of this crate's own kinds carries, as a registered bus
//! message.
//!
//! One [`Message`] per [`PipelineKind`], under `agent.pipeline.<kind>@2` — the
//! envelope version those records are written at — and every one of them in the
//! registry [`crate::event::registry`] constructs, beside the agent profile's
//! envelope that carries it. The merged stream's own kinds are then declared in
//! the same registry as the siblings' (`agent.agentgraph.<kind>`), and a payload
//! is checkable against a document rather than against whichever reader folds it.
//!
//! A document states what every writer of that kind **always** writes, and
//! leaves optional what a writer omits: a key one emit site adds and another does
//! not, a key the fold defaults when absent because a record written before it
//! existed carries none, and a key whose value may be `null`. Nested values this
//! crate reads through a type of its own — a plan, a command, an operation, a
//! hold reason, a note — are held to their JSON kind here and to their own schema
//! where they are read. A key no document names is not refused: the fold ignores
//! one, and a record a later build wrote is the ordinary contents of a runs root.

use onemessagebus::{Message, Registry, SchemaId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::event::PipelineKind;

/// A JSON object whose members this crate reads through a type of its own.
type Object = Map<String, Value>;

// Each closed word a payload carries is an enum here, spelled as its emitter writes
// it and built from the type that owns the vocabulary through an exhaustive `From`:
// a word that owner adds fails to compile here, and
// `tests::every_payload_word_is_its_owners_own_spelling` holds each spelling to the
// owner's own.

/// A node's status, as a settlement records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Status {
    /// `pending`.
    Pending,
    /// `ready`.
    Ready,
    /// `running`.
    Running,
    /// `waiting`.
    Waiting,
    /// `blocked`.
    Blocked,
    /// `parked`.
    Parked,
    /// `cancelled`.
    Cancelled,
    /// `done`.
    Done,
    /// `complete-but-draft`.
    #[serde(rename = "complete-but-draft")]
    CompleteDraft,
    /// `failed`.
    Failed,
    /// `skipped`.
    Skipped,
}

impl From<crate::graph::NodeStatus> for Status {
    fn from(status: crate::graph::NodeStatus) -> Self {
        use crate::graph::NodeStatus as S;
        match status {
            S::Pending => Self::Pending,
            S::Ready => Self::Ready,
            S::Running => Self::Running,
            S::Waiting => Self::Waiting,
            S::Blocked => Self::Blocked,
            S::Parked => Self::Parked,
            S::Cancelled => Self::Cancelled,
            S::Done => Self::Done,
            S::CompleteDraft => Self::CompleteDraft,
            S::Failed => Self::Failed,
            S::Skipped => Self::Skipped,
        }
    }
}

/// Whether a published change reached its base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LandingWord {
    /// `landed`.
    Landed,
    /// `unlanded`.
    Unlanded,
}

impl From<crate::graph::Landing> for LandingWord {
    fn from(landing: crate::graph::Landing) -> Self {
        match landing {
            crate::graph::Landing::Landed => Self::Landed,
            crate::graph::Landing::Unlanded => Self::Unlanded,
        }
    }
}

/// Who submitted a command or a reply.
// llmlint: ignore-block[invalid_states_unrepresentable] the word of an `Author` this
// crate already validated when it accepted the envelope, and nothing else builds one.
// It reads back as a bare word on purpose: a recorded run is never refused for its
// author (Contract A), so a payload naming a word no configuration declares any more
// still folds and renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub(crate) struct AuthorWord(String);
// llmlint: ignore-end[invalid_states_unrepresentable]

impl From<crate::channel::Author> for AuthorWord {
    fn from(author: crate::channel::Author) -> Self {
        Self(author.as_str().to_owned())
    }
}

/// Which of the two release styles a target is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReleaseStyleWord {
    /// `automated`.
    Automated,
    /// `human-step`.
    HumanStep,
}

impl From<onevcs::releases::ReleaseStyle> for ReleaseStyleWord {
    fn from(style: onevcs::releases::ReleaseStyle) -> Self {
        match style {
            onevcs::releases::ReleaseStyle::Automated => Self::Automated,
            onevcs::releases::ReleaseStyle::HumanStep => Self::HumanStep,
        }
    }
}

/// Where a release note was delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DeliveryWord {
    /// `live`: into the running turn.
    Live,
    /// `next`: onto the node's next dispatch.
    Next,
}

impl From<crate::edits::Delivery> for DeliveryWord {
    fn from(delivery: crate::edits::Delivery) -> Self {
        match delivery {
            crate::edits::Delivery::Live => Self::Live,
            crate::edits::Delivery::Deferred => Self::Next,
        }
    }
}

/// What checking a criterion answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AnswerWord {
    /// `match`.
    Match,
    /// `mismatch`.
    Mismatch,
    /// `unread`.
    Unread,
}

impl From<&crate::criteria::Answer> for AnswerWord {
    fn from(answer: &crate::criteria::Answer) -> Self {
        match answer {
            crate::criteria::Answer::Match => Self::Match,
            crate::criteria::Answer::Mismatch { .. } => Self::Mismatch,
            crate::criteria::Answer::Unread { .. } => Self::Unread,
        }
    }
}

/// How a drafting dispatch ended without a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EndingWord {
    /// `dispatch-failed`.
    DispatchFailed,
    /// `schema-refused`.
    SchemaRefused,
    /// `no-body`.
    NoBody,
}

impl From<&crate::lifecycle::Undrafted> for EndingWord {
    fn from(undrafted: &crate::lifecycle::Undrafted) -> Self {
        match undrafted {
            crate::lifecycle::Undrafted::Dispatch(_) => Self::DispatchFailed,
            crate::lifecycle::Undrafted::SchemaRefused => Self::SchemaRefused,
            crate::lifecycle::Undrafted::Bodyless => Self::NoBody,
        }
    }
}

/// Where a note reached, as its record's tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReachedWord {
    /// `queued`.
    Queued,
    /// `worker`.
    Worker,
    /// `supervisor`.
    Supervisor,
    /// `judged-with`.
    JudgedWith,
    /// `carried`.
    Carried,
}

impl From<&crate::note::Reached> for ReachedWord {
    fn from(reached: &crate::note::Reached) -> Self {
        use crate::note::Reached as R;
        match reached {
            R::Queued => Self::Queued,
            R::Worker => Self::Worker,
            R::Supervisor => Self::Supervisor,
            R::JudgedWith { .. } => Self::JudgedWith,
            R::Carried => Self::Carried,
        }
    }
}

/// What a `note-shown` was decided from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EvidenceWord {
    /// `delivered-origin`.
    DeliveredOrigin,
    /// `opening-task`.
    OpeningTask,
    /// `instruction-text`.
    InstructionText,
    /// `answering-turn`.
    AnsweringTurn,
}

impl From<crate::note::Evidence> for EvidenceWord {
    fn from(evidence: crate::note::Evidence) -> Self {
        use crate::note::Evidence as E;
        match evidence {
            E::DeliveredOrigin => Self::DeliveredOrigin,
            E::OpeningTask => Self::OpeningTask,
            E::InstructionText => Self::InstructionText,
            E::AnsweringTurn => Self::AnsweringTurn,
        }
    }
}

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] these documents are
// not a second copy maintained beside the emitters: `Journal::emit` checks every record
// it appends against its kind's document in every debug build of the binary, which is
// the build each end-to-end and note journey drives, and panics naming the kind and the
// refusal, so an emit site that drifts fails the journeys that reach it. The
// `every_kinds_recorded_envelope_validates_against_its_registered_document` test
// below holds every recorded envelope to its kind's document. The words declared above
// this block are built from their owning types through an exhaustive `From`, and
// `every_payload_word_is_its_owners_own_spelling` holds each spelling to the owner's.
// The run-hook words declared inside it are not: their owners in `src/hooks.rs` are
// private to that module. `WithheldSettlementWord` is held by that same spelling test
// to `hooks::PAUSED`, the constant its emitter writes. `HookWord`, `HookReasonWord` and
// `HookEndingWord` have neither a `From` nor a spelling test: they are held by that
// document check and by `tests/e2e/run_end_hooks.rs`, whose journeys drive the debug
// binary through every word of the three. Moving every emit site onto these types is a
// rewrite of the emitters across the engine, outside this wire adoption.
/// `run-started`: the plan the run was launched with, and how it is driven.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct RunStarted {
    /// The plan document, read back through the plan schema.
    pub(crate) plan: Object,
    /// The observer graph, `null` when the launch named none.
    pub(crate) graph: Option<String>,
    /// The directory the run was launched from.
    pub(crate) dir: String,
    /// The pacemaker's interval, in seconds.
    pub(crate) heartbeat_interval: u64,
}

/// `concurrent-acknowledged`: a launch beside live repository holders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ConcurrentAcknowledged {
    /// The identities the launch shares with a live holder.
    pub(crate) shared_identities: Vec<String>,
    /// The launching run and the sessions holding them.
    pub(crate) runs: ConcurrentRuns,
    /// Each live holder.
    pub(crate) holders: Vec<Holder>,
}

/// The runs a concurrent launch acknowledged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ConcurrentRuns {
    /// The run being launched.
    pub(crate) launching: String,
    /// The sessions holding a shared identity.
    pub(crate) holding_sessions: Vec<String>,
}

/// One live holder of a shared identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Holder {
    /// The holding session.
    pub(crate) session: String,
    /// The process that owns it.
    pub(crate) owner_pid: u64,
}

/// `node-ready`: carries nothing beyond the node label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NodeReady {}

/// `node-dispatched`: a first dispatch, or a re-dispatch and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NodeDispatched {
    /// Which attempt this is, from 1.
    pub(crate) attempt: u64,
    /// The persona a first dispatch runs under, `null` for none.
    #[serde(default)]
    pub(crate) persona: Option<String>,
    /// A re-dispatch's budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) attempts: Option<u64>,
    /// What the previous attempt ended with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    /// The notes a first dispatch was handed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) notes_spent: Option<Vec<Object>>,
    /// The notes a re-dispatch carried forward.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) notes_carried: Option<Vec<Object>>,
}

/// `node-settled`: the status a node reached, and what it left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NodeSettled {
    /// The terminal status.
    pub(crate) status: Status,
    /// The word its settlement was reached under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) outcome: Option<String>,
    /// The bounded detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    /// The branch the node left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) branch: Option<String>,
    /// The change request its publication opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) change_url: Option<String>,
    /// The producer's classification of a dispatch that died.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cause: Option<String>,
    /// The commit the branch was left at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) head: Option<String>,
    /// Whether the change reached its base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) landing: Option<LandingWord>,
    /// The steps the branch carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) completed_steps: Option<Vec<String>>,
}

/// `edit-committed`: an accepted command that changed something.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct EditCommitted {
    /// Who submitted it.
    pub(crate) author: AuthorWord,
    /// The command as submitted.
    pub(crate) command: Object,
    /// The operations it compiled to.
    pub(crate) operations: Vec<Object>,
    /// Each operation's kind, in order.
    pub(crate) operation_kinds: Vec<String>,
}

/// `command-accepted`: an accepted command that committed nothing a reader folds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct CommandAccepted {
    /// Who submitted it.
    pub(crate) author: AuthorWord,
    /// The command as submitted.
    pub(crate) command: Object,
    /// The operations it compiled to.
    pub(crate) operations: Vec<Object>,
    /// Each operation's kind, in order.
    pub(crate) operation_kinds: Vec<String>,
}

/// `edit-rejected`: a refused command and the reason its submitter was told.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct EditRejected {
    /// Who submitted it.
    pub(crate) author: AuthorWord,
    /// The command as submitted.
    pub(crate) command: Object,
    /// Why it was refused.
    pub(crate) reason: String,
}

/// `planner-surface-queued`: a surface sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct PlannerSurfaceQueued {
    /// The surface's kind.
    // llmlint: ignore-block[invalid_states_unrepresentable] a queued surface's kind is
    // not `channel::SurfaceKind`'s closed set: the engine queues surfaces of its own —
    // the recorded journal carries `quiet-worker` — and `SurfaceKind` is only the kinds
    // an operator may raise from the command line. An enum here would refuse records
    // this crate writes.
    pub(crate) kind: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// What it says.
    pub(crate) message: String,
    /// What raised it.
    pub(crate) source: String,
    /// Whether it holds dependents back.
    pub(crate) blocking: bool,
}

/// `planner-surfaced`: a surface consumed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct PlannerSurfaced {
    /// The surface's kind.
    // llmlint: ignore-block[invalid_states_unrepresentable] a queued surface's kind is
    // not `channel::SurfaceKind`'s closed set: the engine queues surfaces of its own —
    // the recorded journal carries `quiet-worker` — and `SurfaceKind` is only the kinds
    // an operator may raise from the command line. An enum here would refuse records
    // this crate writes.
    pub(crate) kind: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// What it says.
    pub(crate) message: String,
    /// What raised it.
    pub(crate) source: String,
    /// Whether it held dependents back.
    pub(crate) blocking: bool,
    /// When it was queued, in milliseconds; absent from a hand-out recorded
    /// before the instant was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) queued_at: Option<u64>,
}

/// `planner-replied`: the verdict half of a reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct PlannerReplied {
    /// Who replied.
    pub(crate) author: AuthorWord,
    /// Whether it declared completion, `null` for no word.
    #[serde(default)]
    pub(crate) completion: Option<bool>,
    /// Its reason, `null` for none.
    #[serde(default)]
    pub(crate) reason: Option<String>,
}

/// `human-attested`: a human action attested.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct HumanAttested {
    /// The reference attested.
    #[serde(rename = "ref")]
    pub(crate) reference: String,
}

/// `driver-adopted`: a fresh driver on an intact ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct DriverAdopted {
    /// How many adoptions the run has had.
    pub(crate) adoption: u64,
    /// The adopting driver.
    pub(crate) pid: u64,
    /// The dispatches the previous driver left running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) abandoned: Option<Vec<Abandoned>>,
}

/// One dispatch a previous driver left behind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Abandoned {
    /// The node.
    pub(crate) node: String,
    /// Its session.
    pub(crate) session: String,
    /// Its branch.
    pub(crate) branch: String,
}

/// `run-stopped`: the run ended by `stop`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct RunStopped {
    /// Who stopped it.
    pub(crate) owner: String,
    /// Whether it was forced.
    pub(crate) forced: bool,
    /// What its teardown established; absent from a record written before it
    /// was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) teardown: Option<String>,
}

/// `quiet-worker`: a dispatch silent past the threshold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct QuietWorker {
    /// How long it has been silent.
    pub(crate) quiet_for_seconds: u64,
    /// The threshold it passed.
    pub(crate) threshold_seconds: u64,
    /// The persona it runs under.
    pub(crate) persona: String,
}

/// `node-held`: why the loop is not running a node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NodeHeld {
    /// One entry per reason holding it.
    pub(crate) reasons: Vec<Object>,
}

/// `node-unheld`: the hold cleared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NodeUnheld {
    /// The reasons that were holding it.
    pub(crate) released: Vec<Object>,
}

/// `decision-pending`: a blocking surface holding dependents back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct DecisionPending {
    /// The node or surface deciding.
    pub(crate) reference: String,
    /// The decision's kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<String>,
    /// What it holds back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unblocks: Option<Vec<String>>,
}

/// `decision-cleared`: that surface cleared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct DecisionCleared {
    /// The node or surface that decided.
    pub(crate) reference: String,
    /// The decision's kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<String>,
    /// What it released.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) released: Option<Vec<String>>,
}

/// `cross-dag-satisfied`: a cross-DAG edge resolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct CrossDagSatisfied {
    /// The `run:<id>#<node>` reference.
    pub(crate) dependency: String,
    /// How many records the upstream's store held.
    pub(crate) last_seq: u64,
}

/// `upstream-modified`: that upstream advanced afterwards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct UpstreamModified {
    /// The `run:<id>#<node>` reference.
    pub(crate) dependency: String,
    /// What the consumer recorded.
    pub(crate) captured_last_seq: u64,
    /// What the upstream holds now.
    pub(crate) observed_last_seq: u64,
}

/// `completion-requested`: the planner asked to complete.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct CompletionRequested {
    /// Why.
    pub(crate) reason: String,
}

/// `release-wait`: a node waiting on releases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReleaseWait {
    /// The waiting node.
    pub(crate) node: String,
    /// Each release it waits on.
    pub(crate) awaiting: Vec<Object>,
}

/// `release-arrived`: one awaited release happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReleaseArrived {
    /// The waiting node.
    pub(crate) node: String,
    /// The dependency it names.
    pub(crate) dep: String,
    /// The repository identity.
    pub(crate) identity: String,
    /// The version released.
    pub(crate) version: String,
    /// The release target, `null` for none.
    #[serde(default)]
    pub(crate) target: Option<String>,
    /// The release style, `null` for none.
    #[serde(default)]
    pub(crate) style: Option<ReleaseStyleWord>,
}

/// `release-adopted`: a fast-adoption node told its releases arrived.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReleaseAdopted {
    /// The node.
    pub(crate) node: String,
    /// Whether the note reached a running turn or its next dispatch.
    pub(crate) delivery: DeliveryWord,
    /// The releases it was told of.
    pub(crate) versions: Vec<AdoptedVersion>,
}

/// One release a node was told of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct AdoptedVersion {
    /// The repository identity.
    pub(crate) identity: String,
    /// The release target.
    pub(crate) target: String,
    /// The version.
    pub(crate) version: String,
    /// The dependency it names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dep: Option<String>,
    /// The branch it landed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) branch: Option<String>,
    /// The landing commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) commit: Option<String>,
    /// The producer's adoption instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) instructions: Option<String>,
}

/// `criterion-checked`: one checkable criterion against the settled branch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct CriterionChecked {
    /// The criterion's text.
    pub(crate) criterion: String,
    /// The file it names.
    pub(crate) file: String,
    /// The literal it expects.
    pub(crate) expected: String,
    /// `match`, `mismatch`, or `unread`.
    pub(crate) answer: AnswerWord,
    /// What the file holds instead, on a mismatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) holds: Option<String>,
    /// Why it was not read, on an unread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

/// `body-not-drafted`: a drafting dispatch that produced no body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct BodyNotDrafted {
    /// Which of the three endings.
    pub(crate) ending: EndingWord,
    /// The detail the settlement carries too.
    pub(crate) detail: String,
}

/// `note-shown`: a party of a conversation shown a manager's note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NoteShown {
    /// Who the note was addressed to.
    pub(crate) addressee: crate::note::Addressee,
    /// The note.
    pub(crate) text: String,
    /// Where it reached.
    pub(crate) reached: ReachedWord,
    /// The criterion it carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) criterion: Option<String>,
    /// A judged-with delivery's completion reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) completion_reason: Option<String>,
    /// The party shown it.
    pub(crate) party: crate::note::Party,
    /// The turn the presentation opened.
    pub(crate) turn: u64,
    /// What showed the presentation happening.
    pub(crate) evidence: EvidenceWord,
}
/// The word a run-end hook record names its hook by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum HookWord {
    /// Every node settled `done`.
    Success,
    /// The run ended any other way.
    Failure,
}

/// Why the failure hook fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum HookReasonWord {
    /// The graph holds a `failed` or `skipped` node.
    Nodes,
    /// The graph holds a node that is not `done`, none failed or skipped, and no
    /// decision is outstanding.
    Unfinished,
    /// `stop` established a clean teardown.
    Stopped,
}

/// One node that was not `done` when a hook was judged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct UnsettledNode {
    /// The node.
    pub(crate) id: String,
    /// Its status word.
    pub(crate) status: Status,
    /// Its outcome, written as `null` rather than omitted where it has none.
    pub(crate) outcome: Option<String>,
}

/// Why the failure hook fired, and every node not `done` when it was judged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct HookReason {
    /// Why.
    pub(crate) kind: HookReasonWord,
    /// Every node not `done`.
    pub(crate) nodes: Vec<UnsettledNode>,
}

/// `run-hook-fired`: a run-end hook marked as fired, once per ending the run
/// reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct RunHookFired {
    /// Which hook.
    pub(crate) hook: HookWord,
    /// The command the launch record names for it.
    pub(crate) command: String,
    /// Why the failure hook fired; `null` for the success hook.
    pub(crate) reason: Option<HookReason>,
}

/// How a run-end hook ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum HookEndingWord {
    /// Exit status zero.
    Succeeded,
    /// Any other exit status.
    Failed,
    /// The command could not be started.
    CouldNotStart,
    /// The bound on its running elapsed.
    TimedOut,
}

/// `run-hook-finished`: how the fired hook ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct RunHookFinished {
    /// Which hook.
    pub(crate) hook: HookWord,
    /// Its exit status; `null` where it never started or was ended.
    pub(crate) exit: Option<i32>,
    /// How it ended.
    pub(crate) ending: HookEndingWord,
    /// Where its output was captured.
    pub(crate) log: String,
}

/// The settlement a run is withheld its hook under: the one that pauses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum WithheldSettlementWord {
    /// `awaiting-planner`: a decision is outstanding.
    AwaitingPlanner,
}

/// `run-hook-withheld`: a driver let go of a paused run without firing a hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct RunHookWithheld {
    /// The settlement word the run was left under.
    pub(crate) settlement: WithheldSettlementWord,
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// Declares each payload as the bus [`Message`] its kind carries, the total map
/// from a kind to that id, and the registry every one of them is registered in.
macro_rules! payload_messages {
    ($($payload:ident => $wire:literal;)*) => {
        $(
            impl Message for $payload {
                const SCHEMA: SchemaId =
                    SchemaId::literal("agent", concat!("pipeline.", $wire), 2);
            }
        )*

        /// The schema a kind's payload is registered under.
        ///
        /// A match rather than a table, so a kind added without a payload document
        /// does not compile.
        pub(crate) const fn schema_of(kind: PipelineKind) -> SchemaId {
            match kind {
                $(PipelineKind::$payload => <$payload as Message>::SCHEMA,)*
            }
        }

        /// The agent profile's registry, with every payload this crate emits
        /// registered beside the envelope that carries it.
        ///
        /// # Panics
        ///
        /// Never for this build's own types: each document is generated from the
        /// type it names, and each id is distinct.
        pub(crate) fn registry() -> Registry {
            let mut registry = onemessagebus_agent::registry();
            $(
                registry
                    .register::<$payload>()
                    .expect(concat!("the ", $wire, " payload schema registers"));
            )*
            for kind in crate::event::PIPELINE_KINDS {
                debug_assert!(
                    registry.schema(&schema_of(*kind)).is_some(),
                    "{kind} has no payload document in the registry"
                );
            }
            registry
        }
    };
}

payload_messages! {
    RunStarted => "run-started";
    ConcurrentAcknowledged => "concurrent-acknowledged";
    NodeReady => "node-ready";
    NodeDispatched => "node-dispatched";
    NodeSettled => "node-settled";
    EditCommitted => "edit-committed";
    CommandAccepted => "command-accepted";
    EditRejected => "edit-rejected";
    PlannerSurfaceQueued => "planner-surface-queued";
    PlannerSurfaced => "planner-surfaced";
    PlannerReplied => "planner-replied";
    HumanAttested => "human-attested";
    DriverAdopted => "driver-adopted";
    RunStopped => "run-stopped";
    QuietWorker => "quiet-worker";
    NodeHeld => "node-held";
    NodeUnheld => "node-unheld";
    DecisionPending => "decision-pending";
    DecisionCleared => "decision-cleared";
    CrossDagSatisfied => "cross-dag-satisfied";
    UpstreamModified => "upstream-modified";
    CompletionRequested => "completion-requested";
    ReleaseWait => "release-wait";
    ReleaseArrived => "release-arrived";
    ReleaseAdopted => "release-adopted";
    CriterionChecked => "criterion-checked";
    BodyNotDrafted => "body-not-drafted";
    NoteShown => "note-shown";
    RunHookFired => "run-hook-fired";
    RunHookFinished => "run-hook-finished";
    RunHookWithheld => "run-hook-withheld";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{registry, ENVELOPE_VERSION, PIPELINE_KINDS};
    use onemessagebus::CheckError;

    /// Every kind this crate emits has its own document, named for the kind and
    /// the envelope version it is written at, and the registry the crate
    /// constructs holds each one.
    #[test]
    fn every_kind_this_crate_emits_is_a_registered_bus_message() {
        let mut seen = std::collections::BTreeSet::new();
        for kind in PIPELINE_KINDS {
            let id = schema_of(*kind);
            assert_eq!(
                id.to_string(),
                format!("agent.pipeline.{kind}@{ENVELOPE_VERSION}")
            );
            assert!(
                registry().schema(&id).is_some(),
                "{id} is not in the registry this crate constructs"
            );
            assert!(seen.insert(id.clone()), "{id} is claimed by two kinds");
        }
        assert_eq!(
            registry()
                .ids()
                .iter()
                .filter(|id| id.family().starts_with("agent.pipeline."))
                .count(),
            PIPELINE_KINDS.len(),
            "the registry holds a pipeline payload no kind names"
        );
    }

    /// A payload its document does not admit is refused by the registry, and the
    /// refusal names the document it was checked against and where in the payload
    /// the fault is — a key of the wrong type at that key's pointer, and a missing
    /// key at the payload itself.
    #[test]
    fn a_payload_violating_its_document_is_refused_naming_the_id_and_the_pointer() {
        let id = schema_of(PipelineKind::NodeSettled);
        let admitted = serde_json::json!({"status": "done", "outcome": "task-completed"});
        registry()
            .check(&id, &admitted)
            .expect("a settlement this crate writes is admitted");

        let refused =
            |payload: serde_json::Value, pointer: &str| match registry().check(&id, &payload) {
                Err(CheckError::Violation(violation)) => {
                    assert_eq!(violation.id, id);
                    assert_eq!(violation.pointer, pointer, "{violation}");
                    let said = CheckError::Violation(violation).to_string();
                    assert!(
                        said.contains("agent.pipeline.node-settled@2") && said.contains(pointer),
                        "the refusal does not name the id and the pointer: {said}"
                    );
                }
                other => panic!("{payload} was not refused as a violation: {other:?}"),
            };
        refused(serde_json::json!({"status": 5}), "/status");
        refused(
            serde_json::json!({"status": "done", "completed_steps": "implement"}),
            "/completed_steps",
        );
        refused(serde_json::json!({"outcome": "task-completed"}), "");
    }

    /// One envelope of every kind this crate emits, recorded from real runs: the
    /// operator's host's journals for every kind a run there has written, and this
    /// build's own end-to-end journeys for the kinds no host run has, with free
    /// text and host paths stood in for. A kind whose payload takes more than one
    /// shape — `run-hook-fired`'s `null` reason for success and a reason listing
    /// nodes for failure — is recorded once in each.
    const RECORDED: &str = include_str!("../tests/recorded/pipeline-kinds.jsonl");

    /// Every recorded envelope is admitted by its kind's registered document, every
    /// kind has one, and a payload violating that document — a key every writer of
    /// the kind writes, taken away — is refused naming the document and the pointer.
    #[test]
    fn every_kinds_recorded_envelope_validates_against_its_registered_document() {
        let recorded: Vec<crate::event::Envelope> = RECORDED
            .lines()
            .map(|line| serde_json::from_str(line).expect("a recorded envelope reads"))
            .collect();
        for envelope in &recorded {
            let kind = PipelineKind::from_wire(&envelope.kind)
                .unwrap_or_else(|| panic!("{} is not a kind this crate emits", envelope.kind));
            assert_eq!(
                envelope.v, ENVELOPE_VERSION,
                "{kind} was recorded at another version"
            );
            let payload = serde_json::Value::Object(envelope.payload.clone());
            registry()
                .check(&schema_of(kind), &payload)
                .unwrap_or_else(|refusal| panic!("the recorded {kind} is refused: {refusal}"));
        }
        for kind in PIPELINE_KINDS {
            let id = schema_of(*kind);
            let envelope = recorded
                .iter()
                .find(|envelope| PipelineKind::from_wire(&envelope.kind) == Some(*kind))
                .unwrap_or_else(|| panic!("no recorded envelope of {kind}"));

            let document = registry().schema(&id).expect("registered");
            let Some(required) = document["required"]
                .as_array()
                .and_then(|keys| keys.first())
            else {
                continue;
            };
            let key = required.as_str().expect("a required key is a name");
            let mut violating = envelope.payload.clone();
            violating.remove(key);
            match registry().check(&id, &serde_json::Value::Object(violating)) {
                Err(CheckError::Violation(violation)) => {
                    assert_eq!(violation.id, id);
                    assert_eq!(violation.pointer, "", "{violation}");
                    assert!(
                        violation.to_string().contains(&id.to_string()),
                        "{violation}"
                    );
                }
                other => panic!("{kind} without `{key}` was not refused: {other:?}"),
            }
        }
    }

    /// Every payload word is spelled exactly as the type that owns its vocabulary
    /// spells it, variant by variant — which is what lets a document admit only the
    /// words an emitter can write.
    #[test]
    fn every_payload_word_is_its_owners_own_spelling() {
        fn word<T: Serialize>(value: &T) -> String {
            serde_json::to_value(value)
                .expect("a word serializes")
                .as_str()
                .expect("a word is text")
                .to_owned()
        }
        use crate::graph::NodeStatus as S;
        for status in [
            S::Pending,
            S::Ready,
            S::Running,
            S::Waiting,
            S::Blocked,
            S::Parked,
            S::Cancelled,
            S::Done,
            S::CompleteDraft,
            S::Failed,
            S::Skipped,
        ] {
            assert_eq!(word(&Status::from(status)), status.as_str());
        }
        for landing in [
            crate::graph::Landing::Landed,
            crate::graph::Landing::Unlanded,
        ] {
            assert_eq!(word(&LandingWord::from(landing)), landing.as_str());
        }
        for author in [
            crate::channel::Author::planner(),
            crate::channel::Author::from("monitor"),
        ] {
            assert_eq!(word(&AuthorWord::from(author.clone())), author.as_str());
            assert_eq!(word(&AuthorWord::from(author.clone())), word(&author));
        }
        for style in [
            onevcs::releases::ReleaseStyle::Automated,
            onevcs::releases::ReleaseStyle::HumanStep,
        ] {
            assert_eq!(word(&ReleaseStyleWord::from(style)), style.as_str());
        }
        for delivery in [
            crate::edits::Delivery::Live,
            crate::edits::Delivery::Deferred,
        ] {
            assert_eq!(
                word(&DeliveryWord::from(delivery)),
                crate::engine::adoption_delivery(delivery)
            );
        }
        for answer in [
            crate::criteria::Answer::Match,
            crate::criteria::Answer::Mismatch {
                holds: String::new(),
            },
            crate::criteria::Answer::Unread {
                reason: String::new(),
            },
        ] {
            assert_eq!(word(&AnswerWord::from(&answer)), answer.as_str());
        }
        for undrafted in [
            crate::lifecycle::Undrafted::Dispatch(String::new()),
            crate::lifecycle::Undrafted::SchemaRefused,
            crate::lifecycle::Undrafted::Bodyless,
        ] {
            assert_eq!(word(&EndingWord::from(&undrafted)), undrafted.ending());
        }
        use crate::note::Reached as R;
        for reached in [
            R::Queued,
            R::Worker,
            R::Supervisor,
            R::JudgedWith {
                completion_reason: String::new(),
            },
            R::Carried,
        ] {
            let tagged = serde_json::to_value(&reached).expect("a delivery serializes");
            assert_eq!(
                serde_json::json!(word(&ReachedWord::from(&reached))),
                tagged["reached"]
            );
        }
        assert_eq!(
            word(&WithheldSettlementWord::AwaitingPlanner),
            crate::hooks::PAUSED
        );
        use crate::note::Evidence as E;
        for evidence in [
            E::DeliveredOrigin,
            E::OpeningTask,
            E::InstructionText,
            E::AnsweringTurn,
        ] {
            assert_eq!(word(&EvidenceWord::from(evidence)), word(&evidence));
        }
    }
}
