//! The live-edit reconciler: validating one delta against the live frontier,
//! and compiling it into the mutations the journal records.
//!
//! Every edit is **applied or rejected with a reason**, and the reason is the
//! one the reconciler would give — [`compile`] is the single validator both the
//! submission check and the reconciler run, so a planner is never told an edit
//! was accepted that the reconciler then quietly drops.
//!
//! An accepted delta compiles to a list of [`Operation`]s recorded as one
//! `edit-committed` event carrying both the submitted command and its compiled
//! operations. Replay therefore sees all of a delta's mutations or none of them,
//! and reconstructs the graph from what was actually submitted.

use std::collections::{BTreeMap, BTreeSet};

use onevcs::releases::TargetName;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::channel::{Command, Dependents};
use crate::error::{Error, Result};
use crate::graph::{self, Graph, NodeStatus};
use crate::plan::Node;

/// One compiled mutation inside an atomic edit commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Operation {
    /// A node joined the graph.
    NodeAdded {
        /// The node's full definition.
        node: Box<Node>,
        /// The node this one supersedes, when it came from a `retry`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_of: Option<String>,
    },
    /// A dependency edge was added.
    EdgeAdded {
        /// The dependency.
        from: String,
        /// The dependent.
        to: String,
        /// The release target the dependent consumes this dependency at, where
        /// it states one.
        ///
        /// Carried because `consumes` is keyed by **dependency node id**, so an
        /// edge that moves moves the target with it — and replay reconstructs
        /// `deps` from these operations, so a target the reconciler rekeyed and
        /// this record did not carry would leave the projected graph differing
        /// from the executing one. Omitted where the edge has no target, which
        /// is every edge in every record written before this field existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<TargetName>,
    },
    /// A dependency edge was removed.
    ///
    /// Takes the dependent's `consumes` entry keyed on that dependency with it:
    /// there is no dependency left for a target to name, and `validate_node`
    /// refuses a key that is not a dep.
    EdgeRemoved {
        /// The dependency.
        from: String,
        /// The dependent.
        to: String,
    },
    /// A node left the graph.
    NodeDropped {
        /// The node.
        node: String,
        /// What was asked of its dependents.
        dependents: Dependents,
    },
    /// A node's dependencies were replaced wholesale.
    Reparent {
        /// The node.
        node: String,
        /// Its dependencies before.
        from: Vec<String>,
        /// Its dependencies after.
        to: Vec<String>,
    },
    /// A node was superseded by a fresh lineage.
    RetryRequested {
        /// The superseded node.
        node: String,
        /// The replacement's id.
        replacement: String,
        /// The dependents whose derived state the replacement resets.
        reset: Vec<String>,
    },
    /// A node was parked by a planner `cancel`.
    NodeParked {
        /// The node.
        node: String,
    },
    /// A parked node returned to the desired frontier.
    NodeRequeued {
        /// The node.
        node: String,
        /// The overrides merged onto it. Omitted when there were none, so
        /// "amended nothing" and "amended with nothing" are one record.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        amend: Option<Map<String, Value>>,
    },
    /// A human action was completed.
    HumanAttested {
        /// The action's reference.
        node: String,
    },
    /// The planner asked for completion, independently of graph mutation.
    CompletionRequested {
        /// Why.
        reason: String,
    },
    // llmlint: ignore-block[invalid_states_unrepresentable] all three fields are spelled
    // as the wire and every neighbouring variant already spell them, and both narrowable
    // ones are narrowed where they are judged: `compile_finding` refuses a node the graph
    // does not have and a message that is blank.
    /// A finding was raised to the planner, without touching the graph.
    FindingRaised {
        /// The node it is about, when it named one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<String>,
        /// The finding's text.
        message: String,
        /// Whether the planner's answer holds the node's subtree back.
        blocking: bool,
    },
    // llmlint: ignore-end[invalid_states_unrepresentable]
    // llmlint: ignore-block[invalid_states_unrepresentable] both fields are spelled as the
    // wire spells them, as every neighbouring variant is, and the narrowable one is
    // narrowed where it is judged: `compile_amend` refuses a node the graph does not have
    // and a ruling that is blank, and `graph::validate_node` refuses a blank one arriving
    // in a plan file. A newtype here would have to deserialize a record this build did not
    // write, which is the one place the invariant cannot hold.
    /// A node's effective task gained a binding amendment.
    ///
    /// Its own operation rather than a `requeue`-shaped merge, so replaying a
    /// run's journal reconstructs the amended task without re-judging the
    /// amendment — and so a reader of the record can see what a node was told,
    /// and when, after a later amendment has replaced it.
    TaskAmended {
        /// The node whose bar this moves — a node the graph still holds and can
        /// still dispatch, which is what `compile_amend` established.
        node: String,
        /// The binding text, which **replaces** whatever the node carried.
        text: String,
    },
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// A planner note reached a node.
    ContextAdded {
        /// The node.
        node: String,
        /// The note.
        note: String,
        /// Whether it went into the node's running turn or onto its next
        /// dispatch. Absent from a record written before delivery had modes,
        /// which is [`Delivery::Deferred`] — the only thing those records did.
        #[serde(default)]
        delivery: Delivery,
    },
    // llmlint: ignore-block[invalid_states_unrepresentable] `node` is the `String` every
    // neighbouring variant spells a node id with, narrowed where it is judged by
    // `compile_note`; the note's own two narrowable values are **not** strings here — the
    // addressee is a closed enum and the text and criterion are the seam's validated
    // newtypes, so a record this build did not write is refused by their own conversions.
    /// A manager's note was delivered into a node's live conversation.
    ///
    /// Recorded for what only this event says: a note is delivered to whichever
    /// party is live, so *which* party took it is the answer a reader of the run
    /// has no second source for. It mutates nothing — the note went into a
    /// conversation rather than onto the graph — so replay reconstructs it by
    /// changing nothing, exactly as the reconciler did.
    NoteDelivered {
        /// The node whose conversation took it.
        node: String,
        /// Whose task it said it was updating.
        addressee: crate::note::Addressee,
        /// What that party read.
        text: crate::note::NoteText,
        /// The criterion it bound, when it bound one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criterion: Option<crate::note::Criterion>,
        /// Which party of the conversation actually took it.
        #[serde(flatten)]
        reached: crate::note::Reached,
    },
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// How a planner note actually reached its node.
///
/// The fact `edit-committed` records, and what tells replay whether the note is
/// still owed to a later dispatch: a note the running turn took has been read,
/// so carrying it onto the node's next dispatch would repeat a correction the
/// worker has already acted on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// Into the turn that was running when the edit arrived.
    Live,
    /// Onto the node's next dispatch.
    #[default]
    Deferred,
}

/// What the reconciler knows about the run when it judges an edit.
#[derive(Debug, Clone, Default)]
pub struct Frontier {
    /// The statuses the journal actually recorded. A node absent from this map
    /// has not started, which is what `reparent` and `cancel` test for.
    pub recorded: BTreeMap<String, NodeStatus>,
    /// The human actions already attested.
    pub attestations: BTreeSet<String>,
    /// The dispatches the loop still has running, by node.
    ///
    /// Not derivable from [`recorded`](Self::recorded), which is the whole
    /// reason it is carried: a `cancel` parks the node the moment it is
    /// committed and the dispatch it cancelled goes on running until it stops
    /// itself, so the journal says `parked` while a process still holds the
    /// node's workspace. Only the loop knows this, so only the loop fills it in;
    /// a caller judging an edit from the ledger alone leaves it empty and the
    /// reconciler is where the refusal lands.
    pub in_flight: BTreeMap<String, LiveDispatch>,
    // llmlint: ignore-block[invalid_states_unrepresentable] the validator stays the
    // `String` the launch record carries, for the reason `LaunchRecord`'s own graph
    // references do: this is a **durable record field** read strictly at the start of the
    // pass, and the record is the schema. A newtype could add exactly one invariant —
    // non-blank — which `LaunchConfig::load` and `start`'s own resolution already refuse
    // at the trust boundary, so it would carry no invariant of its own. The property a
    // command actually has to have is that it *runs*, which nothing but running it can
    // establish; `offer_to_validator` establishes it, and fails closed where it does not.
    /// The command this run's launch named to check a node before it joins the
    /// graph, when it named one.
    ///
    /// A launch that named none leaves this empty and every edit is judged
    /// exactly as it was before this field existed. The rules a validator
    /// applies are the **consuming host's** — which acceptance criteria name a
    /// property rather than a procedure, which review bar a node's own task must
    /// answer — and none of them are this crate's to hold: a plan file has been
    /// checked by one all along, and what was missing is that a node introduced
    /// by a live edit reached a dispatch having been checked by nothing.
    pub node_validator: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// One dispatch the loop still has in flight, as an edit sees it.
///
/// Enough to *name* the thing an edit has to wait for: a refusal that said only
/// "it is still running" leaves a supervisor looking for something to look at.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LiveDispatch {
    /// The `oneagentgraph` run carrying it, once its stream has named one.
    /// Absent before the dispatch has said anything, which is also when there is
    /// nothing to address.
    pub graph_run: Option<String>,
    /// How long it has been running.
    pub running_for_seconds: u64,
}

impl LiveDispatch {
    /// The dispatch as a refusal names it.
    fn named(&self) -> String {
        let run = match &self.graph_run {
            Some(run) => format!("graph run '{run}'"),
            // Nothing has named a turn yet, which is a dispatch that has started
            // and not spoken rather than no dispatch at all.
            None => "a graph run it has not yet named".to_string(),
        };
        format!(
            "{run}, running for {}",
            crate::telemetry::duration(self.running_for_seconds * 1_000)
        )
    }
}

/// Validate one command against the live frontier and compile its mutations.
///
/// The graph is mutated in place only on success: a command that fails
/// validation leaves it exactly as it was, so a rejected edit in the middle of
/// an envelope cannot half-apply.
pub fn compile(
    graph: &mut Graph,
    frontier: &Frontier,
    command: &Command,
) -> Result<Vec<Operation>> {
    compile_with(graph, frontier, command, Delivery::Deferred)
}

/// [`compile`], for a caller that has already delivered the note.
///
/// Only a `context` command reads `delivery`, and only the reconciler has an
/// answer for it: whether a note reached the running turn is the outcome of an
/// `oneagentgraph interrupt`, not a judgement about the graph. Every other
/// caller — the submission check most of all — validates the same command
/// without pulling that lever, which is why the delivery is a parameter rather
/// than something this module works out.
pub fn compile_with(
    graph: &mut Graph,
    frontier: &Frontier,
    command: &Command,
    delivery: Delivery,
) -> Result<Vec<Operation>> {
    // Validate against a copy, so a refusal partway through a multi-edge
    // mutation cannot leave the caller's graph in a state nothing submitted.
    let mut candidate = graph.clone();
    let operations = compile_into(&mut candidate, frontier, command, delivery)?;
    if !matches!(
        command,
        Command::Complete { .. } | Command::Attest { .. } | Command::Finding { .. }
    ) {
        // The resulting graph must still satisfy the normal plan schema.
        let plan = candidate.to_plan(&crate::plan::Plan {
            schema_version: crate::plan::PLAN_SCHEMA_VERSION,
            goal: None,
            name: None,
            concurrency: candidate.concurrency,
            tasks: Vec::new(),
        });
        graph::validate_edited(&plan).map_err(|e| Error::Refused(e.to_string()))?;
    }
    // Last, and over the node the edit actually produced: the host's own rules
    // are the expensive check and the specific one, so a node this crate's own
    // schema would refuse never reaches them.
    if let Some(node) = node_whose_task_is_new(command, &candidate) {
        offer_to_validator(frontier.node_validator.as_deref(), command, node)?;
    }
    *graph = candidate;
    Ok(operations)
}

/// The node whose task one command puts in front of a dispatch, once the command
/// has been compiled against the candidate graph.
///
/// Four ops reach a validator, and they are exactly the ones that put task prose
/// in front of a dispatch that nothing has checked: `add` and `retry` introduce
/// a node, `amend` changes the bar an existing one is judged against, and a
/// `requeue` is only one of them when its amendment touches `task` — a requeue
/// that raises a turn budget changes nothing a validator has an opinion about.
/// Every other op moves edges, parks, attests, or reports.
fn node_whose_task_is_new<'a>(command: &Command, graph: &'a Graph) -> Option<&'a Node> {
    let id = match command {
        Command::Add { node } | Command::Retry { node, .. } => node.id.as_str(),
        Command::Amend { id, .. } => id.as_str(),
        Command::Requeue { id, amend } => {
            amend.as_ref().filter(|a| a.contains_key("task"))?;
            id.as_str()
        }
        _ => return None,
    };
    graph.get(id)
}

/// How much of a hook's stderr reaches the refusal it becomes.
///
/// A hook is an external program and its stderr is **external input**: it is
/// read into this process, rendered into a refusal, surfaced to the planner,
/// and written to the journal, where every payload text this crate writes is
/// already bounded. So it is bounded on the way in rather than after it has been
/// held whole — a hook that printed a gigabyte would otherwise be a gigabyte in
/// the memory of every `reply` that ran it.
const MAX_HOOK_STDERR: u64 = crate::event::MAX_PAYLOAD_TEXT_BYTES as u64;

/// What one hook answered, once it has been run to completion.
struct HookAnswer {
    /// How it ended. This, and nothing else, is what decides the edit.
    status: std::process::ExitStatus,
    /// What it said on stderr, bounded on the way in and lossily decoded.
    stderr: Vec<u8>,
}

impl HookAnswer {
    /// The hook's own words, as a refusal carries them.
    ///
    /// Control characters stripped and the whole thing kept to one line: this
    /// reaches a terminal, a planner's queue, and the journal, and a hook that
    /// emitted escape sequences would be writing into all three. A hook that
    /// said nothing is never reported silently either — a refusal nobody can act
    /// on is the failure these hooks exist to end, so the exit code is at least
    /// something to look at.
    fn reason(&self) -> String {
        self.reason_from(&String::from_utf8_lossy(&self.stderr))
    }

    /// The same, over one part of what it said.
    ///
    /// The envelope reviewer lifts the lines a hook declared its objection on
    /// out of its stderr before quoting the rest, so the reason a refusal
    /// carries is the reviewer's own sentence rather than that sentence with a
    /// declaration read back in front of it. Everything else is identical, the
    /// status fallback included: a hook whose every line was a declaration said
    /// nothing a reader can act on, which is the case that fallback is for.
    fn reason_from(&self, said: &str) -> String {
        let said = crate::views::one_line(said).trim().to_string();
        if !said.is_empty() {
            return said;
        }
        format!(
            "it exited {} and said nothing on stderr",
            self.status
                .code()
                .map_or_else(|| "without a status".to_string(), |code| code.to_string())
        )
    }
}

/// Why a hook gave no answer at all, which is never an acceptance.
enum HookFailure {
    /// It could not be started: a launch configured wrongly.
    NotStarted(std::io::Error),
    /// It started and this process could not collect it.
    NotCollected(std::io::Error),
}

/// Run one hook over one document and wait for its answer.
///
/// The mechanics both hooks share, so the two of them cannot drift into
/// answering differently: the document crosses on the hook's stdin, its stdout
/// goes nowhere — this runs inside `reply`, whose own stdout is the JSON verdict
/// its caller parses — and its stderr is read [`MAX_HOOK_STDERR`] and no
/// further. What each hook makes of the answer is its caller's, because only the
/// caller knows what it was asking about.
fn ask_hook(hook: &str, document: &str) -> std::result::Result<HookAnswer, HookFailure> {
    let mut child = std::process::Command::new(hook)
        .stdin(std::process::Stdio::piped())
        // Never inherited and never held: a hook's narration is not the caller's
        // answer, and this process's own stdout is a parsed verdict.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(HookFailure::NotStarted)?;
    if let Some(mut stdin) = child.stdin.take() {
        // A hook that refuses without reading its input is answering, not
        // failing, and the broken pipe that leaves here is not what decides the
        // edit — the exit status below is.
        use std::io::Write;
        let _ = stdin.write_all(document.as_bytes());
    }
    // Bounded on the way in, and the rest of the pipe drained so the child is
    // never left blocked on a reader that stopped reading.
    let mut stderr = Vec::new();
    if let Some(pipe) = child.stderr.take() {
        use std::io::Read;
        let mut bounded = pipe.take(MAX_HOOK_STDERR);
        if let Err(e) = bounded.read_to_end(&mut stderr) {
            // Neither fatal nor silent. The status below is what decides the
            // edit, so a stderr this process could not finish reading is a
            // refusal carrying less of the hook's trace and never an acceptance
            // — and the manager is told the trace is short rather than left
            // reading a truncation as all the hook said.
            stderr.extend_from_slice(format!(" [its stderr stopped early: {e}]").as_bytes());
        }
        // Draining is not a read: it is here so the child is never left blocked
        // on a reader that stopped reading, and a pipe that fails it is one that
        // is already gone — which is the state draining is for.
        let _ = std::io::copy(&mut bounded.into_inner(), &mut std::io::sink());
    }
    // llmlint: ignore[changed_behavior_has_e2e] this arm is this process failing to
    // collect a child it started — an I/O failure of the parent, which no journey can
    // provoke without breaking the harness that runs it. It is here so the failure is
    // reported rather than read as a verdict, which is the same fail-closed rule the
    // spawn above is driven for end to end.
    let status = child.wait().map_err(HookFailure::NotCollected)?;
    Ok(HookAnswer { status, stderr })
}

/// Offer one node to the validator this run's launch named, if it named one.
///
/// The node crosses as JSON on the validator's stdin — the same document a plan
/// file states it in, so a host that already checks plan files reads one shape.
/// Exit 0 accepts the edit; a non-zero exit refuses it carrying the validator's
/// own stderr, because the rules are the host's and only it can say which one
/// this node broke. That stderr is external input and is treated as such: read
/// to [`MAX_HOOK_STDERR`] and no further, and stripped of control characters,
/// because it is about to be a refusal on a terminal, a surface in a planner's
/// queue, and a line in the journal. Its stdout goes nowhere at all: this runs
/// inside `reply`, whose own stdout is the JSON verdict its caller parses.
///
/// **Fails closed.** A validator that cannot be run is a launch configured
/// wrongly; accepting the edit anyway would decide that an unenforced rule is no
/// rule, silently, on the path a manager reaches for under pressure.
///
/// An accepted edit is offered **twice**, by the submission check and again by
/// the reconciler, because [`compile`] is the one validator both run. Asking a
/// read-only check twice asks the same question; a refused edit is asked once.
fn offer_to_validator(validator: Option<&str>, command: &Command, node: &Node) -> Result<()> {
    let Some(validator) = validator.filter(|command| !command.trim().is_empty()) else {
        return Ok(());
    };
    let op = crate::channel::op_of(command);
    let document = serde_json::to_string(node)
        .map_err(|e| refuse(format!("{op}: node '{}' does not serialize: {e}", node.id)))?;
    let answer = ask_hook(validator, &document).map_err(|failure| match failure {
        HookFailure::NotStarted(e) => refuse(format!(
            "{op}: the node validator '{validator}' this run was launched with could not \
             be started ({e}), so node '{}' was checked by nothing and the edit was not \
             applied",
            node.id
        )),
        HookFailure::NotCollected(e) => refuse(format!(
            "{op}: the node validator '{validator}' did not answer for node '{}' ({e}), so the \
             edit was not applied",
            node.id
        )),
    })?;
    if answer.status.success() {
        return Ok(());
    }
    Err(refuse(format!(
        "{op}: the node validator refused node '{}': {}",
        node.id,
        answer.reason()
    )))
}

/// One node an envelope introduces or changes, as the reviewer is handed it.
///
/// The op is carried beside the node because the same node reads differently
/// depending on how it got there: a node an `add` introduced is prose nothing
/// has checked, and the same node under an `amend` is a bar that was moved on a
/// node already in flight.
// llmlint: ignore-block[invalid_states_unrepresentable] the op is a `&'static str` because
// it is **produced** rather than accepted: every value comes from `channel::op_of`, which
// is `Command`'s own discriminant already spelled for the wire and published as this
// crate's one word for an op. An enum here could hold no value that one does not, and
// would be a second vocabulary to keep in step with the first.
#[derive(Debug, Serialize)]
struct ChangedNode<'a> {
    op: &'static str,
    /// Not the node the command carried: the one the whole envelope leaves
    /// behind. Two commands that touched one node are two entries, under the op
    /// each carried, and both show the node as it will be dispatched.
    node: &'a Node,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// The document the envelope reviewer reads on its stdin.
///
/// Everything a plan-quality review needs and no per-command check can carry:
/// the goal the run is judged against, every node this one envelope introduces
/// or changes with the op that produced each, and the plan they are being edited
/// into — as the envelope leaves it, so a reviewer sees the graph the run would
/// actually converge on rather than one it has to assemble. Which nodes are the
/// edit and which are its context is [`changes`](Self::changes) rather than a
/// diff the reviewer works out.
///
/// The goal is hoisted out of the plan it is also part of, because it is what
/// the whole envelope is judged against: a reviewer reading this document should
/// not have to know the plan schema to find the one sentence stating what the
/// run is for.
#[derive(Debug, Serialize)]
struct EnvelopeUnderReview<'a> {
    /// What the run is for, in the launching plan's own words. Omitted when the
    /// plan states none, which is what a plan with no `goal` already means.
    #[serde(skip_serializing_if = "Option::is_none")]
    goal: Option<&'a str>,
    /// Every node this envelope introduces or changes, in the order the
    /// envelope wrote them. Empty for an envelope that only drops, parks, or
    /// asks for completion — those change the plan without putting new prose in
    /// front of a dispatch, and the plan below is where the reviewer sees it.
    changes: Vec<ChangedNode<'a>>,
    plan: crate::plan::Plan,
}

/// The node one command introduces or changes, once the envelope has been
/// compiled against the candidate graph.
///
/// A wider set than [`node_whose_task_is_new`]'s four, and deliberately: that
/// one answers *whose task prose is unchecked*, which is the per-node question,
/// while this one answers *what this envelope did to the graph*. So a `reparent`
/// is here — it changes the edges a whole-plan review is about — and so is a
/// `requeue` carrying any amendment, whose changed turn budget is part of the
/// node the reviewer reads even though no per-node rule has an opinion about it.
/// A `drop` names no node here because the node it names is gone from the plan
/// below, which is where a review sees it; `cancel`, `context`, `attest`,
/// `complete`, and `finding` leave every node's definition exactly as it was.
fn node_the_command_changes<'a>(command: &Command, graph: &'a Graph) -> Option<&'a Node> {
    let id = match command {
        Command::Add { node } | Command::Retry { node, .. } => node.id.as_str(),
        Command::Amend { id, .. } | Command::Reparent { id, .. } => id.as_str(),
        Command::Requeue { id, amend } => {
            amend.as_ref()?;
            id.as_str()
        }
        _ => return None,
    };
    graph.get(id)
}

/// Offer one whole envelope to the reviewer this run's launch named, if it
/// named one.
///
/// The seam the per-node validator cannot reach. That one is handed a single
/// node serialized on its own, so a reply carrying several related ops is seen
/// as several unrelated nodes: nothing checks two added nodes that duplicate
/// each other, a contract seam *between* two nodes of one edit, the dependency
/// edges the edit introduces, or whether the edited graph still delivers the
/// run's goal. Those are the checks a plan-quality reviewer makes over a whole
/// plan, and this is the invocation that can carry them —
/// [`EnvelopeUnderReview`] is what crosses its stdin.
///
/// Exit 0 accepts the envelope. A non-zero exit refuses it **whole**: this runs
/// before any of its commands is compiled into the journal, so no command of a
/// refused envelope half-applies. The refusal carries the reviewer's own words,
/// bounded and control-stripped exactly as the per-node validator's are, and
/// names two sets that are not the same one: the node the reviewer **objected
/// to**, declared on an [`OBJECTION_PREFIX`] line of its stderr, and every op
/// and node the envelope **carried**. An envelope is no longer one command, so
/// what it carried is not what the reviewer turned down, and a reader given only
/// the first still cannot tell which node to go and change. A reviewer that
/// declared no
/// node is reported as having declared none, rather than having the whole
/// envelope read back as its objection — those are different facts about a
/// refusal and a reader acts differently on each.
///
/// **Fails closed**, for the reason the per-node validator does: a reviewer that
/// cannot be started is a launch configured wrongly, and letting the envelope
/// through would decide silently that an unenforced rule is no rule.
///
/// An accepted envelope is offered **once**, and this is the difference from the
/// per-node validator, which is offered an accepted edit twice. Three reasons,
/// and they point the same way. Asking twice is only free for a read-only script;
/// this hook exists for a review no deterministic check can make, so the host
/// answering it is plausibly an agent, and a second offer is a second bill for
/// one question. The submission check is the only place a refusal can still be
/// **whole** — the reconciler applies an envelope's commands one at a time and
/// stops at the first refusal, so a reviewer consulted there would be answering
/// about edits that are already committed. And it is the one door: every
/// envelope carrying commands reaches the durable queue through this check, so
/// once here is once per envelope rather than once per path.
// llmlint: ignore-block[invalid_states_unrepresentable] the reviewer stays the
// `Option<&str>` `offer_to_validator` takes beside it, and for the same reason: what a
// launch record holds is a `String`, and *blank means this launch names none* is a rule of
// the rungs rather than a state to be made unrepresentable — `driver::start` applies it
// once for all three, `LaunchConfig::load` refuses a blank key outright, and this filter is
// the last of the three rather than a reinterpretation of a value that got past them.
pub(crate) fn offer_envelope_to_reviewer(
    reviewer: Option<&str>,
    commands: &[Command],
    edited: &Graph,
    launched_with: Option<&crate::plan::Plan>,
) -> Result<()> {
    let Some(reviewer) = reviewer.filter(|command| !command.trim().is_empty()) else {
        return Ok(());
    };
    // The launching plan's own fields, for the two a graph does not carry. A run
    // whose ledger has no plan — one launched before that was recorded — still
    // gets a review of its edited graph, with the goal stated as absent rather
    // than invented.
    let source = launched_with.cloned().unwrap_or_else(|| crate::plan::Plan {
        schema_version: crate::plan::PLAN_SCHEMA_VERSION,
        goal: None,
        name: None,
        concurrency: edited.concurrency,
        tasks: Vec::new(),
    });
    let under_review = EnvelopeUnderReview {
        goal: source.goal.as_ref().map(|goal| goal.text.as_str()),
        changes: commands
            .iter()
            .filter_map(|command| {
                node_the_command_changes(command, edited).map(|node| ChangedNode {
                    op: crate::channel::op_of(command),
                    node,
                })
            })
            .collect(),
        plan: edited.to_plan(&source),
    };
    let document = serde_json::to_string(&under_review)
        .map_err(|e| refuse(format!("this envelope does not serialize: {e}")))?;
    let answer = ask_hook(reviewer, &document).map_err(|failure| match failure {
        HookFailure::NotStarted(e) => refuse(format!(
            "the envelope reviewer '{reviewer}' this run was launched with could not be \
             started ({e}), so this envelope was reviewed by nothing and none of its edits \
             were applied"
        )),
        HookFailure::NotCollected(e) => refuse(format!(
            "the envelope reviewer '{reviewer}' did not answer ({e}), so none of this \
             envelope's edits were applied"
        )),
    })?;
    if answer.status.success() {
        return Ok(());
    }
    // What the reviewer declared it objected to, held against the names this
    // envelope actually carries — the same set `carried` prints below, so a node
    // the refusal points at is one a reader finds again in the list beside it.
    let said = String::from_utf8_lossy(&answer.stderr);
    let objection = Objection::read(&said);
    let envelope_named: BTreeSet<String> = commands
        .iter()
        .filter_map(crate::channel::target_of)
        .collect();
    Err(refuse(format!(
        "the envelope reviewer refused this envelope{}, so none of its edits were applied — \
         it carried {}: {}",
        objection.against(&envelope_named),
        carried(commands),
        answer.reason_from(&objection.said)
    )))
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// What the envelope held, as its refusal names it: every op in it with the node
/// that op is about.
///
/// Not what crossed the reviewer's stdin, which is a narrower thing:
/// [`EnvelopeUnderReview`] lists only the nodes the envelope changes, and a
/// `drop` or a `cancel` reaches the reviewer as the plan it leaves behind rather
/// than as an op of its own. Every op is named here anyway, because a refusal is
/// read by somebody looking for what to change and an envelope's `drop` is as
/// much a reason to refuse it as its `add` is.
fn carried(commands: &[Command]) -> String {
    commands
        .iter()
        .map(|command| {
            let op = crate::channel::op_of(command);
            match crate::channel::target_of(command) {
                Some(id) => format!("{op} '{id}'"),
                None => op.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The line prefix a reviewer names the node it objected to on.
///
/// The refusal has to say *which* node, and only the reviewer can: it read the
/// whole envelope and the objection is its own reasoning. So it declares the
/// node on a line of its stderr reading `objection: cover` — one line per node,
/// matched case-insensitively, anywhere in what it says, and repeatable for an
/// objection that is about a seam between two of them.
///
/// A prefix on the stream a hook already has, rather than a second channel: this
/// crate's half of the answer is an exit status and prose, its stdout is not
/// available — `reply`'s own stdout is a parsed verdict — and a JSON answer
/// would make every host's reviewer a serializer to say one node's name. A hook
/// that declares nothing is not refused for it either; it is reported as having
/// declared nothing, which is what the shell scripts written against this hook
/// before the line existed do.
const OBJECTION_PREFIX: &str = "objection:";

/// What a reviewer declared it objected to, lifted out of what it said.
#[derive(Debug)]
struct Objection {
    /// The names it declared, in the order it declared them and none twice.
    /// Empty for a reviewer that declared none, which is a fact about the
    /// refusal rather than a reason to invent one.
    named: Vec<String>,
    /// Everything else it said: its own sentence, with the declarations taken
    /// out so a refusal does not read the same name back in front of it.
    said: String,
}

impl Objection {
    /// Read one reviewer's declarations out of its stderr.
    fn read(stderr: &str) -> Self {
        let mut named: Vec<String> = Vec::new();
        let mut said: Vec<&str> = Vec::new();
        for line in stderr.lines() {
            let trimmed = line.trim();
            let declared = trimmed
                .get(..OBJECTION_PREFIX.len())
                .filter(|start| start.eq_ignore_ascii_case(OBJECTION_PREFIX))
                .map(|_| &trimmed[OBJECTION_PREFIX.len()..]);
            let Some(declared) = declared else {
                said.push(line);
                continue;
            };
            // A declaration is external input on its way to a terminal, a
            // planner's queue, and the journal, so it is control-stripped where
            // the reviewer's prose beside it is. One that names nothing declares
            // nothing — it is still lifted out of the prose, and a reviewer
            // whose every declaration was blank named no node, which the refusal
            // says outright rather than pointing at an empty name.
            let name = crate::views::one_line(declared).trim().to_string();
            if !name.is_empty() && !named.contains(&name) {
                named.push(name);
            }
        }
        Self {
            named,
            said: said.join("\n"),
        }
    }

    /// How a refusal names what the reviewer objected to, against the names the
    /// envelope put in front of it.
    ///
    /// Three different facts, and a reader acts differently on each: a node this
    /// envelope changes is one to go and fix, a name it does not carry is a
    /// reviewer pointing somewhere else — at a node already in the plan, or at
    /// nothing — and no declaration at all is a refusal whose target is simply
    /// unstated. Reporting the first for the third by listing every node the
    /// envelope carried is the failure this whole line exists to end.
    fn against(&self, envelope_named: &BTreeSet<String>) -> String {
        let (changed, elsewhere): (Vec<&String>, Vec<&String>) = self
            .named
            .iter()
            .partition(|name| envelope_named.contains(*name));
        let unknown = |names| {
            listed("the name", names)
                .map(|named| format!("{named}, which no node this envelope changes goes by"))
        };
        match (listed("node", &changed), unknown(&elsewhere)) {
            (Some(changed), Some(elsewhere)) => format!(" over {changed}, and over {elsewhere}"),
            (Some(changed), None) => format!(" over {changed}"),
            (None, Some(elsewhere)) => format!(" over {elsewhere}"),
            (None, None) => " without declaring the node it objected to".to_string(),
        }
    }
}

/// One list of names as a refusal says it — `node 'a'`, `nodes 'a', 'b'` — and
/// `None` for no names at all, which is a clause the sentence leaves out rather
/// than an empty one it keeps.
fn listed(noun: &str, names: &[&String]) -> Option<String> {
    let (first, rest) = names.split_first()?;
    let plural = match rest.is_empty() {
        true => String::new(),
        false => "s".to_string(),
    };
    let quoted = std::iter::once(first)
        .chain(rest)
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("{noun}{plural} {quoted}"))
}

fn compile_into(
    graph: &mut Graph,
    frontier: &Frontier,
    command: &Command,
    delivery: Delivery,
) -> Result<Vec<Operation>> {
    match command {
        Command::Add { node } => compile_add(graph, node),
        Command::Drop { id, dependents } => compile_drop(graph, frontier, id, *dependents),
        Command::Reparent { id, deps } => compile_reparent(graph, frontier, id, deps),
        Command::Retry { id, node } => compile_retry(graph, frontier, id, node),
        Command::Cancel { id } => compile_cancel(graph, frontier, id),
        Command::Requeue { id, amend } => compile_requeue(graph, frontier, id, amend.as_ref()),
        Command::Attest { reference } => compile_attest(frontier, reference),
        Command::Complete { reason } => Ok(vec![Operation::CompletionRequested {
            reason: reason.clone(),
        }]),
        Command::Context { id, note, .. } => compile_context(graph, frontier, id, note, delivery),
        Command::Amend { id, text } => compile_amend(graph, frontier, id, text),
        Command::Note {
            id,
            addressee,
            text,
            criterion,
        } => compile_note(graph, id, *addressee, text, criterion.as_ref()),
        Command::Finding {
            message,
            blocking,
            id,
        } => compile_finding(graph, id.as_deref(), message, *blocking),
    }
}

fn refuse(what: impl Into<String>) -> Error {
    Error::Refused(what.into())
}

fn compile_add(graph: &mut Graph, node: &Node) -> Result<Vec<Operation>> {
    if graph.contains(&node.id) {
        return Err(refuse(format!("add: node '{}' already exists", node.id)));
    }
    graph::validate_node(node).map_err(|e| refuse(e.to_string()))?;
    let mut operations = vec![Operation::NodeAdded {
        node: Box::new(node.clone()),
        retry_of: None,
    }];
    for dep in &node.deps {
        operations.push(Operation::EdgeAdded {
            from: dep.clone(),
            to: node.id.clone(),
            target: node.consumes.get(dep).cloned(),
        });
    }
    graph.insert(node.clone());
    Ok(operations)
}

fn compile_reparent(
    graph: &mut Graph,
    frontier: &Frontier,
    id: &str,
    deps: &[String],
) -> Result<Vec<Operation>> {
    let Some(node) = graph.get(id) else {
        return Err(refuse(format!("reparent: no node '{id}'")));
    };
    if frontier.recorded.contains_key(id) {
        return Err(refuse(format!("reparent: node '{id}' has already started")));
    }
    let previous = node.deps.clone();
    let consumes = node.consumes.clone();
    let mut operations: Vec<Operation> = previous
        .iter()
        .map(|dep| Operation::EdgeRemoved {
            from: dep.clone(),
            to: id.to_string(),
        })
        .collect();
    operations.extend(deps.iter().map(|dep| Operation::EdgeAdded {
        from: dep.clone(),
        to: id.to_string(),
        // The dep survived the reparent, so the target this node stated for it
        // survives too — the edge is the same edge, moved nowhere.
        target: consumes.get(dep).cloned(),
    }));
    operations.push(Operation::Reparent {
        node: id.to_string(),
        from: previous,
        to: deps.to_vec(),
    });
    if let Some(node) = graph.get_mut(id) {
        node.deps = deps.to_vec();
        // A target for a dep the new list no longer carries names nothing. It
        // is dropped rather than kept, because `validate_node` refuses a
        // `consumes` key that is not a dep — and inventing one for a new dep
        // would apply a target the plan's author never wrote.
        node.consumes
            .retain(|dep, _| node.deps.iter().any(|d| d == dep));
    }
    Ok(operations)
}

fn compile_drop(
    graph: &mut Graph,
    frontier: &Frontier,
    id: &str,
    dependents: Dependents,
) -> Result<Vec<Operation>> {
    let Some(target) = graph.get(id).cloned() else {
        return Err(refuse(format!("drop: no node '{id}'")));
    };
    let direct = graph.dependents_of(id);

    // A lifecycle node is its repository's publication anchor. Removing the last
    // one while an unresolved dependent still targets that repository would cut
    // that dependent's branch from its root.
    if let Some(repo) = &target.repo {
        let unresolved_same_identity = direct.iter().any(|dependent| {
            graph.get(dependent).and_then(|n| n.repo.as_ref()) == Some(repo)
                && frontier.recorded.get(dependent) != Some(&NodeStatus::Done)
        });
        let alternative_anchor = graph.iter().any(|node| {
            node.id != id
                && node.repo.as_ref() == Some(repo)
                && frontier.recorded.get(&node.id) == Some(&NodeStatus::Done)
        });
        if unresolved_same_identity && !alternative_anchor {
            return Err(refuse(
                "drop: would remove the last unresolved publication anchor",
            ));
        }
    }

    let mut operations = Vec::new();
    let mut removed: BTreeSet<String> = BTreeSet::from([id.to_string()]);
    match dependents {
        Dependents::Drop => {
            let mut pending = direct;
            while let Some(candidate) = pending.pop() {
                if !removed.insert(candidate.clone()) {
                    continue;
                }
                pending.extend(graph.dependents_of(&candidate));
            }
        }
        Dependents::Detach => {
            for dependent in &direct {
                if let Some(node) = graph.get_mut(dependent) {
                    node.deps.retain(|dep| dep != id);
                    // The dependency is gone, so a target naming it names
                    // nothing — and `validate_node` refuses a `consumes` key
                    // that is not a dep, which is what left this edit refusable
                    // in every order an operator could try.
                    node.consumes.remove(id);
                }
                operations.push(Operation::EdgeRemoved {
                    from: id.to_string(),
                    to: dependent.clone(),
                });
            }
        }
    }
    for dropped in &removed {
        graph.remove(dropped);
        operations.push(Operation::NodeDropped {
            node: dropped.clone(),
            dependents,
        });
    }
    Ok(operations)
}

fn compile_retry(
    graph: &mut Graph,
    frontier: &Frontier,
    id: &str,
    replacement: &Node,
) -> Result<Vec<Operation>> {
    let Some(target) = graph.get(id).cloned() else {
        return Err(refuse(format!("retry: no node '{id}'")));
    };
    match frontier.recorded.get(id) {
        Some(NodeStatus::Running | NodeStatus::Failed | NodeStatus::Cancelled) => {}
        _ => {
            return Err(refuse(format!(
                "retry: node '{id}' is not running, failed, or cancelled"
            )))
        }
    }
    if replacement.id.trim().is_empty() {
        return Err(refuse("retry: the replacement needs a non-empty id"));
    }
    if graph.contains(&replacement.id) {
        return Err(refuse(format!(
            "retry: replacement id '{}' must be new",
            replacement.id
        )));
    }
    let replacement = pin_retry_branch(inherit_preserved_branch(
        validate_retry_pin(replacement)?,
        &target,
    ));
    graph::validate_node(&replacement).map_err(|e| refuse(e.to_string()))?;

    let mut replacement = replacement;
    if replacement.deps.is_empty() {
        replacement.deps = target.deps.clone();
        // On the same condition and in the same way: a replacement that states
        // no dependencies of its own consumes them at the targets the node it
        // supersedes stated. `validate_node` above already refused a
        // replacement that named targets without naming the deps they key on,
        // so there is nothing of its own here to overwrite.
        replacement.consumes.clone_from(&target.consumes);
    }

    let direct = graph.dependents_of(id);
    let mut reset: BTreeSet<String> = direct.iter().cloned().collect();
    let mut pending: Vec<String> = direct.clone();
    while let Some(predecessor) = pending.pop() {
        for dependent in graph.dependents_of(&predecessor) {
            if reset.insert(dependent.clone()) {
                pending.push(dependent);
            }
        }
    }

    let mut operations = vec![
        Operation::RetryRequested {
            node: id.to_string(),
            replacement: replacement.id.clone(),
            reset: reset.into_iter().collect(),
        },
        Operation::NodeAdded {
            node: Box::new(replacement.clone()),
            retry_of: Some(id.to_string()),
        },
    ];
    for dep in &replacement.deps {
        operations.push(Operation::EdgeAdded {
            from: dep.clone(),
            to: replacement.id.clone(),
            target: replacement.consumes.get(dep).cloned(),
        });
    }
    graph.insert(replacement.clone());
    for dependent in &direct {
        // `consumes` is keyed by dependency node id, so an edge rewired onto
        // the replacement takes the target keyed on the superseded id with it.
        // Rekeyed rather than re-derived: the value is the one the plan or an
        // accepted edit stated, carried across unchanged.
        let mut target = None;
        if let Some(node) = graph.get_mut(dependent) {
            for dep in &mut node.deps {
                if dep == id {
                    dep.clone_from(&replacement.id);
                }
            }
            target = node.consumes.remove(id);
            if let Some(target) = target.clone() {
                node.consumes.insert(replacement.id.clone(), target);
            }
        }
        operations.push(Operation::EdgeRemoved {
            from: id.to_string(),
            to: dependent.clone(),
        });
        operations.push(Operation::EdgeAdded {
            from: replacement.id.clone(),
            to: dependent.clone(),
            target,
        });
    }
    // The superseded node leaves the graph, exactly as a `drop` would take it:
    // its dependents are already rewired onto the replacement, and its work is
    // the replacement's now. Left in, it would hold the whole run in `waiting`
    // for a node nothing will ever dispatch again — a graph that can never
    // settle because something was retried.
    graph.remove(id);
    operations.push(Operation::NodeDropped {
        node: id.to_string(),
        dependents: Dependents::Detach,
    });
    Ok(operations)
}

/// A retry may name only one branch, and it gets that branch every time.
///
/// A replacement carrying both a `branch` pin and a `resume` checkpoint that
/// name different branches is answering "which branch does this work live on?"
/// twice, and the lifecycle honours the checkpoint. The planner would not get
/// the branch it named, so the disagreement is refused at submission rather than
/// resolved silently.
fn validate_retry_pin(node: &Node) -> Result<Node> {
    if let (Some(branch), Some(resume)) = (&node.branch, &node.resume) {
        if &resume.branch != branch {
            return Err(refuse(format!(
                "retry: replacement '{}' pins branch '{branch}' but resumes branch '{}'; \
                 a retry may name only one branch",
                node.id, resume.branch
            )));
        }
    }
    Ok(node.clone())
}

/// A replacement that names no branch of its own continues the one the node it
/// supersedes left behind.
///
/// The superseded node's attempt ran, committed, and stopped, so its branch
/// holds work. A replacement that cut a fresh branch beside it would retry the
/// publication against an empty tree and leave the committed work for a person
/// to find. The pin is on the node because the fold put it there when the
/// attempt settled — see `projection::pin_preserved_branch` — so this reads what
/// the run recorded rather than guessing a name.
///
/// A planner who named either field is answered with what they named: naming a
/// branch is a decision somebody made after reading the result.
fn inherit_preserved_branch(mut node: Node, superseded: &Node) -> Node {
    if node.branch.is_some() || node.resume.is_some() {
        return node;
    }
    node.branch.clone_from(&superseded.branch);
    node.resume.clone_from(&superseded.resume);
    node
}

/// A retry that states a `resume` and no `branch` is pinned to the resume's own
/// branch, because naming a continuation is naming the branch it lives on.
fn pin_retry_branch(mut node: Node) -> Node {
    if node.branch.is_none() {
        if let Some(resume) = &node.resume {
            if !resume.branch.is_empty() {
                node.branch = Some(resume.branch.clone());
            }
        }
    }
    node
}

fn compile_cancel(graph: &mut Graph, frontier: &Frontier, id: &str) -> Result<Vec<Operation>> {
    let Some(node) = graph.get(id) else {
        return Err(refuse(format!("cancel: no node '{id}'")));
    };
    // The definition is the authoritative record of parking, not the frontier: a
    // node parked before it was ever dispatched has nothing journalled about it
    // there, so a frontier lookup alone would let the same node be parked twice.
    if node.parked {
        return Err(refuse(format!("cancel: node '{id}' is already parked")));
    }
    match frontier.recorded.get(id) {
        None | Some(NodeStatus::Running) => {}
        Some(status) => {
            return Err(refuse(format!(
                "cancel: node '{id}' is {}, not pending or running",
                status.as_str()
            )))
        }
    }
    if let Some(node) = graph.get_mut(id) {
        node.parked = true;
    }
    Ok(vec![Operation::NodeParked {
        node: id.to_string(),
    }])
}

fn compile_requeue(
    graph: &mut Graph,
    frontier: &Frontier,
    id: &str,
    amend: Option<&Map<String, Value>>,
) -> Result<Vec<Operation>> {
    let Some(node) = graph.get(id).cloned() else {
        return Err(refuse(format!("requeue: no node '{id}'")));
    };
    if !node.parked {
        return Err(refuse(format!("requeue: node '{id}' is not parked")));
    }
    // Parked is not the same as stopped. A `cancel` parks the node and *asks*
    // its dispatch to end; until that dispatch settles it still holds the
    // node's workspace, so a requeue accepted here returns the node to the
    // frontier where it waits on the occupancy lease its own predecessor holds,
    // with nothing said about why. That is the state right after every cancel,
    // and it is refused rather than accepted-and-stuck.
    if let Some(live) = frontier.in_flight.get(id) {
        return Err(refuse(format!(
            "requeue: node '{id}' still has a dispatch in flight ({}); a cancel asks that \
             dispatch to stop rather than stopping it, so wait for the node to settle and \
             requeue it then",
            live.named()
        )));
    }
    // `id` names the node being requeued and `deps` is `reparent`'s to change;
    // letting an amendment rewrite either would make one op silently do the work
    // of another, with no separate record of the rewiring.
    if let Some(amend) = amend {
        let forbidden: Vec<&str> = ["id", "deps"]
            .into_iter()
            .filter(|key| amend.contains_key(*key))
            .collect();
        if !forbidden.is_empty() {
            return Err(refuse(format!(
                "requeue: cannot amend {}: use 'add' or 'reparent' for that",
                forbidden.join(", ")
            )));
        }
    }

    let mut merged =
        serde_json::to_value(&node).map_err(|e| refuse(format!("requeue: node '{id}': {e}")))?;
    if let (Some(object), Some(amend)) = (merged.as_object_mut(), amend) {
        for (key, value) in amend {
            object.insert(key.clone(), value.clone());
        }
    }
    if let Some(object) = merged.as_object_mut() {
        object.remove("parked");
    }
    // A retired field can only have come from the amendment — the node's own
    // serialization no longer carries one — and it is named here rather than
    // reaching the schema as an unknown field, because a retry that carries a
    // corrected review bar is exactly where a planner writes one.
    if let Some(retired) = crate::plan::retired_field_refusal(&merged) {
        return Err(refuse(format!("requeue: node {retired}")));
    }
    // The amended node is validated as the node it produces, so a malformed pin
    // is refused at submission rather than at the next dispatch.
    let amended: Node = serde_json::from_value(merged)
        .map_err(|e| refuse(format!("requeue: amended node '{id}' is invalid: {e}")))?;
    graph::validate_node(&amended).map_err(|e| refuse(e.to_string()))?;
    graph.insert(amended);

    Ok(vec![Operation::NodeRequeued {
        node: id.to_string(),
        amend: amend.filter(|a| !a.is_empty()).cloned(),
    }])
}

/// Validate one finding: it changes nothing, so all there is to judge is
/// whether it says something about a node this graph has.
///
/// A finding naming a node the graph does not carry is refused rather than
/// queued about nothing. That matters most when it is blocking: the subtree it
/// would hold is derived from the node it names, so a name nothing matches holds
/// nothing back while still reading, to every planner view, as a decision the
/// run is waiting on.
fn compile_finding(
    graph: &Graph,
    id: Option<&str>,
    message: &str,
    blocking: bool,
) -> Result<Vec<Operation>> {
    if message.trim().is_empty() {
        return Err(refuse(
            "a finding carries what was found: this one has an empty message",
        ));
    }
    if let Some(node) = id {
        if !graph.contains(node) {
            return Err(refuse(format!(
                "cannot raise a finding about node '{node}', which this run does not have; \
                 it has: {}",
                graph.ids().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
    }
    Ok(vec![Operation::FindingRaised {
        node: id.map(str::to_string),
        message: message.to_string(),
        blocking,
    }])
}

/// Compile an `attest`, whose accepted settlements are open divergence 36's —
/// argued and sourced there, not here.
fn compile_attest(frontier: &Frontier, reference: &str) -> Result<Vec<Operation>> {
    // Before the settlement check, because an attestation folds the node to
    // `done`: asked the other way round, a second one is answered with the
    // reference this op does not take rather than with what it needs to hear.
    if frontier.attestations.contains(reference) {
        return Err(refuse(format!(
            "attest: '{reference}' was already attested"
        )));
    }
    if !matches!(
        frontier.recorded.get(reference),
        Some(NodeStatus::Waiting | NodeStatus::Failed)
    ) {
        return Err(refuse(format!(
            "attest: '{reference}' is not a ready, waiting human action, nor a node \
             that settled failed; attest accepts one of those two references"
        )));
    }
    Ok(vec![Operation::HumanAttested {
        node: reference.to_string(),
    }])
}

fn compile_context(
    graph: &mut Graph,
    frontier: &Frontier,
    id: &str,
    note: &str,
    delivery: Delivery,
) -> Result<Vec<Operation>> {
    if !graph.contains(id) {
        return Err(refuse(format!("context: no node '{id}'")));
    }
    if note.trim().is_empty() {
        return Err(refuse("context: the note cannot be empty"));
    }
    // A note is read by the node's running turn or by a *later* dispatch of it.
    // A node that already settled `done` has neither, so the planner is told
    // rather than left believing it landed.
    if frontier.recorded.get(id) == Some(&NodeStatus::Done) {
        return Err(refuse(format!(
            "context: node '{id}' has settled done, so nothing will read the note"
        )));
    }
    // A note the running turn took has been read, so it is not also owed to the
    // next dispatch: attaching it there too would re-state a correction the
    // worker has already acted on.
    if delivery == Delivery::Deferred {
        if let Some(node) = graph.get_mut(id) {
            node.context = Some(note.to_string());
        }
    }
    Ok(vec![Operation::ContextAdded {
        node: id.to_string(),
        note: note.to_string(),
        delivery,
    }])
}

/// Validate one note, and record nothing.
///
/// The two halves of a note are judged in two places, deliberately. **Whether the
/// ask is one this run can act on** is a question about the graph and is answered
/// here: a node the graph does not hold has no conversation to reach, and the
/// note's own text and criterion were already refused by their newtypes at the
/// envelope's boundary if they were unusable. **Whether it was delivered** is a
/// question only the conversation can answer, so it is asked where the delivery is
/// made and recorded there — which is why nothing comes back from here.
///
/// Notably *not* refused here: a node that has settled. A `context` note to one is
/// turned away by this module because it would attach to a dispatch that will never
/// run; a *note* is not attached to anything, it is handed to a conversation, and
/// the conversation's own answer — which names how it ended — is a better refusal
/// than this module could compose. Refusing here would also record nothing, and a
/// non-delivery the run does not record is the silence this seam exists to end.
fn compile_note(
    graph: &mut Graph,
    id: &str,
    _addressee: crate::note::Addressee,
    _text: &crate::note::NoteText,
    _criterion: Option<&crate::note::Criterion>,
) -> Result<Vec<Operation>> {
    if !graph.contains(id) {
        return Err(refuse(format!("note: no node '{id}'")));
    }
    Ok(Vec::new())
}

/// Validate one amendment and record it: the node's whole amendment, replacing
/// whatever it carried.
///
/// The three refusals are the three ways an amendment reaches nobody, and each
/// says which one it was. A node the graph does not hold has no bar to move; a
/// node that has settled `done` will never be judged again, which is why
/// `context` refuses one for the same reason; and a blank amendment is a bar
/// nobody can clear, so it is refused rather than recorded as one.
fn compile_amend(
    graph: &mut Graph,
    frontier: &Frontier,
    id: &str,
    text: &str,
) -> Result<Vec<Operation>> {
    if !graph.contains(id) {
        return Err(refuse(format!("amend: no node '{id}'")));
    }
    if text.trim().is_empty() {
        return Err(refuse(format!(
            "amend: node '{id}': the amendment cannot be blank"
        )));
    }
    if frontier.recorded.get(id) == Some(&NodeStatus::Done) {
        return Err(refuse(format!(
            "amend: node '{id}' has settled done, so nothing will read the amendment"
        )));
    }
    if let Some(node) = graph.get_mut(id) {
        node.amendment = Some(text.to_string());
    }
    Ok(vec![Operation::TaskAmended {
        node: id.to_string(),
        text: text.to_string(),
    }])
}

/// Fold one recorded operation back onto a graph.
///
/// This is replay's half of [`compile`]: the reconciler validated and mutated,
/// and this reconstructs the same mutation from the journal without re-judging
/// it. A rejected edit was never recorded, so nothing here can refuse.
pub fn apply(graph: &mut Graph, operation: &Operation) {
    match operation {
        Operation::NodeAdded { node, .. } => graph.insert((**node).clone()),
        Operation::NodeDropped { node, .. } => {
            graph.remove(node);
        }
        Operation::Reparent { node, to, .. } => {
            if let Some(node) = graph.get_mut(node) {
                node.deps.clone_from(to);
                node.consumes.retain(|dep, _| to.iter().any(|d| d == dep));
            }
        }
        Operation::EdgeRemoved { from, to } => {
            if let Some(node) = graph.get_mut(to) {
                node.deps.retain(|dep| dep != from);
                node.consumes.remove(from);
            }
        }
        Operation::EdgeAdded { from, to, target } => {
            if let Some(node) = graph.get_mut(to) {
                if !node.deps.contains(from) {
                    node.deps.push(from.clone());
                }
                // Only where the record states one. A record that states none
                // is an edge with no target — which is every edge written
                // before this field existed — and the node's own definition,
                // which `node-added` carried whole, is left as it stands.
                if let Some(target) = target {
                    node.consumes.insert(from.clone(), target.clone());
                }
            }
        }
        Operation::NodeParked { node } => {
            if let Some(node) = graph.get_mut(node) {
                node.parked = true;
            }
        }
        Operation::NodeRequeued { node, amend } => {
            let Some(existing) = graph.get(node).cloned() else {
                return;
            };
            let mut merged = match serde_json::to_value(&existing) {
                Ok(value) => value,
                Err(_) => return,
            };
            if let Some(object) = merged.as_object_mut() {
                object.remove("parked");
                for (key, value) in amend.iter().flatten() {
                    object.insert(key.clone(), value.clone());
                }
            }
            if let Ok(amended) = serde_json::from_value::<Node>(merged) {
                graph.insert(amended);
            }
        }
        // Replace, exactly as the reconciler replaced: the last one recorded is
        // the node's amendment, so replaying them in order lands on it.
        Operation::TaskAmended { node, text } => {
            if let Some(node) = graph.get_mut(node) {
                node.amendment = Some(text.clone());
            }
        }
        // A live delivery went into a turn rather than onto the graph, so
        // replay reconstructs it by changing nothing — exactly as the
        // reconciler did.
        Operation::ContextAdded {
            node,
            note,
            delivery: Delivery::Deferred,
        } => {
            if let Some(node) = graph.get_mut(node) {
                node.context = Some(note.clone());
            }
        }
        Operation::ContextAdded { .. } => {}
        // None mutates the graph: an attestation settles a node, a completion
        // request is journalled for audit, a finding went to the planner's
        // queue, and a note went into a conversation — all folded elsewhere, or
        // nowhere.
        Operation::NoteDelivered { .. }
        | Operation::HumanAttested { .. }
        | Operation::CompletionRequested { .. }
        | Operation::FindingRaised { .. }
        | Operation::RetryRequested { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{NodeKind, Plan, Resume, PLAN_SCHEMA_VERSION};

    fn agent(id: &str, deps: &[&str]) -> Node {
        Node {
            id: id.into(),
            persona: Some("engineer".into()),
            task: Some("## What\ndo it".into()),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
            ..Node::default()
        }
    }

    /// A release target, spelled the one way a plan spells one.
    fn target(name: &str) -> TargetName {
        name.parse().expect("a release target name")
    }

    /// Every release target the graph holds, as `(node, dep, target)`.
    ///
    /// The whole of what an edit is allowed to move: a target that appears here
    /// afterwards and was not stated by a plan or by the edit itself is one this
    /// crate invented.
    fn targets_in(graph: &Graph) -> BTreeSet<(String, String, String)> {
        graph
            .iter()
            .flat_map(|node| {
                node.consumes
                    .iter()
                    .map(|(dep, target)| (node.id.clone(), dep.clone(), target.to_string()))
            })
            .collect()
    }

    fn graph_of(nodes: Vec<Node>) -> Graph {
        Graph::from_plan(&Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            goal: None,
            name: None,
            concurrency: 4,
            tasks: nodes,
        })
    }

    /// A `context` edit as a planner who says nothing about delivery writes one.
    fn note_for(id: &str, note: &str) -> Command {
        Command::Context {
            id: id.into(),
            note: note.into(),
            deliver: crate::channel::Deliver::default(),
        }
    }

    fn frontier(entries: &[(&str, NodeStatus)]) -> Frontier {
        Frontier {
            recorded: entries
                .iter()
                .map(|(id, status)| ((*id).to_string(), *status))
                .collect(),
            ..Frontier::default()
        }
    }

    #[test]
    fn add_inserts_a_node_and_records_its_edges() {
        let mut graph = graph_of(vec![agent("a", &[])]);
        let operations = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Add {
                node: agent("b", &["a"]),
            },
        )
        .expect("the add is legal");
        assert!(graph.contains("b"));
        assert!(matches!(operations[0], Operation::NodeAdded { .. }));
        assert!(
            matches!(&operations[1], Operation::EdgeAdded { from, to, .. } if from == "a" && to == "b")
        );
    }

    #[test]
    fn add_refuses_a_duplicate_id_an_invalid_node_and_a_dangling_dependency() {
        let mut graph = graph_of(vec![agent("a", &[])]);
        let refusals = [
            (
                Command::Add {
                    node: agent("a", &[]),
                },
                "already exists",
            ),
            (
                Command::Add {
                    node: Node {
                        id: "b".into(),
                        ..Node::default()
                    },
                },
                "needs a persona",
            ),
            (
                Command::Add {
                    node: agent("c", &["nowhere"]),
                },
                "not in the plan",
            ),
        ];
        for (command, expected) in refusals {
            let message = compile(&mut graph, &Frontier::default(), &command)
                .unwrap_err()
                .to_string();
            assert!(message.contains(expected), "{message:?} lacks {expected:?}");
        }
        assert_eq!(graph.len(), 1, "a refused add mutated the graph");
    }

    #[test]
    fn an_add_that_would_create_a_cycle_is_refused_and_changes_nothing() {
        let mut graph = graph_of(vec![agent("a", &["b"]), agent("b", &[])]);
        // `b` already exists, so this is a reparent-shaped cycle through add's
        // validation of the resulting graph.
        let message = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Reparent {
                id: "b".into(),
                deps: vec!["a".into()],
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("cycle"), "{message}");
        assert!(graph.get("b").expect("b").deps.is_empty());
    }

    #[test]
    fn reparent_requires_an_unstarted_node() {
        let mut graph = graph_of(vec![agent("a", &[]), agent("b", &[])]);
        let started = frontier(&[("b", NodeStatus::Running)]);
        let message = compile(
            &mut graph,
            &started,
            &Command::Reparent {
                id: "b".into(),
                deps: vec!["a".into()],
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("already started"), "{message}");

        let operations = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Reparent {
                id: "b".into(),
                deps: vec!["a".into()],
            },
        )
        .expect("an unstarted node reparents");
        assert_eq!(graph.get("b").expect("b").deps, vec!["a".to_string()]);
        assert!(operations
            .iter()
            .any(|op| matches!(op, Operation::Reparent { .. })));

        assert!(compile(
            &mut graph,
            &Frontier::default(),
            &Command::Reparent {
                id: "nowhere".into(),
                deps: vec![],
            },
        )
        .unwrap_err()
        .to_string()
        .contains("no node"));
    }

    #[test]
    fn drop_detaches_or_recursively_removes_and_must_state_which() {
        let mut graph = graph_of(vec![
            agent("a", &[]),
            agent("b", &["a"]),
            agent("c", &["b"]),
        ]);
        compile(
            &mut graph,
            &Frontier::default(),
            &Command::Drop {
                id: "a".into(),
                dependents: Dependents::Detach,
            },
        )
        .expect("detaching is legal");
        assert!(!graph.contains("a"));
        assert!(graph.get("b").expect("b").deps.is_empty());
        assert!(graph.contains("c"));

        let mut graph = graph_of(vec![
            agent("a", &[]),
            agent("b", &["a"]),
            agent("c", &["b"]),
        ]);
        compile(
            &mut graph,
            &Frontier::default(),
            &Command::Drop {
                id: "a".into(),
                dependents: Dependents::Drop,
            },
        )
        .expect("recursive dropping is legal");
        assert!(graph.is_empty(), "the dependents were not dropped too");
    }

    #[test]
    fn drop_refuses_to_remove_the_last_unresolved_publication_anchor() {
        let lifecycle = |id: &str, deps: &[&str]| Node {
            repo: Some("owner/repo".into()),
            ..agent(id, deps)
        };
        let mut graph = graph_of(vec![
            lifecycle("anchor", &[]),
            lifecycle("stacked", &["anchor"]),
        ]);
        let message = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Drop {
                id: "anchor".into(),
                dependents: Dependents::Detach,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("publication anchor"), "{message}");
        assert!(graph.contains("anchor"));

        // Another settled anchor on the same identity makes it legal.
        let mut graph = graph_of(vec![
            lifecycle("anchor", &[]),
            lifecycle("stacked", &["anchor"]),
            lifecycle("landed", &[]),
        ]);
        compile(
            &mut graph,
            &frontier(&[("landed", NodeStatus::Done)]),
            &Command::Drop {
                id: "anchor".into(),
                dependents: Dependents::Detach,
            },
        )
        .expect("a settled alternative anchor allows the drop");
    }

    #[test]
    fn retry_supersedes_only_a_running_failed_or_cancelled_node() {
        let mut graph = graph_of(vec![agent("build", &[]), agent("ship", &["build"])]);
        let message = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Retry {
                id: "build".into(),
                node: agent("build-2", &[]),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(
            message.contains("not running, failed, or cancelled"),
            "{message}"
        );

        let operations = compile(
            &mut graph,
            &frontier(&[("build", NodeStatus::Failed)]),
            &Command::Retry {
                id: "build".into(),
                node: agent("build-2", &[]),
            },
        )
        .expect("a failed node retries");
        assert!(graph.contains("build-2"));
        assert_eq!(
            graph.get("ship").expect("ship").deps,
            vec!["build-2".to_string()],
            "the dependent was not redirected"
        );
        assert!(operations
            .iter()
            .any(|op| matches!(op, Operation::RetryRequested { .. })));
    }

    #[test]
    fn retry_demands_a_new_id() {
        let mut graph = graph_of(vec![agent("build", &[])]);
        let failed = frontier(&[("build", NodeStatus::Failed)]);
        for (node, expected) in [
            (agent("build", &[]), "must be new"),
            (
                Node {
                    id: String::new(),
                    ..agent("x", &[])
                },
                "non-empty id",
            ),
        ] {
            let message = compile(
                &mut graph,
                &failed,
                &Command::Retry {
                    id: "build".into(),
                    node,
                },
            )
            .unwrap_err()
            .to_string();
            assert!(message.contains(expected), "{message:?} lacks {expected:?}");
        }
        assert!(compile(
            &mut graph,
            &failed,
            &Command::Retry {
                id: "nowhere".into(),
                node: agent("x", &[])
            }
        )
        .unwrap_err()
        .to_string()
        .contains("no node"));
    }

    #[test]
    fn a_retry_may_name_only_one_branch() {
        let mut graph = graph_of(vec![Node {
            repo: Some("owner/repo".into()),
            ..agent("build", &[])
        }]);
        let failed = frontier(&[("build", NodeStatus::Failed)]);

        let disagreeing = Node {
            repo: Some("owner/repo".into()),
            branch: Some("pinned".into()),
            resume: Some(Resume {
                branch: "preserved".into(),
                checkpoint: None,
                completed_steps: Vec::new(),
            }),
            ..agent("build-2", &[])
        };
        let message = compile(
            &mut graph,
            &failed,
            &Command::Retry {
                id: "build".into(),
                node: disagreeing,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("only one branch"), "{message}");

        // A resume with no pin *is* the pin.
        let resuming = Node {
            repo: Some("owner/repo".into()),
            resume: Some(Resume {
                branch: "preserved".into(),
                checkpoint: Some("abc123".into()),
                completed_steps: Vec::new(),
            }),
            ..agent("build-3", &[])
        };
        compile(
            &mut graph,
            &failed,
            &Command::Retry {
                id: "build".into(),
                node: resuming,
            },
        )
        .expect("naming a continuation is naming its branch");
        assert_eq!(
            graph.get("build-3").expect("build-3").branch.as_deref(),
            Some("preserved")
        );
    }

    #[test]
    fn cancel_parks_a_pending_or_running_node_and_nothing_else() {
        let mut graph = graph_of(vec![agent("sweep", &[])]);
        compile(
            &mut graph,
            &Frontier::default(),
            &Command::Cancel { id: "sweep".into() },
        )
        .expect("a pending node parks");
        assert!(graph.get("sweep").expect("sweep").parked);

        let message = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Cancel { id: "sweep".into() },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("already parked"), "{message}");

        let mut graph = graph_of(vec![agent("done", &[])]);
        let message = compile(
            &mut graph,
            &frontier(&[("done", NodeStatus::Done)]),
            &Command::Cancel { id: "done".into() },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("not pending or running"), "{message}");

        assert!(compile(
            &mut graph,
            &Frontier::default(),
            &Command::Cancel {
                id: "nowhere".into()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("no node"));
    }

    #[test]
    fn requeue_returns_a_parked_node_and_refuses_to_rewrite_id_or_deps() {
        let mut parked = agent("sweep", &[]);
        parked.parked = true;
        let mut graph = graph_of(vec![parked]);

        for key in ["id", "deps"] {
            let mut amend = Map::new();
            amend.insert(key.to_string(), Value::String("other".into()));
            let message = compile(
                &mut graph,
                &Frontier::default(),
                &Command::Requeue {
                    id: "sweep".into(),
                    amend: Some(amend),
                },
            )
            .unwrap_err()
            .to_string();
            assert!(message.contains("cannot amend"), "{message}");
        }

        let mut amend = Map::new();
        amend.insert("max_turns".into(), Value::from(32));
        let operations = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Requeue {
                id: "sweep".into(),
                amend: Some(amend),
            },
        )
        .expect("a parked node requeues");
        let node = graph.get("sweep").expect("sweep");
        assert!(!node.parked);
        assert_eq!(node.max_turns, Some(32));
        assert!(matches!(
            &operations[0],
            Operation::NodeRequeued { amend: Some(_), .. }
        ));
    }

    #[test]
    fn a_bare_requeue_records_no_amendment_at_all() {
        let mut parked = agent("sweep", &[]);
        parked.parked = true;
        let mut graph = graph_of(vec![parked]);
        let operations = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Requeue {
                id: "sweep".into(),
                amend: Some(Map::new()),
            },
        )
        .expect("a bare requeue is legal");
        assert!(matches!(
            &operations[0],
            Operation::NodeRequeued { amend: None, .. }
        ));
    }

    #[test]
    fn requeue_refuses_an_unparked_or_unknown_node_and_a_malformed_amendment() {
        let mut graph = graph_of(vec![agent("live", &[])]);
        assert!(compile(
            &mut graph,
            &Frontier::default(),
            &Command::Requeue {
                id: "live".into(),
                amend: None
            }
        )
        .unwrap_err()
        .to_string()
        .contains("not parked"));
        assert!(compile(
            &mut graph,
            &Frontier::default(),
            &Command::Requeue {
                id: "nowhere".into(),
                amend: None
            }
        )
        .unwrap_err()
        .to_string()
        .contains("no node"));

        let mut parked = agent("sweep", &[]);
        parked.parked = true;
        let mut graph = graph_of(vec![parked]);
        let mut amend = Map::new();
        amend.insert("max_turns".into(), Value::String("lots".into()));
        let message = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Requeue {
                id: "sweep".into(),
                amend: Some(amend),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("is invalid"), "{message}");
    }

    /// The retry that carried a corrected review bar is where a planner writes
    /// one, so the amendment gets the same named refusal a project does — not
    /// the schema's bare `unknown field`.
    #[test]
    fn requeue_refuses_a_retired_field_by_name_and_says_where_the_bar_goes() {
        let mut parked = agent("contract", &[]);
        parked.parked = true;
        let mut graph = graph_of(vec![parked]);
        let mut amend = Map::new();
        amend.insert(
            "done_when".into(),
            Value::String("the gate is green".into()),
        );
        let message = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Requeue {
                id: "contract".into(),
                amend: Some(amend),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("'contract':"), "{message}");
        assert!(
            message.contains("`done_when` is no longer a plan field"),
            "{message}"
        );
        assert!(
            message.contains("`## Acceptance criteria` section of its own task"),
            "the refusal does not say where the bar goes: {message}"
        );
        assert!(!message.contains("unknown field"), "{message}");
    }

    #[test]
    fn attest_needs_a_ready_waiting_action_that_is_not_already_done() {
        let graph_node = Node {
            id: "approve".into(),
            kind: NodeKind::Human,
            task: Some("approve it".into()),
            ..Node::default()
        };
        let mut graph = graph_of(vec![graph_node]);

        assert!(compile(
            &mut graph,
            &Frontier::default(),
            &Command::Attest {
                reference: "approve".into()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("not a ready, waiting human action"));

        let waiting = frontier(&[("approve", NodeStatus::Waiting)]);
        compile(
            &mut graph,
            &waiting,
            &Command::Attest {
                reference: "approve".into(),
            },
        )
        .expect("a waiting action attests");

        let mut already = waiting.clone();
        already.attestations.insert("approve".into());
        assert!(compile(
            &mut graph,
            &already,
            &Command::Attest {
                reference: "approve".into()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("already attested"));
    }

    /// The second settlement `attest` accepts, and the refusal that names both.
    ///
    /// A failed node gates every dependent that named it for ever — the skip is
    /// re-derived on every pass — so an attestation that the work landed anyway
    /// is the only thing in this vocabulary that can release them.
    #[test]
    fn attest_takes_a_node_that_settled_failed_and_names_both_references_when_it_refuses() {
        let mut graph = graph_of(vec![agent("build", &[]), agent("ship", &["build"])]);

        let failed = frontier(&[("build", NodeStatus::Failed)]);
        assert_eq!(
            compile(
                &mut graph,
                &failed,
                &Command::Attest {
                    reference: "build".into(),
                },
            )
            .expect("a failed node attests"),
            vec![Operation::HumanAttested {
                node: "build".into()
            }]
        );

        let mut again = failed.clone();
        again.attestations.insert("build".into());
        assert!(compile(
            &mut graph,
            &again,
            &Command::Attest {
                reference: "build".into()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("already attested"));

        let message = compile(
            &mut graph,
            &frontier(&[("build", NodeStatus::Running)]),
            &Command::Attest {
                reference: "build".into(),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(
            message.contains("not a ready, waiting human action"),
            "{message}"
        );
        assert!(message.contains("node that settled failed"), "{message}");
    }

    #[test]
    fn context_reaches_a_node_that_can_still_be_dispatched() {
        let mut graph = graph_of(vec![agent("build", &[])]);
        let operations = compile(
            &mut graph,
            &Frontier::default(),
            &note_for("build", "the fixture moved"),
        )
        .expect("a live node takes a note");
        assert_eq!(
            graph.get("build").expect("build").context.as_deref(),
            Some("the fixture moved")
        );
        assert!(
            matches!(
                &operations[0],
                Operation::ContextAdded {
                    delivery: Delivery::Deferred,
                    ..
                }
            ),
            "a note nobody delivered is owed to the next dispatch: {operations:?}"
        );

        for (frontier_state, command, expected) in [
            (
                frontier(&[("build", NodeStatus::Done)]),
                note_for("build", "too late"),
                "settled done",
            ),
            (
                Frontier::default(),
                note_for("build", "   "),
                "cannot be empty",
            ),
            (Frontier::default(), note_for("nowhere", "hello"), "no node"),
        ] {
            let message = compile(&mut graph, &frontier_state, &command)
                .unwrap_err()
                .to_string();
            assert!(message.contains(expected), "{message:?} lacks {expected:?}");
        }
    }

    /// A note the running turn took is not also owed to the next dispatch —
    /// otherwise that dispatch would re-state a correction the worker has
    /// already acted on.
    #[test]
    fn a_note_delivered_live_leaves_nothing_on_the_node_for_a_later_dispatch() {
        let mut graph = graph_of(vec![agent("build", &[])]);
        let operations = compile_with(
            &mut graph,
            &frontier(&[("build", NodeStatus::Running)]),
            &note_for("build", "the fixture moved"),
            Delivery::Live,
        )
        .expect("a running node takes a note into its turn");
        assert_eq!(
            graph.get("build").expect("build").context,
            None,
            "a live note was also queued for the next dispatch"
        );
        assert!(
            matches!(
                &operations[0],
                Operation::ContextAdded {
                    delivery: Delivery::Live,
                    note,
                    ..
                } if note == "the fixture moved"
            ),
            "{operations:?}"
        );

        // And replay reconstructs exactly that: nothing on the graph.
        let mut replayed = graph_of(vec![agent("build", &[])]);
        apply(&mut replayed, &operations[0]);
        assert_eq!(replayed.get("build").expect("build").context, None);
    }

    /// A record written before delivery had modes says nothing about it, and the
    /// only thing those records ever did is what an absent value must mean.
    #[test]
    fn a_context_operation_from_before_this_field_replays_as_deferred() {
        let operation: Operation = serde_json::from_value(serde_json::json!({
            "kind": "context-added",
            "node": "build",
            "note": "the fixture moved",
        }))
        .expect("an operation without a delivery still parses");
        let mut graph = graph_of(vec![agent("build", &[])]);
        apply(&mut graph, &operation);
        assert_eq!(
            graph.get("build").expect("build").context.as_deref(),
            Some("the fixture moved"),
            "an older record stopped attaching its note"
        );
    }

    /// An amendment becomes part of the node's effective task, and a second one
    /// **replaces** the first rather than joining it.
    #[test]
    fn amend_binds_the_node_and_a_second_amendment_replaces_the_first() {
        let mut graph = graph_of(vec![agent("build", &[])]);
        let amend = |text: &str| Command::Amend {
            id: "build".into(),
            text: text.into(),
        };

        let first = compile(
            &mut graph,
            &Frontier::default(),
            &amend("leave the comments"),
        )
        .expect("a live node takes an amendment");
        assert_eq!(
            graph.get("build").expect("build").amendment.as_deref(),
            Some("leave the comments")
        );
        assert!(
            graph
                .get("build")
                .expect("build")
                .rendered_task()
                .contains("leave the comments"),
            "the amendment is not part of the effective task"
        );
        assert!(
            matches!(&first[0], Operation::TaskAmended { node, text }
                if node == "build" && text == "leave the comments"),
            "{first:?}"
        );

        let second = compile(
            &mut graph,
            &frontier(&[("build", NodeStatus::Running)]),
            &amend("restore the comments after all"),
        )
        .expect("a running node takes one too");
        let effective = graph.get("build").expect("build").rendered_task();
        assert!(
            effective.contains("restore the comments after all"),
            "{effective}"
        );
        assert!(
            !effective.contains("leave the comments"),
            "the replaced ruling is still binding the judge beside its own correction: {effective}"
        );

        // Replay reconstructs the amended task without re-judging either one.
        let mut replayed = graph_of(vec![agent("build", &[])]);
        for operation in first.iter().chain(second.iter()) {
            apply(&mut replayed, operation);
        }
        assert_eq!(
            replayed.get("build").expect("build").amendment.as_deref(),
            Some("restore the comments after all")
        );
    }

    /// The three ways an amendment reaches nobody, each refused by the one it
    /// was — and none of them touching the graph.
    #[test]
    fn amend_refuses_an_unknown_node_a_settled_one_and_a_blank_ruling() {
        let mut graph = graph_of(vec![agent("build", &[])]);
        let before = graph.clone();
        for (frontier_state, id, text, expected) in [
            (
                Frontier::default(),
                "nowhere",
                "a ruling",
                "no node 'nowhere'",
            ),
            (
                frontier(&[("build", NodeStatus::Done)]),
                "build",
                "a ruling",
                "settled done",
            ),
            (Frontier::default(), "build", "   \n", "cannot be blank"),
        ] {
            let message = compile(
                &mut graph,
                &frontier_state,
                &Command::Amend {
                    id: id.into(),
                    text: text.into(),
                },
            )
            .unwrap_err()
            .to_string();
            assert!(message.contains(expected), "{message:?} lacks {expected:?}");
        }
        assert_eq!(graph, before, "a refused amendment changed the graph");
    }

    /// A scratch directory of this test process's own, for the validator
    /// programs the journeys below run.
    ///
    /// Gated with the programs it holds: every caller is one of the `cfg(unix)`
    /// journeys below, so on Windows this is dead code and `-D warnings` says so.
    #[cfg(unix)]
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("onepipeline-edits-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch root");
        dir
    }

    /// One validator program, written and made runnable.
    ///
    /// A real executable rather than a double: what the hook promises is that a
    /// command the host names is *run*, so a stand-in for running it would prove
    /// nothing. Unix-only because the program is a shell script; the two
    /// platform-independent halves — a launch that names no validator, and one
    /// whose validator cannot be started — are tested without one.
    #[cfg(unix)]
    fn validator(dir: &std::path::Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}"))
            .expect("the validator program is written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("it is runnable");
        path.to_string_lossy().into_owned()
    }

    /// A frontier judging edits under one named validator.
    fn validated_by(command: &str) -> Frontier {
        Frontier {
            node_validator: Some(command.to_string()),
            ..Frontier::default()
        }
    }

    /// The four ops that put unchecked task prose in front of a dispatch are the
    /// four the validator sees, and nothing else is.
    ///
    /// Both directions, because each is a way the guard fails silently: an op
    /// that reaches no validator is the hole this hook exists to close, and an
    /// op that reaches one it has no opinion about — a requeue raising a turn
    /// budget — spends a subprocess to be told nothing.
    #[test]
    #[cfg(unix)]
    fn every_op_that_introduces_or_changes_a_task_is_offered_to_the_validator() {
        let dir = scratch("offered");
        let seen = dir.join("seen.jsonl");
        let accept = validator(
            &dir,
            "accept.sh",
            &format!("cat >> {0}\nprintf '\\n' >> {0}\nexit 0\n", seen.display()),
        );
        let frontier_state = Frontier {
            recorded: [("build".to_string(), NodeStatus::Failed)]
                .into_iter()
                .collect(),
            ..validated_by(&accept)
        };

        let mut graph = graph_of(vec![agent("build", &[]), agent("docs", &[])]);
        for command in [
            Command::Add {
                node: agent("fresh", &[]),
            },
            Command::Retry {
                id: "build".into(),
                node: agent("build-2", &[]),
            },
            Command::Amend {
                id: "docs".into(),
                text: "the ruling".into(),
            },
            Command::Cancel { id: "docs".into() },
            // Offered: this requeue's amendment rewrites the task.
            Command::Requeue {
                id: "docs".into(),
                amend: Some(
                    serde_json::json!({"task": "## What\nsomething else"})
                        .as_object()
                        .expect("an object")
                        .clone(),
                ),
            },
            Command::Cancel { id: "fresh".into() },
            // Not offered: this one raises a turn budget, which changes nothing
            // a dispatch is asked to do — and neither does a cancel or a note.
            Command::Requeue {
                id: "fresh".into(),
                amend: Some(
                    serde_json::json!({"max_turns": 32})
                        .as_object()
                        .expect("an object")
                        .clone(),
                ),
            },
            note_for("docs", "a note"),
        ] {
            compile(&mut graph, &frontier_state, &command)
                .unwrap_or_else(|e| panic!("the accepting validator refused {command:?}: {e}"));
        }

        let offered: Vec<String> = std::fs::read_to_string(&seen)
            .expect("the validator recorded what it was given")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let node: Node = serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("the node crossed as a plan node: {e} in {line}"));
                node.id
            })
            .collect();
        assert_eq!(
            offered,
            vec!["fresh", "build-2", "docs", "docs"],
            "the validator was offered the wrong edits"
        );

        // And what crossed is the node the edit *produced*, amendment and all —
        // so a host checking a node's bar is checking the bar it will be judged
        // against.
        let amended: Node = serde_json::from_str(
            std::fs::read_to_string(&seen)
                .expect("readable")
                .lines()
                .filter(|line| !line.trim().is_empty())
                .nth(2)
                .expect("the amend's own offering, third of the four"),
        )
        .expect("it parses");
        assert_eq!(amended.amendment.as_deref(), Some("the ruling"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A validator's stderr is external input: bounded on the way in, and stripped
    /// of the control characters that would otherwise reach a terminal, a
    /// planner's queue, and the journal.
    #[test]
    #[cfg(unix)]
    fn a_validators_stderr_is_bounded_and_stripped_before_it_becomes_a_refusal() {
        let dir = scratch("loud");
        // An escape sequence, then far more output than any payload this crate
        // writes may carry.
        let loud = validator(
            &dir,
            "loud.sh",
            "cat > /dev/null\n\
             printf '\\033[31mrule 3\\033[0m failed\\n' >&2\n\
             head -c 100000 /dev/zero | tr '\\0' 'x' >&2\n\
             exit 1\n",
        );
        let mut graph = graph_of(vec![agent("build", &[])]);
        let refusal = compile(
            &mut graph,
            &validated_by(&loud),
            &Command::Add {
                node: agent("fresh", &[]),
            },
        )
        .expect_err("the validator refused it")
        .to_string();

        assert!(refusal.contains("rule 3"), "the words were lost: {refusal}");
        assert!(
            !refusal.contains('\u{1b}') && !refusal.contains('\n'),
            "a validator wrote control characters into a refusal: {refusal:?}"
        );
        assert!(
            refusal.len() <= MAX_HOOK_STDERR as usize + 200,
            "an unbounded validator wrote {} bytes into a refusal",
            refusal.len()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A validator that refuses is answered with its own words, and the graph is
    /// left exactly as it was.
    #[test]
    #[cfg(unix)]
    fn an_edit_the_validator_refuses_carries_its_own_words_and_changes_nothing() {
        let dir = scratch("refused");
        let refuse = validator(
            &dir,
            "refuse.sh",
            "cat > /dev/null\n\
             echo \"acceptance criterion 3 names a procedure, not a property\" >&2\n\
             exit 1\n",
        );
        let mut graph = graph_of(vec![agent("build", &[])]);
        let before = graph.clone();
        let refusal = compile(
            &mut graph,
            &validated_by(&refuse),
            &Command::Add {
                node: agent("fresh", &[]),
            },
        )
        .expect_err("the validator refused it")
        .to_string();
        assert!(
            refusal.contains("acceptance criterion 3 names a procedure, not a property"),
            "the refusal does not carry the validator's own words: {refusal}"
        );
        assert!(refusal.contains("fresh"), "{refusal}");
        assert_eq!(graph, before, "a refused edit reached the graph");

        // A validator that refuses without saying anything is still not silent:
        // it exits without reading its input, and the refusal names the status
        // so somebody has something to look at.
        let silent = validator(&dir, "silent.sh", "exit 3\n");
        let said = compile(
            &mut graph,
            &validated_by(&silent),
            &Command::Add {
                node: agent("fresh", &[]),
            },
        )
        .expect_err("it refused")
        .to_string();
        assert!(said.contains("exited 3"), "{said}");
        assert_eq!(graph, before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A launch that named no validator judges an edit exactly as it did before
    /// the hook existed, and one whose validator cannot be started refuses
    /// rather than letting the node through unchecked.
    #[test]
    fn no_validator_changes_nothing_and_an_unstartable_one_fails_closed() {
        let mut graph = graph_of(vec![agent("build", &[])]);
        compile(
            &mut graph,
            &Frontier::default(),
            &Command::Add {
                node: agent("fresh", &[]),
            },
        )
        .expect("a launch that named no validator adds a node as it always did");
        assert!(graph.contains("fresh"));

        let before = graph.clone();
        let missing = std::env::temp_dir().join("onepipeline-no-such-node-validator");
        let refusal = compile(
            &mut graph,
            &validated_by(&missing.to_string_lossy()),
            &Command::Add {
                node: agent("second", &[]),
            },
        )
        .expect_err("a validator that cannot be started refuses the edit")
        .to_string();
        assert!(
            refusal.contains("could not be started") && refusal.contains("checked by nothing"),
            "{refusal}"
        );
        assert_eq!(graph, before, "an unchecked node reached the graph");
    }

    /// The whole envelope reaches the reviewer as one document: every node it
    /// introduces or changes with the op that produced it, the plan they are
    /// being edited into, and the run's goal.
    ///
    /// The document is what this hook exists for — a per-node check cannot see
    /// two added nodes that duplicate each other, the edges between them, or
    /// whether the edited graph still delivers the goal — so what crosses the
    /// stdin is asserted rather than assumed.
    #[test]
    #[cfg(unix)]
    fn the_reviewer_is_handed_every_changed_node_the_edited_plan_and_the_goal() {
        let dir = scratch("reviewed");
        let seen = dir.join("envelope.json");
        let reviewer = validator(
            &dir,
            "review.sh",
            &format!("cat > {}\nexit 0\n", seen.display()),
        );
        let launched_with = Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            goal: Some(crate::plan::Goal {
                text: "ship the coverage floor".into(),
            }),
            name: Some("cover".into()),
            concurrency: 4,
            tasks: vec![agent("build", &[])],
        };
        let mut graph = Graph::from_plan(&launched_with);
        let commands = vec![
            Command::Add {
                node: agent("fresh", &["build"]),
            },
            Command::Amend {
                id: "build".into(),
                text: "the ruling".into(),
            },
            // Changes the plan without changing any node's definition, so it is
            // the plan below rather than a change of its own.
            Command::Cancel { id: "fresh".into() },
        ];
        for command in &commands {
            compile(&mut graph, &Frontier::default(), command).expect("each command compiles");
        }
        offer_envelope_to_reviewer(Some(&reviewer), &commands, &graph, Some(&launched_with))
            .expect("the reviewer accepted the envelope");

        let document: Value = serde_json::from_str(
            &std::fs::read_to_string(&seen).expect("the reviewer was handed a document"),
        )
        .expect("it is JSON");
        assert_eq!(
            document["goal"],
            serde_json::json!("ship the coverage floor")
        );
        assert_eq!(
            document["changes"]
                .as_array()
                .expect("the changes are a list")
                .iter()
                .map(|change| (
                    change["op"].as_str().expect("an op").to_string(),
                    change["node"]["id"].as_str().expect("a node").to_string()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("add".to_string(), "fresh".to_string()),
                ("amend".to_string(), "build".to_string())
            ],
            "{document}"
        );
        // The whole node, not a summary of it: the prose is what a plan-quality
        // review reads.
        assert_eq!(
            document["changes"][0]["node"]["deps"],
            serde_json::json!(["build"])
        );
        assert_eq!(
            document["changes"][1]["node"]["amendment"],
            serde_json::json!("the ruling")
        );
        // And the plan as this envelope leaves it, carrying the launch's own
        // fields, so the reviewer sees the graph the run would converge on.
        assert_eq!(document["plan"]["name"], serde_json::json!("cover"));
        assert_eq!(
            document["plan"]["tasks"]
                .as_array()
                .expect("the plan carries its tasks")
                .iter()
                .map(|task| task["id"].as_str().expect("an id").to_string())
                .collect::<Vec<_>>(),
            vec!["build".to_string(), "fresh".to_string()],
            "{document}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A reviewer that refuses turns the whole envelope away, in its own words,
    /// naming both the node it objected to and everything the envelope carried.
    #[test]
    #[cfg(unix)]
    fn a_refused_envelope_carries_the_reviewers_words_and_names_what_it_objected_to() {
        let dir = scratch("reviewer-refuses");
        let objection = "it repeats the contract seam node 'build' already owns";
        let reviewer = validator(
            &dir,
            "refuse.sh",
            &format!(
                "cat > /dev/null\nprintf '%s\\n%s\\n' \"{OBJECTION_PREFIX} fresh\" \
                 \"{objection}\" >&2\nexit 1\n"
            ),
        );
        let commands = vec![
            Command::Add {
                node: agent("fresh", &[]),
            },
            Command::Drop {
                id: "build".into(),
                dependents: Dependents::Detach,
            },
        ];
        let graph = graph_of(vec![agent("fresh", &[])]);
        let refusal = offer_envelope_to_reviewer(Some(&reviewer), &commands, &graph, None)
            .expect_err("the reviewer refused the envelope")
            .to_string();
        assert!(
            refusal.contains(objection),
            "the words were lost: {refusal}"
        );
        assert!(
            refusal.contains("none of its edits were applied"),
            "{refusal}"
        );
        // The node it objected to, told apart from the other node the same
        // envelope carried: an envelope is no longer one command, so a reason
        // nobody can locate is one nobody can act on.
        assert!(
            refusal.contains("refused this envelope over node 'fresh',"),
            "{refusal}"
        );
        // And every op the envelope carried beside it, which is not the same
        // set as the one the reviewer turned down.
        assert!(
            refusal.contains("add 'fresh'") && refusal.contains("drop 'build'"),
            "{refusal}"
        );
        // The declaration is lifted out of the sentence rather than read back
        // into it.
        assert!(!refusal.contains(OBJECTION_PREFIX), "{refusal}");

        // A reviewer that says nothing is still an answer that has to be acted
        // on, so the status is reported rather than a blank reason.
        let silent = validator(&dir, "silent.sh", "cat > /dev/null\nexit 4\n");
        let said = offer_envelope_to_reviewer(Some(&silent), &commands, &graph, None)
            .expect_err("it refused")
            .to_string();
        assert!(said.contains("exited 4"), "{said}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a refusal says about the node the reviewer objected to, in every
    /// shape a reviewer can leave that question in.
    ///
    /// The three are different facts and a reader acts differently on each: a
    /// node this envelope changes is one to go and fix, a name it does not carry
    /// is the reviewer pointing somewhere else, and no declaration at all is a
    /// refusal whose target is unstated. Falling back to listing every node the
    /// envelope carried would tell a reader the first when the truth is the
    /// third, which is the failure the declaration exists to end.
    #[test]
    #[cfg(unix)]
    fn a_refusal_tells_a_declared_node_from_a_stray_name_and_from_no_declaration() {
        let dir = scratch("reviewer-objections");
        let commands = vec![
            Command::Add {
                node: agent("fresh", &[]),
            },
            Command::Drop {
                id: "build".into(),
                dependents: Dependents::Detach,
            },
        ];
        let graph = graph_of(vec![agent("fresh", &[])]);
        for (which, declares, expected) in [
            (
                "nothing at all",
                vec![],
                "without declaring the node it objected to",
            ),
            (
                "only a blank declaration",
                vec![OBJECTION_PREFIX.to_string()],
                "without declaring the node it objected to",
            ),
            (
                "one node the envelope changes",
                vec![format!("{OBJECTION_PREFIX} fresh")],
                "over node 'fresh',",
            ),
            (
                // Declared as the reviewer wrote it: the prefix is matched
                // case-insensitively and the name is trimmed, because a host's
                // reviewer writes a sentence rather than a wire format.
                "two of them, its own way",
                vec![
                    "  Objection:   build  ".to_string(),
                    format!("{OBJECTION_PREFIX} fresh"),
                ],
                "over nodes 'build', 'fresh',",
            ),
            (
                "a name the envelope does not carry",
                vec![format!("{OBJECTION_PREFIX} ghost")],
                "over the name 'ghost', which no node this envelope changes goes by",
            ),
            (
                "one of each",
                vec![
                    format!("{OBJECTION_PREFIX} fresh"),
                    format!("{OBJECTION_PREFIX} ghost"),
                ],
                "over node 'fresh', and over the name 'ghost', which no node this envelope \
                 changes goes by",
            ),
        ] {
            let lines = declares
                .iter()
                .map(|line| format!("printf '%s\\n' \"{line}\" >&2\n"))
                .collect::<String>();
            let reviewer = validator(
                &dir,
                &format!("refuse-{}.sh", which.replace(' ', "-")),
                &format!("cat > /dev/null\n{lines}printf 'the seam is wrong\\n' >&2\nexit 1\n"),
            );
            let refusal = offer_envelope_to_reviewer(Some(&reviewer), &commands, &graph, None)
                .expect_err("the reviewer refused the envelope")
                .to_string();
            assert!(
                refusal.contains(expected),
                "a reviewer declaring {which} was reported as {refusal}"
            );
            // Its own sentence survives every shape of declaration, and no
            // declaration is read back into it.
            assert!(refusal.contains("the seam is wrong"), "{refusal}");
            assert!(
                !refusal.to_lowercase().contains(OBJECTION_PREFIX),
                "{refusal}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A launch that named no reviewer is exactly the launch it was before this
    /// hook existed, and one whose reviewer cannot be started refuses the
    /// envelope rather than letting it through unreviewed.
    #[test]
    fn no_reviewer_changes_nothing_and_an_unstartable_one_fails_closed() {
        let commands = vec![Command::Add {
            node: agent("fresh", &[]),
        }];
        let graph = graph_of(vec![agent("fresh", &[])]);
        for named in [None, Some("   ")] {
            offer_envelope_to_reviewer(named, &commands, &graph, None)
                .expect("a launch that named no reviewer commits the envelope as it always did");
        }

        let missing = std::env::temp_dir().join("onepipeline-no-such-envelope-reviewer");
        let refusal =
            offer_envelope_to_reviewer(Some(&missing.to_string_lossy()), &commands, &graph, None)
                .expect_err("a reviewer that cannot be started refuses the envelope")
                .to_string();
        assert!(
            refusal.contains("could not be started") && refusal.contains("reviewed by nothing"),
            "{refusal}"
        );
    }

    #[test]
    fn complete_journals_a_reason_without_touching_the_graph() {
        let mut graph = graph_of(vec![agent("a", &[])]);
        let before = graph.clone();
        let operations = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Complete {
                reason: "publication verified".into(),
            },
        )
        .expect("complete is always legal");
        assert_eq!(graph, before);
        assert!(matches!(
            &operations[0],
            Operation::CompletionRequested { reason } if reason == "publication verified"
        ));
    }

    #[test]
    fn every_compiled_operation_replays_onto_the_same_graph() {
        let mut live = graph_of(vec![agent("a", &[]), agent("b", &["a"])]);
        let mut replayed = live.clone();
        let frontier_state = frontier(&[("a", NodeStatus::Failed)]);

        for command in [
            Command::Add {
                node: agent("c", &["a"]),
            },
            note_for("b", "a note"),
            Command::Retry {
                id: "a".into(),
                node: agent("a-2", &[]),
            },
            Command::Cancel { id: "c".into() },
            Command::Requeue {
                id: "c".into(),
                amend: None,
            },
            Command::Amend {
                id: "b".into(),
                text: "the ruling".into(),
            },
            Command::Drop {
                id: "c".into(),
                dependents: Dependents::Detach,
            },
        ] {
            let operations =
                compile(&mut live, &frontier_state, &command).expect("each command is legal");
            for operation in &operations {
                apply(&mut replayed, operation);
            }
        }
        assert_eq!(replayed, live, "replay did not reconstruct the live graph");
    }

    #[test]
    fn replaying_an_operation_against_a_graph_that_lost_its_node_is_a_no_op() {
        let mut graph = graph_of(vec![agent("a", &[])]);
        for operation in [
            Operation::Reparent {
                node: "gone".into(),
                from: vec![],
                to: vec!["a".into()],
            },
            Operation::EdgeAdded {
                from: "a".into(),
                to: "gone".into(),
                target: None,
            },
            Operation::EdgeRemoved {
                from: "a".into(),
                to: "gone".into(),
            },
            Operation::NodeParked {
                node: "gone".into(),
            },
            Operation::TaskAmended {
                node: "gone".into(),
                text: "the ruling".into(),
            },
            Operation::NodeRequeued {
                node: "gone".into(),
                amend: None,
            },
            Operation::ContextAdded {
                node: "gone".into(),
                note: "n".into(),
                delivery: Delivery::Deferred,
            },
            Operation::HumanAttested {
                node: "gone".into(),
            },
            Operation::CompletionRequested { reason: "r".into() },
        ] {
            apply(&mut graph, &operation);
        }
        assert_eq!(graph.len(), 1);
    }

    /// `consumes` is keyed by **dependency node id**, so an edge a `retry`
    /// rewires takes the target keyed on it along.
    ///
    /// The shape this defect was found in: a lifecycle node whose `published`
    /// dependent consumes it at a named target, lost to a provider failure. Left
    /// behind, the dependent's key names a node its own `deps` no longer carry,
    /// `graph::validate_node` refuses the candidate graph, and the whole retry is
    /// rejected — which is a run nothing can move past, because the replacement's
    /// id necessarily differs from the superseded one.
    #[test]
    fn a_retry_rekeys_a_dependents_consumes_onto_the_replacement() {
        let engine = Node {
            repo: Some("owner/engine".into()),
            ..agent("engine-run-reading", &[])
        };
        let mut adopt = Node {
            adoption: Some(onevcs::Adoption::Published),
            ..agent("ao-adopt", &["engine-run-reading"])
        };
        adopt
            .consumes
            .insert("engine-run-reading".into(), target("crate"));
        // A second dependent that consumes nothing, so "nothing was defaulted"
        // is asserted on the same edit that rekeys.
        let plain = agent("audit", &["engine-run-reading"]);
        let mut graph = graph_of(vec![engine, adopt, plain]);
        let stated = targets_in(&graph);

        compile(
            &mut graph,
            &frontier(&[("engine-run-reading", NodeStatus::Failed)]),
            &Command::Retry {
                id: "engine-run-reading".into(),
                node: Node {
                    repo: Some("owner/engine".into()),
                    ..agent("engine-run-reading-2", &[])
                },
            },
        )
        .expect("a node another node consumes retries");

        let adopt = graph.get("ao-adopt").expect("the dependent survived");
        assert_eq!(adopt.deps, vec!["engine-run-reading-2".to_string()]);
        assert_eq!(
            adopt.consumes,
            BTreeMap::from([("engine-run-reading-2".to_string(), target("crate"))]),
            "the target did not follow the edge onto the replacement"
        );
        assert!(
            graph
                .get("audit")
                .expect("the other dependent")
                .consumes
                .is_empty(),
            "a dependent that stated no target was given one"
        );
        assert_eq!(
            targets_in(&graph)
                .into_iter()
                .map(|(_, _, target)| target)
                .collect::<BTreeSet<_>>(),
            stated
                .into_iter()
                .map(|(_, _, target)| target)
                .collect::<BTreeSet<_>>(),
            "the retry altered a release target, or invented one"
        );
    }

    /// A replacement that states no dependencies of its own inherits the
    /// superseded node's — and inherits its targets on the same condition, since
    /// a target is only meaningful beside the dep it keys on. One that states its
    /// own dependencies is answered with what it stated, targets included.
    #[test]
    fn a_replacement_inherits_the_consumes_it_inherits_deps_with() {
        let stated = |id: &str| {
            let mut node = agent(id, &["engine", "packager"]);
            node.consumes.insert("engine".into(), target("crate"));
            node.consumes.insert("packager".into(), target("wheel"));
            node
        };
        let base = || {
            graph_of(vec![
                agent("engine", &[]),
                agent("packager", &[]),
                stated("build"),
            ])
        };
        let failed = frontier(&[("build", NodeStatus::Failed)]);

        let mut graph = base();
        compile(
            &mut graph,
            &failed,
            &Command::Retry {
                id: "build".into(),
                node: agent("build-2", &[]),
            },
        )
        .expect("a replacement stating no deps inherits them");
        let inherited = graph.get("build-2").expect("the replacement");
        assert_eq!(
            inherited.deps,
            vec!["engine".to_string(), "packager".to_string()]
        );
        assert_eq!(inherited.consumes, stated("build").consumes);

        // And one that states its own is answered with what it stated: the
        // inheritance is a default, never an override.
        let mut graph = base();
        let mut own = agent("build-2", &["packager"]);
        own.consumes.insert("packager".into(), target("crate"));
        compile(
            &mut graph,
            &failed,
            &Command::Retry {
                id: "build".into(),
                node: own,
            },
        )
        .expect("a replacement stating its own deps keeps them");
        let stated_its_own = graph.get("build-2").expect("the replacement");
        assert_eq!(stated_its_own.deps, vec!["packager".to_string()]);
        assert_eq!(
            stated_its_own.consumes,
            BTreeMap::from([("packager".to_string(), target("crate"))]),
            "the replacement was given the superseded node's targets over its own"
        );
    }

    /// Detaching takes the dependency away, so the target keyed on it names
    /// nothing and goes with it.
    #[test]
    fn dropping_a_consumed_node_takes_its_dependents_target_with_the_edge() {
        let mut consumer = agent("ship", &["engine", "packager"]);
        consumer.consumes.insert("engine".into(), target("crate"));
        consumer.consumes.insert("packager".into(), target("wheel"));
        let mut graph = graph_of(vec![agent("engine", &[]), agent("packager", &[]), consumer]);
        let stated = targets_in(&graph);

        compile(
            &mut graph,
            &Frontier::default(),
            &Command::Drop {
                id: "engine".into(),
                dependents: Dependents::Detach,
            },
        )
        .expect("a node another node consumes detaches");

        let ship = graph.get("ship").expect("the dependent survived");
        assert_eq!(ship.deps, vec!["packager".to_string()]);
        assert_eq!(
            ship.consumes,
            BTreeMap::from([("packager".to_string(), target("wheel"))]),
            "the dropped dependency's target outlived the dependency"
        );
        assert!(
            targets_in(&graph).is_subset(&stated),
            "the drop invented or altered a release target"
        );
    }

    /// A reparent replaces `deps` wholesale, so it drops exactly the targets
    /// whose dep the new list no longer carries and leaves every other alone.
    #[test]
    fn reparenting_away_from_a_consumed_dep_drops_only_that_target() {
        let mut consumer = agent("ship", &["engine", "packager"]);
        consumer.consumes.insert("engine".into(), target("crate"));
        consumer.consumes.insert("packager".into(), target("wheel"));
        let mut graph = graph_of(vec![
            agent("engine", &[]),
            agent("packager", &[]),
            agent("docs", &[]),
            consumer,
        ]);
        let stated = targets_in(&graph);

        compile(
            &mut graph,
            &Frontier::default(),
            &Command::Reparent {
                id: "ship".into(),
                deps: vec!["packager".into(), "docs".into()],
            },
        )
        .expect("a node reparents away from a dep it consumes");

        let ship = graph.get("ship").expect("the reparented node");
        assert_eq!(ship.deps, vec!["packager".to_string(), "docs".to_string()]);
        assert_eq!(
            ship.consumes,
            BTreeMap::from([("packager".to_string(), target("wheel"))]),
            "the surviving dep's target moved, or the removed dep's target stayed"
        );
        assert!(
            targets_in(&graph).is_subset(&stated),
            "the reparent invented or altered a release target"
        );
    }

    /// This change removes the *cause* of `validate_node`'s refusal on three
    /// paths; it does not relax the rule, and it does not widen `requeue`.
    ///
    /// Both refusals are what stops a plan silently not applying a target its
    /// author wrote, and what keeps a rewiring recorded as the op that did it.
    #[test]
    fn neither_refusal_this_change_works_around_is_relaxed() {
        let mut orphaned = agent("ship", &["engine"]);
        orphaned.consumes.insert("packager".into(), target("crate"));
        let refusal = graph::validate_node(&orphaned)
            .expect_err("a target keyed on something that is not a dep is refused")
            .to_string();
        assert!(
            refusal.contains("`consumes` names 'packager'")
                && refusal.contains("not one of this node's deps"),
            "{refusal}"
        );

        let mut parked = agent("ship", &["engine"]);
        parked.parked = true;
        let mut graph = graph_of(vec![agent("engine", &[]), parked]);
        for key in ["id", "deps"] {
            let mut amend = Map::new();
            amend.insert(key.to_string(), Value::String("other".into()));
            let message = compile(
                &mut graph,
                &Frontier::default(),
                &Command::Requeue {
                    id: "ship".into(),
                    amend: Some(amend),
                },
            )
            .unwrap_err()
            .to_string();
            assert!(message.contains("cannot amend"), "{message}");
        }
    }

    #[test]
    fn an_added_edge_is_not_duplicated_on_replay() {
        let mut graph = graph_of(vec![agent("a", &[]), agent("b", &["a"])]);
        apply(
            &mut graph,
            &Operation::EdgeAdded {
                from: "a".into(),
                to: "b".into(),
                target: None,
            },
        );
        assert_eq!(graph.get("b").expect("b").deps, vec!["a".to_string()]);
    }
}
