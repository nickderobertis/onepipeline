//! The planner channel: the wire shapes, and the durable queues behind them.
//!
//! A reply is one JSON envelope: a legacy verdict, a versioned list of graph
//! edits, or both. The edits' required fields and validation semantics are
//! `ai-orchestrator`'s live-edit protocol exactly, per `docs/contract.md`.
//!
//! Which reader takes one follows from **which of those three it is**, and not
//! from which reader reached the queue first: see [`Reply`].
//!
//! The queues are `onemessagebus`'s. `ChannelState` is the run's `channel/`
//! directory opened as the `planner-channel` layout `onemessagebus-agent`
//! declares, over the bus's local transport: the logs, their policies, the
//! projection with its stamp and seal, the cursors, the asker identity, the
//! correlation a question is answered by, and the author allowlist are all
//! calls into that crate, which is where `docs/queues.md` and `docs/ask.md`
//! state them. What is left here is what this crate means by the records those
//! queues hold, and which bus call each channel operation is. It does not
//! *judge* an edit — whether a target exists, is in the right state, and leaves
//! an acyclic graph is a question about the live frontier, and the reconciler
//! in `edits` is what asks it.

// llmlint: ignore-file[invalid_states_unrepresentable] every node id, dependency
// reference, and human-action reference here is a `String` because a `NodeId`/`NodeRef`
// newtype is a public item `docs/contract.md` does not name, and minting one is interface
// drift — a published promise the contract never made (see src/AGENTS.md). `version` and
// `completion` stay independent optionals for a different reason: the contract's envelope
// is "legacy verdicts *plus* a versioned command list", so a reply may legally carry
// either, both, or a version this build does not know — and collapsing that into one enum
// would reject envelopes the protocol accepts. The references are narrowed where they are
// judged, against the graph `edits` reconciles them into.

// llmlint: ignore-file[boundary_inputs_validated] a reply is external input and its
// *structural* boundary is enforced here — an unknown `op`, a missing required field, or
// an unknown key is rejected by serde and asserted in `tests/contract.rs`. The *semantic*
// validation the contract specifies (the target exists, is in the right state, and the
// resulting graph is still acyclic) is a judgement against the live frontier, so it is
// made in `edits`, where that frontier is, and its verdict comes back through the command
// outcomes this file records.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use onemessagebus::{
    Allowlist, AskOptions, Bus, BusError, Config, ConsumerName, Correlation, Fingerprint, Layouts,
    Lifetime, LocalTransport, Message, OpWord, Pending, QueueError, QueueName, QueueSpec, RawQueue,
    Read, Registry, SchemaId, Transport, TransportConfig, TransportKinds,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::note::{Addressee, Criterion, NoteText};
use crate::plan::Node;

pub mod layout;

use layout::{
    recorded_correlation, PlannerChannel, COMMANDS, COMMAND_OUTCOMES, PLANNER_CHANNEL, REPLIES,
    SURFACES,
};
pub use layout::{
    source, CommandOutcome, CommandResult, CommandVerdict, Surface, ASKER_ENV,
    REPLY_ENVELOPE_VERSION, REPLY_ENVELOPE_VERSIONS_READ,
};

/// Every schema the `planner-channel` layout registers, built once per process.
///
/// Building it compiles each document, so it is paid for the first time a reply
/// is read or a queue is written rather than on every read.
fn registry() -> &'static std::sync::Arc<Registry> {
    static REGISTRY: std::sync::OnceLock<std::sync::Arc<Registry>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| std::sync::Arc::new(layout::registry()))
}

/// Carry an envelope written against a version this build still reads forward to
/// the version it is read **at**.
///
/// The number an envelope declares is the version its author wrote it against;
/// what every reader downstream asks is whether this build reads that envelope,
/// and across the read set there is one answer — the registry's `read_at`. A
/// number this build does not read is left exactly as it was declared, so the
/// caller meets the refusal that names the version an edit envelope requires,
/// made where it has always been made — refusing here instead would turn it into
/// a parse error, and would refuse a legacy verdict-only envelope naming an old
/// version and carrying no commands at all, which is accepted today and stays
/// accepted.
fn read_at_a_version_this_build_reads<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let declared = Option::<u32>::deserialize(deserializer)?;
    Ok(declared.map(
        |version| match registry().read_at(layout::REPLY_ENVELOPE_FAMILY, version) {
            Read::At(read) => read,
            Read::Unknown(_) => version,
        },
    ))
}

/// Who wrote a reply, and therefore which ops it may carry.
///
/// An open word: the planner, or any author the launch's bus configuration
/// declares — what each may issue is that configuration's allowlist, enforced
/// where an envelope is offered and where the driver applies it rather than
/// trusted. Omitted, an envelope is the planner's — every reply written before
/// this field existed was.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct Author(String);

impl Author {
    /// The default author: the one an envelope that omits `author` is.
    pub fn planner() -> Self {
        Self("planner".into())
    }
    /// The author's wire word.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the default, so serialization can omit it.
    pub(crate) fn is_planner(&self) -> bool {
        self.as_str() == "planner"
    }

    /// The bus's open author this one is, as an allowlist names it.
    pub(crate) fn word(&self) -> onemessagebus::Author {
        onemessagebus::Author::from(self.as_str())
    }
}

impl Default for Author {
    fn default() -> Self {
        Self::planner()
    }
}
impl From<&str> for Author {
    fn from(word: &str) -> Self {
        Self(word.into())
    }
}
impl TryFrom<String> for Author {
    type Error = String;
    fn try_from(word: String) -> Result<Self, Self::Error> {
        valid_word(&word)
            .then(|| Self(word.clone()))
            .ok_or_else(|| {
                format!("'{word}' is not an author: authors match ^[a-z][a-z0-9-]{{0,63}}$")
            })
    }
}
impl From<Author> for String {
    fn from(author: Author) -> Self {
        author.0
    }
}

fn valid_word(word: &str) -> bool {
    (1..=64).contains(&word.len())
        && word.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && word
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The `planner-channel` layout's allowlist as the layout declares it, before
/// any configuration narrows it.
fn layout_allowlist() -> &'static Allowlist<OpWord> {
    static ALLOWLIST: std::sync::OnceLock<Allowlist<OpWord>> = std::sync::OnceLock::new();
    ALLOWLIST.get_or_init(|| layout::allowlist().words())
}

/// Whether one author may declare the run finished, or a refusal saying why not.
///
/// The legacy verdict says the same thing `complete` says, in a field rather
/// than in an op — so an allowlist that guarded only the ops would let a
/// commandless reply walk straight past it. Whether the run is finished is one
/// decision however it is spelled, which is why the profile grants or refuses
/// it exactly as it grants or refuses `complete`.
pub fn allows_completion(author: Author, completion: Option<bool>) -> crate::Result<()> {
    completion_allowed_by(layout_allowlist(), author, completion)
}

/// [`allows_completion`], under an allowlist a run's configuration narrowed.
pub(crate) fn completion_allowed_by(
    allowlist: &Allowlist<OpWord>,
    author: Author,
    completion: Option<bool>,
) -> crate::Result<()> {
    layout::allows_completion(allowlist, &author.word(), completion).map_err(crate::Error::Refused)
}

/// The ops one author may issue, or a refusal naming what it may not.
///
/// The allowlist is the `planner-channel` profile's, and it is exhaustive: an op
/// nothing grants is refused, so a new op is refused for every author but the
/// planner until somebody decides otherwise rather than being granted by
/// omission. Every op the profile does not grant such an author carries the
/// reason it is refused with, and `docs/contract.md` states each of those
/// refusals.
pub fn allows(author: Author, command: &Command) -> crate::Result<()> {
    allowed_by(layout_allowlist(), author, command)
}

/// [`allows`], under an allowlist a run's configuration narrowed.
pub(crate) fn allowed_by(
    allowlist: &Allowlist<OpWord>,
    author: Author,
    command: &Command,
) -> crate::Result<()> {
    layout::allows(allowlist, &author.word(), op_of(command)).map_err(crate::Error::Refused)
}

/// The wire word for one command's op.
pub fn op_of(command: &Command) -> &'static str {
    match command {
        Command::Add { .. } => "add",
        Command::Drop { .. } => "drop",
        Command::Reparent { .. } => "reparent",
        Command::Retry { .. } => "retry",
        Command::Cancel { .. } => "cancel",
        Command::Requeue { .. } => "requeue",
        Command::Attest { .. } => "attest",
        Command::Complete { .. } => "complete",
        Command::Amend { .. } => "amend",
        Command::Note { .. } => "note",
        Command::Finding { .. } => "finding",
        Command::Settle { .. } => "settle",
    }
}

/// The node one command is about, when it names one.
pub fn target_of(command: &Command) -> Option<String> {
    match command {
        Command::Add { node } => Some(node.id.clone()),
        Command::Drop { id, .. }
        | Command::Reparent { id, .. }
        | Command::Retry { id, .. }
        | Command::Cancel { id, .. }
        | Command::Requeue { id, .. }
        | Command::Note { id, .. }
        | Command::Settle { id, .. }
        | Command::Amend { id, .. } => Some(id.clone()),
        Command::Attest { reference } => Some(reference.clone()),
        Command::Finding { id, .. } => id.clone(),
        Command::Complete { .. } => None,
    }
}

/// One reply to a planner surface.
///
/// It carries two halves, and each has its own reader. The **verdict** half —
/// [`completion`](Self::completion), [`message`](Self::message),
/// [`reason`](Self::reason) — is what answers a pending surface, and is what a
/// supervisor-side reader waiting on the channel reads. The **commands** half is
/// the reconciler's, reconciled against the graph in order. An envelope carrying
/// both is delivered to both: its commands to the command path, and the envelope
/// itself to the pending surface, out of which its reader reads the verdict.
///
/// A **commands-only** envelope — a version and commands, no verdict — is the
/// command path's alone. It is never queued on the reply path, because the
/// reader waiting there asked for a ruling and a graph edit is not one: handing
/// it over is what killed the observers this routing exists to keep alive.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    /// A version this build reads when the envelope carries commands — one of
    /// [`REPLY_ENVELOPE_VERSIONS_READ`], read at [`REPLY_ENVELOPE_VERSION`].
    #[serde(
        default,
        deserialize_with = "read_at_a_version_this_build_reads",
        skip_serializing_if = "Option::is_none"
    )]
    pub version: Option<u32>,
    /// Who wrote it. Omitted, [`Author::planner()`].
    #[serde(default, skip_serializing_if = "Author::is_planner")]
    pub author: Author,
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

/// A reply envelope is the profile's `agent.reply-envelope` message, at the
/// version this build writes.
///
/// The outer shape is the profile's to register and each command's meaning is
/// this crate's, so the schema this type generates names every op's own fields
/// and carries a node as the mapping it is written as.
impl Message for Reply {
    const SCHEMA: SchemaId = SchemaId::literal("agent", "reply-envelope", REPLY_ENVELOPE_VERSION);
}

impl Reply {
    /// Whether this envelope carries a verdict half.
    ///
    /// The verdict is three optional fields rather than one, because the
    /// protocol lets a planner send any of them alone — a bare `message` is as
    /// much a ruling as a `completion` is. Any of the three present is a reply
    /// a pending surface can be answered with, and a reader waiting for a
    /// ruling can read.
    pub(crate) fn carries_verdict(&self) -> bool {
        self.completion.is_some() || self.message.is_some() || self.reason.is_some()
    }
}

/// What happens to a dropped node's direct dependents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
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
/// with one subtraction and one replacement: the table's `context` is **gone**,
/// and [`Note`](Self::Note) is the single manager-note op that carries what both
/// of them did. An envelope still naming `context` is refused by serde, by that
/// name, because the field set below rejects what it does not declare.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
pub enum Command {
    /// Add a new node. Its `deps`, if any, must name graph nodes or valid
    /// cross-DAG references.
    Add {
        /// The full node mapping.
        #[schemars(with = "Map<String, Value>")]
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
        #[schemars(with = "Map<String, Value>")]
        node: Node,
    },
    /// Park a pending or running node: cancel its dispatch cooperatively and
    /// hold it out of every later dispatch until a `requeue`.
    Cancel {
        /// The node to park.
        id: String,
        /// Why, in the parking author's own words.
        ///
        /// Optional, so every `cancel` written before this field existed still
        /// parks exactly as it did — but it is the fact the record was missing:
        /// a park carrying only a node id is indistinguishable, downstream, from
        /// a node sitting idle for no reason anybody decided, and an observer
        /// reading it as one has requeued deliberate decisions. Present and
        /// blank is refused rather than recorded, as every other text this
        /// vocabulary carries is: a reason nobody can read is not one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
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
    /// Make one binding amendment to what a node is judged against.
    ///
    /// The lever a manager has that a [`Note`](Self::Note) is not, and the
    /// answer to the one thing a note deliberately cannot do. A note reaches the
    /// conversation running now, or is carried to the node's next dispatch, and
    /// never both; this becomes part of the node's **effective task**, which the
    /// worker and the judge reviewing it are handed alike, on the dispatch that
    /// follows it and on every later one. A turn already running is not reached
    /// — its task was composed before the ruling existed — which is the
    /// asymmetry with a note, whose point is the turn running now. A correction
    /// that has to reach the live turn *and* still bind a re-dispatch is this
    /// op, not a note.
    Amend {
        /// The node to amend. It must be one the graph holds and can still be
        /// dispatched: a node that has settled `done` is refused for the reason
        /// a note to one is, since nothing will read the amendment.
        id: String,
        /// The binding text. Blank is refused rather than recorded.
        ///
        /// A second amendment **replaces** the first: the latest is the node's
        /// amendment and the earlier one stops being part of the effective task.
        /// A bar that could only grow could not be corrected.
        text: String,
    },
    /// Deliver one note into the node's live dispatch, to whichever party of it
    /// is speaking — and, where it reached no turn, carry it to the node's next
    /// dispatch.
    ///
    /// **The one manager-note op**, and the authoritative declaration of its
    /// shape: the field set, each field's type and default, and what the op
    /// answers are stated here and nowhere else in this repository, so a
    /// consumer's documentation derives from this rather than restating it.
    ///
    /// It goes to the node's running conversation through the delivery seam
    /// [`oneagentgraph`](crate::note) publishes rather than through a bare
    /// interrupt, so the party that is live takes it and the other party
    /// receives it with that party's response; a [`criterion`](Self::Note::criterion)
    /// it carries enters the acceptance criteria that conversation's judge
    /// decides against.
    ///
    /// # `deliver` and `persist` are two axes, not one
    ///
    /// [`deliver`](Self::Note::deliver) decides **whether live delivery is
    /// attempted**; [`persist`](Self::Note::persist) decides **whether the note
    /// is composed into the node's next dispatch**. Neither decides the other's
    /// question, and saying so is load-bearing: `deliver: next` and
    /// `persist: true` both read as "on the next dispatch", and a reader who
    /// conflates them gets this contract wrong. Their four combinations:
    ///
    /// * `live` with `persist: false` — the running turn is attempted; where it
    ///   took the note that is the whole of the delivery, and where it did not
    ///   the note is **refused**, because it composes forward into nothing and
    ///   so reached nobody. This is the combination a caller uses when it needs
    ///   that refusal.
    /// * `live` with `persist: true` — the running turn is attempted; where it
    ///   took the note the note composes forward into nothing, because it
    ///   reached a running turn; where it did not, the note is composed into the
    ///   node's next dispatch and is **not** a refusal. **This is the default**,
    ///   and it is exactly what the removed `context` op's `auto` delivery meant.
    /// * `next` with `persist: true` — the running turn is not interrupted, so
    ///   the note never reaches one and is always composed into the node's next
    ///   dispatch.
    /// * `next` with `persist: false` — no live delivery is attempted and the
    ///   note composes forward into nothing, so it reaches nobody whatever the
    ///   run does. Refused at this envelope, before the run is reached.
    ///
    /// The default a caller gets by omitting both is `deliver: live` with
    /// `persist: true`, because it is the combination that attempts the running
    /// turn *and* cannot leave the note nowhere. It is not the only one that
    /// cannot leave it nowhere — `deliver: next` with `persist: true` never
    /// reaches a running turn and so always composes forward — but that one
    /// declines the live attempt, which is what disqualifies it as the default.
    ///
    /// # One refusal rule
    ///
    /// **A note that would reach nobody is refused, naming what left it nowhere
    /// to go.** One sentence, checked wherever it can be decided: at this
    /// envelope, where `deliver: next` with `persist: false` reaches nobody by
    /// construction and never reaches a run at all; and at delivery, where only
    /// the run can decide it — `deliver: live` with `persist: false` and no turn
    /// that took it. Neither is a special case beside the other. The op also
    /// refuses a blank [`text`](Self::Note::text) and an [`id`](Self::Note::id)
    /// naming a node the graph cannot reach, and each refusal names which of
    /// those it is.
    ///
    /// # What it deliberately cannot do
    ///
    /// Reaching the running turn and being carried into the next dispatch are
    /// mutually exclusive under `persist`'s biconditional, so this op offers **no
    /// way to do both**. That is deliberate rather than a gap: a correction that
    /// has to reach the live turn *and* still bind the node's next dispatch is a
    /// ruling that survives a re-dispatch, and [`Amend`](Self::Amend) is the op
    /// for that and is unchanged. So this one is not given a second, weaker way
    /// to say what `amend` already says properly.
    ///
    /// # What it answers
    ///
    /// Six dispositions, each named in what the caller reads back — the
    /// `reached` word the run records, and [`Delivered`](crate::note::Delivered)
    /// for a caller on this crate's own surface:
    ///
    /// * `worker` — the worker's live turn took it.
    /// * `supervisor` — the supervisor's live turn took it.
    /// * `judged-with` — the supervisor's decision was re-taken with it in hand
    ///   and completed, carrying that completion reason, so no further worker
    ///   turn took it.
    /// * `queued` — no turn was live, so the next turn of that conversation to
    ///   open takes it.
    /// * `carried` — the note was carried to the node's next dispatch instead of
    ///   being taken by a running turn.
    /// * [`Delivered::Queued`](crate::note::Delivered::Queued) — the run accepted
    ///   it durably without the reconciler having answered within the reply
    ///   timeout. Still queued rather than a refusal, and **never** an
    ///   instruction to send it again.
    ///
    /// `worker`, `supervisor`, `judged-with` and `queued` are the note reaching
    /// the running dispatch's conversation; `carried` is the note reaching no
    /// turn of it. Under the default those two are the only ways one accepted
    /// note succeeds, and they are exhaustive and mutually exclusive — which is
    /// the same biconditional `persist` is defined by. The disposition's shape
    /// and the field's semantics were chosen together rather than arrived at
    /// separately: they are materially different to whoever sent the note, and a
    /// caller that cannot tell them apart is back in the incident this op was
    /// written from.
    Note {
        /// The node whose dispatch it is for.
        ///
        /// Required; absent, this envelope refuses. It must name a node the
        /// graph holds and can still be reached — one that will never be
        /// dispatched again and has no conversation left is refused under the
        /// reach-nobody rule rather than accepted and dropped.
        id: String,
        /// Whose task this updates. One of `worker`, `supervisor`, `both`, and
        /// no other value.
        ///
        /// Required, with no default, and **never inferred**: a note whose
        /// addressee is guessed is one the judge may read as work for itself. An
        /// envelope omitting it is refused rather than defaulted to any of the
        /// three.
        addressee: Addressee,
        /// What the addressee reads.
        ///
        /// Required; a blank or whitespace-only value is refused at this
        /// boundary by the seam's own newtype, rather than accepted here and
        /// dropped later.
        text: NoteText,
        /// The property the finished tree must have, when this note changes
        /// that.
        ///
        /// Optional, defaulting to absent, and omitted from a serialized note
        /// that does not carry one. Present, it enters the acceptance criteria
        /// the judge of the conversation it is delivered into decides against;
        /// absent, the note is observational and touches no acceptance
        /// criterion. It binds the conversation it reached and not the node's
        /// stored bar — [`Amend`](Self::Amend) is the op for that.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criterion: Option<Criterion>,
        /// Whether live delivery is attempted, and **only** that.
        ///
        /// Optional, defaulting to [`Deliver::Live`], and omitted from a
        /// serialized note that carries the default. `live` attempts to reach
        /// the node's running turn; `next` attempts no live delivery at all.
        /// Whether the note is composed into a later dispatch is
        /// [`persist`](Self::Note::persist)'s question alone.
        #[serde(default, skip_serializing_if = "Deliver::is_default")]
        deliver: Deliver,
        /// Whether the note is composed into the node's next dispatch, and
        /// **only** that.
        ///
        /// Optional, defaulting to `true`, and omitted from a serialized note
        /// that carries the default. One sentence states it for both `deliver`
        /// values: `true` composes the note into the node's next dispatch **if
        /// and only if the note did not reach a running turn**, where it is
        /// consumed when that dispatch takes it; `false` composes it into no
        /// dispatch, whatever `deliver` did. Neither value says anything about
        /// whether a live attempt was made, which is
        /// [`deliver`](Self::Note::deliver)'s question alone.
        ///
        /// Read it as "do not lose this" rather than as "send it twice".
        #[serde(default = "persists", skip_serializing_if = "is_true")]
        persist: bool,
    },
    /// Raise one finding to the planner, changing nothing about the graph.
    ///
    /// The op an observer reports *through*. Its edits already travel in this
    /// envelope, so a member that emitted its observations as raw turn text
    /// surfaced one on every turn it took — including the turns that only said
    /// it was about to look. A finding is a deliberate act instead: a turn with
    /// nothing to report issues no op, and the planner's queue stays empty.
    Finding {
        /// The finding's text. Blank is refused rather than queued.
        message: String,
        /// Whether the run waits on the planner's answer. Omitted, `false`: an
        /// observation holds nothing back, and a finding that means to stop the
        /// subtree it names says so.
        #[serde(default, skip_serializing_if = "is_false")]
        blocking: bool,
        /// The node it is about, when it is about one. It must be a node the
        /// graph has: a name the graph does not carry would pass validation and
        /// then hold nothing, so a blocking finding raised about work nobody is
        /// doing would read as one the run is waiting on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// Settle a node at what the operator can see it actually reached, from
    /// evidence this run never observed.
    ///
    /// The op for a record that has gone wrong rather than for work that has:
    /// a change that merged while the node read `failed`, a wait that can never
    /// clear. Without it the only route is replacing the node with a stand-in
    /// that dispatches nothing and carries the evidence in prose — which loses
    /// the node's identity, renames it in every downstream reference, and
    /// forces a rewiring cascade through its dependents.
    ///
    /// So it **keeps the node**: its id, its lineage, and its dependents' edges
    /// are all exactly as they were, and the only thing that moves is what the
    /// run's record says became of it.
    Settle {
        /// The node to settle. It must be one the graph holds, and one whose
        /// record does not already say what this states — a settle that changes
        /// nothing is a duplicate rather than a correction. A node that settled
        /// **something else** is exactly what this is for: a change that merged
        /// while the node read `failed` is the case the op exists for, and the
        /// earlier settlement stays in the journal beside this one.
        id: String,
        /// What it settled as.
        outcome: SettleOutcome,
        /// What the operator saw, in their own words. Required and never blank:
        /// this is journalled as the reason the node is in the state it is, and
        /// a settlement nothing accounts for is the record this op exists to
        /// stop writing.
        evidence: String,
        /// Where the node's work landed: the commit it reached its base at, or
        /// the change request a person reads it in. Release correlation joins a
        /// release to work through it, which is what a settlement from evidence
        /// could not say — see `docs/contract-divergences.md` entry 57.
        ///
        /// Optional: omitted, the settlement records what it always recorded and
        /// attributes no landing. Present and unusable is refused rather than
        /// recorded, as blank evidence is — the value is handed back to `onevcs`
        /// as a reference and rendered into views beside it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        landing: Option<String>,
        /// The release that carries that landing, as the operator verified it:
        /// a release target and the version of it the work first shipped in.
        ///
        /// Recorded with `onevcs release acknowledge`'s own semantics before the
        /// envelope is queued, which is what a landing with **no release
        /// baseline** needs — nothing recorded what that target had published
        /// when the work landed, so no probe answer can say a release carries it,
        /// and a node waiting on that release holds until a person says which one
        /// does. Only beside a `landing`, because that is what it is recorded
        /// against. Optional: omitted, nothing is attributed, and a settle naming
        /// no release is exactly the settle it was. See
        /// `docs/contract-divergences.md` entry 57.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        release: Option<StatedRelease>,
    },
}

/// The release a `settle` states carries the landing it names.
///
/// Both halves are what `onevcs release acknowledge` records, and neither is
/// guessed at: a target this build cannot name is refused where the envelope is
/// read, and a version is recorded only if `onevcs` accepts it as one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatedRelease {
    /// The release target that carries the work, as the landing's repository
    /// declares it.
    #[schemars(with = "String")]
    pub target: onevcs::releases::TargetName,
    // llmlint: ignore[invalid_states_unrepresentable] a version is `onevcs`'s to judge —
    // it compares releases by semantic-version ordering and refuses one that is not a
    // semantic version by name — and this crate links no parser of its own for it. What
    // this side can hold it to, one usable word, is checked where the settle compiles.
    /// The version of that target the work first shipped in.
    pub version: String,
}

/// What a `settle` states a node actually reached.
///
/// The two settled statuses a node can be **put** at, and deliberately not every
/// status a node can be *in*. `pending`, `ready`, `blocked` and `skipped` are
/// derived from the graph on every pass rather than recorded, so a node settled
/// at one of them would be re-derived out of it before the next dispatch and the
/// operator's statement would silently not hold; `parked` and `cancelled` have
/// ops of their own. A wait that can never clear is settled `failed` carrying
/// the evidence that says so, which is a record that sticks. A value outside
/// these two is refused by serde, naming what it read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SettleOutcome {
    /// The work was done, whatever this run's record says.
    Done,
    /// It was not, and nothing further is going to change that.
    Failed,
}

impl SettleOutcome {
    /// The word a record names this outcome with, which is the status word the
    /// node's settlement is written under.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// Whether a flag is at its `false` default, so serialization can omit it.
fn is_false(value: &bool) -> bool {
    !*value
}

/// Whether a flag is at its `true` default, so serialization can omit it.
fn is_true(value: &bool) -> bool {
    *value
}

/// A note's [`persist`](Command::Note::persist) default.
fn persists() -> bool {
    true
}

/// Whether a [`Note`](Command::Note) attempts live delivery — and nothing else.
///
/// Two values, and the third one this crate used to carry is **gone**. `auto`
/// named a combination of *both* axes — attempt the running turn, and fall
/// through to the next dispatch when there is none — and that fall-through is
/// persistence spelled a second way, so a contract carrying both `auto` and
/// [`persist`](Command::Note::persist) would have two ways to say one thing.
/// What `auto` meant has an exact spelling in the survivor: `deliver: live` with
/// `persist: true`, which is also the default, so a caller that says nothing
/// gets it.
///
/// A value outside these two is refused by serde, naming what it read — this is
/// external input like any other field, and `auto` is refused by that same rule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Deliver {
    /// Attempt the node's running turn.
    ///
    /// What happens where there is none is not this field's question: it is
    /// [`persist`](Command::Note::persist)'s, which either carries the note to
    /// the node's next dispatch or refuses it for having reached nobody.
    #[default]
    Live,
    /// Attempt no live delivery at all, leaving a running turn alone.
    Next,
}

impl Deliver {
    /// Whether this is the default, so serialization can omit it.
    fn is_default(&self) -> bool {
        matches!(self, Self::Live)
    }
}

/// What a planner surface is asking about.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct SurfaceKind(String);

impl SurfaceKind {
    /// The word a queued surface names this kind with.
    ///
    /// The wire spelling belongs to this validated type rather than a string beside it, so the
    /// kind a queue holds and the kind a command line accepts cannot drift.
    pub const CHECK_IN: &'static str = "check-in";
    /// Something a watcher decided the planner should know.
    pub const FINDING: &'static str = "finding";
    /// An edit an author other than the planner applied on its own judgement,
    /// reported after the fact; the surface's source is that author's word.
    pub const EDIT_APPLIED: &'static str = "edit-applied";
    /// The engine's check-in kind.
    pub fn check_in() -> Self {
        Self(Self::CHECK_IN.into())
    }
    /// The engine's finding kind.
    pub fn finding() -> Self {
        Self(Self::FINDING.into())
    }
    /// The kind's wire word.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::str::FromStr for SurfaceKind {
    type Err = String;
    fn from_str(word: &str) -> Result<Self, Self::Err> {
        valid_word(word).then(|| Self(word.into())).ok_or_else(|| {
            format!("'{word}' is not a surface kind: kinds match ^[a-z][a-z0-9-]{{0,63}}$")
        })
    }
}
impl TryFrom<String> for SurfaceKind {
    type Error = String;
    fn try_from(word: String) -> Result<Self, Self::Error> {
        word.parse()
    }
}
impl From<SurfaceKind> for String {
    fn from(kind: SurfaceKind) -> Self {
        kind.0
    }
}

/// The environment variable bounding how long `reply` waits for the
/// reconciler's verdict before reporting the edits queued.
pub const REPLY_TIMEOUT_ENV: &str = "ONEPIPELINE_REPLY_TIMEOUT_SECONDS";

/// How long `reply` waits for the reconciler's verdict when nothing overrides
/// it.
pub const DEFAULT_REPLY_TIMEOUT_SECONDS: u64 = 30;

/// One asker's name: the word by which two serving sessions are one side.
///
/// The bus's own type, whose two refusals — a **blank** value, which every
/// session carrying it would match, and one that is **not Unicode**, which
/// collapses onto every other such value when it is read — are the words this
/// crate refused them with, named by where the value came from.
pub(crate) use onemessagebus::Asker;

/// The durable channel state for one run.
///
/// Transport state lives beside the journal rather than in memory, so both
/// sides may exit and reattach between messages: **acceptance means delivery**.
/// Nothing has to be listening at the moment the planner writes.
///
/// The run's `channel/` directory, opened as the bus's local transport under the
/// `planner-channel` layout. Each handle below is opened the first time an
/// operation needs it: a read of the surfaces opens the transport and that one
/// queue, and only an operation that routes, asks, binds or judges an offer
/// resolves the whole bus, whose registry is the expensive part.
#[derive(Clone)]
pub(crate) struct ChannelState {
    paths: crate::ledger::RunPaths,
    /// The configuration the run's launch named, checked at the launch.
    config: Option<Arc<Config>>,
    transport: Arc<OnceLock<std::result::Result<Arc<dyn Transport>, String>>>,
    surfaces: Arc<OnceLock<std::result::Result<onemessagebus::Queue<Surface>, String>>>,
    /// The bus the channel's records are appended through: the run's
    /// configuration with its validators left out, because what is appended has
    /// already been judged.
    bus: Arc<OnceLock<std::result::Result<Bus, String>>>,
    /// The bus an offer is judged by: the run's configuration, validators and
    /// all. Resolved only for a run whose configuration declares validators.
    judging: Arc<OnceLock<std::result::Result<Bus, String>>>,
    /// What the last read of the surfaces found, keyed on the transport's change
    /// token for that queue: a reader asking again of a channel nothing has
    /// written since is answered without folding its log a second time.
    seen: Arc<Mutex<Option<(Fingerprint, Queue)>>>,
}

impl std::fmt::Debug for ChannelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelState")
            .field("run", &self.paths.run)
            .finish_non_exhaustive()
    }
}

/// What is waiting to be read, and what has been read but not answered.
///
/// A reading of the surfaces queue — the bus's projection brought up to date
/// with its log, which is where `docs/queues.md` says how — and never a record
/// of it: what a reader decides from, and nothing any writer writes back.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Queue {
    /// The surfaces nobody has read yet, oldest first.
    pub waiting: Vec<Surface>,
    /// The surface a planner consumed and has not answered, abandoned or not.
    pub pending: Option<Surface>,
}

/// One reply as it sits in the durable queue.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct QueuedReply {
    /// Monotonic within the run.
    pub id: u64,
    /// The envelope the planner wrote.
    pub reply: Reply,
    /// When it was written, in epoch milliseconds.
    pub at: u64,
    /// The correlation of the question it answers, when it was bound to one.
    /// Omitted while absent, so a reply bound to no question is the record
    /// 0.28.2 wrote.
    #[serde(
        default,
        deserialize_with = "recorded_correlation",
        skip_serializing_if = "Option::is_none"
    )]
    pub correlation: Option<Correlation>,
}

/// One submitted edit envelope, awaiting the reconciler.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct QueuedCommands {
    /// Monotonic within the run.
    pub id: u64,
    /// Who submitted it, which is what decides the ops it may carry.
    #[serde(default)]
    pub author: Author,
    /// The commands, reconciled in order.
    pub commands: Vec<Command>,
}

/// The one layout a run's channel is kept under.
fn planner_channel() -> Layouts {
    Layouts::new().with(Arc::new(PlannerChannel))
}

/// Read the `onemessagebus` configuration a launch names, and refuse — naming
/// the key and the value read — what a run's channel cannot be kept under.
///
/// Everything is decided before the run exists. What the file alone decides is
/// `Config::load`'s. What only this crate decides is checked here: the transport
/// is the run root's local one, so another kind, a directory or a key of its own
/// is refused; the layout is `planner-channel`, whose queues are the files a
/// release that predates the bus reads, so another profile or a `queues` block
/// is refused. The host-owned bus server alone interprets `schemas` and
/// `codecs`; this engine accepts and ignores both. What only the layout
/// decides — an author it does not declare, a grant it does not give, a
/// validator on a queue it does not have — is decided by resolving the
/// configuration against it over a transport that keeps nothing, so a refused
/// launch writes nothing anywhere.
///
/// # Errors
///
/// [`crate::Error::Invalid`], naming the file, the key and the value read.
pub(crate) fn launch_bus_config(path: &std::path::Path) -> crate::Result<Config> {
    let named =
        |why: String| crate::Error::Invalid(format!("--bus-config {}: {why}", path.display()));
    let mut config = Config::load(path).map_err(|failure| named(failure.to_string()))?;
    if config.transport.kind.as_str() != onemessagebus::LOCAL {
        return Err(named(format!(
            "transport.kind is `{}`, and a run's channel is kept on the `{local}` transport over \
             the run's own channel directory — name `kind: {local}`",
            config.transport.kind,
            local = onemessagebus::LOCAL,
        )));
    }
    if let Some(dir) = &config.transport.dir {
        return Err(named(format!(
            "transport.dir is `{}`, and where a run's channel is kept is its run root's to \
             decide rather than the configuration's — leave `transport.dir` out",
            dir.display()
        )));
    }
    if let Some((key, value)) = config.transport.options.iter().next() {
        // Named as the file spells it, as every other key here is: a string
        // option bare, and anything else in its JSON form.
        let value = value
            .as_str()
            .map_or_else(|| value.to_string(), str::to_owned);
        return Err(named(format!(
            "transport.{key} is `{value}`, which the {} transport does not take",
            onemessagebus::LOCAL
        )));
    }
    if let Some(profile) = config
        .profile
        .as_deref()
        .filter(|profile| *profile != PLANNER_CHANNEL)
    {
        return Err(named(format!(
            "profile is `{profile}`, and a run's channel is the `{PLANNER_CHANNEL}` layout — name \
             it, or leave `profile` out"
        )));
    }
    if let Some(queue) = config.queues.keys().next() {
        return Err(named(format!(
            "queues.{queue} is declared, and a run's queues are the `{PLANNER_CHANNEL}` layout's \
             — leave `queues` out"
        )));
    }
    config.profile = Some(PLANNER_CHANNEL.to_owned());
    let mut probe = config.clone();
    probe.transport = TransportConfig {
        kind: onemessagebus::MEMORY
            .parse()
            .map_err(|failure| named(format!("{failure}")))?,
        dir: None,
        options: Map::new(),
    };
    probe
        .resolve(&planner_channel(), &TransportKinds::builtin())
        .map_err(|failure| named(failure.to_string()))?;
    Ok(config)
}

/// A queue the `planner-channel` layout declares, by the name it declares it
/// under.
fn queue_name(text: &str) -> QueueName {
    QueueName::try_from(text)
        .unwrap_or_else(|_| unreachable!("{text} is a queue the planner-channel layout declares"))
}

/// The layout's own declaration of one of its queues.
fn declared(text: &str) -> QueueSpec {
    layout::queues()
        .into_iter()
        .find(|spec| spec.name.as_str() == text)
        .unwrap_or_else(|| unreachable!("the planner-channel layout declares {text}"))
}

/// A queue's refusal or failure, in the words this channel has always used for
/// it where it had words of its own.
fn queue_failure(failure: QueueError) -> crate::Error {
    match failure {
        // The allocator's refusal: the queue in question is this channel's.
        QueueError::NoIdLeft { last, .. } => crate::Error::Refused(format!(
            "surface: the channel has no id left to allocate; the last one, {last}, has already \
             been queued"
        )),
        // A validator's own words, unaltered.
        QueueError::Refused { reason, .. } | QueueError::Unjudged { reason, .. } => {
            crate::Error::Refused(reason)
        }
        other => crate::Error::Refused(other.to_string()),
    }
}

/// The bus's refusal or failure: the layout's own words where it refused — the
/// allowlist's among them — and the queue's where a queue did.
fn bus_failure(failure: BusError) -> crate::Error {
    match failure {
        BusError::Refused { why, .. } => crate::Error::Refused(why),
        BusError::Queue(failure) => queue_failure(failure),
        other => crate::Error::Refused(other.to_string()),
    }
}

/// Surfaces a queue handed back as records, as the type they are.
#[allow(
    dead_code,
    reason = "shared bus primitives remain part of the library channel API after the CLI server was retired"
)]
fn read_surfaces(records: Vec<Value>) -> crate::Result<Vec<Surface>> {
    records
        .into_iter()
        .map(|record| {
            serde_json::from_value(record)
                .map_err(|failure| crate::Error::Invalid(format!("surface: {failure}")))
        })
        .collect()
}

/// The mark the host's asking wrapper puts in front of the token a question
/// asks its answer to echo.
///
/// A convention rather than a field: at 0.28.2 a reply carried no correlation,
/// so the one binding a listener honoured was this token, echoed inside the
/// verdict's `message`, and every host script answering a question answers that
/// way. A verdict that echoes one binds to the question that carries it before
/// any other rule is asked — see [`ChannelState::answer`].
const ECHOED_TOKEN_PREFIX: &str = "ask-manager-token:";

/// The token a question's text asks its answer to echo, or `None` where it
/// asks for none: the prefix and everything after it on the first line that
/// carries it, which is the whole of what the asking wrapper compares.
fn echoed_token(message: &str) -> Option<String> {
    message.lines().find_map(|line| {
        line.split_once(ECHOED_TOKEN_PREFIX)
            .map(|(_, rest)| format!("{ECHOED_TOKEN_PREFIX}{}", rest.trim()))
    })
}

#[allow(
    dead_code,
    reason = "shared bus primitives remain part of the library channel API after the CLI server was retired"
)]
impl ChannelState {
    /// The channel for one run.
    pub fn new(paths: &crate::ledger::RunPaths) -> Self {
        Self {
            paths: paths.clone(),
            config: None,
            transport: Arc::default(),
            surfaces: Arc::default(),
            bus: Arc::default(),
            judging: Arc::default(),
            seen: Arc::default(),
        }
    }

    /// The channel of one run, under the bus configuration its launch named.
    ///
    /// Every writer that authors, judges or waits on the channel opens it this
    /// way, so the grants, validators and reply window a run enforces are the
    /// ones it was launched under wherever it is written from.
    pub(crate) fn of_run(
        paths: &crate::ledger::RunPaths,
        launch: &crate::ledger::LaunchRecord,
    ) -> Self {
        Self {
            config: launch
                .bus_config
                .as_ref()
                .map(|recorded| Arc::new(recorded.config().clone())),
            ..Self::new(paths)
        }
    }

    /// The bus's local transport over this run's channel directory.
    fn transport(&self) -> crate::Result<Arc<dyn Transport>> {
        self.transport
            .get_or_init(|| {
                LocalTransport::open(self.paths.channel_dir())
                    .map(|local| Arc::new(local) as Arc<dyn Transport>)
                    .map_err(|failure| failure.to_string())
            })
            .clone()
            .map_err(crate::Error::Refused)
    }

    /// The surfaces queue, typed, so every line the bus writes is in
    /// [`Surface`]'s field order.
    fn surfaces(&self) -> crate::Result<onemessagebus::Queue<Surface>> {
        if let Some(opened) = self.surfaces.get() {
            return opened.clone().map_err(crate::Error::Refused);
        }
        let transport = self.transport()?;
        self.surfaces
            .get_or_init(|| {
                onemessagebus::Queue::open(transport, declared(SURFACES))
                    .map_err(|failure| failure.to_string())
            })
            .clone()
            .map_err(crate::Error::Refused)
    }

    /// One of the layout's plain queues over this run's transport, checked
    /// against the schemas the profile registers.
    fn plain(&self, name: &str) -> crate::Result<RawQueue> {
        Ok(RawQueue::open(
            self.transport()?,
            declared(name),
            Arc::clone(registry()),
        ))
    }

    /// The run's bus: the `planner-channel` layout resolved over this run's
    /// channel directory, which is what routes a reply by its halves, asks and
    /// binds a question by its correlation, and judges an offer by its queue's
    /// validators.
    fn bus(&self) -> crate::Result<Bus> {
        self.bus
            .get_or_init(|| self.resolved(false))
            .clone()
            .map_err(crate::Error::Refused)
    }

    /// The bus an offer to this channel is judged by.
    fn judging_bus(&self) -> crate::Result<Bus> {
        if self
            .config
            .as_ref()
            .is_none_or(|config| config.validators.is_empty())
        {
            return self.bus();
        }
        self.judging
            .get_or_init(|| self.resolved(true))
            .clone()
            .map_err(crate::Error::Refused)
    }

    /// The run's configuration — or the profile as declared, for a run whose
    /// launch named none — resolved over this run's channel directory, with its
    /// validators or without them.
    fn resolved(&self, judging: bool) -> std::result::Result<Bus, String> {
        let mut config = match &self.config {
            Some(config) => (**config).clone(),
            None => Config::local(self.paths.channel_dir(), Some(PLANNER_CHANNEL)),
        }
        .with_transport_dir(self.paths.channel_dir());
        config.profile = Some(PLANNER_CHANNEL.to_owned());
        if !judging {
            config.validators.clear();
        }
        config
            .resolve(&planner_channel(), &TransportKinds::builtin())
            .map_err(|failure| failure.to_string())
    }

    /// Whether the run's configuration declares a validator on `queue`.
    fn validates(&self, queue: &str) -> bool {
        self.config.as_ref().is_some_and(|config| {
            config
                .validators
                .iter()
                .any(|validator| validator.on.as_str() == queue)
        })
    }

    /// Whether `author` may issue `command` on this run, under the grants its
    /// configuration narrowed.
    pub(crate) fn allows(&self, author: Author, command: &Command) -> crate::Result<()> {
        match &self.config {
            None => allowed_by(layout_allowlist(), author, command),
            Some(_) => allowed_by(self.bus()?.allowlist(), author, command),
        }
    }

    pub(crate) fn declares(&self, author: &Author) -> crate::Result<()> {
        let configured;
        let allowlist = match &self.config {
            None => layout_allowlist(),
            Some(_) => {
                configured = self.bus()?;
                configured.allowlist()
            }
        };
        if allowlist.declares(&author.word()) {
            return Ok(());
        }
        Err(crate::Error::Refused(format!(
            "the envelope's author `{}` is not declared; the declared authors are: {}",
            author.as_str(),
            allowlist
                .authors()
                .iter()
                .map(onemessagebus::Author::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }

    /// Whether `author` may declare this run finished through a verdict, under
    /// the grants its configuration narrowed.
    pub(crate) fn allows_completion(
        &self,
        author: Author,
        completion: Option<bool>,
    ) -> crate::Result<()> {
        match &self.config {
            None => completion_allowed_by(layout_allowlist(), author, completion),
            Some(_) => completion_allowed_by(self.bus()?.allowlist(), author, completion),
        }
    }

    /// The transport's change tokens for the two queues the reconcile loop reads
    /// off this channel: the surfaces its decisions are held on, and the command
    /// envelopes it applies.
    ///
    /// Compared, never read. A queue the transport could not look at is left
    /// out, so a channel that could not be looked at at all differs from every
    /// one that could, and the moment it can be the loop wakes.
    pub(crate) fn fingerprint(&self) -> Vec<Fingerprint> {
        let Ok(transport) = self.transport() else {
            return Vec::new();
        };
        [SURFACES, COMMANDS]
            .into_iter()
            .filter_map(|name| transport.fingerprint(&queue_name(name)).ok())
            .collect()
    }

    /// The live queue: what is waiting, and what the pending slot holds.
    ///
    /// The bus folds the surfaces log into its projection and repairs that
    /// projection where the log has grown past it, and a projection that could
    /// not be written back costs the next reader a fold, never an answer — so a
    /// view of a run whose own record is intact still renders. A run with no
    /// channel directory has no surfaces, and a read makes no directory in its
    /// place.
    pub fn queue(&self) -> Queue {
        if !self.paths.channel_dir().is_dir() {
            return Queue::default();
        }
        let Ok(surfaces) = self.surfaces() else {
            return Queue::default();
        };
        // Taken before the reads, so a surface queued while they run moves the
        // token past the one this reading is kept under.
        let mark = surfaces.raw().fingerprint().ok();
        if let Some(mark) = &mark {
            let seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((seen, queue)) = seen.as_ref() {
                if seen == mark {
                    return queue.clone();
                }
            }
        }
        // llmlint: ignore-block[no_panics_on_recoverable_errors] a view's read of the channel is lenient by contract: a log this reader could not read renders as the empty queue 0.28.2 rendered, rather than refusing a view of a run whose other records are intact, and every write to the channel still refuses on the same failure — `channel::a_push_whose_log_cannot_be_read_is_refused_and_records_nothing` drives that.
        let queue = Queue {
            waiting: surfaces.waiting().unwrap_or_default(),
            pending: surfaces
                .raw()
                .held()
                .ok()
                .flatten()
                .and_then(|held| serde_json::from_value(held.record).ok()),
        };
        // llmlint: ignore-end[no_panics_on_recoverable_errors]
        if let Some(mark) = mark {
            *self.seen.lock().unwrap_or_else(PoisonError::into_inner) = Some((mark, queue.clone()));
        }
        queue
    }

    /// Queue one surface, and record that it was *sent*.
    ///
    /// The id is the bus's to allocate: one past the highest the log has ever
    /// queued, under the queue's exclusive section, so a surface queued while a
    /// reader is reading the channel keeps its id and its place, and the one id
    /// no id could follow is refused rather than handed out. Exactly one
    /// check-in is ever waiting: the layout supersedes a waiting check-in with
    /// the next, so being ignored makes the harness louder rather than quieter.
    pub fn push(&self, surface: Surface) -> crate::Result<Surface> {
        if self.validates(SURFACES) {
            let surfaces = queue_name(SURFACES);
            let offered = serde_json::to_value(&surface)
                .map_err(|failure| crate::Error::Invalid(format!("surface: {failure}")))?;
            QueueError::of_verdict(
                &surfaces,
                self.judging_bus()?
                    .validate(&surfaces, offered)
                    .map_err(bus_failure)?,
            )
            .map_err(queue_failure)?;
        }
        Ok(self
            .surfaces()?
            .push(&surface)
            .map_err(queue_failure)?
            .record)
    }

    /// Claim the next readable surface: **a blocking one first**, and arrival
    /// order within each class.
    ///
    /// A blocking surface holds back the subtree that depends on it and produces
    /// no other signal until somebody reads it, so a question queued behind
    /// narration is claimed before it. A blocking surface outlives its delivery
    /// while it waits for an answer, held in the pending slot; an abandoned one
    /// is handed over last, and takes the slot only where nothing else holds it.
    /// The claim is recorded before the surface is handed over, which is what
    /// keeps it from being handed out twice.
    pub fn claim(&self) -> crate::Result<Option<Surface>> {
        Ok(self
            .surfaces()?
            .claim(&ConsumerName::default_consumer())
            .map_err(queue_failure)?
            .map(|claimed| claimed.record))
    }

    /// Say of every surface in `raised` that nobody is waiting for its answer.
    ///
    /// Called when a serving process is about to exit. **Marked, not
    /// discarded**, and **nothing moves**, the pending slot included: the text
    /// stays exactly where a reader looks for it, and what the mark gives up is
    /// the surface's claim on the unread count and on the decisions the run is
    /// held on. [`attend`](Self::attend) is what can take it back. Returns what
    /// it marked, so the caller can record it.
    pub fn abandon(&self, raised: &[u64]) -> crate::Result<Vec<Surface>> {
        read_surfaces(
            self.surfaces()?
                .raw()
                .abandon(raised)
                .map_err(queue_failure)?,
        )
    }

    /// Take back over everything `asker` left outstanding, and say what was
    /// taken.
    ///
    /// Scoped to the asker: a session of some *other* asker is not a reader for
    /// this one's questions, and a surface naming no asker is adopted by nobody.
    /// Nothing moves and nothing is re-queued, so a surface a manager has
    /// already been handed is not handed to them twice by its asker coming back.
    pub fn attend(&self, asker: &Asker) -> crate::Result<Vec<Surface>> {
        read_surfaces(
            self.surfaces()?
                .raw()
                .attend(asker)
                .map_err(queue_failure)?,
        )
    }

    /// The surface waiting for an answer, if one is.
    ///
    /// The slot can hold a surface nobody is waiting on — an abandoned one stays
    /// where it was delivered — and that is not a surface waiting for an answer.
    pub fn pending(&self) -> Option<Surface> {
        self.held().filter(|surface| !surface.abandoned)
    }

    /// Whatever the pending slot holds, abandoned or not.
    pub fn held(&self) -> Option<Surface> {
        self.queue().pending
    }

    /// Record that a reply answered whatever was pending, and answer the reply's
    /// id.
    ///
    /// This is the **verdict** path: what it queues is what a reader waiting on
    /// a ruling takes, and releasing the pending slot is what releases the
    /// subtree a blocking surface held. An envelope with edits reaches it
    /// through [`answer_if_verdict`](Self::answer_if_verdict).
    ///
    /// A verdict naming no question is bound to one where it can be, in this
    /// order, and answers it through the bus — which stamps the question's
    /// correlation on the reply, so the listener waiting on that question and
    /// no other takes it:
    ///
    /// 1. the question whose [`echoed_token`] the verdict's `message` carries;
    /// 2. the question the pending slot holds;
    /// 3. the oldest question still outstanding that a live listener is waiting
    ///    on.
    ///
    /// With none of those it is queued as 0.28.2 queued it, carrying no
    /// correlation, for the next listener to read: a question whose listener
    /// has gone may already have been handed a verdict bound to nothing, and
    /// binding a later one to it would keep that verdict from every listener
    /// still reading. Either way the pending slot is released, as a verdict has
    /// always released it.
    pub fn answer(&self, reply: &Reply) -> crate::Result<u64> {
        self.answer_bound(reply, None)
    }

    /// [`answer`](Self::answer), bound to the question `named` carries where a
    /// caller named one.
    ///
    /// A named correlation is the whole of the binding: nothing pending holding
    /// it is refused naming it, with nothing appended, and a slot holding some
    /// other question is left holding it.
    pub(crate) fn answer_bound(
        &self,
        reply: &Reply,
        named: Option<&Correlation>,
    ) -> crate::Result<u64> {
        let bus = self.bus()?;
        let framed = serde_json::json!({"id": 0, "reply": reply, "at": crate::sys::now_millis()});
        if let Some(correlation) = named {
            return Self::bound_through(&bus, correlation, framed);
        }
        let id = match self.binding_for(&bus, reply)? {
            Some(correlation) => Self::bound_through(&bus, &correlation, framed)?,
            None => {
                let replies = queue_name(REPLIES);
                QueueError::of_verdict(
                    &replies,
                    bus.validate(&replies, framed.clone())
                        .map_err(bus_failure)?,
                )
                .map_err(queue_failure)?;
                // Released first and appended after, as 0.28.2 did: a reader that
                // finds the reply finds the slot already released.
                self.surfaces()?
                    .raw()
                    .answer_pending()
                    .map_err(queue_failure)?;
                self.plain(REPLIES)?
                    .push(framed)
                    .map_err(queue_failure)?
                    .id
                    .unwrap_or_default()
            }
        };
        self.surfaces()?
            .raw()
            .answer_pending()
            .map_err(queue_failure)?;
        Ok(id)
    }

    /// Answer the question `correlation` names with the framed reply, through
    /// the bus's reply binding, and answer the reply's id.
    fn bound_through(bus: &Bus, correlation: &Correlation, framed: Value) -> crate::Result<u64> {
        let bound = bus
            .reply(&queue_name(SURFACES), Some(correlation), framed)
            .map_err(bus_failure)?;
        Ok(bound
            .sent
            .iter()
            .find(|(queue, _)| queue.as_str() == REPLIES)
            .and_then(|(_, pushed)| pushed.id)
            .unwrap_or_default())
    }

    /// The question a verdict naming none is bound to, where it binds to one;
    /// [`answer`](Self::answer) states the order.
    fn binding_for(&self, bus: &Bus, reply: &Reply) -> crate::Result<Option<Correlation>> {
        let outstanding = self.outstanding()?;
        if outstanding.is_empty() {
            return Ok(None);
        }
        if let Some(message) = &reply.message {
            if let Some(asked) = outstanding.iter().find(|asked| {
                echoed_token(&asked.message).is_some_and(|token| message.contains(&token))
            }) {
                return Ok(asked.correlation.clone());
            }
        }
        if let Some(held) = self.held().and_then(|held| held.correlation) {
            if outstanding
                .iter()
                .any(|asked| asked.correlation.as_ref() == Some(&held))
            {
                return Ok(Some(held));
            }
        }
        let surfaces = queue_name(SURFACES);
        for correlation in outstanding
            .iter()
            .filter_map(|asked| asked.correlation.as_ref())
        {
            let listener = bus
                .listen::<Value>(&surfaces, correlation, &Lifetime::Session)
                .map_err(bus_failure)?;
            if !listener.is_abandoned().map_err(queue_failure)? {
                return Ok(Some(correlation.clone()));
            }
        }
        Ok(None)
    }

    /// Every question still waiting for its answer, oldest first: each surface
    /// the log queued under a correlation no reply on the reply log carries.
    fn outstanding(&self) -> crate::Result<Vec<Surface>> {
        let answered: BTreeSet<Correlation> = self
            .replies()
            .into_iter()
            .filter_map(|queued| queued.correlation)
            .collect();
        let mut asked = BTreeSet::new();
        Ok(self
            .surfaces()?
            .raw()
            .log(None)
            .map_err(queue_failure)?
            .into_iter()
            .filter(|(line, _)| line.get("event").and_then(Value::as_str) == Some("queued"))
            .filter_map(|(line, _)| serde_json::from_value::<Surface>(line).ok())
            .filter(|surface| {
                surface.correlation.as_ref().is_some_and(|correlation| {
                    !answered.contains(correlation) && asked.insert(correlation.clone())
                })
            })
            .collect())
    }

    /// Route a reply that carried edits by the halves it carries.
    ///
    /// Its commands have already reached the command path — applied inline, or
    /// through the durable queue [`submit`](Self::submit) writes — so what is
    /// left to route is the verdict half. Present, it answers the pending
    /// surface exactly as a commandless reply does. Absent, this envelope is the
    /// command path's alone: `pending` is left standing, because a graph edit
    /// answers no question, and nothing is queued for a reader that could only
    /// misread it as a ruling.
    pub fn answer_if_verdict(&self, reply: &Reply) -> crate::Result<()> {
        self.answer_if_verdict_bound(reply, None)
    }

    /// [`answer_if_verdict`](Self::answer_if_verdict), bound as
    /// [`answer_bound`](Self::answer_bound) binds.
    pub(crate) fn answer_if_verdict_bound(
        &self,
        reply: &Reply,
        named: Option<&Correlation>,
    ) -> crate::Result<()> {
        if reply.carries_verdict() {
            self.answer_bound(reply, named)?;
        }
        Ok(())
    }

    /// Every reply the planner has written, in order.
    pub fn replies(&self) -> Vec<QueuedReply> {
        let Ok(replies) = self.plain(REPLIES) else {
            return Vec::new();
        };
        replies
            .log(None)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(record, _)| serde_json::from_value(record).ok())
            .collect()
    }

    /// Claim the next verdict no reader has taken yet.
    ///
    /// **One claim, one reply**, oldest first, each claim moving the reply
    /// cursor just past the reply it took. A commands-only envelope is not on
    /// the verdict side of the channel, so the layout's claim passes over it:
    /// its reader is the command queue, which holds its own copy behind its own
    /// cursor.
    pub fn claim_reply(&self) -> crate::Result<Option<QueuedReply>> {
        let replies = self.plain(REPLIES)?;
        while let Some(claimed) = replies
            .claim(&ConsumerName::default_consumer())
            .map_err(queue_failure)?
        {
            if let Ok(queued) = serde_json::from_value::<QueuedReply>(claimed.record) {
                return Ok(Some(queued));
            }
        }
        Ok(None)
    }

    /// Record that `reply` reached the listener it answers.
    ///
    /// The listener took it by its correlation rather than by claiming it, so
    /// where the reply cursor has not got past it, the replies up to it are
    /// claimed — which leaves the cursor just past it, where 0.28.2's listener,
    /// claiming it, left the cursor. A cursor already past it stays where it is.
    pub(crate) fn delivered(&self, reply: &QueuedReply) -> crate::Result<()> {
        let behind = self
            .plain(REPLIES)?
            .waiting()
            .map_err(queue_failure)?
            .iter()
            .any(|record| record.get("id").and_then(Value::as_u64) == Some(reply.id));
        if behind {
            while let Some(claimed) = self.claim_reply()? {
                if claimed.id >= reply.id {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Raise `question` as one the bus answers by correlation, and hand back
    /// the handle its answer arrives on.
    ///
    /// Stamped by the bus with its correlation, its blocking flag and its
    /// asker, and judged, validated and appended as any surface is.
    pub(crate) fn ask(&self, question: Surface) -> crate::Result<Pending<Value>> {
        let options = AskOptions {
            blocking: question.blocking,
            asker: question.asker.clone(),
            about: None,
        };
        self.judging_bus()?
            .ask::<Surface, Value>(&queue_name(SURFACES), question, options)
            .map_err(bus_failure)
    }

    /// Listen again for the question `correlation` names, raising nothing: a
    /// listener of the asker that question carries takes it back, and any other
    /// attends nothing.
    pub(crate) fn listen(
        &self,
        correlation: &Correlation,
        asker: Option<&Asker>,
    ) -> crate::Result<Pending<Value>> {
        let lifetime = asker.map_or(Lifetime::Session, |asker| Lifetime::Durable(asker.clone()));
        self.bus()?
            .listen::<Value>(&queue_name(SURFACES), correlation, &lifetime)
            .map_err(bus_failure)
    }

    /// The oldest verdict still after the reply cursor that a listener of
    /// `asker` is owed without having asked for it by its correlation: one
    /// answering a question that asker raised, or one bound to no question at
    /// all.
    ///
    /// Both are what 0.28.2's listener read. A listener is rented and the asker
    /// is not, so a session of the asker that re-armed behind a question of its
    /// own is still the reader of the ruling on the question the asker is
    /// blocked on; and a verdict queued while no question was outstanding is
    /// taken by whichever listener reads next, as every verdict was then. A
    /// ruling on another asker's question is never one of them.
    pub(crate) fn owed(&self, asker: Option<&Asker>) -> crate::Result<Option<QueuedReply>> {
        let raised: BTreeSet<Correlation> = match asker {
            Some(asker) => self
                .surfaces()?
                .raw()
                .log(None)
                .map_err(queue_failure)?
                .into_iter()
                .filter(|(line, _)| line.get("event").and_then(Value::as_str) == Some("queued"))
                .filter_map(|(line, _)| serde_json::from_value::<Surface>(line).ok())
                .filter(|surface| surface.asker.as_ref() == Some(asker))
                .filter_map(|surface| surface.correlation)
                .collect(),
            None => BTreeSet::new(),
        };
        Ok(self
            .plain(REPLIES)?
            .waiting()
            .map_err(queue_failure)?
            .into_iter()
            .filter_map(|record| serde_json::from_value::<QueuedReply>(record).ok())
            .find(|reply| {
                reply
                    .correlation
                    .as_ref()
                    .is_none_or(|correlation| raised.contains(correlation))
            }))
    }

    /// The transport's change token for the reply queue.
    pub(crate) fn replies_mark(&self) -> crate::Result<Fingerprint> {
        self.transport()?
            .fingerprint(&queue_name(REPLIES))
            .map_err(|failure| crate::Error::Refused(failure.to_string()))
    }

    /// Wait up to `timeout` for the reply queue to move from `since`: the
    /// transport's own wait, which is the only wait a listener makes.
    pub(crate) fn wait_for_replies(
        &self,
        since: &Fingerprint,
        timeout: std::time::Duration,
    ) -> crate::Result<()> {
        self.transport()?
            .wait_for_change(&queue_name(REPLIES), since, timeout)
            .map(|_| ())
            .map_err(|failure| crate::Error::Refused(failure.to_string()))
    }

    /// Judge a reply envelope as the reply queue judges one offered to it,
    /// appending nothing: the layout's routing and the author's grants, `review`
    /// registered on the reply queue as a validator of it, and each queue the
    /// envelope would be routed to by that queue's own validators.
    pub(crate) fn judge_reply<V: onemessagebus::Validator<Value> + 'static>(
        &self,
        reply: &Reply,
        review: Option<V>,
    ) -> crate::Result<()> {
        if review.is_none() && !self.validates(REPLIES) && !self.validates(COMMANDS) {
            return Ok(());
        }
        let replies = queue_name(REPLIES);
        let offered = serde_json::to_value(reply)
            .map_err(|failure| crate::Error::Invalid(format!("reply: {failure}")))?;
        let mut bus = self.judging_bus()?;
        if let Some(review) = review {
            bus = bus
                .with_validator::<Value>(&replies, review)
                .map_err(bus_failure)?;
        }
        QueueError::of_verdict(
            &replies,
            bus.validate(&replies, offered).map_err(bus_failure)?,
        )
        .map_err(queue_failure)
    }

    /// Append one envelope of edits to the durable command queue.
    ///
    /// Offered through the bus, so the layout checks each op against the
    /// author's grants before anything is appended.
    pub fn submit(&self, author: Author, commands: &[Command]) -> crate::Result<u64> {
        let offered = serde_json::json!({"id": 0, "author": author, "commands": commands});
        let sent = self
            .bus()?
            .send(&queue_name(COMMANDS), offered)
            .map_err(bus_failure)?;
        Ok(sent
            .first()
            .and_then(|(_, pushed)| pushed.id)
            .unwrap_or_default())
    }

    /// Exactly what [`claim_commands`](Self::claim_commands) would take,
    /// **without** taking it.
    ///
    /// What a writer about to let go of the run asks: whether there is anything
    /// left for a reconciler to do. Claiming to find out would take envelopes
    /// off the queue this process is not going to apply.
    // llmlint: ignore-block[changed_behavior_has_e2e] the leniency here is not this
    // function's own: it reads the queue exactly as `claim_commands` does, because the
    // question it answers is what that call would take. A record no build can read is
    // passed over by both, and a writer that stayed for one would be waiting on work
    // nothing will ever claim — which is the state that has no way out.
    pub(crate) fn claimable_commands(&self) -> Vec<QueuedCommands> {
        let Ok(commands) = self.plain(COMMANDS) else {
            return Vec::new();
        };
        commands
            .waiting()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|record| serde_json::from_value(record).ok())
            .collect()
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]

    /// Claim the command envelopes the reconciler has not drained yet.
    pub fn claim_commands(&self) -> crate::Result<Vec<QueuedCommands>> {
        let commands = self.plain(COMMANDS)?;
        let mut claimed = Vec::new();
        while let Some(envelope) = commands
            .claim(&ConsumerName::default_consumer())
            .map_err(queue_failure)?
        {
            if let Ok(envelope) = serde_json::from_value(envelope.record) {
                claimed.push(envelope);
            }
        }
        Ok(claimed)
    }

    /// Answer one claimed envelope, so its submitter can stop waiting.
    pub fn answer_commands(&self, outcome: &CommandOutcome) -> crate::Result<()> {
        let record = serde_json::to_value(outcome)
            .map_err(|failure| crate::Error::Invalid(format!("outcome: {failure}")))?;
        self.plain(COMMAND_OUTCOMES)?
            .push(record)
            .map_err(queue_failure)?;
        Ok(())
    }

    /// Every answer the reconciler has given, in the order it gave them.
    // llmlint: ignore-block[changed_behavior_has_e2e] a view's read of the channel is
    // lenient by contract, on the terms `queue` and `replies` above already read by: a
    // queue this reader cannot open, a log it cannot read, or a record this build cannot
    // decode renders as the answer 0.28.2 rendered — nothing — rather than refusing a
    // view of a run whose other records are intact, and every *write* to the channel
    // still refuses on the same failure (`channel::a_push_whose_log_cannot_be_read_is_refused_and_records_nothing`).
    // An unreadable or half-written channel log is a host state no verb produces; the
    // reads that succeed are driven end to end by `parity::channel_queue_reads_the_channel_and_refuses_a_run_that_is_not_there`
    // over a recorded channel holding replies, edits and outcomes.
    pub fn outcomes(&self) -> Vec<CommandOutcome> {
        let Ok(outcomes) = self.plain(COMMAND_OUTCOMES) else {
            return Vec::new();
        };
        outcomes
            .log(None)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(record, _)| serde_json::from_value(record).ok())
            .collect()
    }

    /// Every surface the run has ever raised, in the order it raised them —
    /// read and unread, answered and abandoned alike — each as its **latest**
    /// record.
    ///
    /// The surfaces log is a log of transitions: a surface is appended when it is
    /// queued, again when it is claimed, and again when it is answered, each
    /// record carrying the whole surface as it then stood. One entry per surface
    /// is what a reader wants, and the last record is the surface as it stands.
    pub fn every_surface(&self) -> Vec<Surface> {
        if !self.paths.channel_dir().is_dir() {
            return Vec::new();
        }
        let Ok(surfaces) = self.surfaces() else {
            return Vec::new();
        };
        let mut latest: Vec<Surface> = Vec::new();
        for record in surfaces
            .raw()
            .log(None)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(record, _)| serde_json::from_value::<Surface>(record).ok())
        {
            match latest.iter_mut().find(|held| held.id == record.id) {
                Some(held) => *held = record,
                None => latest.push(record),
            }
        }
        latest
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]

    /// The reconciler's answer to one envelope, if it has given one.
    pub fn outcome_of(&self, id: u64) -> Option<CommandOutcome> {
        self.plain(COMMAND_OUTCOMES)
            .ok()?
            .log(None)
            .ok()?
            .into_iter()
            .filter_map(|(record, _)| serde_json::from_value::<CommandOutcome>(record).ok())
            .find(|outcome| outcome.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The checked-in shape of the envelope this build writes.
    ///
    /// Read rather than restated, for the reason `src/filter.rs`'s launch-config
    /// golden is: this is the document a *person types* and this build parses, so
    /// it is the only thing that stops a field being renamed, an optional one
    /// becoming an explicit null, or the version moving without anyone deciding
    /// to move it.
    const ENVELOPE_GOLDEN: &str = include_str!("../tests/golden/reply-envelope-v3.json");

    /// The checked-in envelope at the version before this one.
    ///
    /// A bump is only additive if the envelopes written against the version it
    /// leaves behind are still read, so the one this build has to go on reading
    /// is checked in beside the one it writes rather than described.
    const ENVELOPE_GOLDEN_BEFORE: &str = include_str!("../tests/golden/reply-envelope-v2.json");

    /// The envelope the golden pins, as the types hold it.
    ///
    /// Three settles, because what the golden has to pin about this op is the
    /// **optional** landing at every value it takes: each of the two spellings
    /// `onevcs` resolves work by, and the absence that is every envelope written
    /// before the field existed. The author is the planner and the golden names
    /// none, which is the same rule one rung up: omitted *is* the planner, and an
    /// envelope that wrote the default back would be a key every caller written
    /// before authors existed never sent.
    fn envelope_golden() -> Reply {
        let settled = |id: &str, outcome: SettleOutcome, evidence: &str, landing: Option<&str>| {
            Command::Settle {
                id: id.to_owned(),
                outcome,
                evidence: evidence.to_owned(),
                landing: landing.map(str::to_owned),
                // The one settle here that is a merge somebody verified the release
                // of, so the golden pins the release beside its landing too.
                release: (id == "release").then(|| StatedRelease {
                    target: "crate".parse().expect("a target name"),
                    version: "0.2.31".to_owned(),
                }),
            }
        };
        Reply {
            version: Some(REPLY_ENVELOPE_VERSION),
            author: Author::planner(),
            commands: vec![
                settled(
                    "publish",
                    SettleOutcome::Done,
                    "the change merged while the dispatch was dying; the run recorded the \
                     death and never the merge",
                    Some("https://github.com/owner/engine/pull/12"),
                ),
                settled(
                    "release",
                    SettleOutcome::Done,
                    "the operator read the merge on the base branch",
                    Some("3f9a1c2e5b7d9081f2a3b4c5d6e7f8091a2b3c4d"),
                ),
                settled(
                    "announce",
                    SettleOutcome::Failed,
                    "the wait it was on can never clear, and nothing published",
                    None,
                ),
            ],
            ..Reply::default()
        }
    }

    /// The envelope is the shape the golden pins, at the version this build
    /// writes.
    ///
    /// A change to this serialized contract moves the version and brings its own
    /// golden — `docs/contract-divergences.md` entry 60 moved it to 2 and entry
    /// 57 moves it to 3 for the `landing` this file carries, and
    /// `tests/contract.rs` holds the constant and the read set against the
    /// numbers those entries name.
    #[test]
    fn the_reply_envelope_is_the_shape_the_golden_pins() {
        let rendered =
            serde_json::to_string_pretty(&envelope_golden()).expect("the envelope serialises");
        assert_eq!(
            rendered.trim(),
            ENVELOPE_GOLDEN.trim(),
            "the reply envelope changed shape. Bump REPLY_ENVELOPE_VERSION, add the golden \
             for the new version beside this one, keep this one as the version the build goes \
             on reading, and say so in entry 57"
        );
    }

    /// Both goldens are envelopes the schema this crate generates admits, and
    /// each is admitted by the document the agent profile registers for the
    /// version it was written against — so the type, the profile's documents and
    /// what a person types are one shape.
    #[test]
    fn both_goldens_validate_under_this_crates_schema_and_the_profiles_documents() {
        let mut own = onemessagebus::Registry::new();
        own.register::<Reply>()
            .expect("this crate's envelope schema registers");
        let profile = layout::registry();
        let at = |version: u32| SchemaId::literal("agent", "reply-envelope", version);
        for (golden, version) in [(ENVELOPE_GOLDEN, 3), (ENVELOPE_GOLDEN_BEFORE, 2)] {
            let document: Value = serde_json::from_str(golden).expect("the golden is JSON");
            own.check(&Reply::SCHEMA, &document)
                .unwrap_or_else(|failure| {
                    panic!(
                        "the version {version} golden is refused by this crate's schema: {failure}"
                    )
                });
            profile.check(&at(version), &document).unwrap_or_else(|failure| {
                panic!("the version {version} golden is refused by the profile's document: {failure}")
            });
        }
        // Neither schema is vacuous: a field no envelope has is refused by both.
        let mut stray: Value = serde_json::from_str(ENVELOPE_GOLDEN).expect("the golden is JSON");
        stray["stray"] = json!(true);
        assert!(own.check(&Reply::SCHEMA, &stray).is_err());
        assert!(profile.check(&at(3), &stray).is_err());
    }

    /// An envelope written against the version before this one is still read, and
    /// is read at the version this build reads it at.
    ///
    /// This is the whole of what makes the bump additive: version 3 added an
    /// optional field and took nothing away, so every caller still typing 2 —
    /// this crate's own journeys, the shipped observer persona, and the
    /// orchestration repository's reply wrapper, which passes an operator's
    /// envelope through — sends a document this build reads unchanged. The number
    /// it declares is what it was written against; the number it is read at is
    /// the one every reader downstream asks about.
    #[test]
    fn an_envelope_at_the_version_before_this_one_is_still_read() {
        let before: Reply =
            serde_json::from_str(ENVELOPE_GOLDEN_BEFORE).expect("the older envelope still reads");
        let declared: Value =
            serde_json::from_str(ENVELOPE_GOLDEN_BEFORE).expect("the golden is JSON");
        assert_eq!(
            declared["version"],
            json!(2),
            "the golden for the version before this one is not at that version"
        );
        assert_eq!(
            before.version,
            Some(REPLY_ENVELOPE_VERSION),
            "an envelope at a version this build reads was not read at the version it reads"
        );
        assert!(
            REPLY_ENVELOPE_VERSIONS_READ.contains(&2),
            "the version that golden was written against is no longer read"
        );
        assert_eq!(
            before.commands.len(),
            2,
            "the older envelope's commands did not survive: {before:?}"
        );
        for command in &before.commands {
            let Command::Settle {
                landing, release, ..
            } = command
            else {
                panic!("the older envelope carries something other than a settle: {command:?}");
            };
            assert_eq!(
                landing.as_deref(),
                None,
                "a settle written before the landing existed came back carrying one"
            );
            assert_eq!(
                release, &None,
                "a settle written before it gained a release"
            );
        }

        // A number this build does not read is left as the envelope declared it,
        // so the refusal a caller meets is the one the driver has always made
        // about an edit envelope's version rather than a parse error here.
        let ancient: Reply = serde_json::from_value(json!({
            "version": 1,
            "commands": [{"op": "cancel", "id": "build"}],
        }))
        .expect("an unreadable version is not a parse failure");
        assert_eq!(
            ancient.version,
            Some(1),
            "a version this build does not read was carried forward as one it does"
        );
    }

    /// Both landings survive the wire, and a settle that names none carries no
    /// key.
    ///
    /// The omission is the half that has to be checked at the wire rather than
    /// through the types: `None` and a landing are different values in Rust
    /// whatever the serializer does, but a `landing: null` on a settle that named
    /// none would have every caller written before the field branching on one
    /// that is always present and usually meaningless — which is the whole of
    /// what makes an optional field additive.
    #[test]
    fn a_settled_landing_round_trips_at_both_spellings_and_is_omitted_where_there_is_none() {
        let envelope = envelope_golden();
        let read: Reply =
            serde_json::from_str(ENVELOPE_GOLDEN).expect("the golden reads back into the types");
        assert_eq!(read, envelope, "the golden is not the envelope it pins");
        let again: Reply =
            serde_json::from_str(&serde_json::to_string(&envelope).expect("it serialises"))
                .expect("it reads back");
        assert_eq!(again, envelope, "the envelope does not round-trip");

        let document: Value =
            serde_json::from_str(&serde_json::to_string(&envelope).expect("it serialises"))
                .expect("it is JSON");
        assert_eq!(
            document["commands"][0]["landing"],
            json!("https://github.com/owner/engine/pull/12"),
            "the change-request spelling of a landing did not survive the wire"
        );
        assert_eq!(
            document["commands"][1]["landing"],
            json!("3f9a1c2e5b7d9081f2a3b4c5d6e7f8091a2b3c4d"),
            "the commit spelling of a landing did not survive the wire"
        );
        assert!(
            document["commands"][2].get("landing").is_none(),
            "a settle that named no landing carries a landing key anyway: {}",
            document["commands"][2]
        );

        // And an envelope written before the field existed is exactly the
        // envelope it was: it parses, carries no landing, and writes none back.
        let bare = json!({
            "version": REPLY_ENVELOPE_VERSION,
            "commands": [{
                "op": "settle", "id": "announce", "outcome": "failed",
                "evidence": "the wait it was on can never clear, and nothing published",
            }],
        });
        let before: Reply = serde_json::from_value(bare.clone()).expect("it parses");
        assert_eq!(
            serde_json::to_value(&before).expect("it serialises"),
            bare,
            "an envelope written before the landing existed did not round-trip unchanged"
        );
    }

    /// A question's token is the prefix and the rest of the first line carrying
    /// it, and a message carrying none asks for none.
    #[test]
    fn a_token_is_read_off_the_first_line_that_carries_it() {
        assert_eq!(
            echoed_token("Blocked on this.\nThe token to echo:\nask-manager-token:0a1b2c  \n"),
            Some("ask-manager-token:0a1b2c".to_owned())
        );
        assert_eq!(
            echoed_token("ask-manager-token:first\nask-manager-token:second"),
            Some("ask-manager-token:first".to_owned())
        );
        assert_eq!(echoed_token("nothing to echo here"), None);
    }

    /// The prefix is the one the contract and the README tell a host its answer
    /// echoes. The asking wrapper is another repository's, and those two
    /// documents are what it is written against, so a prefix changed here and
    /// not there fails here rather than silently ending binding by token.
    #[test]
    fn the_token_prefix_is_the_one_the_contract_and_the_readme_state() {
        for document in ["docs/contract.md", "README.md"] {
            let text = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(document),
            )
            .expect("the document ships");
            assert!(
                text.contains(&format!("`{ECHOED_TOKEN_PREFIX}`")),
                "{document} does not state the prefix `{ECHOED_TOKEN_PREFIX}` a verdict echoes"
            );
        }
    }

    /// A channel under a fresh run root, removed when the test ends.
    struct Scratch {
        root: std::path::PathBuf,
        channel: ChannelState,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("onepipeline-channel-{name}-{}", crate::sys::pid()));
            let _ = std::fs::remove_dir_all(&root);
            let paths = crate::ledger::RunPaths::under(&root, "bound");
            paths.create().expect("the run directory");
            Self {
                channel: ChannelState::new(&paths),
                root,
            }
        }

        /// A question raised through the host bus.
        fn asked(&self, message: &str, blocking: bool) -> Correlation {
            self.channel
                .ask(Surface {
                    id: 0,
                    kind: "planner-question".to_owned(),
                    message: message.to_owned(),
                    source: source::PROPOSAL.to_owned(),
                    blocking,
                    queued_at: 1,
                    workstream: None,
                    abandoned: false,
                    asker: None,
                    correlation: None,
                })
                .expect("the question is asked")
                .correlation()
                .clone()
        }

        fn verdict(message: &str) -> Reply {
            Reply {
                completion: Some(false),
                message: Some(message.to_owned()),
                ..Reply::default()
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A verdict echoing a question's token binds to that question, ahead of
    /// the one the pending slot holds and ahead of any a live listener waits on.
    ///
    /// Two asks outstanding at once is the case the order exists for: a manager
    /// answering the second must not have the answer routed to the first by
    /// arrival order.
    #[test]
    fn a_verdict_binds_to_the_question_whose_token_it_echoes() {
        let scratch = Scratch::new("token");
        let first = scratch.asked("first\nask-manager-token:aaaa", true);
        let second = scratch.asked("second\nask-manager-token:bbbb", true);
        scratch.channel.claim().expect("the first is claimed");

        let id = scratch
            .channel
            .answer(&Scratch::verdict("main, ask-manager-token:bbbb"))
            .expect("the verdict is answered");
        let replies = scratch.channel.replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].id, id);
        assert_eq!(replies[0].correlation.as_ref(), Some(&second));

        // And one echoing nothing binds to the slot's question, the first.
        scratch
            .channel
            .answer(&Scratch::verdict("carry on"))
            .expect("the second verdict is answered");
        assert_eq!(
            scratch.channel.replies()[1].correlation.as_ref(),
            Some(&first)
        );
        assert_eq!(scratch.channel.held(), None, "the slot was not released");
    }

    /// With no question outstanding a verdict is the record 0.28.2 queued, and a
    /// named correlation nothing pending holds is refused naming it with nothing
    /// appended.
    #[test]
    fn a_verdict_bound_to_nothing_is_queued_as_it_was_and_a_named_stranger_is_refused() {
        let scratch = Scratch::new("unbound");
        scratch
            .channel
            .answer(&Scratch::verdict("nothing asked"))
            .expect("the verdict is queued");
        let queued = scratch.channel.replies();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].correlation, None);
        let line = std::fs::read_to_string(scratch.root.join("bound/channel/replies.jsonl"))
            .expect("the reply log");
        assert!(!line.contains("correlation"), "{line}");

        let stranger: Correlation = "c-not-asked".parse().expect("a correlation");
        let refused = scratch
            .channel
            .answer_bound(&Scratch::verdict("to nobody"), Some(&stranger))
            .expect_err("a correlation nothing holds binds nothing");
        assert!(refused.to_string().contains("c-not-asked"), "{refused}");
        assert_eq!(
            scratch.channel.replies().len(),
            1,
            "a refused reply was appended"
        );
    }

    /// A read of a run with no channel directory is an empty queue, and makes no
    /// directory in its place.
    #[test]
    fn a_run_with_no_channel_directory_reads_as_an_empty_queue_and_gains_none() {
        let scratch = Scratch::new("absent");
        let channel = scratch.root.join("bound/channel");
        std::fs::remove_dir_all(&channel).expect("the channel directory is removed");
        assert_eq!(scratch.channel.queue(), Queue::default());
        assert!(!channel.exists(), "a read made the channel directory");
    }
}
