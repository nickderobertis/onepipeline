//! The manager-note delivery seam: one note that reaches whichever party of a
//! node's live dispatch is speaking, and reaches the other with its response.
//!
//! A manager correcting a node in flight has had two levers and neither did both
//! halves of the job. [`Context`](crate::channel::Command::Context) is delivered by
//! interrupting the live agent turn, so it reaches the worker and never the judge,
//! and it binds nothing. [`Amend`](crate::channel::Command::Amend) replaces the
//! node's binding amendment and so binds the judge, but composes the task of the
//! *next* dispatch and cannot reach the turn running now. A manager who needed a
//! correction to reach both had to kill a running dispatch to get it: one ruling
//! delivered at 15:50:23Z was contradicted by that node's own judge at 15:57:19Z,
//! reviewing against a task that never mentioned it.
//!
//! A **note** is the lever that does both, and none of its routing is this crate's:
//! the two-party conversation belongs to `onejudge`, the member running it to
//! `oneagentgraph`, and the shapes below are that seam's own re-exported rather
//! than restated. What this crate owns is which node a note is for, getting it to
//! that node's live member, and putting what came back into the run's record.
//!
//! * It is delivered to **whoever is live** — the worker's turn, the judge's turn,
//!   or, between turns, the next turn to open — and the other party receives it
//!   with that party's response.
//! * The party that receives it is told **which role it is for** ([`Addressee`]),
//!   so a judge handed an update to the *worker's* task does not take the worker's
//!   job on.
//! * A note may carry a [`Criterion`], and a delivered one enters the acceptance
//!   criteria the judge evaluates against rather than appearing only as narration.
//! * **Undelivered is an error.** A note arriving once the node's dispatch has
//!   completed does not settle quietly into a record nobody reads: it is refused,
//!   naming that it was not delivered and why, so the caller chooses relaunch,
//!   tweak, or follow-up. That refusal is the same treatment `deliver: live`
//!   already gives a note it cannot deliver, extended to the case that was silent.
//!
//! `context` and `amend` keep their names and their meanings: a caller using
//! either today is answered exactly as it was.
//!
//! # What a note does *not* do
//!
//! It does not move the node's stored bar. A criterion it binds is in force for the
//! conversation it was delivered into, which is the conversation whose verdict the
//! manager is correcting; [`Amend`](crate::channel::Command::Amend) is still the
//! lever for a bar that has to survive a re-dispatch, and the two are deliberately
//! not the same op.

use oneagentgraph::note::Accepted;
use serde::{Deserialize, Serialize};

pub use oneagentgraph::note::{
    Addressee, Criterion, Note, NoteRefused, NoteText, Party, Undelivered,
};

use crate::channel::{Author, Command, Reply, REPLY_ENVELOPE_VERSION};
use crate::error::{Error, Result};
use crate::views::RunPaths;

/// Which party of the node's conversation took a note, as the run records it.
///
/// This crate's own spelling of [`oneagentgraph::note::Accepted`], and it exists
/// for one reason: the answer is written into the run's journal, and that library's
/// enum is deliberately not serializable — what crosses a boundary is the
/// transport's decision rather than the conversation's. The mapping below is
/// exhaustive in both directions, so a disposition added upstream fails this build
/// instead of being dropped on the way into the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reached", rename_all = "kebab-case")]
pub enum Reached {
    /// Nobody was taking a turn, so the next turn to open takes it.
    Queued,
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
    /// The conversation took it, and this is which party did.
    To(Reached),
    /// Accepted and durable, and the run's reconciler had not answered it within
    /// [`REPLY_TIMEOUT_ENV`](crate::channel::REPLY_TIMEOUT_ENV). It is still
    /// queued: this is **not** an instruction to send it again.
    Queued,
}

/// Deliver one note to a node of `run`, and answer what became of it.
///
/// The seam on this crate's own surface, so a caller composing this engine reaches
/// it without composing a reply envelope by hand — and so the two spellings cannot
/// mean different things, since this one *is* the envelope's `note` op, submitted
/// through the same channel and judged by the same reconciler.
///
/// # Errors
///
/// [`Error::Refused`] when the note was **not delivered**, carrying the
/// conversation's own [`Undelivered`] sentence and what the caller can do instead;
/// or when the ask itself was not one this run can act on — no such node, a run
/// this process cannot read.
pub fn deliver(run: &RunPaths, node: &str, note: &Note) -> Result<Delivered> {
    let envelope = Reply {
        version: Some(REPLY_ENVELOPE_VERSION),
        author: Author::Planner,
        commands: vec![Command::Note {
            id: node.to_string(),
            addressee: note.addressee,
            text: note.text.clone(),
            criterion: note.criterion.clone(),
        }],
        ..Reply::default()
    };
    crate::driver::deliver_note_envelope(run, &envelope)
}

/// The refusal a note that reached nobody is answered with.
///
/// One sentence for both transports — the envelope's op and [`deliver`] — because
/// they are one delivery: the conversation's own words about why it will never be
/// read, under the node it was addressed to.
pub(crate) fn undelivered(node: &str, why: &Undelivered) -> Error {
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
