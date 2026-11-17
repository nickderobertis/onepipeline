//! The link a copy records on the item it copied, `onetaskgraph.copies`, and the copy that
//! follows it — as a **Rust caller** reaches both, through `Engine::copy`.
//!
//! A copy records where each item landed on the item itself, keyed by destination, so the
//! next copy of that item reads its counterpart by id instead of scanning the destination.
//! What these hold is the order the rules are tried in, what a link that no longer holds
//! does, and that a copy which cannot finish takes its links back with everything else.
//! The destination scans are counted rather than inferred: every page read of the
//! destination goes through [`Watched`], and a copy found by its link reads none.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use onetaskgraph_core::{
    ConfiguredSource, CopyAction, CopyItems, CopyLink, CopyOutcome, CopyReport, CopyRequest,
    CopyScope, Engine, EngineError, Failure, GlobalId, MatchBy, ResolvedSource,
};
use onetaskgraph_plugin_api::{
    Capabilities, DependencyEdge, Direction, Document, DocumentQuery, Health, ItemWrite, Label,
    Location, MetadataKey, MetadataRecord, NativeId, Page, PageRequest, Project, ProjectQuery,
    SecretResolver, SourceError, SourceName, SourcePlugin, Task, TaskQuery, TaskSource,
    WriteSupport,
};
use secrecy::SecretString;
use serde_json::{Value, json};

/// No source in this crate's tests needs a credential.
struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

fn name(value: &str) -> SourceName {
    SourceName::new(value).expect("a valid source name")
}

fn id(value: &str) -> GlobalId {
    value.parse().expect("a qualified id")
}

/// An `in-memory` source that counts every page it serves and can be told how one item's
/// metadata write goes wrong, as a backend or a plugin would.
///
/// Every page read goes through the three `query_*` methods, so `pages` is every scan of
/// this source — the rule-2 search, a `--match-by` search, the walk for a project to file
/// under and the walk for what a project copy left behind alike.
struct Watched {
    inner: Box<dyn TaskSource>,
    pages: Arc<AtomicU32>,
    faults: Vec<(NativeId, Fault)>,
    /// How one item's `onetaskgraph.copy_locations` write goes wrong, its link's left alone.
    location_faults: Vec<(NativeId, Fault)>,
    /// Whether this source says where it wrote each item, as a hosted one's create answers.
    locates: bool,
    /// How this source answers a batch of creates, when it creates several in one request.
    batches: Option<Batched>,
    /// Every item and key whose metadata write has reached the source, for
    /// [`Fault::Unrestorable`].
    written: std::sync::Mutex<Vec<(NativeId, MetadataKey)>>,
}

/// How a source creating several tasks in one request answers what it created.
#[derive(Clone, Copy)]
enum Batched {
    /// With every id, as a source does.
    Whole,
    /// With one id fewer than it was asked to create.
    Short,
    /// With the first id answered twice in place of the second.
    Repeated,
}

/// How one item's metadata write goes wrong.
#[derive(Clone, Copy)]
enum Fault {
    /// The backend drops it: neither of the answers a copy reads as the source being unable
    /// to hold a link, so the copy fails and has to undo itself.
    Unavailable,
    /// The source answers that it holds no such item, as one deleted mid-copy would.
    Gone,
    /// A plugin written when §4.18 refused every reserved key refuses this one as malformed
    /// — after having written it, as far as the copy can tell.
    Malformed,
    /// The first write lands, and every later one — the copy putting it back — is dropped.
    Unrestorable,
}

impl Watched {
    /// What the write of `key` on `id` does before it reaches the source: the answer it gives
    /// in place of the source's, the failure, or nothing.
    fn before<T>(
        &self,
        id: &NativeId,
        key: &MetadataKey,
    ) -> Result<Option<Option<T>>, SourceError> {
        let dropped = || SourceError::Unavailable {
            message: format!("the backend dropped the metadata write of {id}"),
        };
        let mut written = self.written.lock().expect("the record of writes");
        let written_now = (id.clone(), key.clone());
        let again = written.contains(&written_now);
        written.push(written_now);
        match self.fault_of(id, key) {
            Some(Fault::Gone) => Ok(Some(None)),
            Some(Fault::Unavailable) => Err(dropped()),
            Some(Fault::Unrestorable) if again => Err(dropped()),
            Some(Fault::Malformed | Fault::Unrestorable) | None => Ok(None),
        }
    }

    /// What the write of `key` on `id` answers once the source has made it.
    fn after<T>(
        &self,
        id: &NativeId,
        key: &MetadataKey,
        written: Option<T>,
    ) -> Result<Option<T>, SourceError> {
        match self.fault_of(id, key) {
            Some(Fault::Malformed) => Err(SourceError::Malformed {
                message: format!("the reserved key sent for {id} is not a caller's own"),
            }),
            _ => Ok(written),
        }
    }

    fn fault_of(&self, id: &NativeId, key: &MetadataKey) -> Option<Fault> {
        // Every fault is a link's: the intent a create records is written as it always is.
        if key.as_str() == MetadataKey::CREATING_KEY {
            return None;
        }
        let faults = if key.is_copy_locations() {
            &self.location_faults
        } else {
            &self.faults
        };
        faults
            .iter()
            .find(|(faulty, _)| faulty == id)
            .map(|(_, fault)| *fault)
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
        self.inner.get_task(id).await
    }

    async fn get_project(&self, id: &NativeId) -> Result<Option<Project>, SourceError> {
        self.inner.get_project(id).await
    }

    async fn get_document(&self, id: &NativeId) -> Result<Option<Document>, SourceError> {
        self.inner.get_document(id).await
    }

    async fn query_tasks(
        &self,
        query: &TaskQuery,
        page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        self.pages.fetch_add(1, Ordering::Relaxed);
        self.inner.query_tasks(query, page).await
    }

    async fn query_projects(
        &self,
        query: &ProjectQuery,
        page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        self.pages.fetch_add(1, Ordering::Relaxed);
        self.inner.query_projects(query, page).await
    }

    async fn query_documents(
        &self,
        query: &DocumentQuery,
        page: &PageRequest,
    ) -> Result<Page<Document>, SourceError> {
        self.pages.fetch_add(1, Ordering::Relaxed);
        self.inner.query_documents(query, page).await
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

    async fn write_task(&self, write: &ItemWrite<Task>) -> Result<NativeId, SourceError> {
        self.inner.write_task(write).await
    }

    async fn write_project(&self, write: &ItemWrite<Project>) -> Result<NativeId, SourceError> {
        self.inner.write_project(write).await
    }

    async fn write_document(&self, write: &ItemWrite<Document>) -> Result<NativeId, SourceError> {
        self.inner.write_document(write).await
    }

    async fn set_task_metadata(
        &self,
        id: &NativeId,
        key: &MetadataKey,
        value: &Value,
    ) -> Result<Option<Task>, SourceError> {
        if let Some(answer) = self.before(id, key)? {
            return Ok(answer);
        }
        let written = self.inner.set_task_metadata(id, key, value).await?;
        self.after(id, key, written)
    }

    /// The link write a copy makes while it also takes an intent to create off the item, held
    /// to the same faults as the narrow write of the link alone.
    async fn update_task(
        &self,
        id: &NativeId,
        update: &onetaskgraph_plugin_api::TaskUpdate,
    ) -> Result<Option<onetaskgraph_plugin_api::TaskUpdateOutcome>, SourceError> {
        let linked = update
            .metadata_set
            .keys()
            .find(|key| key.is_copies())
            .cloned();
        if let Some(key) = &linked
            && let Some(answer) = self.before(id, key)?
        {
            return Ok(answer);
        }
        let outcome = self.inner.update_task(id, update).await?;
        match &linked {
            Some(key) => self.after(id, key, outcome),
            None => Ok(outcome),
        }
    }

    async fn set_project_metadata(
        &self,
        id: &NativeId,
        key: &MetadataKey,
        value: &Value,
    ) -> Result<Option<Project>, SourceError> {
        if let Some(answer) = self.before(id, key)? {
            return Ok(answer);
        }
        let written = self.inner.set_project_metadata(id, key, value).await?;
        self.after(id, key, written)
    }

    async fn set_document_metadata(
        &self,
        id: &NativeId,
        key: &MetadataKey,
        value: &Value,
    ) -> Result<Option<Document>, SourceError> {
        if let Some(answer) = self.before(id, key)? {
            return Ok(answer);
        }
        let written = self.inner.set_document_metadata(id, key, value).await?;
        self.after(id, key, written)
    }

    async fn delete_task(&self, id: &NativeId) -> Result<(), SourceError> {
        self.inner.delete_task(id).await
    }

    fn create_batch_limit(&self) -> usize {
        if self.batches.is_some() { 11 } else { 1 }
    }

    async fn write_tasks(&self, writes: &[ItemWrite<Task>]) -> Result<Vec<NativeId>, SourceError> {
        let mut ids = self.inner.write_tasks(writes).await?;
        match self.batches {
            Some(Batched::Short) => {
                ids.pop();
            }
            Some(Batched::Repeated) if ids.len() > 1 => ids[1] = ids[0].clone(),
            Some(Batched::Whole | Batched::Repeated) | None => {}
        }
        Ok(ids)
    }

    fn written_location(&self, _record: MetadataRecord, id: &NativeId) -> Option<Location> {
        self.locates
            .then(|| Location::Url(format!("https://into.example/{}", id.0)))
    }

    async fn delete_project(&self, id: &NativeId) -> Result<(), SourceError> {
        self.inner.delete_project(id).await
    }

    async fn delete_document(&self, id: &NativeId) -> Result<(), SourceError> {
        self.inner.delete_document(id).await
    }
}

/// One `in-memory` source named `source`, watched, and the count of the pages it serves.
fn watched(
    source: &str,
    config: &Value,
    faults: &[(&str, Fault)],
) -> (ConfiguredSource, Arc<AtomicU32>) {
    watched_locating(source, config, faults, &[], false)
}

/// An engine copying out of `from` into `into`, which creates several tasks in one request and
/// answers each batch as `batches` says.
fn engine_batching(from: &Value, into: &Value, batches: Batched) -> Engine {
    let (from, _) = watched("from", from, &[]);
    let pages = Arc::new(AtomicU32::new(0));
    let inner = onetaskgraph_in_memory::Plugin
        .build(&name("into"), into, &NoSecrets)
        .expect("the in-memory plugin builds");
    let into = Watched {
        inner,
        pages,
        faults: Vec::new(),
        location_faults: Vec::new(),
        locates: false,
        batches: Some(batches),
        written: std::sync::Mutex::new(Vec::new()),
    };
    let into = ConfiguredSource::Ready(ResolvedSource::adopt(name("into"), Box::new(into)));
    Engine::new(vec![from, into], vec![name("from"), name("into")])
}

/// [`watched`], answering each `onetaskgraph.copy_locations` write named in `location_faults`
/// as it says, and saying where it wrote each item when `locates`.
fn watched_locating(
    source: &str,
    config: &Value,
    faults: &[(&str, Fault)],
    location_faults: &[(&str, Fault)],
    locates: bool,
) -> (ConfiguredSource, Arc<AtomicU32>) {
    let pages = Arc::new(AtomicU32::new(0));
    let inner = onetaskgraph_in_memory::Plugin
        .build(&name(source), config, &NoSecrets)
        .expect("the in-memory plugin builds");
    let built = Watched {
        inner,
        pages: Arc::clone(&pages),
        faults: faults
            .iter()
            .map(|(id, fault)| (NativeId((*id).to_owned()), *fault))
            .collect(),
        location_faults: location_faults
            .iter()
            .map(|(id, fault)| (NativeId((*id).to_owned()), *fault))
            .collect(),
        locates,
        batches: None,
        written: std::sync::Mutex::new(Vec::new()),
    };
    (
        ConfiguredSource::Ready(ResolvedSource::adopt(name(source), Box::new(built))),
        pages,
    )
}

/// An engine copying out of `from` into `into`, both `in-memory` and both watched, and the
/// count of the pages `into` serves.
fn engine(from: &Value, into: &Value) -> (Engine, Arc<AtomicU32>) {
    engine_failing(from, into, &[])
}

/// The same, with `from` answering each metadata write named in `faults` as it says.
fn engine_failing(
    from: &Value,
    into: &Value,
    faults: &[(&str, Fault)],
) -> (Engine, Arc<AtomicU32>) {
    let (from, _) = watched("from", from, faults);
    let (into, pages) = watched("into", into, &[]);
    (
        Engine::new(vec![from, into], vec![name("from"), name("into")]),
        pages,
    )
}

/// An engine copying out of `from`, whose `onetaskgraph.copy_locations` writes go as
/// `location_faults` says, into `into`, which says where it wrote each item.
fn engine_locating(from: &Value, into: &Value, location_faults: &[(&str, Fault)]) -> Engine {
    engine_locating_failing(from, into, &[], location_faults)
}

/// [`engine_locating`], with `from` also answering each link write named in `faults` as it
/// says.
fn engine_locating_failing(
    from: &Value,
    into: &Value,
    faults: &[(&str, Fault)],
    location_faults: &[(&str, Fault)],
) -> Engine {
    let (from, _) = watched_locating("from", from, faults, location_faults, false);
    let (into, _) = watched_locating("into", into, &[], &[], true);
    Engine::new(vec![from, into], vec![name("from"), name("into")])
}

/// What one task records at `onetaskgraph.copy_locations`, or `Value::Null` when it records
/// none.
async fn locations_of(engine: &Engine, item: &str) -> Value {
    task_metadata(engine, item)
        .await
        .get(MetadataKey::COPY_LOCATIONS_KEY)
        .cloned()
        .unwrap_or(Value::Null)
}

/// One task, as an `in-memory` source holds it, recording `metadata`.
fn task(id: &str, title: &str, metadata: &Value) -> Value {
    json!({
        "id": id,
        "title": title,
        "content": "the engine core",
        "status": {"category": "todo", "name": "Todo"},
        "labels": [],
        "metadata": metadata,
    })
}

/// A copy of `items` into `into`, with every escape switched off.
fn copy_of(items: &[&str], scope: CopyScope) -> CopyRequest {
    CopyRequest {
        items: CopyItems::new(items.iter().map(|item| id(item)).collect())
            .expect("a copy names at least one item"),
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

/// A copy of one task into `into`.
fn one(item: &str) -> CopyRequest {
    copy_of(&[item], CopyScope::Tasks)
}

/// What a report says about one item: where it landed, what the copy did, which rule found
/// it and what became of its link.
fn said(outcome: &CopyOutcome) -> (Option<String>, String, Option<String>, Option<CopyLink>) {
    (
        outcome.destination().map(ToString::to_string),
        outcome.action.name(),
        via(outcome),
        outcome.action.link(),
    )
}

/// The word an outcome reports at `via`, read off its own wire form: `created`, or the
/// `CopyVia` that found its counterpart.
fn via(outcome: &CopyOutcome) -> Option<String> {
    let word = serde_json::to_value(outcome).expect("an outcome serialises")["via"]
        .as_str()
        .map(ToOwned::to_owned);
    // The typed reading agrees with the wire: the rule for an item found, `created` for one
    // created, and nothing for an orphan.
    let typed = outcome.action.found_by().map(|found| {
        serde_json::to_value(found)
            .expect("a word")
            .as_str()
            .expect("a word")
            .to_owned()
    });
    match &outcome.action {
        CopyAction::Created { .. } => assert_eq!(word.as_deref(), Some("created")),
        CopyAction::Orphaned { .. }
        | CopyAction::Failed { .. }
        | CopyAction::Skipped { .. }
        | CopyAction::Oversize { .. } => {
            assert_eq!(word, None);
        }
        CopyAction::Updated { .. } | CopyAction::Unchanged { .. } => assert_eq!(word, typed),
    }
    word
}

fn landed(
    destination: &str,
    action: &str,
    via: &str,
    link: CopyLink,
) -> (Option<String>, String, Option<String>, Option<CopyLink>) {
    (
        Some(destination.to_owned()),
        action.to_owned(),
        Some(via.to_owned()),
        Some(link),
    )
}

/// The metadata one task holds, read back through the engine's own show verb.
async fn task_metadata(engine: &Engine, item: &str) -> serde_json::Map<String, Value> {
    engine
        .task(&id(item))
        .await
        .expect("the show verb answers")
        .items[0]
        .item
        .metadata
        .clone()
        .into_iter()
        .collect()
}

/// What one task records at `onetaskgraph.copies`, or `Value::Null` when it records none.
async fn links_of(engine: &Engine, item: &str) -> Value {
    task_metadata(engine, item)
        .await
        .get(MetadataKey::COPIES_KEY)
        .cloned()
        .unwrap_or(Value::Null)
}

/// Every task one in-memory source holds, by id.
async fn held(engine: &Engine, source: &str) -> Vec<String> {
    let response = engine
        .tasks(&onetaskgraph_core::TaskRequest {
            sources: vec![name(source)],
            filters: onetaskgraph_core::Filters::default(),
            project: onetaskgraph_core::ProjectSelector::Any,
            priorities: Vec::new(),
            commented_since: None,
            metadata: Vec::new(),
            origin: None,
            include_members: false,
            paging: onetaskgraph_core::Paging {
                limit: std::num::NonZeroU32::new(50).expect("a non-zero limit"),
                token: None,
            },
        })
        .await
        .expect("the list verb answers");
    response
        .items
        .into_iter()
        .map(|task| task.id.to_string())
        .collect()
}

async fn copied(engine: &Engine, request: &CopyRequest) -> CopyReport {
    engine.copy(request).await.expect("the copy runs")
}

#[tokio::test]
async fn a_copied_task_records_its_link_and_the_next_copy_follows_it_without_a_scan() {
    let (engine, pages) = engine(
        &json!({"tasks": [task("T-1", "Alpha", &json!({}))]}),
        &json!({}),
    );

    let first = copied(&engine, &one("from:T-1")).await;
    assert_eq!(
        said(&first.items[0]),
        landed("into:T-1", "created", "created", CopyLink::Recorded)
    );
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"into": "into:T-1"})
    );
    // The link is the copied item's own and never travels: the destination item records
    // its origin, and nothing at the other key.
    let arrived = task_metadata(&engine, "into:T-1").await;
    assert_eq!(arrived[GlobalId::ORIGIN_KEY], json!("from:T-1"));
    assert!(
        !arrived.contains_key(MetadataKey::COPIES_KEY),
        "the link is not carried onto the destination: {arrived:?}"
    );

    // The next copy reads its counterpart by id and scans nothing — unchanged, and then
    // after an edit at the source, updated.
    pages.store(0, Ordering::Relaxed);
    let again = copied(&engine, &one("from:T-1")).await;
    assert_eq!(
        said(&again.items[0]),
        landed("into:T-1", "unchanged", "link", CopyLink::Unchanged)
    );
    engine
        .set_task_metadata(
            &id("from:T-1"),
            &MetadataKey::new("caller.edited").expect("a caller key"),
            &json!(true),
        )
        .await
        .expect("the source takes an edit");
    let edited = copied(&engine, &one("from:T-1")).await;
    assert_eq!(
        said(&edited.items[0]),
        landed("into:T-1", "updated", "link", CopyLink::Unchanged)
    );
    assert_eq!(
        pages.load(Ordering::Relaxed),
        0,
        "a copy found by its link reads no page of the destination"
    );
    assert_eq!(
        task_metadata(&engine, "into:T-1").await["caller.edited"],
        json!(true)
    );
    assert_eq!(held(&engine, "into").await, vec!["into:T-1".to_owned()]);
}

#[tokio::test]
async fn a_project_and_a_document_record_their_links_and_are_found_by_them() {
    let from = json!({
        "capabilities": {"documents": "native"},
        "projects": [{"id": "P-1", "title": "Engine",
                      "status": {"category": "todo", "name": "Todo"}, "labels": []}],
        "documents": [{"id": "D-1", "title": "Design", "content": "why",
                       "project": null, "labels": []}],
    });
    let (engine, pages) = engine(&from, &json!({"capabilities": {"documents": "native"}}));
    let project = copy_of(
        &["from:P-1"],
        CopyScope::Projects {
            tasks: false,
            documents: false,
        },
    );
    let document = copy_of(&["from:D-1"], CopyScope::Documents);

    for (request, landed_on) in [(&project, "into:P-1"), (&document, "into:D-1")] {
        let first = copied(&engine, request).await;
        assert_eq!(
            said(&first.items[0]),
            landed(landed_on, "created", "created", CopyLink::Recorded)
        );
    }
    let project_links = engine
        .project(&id("from:P-1"))
        .await
        .expect("the show verb answers")
        .items[0]
        .item
        .metadata[MetadataKey::COPIES_KEY]
        .clone();
    assert_eq!(project_links, json!({"into": "into:P-1"}));
    let document_links = engine
        .document(&id("from:D-1"))
        .await
        .expect("the show verb answers")
        .items[0]
        .item
        .metadata[MetadataKey::COPIES_KEY]
        .clone();
    assert_eq!(document_links, json!({"into": "into:D-1"}));

    pages.store(0, Ordering::Relaxed);
    for (request, landed_on) in [(&project, "into:P-1"), (&document, "into:D-1")] {
        let again = copied(&engine, request).await;
        assert_eq!(
            said(&again.items[0]),
            landed(landed_on, "unchanged", "link", CopyLink::Unchanged)
        );
    }
    assert_eq!(
        pages.load(Ordering::Relaxed),
        0,
        "a project and a document found by their links read no page of the destination"
    );
}

#[tokio::test]
async fn a_link_naming_nothing_there_refuses_until_recreate_creates_and_records_it_again() {
    let from = json!({"tasks": [task("T-1", "Alpha",
        &json!({MetadataKey::COPIES_KEY: {"into": "into:GONE", "elsewhere": "elsewhere:E-1"}}))]});
    let (engine, _) = engine(&from, &json!({}));

    let refused = engine
        .copy(&one("from:T-1"))
        .await
        .expect_err("a link naming nothing is refused");
    assert!(
        matches!(&refused, EngineError::StaleLink { item, link }
            if item == "from:T-1" && link == "into:GONE"),
        "{refused:?}"
    );
    let message = refused.to_string();
    assert!(
        message.contains("from:T-1") && message.contains("into:GONE"),
        "the refusal names both ids: {message}"
    );
    assert!(message.contains("--recreate"), "and the escape: {message}");
    assert_eq!(Failure::from(&refused).kind(), "stale-link");
    assert!(
        held(&engine, "into").await.is_empty(),
        "nothing was written"
    );

    let recreated = copied(
        &engine,
        &CopyRequest {
            recreate: true,
            ..one("from:T-1")
        },
    )
    .await;
    assert_eq!(
        said(&recreated.items[0]),
        landed("into:T-1", "created", "created", CopyLink::Recorded)
    );
    // The one entry is rewritten and every other destination's is left as it was.
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"into": "into:T-1", "elsewhere": "elsewhere:E-1"})
    );
}

#[tokio::test]
async fn a_link_to_an_item_somebody_repointed_is_ignored_and_rewritten_to_what_the_rules_find() {
    let from = json!({"tasks": [task("T-1", "Alpha",
        &json!({MetadataKey::COPIES_KEY: {"into": "into:X"}}))]});
    let into = json!({"tasks": [
        // The item the link names, re-pointed by a person at something else.
        task("X", "Somebody else's", &json!({GlobalId::ORIGIN_KEY: "elsewhere:Z"})),
        // The item that really records this one as its origin.
        task("Y", "Alpha", &json!({GlobalId::ORIGIN_KEY: "from:T-1"})),
    ]});
    let (engine, pages) = engine(&from, &into);

    let report = copied(&engine, &one("from:T-1")).await;
    assert_eq!(
        said(&report.items[0]),
        landed("into:Y", "unchanged", "scan", CopyLink::Recorded)
    );
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"into": "into:Y"})
    );
    assert_eq!(
        task_metadata(&engine, "into:X").await[GlobalId::ORIGIN_KEY],
        json!("elsewhere:Z"),
        "the re-pointed item is not touched"
    );

    pages.store(0, Ordering::Relaxed);
    let again = copied(&engine, &one("from:T-1")).await;
    assert_eq!(
        said(&again.items[0]),
        landed("into:Y", "unchanged", "link", CopyLink::Unchanged)
    );
    assert_eq!(pages.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn a_repointed_link_with_nothing_else_to_find_creates_and_records_the_new_item() {
    let from = json!({"tasks": [task("T-1", "Alpha",
        &json!({MetadataKey::COPIES_KEY: {"into": "into:X"}}))]});
    let into = json!({"tasks": [
        task("X", "Somebody else's", &json!({GlobalId::ORIGIN_KEY: "elsewhere:Z"})),
    ]});
    let (engine, _) = engine(&from, &into);

    let report = copied(&engine, &one("from:T-1")).await;
    assert_eq!(
        said(&report.items[0]),
        landed("into:T-1", "created", "created", CopyLink::Recorded)
    );
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"into": "into:T-1"})
    );
}

#[tokio::test]
async fn an_item_whose_own_origin_names_the_destination_is_found_by_it_and_records_nothing() {
    // The two hand-written shapes a caller used before there was a link — a follow-up
    // ticket and a write-back's shadow task each naming their board item at their own
    // origin — keep being found by rule 1 and gain no write.
    let from = json!({"tasks": [task("T-1", "Alpha, settled",
        &json!({GlobalId::ORIGIN_KEY: "into:A"}))]});
    let into = json!({"tasks": [task("A", "Alpha", &json!({}))]});
    let (engine, pages) = engine(&from, &into);

    let report = copied(&engine, &one("from:T-1")).await;
    assert_eq!(
        said(&report.items[0]),
        landed("into:A", "updated", "origin", CopyLink::Unchanged)
    );
    assert_eq!(links_of(&engine, "from:T-1").await, Value::Null);
    assert_eq!(pages.load(Ordering::Relaxed), 0);
    assert!(
        !task_metadata(&engine, "into:A")
            .await
            .contains_key(GlobalId::ORIGIN_KEY),
        "a copy-back leaves the original's own provenance as it was"
    );
}

#[tokio::test]
async fn a_match_by_escape_is_reported_as_a_match_and_records_the_link() {
    let from = json!({"tasks": [task("T-1", "Alpha", &json!({}))]});
    let into = json!({"tasks": [task("M", "Alpha", &json!({}))]});
    let (engine, _) = engine(&from, &into);

    let report = copied(
        &engine,
        &CopyRequest {
            match_by: Some(MatchBy::Title),
            ..one("from:T-1")
        },
    )
    .await;
    assert_eq!(
        said(&report.items[0]),
        landed("into:M", "updated", "match", CopyLink::Recorded)
    );
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"into": "into:M"})
    );
}

#[tokio::test]
async fn a_dry_run_says_which_rule_would_answer_and_records_no_link() {
    let (engine, _) = engine(
        &json!({"tasks": [task("T-1", "Alpha", &json!({}))]}),
        &json!({}),
    );

    let report = copied(
        &engine,
        &CopyRequest {
            dry_run: true,
            ..one("from:T-1")
        },
    )
    .await;
    assert_eq!(
        (via(&report.items[0]), report.items[0].action.link()),
        (Some("created".to_owned()), None)
    );
    let wire = serde_json::to_value(&report).expect("a report serialises");
    assert_eq!(wire["items"][0]["via"], json!("created"));
    assert!(
        wire["items"][0].get("link").is_none(),
        "a dry run leaves `link` out: {wire}"
    );
    assert_eq!(links_of(&engine, "from:T-1").await, Value::Null);
}

#[tokio::test]
async fn a_source_that_cannot_hold_a_link_is_copied_from_as_before_and_says_so() {
    for (why, capabilities) in [
        ("has no write side", json!({"writes": "unsupported"})),
        (
            "refuses the key",
            json!({"unwritable_metadata_keys": [MetadataKey::COPIES_KEY]}),
        ),
    ] {
        let from = json!({"capabilities": capabilities,
                          "tasks": [task("T-1", "Alpha", &json!({}))]});
        let (engine, pages) = engine(&from, &json!({}));

        let first = copied(&engine, &one("from:T-1")).await;
        assert_eq!(
            said(&first.items[0]),
            landed("into:T-1", "created", "created", CopyLink::Unrecorded),
            "a source that {why}"
        );
        assert_eq!(links_of(&engine, "from:T-1").await, Value::Null, "{why}");
        // With no link, the next copy is found exactly as it was before there were any.
        pages.store(0, Ordering::Relaxed);
        let again = copied(&engine, &one("from:T-1")).await;
        assert_eq!(
            said(&again.items[0]),
            landed("into:T-1", "unchanged", "scan", CopyLink::Unrecorded),
            "a source that {why}"
        );
        assert!(pages.load(Ordering::Relaxed) > 0, "{why}");
    }
}

#[tokio::test]
async fn a_copy_that_fails_after_creating_leaves_the_source_without_the_link() {
    let from = json!({"tasks": [
        task("T-1", "Alpha", &json!({})),
        task("T-2", "Beta", &json!({})),
    ]});
    let (engine, _) = engine(
        &from,
        &json!({"capabilities": {"uncreatable_titles": ["Beta"]}}),
    );

    engine
        .copy(&copy_of(&["from:T-1", "from:T-2"], CopyScope::Tasks))
        .await
        .expect_err("the second create is refused");
    assert!(
        held(&engine, "into").await.is_empty(),
        "the destination holds none of the copy's items"
    );
    for item in ["from:T-1", "from:T-2"] {
        assert_eq!(links_of(&engine, item).await, Value::Null, "{item}");
    }
}

#[tokio::test]
async fn a_link_write_that_fails_undoes_every_link_and_every_item_the_copy_wrote() {
    // Three items, and the third one's link write fails after the first two have landed:
    // one of them held a link to another destination before, which is written back as it
    // was, and one held none, which is left holding none.
    let from = json!({"tasks": [
        task("T-1", "Alpha", &json!({MetadataKey::COPIES_KEY: {"elsewhere": "elsewhere:E-1"}})),
        task("T-3", "Gamma", &json!({"caller.kept": [1, "two"]})),
        task("T-2", "Beta", &json!({})),
    ]});
    let (engine, _) = engine_failing(&from, &json!({}), &[("T-2", Fault::Unavailable)]);

    let refused = engine
        .copy(&copy_of(
            &["from:T-1", "from:T-3", "from:T-2"],
            CopyScope::Tasks,
        ))
        .await
        .expect_err("the third link write fails");
    assert!(
        refused
            .to_string()
            .contains("dropped the metadata write of T-2"),
        "the copy names what failed: {refused}"
    );
    assert!(held(&engine, "into").await.is_empty(), "{refused}");
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"elsewhere": "elsewhere:E-1"})
    );
    let untouched = task_metadata(&engine, "from:T-3").await;
    assert!(
        !untouched.contains_key(MetadataKey::COPIES_KEY),
        "{untouched:?}"
    );
    assert_eq!(untouched["caller.kept"], json!([1, "two"]));
    assert_eq!(links_of(&engine, "from:T-2").await, Value::Null);
}

#[tokio::test]
async fn a_repeated_copy_of_a_linked_item_keeps_the_destinations_own_link() {
    // The copy-back of an item whose destination counterpart has itself been copied on
    // somewhere: that counterpart's own link is its own, and the copy keeps it.
    let (engine, _) = engine(
        &json!({"tasks": [task("T-1", "Alpha", &json!({}))]}),
        &json!({}),
    );
    copied(&engine, &one("from:T-1")).await;

    let back = copied(
        &engine,
        &CopyRequest {
            destination: name("from"),
            ..one("into:T-1")
        },
    )
    .await;
    assert_eq!(
        said(&back.items[0]),
        landed("from:T-1", "unchanged", "origin", CopyLink::Unchanged),
        "the copy back finds the original by the copy's own origin, and the original's \
         link is not a difference"
    );
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"into": "into:T-1"})
    );
    assert!(matches!(back.items[0].action, CopyAction::Unchanged { .. }));
}

/// A source holding project `P-1` and task `T-1` filed under it.
fn a_project_and_its_task() -> Value {
    json!({
        "projects": [{"id": "P-1", "title": "Engine",
                      "status": {"category": "todo", "name": "Todo"}, "labels": []}],
        "tasks": [{"id": "T-1", "title": "Alpha", "project": "P-1",
                   "status": {"category": "todo", "name": "Todo"}, "labels": []}],
    })
}

#[tokio::test]
async fn a_task_copied_on_its_own_is_filed_under_its_project_by_the_projects_link() {
    let (engine, pages) = engine(&a_project_and_its_task(), &json!({}));
    copied(
        &engine,
        &copy_of(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ),
    )
    .await;

    pages.store(0, Ordering::Relaxed);
    let report = copied(&engine, &one("from:T-1")).await;
    assert_eq!(
        said(&report.items[0]),
        landed("into:T-1", "unchanged", "link", CopyLink::Unchanged)
    );
    assert_eq!(
        pages.load(Ordering::Relaxed),
        0,
        "neither the task nor the project it is filed under was searched for"
    );
    assert_eq!(
        engine
            .task(&id("into:T-1"))
            .await
            .expect("the show verb answers")
            .items[0]
            .item
            .project,
        Some(NativeId("P-1".to_owned()))
    );
}

#[tokio::test]
async fn a_project_link_naming_nothing_there_files_by_searching_instead_of_refusing() {
    // Only a copy's own target is refused for a stale link: where an item is filed is a
    // lookup, and a lookup the link cannot answer is answered the way it always was.
    let mut from = a_project_and_its_task();
    from["projects"][0]["metadata"] = json!({MetadataKey::COPIES_KEY: {"into": "into:GONE"}});
    let into = json!({"projects": [{"id": "Q", "title": "Engine",
        "status": {"category": "todo", "name": "Todo"}, "labels": [],
        "metadata": {GlobalId::ORIGIN_KEY: "from:P-1"}}]});
    let (engine, pages) = engine(&from, &into);

    let report = copied(&engine, &one("from:T-1")).await;
    assert_eq!(
        said(&report.items[0]),
        landed("into:T-1", "created", "created", CopyLink::Recorded)
    );
    assert!(pages.load(Ordering::Relaxed) > 0);
    assert_eq!(
        engine
            .task(&id("into:T-1"))
            .await
            .expect("the show verb answers")
            .items[0]
            .item
            .project,
        Some(NativeId("Q".to_owned())),
        "filed under the project the search found"
    );
}

#[tokio::test]
async fn a_source_that_no_longer_holds_the_item_or_answers_malformed_leaves_it_unrecorded() {
    // Neither answer is a failure of the copy: the destination landed, and what went wrong
    // is only that the item could not be told where.
    let from = json!({"tasks": [
        task("T-1", "Alpha", &json!({})),
        task("T-2", "Beta", &json!({})),
        task("T-3", "Gamma", &json!({})),
    ]});
    let (engine, _) = engine_failing(
        &from,
        &json!({}),
        &[("T-1", Fault::Gone), ("T-2", Fault::Malformed)],
    );

    let report = copied(
        &engine,
        &copy_of(&["from:T-1", "from:T-2", "from:T-3"], CopyScope::Tasks),
    )
    .await;
    let links: Vec<Option<CopyLink>> = report
        .items
        .iter()
        .map(|outcome| outcome.action.link())
        .collect();
    assert_eq!(
        links,
        [
            Some(CopyLink::Unrecorded),
            Some(CopyLink::Unrecorded),
            Some(CopyLink::Recorded)
        ]
    );
    assert_eq!(
        held(&engine, "into").await,
        ["into:T-1", "into:T-2", "into:T-3"]
    );
    assert_eq!(
        links_of(&engine, "from:T-3").await,
        json!({"into": "into:T-3"})
    );
}

#[tokio::test]
async fn a_malformed_answer_is_still_put_back_when_the_copy_cannot_finish() {
    // A plugin that answered malformed may have written the link before it did, so the
    // journal keeps the write and a later failure puts the item back regardless.
    let from = json!({"tasks": [
        task("T-1", "Alpha", &json!({MetadataKey::COPIES_KEY: {"elsewhere": "elsewhere:E"}})),
        task("T-2", "Beta", &json!({})),
    ]});
    let (engine, _) = engine_failing(
        &from,
        &json!({}),
        &[("T-1", Fault::Malformed), ("T-2", Fault::Unavailable)],
    );

    engine
        .copy(&copy_of(&["from:T-1", "from:T-2"], CopyScope::Tasks))
        .await
        .expect_err("the second link write fails");
    assert!(held(&engine, "into").await.is_empty());
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"elsewhere": "elsewhere:E"})
    );
}

#[tokio::test]
async fn entries_that_are_not_links_are_dropped_when_the_link_is_recorded() {
    // A person's typo under the key is no link any copy can follow, so it is not carried into
    // the value the source is handed — which a plugin behind the stdio boundary would refuse.
    let from = json!({"tasks": [task("T-1", "Alpha", &json!({MetadataKey::COPIES_KEY: {
        "elsewhere": "elsewhere:E",
        "notes": 5,
        "board": "elsewhere:X",
    }}))]});
    let (engine, _) = engine(&from, &json!({}));

    let report = copied(&engine, &one("from:T-1")).await;
    assert_eq!(report.items[0].action.link(), Some(CopyLink::Recorded));
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"elsewhere": "elsewhere:E", "into": "into:T-1"})
    );
}

#[tokio::test]
async fn a_link_the_source_will_not_take_back_is_named_as_left_behind() {
    // The copy fails after `T-1`'s link landed, and `T-1`'s source then refuses to put its
    // link back: the refusal says the copy could not be undone and names `T-1` as holding
    // what it wrote, while the destination items it created are still taken back.
    let from = json!({"tasks": [
        task("T-1", "Alpha", &json!({MetadataKey::COPIES_KEY: {"elsewhere": "elsewhere:E"}})),
        task("T-2", "Beta", &json!({})),
    ]});
    let (engine, _) = engine_failing(
        &from,
        &json!({}),
        &[("T-1", Fault::Unrestorable), ("T-2", Fault::Unavailable)],
    );

    let refused = engine
        .copy(&copy_of(&["from:T-1", "from:T-2"], CopyScope::Tasks))
        .await
        .expect_err("the second link write fails");
    let EngineError::CopyNotUndone {
        left_behind,
        refusal,
        ..
    } = &refused
    else {
        panic!("the copy could not be undone, and says so: {refused:?}");
    };
    assert_eq!(
        left_behind
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["from:T-1"]
    );
    assert!(
        matches!(refusal, SourceError::Unavailable { .. }),
        "{refusal:?}"
    );
    let message = refused.to_string();
    assert!(
        message.contains("could not be undone") && message.contains("from:T-1"),
        "{message}"
    );
    assert!(
        held(&engine, "into").await.is_empty(),
        "the created items were still taken back"
    );
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"elsewhere": "elsewhere:E", "into": "into:T-1"}),
        "and T-1 holds what the copy wrote, as the refusal says"
    );
}

#[tokio::test]
async fn a_location_the_source_will_not_hold_leaves_the_link_recorded() {
    // The location beside a link is what a later document copy saves a read with, and nothing
    // more: a source that refuses it, answers that it no longer holds the item, or answers as
    // a plugin predating the key does, leaves the copy landed and its link recorded.
    let located = json!({"into": {"id": "into:T-1",
                                  "location": {"url": "https://into.example/T-1"}}});
    for (why, capabilities, fault, held) in [
        ("holds it", json!({}), None, located.clone()),
        (
            "refuses the key",
            json!({"unwritable_metadata_keys": [MetadataKey::COPY_LOCATIONS_KEY]}),
            None,
            Value::Null,
        ),
        (
            "no longer holds the item",
            json!({}),
            Some(Fault::Gone),
            Value::Null,
        ),
        // Written before the answer, as far as the copy can tell: kept, not taken back.
        (
            "answers malformed",
            json!({}),
            Some(Fault::Malformed),
            located.clone(),
        ),
    ] {
        let from = json!({"capabilities": capabilities,
                          "tasks": [task("T-1", "Alpha", &json!({}))]});
        let faults: Vec<(&str, Fault)> = fault.into_iter().map(|fault| ("T-1", fault)).collect();
        let engine = engine_locating(&from, &json!({}), &faults);
        let report = copied(&engine, &one("from:T-1")).await;
        assert_eq!(
            said(&report.items[0]),
            landed("into:T-1", "created", "created", CopyLink::Recorded),
            "a source that {why}"
        );
        assert_eq!(
            links_of(&engine, "from:T-1").await,
            json!({"into": "into:T-1"}),
            "{why}"
        );
        assert_eq!(locations_of(&engine, "from:T-1").await, held, "{why}");
    }
}

#[tokio::test]
async fn a_location_write_that_fails_undoes_every_location_link_and_item_the_copy_wrote() {
    // Three items, and the third one's location write is dropped after the first two items'
    // links and locations, and its own link, landed: the one that held a location for another
    // destination before holds it again, and the ones that held none hold none.
    let elsewhere = json!({"elsewhere": {"id": "elsewhere:E-1",
                                         "location": {"url": "https://elsewhere.example/E-1"}}});
    let from = json!({"tasks": [
        task("T-1", "Alpha", &json!({
            MetadataKey::COPIES_KEY: {"elsewhere": "elsewhere:E-1"},
            MetadataKey::COPY_LOCATIONS_KEY: elsewhere,
        })),
        task("T-3", "Gamma", &json!({"caller.kept": [1, "two"]})),
        task("T-2", "Beta", &json!({})),
    ]});
    let engine = engine_locating(&from, &json!({}), &[("T-2", Fault::Unavailable)]);

    let refused = engine
        .copy(&copy_of(
            &["from:T-1", "from:T-3", "from:T-2"],
            CopyScope::Tasks,
        ))
        .await
        .expect_err("the third location write fails");
    assert!(
        refused
            .to_string()
            .contains("dropped the metadata write of T-2"),
        "the copy names what failed: {refused}"
    );
    assert!(held(&engine, "into").await.is_empty(), "{refused}");
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"elsewhere": "elsewhere:E-1"})
    );
    assert_eq!(locations_of(&engine, "from:T-1").await, elsewhere);
    for item in ["from:T-3", "from:T-2"] {
        let untouched = task_metadata(&engine, item).await;
        assert!(
            !untouched.contains_key(MetadataKey::COPIES_KEY)
                && !untouched.contains_key(MetadataKey::COPY_LOCATIONS_KEY),
            "{item}: {untouched:?}"
        );
    }
    assert_eq!(
        task_metadata(&engine, "from:T-3").await["caller.kept"],
        json!([1, "two"])
    );
}

#[tokio::test]
async fn a_batch_of_creates_answered_with_the_wrong_ids_leaves_the_destination_uncertain() {
    // Created together, the tasks are only as known as the ids their batch answers with: an
    // answer one short, or naming one id twice, cannot say which item is which, so the copy
    // records no link and refuses naming the destination and the method, rather than
    // journalling items it cannot name.
    let from = json!({"tasks": [
        task("T-1", "Alpha", &json!({})),
        task("T-2", "Beta", &json!({})),
    ]});
    let whole = engine_batching(&from, &json!({}), Batched::Whole);
    let report = copied(
        &whole,
        &copy_of(&["from:T-1", "from:T-2"], CopyScope::Tasks),
    )
    .await;
    assert_eq!(report.items.len(), 2);
    assert_eq!(held(&whole, "into").await.len(), 2, "both created together");

    for batched in [Batched::Short, Batched::Repeated] {
        let engine = engine_batching(&from, &json!({}), batched);
        let refused = engine
            .copy(&copy_of(&["from:T-1", "from:T-2"], CopyScope::Tasks))
            .await
            .expect_err("an answer that cannot name each task created");
        let EngineError::CopyDestinationUncertain {
            destination,
            operation,
            ..
        } = &refused
        else {
            panic!("not an uncertain destination: {refused}");
        };
        assert_eq!(
            (destination.as_str(), operation.as_str()),
            ("into", "write_tasks")
        );
        for item in ["from:T-1", "from:T-2"] {
            assert_eq!(links_of(&engine, item).await, Value::Null, "{item}");
        }
    }
}

#[tokio::test]
async fn a_location_the_source_will_not_take_back_is_named_as_left_behind() {
    // T-1's link and location land beside the ones it held for another destination, T-2's
    // link write fails, and T-1's source then refuses to put its location back: the copy says
    // it could not be undone, names T-1 as holding what it wrote, and still takes back the
    // items it created.
    let elsewhere =
        json!({"id": "elsewhere:E", "location": {"url": "https://elsewhere.example/E"}});
    let from = json!({"tasks": [
        task("T-1", "Alpha", &json!({
            MetadataKey::COPIES_KEY: {"elsewhere": "elsewhere:E"},
            MetadataKey::COPY_LOCATIONS_KEY: {"elsewhere": elsewhere},
        })),
        task("T-2", "Beta", &json!({})),
    ]});
    let engine = engine_locating_failing(
        &from,
        &json!({}),
        &[("T-2", Fault::Unavailable)],
        &[("T-1", Fault::Unrestorable)],
    );
    let refused = engine
        .copy(&copy_of(&["from:T-1", "from:T-2"], CopyScope::Tasks))
        .await
        .expect_err("the second link write fails");
    let EngineError::CopyNotUndone { left_behind, .. } = &refused else {
        panic!("the copy could not be undone, and says so: {refused:?}");
    };
    assert_eq!(
        left_behind
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["from:T-1"]
    );
    assert!(held(&engine, "into").await.is_empty(), "{refused}");
    assert_eq!(
        locations_of(&engine, "from:T-1").await,
        json!({"elsewhere": elsewhere,
               "into": {"id": "into:T-1", "location": {"url": "https://into.example/T-1"}}}),
        "T-1 holds the location the copy wrote, as the refusal says"
    );
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"elsewhere": "elsewhere:E"}),
        "and its link, which it would take back, is put back"
    );
}

#[tokio::test]
async fn a_rust_caller_selects_a_verified_target_before_a_stale_link_without_scanning() {
    let (engine, pages) = engine(
        &json!({"tasks": [task("T-1", "Alpha", &json!({"onetaskgraph.copies": {"into": "into:withdrawn"}}))]}),
        &json!({"tasks": [
            task("first", "First duplicate", &json!({"onetaskgraph.origin": "from:T-1"})),
            task("second", "Second duplicate", &json!({"onetaskgraph.origin": "from:T-1"})),
        ]}),
    );
    let mut request = one("from:T-1");
    request.target_task = Some(id("into:second"));
    let report = copied(&engine, &request).await;
    assert_eq!(
        said(&report.items[0]),
        landed("into:second", "updated", "target-task", CopyLink::Recorded)
    );
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"into": "into:second"})
    );
    let next = copied(&engine, &one("from:T-1")).await;
    assert_eq!(
        said(&next.items[0]),
        landed("into:second", "unchanged", "link", CopyLink::Unchanged)
    );
    assert_eq!(pages.load(Ordering::Relaxed), 0);

    request.target_task = Some(id("into:missing"));
    assert!(matches!(
        engine.copy(&request).await,
        Err(EngineError::TargetTaskMissing { .. })
    ));
    assert_eq!(
        links_of(&engine, "from:T-1").await,
        json!({"into": "into:second"})
    );
}

#[tokio::test]
async fn an_explicit_target_refuses_non_task_scopes_before_reading() {
    let engine = Engine::new(Vec::new(), Vec::new());
    for scope in [
        CopyScope::Documents,
        CopyScope::Projects {
            tasks: false,
            documents: false,
        },
    ] {
        let mut request = copy_of(&["from:T-1"], scope);
        request.target_task = Some(id("into:second"));
        assert!(matches!(
            engine.copy(&request).await,
            Err(EngineError::TargetTaskNeedsOne)
        ));
    }
}

#[tokio::test]
async fn a_destination_guard_on_a_project_or_document_copy_is_refused_before_reading() {
    // No source is configured: a guard the engine read anything for would fail on that instead.
    let engine = Engine::new(Vec::new(), Vec::new());
    for scope in [
        CopyScope::Documents,
        CopyScope::Projects {
            tasks: true,
            documents: false,
        },
    ] {
        for conditions in [
            onetaskgraph_core::CopyConditions {
                status: vec![onetaskgraph_plugin_api::StatusCategory::Todo],
                ..Default::default()
            },
            onetaskgraph_core::CopyConditions {
                priority: vec![onetaskgraph_plugin_api::Priority::None],
                ..Default::default()
            },
        ] {
            let mut request = copy_of(&["from:P-1"], scope.clone());
            request.conditions = conditions;
            assert!(
                matches!(
                    engine.copy(&request).await,
                    Err(EngineError::ConditionsNotTasks)
                ),
                "{scope:?}"
            );
        }
    }
}
