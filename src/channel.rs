//! The planner channel: the wire shapes, and the durable queue behind them.
//!
//! A reply is one JSON envelope: a legacy verdict, a version-1 list of graph
//! edits, or both. The edits' required fields and validation semantics are
//! `ai-orchestrator`'s live-edit protocol exactly, per `docs/contract.md`.
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
    /// Whether the run is waiting on the answer. A non-blocking surface does
    /// not stop the graph frontier.
    pub blocking: bool,
    /// The round it was written for. One that outlives its round is discarded
    /// rather than kept consumable: it describes work in a round that has
    /// finished.
    pub round: u64,
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

    /// Claim the next readable surface for the active round.
    ///
    /// A surface that outlives the round it was written for is **discarded**,
    /// not kept consumable: it describes work in a round that has finished, and
    /// the check-in that replaces it describes the round actually running.
    pub fn claim(&self, active_round: u64) -> crate::Result<Option<Surface>> {
        let mut queue = self.queue();
        let mut claimed = None;
        while !queue.waiting.is_empty() {
            let candidate = queue.waiting.remove(0);
            if candidate.round != 0 && active_round != 0 && candidate.round != active_round {
                continue;
            }
            claimed = Some(candidate);
            break;
        }
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

    /// Every reply the planner has written, in order.
    pub fn replies(&self) -> Vec<QueuedReply> {
        crate::ledger::read_lines(&self.paths.channel("replies.jsonl"))
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Claim the replies no reader has taken yet.
    ///
    /// A reply is claimed from the durable queue by whichever reader reaches it
    /// next, each claim advancing the cursor, so one reply reaches exactly one
    /// reader and no reader can lose it.
    pub fn claim_replies(&self) -> crate::Result<Vec<QueuedReply>> {
        let cursor_path = self.paths.channel("replies-cursor.json");
        let claimed_through: u64 = crate::ledger::read_json_opt(&cursor_path).unwrap_or(0);
        let fresh: Vec<QueuedReply> = self
            .replies()
            .into_iter()
            .filter(|reply| reply.id >= claimed_through)
            .collect();
        if let Some(last) = fresh.last() {
            crate::ledger::write_json(&cursor_path, &(last.id + 1))?;
        }
        Ok(fresh)
    }

    /// Append one envelope of edits to the durable command queue.
    pub fn submit(&self, commands: &[Command]) -> crate::Result<u64> {
        let path = self.paths.channel("commands.jsonl");
        let id = crate::ledger::read_lines(&path).len() as u64;
        let queued = QueuedCommands {
            id,
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
