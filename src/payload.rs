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
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::event::PipelineKind;

/// A JSON object whose members this crate reads through a type of its own.
type Object = Map<String, Value>;

/// A string document admitting exactly `words`.
fn one_of(words: impl IntoIterator<Item = String>) -> Schema {
    let words: Vec<String> = words.into_iter().collect();
    schemars::json_schema!({"type": "string", "enum": words})
}

/// A value's wire word, through the owning type's own serializer — under `tag`
/// for an internally tagged enum, and as the value itself otherwise.
fn spelled<T: Serialize>(value: &T, tag: Option<&str>) -> String {
    let wire = serde_json::to_value(value).expect("a vocabulary value serializes");
    let word = match tag {
        Some(tag) => wire.get(tag).cloned(),
        None => Some(wire),
    };
    word.and_then(|word| word.as_str().map(str::to_owned))
        .expect("a vocabulary value serializes to a word")
}

// Each closed vocabulary below is read off the type that owns it. The match beside
// each list names every variant, so a variant its owner adds fails to compile here,
// beside the list it has to join — and the words are that type's own spelling, so
// a word it renames is renamed here too.

/// `NodeStatus`'s words.
fn statuses(_: &mut SchemaGenerator) -> Schema {
    use crate::graph::NodeStatus as S;
    let listed = |status: S| match status {
        S::Pending
        | S::Ready
        | S::Running
        | S::Waiting
        | S::Blocked
        | S::Parked
        | S::Cancelled
        | S::Done
        | S::CompleteDraft
        | S::Failed
        | S::Skipped => status.as_str().to_owned(),
    };
    one_of(
        [
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
        ]
        .map(listed),
    )
}

/// `Landing`'s words.
fn landings(_: &mut SchemaGenerator) -> Schema {
    use crate::graph::Landing as L;
    let listed = |landing: L| match landing {
        L::Landed | L::Unlanded => landing.as_str().to_owned(),
    };
    one_of([L::Landed, L::Unlanded].map(listed))
}

/// A reply's `Author`'s words.
fn authors(_: &mut SchemaGenerator) -> Schema {
    use crate::channel::Author as A;
    let listed = |author: A| match author {
        A::Planner | A::Monitor => author.as_str().to_owned(),
    };
    one_of([A::Planner, A::Monitor].map(listed))
}

/// The words a release note's delivery is recorded under.
fn deliveries(_: &mut SchemaGenerator) -> Schema {
    use crate::edits::Delivery as D;
    let listed = |delivery: D| match delivery {
        D::Live | D::Deferred => crate::engine::adoption_delivery(delivery).to_owned(),
    };
    one_of([D::Live, D::Deferred].map(listed))
}

/// A checked criterion's `Answer` words.
fn answers(_: &mut SchemaGenerator) -> Schema {
    use crate::criteria::Answer as A;
    let listed = |answer: A| match answer {
        A::Match | A::Mismatch { .. } | A::Unread { .. } => answer.as_str().to_owned(),
    };
    one_of(
        [
            A::Match,
            A::Mismatch {
                holds: String::new(),
            },
            A::Unread {
                reason: String::new(),
            },
        ]
        .map(listed),
    )
}

/// An undrafted body's endings.
fn endings(_: &mut SchemaGenerator) -> Schema {
    use crate::lifecycle::Undrafted as U;
    let listed = |undrafted: U| match undrafted {
        U::Dispatch(_) | U::SchemaRefused | U::Bodyless => undrafted.ending().to_owned(),
    };
    one_of([U::Dispatch(String::new()), U::SchemaRefused, U::Bodyless].map(listed))
}

/// Where a note `Reached`, as the tag its record carries.
fn reaches(_: &mut SchemaGenerator) -> Schema {
    use crate::note::Reached as R;
    let listed = |reached: R| match reached {
        R::Queued | R::Worker | R::Supervisor | R::JudgedWith { .. } | R::Carried => {
            spelled(&reached, Some("reached"))
        }
    };
    one_of(
        [
            R::Queued,
            R::Worker,
            R::Supervisor,
            R::JudgedWith {
                completion_reason: String::new(),
            },
            R::Carried,
        ]
        .map(listed),
    )
}

/// What a `note-shown` was decided from.
fn evidences(_: &mut SchemaGenerator) -> Schema {
    use crate::note::Evidence as E;
    let listed = |evidence: E| match evidence {
        E::DeliveredOrigin | E::OpeningTask | E::InstructionText | E::AnsweringTurn => {
            spelled(&evidence, None)
        }
    };
    one_of(
        [
            E::DeliveredOrigin,
            E::OpeningTask,
            E::InstructionText,
            E::AnsweringTurn,
        ]
        .map(listed),
    )
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
    #[schemars(schema_with = "statuses")]
    pub(crate) status: String,
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
    #[schemars(schema_with = "landings")]
    pub(crate) landing: Option<String>,
    /// The steps the branch carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) completed_steps: Option<Vec<String>>,
}

/// `edit-committed`: an accepted command that changed something.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct EditCommitted {
    /// Who submitted it.
    pub(crate) author: String,
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
    pub(crate) author: String,
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
    pub(crate) author: String,
    /// The command as submitted.
    pub(crate) command: Object,
    /// Why it was refused.
    pub(crate) reason: String,
}

/// `planner-surface-queued`: a surface sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct PlannerSurfaceQueued {
    /// The surface's kind.
    pub(crate) kind: String,
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
    pub(crate) kind: String,
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
    #[schemars(schema_with = "authors")]
    pub(crate) author: String,
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
    pub(crate) style: Option<String>,
}

/// `release-adopted`: a fast-adoption node told its releases arrived.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReleaseAdopted {
    /// The node.
    pub(crate) node: String,
    /// Whether the note reached a running turn or its next dispatch.
    #[schemars(schema_with = "deliveries")]
    pub(crate) delivery: String,
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
    #[schemars(schema_with = "answers")]
    pub(crate) answer: String,
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
    #[schemars(schema_with = "endings")]
    pub(crate) ending: String,
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
    #[schemars(schema_with = "reaches")]
    pub(crate) reached: String,
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
    #[schemars(schema_with = "evidences")]
    pub(crate) evidence: String,
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
    /// build's own end-to-end journeys for the three no host run has, with free
    /// text and host paths stood in for.
    const RECORDED: &str = include_str!("../tests/recorded/pipeline-kinds.jsonl");

    /// Every kind's recorded envelope is admitted by its registered document, and a
    /// payload violating that document — a key every writer of the kind writes,
    /// taken away — is refused naming the document and the pointer.
    #[test]
    fn every_kinds_recorded_envelope_validates_against_its_registered_document() {
        let recorded: Vec<crate::event::Envelope> = RECORDED
            .lines()
            .map(|line| serde_json::from_str(line).expect("a recorded envelope reads"))
            .collect();
        for kind in PIPELINE_KINDS {
            let id = schema_of(*kind);
            let envelope = recorded
                .iter()
                .find(|envelope| PipelineKind::from_wire(&envelope.kind) == Some(*kind))
                .unwrap_or_else(|| panic!("no recorded envelope of {kind}"));
            assert_eq!(
                envelope.v, ENVELOPE_VERSION,
                "{kind} was recorded at another version"
            );
            let payload = serde_json::Value::Object(envelope.payload.clone());
            registry()
                .check(&id, &payload)
                .unwrap_or_else(|refusal| panic!("the recorded {kind} is refused: {refusal}"));

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
}
