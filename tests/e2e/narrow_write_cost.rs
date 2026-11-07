//! What a narrow write costs a public GitHub board under a policy that can rewrite: exactly what
//! the same write costs under one that only checks, with its own text stored replaced.
//!
//! The store is the `onetaskgraph-core` this crate links, built from a real configuration, and
//! its policy is the `onevcs` binary answering from a world whose private repository declares a
//! term and its replacement. GitHub is `tests/e2e/github_world.py`, whose request log is what is
//! compared: every document and every variable, so a read of the item, its comments or its
//! dependencies that only a rewriting policy makes fails the comparison by naming it.
//!
//! A linking caller hands the store a policy of its own rather than a configured command, as
//! `taskgraph` and the write-back do, so [`Policy::Linked`] drives that mode too, and
//! [`a_runs_settlement_under_a_rewriting_policy_reads_what_it_reads_under_none`] holds the
//! write-back's own settlement through the compiled binary. None of them opts into
//! `with_sanitize_stored_on_write`: a term already stored on an item is left for
//! `sources sanitize`, by the user's ruling that screening adds no GitHub request to a small board
//! edit. Opting in fails these journeys, on the read of the item and its comments it adds.

// llmlint: ignore-file[e2e_not_mocked] GitHub is the loopback beside this file — real HTTP on a
// real socket, reached by the real `github-projects` plugin the engine links — because an offline
// check may not reach the hosted API. `harness.rs` carries the same suppression and the full
// rationale.

use std::sync::Arc;

use onetaskgraph_core::boundary::CommandWritePolicy;
use onetaskgraph_core::config::{self, Layer};
use onetaskgraph_core::{Engine, Environment, GlobalId};
use onetaskgraph_plugin_api::{MetadataKey, TaskUpdate};
use serde_json::{json, Value};

use crate::harness::World;
use crate::linear_writeback::from_the_launch_directory;
use crate::private_github_home::{described, metadata_of, GitHub};
use crate::publication_boundary::{declare_replacement, mixed_world};

/// The invented private term the private repository declares, and what replaces it.
const TERM: &str = "velvetquasar";
const WITH: &str = "sample";
/// The public repository holding the item written to.
const PUBLIC: &str = "owner/library";
const BOARD: u64 = 2;
const TOKEN_ENV: &str = "ONEPIPELINE_LOOPBACK_BOARD_TOKEN";
/// The item every write here is made to, as the source names it.
const ITEM: &str = "board:note";
const NOTE: &str = "myapp.note";

/// What the store is told about its boundary.
#[derive(Clone, Copy, Debug)]
enum Policy {
    /// `visibility_command` and `check_command`, with no `sanitize_command`.
    Checks,
    /// The same with a `sanitize_command`.
    Sanitizes,
    /// A policy handed to the store by its linking caller, with no opt-in to the read-first guard.
    Linked,
}

/// One narrow write of each kind the engine makes.
#[derive(Clone, Copy, Debug)]
enum Write {
    TaskUpdate,
    MetadataSet,
}

/// The `onevcs` policy command `arguments` names, as the configuration spells a command.
fn policy_command(arguments: &[&str]) -> Value {
    let onevcs = crate::harness::onevcs_binary()
        .to_string_lossy()
        .into_owned();
    let mut command = vec![json!(onevcs)];
    command.extend(arguments.iter().map(|word| json!(word)));
    Value::Array(command)
}

/// Make `write` carrying `word` under `policy`, on a board and a host of its own: every request
/// the write sent, in a stable order, and the issue it wrote to as the board then holds it.
fn written(name: &str, policy: Policy, write: Write, word: &str) -> (Vec<Value>, Value) {
    let (world, private, _public) = mixed_world(name);
    declare_replacement(&world, &private);
    let github = GitHub::serve(
        &world,
        &json!({
            "owner": "owner",
            "repositories": {"owner/service": "private", PUBLIC: "public"},
            "boards": {BOARD.to_string(): {"public": true}},
            "issues": [{"id": "note", "repository": PUBLIC, "title": "Plain", "board": BOARD,
                        "status": "Todo", "parent": null, "body": "Nothing private here."}],
        }),
    );
    let engine = engine_over(&world, &github, policy);
    let item: GlobalId = ITEM.parse().expect("the item's id");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let said = format!("Retry the way {word} does.");
    // The policy's commands inherit this process's environment, so the host they answer from is
    // this world's for exactly as long as the write runs.
    let pointed = world.pointed_at_this_world();
    let mark = github.state()["requests"]
        .as_array()
        .expect("the request log")
        .len();
    match write {
        Write::TaskUpdate => {
            let update = TaskUpdate {
                title: Some(format!("Port {word}")),
                content: Some(said),
                ..TaskUpdate::default()
            };
            runtime
                .block_on(engine.update_task(&item, &update))
                .unwrap_or_else(|error| panic!("{policy:?} {write:?}: {error}"));
        }
        Write::MetadataSet => {
            let key = MetadataKey::new(NOTE.to_owned()).expect("the key");
            runtime
                .block_on(engine.set_task_metadata(&item, &key, &json!(said)))
                .unwrap_or_else(|error| panic!("{policy:?} {write:?}: {error}"));
        }
    }
    drop(pointed);
    let state = github.state();
    assert_eq!(state["unsupported"], json!([]), "{policy:?} {write:?}");
    let mut sent = state["requests"].as_array().expect("the request log")[mark..].to_vec();
    sent.sort_by_key(Value::to_string);
    (sent, state["issues"]["note"].clone())
}

/// The store a configuration names over `github`'s public board, under `policy`.
fn engine_over(world: &World, github: &GitHub, policy: Policy) -> Engine {
    let directory = world.root.join("store-configuration");
    std::fs::create_dir_all(&directory).expect("the configuration's directory");
    let mut write_policy = json!({
        "visibility_command": policy_command(&["boundary", "inspect", "--input", "-", "--json"]),
        "check_command": policy_command(
            &["boundary", "check", "--destination", "public", "--input", "-"],
        ),
    });
    if matches!(policy, Policy::Sanitizes | Policy::Linked) {
        write_policy["sanitize_command"] =
            policy_command(&["boundary", "sanitize", "--input", "-"]);
    }
    let document = json!({
        "sources": {"board": {
            "plugin": "github-projects",
            "visibility": "public",
            "config": {
                "owner": "owner", "project_number": BOARD, "repository": PUBLIC,
                "token_env": TOKEN_ENV, "endpoint": github.board(BOARD),
                "status_mapping": {
                    "todo": "Todo", "queued": "Queued", "in-progress": "In Progress",
                    "unknown": "Needs Attention", "done": "Done", "cancelled": "Canceled",
                },
            },
        }},
        "write_policy": write_policy,
    });
    std::fs::write(directory.join("onetaskgraph.yaml"), document.to_string())
        .expect("the store configuration");
    let home = world.root.to_string_lossy().into_owned();
    let environment = Environment::from_pairs([
        ("HOME", home.clone()),
        ("XDG_CONFIG_HOME", format!("{home}/xdg")),
        (TOKEN_ENV, "loopback-token".to_owned()),
    ]);
    let loaded = config::load(&directory, &environment, &Layer::default())
        .unwrap_or_else(|error| panic!("the store's configuration: {error}"));
    let engine = Engine::build(&loaded.config, &loaded.secrets);
    match policy {
        Policy::Checks | Policy::Sanitizes => engine,
        Policy::Linked => {
            let answered =
                serde_json::from_value(document["write_policy"].clone()).expect("the policy");
            engine.with_write_policy(Arc::new(CommandWritePolicy::new(&answered)))
        }
    }
}

/// A narrow write of each kind, under each policy that can rewrite, sends the board exactly what
/// it sends under one that only checks. The text the policy rewrote goes out as the
/// text the check-only store was handed directly, so the two logs compare whole, and the write
/// is stored replaced.
#[test]
fn a_narrow_write_under_a_rewriting_policy_reads_what_it_reads_under_one_that_only_checks() {
    for (write, label) in [
        (Write::TaskUpdate, "update"),
        (Write::MetadataSet, "metadata"),
    ] {
        let (expected, _) = written(
            &format!("narrow-{label}-checks"),
            Policy::Checks,
            write,
            WITH,
        );
        assert!(!expected.is_empty(), "{write:?} sent the board nothing");
        for (policy, kind) in [(Policy::Sanitizes, "sanitizes"), (Policy::Linked, "linked")] {
            let (sent, issue) = written(&format!("narrow-{label}-{kind}"), policy, write, TERM);
            assert_eq!(sent, expected, "{policy:?} {write:?}");
            let replaced = format!("Retry the way {WITH} does.");
            let body = issue["body"].as_str().expect("the issue's body");
            match write {
                Write::TaskUpdate => {
                    assert_eq!(issue["title"], format!("Port {WITH}"), "{issue:#}");
                    assert!(body.starts_with(&replaced), "{issue:#}");
                }
                Write::MetadataSet => {
                    assert_eq!(
                        metadata_of(body).get(NOTE),
                        Some(&json!(replaced)),
                        "{issue:#}"
                    );
                }
            }
            assert!(!issue.to_string().contains(TERM), "{issue:#}");
        }
    }
}

/// Every read a run of one lifecycle node sends a public board from its start through its
/// settlement, in a stable order, and that node's issue once the run is over. The item holds
/// [`TERM`] from the start, as a person's edit would leave it. `screened` configures the rewriting
/// policy the write-back then answers through its own; otherwise the store has no boundary at all.
fn reads_of_a_run(name: &str, screened: bool) -> (Vec<Value>, Value) {
    let (world, private, _public) = mixed_world(name);
    declare_replacement(&world, &private);
    let mut world = world.with_env(TOKEN_ENV, "loopback-token");
    if screened {
        let onevcs = crate::harness::onevcs_binary()
            .to_string_lossy()
            .into_owned();
        for (variable, arguments) in [
            ("CHECK", "boundary,check,--destination,public,--input,-"),
            ("VISIBILITY", "boundary,inspect,--input,-,--json"),
            ("SANITIZE", "boundary,sanitize,--input,-"),
        ] {
            world = world.with_env(
                &format!("ONETASKGRAPH_WRITE_POLICY__{variable}_COMMAND"),
                &format!("{onevcs},{arguments}"),
            );
        }
    }
    let github = GitHub::serve(
        &world,
        &json!({
            "owner": "owner",
            "repositories": {"owner/service": "private", PUBLIC: "public"},
            "boards": {BOARD.to_string(): {"public": true}},
            "issues": [
                {"id": "plan", "repository": PUBLIC, "title": name, "board": BOARD,
                 "status": "Todo", "parent": null,
                 "body": described("## Plan\n- Settle one node.", &json!({
                     "onepipeline.schema_version": onepipeline::plan::PLAN_SCHEMA_VERSION,
                     "onepipeline.concurrency": 1,
                     "onepipeline.goal": {"text": "Settle one node"},
                 }))},
                {"id": "work", "repository": PUBLIC, "title": "chore: work", "board": BOARD,
                 "status": "Todo", "parent": "plan",
                 "body": described(
                     &format!(
                         "## What\nRetry the way {TERM} does.\n\n## Why\nSo the run settles.\n\n\
                          ## Acceptance criteria\n- work is done."
                     ),
                     &json!({"onepipeline.id": "work", "onepipeline.persona": "engineer"}))},
            ],
        }),
    );
    let launch = world.root.join("launch-directory");
    std::fs::create_dir_all(&launch).expect("the launch directory");
    std::fs::write(
        launch.join("onetaskgraph.yaml"),
        format!(
            "sources:\n  board:\n    plugin: github-projects\n    visibility: public\n    \
             config:\n      owner: owner\n      project_number: {BOARD}\n      \
             repository: {PUBLIC}\n      token_env: {TOKEN_ENV}\n      endpoint: {}\n      \
             status_mapping:\n        todo: Todo\n        queued: Queued\n        \
             in-progress: In Progress\n        unknown: Needs Attention\n        done: Done\n        \
             cancelled: Canceled\n",
            github.board(BOARD),
        ),
    )
    .expect("the store configuration");
    world.script("work.work", "a neutral example\n");
    let start = from_the_launch_directory(&world, &launch, &["start", "board:plan", "--detach"]);
    world.run_on(start, "start the run").exited(0);
    world.until("the run to finish", |world| {
        !world.events_of(name, "run-stopped").is_empty()
    });
    world.until("the settlement to land", |_| {
        let body = github.state()["issues"]["work"]["body"].clone();
        metadata_of(body.as_str().unwrap_or_default()).contains_key("onepipeline.settlement")
    });
    let state = github.state();
    assert_eq!(state["unsupported"], json!([]), "{name}");
    let mut reads: Vec<Value> = state["requests"]
        .as_array()
        .expect("the request log")
        .iter()
        .filter(|request| {
            !request["query"]
                .as_str()
                .is_some_and(|q| q.starts_with("mutation"))
        })
        .cloned()
        .collect();
    reads.sort_by_key(Value::to_string);
    (reads, state["issues"]["work"].clone())
}

/// The write-back's own settlement, driven through the compiled binary: under a rewriting policy
/// a run reads the board exactly what it reads with no policy at all, so its settlement's narrow
/// write reads nothing for screening, and a term already stored stays for `sources sanitize`.
#[test]
fn a_runs_settlement_under_a_rewriting_policy_reads_what_it_reads_under_none() {
    let (expected, plain) = reads_of_a_run("narrow-settlement-plain", false);
    assert!(!expected.is_empty(), "the run read nothing of the board");
    let (reads, issue) = reads_of_a_run("narrow-settlement-screened", true);
    assert_eq!(reads, expected);
    for issue in [&plain, &issue] {
        let body = issue["body"].as_str().expect("the issue's body");
        assert!(
            body.contains(&format!("Retry the way {TERM} does.")),
            "{issue:#}"
        );
    }
}
