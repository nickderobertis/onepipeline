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

use crate::channel::{ChannelState, Command, CommandOutcome, CommandVerdict};
use crate::edits::Operation;
use crate::error::{Error, Result};
use crate::event::Envelope;
use crate::note::{Placement, Reached, WithdrawalReason, WithdrawnFrom};
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
    let Ok(reason) = WithdrawalReason::try_from(reason.to_string()) else {
        return Err(refuse(
            "`reason` is blank; say why the note is withdrawn".to_string(),
        ));
    };
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
    let records = committed(&journal)?;
    if let Some(by) = withdrawn_by(&records, id, index) {
        return Err(refuse(format!(
            "{named} was already {}",
            withdrawn_by_words(by).trim_end_matches(':')
        )));
    }
    match delivered(&records, id, index) {
        Some(Reached::Carried { .. }) => {
            let standing = crate::note::standing(&journal, &node)?;
            let owed: Vec<&crate::note::Held> = standing
                .held
                .iter()
                .filter(|held| held.placement == Placement::DeliveredSince)
                .collect();
            let Some(at) = owed.iter().position(|held| held.sent == Some((id, index))) else {
                return Err(refuse(format!(
                    "{named} was carried and has already been composed into a dispatch of \
                     that node, which read it"
                )));
            };
            let Some(Command::Note { text, .. }) = sent.commands.get(position) else {
                return Err(refuse(format!("{named} is no longer a note")));
            };
            // Which of the node's identical carried entries is this note's, for
            // replay to take out exactly it.
            let alike_after = owed[at + 1..]
                .iter()
                .filter(|held| {
                    !held.note.reached.a_conversation_read_it() && held.note.text == *text
                })
                .count();
            Ok(vec![Operation::NoteWithdrawn {
                node,
                envelope: id,
                index,
                reason,
                from: WithdrawnFrom::Carried,
                text: text.clone(),
                alike_after: u32::try_from(alike_after).unwrap_or(u32::MAX),
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
/// An envelope not answered yet is left alone here: it is answered once its last
/// note is, and that answer is written through [`as_withdrawn`], which refuses
/// this note there.
///
/// # Errors
///
/// The channel's, where the answer cannot be written.
pub(crate) fn answer_withdrawn(
    paths: &RunPaths,
    sent: u64,
    index: u32,
    reason: &WithdrawalReason,
    by: Option<u64>,
) -> Result<()> {
    let channel = ChannelState::new(paths);
    let Some(mut outcome) = channel.outcome_of(sent) else {
        return Ok(());
    };
    if !refuse_withdrawn(&mut outcome, index, reason, by) {
        return Ok(());
    }
    channel.answer_commands(&outcome)
}

/// `outcome`, with each note of its envelope a committed withdrawal took back
/// answered refused, carrying why: what an envelope's answer is written as when
/// a note of it was withdrawn while the rest of it was still being delivered.
///
/// # Errors
///
/// None today; the `Result` is the room the channel write beside it leaves.
pub(crate) fn as_withdrawn(
    paths: &RunPaths,
    mut outcome: CommandOutcome,
) -> Result<CommandOutcome> {
    for (by, operation) in readable(&crate::journal::read(&paths.journal())) {
        if let Operation::NoteWithdrawn {
            envelope,
            index,
            reason,
            ..
        } = operation
        {
            if envelope == outcome.id {
                refuse_withdrawn(&mut outcome, index, &reason, by);
            }
        }
    }
    Ok(outcome)
}

/// Answer command `index` of `outcome` refused, as withdrawn by envelope `by` for
/// `reason`. `false` where the outcome has no such command.
fn refuse_withdrawn(
    outcome: &mut CommandOutcome,
    index: u32,
    reason: &WithdrawalReason,
    by: Option<u64>,
) -> bool {
    let position = usize::try_from(index).unwrap_or(usize::MAX);
    let Some(result) = outcome.results.get_mut(position) else {
        return false;
    };
    let why = format!("{} {}", withdrawn_by_words(by), reason.as_str());
    result.outcome = CommandVerdict::Refused;
    result.reason = Some(why.clone());
    result.refusal = None;
    result.turn = None;
    outcome.applied = false;
    outcome.reason = Some(format!(
        "command {index} ('note') was {why}; every other command's answer stands"
    ));
    true
}

/// Answer again every envelope a committed withdrawal took a note of that is not
/// yet answered that way.
///
/// The withdrawal is committed before its envelope is answered again, so a driver
/// that dies between the two writes leaves the second owed; a driver taking the
/// run over writes it, and one that finds it written writes nothing.
///
/// # Errors
///
/// The channel's, where an answer cannot be written.
pub(crate) fn answer_owed(paths: &RunPaths) -> Result<()> {
    let channel = ChannelState::new(paths);
    for (by, operation) in readable(&crate::journal::read(&paths.journal())) {
        let Operation::NoteWithdrawn {
            envelope: sent,
            index,
            reason,
            ..
        } = operation
        else {
            continue;
        };
        let answered = channel.outcome_of(sent).is_some_and(|outcome| {
            outcome
                .results
                .get(usize::try_from(index).unwrap_or(usize::MAX))
                .and_then(|result| result.reason.as_deref())
                .is_some_and(|said| said.starts_with(&withdrawn_by_words(by)))
        });
        if !answered {
            answer_withdrawn(paths, sent, index, &reason, by)?;
        }
    }
    Ok(())
}

/// How an answer names the withdrawal that refused a note: by its envelope, or —
/// for one `reply` applied with nothing driving the run, which no queued envelope
/// carried — as a `withdraw-note`.
fn withdrawn_by_words(by: Option<u64>) -> String {
    match by {
        Some(by) => format!("withdrawn by envelope {by}:"),
        None => "withdrawn by a `withdraw-note`:".to_string(),
    }
}

/// Answer again, for each withdrawal among `operations`, the envelope it took a
/// note of: what either writer does once it has committed them, `by` naming the
/// envelope that carried the withdrawals where a queued one did.
///
/// # Errors
///
/// The channel's, where an answer cannot be written.
pub(crate) fn answer_each(
    paths: &RunPaths,
    operations: &[Operation],
    by: Option<u64>,
) -> Result<()> {
    for operation in operations {
        if let Operation::NoteWithdrawn {
            envelope: sent,
            index,
            reason,
            ..
        } = operation
        {
            answer_withdrawn(paths, *sent, *index, reason, by)?;
        }
    }
    Ok(())
}

/// Whether note `index` of envelope `sent` was already withdrawn, and by which
/// envelope where a queued one carried the withdrawal: `Some(None)` is one `reply`
/// applied with nothing driving the run.
fn withdrawn_by(
    records: &[(Option<u64>, Operation)],
    sent: u64,
    index: u32,
) -> Option<Option<u64>> {
    records
        .iter()
        .find_map(|(envelope, operation)| match operation {
            Operation::NoteWithdrawn {
                envelope: from,
                index: at,
                ..
            } if *from == sent && *at == index => Some(*envelope),
            _ => None,
        })
}

/// What the delivery of note `index` of envelope `sent` recorded, if it recorded
/// anything.
fn delivered(records: &[(Option<u64>, Operation)], sent: u64, index: u32) -> Option<Reached> {
    records.iter().find_map(|(_, operation)| match operation {
        Operation::NoteDelivered {
            envelope: Some(from),
            index: Some(at),
            reached,
            ..
        } if *from == sent && *at == index => Some(reached.clone()),
        _ => None,
    })
}

/// Every operation the run committed that this build can read, with the envelope
/// that committed it: what a driver's recovery and an envelope's answer read the
/// run's withdrawals from.
///
/// Lenient where [`committed`] is strict, and on purpose: a run whose journal
/// holds an edit this build cannot fold is still driven and adopted — the
/// projection reads past it the same way — so neither a driver taking it over
/// nor an answer it owes is refused for it. A withdrawal hidden in such a record
/// is one this build cannot act on either way; the `withdraw-note` that would
/// have to decide against it is what refuses.
fn readable(journal: &[Envelope]) -> Vec<(Option<u64>, Operation)> {
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
        .collect()
}

/// Every operation the run committed, with the envelope that committed it.
///
/// Refused rather than read past where a commit of this crate's own cannot be
/// read, exactly as [`crate::note::standing`] refuses it: a withdrawal hidden in
/// an unreadable record would let the same note be withdrawn twice, or be
/// answered as a disposition it no longer has.
///
/// # Errors
///
/// [`Error::Invalid`] naming the record whose operations this build cannot read.
fn committed(journal: &[Envelope]) -> Result<Vec<(Option<u64>, Operation)>> {
    let mut records = Vec::new();
    for envelope in journal.iter().filter(|envelope| {
        envelope.source == crate::event::Source::Pipeline
            && envelope.kind.0 == crate::event::PipelineKind::EditCommitted.as_str()
    }) {
        let by = envelope
            .payload
            .get("envelope")
            .and_then(serde_json::Value::as_u64);
        // llmlint: ignore[changed_behavior_has_e2e] a commit of this crate's own whose operations this build cannot read is a journal edited by hand — no verb, driver or death writes one — so a journey reaching it would prove the fixture rather than the code; the refusal is held by `withdrawal::tests` over such a record, as `note::standing`'s is by `note::tests`, and `validate` and `as_withdrawn` take it through `?`.
        let operations: Vec<Operation> = serde_json::from_value(
            envelope
                .payload
                .get("operations")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )
        .map_err(|error| {
            Error::Invalid(format!(
                "withdraw-note: the run's record of its committed edits cannot be read: \
                 `edit-committed` record {}/{} carries operations this build cannot read \
                 ({error}), so what became of the note cannot be decided from it",
                envelope.stream, envelope.seq
            ))
        })?;
        records.extend(operations.into_iter().map(|operation| (by, operation)));
    }
    Ok(records)
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

    /// A commit whose operations this build cannot read is refused by name when a
    /// `withdraw-note` is decided against it — a withdrawal it hid would let the
    /// same note be withdrawn twice — and read past when a driver or an answer
    /// reads the run's withdrawals, which neither may be refused for.
    #[test]
    fn a_commit_a_withdrawal_cannot_read_is_refused_by_name() {
        use crate::journal::{labels, payload, PipelineKind};
        let unreadable = Envelope {
            v: crate::event::ENVELOPE_VERSION,
            ts: crate::sys::rfc3339_from_millis(1_786_000_000_000),
            stream: "s".into(),
            seq: 4,
            source: crate::event::Source::Pipeline,
            kind: PipelineKind::EditCommitted.into(),
            dimensions: Default::default(),
            labels: labels("demo", None),
            payload: payload(&[
                (
                    "operations",
                    serde_json::json!([{"kind": "from-the-future"}]),
                ),
                ("envelope", serde_json::json!(3)),
            ]),
            artifacts: Vec::new(),
        };
        let refused = committed(std::slice::from_ref(&unreadable))
            .expect_err("an unreadable commit is not read past");
        let said = refused.to_string();
        assert!(
            said.contains("edit-committed") && said.contains("s/4"),
            "the refusal does not name the record: {said}"
        );
        assert!(readable(&[unreadable]).is_empty());
    }

    /// A deadline the clock can name is that instant; the largest a note may
    /// carry still forms one — the furthest the clock can name — without the
    /// overflow panicking, and never earlier than one it could name.
    #[test]
    fn every_accepted_deadline_forms_an_instant_without_overflow() {
        // The latest instant the run names is the latest RFC 3339 writes, and
        // reads back; a deadline past it is held there.
        assert_eq!(
            crate::sys::rfc3339_from_millis(crate::note::LATEST_NAMEABLE_MILLIS),
            "9999-12-31T23:59:59.999Z"
        );
        assert_eq!(
            crate::projection::millis_of("9999-12-31T23:59:59.999Z"),
            Some(crate::note::LATEST_NAMEABLE_MILLIS)
        );
        let at = crate::sys::now_millis();
        assert_eq!(
            at + u64::try_from(
                crate::note::bounded_deadline(std::time::Duration::from_secs(u64::MAX), at)
                    .as_millis()
            )
            .expect("bounded"),
            crate::note::LATEST_NAMEABLE_MILLIS
        );
        assert_eq!(
            crate::note::bounded_deadline(std::time::Duration::from_secs(2), at),
            std::time::Duration::from_secs(2)
        );
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let near = Duration::from_secs(2);
        assert_eq!(crate::note::deadline_instant(now, near), now + near);
        let furthest = crate::note::deadline_instant(now, Duration::from_secs(u64::MAX));
        assert!(furthest >= now + Duration::from_secs(365 * 24 * 60 * 60));
        assert!(furthest.checked_add(Duration::from_secs(2)).is_none());
    }

    /// Carried notes accumulate on a node's context, and withdrawing one takes
    /// out exactly its text — whichever of them it is — leaving the others in
    /// order, and nothing where it was the only one.
    #[test]
    fn a_withdrawn_carried_note_leaves_every_other_carried_note_in_place() {
        use crate::note::{with_carried, without_carried};
        let two = with_carried(Some(&with_carried(None, "first")), "second");
        assert_eq!(two, "first\n\nsecond");
        assert_eq!(without_carried(&two, "first", 0).as_deref(), Some("second"));
        assert_eq!(without_carried(&two, "second", 0).as_deref(), Some("first"));
        assert_eq!(without_carried("only", "only", 0), None);
        assert_eq!(
            without_carried(&two, "neither", 0).as_deref(),
            Some(two.as_str())
        );
        // A note whose own text holds a blank line is one note, taken out whole,
        // wherever it sits among the others.
        let spaced = "keep the loader\n\nand its signature";
        let three = with_carried(Some(&with_carried(Some("first"), spaced)), "last");
        assert_eq!(
            without_carried(&three, spaced, 0).as_deref(),
            Some("first\n\nlast")
        );
        // A note reading as two others joined is itself one entry: withdrawing
        // either of the two leaves it, and withdrawing it leaves both.
        let joined = with_carried(
            Some(&with_carried(Some(&with_carried(None, "a")), "b")),
            "a\n\nb",
        );
        assert_eq!(
            without_carried(&joined, "a", 0).as_deref(),
            Some("b\n\na\n \nb")
        );
        assert_eq!(
            without_carried(&joined, "a\n\nb", 0).as_deref(),
            Some("a\n\nb")
        );
        // The same text carried twice, another between: which one is withdrawn
        // is the count of identical entries after it.
        let twice = with_carried(Some(&with_carried(Some("a"), "b")), "a");
        assert_eq!(without_carried(&twice, "a", 1).as_deref(), Some("b\n\na"));
        assert_eq!(without_carried(&twice, "a", 0).as_deref(), Some("a\n\nb"));
        let led = with_carried(Some(&with_carried(None, spaced)), "last");
        assert_eq!(without_carried(&led, spaced, 0).as_deref(), Some("last"));
        let trailed = with_carried(Some("first"), spaced);
        assert_eq!(
            without_carried(&trailed, spaced, 0).as_deref(),
            Some("first")
        );
        // And a blank reason is no withdrawal's, read back from a record or not.
        assert!(serde_json::from_value::<WithdrawalReason>(serde_json::json!("  ")).is_err());
    }
}
