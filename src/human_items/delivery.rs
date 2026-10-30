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

fn journal_error(journal: &Path, error: std::io::Error) -> TransportError {
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
    let mut delivered: Delivered = transport
        .document(&control, &document)?
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
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
    if delivered.through > length {
        // The journal was replaced under delivery: carry it again from the start;
        // each watch drops what it already printed by sequence.
        delivered.through = 0;
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
        if let Ok(envelope) = serde_json::from_slice::<Envelope>(line) {
            if let Some(session) = envelope
                .payload
                .get("owner")
                .and_then(|owner| owner.get("session"))
                .and_then(serde_json::Value::as_str)
            {
                transport.append(&queue_of(session), line)?;
                carried += 1;
            }
        }
        offset += end + 1;
        delivered.through += end as u64 + 1;
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
    let since = transport.fingerprint(&queue).map_err(|e| e.to_string())?;
    let mut batch = read_pending(&transport, &queue, &consumer)?;
    if batch.is_empty() && !within.is_zero() {
        transport
            .wait_for_change(&queue, &since, within)
            .map_err(|e| e.to_string())?;
        batch = read_pending(&transport, &queue, &consumer)?;
    }
    let mut watched: Watched = transport
        .document(&queue, &document)
        .map_err(|e| e.to_string())?
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut printed = 0;
    let mut last = None;
    for (envelope, after) in &batch {
        if watched.seq.is_none_or(|seq| envelope.seq > seq) {
            print(envelope).map_err(|e| e.to_string())?;
            watched.seq = Some(envelope.seq);
            printed += 1;
        }
        last = Some(*after);
    }
    if let Some(after) = last {
        transport
            .replace_document(
                &queue,
                &document,
                &serde_json::to_vec(&watched).unwrap_or_default(),
            )
            .map_err(|e| e.to_string())?;
        transport
            .commit(&queue, &consumer, &after)
            .map_err(|e| e.to_string())?;
    }
    Ok(printed)
}

fn read_pending(
    transport: &LocalTransport,
    queue: &QueueName,
    consumer: &ConsumerName,
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
            serde_json::from_slice::<Envelope>(&stored.bytes)
                .map(|envelope| (envelope, stored.after))
                .map_err(|e| {
                    format!("the owner's queue holds a record that is not an envelope: {e}")
                })
        })
        .collect()
}
