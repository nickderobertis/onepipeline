//! The manager-note delivery seam: one note that reaches whichever party of a
//! node's live dispatch is speaking, and reaches the other with its response —
//! or, where it reached no turn, is carried to the node's next dispatch.
//!
//! A manager correcting a node in flight used to have two levers and neither did
//! both halves of the job. `context` was delivered by interrupting the live agent
//! turn, so it reached the worker and never the judge, and it bound nothing; the
//! note reached both parties and could bind, but had no way to survive finding no
//! turn at all. They overlapped on the entire hard part and differed only in
//! fields, so they are **one op** now: [`Note`](crate::channel::Command::Note),
//! taking `id`, `addressee`, `text`, an optional `criterion`, `deliver`, and
//! `persist`. `context` is gone rather than aliased — the envelope refuses
//! unknown fields, so a caller still sending it is refused by that name.
//!
//! None of the note's routing is this crate's: the two-party conversation belongs
//! to `onejudge`, the member running it to `oneagentgraph`, and the shapes below
//! are that seam's own re-exported rather than restated. What this crate owns is
//! which node a note is for, getting it to that node's live member, carrying it to
//! that node's next dispatch where no turn took it, and putting what came back
//! into the run's record.
//!
//! * It is delivered to **whoever is live** — the worker's turn, the judge's turn,
//!   or, between turns, the next turn of that conversation to open — and the other
//!   party receives it with that party's response.
//! * The party that receives it is told **which role it is for** ([`Addressee`]),
//!   so a judge handed an update to the *worker's* task does not take the worker's
//!   job on.
//! * A note may carry a [`Criterion`], and a delivered one enters the acceptance
//!   criteria the judge of the conversation it reached evaluates against rather
//!   than appearing only as narration.
//! * **Reaching nobody is an error.** One rule, stated once and applied wherever
//!   it can be decided: a note that would reach nobody is refused, naming what
//!   left it nowhere to go — so the caller chooses relaunch, tweak, or follow-up
//!   rather than settling quietly into a record nobody reads.
//!
//! The field set, each field's default, the four combinations of `deliver` and
//! `persist`, and the six dispositions are declared **once**, on
//! [`Command::Note`]. Nothing here restates them.
//!
//! # What a note does *not* do
//!
//! It does not move the node's stored bar, and it gives no way to both reach the
//! live turn and bind a later dispatch — `persist` carries forward only what no
//! running turn took, so the two are mutually exclusive. A criterion it binds is
//! in force for the conversation it was delivered into, which is the conversation
//! whose verdict the manager is correcting;
//! [`Amend`](crate::channel::Command::Amend) is still the lever for a ruling that
//! has to survive a re-dispatch, and the two are deliberately not the same op.

use oneagentgraph::note::Accepted;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use oneagentgraph::note::{
    Addressee, Criterion, Note, NoteRefused, NoteText, Party, Undelivered,
};

use crate::channel::{Author, Command, Deliver, Reply, REPLY_ENVELOPE_VERSION};
use crate::error::{Error, Result};
use crate::event::Envelope;
use crate::views::RunPaths;

/// What became of one note, as the run records it.
///
/// Four of the five are this crate's own spelling of
/// [`oneagentgraph::note::Accepted`], and they exist for one reason: the answer is
/// written into the run's journal, and that library's enum is deliberately not
/// serializable — what crosses a boundary is the transport's decision rather than
/// the conversation's. That mapping is exhaustive in both directions, so a
/// disposition added upstream fails this build instead of being dropped on the way
/// into the record.
///
/// [`Carried`](Self::Carried) is the fifth and is this crate's own: the
/// conversation cannot report it, because it is what happened when no turn of that
/// conversation took the note at all. It and the four above it are exhaustive and
/// mutually exclusive, which is exactly the biconditional
/// [`persist`](crate::channel::Command::Note::persist) is defined by — the shape
/// here and that field's semantics were chosen together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reached", rename_all = "kebab-case")]
pub enum Reached {
    // llmlint: ignore-block[changed_behavior_has_e2e] no journey here drives this
    // disposition because none can: the conversation answers it only for a note
    // offered with **no turn live**, and the gap between two turns has no seam
    // this suite can hold open — the one process it may stand in for is the
    // harness, and a harness runs *inside* a turn. `oneagentgraph` holds that gap
    // itself, behind its non-default `test-doubles` feature, and drives this
    // disposition there; what is left here is the mapping below, which is
    // exhaustive in both directions and fails this build if the sibling adds one.
    /// Nobody was taking a turn, so the next turn to open takes it.
    Queued,
    // llmlint: ignore-end[changed_behavior_has_e2e]
    /// The worker's turn was live and was reopened carrying it, before the judge
    /// was consulted — so the judge reads it with the worker's response.
    Worker,
    /// The judge's turn was live, so its decision was re-taken with the note in
    /// hand and the note rides that response to the worker.
    Supervisor,
    /// The judge's re-taken decision was completion: the work was passed with the
    /// note in hand, and there was no next worker turn to deliver it into.
    JudgedWith {
        /// The judge's completion reason, decided with the note in hand.
        completion_reason: String,
    },
    /// No turn of the node's dispatch took it, so it was carried to that node's
    /// **next** dispatch, where it is consumed when that dispatch takes it.
    ///
    /// The disposition [`persist`](crate::channel::Command::Note::persist)
    /// answers, and materially different to whoever sent the note: the four
    /// above say a live conversation read it, and this one says the next one
    /// will. A caller that cannot tell them apart is back in the incident this
    /// op was written from, so it is named rather than left to inference.
    Carried,
}

impl Reached {
    /// Whether a conversation actually read the note.
    ///
    /// The four dispositions a conversation answers with all mean a party of it
    /// took the note — [`Queued`](Self::Queued) included, because the turn that
    /// opens next is that same conversation's and the acceptance is already
    /// made. [`Carried`](Self::Carried) is the one that means nobody read it:
    /// the note is owed to a dispatch that has not started.
    ///
    /// The question an envelope's atomicity turns on, which is why it is named
    /// here rather than pattern-matched at the one place that asks it: a
    /// conversation has no undo, so a note this answers `true` for is the one
    /// effect of an envelope that a later refusal cannot take back.
    #[must_use]
    pub fn a_conversation_read_it(&self) -> bool {
        !matches!(self, Self::Carried)
    }

    /// The word the run's own record carries this disposition under.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Worker => "worker",
            Self::Supervisor => "supervisor",
            Self::JudgedWith { .. } => "judged-with",
            Self::Carried => "carried",
        }
    }

    /// The parties the conversation had **already shown** the note to when it
    /// acknowledged it.
    ///
    /// Only what is confirmed at that instant, never what the conversation
    /// intends to do next. The judge's re-taken decision and its completion
    /// with the note in hand are both settled *after* the decision was taken,
    /// so the supervisor has been presented the note by the time either is
    /// answered. A worker turn reopened to carry it is acknowledged *before*
    /// that turn opens, so nothing is confirmed: the receipt would otherwise be
    /// written at submission and read afterwards as a receipt for arrival, and a
    /// dispatch cancelled between the two would leave a record asserting a
    /// presentation that never happened. What the conversation routes onward is
    /// [`routed_to`](Self::routed_to), and each presentation that then happens
    /// is recorded as its own `note-shown` when the stream shows it.
    #[must_use]
    pub fn shown_at_delivery(&self) -> &'static [Party] {
        match self {
            Self::Supervisor | Self::JudgedWith { .. } => &[Party::Supervisor],
            Self::Queued | Self::Worker | Self::Carried => &[],
        }
    }

    /// The parties the conversation said it **will** present the note to,
    /// which the acknowledgement does not confirm.
    ///
    /// A worker's reopened turn presents it to the worker and, with the worker's
    /// response, to the judge; a judge's re-taken decision rides to the worker
    /// with that decision; a queued note reaches whichever turn opens next and
    /// the other party after. A completion leaves no turn to route to, and a
    /// carried note is routed by the dispatch that composes it rather than by a
    /// conversation.
    #[must_use]
    pub fn routed_to(&self) -> &'static [Party] {
        match self {
            Self::Worker | Self::Queued => &[Party::Worker, Party::Supervisor],
            Self::Supervisor => &[Party::Worker],
            Self::JudgedWith { .. } | Self::Carried => &[],
        }
    }
}

impl From<&Accepted> for Reached {
    fn from(accepted: &Accepted) -> Self {
        match accepted {
            Accepted::Queued => Self::Queued,
            Accepted::Interrupted {
                party: Party::Worker,
            } => Self::Worker,
            Accepted::Interrupted {
                party: Party::Supervisor,
            } => Self::Supervisor,
            Accepted::JudgedWith { completion_reason } => Self::JudgedWith {
                completion_reason: completion_reason.clone(),
            },
        }
    }
}

/// What one [`deliver`] answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivered {
    /// The run recorded a disposition for it, and this is which — a party of the
    /// conversation that took it, or [`Reached::Carried`] for the note no turn
    /// took and the node's next dispatch will.
    To(Reached),
    /// Accepted and durable, and the run's reconciler had not answered it within
    /// [`REPLY_TIMEOUT_ENV`](crate::channel::REPLY_TIMEOUT_ENV). It is still
    /// queued: this is **not** an instruction to send it again.
    Queued,
}

/// Deliver one note to a node of `run` at the op's own defaults, and answer what
/// became of it.
///
/// The seam on this crate's own surface, so a caller composing this engine reaches
/// it without composing a reply envelope by hand — and so the two spellings cannot
/// mean different things, since this one *is* the envelope's `note` op, submitted
/// through the same channel and judged by the same reconciler.
///
/// The defaults are the op's: [`Deliver::Live`] with `persist` on, which attempts
/// the running turn and carries the note to the node's next dispatch where there
/// was none. [`deliver_with`] is the same call for a caller that wants one of the
/// other three combinations.
///
/// # Errors
///
/// [`Error::Refused`] when the note reached **nobody**, naming what left it
/// nowhere to go; or when the ask itself was not one this run can act on — no such
/// node, a run this process cannot read.
pub fn deliver(run: &RunPaths, node: &str, note: &Note) -> Result<Delivered> {
    deliver_with(run, node, note, Deliver::Live, true)
}

/// The same delivery, naming both axes explicitly.
///
/// `deliver` decides whether the running turn is attempted and `persist` whether
/// the note is composed into the node's next dispatch; what each of their four
/// combinations means is declared once, on
/// [`Command::Note`].
///
/// # Errors
///
/// [`deliver`]'s, plus the combination that reaches nobody by construction —
/// [`Deliver::Next`] with `persist` off is refused before the run is reached.
// llmlint: ignore[invalid_states_unrepresentable] the two axes stay two bare wire
// values here on purpose: this call *is* the envelope's `note` op on this crate's
// own surface, so a Rust caller and a JSON caller must be able to say the same
// four combinations and get the same answer to each. Narrowing the pair to
// [`Reach`] at this boundary would make the combination that reaches nobody
// inexpressible in Rust and expressible in JSON, which is the two spellings
// meaning different things — the one thing publishing this call exists to
// prevent. The narrowing happens one step in, where [`Reach::of`] refuses that
// combination for both spellings alike.
pub fn deliver_with(
    run: &RunPaths,
    node: &str,
    note: &Note,
    deliver: Deliver,
    persist: bool,
) -> Result<Delivered> {
    let envelope = Reply {
        version: Some(REPLY_ENVELOPE_VERSION),
        author: Author::Planner,
        commands: vec![Command::Note {
            id: node.to_string(),
            addressee: note.addressee,
            text: note.text.clone(),
            criterion: note.criterion.clone(),
            deliver,
            persist,
        }],
        ..Reply::default()
    };
    crate::driver::deliver_note_envelope(run, &envelope)
}

/// Where one note may land: its two axes as a pair, minus the combination that
/// lands nowhere.
///
/// `deliver` and `persist` are two independent fields on the wire and stay two —
/// [`Command::Note`] is where what each of them decides is declared, and neither
/// decides the other's question. Past the envelope they are only ever read
/// together, and one of their four combinations reaches nobody by construction:
/// this is that pair with the fourth removed, so nothing downstream of the
/// boundary that refuses it can be handed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    /// `deliver: live` with `persist: false`: the running turn and nothing else,
    /// so a note no turn took is refused.
    LiveOnly,
    /// `deliver: live` with `persist: true`: the running turn, and the node's next
    /// dispatch where there was no running turn. The op's default.
    LiveThenNext,
    /// `deliver: next` with `persist: true`: no live attempt, so the note never
    /// reaches a running turn and is always composed forward.
    NextOnly,
}

impl Reach {
    // llmlint: ignore[invalid_states_unrepresentable] this constructor is what
    // makes the invalid state unrepresentable: it takes the envelope's two bare
    // fields exactly as the wire carries them and returns the three-variant enum
    // the rule asks for, refusing the fourth. A parser of external input has to
    // accept the shape that input can have, or there is nothing left to refuse.
    /// The pair as the envelope carries it, or the one refusal the two fields
    /// decide between them.
    ///
    /// This is the envelope-time half of the reach-nobody rule, and it is composed
    /// here — beside [`reaches_nobody`] and the delivery-time half — so the two
    /// halves cannot come to word one rule differently.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] for `deliver: next` with `persist: false`, which
    /// attempts no live delivery and composes the note into no dispatch.
    pub(crate) fn of(node: &str, deliver: Deliver, persist: bool) -> Result<Self> {
        match (deliver, persist) {
            (Deliver::Live, false) => Ok(Self::LiveOnly),
            (Deliver::Live, true) => Ok(Self::LiveThenNext),
            (Deliver::Next, true) => Ok(Self::NextOnly),
            (Deliver::Next, false) => Err(reaches_nobody(
                node,
                "`deliver: next` attempts no live delivery and `persist: false` composes it \
                 into no dispatch, so this note reaches nobody whatever the run does",
            )),
        }
    }

    /// Whether the node's running turn is attempted at all.
    pub(crate) fn attempts_a_live_turn(self) -> bool {
        !matches!(self, Self::NextOnly)
    }

    /// Whether a note no running turn took is composed into the node's next
    /// dispatch.
    pub(crate) fn composes_forward(self) -> bool {
        !matches!(self, Self::LiveOnly)
    }
}

/// The one refusal a note about delivery gets: it would reach nobody, and this
/// names what left it nowhere to go.
///
/// **One rule rather than a table of special cases**, and one sentence for every
/// transport — the envelope's op and [`deliver`] — because they are one delivery.
/// It is composed here so that the two places it can be decided cannot come to
/// word it differently: the envelope, where `deliver` and `persist` decide it
/// between them, and the delivery, where only the run can.
pub(crate) fn reaches_nobody(node: &str, why: &str) -> Error {
    Error::Refused(format!("note: node '{node}': {why}"))
}

/// The note one `note` op carries, built through the seam's own constructors.
///
/// The op spells its fields rather than nesting the sibling's struct, because the
/// wire shape a planner types is this crate's to declare — but the *value* it
/// becomes is built here, through constructors that re-check text and criterion, so
/// no path assembles a note the conversation would have refused.
pub(crate) fn of(
    addressee: Addressee,
    text: &NoteText,
    criterion: Option<&Criterion>,
) -> std::result::Result<Note, NoteRefused> {
    let note = Note::new(addressee, text.as_str())?;
    match criterion {
        None => Ok(note),
        Some(criterion) => note.binding(criterion.as_str()),
    }
}

/// One note as a dispatch of the node it was for is handed it, and as the run's
/// record names it against that dispatch.
///
/// The same fields the delivery committed under
/// [`NoteDelivered`](crate::edits::Operation::NoteDelivered), carried whole rather
/// than referenced, because a note has no id of its own: the record that says a
/// dispatch was given a note has to be able to say *which*, and a reader
/// verifying that a ruling reached the party it was for reads it off this without
/// joining another record. Who was shown it is not here: that is what each
/// `note-shown` records, as the stream shows it happening.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Consumed {
    /// Whose task it said it was updating.
    pub addressee: Addressee,
    /// What that party read.
    pub text: NoteText,
    /// The criterion it bound, when it bound one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criterion: Option<Criterion>,
    /// What became of it when it was delivered.
    #[serde(flatten)]
    pub reached: Reached,
}

/// The payload key under which a `node-dispatched` names the notes the dispatch
/// it announces was **composed with**: each was read by an earlier dispatch of
/// the node, or carried to this one, and this dispatch's task carries it.
pub(crate) const CARRIED_KEY: &str = "notes_carried";

/// The payload key under which a `node-dispatched` names the notes an earlier
/// dispatch of the node read that this dispatch was **not** composed with.
///
/// The other half of [`CARRIED_KEY`], and the one a manager reads: a note that
/// reached a conversation is consumed by it, and a dispatch composed without it
/// has spent it. The receipt for the delivery named the party that read it, which
/// looks like success; this is the record that says the ruling did not survive
/// the dispatch it was issued during, so the manager knows to re-issue it rather
/// than believe the receipt.
pub(crate) const SPENT_KEY: &str = "notes_spent";

/// The value a `node-dispatched` carries a list of notes as.
pub(crate) fn payload_of(notes: &[Consumed]) -> Value {
    serde_json::to_value(notes).unwrap_or_else(|_| Value::Array(Vec::new()))
}

/// Where one standing note sits relative to the node's current dispatch, which
/// decides whether that dispatch's conversation has read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    /// Composed into the current dispatch's task, so read by both of its parties
    /// whatever the disposition it was first delivered under.
    ComposedIntoIt,
    /// Delivered since the current dispatch was composed; whether a party read
    /// it is its own disposition's to say.
    DeliveredSince,
}

/// One standing note and where it sits — exactly one place, so a note cannot be
/// both composed into the dispatch and delivered after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Held {
    pub note: Consumed,
    pub placement: Placement,
}

impl Held {
    /// Whether a conversation of the node's current dispatch has read this note.
    fn read(&self) -> bool {
        match self.placement {
            Placement::ComposedIntoIt => true,
            Placement::DeliveredSince => self.note.reached.a_conversation_read_it(),
        }
    }
}

/// The notes a node's **current** dispatch holds, as the run's record has them:
/// what that dispatch was composed with, and every note the run delivered to
/// the node since — each in exactly one of those two places.
///
/// Folded off the journal rather than kept in memory, because the two readers of
/// it are on different threads and neither owns the answer: the dispatch thread
/// composing the node's next attempt, and the reconcile loop about to announce a
/// dispatch that was composed without them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Standing {
    /// Every note, in the order the run recorded them.
    pub held: Vec<Held>,
}

impl Standing {
    /// Every note, as the node's next dispatch is composed with all of them.
    pub(crate) fn notes(&self) -> Vec<Consumed> {
        self.held.iter().map(|held| held.note.clone()).collect()
    }

    /// The notes a conversation of the node's current dispatch has read.
    pub(crate) fn read(&self) -> Vec<Consumed> {
        self.held
            .iter()
            .filter(|held| held.read())
            .map(|held| held.note.clone())
            .collect()
    }
}

/// Fold what stands for `node` out of the run's journal.
///
/// A `node-dispatched` resets the fold to what that dispatch was composed with —
/// [`CARRIED_KEY`], or nothing for a dispatch composed with none — and a
/// committed `note` adds its note. Both are this crate's own records, and both
/// are **refused** rather than read past where they cannot be read: a
/// `notes_carried` that is not a list of notes, or an operation list this build
/// cannot parse, would otherwise decide *by its absence* which rulings a dispatch
/// is composed with, which is the silent loss this fold exists to end. The
/// refusal names the record, so a reader is sent at the line rather than at the
/// run.
///
/// # Errors
///
/// [`Error::Invalid`] naming the record that could not be read as what its kind
/// says it is.
pub(crate) fn standing(journal: &[Envelope], node: &str) -> Result<Standing> {
    let mut standing = Standing::default();
    for envelope in journal {
        // A dispatch is stamped with its node; a committed edit is not — it may
        // touch several — so the note inside it is matched on its own `node`.
        if envelope.kind.0 == crate::event::PipelineKind::NodeDispatched.as_str() {
            if envelope.labels.node.as_deref() != Some(node) {
                continue;
            }
            let composed: Vec<Consumed> = match envelope.payload.get(CARRIED_KEY) {
                None => Vec::new(),
                Some(carried) => serde_json::from_value(carried.clone())
                    .map_err(|error| unreadable_record(envelope, CARRIED_KEY, &error))?,
            };
            standing.held = composed
                .into_iter()
                .map(|note| Held {
                    note,
                    placement: Placement::ComposedIntoIt,
                })
                .collect();
            continue;
        }
        let is_a_commit = [
            crate::event::PipelineKind::EditCommitted,
            crate::event::PipelineKind::CommandAccepted,
        ]
        .iter()
        .any(|kind| envelope.kind.0 == kind.as_str());
        if !is_a_commit || envelope.source != crate::event::Source::Pipeline {
            continue;
        }
        let Some(operations) = envelope.payload.get("operations") else {
            continue;
        };
        let operations: Vec<crate::edits::Operation> =
            serde_json::from_value(operations.clone())
                .map_err(|error| unreadable_record(envelope, "operations", &error))?;
        for operation in operations {
            let crate::edits::Operation::NoteDelivered {
                node: whose,
                addressee,
                text,
                criterion,
                reached,
                ..
            } = operation
            else {
                continue;
            };
            if whose == node {
                standing.held.push(Held {
                    note: Consumed {
                        addressee,
                        text,
                        criterion,
                        reached,
                    },
                    placement: Placement::DeliveredSince,
                });
            }
        }
    }
    Ok(standing)
}

/// The refusal for a record of this crate's own that cannot be read as what its
/// kind says it carries.
fn unreadable_record(envelope: &Envelope, field: &str, error: &serde_json::Error) -> Error {
    Error::Invalid(format!(
        "the run's record of the notes delivered to its nodes cannot be read: `{}` record \
         {}/{} carries a `{field}` this build cannot read ({error}), so which notes a \
         dispatch is composed with cannot be decided from it",
        envelope.kind.0, envelope.stream, envelope.seq
    ))
}

/// The same, read off the run's own journal.
///
/// # Errors
///
/// [`standing`]'s.
pub(crate) fn standing_for(paths: &RunPaths, node: &str) -> Result<Standing> {
    standing(&crate::journal::read(&paths.journal()), node)
}

/// One note a conversation has been told to present, and to whom it has not yet
/// been shown.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Routed {
    note: Consumed,
    /// The parties still owed a presentation.
    awaiting: Vec<Party>,
    /// Whether the worker has to be shown it before the supervisor can be: true
    /// for a note that rides a reopened worker turn — the judge receives it with
    /// the worker's response, so a supervisor turn opening *before* that worker
    /// turn was not shown it — and for one composed into a dispatch's task; false
    /// for a queued note, which whichever turn opens next takes.
    worker_first: bool,
    /// Whether only a worker turn the producer stamps as **delivered** counts as
    /// the worker's presentation. True for every note a conversation routed;
    /// false for one composed into the dispatch's task, which the opening turn
    /// carries as the task itself.
    delivered_turn_only: bool,
    /// The instant this routing was recorded, against which a relayed turn is
    /// read: a turn that opened before it cannot be the presentation of it.
    routed_at: u64,
    /// The worker turn that presented it, once one has.
    worker_turn: Option<u64>,
}

/// One presentation the stream showed happening.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shown {
    /// Who was shown the note.
    pub party: Party,
    /// The turn of theirs that opened carrying it.
    pub turn: u64,
    /// The note.
    pub note: Consumed,
}

impl Shown {
    /// The payload a `note-shown` carries.
    pub(crate) fn payload(&self) -> serde_json::Map<String, Value> {
        let mut payload = match serde_json::to_value(&self.note) {
            Ok(Value::Object(note)) => note,
            _ => serde_json::Map::new(),
        };
        payload.insert("party".into(), json_party(self.party));
        payload.insert("turn".into(), Value::from(self.turn));
        payload
    }
}

fn json_party(party: Party) -> Value {
    serde_json::to_value(party).unwrap_or(Value::Null)
}

/// What one dispatch's conversation has been told to present and has not yet
/// been seen presenting.
///
/// Kept by the writer that relays the conversation's stream, per in-flight
/// dispatch, so a presentation is recorded when the stream shows it and never
/// when it is merely intended. The stream is the producer's own account: a
/// worker turn the producer stamps `delivered` is the turn that opened on a
/// note, and a supervisor turn that opens after it is one the judge takes with
/// every delivered note in hand. A dispatch that ends between the two — cancelled,
/// or a worker turn that fails — drops this with it, and the record keeps only
/// the presentations that happened.
#[derive(Debug, Clone, Default)]
pub(crate) struct Presentations {
    routed: Vec<Routed>,
}

impl Presentations {
    /// A note a conversation acknowledged and routed onward, as
    /// [`Reached::routed_to`] says.
    pub(crate) fn routed_by_the_conversation(&mut self, note: Consumed, at: u64) {
        let awaiting = note.reached.routed_to().to_vec();
        if awaiting.is_empty() {
            return;
        }
        let worker_first = !matches!(note.reached, Reached::Queued);
        self.routed.push(Routed {
            note,
            awaiting,
            worker_first,
            delivered_turn_only: true,
            routed_at: at,
            worker_turn: None,
        });
    }

    /// A note composed into the dispatch's own task, which its opening worker
    /// turn carries and every supervisor turn after that reads.
    pub(crate) fn composed_into_the_task(&mut self, note: Consumed, at: u64) {
        self.routed.push(Routed {
            note,
            awaiting: vec![Party::Worker, Party::Supervisor],
            worker_first: true,
            delivered_turn_only: false,
            routed_at: at,
            worker_turn: None,
        });
    }

    /// Read one relayed envelope of the conversation, and answer every
    /// presentation it shows happening.
    ///
    /// Only a `turn-started` the producer published, opened no earlier than the
    /// routing it would confirm, on the member the notes were addressed to. The
    /// caller decides the member; this reads the role, the origin and the turn
    /// off the payload the producer wrote — by name, because this crate relays
    /// that payload as an opaque map and reads no variant of it otherwise.
    pub(crate) fn observe(&mut self, envelope: &Envelope) -> Vec<Shown> {
        if self.routed.is_empty()
            || envelope.source != crate::event::Source::Agentgraph
            || envelope.kind.0 != oneagentgraph::event::EventKind::TurnStarted.as_str()
        {
            return Vec::new();
        }
        let (Some(role), Some(turn), Some(opened_at)) = (
            envelope.payload.get("role").and_then(Value::as_str),
            envelope.payload.get("turn").and_then(Value::as_u64),
            crate::projection::millis_of(&envelope.ts),
        ) else {
            return Vec::new();
        };
        let delivered = envelope.payload.get("origin").and_then(Value::as_str)
            == Some(oneagentgraph::event::Origin::Delivered.as_str());
        let party = if role == oneagentgraph::event::Party::Assistant.as_str() {
            Party::Worker
        } else if role == oneagentgraph::event::Party::User.as_str() {
            Party::Supervisor
        } else {
            return Vec::new();
        };
        let mut shown = Vec::new();
        for routed in &mut self.routed {
            if opened_at < routed.routed_at || !routed.awaiting.contains(&party) {
                continue;
            }
            let presents = match party {
                Party::Worker => delivered || !routed.delivered_turn_only,
                // The supervisor answers the worker's reply under that reply's
                // own turn number, so the turn that reads a note the worker's
                // turn carried is numbered as that turn, not after it.
                Party::Supervisor => match routed.worker_turn {
                    Some(worker_turn) => turn >= worker_turn,
                    None => !routed.worker_first || !routed.awaiting.contains(&Party::Worker),
                },
            };
            if !presents {
                continue;
            }
            routed.awaiting.retain(|owed| *owed != party);
            if party == Party::Worker {
                routed.worker_turn = Some(turn);
            }
            shown.push(Shown {
                party,
                turn,
                note: routed.note.clone(),
            });
        }
        self.routed.retain(|routed| !routed.awaiting.is_empty());
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edits::Operation;
    use crate::event::{Labels, Source, ENVELOPE_VERSION};
    use crate::journal::{self, labels, payload};
    use serde_json::json;

    fn pipeline(
        kind: journal::PipelineKind,
        seq: u64,
        node: Option<&str>,
        fields: &[(&str, Value)],
    ) -> Envelope {
        Envelope {
            v: ENVELOPE_VERSION,
            ts: crate::sys::rfc3339_from_millis(1_786_000_000_000 + seq * 1_000),
            stream: "s".into(),
            seq,
            source: Source::Pipeline,
            kind: kind.into(),
            phase: None,
            labels: Labels {
                node: node.map(str::to_string),
                ..labels("demo", None)
            },
            payload: payload(fields),
            artifacts: Vec::new(),
        }
    }

    /// One committed `note`, as the reconciler journals it: unlabelled, because a
    /// committed edit may touch several nodes, with the node on the operation.
    fn delivered(seq: u64, node: &str, text: &str, reached: Reached) -> Envelope {
        pipeline(
            journal::PipelineKind::EditCommitted,
            seq,
            None,
            &[(
                "operations",
                json!([Operation::NoteDelivered {
                    node: node.into(),
                    addressee: Addressee::Worker,
                    text: text.parse().expect("a usable note"),
                    criterion: None,
                    shown_to: reached.shown_at_delivery().to_vec(),
                    routed_to: reached.routed_to().to_vec(),
                    reached,
                }]),
            )],
        )
    }

    fn texts(notes: &[Consumed]) -> Vec<&str> {
        notes.iter().map(|note| note.text.as_str()).collect()
    }

    /// What stands for a node is what its last dispatch was composed with plus
    /// what reached it since — and nothing from before that dispatch, which a
    /// record composed without it already spent.
    #[test]
    fn what_stands_for_a_node_starts_at_its_last_dispatch_and_reads_forward() {
        let journal = vec![
            pipeline(
                journal::PipelineKind::NodeDispatched,
                1,
                Some("build"),
                &[("attempt", json!(1))],
            ),
            delivered(2, "build", "first ruling", Reached::Worker),
            // Another node's note, on an unlabelled record like every committed
            // edit: matched on the operation's own node, so it stays out.
            delivered(3, "other", "not yours", Reached::Worker),
            delivered(4, "build", "landed nowhere", Reached::Carried),
        ];
        let before = standing(&journal, "build").expect("the record reads");
        assert_eq!(texts(&before.notes()), ["first ruling", "landed nowhere"]);
        // Read by this dispatch's conversation: the one a turn took. The one
        // that landed nowhere is owed forward and not yet read by anybody.
        assert_eq!(texts(&before.read()), ["first ruling"]);

        let composed = payload_of(&before.notes());
        assert_eq!(composed[1]["reached"], json!("carried"));
        assert!(
            composed[0].get("shown_to").is_none(),
            "a dispatch's record claimed a presentation it has not made: {composed}"
        );

        // A continuation composed with them resets the fold to exactly them, and
        // a dispatch composed with none resets it to nothing: what an earlier
        // dispatch read is spent by the first dispatch that does not carry it.
        let mut continued = journal.clone();
        continued.push(pipeline(
            journal::PipelineKind::NodeDispatched,
            5,
            Some("build"),
            &[("attempt", json!(2)), (CARRIED_KEY, composed)],
        ));
        continued.push(delivered(6, "build", "second ruling", Reached::Supervisor));
        let after = standing(&continued, "build").expect("the record reads");
        assert_eq!(
            texts(&after.notes()),
            ["first ruling", "landed nowhere", "second ruling"]
        );
        // The note that landed nowhere was composed into this dispatch's task,
        // so its conversation has read it: nothing is owed forward, and a
        // dispatch composed without it would spend it.
        assert_eq!(
            texts(&after.read()),
            ["first ruling", "landed nowhere", "second ruling"]
        );

        let mut fresh = continued.clone();
        fresh.push(pipeline(
            journal::PipelineKind::NodeDispatched,
            7,
            Some("build"),
            &[("attempt", json!(1))],
        ));
        assert_eq!(
            standing(&fresh, "build").expect("the record reads"),
            Standing::default()
        );
    }

    /// A record of this crate's own that cannot be read as what it says it
    /// carries is refused, naming the record — never read past, because a
    /// dispatch composed without the notes it cannot read is the silent loss the
    /// fold exists to end.
    #[test]
    fn a_record_the_fold_cannot_read_is_refused_by_name_rather_than_read_past() {
        let carried_wrong = vec![pipeline(
            journal::PipelineKind::NodeDispatched,
            1,
            Some("build"),
            &[("attempt", json!(2)), (CARRIED_KEY, json!("a ruling"))],
        )];
        let refused =
            standing(&carried_wrong, "build").expect_err("a string is not a list of notes");
        let said = refused.to_string();
        assert!(
            said.contains("node-dispatched") && said.contains("s/1") && said.contains(CARRIED_KEY),
            "the refusal does not name the record or the field: {said}"
        );

        let operations_wrong = vec![pipeline(
            journal::PipelineKind::EditCommitted,
            2,
            None,
            &[("operations", json!([{"kind": "from-the-future"}]))],
        )];
        let refused = standing(&operations_wrong, "build")
            .expect_err("an operation this build does not know is not read past");
        assert!(
            refused.to_string().contains("`operations`"),
            "the refusal does not name the field: {refused}"
        );

        // A sibling's record is not this crate's to read, whatever it is called.
        let mut foreign = pipeline(
            journal::PipelineKind::EditCommitted,
            3,
            None,
            &[("operations", json!("not ours"))],
        );
        foreign.source = Source::Agentgraph;
        assert_eq!(
            standing(&[foreign], "build").expect("a sibling's record is passed over"),
            Standing::default()
        );
    }

    /// What each disposition confirms at the acknowledgement and what it only
    /// routes onward, written on the record as two facts rather than one.
    #[test]
    fn each_disposition_tells_a_confirmed_presentation_from_a_routed_one() {
        assert!(Reached::Worker.shown_at_delivery().is_empty());
        assert_eq!(
            Reached::Worker.routed_to(),
            [Party::Worker, Party::Supervisor]
        );
        assert_eq!(Reached::Supervisor.shown_at_delivery(), [Party::Supervisor]);
        assert_eq!(Reached::Supervisor.routed_to(), [Party::Worker]);
        let judged = Reached::JudgedWith {
            completion_reason: "done".into(),
        };
        assert_eq!(judged.shown_at_delivery(), [Party::Supervisor]);
        assert!(judged.routed_to().is_empty());
        assert!(Reached::Queued.shown_at_delivery().is_empty());
        assert_eq!(
            Reached::Queued.routed_to(),
            [Party::Worker, Party::Supervisor]
        );
        assert!(Reached::Carried.shown_at_delivery().is_empty());
        assert!(Reached::Carried.routed_to().is_empty());

        // And on the wire each field is omitted where it is empty, so a record
        // of a carried note reads exactly as it did before either field.
        let record = |reached: Reached| Operation::NoteDelivered {
            node: "build".into(),
            addressee: Addressee::Both,
            text: "ship it".parse().expect("a usable note"),
            criterion: None,
            shown_to: reached.shown_at_delivery().to_vec(),
            routed_to: reached.routed_to().to_vec(),
            reached,
        };
        let wire = serde_json::to_value(record(Reached::Carried)).expect("it serializes");
        assert!(
            wire.get("shown_to").is_none() && wire.get("routed_to").is_none(),
            "{wire}"
        );
        let wire = serde_json::to_value(record(Reached::Worker)).expect("it serializes");
        assert!(wire.get("shown_to").is_none(), "{wire}");
        assert_eq!(wire["routed_to"], json!(["worker", "supervisor"]), "{wire}");
        let wire = serde_json::to_value(record(Reached::Supervisor)).expect("it serializes");
        assert_eq!(wire["shown_to"], json!(["supervisor"]), "{wire}");
        assert_eq!(wire["routed_to"], json!(["worker"]), "{wire}");
        assert_eq!(
            serde_json::from_value::<Operation>(wire).expect("it reads back"),
            record(Reached::Supervisor)
        );
    }

    /// A relayed turn, as the producer publishes it.
    fn turn(seq: u64, at: u64, role: &str, turn: u64, origin: Option<&str>) -> Envelope {
        let mut payload = payload(&[
            ("turn", json!(turn)),
            ("role", json!(role)),
            ("instruction", json!("do it")),
            ("started_at", json!(crate::sys::rfc3339_from_millis(at))),
        ]);
        if let Some(origin) = origin {
            payload.insert("origin".into(), json!(origin));
        }
        Envelope {
            v: 1,
            ts: crate::sys::rfc3339_from_millis(at),
            stream: "node-scope-1".into(),
            seq,
            source: Source::Agentgraph,
            kind: crate::event::EventKind(
                oneagentgraph::event::EventKind::TurnStarted.as_str().into(),
            ),
            phase: None,
            labels: labels("demo", Some("build")),
            payload,
            artifacts: Vec::new(),
        }
    }

    fn consumed(text: &str, reached: Reached) -> Consumed {
        Consumed {
            addressee: Addressee::Both,
            text: text.parse().expect("a usable note"),
            criterion: None,
            reached,
        }
    }

    /// A presentation is recorded when the stream shows it and not before: a
    /// note the worker's reopened turn carries is shown to the worker by the
    /// turn the producer stamps `delivered`, and to the supervisor by the turn
    /// that answers it — never by a turn that opened before the note was
    /// offered, and never to the supervisor ahead of the worker.
    ///
    /// The shape an interrupted conversation leaves is the unit half of what
    /// `tests/note` drives against a real one: the worker's turn shown and the
    /// supervisor's never confirmed, because nothing here invents it.
    #[test]
    fn a_presentation_is_recorded_when_the_stream_shows_it_and_in_the_producers_order() {
        let mut watch = Presentations::default();
        watch.routed_by_the_conversation(consumed("stop", Reached::Worker), 1_000);

        // A supervisor turn that opened before the note was offered, and a
        // worker turn that opened before it too, confirm nothing — whatever
        // order the writer meets them in.
        assert!(watch.observe(&turn(1, 900, "user", 1, None)).is_empty());
        assert!(watch
            .observe(&turn(2, 950, "assistant", 1, Some("task")))
            .is_empty());
        // Nor does a worker turn after it that the producer does not stamp as a
        // delivery: that turn opened on something else.
        assert!(watch
            .observe(&turn(3, 1_100, "assistant", 2, Some("supervisor")))
            .is_empty());
        // Nor a supervisor turn ahead of the worker's delivered one.
        assert!(watch.observe(&turn(4, 1_200, "user", 2, None)).is_empty());

        let shown = watch.observe(&turn(5, 1_300, "assistant", 3, Some("delivered")));
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert_eq!(shown[0].party, Party::Worker);
        assert_eq!(shown[0].turn, 3);
        assert_eq!(shown[0].payload()["party"], json!("worker"));
        assert_eq!(shown[0].payload()["text"], json!("stop"));
        assert_eq!(shown[0].payload()["reached"], json!("worker"));

        // The supervisor answers that reply under the same turn number, and
        // that is the presentation; a second supervisor turn confirms nothing
        // twice.
        let shown = watch.observe(&turn(6, 1_400, "user", 3, None));
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert_eq!(shown[0].party, Party::Supervisor);
        assert!(watch.observe(&turn(7, 1_500, "user", 4, None)).is_empty());

        // A note the judge re-took its decision with is confirmed to the judge at
        // delivery and owed only to the worker, by the delivered turn that rides
        // the decision.
        let mut watch = Presentations::default();
        watch.routed_by_the_conversation(consumed("ruling", Reached::Supervisor), 2_000);
        assert!(watch.observe(&turn(8, 2_100, "user", 5, None)).is_empty());
        let shown = watch.observe(&turn(9, 2_200, "assistant", 6, Some("delivered")));
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].party, Party::Worker);

        // One composed into the task is carried by the opening turn as the
        // task, and read by the supervisor that answers it.
        let mut watch = Presentations::default();
        watch.composed_into_the_task(consumed("carried in", Reached::Carried), 3_000);
        let shown = watch.observe(&turn(10, 3_100, "assistant", 1, Some("task")));
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].party, Party::Worker);
        let shown = watch.observe(&turn(11, 3_200, "user", 1, None));
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].party, Party::Supervisor);

        // Nothing the conversation routes nowhere is watched at all.
        let mut watch = Presentations::default();
        watch.routed_by_the_conversation(
            consumed(
                "passed",
                Reached::JudgedWith {
                    completion_reason: "done".into(),
                },
            ),
            4_000,
        );
        assert!(watch
            .observe(&turn(12, 4_100, "assistant", 7, Some("delivered")))
            .is_empty());
    }

    /// The names this module writes are the ones divergence entry 69 states, so
    /// the record and the document cannot drift apart: the two payload keys, the
    /// field a delivery is stamped with, and the heading a re-dispatch renders
    /// the notes under.
    ///
    /// Read here rather than in `tests/contract.rs` because the constants are
    /// the crate's own and not part of its published surface; the table of what
    /// each disposition confirms and routes is held there, through the public
    /// [`Reached::shown_at_delivery`] and [`Reached::routed_to`].
    #[test]
    fn the_names_this_module_writes_are_the_ones_the_divergence_record_states() {
        let record = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with("69."))
            .expect("the record carries entry 69");
        let block: Value = entry
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .map(|block| serde_json::from_str(block).expect("entry 69's block is JSON"))
            .expect("entry 69 carries the json block this test drives");
        assert_eq!(
            block["node_dispatched_keys"],
            json!([CARRIED_KEY, SPENT_KEY]),
            "the keys a `node-dispatched` carries are not the ones entry 69 names"
        );
        assert_eq!(
            block["heading"],
            json!(crate::plan::MANAGER_NOTES_HEADING),
            "the heading a re-dispatch renders the notes under is not the one entry 69 names"
        );
        // The field name is read off what the operation really writes rather
        // than off a constant, because serde's attribute is the one source of it.
        let written = serde_json::to_value(Operation::NoteDelivered {
            node: "build".into(),
            addressee: Addressee::Worker,
            text: "ship it".parse().expect("a usable note"),
            criterion: None,
            shown_to: Reached::Supervisor.shown_at_delivery().to_vec(),
            routed_to: Reached::Supervisor.routed_to().to_vec(),
            reached: Reached::Supervisor,
        })
        .expect("it serializes");
        let fields: Vec<String> = serde_json::from_value(block["note_delivered_fields"].clone())
            .expect("entry 69 names the fields");
        for field in &fields {
            assert!(
                written.get(field).is_some(),
                "a delivery is not stamped under the field entry 69 names (`{field}`): {written}"
            );
        }
        assert_eq!(
            block["event_kinds"],
            json!([crate::event::PipelineKind::NoteShown.as_str()]),
            "the kind a presentation is recorded under is not the one entry 69 names"
        );
    }
}
