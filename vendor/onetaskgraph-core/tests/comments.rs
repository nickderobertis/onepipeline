//! The comment verbs are the engine's own, driven as library calls.
//!
//! Not only through the command line, deliberately: this product is exposed three ways from
//! one engine, and the Rust caller that links the crate is the consumer a CLI-only proof would
//! strand. This is also where an in-memory source's add-then-list is proven, because its work
//! lives in the process that holds it and the journeys that drive the compiled binary can only
//! read one invocation's answer — see `crates/onetaskgraph-e2e/tests/e2e/comments.rs`.

use onetaskgraph_core::{
    CommentCounts, CommentList, Config, ConfiguredSource, CopyItems, CopyReport, CopyRequest,
    CopyScope, DeletedComment, Engine, EngineError, GlobalId, ResolvedSource,
};
use onetaskgraph_plugin_api::{
    Capabilities, Comment, CommentBody, Cursor, DependencyEdge, DependencySupport, Direction,
    Health, Label, NativeId, NewComment, Page, PageRequest, Priority, Project, ProjectQuery,
    SecretResolver, SourceError, SourceName, SourcePlugin, Status, StatusCategory, Support, Task,
    TaskQuery, TaskSource, WriteSupport,
};
use secrecy::SecretString;
use serde_json::{Value, json};

struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

fn name(value: &str) -> SourceName {
    SourceName::new(value).expect("a valid source name")
}

fn id(qualified: &str) -> GlobalId {
    qualified.parse().expect("a qualified id")
}

fn body(text: &str) -> CommentBody {
    CommentBody::new(text).expect("a non-empty body")
}

fn task_value(native: &str) -> Value {
    json!({"id": native, "title": native, "content": null,
           "status": {"category": "todo", "name": "Todo"}, "labels": []})
}

/// Five comments on `T-1`, so a source serving two rows at a time is walked over three pages.
fn seeded() -> Value {
    Value::Array(
        (1..=5)
            .map(|n| {
                json!({"task": "T-1", "comment": {
                    "id": format!("C-{n}"), "author": null,
                    "created_at": format!("2026-09-13T15:1{n}:00Z"),
                    "updated_at": format!("2026-09-13T15:1{n}:00Z"),
                    "body": format!("comment {n}\n"), "url": null}})
            })
            .collect(),
    )
}

/// Four sources: one whose comments can be written, one whose comments can only be read, one
/// whose tasks have none, and one configured incoherently enough that it never builds.
fn engine() -> Engine {
    let sources = json!({
        "memory": {"plugin": "in-memory", "config": {
            "capabilities": {"comments": "native", "max_page_size": 2},
            "tasks": [task_value("T-1"), task_value("T-2")],
            "comments": seeded(),
        }},
        "readonly": {"plugin": "in-memory", "config": {
            "capabilities": {"comments": "native", "writes": "unsupported"},
            "tasks": [task_value("T-1")],
            "comments": seeded(),
        }},
        "bare": {"plugin": "in-memory", "config": {"tasks": [task_value("T-1")]}},
        "broken": {"plugin": "in-memory", "config": {
            "tasks": [task_value("T-1"), task_value("T-1")],
        }},
    });
    let config = Config::from_document(json!({"sources": sources})).expect("a valid configuration");
    Engine::build(&config, &NoSecrets)
}

fn bodies(list: &CommentList) -> Vec<&str> {
    list.comments
        .iter()
        .map(|comment| comment.body.as_str())
        .collect()
}

#[tokio::test]
async fn a_comment_added_is_listed_edited_and_deleted_by_the_same_engine() {
    let engine = engine();
    let task = id("memory:T-1");

    let added = engine
        .add_comment(
            &task,
            &NewComment {
                body: body("Added.\n\n## with a heading\n"),
                author: Some("ada".to_owned()),
            },
        )
        .await
        .expect("the comment is added");
    assert_eq!(added.id.as_str(), "C-6");
    assert_eq!(added.author.as_deref(), Some("ada"));

    // Oldest first, walked to the end of the source's two-row pages.
    let listed = engine.comments(&task).await.expect("the comments read");
    assert_eq!(
        bodies(&listed),
        [
            "comment 1\n",
            "comment 2\n",
            "comment 3\n",
            "comment 4\n",
            "comment 5\n",
            "Added.\n\n## with a heading\n"
        ]
    );
    assert_eq!(listed.comments.last(), Some(&added));

    let edited = engine
        .edit_comment(&task, &added.id, &body("Added, then edited.\n"))
        .await
        .expect("the comment is edited");
    assert_eq!(edited.id, added.id);
    assert_eq!(edited.created_at, added.created_at);
    assert_eq!(
        engine.comments(&task).await.unwrap().comments.last(),
        Some(&edited)
    );

    let deleted = engine
        .delete_comment(&task, &NativeId::from("C-3"))
        .await
        .expect("the comment is deleted");
    assert_eq!(
        deleted,
        DeletedComment {
            deleted: NativeId::from("C-3"),
            result: None,
            fallback: None,
            spent: None,
        }
    );
    let remaining = engine.comments(&task).await.unwrap();
    assert_eq!(remaining.comments.len(), 5);
    assert!(
        remaining
            .comments
            .iter()
            .all(|comment| comment.id.as_str() != "C-3")
    );

    let detail = engine.task_detail(&task).await.expect("the task reads");
    assert_eq!(detail.response.items.len(), 1);
    assert!(detail.response.errors.is_empty());
    assert_eq!(detail.comments, Some(remaining.comments));
}

#[tokio::test]
async fn copies_on_a_source_keeping_the_marker_read_back_as_copies_and_stay_so_through_an_edit() {
    // `memory` keeps a copy's origin as the hidden marker the trait's default writes, so this
    // is that default's whole round trip: written, read back, edited as a person would, and
    // copied over again.
    let config = Config::from_document(json!({"sources": {
        "memory": {"plugin": "in-memory", "config": {
            "capabilities": {"comments": "native", "max_page_size": 2},
            "tasks": [task_value("T-1")],
            "comments": seeded(),
        }},
        "into": {"plugin": "in-memory", "config": {"capabilities": {"comments": "native"}}},
    }}))
    .expect("a valid configuration");
    let engine = Engine::build(&config, &NoSecrets);
    let copy = CopyRequest {
        items: CopyItems::new(vec![id("memory:T-1")]).expect("one item"),
        scope: CopyScope::Tasks,
        destination: name("into"),
        target_task: None,
        match_by: None,
        recreate: false,
        create: false,
        dry_run: false,
        comments: true,
        no_link: true,
        expect: None,
        status_policy: onetaskgraph_core::StatusPolicy::Source,
        statuses: Vec::new(),
        conditions: Default::default(),
    };
    let counts = |report: &CopyReport| report.items[0].comments().copied();
    let first = engine.copy(&copy).await.expect("the copy lands");
    assert_eq!(
        counts(&first),
        Some(CommentCounts {
            added: 5,
            ..CommentCounts::default()
        })
    );
    let landed = first.items[0].destination().expect("it landed").clone();
    let copies = engine
        .comments(&landed)
        .await
        .expect("the copies read")
        .comments;
    assert_eq!(copies.len(), 5);
    for (n, copy) in copies.iter().enumerate() {
        assert_eq!(
            copy.body,
            format!("comment {}\n", n + 1),
            "the marker is read out"
        );
        let origin = copy.origin.as_ref().expect("a copy reads back as one");
        assert_eq!(origin.task, "memory:T-1");
        assert_eq!(origin.id.as_str(), format!("C-{}", n + 1));
    }

    let again = engine.copy(&copy).await.expect("the copy runs again");
    assert_eq!(
        counts(&again),
        Some(CommentCounts {
            unchanged: 5,
            ..CommentCounts::default()
        }),
        "every copy is found as the copy it is, so nothing is added beside it"
    );

    // A person edits a copy where it landed: it is still that copy, and the next copy puts
    // back what its origin says.
    let edited = engine
        .edit_comment(&landed, &copies[0].id, &body("edited where it landed\n"))
        .await
        .expect("the copy is edited");
    assert_eq!(edited.body, "edited where it landed\n");
    assert_eq!(edited.origin, copies[0].origin);
    let restored = engine.copy(&copy).await.expect("the copy runs again");
    assert_eq!(
        counts(&restored),
        Some(CommentCounts {
            edited: 1,
            unchanged: 4,
            ..CommentCounts::default()
        })
    );
    let held = engine
        .comments(&landed)
        .await
        .expect("the copies read")
        .comments;
    assert_eq!(held.len(), 5);
    assert_eq!(held[0].body, "comment 1\n");
    assert_eq!(held[0].origin, copies[0].origin);
}

#[tokio::test]
async fn a_task_detail_carries_comments_only_for_a_task_found_on_a_source_that_has_them() {
    let engine = engine();

    let bare = engine.task_detail(&id("bare:T-1")).await.unwrap();
    assert_eq!(bare.response.items.len(), 1);
    assert_eq!(bare.comments, None);

    let missing = engine.task_detail(&id("memory:T-404")).await.unwrap();
    assert!(missing.response.items.is_empty());
    assert_eq!(missing.comments, None);

    let none = engine.task_detail(&id("memory:T-2")).await.unwrap();
    assert_eq!(none.comments, Some(Vec::new()));

    // Serialized, the list sits beside the response rather than inside it, and is absent —
    // not empty — where the source has no comments.
    let rendered = serde_json::to_value(&none).unwrap();
    assert_eq!(rendered["comments"], json!([]));
    assert!(rendered["items"].is_array());
    assert!(
        serde_json::to_value(&bare)
            .unwrap()
            .get("comments")
            .is_none()
    );
}

#[tokio::test]
async fn every_refusal_names_what_was_not_there_and_says_what_to_do_next() {
    let engine = engine();
    let new = || NewComment {
        body: body("x"),
        author: None,
    };
    let refusals: Vec<(EngineError, &str)> = vec![
        (
            engine.comments(&id("bare:T-1")).await.unwrap_err(),
            "source bare has no comments: its plugin is in-memory",
        ),
        (
            engine
                .add_comment(&id("bare:T-1"), &new())
                .await
                .unwrap_err(),
            "source bare has no comments",
        ),
        (
            engine
                .add_comment(&id("readonly:T-1"), &new())
                .await
                .unwrap_err(),
            "source readonly cannot be written",
        ),
        (
            engine
                .edit_comment(&id("readonly:T-1"), &NativeId::from("C-1"), &body("x"))
                .await
                .unwrap_err(),
            "source readonly cannot be written",
        ),
        (
            engine
                .add_comment(&id("memory:T-404"), &new())
                .await
                .unwrap_err(),
            "no task with the id memory:T-404",
        ),
        (
            engine.comments(&id("memory:T-404")).await.unwrap_err(),
            "no task with the id memory:T-404",
        ),
        (
            engine
                .edit_comment(&id("memory:T-1"), &NativeId::from("C-999"), &body("x"))
                .await
                .unwrap_err(),
            "task memory:T-1 has no comment with the id C-999",
        ),
        (
            engine
                .delete_comment(&id("memory:T-404"), &NativeId::from("C-1"))
                .await
                .unwrap_err(),
            "no task with the id memory:T-404",
        ),
        (
            engine.comments(&id("nowhere:T-1")).await.unwrap_err(),
            "no source named \"nowhere\"",
        ),
        (
            engine.comments(&id("broken:T-1")).await.unwrap_err(),
            "source broken could not be built",
        ),
    ];
    for (refusal, says) in refusals {
        let message = refusal.to_string();
        assert!(message.contains(says), "{message}");
        assert!(message.contains("next:"), "{message}");
    }

    // A source whose comments can only be read still answers the read.
    assert_eq!(
        engine
            .comments(&id("readonly:T-1"))
            .await
            .unwrap()
            .comments
            .len(),
        5
    );
}

/// How [`Misbehaving`] answers a comment read.
#[derive(Clone, Copy)]
enum Answer {
    /// Hands back the cursor it was given, so a walk that trusted it would never end.
    RepeatedCursor,
    /// Returns more rows than the page asked for.
    Overlong,
    /// Fails outright, and fails a task read of `T-gone`.
    Failure,
    /// Holds the task for the first page and not the second.
    Vanishing,
    /// Declines to read comments on this task, as a source does for an item that cannot
    /// carry any — a GitHub draft.
    Refusal,
}

/// A source that breaks one rule of the contract on cue, which no real plugin does on demand.
struct Misbehaving(Answer);

fn a_task(native: &str) -> Task {
    Task {
        version: None,
        id: NativeId::from(native),
        key: None,
        title: native.to_owned(),
        content: None,
        status: Status {
            category: StatusCategory::Todo,
            name: "Todo".to_owned(),
        },
        priority: Priority::None,
        labels: Vec::new(),
        project: None,
        url: None,
        location: None,
        created_at: None,
        updated_at: None,
        metadata: Default::default(),
        repositories: Vec::new(),
        delivers: Vec::new(),
        delivered_by: Vec::new(),
        classification: Default::default(),
        assignees: None,
    }
}

fn a_comment(native: &str) -> Comment {
    Comment {
        origin: None,
        id: NativeId::from(native),
        author: None,
        created_at: None,
        updated_at: None,
        body: "x".to_owned(),
        url: None,
    }
}

#[async_trait::async_trait]
impl TaskSource for Misbehaving {
    fn kind(&self) -> &'static str {
        "misbehaving"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            projects: Support::Native,
            documents: Support::Unsupported,
            comments: Support::Native,
            assets: Support::Unsupported,
            priority: Support::Unsupported,
            filter_by_priority: Support::Unsupported,
            filter_by_comment_activity: Support::Unsupported,
            filter_by_metadata: Support::Unsupported,
            filter_by_origin: Support::Unsupported,
            reconcile_status: Support::Unsupported,
            complete_by_issue: Support::Unsupported,
            orphan_tasks: Support::Native,
            filter_by_label: Support::Native,
            filter_by_status: Support::Native,
            search_title: Support::Native,
            search_content: Support::Native,
            task_dependencies: DependencySupport::BothDirections,
            project_dependencies: DependencySupport::BothDirections,
            max_page_size: 1,
            assignees: Support::Unsupported,
        }
    }
    fn writes(&self) -> WriteSupport {
        WriteSupport::Supported
    }
    async fn health(&self) -> Result<Health, SourceError> {
        Ok(Health {
            reachable: true,
            detail: None,
        })
    }
    async fn get_task(&self, id: &NativeId) -> Result<Option<Task>, SourceError> {
        match (self.0, id.as_str()) {
            (Answer::Failure, "T-gone") => Err(SourceError::Unavailable {
                message: "the task read failed".to_owned(),
            }),
            _ => Ok(Some(a_task(id.as_str()))),
        }
    }
    async fn get_project(&self, _id: &NativeId) -> Result<Option<Project>, SourceError> {
        Ok(None)
    }
    async fn query_tasks(
        &self,
        _query: &TaskQuery,
        _page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        Ok(Page::last(Vec::new()))
    }
    async fn query_projects(
        &self,
        _query: &ProjectQuery,
        _page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        Ok(Page::last(Vec::new()))
    }
    async fn labels(&self, _page: &PageRequest) -> Result<Page<Label>, SourceError> {
        Ok(Page::last(Vec::new()))
    }
    async fn task_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page::last(Vec::new()))
    }
    async fn project_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page::last(Vec::new()))
    }
    async fn task_comments(
        &self,
        _task: &NativeId,
        page: &PageRequest,
    ) -> Result<Option<Page<Comment>>, SourceError> {
        match self.0 {
            Answer::RepeatedCursor => Ok(Some(Page {
                items: Vec::new(),
                next: Some(page.cursor.clone().unwrap_or(Cursor("again".to_owned()))),
            })),
            Answer::Overlong => Ok(Some(Page::last(vec![a_comment("C-1"), a_comment("C-2")]))),
            Answer::Failure => Err(SourceError::Unavailable {
                message: "the comment read failed".to_owned(),
            }),
            Answer::Vanishing => Ok(page.cursor.is_none().then(|| Page {
                items: vec![a_comment("C-1")],
                next: Some(Cursor("1".to_owned())),
            })),
            Answer::Refusal => Err(SourceError::Refused {
                message: "this item is a draft, which has no comments".to_owned(),
            }),
        }
    }
    async fn add_comment(
        &self,
        _task: &NativeId,
        _comment: &NewComment,
    ) -> Result<Option<Comment>, SourceError> {
        Err(SourceError::Refused {
            message: "this source refuses every comment it is given".to_owned(),
        })
    }
    async fn edit_comment(
        &self,
        _task: &NativeId,
        _comment: &NativeId,
        _body: &CommentBody,
    ) -> Result<Option<Comment>, SourceError> {
        Ok(None)
    }
    async fn delete_comment(
        &self,
        _task: &NativeId,
        _comment: &NativeId,
    ) -> Result<Option<NativeId>, SourceError> {
        Err(SourceError::Unavailable {
            message: "the delete failed".to_owned(),
        })
    }
}

fn misbehaving(answer: Answer) -> Engine {
    Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name("odd"),
            Box::new(Misbehaving(answer)),
        ))],
        vec![name("odd")],
    )
}

#[tokio::test]
async fn a_walk_refuses_a_repeated_cursor_and_an_overlong_page_rather_than_trusting_them() {
    let repeated = misbehaving(Answer::RepeatedCursor)
        .comments(&id("odd:T-1"))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        repeated.contains("returned the cursor it was given"),
        "{repeated}"
    );

    let overlong = misbehaving(Answer::Overlong)
        .comments(&id("odd:T-1"))
        .await
        .unwrap_err()
        .to_string();
    assert!(overlong.contains("never more"), "{overlong}");

    // A task that is gone part way through a walk is gone.
    let vanished = misbehaving(Answer::Vanishing)
        .comments(&id("odd:T-1"))
        .await
        .unwrap_err();
    assert!(
        matches!(vanished, EngineError::NoSuchTask { .. }),
        "{vanished}"
    );
}

#[tokio::test]
async fn a_source_that_fails_a_comment_call_is_named_and_a_task_detail_keeps_the_task() {
    let engine = misbehaving(Answer::Failure);
    let task = id("odd:T-1");

    let failed = engine.comments(&task).await.unwrap_err();
    assert!(
        matches!(&failed, EngineError::SourceFailed { name, .. } if name == "odd"),
        "{failed}"
    );
    let added = engine
        .add_comment(
            &task,
            &NewComment {
                body: body("x"),
                author: None,
            },
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(added.contains("refuses every comment"), "{added}");
    let deleted = engine
        .delete_comment(&task, &NativeId::from("C-1"))
        .await
        .unwrap_err()
        .to_string();
    assert!(deleted.contains("the delete failed"), "{deleted}");

    // An edit the source answered with nothing, on a task the source then cannot read, reports
    // the read that failed rather than guessing which of the two was missing.
    let unknowable = engine
        .edit_comment(&id("odd:T-gone"), &NativeId::from("C-1"), &body("x"))
        .await
        .unwrap_err()
        .to_string();
    assert!(unknowable.contains("the task read failed"), "{unknowable}");

    let detail = engine.task_detail(&task).await.unwrap();
    assert_eq!(detail.response.items.len(), 1);
    assert_eq!(detail.comments, None);
    assert_eq!(detail.response.errors.len(), 1);
    assert!(
        detail.response.errors[0]
            .error
            .to_string()
            .contains("the comment read failed")
    );
}

#[tokio::test]
async fn a_task_detail_reports_a_comment_read_its_source_refuses_as_that_sources_failure() {
    let engine = misbehaving(Answer::Refusal);
    let task = id("odd:T-1");

    // Asked for directly, the refusal is the answer, named as the source's own.
    let refused = engine.comments(&task).await.unwrap_err().to_string();
    assert!(refused.contains("which has no comments"), "{refused}");

    // Asked for beside the task, the task is still reported and the refusal is a failure of
    // that source rather than an absence nobody explains: an omitted list would read exactly
    // like a source without comments.
    let detail = engine.task_detail(&task).await.unwrap();
    assert_eq!(detail.response.items.len(), 1);
    assert_eq!(detail.comments, None);
    assert_eq!(detail.response.errors.len(), 1);
    assert!(
        detail.response.errors[0]
            .error
            .to_string()
            .contains("which has no comments")
    );
}

/// Several tasks read at once answer each id exactly as reading it alone does — every comment
/// page walked, a source with none answered without them — and an id that alone would be
/// refused or answered with nothing carries why in its own detail instead.
#[tokio::test]
async fn task_details_answer_each_id_as_one_task_detail_does_in_the_order_asked() {
    let engine = engine();
    let ids = [
        id("memory:T-2"),
        id("memory:T-1"),
        id("bare:T-1"),
        id("broken:T-1"),
        id("memory:T-9"),
        id("nowhere:T-1"),
    ];
    let details = engine.task_details(&ids, true).await.details;
    assert_eq!(details.len(), ids.len());
    for (at, id) in ids.iter().enumerate().take(4) {
        assert_eq!(
            details[at],
            engine.task_detail(id).await.expect("a configured source"),
            "{id} reads as it does alone"
        );
    }
    // Five comments over pages of two, walked to the end from the page the read carried.
    assert_eq!(
        details[1].comments.as_ref().map(Vec::len),
        Some(5),
        "{:?}",
        details[1]
    );
    assert!(details[2].comments.is_none(), "a source with no comments");
    assert!(
        !details[3].response.errors.is_empty(),
        "a source that never built"
    );
    let missing = &details[4].response;
    assert!(missing.items.is_empty());
    assert!(
        missing.errors[0]
            .error
            .to_string()
            .contains("no task with the id memory:T-9"),
        "{missing:?}"
    );
    let unknown = &details[5].response;
    assert_eq!(unknown.errors[0].source, name("nowhere"));
    assert!(
        unknown.errors[0]
            .error
            .to_string()
            .contains("no source named \"nowhere\""),
        "{unknown:?}"
    );

    // Without comments, each is the bare task read.
    let records = engine.task_details(&ids[..2], false).await.details;
    for (detail, id) in records.iter().zip(&ids) {
        assert_eq!(detail.comments, None);
        assert_eq!(
            detail.response,
            engine.task(id).await.expect("a configured source")
        );
    }
}

enum DetailAnswer {
    Normal,
    Missing,
    Extra,
    Mismatched,
}

/// A real in-memory source whose batch answer can violate the source contract.
struct MalformedDetails {
    inner: Box<dyn TaskSource>,
    answer: DetailAnswer,
}

#[async_trait::async_trait]
impl TaskSource for MalformedDetails {
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    async fn health(&self) -> Result<Health, SourceError> {
        self.inner.health().await
    }
    async fn get_task(&self, id: &NativeId) -> Result<Option<Task>, SourceError> {
        self.inner.get_task(id).await
    }
    async fn get_project(&self, id: &NativeId) -> Result<Option<Project>, SourceError> {
        self.inner.get_project(id).await
    }
    async fn query_tasks(
        &self,
        query: &TaskQuery,
        page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        self.inner.query_tasks(query, page).await
    }
    async fn query_projects(
        &self,
        query: &ProjectQuery,
        page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        self.inner.query_projects(query, page).await
    }
    async fn labels(&self, page: &PageRequest) -> Result<Page<Label>, SourceError> {
        self.inner.labels(page).await
    }
    async fn task_dependencies(
        &self,
        id: &NativeId,
        direction: Direction,
        page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        self.inner.task_dependencies(id, direction, page).await
    }
    async fn project_dependencies(
        &self,
        id: &NativeId,
        direction: Direction,
        page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        self.inner.project_dependencies(id, direction, page).await
    }
    async fn get_task_details(
        &self,
        ids: &[NativeId],
        comments: Option<&PageRequest>,
    ) -> Vec<Result<Option<onetaskgraph_plugin_api::TaskDetailRead>, SourceError>> {
        let mut read = self.inner.get_task_details(ids, comments).await;
        match self.answer {
            DetailAnswer::Normal => {}
            DetailAnswer::Missing => {
                read.pop();
            }
            DetailAnswer::Extra => {
                if let Some(first) = read.first().cloned() {
                    read.push(first);
                }
            }
            DetailAnswer::Mismatched => {
                if let Some(Ok(Some(first))) = read.first_mut() {
                    first.task.id = NativeId::from("other");
                }
            }
        }
        read
    }
}

#[tokio::test]
async fn a_source_answering_the_wrong_number_of_details_fails_every_id_it_was_asked() {
    let inner = onetaskgraph_in_memory::Plugin
        .build(
            &name("short"),
            &json!({"tasks": [task_value("T-1"), task_value("T-2")]}),
            &NoSecrets,
        )
        .expect("the in-memory plugin builds");
    let engine = Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name("short"),
            Box::new(MalformedDetails {
                inner,
                answer: DetailAnswer::Missing,
            }),
        ))],
        vec![name("short")],
    );
    let details = engine
        .task_details(&[id("short:T-1"), id("short:T-2")], false)
        .await
        .details;
    assert_eq!(
        details.len(),
        2,
        "one detail per id, whatever the source answered"
    );
    for detail in &details {
        assert!(detail.response.items.is_empty(), "{detail:?}");
        assert!(
            matches!(&detail.response.errors[0].error, SourceError::Malformed { message }
                if message.contains("answered 1 task details for the 2 ids")),
            "{detail:?}"
        );
    }
}

/// A linking caller able to list an empty declaration set, sufficient for a complete sweep.
struct ListingPolicy;

#[async_trait::async_trait]
impl onetaskgraph_core::boundary::WritePolicy for ListingPolicy {
    async fn repository_visibility(
        &self,
        _: &onetaskgraph_plugin_api::Repository,
    ) -> Result<
        onetaskgraph_core::boundary::RepositoryVisibility,
        onetaskgraph_core::boundary::PolicyError,
    > {
        Ok(onetaskgraph_core::boundary::RepositoryVisibility::Public)
    }
    async fn check_public_write(
        &self,
        _: &onetaskgraph_core::boundary::PublicWriteInput,
    ) -> Result<onetaskgraph_core::boundary::WriteVerdict, onetaskgraph_core::boundary::PolicyError>
    {
        Ok(onetaskgraph_core::boundary::WriteVerdict::Pass)
    }
    async fn declared_terms(
        &self,
        _: &onetaskgraph_core::boundary::TermsRequest,
    ) -> Result<
        Option<Vec<onetaskgraph_core::boundary::DeclaredTerm>>,
        onetaskgraph_core::boundary::PolicyError,
    > {
        Ok(Some(Vec::new()))
    }
}

#[tokio::test]
async fn a_sweep_refuses_missing_extra_and_misidentified_batch_details() {
    for answer in [
        DetailAnswer::Normal,
        DetailAnswer::Missing,
        DetailAnswer::Extra,
        DetailAnswer::Mismatched,
    ] {
        let healthy = matches!(answer, DetailAnswer::Normal);
        let inner = onetaskgraph_in_memory::Plugin
            .build(
                &name("batch"),
                // This older source refuses commented listings, so the sweep must
                // validate its fallback batch read rather than use a plain listing.
                &json!({"capabilities":{"comments":"native"},
                    "tasks":[task_value("T-1"),task_value("T-2")]}),
                &NoSecrets,
            )
            .expect("source");
        let engine = Engine::new(
            vec![ConfiguredSource::Ready(ResolvedSource::adopt(
                name("batch"),
                Box::new(MalformedDetails { inner, answer }),
            ))],
            vec![name("batch")],
        )
        .with_write_policy(std::sync::Arc::new(ListingPolicy));
        let result = engine.sanitize_tasks(&name("batch"), false, true).await;
        if healthy {
            assert!(result.expect("complete sweep").coverage.complete);
        } else {
            let error = result.expect_err("a malformed batch is no clean sweep");
            assert!(
                matches!(
                    error,
                    EngineError::SourceRefused {
                        error: SourceError::Malformed { .. },
                        ..
                    }
                ),
                "{error}"
            );
        }
        for native in ["T-1", "T-2"] {
            let response = engine
                .task(&id(&format!("batch:{native}")))
                .await
                .expect("task");
            assert_eq!(response.items[0].item.title, native);
            assert_eq!(response.items[0].item.content, None);
        }
    }
}
