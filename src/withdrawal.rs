//! `withdraw-note`: taking back a manager's note before any turn reads it.
//!
//! A note is named by where it was sent — an envelope's id and the note's
//! position among that envelope's commands — and what can be done with it is
//! read off the run's own record of it: the `note-delivered` its delivery
//! committed, the answer its envelope was given, and whether a dispatch has been
//! composed with it since.
//!
//! **This build withdraws carried notes only.** A note still waiting in its
//! live conversation's inbox is held by a delivery that is waiting on it, and
//! the offer `oneagentgraph` hands back cannot be taken back while it is waited
//! on, so such a withdrawal is refused naming that, and the note's own delivery
//! carries on untouched. `docs/contract-divergences.md` states the interim and
//! what retires it.

use crate::channel::{ChannelState, Command, CommandVerdict};
use crate::edits::Operation;
use crate::error::{Error, Result};
use crate::event::Envelope;
use crate::note::{Placement, Reached, WithdrawnFrom};
use crate::views::RunPaths;

/// Validate one `withdraw-note` against the run's record, and compile the one
/// operation it commits.
///
/// # Errors
///
/// [`Error::Refused`] naming why the note cannot be withdrawn: there is no such
/// envelope or no note at that position; the note was already answered, already
/// composed into a dispatch, or already withdrawn; or it is still being
/// delivered live, which this build cannot take back.
pub(crate) fn validate(
    paths: &RunPaths,
    envelope: &str,
    index: u32,
    reason: &str,
) -> Result<Vec<Operation>> {
    let refuse = |why: String| Error::Refused(format!("withdraw-note: {why}"));
    if reason.trim().is_empty() {
        return Err(refuse(
            "`reason` is blank; say why the note is withdrawn".to_string(),
        ));
    }
    let Ok(id) = envelope.trim().parse::<u64>() else {
        return Err(refuse(format!(
            "'{envelope}' is not an envelope id: the run numbers its envelopes 0, 1, 2, …"
        )));
    };
    let channel = ChannelState::new(paths);
    let Some(sent) = channel.commands_of(id) else {
        return Err(refuse(format!("the run holds no envelope {id}")));
    };
    let position = usize::try_from(index).unwrap_or(usize::MAX);
    let node = match sent.commands.get(position) {
        Some(Command::Note { id: node, .. }) => node.clone(),
        Some(other) => {
            return Err(refuse(format!(
                "command {index} of envelope {id} is a '{}', not a note",
                crate::channel::op_of(other)
            )))
        }
        None => {
            return Err(refuse(format!(
                "envelope {id} carries {} command(s), so it holds no note at index {index}",
                sent.commands.len()
            )))
        }
    };
    let named = format!("note {index} of envelope {id}, to node '{node}'");
    let journal = crate::journal::read(&paths.journal());
    if let Some(by) = withdrawn_by(&journal, id, index) {
        return Err(refuse(format!(
            "{named} was already withdrawn by envelope {by}"
        )));
    }
    match delivered(&journal, id, index) {
        Some(Reached::Carried { .. }) => {
            let standing = crate::note::standing(&journal, &node)?;
            let owed = standing.held.iter().any(|held| {
                held.placement == Placement::DeliveredSince && held.sent == Some((id, index))
            });
            if !owed {
                return Err(refuse(format!(
                    "{named} was carried and has already been composed into a dispatch of \
                     that node, which read it"
                )));
            }
            let Some(Command::Note { text, .. }) = sent.commands.get(position) else {
                return Err(refuse(format!("{named} is no longer a note")));
            };
            Ok(vec![Operation::NoteWithdrawn {
                node,
                envelope: id,
                index,
                reason: reason.to_string(),
                from: WithdrawnFrom::Carried,
                text: text.clone(),
            }])
        }
        Some(reached) => Err(refuse(format!(
            "{named} was already answered {}: a turn took it, and a conversation has no undo",
            disposition(&reached)
        ))),
        None => match channel
            .outcome_of(id)
            .and_then(|outcome| outcome.results.into_iter().nth(position))
        {
            Some(result) if result.outcome == CommandVerdict::Refused => Err(refuse(format!(
                "{named} was already answered refused: {}",
                result.reason.as_deref().unwrap_or("no reason was recorded")
            ))),
            Some(result) => Err(refuse(format!(
                "{named} was already answered `{}`, and the run's record holds no carried \
                 note of it to take back",
                verdict(result.outcome)
            ))),
            None => Err(refuse(format!(
                "{named} is still being delivered to that node's live conversation, and a \
                 live note cannot be withdrawn yet: withdrawing it waits on oneagentgraph \
                 making a note offer withdrawable while it is waited on. Its delivery is left \
                 as it was"
            ))),
        },
    }
}

/// Answer `sent` again now that its note at `index` was withdrawn by envelope
/// `by`: that command refused, carrying `reason`, and every other command's
/// answer as it stood.
///
/// An envelope that was never answered is left alone: its answer, when it comes,
/// is its own.
///
/// # Errors
///
/// The channel's, where the answer cannot be written.
pub(crate) fn answer_withdrawn(
    paths: &RunPaths,
    sent: u64,
    index: u32,
    reason: &str,
    by: u64,
) -> Result<()> {
    let channel = ChannelState::new(paths);
    let Some(mut outcome) = channel.outcome_of(sent) else {
        return Ok(());
    };
    let position = usize::try_from(index).unwrap_or(usize::MAX);
    let Some(result) = outcome.results.get_mut(position) else {
        return Ok(());
    };
    let why = format!("withdrawn by envelope {by}: {reason}");
    result.outcome = CommandVerdict::Refused;
    result.reason = Some(why.clone());
    result.refusal = None;
    result.turn = None;
    outcome.applied = false;
    outcome.reason = Some(format!(
        "command {index} ('note') was {why}; every other command's answer stands"
    ));
    channel.answer_commands(&outcome)
}

/// The envelope that already withdrew note `index` of envelope `sent`, if one did.
fn withdrawn_by(journal: &[Envelope], sent: u64, index: u32) -> Option<u64> {
    committed(journal).find_map(|(envelope, operation)| match operation {
        Operation::NoteWithdrawn {
            envelope: from,
            index: at,
            ..
        } if from == sent && at == index => envelope,
        _ => None,
    })
}

/// What the delivery of note `index` of envelope `sent` recorded, if it recorded
/// anything.
fn delivered(journal: &[Envelope], sent: u64, index: u32) -> Option<Reached> {
    committed(journal).find_map(|(_, operation)| match operation {
        Operation::NoteDelivered {
            envelope: Some(from),
            index: Some(at),
            reached,
            ..
        } if from == sent && at == index => Some(reached),
        _ => None,
    })
}

/// Every operation the run committed, with the envelope that committed it.
///
/// A record this build cannot read is passed over: it is folded nowhere else
/// either, and a withdrawal it hid is refused as one the record does not show.
fn committed(journal: &[Envelope]) -> impl Iterator<Item = (Option<u64>, Operation)> + '_ {
    journal
        .iter()
        .filter(|envelope| {
            envelope.source == crate::event::Source::Pipeline
                && envelope.kind.0 == crate::event::PipelineKind::EditCommitted.as_str()
        })
        .flat_map(|envelope| {
            let by = envelope
                .payload
                .get("envelope")
                .and_then(serde_json::Value::as_u64);
            serde_json::from_value::<Vec<Operation>>(
                envelope
                    .payload
                    .get("operations")
                    .cloned()
                    .unwrap_or_default(),
            )
            .unwrap_or_default()
            .into_iter()
            .map(move |operation| (by, operation))
        })
}

/// How a delivered note's record words its disposition, for a refusal.
fn disposition(reached: &Reached) -> String {
    match (reached, reached.turn()) {
        (Reached::JudgedWith { completion_reason }, _) => {
            format!("`judged-with` ({completion_reason})")
        }
        (_, Some(turn)) => format!("`{}` (turn: {})", reached.as_str(), turn.as_str()),
        (_, None) => format!("`{}`", reached.as_str()),
    }
}

/// A command verdict's wire word.
fn verdict(verdict: CommandVerdict) -> &'static str {
    match verdict {
        CommandVerdict::Applied => "applied",
        CommandVerdict::Validated => "validated",
        CommandVerdict::Delivered => "delivered",
        CommandVerdict::Refused => "refused",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn divergence_block(number: &str) -> serde_json::Value {
        let record = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with(number))
            .unwrap_or_else(|| panic!("the record still carries entry {number}"));
        entry
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .and_then(|block| serde_json::from_str(block).ok())
            .unwrap_or_else(|| panic!("entry {number} carries the json block this test drives"))
    }

    /// The records entry 133 names are the operations this build commits, field
    /// for field: a delivery naming how its turn took the note and where it was
    /// sent, one written before either, an expired carry naming its deadline, and
    /// a withdrawal.
    #[test]
    fn the_records_entry_133_names_are_the_operations_this_build_commits() {
        let block = divergence_block("133.");
        for name in [
            "delivered",
            "delivered_before",
            "carried_expired",
            "withdrawn",
        ] {
            let operation: Operation = serde_json::from_value(block[name].clone())
                .unwrap_or_else(|why| panic!("entry 133's `{name}` is not an operation: {why}"));
            assert_eq!(
                serde_json::to_value(&operation).expect("it writes"),
                block[name],
                "entry 133's `{name}` does not write back as it reads"
            );
        }
        let Ok(Operation::NoteWithdrawn { from, .. }) =
            serde_json::from_value::<Operation>(block["withdrawn"].clone())
        else {
            panic!("entry 133's `withdrawn` is not a withdrawal");
        };
        assert_eq!(from, WithdrawnFrom::Carried);
        assert_eq!(
            block["withdrawn_from"],
            serde_json::json!([WithdrawnFrom::Live, WithdrawnFrom::Carried])
        );
        let Ok(Operation::NoteDelivered {
            reached: Reached::Carried {
                reason: Some(reason),
            },
            ..
        }) = serde_json::from_value::<Operation>(block["carried_expired"].clone())
        else {
            panic!("entry 133's `carried_expired` is not an expired carry");
        };
        assert_eq!(
            reason,
            crate::note::expired(std::time::Duration::from_secs(2))
        );
    }

    /// Carried notes accumulate on a node's context, and withdrawing one takes
    /// out exactly its text — whichever of them it is — leaving the others in
    /// order, and nothing where it was the only one.
    #[test]
    fn a_withdrawn_carried_note_leaves_every_other_carried_note_in_place() {
        use crate::note::{with_carried, without_carried};
        let two = with_carried(Some(&with_carried(None, "first")), "second");
        assert_eq!(two, "first\n\nsecond");
        assert_eq!(without_carried(&two, "first").as_deref(), Some("second"));
        assert_eq!(without_carried(&two, "second").as_deref(), Some("first"));
        assert_eq!(without_carried("only", "only"), None);
        assert_eq!(
            without_carried(&two, "neither").as_deref(),
            Some(two.as_str())
        );
    }
}
