//! The registries this crate constructs: the read set of the event envelope,
//! the forward-carry rule, the two golden envelopes, and — the thing the move
//! of the vocabulary promised — the same ids under the same documents as the
//! bus's agent profile crate listed at its last release.
//!
//! Moved here with the vocabulary itself, from `onemessagebus`'s agent profile
//! crate at tag `onemessagebus-agent-v0.8.0`, and extended by the capture under
//! `tests/recorded/registry/`.

use std::collections::BTreeMap;

use onemessagebus::{CheckError, Read, Registry, SchemaId};
use onepipeline::vocabulary::{registry, EVENT_ENVELOPE_FAMILY};
use serde_json::{json, Value};

const ENVELOPE_V1: &str = include_str!("golden/envelope-v1.json");
const ENVELOPE_V2: &str = include_str!("golden/envelope-v2.json");

/// Every id `onemessagebus_agent::registry()` listed at 0.8.0, and the document
/// it held under each. See `tests/recorded/registry/README.md`.
const CAPTURED: &str = include_str!("recorded/registry/onemessagebus-agent-0.8.0.json");

fn id(text: &str) -> SchemaId {
    text.parse().expect("an id")
}

fn captured() -> BTreeMap<String, Value> {
    serde_json::from_str(CAPTURED).expect("the capture is JSON")
}

#[test]
fn the_vocabulary_reads_two_versions_of_the_event_envelope_and_writes_the_newest() {
    let registry = registry();
    assert_eq!(registry.read_set(EVENT_ENVELOPE_FAMILY), vec![2, 1]);
    assert_eq!(registry.writes(EVENT_ENVELOPE_FAMILY), Some(2));
    assert_eq!(registry.read_set("agent.reply-envelope"), Vec::<u32>::new());
    assert_eq!(registry.read_set("agent.nothing"), Vec::<u32>::new());
    assert_eq!(registry.writes("agent.nothing"), None);
}

/// Forward-carry: every supported older version reads as the version this
/// build writes, so a registry that hands a declared older version back
/// unchanged fails here.
#[test]
fn a_declared_version_in_the_read_set_reads_at_the_version_this_build_writes() {
    let registry = registry();
    assert_eq!(registry.read_at(EVENT_ENVELOPE_FAMILY, 1), Read::At(2));
    assert_eq!(registry.read_at(EVENT_ENVELOPE_FAMILY, 2), Read::At(2));
}

/// A version outside the set is unknown, naming the declared version and the
/// set — never carried, never rewritten.
#[test]
fn a_declared_version_outside_the_read_set_is_unknown_naming_the_version_and_the_set() {
    let registry = registry();
    for (family, declared, set) in [
        (EVENT_ENVELOPE_FAMILY, 0, "[2, 1]"),
        (EVENT_ENVELOPE_FAMILY, 3, "[2, 1]"),
    ] {
        match registry.read_at(family, declared) {
            Read::Unknown(unknown) => {
                assert_eq!(unknown.declared, declared);
                assert_eq!(unknown.family, family);
                let said = unknown.to_string();
                assert!(said.contains(&format!("{family}@{declared}")), "{said}");
                assert!(said.contains(set), "{said}");
            }
            Read::At(version) => panic!("{family}@{declared} was read at {version}"),
        }
    }
}

#[test]
fn the_golden_event_envelopes_validate_against_their_versions_and_read_at_two() {
    let registry = registry();
    for (document, version) in [(ENVELOPE_V1, 1), (ENVELOPE_V2, 2)] {
        let envelope: Value = serde_json::from_str(document).expect("the golden is JSON");
        assert_eq!(envelope["v"], json!(version));
        let schema = id(&format!("{EVENT_ENVELOPE_FAMILY}@{version}"));
        registry
            .check(&schema, &envelope)
            .unwrap_or_else(|e| panic!("envelope-v{version}.json does not validate: {e}"));
        assert_eq!(
            registry.read_at(EVENT_ENVELOPE_FAMILY, version),
            Read::At(2)
        );
        // It is also this crate's own type, whole.
        let typed: onepipeline::event::Envelope =
            serde_json::from_value(envelope.clone()).expect("the golden is an agent envelope");
        assert_eq!(serde_json::to_value(&typed).expect("serializes"), envelope);
    }
    // And each against the other version's schema is a violation at `/v`.
    let v1: Value = serde_json::from_str(ENVELOPE_V1).expect("JSON");
    match registry.check(&id("agent.event-envelope@2"), &v1) {
        Err(CheckError::Violation(violation)) => assert_eq!(violation.pointer, "/v"),
        other => panic!("a v1 envelope validated against @2: {other:?}"),
    }
}

/// The stack's registry lists exactly the ids it listed before the vocabulary
/// moved, in the same order: the agent namespace holds this crate's event
/// families and `onejudge`'s note, and the core's `onemessagebus` namespace the
/// transport plugin protocol's shapes an SDK client validates against.
#[test]
fn every_registered_id_is_listed_in_order() {
    let ids: Vec<String> = registry().ids().iter().map(ToString::to_string).collect();
    assert_eq!(
        ids,
        [
            "agent.artifact-ref@1",
            "agent.event-envelope@1",
            "agent.event-envelope@2",
            "agent.event-filter@1",
            "agent.labels@1",
            "agent.note@1",
            "onemessagebus.transport-hello@1",
            "onemessagebus.transport-reply@1",
            "onemessagebus.transport-request@1",
        ]
    );
    assert_eq!(
        ids,
        captured().keys().cloned().collect::<Vec<String>>(),
        "the ids this build registers are not the ones the profile crate listed"
    );
}

/// Every JSON pointer at which two documents differ.
fn differences(left: &Value, right: &Value, at: &str, found: &mut Vec<String>) {
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            let mut keys: Vec<&String> = left.keys().chain(right.keys()).collect();
            keys.sort_unstable();
            keys.dedup();
            for key in keys {
                match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) => {
                        differences(left, right, &format!("{at}/{key}"), found);
                    }
                    _ => found.push(format!("{at}/{key}")),
                }
            }
        }
        (Value::Array(left), Value::Array(right)) if left.len() == right.len() => {
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                differences(left, right, &format!("{at}/{index}"), found);
            }
        }
        _ => {
            if left != right {
                found.push(at.to_owned());
            }
        }
    }
}

/// **The documents did not move either.** Every id the profile crate held at
/// 0.8.0 is registered here under the document it held, so a record any build
/// wrote validates the same way whichever registry reads it.
///
/// *The document it held* is read here as everything a validator reads. The one
/// thing a document may differ in is a `description`, which is prose `schemars`
/// copies off a doc comment and which constrains nothing — and one such
/// difference is unavoidable and deliberate: `Phase` is now **`onevcs`'s** type,
/// re-exported, because that is the producer that stamps a phase and classifies
/// its own kinds into one, so its doc comment is written and wrapped there. This
/// asserts that every difference is one of those, by pointer, which is a
/// stronger statement than comparing the documents with their prose stripped: a
/// changed `const`, `type`, `required` or `properties` fails here by name.
///
/// Asked of the stack's registry and of the planner channel's, which both
/// started from that crate's `registry()` and both now build the set from its
/// owners. The third — `payload::registry()`, which `onepipeline::event` reads
/// envelopes through — is private, so it asks the same question of the same
/// capture in `payload::tests::the_registry_this_crate_constructs_holds_every_captured_document`.
/// Each registry also registers ids of its own, which is why this checks
/// containment rather than equality; `every_registered_id_is_listed_in_order`
/// above pins the shared set exactly.
#[test]
fn both_registries_hold_every_captured_id_under_its_captured_document() {
    let captured = captured();
    let holders: [(&str, Registry); 2] = [
        ("the stack's registry", registry()),
        (
            "the planner-channel layout's registry",
            onepipeline::channel::layout::registry(),
        ),
    ];
    for (name, registry) in holders {
        for (text, document) in &captured {
            let id = id(text);
            let held = registry
                .schema(&id)
                .unwrap_or_else(|| panic!("{name} does not register {text} at all"));
            let mut found = Vec::new();
            differences(held, document, "", &mut found);
            let constraining: Vec<&String> = found
                .iter()
                .filter(|pointer| !pointer.ends_with("/description"))
                .collect();
            assert!(
                constraining.is_empty(),
                "{name} holds {text} under a document that constrains differently from the \
                 one `onemessagebus-agent` 0.8.0 registered it under, at {constraining:?}"
            );
        }
    }
}

/// The reader above tells a constraining difference from a prose one, or every
/// assertion it backs would pass by reading nothing.
#[test]
fn the_document_reader_names_a_constraining_difference_and_a_prose_one_apart() {
    let held = json!({"type": "object", "description": "one", "properties": {"v": {"const": 1}}});
    let mut found = Vec::new();
    differences(&held, &held, "", &mut found);
    assert!(found.is_empty(), "{found:?}");

    let mut prose = held.clone();
    prose["description"] = json!("another");
    let mut found = Vec::new();
    differences(&held, &prose, "", &mut found);
    assert_eq!(found, ["/description"]);

    let mut constraining = held.clone();
    constraining["properties"]["v"]["const"] = json!(2);
    let mut found = Vec::new();
    differences(&held, &constraining, "", &mut found);
    assert_eq!(found, ["/properties/v/const"]);

    // A key one side has and the other does not is named at that key, whichever
    // side is missing it.
    let mut added = held.clone();
    added["properties"]["extra"] = json!({"type": "string"});
    let mut found = Vec::new();
    differences(&held, &added, "", &mut found);
    assert_eq!(found, ["/properties/extra"]);
    let mut found = Vec::new();
    differences(&added, &held, "", &mut found);
    assert_eq!(found, ["/properties/extra"]);
}

/// And `Phase`'s prose really is the one difference the assertion above tolerates
/// — so a second one appearing is something somebody decided rather than
/// something this reader was already letting through.
#[test]
fn the_only_prose_that_moved_is_the_phase_its_producer_now_owns() {
    let captured = captured();
    let registry = registry();
    let mut moved: Vec<String> = Vec::new();
    for (text, document) in &captured {
        let id = id(text);
        let held = registry.schema(&id).expect("registered");
        let mut found = Vec::new();
        differences(held, document, text, &mut found);
        moved.extend(found);
    }
    assert_eq!(
        moved,
        [
            "agent.event-envelope@1/$defs/Phase/description",
            "agent.event-envelope@2/$defs/Phase/description",
            "agent.event-filter@1/$defs/Phase/description",
        ],
        "the prose that moved with the vocabulary is not only `onevcs`'s `Phase`"
    );
    // It is that producer's own words, not a third wording invented here.
    let phase = registry
        .schema(&id("agent.event-envelope@2"))
        .expect("registered")["$defs"]["Phase"]["description"]
        .as_str()
        .expect("a description");
    let onevcs = serde_json::to_value(schemars::schema_for!(onevcs::Phase))
        .expect("onevcs's phase document serializes");
    assert_eq!(Some(phase), onevcs["description"].as_str());
}
