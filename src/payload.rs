//! The payload each of this crate's own kinds carries, as a registered bus
//! message.
//!
//! One [`Message`] per [`PipelineKind`], under `agent.pipeline.<kind>@2` — the
//! envelope version those records are written at — and every one of them in the
//! registry [`registry`] constructs, beside the [`crate::vocabulary`] envelope
//! that carries it. The merged stream's own kinds are then declared in the same
//! registry as the siblings' (`agent.agentgraph.<kind>`), and a payload is
//! checkable against a document rather than against whichever reader folds it.
//!
//! [`registry`] and [`schema_of`] are the public surface: a reader of a run's
//! journal validates a payload against the document this build registers for
//! its kind, and `schemas/events.json` publishes the same documents for a reader
//! in another language. The payload types themselves stay this crate's — the
//! schemas are the contract, not the Rust shapes behind them.
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
/// The closure rule's marker, `journal::OWED_UNTIL_CLOSED`: `true` where it is
/// written, and absent — never `false` — where it is not, so a payload has no
/// third spelling a reader could take for either. The checkpoint's closure
/// carries it the same way, once any record of the run has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Owed;

impl Serialize for Owed {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for Owed {
    fn deserialize<D: serde::Deserializer<'de>>(reader: D) -> Result<Self, D::Error> {
        if bool::deserialize(reader)? {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(
                "owed_until_closed is written `true` or not at all",
            ))
        }
    }
}

impl JsonSchema for Owed {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Owed".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"const": true})
    }
}

/// The adoption's run-end hook epoch, `journal::ADOPTED_HOOK_EPOCH`: `true`
/// where it is written and absent where it is not, for the reason [`Owed`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HookEpoch;

impl Serialize for HookEpoch {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for HookEpoch {
    fn deserialize<D: serde::Deserializer<'de>>(reader: D) -> Result<Self, D::Error> {
        if bool::deserialize(reader)? {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(
                "hook_epoch is written `true` or not at all",
            ))
        }
    }
}

impl JsonSchema for HookEpoch {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "HookEpoch".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"const": true})
    }
}

/// `run-started`: the plan the run was launched with, and how it is driven.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct RunStarted {
    /// The plan document, read back through the plan schema.
    pub(crate) plan: Object,
    /// The observer graph, `null` when the launch named none.
    pub(crate) graph: Option<String>,
    /// The directory the run was launched from.
    pub(crate) dir: String,
    /// The check-in interval, in seconds.
    pub(crate) heartbeat_interval: u64,
    /// `true` where the launching driver holds the run to the closure rule;
    /// absent from a record an earlier build wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owed_until_closed: Option<Owed>,
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
    // llmlint: ignore-block[invalid_states_unrepresentable] a wire document, as the task's
    // contract spells it: `identity`, `session`, `run`, `node`, `dependency` and `dependents`
    // are strings on the wire, as `session` and `launching` beside them already are, and
    // `onevcs::SessionHolder` carries the identity and the labels as `String` itself; this
    // crate writes them and reads them back only to render them.
    /// The identity it holds; absent from a record an earlier build wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) identity: Option<String>,
    /// The run its session was opened for, from the session's `run` label;
    /// absent for an unattributed holder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) run: Option<String>,
    /// The node its session was opened for, from the session's `node` label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) node: Option<String>,
    /// The `run:<run>#<node>` dependency that also acknowledges it, set only
    /// where one does, so a reader tells those from flag-only holders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dependency: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// `concurrent-deferred`: a launch whose plan depends on live repository
/// holders, so waits for them rather than racing them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ConcurrentDeferred {
    // llmlint: ignore-block[invalid_states_unrepresentable] a wire document, as the task's
    // contract spells it: `identity`, `session`, `run`, `node`, `dependency` and `dependents`
    // are strings on the wire, as `session` and `launching` beside them already are, and
    // `onevcs::SessionHolder` carries the identity and the labels as `String` itself; this
    // crate writes them and reads them back only to render them.
    /// The run being launched.
    pub(crate) launching: String,
    /// Each live holder a dependency acknowledges.
    pub(crate) holders: Vec<DeferredHolder>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// One live holder the launching plan depends on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct DeferredHolder {
    // llmlint: ignore-block[invalid_states_unrepresentable] a wire document, as the task's
    // contract spells it: `identity`, `session`, `run`, `node`, `dependency` and `dependents`
    // are strings on the wire, as `session` and `launching` beside them already are, and
    // `onevcs::SessionHolder` carries the identity and the labels as `String` itself; this
    // crate writes them and reads them back only to render them.
    /// The identity it holds.
    pub(crate) identity: String,
    /// The holding session.
    pub(crate) session: String,
    /// The process that owns it.
    pub(crate) owner_pid: u64,
    /// The run its session was opened for.
    pub(crate) run: String,
    /// The node its session was opened for.
    pub(crate) node: String,
    /// The reference the dependents reach, `run:<run>#<node>`.
    pub(crate) dependency: String,
    /// The launching plan's nodes on the identity, each of which reaches it.
    pub(crate) dependents: Vec<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
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

/// The reason a dispatched node was handed back to the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RequeueReasonWord {
    /// `workspace-exhausted`: its repository identity admitted no session.
    WorkspaceExhausted,
}

/// `node-requeued`: a dispatched node handed back to the queue, unsettled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NodeRequeued {
    /// Which ending handed it back.
    pub(crate) reason: RequeueReasonWord,
    /// The sibling's own account of the refusal.
    pub(crate) detail: String,
    /// The branch a re-dispatch keeps its pin to through the queue. A first
    /// attempt has no pin and its record carries neither this nor `attempt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) branch: Option<String>,
    /// The publication attempt the refusal interrupted, for a pinned re-dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) attempt: Option<u64>,
}

/// `node-settled`: the status a node reached, and what it left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NodeSettled {
    /// The terminal status.
    pub(crate) status: Status,
    /// The word its settlement was reached under. A review draft republished
    /// without a check watch settles change-review-draft-unchecked at its push.
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
    // llmlint: ignore-block[invalid_states_unrepresentable] a wire document, as the stacked
    // drafts contract spells it: `base` is a branch name on the wire, as `branch` beside it
    // already is, and this crate writes it from the session `onevcs` opened and reads it
    // back only to place a dependent and to render it.
    /// The branch the node's change request is based on: the root, a stack
    /// parent's branch, or the stack base merging its stack parents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) base: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// The unlanded same-repository changes the node was drafted on top of,
    /// present exactly when it had any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stack: Option<SettledStack>,
    /// Where a settlement this run did not reach itself came from: present exactly on
    /// the settlement a launch read back from its project's store and continued,
    /// rather than dispatched. Entry 119 of `docs/contract-divergences.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) continued: Option<Continued>,
}

/// A settlement a launch continued from its project's store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Continued {
    /// Rework provenance loaded from the store, journal-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rework: Option<ContinuedRework>,
    /// Where the settlement was read: the project's store.
    pub(crate) source: ContinuedSource,
    // llmlint: ignore-block[invalid_states_unrepresentable] a wire document, as the
    // continuation entry spells it: a run id is a string on the wire, as `run_id` on every
    // label already is.
    /// The run the project's driver claim named when the launch read it, `null` where
    /// the project carried none.
    pub(crate) run: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

// llmlint: ignore-block[invalid_states_unrepresentable] Entry 122 fixes these serialized provenance fields as strings. continuation_of validates the store input before creating this DTO; projection preserves its original metadata rather than executing predecessor text as an instruction.
/// The rework a continued lineage already carries on its store item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContinuedRework {
    pub(crate) replacement: String,
    pub(crate) replaces: String,
    pub(crate) comments: Vec<crate::channel::ReworkComment>,
    /// The rework's reason and instruction, byte-for-byte as the store holds them:
    /// what a reply drafting on this host is handed as `## Reason`. Empty in a
    /// record written before they were carried.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) instruction: String,
    /// The superseded node's settled head, where the store's provenance carries
    /// it: where the commits the rework added begin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) replaces_head: Option<String>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// Where a continued settlement was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ContinuedSource {
    /// The project's own `onepipeline.settlement`, on its store.
    Store,
}

// llmlint: ignore-block[invalid_states_unrepresentable] a wire document, as the continuation
// entry spells it: a run id, a host name, a session and an RFC 3339 instant are strings on the
// wire, as the launch record and `driver-exited` already carry them.
/// A project's `onepipeline.driver` claim: the run driving it, the host and launching
/// session that run is driven from, its driver's pid, and when it claimed.
///
/// The document a journal record carries, and so no stricter than any other payload here;
/// the store's copy is external input, and `continuation::claim_of` refuses one carrying a
/// key this shape does not name before it is read as one.
// llmlint: ignore[boundary_inputs_validated] the rule stated at this module's head: no payload document denies unknown fields; the store boundary checks its keys in `continuation::claim_of`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct DriverClaim {
    /// The run that claims the project.
    pub(crate) run: String,
    /// The host its driver runs on.
    pub(crate) host: String,
    /// The session that launched it, `null` where none was named.
    pub(crate) session: Option<String>,
    /// Its driver's pid, meaningful on `host`.
    pub(crate) pid: u32,
    /// When the claim was written, RFC 3339.
    pub(crate) claimed_at: String,
    /// The terms the claiming driver's lease runs on, which a run taking the project over
    /// waits out; absent from a claim written by a build that kept no lease.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lease: Option<ClaimLease>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// The terms a claiming driver's lease runs on: how often it reads its claim, and how long
/// a dispatch its lease ends has to stop before it is killed.
// llmlint: ignore[boundary_inputs_validated] the rule stated at this module's head: no payload document denies unknown fields; the store boundary checks these in `continuation::claim_of`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ClaimLease {
    /// The driver's claim interval, in whole seconds.
    // llmlint: ignore[invalid_states_unrepresentable] zero is unrepresentable; the day's upper bound is the setting's (`continuation::MAX_CLAIM_INTERVAL`), checked where a claim is read at the store boundary, and a newtype for it would publish a schema of its own in `schemas/events.json` for a bound no reader of a journal record needs.
    pub(crate) interval_seconds: std::num::NonZeroU64,
    /// The driver's cancel grace, in whole seconds.
    pub(crate) cancel_grace_seconds: std::num::NonZeroU64,
}

impl DriverClaim {
    /// Whether this claim names the run `run` driven on `host`: the one test of "names this
    /// run" — a pid changes with every adoption, and a run id alone repeats across hosts.
    pub(crate) fn names(&self, run: &str, host: &str) -> bool {
        self.run == run && self.host == host
    }
}

/// `driver-taken-over`: the claim a `start --take-over` passed.
// llmlint: ignore[boundary_inputs_validated] the rule stated at this module's head: no payload document denies unknown fields, so a newer build's record still reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct DriverTakenOver {
    /// The claim the project carried when the launch read it.
    pub(crate) previous: DriverClaim,
}

/// `driver-superseded`: the claim a driver found naming another run.
// llmlint: ignore[boundary_inputs_validated] the rule stated at this module's head: no payload document denies unknown fields, so a newer build's record still reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct DriverSuperseded {
    /// The claim the driver read.
    pub(crate) by: DriverClaim,
}

/// The stack a lifecycle node was drafted on: the stack parents its branch was
/// placed on, and the stack base merging them when there were several.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SettledStack {
    /// Each stack parent, in the node's declared dependency order: at least one.
    // llmlint: ignore[invalid_states_unrepresentable] a wire document's list, as the stacked drafts contract spells it: its published schema states `minItems: 1`, the fold refuses an empty one (`is_usable`) before anything places on it, and its one writer builds it from `stack::Stack`, whose stack base holds `stack::Several`.
    #[schemars(length(min = 1))]
    pub(crate) parents: Vec<StackParent>,
    // llmlint: ignore-block[invalid_states_unrepresentable] a wire document, as the stacked
    // drafts contract spells it: `stack_base` is the branch `onevcs::StackBase` names as a
    // `String` itself, and `null` where one stack parent's branch is the base.
    /// The stack base the node was placed on, `null` where it had one stack
    /// parent and was placed on that parent's branch.
    #[serde(default)]
    pub(crate) stack_base: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

impl SettledStack {
    /// Whether every node, name and commit it carries is one a view can print on a
    /// line of its own — what [`crate::vcs::usable`] accepts — and its parents are
    /// not empty.
    pub(crate) fn is_usable(&self) -> bool {
        !self.parents.is_empty()
            && self.parents.iter().all(|parent| {
                crate::vcs::usable(&parent.node).is_some()
                    && crate::vcs::usable(&parent.branch).is_some()
                    && crate::vcs::usable(&parent.head).is_some()
            })
            && self
                .stack_base
                .as_deref()
                .is_none_or(|stack_base| crate::vcs::usable(stack_base).is_some())
    }
}

/// One stack parent of a settled node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct StackParent {
    // llmlint: ignore-block[invalid_states_unrepresentable] a wire document, as the stacked
    // drafts contract spells it: a node id, a branch and a commit are strings on the wire,
    // as `branch` and `head` on the settlement itself already are.
    /// The parent node.
    pub(crate) node: String,
    /// Its branch, which the node was placed on or which the stack base merges.
    pub(crate) branch: String,
    /// The commit that branch stood at when the node was placed on it.
    pub(crate) head: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
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
    /// The command envelope it came in. Absent from a record written before the
    /// field existed, and from one no queued envelope produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) envelope: Option<u64>,
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
    /// The command envelope it came in. Absent from a record written before the
    /// field existed, and from one no queued envelope produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) envelope: Option<u64>,
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
    /// The command envelope it came in. Absent from a record written before the
    /// field existed, and from one no queued envelope produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) envelope: Option<u64>,
    /// The refusal by kind, where it was one the reconciler names as one; `reason`
    /// is then its text. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) refusal: Option<crate::channel::Refusal>,
}

/// `planner-surface-queued`: a surface sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct PlannerSurfaceQueued {
    // llmlint: ignore-block[invalid_states_unrepresentable] This optional journal DTO preserves entry 122’s wire string. Surface.correlation is the bus’s validated Correlation before record_surface serializes it; no command is executed from this journal text.
    /// Review revision identity, where the surface names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) correlation: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
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
    /// `true` where the adopting driver holds the run to the closure rule;
    /// absent from a record an earlier build wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owed_until_closed: Option<Owed>,
    /// `true` where the adoption started a new run-end hook epoch, retiring the
    /// marker that stood over the live graph it adopted; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) hook_epoch: Option<HookEpoch>,
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

/// The settlement a driver let go of a run at: the attached settlement word it
/// returns, or `error` for a driver-side error it reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ExitSettlementWord {
    /// `complete`.
    Complete,
    /// `awaiting-planner`.
    AwaitingPlanner,
    /// `unattended`.
    Unattended,
    /// `error`: the driver reported an error of its own and let go.
    Error,
}

impl From<crate::driver::Settlement> for ExitSettlementWord {
    fn from(settlement: crate::driver::Settlement) -> Self {
        use crate::driver::Settlement as S;
        match settlement {
            S::Complete => Self::Complete,
            S::AwaitingPlanner => Self::AwaitingPlanner,
            S::Unattended => Self::Unattended,
        }
    }
}

impl ExitSettlementWord {
    /// The word as the record spells it.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::AwaitingPlanner => "awaiting-planner",
            Self::Unattended => "unattended",
            Self::Error => "error",
        }
    }
}

/// `driver-exited`: a driver let go of the run it drove without crashing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct DriverExited {
    /// The driver.
    pub(crate) pid: u32,
    /// The settlement it let go at.
    pub(crate) settlement: ExitSettlementWord,
    /// The error's message for `error`, and `null` otherwise.
    pub(crate) reason: Option<String>,
    /// The highest command envelope id this driver wrote an outcome line for, or
    /// `null` where it answered none.
    pub(crate) last_answered_command: Option<u64>,
}

/// `dispatch-stopped`: one live dispatch a host shutdown acted on.
// llmlint: ignore[boundary_inputs_validated] this module's own rule, stated at its head: a key no document names is not refused, because a record a later build wrote is the ordinary contents of a runs root — no payload document here denies unknown fields, and a reader of this one that did would call a newer build's record unreadable. What a reader acts on is still typed and required.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct DispatchStopped {
    /// The process the dispatch was running in.
    pub(crate) pid: u32,
    /// What the interrupt was answered with.
    pub(crate) interrupt: InterruptWord,
    /// The answer in the words a reader is shown.
    pub(crate) detail: String,
    /// How the dispatch ended.
    pub(crate) ended: DispatchEndingWord,
    /// How long it was watched for, from the moment it was asked.
    pub(crate) waited_ms: u64,
}

/// What one dispatch's interrupt was answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum InterruptWord {
    Delivered,
    NoTurn,
    Failed,
    /// `not-asked`: nothing was asked at all — the `--force` path, or a run the
    /// hold could not be written for.
    NotAsked,
}

impl From<crate::shutdown::Answer> for InterruptWord {
    fn from(answer: crate::shutdown::Answer) -> Self {
        use crate::shutdown::Answer as A;
        match answer {
            A::Delivered => Self::Delivered,
            A::NoTurn => Self::NoTurn,
            A::Failed => Self::Failed,
            A::NotAsked => Self::NotAsked,
        }
    }
}

/// How a dispatch a host shutdown acted on ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DispatchEndingWord {
    Graceful,
    Killed,
    StillRunning,
}

impl From<crate::shutdown::DispatchEnding> for DispatchEndingWord {
    fn from(ending: crate::shutdown::DispatchEnding) -> Self {
        use crate::shutdown::DispatchEnding as E;
        match ending {
            E::Graceful => Self::Graceful,
            E::Killed => Self::Killed,
            E::StillRunning => Self::StillRunning,
        }
    }
}

/// `host-shutdown`: the run was put down mid-flight by a host shutdown.
// llmlint: ignore[boundary_inputs_validated] this module's own rule, stated at its head: a key no document names is not refused, because a record a later build wrote is the ordinary contents of a runs root — no payload document here denies unknown fields, and a reader of this one that did would call a newer build's record unreadable. What a reader acts on is still typed and required.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct HostShutdown {
    /// Which scope selected this run.
    pub(crate) scope: ScopeWord,
    /// Who owns it.
    // llmlint: ignore[invalid_states_unrepresentable] a rendering rather than an identity — `LaunchRecord::owner_label`'s `[mine]`, `[<launcher>:<digest>]` or `[unknown]`, written for a reader — carried exactly as `run-stopped`'s own `owner` has always been, and the approved contract spells this record's field `"owner": string`; nothing reads it back to decide anything.
    pub(crate) owner: String,
    /// Whether the interrupt and the wait were skipped.
    // llmlint: ignore[invalid_states_unrepresentable] the approved contract spells this record's field as `"forced": bool`, and this document is held to what writers already write; a closed mode word here would be a second wire shape for the same fact.
    pub(crate) forced: bool,
    /// The grace a dispatch had to end itself.
    pub(crate) grace_seconds: u64,
    /// How many live dispatches the shutdown acted on.
    pub(crate) dispatches: u32,
    /// How many of them ended within the grace.
    pub(crate) graceful: u32,
    /// How many were killed at the deadline.
    pub(crate) killed: u32,
    /// What the teardown established, in `run-stopped`'s own vocabulary.
    pub(crate) teardown: TeardownWord,
    /// The runs root the shutdown read.
    // llmlint: ignore[invalid_states_unrepresentable] a rendering for a reader rather than a path anything reopens — the runs root as the shutdown displayed it — and the approved contract spells this record's field `"root": string`; nothing reads it back to decide anything.
    pub(crate) root: String,
    /// One entry per branch the run's records named.
    pub(crate) branches: Vec<BranchPreserved>,
}

/// What a teardown established, in the words `run-stopped` carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TeardownWord {
    Signalled,
    NothingToStop,
    IdentityDeclined,
    NotAttempted,
    PartlySignalled,
    Refused,
    Elsewhere,
}

impl From<crate::journal::StopTeardown> for TeardownWord {
    fn from(teardown: crate::journal::StopTeardown) -> Self {
        use crate::journal::StopTeardown as T;
        match teardown {
            T::Signalled => Self::Signalled,
            T::NothingToStop => Self::NothingToStop,
            T::IdentityDeclined => Self::IdentityDeclined,
            T::NotAttempted => Self::NotAttempted,
            T::PartlySignalled => Self::PartlySignalled,
            T::Refused => Self::Refused,
            T::Elsewhere => Self::Elsewhere,
        }
    }
}

/// Which scope a host shutdown selected a run under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ScopeWord {
    /// `run`: the positional run id.
    Run,
    /// `mine`: every run this session owns.
    Mine,
    /// `host`: every run under the runs root.
    Host,
}

impl From<&crate::shutdown::ShutdownScope> for ScopeWord {
    fn from(scope: &crate::shutdown::ShutdownScope) -> Self {
        use crate::shutdown::ShutdownScope as S;
        match scope {
            S::Run(_) => Self::Run,
            S::Mine => Self::Mine,
            S::Host => Self::Host,
        }
    }
}

/// One branch a host shutdown offered to `onevcs::preserve`.
// llmlint: ignore[boundary_inputs_validated] this module's own rule, stated at its head: a key no document names is not refused, because a record a later build wrote is the ordinary contents of a runs root — no payload document here denies unknown fields, and a reader of this one that did would call a newer build's record unreadable. What a reader acts on is still typed and required.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct BranchPreserved {
    // llmlint: ignore-block[invalid_states_unrepresentable] a wire document of what `onevcs::preserve` answered, field for field as the approved contract spells the record — `identity`, `branch`, `remote: string|null`, `commit: string|null` — and that library carries each as a `String` itself; this crate reads them back only to render them.
    /// The repository identity it belongs to.
    pub(crate) identity: String,
    /// The branch, under the name it already had.
    pub(crate) branch: String,
    /// What preserving it found to do.
    pub(crate) result: PreservedWord,
    /// The origin it went to, where there was one.
    pub(crate) remote: Option<String>,
    /// The commit it stands at, where the preservation read one.
    pub(crate) commit: Option<String>,
    /// What the sibling said, in the words a reader is shown.
    pub(crate) detail: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// What preserving one branch found to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PreservedWord {
    Pushed,
    AlreadyOnOrigin,
    NoRemote,
    Refused,
}

impl From<crate::shutdown::Preserved> for PreservedWord {
    fn from(preserved: crate::shutdown::Preserved) -> Self {
        use crate::shutdown::Preserved as P;
        match preserved {
            P::Pushed => Self::Pushed,
            P::AlreadyOnOrigin => Self::AlreadyOnOrigin,
            P::NoRemote => Self::NoRemote,
            P::Refused => Self::Refused,
        }
    }
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
    /// The command envelope that delivered the note. Absent from a record
    /// written before the field existed, and for a note a dispatch's task was
    /// composed with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) envelope: Option<u64>,
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

/// `pool-maintenance`: one sweep of the schedule that did something.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct PoolMaintenance {
    /// When the sweep started, RFC3339 — spelled as every other instant a record
    /// of this crate carries is, and as the sibling spells `last_maintained`
    /// beside it: the document names a string, and the writer is `sys::now_rfc3339`.
    // llmlint: ignore[invalid_states_unrepresentable] the instant is a `String` on every record this crate and its sibling write — `LaunchRecord::started_at`, the envelope's own `ts`, `onevcs`'s `SlotStatus::last_maintained` — and this document names the same shape; the one writer stamps it through `sys::now_rfc3339`, and a reader treats an unparseable one as an unrecorded time rather than an instant.
    pub(crate) started_at: String,
    /// Every identity the sweep records: one whose slot ran, one another run had
    /// claimed, or one that failed.
    pub(crate) identities: Vec<MaintainedIdentity>,
    /// Why the host's identities could not be enumerated, where they could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// Where the pass stopped before reaching every identity; absent for a pass
    /// that visited them all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cut_short: Option<CutShort>,
}

/// Why a pass of the sweep stopped before reaching every identity, and which
/// identities it never began: the `cut_short` of a `pool-maintenance` or
/// `branches-retired` record.
///
/// The identity in progress when the stop arrived is finished and reported as an
/// ordinary entry of the record, and is never one of `unreached`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct CutShort {
    /// What stopped it.
    pub(crate) reason: CutReason,
    /// Every identity the pass never began, spelled as the record's `identity`
    /// fields are, in the sorted order the sweep visits them.
    pub(crate) unreached: Unreached,
}

/// The identities a cut-short pass never began: never empty, because a pass that
/// reached every identity carries no `cut_short` at all. Built through
/// [`Unreached::new`] and read back refusing an empty list, so no value of this
/// type is a cut that left nothing out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
pub(crate) struct Unreached(#[schemars(length(min = 1))] Vec<String>);

impl Unreached {
    /// `None` for an empty list, which is a pass that was not cut short.
    pub(crate) fn new(identities: Vec<String>) -> Option<Self> {
        (!identities.is_empty()).then_some(Self(identities))
    }

    /// The identity keys, in visiting order.
    pub(crate) fn identities(&self) -> &[String] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Unreached {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(Vec::deserialize(deserializer)?).ok_or_else(|| {
            serde::de::Error::custom(
                "`unreached` is empty, and a pass cut short names at least one identity it \
                 never began",
            )
        })
    }
}

/// What stopped a pass of the sweep before every identity.
///
/// One word, because a sweep stops early for one reason: the driver running it
/// is closing out, and stops it at the next identity boundary rather than wait
/// for the rest. What it left due, another driver's idle pass or the host's own
/// sweep picks up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CutReason {
    /// `driver-closing`: the driver running the sweep was closing out.
    DriverClosing,
}

/// One identity of a `pool-maintenance` record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct MaintainedIdentity {
    /// The identity key.
    pub(crate) identity: String,
    /// The cadence it was maintained on, as the schedule spells a span.
    // llmlint: ignore[invalid_states_unrepresentable] the value is a `onevcs::Span`, written through that type's own `Display` by the one writer and read back through its `FromStr` by nothing — `results` renders the spelling; that type derives no `JsonSchema`, so this *document*, a generated schema, carries the spelling the sibling's grammar fixes rather than a restatement of its grammar here.
    pub(crate) every: String,
    /// What became of it: the sibling's outcome, or the reason there is none.
    #[serde(flatten)]
    pub(crate) answer: MaintainedAnswer,
}

/// What one identity of a `pool-maintenance` record answered: exactly one of the
/// sibling's outcome and an error, never both and never neither.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum MaintainedAnswer {
    /// The sibling's own `IdentityOutcome`, as `onevcs` serializes it:
    /// `{"claimed": {"by_pid": N}}`, or `{"slots": [{"number": N, "outcome": ...},
    /// ...]}` with each slot's outcome spelled by that library.
    // llmlint: ignore[invalid_states_unrepresentable] `onevcs::IdentityOutcome` is the type, and it is what the writer serializes and what `results` reads back; it derives no `JsonSchema`, so this *document* — a generated schema — carries it as the JSON the sibling declares rather than as a shape restated here, which would be a second copy of that library's vocabulary to drift.
    Outcome {
        /// The sibling's outcome.
        outcome: Value,
    },
    /// Why the identity could not be maintained.
    Error {
        /// The reason, bounded as every payload text is.
        error: String,
    },
}

/// `branches-superseded`: what one landed lineage told `onevcs` about the earlier
/// attempts it superseded on branches of their own.
// llmlint: ignore[boundary_inputs_validated] the rule stated at this module's head: no payload document denies unknown fields, so a newer build's record still reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct BranchesSuperseded {
    /// The node whose landing superseded them: the head of the lineage.
    pub(crate) node: String,
    /// Where it landed, as `onevcs` was told: a commit, or a change request's URL.
    #[schemars(with = "String")]
    pub(crate) landing: crate::edits::StatedLanding,
    /// Every earlier attempt recorded as superseded.
    pub(crate) superseded: Vec<SupersededAttempt>,
    /// Every earlier attempt whose record `onevcs` refused, and why.
    pub(crate) failed: Vec<UnrecordedAttempt>,
}

/// One earlier attempt of a `branches-superseded` record.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SupersededAttempt {
    pub(crate) node: String,
    pub(crate) branch: String,
}

/// One earlier attempt whose supersession `onevcs` did not record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct UnrecordedAttempt {
    pub(crate) node: String,
    pub(crate) branch: String,
    /// What `onevcs` answered, bounded as every payload text is.
    pub(crate) error: String,
}

/// `branches-retired`: what one idle pass's `onevcs::retire_finished` retired, or
/// why a pass over an identity could not be made.
// llmlint: ignore[boundary_inputs_validated] the rule stated at this module's head: no payload document denies unknown fields, so a newer build's record still reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct BranchesRetired {
    /// Every branch the pass deleted.
    pub(crate) retired: Vec<RetiredBranch>,
    /// Every identity whose pass failed, and every branch a deletion did not
    /// finish.
    pub(crate) failed: Vec<UnretiredIdentity>,
    /// Where the pass stopped before reaching every identity — or never began,
    /// its sweep's maintenance having been cut short; absent for a pass that
    /// visited them all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cut_short: Option<CutShort>,
}

/// One branch of a `branches-retired` record, in the words `onevcs`'s report
/// carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct RetiredBranch {
    /// The identity key the pass was scoped to, as `onevcs` reports it.
    pub(crate) identity: String,
    /// The branch name every copy of which the pass deleted.
    pub(crate) branch: String,
    /// Its class, as `onevcs` serializes it. That type derives no `JsonSchema`,
    /// so the document says a string and the type is what is read and written.
    #[schemars(with = "String")]
    pub(crate) class: onevcs::RetirementClass,
    /// What proved it held nothing beyond its base.
    // llmlint: ignore[invalid_states_unrepresentable] `onevcs::RetirementProof` is the type, and it is what the writer serializes; it derives no `JsonSchema`, so this *document* — a generated schema — carries it as the JSON the sibling declares rather than as a shape restated here, which would be a second copy of that library's vocabulary to drift.
    pub(crate) proof: Value,
    /// What retired it.
    pub(crate) trigger: RetirementTrigger,
}

/// What retired a branch of a `branches-retired` record.
///
/// One word, because this crate reaches retirement through `retire_finished`
/// alone. `onevcs` keeps its trigger vocabulary private, so the spelling is held
/// to the one it records by `tests/e2e/retirement.rs`, which reads that
/// library's own record of the branch back through `onevcs status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RetirementTrigger {
    /// The automatic pass `onevcs::retire_finished` makes.
    Pass,
}

/// One identity, or one branch of it, an idle pass did not finish retiring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct UnretiredIdentity {
    /// The identity key.
    pub(crate) identity: String,
    /// The branch, where the failure was one branch's rather than the pass's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) branch: Option<String>,
    /// What stopped it, bounded as every payload text is.
    pub(crate) error: String,
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// Who retitled a change request, as `change-retitled` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RetitledBy {
    /// A `rework` stating a title.
    Op,
    /// The body drafter's `replaced` verdict on a rework's publication.
    Drafter,
    /// A `retitle`.
    Retitle,
}

// llmlint: ignore-block[invalid_states_unrepresentable] wire documents, as the rework
// contract spells them: node ids, titles, branches and commits are strings on the wire, as
// `node-settled`'s own `branch` and `head` are, each checked where its writer composed it.
/// `change-retitled`: a lifecycle node's change request carries a new title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ChangeRetitled {
    /// The node.
    pub(crate) node: String,
    /// The title the change request carried before.
    pub(crate) from: String,
    /// The title it carries now.
    pub(crate) to: String,
    /// Who decided it.
    pub(crate) by: RetitledBy,
}

/// `title-unverified`: a rework's publication left its change request's title
/// unchanged without anything saying it still describes the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct TitleUnverified {
    /// The node.
    pub(crate) node: String,
    /// The title it kept.
    pub(crate) title: String,
    /// Why nothing verified it.
    pub(crate) reason: String,
}

/// One branch a restack brings a draft onto, and the head it brings it onto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct RestackOnto {
    /// The stack parent's branch.
    pub(crate) branch: String,
    /// The commit it stands at.
    pub(crate) head: String,
}

/// The stack parents a restack brings a draft onto: at least one, which is the
/// only list a restack has — a draft with no stack parent owes none.
///
/// Read through [`TryFrom`], so a record naming none is refused where it is read
/// rather than folded as a restack onto nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<RestackOnto>", into = "Vec<RestackOnto>")]
pub(crate) struct Onto(Vec<RestackOnto>);

impl TryFrom<Vec<RestackOnto>> for Onto {
    type Error = String;

    fn try_from(parents: Vec<RestackOnto>) -> Result<Self, String> {
        if parents.is_empty() {
            return Err("a restack names at least one stack parent it brings the node onto".into());
        }
        Ok(Self(parents))
    }
}

impl From<Onto> for Vec<RestackOnto> {
    fn from(onto: Onto) -> Self {
        onto.0
    }
}

impl Onto {
    /// Each stack parent's branch and head, in the node's dependency order.
    pub(crate) fn as_slice(&self) -> &[RestackOnto] {
        &self.0
    }
}

/// Why a draft is restacked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RestackCause {
    /// A stack parent's rework republished with new commits.
    Rework,
    /// A stack parent's own restack republished with new commits.
    Restack,
    /// A node it is connected to landed or was absorbed, so the draft is brought onto
    /// its placement: its nearest unlanded parents, or the root.
    Readjust,
}

/// `node-restack-started`: a draft began being brought onto its stack parents'
/// new heads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NodeRestackStarted {
    /// The node.
    pub(crate) node: String,
    /// Each stack parent's branch and the head it is brought onto: at least one.
    #[schemars(with = "Vec<RestackOnto>", length(min = 1))]
    pub(crate) onto: Onto,
    /// Why.
    pub(crate) cause: RestackCause,
}

/// `node-restacked`: a restack published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NodeRestacked {
    /// The node.
    pub(crate) node: String,
    /// The commit the node's branch stands at after the restack.
    pub(crate) head: String,
    /// Whether an agent was dispatched to conclude a conflict.
    pub(crate) agent_dispatched: bool,
    /// The publication's outcome word, as the node's settlement carries it.
    pub(crate) outcome: String,
}

/// How a node's landing was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LandingDetection {
    /// A readjustment's standings read reported its change request merged into its root.
    Readjust,
    /// The node it was absorbed into landed, carrying its work.
    AbsorberLanded,
}

/// The action a readjustment ran first in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReadjustTrigger {
    /// The pass `onepipeline review`.
    Review,
    /// The verb `onepipeline readjust`.
    Readjust,
    /// Applying a `rework`, or the republication that continues one.
    Rework,
    /// A restack, or a placement of a lifecycle node at dispatch.
    Placement,
    /// A continuing launch or an adoption, before its first dispatch.
    Continuation,
}

/// `node-landed`: a draft's change request was found merged into its root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NodeLanded {
    /// The node.
    pub(crate) node: String,
    /// Its change request.
    pub(crate) change_url: String,
    /// The commit its work reached the root as.
    pub(crate) commit: String,
    /// How the landing was found.
    pub(crate) detected_by: LandingDetection,
    /// The action whose readjustment found it.
    pub(crate) trigger: ReadjustTrigger,
    /// The node whose landing carried it, for an absorbed node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) via: Option<String>,
}

/// `landing-closures`: what the store did with the GitHub issues one landing closed —
/// entry 132 of `docs/contract-divergences.md`, whose fenced example `tests/contract.rs`
/// holds to this type.
///
/// Every member is always written, `node`, `class` and `reason` as `null` when they have
/// nothing to say, so a reader reads one fixed shape.
// llmlint: ignore[invalid_states_unrepresentable] Entry 132 fixes this record's flat shape for its consumers — `class` and `reason` beside `outcome` rather than inside a tagged variant — and `closures::record` is its one constructor, which fills both exactly when the outcome is `failed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct LandingClosures {
    /// The node whose landing it is, or `null` for a `publish-branch`, `repo-recover` or
    /// `integrate` landing outside a run.
    pub(crate) node: Option<String>,
    /// The commit the landing reached its base at.
    pub(crate) commit: String,
    /// The issues the landing reported closing, as `owner/name#N`, as it reported them.
    pub(crate) closes: Vec<String>,
    /// Whether the store's call finished with every item written, or failed for good.
    pub(crate) outcome: ClosureOutcome,
    /// The store's `items`, verbatim: one per issue and source asked.
    pub(crate) items: Vec<Value>,
    /// The store's `unsupported` sources, verbatim: each was sent no request.
    pub(crate) unsupported: Vec<String>,
    /// The failure's class, as the store decided it — `null` unless `outcome` is `failed`.
    pub(crate) class: Option<crate::writeback::FailureClass>,
    /// Why it failed, in the store's words — `null` unless `outcome` is `failed`.
    pub(crate) reason: Option<String>,
}

/// What one landing's closure call came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ClosureOutcome {
    /// The call finished and no item it reports failed.
    Projected,
    /// The call, or an item it reports, failed, and is not asked again.
    Failed,
}

/// `home-status`: what the store answered when the run's home was marked `done` — entry 133
/// of `docs/contract-divergences.md`, whose fenced examples `tests/contract.rs` holds to this
/// type.
///
/// One per run, written once the call has finished. Every member is always written, `status`,
/// `class` and `reason` as `null` when they have nothing to say, so a reader reads one fixed
/// shape.
// llmlint: ignore[invalid_states_unrepresentable] Entry 133 fixes this record's flat shape for its consumers — `status`, `class` and `reason` beside `outcome` rather than inside a tagged variant, as `landing-closures` has them — and `home::record` is its one constructor, which fills `status` exactly when the outcome is `written` and the other two exactly when it is `failed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HomeStatus {
    /// The run's home, by the qualified id its launch recorded.
    pub(crate) project: String,
    /// Whether the store wrote the home `done`, or failed to for good.
    pub(crate) outcome: HomeOutcome,
    /// The home's status as the store read it back — `null` unless `outcome` is `written`.
    pub(crate) status: Option<onetaskgraph_plugin_api::Status>,
    /// The failure's class, as the store decided it — `null` unless `outcome` is `failed`.
    pub(crate) class: Option<crate::writeback::FailureClass>,
    /// Why it failed, in the store's words — `null` unless `outcome` is `failed`.
    pub(crate) reason: Option<String>,
}

/// What the call marking a run's home `done` came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum HomeOutcome {
    /// The store wrote the home `done`.
    Written,
    /// The store refused the write, and it is not asked again.
    Failed,
}

/// `node-absorbed`: a draft's change request was merged into another node's branch, so
/// its work reaches the root only with that node's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NodeAbsorbed {
    /// The node.
    pub(crate) node: String,
    /// The node whose branch it merged into.
    pub(crate) into: String,
    /// Its change request.
    pub(crate) change_url: String,
    /// The commit the merge made on that branch.
    pub(crate) commit: String,
}

/// `change-reopened`: a draft the host closed when its landed parent's branch was
/// deleted had that branch restored and was reopened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ChangeReopened {
    /// The node.
    pub(crate) node: String,
    /// Its change request.
    pub(crate) change_url: String,
    /// The branch restored on the origin, which its retarget then deletes again.
    pub(crate) restored_base: String,
}

/// Where a draft is placed: its base and the stack parents whose branches it stands on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Placement {
    /// The branch its change request is based on.
    pub(crate) base: String,
    /// Its stack parents, in declared order; empty on the root.
    pub(crate) stack_parents: Vec<String>,
}

/// Whether a readjustment moved a draft's change request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RetargetWord {
    /// Its base moved on the host.
    Retargeted,
    /// The host already targeted the computed base.
    Unchanged,
    /// No retarget was asked: its placement did not move.
    None,
}

/// Whether a readjusted draft's branch now carries its new base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReadjustContent {
    /// Its base's head was merged into its branch and pushed.
    Merged,
    /// Its change request was moved with no merge: a `retry` merges the base.
    Pending,
}

/// `node-readjusted`: a draft was brought onto its placement after a node it is
/// connected to landed or was absorbed. The node's ordinary `node-settled` follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NodeReadjusted {
    /// The node.
    pub(crate) node: String,
    /// Where it was placed before.
    pub(crate) from: Placement,
    /// Where it is placed now.
    pub(crate) to: Placement,
    /// The nodes whose landing or absorption moved it.
    pub(crate) cause: Vec<String>,
    /// The commit its branch stands at afterwards.
    pub(crate) head: String,
    /// Whether an agent was dispatched to conclude a conflict.
    pub(crate) agent_dispatched: bool,
    /// What became of its change request's base.
    pub(crate) retarget: RetargetWord,
    /// Whether its branch carries its new base.
    pub(crate) content: ReadjustContent,
}

/// `stack-landed-out-of-order`: a draft merged into the root while unlanded ancestors'
/// work rode along with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct StackLandedOutOfOrder {
    /// The node that landed.
    pub(crate) node: String,
    /// Each unlanded ancestor whose work reached the root through it.
    pub(crate) carried: Vec<String>,
}

/// `change-closed`: a draft's change request was closed without being merged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ChangeClosed {
    /// The node.
    pub(crate) node: String,
    /// Its change request.
    pub(crate) change_url: String,
}

/// A deferred republication's required-checks verdict, as a later read found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ChecksVerdictWord {
    /// Something required has not settled.
    Pending,
    /// Every required check passed.
    Passed,
    /// Every required check passed or was skipped.
    PassedWithSkipped,
    /// A required check failed.
    Failed,
    /// The base requires nothing.
    NoneRequired,
}

/// `node-checks-read`: a later trigger's read of a draft republished without a check
/// watch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NodeChecksRead {
    /// The node.
    pub(crate) node: String,
    /// The head the verdict is about.
    pub(crate) head: String,
    /// The verdict.
    pub(crate) verdict: ChecksVerdictWord,
}

/// `readjust-read-failed`: one repository's standings read failed, and its drafts were
/// left as they were.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReadjustReadFailed {
    /// The repository, as its drafts name it.
    pub(crate) repo: String,
    /// What the host or the sibling said.
    pub(crate) detail: String,
}

/// How a posted reply answers its comment: with the rework's commits, or declining it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReplyDisposition {
    /// Answered by commits a rework added.
    Answered,
    /// Left, saying why.
    Declined,
}

/// `review-reply-posted`: one reply the host accepted, in its comment's thread or as a
/// conversation comment linking it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReviewReplyPosted {
    /// The node whose draft carries the comment.
    pub(crate) node: String,
    /// The comment answered.
    pub(crate) comment: String,
    /// Whether it was answered or declined.
    pub(crate) disposition: ReplyDisposition,
    /// Where the reply is read.
    pub(crate) url: String,
    /// The commits its body links, by full sha; empty for a decline.
    pub(crate) commits: Vec<String>,
}

/// Why a reply drafting posted nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReplyNotDraftedEnding {
    /// No reply drafter is configured for the run.
    Unconfigured,
    /// The drafting dispatch failed, or answered nothing the schema accepted.
    Failed,
    /// The drafter answered a result the engine refused.
    Invalid,
}

/// `review-reply-not-drafted`: a drafting of a rework's replies posted nothing; the
/// replies stay owed and are drafted again on the next pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReviewReplyNotDrafted {
    /// The node whose draft carries the comments.
    pub(crate) node: String,
    /// The comments the drafting named.
    pub(crate) comments: Vec<String>,
    /// Why nothing was posted.
    pub(crate) ending: ReplyNotDraftedEnding,
    /// What failed, in the drafter's or the engine's words.
    pub(crate) detail: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// The scratch removed at one ending or manager request.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScratchRemoved {
    pub(crate) trigger: ScratchTrigger,
    pub(crate) dirs: u64,
    pub(crate) bytes: u64,
    pub(crate) failed: Vec<ScratchFailure>,
}

/// Why scratch is removed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ScratchTrigger {
    Success,
    Discard,
}

/// A producer-platform absolute path, including Windows paths in a Unix reader.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub(crate) struct AbsolutePath(std::path::PathBuf);

impl AbsolutePath {
    pub(crate) fn new(path: std::path::PathBuf) -> Result<Self, &'static str> {
        let text = path.to_str().ok_or("scratch path is not UTF-8")?;
        let bytes = text.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\');
        let unc = text.strip_prefix("\\\\").is_some_and(|tail| {
            let mut parts = tail.split('\\');
            parts.next().is_some_and(|part| !part.is_empty())
                && parts.next().is_some_and(|part| !part.is_empty())
        });
        if text.starts_with('/') || drive || unc {
            Ok(Self(path))
        } else {
            Err("scratch path must be absolute")
        }
    }

    pub(crate) fn display(&self) -> std::path::Display<'_> {
        self.0.display()
    }
}

impl<'de> Deserialize<'de> for AbsolutePath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(std::path::PathBuf::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for AbsolutePath {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "AbsolutePath".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "string", "pattern": r"^(?:/|[A-Za-z]:[\\/]|\\\\[^\\]+\\[^\\]+)"})
    }
}

/// A failure always states a reason.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub(crate) struct ScratchError(String);

impl ScratchError {
    pub(crate) fn new(error: String) -> Result<Self, &'static str> {
        if error.is_empty() {
            Err("scratch failure error must not be empty")
        } else {
            Ok(Self(error))
        }
    }
}

impl std::fmt::Display for ScratchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for ScratchError {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ScratchError {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ScratchError".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "string", "minLength": 1})
    }
}

/// A directory removal that the filesystem refused.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScratchFailure {
    pub(crate) path: AbsolutePath,
    pub(crate) error: ScratchError,
}

/// A directory's allocated disk usage at an ending.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScratchDir {
    pub(crate) path: AbsolutePath,
    pub(crate) bytes: u64,
}

/// Scratch kept for inspection or retry.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScratchRetained {
    pub(crate) ending: ScratchEnding,
    pub(crate) dirs: Vec<ScratchDir>,
    pub(crate) total_bytes: u64,
}

/// A non-success ending or pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ScratchEnding {
    Failed,
    Unfinished,
    Stopped,
    Paused,
}

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
        #[must_use]
        pub const fn schema_of(kind: PipelineKind) -> SchemaId {
            match kind {
                $(PipelineKind::$payload => <$payload as Message>::SCHEMA,)*
            }
        }

        /// The stack's registry ([`crate::vocabulary::registry`]), with every
        /// payload this crate emits registered beside the envelope that carries
        /// it.
        ///
        /// # Panics
        ///
        /// Never for this build's own types: each document is generated from the
        /// type it names, and each id is distinct.
        #[must_use]
        pub fn registry() -> Registry {
            let mut registry = crate::vocabulary::registry();
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

// llmlint: ignore-block[invalid_states_unrepresentable] Entry 122 fixes these journal DTOs, including optional flattened location fields alongside the sibling CommentKind. metadata derives all location fields from that same kind; nodes come from the validated graph and host values from onevcs review_comments. These schemas describe recorded output, not a second host parser.
/// One host read of a draft's review comments; bodies are retained as evidence.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReviewCommentsRead {
    pub(crate) node: String,
    pub(crate) change_url: String,
    #[schemars(with = "String")]
    pub(crate) marker: onevcs::ReadMarker,
    pub(crate) unchanged: bool,
    #[schemars(schema_with = "read_cost_schema")]
    pub(crate) cost: onevcs::ReadCost,
    pub(crate) comments: Vec<ReviewCommentMetadata>,
}

/// The metadata of one host comment, without the body retained as an artifact.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReviewCommentMetadata {
    #[schemars(with = "String")]
    pub(crate) id: onevcs::CommentId,
    #[schemars(with = "Object")]
    pub(crate) kind: onevcs::CommentKind,
    pub(crate) author: String,
    pub(crate) url: String,
    pub(crate) updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub(crate) in_reply_to: Option<onevcs::CommentId>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

// onevcs owns the Rust type; its two unsigned metrics are described here because
// that producer type carries no JsonSchema implementation. Contract tests validate
// both serialized extrema and rejected out-of-range values through this document.
// Exhaustive producer destructuring and u32 signatures also hold the complete
// field set and widths, including future serde-defaulted fields.
fn read_cost_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "object",
        "properties": {
            "graphql_points": {"type": "integer", "minimum": 0, "maximum": u32::MAX},
            "rest_requests": {"type": "integer", "minimum": 0, "maximum": u32::MAX}
        },
        "required": ["graphql_points", "rest_requests"]
    })
}

/// `landing-registered`: a manager landing's ownership, recorded before any
/// side effect.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub(crate) struct LandingRegistered(pub(crate) crate::flow::LandingTransition);

/// `landing-drafting`: a manager landing started drafting its body.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub(crate) struct LandingDrafting(pub(crate) crate::flow::LandingTransition);

/// `landing-started`: a manager landing started its forwarded verb.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub(crate) struct LandingStarted(pub(crate) crate::flow::LandingTransition);

/// `landing-finished`: a manager landing's verb ended `0`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub(crate) struct LandingFinished(pub(crate) crate::flow::LandingTerminal);

/// `landing-refused`: a manager landing's verb ended non-zero, or its progress
/// could not be relayed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub(crate) struct LandingRefused(pub(crate) crate::flow::LandingTerminal);

/// `landing-died`: a manager landing's process proved gone with no ending.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub(crate) struct LandingDied(pub(crate) crate::flow::LandingTransition);

payload_messages! {
    ScratchRemoved => "scratch-removed";
    ScratchRetained => "scratch-retained";
    RunStarted => "run-started";
    ConcurrentAcknowledged => "concurrent-acknowledged";
    ConcurrentDeferred => "concurrent-deferred";
    NodeReady => "node-ready";
    NodeDispatched => "node-dispatched";
    NodeSettled => "node-settled";
    LandingClosures => "landing-closures";
    HomeStatus => "home-status";
    NodeRequeued => "node-requeued";
    EditCommitted => "edit-committed";
    CommandAccepted => "command-accepted";
    EditRejected => "edit-rejected";
    PlannerSurfaceQueued => "planner-surface-queued";
    PlannerSurfaced => "planner-surfaced";
    PlannerReplied => "planner-replied";
    HumanAttested => "human-attested";
    DriverAdopted => "driver-adopted";
    RunStopped => "run-stopped";
    DriverExited => "driver-exited";
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
    PoolMaintenance => "pool-maintenance";
    DispatchStopped => "dispatch-stopped";
    HostShutdown => "host-shutdown";
    BranchesSuperseded => "branches-superseded";
    BranchesRetired => "branches-retired";
    ChangeRetitled => "change-retitled";
    ReviewCommentsRead => "review-comments-read";
    TitleUnverified => "title-unverified";
    NodeRestackStarted => "node-restack-started";
    NodeRestacked => "node-restacked";
    NodeLanded => "node-landed";
    NodeAbsorbed => "node-absorbed";
    ChangeReopened => "change-reopened";
    NodeReadjusted => "node-readjusted";
    StackLandedOutOfOrder => "stack-landed-out-of-order";
    ChangeClosed => "change-closed";
    NodeChecksRead => "node-checks-read";
    ReadjustReadFailed => "readjust-read-failed";
    ReviewReplyPosted => "review-reply-posted";
    ReviewReplyNotDrafted => "review-reply-not-drafted";
    DriverTakenOver => "driver-taken-over";
    DriverSuperseded => "driver-superseded";
    LandingRegistered => "landing-registered";
    LandingDrafting => "landing-drafting";
    LandingStarted => "landing-started";
    LandingFinished => "landing-finished";
    LandingRefused => "landing-refused";
    LandingDied => "landing-died";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{registry, ENVELOPE_VERSION, PIPELINE_KINDS};
    use onemessagebus::CheckError;

    /// The registry this crate reads envelopes through holds every id the bus's
    /// agent profile crate held at its last release, under exactly the document
    /// it held — so a record any build wrote validates the same way here.
    ///
    /// The private half of `tests/registry.rs`'s
    /// `both_registries_hold_every_captured_id_under_its_captured_document`,
    /// over the same committed capture; that file's own doc says why it is
    /// split. `tests/recorded/registry/README.md` says where the capture came
    /// from.
    /// A document with every `description` removed, at every depth: what a
    /// validator reads of it.
    fn without_prose(document: &serde_json::Value) -> serde_json::Value {
        match document {
            serde_json::Value::Object(object) => serde_json::Value::Object(
                object
                    .iter()
                    .filter(|(key, _)| key.as_str() != "description")
                    .map(|(key, value)| (key.clone(), without_prose(value)))
                    .collect(),
            ),
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(without_prose).collect())
            }
            other => other.clone(),
        }
    }

    #[test]
    fn the_registry_this_crate_constructs_holds_every_captured_document() {
        let captured: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_str(
            include_str!("../tests/recorded/registry/onemessagebus-agent-0.8.0.json"),
        )
        .expect("the capture is JSON");
        assert!(!captured.is_empty(), "the capture is empty");
        for (text, document) in &captured {
            let id: SchemaId = text.parse().expect("an id");
            let held = registry()
                .schema(&id)
                .unwrap_or_else(|| panic!("the registry does not register {text} at all"));
            // Everything a validator reads, which is the document minus its
            // prose: `Phase` is `onevcs`'s type now, so its `description` is
            // written and wrapped there. `tests/registry.rs` is where that
            // tolerance is pinned to exactly those three pointers, and to the
            // one deliberate addition since the capture — the filter's optional
            // `significance` matcher field, which only admits more.
            let mut held = held.clone();
            if text == "agent.event-filter@1" {
                if let Some(properties) = held
                    .pointer_mut("/$defs/Matcher/properties")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    properties.remove("significance");
                }
                if let Some(definitions) = held
                    .pointer_mut("/$defs")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    definitions.remove("Significance");
                }
            }
            assert_eq!(
                without_prose(&held),
                without_prose(document),
                "the registry holds {text} under a document that constrains differently from \
                 the one `onemessagebus-agent` 0.8.0 registered it under"
            );
        }
    }

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
    /// nodes for failure, `node-requeued`'s first attempt and its pinned
    /// re-dispatch carrying `branch` and `attempt` — is recorded once in each.
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

    /// `driver-exited` declares all four keys its one writer always writes —
    /// requiring `pid` and `settlement`, and leaving `reason` and
    /// `last_answered_command` optional as this module's rule leaves every key
    /// whose value may be `null` — admits only the settlement words a driver
    /// returns plus `error`, and refuses a payload that disagrees with it, which
    /// is what a debug build of the binary checks every emitted record against.
    #[test]
    fn the_driver_exited_document_declares_every_key_and_admits_only_its_words() {
        let id = schema_of(PipelineKind::DriverExited);
        assert_eq!(id.to_string(), "agent.pipeline.driver-exited@2");
        let document = registry().schema(&id).expect("registered").clone();
        let mut required: Vec<&str> = document["required"]
            .as_array()
            .expect("the document requires keys")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        required.sort_unstable();
        assert_eq!(required, ["pid", "settlement"]);
        for key in ["pid", "settlement", "reason", "last_answered_command"] {
            assert!(
                document["properties"].get(key).is_some(),
                "{id} does not declare {key}"
            );
        }
        for written in [
            DriverExited {
                pid: 7,
                settlement: ExitSettlementWord::Complete,
                reason: None,
                last_answered_command: Some(3),
            },
            DriverExited {
                pid: 7,
                settlement: ExitSettlementWord::Error,
                reason: Some("the queue could not be read".to_string()),
                last_answered_command: None,
            },
        ] {
            let payload = serde_json::to_value(&written).expect("it serializes");
            registry()
                .check(&id, &payload)
                .unwrap_or_else(|refusal| panic!("{payload} is refused: {refusal}"));
            let read: DriverExited = serde_json::from_value(payload).expect("it reads back");
            assert_eq!(read, written);
        }
        let whole = serde_json::json!({
            "pid": 7, "settlement": "awaiting-planner", "reason": null,
            "last_answered_command": null,
        });
        registry()
            .check(&id, &whole)
            .expect("a paused exit is admitted");
        for (key, wrong) in [
            ("settlement", serde_json::json!("crashed")),
            ("settlement", serde_json::json!(null)),
            ("pid", serde_json::json!(-1)),
            ("reason", serde_json::json!(5)),
            ("last_answered_command", serde_json::json!("3")),
        ] {
            let mut refused = whole.clone();
            refused[key] = wrong.clone();
            assert!(
                registry().check(&id, &refused).is_err(),
                "{id} admitted {key} = {wrong}"
            );
        }
        for key in ["pid", "settlement"] {
            let mut missing = whole.clone();
            missing.as_object_mut().expect("an object").remove(key);
            assert!(
                registry().check(&id, &missing).is_err(),
                "{id} admitted a payload without {key}"
            );
        }
    }

    /// The `driver-exited` payload `docs/contract.md` states is the one this
    /// crate registers and writes: the same keys as the document declares, and the
    /// same settlement words as the type that writes them — every word the
    /// contract lists is admitted, and every word the type can write is listed.
    #[test]
    fn the_contracts_driver_exited_payload_is_the_registered_document() {
        let contract = include_str!("../docs/contract.md");
        let stated = contract
            .split("**`driver-exited`**")
            .nth(1)
            .and_then(|rest| rest.split("Its payload is `").nth(1))
            .and_then(|rest| rest.split('`').next())
            .expect("the contract states the driver-exited payload");
        let mut keys = Vec::new();
        let mut words = Vec::new();
        for (index, quoted) in stated.split('"').enumerate() {
            if index % 2 == 0 {
                continue;
            }
            let after = stated
                .split(&format!("\"{quoted}\""))
                .nth(1)
                .unwrap_or_default();
            if after.starts_with(':') {
                keys.push(quoted.to_string());
            } else {
                words.push(quoted.to_string());
            }
        }
        let id = schema_of(PipelineKind::DriverExited);
        let document = registry().schema(&id).expect("registered").clone();
        let mut declared: Vec<String> = document["properties"]
            .as_object()
            .expect("the document declares properties")
            .keys()
            .cloned()
            .collect();
        declared.sort_unstable();
        keys.sort_unstable();
        assert_eq!(keys, declared, "the contract and {id} name different keys");

        for word in &words {
            let payload = serde_json::json!({"pid": 1, "settlement": word});
            registry()
                .check(&id, &payload)
                .unwrap_or_else(|refusal| panic!("the contract's `{word}` is refused: {refusal}"));
        }
        // Every word the writer has, by a match that stops compiling when it grows.
        let every = |word: ExitSettlementWord| match word {
            ExitSettlementWord::Complete
            | ExitSettlementWord::AwaitingPlanner
            | ExitSettlementWord::Unattended
            | ExitSettlementWord::Error => word.as_str().to_string(),
        };
        let written: Vec<String> = [
            ExitSettlementWord::Complete,
            ExitSettlementWord::AwaitingPlanner,
            ExitSettlementWord::Unattended,
            ExitSettlementWord::Error,
        ]
        .into_iter()
        .map(every)
        .collect();
        assert_eq!(
            written, words,
            "the contract and the writer list different words"
        );
    }

    /// A `concurrent-acknowledged` holder written before a holder could say
    /// whose it was still reads, and the four keys that say so are optional in
    /// its document and omitted when absent.
    #[test]
    fn a_concurrent_acknowledgement_written_before_attribution_still_reads() {
        let older = serde_json::json!({
            "shared_identities": ["github.com/owner/service"],
            "runs": {"launching": "second", "holding_sessions": ["s-1"]},
            "holders": [{"session": "s-1", "owner_pid": 7}],
        });
        let read: ConcurrentAcknowledged =
            serde_json::from_value(older.clone()).expect("an older record reads");
        assert_eq!(read.holders[0].identity, None);
        assert_eq!(read.holders[0].dependency, None);
        assert_eq!(serde_json::to_value(&read).expect("it writes"), older);
        let id = schema_of(PipelineKind::ConcurrentAcknowledged);
        registry()
            .check(&id, &older)
            .unwrap_or_else(|refusal| panic!("an older record is refused: {refusal}"));
        let mut attributed = older;
        attributed["holders"][0] = serde_json::json!({
            "session": "s-1", "owner_pid": 7, "identity": "github.com/owner/service",
            "run": "first", "node": "build", "dependency": "run:first#build",
        });
        registry()
            .check(&id, &attributed)
            .unwrap_or_else(|refusal| panic!("an attributed record is refused: {refusal}"));
    }

    /// Each record an envelope's effects leave — `edit-committed`,
    /// `command-accepted`, `edit-rejected` and `note-shown` — leaves `envelope`
    /// optional: a record written before the field existed reads, writes back
    /// without it, and validates, and one naming its envelope validates too.
    #[test]
    fn a_record_written_before_records_named_their_envelope_still_reads() {
        let command = serde_json::json!({"op": "amend", "id": "build", "text": "smaller"});
        let accepted = serde_json::json!({
            "author": "planner", "command": command,
            "operations": [{"kind": "task-amended", "node": "build", "text": "smaller"}],
            "operation_kinds": ["task-amended"],
        });
        let rejected = serde_json::json!({
            "author": "planner", "command": command, "reason": "refused: no such node",
        });
        let shown = serde_json::json!({
            "addressee": "worker", "text": "stop", "reached": "worker",
            "party": "worker", "turn": 3, "evidence": "delivered-origin",
        });
        fn written_back<T: serde::de::DeserializeOwned + Serialize>(older: &Value) -> T {
            let read: T = serde_json::from_value(older.clone()).expect("an older record reads");
            assert_eq!(&serde_json::to_value(&read).expect("it writes"), older);
            read
        }
        assert_eq!(written_back::<EditCommitted>(&accepted).envelope, None);
        assert_eq!(written_back::<CommandAccepted>(&accepted).envelope, None);
        assert_eq!(written_back::<EditRejected>(&rejected).envelope, None);
        assert_eq!(written_back::<NoteShown>(&shown).envelope, None);
        for (kind, older) in [
            (PipelineKind::EditCommitted, &accepted),
            (PipelineKind::CommandAccepted, &accepted),
            (PipelineKind::EditRejected, &rejected),
            (PipelineKind::NoteShown, &shown),
        ] {
            let id = schema_of(kind);
            registry()
                .check(&id, older)
                .unwrap_or_else(|refusal| panic!("an older {kind} is refused: {refusal}"));
            let mut named = older.clone();
            named["envelope"] = serde_json::json!(4);
            registry().check(&id, &named).unwrap_or_else(|refusal| {
                panic!("a {kind} naming its envelope is refused: {refusal}")
            });
            named["envelope"] = serde_json::json!("four");
            assert!(
                registry().check(&id, &named).is_err(),
                "a {kind} naming its envelope by a word was admitted"
            );
        }
    }

    /// `pool-maintenance` and `branches-retired` each declare `cut_short` and
    /// leave it optional: a record without it — every record written before it
    /// existed — still validates, one carrying it validates, and the key admits
    /// only the closed reason and a non-empty `unreached`.
    #[test]
    fn a_cut_short_pass_is_an_optional_key_of_both_sweep_documents() {
        let cut =
            serde_json::json!({"reason": "driver-closing", "unreached": ["github.com/owner/b"]});
        for (kind, whole) in [
            (
                PipelineKind::PoolMaintenance,
                serde_json::json!({"started_at": "2026-09-20T00:00:00.000Z", "identities": []}),
            ),
            (
                PipelineKind::BranchesRetired,
                serde_json::json!({"retired": [], "failed": []}),
            ),
        ] {
            let id = schema_of(kind);
            let document = registry().schema(&id).expect("registered").clone();
            assert!(
                document["properties"].get("cut_short").is_some(),
                "{id} does not declare cut_short"
            );
            assert!(
                !document["required"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|key| key == "cut_short"),
                "{id} requires cut_short"
            );
            registry()
                .check(&id, &whole)
                .unwrap_or_else(|refusal| panic!("{id} without cut_short: {refusal}"));
            let mut carried = whole.clone();
            carried["cut_short"] = cut.clone();
            registry()
                .check(&id, &carried)
                .unwrap_or_else(|refusal| panic!("{id} with cut_short: {refusal}"));
            for wrong in [
                serde_json::json!({"reason": "driver-closing", "unreached": []}),
                serde_json::json!({"reason": "bored", "unreached": ["a"]}),
                serde_json::json!({"unreached": ["a"]}),
            ] {
                let mut refused = whole.clone();
                refused["cut_short"] = wrong.clone();
                assert!(
                    registry().check(&id, &refused).is_err(),
                    "{id} admitted cut_short {wrong}"
                );
            }
        }
        assert_eq!(
            serde_json::to_value(CutReason::DriverClosing).expect("a word"),
            "driver-closing"
        );
        // The type holds the same rule as the document: no cut leaves nothing out.
        assert!(Unreached::new(Vec::new()).is_none());
        let read: CutShort = serde_json::from_value(cut.clone()).expect("a cut reads back");
        assert_eq!(read.unreached.identities(), ["github.com/owner/b"]);
        assert_eq!(serde_json::to_value(&read).expect("it serializes"), cut);
        let empty = serde_json::from_value::<CutShort>(
            serde_json::json!({"reason": "driver-closing", "unreached": []}),
        )
        .expect_err("an empty cut is refused");
        assert!(
            empty.to_string().contains("`unreached` is empty"),
            "{empty}"
        );
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
            crate::channel::Author::from("watcher"),
        ] {
            assert_eq!(word(&AuthorWord::from(author.clone())), author.as_str());
            assert_eq!(word(&AuthorWord::from(author.clone())), word(&author));
        }
        for ending in [
            crate::shutdown::DispatchEnding::Graceful,
            crate::shutdown::DispatchEnding::Killed,
            crate::shutdown::DispatchEnding::StillRunning,
        ] {
            assert_eq!(word(&DispatchEndingWord::from(ending)), ending.as_str());
        }
        for preserved in [
            crate::shutdown::Preserved::Pushed,
            crate::shutdown::Preserved::AlreadyOnOrigin,
            crate::shutdown::Preserved::NoRemote,
            crate::shutdown::Preserved::Refused,
        ] {
            assert_eq!(word(&PreservedWord::from(preserved)), preserved.as_str());
        }
        for answer in [
            crate::shutdown::Answer::Delivered,
            crate::shutdown::Answer::NoTurn,
            crate::shutdown::Answer::Failed,
            crate::shutdown::Answer::NotAsked,
        ] {
            assert_eq!(word(&InterruptWord::from(answer)), answer.as_str());
        }
        for teardown in [
            crate::journal::StopTeardown::Signalled,
            crate::journal::StopTeardown::NothingToStop,
            crate::journal::StopTeardown::IdentityDeclined,
            crate::journal::StopTeardown::NotAttempted,
            crate::journal::StopTeardown::PartlySignalled,
            crate::journal::StopTeardown::Refused,
            crate::journal::StopTeardown::Elsewhere,
        ] {
            assert_eq!(word(&TeardownWord::from(teardown)), teardown.word());
            assert_eq!(word(&TeardownWord::from(teardown)), word(&teardown));
        }
        for scope in [
            crate::shutdown::ShutdownScope::Run(String::new()),
            crate::shutdown::ShutdownScope::Mine,
            crate::shutdown::ShutdownScope::Host,
        ] {
            assert_eq!(word(&ScopeWord::from(&scope)), scope.as_str());
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
        use crate::driver::Settlement as Settled;
        for settled in [
            Settled::Complete,
            Settled::AwaitingPlanner,
            Settled::Unattended,
        ] {
            assert_eq!(word(&ExitSettlementWord::from(settled)), settled.as_str());
            assert_eq!(ExitSettlementWord::from(settled).as_str(), settled.as_str());
        }
        assert_eq!(
            word(&ExitSettlementWord::Error),
            ExitSettlementWord::Error.as_str()
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

#[cfg(test)]
mod scratch_boundaries {
    use super::{AbsolutePath, ScratchError};

    #[test]
    fn scratch_paths_and_errors_round_trip_and_reject_invalid_input() {
        for path in [
            "/absolute/dispatch",
            r"C:\scratch\dispatch",
            r"\\server\share\dispatch",
        ] {
            let encoded = serde_json::json!(path);
            let decoded: AbsolutePath = serde_json::from_value(encoded.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
        }
        for path in ["relative/dispatch", "C:relative", ""] {
            assert!(serde_json::from_value::<AbsolutePath>(serde_json::json!(path)).is_err());
        }
        let reason: ScratchError =
            serde_json::from_value(serde_json::json!("permission denied")).unwrap();
        assert_eq!(serde_json::to_value(reason).unwrap(), "permission denied");
        assert!(serde_json::from_value::<ScratchError>(serde_json::json!("")).is_err());
    }
}
