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

pub use oneagentgraph::note::{
    Addressee, Criterion, Note, NoteRefused, NoteText, Party, Undelivered,
};

use crate::channel::{Author, Command, Deliver, Reply, REPLY_ENVELOPE_VERSION};
use crate::error::{Error, Result};
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
