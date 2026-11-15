//! The follow-up loop's store features as a **Rust caller** reaches them, through the engine's
//! public API over real folders of Markdown and a real in-memory source: every write held to a
//! version and its typed refusal, two held writes racing inside one process, the guarded
//! priority set, the repositories update, the comments listing, every page of a listing, and a
//! task read with its edges.
//!
//! The journeys that drive the same through the binary, against every source kind, are in
//! `crates/onetaskgraph-e2e/tests/e2e/held_writes.rs` and `comments_since.rs`.

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use onetaskgraph_core::{
    CommentsSinceRequest, Config, CopyItems, CopyRequest, CopyScope, Engine, EngineError, Filters,
    GlobalId, GuardOutcome, Paging, ProjectSelector, TaskRequest,
};
use onetaskgraph_plugin_api::{
    AuthorKind, CommentBody, MetadataKey, NewComment, Priority, Repository, SecretResolver,
    SourceName, StatusCategory, TaskUpdate,
};
use secrecy::SecretString;
use serde_json::{Value, json};

struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

fn engine_over(document: Value) -> Engine {
    let config = Config::from_document(document).expect("a valid configuration");
    Engine::build(&config, &NoSecrets)
}

fn id(value: &str) -> GlobalId {
    value.parse().expect("a qualified id")
}

fn mapping() -> Value {
    json!({"backlog": "backlog", "todo": "todo", "queued": "queued",
           "in-progress": "in-progress", "done": "done", "cancelled": "cancelled"})
}

fn task_file(root: &Path, name: &str, front: &str) {
    let tasks = root.join("tasks");
    std::fs::create_dir_all(&tasks).expect("the task folder");
    std::fs::write(
        tasks.join(format!("{name}.md")),
        format!("---\ntitle: {name}\nstatus: todo\npriority: high\n{front}---\nBody of {name}.\n"),
    )
    .expect("a task");
}

/// A `plan` folder of two tasks — `b` depending on `a` — and a `work` folder copies land in.
fn folders(root: &Path) -> Engine {
    task_file(&root.join("plan"), "a", "");
    task_file(&root.join("plan"), "b", "depends_on: [a]\n");
    std::fs::create_dir_all(root.join("work")).expect("a folder");
    engine_over(json!({
        "page_size": 1,
        "sources": {
            "plan": {"plugin": "local-md",
                     "config": {"root": root.join("plan"), "status_mapping": mapping()}},
            "work": {"plugin": "local-md",
                     "config": {"root": root.join("work"), "status_mapping": mapping()}},
        },
    }))
}

async fn version(engine: &Engine, task: &GlobalId) -> String {
    engine
        .task(task)
        .await
        .expect("read")
        .items
        .first()
        .and_then(|task| task.item.version.clone())
        .expect("a task carries its version")
}

fn copy(item: &str, to: &str) -> CopyRequest {
    CopyRequest {
        items: CopyItems::new(vec![id(item)]).expect("items"),
        scope: CopyScope::Tasks,
        destination: SourceName::new(to).expect("a name"),
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

fn body(text: &str) -> CommentBody {
    CommentBody::new(text).expect("a body")
}

/// One of the eight held writes, writing its `round`-th value.
async fn held(
    engine: &Engine,
    verb: usize,
    comment: &onetaskgraph_plugin_api::NativeId,
    round: usize,
    version: &str,
) -> Result<(), EngineError> {
    let a = id("plan:a");
    match verb {
        0 => engine
            .held_write(
                &a,
                version,
                engine.update_task(
                    &a,
                    &TaskUpdate {
                        title: Some(format!("Round {round}")),
                        ..TaskUpdate::default()
                    },
                ),
            )
            .await
            .map(drop),
        1 => engine
            .held_write(
                &a,
                version,
                engine.set_task_priority(&a, [Priority::Low, Priority::Urgent][round % 2]),
            )
            .await
            .map(drop),
        2 => engine
            .held_write(
                &a,
                version,
                engine.set_task_content(&a, &format!("Content {round}.\n")),
            )
            .await
            .map(drop),
        3 => engine
            .held_write(
                &a,
                version,
                engine.set_task_metadata(
                    &a,
                    &MetadataKey::new("followup.round").expect("a key"),
                    &json!(round),
                ),
            )
            .await
            .map(drop),
        4 => engine
            .held_write(
                &a,
                version,
                engine.set_task_status(
                    &a,
                    [StatusCategory::Backlog, StatusCategory::InProgress][round % 2],
                ),
            )
            .await
            .map(drop),
        5 => engine
            .held_write(
                &a,
                version,
                engine.add_comment(
                    &a,
                    &NewComment {
                        body: body(&format!("Follow-up {round}.\n")),
                        author: None,
                    },
                ),
            )
            .await
            .map(drop),
        6 => engine
            .held_write(
                &a,
                version,
                engine.edit_comment(&a, comment, &body(&format!("Edited {round}.\n"))),
            )
            .await
            .map(drop),
        _ => engine
            .copy_held(&copy("plan:a", "work"), version)
            .await
            .map(drop),
    }
}

#[tokio::test]
async fn every_held_write_lands_at_the_version_and_is_refused_as_moved_at_another() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = folders(root.path());
    let comment = engine
        .add_comment(
            &id("plan:a"),
            &NewComment {
                body: body("The comment an edit edits.\n"),
                author: None,
            },
        )
        .await
        .expect("added")
        .id;
    engine.copy(&copy("plan:a", "work")).await.expect("copied");
    let counterpart = engine
        .tasks_all(&TaskRequest {
            sources: vec![SourceName::new("work").expect("a name")],
            filters: Filters::default(),
            project: ProjectSelector::Any,
            priorities: Vec::new(),
            commented_since: None,
            metadata: Vec::new(),
            origin: None,
            include_members: false,
            paging: Paging {
                limit: NonZeroU32::new(1).expect("a page"),
                token: None,
            },
        })
        .await
        .expect("listed")
        .items[0]
        .id
        .clone();
    for verb in 0..8 {
        // A copy is held to the version of the destination task it writes over.
        let task = if verb == 7 {
            counterpart.clone()
        } else {
            id("plan:a")
        };
        let read = version(&engine, &task).await;
        held(&engine, verb, &comment, 2 * verb, &read)
            .await
            .unwrap_or_else(|error| panic!("verb {verb} at its version: {error}"));
        let written = version(&engine, &task).await;
        assert_ne!(written, read, "verb {verb} landed");

        // Another writer moves the task after this caller's read of it.
        engine
            .update_task(
                &task,
                &TaskUpdate {
                    title: Some(format!("Moved before {verb}")),
                    ..TaskUpdate::default()
                },
            )
            .await
            .expect("the other writer");
        let moved = version(&engine, &task).await;
        let before = engine.task(&task).await.expect("read").items[0]
            .item
            .clone();
        let refused = held(&engine, verb, &comment, 2 * verb + 1, &written)
            .await
            .expect_err("a stale version is refused");
        match &refused {
            EngineError::VersionMoved {
                found,
                expected,
                held,
                ..
            } => {
                assert_eq!(found, &moved, "verb {verb}");
                assert_eq!(expected, &written, "verb {verb}");
                assert_eq!(held.version.as_deref(), Some(moved.as_str()));
            }
            other => panic!("verb {verb}: {other}"),
        }
        assert_eq!(
            onetaskgraph_core::Failure::from(&refused).kind(),
            "version-moved"
        );
        let after = engine.task(&task).await.expect("read").items[0]
            .item
            .clone();
        assert_eq!(after, before, "verb {verb} wrote nothing");
    }
}

#[test]
fn two_held_writes_racing_in_one_process_on_an_in_memory_task_never_both_land() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("a runtime");
    runtime.block_on(async {
        let engine = Arc::new(engine_over(json!({"sources": {"memory": {
            "plugin": "in-memory",
            "config": {"capabilities": {"priority": "native"},
                       "tasks": [{"id": "T-1", "title": "Raced", "labels": [],
                                  "status": {"category": "todo", "name": "todo"}}]},
        }}})));
        let task = id("memory:T-1");
        for round in 0..64 {
            let version = version(&engine, &task).await;
            let racers = [Priority::Low, Priority::Urgent].map(|priority| {
                let (engine, task, version) = (Arc::clone(&engine), task.clone(), version.clone());
                tokio::spawn(async move {
                    engine
                        .held_write(&task, &version, engine.set_task_priority(&task, priority))
                        .await
                        .map(|set| set.priority)
                })
            });
            let mut landed = Vec::new();
            let mut refused = 0;
            for racer in racers {
                match racer.await.expect("the racer finished") {
                    Ok(priority) => landed.push(priority),
                    Err(EngineError::VersionMoved { .. }) => refused += 1,
                    Err(other) => panic!("round {round}: {other}"),
                }
            }
            assert_eq!((landed.len(), refused), (1, 1), "round {round}");
            let held = engine.task(&task).await.expect("read").items[0]
                .item
                .priority;
            assert_eq!(
                held, landed[0],
                "round {round}: what landed is what it holds"
            );
            // Back to none, unheld, for the next round.
            engine
                .set_task_priority(&task, Priority::None)
                .await
                .expect("reset");
        }
    });
}

#[tokio::test]
async fn the_guard_the_repositories_the_comments_every_page_and_the_edges() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = folders(root.path());
    let a = id("plan:a");

    let held = engine
        .set_task_priority_when(&a, Priority::Low, &[Priority::None])
        .await
        .expect("answered");
    assert_eq!(
        held.guard.map(|guard| guard.outcome()),
        Some(GuardOutcome::Held)
    );
    assert_eq!(held.priority, Priority::High);
    let written = engine
        .set_task_priority_when(&a, Priority::Low, &[Priority::High])
        .await
        .expect("written");
    assert_eq!(
        written.guard.map(|guard| guard.outcome()),
        Some(GuardOutcome::Written)
    );
    assert_eq!(written.priority, Priority::Low);

    let widgets = Repository::try_from("github.com/acme/widgets".to_owned()).expect("an origin");
    let updated = engine
        .update_task(
            &a,
            &TaskUpdate {
                repositories: Some(vec![widgets.clone()]),
                ..TaskUpdate::default()
            },
        )
        .await
        .expect("updated");
    assert_eq!(updated.task.repositories, vec![widgets]);
    assert_eq!(updated.task.title, "a");
    let cleared = engine
        .update_task(
            &a,
            &TaskUpdate {
                repositories: Some(Vec::new()),
                ..TaskUpdate::default()
            },
        )
        .await
        .expect("cleared");
    assert!(cleared.task.repositories.is_empty());

    engine
        .add_comment(
            &a,
            &NewComment {
                body: body("A bot follows up.\n"),
                author: Some("ci-helper[bot]".to_owned()),
            },
        )
        .await
        .expect("added");
    let listed = engine
        .comments_since(&CommentsSinceRequest {
            source: SourceName::new("plan").expect("a name"),
            since: "2020-01-01T00:00:00Z".parse().expect("an instant"),
            metadata: Vec::new(),
            statuses: Vec::new(),
            items: Vec::new(),
            page_size: NonZeroU32::new(1).expect("a page"),
        })
        .await
        .expect("listed");
    assert_eq!(listed.schema_version, 1);
    assert_eq!(listed.comments.len(), 1);
    assert_eq!(listed.comments[0].author_kind, AuthorKind::Bot);
    assert_eq!(listed.comments[0].item, a);

    let every = engine
        .tasks_all(&TaskRequest {
            sources: vec![SourceName::new("plan").expect("a name")],
            filters: Filters::default(),
            project: ProjectSelector::Any,
            priorities: Vec::new(),
            commented_since: None,
            metadata: Vec::new(),
            origin: None,
            include_members: false,
            paging: Paging {
                limit: NonZeroU32::new(1).expect("a page"),
                token: None,
            },
        })
        .await
        .expect("listed");
    assert_eq!(every.items.len(), 2);
    assert!(every.next.is_none());

    let b = id("plan:b");
    let detail = engine.task_detail(&b).await.expect("read");
    let detail = engine
        .with_task_dependencies(&b, detail)
        .await
        .expect("read");
    let edges = detail.depends_on.expect("the edges, asked for");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].to.id, a);
}

/// An in-memory source whose task listing cycles between two cursors — each page a cursor other
/// than the one it was asked from, so no single step repeats, and the walk never ends.
struct Stalling(Box<dyn onetaskgraph_plugin_api::TaskSource>);

#[async_trait::async_trait]
impl onetaskgraph_plugin_api::TaskSource for Stalling {
    fn kind(&self) -> &'static str {
        self.0.kind()
    }
    fn capabilities(&self) -> onetaskgraph_plugin_api::Capabilities {
        self.0.capabilities()
    }
    async fn health(
        &self,
    ) -> Result<onetaskgraph_plugin_api::Health, onetaskgraph_plugin_api::SourceError> {
        self.0.health().await
    }
    async fn get_task(
        &self,
        id: &onetaskgraph_plugin_api::NativeId,
    ) -> Result<Option<onetaskgraph_plugin_api::Task>, onetaskgraph_plugin_api::SourceError> {
        self.0.get_task(id).await
    }
    async fn get_project(
        &self,
        id: &onetaskgraph_plugin_api::NativeId,
    ) -> Result<Option<onetaskgraph_plugin_api::Project>, onetaskgraph_plugin_api::SourceError>
    {
        self.0.get_project(id).await
    }
    async fn query_tasks(
        &self,
        query: &onetaskgraph_plugin_api::TaskQuery,
        page: &onetaskgraph_plugin_api::PageRequest,
    ) -> Result<
        onetaskgraph_plugin_api::Page<onetaskgraph_plugin_api::Task>,
        onetaskgraph_plugin_api::SourceError,
    > {
        let mut answered = self
            .0
            .query_tasks(
                query,
                &onetaskgraph_plugin_api::PageRequest {
                    cursor: None,
                    limit: page.limit,
                },
            )
            .await?;
        let next = match page.cursor.as_ref().map(|cursor| cursor.0.as_str()) {
            Some("one") => "two",
            _ => "one",
        };
        answered.next = Some(onetaskgraph_plugin_api::Cursor(next.to_owned()));
        Ok(answered)
    }
    async fn query_projects(
        &self,
        query: &onetaskgraph_plugin_api::ProjectQuery,
        page: &onetaskgraph_plugin_api::PageRequest,
    ) -> Result<
        onetaskgraph_plugin_api::Page<onetaskgraph_plugin_api::Project>,
        onetaskgraph_plugin_api::SourceError,
    > {
        self.0.query_projects(query, page).await
    }
    async fn labels(
        &self,
        page: &onetaskgraph_plugin_api::PageRequest,
    ) -> Result<
        onetaskgraph_plugin_api::Page<onetaskgraph_plugin_api::Label>,
        onetaskgraph_plugin_api::SourceError,
    > {
        self.0.labels(page).await
    }
    async fn task_dependencies(
        &self,
        id: &onetaskgraph_plugin_api::NativeId,
        direction: onetaskgraph_plugin_api::Direction,
        page: &onetaskgraph_plugin_api::PageRequest,
    ) -> Result<
        onetaskgraph_plugin_api::Page<onetaskgraph_plugin_api::DependencyEdge>,
        onetaskgraph_plugin_api::SourceError,
    > {
        self.0.task_dependencies(id, direction, page).await
    }
    async fn project_dependencies(
        &self,
        id: &onetaskgraph_plugin_api::NativeId,
        direction: onetaskgraph_plugin_api::Direction,
        page: &onetaskgraph_plugin_api::PageRequest,
    ) -> Result<
        onetaskgraph_plugin_api::Page<onetaskgraph_plugin_api::DependencyEdge>,
        onetaskgraph_plugin_api::SourceError,
    > {
        self.0.project_dependencies(id, direction, page).await
    }
}

#[tokio::test]
async fn every_page_of_a_listing_refuses_a_page_token_it_has_already_followed() {
    use onetaskgraph_core::{ConfiguredSource, ResolvedSource};
    use onetaskgraph_plugin_api::SourcePlugin as _;
    let name = SourceName::new("stalling").expect("a name");
    let inner = onetaskgraph_in_memory::Plugin
        .build(
            &name,
            &json!({"tasks": [
                {"id": "T-1", "title": "One", "labels": [],
                 "status": {"category": "todo", "name": "todo"}},
                {"id": "T-2", "title": "Two", "labels": [],
                 "status": {"category": "todo", "name": "todo"}}]}),
            &NoSecrets,
        )
        .expect("builds");
    let engine = Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name.clone(),
            Box::new(Stalling(inner)),
        ))],
        vec![name.clone()],
    );
    let refused = engine
        .tasks_all(&TaskRequest {
            sources: vec![name],
            filters: Filters::default(),
            project: ProjectSelector::Any,
            priorities: Vec::new(),
            commented_since: None,
            metadata: Vec::new(),
            origin: None,
            include_members: false,
            paging: Paging {
                limit: NonZeroU32::new(1).expect("a page"),
                token: None,
            },
        })
        .await
        .expect_err("a repeated cursor is refused");
    assert!(
        matches!(refused, EngineError::RepeatedPage { .. }),
        "{refused}"
    );
    assert_eq!(
        onetaskgraph_core::Failure::from(&refused).kind(),
        "repeated-page"
    );
}

/// A linking caller's policy for which `github.com/acme/secret` is private and every other
/// repository public, and every public write's text passes.
struct SecretRepository;

#[async_trait::async_trait]
impl onetaskgraph_core::boundary::WritePolicy for SecretRepository {
    async fn repository_visibility(
        &self,
        repository: &Repository,
    ) -> Result<
        onetaskgraph_core::boundary::RepositoryVisibility,
        onetaskgraph_core::boundary::PolicyError,
    > {
        Ok(if repository.as_str() == "github.com/acme/secret" {
            onetaskgraph_core::boundary::RepositoryVisibility::Private
        } else {
            onetaskgraph_core::boundary::RepositoryVisibility::Public
        })
    }
    async fn check_public_write(
        &self,
        _: &onetaskgraph_core::boundary::PublicWriteInput,
    ) -> Result<onetaskgraph_core::boundary::WriteVerdict, onetaskgraph_core::boundary::PolicyError>
    {
        Ok(onetaskgraph_core::boundary::WriteVerdict::Pass)
    }
}

#[tokio::test]
async fn a_repositories_update_naming_a_private_repository_is_refused_onto_a_public_source() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = folders(root.path()).with_write_policy(Arc::new(SecretRepository));
    let a = id("plan:a");
    let file = root.path().join("plan").join("tasks").join("a.md");
    let before = std::fs::read(&file).expect("the task file");
    let secret = Repository::try_from("github.com/acme/secret".to_owned()).expect("an origin");
    let refused = engine
        .update_task(
            &a,
            &TaskUpdate {
                repositories: Some(vec![secret]),
                ..TaskUpdate::default()
            },
        )
        .await
        .expect_err("a private repository is refused onto a source not declared private");
    assert!(refused.to_string().contains("private"), "{refused}");
    assert_eq!(
        std::fs::read(&file).expect("the task file"),
        before,
        "nothing was written"
    );

    // A public one is written as any update is.
    let public = Repository::try_from("github.com/acme/widgets".to_owned()).expect("an origin");
    let updated = engine
        .update_task(
            &a,
            &TaskUpdate {
                repositories: Some(vec![public.clone()]),
                ..TaskUpdate::default()
            },
        )
        .await
        .expect("a public repository lands");
    assert_eq!(updated.task.repositories, vec![public]);
}
