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
    let mut carried = 0;
    let mut offset = 0usize;
    while let Some(end) = rest[offset..].iter().position(|b| *b == b'\n') {
        let line = &rest[offset..offset + end];
        // The journal's own records, held to the item event they must carry: one
        // that is not is refused here rather than carried or passed over, and
        // delivery stops at it until it is repaired.
        let payload = item_event(line).map_err(|why| {
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
        transport.replace_document(
            &control,
            &document,
            &serde_json::to_vec(&delivered).unwrap_or_default(),
        )?;
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
