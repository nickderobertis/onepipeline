//! The bus core's vocabulary journey, driven over this crate's vocabulary: the
//! same table the core runs over a vocabulary with no agent word in it,
//! differing only in the fixture handed in.
//!
//! Moved here with the vocabulary itself, from `onemessagebus`'s agent profile
//! crate at tag `onemessagebus-agent-v0.8.0`. It is what makes the declaration
//! in `src/vocabulary.rs` a conforming one rather than merely a compiling one:
//! the core drives the reserved keys, the dimensions, the matcher fields, the
//! refusals for an unknown key and a key of the wrong type, and the filter
//! grammar, all against `Agent`.

use onemessagebus::conformance::{drive, Fixture, Sample};
use onepipeline::vocabulary::{Agent, Dimensions, Labels, MatchFields, Phase, Source};
use serde_json::json;

#[test]
fn the_agent_vocabulary_holds_the_conformance_table() {
    let fixture = Fixture::<Agent> {
        namespace: "agent",
        sources: [Source::Agentgraph, Source::Pipeline],
        labels: Labels {
            run_id: Some("R".to_owned()),
            member: Some("worker".to_owned()),
            persona: Some("engineer".to_owned()),
            ..Labels::default()
        },
        other_labels: Labels {
            run_id: Some("R".to_owned()),
            member: Some("supervisor".to_owned()),
            persona: Some("reviewer".to_owned()),
            ..Labels::default()
        },
        matching: MatchFields {
            member: Some("worker".to_owned()),
            ..MatchFields::default()
        },
        dimensions: Dimensions::at(Phase::Development),
        unknown_key: "stage",
    };
    let sample = Sample::<Labels> {
        conforming: Labels {
            run_id: Some("R".to_owned()),
            round: Some(2),
            ..Labels::default()
        },
        violating: (json!({ "round": "two" }), "/round"),
    };
    drive(&fixture, &sample);
}
