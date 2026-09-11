//! The planner channel: the wire shapes, and the durable queue behind them.
//!
//! A reply is one JSON envelope: a legacy verdict, a versioned list of graph
//! edits, or both. The edits' required fields and validation semantics are
//! `ai-orchestrator`'s live-edit protocol exactly, per `docs/contract.md`.
//!
//! Which reader takes one follows from **which of those three it is**, and not
//! from which reader reached the queue first: see [`Reply`].
//!
//! `ChannelState` is the transport: it queues surfaces and replies, hands each
//! out once, and records what a submitted command list was answered with. It
//! does not *judge* an edit — whether a target exists, is in the right state,
//! and leaves an acyclic graph is a question about the live frontier, and the
//! reconciler in `edits` is what asks it. This file's promise is that nothing
//! queued is lost and nothing is delivered twice.

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

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::note::{Addressee, Criterion, NoteText};
use crate::plan::Node;

/// The reply envelope version this crate **writes**, and the newest it reads.
pub const REPLY_ENVELOPE_VERSION: u32 = 3;

/// Every envelope version this crate **reads**, newest first.
///
/// The bump to 3 is additive the way the plan schema's and the launch config's
/// are: version 3 adds one optional field — a `settle`'s `landing` — and takes
/// nothing away, so an envelope written against 2 is a complete envelope at 3 and
/// this build reads it as one. Entry 57 of `docs/contract-divergences.md` records
/// it, and `tests/golden/reply-envelope-v2.json` is an envelope at the older
/// version, kept beside the current one so what this build reads is checked in
/// rather than asserted.
///
/// A number outside this set is refused where an edit envelope's version has
/// always been checked, naming the version an edit requires.
pub const REPLY_ENVELOPE_VERSIONS_READ: &[u32] = &[REPLY_ENVELOPE_VERSION, 2];

/// Carry an envelope written against a version this build still reads forward to
/// the version it is read **at**.
///
/// The number an envelope declares is the version its author wrote it against;
/// what every reader downstream asks is whether this build reads that envelope,
/// and across the read set there is one answer. A number this build does not read
/// is left exactly as it was declared, so the caller meets the refusal that names
/// the version an edit envelope requires, made where it has always been made —
/// refusing here instead would turn it into a parse error, and would refuse a
/// legacy verdict-only envelope naming an old version and carrying no commands at
/// all, which is accepted today and stays accepted.
fn read_at_a_version_this_build_reads<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let declared = Option::<u32>::deserialize(deserializer)?;
    Ok(declared.map(|version| {
        if REPLY_ENVELOPE_VERSIONS_READ.contains(&version) {
            REPLY_ENVELOPE_VERSION
        } else {
            version
        }
    }))
}

/// Who wrote a reply, and therefore which ops it may carry.
///
/// A channel with two authors needs to say which one is speaking: the planner
/// owns the graph and the monitor only watches it, and the difference has to be
/// enforced rather than trusted. Omitted, an envelope is the planner's — every
/// reply written before this field existed was.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    /// The planner: it owns decomposition and review, and may issue every op.
    #[default]
    Planner,
    /// An observing monitor: it may correct and re-run work, and may not decide
    /// that the run is finished, that a person acted, or that a node goes away.
    Monitor,
}

impl Author {
    /// The word a record names this author with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planner => "planner",
            Self::Monitor => "monitor",
        }
    }

    /// Whether this is the default, so serialization can omit it.
    pub(crate) fn is_planner(&self) -> bool {
        matches!(self, Self::Planner)
    }
}

/// Whether one author may declare the run finished, or a refusal saying why not.
///
/// The legacy verdict says the same thing `complete` says, in a field rather
/// than in an op — so an allowlist that guarded only the ops would let a
/// commandless reply walk straight past it. Whether the run is finished is one
/// decision however it is spelled.
pub fn allows_completion(author: Author, completion: Option<bool>) -> crate::Result<()> {
    if author == Author::Planner || completion != Some(true) {
        return Ok(());
    }
    Err(crate::Error::Refused(
        "declaring the run complete is not something the monitor may do: whether the run \
         is finished is the planner's verdict, not an observation. Surface it to the \
         planner instead"
            .to_string(),
    ))
}

/// The ops one author may issue, or a refusal naming what it may not.
///
/// The allowlist is per author and it is exhaustive: an op that is not on it is
/// refused, so a new op is refused for the monitor until somebody decides
/// otherwise rather than being granted by omission.
pub fn allows(author: Author, command: &Command) -> crate::Result<()> {
    if author == Author::Planner {
        return Ok(());
    }
    let refused = match command {
        Command::Retry { .. }
        | Command::Requeue { .. }
        | Command::Cancel { .. }
        | Command::Finding { .. }
        | Command::Add { .. } => return Ok(()),
        Command::Complete { .. } => {
            "whether the run is finished is the planner's verdict, not an observation"
        }
        Command::Attest { .. } => {
            "a human action is attested by the person who took it, never by a watcher"
        }
        Command::Drop { .. } => {
            "removing work from the graph is a decomposition decision the planner owns"
        }
        Command::Reparent { .. } => {
            "rewiring dependencies is a decomposition decision the planner owns"
        }
        // The op the monitor most obviously *could* use, and the one it must
        // not: an observer that could move a node's bar would resolve an
        // ambiguity by editing rather than by escalating, which is the whole of
        // what its own persona reserves to the planner.
        Command::Amend { .. } => {
            "what a node is judged against is a decomposition decision the planner owns"
        }
        // A note may carry a criterion, and a delivered one enters the bar the
        // node's judge decides against — the same decision `amend` makes, taken
        // against the conversation running now, which the observer's own persona
        // reserves to the planner. It is the whole of the manager-note surface
        // since `context` was collapsed into it, so an observer that wants a node
        // told something surfaces it rather than sending it.
        Command::Note { .. } => {
            "a note may bind a criterion the node's judge decides against, which is the \
             planner's decision rather than an observation"
        }
        // The op that writes an outcome the run itself never observed. An
        // observer's whole authority is what the stream shows it, and this one
        // is deliberately the opposite: a person read a merge, or a wait that
        // can never clear, somewhere the run cannot see.
        Command::Settle { .. } => {
            "settling a node from evidence declares an outcome this run never observed, \
             which is the planner's decision rather than an observation"
        }
    };
    Err(crate::Error::Refused(format!(
        "'{}' is not an op the monitor may issue: {refused}. Surface it to the planner instead",
        op_of(command)
    )))
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    /// Who wrote it. Omitted, [`Author::Planner`].
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

    /// Whether this envelope carries edits and no verdict — the contract's
    /// **commands-only** envelope, the one shape with nothing in it for the
    /// reply path.
    ///
    /// This is the discrimination the two readers are routed by, and it is made
    /// from the shape the envelope already declares rather than from an address
    /// it would have had to remember to carry. It says nothing about the
    /// envelopes that carry both halves, which reach the command path too — it
    /// asks only whether the reply path is owed anything.
    pub(crate) fn carries_edits_without_a_verdict(&self) -> bool {
        !self.commands.is_empty() && !self.carries_verdict()
    }
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
/// with one subtraction and one replacement: the table's `context` is **gone**,
/// and [`Note`](Self::Note) is the single manager-note op that carries what both
/// of them did. An envelope still naming `context` is refused by serde, by that
/// name, because the field set below rejects what it does not declare.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
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
    },
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum SurfaceKind {
    /// The durable planner-update pacemaker came due. Consuming one resets that
    /// clock through `oneagentgraph reset-timer RUN check-in`.
    CheckIn,
    /// Something a watcher saw and decided the planner should know. Raised
    /// deliberately — by the [`Command::Finding`] op, or by `surface` — rather
    /// than as a side effect of a turn having happened.
    Finding,
}

impl SurfaceKind {
    /// The word a queued surface names this kind with.
    ///
    /// The wire spelling is this enum's rather than a string beside it, so the
    /// kind a queue holds and the kind a command line accepts cannot drift.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::CheckIn => "check-in",
            Self::Finding => "finding",
        }
    }
}

/// The environment variable bounding how long `reply` waits for the
/// reconciler's verdict before reporting the edits queued.
pub const REPLY_TIMEOUT_ENV: &str = "ONEPIPELINE_REPLY_TIMEOUT_SECONDS";

/// How long `reply` waits for the reconciler's verdict when nothing overrides
/// it.
pub const DEFAULT_REPLY_TIMEOUT_SECONDS: u64 = 30;

/// The environment variable bounding how long one `channel serve` session
/// serves before it stops of its own accord.
///
/// Unset — the default — is unbounded, which is what a member whose whole
/// conversation this channel carries wants: the session ends when that member's
/// frame stream does. A host that spawns the judge side **per turn** bounds it
/// instead, so the server does not outlive the turn it was spawned for while the
/// member that spawned it goes on working.
///
/// It is a deadline and not a hint: a member that has gone quiet does not hold a
/// session past the moment it said it would stop, and neither does one still
/// sending. What it never does is land mid-exchange — it is read before a frame
/// is, so a verdict a member is waiting on is never cut off half-written.
///
/// The endings are not the same fact and are deliberately not treated the same.
/// A stream that ended leaves what this session raised marked: nothing is
/// listening for those answers *now*, which is what the mark says and all it says
/// — an asker that rents its listeners takes them back through
/// `ChannelState::attend` the moment it arms another. A session that reached
/// this bound does not even say that much: the stream is still open, the member
/// is still there, and every question it raised is still owed an answer, so
/// nothing is marked at all.
pub const SERVE_SESSION_ENV: &str = "ONEPIPELINE_SERVE_SESSION_SECONDS";

/// The environment variable naming who a `channel serve` session listens on
/// behalf of.
///
/// A serving process is a listener a side rents, and never that side itself: an
/// asker may raise one question through one session and wait for the verdict
/// through a succession of them. Two sessions carrying the same value are one
/// asker, and the later takes back over what the earlier left outstanding — see
/// `ChannelState::attend`, which is where that is spelled out.
///
/// The value is **opaque and compared for equality only**. Every dispatch this
/// crate makes carries one of its own, composed in
/// `executor::prepare_dispatch_env`. A session carrying none listens on its own:
/// it adopts nothing and nothing adopts what it raised.
pub const ASKER_ENV: &str = "ONEPIPELINE_CHANNEL_ASKER";

/// One asker's name: the word by which two serving sessions are one side.
///
/// A type rather than a `String`, so that the two names which are not identities
/// are unrepresentable in everything that takes one — a **blank** one, which
/// every session carrying it would match, and one that is **not Unicode**, which
/// collapses onto every other such value when it is read. The refusals below say
/// what each would cost. The value is otherwise opaque: compared for equality,
/// never parsed, and written as the word itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct Asker(String);

impl Asker {
    /// The asker one environment value names, or a refusal saying why it names
    /// none.
    pub(crate) fn named(value: &std::ffi::OsStr) -> crate::Result<Self> {
        let value = value.to_str().ok_or_else(|| {
            crate::Error::Refused(format!(
                "{ASKER_ENV} is set to a value this host cannot read as text; an asker is \
                 compared to other askers as one word, and two values that are not text read \
                 as the same word — set it to a name in Unicode, or leave it unset for a \
                 session that listens on its own"
            ))
        })?;
        Self::checked(value)
    }

    /// The same check, over a name that is already text: the queue's own record
    /// comes back this way.
    fn checked(value: &str) -> crate::Result<Self> {
        if value.trim().is_empty() {
            return Err(crate::Error::Refused(format!(
                "{ASKER_ENV} is set to a blank value, which names no asker; leave it unset for \
                 a session that listens on its own, or set it to the one value every session \
                 of this asker carries"
            )));
        }
        Ok(Self(value.to_owned()))
    }
}

/// Read the asker a queue recorded, reading a name that names nobody as none.
///
/// The one lenient boundary in this file, and the leniency is the point. A
/// surface is read out of a whole projection or out of one line of the log, and
/// a refusal here would refuse the record around it rather than one field: the
/// **whole queue** read as empty, or the surface dropped from the fold — either
/// way a surface lost, which is a far worse answer to a name this crate never
/// writes than simply not knowing whose it was. A blank name identifies nobody,
/// and `None` is what this file already means by that, so it is read as that and
/// the invariant `Asker` carries survives the round trip.
fn recorded_asker<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Asker>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.and_then(|name| Asker::checked(&name).ok()))
}

/// What raised a surface.
///
/// A pacemaker update and a worker's proposal are the same wire shape and
/// different facts, so a journal reader can tell "nothing was sent" from
/// "updates were sent and nobody read them".
pub(crate) mod source {
    /// The durable pacemaker came due.
    pub const CHECK_IN: &str = "check-in";
    /// A settled worker or the orchestrator raised advice.
    pub const PROPOSAL: &str = "proposal";
    /// The reconciler answered an edit it could not apply.
    pub const RECONCILER: &str = "reconciler";
    /// An observing monitor applied an edit of its own.
    pub const MONITOR: &str = "monitor";
}

/// One surface, as it sits in the durable queue.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Surface {
    /// Monotonic within the run, so a consumer can report which one it read.
    pub id: u64,
    /// What the surface is asking about.
    pub kind: String,
    /// Its text.
    pub message: String,
    /// What raised it — see [`source`].
    pub source: String,
    /// Whether the run is waiting on the answer. A **blocking** surface is a
    /// decision point and holds the subtree that depends on
    /// [`workstream`](Self::workstream); a non-blocking one holds nothing.
    pub blocking: bool,
    /// When it was queued, in epoch milliseconds.
    pub queued_at: u64,
    /// The node that provoked it, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workstream: Option<String>,
    /// Whether anybody is listening for the answer.
    ///
    /// Set by [`abandon`](ChannelState::abandon) when the process serving this
    /// surface exited without an answer, and lifted by
    /// [`attend`](ChannelState::attend) when a later listener of the same asker
    /// takes it back over — a listener ending is not the asker going. While it
    /// stands, the surface keeps its text and its place: what it gives up is its
    /// claim on the unread count, on the subtree a blocking surface holds, and on
    /// being reported as a question the run awaits a verdict on. Omitted from the
    /// wire while it is false, so a queue nothing has abandoned serializes
    /// exactly as it always did.
    #[serde(default, skip_serializing_if = "is_false")]
    pub abandoned: bool,
    /// Who raised it, when the session that did named an asker.
    ///
    /// The key [`attend`](ChannelState::attend) matches on: a later session of
    /// the same asker takes this surface back over, and a session of any other
    /// asker leaves it exactly where it is. `None` is a surface nobody named an
    /// asker for — every one an older build wrote, and every one raised outside a
    /// serving session — and nothing ever adopts one of those. Omitted from the
    /// wire while it is absent, so a queue no asker was named on serializes
    /// exactly as it always did.
    #[serde(
        default,
        deserialize_with = "recorded_asker",
        skip_serializing_if = "Option::is_none"
    )]
    pub asker: Option<Asker>,
}

/// The durable channel state for one run.
///
/// Transport state lives beside the journal rather than in memory, so both
/// sides may exit and reattach between messages: **acceptance means delivery**.
/// Nothing has to be listening at the moment the planner writes.
#[derive(Debug, Clone)]
pub(crate) struct ChannelState {
    paths: crate::ledger::RunPaths,
}

/// What is waiting to be read, and what has been read but not answered.
///
/// A **projection** of the surface log, and never the truth about it. Every
/// state a surface reaches is an append to `surfaces.jsonl` — see
/// [`SurfaceEvent`] — and this document is that log folded: what
/// [`ChannelState::queue`] hands back is this checkpoint brought up to date with
/// every record the log has grown by since it was written, and what every
/// mutation writes is the fold as it stands the moment its own record is
/// appended. It earns its place by being cheap where the log is not: the unread
/// count is one read of it, paid per run root by the listing across every root on
/// the host, and its modification stamp is what lets the reconcile loop skip an
/// unchanged channel without opening the log.
///
/// It used to be the truth, read, modified, and written back whole by every
/// writer and every reader, and that lost data: a push landing between a
/// reader's read and its write-back was overwritten by the reader's stale copy,
/// and a worker's blocking question was destroyed by the manager's own act of
/// reading the channel. As a projection a lost write costs the next reader a
/// fold and never a surface.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Queue {
    /// The surfaces nobody has read yet, oldest first.
    #[serde(default)]
    pub waiting: Vec<Surface>,
    /// The surface a planner consumed and has not answered.
    #[serde(default)]
    pub pending: Option<Surface>,
    /// The id the next surface takes: one past the highest the log has
    /// allocated. Derived, never read and incremented — the allocation is made
    /// under the log's lock from the log itself, so two writers cannot take one
    /// id and a stale copy cannot hand out one already taken.
    #[serde(default)]
    pub next_id: u64,
    /// How many bytes of the surface log this projection accounts for, at a
    /// record boundary.
    ///
    /// What makes the projection *checkable*: a log longer than this holds
    /// records the document has not folded, and a reader folds them before
    /// answering. Absent from a document an older build wrote, which kept no
    /// stamp and logged no claim or answer to replay — such a document is taken
    /// as it stands, exactly as that build took it, and is stamped the first time
    /// this build writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounted: Option<u64>,
    /// The seal over everything above: what makes a stamped projection
    /// **checkable in itself** rather than taken on trust because its stamp
    /// happens to match the log's length.
    ///
    /// A document whose `waiting` was emptied or whose `next_id` was reset with
    /// its stamp intact is one nothing here wrote, and without this the read that
    /// trusts the stamp would hide a logged question for good and hand its id out
    /// again. So every writer seals what it writes and every reader recomputes
    /// the seal from the document alone — one read, no look at the log — and a
    /// stamped document that does not seal is read as no document at all, which
    /// folds the whole log. The same integrity check and the same boundary as the
    /// checkpoint's: what it detects is the accidents, and a rewrite crafted to
    /// match is not one anybody here meets. Absent from an older build's document,
    /// which carries no stamp to vouch for either.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "as_hex_opt",
        deserialize_with = "of_hex_opt"
    )]
    pub seal: Option<u128>,
}

fn as_hex_opt<S: serde::Serializer>(digest: &Option<u128>, writer: S) -> Result<S::Ok, S::Error> {
    match digest {
        Some(digest) => crate::checkpoint::as_hex(digest, writer),
        None => writer.serialize_none(),
    }
}

/// Read a seal, refusing the values [`as_hex_opt`] never wrote — as the
/// checkpoint refuses them — so a document carrying one reads as no document.
fn of_hex_opt<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<Option<u128>, D::Error> {
    #[derive(serde::Deserialize)]
    struct Hex(#[serde(deserialize_with = "crate::checkpoint::of_hex")] u128);
    Ok(Option::<Hex>::deserialize(reader)?.map(|Hex(digest)| digest))
}

/// What one line of the surface log says happened.
///
/// The log is a complete event stream: a surface is written to it when it is
/// queued and again at every state it reaches afterwards, each line carrying the
/// surface as it was at that moment under its own id. Appends are atomic and
/// serialised under the log's lock, so the log alone accounts for a surface's
/// whole life and the projection beside it can always be rebuilt from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SurfaceEvent {
    Queued,
    /// A blocking one goes to the pending slot here; narration goes nowhere.
    Claimed,
    /// Releases the pending slot.
    Answered,
    Abandoned,
    Attended,
}

/// One line of the surface log.
///
/// The event is read as optional because every line an older build wrote lacks
/// it: that build logged a surface when it was queued, when it was abandoned, and
/// when it was attended, and never a claim or an answer. Such a line is read as
/// what that build meant by it — see [`SurfaceRecord::event`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SurfaceRecord {
    /// What happened. Always written; read leniently for the reason above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<SurfaceEvent>,
    /// The surface as it was when this happened.
    #[serde(flatten)]
    pub surface: Surface,
}

impl SurfaceRecord {
    /// What this line says happened, inferring it for a line that does not say.
    ///
    /// An older build wrote the surface itself at three moments and marked none
    /// of them: the first line under an id was the surface being queued, and every
    /// later line under it was the surface being abandoned or taken back, which
    /// the flag it carries tells apart. `next_id` is what decides whether the id
    /// has been seen: the fold keeps it one past every id queued so far.
    // llmlint: ignore-block[changed_behavior_has_e2e] the lines this reads are
    // ones only an older build writes — this build writes the event on every line
    // — so no journey driving this binary can produce one, and placing one by hand
    // is what `a_log_and_a_projection_an_older_build_wrote_are_read_as_that_build_
    // meant_them` does, against the real files.
    fn event(&self, next_id: u64) -> SurfaceEvent {
        self.event.unwrap_or(if self.surface.id >= next_id {
            SurfaceEvent::Queued
        } else if self.surface.abandoned {
            SurfaceEvent::Abandoned
        } else {
            SurfaceEvent::Attended
        })
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
}

impl Queue {
    /// The seal over this projection's claims: the surfaces it holds, the id it
    /// would allocate, and the bytes of the log it accounts for.
    ///
    /// Through the document's own serialization rather than field by field, so a
    /// field added to a surface is sealed by existing rather than by somebody
    /// remembering to add it here. [`seal`](Self::seal) itself does not go in,
    /// which is what stops it sealing over itself; a document without a stamp
    /// seals to nothing, because there is no claim in it to vouch for.
    fn sealed(&self) -> Option<u128> {
        let accounted = self.accounted?;
        let claims =
            serde_json::to_vec(&(&self.waiting, &self.pending, self.next_id)).unwrap_or_default();
        let sealed = crate::checkpoint::digested(crate::checkpoint::NOTHING_DIGESTED, &claims);
        Some(crate::checkpoint::digested(
            sealed,
            &accounted.to_le_bytes(),
        ))
    }

    /// Seal this projection so a reader can check it.
    fn seal(&mut self) {
        self.seal = self.sealed();
    }

    /// Whether a stamped projection is one a writer here sealed and nothing has
    /// moved since. An unstamped one is an older build's, and is vouched for by
    /// nothing — see [`ChannelState::current`].
    fn is_sealed(&self) -> bool {
        self.accounted.is_none() || self.seal.is_some() && self.seal == self.sealed()
    }

    /// Fold one record of the surface log into this projection.
    ///
    /// The one place the queue's transitions are defined: every mutation applies
    /// its own record through here after appending it, and every reader applies
    /// the records the log grew by, so the two cannot disagree about what a line
    /// means. A record about a surface this projection no longer holds — a claim
    /// of one already claimed, an answer to one already answered — folds to
    /// nothing, which is what makes a replay from any checkpoint land on the same
    /// state.
    fn apply(&mut self, record: &SurfaceRecord) {
        let surface = &record.surface;
        match record.event(self.next_id) {
            SurfaceEvent::Queued => {
                // An id with no successor is one no writer here allocated, and
                // it is refused rather than folded. The counter is one past the
                // highest id queued, and there is no one past this: a fold that
                // kept the record and left the counter at the id itself would
                // hand that id to every surface queued afterwards, for good —
                // the collision this whole design exists to rule out. Refused,
                // the record costs one line nobody wrote honestly, and the
                // counter stays where the last usable id put it.
                let Some(after) = surface.id.checked_add(1) else {
                    return;
                };
                // Exactly one check-in is ever pending, and it is kept current
                // rather than kept still: the next interval's update replaces
                // the queued one instead of being blocked by it.
                if surface.source == source::CHECK_IN {
                    self.waiting
                        .retain(|existing| existing.source != source::CHECK_IN);
                }
                self.waiting.push(surface.clone());
                self.next_id = self.next_id.max(after);
            }
            SurfaceEvent::Claimed => {
                let Some(at) = self.waiting.iter().position(|w| w.id == surface.id) else {
                    return;
                };
                let taken = self.waiting.remove(at);
                // A blocking surface outlives its delivery while it waits for an
                // answer, so it is held rather than dropped. An abandoned one
                // takes the slot only when nothing else is holding one: nothing
                // waits on its answer, so it may not displace a question somebody
                // does wait on. The slot's own text is not written over on its way
                // out: an abandoned occupant goes back among the readable ones,
                // because this queue is the only place a reader can still reach
                // it.
                if taken.blocking && (!taken.abandoned || self.pending.is_none()) {
                    if let Some(displaced) = self.pending.replace(taken) {
                        if displaced.abandoned {
                            self.waiting.push(displaced);
                        }
                    }
                }
            }
            SurfaceEvent::Answered => {
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|held| held.id == surface.id)
                {
                    self.pending = None;
                }
            }
            SurfaceEvent::Abandoned | SurfaceEvent::Attended => {
                let abandoned = record.event(self.next_id) == SurfaceEvent::Abandoned;
                for held in self.waiting.iter_mut().chain(self.pending.iter_mut()) {
                    if held.id == surface.id {
                        held.abandoned = abandoned;
                    }
                }
            }
        }
    }
}

/// One reply as it sits in the durable queue.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct QueuedReply {
    /// Monotonic within the run.
    pub id: u64,
    /// The envelope the planner wrote.
    pub reply: Reply,
    /// When it was written, in epoch milliseconds.
    pub at: u64,
}

/// One submitted edit envelope, awaiting the reconciler.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct QueuedCommands {
    /// Monotonic within the run.
    pub id: u64,
    /// Who submitted it, which is what decides the ops it may carry.
    #[serde(default)]
    pub author: Author,
    /// The commands, reconciled in order.
    pub commands: Vec<Command>,
}

/// What the reconcile loop last saw of the channel's files.
///
/// Compared rather than read: see [`ChannelState::fingerprint`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    queue: Option<(u64, std::time::SystemTime)>,
    surfaces: Option<(u64, std::time::SystemTime)>,
    commands: Option<(u64, std::time::SystemTime)>,
}

/// One file's length and modification time, or `None` where there is no file.
///
/// A modification time the platform declines to report reads as the epoch, so a
/// host with no such clock falls back to comparing lengths — which is the whole
/// answer for the append-only half and is never *worse* than not looking.
fn mark(path: &std::path::Path) -> Option<(u64, std::time::SystemTime)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((
        metadata.len(),
        metadata
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
    ))
}

/// The reconciler's answer to one submitted envelope.
///
/// An envelope is all-or-nothing, so [`applied`](Self::applied) is still the
/// whole envelope's answer and every reader that predates
/// [`results`](Self::results) keeps reading exactly what it read. What that
/// boolean could never say is *which* command decided it, which is what left a
/// manager believing a node's bar had changed when the command that would have
/// changed it was never compiled.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CommandOutcome {
    /// The envelope this answers.
    pub id: u64,
    /// Whether every command in it was applied.
    pub applied: bool,
    /// Why not, when it was not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// One entry per command the envelope carried, in the order it carried them.
    ///
    /// Omitted when empty, so a record this build writes for an envelope with no
    /// commands is byte-for-byte the record an older build wrote.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<CommandResult>,
}

/// What became of **one** command of an envelope.
///
/// Every command is evaluated, whatever the ones before it said, so each entry is
/// that command's **own** answer. The envelope is still atomic — a refusal
/// anywhere in it applies none of it — and the four words below are what tells
/// apart the facts a single boolean could not: which commands were wrong, which
/// were fine and went down with them, and which of those had already been read by
/// a conversation that cannot unread it. A manager reading them knows which to
/// fix, which to resend unchanged, and which not to resend at all.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CommandResult {
    /// Where in the envelope's `commands` this one sat, from zero.
    pub index: usize,
    /// The command's op, as the envelope spelled it.
    ///
    /// Carried so an entry names the command it belongs to rather than leaving a
    /// reader to count positions in the envelope it sent.
    pub op: String,
    /// What became of it.
    pub outcome: CommandVerdict,
    /// Why it refused, or what refused around it, when either happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The four things that can become of one command of an envelope.
///
/// One field rather than a boolean and a sentence, because "not applied" was two
/// facts wearing one word: a command that was wrong and a command that was fine.
/// A reader that cannot tell them apart resends the wrong one and fixes the right
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CommandVerdict {
    /// It was validated and committed.
    Applied,
    /// It was validated and nothing was wrong with it, and **nothing of it
    /// happened**: no conversation was offered anything on its behalf, no graph
    /// moved, and no record was written for it. Something else in the envelope
    /// refused, and an envelope applies all of its commands or none. Resending it
    /// on its own is what gets it in, and resending it costs nothing, because it
    /// had no effect to repeat.
    Validated,
    /// A `note` whose conversation **took it**, in an envelope refused after that.
    ///
    /// Nothing of this command was committed — the same nothing
    /// [`Validated`](Self::Validated) reports — but a conversation has no undo, so
    /// resending the envelope hands that party the note a second time.
    /// `engine::deliver_envelope` states the one window this is reachable through.
    // llmlint: ignore[changed_behavior_has_e2e] no journey can arrange that
    // window: it takes a live conversation accepting a note and, in the same run
    // and envelope, a second node whose member has settled — and the harness
    // double holds every turn of a run on one shared gate. Its two sides are
    // driven end to end in `tests/note/main.rs` and the word itself by
    // `engine::tests::a_refused_envelope_answers_a_delivered_note_differently_from_an_untouched_command`.
    Delivered,
    /// It refused, and [`reason`](CommandResult::reason) is what it said.
    Refused,
}

impl ChannelState {
    /// The channel for one run.
    pub fn new(paths: &crate::ledger::RunPaths) -> Self {
        Self {
            paths: paths.clone(),
        }
    }

    fn queue_path(&self) -> std::path::PathBuf {
        self.paths.channel("queue.json")
    }

    fn log_path(&self) -> std::path::PathBuf {
        self.paths.channel("surfaces.jsonl")
    }

    /// A cheap look at everything the reconcile loop reads off this channel.
    ///
    /// Three `stat` calls and no read, so a converged driver can check for an
    /// arriving edit five times a second for nothing: the loop reconciles only
    /// when this moved. An absent file fingerprints as absent, so the moment one
    /// appears the fingerprint has changed.
    ///
    /// Length is the load-bearing half — the two logs only grow, and every queue
    /// transition the loop can read changes the projection's length too — and
    /// the timestamp is the belt beside those braces. The surface log is marked
    /// beside its projection so that a writer which appended and then died
    /// before it wrote the projection still wakes the loop, whose read of the
    /// queue is what folds that record in.
    // llmlint: ignore-block[changed_behavior_has_e2e] the state that half
    // exists for is a writer dying between its append and its write of the
    // projection, which no journey can arrange on purpose — every push the
    // binary makes writes both — so `a_record_appended_without_its_projection_
    // still_moves_the_fingerprint` places that append against the real files
    // and holds that the fingerprint moves and the read folds it.
    pub(crate) fn fingerprint(&self) -> Fingerprint {
        Fingerprint {
            queue: mark(&self.queue_path()),
            surfaces: mark(&self.log_path()),
            commands: mark(&self.paths.channel("commands.jsonl")),
        }
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]

    /// The live queue: the projection, brought up to date with every record the
    /// surface log has grown by since the projection was written.
    ///
    /// One read of the projection and one `stat` of the log when the two agree,
    /// which is every read of a channel nothing has written since. Where they do
    /// not — a writer died between its append and its write of the projection,
    /// or a stale projection landed over a fresh one — the records the projection
    /// has not accounted for are folded in, and the repaired projection is
    /// written back so the next reader pays one read again. The repair is best
    /// effort: a projection that could not be written costs the next reader a
    /// fold, never an answer, and a read that refused over it would be a view
    /// unable to render a run whose own record is intact.
    pub fn queue(&self) -> Queue {
        // The file reader hands back nothing for a log it cannot read — the
        // leniency every ledger reader follows — so the fold here cannot refuse;
        // what it folded nothing of is left unstamped for the next reader.
        let Ok((queue, folded)) = self.current(|from| {
            Ok::<_, std::convert::Infallible>(crate::ledger::read_records_from(
                &self.log_path(),
                from,
            ))
        });
        if folded {
            // llmlint: ignore-block[no_panics_on_recoverable_errors] the repair is a cache write and the answer is already in hand: failing the read over it would refuse a view of a run whose log is intact, and the next reader simply folds again — `tests/e2e/channel.rs` drives that against a channel directory the reader may not write.
            let _ = self.write_queue(&queue);
            // llmlint: ignore-end[no_panics_on_recoverable_errors]
        }
        queue
    }

    /// The projection with the log's tail folded in, and whether anything was.
    ///
    /// `tail` reads the log from a record boundary; a caller holding the log's
    /// lock reads through the handle it holds, and one that does not reads the
    /// file. It is not called at all when the log is exactly as long as the
    /// stamp, which is every read of a channel nothing has written since: that
    /// read is the projection and one `stat`. A log shorter than the stamp is
    /// one that was replaced, and is folded whole from an empty projection
    /// rather than from a boundary it no longer has. A record whose writer has
    /// not finished it ends the fold: the stamp stays at the boundary before it,
    /// so a later read resumes there and the record is folded whole once its
    /// writer is done.
    ///
    /// **A projection with no stamp is an older build's**, and is brought over
    /// once. That build logged a surface when it queued it and never when it
    /// claimed or answered it, so the log cannot say which of its surfaces were
    /// read; what it can say is which were **lost**. The old build allocated an
    /// id as the counter it then wrote back, so a logged id at or past the
    /// counter the projection holds is a surface whose write-back was overwritten
    /// — the exact shape of the loss this design replaces, a queue reading
    /// `next_id: 0` beside a log carrying id 0. Those are folded in from the log,
    /// with everything the log went on to say about them; every id below the
    /// counter was accounted for by that build by its own means and is taken as
    /// the projection has it. The result is stamped, so the log is read this way
    /// once and never again.
    fn current<E>(
        &self,
        tail: impl FnOnce(u64) -> Result<Vec<crate::ledger::Record>, E>,
    ) -> Result<(Queue, bool), E> {
        // A stamped document that does not seal is one nothing here wrote: read as
        // no document, so the whole log is folded rather than its claims trusted.
        let checkpoint: Option<Queue> =
            crate::ledger::read_json_opt(&self.queue_path()).filter(Queue::is_sealed);
        // The id below which an older build's projection is taken at its word.
        let mut floor: Option<u64> = None;
        let (mut queue, from) = match checkpoint {
            Some(queue) => match queue.accounted {
                Some(accounted) => (queue, accounted),
                None => {
                    floor = Some(queue.next_id);
                    (queue, 0)
                }
            },
            None => (Queue::default(), 0),
        };
        let length = mark(&self.log_path()).map_or(0, |(length, _)| length);
        // llmlint: ignore-block[changed_behavior_has_e2e] no user-facing route
        // replaces or truncates a run's surface log — every append heals back to
        // a boundary at or past every stamp ever written — so the journey that
        // would drive this has no way to arrange it; `the_queue_is_rebuilt_from_
        // the_log_alone_whatever_became_of_the_projection` holds it against the
        // real files.
        let replaced = length < from;
        let from = if replaced {
            queue = Queue::default();
            0
        } else {
            from
        };
        // llmlint: ignore-end[changed_behavior_has_e2e]
        let mut accounted = from;
        // An older build's projection is rewritten stamped even when the log
        // restores nothing to it, so it is read this way once rather than on
        // every read.
        let mut folded = replaced || floor.is_some();
        let records = if length > from {
            tail(from)?
        } else {
            Vec::new()
        };
        for record in records {
            // llmlint: ignore-block[changed_behavior_has_e2e] a record whose
            // writer has not finished it takes a writer dying mid-append, which
            // is the tear `tests/e2e/journal.rs` drives for the same primitive
            // against the store where it actually happens; this fold's half of
            // it — the stamp staying before the fragment until the record is
            // whole — is held by `the_queue_is_rebuilt_from_the_log_alone_
            // whatever_became_of_the_projection` against the real file.
            if !record.terminated {
                break;
            }
            // llmlint: ignore-end[changed_behavior_has_e2e]
            accounted = record.offset + record.bytes + 1;
            folded = true;
            // A line this build cannot read is still a line the file holds, so
            // it advances the stamp: leaving it would fold the same tail on every
            // read for ever. What it costs is one record going unfolded, which
            // is one surface a reader cannot see — and every line here is one
            // this crate wrote.
            //
            // llmlint: ignore-block[changed_behavior_has_e2e] nothing this crate
            // does writes a line here it cannot read back, so no journey can
            // place one without editing the run's own record; the same unit test
            // places one and holds that the fold goes on past it.
            if let Ok(record) = serde_json::from_str::<SurfaceRecord>(&record.text) {
                if floor.is_some_and(|floor| record.surface.id < floor) {
                    continue;
                }
                queue.apply(&record);
            }
            // llmlint: ignore-end[changed_behavior_has_e2e]
        }
        queue.accounted = Some(accounted);
        queue.seal();
        Ok((queue, folded))
    }

    fn write_queue(&self, queue: &Queue) -> crate::Result<()> {
        crate::ledger::write_json(&self.queue_path(), queue)
    }

    /// Append what `derive` decides to the surface log, under its lock, and
    /// write the projection as it then stands.
    ///
    /// Every mutation of the queue is this. The lock is held from before the
    /// queue is read until after the projection is written, so what `derive`
    /// sees is the log as it is and nothing lands between the decision and the
    /// record of it: an id it allocates is one no other writer can allocate, and
    /// a surface it claims is one no other reader can claim. A refusal `derive`
    /// hands back records nothing and is handed on. What it returns is the
    /// surfaces it recorded, in the order it recorded them.
    fn record(
        &self,
        derive: impl FnOnce(&Queue) -> crate::Result<Vec<(SurfaceEvent, Surface)>>,
    ) -> crate::Result<Vec<Surface>> {
        let mut log = crate::ledger::Appender::open(&self.log_path())?;
        // A tail this handle cannot read refuses the mutation: this path stamps
        // the log's whole length afterwards, and a fold that read nothing of the
        // tail would stamp records the projection never folded as accounted for
        // — a question hidden for good, by the very write meant to record one.
        let (mut queue, _) = self.current(|from| log.records_from(from))?;
        let mut recorded = Vec::new();
        for (event, surface) in derive(&queue)? {
            let record = SurfaceRecord {
                event: Some(event),
                surface,
            };
            log.append(
                &serde_json::to_string(&record)
                    .map_err(|e| crate::Error::Invalid(format!("surface: {e}")))?,
            )?;
            queue.apply(&record);
            recorded.push(record.surface);
        }
        queue.accounted = Some(log.len()?);
        queue.seal();
        self.write_queue(&queue)?;
        Ok(recorded)
    }

    /// Queue one surface, and record that it was *sent*.
    ///
    /// The id is allocated from the log under its lock — one past the highest it
    /// has ever queued — so a surface queued while a reader is reading the
    /// channel keeps its id and its place. The one id that cannot be allocated
    /// is the last one there is: no id could follow it, so a surface queued
    /// under it would be the id every later surface was allocated too. The push
    /// is refused instead, and records nothing. Exactly one check-in is ever pending,
    /// and it is kept current rather than kept still: the next interval's update
    /// **replaces** the queued one instead of being blocked by it, so being
    /// ignored makes the harness louder rather than quieter. The clock is not
    /// reset by queuing, so the staleness a view reports keeps growing while the
    /// queued content stays fresh.
    pub fn push(&self, mut surface: Surface) -> crate::Result<Surface> {
        let mut queued = self.record(|queue| {
            if queue.next_id.checked_add(1).is_none() {
                return Err(crate::Error::Refused(format!(
                    "surface: the channel has no id left to allocate; the last one, \
                     {}, has already been queued",
                    queue.next_id - 1
                )));
            }
            surface.id = queue.next_id;
            Ok(vec![(SurfaceEvent::Queued, surface)])
        })?;
        queued
            .pop()
            .ok_or_else(|| crate::Error::Invalid("surface: nothing was queued".to_owned()))
    }

    /// Claim the next readable surface: **a blocking one first**, and arrival
    /// order within each class.
    ///
    /// Strict arrival order is the wrong order here, and only for one reason. A
    /// blocking surface holds back the subtree that depends on it and produces
    /// no other signal until somebody reads it; nothing else in the queue does
    /// either of those things. So a question queued behind narration is a
    /// stopped frontier waiting on a reader who is working through a backlog,
    /// while the narration it is behind loses nothing by being read second.
    ///
    /// Nothing to outlive: a surface describes the one continuous run, so it
    /// stays consumable until somebody reads it. A check-in that has been
    /// superseded is replaced at [`push`](Self::push) rather than discarded
    /// here.
    ///
    /// A blocking surface outlives its delivery while it waits for an answer,
    /// so it is held in the pending slot rather than dropped: the run is
    /// reported as waiting for a planner decision until a reply arrives.
    /// Narration read afterwards leaves that standing — reading a report is not
    /// answering a question. An abandoned one is handed over too — the text is
    /// what a manager reads it for — and last, behind everything somebody is
    /// still waiting on. What the run *reports* is not decided here but by
    /// [`pending`](Self::pending), which passes over an abandoned occupant; what
    /// the slot does with each is [`Queue::apply`]'s.
    pub fn claim(&self) -> crate::Result<Option<Surface>> {
        let mut claimed = self.record(|queue| {
            let next = queue
                .waiting
                .iter()
                .position(|surface| surface.blocking && !surface.abandoned)
                .or_else(|| queue.waiting.iter().position(|surface| !surface.abandoned))
                .unwrap_or(0);
            Ok(queue
                .waiting
                .get(next)
                .map(|surface| vec![(SurfaceEvent::Claimed, surface.clone())])
                .unwrap_or_default())
        })?;
        Ok(claimed.pop())
    }

    /// Say of every surface in `raised` that nobody is waiting for its answer.
    ///
    /// Called when a serving process is about to exit: no answer to anything it
    /// raised has a reader left *in this session*. Answering those surfaces is
    /// not the same question as whether they are still *interesting*, which is
    /// why this marks rather than deletes.
    ///
    /// **Marked, not discarded**, and the queue is what decides it. A surface
    /// still in `waiting` is one no manager has ever seen, and this queue holds
    /// the only copy of its text any reader can reach — discarding it would
    /// throw away an observer's finding in order to fix a count, which is a
    /// worse bargain than the count. So the text stays exactly where a reader
    /// already looks for it and [`claim`](Self::claim) still hands it out; the
    /// flag is what the unread accounting and the decision points read.
    ///
    /// **Nothing moves**, the pending slot included. A surface in that slot has
    /// been delivered to a manager and is the one a verdict binds to, so taking
    /// it out and putting it back among the readable ones both delivers it twice
    /// and leaves the run with no question for a verdict to name — which is how
    /// a question whose listener merely re-armed was lost. It stays in the slot
    /// and is marked there; [`pending`](Self::pending) passes over it, so the
    /// run stops reporting that it awaits a planner nobody is waiting on, and
    /// [`attend`](Self::attend) is what can take it back.
    ///
    /// Returns what it marked, so the caller can record it. The run's own
    /// record of what became of each is one further line of the surface log
    /// under the same id, carrying the same text and saying nobody is waiting on
    /// it.
    pub fn abandon(&self, raised: &[u64]) -> crate::Result<Vec<Surface>> {
        self.record(|queue| {
            Ok(queue
                .waiting
                .iter()
                .chain(queue.pending.iter())
                .filter(|surface| raised.contains(&surface.id) && !surface.abandoned)
                .map(|surface| {
                    (
                        SurfaceEvent::Abandoned,
                        Surface {
                            abandoned: true,
                            ..surface.clone()
                        },
                    )
                })
                .collect())
        })
    }

    /// Take back over everything `asker` left outstanding, and say what was
    /// taken.
    ///
    /// The other half of [`abandon`](Self::abandon), and the half that makes its
    /// verdict revisable rather than final. A listener exiting proves that
    /// listener is done; only the *asker* going proves nobody is waiting, and a
    /// wrapper that re-arms produces the first without the second, over and over,
    /// against one question that stays open the whole time. So a session says who
    /// it listens for before it reads a frame, and what an earlier session of the
    /// same asker gave up is simply given back: the mark comes off, the surface
    /// counts again, and a question still in the pending slot is a question a
    /// verdict can name again.
    ///
    /// Scoped to the asker, and that is the whole of what keeps it honest. A
    /// session of some *other* asker is not a reader for this one's questions,
    /// and adopting run-wide would resurrect a dead member's question for as long
    /// as any unrelated session happened to be serving — which is the defect
    /// `abandon` exists to stop, returned by another door. A surface naming no
    /// asker is adopted by nobody.
    ///
    /// Nothing moves and nothing is re-queued: this only clears a flag, so a
    /// surface a manager has already been handed is not handed to them twice by
    /// its asker coming back. The correction is recorded under the same id and
    /// beside the line that said nobody was waiting on it, so the run's own
    /// record carries it rather than ending on a statement that stopped being
    /// true.
    pub fn attend(&self, asker: &Asker) -> crate::Result<Vec<Surface>> {
        self.record(|queue| {
            Ok(queue
                .waiting
                .iter()
                .chain(queue.pending.iter())
                .filter(|surface| surface.abandoned && surface.asker.as_ref() == Some(asker))
                .map(|surface| {
                    (
                        SurfaceEvent::Attended,
                        Surface {
                            abandoned: false,
                            ..surface.clone()
                        },
                    )
                })
                .collect())
        })
    }

    /// The surface waiting for an answer, if one is.
    ///
    /// The slot can hold a surface nobody is waiting on — an abandoned one stays
    /// where it was delivered, so the listener that comes back for it finds it
    /// there — and that is not a surface waiting for an answer. Every reader
    /// asking whether this run owes a verdict asks here; [`held`](Self::held) is
    /// for the one reader that asks what is in the slot whatever became of it.
    pub fn pending(&self) -> Option<Surface> {
        self.held().filter(|surface| !surface.abandoned)
    }

    /// Whatever the pending slot holds, abandoned or not.
    pub fn held(&self) -> Option<Surface> {
        self.queue().pending
    }

    /// Record that a reply answered whatever was pending.
    ///
    /// This is the **verdict** path: what it queues is what a reader waiting on
    /// a ruling takes, and clearing `pending` is what releases the subtree a
    /// blocking surface held. An envelope with edits reaches it through
    /// [`answer_if_verdict`](Self::answer_if_verdict), which is where the two
    /// halves are routed apart.
    ///
    /// The release is one further line of the surface log under the answered
    /// surface's id, and the reply itself is appended to the replies after it:
    /// a reader that finds the reply finds the slot already released.
    pub fn answer(&self, reply: &Reply) -> crate::Result<u64> {
        self.record(|queue| {
            Ok(queue
                .pending
                .iter()
                .map(|held| (SurfaceEvent::Answered, held.clone()))
                .collect())
        })?;
        let path = self.paths.channel("replies.jsonl");
        let id = crate::ledger::read_lines(&path).len() as u64;
        let queued = QueuedReply {
            id,
            reply: reply.clone(),
            at: crate::sys::now_millis(),
        };
        crate::ledger::append_line(
            &path,
            &serde_json::to_string(&queued)
                .map_err(|e| crate::Error::Invalid(format!("reply: {e}")))?,
        )?;
        Ok(id)
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
        if reply.carries_verdict() {
            self.answer(reply)?;
        }
        Ok(())
    }

    /// Every reply the planner has written, in order.
    pub fn replies(&self) -> Vec<QueuedReply> {
        crate::ledger::read_lines(&self.paths.channel("replies.jsonl"))
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Claim the next verdict no reader has taken yet.
    ///
    /// A reply is claimed from the durable queue by whichever reader reaches it
    /// next, each claim advancing the cursor, so one reply reaches exactly one
    /// reader and no reader can lose it. **Which** readers reach it is decided
    /// before arrival order gets a say: this queue is the verdict side of the
    /// channel, so a commands-only envelope is not on it and is not handed out
    /// here. [`answer_if_verdict`](Self::answer_if_verdict) keeps it off, and it
    /// is skipped here as well for the run whose queue an older build already
    /// wrote one into — that envelope's reader is the command queue, which holds
    /// its own copy behind its own cursor, so passing over this one takes
    /// nothing from anybody.
    ///
    /// **One claim, one reply**, oldest first: two verdicts written inside one
    /// reader's poll are two rulings about two questions, and handing over the
    /// batch would deliver the newer and lose the older. The cursor lands just
    /// past the reply this claim took — so a skipped envelope is passed over
    /// behind a delivery rather than on its own account, and a poll that takes
    /// nothing leaves the cursor exactly where it was.
    ///
    /// Among the readers that do reach it, none is addressed: entry 63 of
    /// `docs/contract-divergences.md` is what that costs and what it is waiting
    /// on.
    pub fn claim_reply(&self) -> crate::Result<Option<QueuedReply>> {
        let cursor_path = self.paths.channel("replies-cursor.json");
        let claimed_through: u64 = crate::ledger::read_json_opt(&cursor_path).unwrap_or(0);
        let claimed = self.replies().into_iter().find(|queued| {
            queued.id >= claimed_through && !queued.reply.carries_edits_without_a_verdict()
        });
        if let Some(claimed) = &claimed {
            crate::ledger::write_json(&cursor_path, &(claimed.id + 1))?;
        }
        Ok(claimed)
    }

    /// Append one envelope of edits to the durable command queue.
    pub fn submit(&self, author: Author, commands: &[Command]) -> crate::Result<u64> {
        let path = self.paths.channel("commands.jsonl");
        let id = crate::ledger::read_lines(&path).len() as u64;
        let queued = QueuedCommands {
            id,
            author,
            commands: commands.to_vec(),
        };
        crate::ledger::append_line(
            &path,
            &serde_json::to_string(&queued)
                .map_err(|e| crate::Error::Invalid(format!("commands: {e}")))?,
        )?;
        Ok(id)
    }

    /// Exactly what [`claim_commands`](Self::claim_commands) would take, **without**
    /// taking it.
    ///
    /// What a writer about to let go of the run asks: whether there is anything
    /// left for a reconciler to do. Claiming to find out would take envelopes off
    /// the queue this process is not going to apply.
    ///
    /// Named for what a reconciler would claim rather than for everything the
    /// file holds, because the two differ by a line no build can read: that line
    /// is passed over here exactly as the claim passes over it, so a writer does
    /// not stay for work nothing will ever take.
    // llmlint: ignore-block[changed_behavior_has_e2e] the leniency here is not this
    // function's own: it reads the cursor and the queue exactly as `claim_commands` does,
    // line for line, because the question it answers is what that call would take. A
    // cursor or a record no build can read is passed over by both, and a writer that
    // stayed for one would be waiting on work nothing will ever claim — which is the state
    // that has no way out. What that leniency costs is `claim_commands`'s to answer for,
    // and it predates this change.
    pub(crate) fn claimable_commands(&self) -> Vec<QueuedCommands> {
        let claimed_through: u64 =
            crate::ledger::read_json_opt(&self.paths.channel("commands-cursor.json")).unwrap_or(0);
        crate::ledger::read_lines(&self.paths.channel("commands.jsonl"))
            .iter()
            .filter_map(|line| serde_json::from_str::<QueuedCommands>(line).ok())
            .filter(|queued| queued.id >= claimed_through)
            .collect()
    }

    // llmlint: ignore-end[changed_behavior_has_e2e]

    /// Claim the command envelopes the reconciler has not drained yet.
    pub fn claim_commands(&self) -> crate::Result<Vec<QueuedCommands>> {
        let cursor_path = self.paths.channel("commands-cursor.json");
        let claimed_through: u64 = crate::ledger::read_json_opt(&cursor_path).unwrap_or(0);
        let fresh: Vec<QueuedCommands> =
            crate::ledger::read_lines(&self.paths.channel("commands.jsonl"))
                .iter()
                .filter_map(|line| serde_json::from_str::<QueuedCommands>(line).ok())
                .filter(|queued| queued.id >= claimed_through)
                .collect();
        if let Some(last) = fresh.last() {
            crate::ledger::write_json(&cursor_path, &(last.id + 1))?;
        }
        Ok(fresh)
    }

    /// Answer one claimed envelope, so its submitter can stop waiting.
    pub fn answer_commands(&self, outcome: &CommandOutcome) -> crate::Result<()> {
        crate::ledger::append_line(
            &self.paths.channel("command-outcomes.jsonl"),
            &serde_json::to_string(outcome)
                .map_err(|e| crate::Error::Invalid(format!("outcome: {e}")))?,
        )
    }

    /// The reconciler's answer to one envelope, if it has given one.
    pub fn outcome_of(&self, id: u64) -> Option<CommandOutcome> {
        crate::ledger::read_lines(&self.paths.channel("command-outcomes.jsonl"))
            .iter()
            .filter_map(|line| serde_json::from_str::<CommandOutcome>(line).ok())
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
            }
        };
        Reply {
            version: Some(REPLY_ENVELOPE_VERSION),
            author: Author::Planner,
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

    /// An envelope written against the version before this one is still read, and
    /// is read at the version this build reads it at.
    ///
    /// This is the whole of what makes the bump additive: version 3 added an
    /// optional field and took nothing away, so every caller still typing 2 —
    /// this crate's own journeys, the shipped monitor persona, and the
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
            let Command::Settle { landing, .. } = command else {
                panic!("the older envelope carries something other than a settle: {command:?}");
            };
            assert_eq!(
                landing.as_deref(),
                None,
                "a settle written before the landing existed came back carrying one"
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

    fn surface(id: u64, blocking: bool) -> Surface {
        Surface {
            id,
            kind: "finding".to_owned(),
            message: "something happened".to_owned(),
            source: source::PROPOSAL.to_owned(),
            blocking,
            queued_at: 0,
            abandoned: false,
            asker: None,
            workstream: Some("ship".to_owned()),
        }
    }

    /// Every transition of the surface queue that changes what the reconcile loop
    /// reads off it changes the length that loop fingerprints.
    ///
    /// The fingerprint is what a converged driver waits on, and it is two `stat`
    /// calls rather than a read — so the one thing it must not do is let a
    /// transition through unseen. A modification time can repeat on a filesystem
    /// with coarse timestamps; a length is decided by the bytes. This holds the
    /// half that does not depend on the clock: push, claim and answer, against the
    /// decision set the loop actually derives from each, abandonment included.
    #[test]
    fn every_queue_change_the_loop_reads_shows_in_its_length() {
        let root =
            std::env::temp_dir().join(format!("onepipeline-queuemark-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = crate::ledger::RunPaths::under(&root, "marks");
        paths.create().expect("the run directory");
        let channel = ChannelState::new(&paths);
        // What the loop reads: every blocking surface outstanding, whether it is
        // waiting to be delivered or already delivered and unanswered.
        let outstanding = |channel: &ChannelState| -> Vec<u64> {
            let queue = channel.queue();
            queue
                .waiting
                .iter()
                .chain(queue.pending.iter())
                .filter(|surface| surface.blocking && !surface.abandoned)
                .map(|surface| surface.id)
                .collect()
        };
        let length = |channel: &ChannelState| -> Option<u64> {
            mark(&channel.queue_path()).map(|(bytes, _)| bytes)
        };

        let empty = length(&channel);
        // The queue issues the id, so what it handed back is what to look for.
        let pushed = channel.push(surface(0, true)).expect("a surface is queued");
        let queued = length(&channel);
        assert_ne!(empty, queued, "a queued surface did not change the length");
        assert_eq!(outstanding(&channel), vec![pushed.id]);

        // A claim is the one transition that leaves the decision set alone, so it
        // is the one a fingerprint could miss without losing anything.
        let claimed = channel.claim().expect("the surface is claimed");
        assert!(claimed.is_some());
        assert_eq!(
            outstanding(&channel),
            vec![pushed.id],
            "a claimed blocking surface stopped being outstanding"
        );

        channel
            .answer(&Reply {
                completion: None,
                commands: Vec::new(),
                ..Reply::default()
            })
            .expect("the surface is answered");
        assert_ne!(
            length(&channel),
            queued,
            "an answered surface did not change the length"
        );
        assert_eq!(outstanding(&channel), Vec::<u64>::new());

        // Abandonment is the fourth transition, and the loop derives its
        // decision set from it exactly as it does from an answer.
        let second = channel.push(surface(0, true)).expect("a surface is queued");
        let waiting = length(&channel);
        assert_eq!(outstanding(&channel), vec![second.id]);
        let marked = channel
            .abandon(&[second.id])
            .expect("the surface is marked");
        assert_eq!(marked.len(), 1, "{marked:?}");
        assert!(marked[0].abandoned);
        assert_ne!(
            length(&channel),
            waiting,
            "an abandoned surface did not change the length"
        );
        assert_eq!(outstanding(&channel), Vec::<u64>::new());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What abandonment does to each of the two places a surface can be, and to
    /// the text in both.
    ///
    /// The two are not the same case and are deliberately not treated the same.
    /// A surface still `waiting` is one no manager has seen, so the queue holds
    /// the only copy of its text: it is marked in place and stays claimable. One
    /// in `pending` has been delivered to a reader and is the surface a verdict
    /// binds to, so it is marked *where it is* — the slot keeps it, and nothing
    /// reports the run as awaiting a planner, because that is
    /// [`ChannelState::pending`]'s answer rather than the slot's occupancy.
    #[test]
    fn abandoning_keeps_every_surface_readable_and_leaves_the_slot_holding_its_own() {
        let root = std::env::temp_dir().join(format!("onepipeline-abandon-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = crate::ledger::RunPaths::under(&root, "gone");
        paths.create().expect("the run directory");
        let channel = ChannelState::new(&paths);

        let read = channel.push(surface(0, true)).expect("the question queues");
        let unread = channel
            .push(Surface {
                message: "nobody has seen this".to_owned(),
                ..surface(0, false)
            })
            .expect("the narration queues");
        // A blocking surface is claimed first, so this is the one now pending.
        channel.claim().expect("a claim").expect("a surface");
        assert_eq!(channel.pending().map(|held| held.id), Some(read.id));

        let marked = channel
            .abandon(&[read.id, unread.id])
            .expect("both are marked");
        assert_eq!(marked.len(), 2, "{marked:?}");
        assert!(marked.iter().all(|surface| surface.abandoned));

        // Nothing is reported as awaiting a planner, and the slot still holds
        // the question it was handed: those are two facts rather than one.
        assert_eq!(channel.pending(), None);
        assert_eq!(channel.held().map(|held| held.id), Some(read.id));
        assert!(channel.held().is_some_and(|held| held.abandoned));
        let queue = channel.queue();
        assert_eq!(queue.waiting.len(), 1, "{queue:?}");
        // Both texts survive, the delivered one included.
        let mut messages: Vec<String> = queue
            .waiting
            .iter()
            .chain(queue.pending.iter())
            .map(|surface| surface.message.clone())
            .collect();
        messages.sort();
        assert_eq!(messages, vec!["nobody has seen this", "something happened"]);

        // The unread one stays claimable, and taking it does not put the run
        // back to awaiting a ruling nobody is owed. The delivered one is not
        // handed out a second time: it is in the slot its reader already has it
        // from.
        let first = channel.claim().expect("a claim").expect("a surface");
        assert_eq!(first.id, unread.id);
        assert!(first.abandoned);
        assert_eq!(channel.pending(), None);
        assert_eq!(channel.claim().expect("a claim"), None);

        // The run's own record carries what became of each, under its own id.
        let logged: Vec<Surface> = crate::ledger::read_lines(&paths.channel("surfaces.jsonl"))
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        for id in [read.id, unread.id] {
            assert!(
                logged
                    .iter()
                    .any(|surface| surface.id == id && surface.abandoned),
                "no record that surface {id} was abandoned: {logged:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A recorded asker comes back as the name it was, and one that names nobody
    /// comes back as nobody — rather than taking the queue around it down.
    #[test]
    fn a_recorded_asker_that_names_nobody_reads_as_nobody() {
        let raised = Surface {
            asker: Some(Asker::checked("dispatch-a").expect("a name")),
            ..surface(7, true)
        };
        let written = serde_json::to_string(&raised).expect("the surface writes");
        assert!(written.contains(r#""asker":"dispatch-a""#), "{written}");
        let read: Surface = serde_json::from_str(&written).expect("the surface reads back");
        assert_eq!(read.asker, raised.asker);

        // The two shapes this crate never writes: a name that identifies nobody,
        // and no name at all. Both are the same fact, and neither costs the
        // surface around them.
        for recorded in [r##","asker":"""##, ""] {
            let line = written.replace(r#","asker":"dispatch-a""#, recorded);
            let read: Surface = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("a queue recording {recorded:?} was lost: {e}"));
            assert_eq!(read.asker, None, "{line}");
            assert_eq!(read.message, raised.message);
        }
    }

    /// A live question takes the slot from an abandoned one without taking its
    /// text with it.
    ///
    /// The slot holds one surface, and a question somebody is waiting on outranks
    /// one nobody is. What must not happen is the abandoned one being written
    /// over: the queue is the only place a reader can still reach it, so it goes
    /// back among the readable ones on its way out.
    #[test]
    fn a_live_question_takes_the_slot_and_the_abandoned_one_it_displaces_stays_readable() {
        let root = std::env::temp_dir().join(format!("onepipeline-displace-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = crate::ledger::RunPaths::under(&root, "displaced");
        paths.create().expect("the run directory");
        let channel = ChannelState::new(&paths);

        let gone = channel.push(surface(0, true)).expect("the question queues");
        channel.claim().expect("a claim").expect("a surface");
        channel.abandon(&[gone.id]).expect("it is marked");
        assert_eq!(channel.held().map(|held| held.id), Some(gone.id));

        let live = channel
            .push(Surface {
                message: "somebody is waiting on this".to_owned(),
                ..surface(0, true)
            })
            .expect("the live question queues");
        let claimed = channel.claim().expect("a claim").expect("a surface");
        assert_eq!(claimed.id, live.id);
        assert_eq!(channel.pending().map(|held| held.id), Some(live.id));
        // And the one it displaced is still there to read.
        let queue = channel.queue();
        assert_eq!(
            queue
                .waiting
                .iter()
                .map(|surface| (surface.id, surface.message.as_str()))
                .collect::<Vec<_>>(),
            vec![(gone.id, "something happened")],
            "{queue:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A listener that comes back takes its own asker's questions and nobody
    /// else's, and a surface naming no asker is taken by neither.
    ///
    /// The scoping is the whole of what keeps adoption honest, so it is stated
    /// here against the queue directly: run-wide adoption would hand a dead
    /// member's question back to the run for as long as any unrelated session
    /// happened to be serving it.
    #[test]
    fn attending_takes_back_one_askers_surfaces_and_leaves_every_other_alone() {
        let root = std::env::temp_dir().join(format!("onepipeline-attend-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = crate::ledger::RunPaths::under(&root, "back");
        paths.create().expect("the run directory");
        let channel = ChannelState::new(&paths);

        let asked = |asker: Option<&str>, blocking: bool, message: &str| Surface {
            message: message.to_owned(),
            asker: asker.map(|name| Asker::checked(name).expect("a name")),
            ..surface(0, blocking)
        };
        let mine = channel
            .push(asked(Some("dispatch-a"), true, "is this base still right?"))
            .expect("the question queues");
        let theirs = channel
            .push(asked(Some("dispatch-b"), true, "somebody else's question"))
            .expect("their question queues");
        let nobodys = channel
            .push(asked(None, false, "raised by no session"))
            .expect("the narration queues");
        // Mine is the one a manager read, so it is the one in the slot.
        channel.claim().expect("a claim").expect("a surface");
        assert_eq!(channel.pending().map(|held| held.id), Some(mine.id));
        channel
            .abandon(&[mine.id, theirs.id, nobodys.id])
            .expect("all three are marked");
        assert_eq!(channel.pending(), None);

        let named = |name: &str| Asker::checked(name).expect("a name");
        let taken = channel
            .attend(&named("dispatch-a"))
            .expect("mine comes back");
        assert_eq!(
            taken.iter().map(|surface| surface.id).collect::<Vec<_>>(),
            vec![mine.id]
        );
        // The question is answerable again, in the slot a verdict names.
        assert_eq!(channel.pending().map(|held| held.id), Some(mine.id));
        let queue = channel.queue();
        assert!(
            queue
                .waiting
                .iter()
                .all(|surface| surface.abandoned && surface.id != mine.id),
            "attending took a surface belonging to another asker: {queue:?}"
        );
        // A second listener of the same asker finds nothing left to take, and
        // says so without writing a further record.
        let lines = crate::ledger::read_lines(&paths.channel("surfaces.jsonl")).len();
        assert!(channel
            .attend(&named("dispatch-a"))
            .expect("nothing")
            .is_empty());
        assert!(channel
            .attend(&named("dispatch-c"))
            .expect("nothing")
            .is_empty());
        assert_eq!(
            crate::ledger::read_lines(&paths.channel("surfaces.jsonl")).len(),
            lines,
            "attending what was already attended wrote a second record"
        );
        // And the record carries the correction under the surface's own id.
        let logged: Vec<Surface> = crate::ledger::read_lines(&paths.channel("surfaces.jsonl"))
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert!(
            logged.iter().any(|surface| surface.id == mine.id
                && !surface.abandoned
                && surface.asker.is_some()),
            "no record that surface {} was taken back: {logged:?}",
            mine.id
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A surface queued while the channel is being read is still readable
    /// afterwards, and no id is ever handed out twice.
    ///
    /// Real threads over a real run root, pushing and claiming at once with no
    /// coordination beyond the channel's own lock. Several writers, because two
    /// sessions raising at once must not share an id; several readers, because
    /// a reader's write-back is what used to destroy a concurrent push. Every
    /// push is accounted for by exactly one claim, every id is distinct, and
    /// the log alone carries both facts.
    #[test]
    fn a_surface_queued_during_a_read_of_the_channel_is_neither_lost_nor_given_a_used_id() {
        const WRITERS: usize = 4;
        const READERS: usize = 3;
        const EACH: usize = 40;
        let root = std::env::temp_dir().join(format!("onepipeline-raced-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = crate::ledger::RunPaths::under(&root, "raced");
        paths.create().expect("the run directory");

        let claimed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Surface>::new()));
        let queued = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Surface>::new()));
        // Lowered once every writer has finished: a reader stops at the first
        // empty claim after that, so a surface the queue lost is a shortfall in
        // what was read rather than a reader waiting for ever.
        let writing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let writers: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let channel = ChannelState::new(&paths);
                let queued = std::sync::Arc::clone(&queued);
                std::thread::spawn(move || {
                    for n in 0..EACH {
                        let pushed = channel
                            .push(Surface {
                                message: format!("writer {writer} question {n}"),
                                ..surface(0, n % 2 == 0)
                            })
                            .expect("a surface is queued");
                        queued.lock().expect("the list").push(pushed);
                    }
                })
            })
            .collect();
        let readers: Vec<_> = (0..READERS)
            .map(|_| {
                let channel = ChannelState::new(&paths);
                let claimed = std::sync::Arc::clone(&claimed);
                let writing = std::sync::Arc::clone(&writing);
                std::thread::spawn(move || loop {
                    if let Some(surface) = channel.claim().expect("a claim") {
                        claimed.lock().expect("the list").push(surface);
                        continue;
                    }
                    if !writing.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    std::thread::yield_now();
                })
            })
            .collect();
        // The readers are released before any writer's failure is raised, so a
        // failed writer fails the test rather than leaving readers spinning on
        // `writing` behind it. Whatever is still waiting once every writer is
        // done is drained by the readers, which stop at the first empty claim
        // after that.
        let written: Vec<_> = writers.into_iter().map(|writer| writer.join()).collect();
        writing.store(false, std::sync::atomic::Ordering::SeqCst);
        let read_out: Vec<_> = readers.into_iter().map(|reader| reader.join()).collect();
        for writer in written {
            writer.expect("a writer finishes");
        }
        for reader in read_out {
            reader.expect("a reader finishes");
        }

        let queued = queued.lock().expect("the list").clone();
        let claimed = claimed.lock().expect("the list").clone();
        let mut ids: Vec<u64> = queued.iter().map(|surface| surface.id).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            (0..(WRITERS * EACH) as u64).collect::<Vec<_>>(),
            "an id was handed out twice or skipped"
        );
        let mut read: Vec<(u64, String)> = claimed
            .iter()
            .map(|surface| (surface.id, surface.message.clone()))
            .collect();
        read.sort();
        let mut sent: Vec<(u64, String)> = queued
            .iter()
            .map(|surface| (surface.id, surface.message.clone()))
            .collect();
        sent.sort();
        assert_eq!(read, sent, "a surface was lost or delivered twice");
        let queue = ChannelState::new(&paths).queue();
        assert!(queue.waiting.is_empty(), "{queue:?}");
        assert_eq!(queue.next_id, (WRITERS * EACH) as u64);

        // And the log alone accounts for both facts: every surface queued once
        // and claimed once, under its own id.
        let records: Vec<SurfaceRecord> =
            crate::ledger::read_lines(&paths.channel("surfaces.jsonl"))
                .iter()
                .map(|line| serde_json::from_str(line).expect("a record this build wrote"))
                .collect();
        for id in 0..(WRITERS * EACH) as u64 {
            for event in [SurfaceEvent::Queued, SurfaceEvent::Claimed] {
                assert_eq!(
                    records
                        .iter()
                        .filter(|record| record.surface.id == id && record.event == Some(event))
                        .count(),
                    1,
                    "surface {id} is not recorded {event:?} exactly once"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A second abandonment of the same surface is not a second record.
    #[test]
    fn abandoning_what_is_already_abandoned_records_nothing_further() {
        let root =
            std::env::temp_dir().join(format!("onepipeline-reabandon-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = crate::ledger::RunPaths::under(&root, "twice");
        paths.create().expect("the run directory");
        let channel = ChannelState::new(&paths);

        let queued = channel.push(surface(0, true)).expect("the surface queues");
        assert_eq!(channel.abandon(&[queued.id]).expect("marked").len(), 1);
        let lines = crate::ledger::read_lines(&paths.channel("surfaces.jsonl")).len();
        assert!(channel.abandon(&[queued.id]).expect("nothing").is_empty());
        assert!(channel.abandon(&[]).expect("nothing").is_empty());
        assert_eq!(
            crate::ledger::read_lines(&paths.channel("surfaces.jsonl")).len(),
            lines,
            "abandoning the same surface twice wrote a second record"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The queue is a projection of the log, and the log alone rebuilds it —
    /// whatever became of the projection.
    ///
    /// Every transition a surface can make is driven here — queued, replaced as
    /// a check-in, claimed, abandoned, attended, answered — and after each the
    /// projection is lost six ways. Three lose the file: deleted, replaced with
    /// something that is not a queue, and overwritten with a copy taken
    /// earlier, which is the stale write-back the lost update was. Three keep
    /// it stamped at the log's length and move its claims under the stamp: its
    /// waiting surfaces emptied, its counter reset, and its seal removed. Each
    /// time the read answers exactly what it answered before, and writes the
    /// repaired projection back so the next read is one read again.
    #[test]
    fn the_queue_is_rebuilt_from_the_log_alone_whatever_became_of_the_projection() {
        let root = std::env::temp_dir().join(format!("onepipeline-rebuilt-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = crate::ledger::RunPaths::under(&root, "rebuilt");
        paths.create().expect("the run directory");
        let channel = ChannelState::new(&paths);
        let queue_path = channel.queue_path();
        let named = |name: &str| Asker::checked(name).expect("a name");

        // What the projection said before the transition under test: the stale
        // copy a racing reader would have written back over it.
        let mut earlier = std::fs::read(&queue_path).ok();
        let lost_six_ways = |channel: &ChannelState, earlier: &Option<Vec<u8>>, what: &str| {
            let expected = channel.queue();
            let stamped = expected
                .accounted
                .expect("this build stamps what it writes");
            let current = std::fs::read(&queue_path).expect("the projection was written");
            // A stamped document whose claims moved under an intact stamp — and
            // under an intact seal, which no longer seals them — is one nothing
            // here wrote: the three ways a writer's claims can be wrong while
            // its stamp still matches the log's length, beside the three ways
            // the file itself can be lost.
            let claims_moved = |edit: &dyn Fn(&mut serde_json::Value)| -> Vec<u8> {
                let mut document: serde_json::Value =
                    serde_json::from_slice(&current).expect("the projection is JSON");
                edit(&mut document);
                serde_json::to_vec(&document).expect("the edited document")
            };
            let placed: Vec<(&str, Option<Vec<u8>>)> = vec![
                ("deleted", None),
                (
                    "replaced with something that is not a queue",
                    Some(b"{\"waiting\": ".to_vec()),
                ),
                ("overwritten with a stale copy", earlier.clone()),
                (
                    "stamped but with its waiting surfaces emptied",
                    Some(claims_moved(&|document| {
                        document["waiting"] = serde_json::json!([]);
                        document["pending"] = serde_json::Value::Null;
                    })),
                ),
                (
                    "stamped but with its counter reset",
                    Some(claims_moved(&|document| {
                        document["next_id"] = serde_json::json!(0);
                    })),
                ),
                (
                    "stamped but with no seal",
                    Some(claims_moved(&|document| {
                        document["waiting"] = serde_json::json!([]);
                        document.as_object_mut().expect("an object").remove("seal");
                    })),
                ),
            ];
            for (how, bytes) in placed {
                match bytes {
                    Some(bytes) => {
                        std::fs::write(&queue_path, bytes).expect("the projection is placed")
                    }
                    None => std::fs::remove_file(&queue_path).expect("the projection is removed"),
                }
                let rebuilt = channel.queue();
                assert_eq!(rebuilt, expected, "after {what}, with the projection {how}");
                assert_eq!(
                    rebuilt.accounted,
                    Some(stamped),
                    "after {what}, with the projection {how}: the stamp moved"
                );
                // As a document rather than as bytes: an edit that changed no
                // claim still seals, and is rightly left as it stands.
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(
                        &std::fs::read(&queue_path).expect("the projection is written back")
                    )
                    .expect("a document"),
                    serde_json::from_slice::<serde_json::Value>(&current).expect("a document"),
                    "after {what}, with the projection {how}: the repair was not written back"
                );
            }
        };
        let mut step = |what: &str, act: &dyn Fn(&ChannelState)| {
            act(&channel);
            lost_six_ways(&channel, &earlier, what);
            earlier = std::fs::read(&queue_path).ok();
        };

        step("a question is queued", &|channel| {
            channel
                .push(Surface {
                    asker: Some(named("dispatch-a")),
                    ..surface(0, true)
                })
                .expect("queued");
        });
        step("narration is queued behind it", &|channel| {
            channel.push(surface(0, false)).expect("queued");
        });
        step("a check-in is queued", &|channel| {
            channel
                .push(Surface {
                    source: source::CHECK_IN.to_owned(),
                    message: "first".to_owned(),
                    ..surface(0, false)
                })
                .expect("queued");
        });
        step("a second check-in replaces it", &|channel| {
            channel
                .push(Surface {
                    source: source::CHECK_IN.to_owned(),
                    message: "second".to_owned(),
                    ..surface(0, false)
                })
                .expect("queued");
            let queue = channel.queue();
            assert_eq!(
                queue
                    .waiting
                    .iter()
                    .filter(|surface| surface.source == source::CHECK_IN)
                    .map(|surface| surface.message.as_str())
                    .collect::<Vec<_>>(),
                vec!["second"]
            );
        });
        step("the question is claimed", &|channel| {
            let claimed = channel.claim().expect("a claim").expect("a surface");
            assert_eq!(claimed.id, 0);
            assert_eq!(channel.pending().map(|held| held.id), Some(0));
        });
        step("its listener leaves", &|channel| {
            assert_eq!(channel.abandon(&[0, 1]).expect("marked").len(), 2);
            assert_eq!(channel.pending(), None);
        });
        step("its asker comes back", &|channel| {
            assert_eq!(
                channel.attend(&named("dispatch-a")).expect("taken").len(),
                1
            );
            assert_eq!(channel.pending().map(|held| held.id), Some(0));
        });
        step("it is answered", &|channel| {
            channel.answer(&Reply::default()).expect("answered");
            assert_eq!(channel.held(), None);
        });
        step("everything left is read", &|channel| {
            while channel.claim().expect("a claim").is_some() {}
            let queue = channel.queue();
            assert!(queue.waiting.is_empty(), "{queue:?}");
            assert_eq!(queue.next_id, 4);
        });

        // A record whose writer has not finished it is not folded and does not
        // move the stamp: the read resumes before it, and folds it whole once
        // its writer is done. A line the build cannot read is passed over and
        // does move the stamp, so it is not re-read for ever.
        let log = paths.channel("surfaces.jsonl");
        let settled = channel.queue();
        let fragment = serde_json::to_string(&SurfaceRecord {
            event: Some(SurfaceEvent::Queued),
            surface: surface(4, true),
        })
        .expect("a record");
        let (head, tail) = fragment.split_at(fragment.len() / 2);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .and_then(|mut file| std::io::Write::write_all(&mut file, head.as_bytes()))
            .expect("the fragment is written");
        let torn = channel.queue();
        assert_eq!(torn, settled, "a torn record was folded");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .and_then(|mut file| {
                std::io::Write::write_all(&mut file, format!("{tail}\nnot a record\n").as_bytes())
            })
            .expect("the record is finished");
        let whole = channel.queue();
        assert_eq!(
            whole
                .waiting
                .iter()
                .map(|surface| surface.id)
                .collect::<Vec<_>>(),
            vec![4],
            "{whole:?}"
        );
        assert_eq!(
            whole.accounted,
            Some(std::fs::metadata(&log).expect("the log").len()),
            "the unreadable line was not accounted for"
        );
        assert_eq!(channel.queue(), whole, "a settled log was folded again");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A log an older build wrote — no event on any line, and no line at all for
    /// a claim or an answer — folds to what that build meant by it, and a
    /// projection that build wrote is taken as it stands.
    ///
    /// The first line under an id is the surface being queued; every later line
    /// under it is the surface being abandoned or taken back, which the flag it
    /// carries tells apart. A projection with no stamp is one such a build kept
    /// consistent by its own means and logged nothing this build could replay
    /// over it, so it is trusted whole rather than rebuilt into every surface it
    /// ever queued.
    #[test]
    fn a_log_and_a_projection_an_older_build_wrote_are_read_as_that_build_meant_them() {
        let root = std::env::temp_dir().join(format!("onepipeline-legacy-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = crate::ledger::RunPaths::under(&root, "legacy");
        paths.create().expect("the run directory");
        let channel = ChannelState::new(&paths);
        let asked = Surface {
            asker: Some(Asker::checked("dispatch-a").expect("a name")),
            ..surface(0, true)
        };
        let gone = Surface {
            abandoned: true,
            ..asked.clone()
        };
        let narration = surface(1, false);
        for line in [&asked, &narration, &gone, &asked] {
            crate::ledger::append_line(
                &paths.channel("surfaces.jsonl"),
                &serde_json::to_string(line).expect("a line"),
            )
            .expect("the older build's line");
        }

        // No projection at all: folded from the log, as that build's lines mean.
        let rebuilt = channel.queue();
        assert_eq!(
            rebuilt
                .waiting
                .iter()
                .map(|surface| (surface.id, surface.abandoned))
                .collect::<Vec<_>>(),
            vec![(0, false), (1, false)],
            "{rebuilt:?}"
        );
        assert_eq!(rebuilt.next_id, 2);
        assert_eq!(rebuilt.waiting[0].asker, asked.asker);

        // That build's own projection, which says the question was read and
        // answered though its log never could: taken as it stands, and stamped
        // by the first write this build makes.
        crate::ledger::write_json(
            &channel.queue_path(),
            &serde_json::json!({"waiting": [narration], "pending": null, "next_id": 2}),
        )
        .expect("the older build's projection");
        let trusted = channel.queue();
        assert_eq!(
            trusted
                .waiting
                .iter()
                .map(|surface| surface.id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(trusted.next_id, 2);
        assert!(trusted.accounted.is_some(), "{trusted:?}");
        let claimed = channel.claim().expect("a claim").expect("a surface");
        assert_eq!(claimed.id, 1);
        let stamped = channel.queue();
        assert!(stamped.waiting.is_empty(), "{stamped:?}");
        assert!(stamped.accounted.is_some(), "{stamped:?}");

        // The projection that build left after the lost update — the counter
        // never advanced past an id the log already carries — is brought over
        // with the lost question restored, and the id is never allocated again.
        // What the log went on to say about each is folded too: the question
        // was abandoned and then taken back, and the narration was claimed
        // above, so it is not restored to the waiting ones.
        crate::ledger::write_json(
            &channel.queue_path(),
            &serde_json::json!({"waiting": [], "pending": null, "next_id": 0}),
        )
        .expect("the older build's projection");
        let restored = channel.queue();
        assert_eq!(
            restored
                .waiting
                .iter()
                .map(|surface| (surface.id, surface.abandoned))
                .collect::<Vec<_>>(),
            vec![(0, false)],
            "{restored:?}"
        );
        assert_eq!(restored.next_id, 2);
        let fresh = channel.push(surface(0, false)).expect("queued");
        assert_eq!(
            fresh.id, 2,
            "an id the log had allocated was handed out again"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A tail the fold cannot read refuses the fold, and nothing is stamped.
    ///
    /// What [`ChannelState::record`] stamps afterwards is the log's whole
    /// length, so a fold that read nothing of the tail and went on would mark
    /// every record in it accounted for without having folded one — a question
    /// hidden for good by the write meant to record one. The refusal is handed
    /// back instead, and the projection on disk is exactly what it was.
    #[test]
    fn a_tail_the_fold_cannot_read_refuses_the_fold_and_stamps_nothing() {
        let root =
            std::env::temp_dir().join(format!("onepipeline-unreadtail-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = crate::ledger::RunPaths::under(&root, "unreadtail");
        paths.create().expect("the run directory");
        let channel = ChannelState::new(&paths);
        channel.push(surface(0, true)).expect("queued");
        let written = std::fs::read(channel.queue_path()).expect("the projection");
        // The log has grown past the stamp, so the fold has a tail to ask for.
        crate::ledger::append_line(
            &paths.channel("surfaces.jsonl"),
            &serde_json::to_string(&SurfaceRecord {
                event: Some(SurfaceEvent::Queued),
                surface: surface(1, false),
            })
            .expect("a record"),
        )
        .expect("the log grows");

        let refused = channel.current(|from| {
            Err(crate::Error::Refused(format!(
                "the tail from byte {from} cannot be read"
            )))
        });
        assert!(
            matches!(&refused, Err(crate::Error::Refused(why)) if why.contains("cannot be read")),
            "{refused:?}"
        );
        assert_eq!(
            std::fs::read(channel.queue_path()).expect("the projection"),
            written,
            "a refused fold moved the projection"
        );
        // And the record the fold could not read is still there for the next one.
        let queue = channel.queue();
        assert_eq!(
            queue.waiting.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![0, 1]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A record a writer appended and died before projecting still wakes the
    /// loop, which folds it in.
    ///
    /// The loop waits on the fingerprint and reads nothing until it moves, so
    /// a record that reached the log and never the projection would be one the
    /// loop slept through until something else woke it. The log is marked
    /// beside the projection for exactly that record: here it is placed as the
    /// dead writer left it — appended, with the projection untouched — and the
    /// fingerprint moves, and the read that follows hands the record over.
    #[test]
    fn a_record_appended_without_its_projection_still_moves_the_fingerprint() {
        let root = std::env::temp_dir().join(format!("onepipeline-logmark-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = crate::ledger::RunPaths::under(&root, "logmark");
        paths.create().expect("the run directory");
        let channel = ChannelState::new(&paths);
        channel.push(surface(0, false)).expect("queued");
        let projected = std::fs::read(channel.queue_path()).expect("the projection");
        let seen = channel.fingerprint();

        crate::ledger::append_line(
            &channel.log_path(),
            &serde_json::to_string(&SurfaceRecord {
                event: Some(SurfaceEvent::Queued),
                surface: surface(1, true),
            })
            .expect("a record"),
        )
        .expect("the log grows");
        assert_eq!(
            std::fs::read(channel.queue_path()).expect("the projection"),
            projected,
            "the append moved the projection, so this proves nothing about the log's mark"
        );
        assert_ne!(
            channel.fingerprint(),
            seen,
            "a record appended without its projection left the fingerprint where it was"
        );
        let queue = channel.queue();
        assert_eq!(
            queue.waiting.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![0, 1]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A read of a channel nothing has written since is one read of the
    /// projection and no read of the log.
    ///
    /// The projection earns its place by being the cheap answer: the unread
    /// count is paid per run root by the listing across every root on the host,
    /// so this holds what the read costs in bytes rather than asserting it.
    #[test]
    fn an_unchanged_channel_is_answered_without_reading_the_log() {
        let root = std::env::temp_dir().join(format!("onepipeline-oneread-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = crate::ledger::RunPaths::under(&root, "oneread");
        paths.create().expect("the run directory");
        let channel = ChannelState::new(&paths);
        channel.push(surface(0, true)).expect("queued");
        channel.push(surface(0, false)).expect("queued");
        channel.claim().expect("a claim").expect("a surface");
        let projection = std::fs::metadata(channel.queue_path())
            .expect("the projection")
            .len();
        let log = std::fs::metadata(paths.channel("surfaces.jsonl"))
            .expect("the log")
            .len();
        assert!(log > 0);

        let before = crate::ledger::bytes_read();
        let queue = channel.queue();
        let cost = crate::ledger::bytes_read() - before;
        assert_eq!(
            cost, projection,
            "an unchanged channel cost {cost} byte(s) to read against a {projection}-byte \
             projection and a {log}-byte log: {queue:?}"
        );
        // Bytes cannot see a read that opens the log and finds nothing past the
        // stamp, so the tail reader itself is what is held to never being asked.
        let Ok((same, folded)) = channel.current(|from| -> Result<_, std::convert::Infallible> {
            panic!("an unchanged channel read its log from byte {from}");
        });
        assert_eq!(same, queue);
        assert!(!folded);
        let _ = std::fs::remove_dir_all(&root);
    }
}
