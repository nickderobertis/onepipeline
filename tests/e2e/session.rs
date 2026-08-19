//! The conversation a turn belongs to, and the record it was written into.
//!
//! `oneagentgraph` names the conversation behind a member's turns — a `session`
//! label on the kinds that name one, and an `oneharness-session` event naming the
//! history record an invocation wrote — and an operator reads an agent's actual
//! transcript out of that. Both are the *sibling's*; what these journeys hold is
//! this crate's half, which is that they survive the relay into the run's own
//! journal with their values, their payload, and their artifact reference intact.
//!
//! Worth journeys of its own rather than a line in `journal.rs`, because
//! everything in between is a place a label can be lost: the envelope crosses a
//! `serde` boundary, an enricher rewrites the labels it lands in, a filter
//! decides what the store sees, and the journal is read back by a second
//! process. Nothing here would fail if a `session` were dropped at any of them —
//! which is exactly how a producer that was written, released, and adopted
//! reached the operator emitting nothing at all.
//!
//! **Which** kinds carry one is upstream's to say and is never restated here:
//! every assertion below asks [`EventKind::carries_session`], and the set of
//! kinds it answers yes to is read off that library's own deserializer, so a
//! fifth kind added there fails this file rather than passing it unnoticed.
//!
//! [`EventKind::carries_session`]: oneagentgraph::event::EventKind::carries_session

// llmlint: ignore-file[e2e_not_mocked] the rationale is `harness.rs`'s; what is specific
// here is that the double emits this contract out of the sibling's *own* `carries_session`,
// `session_label`, `OneharnessSession` and `Artifact`, so these journeys read back that
// library's shape rather than a copy of it.

use crate::harness::{agent, plan_of, World};
use oneagentgraph::event::{
    session_label, EventKind, OneharnessSession, Role, ONEHARNESS_SESSION_ARTIFACT, SESSION_LABEL,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// A note written while the worker is working, which is what pulls the lever
/// that publishes a `turn-interrupted` — one of the kinds that names a
/// conversation, and the only one no ordinary dispatch produces.
const NOTE: &str = "the fixture moved to tests/data";

/// Every event kind the linked `oneagentgraph` can produce.
///
/// Read off that library's own deserializer rather than listed here. There is no
/// public array of them — the sibling keeps its exhaustive one private to its own
/// tests — so this takes the enumeration from the one place the linked build
/// still states it in full: the `expected one of …` serde writes when it is
/// handed a kind it does not know. Round about, and worth it. A copy of the set
/// written out in this file would be a second statement of something that
/// library owns, and would go on passing on the day it grew — which is the whole
/// thing the caller below is checking for.
fn every_kind() -> Vec<EventKind> {
    let refused = serde_json::from_value::<EventKind>(json!("not-a-kind"))
        .expect_err("a kind the sibling does not know is refused")
        .to_string();
    let listed = refused
        .split_once("expected one of ")
        .unwrap_or_else(|| {
            panic!("serde no longer lists the kinds it knows, so nothing here enumerates them: {refused}")
        })
        .1;
    let kinds: Vec<EventKind> = listed
        .split(',')
        .map(|quoted| quoted.trim().trim_matches('`').to_string())
        .map(|name| {
            serde_json::from_value(json!(name)).unwrap_or_else(|error| {
                panic!("{name:?} is listed as a kind but does not read back as one: {error}")
            })
        })
        .collect();
    assert!(
        kinds.iter().copied().any(EventKind::carries_session),
        "no kind the linked oneagentgraph knows names a conversation, so this file proves          nothing: {refused}"
    );
    kinds
}

/// The journey the whole link exists for: every turn a run relays says which
/// conversation it belongs to, and says it with the value its producer stamped.
#[test]
fn every_turn_relayed_into_the_journal_names_the_conversation_it_belongs_to() {
    let world = World::new("session-labels");
    // A turn open for as long as the dispatch is held, so the run really has one
    // a `context` note can be delivered into — a `turn-interrupted` is published
    // by a *second* process on a stream of its own, and a session that was the
    // dispatch's rather than the interrupt's would be the drift worth catching.
    world.script("work.turn-open", "");
    world.script("work.wait", "hold");
    // Alive and saying nothing while it is held: the kind next door to a turn's
    // own, and the one a conversation must never ride.
    world.script("work.heartbeat", "50");
    let path = world.plan(
        "conversation",
        &plan_of("conversation", vec![agent("work", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the held node's turn to open", |world| {
        !world.events_of("conversation", "turn-started").is_empty()
    });

    world
        .run_with_stdin(
            &["reply", "conversation"],
            &json!({
                "version": 1,
                "commands": [{"op": "context", "id": "work", "note": NOTE}],
            })
            .to_string(),
        )
        .exited(0);
    world.until("the interruption to reach the journal", |world| {
        !world
            .events_of("conversation", "turn-interrupted")
            .is_empty()
    });

    world.release("work.go");
    world.until("the run to settle", |world| {
        world.run_file("conversation", "result.json").is_file()
    });

    // Every relayed envelope, judged by the producer's own rule rather than by a
    // list restated here.
    let mut carried: BTreeSet<&str> = BTreeSet::new();
    let mut bare = 0_usize;
    for event in world.journal("conversation") {
        if event["source"] != "agentgraph" {
            continue;
        }
        let kind: EventKind = serde_json::from_value(event["kind"].clone()).unwrap_or_else(|error| {
            panic!("a relayed envelope names a kind the linked oneagentgraph does not know: {error}: {event}")
        });
        let session = &event["labels"][SESSION_LABEL];
        if !kind.carries_session() {
            // The exclusion is the load-bearing half: a consumer renders every
            // labelled envelope that is not an activity or an interruption as
            // one transcript turn, so a `session` riding the thousands of
            // heartbeats one member publishes would make every turn count
            // served from that transcript wrong.
            assert_eq!(
                session,
                &Value::Null,
                "a {} carries a conversation, which renders it as a transcript turn: {event}",
                kind.as_str()
            );
            bare += 1;
            continue;
        }
        let stream = event["stream"]
            .as_str()
            .expect("an envelope names its stream");
        let member = event["labels"]["member"]
            .as_str()
            .unwrap_or_else(|| panic!("a {} names no member: {event}", kind.as_str()));
        // The value, not merely its presence: a conversation is one member's
        // turns on one stream, so a label that had lost either half would merge
        // two members into one transcript.
        assert_eq!(
            session,
            &json!(format!("{stream}.{member}")),
            "a {} reached the journal without the conversation its producer stamped: {event}",
            kind.as_str()
        );
        // And it is the value that library computes rather than a join that
        // happens to agree on these ids: the sanitising and the length bound are
        // the consumer's rule, and a relay that had rewritten either would put a
        // label at the far end that nothing can be opened by.
        assert_eq!(
            session,
            &json!(session_label(stream, member).expect("a stream and a member name one")),
            "a {}'s conversation is not the one its producer would compute: {event}",
            kind.as_str()
        );
        carried.insert(kind.as_str());
    }

    // The drift gate, and the reason the two sets above are derived: this run
    // must reach *every* kind the linked sibling names a conversation on. A
    // fifth one added upstream is a kind the double does not emit, so this fails
    // naming it rather than passing over a half-proven contract.
    let names_one: BTreeSet<&str> = every_kind()
        .into_iter()
        .filter(|kind| kind.carries_session())
        .map(EventKind::as_str)
        .collect();
    assert_eq!(
        carried, names_one,
        "this run does not reach every kind the linked oneagentgraph names a conversation on —          teach the double the ones it is missing"
    );
    assert!(
        bare > 0,
        "no relayed envelope named no conversation, so the exclusion proves nothing"
    );
}

/// The other half of the producer: the pointer an operator opens the agent's
/// actual transcript through.
#[test]
fn the_record_a_turns_conversation_was_written_into_reaches_the_journal() {
    let world = World::new("session-record");
    let path = world.plan("record", &plan_of("record", vec![agent("work", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0);

    let sessions = world.events_of("record", "oneharness-session");
    let [event] = &sessions[..] else {
        panic!(
            "the run's one turn did not name the record it wrote: {:?}",
            world.kinds("record")
        );
    };

    // Read back through the sibling's **own** payload type, which denies unknown
    // fields: every field it declares survived the relay, and nothing this crate
    // invented rode along beside them.
    let session: OneharnessSession = serde_json::from_value(event["payload"].clone())
        .unwrap_or_else(|error| panic!("the relayed payload is not a session: {error}: {event}"));
    assert_eq!(session.role, Role::Agent);
    assert_eq!(session.turn, 1);
    assert!(!session.identity.is_empty(), "{event}");

    // The three path fields are the three arguments the record's reader takes,
    // so the test the relay has to pass is that they still resolve to the file
    // the producer wrote.
    let record = std::path::Path::new(&session.history_dir)
        .join(&session.history_project)
        .join(format!("{}.jsonl", session.history_session));
    let written = std::fs::metadata(&record).unwrap_or_else(|error| {
        panic!(
            "the relayed session names {}, which is not a record: {error}",
            record.display()
        )
    });

    // The artifact beside it, which is that record — the evidence reference is
    // the whole reason the conversation is not inline on the stream.
    let [artifact] = event["artifacts"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    else {
        panic!("the relayed session carries no artifact reference: {event}");
    };
    assert_eq!(artifact["kind"], ONEHARNESS_SESSION_ARTIFACT, "{event}");
    assert_eq!(
        artifact["id"],
        json!(session.history_id),
        "the artifact and the payload name different records: {event}"
    );
    assert_eq!(artifact["bytes"], json!(written.len()), "{event}");

    // And it is placed in *this* run: the merged store keeps the sibling's own
    // stream and run id, and this crate's enricher says which node it was.
    assert_eq!(event["source"], "agentgraph", "{event}");
    assert_eq!(event["labels"]["node"], "work", "{event}");
    assert_eq!(event["labels"]["onepipeline.run_id"], "record", "{event}");
}
