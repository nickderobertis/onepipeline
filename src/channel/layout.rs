//! The `planner-channel` layout: this crate's channel directory, declared as a
//! layout over the `onemessagebus` core's queues.
//!
//! A run keeps its planner channel in `runs/<run>/channel/`: the surfaces a
//! planner is asked about, the replies it writes, the command envelopes a
//! reconciler drains and what each was answered with. This module declares that
//! directory as the `planner-channel` [`Layout`] — four queues, their policies,
//! the channel's operations, and its one built-in author, the planner — so a
//! directory `onepipeline` 0.28.2 wrote is read by this build, and a directory
//! this build writes is read by 0.28.2.
//!
//! This is the engine's own copy of the channel module `onemessagebus-agent`
//! carried through its 0.7.0 line, brought here because the protocol is this
//! engine's rather than the bus's. Behaviour is copied, not
//! prose: the recorded directories under `tests/recorded/channel/` and the
//! 0.28.2 byte-compatibility journey (`tests/release_channel/`, `just release-compat`) are what
//! hold it to what that module did, and `tests/channel_layout.rs` re-applies
//! each recorded history through this layout byte for byte.
//!
//! | queue | policy | files |
//! | --- | --- | --- |
//! | [`SURFACES`] | held pending, blocking first, a waiting check-in superseded, projected | `surfaces.jsonl`, `queue.json` |
//! | [`REPLIES`] | plain, numbered, a claim passing over a commands-only reply | `replies.jsonl`, `replies-cursor.json` |
//! | [`COMMANDS`] | plain, numbered | `commands.jsonl`, `commands-cursor.json` |
//! | [`COMMAND_OUTCOMES`] | plain | `command-outcomes.jsonl` |
//!
//! Three rulings the bus's `docs/queues.md` recorded, each now this crate's and
//! stated in `docs/contract.md`:
//!
//! - **Supersede on `source`, not `kind`.** A waiting surface is superseded on
//!   **`source == check-in`**, because that is what 0.28.2's channel did: an
//!   observer's frame of kind `check-in` carries source `proposal` and is not
//!   superseded.
//! - **A bare reply is routed by its halves**: its commands to [`COMMANDS`], its
//!   verdict to [`REPLIES`], and an envelope carrying commands and no verdict to
//!   [`COMMANDS`] alone.
//! - **An edit envelope requires version 3**: a version this build reads is read
//!   at [`REPLY_ENVELOPE_VERSION`], and an edit envelope at any other is refused
//!   as `an edit envelope requires version 3`.

use std::sync::Arc;

use onemessagebus::{
    Allowlist, Asker, Author, ConsumerName, Correlation, Layout, Message, OpWord, Operation,
    Policy, Position, Predicate, Pushed, Queue, QueueError, QueueName, QueueSpec, Registry, Router,
    SchemaId, Supersede, Transport,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The layout's name, as a configuration's `profile` gives it.
pub const PLANNER_CHANNEL: &str = "planner-channel";

/// The queue of planner surfaces: `surfaces.jsonl`.
pub const SURFACES: &str = "surfaces";

/// The queue of replies: `replies.jsonl`.
pub const REPLIES: &str = "replies";

/// The queue of command envelopes: `commands.jsonl`.
pub const COMMANDS: &str = "commands";

/// The queue of command outcomes: `command-outcomes.jsonl`.
pub const COMMAND_OUTCOMES: &str = "command-outcomes";

/// The surfaces queue's projection document: `queue.json`.
pub const PROJECTION: &str = "queue.json";

/// The environment variable a serving session's asker is read from.
///
/// A serving process is a listener a side rents, and never that side itself: an
/// asker may raise one question through one session and wait for the verdict
/// through a succession of them. Two sessions carrying the same value are one
/// asker, and the later takes back over what the earlier left outstanding.
///
/// The value is **opaque and compared for equality only**. Every dispatch this
/// crate makes carries one of its own, composed in
/// `executor::prepare_dispatch_env`. A session carrying none listens on its own:
/// it adopts nothing and nothing adopts what it raised.
pub const ASKER_ENV: &str = "ONEPIPELINE_CHANNEL_ASKER";

/// The reply envelope version an edit envelope must be read at, and the version
/// this crate writes.
pub const REPLY_ENVELOPE_VERSION: u32 = 3;

/// Every reply envelope version read, newest first; each is read at
/// [`REPLY_ENVELOPE_VERSION`].
pub const REPLY_ENVELOPE_VERSIONS_READ: &[u32] = &[REPLY_ENVELOPE_VERSION, 2];

/// The reply envelope's schema family: `agent.reply-envelope`.
///
/// Registered by this layout at every version of [`REPLY_ENVELOPE_VERSIONS_READ`]
/// from the documents under `schemas/`, which are the documents the agent
/// profile registered for it: the shape is this crate's reply, and the profile
/// declared the family for this channel alone — nothing else of the profile
/// carries a reply envelope.
pub const REPLY_ENVELOPE_FAMILY: &str = "agent.reply-envelope";

/// `agent.planner-surface@1`: [`Surface`].
pub const SURFACE_SCHEMA: SchemaId = SchemaId::literal("agent", "planner-surface", 1);

/// `agent.queued-reply@1`: [`QueuedReply`].
pub const QUEUED_REPLY_SCHEMA: SchemaId = SchemaId::literal("agent", "queued-reply", 1);

/// `agent.queued-commands@1`: [`QueuedCommands`].
pub const QUEUED_COMMANDS_SCHEMA: SchemaId = SchemaId::literal("agent", "queued-commands", 1);

/// `agent.command-outcome@1`: [`CommandOutcome`].
pub const COMMAND_OUTCOME_SCHEMA: SchemaId = SchemaId::literal("agent", "command-outcome", 1);

const REPLY_ENVELOPE_V2: &str = include_str!("../../schemas/reply-envelope-v2.schema.json");
const REPLY_ENVELOPE_V3: &str = include_str!("../../schemas/reply-envelope-v3.schema.json");

/// What raised a surface.
///
/// A check-in update and a worker's proposal are the same wire shape and
/// different facts, so a journal reader can tell "nothing was sent" from
/// "updates were sent and nobody read them".
pub mod source {
    /// A durable progress timer came due; a newer check-in supersedes a waiting one.
    pub const CHECK_IN: &str = "check-in";
    /// A settled worker, an observer or the orchestrator raised advice.
    pub const PROPOSAL: &str = "proposal";
    /// The reconciler answered an edit it could not apply.
    pub const RECONCILER: &str = "reconciler";
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Read the asker a queue recorded, reading a name that names nobody as none.
///
/// The one lenient boundary in this layout, and the leniency is the point. A
/// surface is read out of a whole projection or out of one line of the log, and
/// a refusal here would refuse the record around it rather than one field: the
/// **whole queue** read as empty, or the surface dropped from the fold — either
/// way a surface lost, which is a far worse answer to a name this crate never
/// writes than simply not knowing whose it was.
// llmlint: ignore-block[boundary_inputs_validated] these two read fields of records this
// channel already holds rather than input at a boundary, and they read them as 0.28.2 and
// the bus's copy of this layout did: refusing a malformed asker or correlation would refuse
// the whole surface or queue around it, losing a question, where reading "nobody" loses one
// field. A malformed asker is refused where it enters, by the bus's own `Asker`
// constructor in the serving session that names it.
pub(super) fn recorded_asker<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Asker>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?
        .and_then(|name| Asker::new(&name, "the recorded asker").ok()))
}

/// Read the correlation a record carries, reading one that is not a correlation
/// as none, for the reason [`recorded_asker`] reads a blank asker as none.
pub(super) fn recorded_correlation<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Correlation>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.and_then(|text| text.parse().ok()))
}
// llmlint: ignore-end[boundary_inputs_validated]

/// One surface, as it sits in the durable queue.
///
/// The record the `surfaces` queue holds, registered as `agent.planner-surface@1`
/// and written in this field order — which is the order 0.28.2 wrote, and the
/// order the bus reshapes every line it writes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Surface {
    /// Monotonic within the run, so a consumer can report which one it read:
    /// allocated by the queue, one past the highest the log has queued.
    pub id: u64,
    /// What the surface is asking about.
    pub kind: String,
    /// Its text.
    pub message: String,
    /// What raised it — see [`source`].
    // llmlint: ignore[invalid_states_unrepresentable] 0.28.2's `Surface.source` is a `String` its writers fill with open producer words this crate does not own, so a closed enum here would refuse recorded projections this layout must continue to read byte-for-byte.
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
    /// Set by the channel's `abandon` when the process serving this surface
    /// exited without an answer, and lifted by its `attend` when a later
    /// listener of the same asker takes it back over — a listener ending is not
    /// the asker going. While it stands, the surface keeps its text and its
    /// place: what it gives up is its claim on the unread count, on the subtree
    /// a blocking surface holds, and on being reported as a question the run
    /// awaits a verdict on. Omitted from the wire while it is false, so a queue
    /// nothing has abandoned serializes exactly as it always did.
    #[serde(default, skip_serializing_if = "is_false")]
    pub abandoned: bool,
    /// Who raised it, when the session that did named an asker.
    ///
    /// The key the channel's `attend` matches on: a later session of the same
    /// asker takes this surface back over, and a session of any other asker
    /// leaves it exactly where it is. `None` is a surface nobody named an asker
    /// for — every one an older build wrote, and every one raised outside a
    /// serving session — and nothing ever adopts one of those. Omitted from the
    /// wire while it is absent, so a queue no asker was named on serializes
    /// exactly as it always did.
    #[serde(
        default,
        deserialize_with = "recorded_asker",
        skip_serializing_if = "Option::is_none"
    )]
    pub asker: Option<Asker>,
    /// The correlation a reply echoes to answer it, when it was raised as a
    /// question through the bus's `ask`. Omitted while absent, so a surface
    /// raised any other way is written byte for byte as 0.28.2 wrote it.
    #[serde(
        default,
        deserialize_with = "recorded_correlation",
        skip_serializing_if = "Option::is_none"
    )]
    pub correlation: Option<Correlation>,
}

impl Message for Surface {
    const SCHEMA: SchemaId = SURFACE_SCHEMA;
}

fn planner() -> Author {
    Author::from("planner")
}
fn is_planner(author: &Author) -> bool {
    author.as_str() == "planner"
}

/// A declared version this build reads is read as the version it writes; any
/// other is left as declared, so the refusal naming the version an edit
/// envelope requires is made where it always was.
fn read_at_a_version_this_build_reads<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u32>, D::Error> {
    Ok(Option::<u32>::deserialize(deserializer)?.map(|version| {
        if REPLY_ENVELOPE_VERSIONS_READ.contains(&version) {
            REPLY_ENVELOPE_VERSION
        } else {
            version
        }
    }))
}

/// One reply envelope as the layout reads it: a verdict, a list of graph edits,
/// or both.
///
/// Its commands are carried as JSON, in the order and with the fields their
/// author wrote: which ops exist and what each means is
/// [`channel::Command`](crate::channel::Command)'s, and the layout reads only
/// each command's `op`. The typed envelope the engine reconciles is
/// [`channel::Reply`](crate::channel::Reply).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplyEnvelope {
    /// The version it was written against, read at [`REPLY_ENVELOPE_VERSION`].
    #[serde(
        default,
        deserialize_with = "read_at_a_version_this_build_reads",
        skip_serializing_if = "Option::is_none"
    )]
    pub version: Option<u32>,
    /// Who wrote it. Omitted, the planner.
    #[serde(default = "planner", skip_serializing_if = "is_planner")]
    pub author: Author,
    /// The verdict: whether the author considers the run complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<bool>,
    /// The verdict's message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Why the author reached that verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The graph edits, each an object naming its `op`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<Map<String, Value>>,
}

impl Default for ReplyEnvelope {
    fn default() -> Self {
        Self {
            version: None,
            author: planner(),
            completion: None,
            message: None,
            reason: None,
            commands: Vec::new(),
        }
    }
}

impl ReplyEnvelope {
    /// Whether it carries a verdict half: any of `completion`, `message` and
    /// `reason`.
    #[must_use]
    pub fn carries_verdict(&self) -> bool {
        self.completion.is_some() || self.message.is_some() || self.reason.is_some()
    }

    /// Whether it carries edits and no verdict: the one shape with nothing in it
    /// for the reply queue.
    #[must_use]
    pub fn carries_edits_without_a_verdict(&self) -> bool {
        !self.commands.is_empty() && !self.carries_verdict()
    }
}

/// One reply as `replies.jsonl` holds it: the record the layout registers as
/// `agent.queued-reply@1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QueuedReply {
    /// The number of replies before it.
    pub id: u64,
    /// The envelope.
    pub reply: ReplyEnvelope,
    /// When it was written, in epoch milliseconds.
    pub at: u64,
    /// The correlation of the question it answers, when it answers one asked
    /// through the bus's `ask`. Omitted while absent.
    #[serde(
        default,
        deserialize_with = "recorded_correlation",
        skip_serializing_if = "Option::is_none"
    )]
    pub correlation: Option<Correlation>,
}

impl Message for QueuedReply {
    const SCHEMA: SchemaId = QUEUED_REPLY_SCHEMA;
}

/// One command envelope as `commands.jsonl` holds it: the record the layout
/// registers as `agent.queued-commands@1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QueuedCommands {
    /// The number of envelopes before it.
    pub id: u64,
    /// Who submitted it.
    #[serde(default = "planner")]
    pub author: Author,
    /// The commands, each an object naming its `op`.
    pub commands: Vec<Map<String, Value>>,
}

impl Message for QueuedCommands {
    const SCHEMA: SchemaId = QUEUED_COMMANDS_SCHEMA;
}

/// The four things that can become of one command of an envelope.
///
/// One field rather than a boolean and a sentence, because "not applied" was two
/// facts wearing one word: a command that was wrong and a command that was fine.
/// A reader that cannot tell them apart resends the wrong one and fixes the right
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CommandVerdict {
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

/// What became of **one** command of an envelope.
///
/// Every command is evaluated, whatever the ones before it said, so each entry is
/// that command's **own** answer. The envelope is still atomic — a refusal
/// anywhere in it applies none of it — and the four words of [`CommandVerdict`]
/// are what tells apart the facts a single boolean could not: which commands
/// were wrong, which were fine and went down with them, and which of those had
/// already been read by a conversation that cannot unread it. A manager reading
/// them knows which to fix, which to resend unchanged, and which not to resend
/// at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandResult {
    /// Where in the envelope's `commands` this one sat, from zero.
    pub index: usize,
    /// The command's op, as the envelope spelled it.
    ///
    /// Carried so an entry names the command it belongs to rather than leaving a
    /// reader to count positions in the envelope it sent.
    // llmlint: ignore[invalid_states_unrepresentable] 0.28.2's `CommandResult.op` is the command's op as the envelope spelled it, including an op the reconciler refused as unknown; a closed `Op` here would refuse the very outcome record that reports that refusal.
    pub op: String,
    /// What became of it.
    pub outcome: CommandVerdict,
    /// Why it refused, or what refused around it, when either happened.
    // llmlint: ignore[invalid_states_unrepresentable] the record `command-outcomes.jsonl` holds, in the shape 0.28.2 wrote and the recorded directories carry; folding `reason` into the verdict would change the bytes this layout must read and write exactly, and the reconciler that writes a result is where the pairing is enforced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The reconciler's answer to one submitted envelope, as
/// `command-outcomes.jsonl` holds it: the record the layout registers as
/// `agent.command-outcome@1`.
///
/// An envelope is all-or-nothing, so [`applied`](Self::applied) is still the
/// whole envelope's answer and every reader that predates
/// [`results`](Self::results) keeps reading exactly what it read. What that
/// boolean could never say is *which* command decided it, which is what left a
/// manager believing a node's bar had changed when the command that would have
/// changed it was never compiled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
// llmlint: ignore-block[invalid_states_unrepresentable] the record `command-outcomes.jsonl`
// holds, in the shape 0.28.2 wrote and the recorded directories carry: `applied` and
// `reason` are the whole envelope's answer every reader that predates `results` still
// reads, so they cannot become one tagged value without changing bytes this layout must
// read and write exactly. The reconciler that writes an outcome is where they agree.
pub struct CommandOutcome {
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
// llmlint: ignore-end[invalid_states_unrepresentable]

impl Message for CommandOutcome {
    const SCHEMA: SchemaId = COMMAND_OUTCOME_SCHEMA;
}

/// The planner channel's operations: [`channel::Command`](crate::channel::Command)'s
/// ops, as the allowlist names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Op {
    /// Add a node.
    Add,
    /// Remove a node.
    Drop,
    /// Replace an unstarted node's dependencies.
    Reparent,
    /// Supersede a node with a fresh lineage.
    Retry,
    /// Park a node.
    Cancel,
    /// Return a parked node to the frontier.
    Requeue,
    /// Journal the planner's completion request.
    Complete,
    /// Complete a waiting human action.
    Attest,
    /// Raise a finding to the planner.
    Finding,
    /// Amend what a node is judged against.
    Amend,
    /// Deliver a note into a node's dispatch.
    Note,
    /// Settle a node from evidence.
    Settle,
}

impl Op {
    /// Every op, in the order the contract lists them.
    pub const ALL: [Op; 12] = [
        Op::Add,
        Op::Drop,
        Op::Reparent,
        Op::Retry,
        Op::Cancel,
        Op::Requeue,
        Op::Complete,
        Op::Attest,
        Op::Finding,
        Op::Amend,
        Op::Note,
        Op::Settle,
    ];

    /// The op's wire word.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Op::Add => "add",
            Op::Drop => "drop",
            Op::Reparent => "reparent",
            Op::Retry => "retry",
            Op::Cancel => "cancel",
            Op::Requeue => "requeue",
            Op::Complete => "complete",
            Op::Attest => "attest",
            Op::Finding => "finding",
            Op::Amend => "amend",
            Op::Note => "note",
            Op::Settle => "settle",
        }
    }

    /// The op a wire word names.
    #[must_use]
    pub fn of_word(word: &str) -> Option<Op> {
        Op::ALL.into_iter().find(|op| op.word() == word)
    }
}

impl Operation for Op {
    fn name(&self) -> &str {
        self.word()
    }
}

/// The planner channel's allowlist: the planner is its only built-in author and
/// is granted every operation.
#[must_use]
pub fn allowlist() -> Allowlist<Op> {
    let planner = planner();
    let mut allowlist = Allowlist::new(Op::ALL);
    for op in Op::ALL {
        allowlist.grant(planner.clone(), op);
    }
    allowlist
}

/// Whether `author` may issue the op `word`, refused in this channel's words:
/// `'<op>' is not an op the <author> may issue: <reason>. Surface it to the
/// planner instead`.
///
/// # Errors
///
/// The refusal text, for an op the allowlist does not grant `author` or a word
/// that is no op of the channel.
pub fn allows<O: Operation>(
    allowlist: &Allowlist<O>,
    author: &Author,
    word: &str,
) -> Result<(), String> {
    let Some(op) = allowlist.vocabulary().iter().find(|op| op.name() == word) else {
        return Err(format!(
            "'{word}' is not an op of the planner channel; the ops are: {}",
            Op::ALL.map(Op::word).join(", ")
        ));
    };
    allowlist.allows(author, op).map_err(|refusal| {
        format!(
            "'{}' is not an op the {} may issue: {}. Surface it to the planner instead",
            refusal.op, refusal.author, refusal.reason
        )
    })
}

fn ensure_declared<O: Operation>(allowlist: &Allowlist<O>, author: &Author) -> Result<(), String> {
    if allowlist.declares(author) {
        return Ok(());
    }
    Err(format!(
        "the envelope's author `{author}` is not declared; the declared authors are: {}",
        allowlist
            .authors()
            .iter()
            .map(Author::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Whether `author` may declare the run finished through a verdict's
/// `completion: true` — the legacy spelling of `complete`, granted or refused as
/// that op is.
///
/// # Errors
///
/// The refusal text, for a verdict carrying `completion: true` from an author
/// not granted `complete`.
pub fn allows_completion<O: Operation>(
    allowlist: &Allowlist<O>,
    author: &Author,
    completion: Option<bool>,
) -> Result<(), String> {
    if completion != Some(true) {
        return Ok(());
    }
    let Some(complete) = allowlist
        .vocabulary()
        .iter()
        .find(|op| op.name() == Op::Complete.word())
    else {
        return Ok(());
    };
    allowlist.allows(author, complete).map_err(|refusal| {
        format!(
            "declaring the run complete is not something the {} may do: {}. Surface it to the planner instead",
            refusal.author, refusal.reason
        )
    })
}

fn name(text: &str) -> QueueName {
    QueueName::try_from(text).unwrap_or_else(|_| unreachable!("{text} is a queue name"))
}

fn field(text: &str) -> onemessagebus::FieldPath {
    text.parse()
        .unwrap_or_else(|_| unreachable!("{text} is a field path"))
}

/// A reply a claim on the reply queue hands out: anything but a commands-only
/// envelope, whose reader is the command queue.
fn claims_a_verdict() -> Predicate {
    let verdict = Predicate::Any(
        ["reply.completion", "reply.message", "reply.reason"]
            .into_iter()
            .map(|path| Predicate::Present {
                field: field(path),
                present: true,
            })
            .collect(),
    );
    Predicate::Not(Box::new(Predicate::All(vec![
        Predicate::NonEmpty {
            field: field("reply.commands"),
            non_empty: true,
        },
        Predicate::Not(Box::new(verdict)),
    ])))
}

/// The layout's four queues, as it declares them.
#[must_use]
pub fn queues() -> Vec<QueueSpec> {
    let mut surfaces = QueueSpec::new(
        name(SURFACES),
        Policy {
            // Supersedes on `source`, as 0.28.2 does: see the module's note.
            supersede_on: Some(Supersede {
                key: field("source"),
                when: Some(Predicate::equals(field("source"), source::CHECK_IN)),
            }),
            hold_pending: true,
            blocking_first: true,
            projection: Some(
                PROJECTION
                    .parse()
                    .unwrap_or_else(|_| unreachable!("queue.json is a document name")),
            ),
            ..Policy::default()
        },
    );
    surfaces.schema = Some(SURFACE_SCHEMA);
    surfaces.answers = Some(name(REPLIES));

    let mut replies = QueueSpec::new(name(REPLIES), Policy::default());
    replies.schema = Some(QUEUED_REPLY_SCHEMA);
    replies.claims = Some(claims_a_verdict());
    replies.numbered = true;

    let mut commands = QueueSpec::new(name(COMMANDS), Policy::default());
    commands.schema = Some(QUEUED_COMMANDS_SCHEMA);
    commands.numbered = true;

    let mut outcomes = QueueSpec::new(name(COMMAND_OUTCOMES), Policy::default());
    outcomes.schema = Some(COMMAND_OUTCOME_SCHEMA);

    vec![surfaces, replies, commands, outcomes]
}

/// Every schema the layout's queues name, beside every schema the agent
/// profile registers.
///
/// The layout's own come first — its four records from their types, and the
/// reply envelope family from the documents under `schemas/` — and the
/// profile's are added under every id the layout did not register, so an id the
/// profile still holds a document under is read by this crate's document. The
/// registry the profile builds is what `onepipeline::event` reads envelopes
/// through, and it is not touched.
///
/// # Panics
///
/// Never for this build's own documents: each is a type of this module or a
/// committed JSON Schema.
#[must_use]
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    registry
        .register::<Surface>()
        .expect("the planner surface schema registers");
    registry
        .register::<QueuedReply>()
        .expect("the queued reply schema registers");
    registry
        .register::<QueuedCommands>()
        .expect("the queued commands schema registers");
    registry
        .register::<CommandOutcome>()
        .expect("the command outcome schema registers");
    for (version, document) in [(2, REPLY_ENVELOPE_V2), (3, REPLY_ENVELOPE_V3)] {
        let schema: Value = serde_json::from_str(document).expect("a committed schema is JSON");
        registry
            .register_schema(
                SchemaId::literal("agent", "reply-envelope", version),
                schema,
            )
            .expect("the reply envelope schema registers");
    }
    let profile = onemessagebus_agent::registry();
    for id in profile.ids() {
        if registry.schema(&id).is_some() {
            continue;
        }
        let document = profile
            .schema(&id)
            .cloned()
            .unwrap_or_else(|| unreachable!("the profile holds every id it lists"));
        registry
            .register_schema(id, document)
            .expect("a schema the profile registered registers again");
    }
    registry
}

/// The `planner-channel` layout.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlannerChannel;

impl PlannerChannel {
    /// The records one reply envelope becomes: its commands on the command
    /// queue, and its verdict on the reply queue — a commands-only envelope is
    /// the command queue's alone. Checked against `allowlist` first: a verdict
    /// declaring the run complete as `complete` is, and each command's op.
    fn route_envelope<O: Operation>(
        envelope: Value,
        allowlist: &Allowlist<O>,
    ) -> Result<Vec<(QueueName, Value)>, String> {
        // Checked first: serde reads an array into a struct field by field, so
        // `[3]` would otherwise pass as an envelope of version 3.
        if !envelope.is_object() {
            return Err("the reply is malformed: a reply envelope is a JSON object".to_owned());
        }
        let envelope: ReplyEnvelope = serde_json::from_value(envelope)
            .map_err(|failure| format!("the reply is malformed: {failure}"))?;
        let author = envelope.author.clone();
        ensure_declared(allowlist, &author)?;
        allows_completion(allowlist, &author, envelope.completion)?;
        let mut routed = Vec::new();
        if !envelope.commands.is_empty() {
            if envelope.version != Some(REPLY_ENVELOPE_VERSION) {
                return Err(format!(
                    "an edit envelope requires version {REPLY_ENVELOPE_VERSION}"
                ));
            }
            for command in &envelope.commands {
                let word = command
                    .get("op")
                    .and_then(Value::as_str)
                    .ok_or("a command names its `op`")?;
                allows(allowlist, &author, word)?;
            }
            routed.push((
                name(COMMANDS),
                serde_json::json!({
                    "id": 0,
                    "author": envelope.author,
                    "commands": envelope.commands,
                }),
            ));
        }
        if envelope.carries_verdict() || envelope.commands.is_empty() {
            routed.push((
                name(REPLIES),
                serde_json::json!({ "id": 0, "reply": envelope, "at": crate::sys::now_millis() }),
            ));
        }
        Ok(routed)
    }
}

/// The planner channel's router: a reply offered to the reply queue is routed
/// by the halves it carries.
///
/// A bare envelope carrying a verdict and edits becomes its commands on
/// [`COMMANDS`] and its verdict on [`REPLIES`], so it reaches both the pending
/// ask and the command path; one carrying only commands reaches [`COMMANDS`]
/// alone, and leaves the pending ask standing. A reply already framed
/// (`{id, reply, at}`) is checked and kept whole. Every half is checked against
/// the allowlist first, and what the edits mean stays the reconciler's.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReplyRouter;

impl Router for ReplyRouter {
    fn route(
        &self,
        queue: &QueueName,
        record: Value,
        allowlist: &Allowlist<OpWord>,
    ) -> Result<Vec<(QueueName, Value)>, String> {
        match record.get("reply") {
            Some(envelope) => {
                PlannerChannel::route_envelope(envelope.clone(), allowlist)?;
                Ok(vec![(queue.clone(), record)])
            }
            None => PlannerChannel::route_envelope(record, allowlist),
        }
    }
}

impl Layout for PlannerChannel {
    fn name(&self) -> &str {
        PLANNER_CHANNEL
    }

    fn queues(&self) -> Vec<QueueSpec> {
        queues()
    }

    fn allowlist(&self) -> Allowlist<OpWord> {
        allowlist().words()
    }

    fn registry(&self) -> Registry {
        registry()
    }

    /// What the channel's writers stamp and check:
    ///
    /// - a surface is stamped `queued_at` now when it names none;
    /// - a reply offered as a bare envelope — to the reply queue, or as the
    ///   answer to a surface — is routed by its halves, checked against the
    ///   allowlist, and stamped `at` now; one offered already framed
    ///   (`{id, reply, at}`) is checked and kept whole;
    /// - a command envelope is checked op by op against its author.
    fn prepare(
        &self,
        queue: &QueueName,
        record: Value,
        allowlist: &Allowlist<OpWord>,
    ) -> Result<Vec<(QueueName, Value)>, String> {
        match queue.as_str() {
            SURFACES => {
                let Value::Object(fields) = record else {
                    return Ok(vec![(queue.clone(), record)]);
                };
                // What a question is `about` is the node it names, which a
                // surface carries as its `workstream`. Rebuilt rather than
                // taken out with `remove`, which reorders the record.
                let about = fields.get(onemessagebus::ask::ABOUT).cloned();
                let mut fields: Map<String, Value> = fields
                    .into_iter()
                    .filter(|(key, _)| key != onemessagebus::ask::ABOUT)
                    .collect();
                if let Some(about) = about {
                    fields.entry("workstream").or_insert(about);
                }
                fields
                    .entry("queued_at")
                    .or_insert_with(|| Value::from(crate::sys::now_millis()));
                Ok(vec![(queue.clone(), Value::Object(fields))])
            }
            REPLIES => ReplyRouter.route(queue, record, allowlist),
            COMMANDS => {
                let author: Author = record
                    .get("author")
                    .map(|author| serde_json::from_value(author.clone()))
                    .transpose()
                    .map_err(|failure| format!("the envelope's author: {failure}"))?
                    .unwrap_or_else(planner);
                ensure_declared(allowlist, &author)?;
                for command in record
                    .get("commands")
                    .and_then(Value::as_array)
                    .ok_or("a command envelope carries `commands`")?
                {
                    let word = command
                        .get("op")
                        .and_then(Value::as_str)
                        .ok_or("a command names its `op`")?;
                    allows(allowlist, &author, word)?;
                }
                Ok(vec![(queue.clone(), record)])
            }
            _ => Ok(vec![(queue.clone(), record)]),
        }
    }
}

/// The planner channel over one transport, through its typed queues.
///
/// What a program writing or reading a channel directory outside a run's
/// engine drives: the byte-compatibility journey writes one through it, and a
/// host serving the channel may open it the same way.
#[derive(Debug, Clone)]
pub struct Channel {
    surfaces: Queue<Surface>,
    replies: Queue<QueuedReply>,
    commands: Queue<QueuedCommands>,
    outcomes: Queue<CommandOutcome>,
}

impl Channel {
    /// The channel kept on `transport`.
    ///
    /// # Errors
    ///
    /// A record type whose schema cannot be registered.
    pub fn open(transport: &Arc<dyn Transport>) -> Result<Self, QueueError> {
        let mut specs = queues().into_iter();
        let mut next = || specs.next().unwrap_or_else(|| unreachable!("four queues"));
        Ok(Self {
            surfaces: Queue::open(Arc::clone(transport), next())?,
            replies: Queue::open(Arc::clone(transport), next())?,
            commands: Queue::open(Arc::clone(transport), next())?,
            outcomes: Queue::open(Arc::clone(transport), next())?,
        })
    }

    /// The surfaces queue.
    #[must_use]
    pub fn surfaces(&self) -> &Queue<Surface> {
        &self.surfaces
    }

    /// The reply queue.
    #[must_use]
    pub fn replies(&self) -> &Queue<QueuedReply> {
        &self.replies
    }

    /// The command queue.
    #[must_use]
    pub fn commands(&self) -> &Queue<QueuedCommands> {
        &self.commands
    }

    /// The command outcome queue.
    #[must_use]
    pub fn outcomes(&self) -> &Queue<CommandOutcome> {
        &self.outcomes
    }

    /// Queue a surface; the queue allocates its id.
    ///
    /// # Errors
    ///
    /// A queue failure.
    pub fn push(&self, surface: &Surface) -> Result<Pushed<Surface>, QueueError> {
        self.surfaces.push(surface)
    }

    /// Claim the next surface, a blocking one first.
    ///
    /// # Errors
    ///
    /// A queue failure.
    pub fn claim(&self) -> Result<Option<Surface>, QueueError> {
        Ok(self
            .surfaces
            .claim(&ConsumerName::default_consumer())?
            .map(|claimed| claimed.record))
    }

    /// The surface waiting for an answer, abandoned ones passed over.
    ///
    /// # Errors
    ///
    /// A queue failure.
    pub fn pending(&self) -> Result<Option<Surface>, QueueError> {
        Ok(self
            .surfaces
            .pending(&ConsumerName::default_consumer())?
            .map(|claimed| claimed.record))
    }

    /// Mark the surfaces in `raised` abandoned.
    ///
    /// # Errors
    ///
    /// A queue failure.
    pub fn abandon(&self, raised: &[u64]) -> Result<Vec<Surface>, QueueError> {
        self.typed_surfaces(self.surfaces.raw().abandon(raised)?)
    }

    /// Take back what `asker` abandoned.
    ///
    /// # Errors
    ///
    /// A queue failure.
    pub fn attend(&self, asker: &Asker) -> Result<Vec<Surface>, QueueError> {
        self.typed_surfaces(self.surfaces.raw().attend(asker)?)
    }

    fn typed_surfaces(&self, records: Vec<Value>) -> Result<Vec<Surface>, QueueError> {
        records
            .into_iter()
            .map(|record| {
                serde_json::from_value(record).map_err(|failure| QueueError::Shape {
                    queue: name(SURFACES),
                    why: failure.to_string(),
                })
            })
            .collect()
    }

    /// Record that `reply` answered whatever was pending — the slot released
    /// first, then the reply appended, so a reader that finds the reply finds
    /// the slot already released — and answer the reply's id.
    ///
    /// # Errors
    ///
    /// A queue failure.
    pub fn answer(&self, reply: &ReplyEnvelope, at: u64) -> Result<u64, QueueError> {
        self.surfaces.raw().answer_pending()?;
        let pushed = self.replies.push(&QueuedReply {
            id: 0,
            reply: reply.clone(),
            at,
            correlation: None,
        })?;
        Ok(pushed.id.unwrap_or_default())
    }

    /// Answer the pending surface with `reply` when it carries a verdict; a
    /// commands-only envelope answers nothing.
    ///
    /// # Errors
    ///
    /// A queue failure.
    pub fn answer_if_verdict(
        &self,
        reply: &ReplyEnvelope,
        at: u64,
    ) -> Result<Option<u64>, QueueError> {
        if reply.carries_verdict() {
            return self.answer(reply, at).map(Some);
        }
        Ok(None)
    }

    /// Claim the next reply no reader has taken, passing over a commands-only
    /// envelope.
    ///
    /// # Errors
    ///
    /// A queue failure.
    pub fn claim_reply(&self) -> Result<Option<QueuedReply>, QueueError> {
        Ok(self
            .replies
            .claim(&ConsumerName::default_consumer())?
            .map(|claimed| claimed.record))
    }

    /// Append one command envelope; answers its id.
    ///
    /// # Errors
    ///
    /// A queue failure.
    pub fn submit(
        &self,
        author: Author,
        commands: Vec<Map<String, Value>>,
    ) -> Result<u64, QueueError> {
        let pushed = self.commands.push(&QueuedCommands {
            id: 0,
            author,
            commands,
        })?;
        Ok(pushed.id.unwrap_or_default())
    }

    /// Claim every command envelope the reconciler has not drained.
    ///
    /// # Errors
    ///
    /// A queue failure.
    pub fn claim_commands(&self) -> Result<Vec<QueuedCommands>, QueueError> {
        let mut claimed = Vec::new();
        while let Some(envelope) = self.commands.claim(&ConsumerName::default_consumer())? {
            claimed.push(envelope.record);
        }
        Ok(claimed)
    }

    /// Answer one claimed envelope.
    ///
    /// # Errors
    ///
    /// A queue failure.
    pub fn answer_commands(&self, outcome: &CommandOutcome) -> Result<Position, QueueError> {
        Ok(self.outcomes.push(outcome)?.position)
    }

    /// The reconciler's answer to envelope `id`, if it has given one.
    ///
    /// # Errors
    ///
    /// A queue failure.
    pub fn outcome_of(&self, id: u64) -> Result<Option<CommandOutcome>, QueueError> {
        Ok(self
            .outcomes
            .raw()
            .log(None)?
            .into_iter()
            .filter_map(|(record, _)| serde_json::from_value::<CommandOutcome>(record).ok())
            .find(|outcome| outcome.id == id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ids this layout registers are the ids the linked profile registered
    /// for the channel through its 0.7.0 line, so a record validates against
    /// the same id whichever build wrote it — and where the profile still holds
    /// a document under one, the two admit the same records.
    #[test]
    fn the_layout_registers_the_channels_ids_beside_the_profiles() {
        let own = registry();
        let profile = onemessagebus_agent::registry();
        let channel_ids = [
            SURFACE_SCHEMA,
            QUEUED_REPLY_SCHEMA,
            QUEUED_COMMANDS_SCHEMA,
            COMMAND_OUTCOME_SCHEMA,
            SchemaId::literal("agent", "reply-envelope", 2),
            SchemaId::literal("agent", "reply-envelope", 3),
        ];
        for id in &channel_ids {
            assert!(own.schema(id).is_some(), "{id} is not registered");
        }
        for id in profile.ids() {
            assert!(
                own.schema(&id).is_some(),
                "the profile's {id} is not registered beside the layout's"
            );
        }
        let surface = serde_json::json!({
            "id": 3, "kind": "finding", "message": "m", "source": "proposal",
            "blocking": false, "queued_at": 1, "asker": "a"
        });
        own.check(&SURFACE_SCHEMA, &surface)
            .expect("a surface validates against the layout's document");
        if profile.schema(&SURFACE_SCHEMA).is_some() {
            profile
                .check(&SURFACE_SCHEMA, &surface)
                .expect("the same surface validates against the profile's document");
        }
    }

    /// A version this build reads is read at the version it writes, and any
    /// other is left as declared for the edit-envelope refusal to name.
    #[test]
    fn a_read_version_is_carried_to_the_written_one_and_an_unread_one_is_left() {
        for version in REPLY_ENVELOPE_VERSIONS_READ {
            let envelope: ReplyEnvelope =
                serde_json::from_value(serde_json::json!({"version": version}))
                    .expect("an envelope at a read version parses");
            assert_eq!(envelope.version, Some(REPLY_ENVELOPE_VERSION));
        }
        let envelope: ReplyEnvelope = serde_json::from_value(serde_json::json!({"version": 1}))
            .expect("an envelope at an unread version still parses");
        assert_eq!(envelope.version, Some(1));
        assert!(!envelope.carries_verdict());
        assert!(!envelope.carries_edits_without_a_verdict());
    }

    /// The committed reply envelope documents are held to the two Rust types
    /// that read an envelope — the layout's [`ReplyEnvelope`] and the engine's
    /// [`channel::Reply`](crate::channel::Reply) — so neither can grow, lose or
    /// retype a field the documents do not say, nor a document an op field no
    /// command carries.
    #[test]
    fn the_reply_envelope_documents_are_what_the_envelope_types_read() {
        let generated = [
            schemars::schema_for!(ReplyEnvelope).to_value(),
            schemars::schema_for!(crate::channel::Reply).to_value(),
        ];
        // What a property is on the wire: its JSON type with the `null` an
        // `Option` adds taken out, or the reference it names.
        let wire = |property: &Value| -> Value {
            match &property["type"] {
                Value::Array(types) => {
                    let kept: Vec<&Value> = types.iter().filter(|t| *t != "null").collect();
                    serde_json::json!(kept[0])
                }
                Value::Null => property["$ref"].clone(),
                other => other.clone(),
            }
        };
        let command = &generated[1]["$defs"]["Command"]["oneOf"];
        let op_fields: std::collections::BTreeSet<&str> = command
            .as_array()
            .expect("the command schema is a union of its ops")
            .iter()
            .flat_map(|op| op["properties"].as_object().into_iter().flatten())
            .map(|(field, _)| field.as_str())
            .collect();
        for (version, document) in [(2, REPLY_ENVELOPE_V2), (3, REPLY_ENVELOPE_V3)] {
            let document: Value = serde_json::from_str(document).expect("a committed schema");
            assert_eq!(document["properties"]["version"]["const"], version);
            assert_eq!(document["additionalProperties"], false);
            let properties = document["properties"].as_object().expect("properties");
            for schema in &generated {
                let theirs = schema["properties"].as_object().expect("properties");
                assert_eq!(
                    properties.keys().collect::<Vec<_>>(),
                    theirs.keys().collect::<Vec<_>>(),
                    "version {version}'s document names other fields than {}",
                    schema["title"]
                );
                for (field, property) in properties {
                    if field == "author" || field == "version" {
                        continue;
                    }
                    let theirs = wire(&theirs[field]);
                    let ours = wire(property);
                    assert!(
                        ours == theirs,
                        "version {version}'s `{field}` is {ours}, and {} reads {theirs}",
                        schema["title"]
                    );
                }
            }
            let fields = document["$defs"]["Command"]["properties"]
                .as_object()
                .expect("the command's fields");
            for field in fields.keys() {
                assert!(
                    op_fields.contains(field.as_str()),
                    "version {version}'s document names a command field `{field}` no op carries"
                );
            }
        }
    }
}
