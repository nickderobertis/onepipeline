//! `task`, `project` and `document metadata set` as a **Rust caller** reaches them.
//!
//! Every test drives the engine's own public methods over a real `in-memory` source, wrapped
//! only to record which methods the engine called — and, in one test, to read a value back
//! the way a source that normalises what it stores would. The journeys that drive the same
//! verbs through the binary are in `crates/onetaskgraph-e2e/tests/e2e/metadata.rs`.

use std::sync::{Arc, Mutex};

use onetaskgraph_core::{
    ConfiguredSource, Engine, EngineError, Failure, GlobalId, MetadataSet, ResolvedSource,
};
use onetaskgraph_plugin_api::{
    Capabilities, DependencyEdge, Direction, Document, DocumentQuery, Health, Label, Location,
    MetadataKey, NativeId, Page, PageRequest, Project, ProjectQuery, SecretResolver, SourceError,
    SourceName, SourcePlugin as _, Status, StatusCategory, Task, TaskQuery, TaskRef, TaskSource,
    WriteSupport,
};
use secrecy::SecretString;
use serde_json::{Value, json};

struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

/// An `in-memory` source whose every call is recorded by name, and whose metadata writes read
/// back as [`ReadBack`] says.
struct Watched {
    inner: Box<dyn TaskSource>,
    calls: Arc<Mutex<Vec<&'static str>>>,
    read_back: ReadBack,
}

/// How a [`Watched`] source stores a metadata write.
#[derive(Clone, Copy)]
enum ReadBack {
    /// The value it was given, under the key it was given.
    AsGiven,
    /// The value's JSON text — a source that stores what it is given in a different shape
    /// from the one it was handed.
    AsJsonText,
    /// The value under [`MISFILED`] instead — a source that answers the write with a record
    /// not holding the key it was asked to set.
    Misfiled,
}

/// Where a [`ReadBack::Misfiled`] source puts every metadata write.
const MISFILED: &str = "myapp.misfiled";

impl Watched {
    fn called(&self, method: &'static str) {
        self.calls.lock().expect("the record").push(method);
    }

    fn stored(&self, value: &Value) -> Value {
        match self.read_back {
            ReadBack::AsJsonText => Value::String(value.to_string()),
            ReadBack::AsGiven | ReadBack::Misfiled => value.clone(),
        }
    }

    fn filed_under(&self, key: &MetadataKey) -> MetadataKey {
        match self.read_back {
            ReadBack::Misfiled => MetadataKey::new(MISFILED).expect("a caller key"),
            ReadBack::AsGiven | ReadBack::AsJsonText => key.clone(),
        }
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
    async fn get_document(&self, id: &NativeId) -> Result<Option<Document>, SourceError> {
        self.called("get_document");
        self.inner.get_document(id).await
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
    async fn query_documents(
        &self,
        query: &DocumentQuery,
        page: &PageRequest,
    ) -> Result<Page<Document>, SourceError> {
        self.called("query_documents");
        self.inner.query_documents(query, page).await
    }
    async fn labels(&self, page: &PageRequest) -> Result<Page<Label>, SourceError> {
        self.called("labels");
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
        self.called("project_dependencies");
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
    async fn set_task_metadata(
        &self,
        id: &NativeId,
        key: &MetadataKey,
        value: &Value,
    ) -> Result<Option<Task>, SourceError> {
        self.called("set_task_metadata");
        self.inner
            .set_task_metadata(id, &self.filed_under(key), &self.stored(value))
            .await
    }
    async fn set_project_metadata(
        &self,
        id: &NativeId,
        key: &MetadataKey,
        value: &Value,
    ) -> Result<Option<Project>, SourceError> {
        self.called("set_project_metadata");
        self.inner
            .set_project_metadata(id, &self.filed_under(key), &self.stored(value))
            .await
    }
    async fn set_document_metadata(
        &self,
        id: &NativeId,
        key: &MetadataKey,
        value: &Value,
    ) -> Result<Option<Document>, SourceError> {
        self.called("set_document_metadata");
        self.inner
            .set_document_metadata(id, &self.filed_under(key), &self.stored(value))
            .await
    }
}

/// The work every test here writes into: a task that delivers another, a project and a
/// document, each with a key of its own already held.
fn config(capabilities: Value) -> Value {
    json!({
        "capabilities": capabilities,
        "projects": [
            {"id": "P-1", "title": "Plan", "content": null,
             "status": {"category": "in-progress", "name": "Building"}, "labels": [],
             "metadata": {"myapp.kept": "project"}}
        ],
        "tasks": [
            {"id": "T-1", "title": "Build", "content": "body",
             "status": {"category": "in-progress", "name": "in-progress"}, "labels": [],
             "project": "P-1", "metadata": {"myapp.kept": "task"}, "delivers": ["T-2"]},
            {"id": "T-2", "title": "Ticket", "content": null,
             "status": {"category": "todo", "name": "todo"}, "labels": [],
             "delivered_by": ["work:T-1"]}
        ],
        "documents": [
            {"id": "D-1", "title": "Design", "content": "text", "labels": [], "project": "P-1",
             "location": {"path": "/somewhere/D-1.md"},
             "metadata": {"myapp.kept": "document"}}
        ]
    })
}

/// An engine over one watched source named `work`, and the record of what it was asked.
fn engine(capabilities: Value, read_back: ReadBack) -> (Engine, Arc<Mutex<Vec<&'static str>>>) {
    let name = SourceName::new("work").expect("a valid source name");
    let inner = onetaskgraph_in_memory::Plugin
        .build(&name, &config(capabilities), &NoSecrets)
        .expect("the in-memory plugin builds");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let source = ResolvedSource::adopt(
        name.clone(),
        Box::new(Watched {
            inner,
            calls: Arc::clone(&calls),
            read_back,
        }),
    );
    (
        Engine::new(vec![ConfiguredSource::Ready(source)], vec![name]),
        calls,
    )
}

/// The `kind` a failure is written with.
fn kind(failure: &Failure) -> String {
    serde_json::to_value(failure).expect("a failure serializes")["kind"]
        .as_str()
        .expect("a failure has a kind")
        .to_owned()
}

fn writable() -> Value {
    json!({"writes": "supported", "documents": "native"})
}

fn id(value: &str) -> GlobalId {
    value.parse().expect("a qualified id")
}

fn key(value: &str) -> MetadataKey {
    MetadataKey::new(value).expect("a caller key")
}

#[tokio::test]
async fn each_verb_sets_one_key_and_answers_with_what_the_source_holds() {
    let (engine, calls) = engine(writable(), ReadBack::AsGiven);
    let review = key("myapp.review");

    let task = engine
        .set_task_metadata(&id("work:T-1"), &review, &json!({"approved": true}))
        .await
        .expect("written");
    assert_eq!(
        task,
        MetadataSet {
            id: id("work:T-1"),
            key: review.clone(),
            value: json!({"approved": true}),
            location: None,
            // What the task holds after the write, at no version: this source answers none.
            result: task.result.clone(),
            fallback: None,
            spent: None,
        }
    );
    let result = task
        .result
        .as_ref()
        .expect("a task write answers what it left");
    assert_eq!(result.version, None);
    assert!(
        result.status.is_some() && result.priority.is_some(),
        "{result:?}"
    );
    let project = engine
        .set_project_metadata(&id("work:P-1"), &review, &json!([1, 2]))
        .await
        .expect("written");
    assert_eq!(project.value, json!([1, 2]));
    let document = engine
        .set_document_metadata(&id("work:D-1"), &review, &Value::Null)
        .await
        .expect("written");
    assert_eq!(document.value, Value::Null);
    assert_eq!(
        document.location,
        Some(Location::Path("/somewhere/D-1.md".to_owned()))
    );
    assert_eq!(
        serde_json::to_value(&document).expect("an answer serializes"),
        json!({"id": "work:D-1", "key": "myapp.review", "value": null,
               "location": {"path": "/somewhere/D-1.md"}})
    );
    assert_eq!(
        serde_json::to_value(&task).expect("an answer serializes"),
        json!({"id": "work:T-1", "key": "myapp.review", "value": {"approved": true},
               "result": serde_json::to_value(&task.result).expect("a result serializes")}),
        "an absent location is left out rather than written as null"
    );

    // The one write each, and nothing read around it.
    assert_eq!(
        *calls.lock().expect("the record"),
        [
            "set_task_metadata",
            "set_project_metadata",
            "set_document_metadata"
        ]
    );
    let held = engine
        .task(&id("work:T-1"))
        .await
        .expect("answers")
        .items
        .remove(0)
        .item;
    assert_eq!(
        serde_json::to_value(&held.metadata).expect("plain data"),
        json!({"myapp.kept": "task", "myapp.review": {"approved": true}})
    );
}

#[tokio::test]
async fn the_answer_is_the_value_the_source_reads_back_not_the_value_it_was_given() {
    let (engine, _) = engine(writable(), ReadBack::AsJsonText);
    let answered = engine
        .set_task_metadata(&id("work:T-1"), &key("myapp.review"), &json!({"n": 1}))
        .await
        .expect("written");
    assert_eq!(answered.value, json!("{\"n\":1}"));
    assert_ne!(answered.value, json!({"n": 1}));
}

#[tokio::test]
async fn a_source_answering_without_the_key_it_set_is_refused_as_malformed() {
    let (engine, _) = engine(writable(), ReadBack::Misfiled);
    let review = key("myapp.review");
    for (who, error) in [
        (
            "work:T-1",
            engine
                .set_task_metadata(&id("work:T-1"), &review, &json!(1))
                .await
                .expect_err("misfiled"),
        ),
        (
            "work:P-1",
            engine
                .set_project_metadata(&id("work:P-1"), &review, &json!(1))
                .await
                .expect_err("misfiled"),
        ),
        (
            "work:D-1",
            engine
                .set_document_metadata(&id("work:D-1"), &review, &json!(1))
                .await
                .expect_err("misfiled"),
        ),
    ] {
        assert!(
            matches!(
                &error,
                EngineError::SourceFailed {
                    error: SourceError::Malformed { .. },
                    ..
                }
            ),
            "{who}: {error:?}"
        );
        let failure = Failure::from(&error);
        assert!(
            failure.message().contains(&format!(
                "the record {who} it answered the write with does not hold the key myapp.review"
            )),
            "{}",
            failure.message()
        );
    }
}

#[tokio::test]
async fn a_metadata_set_on_a_delivering_task_leaves_what_it_delivers_alone() {
    let (engine, calls) = engine(writable(), ReadBack::AsGiven);
    let before = engine
        .task(&id("work:T-2"))
        .await
        .expect("answers")
        .items
        .remove(0)
        .item;
    calls.lock().expect("the record").clear();

    engine
        .set_task_metadata(&id("work:T-1"), &key("myapp.review"), &json!("done"))
        .await
        .expect("written");

    let calls = calls.lock().expect("the record").clone();
    assert_eq!(
        calls,
        ["set_task_metadata"],
        "no status or delivered_by write"
    );
    let after = engine
        .task(&id("work:T-2"))
        .await
        .expect("answers")
        .items
        .remove(0)
        .item;
    assert_eq!(after.status, before.status);
    assert_eq!(after.delivered_by, before.delivered_by);
}

#[tokio::test]
async fn every_refusal_names_its_cause() {
    let (engine, calls) = engine(writable(), ReadBack::AsGiven);
    let review = key("myapp.review");

    let unknown = engine
        .set_task_metadata(&id("elsewhere:T-1"), &review, &json!(1))
        .await
        .expect_err("no such source");
    assert!(
        matches!(unknown, EngineError::UnknownSource { .. }),
        "{unknown:?}"
    );
    assert_eq!(kind(&Failure::from(&unknown)), "unknown-source");

    for (error, expected, words) in [
        (
            engine
                .set_task_metadata(&id("work:T-9"), &review, &json!(1))
                .await
                .expect_err("no such task"),
            "no-such-item",
            "no task with the id work:T-9",
        ),
        (
            engine
                .set_project_metadata(&id("work:P-9"), &review, &json!(1))
                .await
                .expect_err("no such project"),
            "no-such-item",
            "no project with the id work:P-9",
        ),
        (
            engine
                .set_document_metadata(&id("work:D-9"), &review, &json!(1))
                .await
                .expect_err("no such document"),
            "no-such-item",
            "no document with the id work:D-9",
        ),
    ] {
        let failure = Failure::from(&error);
        assert_eq!(kind(&failure), expected);
        assert!(failure.message().contains(words), "{}", failure.message());
        assert!(failure.message().contains("next:"), "{}", failure.message());
    }

    let (read_only, calls_read_only) = read_only_engine();
    for error in [
        read_only
            .set_task_metadata(&id("work:T-1"), &review, &json!(1))
            .await
            .expect_err("no write side"),
        read_only
            .set_project_metadata(&id("work:P-1"), &review, &json!(1))
            .await
            .expect_err("no write side"),
        read_only
            .set_document_metadata(&id("work:D-1"), &review, &json!(1))
            .await
            .expect_err("no write side"),
    ] {
        let failure = Failure::from(&error);
        assert_eq!(kind(&failure), "not-writable");
        assert!(
            failure.message().contains("source work cannot write a")
                && failure.message().contains("which has no write side"),
            "{}",
            failure.message()
        );
    }
    assert!(calls_read_only.lock().expect("the record").is_empty());
    drop(calls);
}

/// [`engine`] over a source declaring no write side and native documents — a name of its
/// own because a test holding a local `engine` binding shadows that function.
fn read_only_engine() -> (Engine, Arc<Mutex<Vec<&'static str>>>) {
    engine(
        json!({"writes": "unsupported", "documents": "native"}),
        ReadBack::AsGiven,
    )
}

#[tokio::test]
async fn a_source_declaring_no_documents_is_never_asked_for_a_document_write() {
    let name = SourceName::new("work").expect("a valid source name");
    let inner = onetaskgraph_in_memory::Plugin
        .build(
            &name,
            &json!({"capabilities": {"writes": "supported"}}),
            &NoSecrets,
        )
        .expect("the in-memory plugin builds");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name.clone(),
            Box::new(Watched {
                inner,
                calls: Arc::clone(&calls),
                read_back: ReadBack::AsGiven,
            }),
        ))],
        vec![name],
    );
    let error = engine
        .set_document_metadata(&id("work:D-1"), &key("myapp.review"), &json!(1))
        .await
        .expect_err("no documents");
    assert_eq!(kind(&Failure::from(&error)), "no-documents");
    assert!(calls.lock().expect("the record").is_empty());
}
