//! A task's priority and its content, as a **Rust caller** reaches them: the engine's own
//! `copy`, `tasks`, `set_task_priority` and `set_task_content` over real `in-memory` sources.
//!
//! The journeys that drive the same verbs through the binary, against every source kind, are
//! in `crates/onetaskgraph-e2e/tests/e2e/priority.rs`.

use onetaskgraph_core::{
    Config, CopyItems, CopyRequest, CopyScope, Engine, EngineError, Failure, Filters, GlobalId,
    Paging, Predicate, ProjectSelector, TaskContentSet, TaskPrioritySet, TaskRequest,
};
use onetaskgraph_plugin_api::{Priority, SecretResolver, SourceName};
use secrecy::SecretString;
use serde_json::{Value, json};

struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

fn engine_over(sources: Value) -> Engine {
    let config =
        Config::from_document(json!({ "sources": sources })).expect("a valid configuration");
    Engine::build(&config, &NoSecrets)
}

fn id(value: &str) -> GlobalId {
    value.parse().expect("a qualified id")
}

fn task(id: &str, priority: &str) -> Value {
    json!({"id": id, "title": format!("Task {id}"), "content": "body",
           "status": {"category": "todo", "name": "Todo"}, "labels": [],
           "priority": priority})
}

fn copy_to(destination: &str, items: &[&str]) -> CopyRequest {
    CopyRequest {
        items: CopyItems::new(items.iter().map(|item| id(item)).collect()).expect("items"),
        scope: CopyScope::Tasks,
        destination: SourceName::new(destination).expect("a name"),
        target_task: None,
        match_by: None,
        recreate: false,
        create: false,
        dry_run: false,
        comments: false,
        no_link: false,
        expect: None,
        status_policy: onetaskgraph_core::StatusPolicy::Source,
        statuses: Vec::new(),
        conditions: Default::default(),
    }
}

fn request(priorities: Vec<Priority>) -> TaskRequest {
    TaskRequest {
        sources: Vec::new(),
        filters: Filters::default(),
        project: ProjectSelector::Any,
        priorities,
        commented_since: None,
        metadata: Vec::new(),
        origin: None,
        include_members: false,
        paging: Paging {
            limit: std::num::NonZeroU32::new(50).expect("not zero"),
            token: None,
        },
    }
}

#[tokio::test]
async fn a_copy_carries_a_priority_on_create_and_on_update_and_back_to_none() {
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [task("T-1", "urgent")]}},
        "into": {"plugin": "in-memory", "config": {}},
    }));
    let report = engine
        .copy(&copy_to("into", &["from:T-1"]))
        .await
        .expect("copies");
    let landed = report.items[0]
        .destination()
        .expect("a created item")
        .clone();
    let held = engine.task(&landed).await.expect("reads");
    assert_eq!(held.items[0].item.priority, Priority::Urgent);

    for now in [Priority::None, Priority::Low] {
        engine
            .set_task_priority(&id("from:T-1"), now)
            .await
            .expect("sets");
        let report = engine
            .copy(&copy_to("into", &["from:T-1"]))
            .await
            .expect("copies");
        assert_eq!(report.items[0].destination(), Some(&landed), "an update");
        assert_eq!(report.items[0].action.name(), "updated");
        let held = engine.task(&landed).await.expect("reads");
        assert_eq!(held.items[0].item.priority, now);
    }
}

#[tokio::test]
async fn a_copy_carrying_a_priority_to_a_source_that_holds_none_is_refused_before_it_is_asked() {
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory",
                 "config": {"tasks": [task("T-1", "high"), task("T-2", "none")]}},
        "into": {"plugin": "in-memory",
                 "config": {"capabilities": {"priority": "unsupported"}}},
    }));
    let refused = engine
        .copy(&copy_to("into", &["from:T-2", "from:T-1"]))
        .await
        .expect_err("refused");
    let EngineError::NoPriority {
        name,
        kind,
        task,
        priority,
    } = &refused
    else {
        panic!("not the priority refusal: {refused:?}");
    };
    assert_eq!(
        (name.as_str(), kind.as_str(), task.as_str(), *priority),
        ("into", "in-memory", "from:T-1", Priority::High)
    );
    let failure = Failure::from(&refused);
    assert_eq!(
        serde_json::to_value(&failure).expect("a failure")["kind"],
        "no-priority"
    );
    assert!(
        failure
            .message()
            .contains("source into cannot hold the field priority"),
        "{}",
        failure.message()
    );
    // Planned before anything landed, so the task planned first was not written either.
    let held = engine
        .tasks(&TaskRequest {
            sources: vec![SourceName::new("into").expect("a name")],
            ..request(Vec::new())
        })
        .await
        .expect("reads");
    assert!(held.items.is_empty(), "{:?}", held.items);

    // A task carrying `none` copies exactly as it always did.
    engine
        .copy(&copy_to("into", &["from:T-2"]))
        .await
        .expect("a copy carrying none lands");
}

#[tokio::test]
async fn a_priority_filter_is_pushed_down_or_applied_by_the_engine_with_one_answer() {
    let tasks = json!([
        task("T-1", "high"),
        task("T-2", "none"),
        task("T-3", "urgent"),
        task("T-4", "low")
    ]);
    for (filtering, outcome) in [
        ("native", "pushed_down"),
        ("unsupported", "applied_locally"),
    ] {
        let engine = engine_over(json!({
            "work": {"plugin": "in-memory",
                     "config": {"capabilities": {"filter_by_priority": filtering,
                                                 "max_page_size": 1},
                                "tasks": tasks}},
        }));
        let answered = engine
            .tasks(&request(vec![Priority::Urgent, Priority::None]))
            .await
            .expect("answers");
        let ids: Vec<String> = answered
            .items
            .iter()
            .map(|task| task.id.to_string())
            .collect();
        assert_eq!(ids, ["work:T-2", "work:T-3"], "{filtering}");
        let plan = serde_json::to_value(&answered.plan.per_source[0]).expect("a plan");
        assert_eq!(plan[outcome], json!([Predicate::Priority]), "{filtering}");
    }
}

#[tokio::test]
async fn a_narrow_priority_or_content_write_answers_the_task_and_refuses_what_cannot_take_it() {
    let engine = engine_over(json!({
        "work": {"plugin": "in-memory", "config": {"tasks": [task("T-1", "low")]}},
        "frozen": {"plugin": "in-memory",
                   "config": {"capabilities": {"writes": "unsupported"},
                              "tasks": [task("T-1", "low")]}},
        "unranked": {"plugin": "in-memory",
                     "config": {"capabilities": {"priority": "unsupported"},
                                "tasks": [task("T-1", "none")]}},
    }));
    let set = engine
        .set_task_priority(&id("work:T-1"), Priority::High)
        .await
        .expect("sets");
    // What the task holds after the write, at the token an in-memory source derives.
    let result = set
        .result
        .clone()
        .expect("a task write answers what it left");
    assert!(
        result
            .version
            .as_deref()
            .is_some_and(|version| version.starts_with("mem1|")),
        "{result:?}"
    );
    assert_eq!(result.priority, Some(Priority::High));
    assert!(result.status.is_some());
    assert_eq!(
        set,
        TaskPrioritySet {
            id: id("work:T-1"),
            priority: Priority::High,
            result: set.result.clone(),
            fallback: None,
            spent: None,
            guard: None,
        }
    );
    let set = engine
        .set_task_content(&id("work:T-1"), "new body\n")
        .await
        .expect("sets");
    assert_eq!(
        set.result.as_ref().and_then(|result| result.priority),
        Some(Priority::High)
    );
    assert_eq!(
        set,
        TaskContentSet {
            id: id("work:T-1"),
            // A source that reports nothing of what it stores holds exactly the bytes sent.
            content: "new body\n".to_owned(),
            result: set.result.clone(),
            fallback: None,
            spent: None,
        }
    );
    assert!(matches!(
        engine
            .set_task_priority(&id("work:T-9"), Priority::High)
            .await,
        Err(EngineError::NoSuchTask { .. })
    ));
    assert!(matches!(
        engine.set_task_content(&id("frozen:T-1"), "x").await,
        Err(EngineError::ContentNotWritable { .. })
    ));
    assert!(matches!(
        engine
            .set_task_priority(&id("frozen:T-1"), Priority::High)
            .await,
        Err(EngineError::PriorityNotWritable { .. })
    ));
    for priority in [Priority::High, Priority::None] {
        assert!(matches!(
            engine
                .set_task_priority(&id("unranked:T-1"), priority)
                .await,
            Err(EngineError::NoPriority { .. })
        ));
    }
}
