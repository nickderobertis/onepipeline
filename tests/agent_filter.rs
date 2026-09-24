use onepipeline::vocabulary::{AgentFilter, EventFilter, Labels, Phase, Source};
use serde_json::json;

#[test]
fn a_filter_admits_only_events_matching_its_source_phase_kind_and_labels() {
    let filter: EventFilter = serde_json::from_value(json!({
        "include": [{
            "source": "vcs",
            "kind": "fetch",
            "phase": "development",
            "run_id": "run-1"
        }],
        "exclude": [{ "node": "blocked" }]
    }))
    .expect("a user filter parses");
    filter.validate().expect("a user filter is valid");

    let labels = Labels {
        run_id: Some("run-1".into()),
        node: Some("ready".into()),
        ..Labels::default()
    };
    assert!(filter.admits(Source::Vcs, "fetch", &labels, Some(Phase::Development)));
    assert!(!filter.admits(
        Source::Agentgraph,
        "fetch",
        &labels,
        Some(Phase::Development)
    ));
    assert!(!filter.admits(Source::Vcs, "push", &labels, Some(Phase::Development)));
    assert!(!filter.admits(Source::Vcs, "fetch", &labels, Some(Phase::Review)));
    assert!(!filter.admits(
        Source::Vcs,
        "fetch",
        &Labels {
            run_id: Some("another-run".into()),
            ..labels.clone()
        },
        Some(Phase::Development)
    ));
    assert!(!filter.admits(
        Source::Vcs,
        "fetch",
        &Labels {
            node: Some("blocked".into()),
            ..labels
        },
        Some(Phase::Development)
    ));
}
