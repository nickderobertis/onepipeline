//! The copy verb as a **Rust caller** reaches it: build the request type this crate
//! exports, call the method on the engine, read the outcomes it returns.
//!
//! Nothing here goes through the command line, and that is the point. This product is
//! exposed three ways from one engine, so a copy a script makes, a copy an application
//! makes and a copy typed at a shell have to be the same call — and the consumer a
//! command-line-only copy would strand is this one, the Rust caller that links the crate.
//! The journeys that drive the same verb as a user does are in
//! `crates/onetaskgraph-e2e/tests/e2e/`.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use onetaskgraph_core::{
    BudgetSpent, Completeness, Config, ConfiguredSource, CopyAction, CopyItems, CopyOutcome,
    CopyReport, CopyRequest, CopyScope, DependencyRequest, Engine, EngineError, GlobalId, MatchBy,
    Paging, ProjectGraphRequest, ResolvedSource, Spent, TaskRequest,
};
use onetaskgraph_plugin_api::{
    Capabilities, Cursor, DependencyEdge, DependencyEndpoint, DependencyKind, DependencySupport,
    Direction, Document, DocumentQuery, Health, ItemKind, ItemWrite, Label, MetadataKey, NativeId,
    Page, PageRequest, Priority, Project, ProjectQuery, SecretResolver, SourceError, SourceName,
    SourcePlugin, Status, StatusCategory, Support, Task, TaskQuery, TaskSource, WriteSupport,
    documentless,
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

/// An engine over a configuration document's `sources:` block.
fn engine_over(sources: Value) -> Engine {
    let config =
        Config::from_document(json!({ "sources": sources })).expect("a valid configuration");
    Engine::build(&config, &NoSecrets)
}

/// One task, held by an `in-memory` source.
fn task(id: &str, title: &str) -> Value {
    json!({
        "id": id,
        "title": title,
        "content": "the engine core",
        "status": {"category": "todo", "name": "Todo"},
        "labels": [{"id": "L-1", "name": "bug"}],
        "metadata": {"caller.shape": {"nested": [1, true, null]}},
        "repositories": ["github.com/nickderobertis/onetaskgraph"]
    })
}

/// Two in-memory sources: one holding `T-1`, one empty and writable.
fn pair() -> Engine {
    engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [task("T-1", "Alpha engine")]}},
        "into": {"plugin": "in-memory", "config": {}},
    }))
}

/// A copy of one task into `into`, with every escape switched off.
fn one(item: &str) -> CopyRequest {
    many(&[item], CopyScope::Tasks)
}

/// A copy of several items into `into`, with every escape switched off.
fn many(items: &[&str], scope: CopyScope) -> CopyRequest {
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

/// The destination id and the word an outcome reports, as a comparable pair.
fn landed(outcome: &CopyOutcome) -> (Option<String>, String) {
    (
        outcome.destination().map(ToString::to_string),
        outcome.action.name(),
    )
}

/// Every task one source holds, by qualified id, through the engine's own list verb.
async fn listed(engine: &Engine, source: &str) -> Vec<String> {
    let response = engine
        .tasks(&TaskRequest {
            sources: vec![name(source)],
            filters: onetaskgraph_core::Filters::default(),
            project: onetaskgraph_core::ProjectSelector::Any,
            priorities: Vec::new(),
            commented_since: None,
            metadata: Vec::new(),
            origin: None,
            include_members: false,
            paging: Paging {
                limit: NonZeroU32::new(50).expect("a non-zero limit"),
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

#[tokio::test]
async fn a_rust_caller_creates_then_updates_the_same_destination_item() {
    let engine = pair();

    let created = engine.copy(&one("from:T-1")).await.expect("the copy runs");
    assert_eq!(created.items.len(), 1);
    assert_eq!(created.items[0].source, id("from:T-1"));
    assert_eq!(
        landed(&created.items[0]),
        (Some("into:T-1".to_owned()), "created".to_owned())
    );

    // The destination really holds it, with the value and the JSON type of every
    // caller-defined key intact — read back through the engine, not through the write.
    let copied = engine
        .task(&id("into:T-1"))
        .await
        .expect("the show verb answers");
    let copied = &copied.items[0].item;
    assert_eq!(copied.title, "Alpha engine");
    assert_eq!(
        copied.metadata["caller.shape"],
        json!({"nested": [1, true, null]})
    );
    assert_eq!(
        copied.metadata[GlobalId::ORIGIN_KEY],
        Value::String("from:T-1".to_owned())
    );
    assert_eq!(
        copied.repositories[0].as_str(),
        "github.com/nickderobertis/onetaskgraph"
    );

    // A second copy of the same item updates that one and creates nothing.
    let again = engine.copy(&one("from:T-1")).await.expect("the copy runs");
    assert_eq!(
        landed(&again.items[0]),
        (Some("into:T-1".to_owned()), "unchanged".to_owned())
    );
    assert_eq!(listed(&engine, "into").await, vec!["into:T-1".to_owned()]);

    // And a copy back the other way follows the origin the copied item carries, so the
    // item it came from is the one it lands on rather than a duplicate. Nothing was
    // edited in between and the copy back leaves that item's own origin — none, because
    // it was authored here — exactly as it is, so there is nothing to write.
    let back = engine
        .copy(&CopyRequest {
            destination: name("from"),
            ..one("into:T-1")
        })
        .await
        .expect("the copy runs");
    assert_eq!(
        landed(&back.items[0]),
        (Some("from:T-1".to_owned()), "unchanged".to_owned())
    );
    assert_eq!(listed(&engine, "from").await, vec!["from:T-1".to_owned()]);
    let original = engine
        .task(&id("from:T-1"))
        .await
        .expect("the show verb answers");
    assert!(
        !original.items[0]
            .item
            .metadata
            .contains_key(GlobalId::ORIGIN_KEY),
        "a copy back does not stamp the original with the id of the copy that came from it"
    );
}

/// A provenance entry whose template is `template`, shaped as a render records one.
fn provenance(template: &str) -> Value {
    json!({
        "template": template,
        "digest": format!("sha256:{}", "0".repeat(64)),
        "body_digest": format!("sha256:{}", "1".repeat(64)),
        "answers_digest": format!("sha256:{}", "2".repeat(64)),
    })
}

/// The metadata an earlier copy of `origin` left at the destination, before anybody there
/// recorded something of their own on it: an approval only the destination holds, a key the
/// source has since changed, a link to where the destination item was copied on to, and a
/// provenance entry and a kind marker the source no longer carries.
fn held_at_destination(origin: &str) -> Value {
    json!({
        GlobalId::ORIGIN_KEY: origin,
        "orchestrator.design-approval": {"approved_by": "nick", "body_digest": "sha256:ab"},
        "caller.shared": "the destination's",
        "onetaskgraph.copies": [format!("{origin}-onward")],
        "onetaskgraph.template": provenance("/stale/template.md"),
        "onetaskgraph.item_kind": "task",
    })
}

/// The metadata the source holds for the item `held_at_destination` copied: the shared key
/// at its new value, and a copy link of its own that names where *it* went.
fn held_at_source(template: Option<&str>) -> Value {
    let mut metadata = json!({
        "caller.shared": "the source's",
        "caller.only-source": [1, 2],
        "onetaskgraph.copies": ["elsewhere:X-9"],
    });
    if let Some(template) = template {
        metadata["onetaskgraph.template"] = provenance(template);
    }
    metadata
}

#[tokio::test]
async fn an_updating_copy_keeps_every_key_only_the_destination_holds() {
    // A task, a project and a document, each already copied and since annotated at the
    // destination. The re-copy changes a title, so every one of them is a real update — and
    // an update merges: the destination's own keys stay, a key both hold takes the source's
    // value, and the reserved keys keep the ownership `docs/metadata.md` states.
    let status = json!({"category": "todo", "name": "Todo"});
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "tasks": [{"id": "T-1", "title": "Task, renamed", "status": status, "labels": [],
                "metadata": held_at_source(None)}],
            "projects": [{"id": "P-1", "title": "Project, renamed", "status": status,
                "labels": [], "metadata": held_at_source(Some("/fresh/project.md"))}],
            "documents": [{"id": "D-1", "title": "Document, renamed", "labels": [],
                "content": "the design", "metadata": held_at_source(None)}],
        }},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "tasks": [{"id": "T-1", "title": "Task", "status": status, "labels": [],
                "metadata": held_at_destination("from:T-1")}],
            "projects": [{"id": "P-1", "title": "Project", "status": status, "labels": [],
                "metadata": held_at_destination("from:P-1")}],
            "documents": [{"id": "D-1", "title": "Document", "labels": [],
                "content": "the design", "metadata": held_at_destination("from:D-1")}],
        }},
    }));
    let requests = [
        ("into:T-1", many(&["from:T-1"], CopyScope::Tasks)),
        (
            "into:P-1",
            many(
                &["from:P-1"],
                CopyScope::Projects {
                    tasks: false,
                    documents: false,
                },
            ),
        ),
        ("into:D-1", many(&["from:D-1"], CopyScope::Documents)),
    ];

    for (landing, request) in &requests {
        let copied = engine.copy(request).await.expect("the copy runs");
        assert_eq!(
            landed(&copied.items[0]),
            (Some((*landing).to_owned()), "updated".to_owned())
        );
    }

    let metadata = [
        engine
            .task(&id("into:T-1"))
            .await
            .expect("the show verb answers")
            .items[0]
            .item
            .metadata
            .clone(),
        engine
            .project(&id("into:P-1"))
            .await
            .expect("the show verb answers")
            .items[0]
            .item
            .metadata
            .clone(),
        engine
            .document(&id("into:D-1"))
            .await
            .expect("the show verb answers")
            .items[0]
            .item
            .metadata
            .clone(),
    ];
    for ((landing, _), metadata) in requests.iter().zip(&metadata) {
        let origin = landing.replace("into:", "from:");
        assert_eq!(
            metadata.get("orchestrator.design-approval"),
            Some(&json!({"approved_by": "nick", "body_digest": "sha256:ab"})),
            "{landing}: a key only the destination holds survives the copy"
        );
        assert_eq!(
            metadata["caller.shared"],
            json!("the source's"),
            "{landing}: a key both hold takes the source's value"
        );
        assert_eq!(metadata["caller.only-source"], json!([1, 2]), "{landing}");
        assert_eq!(
            metadata[GlobalId::ORIGIN_KEY],
            json!(origin),
            "{landing}: the origin is the one the copy records"
        );
        assert_eq!(
            metadata["onetaskgraph.copies"],
            json!([format!("{origin}-onward")]),
            "{landing}: the copy link stays the destination's"
        );
        assert!(
            !metadata.contains_key("onetaskgraph.item_kind"),
            "{landing}: a kind marker the source does not carry is dropped"
        );
    }
    // Provenance is the source's: dropped where the source carries none, replaced where it
    // carries its own.
    assert!(
        !metadata[0].contains_key("onetaskgraph.template"),
        "a task's stale provenance does not survive a copy from a source with none"
    );
    assert!(
        !metadata[2].contains_key("onetaskgraph.template"),
        "a document's stale provenance does not survive a copy from a source with none"
    );
    assert_eq!(
        metadata[1]["onetaskgraph.template"],
        provenance("/fresh/project.md")
    );

    // And the merged item is what the destination now holds, so copying again is unchanged:
    // the key the source does not hold no longer reads as a difference to write away.
    for (landing, request) in &requests {
        let again = engine.copy(request).await.expect("the copy runs");
        assert_eq!(
            landed(&again.items[0]),
            (Some((*landing).to_owned()), "unchanged".to_owned())
        );
    }
}

/// The reserved-key table of `docs/metadata.md`, as `(key, owner)` pairs: the backticked keys
/// of each row's first cell, against the first words of its second.
fn reserved_key_owners() -> Vec<(String, String)> {
    let page = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/metadata.md"),
    )
    .expect("docs/metadata.md is readable");
    let table = page
        .split("| Key | Owner on an update |")
        .nth(1)
        .expect("docs/metadata.md carries the reserved-key table");
    let mut owners = Vec::new();
    for row in table
        .lines()
        .skip(2)
        .take_while(|line| line.starts_with('|'))
    {
        let cells: Vec<&str> = row.split('|').map(str::trim).collect();
        let owner = ["the copy", "the destination", "the source", "neither"]
            .into_iter()
            .find(|owner| cells[2].starts_with(owner))
            .unwrap_or_else(|| panic!("a row naming no owner this test knows: {row}"));
        for key in cells[1].split('`').skip(1).step_by(2) {
            owners.push((key.to_owned(), owner.to_owned()));
        }
    }
    owners
}

/// A value of the shape `key` holds, told apart by which end `end` holds it: the engine reads
/// the asset record it keeps, so that one is an asset record rather than a string.
fn reserved_value(key: &str, end: &str) -> Value {
    if key == "onetaskgraph.assets" {
        return json!({format!("{end}.png"): {
            "sha256": "a".repeat(64),
            "url": format!("https://{end}.example/{end}.png"),
        }});
    }
    // An intent to create is an object of intents by destination, one for a destination this
    // copy does not write to, so the copy reads it as one and leaves it where it is.
    if key == "onetaskgraph.creating" {
        return json!({format!("{end}-elsewhere"): {
            "key": "11111111-2222-5333-8444-555555555555", "at": "2026-01-01T00:00:00Z",
            "attempt": "00000000-0000-4000-8000-000000000001",
            "lease_until": "2026-01-01T00:00:00Z",
        }});
    }
    json!(format!("{key} at the {end}"))
}

#[tokio::test]
async fn every_reserved_key_has_the_owner_docs_metadata_states_on_an_update() {
    // The drift gate of that table: each row is put to a real updating copy, so the page and
    // the engine cannot disagree about a key without this failing. A key the destination owns
    // is held at both ends and must keep the destination's value; one the source owns is
    // held at the destination alone and must be dropped; one neither owns is held at both
    // and must be in neither's metadata; and the copy's own is the id it copied from.
    let owners = reserved_key_owners();
    assert!(owners.len() >= 11, "the table was read whole: {owners:?}");
    let mut source = serde_json::Map::new();
    let mut destination = serde_json::Map::new();
    destination.insert(GlobalId::ORIGIN_KEY.to_owned(), json!("from:T-1"));
    // A lineage the destination holds from elsewhere: the copy owns the key, so it is not kept.
    destination.insert(GlobalId::LINEAGE_KEY.to_owned(), json!("elsewhere:T-0"));
    for (key, owner) in &owners {
        match owner.as_str() {
            // The copy's record of itself is held at both ends, and neither end's is kept.
            "the copy" if key.starts_with("onetaskgraph.copied_") => {
                source.insert(key.clone(), reserved_value(key, "source"));
                destination.insert(key.clone(), reserved_value(key, "destination"));
            }
            "the destination" | "neither" => {
                source.insert(key.clone(), reserved_value(key, "source"));
                destination.insert(key.clone(), reserved_value(key, "destination"));
            }
            "the source" => {
                destination.insert(key.clone(), reserved_value(key, "destination"));
            }
            _ => {}
        }
    }
    let status = json!({"category": "todo", "name": "Todo"});
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [{"id": "T-1",
            "title": "Renamed", "status": status, "labels": [], "metadata": source}]}},
        "into": {"plugin": "in-memory", "config": {"tasks": [{"id": "T-1",
            "title": "As it was", "status": status, "labels": [], "metadata": destination}]}},
    }));

    let copied = engine.copy(&one("from:T-1")).await.expect("the copy runs");
    assert_eq!(
        landed(&copied.items[0]),
        (Some("into:T-1".to_owned()), "updated".to_owned())
    );
    let metadata = engine
        .task(&id("into:T-1"))
        .await
        .expect("the show verb answers")
        .items[0]
        .item
        .metadata
        .clone();
    for (key, owner) in &owners {
        let expected = match owner.as_str() {
            // The record this copy wrote: no `copied_with_comments`, because this copy carried
            // none, but a digest of what it left — or the token an in-memory source derives —
            // and the source's own token, or a digest of what it composed of the source.
            "the copy"
                if key == MetadataKey::COPIED_DESTINATION_VERSION_KEY
                    || key == MetadataKey::COPIED_VERSION_KEY =>
            {
                let held = metadata
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    held.starts_with("sha256:") || held.starts_with("mem1|"),
                    "{key}: {held:?}"
                );
                Some(json!(held))
            }
            "the copy" if key.starts_with("onetaskgraph.copied_") => None,
            // The source records no origin, so the origin this copy records is the root and
            // no lineage is recorded beside it.
            "the copy" if key == GlobalId::LINEAGE_KEY => None,
            "the copy" => Some(json!("from:T-1")),
            "the destination" => Some(reserved_value(key, "destination")),
            _ => None,
        };
        assert_eq!(
            metadata.get(key),
            expected.as_ref(),
            "docs/metadata.md says {owner} owns {key} on an update"
        );
    }
}

#[tokio::test]
async fn a_rust_caller_copying_back_leaves_the_destination_its_own_origin() {
    // The write-back a settled run makes, as the Rust caller that links this crate makes
    // it: a plan authored in one store, copied onto a second, projected into a run-owned
    // third, and copied back. The second is the original in that last copy, so its own
    // provenance is not the run's to overwrite — and if it were, the next copy from the
    // store the plan was authored in would match nothing and create a second plan.
    let engine = engine_over(json!({
        "authoring": {"plugin": "in-memory", "config": {"tasks": [task("T-1", "Alpha engine")]}},
        "plans": {"plugin": "in-memory", "config": {}},
        "run": {"plugin": "in-memory", "config": {"tasks": [{
            "id": "T-1", "title": "Alpha engine, settled",
            "content": "the engine core",
            "status": {"category": "todo", "name": "Todo"},
            "labels": [{"id": "L-1", "name": "bug"}],
            "metadata": {
                "caller.shape": {"nested": [1, true, null]},
                GlobalId::ORIGIN_KEY: "plans:T-1",
            },
            "repositories": ["github.com/nickderobertis/onetaskgraph"],
        }]}},
    }));
    let into = |destination: &str, item: &str| CopyRequest {
        destination: name(destination),
        ..one(item)
    };
    /// The origin one destination item records, or `Value::Null` when it records none.
    async fn origin(engine: &Engine, item: &str) -> Value {
        engine
            .task(&id(item))
            .await
            .expect("the show verb answers")
            .items[0]
            .item
            .metadata
            .get(GlobalId::ORIGIN_KEY)
            .cloned()
            .unwrap_or(Value::Null)
    }

    let forward = engine
        .copy(&into("plans", "authoring:T-1"))
        .await
        .expect("the copy runs");
    assert_eq!(
        landed(&forward.items[0]),
        (Some("plans:T-1".to_owned()), "created".to_owned())
    );
    assert_eq!(origin(&engine, "plans:T-1").await, json!("authoring:T-1"));

    // The run's own item names the plan it came from, so this copy reaches it by rule 1.
    let back = engine
        .copy(&into("plans", "run:T-1"))
        .await
        .expect("the copy runs");
    assert_eq!(
        landed(&back.items[0]),
        (Some("plans:T-1".to_owned()), "updated".to_owned())
    );
    assert_eq!(
        engine
            .task(&id("plans:T-1"))
            .await
            .expect("the show verb answers")
            .items[0]
            .item
            .title,
        "Alpha engine, settled",
        "the settled title landed"
    );
    assert_eq!(
        origin(&engine, "plans:T-1").await,
        json!("authoring:T-1"),
        "and the plan still says where it itself came from"
    );

    // Projecting the same settled run again is not a write: preserving the origin leaves
    // the destination reading exactly as it already did.
    let repeated = engine
        .copy(&into("plans", "run:T-1"))
        .await
        .expect("the copy runs");
    assert_eq!(
        landed(&repeated.items[0]),
        (Some("plans:T-1".to_owned()), "unchanged".to_owned())
    );

    // And the plan is still readable back the way it was written.
    let again = engine
        .copy(&into("plans", "authoring:T-1"))
        .await
        .expect("the copy runs");
    assert_eq!(
        landed(&again.items[0]),
        (Some("plans:T-1".to_owned()), "updated".to_owned())
    );
    assert_eq!(listed(&engine, "plans").await, vec!["plans:T-1".to_owned()]);
}

#[tokio::test]
async fn a_rust_caller_is_refused_by_a_destination_configured_with_no_write_side() {
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [task("T-1", "Alpha engine")]}},
        "into": {
            "plugin": "in-memory",
            "config": {"capabilities": {"writes": "unsupported"}},
        },
    }));

    let Err(refusal) = engine.copy(&one("from:T-1")).await else {
        panic!("a destination with no write side must refuse");
    };
    assert!(
        matches!(&refusal, EngineError::NotWritable { name, kind }
            if name == "into" && kind == "in-memory"),
        "{refusal:?}"
    );
    let rendered = refusal.to_string();
    assert!(
        rendered.contains("source into cannot be written"),
        "{rendered}"
    );
    assert!(rendered.contains("its plugin is in-memory"), "{rendered}");
}

#[tokio::test]
async fn a_dry_run_reads_everything_and_writes_nothing() {
    let engine = pair();
    let planned = engine
        .copy(&CopyRequest {
            dry_run: true,
            ..one("from:T-1")
        })
        .await
        .expect("the copy runs");
    // Null only for a dry run that would create: there is no id, because nothing was.
    assert_eq!(
        planned.items[0].action,
        CopyAction::Created {
            destination: None,
            via: onetaskgraph_core::NoCounterpart::Created,
            link: None,
            comments: None,
        }
    );
    assert!(listed(&engine, "into").await.is_empty());
}

#[tokio::test]
async fn a_rust_caller_asking_what_only_a_task_copy_can_do_of_another_is_refused_first() {
    // The command line offers none of these on a document or a project copy, so a Rust caller
    // is the one that can ask: comments of documents, which hold none, a query naming projects,
    // when a query names tasks, and statuses to narrow a copy of projects or documents by, when
    // only a task copy is narrowed by them. Each is refused before anything is read or written.
    let everything = TaskRequest {
        sources: vec![name("from")],
        filters: onetaskgraph_core::Filters::default(),
        project: onetaskgraph_core::ProjectSelector::Any,
        priorities: Vec::new(),
        commented_since: None,
        metadata: Vec::new(),
        origin: None,
        include_members: false,
        paging: Paging {
            limit: NonZeroU32::new(50).expect("a non-zero limit"),
            token: None,
        },
    };
    for (request, refusal) in [
        (
            CopyRequest {
                comments: true,
                ..many(&["from:D-1"], CopyScope::Documents)
            },
            "a document holds no comments",
        ),
        (
            CopyRequest {
                items: CopyItems::matching(everything),
                ..many(
                    &["from:P-1"],
                    CopyScope::Projects {
                        tasks: true,
                        documents: false,
                    },
                )
            },
            "only when it copies tasks",
        ),
        (
            CopyRequest {
                statuses: vec![StatusCategory::Todo],
                ..many(
                    &["from:P-1"],
                    CopyScope::Projects {
                        tasks: true,
                        documents: false,
                    },
                )
            },
            "at a status only when it copies tasks",
        ),
        (
            CopyRequest {
                statuses: vec![StatusCategory::Done],
                ..many(&["from:D-1"], CopyScope::Documents)
            },
            "at a status only when it copies tasks",
        ),
    ] {
        let engine = pair();
        let Err(refused) = engine.copy(&request).await else {
            panic!("expected a refusal naming {refusal:?}");
        };
        assert!(
            matches!(
                refused,
                EngineError::CommentsNotDocuments
                    | EngineError::QueryNotTasks
                    | EngineError::StatusesNotTasks
            ),
            "{refused:?}"
        );
        assert!(refused.to_string().contains(refusal), "{refused}");
        assert!(refused.to_string().contains("next:"), "{refused}");
        assert!(listed(&engine, "into").await.is_empty());
    }
}

#[tokio::test]
async fn an_id_that_names_nothing_and_a_destination_nothing_configures_are_both_refused() {
    let engine = pair();

    let Err(missing) = engine.copy(&one("from:absent")).await else {
        panic!("an id naming nothing must refuse");
    };
    assert!(
        matches!(&missing, EngineError::NoSuchItem { id } if id == "from:absent"),
        "{missing:?}"
    );

    let Err(unknown) = engine
        .copy(&CopyRequest {
            destination: name("nowhere"),
            ..one("from:T-1")
        })
        .await
    else {
        panic!("a destination nothing configures must refuse");
    };
    assert!(
        matches!(&unknown, EngineError::UnknownSource { name, .. } if name == "nowhere"),
        "{unknown:?}"
    );
}

#[tokio::test]
async fn a_stale_origin_refuses_until_recreate_says_to_create_instead() {
    // The item names an origin at `into` that `into` does not hold: the counterpart was
    // deleted or moved on purpose, and creating there would duplicate it.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [{
            "id": "T-1", "title": "Alpha engine",
            "status": {"category": "todo", "name": "Todo"}, "labels": [],
            "metadata": {GlobalId::ORIGIN_KEY: "into:GONE"},
        }]}},
        "into": {"plugin": "in-memory", "config": {}},
    }));

    let Err(stale) = engine.copy(&one("from:T-1")).await else {
        panic!("an origin naming nothing at the destination must refuse");
    };
    assert!(
        matches!(&stale, EngineError::StaleOrigin { item, origin }
            if item == "from:T-1" && origin == "into:GONE"),
        "{stale:?}"
    );
    assert!(stale.to_string().contains("--recreate"), "{stale}");
    assert!(listed(&engine, "into").await.is_empty());

    let created = engine
        .copy(&CopyRequest {
            recreate: true,
            ..one("from:T-1")
        })
        .await
        .expect("--recreate falls through to the search rule");
    assert_eq!(
        landed(&created.items[0]),
        (Some("into:T-1".to_owned()), "created".to_owned())
    );
}

#[tokio::test]
async fn a_lost_origin_creates_a_second_item_until_match_by_re_establishes_it() {
    let engine = pair();
    engine.copy(&one("from:T-1")).await.expect("the copy runs");

    // A person edits the destination and removes the origin key: neither rule can find
    // the counterpart any more, so the next copy creates a second item.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [task("T-1", "Alpha engine")]}},
        "into": {"plugin": "in-memory", "config": {"tasks": [task("T-1", "Alpha engine")]}},
    }));
    let duplicated = engine.copy(&one("from:T-1")).await.expect("the copy runs");
    assert_eq!(
        landed(&duplicated.items[0]),
        (Some("into:T-1-2".to_owned()), "created".to_owned())
    );

    // The caller-named escape re-establishes it without hand-editing ids.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [task("T-1", "Alpha engine")]}},
        "into": {"plugin": "in-memory", "config": {"tasks": [task("OTHER", "Alpha engine")]}},
    }));
    let matched = engine
        .copy(&CopyRequest {
            match_by: Some(MatchBy::parse("title")),
            ..one("from:T-1")
        })
        .await
        .expect("the copy runs");
    assert_eq!(
        landed(&matched.items[0]),
        (Some("into:OTHER".to_owned()), "updated".to_owned())
    );

    // And on a metadata key of the caller's own choosing, for a title that moved.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [task("T-1", "Alpha engine")]}},
        "into": {"plugin": "in-memory", "config": {"tasks": [task("OTHER", "Renamed")]}},
    }));
    let matched = engine
        .copy(&CopyRequest {
            match_by: Some(MatchBy::parse("caller.shape")),
            ..one("from:T-1")
        })
        .await
        .expect("the copy runs");
    assert_eq!(
        landed(&matched.items[0]),
        (Some("into:OTHER".to_owned()), "updated".to_owned())
    );
}

#[tokio::test]
async fn a_destination_that_cannot_carry_a_key_refuses_the_write_naming_it() {
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [task("T-1", "Alpha engine")]}},
        "into": {
            "plugin": "in-memory",
            "config": {"capabilities": {"unwritable_metadata_keys": ["caller.shape"]}},
        },
    }));

    let Err(refused) = engine.copy(&one("from:T-1")).await else {
        panic!("a destination that cannot carry a key must refuse the write");
    };
    let rendered = refused.to_string();
    assert!(
        rendered.contains("source into could not do it"),
        "{rendered}"
    );
    assert!(rendered.contains("caller.shape"), "{rendered}");
    assert!(listed(&engine, "into").await.is_empty());
}

#[tokio::test]
async fn copying_a_project_carries_its_tasks_and_reports_one_the_source_no_longer_holds() {
    let held = |tasks: Value| {
        json!({"plugin": "in-memory", "config": {
            "projects": [{"id": "P-1", "title": "Engine",
                          "status": {"category": "todo", "name": "Todo"}, "labels": []}],
            "tasks": tasks,
        }})
    };
    let member = |id: &str| {
        json!({"id": id, "title": id, "status": {"category": "todo", "name": "Todo"},
               "labels": [], "project": "P-1"})
    };
    let engine = engine_over(json!({
        "from": held(json!([member("T-1"), member("T-2")])),
        "into": {"plugin": "in-memory", "config": {}},
    }));

    let project = many(
        &["from:P-1"],
        CopyScope::Projects {
            tasks: true,
            documents: false,
        },
    );
    let copied = engine.copy(&project).await.expect("the copy runs");
    assert_eq!(
        copied
            .items
            .iter()
            .map(|outcome| (outcome.source.to_string(), outcome.action.name()))
            .collect::<Vec<_>>(),
        vec![
            ("from:P-1".to_owned(), "created".to_owned()),
            ("from:T-1".to_owned(), "created".to_owned()),
            ("from:T-2".to_owned(), "created".to_owned()),
        ]
    );

    // A second copy matches each task independently and duplicates nothing.
    let again = engine.copy(&project).await.expect("the copy runs");
    assert!(
        again
            .items
            .iter()
            .all(|outcome| outcome.action.name() == "unchanged"),
        "{again:?}"
    );
    assert_eq!(
        listed(&engine, "into").await,
        vec!["into:T-1".to_owned(), "into:T-2".to_owned()]
    );

    // `--no-tasks` copies the project alone.
    let alone = engine
        .copy(&many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: false,
                documents: false,
            },
        ))
        .await
        .expect("the copy runs");
    assert_eq!(alone.items.len(), 1);
    assert_eq!(alone.items[0].source, id("from:P-1"));
}

#[tokio::test]
async fn a_destination_item_the_source_no_longer_holds_is_left_alone_and_reported() {
    // The destination holds the counterpart of a task the source has since dropped. A
    // copy never deletes, so it stays exactly as it is and is reported as orphaned.
    let copied = |native: &str, origin: &str| {
        json!({"id": native, "title": native,
               "status": {"category": "todo", "name": "Todo"}, "labels": [],
               "project": "P-1", "metadata": {GlobalId::ORIGIN_KEY: origin}})
    };
    let project = |native: &str, origin: Value| {
        json!({"id": native, "title": "Engine",
               "status": {"category": "todo", "name": "Todo"}, "labels": [],
               "metadata": {GlobalId::ORIGIN_KEY: origin}})
    };
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "projects": [{"id": "P-1", "title": "Engine",
                          "status": {"category": "todo", "name": "Todo"}, "labels": []}],
            "tasks": [{"id": "T-1", "title": "T-1",
                       "status": {"category": "todo", "name": "Todo"}, "labels": [],
                       "project": "P-1"}],
        }},
        "into": {"plugin": "in-memory", "config": {
            "projects": [project("P-1", json!("from:P-1"))],
            "tasks": [copied("T-1", "from:T-1"), copied("T-2", "from:T-2")],
        }},
    }));

    let report = engine
        .copy(&many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
        .expect("the copy runs");
    let orphan = report
        .items
        .iter()
        .find(|outcome| outcome.action.name() == "orphaned")
        .unwrap_or_else(|| panic!("no orphan was reported: {report:?}"));
    assert_eq!(orphan.source, id("from:T-2"));
    assert_eq!(orphan.destination(), Some(&id("into:T-2")));
    // Left exactly as it is: still there, and still saying what it said.
    let held = engine
        .task(&id("into:T-2"))
        .await
        .expect("the show verb answers");
    assert_eq!(held.items[0].item.title, "T-2");
}

#[tokio::test]
async fn the_edges_a_copy_read_are_written_and_a_far_end_that_leaves_the_set_is_qualified() {
    // Three kinds of far end, in one copy: one inside the copied set, which becomes the
    // destination's own id; one the source holds but the copy did not take, which is
    // qualified to the source it stays in; and one already naming another source, which
    // is left exactly as it is.
    let member = |id: &str| {
        json!({"id": id, "title": id, "status": {"category": "todo", "name": "Todo"},
               "labels": []})
    };
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "tasks": [member("T-1"), member("T-2"), member("T-3")],
            "task_dependencies": [
                {"from": "T-1", "to": "T-2", "kind": "blocks"},
                {"from": "T-1", "to": "T-3", "kind": "related"},
                {"from": {"id": "T-1", "kind": "task"},
                 "to": {"id": "elsewhere:P-9", "kind": "project"}, "kind": "blocks"},
            ],
        }},
        "into": {"plugin": "in-memory", "config": {}},
    }));

    let copied = engine
        .copy(&many(&["from:T-1", "from:T-2"], CopyScope::Tasks))
        .await
        .expect("the copy runs");
    assert_eq!(copied.items.len(), 2);

    let edges = engine
        .task_dependencies(&onetaskgraph_core::DependencyRequest {
            id: id("into:T-1"),
            direction: onetaskgraph_plugin_api::Direction::DependsOn,
            completeness: onetaskgraph_core::Completeness::NamingUnasked,
            paging: Paging {
                limit: NonZeroU32::new(50).expect("a non-zero limit"),
                token: None,
            },
        })
        .await
        .expect("the dependency verb answers");
    let mut ends: Vec<String> = edges
        .items
        .iter()
        .map(|edge| edge.to.id.to_string())
        .collect();
    ends.sort();
    assert_eq!(
        ends,
        vec![
            // The member of the copied set, remapped to the id the destination gave it —
            // and it was written *after* this item, so the second pass is what repaired
            // this edge.
            "into:T-2".to_owned(),
            // The far end that leaves the copied set, and the one that already had.
            "elsewhere:P-9".to_owned(),
            "from:T-3".to_owned(),
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );

    // Copying back the other way unqualifies the far end that names the destination's
    // own source, because that is how a source names its own items.
    let back = engine
        .copy(&CopyRequest {
            destination: name("from"),
            ..one("into:T-1")
        })
        .await
        .expect("the copy runs");
    assert_eq!(back.items[0].destination(), Some(&id("from:T-1")));
    let edges = engine
        .task_dependencies(&onetaskgraph_core::DependencyRequest {
            id: id("from:T-1"),
            direction: onetaskgraph_plugin_api::Direction::DependsOn,
            completeness: onetaskgraph_core::Completeness::NamingUnasked,
            paging: Paging {
                limit: NonZeroU32::new(50).expect("a non-zero limit"),
                token: None,
            },
        })
        .await
        .expect("the dependency verb answers");
    assert!(
        edges.items.iter().any(|edge| edge.to.id == id("from:T-3")),
        "{edges:?}"
    );
}

#[tokio::test]
async fn a_task_copied_on_its_own_is_filed_under_the_destinations_own_counterpart() {
    let filed = json!({"id": "T-1", "title": "Alpha",
                       "status": {"category": "todo", "name": "Todo"},
                       "labels": [], "project": "P-1"});
    let project = |id: &str, origin: Option<&str>| {
        let mut project = json!({"id": id, "title": "Engine",
                                 "status": {"category": "todo", "name": "Todo"}, "labels": []});
        if let Some(origin) = origin {
            project["metadata"] = json!({GlobalId::ORIGIN_KEY: origin});
        }
        project
    };

    // The destination holds the counterpart of the task's own project, so the copied task
    // is filed under *that* rather than under an id of the source's the destination never
    // issued.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "projects": [project("P-1", None)], "tasks": [filed],
        }},
        "into": {"plugin": "in-memory", "config": {
            "projects": [project("LOCAL-7", Some("from:P-1"))],
        }},
    }));
    engine.copy(&one("from:T-1")).await.expect("the copy runs");
    let copied = engine
        .task(&id("into:T-1"))
        .await
        .expect("the show verb answers");
    assert_eq!(
        copied.items[0].item.project,
        Some(onetaskgraph_plugin_api::NativeId::from("LOCAL-7"))
    );

    // With no counterpart there, the source's own opaque id is carried rather than
    // dropped: this engine does not interpret it, and losing it would lose what the
    // source said.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "projects": [project("P-1", None)], "tasks": [filed],
        }},
        "into": {"plugin": "in-memory", "config": {}},
    }));
    engine.copy(&one("from:T-1")).await.expect("the copy runs");
    let copied = engine
        .task(&id("into:T-1"))
        .await
        .expect("the show verb answers");
    assert_eq!(
        copied.items[0].item.project,
        Some(onetaskgraph_plugin_api::NativeId::from("P-1"))
    );
}

/// `from:P-1`, whose copy in `home` is the plan's home, and `into:M`, the member that home
/// names in `into`.
fn plan_with_a_member() -> Engine {
    plan_with_a_linked(
        json!({GlobalId::ORIGIN_KEY: "from:P-1", "onetaskgraph.members": ["into:M"]}),
    )
}

/// `from:P-1`, linked to `home:H` holding `home`, and `into:M` naming `home:H` as its home.
fn plan_with_a_linked(home: Value) -> Engine {
    let todo = json!({"category": "todo", "name": "Todo"});
    engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"projects": [{
            "id": "P-1", "title": "Goal", "status": todo, "labels": [],
            "metadata": {"onetaskgraph.copies": {"home": "home:H"}},
        }]}},
        "home": {"plugin": "in-memory", "config": {"projects": [{
            "id": "H", "title": "Goal", "status": todo, "labels": [],
            "metadata": home,
        }]}},
        "into": {"plugin": "in-memory", "config": {"projects": [{
            "id": "M", "title": "Goal", "status": todo, "labels": [],
            "metadata": {"onetaskgraph.member_of": "home:H"},
        }]}},
    }))
}

/// A Rust caller's project copy takes over the member of the plan its destination already
/// holds, as the command line's does — unless the caller sets `create`, which says there is
/// nothing at the destination to look for and is believed.
#[tokio::test]
async fn a_rust_caller_s_project_copy_takes_over_the_plan_s_member_unless_it_asks_to_create() {
    let request = many(
        &["from:P-1"],
        CopyScope::Projects {
            tasks: false,
            documents: false,
        },
    );
    let engine = plan_with_a_member();
    let copied = engine.copy(&request).await.expect("the copy runs");
    assert_eq!(
        landed(&copied.items[0]),
        (Some("into:M".to_owned()), "updated".to_owned())
    );
    let held = &engine
        .project(&id("into:M"))
        .await
        .expect("the show verb answers")
        .items[0]
        .item;
    assert!(
        !held.metadata.contains_key("onetaskgraph.member_of"),
        "{held:?}"
    );
    assert_eq!(held.metadata[GlobalId::ORIGIN_KEY], json!("from:P-1"));

    let engine = plan_with_a_member();
    let created = engine
        .copy(&CopyRequest {
            create: true,
            ..request
        })
        .await
        .expect("the copy runs");
    let (destination, action) = landed(&created.items[0]);
    assert_eq!(action, "created");
    assert_ne!(destination.as_deref(), Some("into:M"));
    let member = &engine
        .project(&id("into:M"))
        .await
        .expect("the show verb answers")
        .items[0]
        .item;
    assert_eq!(member.metadata["onetaskgraph.member_of"], json!("home:H"));
}

/// A project the copied one links to is the plan's home only when it names that project back
/// and is no member itself; otherwise the member it lists is somebody else's, and a project is
/// created beside it rather than taking it over.
#[tokio::test]
async fn a_linked_project_that_is_not_the_plan_s_home_lends_no_member_to_take_over() {
    for home in [
        json!({GlobalId::ORIGIN_KEY: "from:OTHER", "onetaskgraph.members": ["into:M"]}),
        json!({"onetaskgraph.members": ["into:M"]}),
        json!({
            GlobalId::ORIGIN_KEY: "from:P-1",
            "onetaskgraph.members": ["into:M"],
            "onetaskgraph.member_of": "elsewhere:X",
        }),
    ] {
        let engine = plan_with_a_linked(home.clone());
        let copied = engine
            .copy(&many(
                &["from:P-1"],
                CopyScope::Projects {
                    tasks: false,
                    documents: false,
                },
            ))
            .await
            .expect("the copy runs");
        let (destination, action) = landed(&copied.items[0]);
        assert_eq!(action, "created", "{home}");
        assert_ne!(destination.as_deref(), Some("into:M"), "{home}");
        let member = &engine
            .project(&id("into:M"))
            .await
            .expect("the show verb answers")
            .items[0]
            .item;
        assert_eq!(
            member.metadata["onetaskgraph.member_of"],
            json!("home:H"),
            "{home}"
        );
    }
}

#[tokio::test]
async fn a_project_origin_that_still_names_something_updates_that_project_directly() {
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"projects": [{
            "id": "P-1", "title": "Renamed", "status": {"category": "todo", "name": "Todo"},
            "labels": [], "metadata": {GlobalId::ORIGIN_KEY: "into:BOARD"},
        }]}},
        "into": {"plugin": "in-memory", "config": {"projects": [{
            "id": "BOARD", "title": "Engine", "status": {"category": "todo", "name": "Todo"},
            "labels": [],
        }]}},
    }));

    let copied = engine
        .copy(&many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: false,
                documents: false,
            },
        ))
        .await
        .expect("the copy runs");
    assert_eq!(
        landed(&copied.items[0]),
        (Some("into:BOARD".to_owned()), "updated".to_owned())
    );
    assert_eq!(
        engine
            .project(&id("into:BOARD"))
            .await
            .expect("the show verb answers")
            .items[0]
            .item
            .title,
        "Renamed"
    );
}

#[tokio::test]
async fn a_destination_that_could_not_be_built_and_a_source_that_could_not_be_read_both_refuse() {
    // A source that is configured and did not build is not fatal to a *query* — it lands
    // in that response's errors and the others still answer. A copy is one write into one
    // destination, and half of one is not an answer, so both ends refuse by name.
    let broken = json!({"plugin": "local-md", "config": {"root": "/onetaskgraph/not/a/folder"}});
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [task("T-1", "Alpha engine")]}},
        "into": broken,
    }));
    let Err(unavailable) = engine.copy(&one("from:T-1")).await else {
        panic!("a destination that could not be built must refuse");
    };
    assert!(
        matches!(&unavailable, EngineError::DestinationUnavailable { name, .. } if name == "into"),
        "{unavailable:?}"
    );
    assert!(
        unavailable.to_string().contains("could not be built"),
        "{unavailable}"
    );

    let engine = engine_over(json!({
        "from": broken,
        "into": {"plugin": "in-memory", "config": {}},
    }));
    let Err(unreadable) = engine.copy(&one("from:T-1")).await else {
        panic!("a source that could not be built must refuse");
    };
    assert!(
        matches!(&unreadable, EngineError::SourceRefused { name, .. } if name == "from"),
        "{unreadable:?}"
    );
}

#[tokio::test]
async fn the_scan_that_finds_a_counterpart_walks_the_destination_a_page_at_a_time() {
    // One page at a time and nothing written down, which is the same bound every other
    // compensation in this engine works under — so a destination that serves one row per
    // page still finds the counterpart sitting at the end of it.
    let held = |id: &str, origin: &str| {
        json!({"id": id, "title": id, "status": {"category": "todo", "name": "Todo"},
               "labels": [], "metadata": {GlobalId::ORIGIN_KEY: origin}})
    };
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [task("T-1", "Alpha engine")]}},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"max_page_size": 1},
            "tasks": [
                held("A", "somewhere:1"),
                held("B", "somewhere:2"),
                held("C", "from:T-1"),
            ],
        }},
    }));

    let copied = engine.copy(&one("from:T-1")).await.expect("the copy runs");
    assert_eq!(
        landed(&copied.items[0]),
        (Some("into:C".to_owned()), "updated".to_owned())
    );
}

/// Two projects, with the dependencies that only one copied set can resolve: a task on its
/// sibling, a task on a task in the *other* named project, and a project on that project.
fn interlinked() -> Value {
    json!({"plugin": "in-memory", "config": {
        "projects": [
            {"id": "P-1", "title": "Engine",
             "status": {"category": "todo", "name": "Todo"}, "labels": []},
            {"id": "P-2", "title": "Docs",
             "status": {"category": "todo", "name": "Todo"}, "labels": []},
        ],
        "tasks": [
            {"id": "T-1", "title": "Alpha engine",
             "status": {"category": "todo", "name": "Todo"}, "labels": [], "project": "P-1"},
            {"id": "T-2", "title": "Beta",
             "status": {"category": "todo", "name": "Todo"}, "labels": [], "project": "P-1"},
            {"id": "T-3", "title": "Gamma",
             "status": {"category": "todo", "name": "Todo"}, "labels": [], "project": "P-2"},
        ],
        "task_dependencies": [
            {"from": "T-1", "to": "T-2", "kind": "blocks"},
            {"from": "T-1", "to": "T-3", "kind": "blocks"},
        ],
        "project_dependencies": [
            {"from": "P-1", "to": "P-2", "kind": "blocks"},
        ],
    }})
}

/// Every forward edge at one item, as `<far id> <kind>` pairs.
async fn depends_on(engine: &Engine, near: &str) -> Vec<String> {
    let response = engine
        .task_dependencies(&DependencyRequest {
            id: id(near),
            direction: Direction::DependsOn,
            completeness: onetaskgraph_core::Completeness::NamingUnasked,
            paging: Paging {
                limit: NonZeroU32::new(50).expect("a non-zero limit"),
                token: None,
            },
        })
        .await
        .expect("the dependency verb answers");
    assert!(
        response.errors.is_empty(),
        "a dependency read must not fail: {:?}",
        response.errors
    );
    response
        .items
        .into_iter()
        .map(|edge| format!("{} {:?}", edge.to.id, edge.to.kind))
        .collect()
}

#[tokio::test]
async fn a_copy_resolves_a_dependency_on_an_item_it_created_in_the_same_run() {
    // The defect: a copy could not see the items it had itself created. Every project was
    // copied on its own, so a task's edge to a sibling in *another* named project, and a
    // task's edge to the project it belongs to, were both written as the id the far end
    // had at its **source** — a reference the destination has never heard of — or refused
    // outright by a destination that checks its far ends, naming an item that same run had
    // just created.
    let engine = engine_over(json!({
        "from": interlinked(),
        "into": {"plugin": "in-memory", "config": {}},
    }));

    let report = engine
        .copy(&many(
            &["from:P-1", "from:P-2"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
        .expect("the copy runs");
    assert!(
        report
            .items
            .iter()
            .all(|outcome| outcome.action.name() == "created"),
        "{report:?}"
    );

    // Every edge points at the destination's own item, by the destination's own id.
    assert_eq!(
        depends_on(&engine, "into:T-1").await,
        vec!["into:T-2 Task".to_owned(), "into:T-3 Task".to_owned()]
    );
    // Including the project's own edge to the other project of the same copy.
    let projects = engine
        .project_dependencies(&DependencyRequest {
            id: id("into:P-1"),
            direction: Direction::DependsOn,
            completeness: onetaskgraph_core::Completeness::NamingUnasked,
            paging: Paging {
                limit: NonZeroU32::new(50).expect("a non-zero limit"),
                token: None,
            },
        })
        .await
        .expect("the dependency verb answers");
    assert_eq!(
        projects
            .items
            .iter()
            .map(|edge| edge.to.id.to_string())
            .collect::<Vec<_>>(),
        vec!["into:P-2".to_owned()]
    );
}

#[tokio::test]
async fn a_copy_that_cannot_finish_leaves_the_destination_as_it_found_it() {
    // A copy is either complete or it never happened. A half-written project has to be run
    // again, and the re-run is the mutation burst that trips a hosted destination's
    // secondary rate limiter — so undoing this run's own writes is what removes the retry
    // at source. `Beta` is the item this destination will not create, and it is the second
    // task of the project, so the project and the first task have already landed when it
    // refuses.
    let engine = engine_over(json!({
        "from": interlinked(),
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"uncreatable_titles": ["Beta"]},
        }},
    }));

    let Err(refused) = engine
        .copy(&many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
    else {
        panic!("a destination that will not create an item must refuse the copy");
    };
    let rendered = refused.to_string();
    assert!(rendered.contains("Beta"), "{rendered}");
    assert!(
        !rendered.contains("could not be undone"),
        "the destination can be put back, so the copy must not report otherwise: {rendered}"
    );

    // The destination holds none of that copy's items — not the project written first, and
    // not the task that landed before the refusal.
    assert!(listed(&engine, "into").await.is_empty());
    let projects = engine
        .projects(&onetaskgraph_core::ProjectRequest {
            sources: vec![name("into")],
            filters: onetaskgraph_core::Filters::default(),
            paging: Paging {
                limit: NonZeroU32::new(50).expect("a non-zero limit"),
                token: None,
            },
        })
        .await
        .expect("the project list answers");
    assert!(projects.items.is_empty(), "{:?}", projects.items);
}

#[tokio::test]
async fn a_copy_that_cannot_be_undone_names_what_it_left_behind() {
    // Undoing is best effort, and a destination that will not take one of its own items
    // back leaves work the copy owes the user the name of. Told only that the copy failed,
    // they would copy again over a destination nobody has described to them — which is the
    // retry this whole mechanism exists to remove. So the refusal carries both halves: why
    // the copy failed, why it could not be undone, and what is still there.
    let engine = engine_over(json!({
        "from": interlinked(),
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {
                "uncreatable_titles": ["Beta"],
                "undeletable_ids": ["P-1"],
            },
        }},
    }));

    let Err(refused) = engine
        .copy(&many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
    else {
        panic!("the copy must refuse");
    };
    let rendered = refused.to_string();
    assert!(rendered.contains("could not be undone"), "{rendered}");
    // Why it failed, why the undo failed, and the qualified id still sitting there.
    assert!(rendered.contains("Beta"), "{rendered}");
    assert!(rendered.contains("will not remove P-1"), "{rendered}");
    assert!(rendered.contains("into:P-1"), "{rendered}");

    // And it is telling the truth: the project it names is there, and the task it managed
    // to take back is not.
    let projects = engine
        .projects(&onetaskgraph_core::ProjectRequest {
            sources: vec![name("into")],
            filters: onetaskgraph_core::Filters::default(),
            paging: Paging {
                limit: NonZeroU32::new(50).expect("a non-zero limit"),
                token: None,
            },
        })
        .await
        .expect("the project list answers");
    assert_eq!(
        projects
            .items
            .iter()
            .map(|project| project.id.to_string())
            .collect::<Vec<_>>(),
        vec!["into:P-1".to_owned()]
    );
    assert!(listed(&engine, "into").await.is_empty());
}

/// One destination item recorded as the counterpart of `origin`, reading differently from
/// the source so a copy of it is a real update rather than an `unchanged`.
fn counterpart(id: &str, origin: &str, project: Option<&str>) -> Value {
    let mut item = json!({
        "id": id,
        "title": format!("{id} as it was"),
        "content": "as it was",
        "status": {"category": "todo", "name": "Todo"},
        "labels": [],
        "metadata": {GlobalId::ORIGIN_KEY: origin},
    });
    if let Some(project) = project {
        item["project"] = json!(project);
    }
    item
}

/// Every task and project one source holds, as `<id> <title>` pairs.
async fn held(engine: &Engine, source: &str) -> Vec<String> {
    let paging = || Paging {
        limit: NonZeroU32::new(50).expect("a non-zero limit"),
        token: None,
    };
    let projects = engine
        .projects(&onetaskgraph_core::ProjectRequest {
            sources: vec![name(source)],
            filters: onetaskgraph_core::Filters::default(),
            paging: paging(),
        })
        .await
        .expect("the project list answers");
    let tasks = engine
        .tasks(&TaskRequest {
            sources: vec![name(source)],
            filters: onetaskgraph_core::Filters::default(),
            project: onetaskgraph_core::ProjectSelector::Any,
            priorities: Vec::new(),
            commented_since: None,
            metadata: Vec::new(),
            origin: None,
            include_members: false,
            paging: paging(),
        })
        .await
        .expect("the task list answers");
    projects
        .items
        .into_iter()
        .map(|project| format!("{} {}", project.id, project.item.title))
        .chain(
            tasks
                .items
                .into_iter()
                .map(|task| format!("{} {}", task.id, task.item.title)),
        )
        .collect()
}

/// A destination already holding a counterpart of every item of [`interlinked`] but `T-3`.
fn already_holding() -> Value {
    json!({
        "projects": [
            counterpart("D-P1", "from:P-1", None),
            counterpart("D-P2", "from:P-2", None),
        ],
        "tasks": [
            counterpart("D-T1", "from:T-1", Some("D-P1")),
            counterpart("D-T2", "from:T-2", Some("D-P1")),
        ],
    })
}

#[tokio::test]
async fn a_second_copy_updates_every_counterpart_and_repairs_the_edges_among_them() {
    // The destination already holds a counterpart of everything but `T-3`, recorded by
    // origin the way an earlier copy left it and reading differently from the source. So
    // every one of them is a real update, and `P-1` and `T-1` are written twice — once as
    // they land, once when the edges whose far ends did not exist yet are repaired.
    let engine = engine_over(json!({
        "from": interlinked(),
        "into": {"plugin": "in-memory", "config": already_holding()},
    }));

    let report = engine
        .copy(&many(
            &["from:P-1", "from:P-2"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
        .expect("the copy runs");
    assert_eq!(
        report
            .items
            .iter()
            .map(|outcome| (outcome.source.to_string(), outcome.action.name()))
            .collect::<Vec<_>>(),
        vec![
            ("from:P-1".to_owned(), "updated".to_owned()),
            ("from:T-1".to_owned(), "updated".to_owned()),
            ("from:T-2".to_owned(), "updated".to_owned()),
            ("from:P-2".to_owned(), "updated".to_owned()),
            ("from:T-3".to_owned(), "created".to_owned()),
        ]
    );

    // Every edge names the destination's own item, including the one whose far end was
    // created in a project this copy reached after the item that points at it.
    assert_eq!(
        depends_on(&engine, "into:D-T1").await,
        vec!["into:D-T2 Task".to_owned(), "into:T-3 Task".to_owned()]
    );
}

/// One project of three tasks, authored the way a write-back authors its shadow: the
/// project and `T-1` record the destination item each corresponds to, and `T-3` records
/// none. `T-2` depends on `T-1`, or on `T-3` when `on_unrecorded` says so.
fn shadow(on_unrecorded: bool) -> Value {
    let recorded = |id: &str, title: &str, origin: Option<&str>| {
        let mut task = json!({"id": id, "title": title,
            "status": {"category": "todo", "name": "Todo"}, "labels": [], "project": "P-1"});
        if let Some(origin) = origin {
            task["metadata"] = json!({GlobalId::ORIGIN_KEY: origin});
        }
        task
    };
    json!({"plugin": "in-memory", "config": {
        "projects": [
            {"id": "P-1", "title": "Engine", "status": {"category": "todo", "name": "Todo"},
             "labels": [], "metadata": {GlobalId::ORIGIN_KEY: "into:D-P1"}},
        ],
        "tasks": [
            recorded("T-1", "Alpha engine", Some("into:D-T1")),
            recorded("T-2", "Beta", None),
            recorded("T-3", "Gamma", None),
        ],
        "task_dependencies": [
            {"from": "T-2", "to": if on_unrecorded { "T-3" } else { "T-1" }, "kind": "blocks"},
        ],
    }})
}

/// A destination holding a counterpart of the project, `T-1` and `T-3` — each reading
/// differently from the source, so a copy that reached one would write it.
fn holding_the_shadow() -> Value {
    json!({
        "projects": [counterpart("D-P1", "from:P-1", None)],
        "tasks": [
            counterpart("D-T1", "from:T-1", Some("D-P1")),
            counterpart("D-T3", "from:T-3", Some("D-P1")),
        ],
    })
}

/// A member copy of `from:P-1` naming these members.
fn members(named: &[&str]) -> CopyRequest {
    many(
        &["from:P-1"],
        CopyScope::Members(
            CopyItems::new(named.iter().map(|member| id(member)).collect())
                .expect("a member copy names a member"),
        ),
    )
}

#[tokio::test]
async fn a_rust_caller_copies_a_project_and_exactly_the_members_it_names() {
    let engine = engine_over(json!({
        "from": shadow(false),
        "into": {"plugin": "in-memory", "config": holding_the_shadow()},
    }));

    let report = engine
        .copy(&members(&["from:T-2"]))
        .await
        .expect("the member copy runs");
    assert_eq!(
        report.items.iter().map(landed).collect::<Vec<_>>(),
        vec![
            (Some("into:D-P1".to_owned()), "updated".to_owned()),
            (Some("into:T-2".to_owned()), "created".to_owned()),
        ],
        "the project, then the one member named — created, because nothing held it"
    );
    assert_eq!(
        depends_on(&engine, "into:T-2").await,
        vec!["into:D-T1 Task".to_owned()],
        "the edge to a member the copy does not carry names the id that member records"
    );
    let held = held(&engine, "into").await;
    for untouched in ["into:D-T1 D-T1 as it was", "into:D-T3 D-T3 as it was"] {
        assert!(
            held.contains(&untouched.to_owned()),
            "a member the copy was not told about was written: {held:?}"
        );
    }
}

#[tokio::test]
async fn a_rust_caller_is_refused_a_member_copy_before_anything_is_written() {
    let refused_with = |engine: Engine, request: CopyRequest| async move {
        let before = held(&engine, "into").await;
        let error = engine
            .copy(&request)
            .await
            .expect_err("the member copy is refused");
        assert_eq!(
            held(&engine, "into").await,
            before,
            "a refused member copy wrote something"
        );
        error
    };

    let error = refused_with(
        engine_over(json!({
            "from": shadow(false),
            "into": {"plugin": "in-memory", "config": holding_the_shadow()},
        })),
        members(&["from:T-2", "from:T-9"]),
    )
    .await;
    assert_eq!(
        error,
        EngineError::NotAMember {
            id: id("from:T-9"),
            projects: vec![id("from:P-1")],
        }
    );

    let error = refused_with(
        engine_over(json!({
            "from": shadow(true),
            "into": {"plugin": "in-memory", "config": holding_the_shadow()},
        })),
        members(&["from:T-2"]),
    )
    .await;
    assert_eq!(
        error,
        EngineError::UnrecordedMember {
            item: id("from:T-2"),
            member: id("from:T-3"),
            destination: name("into"),
        }
    );
}

/// One reading a [`Reporting`] source gives, in the order it is asked for them.
type Reading = Result<Option<onetaskgraph_plugin_api::Metering>, SourceError>;

/// A source that answers exactly as the `in-memory` source it wraps, and says it metered
/// whatever its script says — one reading each time it is asked, then none.
struct Reporting {
    inner: Box<dyn TaskSource>,
    script: std::sync::Mutex<std::collections::VecDeque<Reading>>,
}

#[async_trait::async_trait]
impl TaskSource for Reporting {
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
        self.inner.query_tasks(query, page).await
    }

    async fn query_projects(
        &self,
        query: &ProjectQuery,
        page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        self.inner.query_projects(query, page).await
    }

    async fn query_documents(
        &self,
        query: &DocumentQuery,
        page: &PageRequest,
    ) -> Result<Page<Document>, SourceError> {
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

    async fn delete_task(&self, id: &NativeId) -> Result<(), SourceError> {
        self.inner.delete_task(id).await
    }

    async fn delete_project(&self, id: &NativeId) -> Result<(), SourceError> {
        self.inner.delete_project(id).await
    }

    async fn delete_document(&self, id: &NativeId) -> Result<(), SourceError> {
        self.inner.delete_document(id).await
    }

    async fn metering(&self) -> Reading {
        self.script
            .lock()
            .expect("an unpoisoned script")
            .pop_front()
            .unwrap_or(Ok(None))
    }
}

/// A running total of `requests`, having spent `(budget, unit, measured, modelled)`.
fn total(requests: u64, budgets: &[(&str, &str, u64, u64)]) -> Reading {
    Ok(Some(onetaskgraph_plugin_api::Metering {
        requests,
        budgets: budgets
            .iter()
            .map(
                |&(budget, unit, measured, modelled)| onetaskgraph_plugin_api::Metered {
                    budget: budget.to_owned(),
                    unit: unit.to_owned(),
                    measured,
                    modelled,
                },
            )
            .collect(),
    }))
}

/// A reading the source could not take.
fn unreadable() -> Reading {
    Err(SourceError::Malformed {
        message: "the reading could not be taken".to_owned(),
    })
}

/// What a copy of `from:T-1` into `into` says it spent, as JSON, with each source giving the
/// two readings its script holds — the one before the copy and the one after it.
async fn spent_with(from: Vec<Reading>, into: Vec<Reading>) -> Value {
    let reporting = |source: &str, config: Value, script: Vec<Reading>| {
        let inner = onetaskgraph_in_memory::Plugin
            .build(&name(source), &config, &NoSecrets)
            .expect("the in-memory plugin builds");
        ConfiguredSource::Ready(ResolvedSource::adopt(
            name(source),
            Box::new(Reporting {
                inner,
                script: std::sync::Mutex::new(script.into()),
            }),
        ))
    };
    let engine = Engine::new(
        vec![
            reporting(
                "from",
                json!({"tasks": [task("T-1", "Alpha engine")]}),
                from,
            ),
            reporting("into", json!({}), into),
        ],
        vec![name("from"), name("into")],
    );
    let report = engine.copy(&one("from:T-1")).await.expect("the copy runs");
    assert_eq!(
        report.items.iter().map(landed).collect::<Vec<_>>(),
        vec![(Some("into:T-1".to_owned()), "created".to_owned())],
        "what a copy spent never changes what it does"
    );
    serde_json::to_value(&report.spent).expect("a copy report serialises")
}

#[tokio::test]
async fn a_rust_caller_reads_what_a_copy_spent_summed_over_the_sources_that_meter() {
    let spent = spent_with(
        vec![
            total(
                10,
                &[("graphql", "points", 5, 0), ("rest", "requests", 2, 0)],
            ),
            total(
                13,
                &[("graphql", "points", 5, 3), ("rest", "requests", 4, 0)],
            ),
        ],
        vec![total(0, &[]), total(4, &[("graphql", "points", 7, 0)])],
    )
    .await;
    assert_eq!(
        spent,
        json!({"requests": 7, "budgets": [
            {"budget": "graphql", "unit": "points", "amount": 10, "lower_bound": true},
            {"budget": "rest", "unit": "requests", "amount": 2, "lower_bound": false},
        ]}),
        "one budget two sources name adds up, measured and modelled together, and is a lower \
         bound because part of it was modelled; a budget nothing modelled is not"
    );
}

#[test]
fn what_a_copy_spent_survives_the_public_reports_wire_round_trip() {
    let report = CopyReport {
        items: Vec::new(),
        references_rewritten: 0,
        references_unresolved: 0,
        references_ambiguous: 0,
        spent: Some(Spent {
            requests: 7,
            budgets: vec![BudgetSpent {
                budget: "graphql".to_owned(),
                unit: "points".to_owned(),
                amount: 12,
                lower_bound: true,
            }],
        }),
        delivers_rewritten: 0,
        delivered: Vec::new(),
    };

    let wire = serde_json::to_value(&report).expect("a populated copy report serialises");
    assert_eq!(
        wire["spent"],
        json!({
            "requests": 7,
            "budgets": [{
                "budget": "graphql",
                "unit": "points",
                "amount": 12,
                "lower_bound": true,
            }],
        })
    );
    let read: CopyReport = serde_json::from_value(wire).expect("the report reads back");
    assert_eq!(read, report);
}

#[tokio::test]
async fn a_source_whose_readings_are_not_a_running_total_is_reported_as_not_metering() {
    let counted = || vec![total(0, &[]), total(4, &[("graphql", "points", 7, 0)])];
    let the_destination_alone = json!({"requests": 4, "budgets": [
        {"budget": "graphql", "unit": "points", "amount": 7, "lower_bound": false},
    ]});
    for (what, from) in [
        (
            "first reading failed",
            vec![unreadable(), total(3, &[("graphql", "points", 3, 0)])],
        ),
        ("second reading failed", vec![total(3, &[]), unreadable()]),
        // Its budget rose, so a clamped difference would still add to the total.
        (
            "requests fell",
            vec![
                total(10, &[("graphql", "points", 1, 0)]),
                total(3, &[("graphql", "points", 2, 0)]),
            ],
        ),
        (
            "budget fell",
            vec![
                total(1, &[("graphql", "points", 9, 0)]),
                total(2, &[("graphql", "points", 4, 0)]),
            ],
        ),
        (
            "budget vanished",
            vec![total(1, &[("rest", "requests", 1, 0)]), total(2, &[])],
        ),
        (
            "budget was named twice",
            vec![
                total(1, &[]),
                total(
                    2,
                    &[("graphql", "points", 1, 0), ("graphql", "points", 1, 0)],
                ),
            ],
        ),
        (
            "budget had no name",
            vec![total(1, &[]), total(2, &[("", "points", 1, 0)])],
        ),
    ] {
        assert_eq!(
            spent_with(from, counted()).await,
            the_destination_alone,
            "a source whose {what} was added to what the copy spent"
        );
    }
    assert_eq!(
        spent_with(
            vec![unreadable(), total(1, &[])],
            vec![total(5, &[]), total(2, &[])]
        )
        .await,
        Value::Null,
        "a copy none of whose sources gave two readings that stand says nothing it spent"
    );
}

#[tokio::test]
async fn a_copy_that_cannot_finish_puts_back_the_items_it_overwrote() {
    // Undoing is not only about the items a copy created. The four counterparts here were
    // at the destination before this copy started and are overwritten by it, and `Gamma`
    // is the item this destination will not create — so the copy refuses after four
    // successful writes, and every one of those four has to read as it did before rather
    // than as this copy's first pass left it.
    let mut into = already_holding();
    into["capabilities"] = json!({"uncreatable_titles": ["Gamma"]});
    let engine = engine_over(json!({
        "from": interlinked(),
        "into": {"plugin": "in-memory", "config": into},
    }));
    let before = held(&engine, "into").await;
    assert_eq!(
        before,
        vec![
            "into:D-P1 D-P1 as it was".to_owned(),
            "into:D-P2 D-P2 as it was".to_owned(),
            "into:D-T1 D-T1 as it was".to_owned(),
            "into:D-T2 D-T2 as it was".to_owned(),
        ]
    );

    let Err(refused) = engine
        .copy(&many(
            &["from:P-1", "from:P-2"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
    else {
        panic!("a destination that will not create an item must refuse the copy");
    };
    assert!(refused.to_string().contains("Gamma"), "{refused}");
    assert!(
        !refused.to_string().contains("could not be undone"),
        "this destination takes its items back: {refused}"
    );

    assert_eq!(
        held(&engine, "into").await,
        before,
        "every item this copy overwrote reads as it did before it started"
    );
}

/// One source project of two tasks, the second of which no destination here will create.
fn one_project_of_two_tasks() -> Value {
    json!({"plugin": "in-memory", "config": {
        "projects": [
            {"id": "P-1", "title": "Engine",
             "status": {"category": "todo", "name": "Todo"}, "labels": []},
        ],
        "tasks": [
            {"id": "T-1", "title": "Alpha engine",
             "status": {"category": "todo", "name": "Todo"}, "labels": [], "project": "P-1"},
            {"id": "T-9", "title": "Gamma",
             "status": {"category": "todo", "name": "Todo"}, "labels": [], "project": "P-1"},
        ],
    }})
}

/// A destination whose task table and project table each hold something called `SHARED`.
///
/// Nothing makes a destination number its two kinds in one namespace, and nothing stops it
/// either: a Markdown store filing `shared.md` as a task beside `shared.md` as a project is
/// the ordinary case rather than the contrived one. So a destination id says which item is
/// meant only once the kind is beside it.
fn sharing_one_id() -> Value {
    let mut project = counterpart("SHARED", "from:P-1", None);
    project["title"] = json!("the project as it was");
    let mut task = counterpart("SHARED", "from:T-1", Some("SHARED"));
    task["title"] = json!("the task as it was");
    json!({
        "projects": [project],
        "tasks": [task],
        "capabilities": {"uncreatable_titles": ["Gamma"]},
    })
}

#[tokio::test]
async fn an_undo_tells_a_task_from_a_project_sharing_one_destination_id() {
    // Both counterparts are updated by this copy and both are journalled under `SHARED`,
    // so a journal that identifies an entry by id alone reads the second as a repeat of
    // the first and drops it. Then `Gamma` cannot be created, the copy undoes itself, and
    // the entry it dropped is the one item nothing puts back — a destination left holding
    // half of a copy that reported it had left nothing behind.
    let engine = engine_over(json!({
        "from": one_project_of_two_tasks(),
        "into": {"plugin": "in-memory", "config": sharing_one_id()},
    }));
    let before = held(&engine, "into").await;
    assert_eq!(
        before,
        vec![
            "into:SHARED the project as it was".to_owned(),
            "into:SHARED the task as it was".to_owned(),
        ]
    );

    let Err(refused) = engine
        .copy(&many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
    else {
        panic!("a destination that will not create an item must refuse the copy");
    };
    assert!(refused.to_string().contains("Gamma"), "{refused}");
    assert!(
        !refused.to_string().contains("could not be undone"),
        "this destination takes its items back: {refused}"
    );

    assert_eq!(
        held(&engine, "into").await,
        before,
        "both items sharing that id are put back, not whichever of them was journalled first"
    );
}

/// A source whose task carries the id the destination already files a project under.
fn a_task_named_like_the_destinations_project() -> Value {
    json!({"plugin": "in-memory", "config": {
        "projects": [
            {"id": "P-1", "title": "Engine",
             "status": {"category": "todo", "name": "Todo"}, "labels": []},
        ],
        "tasks": [
            {"id": "SHARED", "title": "Alpha engine",
             "status": {"category": "todo", "name": "Todo"}, "labels": [], "project": "P-1"},
            {"id": "T-9", "title": "Gamma",
             "status": {"category": "todo", "name": "Todo"}, "labels": [], "project": "P-1"},
        ],
    }})
}

/// A destination holding only the project `SHARED`, and refusing to create `Gamma`.
fn holding_only_the_project() -> Value {
    let mut project = counterpart("SHARED", "from:P-1", None);
    project["title"] = json!("the project as it was");
    json!({
        "projects": [project],
        "capabilities": {"uncreatable_titles": ["Gamma"]},
    })
}

#[tokio::test]
async fn an_item_created_under_one_kind_does_not_hold_back_the_others_restore() {
    // The far side of the same confusion. An undo removes what this copy created rather
    // than restoring it, so every created id is one the restores must skip — and this copy
    // creates a *task* called `SHARED` while updating a *project* that was called `SHARED`
    // before it started. Skipped by id alone, the project is left reading as this copy
    // wrote it: the one item a "nothing was left behind" refusal did leave behind.
    let engine = engine_over(json!({
        "from": a_task_named_like_the_destinations_project(),
        "into": {"plugin": "in-memory", "config": holding_only_the_project()},
    }));
    let before = held(&engine, "into").await;
    assert_eq!(before, vec!["into:SHARED the project as it was".to_owned()]);

    let Err(refused) = engine
        .copy(&many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
    else {
        panic!("a destination that will not create an item must refuse the copy");
    };
    assert!(refused.to_string().contains("Gamma"), "{refused}");
    assert!(
        !refused.to_string().contains("could not be undone"),
        "this destination takes its items back: {refused}"
    );

    assert_eq!(
        held(&engine, "into").await,
        before,
        "the project is restored, and the task this copy created under its id is gone"
    );
}

#[tokio::test]
async fn a_copy_that_stops_part_way_through_an_update_puts_that_item_back_too() {
    // The other half of undoing an overwrite. A destination's own write is several calls —
    // `docs/plugin-protocol.md` §4.9, and the GitHub source's own suite drives one failing
    // after an earlier one landed — so an update can end with the item already changed. No
    // source can put that back: only this journal holds what was there. Recorded after a
    // successful write, this was the one way a copy could stop and leave a destination
    // altered, which is exactly what "either complete or it never happened" forbids.
    //
    // `Beta` is the title this destination applies and then refuses, and it is the third
    // of three updates — so the two before it have landed and the third is half written.
    let mut into = already_holding();
    into["capabilities"] = json!({"half_written_titles": ["Beta"]});
    let engine = engine_over(json!({
        "from": interlinked(),
        "into": {"plugin": "in-memory", "config": into},
    }));
    let before = held(&engine, "into").await;
    assert_eq!(
        before,
        vec![
            "into:D-P1 D-P1 as it was".to_owned(),
            "into:D-P2 D-P2 as it was".to_owned(),
            "into:D-T1 D-T1 as it was".to_owned(),
            "into:D-T2 D-T2 as it was".to_owned(),
        ]
    );

    let Err(refused) = engine
        .copy(&many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
    else {
        panic!("a destination that stops part way through a write must refuse the copy");
    };
    let rendered = refused.to_string();
    assert!(rendered.contains("Beta"), "{rendered}");
    assert!(
        !rendered.contains("could not be undone"),
        "this destination takes its items back: {rendered}"
    );

    assert_eq!(
        held(&engine, "into").await,
        before,
        "the item the write had already changed reads as it did before the copy started"
    );
}

#[tokio::test]
async fn a_restore_the_destination_refuses_names_the_item_left_holding_this_copys_writing() {
    // The item a destination will not take back need not be one this copy created. `D-T2`
    // was here before it started, carrying a provenance entry this source will not accept
    // in a write. Provenance is the copied item's own, and the source's `T-2` carries none,
    // so the copy overwrites it happily without the entry and cannot write the original
    // back. `Gamma` then fails, and the undo that follows puts three of the four items back
    // and is refused the fourth.
    //
    // Both halves have to reach the user: told only that the copy failed, they would copy
    // again over a destination holding one item's content from a run nobody described.
    let mut into = already_holding();
    into["tasks"][1]["metadata"]["onetaskgraph.template"] = provenance("/at/the/destination.md");
    into["capabilities"] = json!({
        "uncreatable_titles": ["Gamma"],
        "unwritable_metadata_keys": ["onetaskgraph.template"],
    });
    let engine = engine_over(json!({
        "from": interlinked(),
        "into": {"plugin": "in-memory", "config": into},
    }));

    let Err(refused) = engine
        .copy(&many(
            &["from:P-1", "from:P-2"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
    else {
        panic!("a destination that will not create an item must refuse the copy");
    };

    let EngineError::CopyNotUndone { left_behind, .. } = &refused else {
        panic!("a refused restore must report the copy as not undone: {refused}");
    };
    assert_eq!(
        left_behind
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["into:D-T2".to_owned()],
        "only the item the destination refused is still this copy's"
    );

    let rendered = refused.to_string();
    // Why the copy failed, why the undo failed, and the one item still holding its writing.
    assert!(rendered.contains("Gamma"), "{rendered}");
    assert!(rendered.contains("onetaskgraph.template"), "{rendered}");
    assert!(rendered.contains("into:D-T2"), "{rendered}");

    // And it is telling the truth about which item that is: everything else reads as it
    // did before the copy, and `D-T2` reads as this copy left it.
    assert_eq!(
        held(&engine, "into").await,
        vec![
            "into:D-P1 D-P1 as it was".to_owned(),
            "into:D-P2 D-P2 as it was".to_owned(),
            "into:D-T1 D-T1 as it was".to_owned(),
            "into:D-T2 Beta".to_owned(),
        ]
    );
}

/// One document, held by an `in-memory` source, carrying caller-defined metadata of
/// several JSON types and a location of its own.
fn a_document(id: &str, title: &str) -> Value {
    json!({
        "id": id,
        "title": title,
        "content": "why the store holds a document",
        "labels": [{"id": "L-1", "name": "spec"}],
        "project": null,
        "location": {"path": "/srv/notes/D-1.md"},
        "metadata": {"caller.shape": {"nested": [1, true, null]}, "onepipeline.turn_budget": 12},
        "repositories": ["github.com/nickderobertis/onetaskgraph"]
    })
}

/// Two document-bearing in-memory sources: one holding `D-1`, one empty and writable.
fn document_pair() -> Engine {
    engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "documents": [a_document("D-1", "Design review")],
        }},
        "into": {"plugin": "in-memory", "config": {"capabilities": {"documents": "native"}}},
    }))
}

#[tokio::test]
async fn a_document_copies_into_another_source_whole_and_a_second_copy_updates_it() {
    let engine = document_pair();

    let first = engine
        .copy(&many(&["from:D-1"], CopyScope::Documents))
        .await
        .expect("a document-bearing destination takes a document");
    assert_eq!(
        first.items.iter().map(landed).collect::<Vec<_>>(),
        [(Some("into:D-1".to_owned()), "created".to_owned())]
    );

    let landed_document = engine
        .document(&id("into:D-1"))
        .await
        .expect("the destination is configured");
    let item = &landed_document.items[0].item;
    assert_eq!(item.title, "Design review");
    assert_eq!(
        item.content.as_deref(),
        Some("why the store holds a document")
    );
    assert_eq!(item.labels[0].name, "spec");
    // Every caller-defined key, with its JSON types intact, plus the origin the copy
    // records so a second copy finds this one rather than adding another.
    assert_eq!(
        item.metadata["caller.shape"],
        json!({"nested": [1, true, null]})
    );
    assert_eq!(item.metadata["onepipeline.turn_budget"], json!(12));
    assert_eq!(item.metadata[GlobalId::ORIGIN_KEY], json!("from:D-1"));
    assert_eq!(
        item.repositories
            .iter()
            .map(|repository| repository.as_str().to_owned())
            .collect::<Vec<_>>(),
        ["github.com/nickderobertis/onetaskgraph"]
    );
    // Where the *source* holds a document says nothing about where the destination does,
    // so the location is the destination's own and is never written.
    assert_eq!(item.location, None);

    let second = engine
        .copy(&many(&["from:D-1"], CopyScope::Documents))
        .await
        .expect("a second copy is an update, not a duplicate");
    assert_eq!(
        second.items.iter().map(landed).collect::<Vec<_>>(),
        [(Some("into:D-1".to_owned()), "unchanged".to_owned())]
    );
    let held = engine
        .documents(&onetaskgraph_core::DocumentRequest {
            sources: vec![name("into")],
            filters: onetaskgraph_core::DocumentFilters::default(),
            project: onetaskgraph_core::ProjectSelector::Any,
            paging: Paging {
                limit: NonZeroU32::new(20).expect("a non-zero limit"),
                token: None,
            },
        })
        .await
        .expect("the destination is configured");
    assert_eq!(
        held.items.len(),
        1,
        "exactly one where there was one before"
    );
}

#[tokio::test]
async fn a_document_copy_naming_a_destination_with_no_documents_is_refused_before_anything_is_read()
{
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "documents": [a_document("D-1", "Design review")],
        }},
        // Writable, and holding no documents: the refusal has to be about the documents.
        "into": {"plugin": "in-memory", "config": {}},
    }));

    let refusal = engine
        .copy(&many(&["from:D-1"], CopyScope::Documents))
        .await
        .expect_err("a destination with no documents has nowhere to put one");
    let EngineError::NoDocuments { name: named, kind } = refusal else {
        panic!("a destination with no documents is refused as one: {refusal:?}");
    };
    assert_eq!(named, "into");
    assert_eq!(kind, "in-memory");

    // Nothing was read and nothing was written: the destination still holds no documents,
    // and the source still holds the one it had.
    assert!(
        engine
            .document(&id("into:D-1"))
            .await
            .expect("the destination is configured")
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn a_document_copy_out_of_a_source_with_no_documents_is_refused_naming_that_source() {
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {}},
        "into": {"plugin": "in-memory", "config": {"capabilities": {"documents": "native"}}},
    }));

    let refusal = engine
        .copy(&many(&["from:D-1"], CopyScope::Documents))
        .await
        .expect_err("a source with no documents holds nothing to copy out");
    let EngineError::NoDocuments { name: named, kind } = refusal else {
        panic!("a source with no documents is refused as one: {refusal:?}");
    };
    assert_eq!(named, "from");
    assert_eq!(kind, "in-memory");
}

#[tokio::test]
async fn a_document_copy_that_cannot_finish_leaves_the_destination_as_it_found_it() {
    // Two documents, the second of which the destination will not create. A copy is either
    // complete or it never happened, so the first one's creation is taken back.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "documents": [a_document("D-1", "Design review"), a_document("D-2", "Refused")],
        }},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native", "uncreatable_titles": ["Refused"]},
        }},
    }));

    let refusal = engine
        .copy(&many(&["from:D-1", "from:D-2"], CopyScope::Documents))
        .await
        .expect_err("a destination that refuses one document fails the whole copy");
    let EngineError::SourceRefused { name: named, .. } = refusal else {
        panic!("the destination's own refusal reaches the caller: {refusal:?}");
    };
    assert_eq!(named, "into");

    let held = engine
        .documents(&onetaskgraph_core::DocumentRequest {
            sources: vec![name("into")],
            filters: onetaskgraph_core::DocumentFilters::default(),
            project: onetaskgraph_core::ProjectSelector::Any,
            paging: Paging {
                limit: NonZeroU32::new(20).expect("a non-zero limit"),
                token: None,
            },
        })
        .await
        .expect("the destination is configured");
    assert!(
        held.items.is_empty(),
        "the copy undid its own writes: {:?}",
        held.items
    );
}

// A copy that would never end.
//
// A source handing back the cursor it was given is a loop with no end, and from outside it
// is indistinguishable from a copy still working — which is what an operator watching one
// spin cannot tell. A source returning more rows than it was asked for overruns the one
// page the engine holds. Both are refused here rather than waited on.
//
// The misbehaviour below is at the boundary, not a stand-in for the layer under test: the
// copy is the real `Engine::copy` over a real destination write.

/// What a source does that no correct plugin does.
#[derive(Debug, Clone, Copy)]
enum Fault {
    /// Answers every cursor with the cursor it was given.
    RepeatsTheCursor,
    /// Returns one more row than the page it was asked for.
    OverrunsThePage,
    /// Answers each of two cursors with the other, for ever.
    ///
    /// Every page advances, so the walk inside the engine's own list verb sees a source
    /// behaving: it fills its budget, stops on the same cursor it stopped on last time,
    /// and hands back the page token it handed back last time. Nothing below that verb
    /// can see it. The copy's walk of a project's members pages by exactly that token,
    /// and its own guard is the only thing between this source and a copy with no end.
    CyclesItsCursors,
}

/// The two cursors [`Fault::CyclesItsCursors`] answers each other with.
const FULL_HALF: &str = "the-page-holding-the-members";
const EMPTY_HALF: &str = "the-page-holding-none";

/// Which read misbehaves, which decides which pagination loop of the copy path it meets.
#[derive(Debug, Clone, Copy, PartialEq)]
enum At {
    /// `query_tasks`: the scan that looks for a counterpart at the destination, the walk
    /// that reports what a project copy left behind there, and the read of a project's
    /// members out of its own source.
    Tasks,
    /// `query_projects`: the same scan, looking for the counterpart of a project.
    Projects,
    /// `query_documents`: the same scan again, looking for the counterpart of a document.
    Documents,
    /// `task_dependencies`: the walk that reads a task's forward edges out of its source.
    TaskEdges,
    /// `project_dependencies`: that walk again, at the project level.
    ProjectEdges,
}

/// When the fault starts.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Onset {
    /// From the first read, which for a task copy is the scan for a counterpart.
    FirstRead,
    /// Only from the *second* read at that level, which leaves the first walk to succeed
    /// and meets the one after it instead.
    ///
    /// A document copy reads a destination's documents twice: the scan that finds the
    /// document's own counterpart, and then the walk that finds the counterparts a
    /// document's references name. `FirstRead` can only ever meet the first of those.
    SecondRead,
    /// Only once this copy has written a task here, which leaves that scan to succeed and
    /// meets the orphan walk that comes after it instead.
    FirstWrite,
}

/// A source that misbehaves in exactly one way, so a copy can be driven into one loop.
struct Misbehaving {
    at: At,
    fault: Fault,
    onset: Onset,
    /// Whether it holds the project a copy names, and so has members to be walked.
    project: bool,
    /// Whether it declares documents, without which a copy never names it at either end.
    documents: bool,
    /// Whether it declares `filter_by_origin`, which sends a copy's search for the records
    /// copied from one origin to it as that one question rather than walking it.
    origins: bool,
    /// The largest page it serves.
    ceiling: u32,
    /// Every page it has served, of any kind, counted so a test can show the walk stopped
    /// rather than ran away.
    pages_served: Arc<AtomicU32>,
    /// Tasks, and only tasks, written here. [`Onset::FirstWrite`] waits for one of
    /// these rather than for any write, because the project of a project copy lands
    /// first: counting that one would turn the fault on before the scan this source has
    /// to answer well.
    tasks_written: AtomicU32,
    /// Reads asked of the interface this source faults at, counted so [`Onset::SecondRead`]
    /// can let the first through.
    reads_at_fault: AtomicU32,
}

impl Misbehaving {
    /// A source holding nothing, whose pages are two rows at most — so one row more than
    /// a page is three, and a test can say which page it was.
    fn new(at: At, fault: Fault, onset: Onset) -> Self {
        Self {
            at,
            fault,
            onset,
            project: false,
            documents: false,
            origins: false,
            ceiling: 2,
            pages_served: Arc::new(AtomicU32::new(0)),
            tasks_written: AtomicU32::new(0),
            reads_at_fault: AtomicU32::new(0),
        }
    }

    /// The same source, holding the one project a copy names.
    ///
    /// Its pages are larger than the page the copy reads a project's members in, so that
    /// read is what decides how large a page the walk asks for — which is what lets one
    /// page of this source fill the copy's budget exactly, and the walk stop there.
    fn holding_a_project(self) -> Self {
        Self {
            project: true,
            ceiling: 100,
            ..self
        }
    }

    /// The same source, declaring that it has documents.
    ///
    /// The engine reads that declaration once at the handshake and refuses a document
    /// copy naming a source without it before anything is read, so this is what gets a
    /// document copy as far as the walk under test.
    fn with_documents(self) -> Self {
        Self {
            documents: true,
            ..self
        }
    }

    /// The same source, declaring that it answers a question about one origin natively.
    fn filtering_by_origin(self) -> Self {
        Self {
            origins: true,
            ..self
        }
    }

    fn misbehaves_at(&self, at: At) -> bool {
        if self.at != at {
            return false;
        }
        match self.onset {
            Onset::FirstRead => true,
            // Counted here rather than in each query method, because this is the one place
            // every read at the faulting level passes through.
            Onset::SecondRead => self.reads_at_fault.fetch_add(1, Ordering::Relaxed) > 0,
            Onset::FirstWrite => self.tasks_written.load(Ordering::Relaxed) > 0,
        }
    }

    fn faulted<T>(&self, page: &PageRequest, row: impl Fn() -> T) -> Page<T> {
        match self.fault {
            Fault::RepeatsTheCursor => Page {
                items: Vec::new(),
                next: Some(page.cursor.clone().unwrap_or(Cursor("start".to_owned()))),
            },
            // One more than it was asked for: a source may return fewer and never more.
            Fault::OverrunsThePage => Page::last((0..=page.limit).map(|_| row()).collect()),
            // Only a task read cycles: what a cycle produces is a repeating page token,
            // and the one walk that pages by one is the copy's read of a project's members.
            Fault::CyclesItsCursors => panic!("an edge walk is not what cycles"),
        }
    }

    /// One half of the cycle, chosen by the cursor this read was given.
    ///
    /// The full half serves exactly the page it was asked for, which under a filter the
    /// source applies itself is the copy's own page: the walk fills its budget there and
    /// stops, on the same cursor, with the same rows behind it, every time.
    fn cycled(&self, page: &PageRequest) -> Page<Task> {
        if page.cursor.as_ref().map(|cursor| cursor.0.as_str()) == Some(EMPTY_HALF) {
            return Page {
                items: Vec::new(),
                next: Some(Cursor(FULL_HALF.to_owned())),
            };
        }
        Page {
            items: (0..page.limit)
                .map(|row| member(&NativeId::from(format!("T-{row}"))))
                .collect(),
            next: Some(Cursor(EMPTY_HALF.to_owned())),
        }
    }
}

fn reported(id: &NativeId) -> Task {
    Task {
        version: None,
        id: id.clone(),
        key: None,
        title: "Alpha engine".to_owned(),
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
        metadata: std::collections::BTreeMap::new(),
        repositories: Vec::new(),
        delivers: Vec::new(),
        delivered_by: Vec::new(),
        classification: Default::default(),
        assignees: None,
    }
}

fn member(id: &NativeId) -> Task {
    Task {
        project: Some(NativeId::from("P-1")),
        ..reported(id)
    }
}

fn held_document(id: &NativeId) -> Document {
    Document {
        version: None,
        id: id.clone(),
        title: "Design review".to_owned(),
        content: None,
        project: None,
        labels: Vec::new(),
        url: None,
        location: None,
        created_at: None,
        updated_at: None,
        metadata: std::collections::BTreeMap::new(),
        repositories: Vec::new(),
        classification: Default::default(),
    }
}

/// The document this source answers `get_document` with: filed under `P-1` and with a body,
/// which is what makes a copy of it read that project's records to find its references.
fn authored_document(id: &NativeId) -> Document {
    Document {
        content: Some("Alpha is at `/srv/from/plans/P-1/A.md`.".to_owned()),
        project: Some(NativeId::from("P-1")),
        ..held_document(id)
    }
}

fn edge(near: &NativeId, kind: ItemKind) -> DependencyEdge {
    DependencyEdge {
        from: DependencyEndpoint::from_native(near.clone(), kind),
        to: DependencyEndpoint::from_native(NativeId::from("T-9"), kind),
        kind: DependencyKind::Blocks,
    }
}

fn held_project(id: &NativeId) -> Project {
    Project {
        version: None,
        id: id.clone(),
        title: "Engine".to_owned(),
        content: None,
        status: Status {
            category: StatusCategory::Todo,
            name: "Todo".to_owned(),
        },
        labels: Vec::new(),
        url: None,
        location: None,
        created_at: None,
        updated_at: None,
        metadata: std::collections::BTreeMap::new(),
        repositories: Vec::new(),
        classification: Default::default(),
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
            documents: if self.documents {
                Support::Native
            } else {
                Support::Unsupported
            },
            comments: Support::Unsupported,
            assets: Support::Unsupported,
            priority: Support::Unsupported,
            filter_by_priority: Support::Unsupported,
            filter_by_comment_activity: Support::Unsupported,
            filter_by_metadata: Support::Unsupported,
            filter_by_origin: if self.origins {
                Support::Native
            } else {
                Support::Unsupported
            },
            reconcile_status: Support::Unsupported,
            complete_by_issue: Support::Unsupported,
            orphan_tasks: Support::Native,
            filter_by_label: Support::Native,
            filter_by_status: Support::Native,
            search_title: Support::Native,
            search_content: Support::Native,
            task_dependencies: DependencySupport::BothDirections,
            project_dependencies: DependencySupport::BothDirections,
            max_page_size: self.ceiling,
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
        Ok(Some(reported(id)))
    }

    async fn get_project(&self, id: &NativeId) -> Result<Option<Project>, SourceError> {
        Ok(self.project.then(|| held_project(id)))
    }

    async fn get_document(&self, id: &NativeId) -> Result<Option<Document>, SourceError> {
        if !self.documents {
            return Err(documentless(self.kind()));
        }
        Ok(Some(authored_document(id)))
    }

    async fn query_tasks(
        &self,
        _query: &TaskQuery,
        page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        self.pages_served.fetch_add(1, Ordering::Relaxed);
        if self.misbehaves_at(At::Tasks) {
            return Ok(match self.fault {
                Fault::CyclesItsCursors => self.cycled(page),
                _ => self.faulted(page, || reported(&NativeId::from("H-1"))),
            });
        }
        Ok(Page::last(Vec::new()))
    }

    async fn query_projects(
        &self,
        _query: &ProjectQuery,
        page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        self.pages_served.fetch_add(1, Ordering::Relaxed);
        if self.misbehaves_at(At::Projects) {
            return Ok(self.faulted(page, || held_project(&NativeId::from("H-1"))));
        }
        Ok(Page::last(Vec::new()))
    }

    async fn query_documents(
        &self,
        _query: &DocumentQuery,
        page: &PageRequest,
    ) -> Result<Page<Document>, SourceError> {
        // What a source with no documents owes: a refusal rather than an empty page,
        // which is what the engine reads the declaration at the handshake to avoid.
        if !self.documents {
            return Err(documentless(self.kind()));
        }
        self.pages_served.fetch_add(1, Ordering::Relaxed);
        if self.misbehaves_at(At::Documents) {
            return Ok(self.faulted(page, || held_document(&NativeId::from("H-1"))));
        }
        Ok(Page::last(Vec::new()))
    }

    async fn labels(&self, _page: &PageRequest) -> Result<Page<Label>, SourceError> {
        self.pages_served.fetch_add(1, Ordering::Relaxed);
        Ok(Page::last(Vec::new()))
    }

    async fn task_dependencies(
        &self,
        id: &NativeId,
        _direction: Direction,
        page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        self.pages_served.fetch_add(1, Ordering::Relaxed);
        if self.misbehaves_at(At::TaskEdges) {
            let near = id.clone();
            return Ok(self.faulted(page, || edge(&near, ItemKind::Task)));
        }
        Ok(Page::last(Vec::new()))
    }

    async fn project_dependencies(
        &self,
        id: &NativeId,
        _direction: Direction,
        page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        self.pages_served.fetch_add(1, Ordering::Relaxed);
        if self.misbehaves_at(At::ProjectEdges) {
            let near = id.clone();
            return Ok(self.faulted(page, || edge(&near, ItemKind::Project)));
        }
        Ok(Page::last(Vec::new()))
    }

    async fn write_task(&self, write: &ItemWrite<Task>) -> Result<NativeId, SourceError> {
        self.tasks_written.fetch_add(1, Ordering::Relaxed);
        Ok(write.target.clone().unwrap_or(NativeId::from("W-1")))
    }

    async fn write_project(&self, write: &ItemWrite<Project>) -> Result<NativeId, SourceError> {
        Ok(write.target.clone().unwrap_or(NativeId::from("W-9")))
    }

    async fn delete_task(&self, _id: &NativeId) -> Result<(), SourceError> {
        Ok(())
    }

    async fn delete_project(&self, _id: &NativeId) -> Result<(), SourceError> {
        Ok(())
    }

    async fn write_document(&self, write: &ItemWrite<Document>) -> Result<NativeId, SourceError> {
        Ok(write.target.clone().unwrap_or(NativeId::from("W-D")))
    }

    async fn delete_document(&self, _id: &NativeId) -> Result<(), SourceError> {
        Ok(())
    }
}

/// One `in-memory` source, built through its own plugin so an engine can hold it beside a
/// source written here.
fn in_memory(source: &str, config: Value) -> ConfiguredSource {
    let built = onetaskgraph_in_memory::Plugin
        .build(&name(source), &config, &NoSecrets)
        .expect("the in-memory plugin builds");
    ConfiguredSource::Ready(ResolvedSource::adopt(name(source), built))
}

/// An engine whose destination misbehaves, over the `in-memory` source described.
fn into_misbehaving_over(from: Value, source: Misbehaving) -> (Engine, Arc<AtomicU32>) {
    let pages = Arc::clone(&source.pages_served);
    let engine = Engine::new(
        vec![
            in_memory("from", from),
            ConfiguredSource::Ready(ResolvedSource::adopt(name("into"), Box::new(source))),
        ],
        vec![name("from"), name("into")],
    );
    (engine, pages)
}

/// The same, over a source holding `T-1` in project `P-1`.
fn into_misbehaving(source: Misbehaving) -> (Engine, Arc<AtomicU32>) {
    into_misbehaving_over(
        json!({
            "projects": [{"id": "P-1", "title": "Engine",
                          "status": {"category": "todo", "name": "Todo"}, "labels": []}],
            "tasks": [{"id": "T-1", "title": "Alpha engine",
                       "status": {"category": "todo", "name": "Todo"},
                       "labels": [], "project": "P-1"}],
        }),
        source,
    )
}

/// The same, over a source whose `P-1` records the destination's own `P-1` as its origin, into
/// a destination holding that project — so a project copy updates it rather than creating
/// one, which is the only kind of copy that walks for what it left behind: a project a copy
/// creates holds nothing it did not file itself.
fn into_misbehaving_holding_the_project(source: Misbehaving) -> (Engine, Arc<AtomicU32>) {
    into_misbehaving_over(
        json!({
            "projects": [{"id": "P-1", "title": "Engine",
                          "status": {"category": "todo", "name": "Todo"}, "labels": [],
                          "metadata": {"onetaskgraph.origin": "into:P-1"}}],
            "tasks": [{"id": "T-1", "title": "Alpha engine",
                       "status": {"category": "todo", "name": "Todo"},
                       "labels": [], "project": "P-1"}],
        }),
        // Holding the project, at the source's own two-row pages rather than the larger ones
        // `holding_a_project` gives a member walk.
        Misbehaving {
            project: true,
            ..source
        },
    )
}

/// An engine whose source misbehaves, copying into an ordinary `in-memory` destination.
fn from_misbehaving(source: Misbehaving) -> (Engine, Arc<AtomicU32>) {
    from_misbehaving_into(source, json!({}))
}

/// The same, into a destination that declares it has documents — without which a document
/// copy is refused at the handshake and never reaches the source at all.
fn from_misbehaving_documentary(source: Misbehaving) -> (Engine, Arc<AtomicU32>) {
    from_misbehaving_into(source, json!({"capabilities": {"documents": "native"}}))
}

fn from_misbehaving_into(source: Misbehaving, into: Value) -> (Engine, Arc<AtomicU32>) {
    let pages = Arc::clone(&source.pages_served);
    let engine = Engine::new(
        vec![
            ConfiguredSource::Ready(ResolvedSource::adopt(name("from"), Box::new(source))),
            in_memory("into", into),
        ],
        vec![name("from"), name("into")],
    );
    (engine, pages)
}

/// What a copy refused, or the test's own fault.
///
/// The ten seconds are a bound where the runtime can apply one rather than the assertion:
/// a walk whose reads all resolve without yielding never gives the timer a turn — which
/// is what an unguarded loop here really does, as removing the guard shows — so what says
/// the walk stopped is the count of reads each test reads back. This is what keeps a
/// regression that *does* yield from waiting for ever.
async fn refusal(engine: &Engine, request: &CopyRequest) -> String {
    let outcome = tokio::time::timeout(Duration::from_secs(10), engine.copy(request))
        .await
        .expect("the copy stops instead of running away");
    match outcome {
        Err(error) => error.to_string(),
        Ok(report) => panic!("a misbehaving source must be refused: {report:?}"),
    }
}

#[tokio::test]
async fn a_destination_that_repeats_its_cursor_stops_the_scan_for_a_counterpart() {
    let (engine, pages) = into_misbehaving(Misbehaving::new(
        At::Tasks,
        Fault::RepeatsTheCursor,
        Onset::FirstRead,
    ));

    let refused = refusal(&engine, &one("from:T-1")).await;

    assert!(refused.contains("source into could not do it"), "{refused}");
    assert!(
        refused.contains(
            "the source returned the cursor it was given while the destination was being \
             scanned for the item to update"
        ),
        "{refused}"
    );
    assert!(pages.load(Ordering::Relaxed) <= 3, "the scan stopped early");
}

#[tokio::test]
async fn a_destination_that_overruns_its_page_stops_the_scan_for_a_counterpart() {
    let (engine, pages) = into_misbehaving(Misbehaving::new(
        At::Tasks,
        Fault::OverrunsThePage,
        Onset::FirstRead,
    ));

    let refused = refusal(&engine, &one("from:T-1")).await;

    assert!(
        refused.contains("the source returned 3 rows for a page of at most 2"),
        "{refused}"
    );
    assert_eq!(pages.load(Ordering::Relaxed), 1, "the scan stopped at once");
}

#[tokio::test]
async fn a_destination_that_repeats_its_cursor_stops_the_walk_for_what_a_copy_left_behind() {
    // The scan for each counterpart has to succeed for the orphan walk to be reached at
    // all, which is why this destination behaves until the copy has written to it.
    let (engine, _) = into_misbehaving_holding_the_project(Misbehaving::new(
        At::Tasks,
        Fault::RepeatsTheCursor,
        Onset::FirstWrite,
    ));

    let refused = refusal(
        &engine,
        &many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ),
    )
    .await;

    assert!(
        refused.contains(
            "the source returned the cursor it was given while the destination was being \
             read for items the copy left behind"
        ),
        "{refused}"
    );
}

#[tokio::test]
async fn a_destination_that_overruns_its_page_stops_the_walk_for_what_a_copy_left_behind() {
    // The other fault at the same loop as the test above: the scan for each counterpart
    // succeeds, and the walk that reports what the copy left behind is handed three rows
    // for a page of two.
    let (engine, _) = into_misbehaving_holding_the_project(Misbehaving::new(
        At::Tasks,
        Fault::OverrunsThePage,
        Onset::FirstWrite,
    ));

    let refused = refusal(
        &engine,
        &many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ),
    )
    .await;

    assert!(refused.contains("source into could not do it"), "{refused}");
    assert!(
        refused.contains("the source returned 3 rows for a page of at most 2"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_destination_that_overruns_its_page_stops_the_scan_for_a_project() {
    // The same scan as the task one, down its project branch: a project copy looks for
    // the counterpart of the project itself before it looks for any of its tasks.
    let (engine, _) = into_misbehaving(Misbehaving::new(
        At::Projects,
        Fault::OverrunsThePage,
        Onset::FirstRead,
    ));

    let refused = refusal(
        &engine,
        &many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: false,
                documents: false,
            },
        ),
    )
    .await;

    assert!(refused.contains("source into could not do it"), "{refused}");
    assert!(
        refused.contains("the source returned 3 rows for a page of at most 2"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_destination_that_overruns_its_page_stops_the_scan_for_a_document() {
    // That scan's third branch. A document is not work — it has no status and no
    // dependencies — but it is looked for at the destination exactly as the other two
    // are, and a destination overrunning that page is refused there too.
    let (engine, _) = into_misbehaving_over(
        json!({
            "capabilities": {"documents": "native"},
            "documents": [a_document("D-1", "Design review")],
        }),
        Misbehaving::new(At::Documents, Fault::OverrunsThePage, Onset::FirstRead).with_documents(),
    );

    let refused = refusal(&engine, &many(&["from:D-1"], CopyScope::Documents)).await;

    assert!(refused.contains("source into could not do it"), "{refused}");
    assert!(
        refused.contains("the source returned 3 rows for a page of at most 2"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_source_that_repeats_its_cursor_stops_the_walk_of_a_projects_own_edges() {
    // The edge walk's other branch: a project's dependencies are read out of its source
    // by the same loop that reads a task's, and are held to the same rule.
    let (engine, _) = from_misbehaving(
        Misbehaving::new(At::ProjectEdges, Fault::RepeatsTheCursor, Onset::FirstRead)
            .holding_a_project(),
    );

    let refused = refusal(
        &engine,
        &many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: false,
                documents: false,
            },
        ),
    )
    .await;

    assert!(refused.contains("source from could not do it"), "{refused}");
    assert!(
        refused.contains(
            "the source returned the cursor it was given while an item's dependencies \
             were being read for a copy"
        ),
        "{refused}"
    );
}

#[tokio::test]
async fn a_source_whose_cursors_cycle_stops_the_walk_of_a_projects_members() {
    // The fault the walk under the list verb cannot see: every page this source serves
    // advances, so nothing below refuses it, and yet the token that verb hands back is the
    // token it handed back last time. The copy asks for the next page of members with the
    // token it was just given, gets the same page again, and would do so for ever. The
    // loop that pages by that token is the only thing that can see it.
    let (engine, _) = from_misbehaving(
        Misbehaving::new(At::Tasks, Fault::CyclesItsCursors, Onset::FirstRead).holding_a_project(),
    );

    let refused = refusal(
        &engine,
        &many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ),
    )
    .await;

    assert!(refused.contains("source from could not do it"), "{refused}");
    assert!(
        refused.contains(
            "the source returned the cursor it was given while the tasks of a project \
             were being read for a copy"
        ),
        "{refused}"
    );
}

#[tokio::test]
async fn a_source_whose_cursors_cycle_stops_the_walk_of_a_projects_graph() {
    // `project graph` reads a project's tasks through the very walk a copy reads its members
    // through, so the same source meets the same guard there — and is refused as a failed
    // read, naming the graph, rather than as a copy to run again.
    let (engine, pages) = from_misbehaving(
        Misbehaving::new(At::Tasks, Fault::CyclesItsCursors, Onset::FirstRead).holding_a_project(),
    );

    let refused = engine
        .project_graph(&ProjectGraphRequest {
            project: id("from:P-1"),
            direction: None,
            group_by: None,
            completeness: Completeness::NamingUnasked,
        })
        .await
        .expect_err("a source whose cursors cycle is refused")
        .to_string();

    assert!(refused.contains("source from could not do it"), "{refused}");
    assert!(
        refused.contains(
            "the source returned the cursor it was given while the tasks of a project \
             were being read for a graph"
        ),
        "{refused}"
    );
    assert!(refused.contains("run the command again"), "{refused}");
    assert!(pages.load(Ordering::SeqCst) < 10, "the walk stopped");
}

#[tokio::test]
async fn a_source_that_overruns_its_page_stops_the_walk_of_a_projects_members() {
    // The same walk, the other fault. This one is refused a level below the copy's own
    // loop, in the walk that reads the source's page — what matters is that it is refused
    // while a project's members are read, and that the source is named.
    let (engine, _) = from_misbehaving(
        Misbehaving::new(At::Tasks, Fault::OverrunsThePage, Onset::FirstRead).holding_a_project(),
    );

    let refused = refusal(
        &engine,
        &many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ),
    )
    .await;

    assert!(refused.contains("source from could not do it"), "{refused}");
    assert!(refused.contains("rows for a page of at most"), "{refused}");
}

#[tokio::test]
async fn a_source_that_repeats_its_cursor_stops_the_walk_of_the_edges_a_copy_reads() {
    let (engine, pages) = from_misbehaving(Misbehaving::new(
        At::TaskEdges,
        Fault::RepeatsTheCursor,
        Onset::FirstRead,
    ));

    let refused = refusal(&engine, &one("from:T-1")).await;

    assert!(refused.contains("source from could not do it"), "{refused}");
    assert!(
        refused.contains(
            "the source returned the cursor it was given while an item's dependencies \
             were being read for a copy"
        ),
        "{refused}"
    );
    assert!(pages.load(Ordering::Relaxed) <= 3, "the walk stopped early");
}

#[tokio::test]
async fn a_source_that_overruns_its_page_stops_the_walk_of_the_edges_a_copy_reads() {
    let (engine, pages) = from_misbehaving(Misbehaving::new(
        At::TaskEdges,
        Fault::OverrunsThePage,
        Onset::FirstRead,
    ));

    let refused = refusal(&engine, &one("from:T-1")).await;

    assert!(
        refused.contains("the source returned 3 rows for a page of at most 2"),
        "{refused}"
    );
    assert_eq!(pages.load(Ordering::Relaxed), 1, "the walk stopped at once");
}

#[tokio::test]
async fn a_well_behaved_copy_still_walks_every_page_of_every_loop_it_has() {
    // The other half of the same rule: a source that advances its cursor is walked to
    // exhaustion exactly as it was before, over sources serving one row per page — so the
    // edges of a task, the members of a project and the items a copy left behind are all
    // read across several pages each.
    let held = |native: &str, origin: &str| {
        json!({"id": native, "title": native,
               "status": {"category": "todo", "name": "Todo"}, "labels": [],
               "project": "P-1", "metadata": {GlobalId::ORIGIN_KEY: origin}})
    };
    let member = |native: &str| {
        json!({"id": native, "title": native,
               "status": {"category": "todo", "name": "Todo"}, "labels": [],
               "project": "P-1"})
    };
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "capabilities": {"max_page_size": 1},
            "projects": [{"id": "P-1", "title": "Engine",
                          "status": {"category": "todo", "name": "Todo"}, "labels": []}],
            "tasks": [member("T-1"), member("T-2"),
                      {"id": "T-3", "title": "T-3",
                       "status": {"category": "todo", "name": "Todo"}, "labels": []}],
            "task_dependencies": [
                {"from": "T-1", "to": "T-2", "kind": "blocks"},
                {"from": "T-1", "to": "T-3", "kind": "related"},
            ],
        }},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"max_page_size": 1},
            "projects": [{"id": "P-1", "title": "Engine",
                          "status": {"category": "todo", "name": "Todo"}, "labels": [],
                          "metadata": {GlobalId::ORIGIN_KEY: "from:P-1"}}],
            "tasks": [held("T-8", "from:T-8"), held("T-9", "from:T-9")],
        }},
    }));

    let report = engine
        .copy(&many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
        .expect("the copy runs");

    // Every member the source holds, found across pages of one; and both items the source
    // no longer holds, found by a walk that had to reach the second page to see the second.
    assert_eq!(
        report
            .items
            .iter()
            .map(|outcome| (outcome.source.to_string(), outcome.action.name()))
            .collect::<Vec<_>>(),
        vec![
            ("from:P-1".to_owned(), "unchanged".to_owned()),
            ("from:T-1".to_owned(), "created".to_owned()),
            ("from:T-2".to_owned(), "created".to_owned()),
            ("from:T-8".to_owned(), "orphaned".to_owned()),
            ("from:T-9".to_owned(), "orphaned".to_owned()),
        ]
    );
    // Both edges of `T-1` were read, across pages of one: the one inside the copied set
    // resolved to the destination's own id, and the one outside it stayed qualified.
    assert_eq!(
        depends_on(&engine, "into:T-1").await,
        vec!["into:T-2 Task".to_owned(), "from:T-3 Task".to_owned()]
    );
}

// The reference rewrite at the engine's own boundary. Driven the way a user drives it,
// against stores that outlive one invocation, in `crates/onetaskgraph-e2e/tests/e2e/copy.rs`.

/// One record of a store, with the location its source reports and the origin it records.
///
/// `origin` is what makes a fixture a *topology* rather than a heap: a record carrying one
/// is a record that arrived from somewhere, and which somewhere is the whole of what the
/// two-key rule can and cannot see.
fn located(id: &str, title: &str, path: &str, origin: Option<&str>) -> Value {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "caller.shape".to_owned(),
        json!({"nested": [1, true, null]}),
    );
    if let Some(origin) = origin {
        metadata.insert(GlobalId::ORIGIN_KEY.to_owned(), json!(origin));
    }
    json!({
        "id": id,
        "title": title,
        "status": {"category": "todo", "name": "Todo"},
        "labels": [],
        "project": "P-1",
        "location": {"path": path},
        "metadata": Value::Object(metadata),
    })
}

/// The project both stores file everything under, at the location the store reports.
fn located_project(path: &str, origin: Option<&str>) -> Value {
    let mut project = located("P-1", "The plan", path, origin);
    project
        .as_object_mut()
        .expect("a record is an object")
        .remove("project");
    project
}

/// The plan document the copy carries, in the shape the artifact this exists for really
/// has: bare absolute paths inside backticks in a table cell.
///
/// The last two lines are the whole-reference guards. `…/A.md.bak` is a path extended by a
/// further suffix, and the project's own location is a directory prefix of every task's, so
/// both occur inside a longer location-like string and neither may be rewritten there.
fn plan_document(content: &str) -> Value {
    json!({
        "id": "D-1",
        "title": "Design review",
        "content": content,
        "project": "P-1",
        "labels": [],
        "location": {"path": "/srv/from/plans/P-1/D-1.md"},
        "metadata": {"caller.shape": {"nested": [1, true, null]}},
    })
}

/// The body the fixtures below copy, naming the two tasks and the project by their paths.
const AUTHORED: &str = "# Plan\n\n\
     | Task | Where |\n\
     | --- | --- |\n\
     | Alpha | `/srv/from/plans/P-1/A.md` |\n\
     | Beta | `/srv/from/plans/P-1/B.md` |\n\n\
     Everything lives under `/srv/from/plans/P-1`, and `/srv/from/plans/P-1/A.md.bak` is a \
     backup.\n";

/// A document-bearing `in-memory` source holding one project, two tasks in it, and the
/// plan document that names all three.
fn authoring_store(origins: Option<(&str, &str, &str)>) -> Value {
    let (project, alpha, beta) = match origins {
        Some((project, alpha, beta)) => (Some(project), Some(alpha), Some(beta)),
        None => (None, None, None),
    };
    json!({
        "capabilities": {"documents": "native"},
        "projects": [located_project("/srv/from/plans/P-1", project)],
        "tasks": [
            located("A", "Alpha", "/srv/from/plans/P-1/A.md", alpha),
            located("B", "Beta", "/srv/from/plans/P-1/B.md", beta),
        ],
        "documents": [plan_document(AUTHORED)],
    })
}

/// The content one document holds at one source, read back through the engine's own show
/// verb rather than off the write.
async fn body(engine: &Engine, id: &str) -> String {
    engine
        .document(&self::id(id))
        .await
        .expect("the show verb answers")
        .items[0]
        .item
        .content
        .clone()
        .expect("the document has a body")
}

/// A copy of one document into `into`, and the figures it reported.
async fn copy_document(engine: &Engine, item: &str) -> onetaskgraph_core::CopyReport {
    engine
        .copy(&many(&[item], CopyScope::Documents))
        .await
        .expect("the document copy runs")
}

/// The three figures a copy reports, as a comparable triple.
fn figures(report: &onetaskgraph_core::CopyReport) -> (u64, u64, u64) {
    (
        report.references_rewritten,
        report.references_unresolved,
        report.references_ambiguous,
    )
}

#[tokio::test]
async fn a_reference_whose_link_no_longer_names_its_counterpart_is_found_by_its_origin() {
    // Alpha's link names a record the destination no longer holds, and Beta's names one a
    // person re-pointed at something else. Neither link is believed: each referent is asked
    // for by its origin instead, and the document names the records that really are theirs.
    let mut from = authoring_store(Some(("root:P-1", "root:A", "root:B")));
    from["tasks"][0]["metadata"][MetadataKey::COPIES_KEY] = json!({"into": "into:GONE"});
    from["tasks"][1]["metadata"][MetadataKey::COPIES_KEY] = json!({"into": "into:X"});
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": from},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/into/board", Some("root:P-1"))],
            "tasks": [
                located("A", "Alpha", "/srv/into/board/A.md", Some("root:A")),
                located("B", "Beta", "/srv/into/board/B.md", Some("root:B")),
                located("X", "Someone else's", "/srv/into/board/X.md", Some("elsewhere:X")),
            ],
        }},
    }));
    let report = copy_document(&engine, "from:D-1").await;
    let landed = report.items[0]
        .destination()
        .expect("the document landed")
        .to_string();
    let content = body(&engine, &landed).await;
    assert!(content.contains("`/srv/into/board/A.md`"), "{content}");
    assert!(content.contains("`/srv/into/board/B.md`"), "{content}");
    assert!(!content.contains("/srv/into/board/X.md"), "{content}");
    assert_eq!(
        figures(&report).0,
        3,
        "both tasks and the project are rewritten"
    );
}

#[tokio::test]
async fn a_reference_whose_link_is_answered_with_another_record_is_found_by_its_origin() {
    // Alpha's link names X, and the destination answers a read of X with A1 — a record that
    // does record Alpha's origin, but is not the one the link names. That answer says nothing
    // about the link, so Alpha is asked for by its origin, which two records answer: the
    // reference is ambiguous and left as it was, rather than pointed at whichever record the
    // destination happened to answer with.
    let mut from = authoring_store(Some(("root:P-1", "root:A", "root:B")));
    from["tasks"][0]["metadata"][MetadataKey::COPIES_KEY] = json!({"into": "into:X"});
    let into = json!({
        "capabilities": {"documents": "native"},
        "projects": [located_project("/srv/into/board", Some("root:P-1"))],
        "tasks": [
            located("A1", "Alpha", "/srv/into/board/A1.md", Some("root:A")),
            located("A2", "Alpha again", "/srv/into/board/A2.md", Some("root:A")),
            located("B", "Beta", "/srv/into/board/B.md", Some("root:B")),
            located("X", "Someone else's", "/srv/into/board/X.md", Some("elsewhere:X")),
        ],
    });
    let built = |source: &str, config: &Value| {
        onetaskgraph_in_memory::Plugin
            .build(&name(source), config, &NoSecrets)
            .expect("the in-memory plugin builds")
    };
    let engine = Engine::new(
        vec![
            ConfiguredSource::Ready(ResolvedSource::adopt(name("from"), built("from", &from))),
            ConfiguredSource::Ready(ResolvedSource::adopt(
                name("into"),
                Box::new(Careless {
                    inner: built("into", &into),
                    malformed: &[],
                    misread: Some(("X", "A1")),
                }),
            )),
        ],
        vec![name("from"), name("into")],
    );
    let report = copy_document(&engine, "from:D-1").await;
    let landed = report.items[0]
        .destination()
        .expect("the document landed")
        .to_string();
    let content = body(&engine, &landed).await;
    assert!(!content.contains("/srv/into/board/A1.md"), "{content}");
    assert!(content.contains("`/srv/from/plans/P-1/A.md`"), "{content}");
    assert!(content.contains("`/srv/into/board/B.md`"), "{content}");
    assert!(
        figures(&report).2 >= 1,
        "Alpha's reference is counted ambiguous: {:?}",
        figures(&report)
    );
}

#[tokio::test]
async fn a_document_arrives_naming_the_destinations_own_records_across_a_one_level_fan_out() {
    // The fan-out: `root` is where all three records were authored, `from` and `into` are
    // the two stores they were copied into, and the document travels by the `from` route
    // while the records it names arrived at `into` by the other one. Neither side holds the
    // other's id, so nothing here resolves on a referent's own id — both sides trace to one
    // common predecessor, which is the whole of what the second key buys.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": authoring_store(Some((
            "root:P-1", "root:A", "root:B",
        )))},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/into/board", Some("root:P-1"))],
            "tasks": [
                located("A", "Alpha", "/srv/into/board/A.md", Some("root:A")),
                located("B", "Beta", "/srv/into/board/B.md", Some("root:B")),
            ],
        }},
    }));

    let first = copy_document(&engine, "from:D-1").await;
    assert_eq!(
        first.items.iter().map(landed).collect::<Vec<_>>(),
        [(Some("into:D-1".to_owned()), "created".to_owned())]
    );
    assert_eq!(figures(&first), (3, 0, 0));

    // Every reference names the destination's own record; every other character of the
    // body is byte-for-byte what the source held, and no section was added.
    assert_eq!(
        body(&engine, "into:D-1").await,
        "# Plan\n\n\
         | Task | Where |\n\
         | --- | --- |\n\
         | Alpha | `/srv/into/board/A.md` |\n\
         | Beta | `/srv/into/board/B.md` |\n\n\
         Everything lives under `/srv/into/board`, and `/srv/from/plans/P-1/A.md.bak` is a \
         backup.\n"
    );

    // Nothing else about the document moved: the caller's own key keeps its JSON types and
    // the copy records the provenance it always did.
    let landed_document = &engine
        .document(&id("into:D-1"))
        .await
        .expect("the destination is configured")
        .items[0]
        .item;
    assert_eq!(
        landed_document.metadata["caller.shape"],
        json!({"nested": [1, true, null]})
    );
    assert_eq!(
        landed_document.metadata[GlobalId::ORIGIN_KEY],
        json!("from:D-1")
    );

    // A correct reference is never rewritten into something else: the second copy reads the
    // same source body, rewrites it the same way, and finds the destination already saying
    // it.
    let before = body(&engine, "into:D-1").await;
    let again = copy_document(&engine, "from:D-1").await;
    assert_eq!(
        again.items.iter().map(landed).collect::<Vec<_>>(),
        [(Some("into:D-1".to_owned()), "unchanged".to_owned())]
    );
    assert_eq!(figures(&again), (3, 0, 0));
    assert_eq!(body(&engine, "into:D-1").await, before);
}

#[tokio::test]
async fn a_reference_spelled_with_windows_separators_alone_is_rewritten() {
    // A folder of Markdown on Windows reports `C:\…\A.md`, which holds no `/`: a body naming
    // its records only that way is still a body that names them, and its project is read.
    let mut from = authoring_store(Some(("root:P-1", "root:A", "root:B")));
    from["projects"][0]["location"] = json!({"path": r"C:\from\plans\P-1"});
    from["tasks"][0]["location"] = json!({"path": r"C:\from\plans\P-1\A.md"});
    from["tasks"][1]["location"] = json!({"path": r"C:\from\plans\P-1\B.md"});
    from["documents"][0]["location"] = json!({"path": r"C:\from\plans\P-1\D-1.md"});
    from["documents"][0]["content"] = json!(
        "| Alpha | `C:\\from\\plans\\P-1\\A.md` |\n| Beta | `C:\\from\\plans\\P-1\\B.md` |\n"
    );
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": from},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project(r"C:\into\board", Some("root:P-1"))],
            "tasks": [
                located("A", "Alpha", r"C:\into\board\A.md", Some("root:A")),
                located("B", "Beta", r"C:\into\board\B.md", Some("root:B")),
            ],
        }},
    }));

    let report = copy_document(&engine, "from:D-1").await;
    assert_eq!(figures(&report), (2, 0, 0), "both rows are rewritten");
    let landed = report.items[0]
        .destination()
        .expect("the document landed")
        .to_string();
    assert_eq!(
        body(&engine, &landed).await,
        "| Alpha | `C:\\into\\board\\A.md` |\n| Beta | `C:\\into\\board\\B.md` |\n"
    );
}

#[tokio::test]
async fn a_history_the_two_keys_cannot_prove_is_left_byte_for_byte_and_counted_unresolved() {
    // The chain the rule cannot reach: the records travelled `from` → an intermediate store
    // → `into`, so `into` keys them by that intermediate, while the document is copied to
    // `into` directly out of `from`, where the records were authored and so record no origin
    // at all. Disjoint key sets, permanently — only one origin is ever recorded and every
    // hop overwrites it.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": authoring_store(None)},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/into/board", Some("mid:P-1"))],
            "tasks": [
                located("A", "Alpha", "/srv/into/board/A.md", Some("mid:A")),
                located("B", "Beta", "/srv/into/board/B.md", Some("mid:B")),
            ],
        }},
    }));

    let report = copy_document(&engine, "from:D-1").await;
    assert_eq!(figures(&report), (0, 3, 0));
    assert_eq!(
        body(&engine, "into:D-1").await,
        AUTHORED,
        "a history the two keys cannot prove is left byte-for-byte, never guessed at"
    );
}

#[tokio::test]
async fn a_destination_holding_two_records_for_one_referent_is_ambiguous_and_scan_still_answers() {
    // One record keyed by the referent's own id, a second keyed by the origin the referent
    // itself records. Both match the two keys, so the correspondence is not confident.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": authoring_store(Some((
            "root:P-1", "root:A", "root:B",
        )))},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/into/board", Some("root:P-1"))],
            "tasks": [
                located("A-by-id", "Alpha", "/srv/into/board/A-by-id.md", Some("from:A")),
                located("A-by-origin", "Alpha", "/srv/into/board/A-by-origin.md", Some("root:A")),
                located("B", "Beta", "/srv/into/board/B.md", Some("root:B")),
            ],
        }},
    }));

    let report = copy_document(&engine, "from:D-1").await;
    // Alpha's one occurrence is ambiguous; Beta's and the project's still resolve.
    assert_eq!(figures(&report), (2, 1, 1));
    assert!(
        body(&engine, "into:D-1")
            .await
            .contains("`/srv/from/plans/P-1/A.md`"),
        "an ambiguous reference is left byte-for-byte, and no record is chosen"
    );

    // `Engine::scan` is untouched by that stricter discipline: it takes the first hit and
    // stops, which is what every consumer of the copy already depends on. The two lookups
    // answer different questions and are meant to disagree on a destination holding
    // duplicates.
    let copied = engine
        .copy(&one("from:A"))
        .await
        .expect("the task copy runs");
    assert_eq!(
        copied.items[0].destination().map(ToString::to_string),
        Some("into:A-by-id".to_owned()),
        "the copy's own target lookup takes the first record recording the id it is \
         copying, exactly as it did before the reference rewrite existed"
    );
}

#[tokio::test]
async fn two_referents_reporting_one_location_leave_every_occurrence_of_it_alone() {
    // The source reports one location for two records, so an occurrence of it cannot be
    // attributed to either — the case where a rewrite would be confidently wrong rather
    // than merely unhelpful.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/from/plans/P-1", Some("root:P-1"))],
            "tasks": [
                located("A", "Alpha", "/srv/from/plans/P-1/shared.md", Some("root:A")),
                located("B", "Beta", "/srv/from/plans/P-1/shared.md", Some("root:B")),
            ],
            "documents": [plan_document(
                "Both rows point at `/srv/from/plans/P-1/shared.md` today.\n",
            )],
        }},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/into/board", Some("root:P-1"))],
            "tasks": [
                located("A", "Alpha", "/srv/into/board/A.md", Some("root:A")),
                located("B", "Beta", "/srv/into/board/B.md", Some("root:B")),
            ],
        }},
    }));

    let report = copy_document(&engine, "from:D-1").await;
    assert_eq!(figures(&report), (0, 1, 1));
    assert_eq!(
        body(&engine, "into:D-1").await,
        "Both rows point at `/srv/from/plans/P-1/shared.md` today.\n"
    );
}

#[tokio::test]
async fn a_counterpart_the_destination_does_not_hold_or_reports_no_location_for_is_left_alone() {
    // Beta has no counterpart at all; Alpha has one the destination reports no location
    // for, which names nowhere a reader could go. Neither is ambiguous — both are the
    // ordinary, expected outcome under the bound this design works to — and the copy still
    // succeeds.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": authoring_store(Some((
            "root:P-1", "root:A", "root:B",
        )))},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/into/board", Some("root:P-1"))],
            "tasks": [{
                "id": "A",
                "title": "Alpha",
                "status": {"category": "todo", "name": "Todo"},
                "labels": [],
                "project": "P-1",
                "metadata": {GlobalId::ORIGIN_KEY: "root:A"},
            }],
        }},
    }));

    let report = copy_document(&engine, "from:D-1").await;
    assert_eq!(figures(&report), (1, 2, 0));
    let landed_body = body(&engine, "into:D-1").await;
    assert!(landed_body.contains("`/srv/from/plans/P-1/A.md`"));
    assert!(landed_body.contains("`/srv/from/plans/P-1/B.md`"));
    assert!(landed_body.contains("`/srv/into/board`"));
}

#[tokio::test]
async fn a_dry_run_reports_the_references_it_would_have_rewritten_and_writes_nothing() {
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": authoring_store(Some((
            "root:P-1", "root:A", "root:B",
        )))},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/into/board", Some("root:P-1"))],
            "tasks": [
                located("A", "Alpha", "/srv/into/board/A.md", Some("root:A")),
                located("B", "Beta", "/srv/into/board/B.md", Some("root:B")),
            ],
        }},
    }));

    let planned = engine
        .copy(&CopyRequest {
            dry_run: true,
            ..many(&["from:D-1"], CopyScope::Documents)
        })
        .await
        .expect("the dry run reads everything");
    assert_eq!(figures(&planned), (3, 0, 0));
    assert_eq!(
        planned.items.iter().map(landed).collect::<Vec<_>>(),
        [(None, "created".to_owned())]
    );
    assert_eq!(
        engine
            .documents(&onetaskgraph_core::DocumentRequest {
                sources: vec![name("into")],
                filters: onetaskgraph_core::DocumentFilters::default(),
                project: onetaskgraph_core::ProjectSelector::Any,
                paging: Paging {
                    limit: NonZeroU32::new(20).expect("a non-zero limit"),
                    token: None,
                },
            })
            .await
            .expect("the destination is configured")
            .items
            .len(),
        0,
        "a dry run writes nothing"
    );
}

/// A destination that answers exactly as the `in-memory` source it wraps, and counts the
/// task pages it was asked for.
///
/// A **document** copy asks a destination for task pages in exactly one place — the walk
/// that finds the counterparts a document's references name. Its own target is found
/// through `query_documents`, and the project it is filed under through `query_projects`.
/// So this count is that walk and nothing else, which is what lets a test say the walk
/// happened once for a whole invocation, or never happened at all.
struct Counting {
    inner: Box<dyn TaskSource>,
    task_pages: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl TaskSource for Counting {
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
        self.task_pages.fetch_add(1, Ordering::Relaxed);
        self.inner.query_tasks(query, page).await
    }

    async fn query_projects(
        &self,
        query: &ProjectQuery,
        page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        self.inner.query_projects(query, page).await
    }

    async fn query_documents(
        &self,
        query: &DocumentQuery,
        page: &PageRequest,
    ) -> Result<Page<Document>, SourceError> {
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

    async fn delete_task(&self, id: &NativeId) -> Result<(), SourceError> {
        self.inner.delete_task(id).await
    }

    async fn delete_project(&self, id: &NativeId) -> Result<(), SourceError> {
        self.inner.delete_project(id).await
    }

    async fn delete_document(&self, id: &NativeId) -> Result<(), SourceError> {
        self.inner.delete_document(id).await
    }
}

/// An engine reading `from` and writing into a counting wrapper around `into`.
fn into_counting(from: Value, into: Value) -> (Engine, Arc<AtomicU32>) {
    let task_pages = Arc::new(AtomicU32::new(0));
    let inner = onetaskgraph_in_memory::Plugin
        .build(&name("into"), &into, &NoSecrets)
        .expect("the in-memory plugin builds");
    let counting = Counting {
        inner,
        task_pages: Arc::clone(&task_pages),
    };
    let engine = Engine::new(
        vec![
            in_memory("from", from),
            ConfiguredSource::Ready(ResolvedSource::adopt(name("into"), Box::new(counting))),
        ],
        vec![name("from"), name("into")],
    );
    (engine, task_pages)
}

/// The authoring store above, plus a second document of the same project naming the same
/// two tasks. Two documents in one project is the ordinary case, and it is what a
/// per-document walk would multiply reads for.
fn two_documents_of_one_project() -> Value {
    let mut store = authoring_store(Some(("root:P-1", "root:A", "root:B")));
    let mut second = plan_document(AUTHORED);
    second["id"] = json!("D-2");
    second["location"] = json!({"path": "/srv/from/plans/P-1/D-2.md"});
    store["documents"] = json!([plan_document(AUTHORED), second]);
    store
}

/// The destination both cases below copy into.
fn board_holding_counterparts() -> Value {
    json!({
        "capabilities": {"documents": "native"},
        "projects": [located_project("/srv/into/board", Some("root:P-1"))],
        "tasks": [
            located("A", "Alpha", "/srv/into/board/A.md", Some("root:A")),
            located("B", "Beta", "/srv/into/board/B.md", Some("root:B")),
        ],
    })
}

#[tokio::test]
async fn each_referent_is_asked_for_once_for_a_whole_invocation_and_nothing_for_nothing() {
    let (engine, task_pages) =
        into_counting(two_documents_of_one_project(), board_holding_counterparts());

    let report = engine
        .copy(&many(&["from:D-1", "from:D-2"], CopyScope::Documents))
        .await
        .expect("the document copy runs");
    // Three references apiece. The destination is asked about each referent's two keys once
    // for the whole invocation — Alpha's own id and its recorded origin, and Beta's — however
    // many documents name them, and never walked: it answers a question about one origin.
    assert_eq!(figures(&report), (6, 0, 0));
    assert_eq!(
        task_pages.load(Ordering::Relaxed),
        4,
        "the destination is asked for each referent's counterparts once per copy \
         invocation, not once per document"
    );

    // A copy whose documents hold no candidate reference makes no such walk at all: the
    // referent set is read at the source, and nothing it holds occurs in this body.
    let mut quiet = authoring_store(Some(("root:P-1", "root:A", "root:B")));
    quiet["documents"] = json!([plan_document("Nothing here names a record.\n")]);
    let (engine, task_pages) = into_counting(quiet, board_holding_counterparts());
    let report = copy_document(&engine, "from:D-1").await;
    assert_eq!(figures(&report), (0, 0, 0));
    assert_eq!(
        task_pages.load(Ordering::Relaxed),
        0,
        "a copy that recognises no reference asks the destination for no task page"
    );
}

#[tokio::test]
async fn a_task_sharing_the_documents_own_id_is_still_a_referent() {
    // A folder of Markdown files `A.md` under `tasks/` and under `documents/` and reports
    // `A` for both, so a source's id does not identify a record on its own. Told apart by
    // id alone, the task here would be dropped from its own document's referent set.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/from/plans/P-1", Some("root:P-1"))],
            "tasks": [located("D-1", "Alpha", "/srv/from/plans/P-1/A.md", Some("root:A"))],
            "documents": [{
                "id": "D-1",
                "title": "Design review",
                "content": "Alpha is at `/srv/from/plans/P-1/A.md` today.\n",
                "project": "P-1",
                "labels": [],
                "location": {"path": "/srv/from/plans/P-1/D-1.md"},
                "metadata": {},
            }],
        }},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/into/board", Some("root:P-1"))],
            "tasks": [located("A", "Alpha", "/srv/into/board/A.md", Some("root:A"))],
        }},
    }));

    let report = copy_document(&engine, "from:D-1").await;
    assert_eq!(figures(&report), (1, 0, 0));
    assert_eq!(
        body(&engine, "into:D-1").await,
        "Alpha is at `/srv/into/board/A.md` today.\n"
    );
}

/// The same record shape as [`located`], reported as a link rather than as a file.
///
/// `Location` has two variants and a plugin picks one — `local-md` reports a canonical
/// absolute path and `github-projects` reports an issue URL — so a rewrite that only ever
/// saw paths would be half the contract.
fn linked(id: &str, title: &str, url: &str, origin: Option<&str>) -> Value {
    let mut record = located(id, title, "unused", origin);
    record["location"] = json!({ "url": url });
    record
}

#[tokio::test]
async fn a_reference_reported_as_a_link_is_rewritten_and_not_inside_a_longer_link() {
    // `.../issues/1` occurs inside `.../issues/12`, which names a different issue: the
    // whole-reference rule has to hold for a link exactly as it does for a path.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/from/plans/P-1", Some("root:P-1"))],
            "tasks": [
                linked("A", "Alpha", "https://example.invalid/from/issues/1", Some("root:A")),
                linked("B", "Beta", "https://example.invalid/from/issues/12", Some("root:B")),
            ],
            "documents": [plan_document(
                "Alpha is `https://example.invalid/from/issues/1` and Beta is \
                 `https://example.invalid/from/issues/12`.\n",
            )],
        }},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/into/board", Some("root:P-1"))],
            "tasks": [
                linked("A", "Alpha", "https://example.invalid/board/issues/7", Some("root:A")),
                linked("B", "Beta", "https://example.invalid/board/issues/8", Some("root:B")),
            ],
        }},
    }));

    let report = copy_document(&engine, "from:D-1").await;
    assert_eq!(figures(&report), (2, 0, 0));
    assert_eq!(
        body(&engine, "into:D-1").await,
        "Alpha is `https://example.invalid/board/issues/7` and Beta is \
         `https://example.invalid/board/issues/8`.\n",
        "the shorter link is rewritten as itself and never inside the longer one"
    );
}

#[tokio::test]
async fn a_location_before_a_full_stop_is_not_recognised_and_comes_through_byte_for_byte() {
    // The cost the boundary rule states, put to the engine rather than left in prose. `.`
    // is not a stop, because `/…/A.md` inside `/…/A.md.bak` is a different file, so a
    // location written bare before a full stop is not recognised at all: left
    // byte-for-byte and counted in neither figure, exactly as a reference to another
    // project's record is. The line above it names the *same* task at the *same*
    // destination inside backticks and is rewritten, so what declines the second
    // occurrence is the delimiter beside it and nothing about the correspondence.
    let authored = "Alpha is at `/srv/from/plans/P-1/A.md`.\n\n\
                    The same file, written bare, is at /srv/from/plans/P-1/A.md.\n";
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/from/plans/P-1", Some("root:P-1"))],
            "tasks": [located("A", "Alpha", "/srv/from/plans/P-1/A.md", Some("root:A"))],
            "documents": [plan_document(authored)],
        }},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/into/board", Some("root:P-1"))],
            "tasks": [located("A", "Alpha", "/srv/into/board/A.md", Some("root:A"))],
        }},
    }));

    let report = copy_document(&engine, "from:D-1").await;
    assert_eq!(
        figures(&report),
        (1, 0, 0),
        "an unrecognised occurrence is not an unresolved one: the figures report what the \
         copy recognised, not a census of what the document holds"
    );
    assert_eq!(
        body(&engine, "into:D-1").await,
        "Alpha is at `/srv/into/board/A.md`.\n\n\
         The same file, written bare, is at /srv/from/plans/P-1/A.md.\n"
    );
}

#[tokio::test]
async fn a_reference_carrying_a_fragment_is_rewritten_and_keeps_the_fragment() {
    // A `#` after a location opens a fragment naming a place inside that record, so the
    // location before it is rewritten and the fragment rides through unchanged — ended by
    // every character that already ends a location, by the end of the content, and by a
    // full stop, which ends the *fragment* here rather than the location. A bare location
    // before a full stop is still not recognised, as the test above pins.
    const STOPS: &str = " \t\n`\"'()[]{}<>|,;";
    let from = "/srv/from/plans/P-1/A.md";
    let into = "/srv/into/board/A.md";
    let link = "https://example.invalid/from/issues/1";
    let linked_into = "https://example.invalid/board/issues/7";
    let lines = |path: &str, url: &str| {
        let mut written: String = STOPS
            .chars()
            .map(|stop| format!("({path}#budgets{stop} ({url}#budgets{stop}\n"))
            .collect();
        written.push_str(&format!("[t1]({path}#budgets). Bare: {path}.\n"));
        written.push_str(&format!("At the end: {path}#budgets"));
        written
    };
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/from/plans/P-1", Some("root:P-1"))],
            "tasks": [
                located("A", "Alpha", from, Some("root:A")),
                linked("B", "Beta", link, Some("root:B")),
            ],
            "documents": [plan_document(&lines(from, link))],
        }},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/into/board", Some("root:P-1"))],
            "tasks": [
                located("A", "Alpha", into, Some("root:A")),
                linked("B", "Beta", linked_into, Some("root:B")),
            ],
        }},
    }));

    let report = copy_document(&engine, "from:D-1").await;
    let stops = STOPS.chars().count() as u64;
    assert_eq!(
        figures(&report),
        (2 * stops + 2, 0, 0),
        "every fragment reference is counted rewritten, and the bare one before a full \
         stop is counted in neither figure"
    );
    let expected =
        lines(into, linked_into).replace(&format!("Bare: {into}."), &format!("Bare: {from}."));
    assert_eq!(body(&engine, "into:D-1").await, expected);
}

#[tokio::test]
async fn a_fragment_reference_is_left_and_counted_as_any_other_reference_without_a_counterpart() {
    // A fragment widens what is *recognised* and nothing else. A referent with no
    // counterpart, and a location two referents report, are left byte-for-byte with their
    // fragments and counted exactly as their bare forms are; and a referent whose own file
    // name holds a `#` is still matched whole, because the longer location is tried first.
    let root = "/srv/from/plans/P-1";
    let authored = format!(
        "Kept: [a]({root}/A.md#budgets) and `{root}/A.md#2`.\n\
         Gone: [c]({root}/C.md#budgets).\n\
         Twice: [d]({root}/D.md#budgets).\n"
    );
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project(root, Some("root:P-1"))],
            "tasks": [
                located("A", "Alpha", &format!("{root}/A.md"), Some("root:A")),
                located("A2", "Alpha two", &format!("{root}/A.md#2"), Some("root:A2")),
                located("C", "Gamma", &format!("{root}/C.md"), Some("root:C")),
                located("D", "Delta", &format!("{root}/D.md"), Some("root:D")),
                located("E", "Epsilon", &format!("{root}/D.md"), Some("root:E")),
            ],
            "documents": [plan_document(&authored)],
        }},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/into/board", Some("root:P-1"))],
            "tasks": [
                located("A", "Alpha", "/srv/into/board/A.md", Some("root:A")),
                located("A2", "Alpha two", "/srv/into/board/A2.md", Some("root:A2")),
                located("D", "Delta", "/srv/into/board/D.md", Some("root:D")),
                located("E", "Epsilon", "/srv/into/board/E.md", Some("root:E")),
            ],
        }},
    }));

    let report = copy_document(&engine, "from:D-1").await;
    assert_eq!(figures(&report), (2, 2, 1));
    assert_eq!(
        body(&engine, "into:D-1").await,
        format!(
            "Kept: [a](/srv/into/board/A.md#budgets) and `/srv/into/board/A2.md`.\n\
             Gone: [c]({root}/C.md#budgets).\n\
             Twice: [d]({root}/D.md#budgets).\n"
        )
    );
}

#[tokio::test]
async fn a_document_naming_another_document_of_its_project_is_rewritten_too() {
    // The referent set is the project's record, its tasks and its *other documents*. A plan
    // that points at the runbook beside it is the case this third read is for.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/from/plans/P-1", Some("root:P-1"))],
            "documents": [
                plan_document("The runbook is `/srv/from/plans/P-1/D-2.md`.\n"),
                {
                    "id": "D-2",
                    "title": "Runbook",
                    "content": "how to read the plan",
                    "project": "P-1",
                    "labels": [],
                    "location": {"path": "/srv/from/plans/P-1/D-2.md"},
                    "metadata": {GlobalId::ORIGIN_KEY: "root:D-2"},
                },
            ],
        }},
        "into": {"plugin": "in-memory", "config": {
            "capabilities": {"documents": "native"},
            "projects": [located_project("/srv/into/board", Some("root:P-1"))],
            "documents": [{
                "id": "D-2",
                "title": "Runbook",
                "content": "how to read the plan",
                "project": "P-1",
                "labels": [],
                "location": {"path": "/srv/into/board/D-2.md"},
                "metadata": {GlobalId::ORIGIN_KEY: "root:D-2"},
            }],
        }},
    }));

    let report = copy_document(&engine, "from:D-1").await;
    assert_eq!(figures(&report), (1, 0, 0));
    assert_eq!(
        body(&engine, "into:D-1").await,
        "The runbook is `/srv/into/board/D-2.md`.\n"
    );
}

/// A document-bearing store whose one document names the project, a task and a second
/// document — so a copy of it walks the destination at all three levels.
///
/// Which level a walk reaches is decided by what the content really names, so a fixture
/// that named only tasks could not drive the other two arms at all.
fn naming_every_level() -> Value {
    json!({
        "capabilities": {"documents": "native"},
        "projects": [located_project("/srv/from/plans/P-1", Some("root:P-1"))],
        "tasks": [located("A", "Alpha", "/srv/from/plans/P-1/A.md", Some("root:A"))],
        "documents": [
            plan_document(
                "Alpha is `/srv/from/plans/P-1/A.md`, the runbook is \
                 `/srv/from/plans/P-1/D-2.md`, and it all lives under \
                 `/srv/from/plans/P-1`.\n",
            ),
            {
                "id": "D-2",
                "title": "Runbook",
                "content": "how to read the plan",
                "project": "P-1",
                "labels": [],
                "location": {"path": "/srv/from/plans/P-1/D-2.md"},
                "metadata": {GlobalId::ORIGIN_KEY: "root:D-2"},
            },
        ],
    })
}

/// A document copy into a destination that misbehaves, and what it refused with.
async fn document_refusal(engine: &Engine) -> String {
    refusal(engine, &many(&["from:D-1"], CopyScope::Documents)).await
}

#[tokio::test]
async fn a_destination_that_overruns_its_page_stops_the_walk_for_a_documents_references() {
    // A document copy asks a destination for a task page in exactly one place — the walk
    // that finds the counterparts its references name — so this is that walk's own guard.
    let (engine, pages) = into_misbehaving_over(
        naming_every_level(),
        Misbehaving::new(At::Tasks, Fault::OverrunsThePage, Onset::FirstRead).with_documents(),
    );

    let refused = document_refusal(&engine).await;

    assert!(
        refused.contains("the source returned 3 rows for a page of at most 2"),
        "{refused}"
    );
    assert!(pages.load(Ordering::Relaxed) <= 3, "the walk stopped early");
}

#[tokio::test]
async fn a_destination_that_repeats_its_cursor_stops_the_walk_for_a_documents_references() {
    let (engine, pages) = into_misbehaving_over(
        naming_every_level(),
        Misbehaving::new(At::Tasks, Fault::RepeatsTheCursor, Onset::FirstRead).with_documents(),
    );

    let refused = document_refusal(&engine).await;

    assert!(
        refused.contains("the destination was being walked for the records a document's"),
        "the refusal names what the walk was doing: {refused}"
    );
    // One page apiece for the document scan and the project and document arms of the walk,
    // then the task arm's first page, which is where the repeated cursor is caught. An
    // unguarded walk would never stop at all.
    assert!(pages.load(Ordering::Relaxed) <= 4, "the walk stopped early");
}

#[tokio::test]
async fn a_destination_that_overruns_its_page_stops_that_walk_at_the_project_level() {
    // The project arm of the same walk, reached because the content names the project's
    // own location. It runs before the scan that files the document under a destination
    // project, so this is the first thing to ask this destination for a project page.
    let (engine, pages) = into_misbehaving_over(
        naming_every_level(),
        Misbehaving::new(At::Projects, Fault::OverrunsThePage, Onset::FirstRead).with_documents(),
    );

    let refused = document_refusal(&engine).await;

    assert!(
        refused.contains("the source returned 3 rows for a page of at most 2"),
        "{refused}"
    );
    assert!(pages.load(Ordering::Relaxed) <= 3, "the walk stopped early");
}

#[tokio::test]
async fn a_destination_that_overruns_its_page_stops_that_walk_at_the_document_level() {
    // The document arm, which `Onset::FirstRead` cannot reach: the copy's own scan for
    // the document's counterpart is the first read of this interface, and the walk is the
    // second. A fault from the first read would stop at the scan every existing test
    // already covers, and say nothing about this walk.
    let (engine, pages) = into_misbehaving_over(
        naming_every_level(),
        Misbehaving::new(At::Documents, Fault::OverrunsThePage, Onset::SecondRead).with_documents(),
    );

    let refused = document_refusal(&engine).await;

    assert!(
        refused.contains("the source returned 3 rows for a page of at most 2"),
        "{refused}"
    );
    assert!(pages.load(Ordering::Relaxed) <= 4, "the walk stopped early");
}

#[tokio::test]
async fn a_source_that_overruns_its_page_stops_the_read_of_a_documents_own_project_members() {
    // Referent discovery reads the document's project, the tasks filed under it and the
    // other documents filed under it. A source that misbehaves during any of those has to
    // stop the copy, not be walked around: this is the task half.
    let (engine, pages) = from_misbehaving_documentary(
        Misbehaving::new(At::Tasks, Fault::OverrunsThePage, Onset::FirstRead)
            .holding_a_project()
            .with_documents(),
    );

    let refused = refusal(&engine, &many(&["from:D-1"], CopyScope::Documents)).await;

    assert!(refused.contains("rows for a page of at most"), "{refused}");
    assert!(pages.load(Ordering::Relaxed) <= 2, "the walk stopped early");
}

#[tokio::test]
async fn a_source_that_overruns_its_page_stops_the_read_of_a_documents_own_project_documents() {
    // And the document half of the same discovery, which is a read this copy did not make
    // before the references it rewrites existed.
    let (engine, pages) = from_misbehaving_documentary(
        Misbehaving::new(At::Documents, Fault::OverrunsThePage, Onset::FirstRead)
            .holding_a_project()
            .with_documents(),
    );

    let refused = refusal(&engine, &many(&["from:D-1"], CopyScope::Documents)).await;

    assert!(refused.contains("rows for a page of at most"), "{refused}");
    assert!(pages.load(Ordering::Relaxed) <= 3, "the walk stopped early");
}

#[tokio::test]
async fn the_reference_figures_are_absent_when_zero_and_read_back_as_zero_when_absent() {
    // The three figures are additive, and this is the whole of what that has to mean on the
    // wire: a copy with nothing to report writes no key, output written before they existed
    // still reads, and a figure that has something to say survives both directions.

    // A report the copy really made, with nothing to say: no `references_*` key at all, so
    // this document is byte-for-byte what a copy emitted before these figures existed.
    let engine = pair();
    let quiet = engine.copy(&one("from:T-1")).await.expect("the copy runs");
    let emitted = serde_json::to_value(&quiet).expect("a copy report serialises");
    let keys: Vec<&String> = emitted
        .as_object()
        .expect("a report is an object")
        .keys()
        .collect();
    assert_eq!(keys, ["items"], "a figure of zero is absent, not nought");

    // Absent reads back as zero rather than refusing, which is what lets a consumer written
    // against the older document hand it to this type unchanged.
    let older: onetaskgraph_core::CopyReport = serde_json::from_value(json!({
        "items": [{"source": "from:T-1", "action": "created", "destination": "into:T-1"}]
    }))
    .expect("a report without the figures still reads");
    assert_eq!(figures(&older), (0, 0, 0));

    // And a figure with something to say is written, survives a round trip, and keeps the
    // ambiguous count at or below the unresolved one it is part of.
    let reported = onetaskgraph_core::CopyReport {
        items: Vec::new(),
        references_rewritten: 3,
        references_unresolved: 2,
        references_ambiguous: 1,
        spent: None,
        delivers_rewritten: 0,
        delivered: Vec::new(),
    };
    let wire = serde_json::to_value(&reported).expect("a copy report serialises");
    assert_eq!(
        wire,
        json!({
            "items": [],
            "references_rewritten": 3,
            "references_unresolved": 2,
            "references_ambiguous": 1,
        })
    );
    let back: onetaskgraph_core::CopyReport = serde_json::from_value(wire).expect("it reads back");
    assert_eq!(back, reported);
    assert!(back.references_ambiguous <= back.references_unresolved);

    // One figure of the three having something to say leaves the other two absent, rather
    // than dragging all three onto the wire together.
    let partial = onetaskgraph_core::CopyReport {
        items: Vec::new(),
        references_rewritten: 2,
        references_unresolved: 0,
        references_ambiguous: 0,
        spent: None,
        delivers_rewritten: 0,
        delivered: Vec::new(),
    };
    assert_eq!(
        serde_json::to_value(&partial).expect("it serialises"),
        json!({"items": [], "references_rewritten": 2})
    );
}

#[tokio::test]
async fn a_destination_whose_cursors_cycle_stops_the_walk_for_a_documents_references() {
    // The cycle `unrepeated` cannot see: two cursors answering each other, so every page
    // advances and no page is ever the one just asked for. Every other walk in this engine
    // meets that defect one level up, as a page token handed back unchanged; this one pages
    // by the destination's own cursor and has no level above it, so its own memory of what
    // it has already asked for is the only thing between this destination and a copy with
    // no end.
    let (engine, pages) = into_misbehaving_over(
        naming_every_level(),
        Misbehaving::new(At::Tasks, Fault::CyclesItsCursors, Onset::FirstRead).with_documents(),
    );

    let refused = document_refusal(&engine).await;

    assert!(
        refused.contains("returned a cursor it had already been given"),
        "the refusal says the walk would never end: {refused}"
    );
    assert!(
        refused.contains("a document's references name"),
        "and names what it was walking for: {refused}"
    );
    assert!(
        pages.load(Ordering::Relaxed) <= 6,
        "the walk stopped early rather than cycling"
    );
}

/// A document copy into a destination answering a question about one origin natively, so the
/// counterparts its references name are asked for one origin at a time rather than walked —
/// and that question's own guards are what stop a destination misbehaving under it.
async fn origin_question_refusal(fault: Fault) -> (String, u32) {
    let (engine, pages) = into_misbehaving_over(
        naming_every_level(),
        Misbehaving::new(At::Tasks, fault, Onset::FirstRead)
            .with_documents()
            .filtering_by_origin(),
    );
    let refused = document_refusal(&engine).await;
    // The pages the copy was served before its task level was asked are the same whatever
    // the fault, so what each case adds to them is the walk it is about.
    (
        refused,
        pages.load(Ordering::Relaxed) - ANSWERED_BEFORE_THE_TASK_LEVEL,
    )
}

/// The pages a document copy over [`naming_every_level`] is served before its task level's
/// question about one origin is asked.
const ANSWERED_BEFORE_THE_TASK_LEVEL: u32 = 3;

#[tokio::test]
async fn a_destination_overrunning_its_page_under_the_origin_question_stops_the_copy() {
    let (refused, pages) = origin_question_refusal(Fault::OverrunsThePage).await;
    assert!(
        refused.contains("the source returned 3 rows for a page of at most 2"),
        "{refused}"
    );
    // The task level's first page, and nothing after it.
    assert_eq!(pages, 1, "the question stopped at the page it refused");
}

#[tokio::test]
async fn a_destination_repeating_its_cursor_under_the_origin_question_stops_the_copy() {
    let (refused, pages) = origin_question_refusal(Fault::RepeatsTheCursor).await;
    assert!(
        refused.contains("the records copied from one origin a document's references name"),
        "the refusal names what the question was for: {refused}"
    );
    // The first page, and the one answering with the cursor it was given.
    assert_eq!(pages, 2, "the question stopped at the cursor it repeated");
}

#[tokio::test]
async fn a_destination_whose_cursors_cycle_under_the_origin_question_stops_the_copy() {
    // Two cursors answering each other: every page advances, so only the question's memory
    // of every cursor it has sent can see it.
    let (refused, pages) = origin_question_refusal(Fault::CyclesItsCursors).await;
    assert!(
        refused.contains(
            "cursor it had already been given while it was being asked for the \
                          records copied from one origin"
        ),
        "the refusal says the question would never end: {refused}"
    );
    // The full half, the empty half and the full half again, whose next cursor is refused
    // before it is sent.
    assert_eq!(pages, 3, "the question stopped rather than cycling");
}

#[tokio::test]
async fn routes_a_rust_caller_builds_place_a_copy_exactly_as_a_configuration_does() {
    // A caller holding sources it built itself states the routes in code, and they are
    // checked by the same rules a document's are.
    let configured = [name("from"), name("into"), name("team")];
    let refused = onetaskgraph_core::config::Routes::new(
        std::collections::BTreeMap::from([(
            name("into"),
            vec![(
                vec!["github.com/nickderobertis/*".to_owned()],
                name("nowhere"),
            )],
        )]),
        &configured,
    )
    .expect_err("a route to no configured source is refused");
    assert!(
        refused.to_string().contains("sources.into.routes.0.to"),
        "{refused}"
    );

    let routes = onetaskgraph_core::config::Routes::new(
        std::collections::BTreeMap::from([(
            name("into"),
            vec![(vec!["github.com/nickderobertis/*".to_owned()], name("team"))],
        )]),
        &configured,
    )
    .expect("valid routes");
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [task("T-1", "Alpha engine")]}},
        "into": {"plugin": "in-memory", "config": {}},
        "team": {"plugin": "in-memory", "config": {}},
    }))
    .with_routes(routes);
    let report = engine.copy(&one("from:T-1")).await.expect("the copy lands");
    let outcome = &report.items[0];
    assert_eq!(
        outcome.destination().map(|landed| landed.source.as_str()),
        Some("team"),
        "{report:?}"
    );
    let placed = outcome
        .placed
        .as_ref()
        .expect("a routed copy names its placement");
    assert_eq!(
        (placed.destination.as_str(), placed.route),
        ("team", Some(0))
    );
}

/// `create` creates each task without asking the destination for a single page, and refuses
/// what contradicts the caller's assertion: a flag that is a way of looking, and an item that
/// itself names a counterpart at the destination — by its link or by its origin.
#[tokio::test]
async fn create_lands_each_task_without_looking_and_refuses_what_says_there_is_a_counterpart() {
    let mut linked = task("T-2", "Linked");
    linked["metadata"] = json!({"onetaskgraph.copies": {"into": "into:HELD"}});
    let mut copied_back = task("T-3", "Copied back");
    copied_back["metadata"] = json!({GlobalId::ORIGIN_KEY: "into:ORIGINAL"});
    let from = json!({"tasks": [task("T-1", "Alpha engine"), linked, copied_back]});

    // Without it, the origin rule scans the destination for a carrier first.
    let (engine, task_pages) = into_counting(from.clone(), json!({}));
    engine.copy(&one("from:T-1")).await.expect("a copy");
    assert!(
        task_pages.load(Ordering::Relaxed) > 0,
        "an ordinary copy looks"
    );

    // With it, the destination is asked for nothing and the task is created, carrying its
    // origin as any created item does.
    let (engine, task_pages) = into_counting(from.clone(), json!({}));
    let created = engine
        .copy(&CopyRequest {
            create: true,
            ..one("from:T-1")
        })
        .await
        .expect("--create creates");
    assert_eq!(
        landed(&created.items[0]),
        (Some("into:T-1".to_owned()), "created".to_owned())
    );
    assert_eq!(
        task_pages.load(Ordering::Relaxed),
        0,
        "--create looks for nothing"
    );
    assert_eq!(listed(&engine, "into").await, ["into:T-1"]);

    // Beside a way of looking, refused before anything is read or written.
    for (request, flag) in [
        (
            CopyRequest {
                create: true,
                match_by: Some(MatchBy::Title),
                ..one("from:T-1")
            },
            "--match-by",
        ),
        (
            CopyRequest {
                create: true,
                recreate: true,
                ..one("from:T-1")
            },
            "--recreate",
        ),
    ] {
        let (engine, task_pages) = into_counting(from.clone(), json!({}));
        let Err(refused) = engine.copy(&request).await else {
            panic!("--create with {flag} must refuse");
        };
        assert!(
            matches!(&refused, EngineError::CreateWith { flag: named } if named.to_string() == flag),
            "{refused:?}"
        );
        assert!(refused.to_string().contains("next:"), "{refused}");
        assert_eq!(task_pages.load(Ordering::Relaxed), 0);
        assert!(listed(&engine, "into").await.is_empty(), "nothing written");
    }

    // An item naming a counterpart at the destination is refused naming it, and nothing is
    // written — a link and an origin alike.
    for (item, carrier) in [("from:T-2", "into:HELD"), ("from:T-3", "into:ORIGINAL")] {
        let (engine, _) = into_counting(from.clone(), json!({}));
        let Err(refused) = engine
            .copy(&CopyRequest {
                create: true,
                ..one(item)
            })
            .await
        else {
            panic!("{item} names {carrier}, so --create must refuse it");
        };
        assert!(
            matches!(&refused, EngineError::CreateCarried { item: named, carrier: held }
                if named.to_string() == item && held.to_string() == carrier),
            "{refused:?}"
        );
        assert!(refused.to_string().contains(carrier), "{refused}");
        assert!(listed(&engine, "into").await.is_empty(), "nothing written");
    }
}

#[tokio::test]
async fn a_destination_never_keeps_the_change_token_a_copied_task_arrives_with() {
    // The source's token is the source's: the destination derives its own on a read — an
    // in-memory source a digest of what it holds — so what a copy lands there never carries the
    // one it arrived with, on the create and on the update over it.
    let mut held = task("T-1", "Alpha engine");
    held["version"] = json!("from-v1");
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [held]}},
        "into": {"plugin": "in-memory", "config": {}},
    }));
    for _ in 0..2 {
        engine.copy(&one("from:T-1")).await.expect("the copy lands");
        let landed = engine
            .tasks(&TaskRequest {
                sources: vec![name("into")],
                filters: onetaskgraph_core::Filters::default(),
                project: onetaskgraph_core::ProjectSelector::Any,
                priorities: Vec::new(),
                commented_since: None,
                metadata: Vec::new(),
                origin: None,
                include_members: false,
                paging: Paging {
                    limit: NonZeroU32::new(50).expect("a non-zero limit"),
                    token: None,
                },
            })
            .await
            .expect("the list verb answers")
            .items;
        assert_eq!(landed.len(), 1);
        let version = landed[0].item.version.as_deref().unwrap_or_default();
        assert!(
            version != "from-v1" && version.starts_with("mem1|"),
            "{:?}",
            landed[0].item
        );
    }
}

/// A source that answers exactly as the `in-memory` source it wraps, except that a write of an
/// item titled one of `malformed` is applied and then answered malformed — the plugin that
/// stored what it was asked to and then could not say under which id — and that a read of the
/// task `misread` names first is answered with the task it names second.
struct Careless {
    inner: Box<dyn TaskSource>,
    malformed: &'static [&'static str],
    misread: Option<(&'static str, &'static str)>,
}

impl Careless {
    /// What a write of an item titled `title` answers, once the wrapped source has written it.
    fn answer(&self, title: &str, written: NativeId) -> Result<NativeId, SourceError> {
        if self.malformed.contains(&title) {
            Err(SourceError::Malformed {
                message: format!("the answer for {title} carries no id"),
            })
        } else {
            Ok(written)
        }
    }
}

#[async_trait::async_trait]
impl TaskSource for Careless {
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
        match self.misread {
            Some((asked, answered)) if id.0 == asked => {
                self.inner.get_task(&NativeId(answered.to_owned())).await
            }
            _ => self.inner.get_task(id).await,
        }
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
        self.inner.query_tasks(query, page).await
    }

    async fn query_projects(
        &self,
        query: &ProjectQuery,
        page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        self.inner.query_projects(query, page).await
    }

    async fn query_documents(
        &self,
        query: &DocumentQuery,
        page: &PageRequest,
    ) -> Result<Page<Document>, SourceError> {
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
        let written = self.inner.write_task(write).await?;
        self.answer(&write.item.title, written)
    }

    async fn write_project(&self, write: &ItemWrite<Project>) -> Result<NativeId, SourceError> {
        let written = self.inner.write_project(write).await?;
        self.answer(&write.item.title, written)
    }

    async fn write_document(&self, write: &ItemWrite<Document>) -> Result<NativeId, SourceError> {
        let written = self.inner.write_document(write).await?;
        self.answer(&write.item.title, written)
    }

    async fn delete_task(&self, id: &NativeId) -> Result<(), SourceError> {
        self.inner.delete_task(id).await
    }

    async fn delete_project(&self, id: &NativeId) -> Result<(), SourceError> {
        self.inner.delete_project(id).await
    }

    async fn delete_document(&self, id: &NativeId) -> Result<(), SourceError> {
        self.inner.delete_document(id).await
    }
}

/// [`interlinked`] as `from`, and `into` an in-memory source holding `config` behind a
/// [`Careless`] answering a write of an item titled one of `malformed` malformed.
fn careless_engine(config: &Value, malformed: &'static [&'static str]) -> Engine {
    let built = |source: &str, config: &Value| {
        onetaskgraph_in_memory::Plugin
            .build(&name(source), config, &NoSecrets)
            .expect("the in-memory plugin builds")
    };
    Engine::new(
        vec![
            ConfiguredSource::Ready(ResolvedSource::adopt(
                name("from"),
                built("from", &interlinked()["config"]),
            )),
            ConfiguredSource::Ready(ResolvedSource::adopt(
                name("into"),
                Box::new(Careless {
                    inner: built("into", config),
                    malformed,
                    misread: None,
                }),
            )),
        ],
        vec![name("from"), name("into")],
    )
}

#[tokio::test]
async fn a_create_answered_malformed_reports_the_destination_uncertain_after_undoing_the_rest() {
    // `Beta` is the second task of `P-1`, so the project and `Alpha engine` have landed, with
    // ids the copy knows, when the destination stores `Beta` and cannot say under which id.
    let engine = careless_engine(&json!({}), &["Beta"]);
    let Err(refused) = engine
        .copy(&many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
    else {
        panic!("a create answered malformed must fail the copy");
    };
    let EngineError::CopyDestinationUncertain {
        error,
        destination,
        operation,
    } = &refused
    else {
        panic!("a malformed create answer leaves the destination uncertain: {refused}");
    };
    assert_eq!(destination, "into");
    assert_eq!(operation, "write_task");
    assert!(
        matches!(
            error.as_ref(),
            EngineError::SourceRefused { name, error: SourceError::Malformed { .. } }
                if name == "into"
        ),
        "{error:?}"
    );
    let rendered = refused.to_string();
    assert!(
        rendered.ends_with(
            "next: inspect into for an item this copy may have created and remove it before \
             copying again."
        ),
        "{rendered}"
    );
    // Every write the copy could identify is undone; what is left is the one it could not.
    assert_eq!(
        held(&engine, "into").await,
        vec!["into:T-2 Beta".to_owned()]
    );
}

#[tokio::test]
async fn an_update_answered_malformed_keeps_the_refusal_it_always_was() {
    // An update is journalled before it is sent, so a malformed answer to one leaves nothing
    // the copy cannot name, and the destination is put back exactly as it was.
    let engine = careless_engine(&already_holding(), &["Beta"]);
    let before = held(&engine, "into").await;
    let Err(refused) = engine
        .copy(&many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
    else {
        panic!("an update answered malformed must fail the copy");
    };
    assert!(
        matches!(
            &refused,
            EngineError::SourceRefused { name, error: SourceError::Malformed { .. } }
                if name == "into"
        ),
        "{refused:?}"
    );
    assert_eq!(held(&engine, "into").await, before);
}

#[tokio::test]
async fn a_project_create_answered_malformed_after_another_project_landed_undoes_that_one() {
    // `P-1` and both its tasks land first; `Docs`, the second project, is the create the
    // destination stores and answers malformed.
    let engine = careless_engine(&json!({}), &["Docs"]);
    let Err(refused) = engine
        .copy(&many(
            &["from:P-1", "from:P-2"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
    else {
        panic!("a create answered malformed must fail the copy");
    };
    let EngineError::CopyDestinationUncertain {
        destination,
        operation,
        ..
    } = &refused
    else {
        panic!("a malformed create answer leaves the destination uncertain: {refused}");
    };
    assert_eq!(
        (destination.as_str(), operation.as_str()),
        ("into", "write_project")
    );
    assert_eq!(
        held(&engine, "into").await,
        vec!["into:P-2 Docs".to_owned()]
    );
}

#[tokio::test]
async fn a_malformed_create_whose_undo_is_refused_names_both_what_it_left_and_what_it_cannot() {
    // The destination will not take back `P-1`, written before `Beta`'s malformed answer. The
    // copy then owes both halves: the item it could identify and could not remove, and the
    // one it could not identify at all.
    let engine = careless_engine(
        &json!({"capabilities": {"undeletable_ids": ["P-1"]}}),
        &["Beta"],
    );
    let Err(refused) = engine
        .copy(&many(
            &["from:P-1"],
            CopyScope::Projects {
                tasks: true,
                documents: false,
            },
        ))
        .await
    else {
        panic!("a create answered malformed must fail the copy");
    };
    let EngineError::CopyNotUndone {
        error, left_behind, ..
    } = &refused
    else {
        panic!("a refused undo reports the copy as not undone: {refused}");
    };
    assert!(
        matches!(
            error.as_ref(),
            EngineError::CopyDestinationUncertain { destination, operation, .. }
                if destination == "into" && operation == "write_task"
        ),
        "{error:?}"
    );
    assert_eq!(
        left_behind
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["into:P-1".to_owned()]
    );
    let rendered = refused.to_string();
    assert!(
        rendered.contains("into may hold an item it could not take back"),
        "{rendered}"
    );
    assert!(
        rendered.contains("so these still hold what it wrote: into:P-1"),
        "{rendered}"
    );
    assert_eq!(
        held(&engine, "into").await,
        vec!["into:P-1 Engine".to_owned(), "into:T-2 Beta".to_owned()]
    );
}

#[tokio::test]
async fn members_only_routes_into_an_existing_member_project_and_refuses_a_missing_one() {
    for present in [false, true] {
        let mut filed = task("T-1", "Routed task");
        filed["project"] = json!("P-1");
        let project = |native: &str, metadata: Value| {
            json!({
                "id": native, "title": "Project", "labels": [],
                "status": {"category": "todo", "name": "Todo"}, "metadata": metadata
            })
        };
        let member_projects = if present {
            vec![project(
                "MEMBER",
                json!({"onetaskgraph.member_of": "into:HOME"}),
            )]
        } else {
            vec![]
        };
        let engine = engine_over(json!({
            "from": {"plugin": "in-memory", "config": {
                "projects": [project("P-1", json!({"onetaskgraph.copies": {"into": "into:HOME"}}))],
                "tasks": [filed]
            }},
            "into": {"plugin": "in-memory", "config": {"projects": [project("HOME", json!({}))]}},
            "team": {"plugin": "in-memory", "config": {"projects": member_projects}}
        }))
        .with_routes(
            onetaskgraph_core::config::Routes::new(
                std::collections::BTreeMap::from([(
                    name("into"),
                    vec![(vec!["github.com/nickderobertis/*".to_owned()], name("team"))],
                )]),
                &[name("from"), name("into"), name("team")],
            )
            .expect("valid routes"),
        );
        let mut request = one("from:P-1");
        request.scope =
            CopyScope::MembersOnly(CopyItems::new(vec![id("from:T-1")]).expect("one member"));
        let result = engine.copy(&request).await;
        if present {
            let report = result.expect("the routed member lands");
            assert_eq!(report.items.len(), 1);
            assert_eq!(report.items[0].destination(), Some(&id("team:T-1")));
            let shown = engine.task(&id("team:T-1")).await.expect("the copied task");
            assert_eq!(shown.items[0].item.project, Some(NativeId::from("MEMBER")));
        } else {
            assert!(
                matches!(result, Err(EngineError::NoProjectCounterpart { missing, .. }) if missing == name("team"))
            );
            assert!(listed(&engine, "team").await.is_empty());
        }
        assert!(listed(&engine, "into").await.is_empty());
    }
}

#[tokio::test]
async fn a_members_only_task_without_repositories_follows_its_project_into_a_routed_source() {
    let project = |native: &str, metadata: Value| {
        json!({"id": native, "title": "Project", "labels": [],
        "status": {"category": "todo", "name": "Todo"}, "metadata": metadata})
    };
    let mut filed = task("T-1", "No repository");
    filed["project"] = json!("P-1");
    filed["repositories"] = json!([]);
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "config": {"tasks": [filed],
            "projects": [project("P-1", json!({"onetaskgraph.copies": {"team": "team:HOME"}}))]}},
        "into": {"plugin": "in-memory", "config": {}},
        "team": {"plugin": "in-memory", "config": {"projects": [project("HOME", json!({}))]}}
    }))
    .with_routes(
        onetaskgraph_core::config::Routes::new(
            std::collections::BTreeMap::from([(
                name("into"),
                vec![(vec!["github.com/nickderobertis/*".to_owned()], name("team"))],
            )]),
            &[name("from"), name("into"), name("team")],
        )
        .expect("routes"),
    );
    let mut request = one("from:P-1");
    request.scope = CopyScope::MembersOnly(CopyItems::new(vec![id("from:T-1")]).expect("member"));
    let report = engine.copy(&request).await.expect("copy lands");
    assert_eq!(report.items[0].destination(), Some(&id("team:T-1")));
    assert_eq!(
        engine.task(&id("team:T-1")).await.expect("task").items[0]
            .item
            .project,
        Some(NativeId::from("HOME"))
    );
    assert!(listed(&engine, "into").await.is_empty());
}

#[tokio::test]
async fn a_rust_caller_preserves_status_then_intentionally_copies_it_with_the_same_source_token() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("tasks")).unwrap();
    std::fs::write(
        directory.path().join("tasks/T-1.md"),
        "---\ntitle: Step\nstatus: todo\n---\noriginal body\n",
    )
    .unwrap();
    let engine = engine_over(json!({
        "from": {"plugin": "local-md", "config": {"root": directory.path()}},
        "into": {"plugin": "in-memory", "config": {"capabilities": {"comments": "native"}}},
    }));
    let mut request = one("from:T-1");
    request.comments = true;
    request.no_link = true;
    request.status_policy = onetaskgraph_core::StatusPolicy::PreserveDestination;
    engine.copy(&request).await.unwrap();
    engine
        .set_task_status(&id("into:T-1"), StatusCategory::Done)
        .await
        .unwrap();
    engine
        .set_task_content(&id("from:T-1"), "a revised body")
        .await
        .unwrap();
    engine.copy(&request).await.unwrap();
    let held = engine.task(&id("into:T-1")).await.unwrap();
    assert_eq!(held.items[0].item.status.category, StatusCategory::Done);
    assert_eq!(
        held.items[0].item.content.as_deref(),
        Some("a revised body")
    );
    assert!(
        engine.copy(&request).await.unwrap().items[0]
            .action
            .skipped()
    );
    request.status_policy = onetaskgraph_core::StatusPolicy::default();
    engine.copy(&request).await.unwrap();
    let held = engine.task(&id("into:T-1")).await.unwrap();
    assert_eq!(held.items[0].item.status.category, StatusCategory::Todo);
}

/// A linking caller's write policy that finds nothing private in any write: what leaves the
/// copy's own withholding as the only thing keeping a private id off a public write.
struct NothingPrivate;

#[async_trait::async_trait]
impl onetaskgraph_core::boundary::WritePolicy for NothingPrivate {
    async fn repository_visibility(
        &self,
        _repository: &onetaskgraph_plugin_api::Repository,
    ) -> Result<
        onetaskgraph_core::boundary::RepositoryVisibility,
        onetaskgraph_core::boundary::PolicyError,
    > {
        Ok(onetaskgraph_core::boundary::RepositoryVisibility::Public)
    }

    async fn check_public_write(
        &self,
        _input: &onetaskgraph_core::boundary::PublicWriteInput,
    ) -> Result<onetaskgraph_core::boundary::WriteVerdict, onetaskgraph_core::boundary::PolicyError>
    {
        Ok(onetaskgraph_core::boundary::WriteVerdict::Pass)
    }
}

#[tokio::test]
async fn the_edges_a_copy_withheld_are_taken_once_and_each_copy_reports_its_own() {
    // `T-1` depends on an item of `ledger`, a source declared private: a copy into the public
    // `into` leaves that edge out and notes it, and a Rust caller taking the notes twice gets
    // them once — the next copy's notes are its own, not the first's again beside them.
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "visibility": "public", "config": {
            "tasks": [{"id": "T-1", "title": "Consumer",
                       "status": {"category": "todo", "name": "Todo"}, "labels": []}],
            "task_dependencies": [
                {"from": {"id": "T-1", "kind": "task"},
                 "to": {"id": "ledger:LED-1", "kind": "task"}, "kind": "blocks"},
            ],
        }},
        "into": {"plugin": "in-memory", "visibility": "public", "config": {}},
        "ledger": {"plugin": "in-memory", "visibility": "private", "config": {}},
    }))
    .with_write_policy(Arc::new(NothingPrivate));
    let withheld = "withheld from:T-1's dependency on ledger:LED-1 from into: its source \
                    ledger is declared private, and its source named no tracker an opaque \
                    reference to it could point into";

    engine.copy(&one("from:T-1")).await.expect("the copy runs");
    assert_eq!(engine.take_withheld_edges(), vec![withheld.to_owned()]);
    assert_eq!(
        engine.take_withheld_edges(),
        Vec::<String>::new(),
        "nothing is left to take once taken"
    );

    engine
        .copy(&one("from:T-1"))
        .await
        .expect("the re-copy runs");
    assert_eq!(
        engine.take_withheld_edges(),
        vec![withheld.to_owned()],
        "the re-copy reports its own withheld edge alone"
    );
}

#[tokio::test]
async fn a_copy_records_its_sources_root_and_never_one_naming_a_private_source_where_it_is_not() {
    // `onetaskgraph.lineage` carries the root of a chain of copies from hop to hop, so an item's
    // root is one read of it. A source item recording an origin of its own hands that root on;
    // one whose root is a private source's hands nothing on to a destination not declared
    // private, because the lineage would name that source where it may not be named — and a
    // destination declared private takes it.
    let status = json!({"category": "todo", "name": "Todo"});
    let engine = engine_over(json!({
        "from": {"plugin": "in-memory", "visibility": "public", "config": {"tasks": [
            {"id": "T-1", "title": "Rooted in public", "status": status, "labels": [],
             "metadata": {"onetaskgraph.origin": "root:R-1"}},
            {"id": "T-2", "title": "Rooted in the vault", "status": status, "labels": [],
             "metadata": {"onetaskgraph.origin": "vault:V-1"}},
            {"id": "T-3", "title": "Copied from nowhere", "status": status, "labels": []},
        ]}},
        "into": {"plugin": "in-memory", "visibility": "public", "config": {}},
        "sealed": {"plugin": "in-memory", "visibility": "private", "config": {}},
        "root": {"plugin": "in-memory", "visibility": "public", "config": {}},
        "vault": {"plugin": "in-memory", "visibility": "private", "config": {}},
    }))
    .with_write_policy(Arc::new(NothingPrivate));
    let recorded = async |request: CopyRequest| {
        let copied = engine.copy(&request).await.expect("the copy runs");
        let landed = copied.items[0].destination().expect("it landed").clone();
        let metadata = engine
            .task(&landed)
            .await
            .expect("the show verb answers")
            .items[0]
            .item
            .metadata
            .clone();
        (
            metadata.get(GlobalId::ORIGIN_KEY).cloned(),
            metadata.get(GlobalId::LINEAGE_KEY).cloned(),
        )
    };

    assert_eq!(
        recorded(one("from:T-1")).await,
        (Some(json!("from:T-1")), Some(json!("root:R-1"))),
        "a second hop records the root its source came from"
    );
    assert_eq!(
        recorded(one("from:T-3")).await,
        (Some(json!("from:T-3")), None),
        "a first hop's origin is its root"
    );
    assert_eq!(
        recorded(one("from:T-2")).await,
        (Some(json!("from:T-2")), None),
        "a root naming a private source is not written where the write is not private"
    );
    let mut sealed = one("from:T-2");
    sealed.destination = name("sealed");
    assert_eq!(
        recorded(sealed).await,
        (Some(json!("from:T-2")), Some(json!("vault:V-1"))),
        "a destination declared private takes it"
    );
}
