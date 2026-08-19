//! The conversation a turn belongs to, and the record it was written into.
//!
//! `oneagentgraph` names the conversation behind every turn — a `session` label
//! on exactly the four kinds that name one, and an `oneharness-session` event
//! naming the history record an invocation wrote — and an operator reads an
//! agent's actual transcript out of that. Both are the *sibling's*; what these
//! journeys hold is this crate's half, which is that they survive the relay into
//! the run's own journal with their values, their payload, and their artifact
//! reference intact.
//!
//! Worth journeys of its own rather than a line in `journal.rs`, because
//! everything in between is a place a label can be lost: the envelope crosses a
//! `serde` boundary, an enricher rewrites the labels it lands in, a filter
//! decides what the store sees, and the journal is read back by a second
//! process. Nothing here would fail if a `session` were dropped at any of them —
//! which is exactly how a producer that was written, released, and adopted
//! reached the operator emitting nothing at all.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The double stamps the session through that library's *own*
// `carries_session` and `session_label`, and builds the `oneharness-session` payload and
// artifact out of its own types, so what these journeys read back is the sibling's shape
// rather than a copy of it. `harness.rs` carries the same suppression and the full
// rationale.

use crate::harness::{agent, plan_of, World};
use oneagentgraph::event::{
    session_label, OneharnessSession, Role, ONEHARNESS_SESSION_ARTIFACT, SESSION_LABEL,
};
use serde_json::{json, Value};

/// A note written while the worker is working, which is what pulls the lever
/// that publishes a `turn-interrupted` — the fourth kind that names a
/// conversation, and the only one no ordinary dispatch produces.
const NOTE: &str = "the fixture moved to tests/data";

/// Exactly the kinds `oneagentgraph` names a conversation on.
const NAMES_A_CONVERSATION: [&str; 4] = [
    "turn-started",
    "turn-activity",
    "turn-completed",
    "turn-interrupted",
];

/// Kinds beside them that must reach the journal carrying no conversation.
///
/// The exclusion is the load-bearing half: a consumer renders every labelled
/// envelope that is not an activity or an interruption as one transcript turn,
/// so a `session` riding the thousands of heartbeats one member publishes would
/// make every turn count served from that transcript wrong.
const NAMES_NONE: [&str; 4] = [
    "member-started",
    "member-heartbeat",
    "member-settled",
    "oneharness-session",
];

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

    let events = world.journal("conversation");
    for kind in NAMES_A_CONVERSATION {
        let named: Vec<&Value> = events.iter().filter(|event| event["kind"] == kind).collect();
        assert!(
            !named.is_empty(),
            "no {kind} reached the journal at all, so this journey proves nothing about it: {:?}",
            world.kinds("conversation")
        );
        for event in named {
            let stream = event["stream"].as_str().expect("an envelope names its stream");
            let member = event["labels"]["member"]
                .as_str()
                .unwrap_or_else(|| panic!("a {kind} names no member: {event}"));
            // The value, not merely its presence: a conversation is one
            // member's turns on one stream, so a label that had lost either
            // half would merge two members into one transcript.
            assert_eq!(
                event["labels"][SESSION_LABEL],
                json!(format!("{stream}.{member}")),
                "a {kind} reached the journal without the conversation its producer stamped: \
                 {event}"
            );
            // And it is the value that library computes rather than a join that
            // happens to agree on these ids: the sanitising and the length bound
            // are the consumer's rule, and a relay that had rewritten either
            // would put a label at the far end that nothing can be opened by.
            assert_eq!(
                event["labels"][SESSION_LABEL],
                json!(session_label(stream, member).expect("a stream and a member name one")),
                "a {kind}'s conversation is not the one its producer would compute: {event}"
            );
        }
    }

    for kind in NAMES_NONE {
        let named: Vec<&Value> = events.iter().filter(|event| event["kind"] == kind).collect();
        assert!(
            !named.is_empty(),
            "no {kind} reached the journal at all, so the exclusion below proves nothing: {:?}",
            world.kinds("conversation")
        );
        for event in named {
            assert_eq!(
                event["labels"][SESSION_LABEL],
                Value::Null,
                "a {kind} carries a conversation, which renders it as a transcript turn: {event}"
            );
        }
    }
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
