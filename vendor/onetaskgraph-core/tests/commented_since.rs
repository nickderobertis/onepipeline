//! A task list narrowed by comment activity, as a **Rust caller** reaches it: the engine's own
//! `tasks` over real `in-memory` sources — one applying the predicate itself, one leaving it to
//! the engine, which reads that source's comments task by task — and the one answer both owe.
//!
//! The journey that drives the same predicate through the binary is
//! `crates/onetaskgraph-e2e/tests/e2e/commented_since.rs`.

use chrono::{DateTime, Utc};
use onetaskgraph_core::{
    Config, Engine, Filters, Paging, Predicate, ProjectSelector, Qualified, QueryResponse,
    TaskRequest,
};
use onetaskgraph_plugin_api::{SecretResolver, SourceName, StatusCategory, Task};
use secrecy::SecretString;
use serde_json::{Value, json};

struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

/// The instant every query below asks about.
const SINCE: &str = "2026-09-20T12:00:00Z";

fn since() -> DateTime<Utc> {
    SINCE.parse().expect("an RFC 3339 instant")
}

fn engine_over(sources: Value) -> Engine {
    let config =
        Config::from_document(json!({ "sources": sources })).expect("a valid configuration");
    Engine::build(&config, &NoSecrets)
}

fn comment(id: &str, task: &str, created: &str, updated: &str) -> Value {
    json!({"task": task, "comment": {"id": id, "author": null, "created_at": created,
           "updated_at": updated, "body": "a word", "url": null}})
}

/// One in-memory store — commented after the instant, never commented, commented only before
/// it, an old comment edited after it and a task with an old comment and a new one — declared
/// with `capabilities`, and paged one task at a time so the engine's narrowing walks pages.
fn store(capabilities: &Value) -> Value {
    let task = |id: &str, category: &str| {
        json!({"id": id, "title": id, "content": null,
               "status": {"category": category, "name": category}, "labels": []})
    };
    let old = "2026-09-01T09:00:00Z";
    json!({"plugin": "in-memory", "config": {
        "capabilities": capabilities,
        "tasks": [
            task("T-new", "todo"),
            task("T-silent", "todo"),
            task("T-old", "todo"),
            task("T-edited", "done"),
            task("T-second", "in-progress"),
        ],
        "comments": [
            comment("c-1", "T-new", "2026-09-21T09:00:00Z", "2026-09-21T09:00:00Z"),
            comment("c-2", "T-old", old, "2026-09-02T09:00:00Z"),
            comment("c-3", "T-edited", old, "2026-09-25T09:00:00Z"),
            comment("c-4", "T-second", old, old),
            comment("c-5", "T-second", "2026-09-22T09:00:00Z", "2026-09-22T09:00:00Z"),
        ]
    }})
}

fn request(sources: &[&str], statuses: Vec<StatusCategory>) -> TaskRequest {
    TaskRequest {
        sources: sources
            .iter()
            .map(|name| SourceName::new(*name).expect("a name"))
            .collect(),
        filters: Filters {
            statuses,
            ..Filters::default()
        },
        project: ProjectSelector::Any,
        priorities: Vec::new(),
        commented_since: Some(since()),
        metadata: Vec::new(),
        origin: None,
        include_members: false,
        paging: Paging {
            limit: std::num::NonZeroU32::new(1).expect("not zero"),
            token: None,
        },
    }
}

/// Every task the request answers, walked page by page to exhaustion, by native id.
async fn walked(engine: &Engine, mut request: TaskRequest) -> (Vec<String>, Vec<Value>) {
    let mut ids = Vec::new();
    let mut plans = Vec::new();
    loop {
        let answered: QueryResponse<Qualified<Task>> =
            engine.tasks(&request).await.expect("answers");
        assert!(answered.errors.is_empty(), "{:?}", answered.errors);
        ids.extend(answered.items.iter().map(|task| task.item.id.to_string()));
        plans.extend(
            answered
                .plan
                .per_source
                .iter()
                .map(|plan| serde_json::to_value(plan).expect("a plan")),
        );
        match answered.next {
            Some(token) => request.paging.token = Some(token),
            None => return (ids, plans),
        }
    }
}

#[tokio::test]
async fn the_engine_narrowing_a_source_answers_exactly_what_a_native_source_answers() {
    let engine = engine_over(json!({
        "native": store(&json!({"comments": "native", "max_page_size": 1})),
        "narrowed": store(&json!({"comments": "native", "max_page_size": 1,
                                  "filter_by_comment_activity": "unsupported"})),
    }));
    for statuses in [
        Vec::new(),
        vec![StatusCategory::Todo],
        vec![StatusCategory::Done, StatusCategory::InProgress],
    ] {
        let (native, native_plans) = walked(&engine, request(&["native"], statuses.clone())).await;
        let (narrowed, narrowed_plans) =
            walked(&engine, request(&["narrowed"], statuses.clone())).await;
        assert_eq!(native, narrowed, "{statuses:?}");
        let expected: Vec<&str> = ["T-new", "T-edited", "T-second"]
            .into_iter()
            .filter(|id| match statuses.as_slice() {
                [] => true,
                [StatusCategory::Todo] => *id == "T-new",
                _ => *id != "T-new",
            })
            .collect();
        assert_eq!(native, expected, "{statuses:?}");
        assert!(
            native_plans.iter().all(|plan| plan["pushed_down"]
                .as_array()
                .expect("a list")
                .contains(&json!(Predicate::CommentedSince))),
            "{native_plans:?}"
        );
        assert!(
            narrowed_plans.iter().all(|plan| plan["applied_locally"]
                .as_array()
                .expect("a list")
                .contains(&json!(Predicate::CommentedSince))),
            "{narrowed_plans:?}"
        );
    }
}

#[tokio::test]
async fn a_source_whose_tasks_have_no_comments_holds_no_comment_activity() {
    let engine = engine_over(json!({
        "bare": {"plugin": "in-memory", "config": {
            "capabilities": {"filter_by_comment_activity": "unsupported"},
            "tasks": [{"id": "T-1", "title": "T-1", "content": null,
                       "status": {"category": "todo", "name": "Todo"}, "labels": []}]
        }},
    }));
    let (ids, plans) = walked(&engine, request(&["bare"], Vec::new())).await;
    assert!(ids.is_empty(), "{ids:?}");
    assert_eq!(
        plans[0]["applied_locally"],
        json!([Predicate::CommentedSince])
    );
}
