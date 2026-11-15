//! Producer intents use the real filesystem and OS locks; tracker evidence is passed at
//! the public journal boundary, never through a substitute engine.
use std::process::Command;

use onetaskgraph_core::task_projection::{
    HumanTaskProjected, HumanTaskProjection, IntentPhase, IntentStore, ProjectionPlacement,
    ProjectionState,
};
use onetaskgraph_plugin_api::{Classification, SourceName, Task};
use serde_json::json;
use tempfile::TempDir;

fn request() -> HumanTaskProjection {
    serde_json::from_value(json!({
        "schema_version":1,"item_id":"host/session/item",
        "owner":{"session":"session","run":null,"plan":null},
        "title":"Review release","body":"Decide whether to release.","priority":"high",
        "repositories":["github.com/operator/product"],"project":"P-1","source":"plans",
        "state":"open","reason":null
    }))
    .expect("v1")
}

fn placement() -> ProjectionPlacement {
    ProjectionPlacement {
        identity: onetaskgraph_core::task_projection::SourceIdentityDigest::new("a".repeat(64))
            .expect("source digest"),
        source: SourceName::new("human".to_owned()).expect("source"),
        project: onetaskgraph_core::task_projection::ProjectionProjectId::new("P-1")
            .expect("project"),
        classification: Classification::Public,
    }
}

fn store(root: &std::path::Path) -> IntentStore {
    IntentStore::new(root.to_path_buf(), "host".to_owned(), "host".to_owned()).expect("store")
}

fn binding() -> HumanTaskProjected {
    serde_json::from_value(json!({
        "task":"human:ISSUE-1","url":"https://github.com/operator/product/issues/1",
        "project":"P-1","source":"human"
    }))
    .expect("binding")
}

#[test]
fn intent_process() {
    let Some(root) = std::env::var_os("OTG_INTENT_CHILD_ROOT") else {
        return;
    };
    let store = store(std::path::Path::new(&root));
    let mut claim = store.claim(&request(), &placement()).expect("claim");
    match std::env::var("OTG_INTENT_CHILD_MODE")
        .expect("mode")
        .as_str()
    {
        "crash-before-create" => std::process::exit(71),
        "crash-after-create-claim" => {
            claim.begin_create().expect("durable creating");
            std::process::exit(72);
        }
        "crash-after-bind" => {
            claim.begin_create().expect("durable creating");
            claim.bind(binding()).expect("durable binding");
            std::process::exit(73);
        }
        "contender" => match claim.begin_create() {
            Ok(()) => std::process::exit(74),
            Err(_) => std::process::exit(75),
        },
        _ => panic!("unknown child mode"),
    }
}

fn child(root: &std::path::Path, mode: &str) -> std::process::Child {
    Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "intent_process", "--nocapture"])
        .env("OTG_INTENT_CHILD_ROOT", root)
        .env("OTG_INTENT_CHILD_MODE", mode)
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("child")
}

#[test]
fn process_crashes_preserve_create_authority_and_binding() {
    for (mode, exit, expected) in [
        ("crash-before-create", 71, IntentPhase::Ready),
        ("crash-after-create-claim", 72, IntentPhase::Creating),
        ("crash-after-bind", 73, IntentPhase::Bound),
    ] {
        let root = TempDir::new().expect("root");
        assert_eq!(
            child(root.path(), mode).wait().expect("child exit").code(),
            Some(exit)
        );
        let mut claim = store(root.path())
            .claim(&request(), &placement())
            .expect("fresh process view");
        assert_eq!(claim.phase(), expected);
        if expected == IntentPhase::Ready {
            claim
                .begin_create()
                .expect("no create could have been sent");
        } else {
            assert!(
                claim.begin_create().is_err(),
                "a retry never authorizes another create"
            );
        }
        if expected == IntentPhase::Bound {
            assert_eq!(claim.binding(), Some(&binding()));
        }
    }
}

#[test]
fn simultaneous_local_owners_serialize_before_authorizing_create() {
    let root = TempDir::new().expect("root");
    let mut first = child(root.path(), "contender");
    let mut second = child(root.path(), "contender");
    let mut exits = [
        first.wait().expect("first").code().expect("code"),
        second.wait().expect("second").code().expect("code"),
    ];
    exits.sort_unstable();
    assert_eq!(
        exits,
        [74, 75],
        "only one process receives create authority"
    );
}

#[test]
fn terminal_intent_is_immutable_and_absent_reason_stays_absent() {
    for reason in [None, Some("Superseded".to_owned())] {
        let root = TempDir::new().expect("root");
        let mut request = request();
        let store = store(root.path());
        let mut claim = store.claim(&request, &placement()).expect("claim");
        claim.begin_create().expect("create");
        claim.bind(binding()).expect("bind");
        claim
            .begin_terminal(ProjectionState::Dismissed, reason.clone())
            .expect("terminal");
        assert!(!claim.terminal_acknowledged());
        drop(claim);
        request.state = ProjectionState::Dismissed;
        request.reason = reason;
        let mut claim = store
            .claim(&request, &placement())
            .expect("resume terminal");
        claim.acknowledge_terminal().expect("acknowledge");
        drop(claim);
        let claim = store.claim(&request, &placement()).expect("no-op");
        assert!(claim.terminal_acknowledged());
        assert_eq!(claim.binding(), Some(&binding()));
        drop(claim);
        request.state = ProjectionState::Completed;
        assert!(
            store.claim(&request, &placement()).is_err(),
            "conflicting terminal refuses"
        );
    }
}

#[test]
fn changed_payload_route_privacy_and_foreign_host_refuse() {
    let root = TempDir::new().expect("root");
    let store = store(root.path());
    drop(
        store
            .claim(&request(), &placement())
            .expect("initial intent"),
    );
    let mut changed = request();
    changed.body = "Changed content".to_owned();
    assert!(store.claim(&changed, &placement()).is_err());
    let mut changed_route = placement();
    changed_route.project =
        onetaskgraph_core::task_projection::ProjectionProjectId::new("P-2").expect("project");
    assert!(store.claim(&request(), &changed_route).is_err());
    let mut changed_privacy = placement();
    changed_privacy.classification = Classification::Private;
    assert!(store.claim(&request(), &changed_privacy).is_err());
    let foreign_root = root.path().join("foreign");
    let foreign = IntentStore::new(foreign_root.clone(), "host".to_owned(), "other".to_owned())
        .expect("explicit authority");
    let error = foreign
        .claim(&request(), &placement())
        .expect_err("foreign host")
        .to_string();
    assert!(error.contains("host") && error.contains("other") && error.contains("task_projection"));
    assert!(!foreign_root.exists(), "authority refusal writes nothing");
}

fn candidate(claim: &onetaskgraph_core::task_projection::ProjectionClaim) -> Task {
    serde_json::from_value(json!({
        "id":"ISSUE-1","title":"Review release","content":"Decide whether to release.",
        "priority":"high","repositories":["github.com/operator/product"],
        "status":{"category":"todo","name":"Todo"},
        "labels":[],"project":"P-1","url":"https://github.com/operator/product/issues/1",
        "metadata":{
            "human.item_id":claim.item_id(),
            "human.owner":request().owner,
            "human.host":claim.host(),
            "human.payload_digest":claim.payload_digest(),
            onetaskgraph_core::GlobalId::ORIGIN_KEY:claim.origin()
        }
    }))
    .expect("tracker evidence")
}

#[test]
fn only_one_verified_indexed_origin_match_can_bind_a_pending_create() {
    let root = TempDir::new().expect("root");
    let store = store(root.path());
    let mut claim = store.claim(&request(), &placement()).expect("claim");
    claim.begin_create().expect("create may have happened");
    let task = candidate(&claim);
    assert!(claim.reconcile(&request(), &[]).is_err());
    assert!(
        claim
            .reconcile(&request(), &[task.clone(), task.clone()])
            .is_err()
    );
    for key in [
        "human.item_id",
        "human.payload_digest",
        "human.host",
        "onetaskgraph.origin",
    ] {
        let mut changed = task.clone();
        changed.metadata.insert(key.to_owned(), json!("different"));
        assert!(claim.reconcile(&request(), &[changed]).is_err(), "{key}");
    }
    let mut unindexed = task.clone();
    unindexed.project = None;
    assert!(claim.reconcile(&request(), &[unindexed]).is_err());
    assert_eq!(claim.phase(), IntentPhase::Creating);
    claim
        .reconcile(&request(), &[task])
        .expect("one verified positive match");
    assert_eq!(claim.binding(), Some(&binding()));
    assert!(claim.begin_create().is_err());
}

#[test]
fn untrusted_placement_cannot_escape_the_intent_directory() {
    let root = TempDir::new().expect("root");
    for invalid in ["../escape".to_owned(), "A".repeat(64), "g".repeat(64)] {
        let mut changed = serde_json::to_value(placement()).expect("placement");
        changed["identity"] = json!(invalid);
        assert!(serde_json::from_value::<ProjectionPlacement>(changed).is_err());
    }
    assert_eq!(
        std::fs::read_dir(root.path()).expect("directory").count(),
        0
    );
}

#[test]
fn invalid_tracker_responses_never_become_durable_card_bindings() {
    let root = TempDir::new().expect("root");
    let store = store(root.path());
    let mut claim = store.claim(&request(), &placement()).expect("claim");
    assert!(
        claim.bind(binding()).is_err(),
        "create authority must be committed first"
    );
    claim.begin_create().expect("begin create");
    for (field, value) in [
        ("task", "other:ISSUE-1"),
        ("task", "not-qualified"),
        ("source", "other"),
        ("project", "P-2"),
        ("url", "javascript:alert(1)"),
        ("url", "https://example.com/ bad"),
    ] {
        let mut invalid = serde_json::to_value(binding()).expect("binding");
        invalid[field] = json!(value);
        if let Ok(invalid) = serde_json::from_value(invalid) {
            assert!(claim.bind(invalid).is_err());
        }
    }
    claim.bind(binding()).expect("canonical response");
    claim.bind(binding()).expect("identical binding is a no-op");
    let held = binding();
    let different = HumanTaskProjected::new(
        onetaskgraph_core::task_projection::ProjectedTaskId::new("human:ISSUE-2").expect("task id"),
        held.url().clone(),
        held.project().clone(),
    );
    assert!(claim.bind(different).is_err(), "a claim cannot be rebound");
}

#[test]
fn invalid_authority_and_blank_titles_refuse_before_persistence() {
    let root = TempDir::new().expect("root");
    for (host, current) in [
        ("", "host"),
        ("bad:host", "host"),
        ("host", ""),
        ("host", "bad host"),
    ] {
        assert!(
            IntentStore::new(
                root.path().to_path_buf(),
                host.to_owned(),
                current.to_owned()
            )
            .is_err()
        );
    }
    assert!(IntentStore::new("relative".into(), "host".to_owned(), "host".to_owned()).is_err());
    let mut blank = serde_json::to_value(request()).expect("request");
    blank["title"] = json!("  ");
    assert!(serde_json::from_value::<HumanTaskProjection>(blank).is_err());
    assert_eq!(
        std::fs::read_dir(root.path()).expect("directory").count(),
        0
    );
}

fn journal_path(root: &std::path::Path) -> std::path::PathBuf {
    std::fs::read_dir(root.join("a".repeat(64)))
        .expect("source directory")
        .map(|entry| entry.expect("entry").path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("committed journal")
}

#[test]
fn corrupted_persisted_evidence_never_authorizes_a_create_or_noop() {
    let root = TempDir::new().expect("root");
    let store = store(root.path());
    let mut claim = store.claim(&request(), &placement()).expect("claim");
    claim.begin_create().expect("create");
    claim.bind(binding()).expect("binding");
    drop(claim);
    let path = journal_path(root.path());
    let original = std::fs::read(&path).expect("journal");
    let document: serde_json::Value = serde_json::from_slice(&original).expect("journal JSON");
    for (pointer, value) in [
        ("/schema_version", json!(2)),
        (
            "/origin",
            json!(format!("wrong-purpose:host:{}", "a".repeat(64))),
        ),
        ("/origin", json!("human-task")),
        ("/origin", json!("human-task:host")),
        (
            "/origin",
            json!(format!("human-task:bad host:{}", "a".repeat(64))),
        ),
        (
            "/origin",
            json!(format!("human-task:host:{}:extra", "a".repeat(64))),
        ),
        ("/payload_digest", json!("G".repeat(64))),
        ("/item_id", json!("different-item")),
        ("/host", json!("different-host")),
        (
            "/origin",
            json!(format!("human-task:host:{}", "b".repeat(64))),
        ),
        ("/payload_digest", json!("b".repeat(64))),
        ("/phase/binding/task", json!("unqualified")),
        ("/phase/binding/project", json!("P-2")),
        (
            "/phase/binding",
            json!({
                "task":"other:ISSUE-1","source":"other","project":"P-1",
                "url":"https://github.com/operator/product/issues/1"
            }),
        ),
        ("/phase/binding/source", json!("different")),
        ("/phase/binding/url", json!("javascript:alert(1)")),
        ("/phase", json!({"kind":"bound"})),
        ("/phase", json!({"kind":"ready","binding":binding()})),
        (
            "/phase/terminal",
            json!({"state":"open","reason":null,"acknowledged":true}),
        ),
    ] {
        let mut changed = document.clone();
        if pointer == "/phase/terminal" {
            changed["phase"]["terminal"] = value;
        } else {
            *changed.pointer_mut(pointer).expect("existing field") = value;
        }
        std::fs::write(&path, serde_json::to_vec(&changed).expect("JSON"))
            .expect("fault injection");
        assert!(store.claim(&request(), &placement()).is_err(), "{pointer}");
    }
    std::fs::write(&path, b"{malformed").expect("malformed journal");
    assert!(store.claim(&request(), &placement()).is_err());
    std::fs::write(&path, original).expect("restore journal");
    let claim = store
        .claim(&request(), &placement())
        .expect("restored binding");
    assert_eq!(claim.binding(), Some(&binding()));
}

#[test]
fn failed_phase_commit_returns_no_create_authority_and_retries_safely() {
    let root = TempDir::new().expect("root");
    let store = store(root.path());
    let mut claim = store
        .claim(&request(), &placement())
        .expect("initial intent");
    let obstruction = journal_path(root.path()).with_extension("pending");
    std::fs::create_dir(&obstruction).expect("obstruct atomic write");
    assert!(
        claim.begin_create().is_err(),
        "failed persistence returns no authority to send create"
    );
    drop(claim);
    std::fs::remove_dir(obstruction).expect("remove obstruction");
    let mut retry = store
        .claim(&request(), &placement())
        .expect("retry committed intent");
    retry
        .begin_create()
        .expect("no earlier create could have been authorized");
    drop(retry);
    let mut fresh = store.claim(&request(), &placement()).expect("fresh view");
    assert!(
        fresh.begin_create().is_err(),
        "successful phase commit cannot authorize twice"
    );
}

#[test]
fn terminal_transitions_require_a_bound_task_and_preserve_completion() {
    let root = TempDir::new().expect("root");
    let store = store(root.path());
    let mut request = request();
    let mut claim = store.claim(&request, &placement()).expect("claim");
    assert!(claim.acknowledge_terminal().is_err());
    assert!(
        claim
            .begin_terminal(ProjectionState::Completed, None)
            .is_err()
    );
    claim.begin_create().expect("create");
    claim.bind(binding()).expect("bind");
    assert!(claim.acknowledge_terminal().is_err());
    assert!(claim.begin_terminal(ProjectionState::Open, None).is_err());
    claim
        .begin_terminal(ProjectionState::Completed, None)
        .expect("completion");
    claim
        .begin_terminal(ProjectionState::Completed, None)
        .expect("same terminal is a no-op");
    assert!(
        claim
            .begin_terminal(ProjectionState::Dismissed, None)
            .is_err()
    );
    assert!(
        claim
            .begin_terminal(ProjectionState::Completed, Some("changed".to_owned()))
            .is_err()
    );
    claim
        .acknowledge_terminal()
        .expect("acknowledge completion");
    drop(claim);
    request.state = ProjectionState::Completed;
    let resumed = store
        .claim(&request, &placement())
        .expect("cold completion retry");
    assert!(resumed.terminal_acknowledged());
}

#[test]
fn reconciliation_refuses_unverifiable_ticket_fields_and_wrong_phases() {
    let root = TempDir::new().expect("root");
    let store = store(root.path());
    let mut claim = store.claim(&request(), &placement()).expect("claim");
    let task = candidate(&claim);
    assert!(
        claim
            .reconcile(&request(), std::slice::from_ref(&task))
            .is_err(),
        "no create sent yet"
    );
    claim.begin_create().expect("create may have happened");
    for (field, value) in [
        ("classification", json!("private")),
        ("url", serde_json::Value::Null),
        ("url", json!("javascript:alert(1)")),
        ("id", json!("")),
        ("priority", json!("none")),
        ("title", json!("Different")),
        ("content", json!("Different")),
        ("repositories", json!([])),
    ] {
        let mut invalid = serde_json::to_value(&task).expect("task");
        invalid[field] = value;
        assert!(
            claim
                .reconcile(
                    &request(),
                    &[serde_json::from_value(invalid).expect("tracker answer")]
                )
                .is_err(),
            "{field}"
        );
    }
    for owner in [
        None,
        Some(json!({"session":"other","run":null,"plan":null})),
    ] {
        let mut changed = task.clone();
        match owner {
            Some(owner) => {
                changed.metadata.insert("human.owner".to_owned(), owner);
            }
            None => {
                changed.metadata.remove("human.owner");
            }
        }
        assert!(claim.reconcile(&request(), &[changed]).is_err());
    }
    let mut changed_request = request();
    changed_request.title =
        onetaskgraph_core::task_projection::ProjectionTitle::new("Changed intent").expect("title");
    assert!(
        claim
            .reconcile(&changed_request, std::slice::from_ref(&task))
            .is_err()
    );
    claim
        .reconcile(&request(), std::slice::from_ref(&task))
        .expect("one verified answer");
    assert!(
        claim.reconcile(&request(), &[task]).is_err(),
        "already bound"
    );
}

#[test]
fn failed_binding_commit_cannot_acknowledge_or_skip_a_later_commit() {
    let root = TempDir::new().expect("root");
    let store = store(root.path());
    let mut claim = store.claim(&request(), &placement()).expect("claim");
    claim.begin_create().expect("create");
    let obstruction = journal_path(root.path()).with_extension("pending");
    std::fs::create_dir(&obstruction).expect("obstruct binding commit");
    assert!(claim.bind(binding()).is_err());
    assert_eq!(claim.phase(), IntentPhase::Uncertain);
    assert_eq!(
        claim.binding(),
        None,
        "an uncommitted binding cannot be acknowledged"
    );
    assert!(
        claim.bind(binding()).is_err(),
        "same live claim must reopen after failed commit"
    );
    std::fs::remove_dir(obstruction).expect("remove obstruction");
    drop(claim);
    let mut reopened = store
        .claim(&request(), &placement())
        .expect("reopen pending create");
    assert_eq!(reopened.binding(), None);
    assert!(
        reopened.begin_create().is_err(),
        "a lost binding never authorizes another create"
    );
    reopened.bind(binding()).expect("commit original binding");
    assert_eq!(reopened.binding(), Some(&binding()));
}

#[test]
fn failed_terminal_intent_commit_requires_reopening_before_tracker_authority() {
    let root = TempDir::new().expect("root");
    let store = store(root.path());
    let mut claim = store.claim(&request(), &placement()).expect("claim");
    claim.begin_create().expect("create");
    claim.bind(binding()).expect("bind");
    let obstruction = journal_path(root.path()).with_extension("pending");
    std::fs::create_dir(&obstruction).expect("obstruct terminal commit");
    assert!(
        claim
            .begin_terminal(ProjectionState::Completed, None)
            .is_err()
    );
    assert!(
        claim
            .begin_terminal(ProjectionState::Completed, None)
            .is_err()
    );
    assert!(!claim.terminal_acknowledged());
    std::fs::remove_dir(obstruction).expect("remove obstruction");
    drop(claim);
    let mut reopened = store
        .claim(&request(), &placement())
        .expect("reopen binding");
    reopened
        .begin_terminal(ProjectionState::Completed, None)
        .expect("commit terminal intent");
}

#[test]
fn failed_acknowledgment_never_reports_a_durable_terminal_noop() {
    let root = TempDir::new().expect("root");
    let store = store(root.path());
    let mut request = request();
    let mut claim = store.claim(&request, &placement()).expect("claim");
    claim.begin_create().expect("create");
    claim.bind(binding()).expect("bind");
    claim
        .begin_terminal(ProjectionState::Completed, None)
        .expect("terminal intent");
    let obstruction = journal_path(root.path()).with_extension("pending");
    std::fs::create_dir(&obstruction).expect("obstruct acknowledgment");
    assert!(claim.acknowledge_terminal().is_err());
    assert!(!claim.terminal_acknowledged());
    assert!(claim.acknowledge_terminal().is_err());
    std::fs::remove_dir(obstruction).expect("remove obstruction");
    drop(claim);
    request.state = ProjectionState::Completed;
    let mut reopened = store
        .claim(&request, &placement())
        .expect("reopen terminal intent");
    assert!(
        !reopened.terminal_acknowledged(),
        "failed acknowledgment did not commit"
    );
    reopened
        .acknowledge_terminal()
        .expect("durable acknowledgment");
    assert!(reopened.terminal_acknowledged());
}

#[cfg(unix)]
#[test]
fn new_intent_directories_and_all_write_artifacts_are_private() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = TempDir::new().expect("root");
    let directory = root.path().join("private-store");
    let store = store(&directory);
    let mut claim = store.claim(&request(), &placement()).expect("claim");
    let source = directory.join("a".repeat(64));
    for path in [&directory, &source] {
        assert_eq!(
            std::fs::metadata(path)
                .expect("directory")
                .permissions()
                .mode()
                & 0o077,
            0
        );
    }
    for entry in std::fs::read_dir(&source).expect("source") {
        assert_eq!(
            entry
                .expect("entry")
                .metadata()
                .expect("file")
                .permissions()
                .mode()
                & 0o077,
            0
        );
    }
    let path = journal_path(&directory);
    std::fs::remove_file(&path).expect("obstruct rename target");
    std::fs::create_dir(&path).expect("directory refuses file rename");
    assert!(claim.begin_create().is_err());
    let pending = path.with_extension("pending");
    assert!(
        pending.is_file(),
        "failed rename leaves the private staged file"
    );
    assert_eq!(
        std::fs::metadata(pending)
            .expect("pending file")
            .permissions()
            .mode()
            & 0o077,
        0
    );
}

#[cfg(unix)]
#[test]
fn post_rename_sync_failure_reopens_as_a_possibly_committed_create() {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let root = TempDir::new().expect("root");
    if std::fs::metadata(root.path()).expect("root").uid() == 0 {
        // A superuser bypasses the directory-read denial used to induce this fault.
        eprintln!("directory-sync denial probe requires an ordinary Unix user");
        return;
    }
    let store = store(root.path());
    let mut claim = store.claim(&request(), &placement()).expect("claim");
    let directory = root.path().join("a".repeat(64));
    // Write and traverse remain permitted: creating the staged file and renaming it
    // succeed, while opening the directory for the final fsync is refused.
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o300))
        .expect("deny directory reads");
    let outcome = claim.begin_create();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .expect("restore directory reads");
    assert!(outcome.is_err());
    assert_eq!(claim.phase(), IntentPhase::Uncertain);
    assert!(claim.begin_create().is_err());
    drop(claim);
    let mut reopened = store
        .claim(&request(), &placement())
        .expect("reopen installed journal");
    assert_eq!(
        reopened.phase(),
        IntentPhase::Creating,
        "rename installed the transition before sync failed"
    );
    assert!(
        reopened.begin_create().is_err(),
        "uncertain commit never authorizes a second create"
    );
}

#[test]
fn claim_establishment_filesystem_failures_recover_without_extra_create_authority() {
    let root = TempDir::new().expect("root");
    let blocked_root = root.path().join("blocked");
    std::fs::write(&blocked_root, b"not a directory").expect("directory obstruction");
    let blocked_store = store(&blocked_root);
    assert!(blocked_store.claim(&request(), &placement()).is_err());
    std::fs::remove_file(blocked_root).expect("remove directory obstruction");
    let mut recovered = blocked_store
        .claim(&request(), &placement())
        .expect("recovered store");
    recovered
        .begin_create()
        .expect("first create authority after failed establishment");
    drop(recovered);

    for fault in ["lock", "read", "initial-commit"] {
        let directory = root.path().join(fault);
        let store = store(&directory);
        drop(
            store
                .claim(&request(), &placement())
                .expect("establish a ready intent"),
        );
        let journal = journal_path(&directory);
        let saved = std::fs::read(&journal).expect("original journal");
        let obstruction = match fault {
            "lock" => journal.with_extension("lock"),
            "read" => journal.clone(),
            _ => journal.with_extension("pending"),
        };
        if obstruction.exists() {
            std::fs::remove_file(&obstruction).expect("remove fault target");
        }
        std::fs::create_dir(&obstruction).expect("obstruct establishment");
        assert!(store.claim(&request(), &placement()).is_err(), "{fault}");
        std::fs::remove_dir(&obstruction).expect("remove obstruction");
        if fault == "read" {
            std::fs::write(&journal, saved).expect("restore original journal");
        }
        let mut claim = store
            .claim(&request(), &placement())
            .expect("recover establishment");
        claim.begin_create().expect("first create authorization");
        drop(claim);
        let mut fresh = store.claim(&request(), &placement()).expect("fresh view");
        assert!(
            fresh.begin_create().is_err(),
            "{fault}: no second authorization"
        );
    }
}

#[test]
fn recovery_accepts_tracker_repository_ordering_and_deduplication() {
    let root = TempDir::new().expect("root");
    let store = store(root.path());
    let mut request = request();
    let first = request.repositories[0].clone();
    let second =
        onetaskgraph_plugin_api::Repository::try_from("github.com/operator/other".to_owned())
            .expect("origin");
    request.repositories = vec![first.clone(), second.clone(), first.clone()];
    let mut claim = store.claim(&request, &placement()).expect("claim");
    claim.begin_create().expect("create");
    let mut task = candidate(&claim);
    task.repositories = vec![second, first];
    claim
        .reconcile(&request, &[task])
        .expect("same repository set");
    assert_eq!(claim.binding(), Some(&binding()));
}
