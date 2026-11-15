//! `Engine::update_task` as a **Rust caller** reaches it, over `in-memory` and `local-md`.
//!
//! Every test drives the engine's own public method over a real source of each kind, wrapped
//! only to record which methods the engine called — which is how "the engine reads nothing of
//! its own" is observed rather than assumed. What each test asserts is read back through the
//! source itself afterwards, and for `local-md` through the bytes of its file. The journeys
//! that drive the same operation through the binary, over GitHub Projects and Linear too, are
//! in `crates/onetaskgraph-e2e/tests/e2e/update.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use onetaskgraph_core::{
    Body, ConfiguredSource, DeliveryOutcome, Engine, EngineError, GlobalId, ResolvedSource,
    TaskCreate, TaskUpdated, UpdatedField,
};
use onetaskgraph_plugin_api::{
    Capabilities, DependencyEdge, DependencyEndpoint, DependencyKind, Direction, Health, ItemKind,
    Label, MetadataKey, Metering, NativeId, Page, PageRequest, Priority, Project, ProjectQuery,
    SecretResolver, SourceError, SourceName, SourcePlugin as _, Status, StatusCategory, Task,
    TaskQuery, TaskRef, TaskSource, TaskUpdate, TaskUpdateOutcome, WriteSupport,
};
use secrecy::SecretString;
use serde_json::{Value, json};

struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

/// A real source whose every call is recorded by name.
struct Watched {
    inner: Arc<dyn TaskSource>,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl Watched {
    fn called(&self, method: &'static str) {
        self.calls.lock().expect("the record").push(method);
    }
}

#[async_trait::async_trait]
impl TaskSource for Watched {
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    fn writes(&self) -> WriteSupport {
        self.inner.writes()
    }
    async fn health(&self) -> Result<Health, SourceError> {
        self.inner.health().await
    }
    async fn get_task(&self, id: &NativeId) -> Result<Option<Task>, SourceError> {
        self.called("get_task");
        self.inner.get_task(id).await
    }
    async fn get_project(&self, id: &NativeId) -> Result<Option<Project>, SourceError> {
        self.called("get_project");
        self.inner.get_project(id).await
    }
    async fn query_tasks(
        &self,
        query: &TaskQuery,
        page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        self.called("query_tasks");
        self.inner.query_tasks(query, page).await
    }
    async fn query_projects(
        &self,
        query: &ProjectQuery,
        page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        self.called("query_projects");
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
        self.called("task_dependencies");
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
    async fn set_task_status(
        &self,
        id: &NativeId,
        category: StatusCategory,
    ) -> Result<Option<Status>, SourceError> {
        self.called("set_task_status");
        self.inner.set_task_status(id, category).await
    }
    async fn set_delivered_by(
        &self,
        id: &NativeId,
        delivered_by: &[TaskRef],
    ) -> Result<Option<()>, SourceError> {
        self.called("set_delivered_by");
        self.inner.set_delivered_by(id, delivered_by).await
    }
    async fn update_task(
        &self,
        id: &NativeId,
        update: &TaskUpdate,
    ) -> Result<Option<TaskUpdateOutcome>, SourceError> {
        self.called("update_task");
        self.inner.update_task(id, update).await
    }
    async fn metering(&self) -> Result<Option<Metering>, SourceError> {
        self.inner.metering().await
    }
}

/// Which kind of source a test runs over.
#[derive(Clone, Copy, Debug)]
enum Kind {
    InMemory,
    LocalMd,
}

const KINDS: [Kind; 2] = [Kind::InMemory, Kind::LocalMd];

/// One source called `work` holding the task under update, `T-1`, and its neighbours: `T-2`,
/// which it depends on; `T-8`, which delivers it; and `T-9` and `T-7`, which it delivers and
/// could deliver.
struct Store {
    engine: Engine,
    inner: Arc<dyn TaskSource>,
    calls: Arc<Mutex<Vec<&'static str>>>,
    /// The folder a `local-md` store keeps, held so it outlives the test.
    folder: Option<tempfile::TempDir>,
}

impl Store {
    fn new(kind: Kind) -> Self {
        let name = SourceName::new("work").expect("a valid source name");
        let (inner, folder): (Box<dyn TaskSource>, _) = match kind {
            Kind::InMemory => (
                onetaskgraph_in_memory::Plugin
                    .build(&name, &in_memory(), &NoSecrets)
                    .expect("the in-memory plugin builds"),
                None,
            ),
            Kind::LocalMd => {
                let folder = tempfile::tempdir().expect("a scratch folder");
                local_md(folder.path());
                let config = json!({
                    "root": folder.path(),
                    "status_mapping": {
                        "todo": "todo", "queued": "queued", "in progress": "in-progress",
                        "done": "done", "cancelled": "cancelled", "failed": "cancelled"
                    }
                });
                (
                    onetaskgraph_local_md::Plugin
                        .build(&name, &config, &NoSecrets)
                        .expect("the local-md plugin builds"),
                    Some(folder),
                )
            }
        };
        let inner: Arc<dyn TaskSource> = Arc::from(inner);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let source = ResolvedSource::adopt(
            name.clone(),
            Box::new(Watched {
                inner: Arc::clone(&inner),
                calls: Arc::clone(&calls),
            }),
        );
        Self {
            engine: Engine::new(vec![ConfiguredSource::Ready(source)], vec![name]),
            inner,
            calls,
            folder,
        }
    }

    async fn update(&self, update: &TaskUpdate) -> Result<TaskUpdated, EngineError> {
        self.calls.lock().expect("the record").clear();
        self.engine.update_task(&id("work:T-1"), update).await
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().expect("the record").clone()
    }

    async fn task(&self, native: &str) -> Task {
        self.inner
            .get_task(&NativeId::from(native))
            .await
            .expect("a read")
            .expect("the task is held")
    }

    async fn edges(&self) -> Vec<String> {
        let page = self
            .inner
            .task_dependencies(
                &NativeId::from("T-1"),
                Direction::DependsOn,
                &PageRequest {
                    cursor: None,
                    limit: 100,
                },
            )
            .await
            .expect("a read");
        page.items
            .iter()
            .map(|edge| edge.to.id().to_owned())
            .collect()
    }

    /// The task file's bytes, for a store that keeps one.
    fn file(&self) -> Option<String> {
        self.folder.as_ref().map(|folder| {
            std::fs::read_to_string(folder.path().join("tasks/T-1.md")).expect("the task file")
        })
    }
}

fn id(value: &str) -> GlobalId {
    value.parse().expect("a qualified id")
}

fn key(value: &str) -> MetadataKey {
    MetadataKey::new(value).expect("a caller key")
}

fn status(category: StatusCategory, name: &str) -> Status {
    Status {
        category,
        name: name.to_owned(),
    }
}

fn in_memory() -> Value {
    let task = |id: &str, category: &str, extra: Value| {
        let mut task = json!({"id": id, "title": format!("Task {id}"), "content": null,
                              "status": {"category": category, "name": category},
                              "labels": [], "project": "P"});
        for (key, value) in extra.as_object().expect("an object") {
            task[key] = value.clone();
        }
        task
    };
    json!({
        "capabilities": {"writes": "supported", "priority": "native"},
        "projects": [{"id": "P", "title": "Plan", "content": null,
                      "status": {"category": "in-progress", "name": "in-progress"},
                      "labels": []}],
        "tasks": [
            task("T-1", "todo", json!({
                "title": "Build the engine", "content": "the body", "priority": "low",
                "labels": [{"id": "l", "name": "l", "color": null}],
                "metadata": {"team.a": 1, "team.b": "two", "team.c": [3]},
                "repositories": ["github.com/o/r"],
                "delivers": ["T-9"], "delivered_by": ["work:T-8"],
            })),
            task("T-2", "todo", json!({})),
            task("T-7", "todo", json!({})),
            task("T-8", "in-progress", json!({"delivers": ["T-1"]})),
            task("T-9", "todo", json!({"delivered_by": ["work:T-1"]})),
        ],
        "labels": [{"id": "l", "name": "l", "color": null}],
        "task_dependencies": [{"from": "T-1", "to": "T-2", "kind": "blocks"}],
    })
}

fn local_md(root: &Path) {
    std::fs::create_dir_all(root.join("tasks")).expect("the task folder");
    std::fs::create_dir_all(root.join("projects")).expect("the project folder");
    let write = |path: &str, text: &str| std::fs::write(root.join(path), text).expect("a file");
    write(
        "projects/P.md",
        "---\ntitle: Plan\nstatus: in progress\nproject: null\n---\nthe plan\n",
    );
    write(
        "tasks/T-1.md",
        "---\ntitle: Build the engine\nstatus: todo\npriority: low\nlabels: [l]\nproject: P\n\
         depends_on: [T-2]\nrepositories: [github.com/o/r]\nmetadata:\n  team.a: 1\n  team.b: \
         two\n  team.c: [3]\ndelivers: [\"T-9\"]\ndelivered_by: [\"work:T-8\"]\n---\nthe body\n",
    );
    for (id, status, extra) in [
        ("T-2", "todo", ""),
        ("T-7", "todo", ""),
        ("T-8", "in progress", "delivers: [\"T-1\"]\n"),
        ("T-9", "todo", "delivered_by: [\"work:T-1\"]\n"),
    ] {
        write(
            &format!("tasks/{id}.md"),
            &format!("---\ntitle: Task {id}\nstatus: {status}\nproject: P\n{extra}---\n"),
        );
    }
}

/// Every member of `task` an update of `named` fields must leave alone, as one comparable
/// value.
fn untouched(task: &Task) -> Value {
    json!({
        "labels": task.labels.iter().map(|label| &label.name).collect::<Vec<_>>(),
        "project": task.project,
        "repositories": task.repositories,
        "delivered_by": task.delivered_by,
        "url": task.url,
        "location": task.location,
    })
}

#[tokio::test]
async fn each_named_field_reads_back_as_asked_and_nothing_else_moves() {
    for kind in KINDS {
        let store = Store::new(kind);
        let before = store.task("T-1").await;
        let update = TaskUpdate {
            title: Some("Build the engine, again".to_owned()),
            content: Some("a new body\n\nwith two paragraphs".to_owned()),
            status: Some(status(StatusCategory::Cancelled, "failed")),
            priority: Some(Priority::High),
            metadata_set: BTreeMap::from([
                (key("team.a"), json!({"n": 2})),
                (key("team.d"), json!([true, null])),
            ]),
            metadata_remove: BTreeSet::from([key("team.b")]),
            ..TaskUpdate::default()
        };
        let answer = store.update(&update).await.expect("the update lands");
        let after = store.task("T-1").await;

        assert_eq!(after.title, "Build the engine, again", "{kind:?}");
        assert_eq!(
            after.content.as_deref(),
            Some("a new body\n\nwith two paragraphs"),
            "{kind:?}"
        );
        // A name that is not one of the category words keeps its name.
        assert_eq!(
            after.status,
            status(StatusCategory::Cancelled, "failed"),
            "{kind:?}"
        );
        assert_eq!(after.priority, Priority::High, "{kind:?}");
        assert_eq!(
            after.metadata,
            BTreeMap::from([
                ("team.a".to_owned(), json!({"n": 2})),
                ("team.c".to_owned(), json!([3])),
                ("team.d".to_owned(), json!([true, null])),
            ]),
            "{kind:?}: the removed key went and its neighbours stayed"
        );
        assert_eq!(untouched(&after), untouched(&before), "{kind:?}");
        assert_eq!(after.delivers, before.delivers, "{kind:?}");
        assert_eq!(store.edges().await, vec!["T-2"], "{kind:?}");

        assert_eq!(
            answer.written,
            BTreeSet::from([
                UpdatedField::Title,
                UpdatedField::Content,
                UpdatedField::Status,
                UpdatedField::Priority,
                UpdatedField::Metadata,
            ]),
            "{kind:?}"
        );
        assert_eq!(answer.id, id("work:T-1"));
        assert_eq!(
            answer.task.title, after.title,
            "{kind:?}: the answer is the read"
        );
        assert_eq!(answer.task.status, after.status, "{kind:?}");
        // The answer's lists are qualified, as every verb reports them.
        assert_eq!(
            answer
                .task
                .delivers
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["work:T-9"],
            "{kind:?}"
        );
        // Status was named, so the one task it delivers was re-evaluated, and nothing else of
        // the source was read or written to answer the update itself.
        assert_eq!(
            answer.delivered.len(),
            1,
            "{kind:?}: {:?}",
            answer.delivered
        );
        assert_eq!(answer.delivered[0].ticket, id("work:T-9"));
        assert_eq!(
            store.calls().first(),
            Some(&"update_task"),
            "{kind:?}: the engine read before it asked: {:?}",
            store.calls()
        );
        assert_eq!(
            store
                .calls()
                .iter()
                .filter(|call| **call == "update_task")
                .count(),
            1,
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn an_update_the_task_already_holds_writes_nothing_and_says_so() {
    for kind in KINDS {
        let store = Store::new(kind);
        let before = store.task("T-1").await;
        let file = store.file();
        let update = TaskUpdate {
            title: Some(before.title.clone()),
            content: before.content.clone(),
            status: Some(before.status.clone()),
            priority: Some(before.priority),
            metadata_set: BTreeMap::from([(key("team.a"), json!(1))]),
            metadata_remove: BTreeSet::from([key("team.absent")]),
            delivers: Some(vec![TaskRef::new("T-9".to_owned()).expect("an id")]),
            depends_on: Some(vec![DependencyEdge {
                from: DependencyEndpoint::from_native(NativeId::from("T-1"), ItemKind::Task),
                to: DependencyEndpoint::from_native(NativeId::from("T-2"), ItemKind::Task),
                kind: DependencyKind::Blocks,
            }]),
            repositories: Some(before.repositories.clone()),
            assignees: None,
        };
        let answer = store.update(&update).await.expect("an answer");
        assert!(answer.written.is_empty(), "{kind:?}: {:?}", answer.written);
        assert_eq!(store.task("T-1").await, before, "{kind:?}");
        assert_eq!(store.file(), file, "{kind:?}: the file was rewritten");
        // No write to the task itself; the delivered task was read to be re-evaluated, and
        // left as it was.
        assert!(
            !store.calls().contains(&"set_task_status"),
            "{kind:?}: {:?}",
            store.calls()
        );
        assert!(matches!(
            answer.delivered[0].outcome,
            DeliveryOutcome::Unchanged { .. }
        ));
    }
}

#[tokio::test]
async fn an_update_naming_no_field_answers_the_task_and_writes_and_re_evaluates_nothing() {
    for kind in KINDS {
        let store = Store::new(kind);
        let before = store.task("T-1").await;
        let file = store.file();
        let ticket = store.task("T-9").await;
        let answer = store
            .update(&TaskUpdate::default())
            .await
            .expect("an empty update is answered, not refused");
        assert!(answer.written.is_empty(), "{kind:?}: {:?}", answer.written);
        assert!(
            answer.delivered.is_empty(),
            "{kind:?}: {:?}",
            answer.delivered
        );
        assert_eq!(answer.task.title, before.title, "{kind:?}");
        assert_eq!(answer.task.metadata, before.metadata, "{kind:?}");
        assert_eq!(store.calls(), vec!["update_task"], "{kind:?}");
        assert_eq!(store.task("T-1").await, before, "{kind:?}");
        assert_eq!(store.file(), file, "{kind:?}: the file was rewritten");
        assert_eq!(store.task("T-9").await, ticket, "{kind:?}");
    }
}

#[tokio::test]
async fn one_changed_field_among_several_unchanged_is_the_only_one_reported_written() {
    for kind in KINDS {
        let store = Store::new(kind);
        let before = store.task("T-1").await;
        let update = TaskUpdate {
            title: Some(before.title.clone()),
            status: Some(before.status.clone()),
            priority: Some(before.priority),
            metadata_set: BTreeMap::from([(key("team.a"), json!(1)), (key("team.c"), json!(4))]),
            ..TaskUpdate::default()
        };
        let answer = store.update(&update).await.expect("the update lands");
        assert_eq!(
            answer.written,
            BTreeSet::from([UpdatedField::Metadata]),
            "{kind:?}"
        );
        assert_eq!(store.task("T-1").await.metadata["team.c"], json!(4));
        if let (Some(before), Some(after)) = (
            // What a narrow edit leaves: exactly one line of the file differs.
            Store::new(kind).file(),
            store.file(),
        ) {
            let changed: Vec<(&str, &str)> = before
                .lines()
                .zip(after.lines())
                .filter(|(was, is)| was != is)
                .collect();
            assert_eq!(changed, vec![("  team.c: [3]", "  \"team.c\": 4")]);
        }
    }
}

#[tokio::test]
async fn an_update_that_sets_and_removes_one_key_is_refused_before_anything_is_written() {
    for kind in KINDS {
        let store = Store::new(kind);
        let before = store.task("T-1").await;
        let file = store.file();
        let update = TaskUpdate {
            title: Some("changed".to_owned()),
            metadata_set: BTreeMap::from([(key("team.a"), json!(2))]),
            metadata_remove: BTreeSet::from([key("team.a")]),
            ..TaskUpdate::default()
        };
        let error = store.update(&update).await.expect_err("a contradiction");
        let EngineError::SourceFailed {
            error: SourceError::Refused { message },
            ..
        } = &error
        else {
            panic!("{kind:?}: refused as {error:?}");
        };
        assert!(message.contains("team.a"), "{message}");
        assert!(store.calls().is_empty(), "{kind:?}: {:?}", store.calls());
        assert_eq!(store.task("T-1").await, before, "{kind:?}");
        assert_eq!(store.file(), file, "{kind:?}");
    }
}

#[tokio::test]
async fn a_task_the_source_does_not_hold_is_no_such_task() {
    for kind in KINDS {
        let store = Store::new(kind);
        let error = store
            .engine
            .update_task(
                &id("work:T-404"),
                &TaskUpdate {
                    title: Some("x".to_owned()),
                    ..TaskUpdate::default()
                },
            )
            .await
            .expect_err("no such task");
        assert_eq!(
            error,
            EngineError::NoSuchTask {
                id: "work:T-404".to_owned()
            },
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn named_edges_replace_the_forward_edges_and_are_reported() {
    for kind in KINDS {
        let store = Store::new(kind);
        let update = TaskUpdate {
            // Qualified to the task's own source, which reaches the source as its native id.
            depends_on: Some(vec![DependencyEdge {
                from: DependencyEndpoint::from_native(NativeId::from("ignored"), ItemKind::Task),
                to: DependencyEndpoint::new("work:T-7".to_owned(), ItemKind::Task)
                    .expect("an endpoint"),
                kind: DependencyKind::Blocks,
            }]),
            ..TaskUpdate::default()
        };
        let before = store.task("T-1").await;
        let answer = store.update(&update).await.expect("the update lands");
        assert_eq!(answer.written, BTreeSet::from([UpdatedField::DependsOn]));
        assert_eq!(store.edges().await, vec!["T-7"], "{kind:?}");
        let after = store.task("T-1").await;
        let untokened = |task: &Task| Task {
            version: None,
            ..task.clone()
        };
        assert_eq!(untokened(&after), untokened(&before), "{kind:?}");
        // An edge is a change a task's token covers, so a source that gives one moves it.
        if before.version.is_some() {
            assert_ne!(after.version, before.version, "{kind:?}");
        }
        assert!(
            answer.delivered.is_empty(),
            "{kind:?}: neither list was named"
        );
    }
}

#[tokio::test]
async fn naming_delivers_keeps_each_ticket_in_step_including_the_one_it_dropped() {
    for kind in KINDS {
        let store = Store::new(kind);
        let update = TaskUpdate {
            delivers: Some(vec![TaskRef::new("work:T-7".to_owned()).expect("an id")]),
            ..TaskUpdate::default()
        };
        let answer = store.update(&update).await.expect("the update lands");
        assert_eq!(answer.written, BTreeSet::from([UpdatedField::Delivers]));
        let tickets: Vec<&GlobalId> = answer.delivered.iter().map(|entry| &entry.ticket).collect();
        assert_eq!(
            tickets,
            vec![&id("work:T-7"), &id("work:T-9")],
            "{kind:?}: the new ticket, then the dropped one"
        );
        let refs = |task: &Task| {
            task.delivered_by
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(refs(&store.task("T-7").await), vec!["work:T-1"], "{kind:?}");
        assert!(refs(&store.task("T-9").await).is_empty(), "{kind:?}");
    }
}

#[tokio::test]
async fn naming_status_on_a_deliverer_re_evaluates_its_tickets() {
    for kind in KINDS {
        let store = Store::new(kind);
        let update = TaskUpdate {
            status: Some(status(StatusCategory::InProgress, "in progress")),
            ..TaskUpdate::default()
        };
        let answer = store.update(&update).await.expect("the update lands");
        assert_eq!(answer.delivered.len(), 1, "{kind:?}");
        assert_eq!(
            answer.delivered[0].outcome,
            DeliveryOutcome::Written {
                from: StatusCategory::Todo,
                to: StatusCategory::InProgress
            },
            "{kind:?}"
        );
        assert_eq!(
            store.task("T-9").await.status.category,
            StatusCategory::InProgress,
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn an_update_naming_neither_status_nor_delivers_re_evaluates_nothing() {
    for kind in KINDS {
        let store = Store::new(kind);
        let update = TaskUpdate {
            title: Some("renamed".to_owned()),
            priority: Some(Priority::Urgent),
            ..TaskUpdate::default()
        };
        let ticket = store.task("T-9").await;
        let answer = store.update(&update).await.expect("the update lands");
        assert!(answer.delivered.is_empty(), "{kind:?}");
        assert_eq!(store.calls(), vec!["update_task"], "{kind:?}");
        assert_eq!(store.task("T-9").await, ticket, "{kind:?}");
    }
}

#[tokio::test]
async fn a_source_with_no_write_side_is_refused_before_it_is_asked() {
    let name = SourceName::new("work").expect("a valid source name");
    let mut config = in_memory();
    config["capabilities"]["writes"] = json!("unsupported");
    let source = onetaskgraph_in_memory::Plugin
        .build(&name, &config, &NoSecrets)
        .expect("the in-memory plugin builds");
    let engine = Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name.clone(),
            source,
        ))],
        vec![name],
    );
    let error = engine
        .update_task(
            &id("work:T-1"),
            &TaskUpdate {
                title: Some("x".to_owned()),
                ..TaskUpdate::default()
            },
        )
        .await
        .expect_err("no write side");
    assert_eq!(
        error,
        EngineError::UpdateNotWritable {
            name: "work".to_owned(),
            kind: "in-memory".to_owned()
        }
    );
    assert!(error.to_string().contains("next:"), "{error}");
    let unknown = engine
        .update_task(&id("elsewhere:T-1"), &TaskUpdate::default())
        .await
        .expect_err("an unknown source");
    assert!(
        matches!(unknown, EngineError::UnknownSource { .. }),
        "{unknown:?}"
    );
}

#[tokio::test]
async fn a_priority_for_a_source_that_holds_none_is_refused_before_it_is_asked() {
    let name = SourceName::new("work").expect("a valid source name");
    let mut config = in_memory();
    config["capabilities"]["priority"] = json!("unsupported");
    config["tasks"][0]["priority"] = json!("none");
    let source = onetaskgraph_in_memory::Plugin
        .build(&name, &config, &NoSecrets)
        .expect("the in-memory plugin builds");
    let engine = Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name.clone(),
            source,
        ))],
        vec![name],
    );
    let error = engine
        .update_task(
            &id("work:T-1"),
            &TaskUpdate {
                priority: Some(Priority::High),
                ..TaskUpdate::default()
            },
        )
        .await
        .expect_err("no priority");
    assert!(matches!(error, EngineError::NoPriority { .. }), "{error:?}");
}

#[tokio::test]
async fn a_status_named_by_its_category_word_takes_the_sources_own_word_for_it() {
    // What `task update --status in-progress` sends with no `--status-name`: the category's
    // own word, which this folder's mapping spells `in progress`. It lands in the folder's
    // word, as `task status set` would write it, rather than being refused.
    let store = Store::new(Kind::LocalMd);
    let answer = store
        .update(&TaskUpdate {
            status: Some(status(StatusCategory::InProgress, "in-progress")),
            ..TaskUpdate::default()
        })
        .await
        .expect("the update lands");
    assert_eq!(
        answer.task.status,
        status(StatusCategory::InProgress, "in progress")
    );
    assert!(
        store
            .file()
            .expect("a file")
            .contains("\nstatus: in progress\n")
    );

    // A word of its own that the mapping reads as another category is still refused.
    let error = store
        .update(&TaskUpdate {
            status: Some(status(StatusCategory::Done, "failed")),
            ..TaskUpdate::default()
        })
        .await
        .expect_err("failed reads as cancelled here");
    assert!(error.to_string().contains("failed"), "{error}");
}

/// A `local-md` store with the fixture's tasks, under an engine that reaches the plugin
/// itself — every write of it, `create_task`'s included — rather than through [`Watched`].
fn unwatched_local_md() -> (Engine, tempfile::TempDir) {
    let name = SourceName::new("work").expect("a valid source name");
    let folder = tempfile::tempdir().expect("a scratch folder");
    local_md(folder.path());
    let source = onetaskgraph_local_md::Plugin
        .build(&name, &json!({"root": folder.path()}), &NoSecrets)
        .expect("the local-md plugin builds");
    let engine = Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name.clone(),
            source,
        ))],
        vec![name],
    );
    (engine, folder)
}

/// What onepipeline writes for a node with `steps`: a sequence of step mappings, each ending
/// in a multi-line `task` that `local-md`'s own writer lays out as a `|` block scalar.
fn steps() -> Value {
    let task = "## What\nWork.\n\n## Acceptance criteria\n\n- x\n";
    json!([
        {"id": "build", "persona": "engineer", "task": task},
        {"id": "check", "persona": "reviewer", "deps": ["build"], "task": task},
    ])
}

#[tokio::test]
async fn a_task_the_source_wrote_with_steps_takes_an_update_of_another_key_byte_for_byte() {
    let (engine, folder) = unwatched_local_md();
    let created = engine
        .create_task(&TaskCreate {
            source: SourceName::new("work").expect("a valid source name"),
            project: NativeId::from("P"),
            title: "A node with steps".to_owned(),
            body: Body::plain("The node's body."),
            status: None,
            labels: Vec::new(),
            repositories: Vec::new(),
            depends_on: Vec::new(),
            delivers: Vec::new(),
            metadata: BTreeMap::from([
                (key("onepipeline.steps"), steps()),
                (key("onepipeline.kind"), json!("implement")),
            ]),
            assets: Vec::new(),
            classification: Default::default(),
            assignee: None,
        })
        .await
        .expect("the task is created")
        .task;
    let path = folder
        .path()
        .join(format!("tasks/{}.md", created.item.id.as_str()));
    let file = std::fs::read_to_string(&path).expect("the task file");
    assert!(
        file.contains("task: |"),
        "the writer lays the steps out as block scalars:\n{file}"
    );

    let answer = engine
        .update_task(
            &created.id,
            &TaskUpdate {
                metadata_set: BTreeMap::from([(key("onepipeline.node"), json!("x"))]),
                ..TaskUpdate::default()
            },
        )
        .await
        .expect("the update lands");
    assert_eq!(answer.written, BTreeSet::from([UpdatedField::Metadata]));
    let after = std::fs::read_to_string(&path).expect("the task file");
    assert_eq!(answer.task.metadata["onepipeline.node"], json!("x"));
    assert_eq!(answer.task.metadata["onepipeline.steps"], steps());
    let added = "  \"onepipeline.node\": \"x\"\n";
    let at = after.find(added).expect("the named key's line");
    assert_eq!(
        format!("{}{}", &after[..at], &after[at + added.len()..]),
        file,
        "{after}"
    );
}

#[tokio::test]
async fn an_update_naming_assignees_is_refused_unasked_by_a_source_not_opted_in_and_beyond_one_person()
 {
    let assign = |people: &[&str]| TaskUpdate {
        assignees: Some(people.iter().map(|person| (*person).to_owned()).collect()),
        ..TaskUpdate::default()
    };
    // Neither source opts in: the write is refused naming the setting, and the source is never
    // asked anything.
    for kind in KINDS {
        let store = Store::new(kind);
        let refused = store.update(&assign(&["ana"])).await;
        assert!(
            matches!(&refused, Err(EngineError::NoAssignees { name, .. }) if name == "work"),
            "{kind:?}: {refused:?}"
        );
        assert!(store.calls().is_empty(), "{kind:?}: {:?}", store.calls());
        let refused = store
            .engine
            .set_task_assignee(&id("work:T-1"), Some("ana"))
            .await;
        assert!(
            matches!(&refused, Err(EngineError::NoAssignees { .. })),
            "{kind:?}: {refused:?}"
        );
        assert!(store.calls().is_empty(), "{kind:?}: {:?}", store.calls());
    }

    // Opted in, an update naming two people is refused before the source is asked, and one
    // naming one replaces whatever the task held.
    let name = SourceName::new("work").expect("a valid source name");
    let mut config = in_memory();
    config["assignees"] = json!(true);
    let inner: Arc<dyn TaskSource> = Arc::from(
        onetaskgraph_in_memory::Plugin
            .build(&name, &config, &NoSecrets)
            .expect("the in-memory plugin builds"),
    );
    let calls = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name.clone(),
            Box::new(Watched {
                inner: Arc::clone(&inner),
                calls: Arc::clone(&calls),
            }),
        ))],
        vec![name],
    );
    let refused = engine
        .update_task(&id("work:T-1"), &assign(&["ana", "ben"]))
        .await;
    assert!(
        matches!(
            &refused,
            Err(EngineError::SeveralAssignees { count: 2, .. })
        ),
        "{refused:?}"
    );
    assert!(calls.lock().expect("the record").is_empty());
    let updated = engine
        .update_task(&id("work:T-1"), &assign(&["ana"]))
        .await
        .expect("the update");
    assert_eq!(updated.task.assignees, Some(vec!["ana".to_owned()]));
    assert!(updated.written.contains(&UpdatedField::Assignees));
    let set = engine
        .set_task_assignee(&id("work:T-1"), None)
        .await
        .expect("the unassign");
    assert!(set.assignees.is_empty());
}
