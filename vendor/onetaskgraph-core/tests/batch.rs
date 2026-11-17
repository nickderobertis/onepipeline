//! The batched store calls as a **Rust caller** reaches them, through the engine's public API
//! over real folders of Markdown and a real in-memory source: many searches in one call, a copy
//! holding each counterpart to its own version, many comments and many held updates in one
//! call, and one update written only while its priority guard holds — each answered per item,
//! one item's failure never writing or refusing another's.
//!
//! The journeys that drive the same through the binary are in
//! `crates/onetaskgraph-e2e/tests/e2e/batch.rs`.

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU32;
use std::path::Path;

use onetaskgraph_core::{
    BATCH_SCHEMA_VERSION, BatchOutcome, CommentBatch, CommentBatchItem, Config, CopyAction,
    CopyItems, CopyRequest, CopyScope, Engine, EngineError, FindIn, FindQuery, FindRequest,
    GlobalId, GuardOutcome, UpdateBatch, UpdateBatchItem,
};
use onetaskgraph_plugin_api::{
    MetadataKey, Priority, SecretResolver, SourceName, StatusCategory, Task, TaskUpdate,
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

fn name(value: &str) -> SourceName {
    SourceName::new(value).expect("a source name")
}

/// One task of the fixture: its id, title, status, priority, `myapp.colour` and origin.
type Fixture = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
);

/// Every task of the fixture.
const TASKS: [Fixture; 6] = [
    ("a", "Alpha batch", "todo", "high", "red", None),
    (
        "b",
        "Bravo batch",
        "todo",
        "high",
        "red",
        Some("elsewhere:B-1"),
    ),
    ("c", "Charlie batch", "in-progress", "low", "blue", None),
    ("d", "Delta", "todo", "none", "red", Some("elsewhere:B-1")),
    ("e", "Echo batch", "done", "none", "green", None),
    ("f", "Foxtrot", "todo", "none", "red", None),
];

/// A `plan` folder holding [`TASKS`] and an empty `snap` folder copies land in.
fn folders(root: &Path) -> Engine {
    let tasks = root.join("plan").join("tasks");
    std::fs::create_dir_all(&tasks).expect("the task folder");
    for (native, title, status, priority, colour, origin) in TASKS {
        let origin = origin.map_or_else(String::new, |origin| {
            format!("  onetaskgraph.origin: {origin}\n")
        });
        std::fs::write(
            tasks.join(format!("{native}.md")),
            format!(
                "---\ntitle: {title}\nstatus: {status}\npriority: {priority}\nmetadata:\n  \
                 myapp.colour: {colour}\n{origin}---\nThe {native} body.\n"
            ),
        )
        .expect("a task");
    }
    std::fs::create_dir_all(root.join("snap")).expect("a folder");
    engine_over(json!({
        "sources": {
            "plan": {"plugin": "local-md", "config": {"root": root.join("plan")}},
            "snap": {"plugin": "local-md", "config": {"root": root.join("snap")}},
        },
    }))
}

/// An in-memory source `memory` holding [`TASKS`], with comments and priorities.
fn memory() -> Engine {
    let tasks: Vec<Value> = TASKS
        .iter()
        .map(|(native, title, status, priority, colour, origin)| {
            let mut metadata = json!({"myapp.colour": colour});
            if let Some(origin) = origin {
                metadata["onetaskgraph.origin"] = json!(origin);
            }
            json!({"id": native, "title": title, "labels": [], "priority": priority,
                   "content": format!("The {native} body."),
                   "status": {"category": status, "name": status}, "metadata": metadata})
        })
        .collect();
    engine_over(json!({"sources": {"memory": {
        "plugin": "in-memory",
        "config": {"capabilities": {"priority": "native", "comments": "native"},
                   "tasks": tasks},
    }}}))
}

async fn task(engine: &Engine, task: &str) -> Task {
    engine
        .task(&id(task))
        .await
        .expect("read")
        .items
        .into_iter()
        .next()
        .expect("the task is there")
        .item
}

async fn version(engine: &Engine, of: &str) -> String {
    task(engine, of)
        .await
        .version
        .expect("a task carries its version")
}

fn query(key: &str) -> FindQuery {
    FindQuery {
        key: key.to_owned(),
        text: None,
        fields: FindIn::Both,
        metadata: BTreeMap::new(),
        origin: None,
        status: Vec::new(),
    }
}

/// The five searches every source is asked: by title text, by a metadata value, by origin, by
/// text and status together, and one nothing matches.
fn five_searches() -> FindRequest {
    FindRequest {
        schema_version: BATCH_SCHEMA_VERSION,
        queries: vec![
            FindQuery {
                text: Some("batch".to_owned()),
                fields: FindIn::Title,
                ..query("text")
            },
            FindQuery {
                metadata: BTreeMap::from([("myapp.colour".to_owned(), "red".to_owned())]),
                ..query("colour")
            },
            FindQuery {
                origin: Some(id("elsewhere:B-1")),
                ..query("origin")
            },
            FindQuery {
                text: Some("batch".to_owned()),
                status: vec![StatusCategory::Todo, StatusCategory::InProgress],
                ..query("open")
            },
            FindQuery {
                text: Some("no task holds this".to_owned()),
                ..query("nothing")
            },
        ],
    }
}

async fn assert_five_searches(engine: &Engine, source: &str) {
    let page = NonZeroU32::new(1).expect("a page");
    let found = engine
        .find_tasks(&name(source), &five_searches(), page)
        .await
        .expect("found");
    let answered: Vec<(String, Vec<String>, bool)> = found
        .answers
        .iter()
        .map(|answer| {
            let mut ids: Vec<String> = answer
                .items
                .iter()
                .map(|item| item.id.native.to_string())
                .collect();
            ids.sort();
            (
                answer.key.clone(),
                ids,
                answer.complete && answer.reason.is_none(),
            )
        })
        .collect();
    let owned = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        answered,
        vec![
            ("text".to_owned(), owned(&["a", "b", "c", "e"]), true),
            ("colour".to_owned(), owned(&["a", "b", "d", "f"]), true),
            ("origin".to_owned(), owned(&["b", "d"]), true),
            ("open".to_owned(), owned(&["a", "b", "c"]), true),
            ("nothing".to_owned(), Vec::new(), true),
        ],
        "{source}: each key holds exactly its matches, across one-task pages",
    );
    let unnarrowed = FindRequest {
        schema_version: BATCH_SCHEMA_VERSION,
        queries: vec![query("everything")],
    };
    match engine.find_tasks(&name(source), &unnarrowed, page).await {
        Err(EngineError::UnnarrowedQuery { key }) => assert_eq!(key, "everything"),
        other => panic!("{source}: an unnarrowed query is refused naming its key, not {other:?}"),
    }
}

#[tokio::test]
async fn many_searches_answer_each_key_with_every_match_across_pages() {
    let root = tempfile::tempdir().expect("a folder");
    assert_five_searches(&folders(root.path()), "plan").await;
    assert_five_searches(&memory(), "memory").await;
}

fn copy(items: &[&str], versions: HashMap<GlobalId, String>) -> CopyRequest {
    CopyRequest {
        items: CopyItems::new(items.iter().map(|item| id(item)).collect()).expect("items"),
        scope: CopyScope::Tasks,
        destination: name("snap"),
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
        conditions: onetaskgraph_core::CopyConditions {
            versions,
            ..Default::default()
        },
    }
}

fn retitle(title: &str) -> TaskUpdate {
    TaskUpdate {
        title: Some(title.to_owned()),
        ..TaskUpdate::default()
    }
}

#[tokio::test]
async fn a_copy_holds_each_counterpart_to_its_own_version() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = folders(root.path());
    let items = ["plan:a", "plan:b", "plan:c", "plan:d"];
    let first = engine
        .copy(&copy(&items, HashMap::new()))
        .await
        .expect("copied");
    let mut counterparts = HashMap::new();
    for outcome in &first.items {
        let CopyAction::Created {
            destination: Some(destination),
            ..
        } = &outcome.action
        else {
            panic!("the first copy creates every counterpart, not {outcome:?}");
        };
        counterparts.insert(outcome.source.to_string(), destination.to_string());
    }
    let mut read = HashMap::new();
    for (source, counterpart) in &counterparts {
        read.insert(source.clone(), version(&engine, counterpart).await);
    }
    // Another writer moves `b`'s counterpart after it was read.
    engine
        .update_task(
            &id(&counterparts["plan:b"]),
            &retitle("Bravo, edited at snap"),
        )
        .await
        .expect("the other writer");
    let moved_to = version(&engine, &counterparts["plan:b"]).await;
    for item in items {
        let title = format!("{}, revised", task(&engine, item).await.title);
        engine
            .update_task(&id(item), &retitle(&title))
            .await
            .expect("revised");
    }

    let held = HashMap::from([
        (id("plan:a"), read["plan:a"].clone()),
        (id("plan:b"), read["plan:b"].clone()),
    ]);
    let second = engine
        .copy(&copy(&items, held))
        .await
        .expect("copied in part");
    for outcome in &second.items {
        let source = outcome.source.to_string();
        let counterpart = task(&engine, &counterparts[&source]).await;
        if source == "plan:b" {
            let CopyAction::Failed { error } = &outcome.action else {
                panic!("the moved counterpart is refused, not {outcome:?}");
            };
            assert_eq!(error.kind(), "version-moved");
            let held = outcome
                .held
                .as_ref()
                .expect("the refusal carries what it found");
            assert_eq!(held.version.as_deref(), Some(moved_to.as_str()));
            assert_eq!(
                counterpart.title, "Bravo, edited at snap",
                "left as the writer left it"
            );
        } else {
            assert!(
                matches!(outcome.action, CopyAction::Updated { .. }),
                "{source} is copied, not {outcome:?}"
            );
            assert!(
                counterpart.title.ends_with(", revised"),
                "{source}: {}",
                counterpart.title
            );
        }
    }
}

#[tokio::test]
async fn many_comments_are_each_answered_on_their_own() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = folders(root.path());
    let stale = version(&engine, "plan:b").await;
    engine
        .update_task(&id("plan:b"), &retitle("Bravo, moved"))
        .await
        .expect("moved");
    let comment = |item: &str, body: &str, if_version: Option<String>| CommentBatchItem {
        item: item.to_owned(),
        body: body.to_owned(),
        if_version,
    };
    let batch = CommentBatch {
        schema_version: BATCH_SCHEMA_VERSION,
        comments: vec![
            comment("plan:a", "first", None),
            comment("plan:b", "held", Some(stale)),
            comment("plan:c", "third", Some(version(&engine, "plan:c").await)),
            comment("plan:nowhere", "lost", None),
            comment("not an id", "malformed", None),
        ],
    };
    let added = engine
        .add_comments(&name("plan"), &batch)
        .await
        .expect("answered");
    let outcomes: Vec<BatchOutcome> = added.results.iter().map(|result| result.outcome).collect();
    assert_eq!(
        outcomes,
        [
            BatchOutcome::Written,
            BatchOutcome::VersionMoved,
            BatchOutcome::Written,
            BatchOutcome::Error,
            BatchOutcome::Error,
        ]
    );
    assert_eq!(
        added.results[1].current_version.as_deref(),
        Some(version(&engine, "plan:b").await.as_str())
    );
    for (at, named) in [(3, "plan:nowhere"), (4, "not an id")] {
        let reason = added.results[at].reason.as_deref().unwrap_or_default();
        assert!(reason.contains(named), "the reason names {named}: {reason}");
    }
    for (at, item, body) in [(0, "plan:a", "first"), (2, "plan:c", "third")] {
        let written = added.results[at]
            .comment
            .as_ref()
            .expect("the comment written");
        let listed = engine.comments(&id(item)).await.expect("listed").comments;
        assert_eq!(listed.len(), 1, "{item} holds the one comment");
        assert_eq!(listed[0].id, written.id);
        assert_eq!(listed[0].body.as_str(), body);
    }
    assert!(
        engine
            .comments(&id("plan:b"))
            .await
            .expect("listed")
            .comments
            .is_empty()
    );
}

fn every_field(item: &str, if_version: Option<String>) -> UpdateBatchItem {
    UpdateBatchItem {
        item: item.to_owned(),
        if_version,
        body: Some("Rewritten.\n".to_owned()),
        status: Some(StatusCategory::Done),
        priority: Some(Priority::Low),
        when_priority: vec![Priority::High, Priority::None],
        metadata: BTreeMap::from([("myapp.colour".to_owned(), json!("black"))]),
        ..UpdateBatchItem::default()
    }
}

/// A task's content, `myapp.colour`, status and priority.
async fn fields(
    engine: &Engine,
    of: &str,
) -> (Option<String>, Option<Value>, StatusCategory, Priority) {
    let task = task(engine, of).await;
    let colour = task.metadata.get("myapp.colour").cloned();
    (task.content, colour, task.status.category, task.priority)
}

async fn assert_many_updates(engine: &Engine, source: &str) {
    let named = |native: &str| format!("{source}:{native}");
    let stale = version(engine, &named("b")).await;
    engine
        .update_task(&id(&named("b")), &retitle("Bravo, moved"))
        .await
        .expect("moved");
    let moved_to = version(engine, &named("b")).await;
    let b_before = fields(engine, &named("b")).await;
    let c_before = fields(engine, &named("c")).await;
    let batch = UpdateBatch {
        schema_version: BATCH_SCHEMA_VERSION,
        updates: vec![
            every_field(&named("a"), Some(version(engine, &named("a")).await)),
            every_field(&named("b"), Some(stale)),
            every_field(&named("c"), None),
            every_field(&named("nowhere"), None),
        ],
    };
    let updated = engine
        .update_tasks(&name(source), &batch)
        .await
        .expect("answered");
    let outcomes: Vec<BatchOutcome> = updated
        .results
        .iter()
        .map(|result| result.outcome)
        .collect();
    assert_eq!(
        outcomes,
        [
            BatchOutcome::Written,
            BatchOutcome::VersionMoved,
            BatchOutcome::Held,
            BatchOutcome::Error,
        ],
        "{source}"
    );
    assert_eq!(
        updated.results[0].version,
        Some(version(engine, &named("a")).await)
    );
    assert_eq!(
        updated.results[1].current_version.as_deref(),
        Some(moved_to.as_str())
    );
    let reason = updated.results[3].reason.as_deref().unwrap_or_default();
    assert!(reason.contains(&named("nowhere")), "{source}: {reason}");

    let (content, colour, status, priority) = fields(engine, &named("a")).await;
    assert_eq!(
        content.as_deref().map(str::trim_end),
        Some("Rewritten."),
        "{source}"
    );
    assert_eq!(
        (colour, status, priority),
        (Some(json!("black")), StatusCategory::Done, Priority::Low),
        "{source}: every field of the written update landed"
    );
    assert_eq!(
        fields(engine, &named("b")).await,
        b_before,
        "{source}: nothing of b"
    );
    assert_eq!(
        fields(engine, &named("c")).await,
        c_before,
        "{source}: nothing of c"
    );
}

#[tokio::test]
async fn many_updates_write_every_field_or_none() {
    let root = tempfile::tempdir().expect("a folder");
    assert_many_updates(&folders(root.path()), "plan").await;
    assert_many_updates(&memory(), "memory").await;
}

#[tokio::test]
async fn a_guarded_update_writes_every_field_only_while_its_guard_holds() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = folders(root.path());
    let update = TaskUpdate {
        content: Some("Guarded.\n".to_owned()),
        priority: Some(Priority::Urgent),
        metadata_set: BTreeMap::from([(
            MetadataKey::new("myapp.colour").expect("a key"),
            json!("black"),
        )]),
        ..TaskUpdate::default()
    };

    let c_before = fields(&engine, "plan:c").await;
    let held = engine
        .update_task_when(&id("plan:c"), &update, &[Priority::High], None)
        .await
        .expect("answered");
    let guard = held.guard.expect("the guard reports");
    assert_eq!(guard.outcome(), GuardOutcome::Held);
    assert_eq!(guard.condition().held, Priority::Low);
    assert!(held.written.is_empty());
    assert_eq!(
        fields(&engine, "plan:c").await,
        c_before,
        "nothing of a held update"
    );

    let written = engine
        .update_task_when(&id("plan:a"), &update, &[Priority::High], None)
        .await
        .expect("written");
    assert_eq!(
        written.guard.expect("the guard reports").outcome(),
        GuardOutcome::Written
    );
    let (content, colour, _, priority) = fields(&engine, "plan:a").await;
    assert_eq!(content.as_deref().map(str::trim_end), Some("Guarded."));
    assert_eq!((colour, priority), (Some(json!("black")), Priority::Urgent));

    // Held to a version the task has moved past, the guard is never reached.
    let stale = version(&engine, "plan:b").await;
    engine
        .update_task(&id("plan:b"), &retitle("Bravo, moved"))
        .await
        .expect("moved");
    let refused = engine
        .update_task_when(&id("plan:b"), &update, &[Priority::High], Some(&stale))
        .await;
    match refused {
        Err(EngineError::VersionMoved { found, .. }) => {
            assert_eq!(found, version(&engine, "plan:b").await);
        }
        other => panic!("a moved task is refused as moved, not {other:?}"),
    }
    assert_eq!(task(&engine, "plan:b").await.priority, Priority::High);
}

/// How a [`Miscounted`] source gets the count of its batched answers wrong.
#[derive(Clone, Copy, Debug)]
enum Miscount {
    /// The last item's answer left out.
    Short,
    /// The first item's answer given twice.
    Long,
}

/// A real folder of Markdown whose batched writes are made as the folder makes them and then
/// answered with one answer too few or too many — a plugin defect only an in-process source can
/// have, since a hosted one is always answered by the trait's defaults.
struct Miscounted {
    inner: Box<dyn onetaskgraph_plugin_api::TaskSource>,
    miscount: Miscount,
}

impl Miscounted {
    fn miscounted<T: Clone>(&self, mut answers: Vec<T>) -> Vec<T> {
        match self.miscount {
            Miscount::Short => {
                answers.pop();
            }
            Miscount::Long => answers.push(answers[0].clone()),
        }
        answers
    }
}

#[async_trait::async_trait]
impl onetaskgraph_plugin_api::TaskSource for Miscounted {
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }
    fn capabilities(&self) -> onetaskgraph_plugin_api::Capabilities {
        self.inner.capabilities()
    }
    fn writes(&self) -> onetaskgraph_plugin_api::WriteSupport {
        self.inner.writes()
    }
    async fn health(
        &self,
    ) -> Result<onetaskgraph_plugin_api::Health, onetaskgraph_plugin_api::SourceError> {
        self.inner.health().await
    }
    async fn get_task(
        &self,
        id: &onetaskgraph_plugin_api::NativeId,
    ) -> Result<Option<Task>, onetaskgraph_plugin_api::SourceError> {
        self.inner.get_task(id).await
    }
    async fn get_project(
        &self,
        id: &onetaskgraph_plugin_api::NativeId,
    ) -> Result<Option<onetaskgraph_plugin_api::Project>, onetaskgraph_plugin_api::SourceError>
    {
        self.inner.get_project(id).await
    }
    async fn query_tasks(
        &self,
        query: &onetaskgraph_plugin_api::TaskQuery,
        page: &onetaskgraph_plugin_api::PageRequest,
    ) -> Result<onetaskgraph_plugin_api::Page<Task>, onetaskgraph_plugin_api::SourceError> {
        self.inner.query_tasks(query, page).await
    }
    async fn query_projects(
        &self,
        query: &onetaskgraph_plugin_api::ProjectQuery,
        page: &onetaskgraph_plugin_api::PageRequest,
    ) -> Result<
        onetaskgraph_plugin_api::Page<onetaskgraph_plugin_api::Project>,
        onetaskgraph_plugin_api::SourceError,
    > {
        self.inner.query_projects(query, page).await
    }
    async fn labels(
        &self,
        page: &onetaskgraph_plugin_api::PageRequest,
    ) -> Result<
        onetaskgraph_plugin_api::Page<onetaskgraph_plugin_api::Label>,
        onetaskgraph_plugin_api::SourceError,
    > {
        self.inner.labels(page).await
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
        self.inner.task_dependencies(id, direction, page).await
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
        self.inner.project_dependencies(id, direction, page).await
    }
    async fn task_comments(
        &self,
        task: &onetaskgraph_plugin_api::NativeId,
        page: &onetaskgraph_plugin_api::PageRequest,
    ) -> Result<
        Option<onetaskgraph_plugin_api::Page<onetaskgraph_plugin_api::Comment>>,
        onetaskgraph_plugin_api::SourceError,
    > {
        self.inner.task_comments(task, page).await
    }
    async fn query_tasks_batch(
        &self,
        asks: &[(
            onetaskgraph_plugin_api::TaskQuery,
            onetaskgraph_plugin_api::PageRequest,
        )],
    ) -> Vec<Result<onetaskgraph_plugin_api::Page<Task>, onetaskgraph_plugin_api::SourceError>>
    {
        self.miscounted(self.inner.query_tasks_batch(asks).await)
    }
    async fn get_task_details(
        &self,
        ids: &[onetaskgraph_plugin_api::NativeId],
        comments: Option<&onetaskgraph_plugin_api::PageRequest>,
    ) -> Vec<
        Result<
            Option<onetaskgraph_plugin_api::TaskDetailRead>,
            onetaskgraph_plugin_api::SourceError,
        >,
    > {
        self.miscounted(self.inner.get_task_details(ids, comments).await)
    }
    async fn add_comments(
        &self,
        comments: &[onetaskgraph_plugin_api::BatchComment],
    ) -> Vec<
        Result<
            onetaskgraph_plugin_api::BatchAnswer<onetaskgraph_plugin_api::Comment>,
            onetaskgraph_plugin_api::SourceError,
        >,
    > {
        self.miscounted(self.inner.add_comments(comments).await)
    }
    async fn update_tasks(
        &self,
        updates: &[onetaskgraph_plugin_api::BatchUpdate],
    ) -> Vec<
        Result<
            onetaskgraph_plugin_api::BatchAnswer<onetaskgraph_plugin_api::TaskUpdateOutcome>,
            onetaskgraph_plugin_api::SourceError,
        >,
    > {
        self.miscounted(self.inner.update_tasks(updates).await)
    }
    async fn metering(
        &self,
    ) -> Result<Option<onetaskgraph_plugin_api::Metering>, onetaskgraph_plugin_api::SourceError>
    {
        self.inner.metering().await
    }
}

/// The `plan` folder of [`folders`] behind a [`Miscounted`] source.
fn miscounted(root: &Path, miscount: Miscount) -> Engine {
    use onetaskgraph_plugin_api::SourcePlugin as _;
    drop(folders(root));
    let plan = name("plan");
    let inner = onetaskgraph_local_md::Plugin
        .build(&plan, &json!({"root": root.join("plan")}), &NoSecrets)
        .expect("the local-md plugin builds");
    let source = onetaskgraph_core::ResolvedSource::adopt(
        plan.clone(),
        Box::new(Miscounted { inner, miscount }),
    );
    Engine::new(
        vec![onetaskgraph_core::ConfiguredSource::Ready(source)],
        vec![plan],
    )
}

#[tokio::test]
async fn a_source_answering_too_few_batched_answers_leaves_the_last_item_an_error_naming_both_counts()
 {
    let root = tempfile::tempdir().expect("a folder");
    let engine = miscounted(root.path(), Miscount::Short);
    let comment = |item: &str| CommentBatchItem {
        item: item.to_owned(),
        body: "Counted.".to_owned(),
        if_version: None,
    };
    let added = engine
        .add_comments(
            &name("plan"),
            &CommentBatch {
                schema_version: BATCH_SCHEMA_VERSION,
                comments: vec![comment("plan:a"), comment("plan:c"), comment("plan:e")],
            },
        )
        .await
        .expect("answered");
    let outcomes: Vec<BatchOutcome> = added.results.iter().map(|result| result.outcome).collect();
    assert_eq!(
        outcomes,
        [
            BatchOutcome::Written,
            BatchOutcome::Written,
            BatchOutcome::Error
        ]
    );
    let reason = added.results[2].reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains("answered 2 answers to the 3 items") && reason.contains("unanswered"),
        "{reason}"
    );

    let updated = engine
        .update_tasks(
            &name("plan"),
            &UpdateBatch {
                schema_version: BATCH_SCHEMA_VERSION,
                updates: vec![
                    UpdateBatchItem {
                        item: "plan:a".to_owned(),
                        title: Some("Alpha, counted".to_owned()),
                        ..UpdateBatchItem::default()
                    },
                    UpdateBatchItem {
                        item: "plan:c".to_owned(),
                        title: Some("Charlie, counted".to_owned()),
                        ..UpdateBatchItem::default()
                    },
                ],
            },
        )
        .await
        .expect("answered");
    let outcomes: Vec<BatchOutcome> = updated
        .results
        .iter()
        .map(|result| result.outcome)
        .collect();
    assert_eq!(outcomes, [BatchOutcome::Written, BatchOutcome::Error]);
    assert_eq!(task(&engine, "plan:a").await.title, "Alpha, counted");
}

#[tokio::test]
async fn a_source_answering_too_many_batched_answers_leaves_every_item_an_error_to_read_again() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = miscounted(root.path(), Miscount::Long);
    let updated = engine
        .update_tasks(
            &name("plan"),
            &UpdateBatch {
                schema_version: BATCH_SCHEMA_VERSION,
                updates: vec![
                    UpdateBatchItem {
                        item: "plan:a".to_owned(),
                        title: Some("Alpha, overcounted".to_owned()),
                        ..UpdateBatchItem::default()
                    },
                    UpdateBatchItem {
                        item: "plan:c".to_owned(),
                        title: Some("Charlie, overcounted".to_owned()),
                        ..UpdateBatchItem::default()
                    },
                ],
            },
        )
        .await
        .expect("answered");
    for result in &updated.results {
        assert_eq!(result.outcome, BatchOutcome::Error, "{result:?}");
        let reason = result.reason.as_deref().unwrap_or_default();
        assert!(
            reason.contains("answered 3 answers to the 2 items")
                && reason.contains("read it before sending it again"),
            "{reason}"
        );
    }
    // Each update was sent, and the folder made it: the error says so rather than claiming
    // nothing landed.
    assert_eq!(task(&engine, "plan:a").await.title, "Alpha, overcounted");
}

/// Two searches of `plan`, each matching titles alone: `batch` (four tasks) and `Delta` (one).
fn two_searches() -> FindRequest {
    let titled = |key: &str, text: &str| FindQuery {
        text: Some(text.to_owned()),
        fields: FindIn::Title,
        ..query(key)
    };
    FindRequest {
        schema_version: BATCH_SCHEMA_VERSION,
        queries: vec![titled("batch", "batch"), titled("delta", "Delta")],
    }
}

#[tokio::test]
async fn a_source_answering_too_few_searches_leaves_the_last_one_incomplete_and_the_first_whole() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = miscounted(root.path(), Miscount::Short);
    let found = engine
        .find_tasks(
            &name("plan"),
            &two_searches(),
            NonZeroU32::new(10).expect("a page"),
        )
        .await
        .expect("answered");
    assert!(found.answers[0].complete, "{:?}", found.answers[0]);
    assert_eq!(found.answers[0].items.len(), 4);
    assert!(!found.answers[1].complete, "{:?}", found.answers[1]);
    let reason = found.answers[1].reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains("answered 1 answers to the 2 items"),
        "{reason}"
    );
}

#[tokio::test]
async fn a_source_answering_too_many_searches_leaves_every_one_incomplete() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = miscounted(root.path(), Miscount::Long);
    let found = engine
        .find_tasks(
            &name("plan"),
            &two_searches(),
            NonZeroU32::new(10).expect("a page"),
        )
        .await
        .expect("answered");
    for answer in &found.answers {
        assert!(!answer.complete, "{answer:?}");
        assert!(answer.items.is_empty(), "{answer:?}");
        let reason = answer.reason.as_deref().unwrap_or_default();
        assert!(
            reason.contains("answered 3 answers to the 2 items"),
            "{reason}"
        );
    }
}

#[tokio::test]
async fn comments_since_over_a_source_miscounting_its_details_reads_each_task_on_its_own() {
    for miscount in [Miscount::Short, Miscount::Long] {
        let root = tempfile::tempdir().expect("a folder");
        let engine = miscounted(root.path(), miscount);
        // Another engine over the same folder writes the comments, so only the read miscounts.
        let writer = engine_over(json!({"sources": {"plan": {
            "plugin": "local-md", "config": {"root": root.path().join("plan")}}}}));
        for (task, body) in [
            ("plan:a", "On a."),
            ("plan:c", "On c."),
            ("plan:e", "On e."),
        ] {
            writer
                .add_comment(
                    &id(task),
                    &onetaskgraph_plugin_api::NewComment {
                        body: onetaskgraph_plugin_api::CommentBody::new(body.to_owned())
                            .expect("a body"),
                        author: None,
                    },
                )
                .await
                .expect("a comment");
        }
        let since = engine
            .comments_since(&onetaskgraph_core::CommentsSinceRequest {
                source: name("plan"),
                since: "2000-01-01T00:00:00Z".parse().expect("an instant"),
                metadata: Vec::new(),
                statuses: Vec::new(),
                items: Vec::new(),
                page_size: NonZeroU32::new(10).expect("a page"),
            })
            .await
            .expect("answered");
        let mut found: Vec<(String, String)> = since
            .comments
            .iter()
            .map(|comment| {
                (
                    comment.item.to_string(),
                    comment.body.as_str().trim_end().to_owned(),
                )
            })
            .collect();
        found.sort();
        assert_eq!(
            found,
            [
                ("plan:a".to_owned(), "On a.".to_owned()),
                ("plan:c".to_owned(), "On c.".to_owned()),
                ("plan:e".to_owned(), "On e.".to_owned()),
            ],
            "{miscount:?}"
        );
    }
}

/// A real folder of Markdown whose reads carry no version, and where another writer raises a
/// task's priority to `high` straight after the first read of it.
struct Versionless {
    inner: Box<dyn onetaskgraph_plugin_api::TaskSource>,
    reads: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl onetaskgraph_plugin_api::TaskSource for Versionless {
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }
    fn capabilities(&self) -> onetaskgraph_plugin_api::Capabilities {
        self.inner.capabilities()
    }
    fn writes(&self) -> onetaskgraph_plugin_api::WriteSupport {
        self.inner.writes()
    }
    async fn health(
        &self,
    ) -> Result<onetaskgraph_plugin_api::Health, onetaskgraph_plugin_api::SourceError> {
        self.inner.health().await
    }
    async fn get_task(
        &self,
        id: &onetaskgraph_plugin_api::NativeId,
    ) -> Result<Option<Task>, onetaskgraph_plugin_api::SourceError> {
        let read = self.inner.get_task(id).await?;
        if self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            let raised = TaskUpdate {
                priority: Some(Priority::High),
                ..TaskUpdate::default()
            };
            self.inner.update_task(id, &raised).await?;
        }
        Ok(read.map(|task| Task {
            version: None,
            ..task
        }))
    }
    async fn get_project(
        &self,
        id: &onetaskgraph_plugin_api::NativeId,
    ) -> Result<Option<onetaskgraph_plugin_api::Project>, onetaskgraph_plugin_api::SourceError>
    {
        self.inner.get_project(id).await
    }
    async fn query_tasks(
        &self,
        query: &onetaskgraph_plugin_api::TaskQuery,
        page: &onetaskgraph_plugin_api::PageRequest,
    ) -> Result<onetaskgraph_plugin_api::Page<Task>, onetaskgraph_plugin_api::SourceError> {
        self.inner.query_tasks(query, page).await
    }
    async fn query_projects(
        &self,
        query: &onetaskgraph_plugin_api::ProjectQuery,
        page: &onetaskgraph_plugin_api::PageRequest,
    ) -> Result<
        onetaskgraph_plugin_api::Page<onetaskgraph_plugin_api::Project>,
        onetaskgraph_plugin_api::SourceError,
    > {
        self.inner.query_projects(query, page).await
    }
    async fn labels(
        &self,
        page: &onetaskgraph_plugin_api::PageRequest,
    ) -> Result<
        onetaskgraph_plugin_api::Page<onetaskgraph_plugin_api::Label>,
        onetaskgraph_plugin_api::SourceError,
    > {
        self.inner.labels(page).await
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
        self.inner.task_dependencies(id, direction, page).await
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
        self.inner.project_dependencies(id, direction, page).await
    }
    async fn update_task(
        &self,
        id: &onetaskgraph_plugin_api::NativeId,
        update: &TaskUpdate,
    ) -> Result<
        Option<onetaskgraph_plugin_api::TaskUpdateOutcome>,
        onetaskgraph_plugin_api::SourceError,
    > {
        self.inner.update_task(id, update).await
    }
    async fn metering(
        &self,
    ) -> Result<Option<onetaskgraph_plugin_api::Metering>, onetaskgraph_plugin_api::SourceError>
    {
        self.inner.metering().await
    }
}

#[tokio::test]
async fn a_guard_a_versionless_source_cannot_hold_is_decided_again_by_the_source_before_it_writes()
{
    use onetaskgraph_plugin_api::SourcePlugin as _;
    let root = tempfile::tempdir().expect("a folder");
    drop(folders(root.path()));
    let plan = name("plan");
    let inner = onetaskgraph_local_md::Plugin
        .build(
            &plan,
            &json!({"root": root.path().join("plan")}),
            &NoSecrets,
        )
        .expect("the local-md plugin builds");
    let source = onetaskgraph_core::ResolvedSource::adopt(
        plan.clone(),
        Box::new(Versionless {
            inner,
            reads: std::sync::atomic::AtomicUsize::new(0),
        }),
    );
    let engine = Engine::new(
        vec![onetaskgraph_core::ConfiguredSource::Ready(source)],
        vec![plan],
    );
    // `c` is at `low` when the engine's guard reads it, and `high` by the time the write would
    // be sent.
    let updated = engine
        .update_task_when(
            &id("plan:c"),
            &retitle("Charlie, guarded"),
            &[Priority::Low],
            None,
        )
        .await
        .expect("answered");
    let guard = updated.guard.expect("the guard says what it found");
    assert_eq!(guard.outcome(), GuardOutcome::Held, "{guard:?}");
    assert!(updated.written.is_empty(), "{updated:?}");
    assert_eq!(task(&engine, "plan:c").await.title, "Charlie batch");
}
