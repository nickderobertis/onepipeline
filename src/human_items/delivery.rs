//! Delivery of human-item events to the manager session that owns each item.
//!
//! Every event the journal records is carried, as the same envelope line, onto
//! one `onemessagebus` queue per owning session — the bus's own local transport,
//! its own record framing and its own consumer cursors — so a session's watch
//! reads its own items' events and nobody else's. Nothing new is declared on the
//! bus: the envelope is the existing one and its kind one of
//! [`super::EVENT_KINDS`].
//!
//! The journal stays the authority. How far delivery has reached is a document
//! of the bus, the journal byte it has carried through; anything past it is
//! carried by the next writer or watch, so a process that recorded an event and
//! died before delivering it loses nothing. A crash between a queue append and
//! the document can carry one event twice; the watch drops what it has already
//! printed by its sequence number.
//!
//! The same carry reaches a manager watching a run. An event a person took is owed,
//! as a non-blocking planner surface, to every run the item is tied to; what is owed
//! is written into the delivery document with the cursor that passes its record, so
//! it survives a run whose channel refuses it and holds nobody else's events back. A
//! surface names the event as its `source`, and that run's own queue is what says it
//! was carried: a crash after a push, a retried carry and an adopted driver all find
//! it there and push nothing.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use onemessagebus::{
    ConsumerName, DocumentName, LocalTransport, QueueName, Transport, TransportError,
};
use serde::{Deserialize, Serialize};

use crate::event::Envelope;

/// The consumer every watch of a session reads its queue as.
const WATCH: &str = "watch";
/// The queue delivery's own documents are kept beside.
const DELIVERY: &str = "delivery";

fn bus_dir(root: &Path) -> PathBuf {
    root.join(super::JOURNAL_PATH).with_file_name("bus")
}

/// The queue one owning session's events are carried on.
pub(crate) fn queue_of(session: &str) -> QueueName {
    let digest = super::live::item_id_for(session);
    QueueName::try_from(format!("owner-{}", &digest[3..]).as_str())
        .unwrap_or_else(|_| unreachable!("a hex digest is a queue name"))
}

fn name<T: for<'a> TryFrom<&'a str>>(text: &str) -> T {
    T::try_from(text).unwrap_or_else(|_| unreachable!("{text} is a bus name"))
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Delivered {
    /// The journal byte delivery has carried every record before.
    through: u64,
    /// The SHA-256 of the record that ends there, so a journal cut or replaced
    /// under delivery is recognised rather than read from the middle of a record.
    #[serde(default)]
    last: String,
    /// The surfaces carried past and not yet on their runs' channels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    owed: Vec<OwedSurface>,
}

// llmlint: ignore-block[invalid_states_unrepresentable] a private record of this module's own document, written only by `owed_for` from a validated journal record and a listed run's id; it is rebuilt by nothing else, and `Surface`, which it becomes, spells all of these as strings.
/// One event's surface, owed to one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwedSurface {
    run: String,
    /// The event it carries, which is what says it was already carried.
    event: String,
    message: String,
    /// The message to carry instead where the run's publication boundary refuses it.
    neutral: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// The surface kind an item event is carried to a run as.
const SURFACE_KIND: &str = "human-item";

/// The two messages an event's surface may say, if a person took it: an item's title,
/// its text and its reason are theirs and may be private, so the neutral one names only
/// the id, the action and the actor.
fn surface_messages(payload: &super::HumanItemEventPayload) -> Option<(String, String)> {
    use super::HumanItemEventAction as Action;
    let (actor, on_behalf_of) = match payload.action {
        Action::Completed | Action::Dismissed => payload
            .resolution
            .as_ref()
            .map(|r| (r.actor.as_str(), r.on_behalf_of.as_deref()))?,
        Action::Reopened | Action::Noted => payload
            .by
            .as_ref()
            .map(|by| (by.actor.as_str(), by.on_behalf_of.as_deref()))?,
        Action::Raised | Action::Answered | Action::AutoResolved => return None,
    };
    if actor == "engine" {
        return None;
    }
    let action = serde_json::to_value(payload.action)
        .ok()?
        .as_str()?
        .to_owned();
    let by = on_behalf_of.map_or_else(
        || actor.to_owned(),
        |user| format!("{actor} on behalf of {user}"),
    );
    let said = payload
        .resolution
        .as_ref()
        .and_then(|r| r.text.as_deref().or(r.reason.as_deref()))
        .map_or_else(String::new, |text| format!(": {text}"));
    Some((
        format!(
            "{action}: {} \"{}\" by {by}{said}",
            payload.item_id, payload.item.title
        ),
        format!(
            "{action}: {} by {by}; its title and text are withheld by this run's publication \
             boundary, and `onepipeline human-items show` prints them",
            payload.item_id
        ),
    ))
}

/// Every listed run `item` is tied to: its owner, the run its work hold names, the runs
/// of the plan its plan hold names, and for a host hold every run. Read off the listing
/// because an item's run and plan are a raiser's words, and whether each has ended is
/// asked when its surface is pushed.
fn tied_runs(root: &Path, item: &super::HumanItem) -> Vec<String> {
    use super::HumanItemBlock as Block;
    crate::ledger::all_runs(root)
        .runs
        .into_iter()
        .filter(|paths| {
            item.owner.run.as_deref() == Some(paths.run.as_str())
                || match &item.block {
                    Block::Host => true,
                    Block::Work { run, .. } => run == &paths.run,
                    Block::Plan { plan } => {
                        crate::ledger::read_json::<crate::ledger::LaunchRecord>(&paths.launch())
                            .is_ok_and(|launch| &launch.project == plan)
                    }
                    Block::Nothing => false,
                }
        })
        .map(|paths| paths.run)
        .collect()
}

fn owed_for(root: &Path, seq: u64, payload: &super::HumanItemEventPayload) -> Vec<OwedSurface> {
    let Some((message, neutral)) = surface_messages(payload) else {
        return Vec::new();
    };
    tied_runs(root, &payload.item)
        .into_iter()
        .map(|run| OwedSurface {
            run,
            event: super::live::event_id(seq),
            message: message.clone(),
            neutral: neutral.clone(),
        })
        .collect()
}

/// Whether a run has ended, by the test that retires its holds: stopped, or converged on
/// anything but waiting. A run waiting on a person has not ended, and neither has one
/// whose failed branch leaves others still moving.
fn run_has_ended(state: &crate::projection::RunState) -> bool {
    let statuses = state.statuses();
    state.stop_recorded()
        || (!statuses.is_empty()
            && crate::graph::is_terminal(&statuses)
            && crate::graph::state_of(&statuses) != crate::graph::GraphState::Waiting)
}

/// Put one owed surface on its run's channel. Answers `Ok` when nothing more is owed:
/// pushed now, found already there, or its run gone or ended.
fn push_owed(root: &Path, owed: &OwedSurface) -> crate::error::Result<()> {
    let paths = crate::ledger::RunPaths::under(root, &owed.run);
    if !paths.journal().is_file() {
        return Ok(());
    }
    let view = crate::views::RunView::open(&paths)?;
    if run_has_ended(&view.state) {
        return Ok(());
    }
    let channel = crate::channel::ChannelState::new(&paths);
    if channel
        .every_surface()
        .iter()
        .any(|s| s.source == owed.event)
    {
        return Ok(());
    }
    let allowed =
        crate::boundary::guard(&view.state, &serde_json::json!({"message": owed.message}));
    let surface = crate::channel::Surface {
        id: 0,
        kind: SURFACE_KIND.to_owned(),
        message: if allowed.is_ok() {
            owed.message.clone()
        } else {
            owed.neutral.clone()
        },
        source: owed.event.clone(),
        blocking: false,
        queued_at: crate::sys::now_millis(),
        workstream: None,
        abandoned: false,
        asker: None,
        correlation: None,
    };
    let mut journal = crate::journal::Journal::open(&paths);
    crate::engine::raise(&paths, &mut journal, surface)
}

/// Push every surface still owed, keeping and saying each one its run refused. Answers
/// whether the list changed.
fn push_every_owed(root: &Path, delivered: &mut Delivered) -> bool {
    let before = delivered.owed.len();
    delivered.owed.retain(|owed| match push_owed(root, owed) {
        Ok(()) => false,
        Err(why) => {
            eprintln!(
                "onepipeline: the human-item event {} is recorded and not yet surfaced on run \
                 '{}' ({why}); the next human-items command or watch carries it",
                owed.event, owed.run
            );
            true
        }
    });
    delivered.owed.len() != before
}

fn digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether the record ending at `delivered.through` is still the one delivery
/// carried last.
fn still_there(file: &mut std::fs::File, length: u64, delivered: &Delivered) -> bool {
    if delivered.through == 0 {
        return true;
    }
    if delivered.through > length {
        return false;
    }
    // The record that ends at the cursor: back from it to the newline before it.
    let start = delivered.through.saturating_sub(64 * 1024);
    let mut window = vec![0u8; usize::try_from(delivered.through - start).unwrap_or(0)];
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut window).is_err() {
        return false;
    }
    let Some((body, b"\n")) = window.split_last_chunk::<1>() else {
        return false;
    };
    let line = match body.iter().rposition(|b| *b == b'\n') {
        Some(at) => &body[at + 1..],
        None if start == 0 => body,
        None => return false,
    };
    digest(line) == delivered.last
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Watched {
    /// The greatest event sequence this session's watch has printed.
    seq: Option<u64>,
}

/// Carry every journal record past what delivery has reached onto its owner's
/// queue. Called with the journal's append lock held, after an append; a
/// failure is said on standard error and left for the next carrier.
pub(crate) fn deliver(root: &Path, _just: &Envelope) {
    if let Err(error) = catch_up(root) {
        eprintln!(
            "onepipeline: a human-item event is recorded and not yet delivered to its owner's \
             watch ({error}); the next human-items command or watch carries it"
        );
    }
}

/// Carry everything recorded and not yet delivered, taking the journal's lock as a
/// writer holds it; a failure is said and left for the next carrier.
pub(crate) fn carry(root: &Path) {
    let journal = root.join(super::JOURNAL_PATH);
    let held = crate::ledger::open_healed_for_append(&journal).map_err(|e| e.to_string());
    if let Err(error) = held.and_then(|_lock| catch_up(root).map_err(|e| e.to_string())) {
        eprintln!(
            "onepipeline: a human-item event is recorded and not yet delivered ({error}); the \
             next human-items command or watch carries it"
        );
    }
}

/// One record of the journal or of an owner's queue, read as the item event it
/// carries.
fn item_event(line: &[u8]) -> Result<super::HumanItemEventPayload, String> {
    super::live::parse_record(line).map(|(_, payload)| payload)
}

fn journal_error(journal: &Path, error: impl std::fmt::Display) -> TransportError {
    TransportError::Backend {
        transport: "human-items".to_owned(),
        detail: format!("{}: {error}", journal.display()),
    }
}

/// Carry everything recorded and not yet delivered. Answers how many records
/// were carried.
pub(crate) fn catch_up(root: &Path) -> Result<u64, TransportError> {
    let dir = bus_dir(root);
    let transport = LocalTransport::open(&dir)?;
    let control: QueueName = name(DELIVERY);
    let document: DocumentName = name("delivered.json");
    let mut delivered: Delivered = match transport.document(&control, &document)? {
        None => Delivered::default(),
        // llmlint: ignore[changed_behavior_has_e2e] the bus replaces this document atomically, so a garbled one is a hand edit; carrying from the start is the path `delivery_survives_a_failed_carry_a_truncated_journal_and_a_failed_print` drives through a journal cut back under delivery, with the watch skipping what it printed.
        Some(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|why| {
            // Carried again from the start rather than guessed at: every watch drops
            // what it already printed by sequence, so nothing is printed twice.
            eprintln!(
                "onepipeline: the human-item delivery record is not one ({why}); every event \
                 is carried again and each watch skips what it has printed"
            );
            Delivered::default()
        }),
    };
    let journal = root.join(super::JOURNAL_PATH);
    let mut file = match std::fs::File::open(&journal) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(journal_error(&journal, e)),
    };
    let length = file
        .metadata()
        .map_err(|e| journal_error(&journal, e))?
        .len();
    if !still_there(&mut file, length, &delivered) {
        // The journal was cut or replaced under delivery: carry it again from the
        // start; each watch drops what it already printed by sequence.
        delivered = Delivered::default();
    }
    file.seek(SeekFrom::Start(delivered.through))
        .map_err(|e| journal_error(&journal, e))?;
    let mut rest = Vec::new();
    file.read_to_end(&mut rest)
        .map_err(|e| journal_error(&journal, e))?;
    let write = |delivered: &Delivered| {
        transport.replace_document(
            &control,
            &document,
            &serde_json::to_vec(delivered).unwrap_or_default(),
        )
    };
    let mut carried = 0;
    let mut offset = 0usize;
    while let Some(end) = rest[offset..].iter().position(|b| *b == b'\n') {
        let line = &rest[offset..offset + end];
        // The journal's own records, held to the item event they must carry: one
        // that is not is refused here rather than carried or passed over, and
        // delivery stops at it until it is repaired.
        let (seq, payload) = super::live::parse_record(line).map_err(|why| {
            journal_error(
                &journal,
                format!(
                    "the record at byte {} is not an item event: {why}",
                    delivered.through
                ),
            )
        })?;
        transport.append(&queue_of(&payload.owner.session), line)?;
        carried += 1;
        offset += end + 1;
        delivered.through += end as u64 + 1;
        delivered.last = digest(line);
        delivered.owed.extend(owed_for(root, seq, &payload));
        write(&delivered)?;
    }
    if push_every_owed(root, &mut delivered) {
        write(&delivered)?;
    }
    Ok(carried)
}

/// Every event delivered to `session` that its watch has not printed, waiting
/// up to `within` for one when there is none. The cursor is committed only
/// once `print` has taken every event, so a watch that dies printing them
/// hands them to the next watch rather than losing them.
pub(crate) fn watch(
    root: &Path,
    session: &str,
    within: Duration,
    print: &mut dyn FnMut(&Envelope) -> std::io::Result<()>,
) -> Result<usize, String> {
    let journal = root.join(super::JOURNAL_PATH);
    if let Some(parent) = journal.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Carried under the journal's lock, as a writer carries it, so nothing the
    // journal holds is missing from the queue this watch reads.
    {
        let _lock = crate::ledger::open_healed_for_append(&journal).map_err(|e| e.to_string())?;
        catch_up(root).map_err(|e| e.to_string())?;
    }
    let transport = LocalTransport::open(bus_dir(root)).map_err(|e| e.to_string())?;
    let queue = queue_of(session);
    let consumer: ConsumerName = name(WATCH);
    let document: DocumentName = name(&format!("watched-{}.json", queue.as_str()));
    if !within.is_zero() && read_pending(&transport, &queue, &consumer, session)?.is_empty() {
        let since = transport.fingerprint(&queue).map_err(|e| e.to_string())?;
        transport
            .wait_for_change(&queue, &since, within)
            .map_err(|e| e.to_string())?;
    }
    // Read, printed and committed as one section over the session's queue, so two
    // watches of one session never print the same events: the second finds them
    // committed.
    let mut answered: Result<usize, String> = Ok(0);
    transport
        .exclusive(&queue, &mut |held| {
            answered = print_pending(held, &queue, &consumer, &document, session, print);
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    answered
}

fn print_pending(
    transport: &dyn Transport,
    queue: &QueueName,
    consumer: &ConsumerName,
    document: &DocumentName,
    session: &str,
    print: &mut dyn FnMut(&Envelope) -> std::io::Result<()>,
) -> Result<usize, String> {
    let batch = read_pending(transport, queue, consumer, session)?;
    let watched_record = transport
        .document(queue, document)
        .map_err(|e| e.to_string())?;
    let mut watched: Watched = match watched_record {
        None => Watched::default(),
        // llmlint: ignore[changed_behavior_has_e2e] the bus replaces this document atomically, so a garbled one is a hand edit; refusing it before printing or committing anything keeps every pending event for the next watch, as a failed print does in `delivery_survives_a_failed_carry_a_truncated_journal_and_a_failed_print`.
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|why| {
            format!(
                "{session}'s watch record is not one ({why}); nothing was printed or \
                 committed, and the next watch reads the same events once it is repaired"
            )
        })?,
    };
    let mut printed = 0;
    let mut last = None;
    let record = |watched: &Watched| {
        transport
            .replace_document(
                queue,
                document,
                &serde_json::to_vec(watched).unwrap_or_default(),
            )
            .map_err(|e| e.to_string())
    };
    for (envelope, after) in &batch {
        if watched.seq.is_none_or(|seq| envelope.seq > seq) {
            if let Err(why) = print(envelope) {
                // What was printed before the failure is recorded as watched, so
                // the next watch prints only what this one did not.
                if printed > 0 {
                    record(&watched)?;
                }
                return Err(why.to_string());
            }
            watched.seq = Some(envelope.seq);
            printed += 1;
        }
        last = Some(*after);
    }
    if let Some(after) = last {
        record(&watched)?;
        transport
            .commit(queue, consumer, &after)
            .map_err(|e| e.to_string())?;
    }
    Ok(printed)
}

fn read_pending(
    transport: &dyn Transport,
    queue: &QueueName,
    consumer: &ConsumerName,
    session: &str,
) -> Result<Vec<(Envelope, onemessagebus::Position)>, String> {
    let from = transport
        .cursor(queue, consumer)
        .map_err(|e| e.to_string())?;
    let batch = transport
        .read(queue, from.as_ref(), usize::MAX)
        .map_err(|e| e.to_string())?;
    batch
        .records
        .into_iter()
        .map(|stored| {
            let event = item_event(&stored.bytes).map_err(|e| {
                format!("the owner's queue holds a record that is not an item event: {e}")
            })?;
            if event.owner.session != session {
                return Err(format!(
                    "the queue of {session} holds an event owned by {}",
                    event.owner.session
                ));
            }
            serde_json::from_slice::<Envelope>(&stored.bytes)
                .map(|envelope| (envelope, stored.after))
                .map_err(|e| e.to_string())
        })
        .collect()
}
