//! The edges of the follow-up loop's store features, as a Rust caller reaches them: every page
//! of a listing and of a task's edges — failures and unanswered sources kept once, a walk
//! starting part way, a cursor cycling — the comments listing's watermark and the comments it
//! leaves out, a held copy's refusals and its create, and a guard read back off the wire.
//!
//! The journeys of each feature's ordinary path are in `tests/held_writes.rs` and, through the
//! binary, in `crates/onetaskgraph-e2e/tests/e2e/`.

use std::num::NonZeroU32;
use std::time::Duration;

use chrono::{DateTime, Utc};
use onetaskgraph_core::{
    CommentsSinceRequest, ConfiguredSource, CopyItems, CopyRequest, CopyScope, DependencyRequest,
    Engine, EngineError, Filters, GlobalId, Paging, ProjectSelector, ResolvedSource,
    TaskPrioritySet, TaskRequest,
};
use onetaskgraph_plugin_api::{
    Capabilities, Comment, Cursor, DependencyEdge, Direction, Health, Label, NativeId, Page,
    PageRequest, Project, ProjectQuery, SecretResolver, SourceError, SourceName, SourcePlugin as _,
    Task, TaskQuery, TaskSource,
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
    SourceName::new(value).expect("a name")
}

fn id(value: &str) -> GlobalId {
    value.parse().expect("a qualified id")
}

fn page(limit: u32) -> Paging {
    Paging {
        limit: NonZeroU32::new(limit).expect("a page"),
        token: None,
    }
}

fn listing(sources: &[&str], limit: u32) -> TaskRequest {
    TaskRequest {
        sources: sources.iter().map(|source| name(source)).collect(),
        filters: Filters::default(),
        project: ProjectSelector::Any,
        priorities: Vec::new(),
        commented_since: None,
        metadata: Vec::new(),
        origin: None,
        include_members: false,
        paging: page(limit),
    }
}

fn task(id: &str) -> Value {
    json!({"id": id, "title": format!("Task {id}"), "labels": [],
           "status": {"category": "todo", "name": "todo"}})
}

fn in_memory(source: &str, config: &Value) -> Box<dyn TaskSource> {
    onetaskgraph_in_memory::Plugin
        .build(&name(source), config, &NoSecrets)
        .expect("the in-memory plugin builds")
}

/// What a scripted source does differently from the in-memory source it wraps.
#[derive(Default)]
struct Script {
    /// Every task listing refused.
    refuse_listing: bool,
    /// Every dependency read refused.
    refuse_edges: bool,
    /// Every dependency page answered with a cursor cycling between two.
    cycle_edges: bool,
    /// The consistency window it states.
    window: Duration,
    /// The time it says its backend last stated.
    served: Option<DateTime<Utc>>,
    /// Every task read with no version, as a backend that gives none answers.
    unversioned: bool,
}

/// An in-memory source behaving as its [`Script`] says.
struct Scripted {
    inner: Box<dyn TaskSource>,
    script: Script,
}

fn refused() -> SourceError {
    SourceError::Unavailable {
        message: "scripted to refuse".to_owned(),
    }
}

#[async_trait::async_trait]
impl TaskSource for Scripted {
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    fn comment_consistency_window(&self) -> Duration {
        self.script.window
    }
    fn served_at(&self) -> Option<DateTime<Utc>> {
        self.script.served
    }
    async fn health(&self) -> Result<Health, SourceError> {
        self.inner.health().await
    }
    async fn get_task(&self, id: &NativeId) -> Result<Option<Task>, SourceError> {
        let read = self.inner.get_task(id).await?;
        Ok(read.map(|task| Task {
            version: if self.script.unversioned {
                None
            } else {
                task.version
            },
            ..task
        }))
    }
    fn writes(&self) -> onetaskgraph_plugin_api::WriteSupport {
        self.inner.writes()
    }
    async fn set_task_priority(
        &self,
        id: &NativeId,
        priority: onetaskgraph_plugin_api::Priority,
    ) -> Result<Option<onetaskgraph_plugin_api::Priority>, SourceError> {
        self.inner.set_task_priority(id, priority).await
    }
    async fn get_project(&self, id: &NativeId) -> Result<Option<Project>, SourceError> {
        self.inner.get_project(id).await
    }
    async fn query_tasks(&self, q: &TaskQuery, p: &PageRequest) -> Result<Page<Task>, SourceError> {
        if self.script.refuse_listing {
            return Err(refused());
        }
        self.inner.query_tasks(q, p).await
    }
    async fn query_projects(
        &self,
        q: &ProjectQuery,
        p: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        self.inner.query_projects(q, p).await
    }
    async fn labels(&self, p: &PageRequest) -> Result<Page<Label>, SourceError> {
        self.inner.labels(p).await
    }
    async fn task_dependencies(
        &self,
        id: &NativeId,
        direction: Direction,
        p: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        if self.script.refuse_edges {
            return Err(refused());
        }
        if self.script.cycle_edges {
            let mut answered = self
                .inner
                .task_dependencies(
                    id,
                    direction,
                    &PageRequest {
                        cursor: None,
                        limit: p.limit,
                    },
                )
                .await?;
            let next = match p.cursor.as_ref().map(|cursor| cursor.0.as_str()) {
                Some("one") => "two",
                _ => "one",
            };
            answered.next = Some(Cursor(next.to_owned()));
            return Ok(answered);
        }
        self.inner.task_dependencies(id, direction, p).await
    }
    async fn project_dependencies(
        &self,
        id: &NativeId,
        direction: Direction,
        p: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        self.inner.project_dependencies(id, direction, p).await
    }
    async fn task_comments(
        &self,
        task: &NativeId,
        p: &PageRequest,
    ) -> Result<Option<Page<Comment>>, SourceError> {
        self.inner.task_comments(task, p).await
    }
}

fn engine_of(sources: Vec<(&str, Box<dyn TaskSource>)>) -> Engine {
    let names: Vec<SourceName> = sources.iter().map(|(source, _)| name(source)).collect();
    Engine::new(
        sources
            .into_iter()
            .map(|(source, built)| {
                ConfiguredSource::Ready(ResolvedSource::adopt(name(source), built))
            })
            .collect(),
        names,
    )
}

/// Five tasks, `T-1` depending on the other four.
fn work() -> Value {
    let tasks: Vec<Value> = ["T-1", "T-2", "T-3", "T-4", "T-5"].map(task).into();
    let edges: Vec<Value> = ["T-2", "T-3", "T-4", "T-5"]
        .map(|to| json!({"from": "T-1", "to": to, "kind": "blocks"}))
        .into();
    json!({
        "capabilities": {"comments": "native"},
        "tasks": tasks,
        "task_dependencies": edges,
    })
}

#[tokio::test]
async fn every_page_holds_each_item_once_from_any_start_and_each_failure_once() {
    let engine = engine_of(vec![
        ("work", in_memory("work", &work())),
        (
            "down",
            Box::new(Scripted {
                inner: in_memory("down", &json!({"tasks": [task("X-1")]})),
                script: Script {
                    refuse_listing: true,
                    ..Script::default()
                },
            }),
        ),
    ]);
    let every = engine
        .tasks_all(&listing(&["work", "down"], 2))
        .await
        .expect("listed");
    let ids: Vec<String> = every.items.iter().map(|task| task.id.to_string()).collect();
    assert_eq!(
        ids,
        ["work:T-1", "work:T-2", "work:T-3", "work:T-4", "work:T-5"]
    );
    assert!(every.next.is_none());
    assert_eq!(
        every.errors.len(),
        1,
        "the down source named once: {:?}",
        every.errors
    );
    assert_eq!(every.errors[0].source, name("down"));

    // A walk resumed part way answers the rest, each once.
    let first = engine
        .tasks(&listing(&["work"], 2))
        .await
        .expect("the first page");
    let mut rest = listing(&["work"], 2);
    rest.paging.token = first.next;
    let rest = engine.tasks_all(&rest).await.expect("listed");
    let ids: Vec<String> = rest.items.iter().map(|task| task.id.to_string()).collect();
    assert_eq!(ids, ["work:T-3", "work:T-4", "work:T-5"]);
}

#[tokio::test]
async fn every_page_of_a_tasks_edges_and_their_refusals() {
    let request = |limit: u32| DependencyRequest {
        id: id("work:T-1"),
        direction: Direction::DependsOn,
        completeness: onetaskgraph_core::Completeness::NamingUnasked,
        paging: page(limit),
    };
    let plain = engine_of(vec![("work", in_memory("work", &work()))]);
    let edges = plain
        .task_dependencies_all(&request(1))
        .await
        .expect("walked");
    let far: Vec<String> = edges
        .items
        .iter()
        .map(|edge| edge.to.id.to_string())
        .collect();
    assert_eq!(far, ["work:T-2", "work:T-3", "work:T-4", "work:T-5"]);

    // A task that is not there carries no edges, and no read of them is made.
    let missing = plain.task_detail(&id("work:T-9")).await.expect("answered");
    let missing = plain
        .with_task_dependencies(&id("work:T-9"), missing)
        .await
        .expect("answered");
    assert!(missing.depends_on.is_none());

    // A cursor cycling between two is refused, naming the token followed twice.
    let cycling = engine_of(vec![(
        "work",
        Box::new(Scripted {
            inner: in_memory("work", &work()),
            script: Script {
                cycle_edges: true,
                ..Script::default()
            },
        }),
    )]);
    let refused = cycling
        .task_dependencies_all(&request(1))
        .await
        .expect_err("a repeated page token");
    assert!(
        matches!(refused, EngineError::RepeatedPage { .. }),
        "{refused}"
    );

    // A source refusing the edge read leaves the task read standing, the failure beside it.
    let refusing = engine_of(vec![(
        "work",
        Box::new(Scripted {
            inner: in_memory("work", &work()),
            script: Script {
                refuse_edges: true,
                ..Script::default()
            },
        }),
    )]);
    let detail = refusing.task_detail(&id("work:T-1")).await.expect("read");
    let detail = refusing
        .with_task_dependencies(&id("work:T-1"), detail)
        .await
        .expect("answered");
    assert!(detail.depends_on.is_none());
    assert_eq!(detail.response.items.len(), 1);
    assert!(!detail.response.errors.is_empty(), "{:?}", detail.response);
}

fn comment(id: &str, created: Option<&str>, updated: Option<&str>) -> Value {
    json!({"task": "T-1", "comment": {"id": id, "author": null, "created_at": created,
           "updated_at": updated, "body": format!("{id}\n"), "url": null}})
}

fn since(source: &str, at: &str) -> CommentsSinceRequest {
    CommentsSinceRequest {
        source: name(source),
        since: at.parse().expect("an instant"),
        metadata: Vec::new(),
        statuses: Vec::new(),
        items: Vec::new(),
        page_size: NonZeroU32::new(1).expect("a page"),
    }
}

#[tokio::test]
async fn the_watermark_is_the_sources_own_time_set_back_and_an_undated_comment_is_left_out() {
    let served: DateTime<Utc> = "2026-10-05T12:00:00Z".parse().expect("an instant");
    let mut config = work();
    config["comments"] = json!([
        comment("undated", None, None),
        comment("created", Some("2026-10-02T00:00:00Z"), None),
        comment(
            "edited",
            Some("2026-09-01T00:00:00Z"),
            Some("2026-10-03T00:00:00Z")
        ),
        comment(
            "older",
            Some("2026-09-01T00:00:00Z"),
            Some("2026-09-02T00:00:00Z")
        ),
        comment("updated-only", None, Some("2026-10-04T00:00:00Z")),
    ]);
    let engine = engine_of(vec![(
        "work",
        Box::new(Scripted {
            inner: in_memory("work", &config),
            script: Script {
                window: Duration::from_secs(90),
                served: Some(served),
                ..Script::default()
            },
        }),
    )]);
    let listed = engine
        .comments_since(&since("work", "2026-10-01T00:00:00Z"))
        .await
        .expect("listed");
    let ids: Vec<&str> = listed
        .comments
        .iter()
        .map(|comment| comment.id.as_str())
        .collect();
    assert_eq!(
        ids,
        ["created", "edited", "updated-only"],
        "the undated and the older left out"
    );
    assert_eq!(
        listed.comments[0].updated_at, listed.comments[0].created_at,
        "a comment never edited last changed when it was made"
    );
    assert_eq!(
        listed.comments[2].created_at, listed.comments[2].updated_at,
        "a comment whose source gave only when it last changed is dated by that"
    );
    assert_eq!(
        listed.watermark,
        served - chrono::TimeDelta::seconds(90),
        "the source's own time, set back by its window"
    );

    // A backend clock ahead of this host's is never trusted past the read's own start.
    let ahead = engine_of(vec![(
        "work",
        Box::new(Scripted {
            inner: in_memory("work", &config),
            script: Script {
                window: Duration::from_secs(90),
                served: Some("2999-01-01T00:00:00Z".parse().expect("an instant")),
                ..Script::default()
            },
        }),
    )]);
    let before = Utc::now();
    let clamped = ahead
        .comments_since(&since("work", "2026-10-01T00:00:00Z"))
        .await
        .expect("listed");
    let after = Utc::now();
    let window = chrono::TimeDelta::seconds(90);
    assert!(
        clamped.watermark >= before - window && clamped.watermark <= after - window,
        "the read's start, set back by the window: {}",
        clamped.watermark
    );

    // A source that could not be listed is a refusal, not an empty answer.
    let down = engine_of(vec![(
        "work",
        Box::new(Scripted {
            inner: in_memory("work", &config),
            script: Script {
                refuse_listing: true,
                ..Script::default()
            },
        }),
    )]);
    let refused = down
        .comments_since(&since("work", "2026-10-01T00:00:00Z"))
        .await
        .expect_err("a source that could not be listed");
    assert!(
        matches!(refused, EngineError::SourceRefused { .. }),
        "{refused}"
    );
}

fn copy_request(items: CopyItems, scope: CopyScope) -> CopyRequest {
    CopyRequest {
        items,
        scope,
        destination: name("into"),
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

#[tokio::test]
async fn a_held_copy_refuses_a_query_and_a_project_and_holds_nothing_it_creates() {
    let engine = engine_of(vec![
        ("work", in_memory("work", &work())),
        ("into", in_memory("into", &json!({}))),
    ]);
    let query = copy_request(CopyItems::matching(listing(&["work"], 5)), CopyScope::Tasks);
    assert!(matches!(
        engine.copy_held(&query, "mem1|any").await,
        Err(EngineError::IfVersionOneTask)
    ));
    let project = copy_request(
        CopyItems::new(vec![id("work:T-1")]).expect("items"),
        CopyScope::Projects {
            tasks: false,
            documents: false,
        },
    );
    assert!(matches!(
        engine.copy_held(&project, "mem1|any").await,
        Err(EngineError::IfVersionOneTask)
    ));
    // No counterpart: the copy creates one, at no version to be held to.
    let created = engine
        .copy_held(
            &copy_request(
                CopyItems::new(vec![id("work:T-1")]).expect("items"),
                CopyScope::Tasks,
            ),
            "mem1|any",
        )
        .await
        .expect("a create is not held");
    assert_eq!(created.items.len(), 1);
}

#[test]
fn a_guard_read_back_off_the_wire_is_held_to_its_condition() {
    let answer = |outcome: &str, met: bool| {
        json!({"id": "work:T-1", "priority": "high", "outcome": outcome,
               "priority_condition": {"held": "high", "met": met}})
    };
    let read: TaskPrioritySet =
        serde_json::from_value(answer("held", false)).expect("a consistent guard");
    assert_eq!(
        read.guard.map(|guard| guard.outcome()),
        Some(onetaskgraph_core::GuardOutcome::Held)
    );
    // An outcome written contradicting its condition reads back as its condition says, and
    // never as no guard at all.
    let contradicting: TaskPrioritySet =
        serde_json::from_value(answer("written", false)).expect("read");
    assert_eq!(
        contradicting.guard.map(|guard| guard.outcome()),
        Some(onetaskgraph_core::GuardOutcome::Held)
    );
    let unguarded: TaskPrioritySet =
        serde_json::from_value(json!({"id": "work:T-1", "priority": "high"})).expect("read");
    assert!(unguarded.guard.is_none());
}

#[tokio::test]
async fn a_held_write_of_a_task_its_backend_gives_no_version_is_refused_writing_nothing() {
    let mut config = work();
    config["capabilities"]["priority"] = json!("native");
    let engine = engine_of(vec![(
        "work",
        Box::new(Scripted {
            inner: in_memory("work", &config),
            script: Script {
                unversioned: true,
                ..Script::default()
            },
        }),
    )]);
    let task = id("work:T-1");
    let before = engine.task(&task).await.expect("read").items[0]
        .item
        .clone();
    assert_eq!(before.version, None, "the backend gives no version");
    let refused = engine
        .held_write(
            &task,
            "mem1|any",
            engine.set_task_priority(&task, onetaskgraph_plugin_api::Priority::Urgent),
        )
        .await
        .expect_err("no version is not the version held");
    match &refused {
        EngineError::VersionMoved { found, held, .. } => {
            assert_eq!(found, "", "the refusal says the task is at no version");
            assert_eq!(held.version.as_deref(), Some(""));
        }
        other => panic!("{other}"),
    }
    let after = engine.task(&task).await.expect("read").items[0]
        .item
        .clone();
    assert_eq!(after, before, "nothing was written");
}

/// A public `site` task `T-1`, and a private `vault` recording `site:T-1` as depending on its
/// `V-1` and `V-2` — a vault that cannot be listed.
fn private_record() -> Engine {
    let recorded = |id: &str| {
        json!({"id": id, "title": id, "labels": [],
               "status": {"category": "todo", "name": "todo"},
               "metadata": {"onetaskgraph.depended_on_by": {
                   "site:T-1": {"id": "site:T-1", "kind": "task"}}}})
    };
    let site = in_memory(
        "site",
        &json!({"tasks": [{"id": "T-1", "title": "T-1", "labels": [],
                           "status": {"category": "todo", "name": "todo"},
                           "metadata": {"onetaskgraph.copied_destination_version": "sha256:00"}}]}),
    );
    let vault = Scripted {
        inner: in_memory(
            "vault",
            &json!({"tasks": [recorded("V-1"), recorded("V-2")]}),
        ),
        script: Script {
            refuse_listing: true,
            ..Script::default()
        },
    };
    engine_of(vec![("site", site), ("vault", Box::new(vault))]).with_routes(
        onetaskgraph_core::config::Routes::default().with_visibility(
            std::collections::BTreeMap::from([
                (
                    name("site"),
                    onetaskgraph_core::config::SourceVisibility::Public,
                ),
                (
                    name("vault"),
                    onetaskgraph_core::config::SourceVisibility::Private,
                ),
            ]),
        ),
    )
}

#[tokio::test]
async fn a_private_source_that_could_not_be_asked_is_named_once_across_every_page_of_edges() {
    let engine = private_record();
    let walked = engine
        .task_dependencies_all(&DependencyRequest {
            id: id("site:T-1"),
            direction: Direction::DependsOn,
            completeness: onetaskgraph_core::Completeness::NamingUnasked,
            paging: page(1),
        })
        .await
        .expect("walked");
    let unanswered: Vec<String> = walked
        .unanswered
        .iter()
        .map(|unasked| format!("{unasked:?}"))
        .collect();
    assert_eq!(unanswered.len(), 1, "the vault named once: {unanswered:?}");
    assert!(unanswered[0].contains("vault"), "{unanswered:?}");
    assert!(walked.errors.is_empty(), "{:?}", walked.errors);

    // Read beside the task, the vault is named once there too, and the task's own edges stand.
    let detail = engine.task_detail(&id("site:T-1")).await.expect("read");
    let detail = engine
        .with_task_dependencies(&id("site:T-1"), detail)
        .await
        .expect("answered");
    assert_eq!(detail.response.unanswered.len(), 1, "{:?}", detail.response);
    assert!(
        detail.depends_on.is_some(),
        "the task's own source answered its edges"
    );
}
