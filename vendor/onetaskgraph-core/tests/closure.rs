//! What the engine refuses of a source's answers about a project's status, as a **Rust caller**
//! reaches it: a project status write that reads back as another category, and a project held
//! for an issue named as delivering tasks — a project delivers none.
//!
//! A source is written by someone else, so what it answers is input. Each test wraps a real
//! `in-memory` source and bends the one answer under test, and holds the engine to refusing it
//! rather than reporting a write that did not land as asked. The journeys that drive the same
//! verbs through the binary are in `crates/onetaskgraph-e2e/tests/e2e/project_status.rs` and
//! `status_complete.rs`.

use std::sync::Arc;

use onetaskgraph_core::{
    CompleteAction, ConfiguredSource, Engine, EngineError, GlobalId, ResolvedSource,
};
use onetaskgraph_plugin_api::{
    Capabilities, Completion, DependencyEdge, Direction, Health, HeldTask, IssueHolding, IssueRef,
    ItemKind, Label, NativeId, Page, PageRequest, Project, ProjectQuery, SecretResolver,
    SourceError, SourceName, SourcePlugin as _, Status, StatusCategory, Support, Task, TaskQuery,
    TaskRef, TaskSource, WriteSupport,
};
use secrecy::SecretString;
use serde_json::json;

struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

/// A real `in-memory` source whose project status write reads back as `todo` whatever was
/// asked, and which holds its project `P-1` for every issue as delivering a task.
struct Bent {
    inner: Arc<dyn TaskSource>,
}

#[async_trait::async_trait]
impl TaskSource for Bent {
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            complete_by_issue: Support::Native,
            ..self.inner.capabilities()
        }
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
    async fn set_project_status(
        &self,
        id: &NativeId,
        _category: StatusCategory,
    ) -> Result<Option<Status>, SourceError> {
        self.inner
            .set_project_status(id, StatusCategory::Todo)
            .await
    }
    async fn issue_holdings(&self, issues: &[IssueRef]) -> Result<Vec<IssueHolding>, SourceError> {
        Ok(issues
            .iter()
            .map(|issue| IssueHolding {
                issue: issue.clone(),
                completion: Completion::Due {
                    task: HeldTask {
                        id: NativeId("P-1".into()),
                        kind: ItemKind::Project,
                        option: Some("In Progress".into()),
                        classification: Default::default(),
                        repositories: Vec::new(),
                        delivers: vec![TaskRef::new("T-1").expect("a task reference")],
                    },
                    to: "Done".into(),
                },
            })
            .collect())
    }
}

/// An engine over one bent source `work`, holding the project `P-1` at `in-progress` and the
/// task `T-1`.
fn engine() -> Engine {
    let name = SourceName::new("work").expect("a name");
    let config = json!({
        "projects": [{"id": "P-1", "title": "Plan", "content": null,
                      "status": {"category": "in-progress", "name": "in-progress"}, "labels": []}],
        "tasks": [{"id": "T-1", "title": "One", "content": null,
                   "status": {"category": "todo", "name": "todo"}, "labels": []}],
    });
    let inner: Arc<dyn TaskSource> = Arc::from(
        onetaskgraph_in_memory::Plugin
            .build(&name, &config, &NoSecrets)
            .expect("the in-memory plugin builds"),
    );
    Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name.clone(),
            Box::new(Bent { inner }),
        ))],
        vec![name],
    )
}

#[tokio::test]
async fn a_project_status_write_read_back_as_another_category_is_refused_as_malformed() {
    let engine = engine();
    let id: GlobalId = "work:P-1".parse().expect("a qualified id");
    let refused = engine
        .set_project_status(&id, StatusCategory::Done)
        .await
        .expect_err("a write that read back as todo is not a write to done");
    let EngineError::SourceFailed { error, .. } = &refused else {
        panic!("refused as the source's failure, not {refused:?}");
    };
    assert!(
        matches!(error, SourceError::Malformed { message }
            if message.contains("a write of work:P-1 to done read back as todo")),
        "{refused}"
    );
}

#[tokio::test]
async fn a_held_project_named_as_delivering_tasks_fails_every_reference_writing_nothing() {
    let engine = engine();
    let issues: Vec<IssueRef> = vec!["acme/plan#1".parse().expect("a reference")];
    let completed = engine
        .complete_issues(&issues, &[], false)
        .await
        .expect("the completion runs");
    let [item] = completed.items.as_slice() else {
        panic!("one item: {completed:?}");
    };
    assert_eq!(item.action, CompleteAction::Failed, "{completed:?}");
    assert!(
        item.error.as_ref().is_some_and(|error| error
            .message()
            .contains("answered project P-1 for acme/plan#1 as delivering tasks")),
        "{completed:?}"
    );
    let shown = engine
        .project(&"work:P-1".parse().expect("a qualified id"))
        .await
        .expect("the project verb answers");
    assert_eq!(
        shown.items.first().expect("held").item.status.category,
        StatusCategory::InProgress,
        "nothing was written"
    );
}
