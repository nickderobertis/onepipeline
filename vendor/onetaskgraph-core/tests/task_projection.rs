//! The projection domain is the one wire contract used by callers and schema consumers.
use onetaskgraph_core::task_projection::{HumanTaskProjected, HumanTaskProjection};
use serde_json::{Value, json};

fn request() -> Value {
    json!({
        "schema_version": 1,
        "item_id": "host/session/item",
        "owner": {"session": "session", "run": null, "plan": null},
        "title": "Review release",
        "body": "Decide whether to release.",
        "priority": "high",
        "repositories": ["github.com/operator/product"],
        "project": null,
        "source": "plans",
        "state": "open",
        "reason": null
    })
}

#[test]
fn projection_request_round_trips_existing_priority_and_nullable_reason() {
    let value = request();
    let parsed: HumanTaskProjection = serde_json::from_value(value.clone()).expect("v1");
    assert_eq!(serde_json::to_value(parsed).expect("serialize"), value);
    for reason in [None, Some(Value::Null), Some(json!("Not needed"))] {
        let mut value = request();
        value["state"] = json!("dismissed");
        match reason {
            Some(reason) => value["reason"] = reason,
            None => {
                value.as_object_mut().expect("mapping").remove("reason");
            }
        }
        let parsed: HumanTaskProjection = serde_json::from_value(value.clone()).expect("dismissal");
        assert_eq!(parsed.reason.as_deref(), value["reason"].as_str());
    }
}

#[test]
fn projection_refuses_questions_unknown_fields_wrong_versions_and_malformed_origins() {
    for (key, invalid) in [
        ("schema_version", json!(2)),
        ("schema_version", json!("1")),
        ("state", json!("question")),
        ("priority", json!("critical")),
        (
            "repositories",
            json!(["https://github.com/operator/product.git"]),
        ),
        ("question", json!("Proceed?")),
        ("item_id", json!(12)),
    ] {
        let mut value = request();
        value[key] = invalid;
        assert!(
            serde_json::from_value::<HumanTaskProjection>(value).is_err(),
            "{key}"
        );
    }
    for key in ["schema_version", "item_id", "owner", "project", "state"] {
        let mut value = request();
        value.as_object_mut().expect("mapping").remove(key);
        assert!(
            serde_json::from_value::<HumanTaskProjection>(value).is_err(),
            "{key}"
        );
    }
    for key in ["run", "plan"] {
        let mut value = request();
        value["owner"]
            .as_object_mut()
            .expect("owner mapping")
            .remove(key);
        assert!(
            serde_json::from_value::<HumanTaskProjection>(value).is_err(),
            "{key}"
        );
    }
    let mut value = request();
    value["owner"]["question"] = json!("Proceed?");
    assert!(serde_json::from_value::<HumanTaskProjection>(value).is_err());
}

#[test]
fn projection_response_has_only_the_declared_card_binding() {
    let value = json!({"task":"human:ISSUE-1","url":"https://github.com/operator/product/issues/1",
        "project":"P-1","source":"human"});
    let parsed: HumanTaskProjected = serde_json::from_value(value.clone()).expect("binding");
    assert_eq!(serde_json::to_value(parsed).expect("serialize"), value);
    let mut invalid = value;
    invalid["question"] = json!("Proceed?");
    assert!(serde_json::from_value::<HumanTaskProjected>(invalid).is_err());
}

#[test]
fn exported_projection_schemas_come_from_the_domain_declaration() {
    let bundle = onetaskgraph_core::schema_bundle();
    for (name, derived) in [
        (
            "HumanTaskProjection",
            schemars::schema_for!(HumanTaskProjection),
        ),
        ("HumanTaskProjected", {
            let payload = schemars::schema_for!(HumanTaskProjected);
            let mut answer =
                schemars::schema_for!(onetaskgraph_core::boundary::WriteAnswer<HumanTaskProjected>);
            for key in ["title", "description"] {
                if let Some(value) = payload.get(key) {
                    answer.insert(key.to_owned(), value.clone());
                }
            }
            answer
        }),
    ] {
        assert_eq!(
            bundle["roots"][name],
            serde_json::to_value(derived).expect("schema")
        );
    }
    let schema = &bundle["roots"]["HumanTaskProjection"];
    let validator = jsonschema::validator_for(schema).expect("valid request schema");
    assert!(validator.is_valid(&request()));
    let mut question = request();
    question["state"] = json!("question");
    assert!(!validator.is_valid(&question));
    let mut version = request();
    version["schema_version"] = json!(2);
    assert!(!validator.is_valid(&version));
    let mut absent_reason = request();
    absent_reason
        .as_object_mut()
        .expect("mapping")
        .remove("reason");
    assert!(validator.is_valid(&absent_reason));
}

#[test]
fn projection_identities_validate_without_changing_the_published_wire_schema() {
    for field in ["item_id", "project"] {
        let mut value = request();
        value[field] = json!("");
        assert!(
            serde_json::from_value::<HumanTaskProjection>(value).is_err(),
            "{field}"
        );
    }
    for field in ["session", "run", "plan"] {
        let mut value = request();
        value["owner"][field] = json!("");
        assert!(
            serde_json::from_value::<HumanTaskProjection>(value).is_err(),
            "{field}"
        );
    }
    for (field, invalid) in [
        ("task", "not-qualified"),
        ("project", ""),
        ("url", "javascript:alert(1)"),
        ("url", "https://"),
        ("url", "https://user:secret@example.com/issue/1"),
    ] {
        let mut value = json!({"task":"human:ISSUE-1","project":"P-1","source":"human",
            "url":"https://github.com/operator/product/issues/1"});
        value[field] = json!(invalid);
        assert!(
            serde_json::from_value::<HumanTaskProjected>(value).is_err(),
            "{field}"
        );
    }
    let task =
        onetaskgraph_core::task_projection::ProjectedTaskId::new("human:ISSUE-1").expect("task");
    assert_eq!(task.global_id().to_string(), "human:ISSUE-1");
    let project =
        onetaskgraph_core::task_projection::ProjectionProjectId::new("P-1").expect("project");
    assert_eq!(project.native_id().as_str(), "P-1");
    let empty_native = onetaskgraph_core::GlobalId::new(
        onetaskgraph_plugin_api::SourceName::new("human".to_owned()).expect("source"),
        onetaskgraph_plugin_api::NativeId(String::new()),
    );
    assert!(onetaskgraph_core::task_projection::ProjectedTaskId::try_from(empty_native).is_err());
}

#[test]
fn projection_identities_refuse_whitespace_and_control_characters() {
    for invalid in [" item", "item ", "item\nline", "item\u{0}"] {
        let mut value = request();
        value["item_id"] = json!(invalid);
        assert!(
            serde_json::from_value::<HumanTaskProjection>(value).is_err(),
            "{invalid:?}"
        );
    }
}

#[test]
fn response_source_is_derived_and_deserialization_refuses_contradictions() {
    let value = json!({"task":"human:ISSUE-1","project":"P-1","source":"human",
        "url":"https://github.com/operator/product/issues/1"});
    let binding: HumanTaskProjected = serde_json::from_value(value.clone()).expect("binding");
    assert_eq!(binding.source().as_str(), "human");
    assert_eq!(binding.task().global_id().source, *binding.source());
    let mut contradiction = value.clone();
    contradiction["source"] = json!("other");
    assert!(serde_json::from_value::<HumanTaskProjected>(contradiction).is_err());
    for field in ["task", "project", "source", "url"] {
        let mut missing = value.clone();
        missing.as_object_mut().expect("mapping").remove(field);
        assert!(
            serde_json::from_value::<HumanTaskProjected>(missing).is_err(),
            "{field}"
        );
    }
}

#[test]
fn tracker_urls_never_retain_control_characters_stripped_by_url_parsing() {
    for url in [
        "\0https://github.com/operator/product/issues/1",
        "https://github.com/operator/product/issues/1\0",
    ] {
        let value = json!({"task":"human:ISSUE-1","project":"P-1","source":"human","url":url});
        assert!(
            serde_json::from_value::<HumanTaskProjected>(value).is_err(),
            "{url:?}"
        );
    }
}
