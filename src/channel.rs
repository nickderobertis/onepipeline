//! The planner channel: the wire shapes, and the durable queue behind them.
//!
//! A reply is one JSON envelope: a legacy verdict, a version-1 list of graph
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
// is "legacy verdicts *plus* a version-1 command list", so a reply may legally carry
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

/// The reply envelope version this crate reads and writes.
pub const REPLY_ENVELOPE_VERSION: u32 = 1;

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
        | Command::Context { .. }
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
        // against the conversation running now. An observer keeps `context`,
        // which reaches the worker and binds nothing.
        Command::Note { .. } => {
            "a note may bind a criterion the node's judge decides against, which is the \
             planner's decision; `context` is the note that binds nothing"
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
        Command::Context { .. } => "context",
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
        | Command::Context { id, .. }
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
    /// [`REPLY_ENVELOPE_VERSION`] when the envelope carries commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
/// The variants and their required fields are the live-edit protocol's table.
/// `context` carries one field beyond it, [`Deliver`], which is optional and
/// defaults to what that table's `context` always did — so an edit written
/// against the table alone is still exactly the edit it was.
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
    /// Carry one planner note to the node, without cancelling or restarting
    /// anything.
    Context {
        /// The node the note is for.
        id: String,
        /// The note. It carries exactly one dispatch: it attaches to the node's
        /// next one and is consumed when that dispatch takes it.
        note: String,
        /// When the note reaches the node. Omitted, it is [`Deliver::Auto`],
        /// which is what every `context` edit written before this field got.
        #[serde(default, skip_serializing_if = "Deliver::is_auto")]
        deliver: Deliver,
    },
    /// Make one binding amendment to what a node is judged against.
    ///
    /// The lever a manager has that a `context` note is not. A note steers the
    /// worker, says of itself that it adds no acceptance criteria, and carries
    /// exactly one dispatch; this becomes part of the node's **effective task**,
    /// which the worker and the judge reviewing it are handed alike, on the
    /// dispatch that follows it and on every later one. A turn already running
    /// is not reached — its task was composed before the ruling existed — which
    /// is the asymmetry with `context`, whose point is the turn running now.
    Amend {
        /// The node to amend. It must be one the graph holds and can still be
        /// dispatched: a node that has settled `done` is refused for the reason
        /// `context` refuses one, since nothing will read the amendment.
        id: String,
        /// The binding text. Blank is refused rather than recorded.
        ///
        /// A second amendment **replaces** the first: the latest is the node's
        /// amendment and the earlier one stops being part of the effective task.
        /// A bar that could only grow could not be corrected.
        text: String,
    },
    /// Deliver one note into the node's live dispatch, to whichever party of it
    /// is speaking.
    ///
    /// The lever `context` and `amend` are each half of. It goes to the node's
    /// running conversation through the delivery seam
    /// [`oneagentgraph`](crate::note) publishes rather than through a bare
    /// interrupt, so the party that is live takes it and the other party
    /// receives it with that party's response; a [`criterion`](Self::Note::criterion)
    /// it carries enters the acceptance criteria that conversation's judge
    /// decides against. **A note that reaches nobody is refused**, naming that it
    /// was not delivered and why, so the planner chooses relaunch, tweak, or
    /// follow-up rather than being told nothing.
    ///
    /// It does not move the node's stored bar: `amend` is still the op for a
    /// ruling that has to survive a re-dispatch.
    Note {
        /// The node whose live dispatch it is for.
        id: String,
        /// Whose task this updates. Required and never guessed: a note whose
        /// addressee is inferred is one the judge may read as work for itself.
        addressee: Addressee,
        /// What the addressee reads. Blank is refused at this boundary by the
        /// seam's own newtype rather than somewhere later.
        text: NoteText,
        /// The property the finished tree must have, when this note changes
        /// that. Omitted, the note is observational: it reaches whoever is live
        /// and touches no acceptance criterion.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criterion: Option<Criterion>,
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

/// When a `context` note reaches the node it is for.
///
/// The default is [`Auto`](Self::Auto), so a `context` edit that says nothing
/// gains live delivery wherever the harness supports one and behaves exactly as
/// it always did where it does not. A value outside these three is refused by
/// serde, naming what it read — this is external input like any other field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Deliver {
    /// Into the node's running turn when it has a controllable one, and onto its
    /// next dispatch when it does not.
    #[default]
    Auto,
    /// Into the node's running turn, or refused with the reason it could not be.
    /// A planner who needs the correction *now* is not silently deferred.
    Live,
    /// Onto the node's next dispatch, and only there.
    Next,
}

impl Deliver {
    /// Whether this is the default, so serialization can omit it.
    fn is_auto(&self) -> bool {
        matches!(self, Self::Auto)
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
    /// Whether anybody is still waiting for the answer.
    ///
    /// Set by [`abandon`](ChannelState::abandon) when the process serving this
    /// surface exited without an answer and the side that asked ended with it.
    /// The surface keeps its text and stays claimable; what it gives up is its
    /// claim on the unread count and on the subtree a blocking surface holds.
    /// Omitted from the wire while it is false, so a queue nothing has abandoned
    /// serializes exactly as it always did.
    #[serde(default, skip_serializing_if = "is_false")]
    pub abandoned: bool,
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
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Queue {
    /// The surfaces nobody has read yet, oldest first.
    #[serde(default)]
    pub waiting: Vec<Surface>,
    /// The surface a planner consumed and has not answered.
    #[serde(default)]
    pub pending: Option<Surface>,
    /// The id the next surface takes.
    #[serde(default)]
    pub next_id: u64,
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

/// What the reconcile loop last saw of the channel's two files.
///
/// Compared rather than read: see [`ChannelState::fingerprint`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    queue: Option<(u64, std::time::SystemTime)>,
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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CommandOutcome {
    /// The envelope this answers.
    pub id: u64,
    /// Whether every command in it was applied.
    pub applied: bool,
    /// Why not, when it was not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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

    /// A cheap look at everything the reconcile loop reads off this channel.
    ///
    /// Two `stat` calls and no read, so a converged driver can check for an
    /// arriving edit five times a second for nothing: the loop reconciles only
    /// when this moved. An absent file fingerprints as absent, so the moment one
    /// appears the fingerprint has changed.
    ///
    /// Length is the load-bearing half — the log only grows, and every queue
    /// transition the loop can read changes the length too — and the timestamp is
    /// the belt beside those braces.
    pub(crate) fn fingerprint(&self) -> Fingerprint {
        Fingerprint {
            queue: mark(&self.queue_path()),
            commands: mark(&self.paths.channel("commands.jsonl")),
        }
    }

    /// The live queue.
    pub fn queue(&self) -> Queue {
        crate::ledger::read_json_opt(&self.queue_path()).unwrap_or_default()
    }

    fn write_queue(&self, queue: &Queue) -> crate::Result<()> {
        crate::ledger::write_json(&self.queue_path(), queue)
    }

    /// Queue one surface, and record that it was *sent*.
    ///
    /// Exactly one check-in is ever pending, and it is kept current rather than
    /// kept still: the next interval's update **replaces** the queued one
    /// instead of being blocked by it, so being ignored makes the harness
    /// louder rather than quieter. The clock is not reset by queuing, so the
    /// staleness a view reports keeps growing while the queued content stays
    /// fresh.
    pub fn push(&self, mut surface: Surface) -> crate::Result<Surface> {
        let mut queue = self.queue();
        surface.id = queue.next_id;
        queue.next_id += 1;
        if surface.source == source::CHECK_IN {
            queue
                .waiting
                .retain(|existing| existing.source != source::CHECK_IN);
        }
        queue.waiting.push(surface.clone());
        self.write_queue(&queue)?;
        crate::ledger::append_line(
            &self.paths.channel("surfaces.jsonl"),
            &serde_json::to_string(&surface)
                .map_err(|e| crate::Error::Invalid(format!("surface: {e}")))?,
        )?;
        Ok(surface)
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
    pub fn claim(&self) -> crate::Result<Option<Surface>> {
        let mut queue = self.queue();
        let next = queue
            .waiting
            .iter()
            .position(|surface| surface.blocking && !surface.abandoned)
            .or_else(|| queue.waiting.iter().position(|surface| !surface.abandoned))
            .unwrap_or(0);
        let claimed = (!queue.waiting.is_empty()).then(|| queue.waiting.remove(next));
        if let Some(surface) = &claimed {
            // A blocking surface outlives its delivery while it waits for an
            // answer, so it is held here rather than dropped: the run is
            // reported as waiting for a planner decision until a reply arrives.
            // Narration read afterwards leaves that standing — reading a report
            // is not answering a question, and a decision the planner never made
            // must not release the subtree it is holding.
            //
            // An abandoned one is the exception, and for the reason the pending
            // slot exists: nothing is waiting for its answer any more, so
            // holding it there would report the run as awaiting a planner on
            // behalf of a reader that has gone. It is still handed over — the
            // text is what a manager reads it for — and it is handed over last,
            // behind everything somebody is still waiting on.
            if surface.blocking && !surface.abandoned {
                queue.pending = Some(surface.clone());
            }
        }
        self.write_queue(&queue)?;
        Ok(claimed)
    }

    /// Say of every surface in `raised` that nobody is waiting for its answer,
    /// and give up the pending slot if one of them is holding it.
    ///
    /// Called when a serving process is about to exit with the side that asked
    /// already gone: its stream ended, so no answer to anything it raised has a
    /// reader left. Answering the surfaces it raised is not the same question as
    /// whether they are still *interesting*, which is why this marks rather than
    /// deletes.
    ///
    /// **Marked, not discarded**, and the queue is what decides it. A surface
    /// still in `waiting` is one no manager has ever seen, and this queue holds
    /// the only copy of its text any reader can reach — discarding it would
    /// throw away an observer's finding in order to fix a count, which is a
    /// worse bargain than the count. So the text stays exactly where a reader
    /// already looks for it and [`claim`](Self::claim) still hands it out; the
    /// flag is what the unread accounting and the decision points read.
    ///
    /// What is genuinely **withdrawn** is narrower and is the half where nothing
    /// can be lost: the *pending slot*. Its surface has already been delivered
    /// to the reader holding it, so putting it back among the readable ones
    /// costs that reader nothing and stops the run reporting that it awaits a
    /// planner nobody is waiting on.
    ///
    /// Returns what it marked, so the caller can record it.
    pub fn abandon(&self, raised: &[u64]) -> crate::Result<Vec<Surface>> {
        let mut queue = self.queue();
        let mut marked: Vec<Surface> = Vec::new();
        for surface in &mut queue.waiting {
            if raised.contains(&surface.id) && !surface.abandoned {
                surface.abandoned = true;
                marked.push(surface.clone());
            }
        }
        if let Some(mut pending) = queue
            .pending
            .clone()
            .filter(|surface| raised.contains(&surface.id))
        {
            queue.pending = None;
            pending.abandoned = true;
            marked.push(pending.clone());
            queue.waiting.push(pending);
        }
        if marked.is_empty() {
            return Ok(marked);
        }
        self.write_queue(&queue)?;
        // The run's own record of what became of each surface, beside the line
        // that recorded it being sent: one further line under the same id,
        // carrying the same text and saying nobody is waiting on it.
        for surface in &marked {
            crate::ledger::append_line(
                &self.paths.channel("surfaces.jsonl"),
                &serde_json::to_string(surface)
                    .map_err(|e| crate::Error::Invalid(format!("surface: {e}")))?,
            )?;
        }
        Ok(marked)
    }

    /// Whether a surface is waiting for an answer.
    pub fn pending(&self) -> Option<Surface> {
        self.queue().pending
    }

    /// Record that a reply answered whatever was pending.
    ///
    /// This is the **verdict** path: what it queues is what a reader waiting on
    /// a ruling takes, and clearing `pending` is what releases the subtree a
    /// blocking surface held. An envelope with edits reaches it through
    /// [`answer_if_verdict`](Self::answer_if_verdict), which is where the two
    /// halves are routed apart.
    pub fn answer(&self, reply: &Reply) -> crate::Result<u64> {
        let mut queue = self.queue();
        queue.pending = None;
        self.write_queue(&queue)?;
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

    fn surface(id: u64, blocking: bool) -> Surface {
        Surface {
            id,
            kind: "finding".to_owned(),
            message: "something happened".to_owned(),
            source: source::PROPOSAL.to_owned(),
            blocking,
            queued_at: 0,
            abandoned: false,
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
    /// in `pending` has already been delivered to a reader, so the *slot* is
    /// withdrawn — nothing is waiting for its answer — and the surface goes back
    /// among the readable ones rather than being dropped.
    #[test]
    fn abandoning_keeps_every_surface_readable_and_withdraws_only_the_pending_slot() {
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

        // The slot is given up, and nothing is reported as awaiting a planner.
        assert_eq!(channel.pending(), None);
        let queue = channel.queue();
        assert_eq!(queue.waiting.len(), 2, "{queue:?}");
        assert!(queue.waiting.iter().all(|surface| surface.abandoned));
        // Both texts survive, the delivered one included.
        let mut messages: Vec<&str> = queue
            .waiting
            .iter()
            .map(|surface| surface.message.as_str())
            .collect();
        messages.sort_unstable();
        assert_eq!(messages, vec!["nobody has seen this", "something happened"]);

        // Both stay claimable, and neither takes the pending slot back.
        let first = channel.claim().expect("a claim").expect("a surface");
        assert!(first.abandoned);
        assert_eq!(channel.pending(), None);
        assert!(channel.claim().expect("a claim").is_some());
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
}
