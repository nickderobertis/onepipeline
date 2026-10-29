//! Human-item journeys through the real compiled CLI in explicit sample mode.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use onepipeline::human_items::{schemas, HumanItemSample};
use serde_json::{json, Value};

struct World(PathBuf);
impl World {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::var_os("ONEPIPELINE_NODE_SCRATCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "human-items-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("sample.json"),
            include_str!("fixtures/human-items.json"),
        )
        .unwrap();
        Self(root)
    }
    fn sample(&self) -> PathBuf {
        self.0.join("sample.json")
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_onepipeline"))
            .args(["human-items", "--sample"])
            .arg(self.sample())
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    fn json(&self, args: &[&str], name: &str) -> Value {
        let value = serde_json::from_str(&self.ok(args)).unwrap();
        assert_schema(name, &value);
        value
    }
    fn state(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.sample()).unwrap()).unwrap()
    }
    fn write(&self, value: &Value) {
        std::fs::write(self.sample(), serde_json::to_vec(value).unwrap()).unwrap();
    }
}
impl Drop for World {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn assert_schema(name: &str, value: &Value) {
    use onepipeline::human_items::{HumanItemDetail, HumanItemReceipt, HumanItemView};
    let roundtrip = match name {
        "HumanItemView" => {
            serde_json::to_value(serde_json::from_value::<HumanItemView>(value.clone()).unwrap())
                .unwrap()
        }
        "HumanItemDetail" => {
            serde_json::to_value(serde_json::from_value::<HumanItemDetail>(value.clone()).unwrap())
                .unwrap()
        }
        "HumanItemReceipt" => {
            serde_json::to_value(serde_json::from_value::<HumanItemReceipt>(value.clone()).unwrap())
                .unwrap()
        }
        _ => panic!("unexpected journey schema {name}"),
    };
    assert_eq!(&roundtrip, value);
    let schema = schemas();
    jsonschema::validator_for(&schema[name])
        .unwrap()
        .validate(value)
        .unwrap();
}
fn refuse(output: Output, reason: &str) {
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(reason),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn sample_list_report_show_and_every_source_selector_use_engine_schema() {
    let world = World::new();
    for verb in ["list", "report"] {
        let text = world.ok(&[verb]);
        for expected in [
            "Critical",
            "High",
            "Medium",
            "Low",
            "Restart the unavailable build service.",
            "Choose the gradual or immediate rollout.",
            "Read the design and confirm it in chat.",
            "Choose a name for the release.",
            "host",
            "plan plan-1 / run run-1",
            "session manager",
            "blocks 5 steps in plan-1",
            "blocks all of plan-1",
            "blocks all work on this host",
        ] {
            assert!(text.contains(expected), "{expected}: {text}");
        }
        for excluded in ["owner-ended", "owner_ended", "scope", "P1", "P2", "P3"] {
            assert!(!text.contains(excluded));
        }
        let all: Value = serde_json::from_str(&world.ok(&[verb, "--json"])).unwrap();
        assert_eq!(all.as_array().unwrap().len(), 4);
        for value in all.as_array().unwrap() {
            assert_schema("HumanItemView", value);
            let plans = value["blocked"]["plans"].as_array().unwrap();
            for plan in plans {
                assert_eq!(plan["preview"].as_array().unwrap().len(), 3);
            }
        }
        for (flag, value, expected) in [
            ("--kind", "question", 2),
            ("--blocking", "yes", 3),
            ("--blocking", "no", 1),
            ("--priority", "Critical", 1),
            ("--priority", "hIGH", 1),
            ("--priority", "Medium", 1),
            ("--priority", "Low", 1),
            ("--state", "open", 4),
            ("--session", "manager", 4),
            ("--run", "run-1", 2),
            ("--plan", "plan-1", 2),
        ] {
            let output: Value =
                serde_json::from_str(&world.ok(&[verb, flag, value, "--json"])).unwrap();
            assert_eq!(output.as_array().unwrap().len(), expected, "{flag} {value}");
            for item in output.as_array().unwrap() {
                assert_schema("HumanItemView", item);
            }
        }
        let host: Value = serde_json::from_str(&world.ok(&[verb, "--host", "--json"])).unwrap();
        assert_eq!(host[0]["item"]["priority"], "urgent");
        let no_host: Value =
            serde_json::from_str(&world.ok(&[verb, "--host", "--blocking", "no", "--json"]))
                .unwrap();
        assert_eq!(no_host, json!([]));
    }
    let detail = world.json(&["show", "item-2", "--json"], "HumanItemDetail");
    assert_eq!(detail["nodes"].as_array().unwrap().len(), 5);
    assert!(world
        .ok(&["show", "item-2"])
        .contains("run-1 / step-4: Step 4"));
    for bad in [
        ["list", "--scope", "host"],
        ["list", "--owner-ended", "yes"],
        ["list", "--priority", "P1"],
    ] {
        refuse(world.run(&bad), "error:");
    }
    refuse(world.run(&["show", "missing"]), "no such human item");
}

#[test]
fn sample_actions_round_trip_receipts_refuse_conflicts_and_preserve_resolutions() {
    let world = World::new();
    let answer = world.json(
        &["answer", "item-2", "--text", "Use gradual rollout."],
        "HumanItemReceipt",
    );
    assert_eq!(
        answer["item"]["item"]["resolution"]["text"],
        "Use gradual rollout."
    );
    refuse(
        world.run(&["answer", "item-2", "--text", "Overwrite"]),
        "conflict",
    );
    refuse(
        world.run(&["answer", "item-4", "--text", " "]),
        "must not be blank",
    );
    refuse(
        world.run(&["answer", "item-1", "--text", "Yes"]),
        "action does not match",
    );
    refuse(world.run(&["complete", "item-4"]), "action does not match");
    let completed = world.json(&["complete", "item-1"], "HumanItemReceipt");
    assert_eq!(completed["item"]["item"]["state"], "completed");
    let review = world.json(&["complete", "item-3"], "HumanItemReceipt");
    assert_eq!(review["item"]["item"]["state"], "completed");
    assert_eq!(
        world.state()["approvals"][0]["approval"],
        json!({"board_copy":"design.md","key":"v1"})
    );
    let dismissed = world.json(&["dismiss", "item-4"], "HumanItemReceipt");
    assert_eq!(
        dismissed["item"]["item"]["resolution"]["reason"],
        Value::Null
    );
    let resolved = world.json(&["show", "item-1", "--json"], "HumanItemDetail");
    assert_eq!(resolved["blocked"], json!({"total":0,"plans":[]}));
    assert_eq!(resolved["nodes"], json!([]));
    assert!(!world.ok(&["show", "item-1"]).contains("blocks all work"));
    let state = world.state();
    let parsed: HumanItemSample = serde_json::from_value(state.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), state);
    for state in ["answered", "completed", "dismissed"] {
        let items: Value =
            serde_json::from_str(&world.ok(&["list", "--state", state, "--json"])).unwrap();
        assert!(!items.as_array().unwrap().is_empty());
    }
    refuse(
        world.run(&["complete", "item-3", "--key", "v2"]),
        "unexpected argument",
    );
    refuse(world.run(&["approve", "item-3"]), "unrecognized subcommand");
}

#[test]
fn raise_dismiss_and_changed_design_recovery_are_explicitly_sample_only() {
    let world = World::new();
    let mut input = world.state()["items"][0]["item"].clone();
    for field in [
        "id",
        "state",
        "resolution",
        "tracker",
        "created_at",
        "updated_at",
    ] {
        input.as_object_mut().unwrap().remove(field);
    }
    input["dedup_key"] = json!("new-task");
    let path = world.0.join("input.json");
    std::fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
    let raised = world.json(
        &["raise", "--input", path.to_str().unwrap()],
        "HumanItemReceipt",
    );
    let id = raised["item"]["item"]["id"].as_str().unwrap();
    refuse(
        world.run(&["raise", "--input", path.to_str().unwrap()]),
        "dedup_key already exists",
    );
    let dismissed = world.json(
        &["dismiss", id, "--reason", "No longer needed"],
        "HumanItemReceipt",
    );
    assert_eq!(
        dismissed["item"]["item"]["resolution"]["reason"],
        "No longer needed"
    );
    let mut state = world.state();
    state["approval_keys"]["design.md"] = json!("v2");
    world.write(&state);
    refuse(
        world.run(&["complete", "item-3"]),
        "Re-read the current document",
    );
    let changed = world.state();
    assert_eq!(changed["items"][2]["item"]["state"], "dismissed");
    assert!(changed.get("approvals").is_none());
    let replacement = changed["items"].as_array().unwrap().last().unwrap()["item"]["id"]
        .as_str()
        .unwrap();
    refuse(world.run(&["complete", "item-3"]), "already resolved");
    assert_eq!(world.state()["items"], changed["items"]);
    world.json(&["complete", replacement], "HumanItemReceipt");
    assert_eq!(world.state()["approvals"][0]["approval"]["key"], "v2");
    for command in [
        vec!["list"],
        vec!["report"],
        vec!["show", "id"],
        vec!["raise", "--input", "missing"],
        vec!["answer", "id", "--text", "yes"],
        vec!["complete", "id"],
        vec!["dismiss", "id"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_onepipeline"))
            .arg("human-items")
            .args(command)
            .env("ONEPIPELINE_HOME", world.0.join("live-home"))
            .output()
            .unwrap();
        refuse(output, r#""kind":"unavailable""#);
        assert!(!Path::new(&world.0.join("live-home/human-items/events.ndjson")).exists());
    }
}

#[test]
fn sample_api_preserves_attribution_and_normalizes_every_absent_reason() {
    use onepipeline::human_items::{HumanItemAction, HumanItemState};
    for reason in [None, Some(""), Some("  "), Some("Retired")] {
        let mut sample: HumanItemSample =
            serde_json::from_str(include_str!("fixtures/human-items.json")).unwrap();
        let receipt = sample
            .apply(HumanItemAction::Dismiss {
                id: "item-4".into(),
                reason: reason.map(str::to_owned),
                actor: "manager".into(),
                on_behalf_of: Some("operator".into()),
            })
            .unwrap();
        assert_eq!(receipt.item.item.state, HumanItemState::Dismissed);
        let resolution = receipt.item.item.resolution.as_ref().unwrap();
        assert_eq!(resolution.actor, "manager");
        assert_eq!(resolution.on_behalf_of.as_deref(), Some("operator"));
        assert_eq!(
            resolution.reason.as_deref(),
            reason.filter(|r| !r.trim().is_empty())
        );
        assert_schema("HumanItemReceipt", &serde_json::to_value(receipt).unwrap());
    }
    for field in ["version", "title", "body", "priority"] {
        let mut sample: HumanItemSample =
            serde_json::from_str(include_str!("fixtures/human-items.json")).unwrap();
        let item = &mut sample.items[0].item;
        match field {
            "version" => item.schema_version = 2,
            "title" => item.title = " ".into(),
            "body" => item.body.clear(),
            "priority" => item.priority = onepipeline::human_items::Priority::None,
            _ => unreachable!(),
        }
        let before = sample.clone();
        assert!(
            sample
                .apply(HumanItemAction::Complete {
                    id: "item-1".into(),
                    actor: "user".into(),
                    on_behalf_of: None
                })
                .is_err(),
            "invalid constructed {field}"
        );
        assert_eq!(sample, before, "refusal must preserve constructed state");
    }
    let mut unsupported: HumanItemSample =
        serde_json::from_str(include_str!("fixtures/human-items.json")).unwrap();
    unsupported.schema_version = 2;
    assert!(unsupported
        .apply(HumanItemAction::Complete {
            id: "item-1".into(),
            actor: "user".into(),
            on_behalf_of: None
        })
        .is_err());
    let fixture: Value = serde_json::from_str(include_str!("fixtures/human-items.json")).unwrap();
    let mut immutable = fixture["items"][0]["item"].clone();
    for field in [
        "id",
        "state",
        "resolution",
        "tracker",
        "created_at",
        "updated_at",
    ] {
        immutable.as_object_mut().unwrap().remove(field);
    }
    immutable["dedup_key"] = json!("new-api-input");
    for field in ["version", "title", "body", "priority", "question-approval"] {
        let mut input: onepipeline::human_items::HumanItemInput =
            serde_json::from_value(immutable.clone()).unwrap();
        match field {
            "version" => input.schema_version = 2,
            "title" => input.title.clear(),
            "body" => input.body = " ".into(),
            "priority" => input.priority = onepipeline::human_items::Priority::None,
            "question-approval" => {
                input.kind = onepipeline::human_items::HumanItemKind::Question;
                input.approval = Some(onepipeline::human_items::HumanItemApproval {
                    board_copy: "design.md".into(),
                    key: "v1".into(),
                });
            }
            _ => unreachable!(),
        }
        let mut sample: HumanItemSample = serde_json::from_value(fixture.clone()).unwrap();
        let before = sample.clone();
        assert!(
            sample.apply(HumanItemAction::Raise(input)).is_err(),
            "invalid constructed input {field}"
        );
        assert_eq!(sample, before);
    }
    let mut sample: HumanItemSample =
        serde_json::from_str(include_str!("fixtures/human-items.json")).unwrap();
    sample
        .apply(HumanItemAction::Complete {
            id: "item-3".into(),
            actor: "manager".into(),
            on_behalf_of: Some("operator".into()),
        })
        .unwrap();
    assert_eq!(
        sample.approvals[0].on_behalf_of.as_deref(),
        Some("operator")
    );
    assert_eq!(sample.approvals[0].actor, "manager");
}

#[test]
fn sample_list_orders_blocking_priority_time_and_id_instead_of_fixture_order() {
    let world = World::new();
    let mut state = world.state();
    let base = state["items"][0].clone();
    let mut items = Vec::new();
    for (id, priority, block, time) in [
        ("urgent-nothing", "urgent", "nothing", "0"),
        ("low-host", "low", "host", "0"),
        ("high-later", "high", "host", "2"),
        ("high-b", "high", "host", "1"),
        ("high-a", "high", "host", "1"),
        ("critical-host", "urgent", "host", "9"),
    ] {
        let mut detail = base.clone();
        detail["item"]["id"] = json!(id);
        detail["item"]["dedup_key"] = json!(id);
        if block == "nothing" {
            detail["blocked"] = json!({"total":0,"plans":[]});
            detail["nodes"] = json!([]);
        }
        detail["item"]["priority"] = json!(priority);
        detail["item"]["block"] = json!({"scope":block});
        detail["item"]["created_at"] = json!(time);
        items.push(detail);
    }
    state["items"] = json!(items);
    world.write(&state);
    let listed: Value = serde_json::from_str(&world.ok(&["list", "--json"])).unwrap();
    let ids: Vec<_> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["item"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "critical-host",
            "high-a",
            "high-b",
            "high-later",
            "low-host",
            "urgent-nothing"
        ]
    );
}

#[test]
fn sample_boundaries_validate_fixtures_and_cover_source_and_authority_recovery() {
    let world = World::new();
    let output = Command::new(env!("CARGO_BIN_EXE_onepipeline"))
        .args(["human-items", "list", "--sample"])
        .arg(world.sample())
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 4);
    let original = world.state();
    for (case, reason) in [
        ("id", "duplicate human-item id"),
        ("dedup", "duplicate human-item dedup_key"),
        ("total", "blocked total differs"),
        ("count", "blocked total differs"),
        ("preview", "preview differs"),
        ("long-preview", "preview contains more than three"),
        ("nodes", "duplicate affected node"),
        (
            "nonblocking",
            "nonblocking items cannot report blocked work",
        ),
        ("duplicate-plan", "duplicate blocked plan"),
        ("overflow", "blocked count overflow"),
        ("short-preview", "preview must contain the first three"),
        ("node-count", "blocked total differs from affected nodes"),
        ("reordered-preview", "preview differs"),
        ("completed-question", "state, kind and resolution conflict"),
        ("question-approval", "approval is only valid on tasks"),
        ("missing-resolution", "state, kind and resolution conflict"),
        ("blank-answer", "state, kind and resolution conflict"),
        ("answered-reason", "state, kind and resolution conflict"),
        ("completed-text", "state, kind and resolution conflict"),
        ("completed-reason", "state, kind and resolution conflict"),
        ("dismissed-text", "state, kind and resolution conflict"),
    ] {
        let mut invalid = original.clone();
        match case {
            "id" => invalid["items"][1]["item"]["id"] = invalid["items"][0]["item"]["id"].clone(),
            "dedup" => {
                invalid["items"][1]["item"]["dedup_key"] =
                    invalid["items"][0]["item"]["dedup_key"].clone()
            }
            "total" => invalid["items"][0]["blocked"]["total"] = json!(6),
            "count" => invalid["items"][0]["blocked"]["plans"][0]["count"] = json!(6),
            "preview" => {
                invalid["items"][0]["blocked"]["plans"][0]["preview"][0]["node"] = json!("wrong")
            }
            "long-preview" => {
                let extra = invalid["items"][0]["nodes"][3].clone();
                invalid["items"][0]["blocked"]["plans"][0]["preview"]
                    .as_array_mut()
                    .unwrap()
                    .push(extra);
            }
            "nodes" => invalid["items"][0]["nodes"][4] = invalid["items"][0]["nodes"][0].clone(),
            "nonblocking" => invalid["items"][0]["item"]["block"] = json!({"scope":"nothing"}),
            "duplicate-plan" => {
                let plan = invalid["items"][0]["blocked"]["plans"][0].clone();
                invalid["items"][0]["blocked"]["plans"]
                    .as_array_mut()
                    .unwrap()
                    .push(plan);
            }
            "overflow" => {
                invalid["items"][0]["blocked"]["plans"][0]["count"] = json!(u64::MAX);
                let preview = invalid["items"][0]["nodes"][4].clone();
                invalid["items"][0]["blocked"]["plans"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"plan":"second-plan","count":1,"preview":[preview]}));
            }
            "short-preview" => {
                invalid["items"][0]["blocked"]["plans"][0]["preview"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            "node-count" => {
                invalid["items"][0]["nodes"].as_array_mut().unwrap().pop();
            }
            "reordered-preview" => {
                invalid["items"][0]["blocked"]["plans"][0]["preview"]
                    .as_array_mut()
                    .unwrap()
                    .swap(0, 1);
            }
            "question-approval" => {
                invalid["items"][1]["item"]["approval"] =
                    json!({"board_copy":"design.md","key":"v1"})
            }
            "missing-resolution" => invalid["items"][0]["item"]["state"] = json!("completed"),
            "completed-question" | "blank-answer" | "answered-reason" => {
                invalid["items"][1]["item"]["state"] = json!(if case == "completed-question" {
                    "completed"
                } else {
                    "answered"
                });
                invalid["items"][1]["item"]["resolution"] = json!({"actor":"user","on_behalf_of":null,"text":if case == "blank-answer" { json!(" ") } else if case == "completed-question" { Value::Null } else { json!("Yes") },"reason":if case == "answered-reason" { json!("invalid") } else { Value::Null }});
            }
            "completed-text" | "completed-reason" | "dismissed-text" => {
                invalid["items"][0]["item"]["state"] = json!(if case == "dismissed-text" {
                    "dismissed"
                } else {
                    "completed"
                });
                invalid["items"][0]["item"]["resolution"] = json!({"actor":"user","on_behalf_of":null,"text":if case == "completed-reason" { Value::Null } else { json!("invalid") },"reason":if case == "completed-reason" { json!("invalid") } else { Value::Null }});
            }
            _ => unreachable!(),
        }
        world.write(&invalid);
        refuse(world.run(&["list"]), reason);
        assert_eq!(
            world.state(),
            invalid,
            "refused input must not be rewritten"
        );
    }
    let mut state = original.clone();
    state["approval_keys"] = json!({});
    world.write(&state);
    refuse(
        world.run(&["complete", "item-3"]),
        "sample approval authority has no evidence",
    );
    assert_eq!(world.state(), state);
    state["approval_keys"]["design.md"] = json!("v1");
    world.write(&state);
    world.json(&["complete", "item-3"], "HumanItemReceipt");

    let mut state = original.clone();
    state["items"][0]["item"]["id"] = json!("sample-5");
    world.write(&state);
    let mut input = original["items"][0]["item"].clone();
    for field in [
        "id",
        "state",
        "resolution",
        "tracker",
        "created_at",
        "updated_at",
    ] {
        input.as_object_mut().unwrap().remove(field);
    }
    input["dedup_key"] = json!("collision-recovery");
    let input_file = world.0.join("collision-input.json");
    std::fs::write(&input_file, serde_json::to_vec(&input).unwrap()).unwrap();
    let receipt = world.json(
        &["raise", "--input", input_file.to_str().unwrap()],
        "HumanItemReceipt",
    );
    assert_eq!(receipt["item"]["item"]["id"], "sample-6");

    let mut state = original.clone();
    state["items"][0]["item"]["block"] = json!({"scope":"nothing"});
    state["items"][0]["blocked"] = json!({"total":0,"plans":[]});
    state["items"][0]["nodes"] = json!([]);
    state["items"][0]["item"]["owner"]["plan"] = json!("plan-only");
    state["items"][1]["item"]["owner"]["plan"] = Value::Null;
    world.write(&state);
    let text = world.ok(&["report"]);
    assert!(text.contains("plan plan-only"));
    assert!(text.contains("run run-1"));

    let mut multi = original;
    for node in multi["items"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .skip(2)
    {
        node["run"] = json!("run-2");
    }
    let nodes = multi["items"][0]["nodes"].as_array().unwrap().clone();
    multi["items"][0]["blocked"]["plans"] = json!([
        {"plan":"plan-1","count":2,"preview":nodes[..2]},
        {"plan":"plan-2","count":3,"preview":nodes[2..]}
    ]);
    world.write(&multi);
    let detail = world.json(&["show", "item-1", "--json"], "HumanItemDetail");
    assert_eq!(
        detail["blocked"]["plans"][0]["preview"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(detail["nodes"][2]["run"], "run-2");
    multi["items"][0]["blocked"]["plans"][1]["preview"][0] = nodes[0].clone();
    world.write(&multi);
    refuse(world.run(&["show", "item-1", "--json"]), "preview differs");
}

#[test]
fn sample_file_failures_refuse_without_a_success_receipt_or_state_change() {
    let world = World::new();
    let original = std::fs::read(world.sample()).unwrap();
    std::fs::remove_file(world.sample()).unwrap();
    refuse(world.run(&["list"]), "sample.json");
    std::fs::write(world.sample(), b"{").unwrap();
    refuse(world.run(&["list"]), "invalid:");
    std::fs::write(world.sample(), &original).unwrap();
    let input = world.0.join("raise.json");
    refuse(
        world.run(&["raise", "--input", input.to_str().unwrap()]),
        "invalid:",
    );
    std::fs::write(&input, b"{").unwrap();
    refuse(
        world.run(&["raise", "--input", input.to_str().unwrap()]),
        "invalid:",
    );
    let mut immutable = world.state()["items"][0]["item"].clone();
    for field in [
        "id",
        "state",
        "resolution",
        "tracker",
        "created_at",
        "updated_at",
    ] {
        immutable.as_object_mut().unwrap().remove(field);
    }
    for (field, value) in [
        ("schema_version", json!(2)),
        ("priority", json!("none")),
        ("body", json!(" ")),
        ("extra", json!(true)),
    ] {
        let mut invalid = immutable.clone();
        invalid[field] = value;
        std::fs::write(&input, serde_json::to_vec(&invalid).unwrap()).unwrap();
        refuse(
            world.run(&["raise", "--input", input.to_str().unwrap()]),
            "invalid:",
        );
    }
    immutable.as_object_mut().unwrap().remove("dedup_key");
    std::fs::write(&input, serde_json::to_vec(&immutable).unwrap()).unwrap();
    refuse(
        world.run(&["raise", "--input", input.to_str().unwrap()]),
        "invalid:",
    );
    refuse(world.run(&["complete", "missing"]), "no such human item");
    assert!(
        std::fs::read(world.sample()).unwrap() == original,
        "a refused mutation must preserve fixture bytes"
    );

    let permissions = std::fs::metadata(world.sample()).unwrap().permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(world.sample(), readonly).unwrap();
    let failed_write = world.run(&["complete", "item-1"]);
    std::fs::set_permissions(world.sample(), permissions).unwrap();
    assert!(
        failed_write.stdout.is_empty(),
        "no receipt may precede successful sample persistence"
    );
    refuse(failed_write, "invalid:");
    assert!(
        std::fs::read(world.sample()).unwrap() == original,
        "a refused mutation must preserve fixture bytes"
    );
}

#[test]
fn raising_a_work_hold_derives_its_affected_steps_or_refuses_without_context() {
    let world = World::new();
    let original = world.state();
    let mut input = original["items"][1]["item"].clone();
    for field in [
        "id",
        "state",
        "resolution",
        "tracker",
        "created_at",
        "updated_at",
    ] {
        input.as_object_mut().unwrap().remove(field);
    }
    input["kind"] = json!("task");
    input["dedup_key"] = json!("new-work-hold");
    input["title"] = json!("Unblock the build step");
    let path = world.0.join("work-input.json");
    std::fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
    let raised = world.json(
        &["raise", "--input", path.to_str().unwrap()],
        "HumanItemReceipt",
    );
    let id = raised["item"]["item"]["id"].as_str().unwrap().to_owned();
    assert_eq!(raised["item"]["blocked"], original["items"][1]["blocked"]);
    for command in ["list", "report"] {
        let text = world.ok(&[command]);
        let line = text
            .lines()
            .find(|line| line.starts_with(&format!("{id}  ")))
            .unwrap_or_else(|| panic!("{command} omits {id}: {text}"));
        assert!(line.ends_with("blocks 5 steps in plan-1"), "{line}");
    }
    let shown = world.ok(&["show", &id]);
    assert!(shown.contains("blocks 5 steps in plan-1"), "{shown}");
    for step in 0..5 {
        assert!(
            shown.contains(&format!("run-1 / step-{step}: Step {step}")),
            "{shown}"
        );
    }
    let detail = world.json(&["show", &id, "--json"], "HumanItemDetail");
    assert_eq!(detail["nodes"], original["items"][1]["nodes"]);

    let before = std::fs::read(world.sample()).unwrap();
    input["dedup_key"] = json!("unknown-work-hold");
    input["block"] = json!({"scope":"work","run":"run-1","node":"deploy"});
    std::fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
    let output = world.run(&["raise", "--input", path.to_str().unwrap()]);
    assert!(output.stdout.is_empty());
    refuse(output, "no affected-node context for run-1 / deploy");
    assert_eq!(std::fs::read(world.sample()).unwrap(), before);

    // Resolution clears a hold's impact, so a resolved hold on the same
    // run/node is no context for a new one.
    let world = World::new();
    world.json(&["dismiss", "item-2"], "HumanItemReceipt");
    let before = std::fs::read(world.sample()).unwrap();
    input["block"] = original["items"][1]["item"]["block"].clone();
    std::fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
    let output = world.run(&["raise", "--input", path.to_str().unwrap()]);
    assert!(output.stdout.is_empty());
    refuse(output, "no affected-node context for run-1 / build");
    assert_eq!(std::fs::read(world.sample()).unwrap(), before);
}
