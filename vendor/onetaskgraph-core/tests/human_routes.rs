//! Human task destinations use the existing validated route declaration.
use onetaskgraph_core::Config;
use onetaskgraph_core::config::RoutePurpose;
use onetaskgraph_plugin_api::{Classification, Repository, SourceName};
use serde_json::{Value, json};

fn configured(purpose: Value) -> Value {
    json!({"sources": {
        "plans": {"plugin": "in-memory", "routes": [
            {"classification": "public", "purpose": purpose, "to": "human-public"},
            {"classification": "private", "purpose": "human-task", "to": "human-private"},
            {"classification": "public", "to": "legacy"}
        ]},
        "human-public": {"plugin": "in-memory"},
        "human-private": {"plugin": "in-memory", "visibility": "private"},
        "legacy": {"plugin": "in-memory"}
    }})
}

#[test]
fn human_routes_preserve_legacy_placement_and_private_safety() {
    let config = Config::from_document(configured(json!("human-task"))).expect("valid routes");
    let routes = config.routes();
    let plans = SourceName::new("plans".to_owned()).expect("source");
    let repositories =
        [Repository::try_from("github.com/operator/work".to_owned()).expect("origin")];
    assert_eq!(
        routes.place(&plans, &repositories).destination.as_str(),
        "legacy"
    );
    for (classification, destination) in [
        (Classification::Public, "human-public"),
        (Classification::Private, "human-private"),
    ] {
        let placed = routes.place_for(
            &plans,
            classification,
            &repositories,
            Some(&RoutePurpose::new("human-task").expect("purpose")),
        );
        assert_eq!(placed.destination.as_str(), destination);
        assert_eq!(
            routes.of(&plans)[placed.route.expect("matched") as usize].purpose(),
            Some(&RoutePurpose::new("human-task").expect("purpose"))
        );
    }

    let mut missing = configured(json!("human-task"));
    missing["sources"]["plans"]["routes"]
        .as_array_mut()
        .expect("routes")
        .remove(1);
    let missing = Config::from_document(missing).expect("missing private route loads");
    let placed = missing.routes().place_for(
        &plans,
        Classification::Private,
        &repositories,
        Some(&RoutePurpose::new("human-task").expect("purpose")),
    );
    assert_eq!(
        placed.destination, plans,
        "private items never fall back publicly"
    );
    assert_eq!(placed.route, None);
}

#[test]
fn malformed_or_unsafe_human_routes_refuse_at_configuration_boundary() {
    for purpose in [
        json!(""),
        json!("-task"),
        json!("task-"),
        json!("task--work"),
        json!("Human Task"),
        json!(1),
        Value::Null,
    ] {
        let refusal = Config::from_document(configured(purpose)).expect_err("invalid purpose");
        assert!(
            refusal
                .to_string()
                .contains("sources.plans.routes.0.purpose"),
            "{refusal}"
        );
    }
    let mut unsafe_route = configured(json!("human-task"));
    unsafe_route["sources"]["plans"]["routes"][1]["to"] = json!("human-public");
    let refusal = Config::from_document(unsafe_route).expect_err("private route to public");
    assert!(
        refusal.to_string().contains("not declared private"),
        "{refusal}"
    );
}

#[test]
fn scoped_repository_routes_keep_precedence_order_and_visibility_filtering() {
    let config = Config::from_document(json!({"sources": {
        "plans": {"plugin": "in-memory", "routes": [
            {"repositories": ["github.com/operator/*"], "purpose": "human-task", "to": "public"},
            {"classification": "public", "purpose": "human-task", "to": "classified"},
            {"repositories": ["github.com/operator/*"], "classification": "public",
             "purpose": "human-task", "to": "combined"},
            {"repositories": ["github.com/operator/*"], "classification": "public",
             "purpose": "human-task", "to": "second"},
            {"repositories": ["github.com/operator/*"], "purpose": "human-task", "to": "vault"}
        ]},
        "public": {"plugin": "in-memory"},
        "classified": {"plugin": "in-memory"},
        "combined": {"plugin": "in-memory"},
        "second": {"plugin": "in-memory"},
        "vault": {"plugin": "in-memory", "visibility": "private"}
    }}))
    .expect("all matcher kinds");
    let plans = SourceName::new("plans".to_owned()).expect("source");
    let repositories =
        [Repository::try_from("github.com/operator/work".to_owned()).expect("origin")];
    let routes = config.routes();
    let place = |classification, repositories: &[Repository]| {
        routes.place_for(
            &plans,
            classification,
            repositories,
            Some(&RoutePurpose::new("human-task").expect("purpose")),
        )
    };
    assert_eq!(
        place(Classification::Public, &repositories)
            .destination
            .as_str(),
        "combined",
        "combined outranks classification and repository; first combined wins"
    );
    assert_eq!(
        place(Classification::Public, &[]).destination.as_str(),
        "classified",
        "repository matchers require repositories"
    );
    assert_eq!(
        place(Classification::Private, &repositories)
            .destination
            .as_str(),
        "vault",
        "matching public repository route is skipped before precedence"
    );
    for index in [0, 2] {
        assert_eq!(
            routes.of(&plans)[index].purpose(),
            Some(&RoutePurpose::new("human-task").expect("purpose"))
        );
    }
    assert_eq!(
        routes.place(&plans, &repositories).destination,
        plans,
        "scoped repository and combined entries never affect legacy callers"
    );
}

#[test]
fn distinct_producer_purposes_cannot_select_each_others_routes() {
    let mut document = configured(json!("human-task"));
    document["sources"]["plans"]["routes"]
        .as_array_mut()
        .expect("routes")
        .insert(
            0,
            json!({"classification": "public", "purpose": "review", "to": "human-private"}),
        );
    let routes = Config::from_document(document)
        .expect("two purposes")
        .routes();
    let plans = SourceName::new("plans".to_owned()).expect("source");
    for (purpose, destination) in [("review", "human-private"), ("human-task", "human-public")] {
        assert_eq!(
            routes
                .place_for(
                    &plans,
                    Classification::Public,
                    &[],
                    Some(&RoutePurpose::new(purpose).expect("purpose"))
                )
                .destination
                .as_str(),
            destination
        );
    }
}

#[test]
fn scoped_routes_never_expand_legacy_write_preflights() {
    let mut document = configured(json!("human-task"));
    let plans = SourceName::new("plans".to_owned()).expect("source");
    let config = Config::from_document(document.clone()).expect("routes");
    let routes = config.routes();
    assert_eq!(
        routes
            .reachable(&plans)
            .iter()
            .map(SourceName::as_str)
            .collect::<Vec<_>>(),
        ["plans", "legacy"],
        "legacy preflight must not contact scoped destinations"
    );
    document["sources"]["plans"]["routes"]
        .as_array_mut()
        .expect("routes")
        .pop();
    let scoped_only = Config::from_document(document)
        .expect("only scoped routes")
        .routes();
    assert!(!scoped_only.routes(&plans));
    assert!(!scoped_only.any());
    assert_eq!(scoped_only.reachable(&plans), vec![plans]);
}

#[test]
fn scoped_entries_still_cannot_create_route_chains() {
    let mut document = configured(json!("human-task"));
    document["sources"]["human-public"]["routes"] = json!([
        {"classification":"public","purpose":"review","to":"legacy"}
    ]);
    let error = Config::from_document(document).expect_err("a scoped target still routes");
    assert!(error.to_string().contains("routes of its own"), "{error}");
}
