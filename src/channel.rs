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
    fn is_planner(&self) -> bool {
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
    }
}

/// The node one command is about, when it names one.
pub fn target_of(command: &Command) -> Option<String> {
    match command {
        Command::Add { node } => Some(node.id.clone()),
        Command::Drop { id, .. }
        | Command::Reparent { id, .. }
        | Command::Retry { id, .. }
        | Command::Cancel { id }
        | Command::Requeue { id, .. }
        | Command::Context { id, .. } => Some(id.clone()),
        Command::Attest { reference } => Some(reference.clone()),
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

    /// Claim the next readable surface.
    ///
    /// Nothing to outlive: a surface describes the one continuous run, so it
    /// stays consumable until somebody reads it. A check-in that has been
    /// superseded is replaced at [`push`](Self::push) rather than discarded
    /// here.
    pub fn claim(&self) -> crate::Result<Option<Surface>> {
        let mut queue = self.queue();
        let claimed = (!queue.waiting.is_empty()).then(|| queue.waiting.remove(0));
        if let Some(surface) = &claimed {
            // A surface outlives its delivery while it waits for an answer, so
            // it is held here rather than dropped: the run is reported as
            // waiting for a planner decision until a reply arrives.
            queue.pending = surface.blocking.then(|| surface.clone());
        }
        self.write_queue(&queue)?;
        Ok(claimed)
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
