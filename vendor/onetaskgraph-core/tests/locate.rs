//! `Engine::locate` as a Rust caller reaches it: the request type this crate exports, the
//! method on the engine, and the report it returns. The journeys that drive `task locate` as a
//! user does, against the copy after it, are in `crates/onetaskgraph-e2e/tests/e2e/locate.rs`.

use onetaskgraph_core::{
    Config, ConfiguredSource, CopyItems, CopyRequest, CopyScope, CopyVia, Engine, EngineError,
    GlobalId, LocateLookup, LocateRequest, LocatedCounterpart, NotATask, ResolvedSource,
    StatusPolicy,
};
use onetaskgraph_plugin_api::{
    Capabilities, DependencyEdge, Direction, Document, Health, Label, NativeId, Page, PageRequest,
    Project, ProjectQuery, SecretResolver, SourceError, SourceName, SourcePlugin, Task, TaskQuery,
    TaskSource, WriteSupport,
};
use secrecy::SecretString;
use serde_json::{Value, json};

/// No source here needs a credential.
struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

fn id(value: &str) -> GlobalId {
    value.parse().expect("a qualified id")
}

fn task(native: &str, metadata: Value) -> Value {
    json!({"id": native, "title": native, "status": {"category": "todo", "name": "Todo"}, "labels": [],
           "metadata": metadata})
}

/// `from` holds `T-1`, `T-2` and the project `P-1`; `into` holds `D-1`, copied from `from:T-2`.
fn engine() -> Engine {
    let config = Config::from_document(json!({"sources": {
        "from": {"plugin": "in-memory", "config": {
            "tasks": [task("T-1", json!({})), task("T-2", json!({}))],
            "projects": [{"id": "P-1", "title": "Plan", "labels": [],
                          "status": {"category": "todo", "name": "Todo"}}],
        }},
        "into": {"plugin": "in-memory", "config": {
            "tasks": [task("D-1", json!({"onetaskgraph.origin": "from:T-2"}))],
        }},
    }}))
    .expect("a valid configuration");
    Engine::build(&config, &NoSecrets)
}

fn locating(items: &[&str], lookup: LocateLookup) -> LocateRequest {
    LocateRequest {
        items: items.iter().map(|item| id(item)).collect(),
        destination: SourceName::new("into").unwrap(),
        lookup,
    }
}

#[tokio::test]
async fn several_tasks_are_answered_in_the_order_named_and_nothing_meters_spends_nothing() {
    let engine = engine();
    let report = engine
        .locate(&locating(
            &["from:T-2", "from:T-1"],
            LocateLookup::Counterpart,
        ))
        .await
        .expect("both are located");
    let sources: Vec<String> = report
        .items
        .iter()
        .map(|item| item.source.to_string())
        .collect();
    assert_eq!(sources, ["from:T-2", "from:T-1"]);
    assert_eq!(
        report.items[0].counterpart,
        Some(Some(LocatedCounterpart {
            id: id("into:D-1"),
            via: CopyVia::Scan,
        }))
    );
    assert_eq!(report.items[0].carriers, Some(vec![id("into:D-1")]));
    assert_eq!(report.items[1].counterpart, Some(None));
    assert_eq!(report.spent, None, "no source here meters its requests");

    // The same answer a copy of them then acts on: `T-2` updates `D-1`, `T-1` is created.
    let copied = engine
        .copy(&CopyRequest {
            items: CopyItems::new(vec![id("from:T-2"), id("from:T-1")]).unwrap(),
            scope: CopyScope::Tasks,
            status_policy: StatusPolicy::Source,
            destination: SourceName::new("into").unwrap(),
            match_by: None,
            target_task: None,
            recreate: false,
            create: false,
            dry_run: true,
            comments: false,
            no_link: true,
            expect: None,
            statuses: Vec::new(),
            conditions: Default::default(),
        })
        .await
        .expect("the copy");
    assert_eq!(
        copied.items[0].destination().map(ToString::to_string),
        Some("into:D-1".to_owned())
    );
    assert_eq!(
        copied.items[1].destination(),
        None,
        "a dry run that would create"
    );
}

#[tokio::test]
async fn naming_no_task_answers_none_and_a_project_a_missing_id_and_two_targets_are_refused() {
    let engine = engine();
    let report = engine
        .locate(&locating(&[], LocateLookup::Counterpart))
        .await
        .expect("no task, no answer");
    assert!(report.items.is_empty() && report.spent.is_none());

    let refused = engine
        .locate(&locating(&["from:P-1"], LocateLookup::NoCounterpart))
        .await
        .expect_err("a project is no task");
    assert_eq!(
        refused,
        EngineError::NotATask {
            id: id("from:P-1"),
            kind: NotATask::Project,
        }
    );
    let missing = engine
        .locate(&locating(&["from:T-404"], LocateLookup::NoCounterpart))
        .await
        .expect_err("nothing there");
    assert!(
        matches!(missing, EngineError::NoSuchItem { .. }),
        "{missing:?}"
    );
    let several = engine
        .locate(&locating(
            &["from:T-1", "from:T-2"],
            LocateLookup::TargetNative("D-1".into()),
        ))
        .await
        .expect_err("--target-native names one task's counterpart");
    assert_eq!(several, EngineError::TargetNativeNeedsOne { count: 2 });
}

/// What a [`Faulty`] source fails as unreachable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fault {
    /// Every project read.
    Projects,
    /// Every document read.
    Documents,
    /// Every page of a task listing after the first.
    LaterTaskPages,
}

/// An `in-memory` source that fails what its [`Fault`] names, and answers everything else.
struct Faulty {
    inner: Box<dyn TaskSource>,
    fault: Fault,
    pages: std::sync::atomic::AtomicUsize,
}

impl Faulty {
    fn unreachable(what: &str) -> SourceError {
        SourceError::Unavailable {
            message: format!("the {what} could not be reached"),
        }
    }
}

#[async_trait::async_trait]
impl TaskSource for Faulty {
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
        self.inner.get_task(id).await
    }
    async fn get_project(&self, id: &NativeId) -> Result<Option<Project>, SourceError> {
        if self.fault == Fault::Projects {
            return Err(Self::unreachable("projects"));
        }
        self.inner.get_project(id).await
    }
    async fn get_document(&self, id: &NativeId) -> Result<Option<Document>, SourceError> {
        if self.fault == Fault::Documents {
            return Err(Self::unreachable("documents"));
        }
        self.inner.get_document(id).await
    }
    async fn query_tasks(
        &self,
        query: &TaskQuery,
        page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        let asked = self.pages.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fault == Fault::LaterTaskPages && asked > 0 {
            return Err(Self::unreachable("later pages"));
        }
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
}

/// An engine over `from` holding `T-1` and `T-2`, and `into` holding two items copied from
/// `from:T-2`, served one a page — the source named by `faulty` failing as `fault` says.
fn faulty_engine(faulty: &str, fault: Fault) -> Engine {
    let build = |name: &str, config: Value| {
        let built = onetaskgraph_in_memory::Plugin
            .build(&SourceName::new(name).unwrap(), &config, &NoSecrets)
            .expect("the in-memory plugin builds");
        let source: Box<dyn TaskSource> = if name == faulty {
            Box::new(Faulty {
                inner: built,
                fault,
                pages: std::sync::atomic::AtomicUsize::new(0),
            })
        } else {
            built
        };
        ConfiguredSource::Ready(ResolvedSource::adopt(
            SourceName::new(name).unwrap(),
            source,
        ))
    };
    let carrier = |native: &str| task(native, json!({"onetaskgraph.origin": "from:T-2"}));
    Engine::new(
        vec![
            build(
                "from",
                json!({"tasks": [task("T-1", json!({})), task("T-2", json!({}))],
                       "capabilities": {"documents": "native"}}),
            ),
            build(
                "into",
                json!({"tasks": [carrier("D-1"), carrier("D-2")],
                       "capabilities": {"max_page_size": 1}}),
            ),
        ],
        vec![
            SourceName::new("from").unwrap(),
            SourceName::new("into").unwrap(),
        ],
    )
}

#[tokio::test]
async fn an_id_naming_no_task_whose_project_or_document_read_fails_is_refused_by_that_failure() {
    for (fault, what) in [
        (Fault::Projects, "projects"),
        (Fault::Documents, "documents"),
    ] {
        let engine = faulty_engine("from", fault);
        let refused = engine
            .locate(&locating(&["from:X-1"], LocateLookup::NoCounterpart))
            .await
            .expect_err("whether it names something else cannot be read");
        let message = refused.to_string();
        assert!(
            message.contains(&format!("the {what} could not be reached")),
            "{what}: {message}"
        );
        assert!(
            !matches!(refused, EngineError::NoSuchItem { .. }),
            "{what}: never answered as naming nothing: {refused:?}"
        );
        // A task it holds is still located.
        let located = engine
            .locate(&locating(&["from:T-1"], LocateLookup::NoCounterpart))
            .await
            .expect("a task is read without the probe");
        assert_eq!(located.items.len(), 1);
    }
}

#[tokio::test]
async fn every_carrier_is_read_or_the_locate_fails_never_answering_part_of_them() {
    let engine = engine_paged();
    let located = engine
        .locate(&locating(&["from:T-2"], LocateLookup::Counterpart))
        .await
        .expect("both pages read");
    assert_eq!(
        located.items[0].carriers,
        Some(vec![id("into:D-1"), id("into:D-2")]),
        "one a page, both"
    );
    // A later page that fails, after the first found the counterpart, fails the locate.
    let engine = faulty_engine("into", Fault::LaterTaskPages);
    let refused = engine
        .locate(&locating(&["from:T-2"], LocateLookup::Counterpart))
        .await
        .expect_err("a carrier list it could not finish is no answer");
    assert!(
        refused
            .to_string()
            .contains("the later pages could not be reached"),
        "{refused}"
    );
}

/// [`faulty_engine`] with nothing failing.
fn engine_paged() -> Engine {
    faulty_engine("nothing", Fault::Projects)
}
