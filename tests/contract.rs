//! The committed contract drives the public types.
//!
//! Every fixture here is read out of `docs/contract.md` at compile time rather
//! than copied beside it, so the document and this crate's wire shapes cannot
//! drift: edit one without the other and this suite fails.
//!
//! The contract's Rust block is an interface sketch, not compilable source, so
//! it is used the way the document uses it — as the naming authority. Each test
//! below builds the real value with the compiler proving the fields exist, then
//! asserts the document still names them.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::{CommandFactory, Parser};
use oneagentgraph::config::{ConfigRef, GraphConfig, JudgeSide, Member};
use onepipeline::channel::{Command as Edit, Dependents, Reply, SurfaceKind};
use onepipeline::cli::{
    Cli, Command, DEFAULT_HEARTBEAT_INTERVAL_SECONDS, DEFAULT_ROUND_BUDGET_SECONDS,
};
use onepipeline::error::{
    EXIT_NOTHING_DRIVING, EXIT_NOT_IMPLEMENTED, EXIT_QUEUED, EXIT_REFUSED, EXIT_SUCCESS,
};
use onepipeline::event::{
    ArtifactId, ArtifactRef, Envelope, EventKind, Labels, Source, ENVELOPE_VERSION,
};
use onepipeline::executor::{
    CancelMode, CancellationToken, Capabilities, CapacityReport, DispatchRequest, Executor,
    LocalExecutor, WorkspaceSpec,
};
use onepipeline::plan::{Node, NodeKind, Plan, Resume, Step, PLAN_SCHEMA_VERSION};
use onepipeline::rules::{ExecutorKind, ExecutorRules, Predicate};
use onevcs::registry::{RepoType, Workflow};
use onevcs::{MergePolicy, SessionRequest};
use serde_json::{json, Value};

/// The approved contract itself.
const CONTRACT: &str = include_str!("../docs/contract.md");

/// The repository root, so a test can open the shipped content the contract
/// names. `CARGO_MANIFEST_DIR` is the crate root, which here is the repo root.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The fenced blocks in the contract carrying the given info string.
///
/// The scanner tracks whether it is *inside* a block rather than matching the
/// opening fence, so a closing fence never reads as an unlabelled opening one.
fn fenced_blocks(language: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut open: Option<String> = None;
    let mut body = String::new();
    for line in CONTRACT.lines() {
        match &open {
            Some(info) => {
                if line.trim_end() == "```" {
                    if info == language {
                        blocks.push(std::mem::take(&mut body));
                    }
                    body.clear();
                    open = None;
                } else {
                    body.push_str(line);
                    body.push('\n');
                }
            }
            None => {
                if let Some(info) = line.trim_end().strip_prefix("```") {
                    open = Some(info.trim().to_string());
                    body.clear();
                }
            }
        }
    }
    assert!(open.is_none(), "unterminated ``` block in docs/contract.md");
    blocks
}

/// The one fenced block in the contract carrying the given info string.
fn fenced_block(language: &str) -> String {
    let blocks = fenced_blocks(language);
    assert_eq!(
        blocks.len(),
        1,
        "expected exactly one ```{language} block in docs/contract.md, found {}",
        blocks.len()
    );
    blocks.into_iter().next().expect("one block")
}

/// Every `` `backticked` `` token in the contract.
fn backticked() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = CONTRACT;
    while let Some(open) = rest.find('`') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('`') else { break };
        out.insert(rest[..close].to_string());
        rest = &rest[close + 1..];
    }
    out
}

/// Assert the contract names each of these, so a document edit that drops one
/// fails here rather than leaving the surface unproven.
fn assert_contract_names(what: &str, names: &[&str]) {
    for name in names {
        assert!(
            CONTRACT.contains(name),
            "docs/contract.md no longer names the {what} `{name}`"
        );
    }
}

#[test]
fn the_contracts_rules_example_parses_and_round_trips() {
    let yaml = fenced_block("yaml");
    let rules: ExecutorRules = serde_norway::from_str(&yaml).expect("the rules example parses");

    assert_eq!(
        rules.executors.len(),
        1,
        "the example declares one executor"
    );
    let local = &rules.executors[0];
    assert_eq!(local.name, "local");
    assert_eq!(local.kind, ExecutorKind::Local);
    assert_eq!(local.max_load1, Some(8.0));
    assert_eq!(
        local.min_free_mem.as_deref(),
        Some("2GiB"),
        "the size is carried as the contract writes it"
    );

    assert_eq!(
        rules.rules.len(),
        2,
        "the example declares two ordered rules"
    );
    assert_eq!(
        rules.rules[0].when,
        Some(Predicate {
            executor_has_capacity: "local".into()
        }),
        "the first rule tests capacity"
    );
    assert_eq!(rules.rules[0].use_executor, "local");
    assert_eq!(
        rules.rules[1].when, None,
        "the last rule is the unconditional fallback"
    );
    assert_eq!(rules.rules[1].use_executor, "local");

    let round_tripped: ExecutorRules =
        serde_norway::from_str(&serde_norway::to_string(&rules).expect("serializes"))
            .expect("re-parses");
    assert_eq!(round_tripped, rules);
}

#[test]
fn an_unknown_rules_key_is_refused_at_the_boundary() {
    let bad = "executors:\n  - {name: local, type: local, mx_load1: 8.0}\nrules:\n  - use: local\n";
    let err = serde_norway::from_str::<ExecutorRules>(bad)
        .expect_err("a mistyped key is rejected, not silently dropped");
    assert!(
        err.to_string().contains("mx_load1"),
        "the error names the offending key: {err}"
    );
}

#[test]
fn the_shipped_rules_example_is_the_contracts_own() {
    let shipped = std::fs::read_to_string(repo_root().join("examples/executors.yaml"))
        .expect("examples/executors.yaml ships");
    let shipped: ExecutorRules = serde_norway::from_str(&shipped).expect("it parses");
    let documented: ExecutorRules =
        serde_norway::from_str(&fenced_block("yaml")).expect("the contract's example parses");
    assert_eq!(
        shipped, documented,
        "the shipped executor-rules example must be the contract's own"
    );
}

#[test]
fn the_dispatch_request_carries_every_field_the_contract_declares() {
    let request = DispatchRequest {
        graph: ConfigRef("./graphs/node-scope.yaml".into()),
        task: "## What\nDo the thing.".into(),
        labels: Labels {
            run_id: Some("run-1".into()),
            round: Some(2),
            node: Some("service".into()),
            step: Some("implement".into()),
            persona: Some("engineer".into()),
            ..Labels::default()
        },
        workspace: WorkspaceSpec::VcsSession(SessionRequest {
            repo: "nickderobertis/some-service".into(),
            branch: None,
            base: None,
            execution_checkout: None,
        }),
        cancel: CancellationToken,
    };

    assert_contract_names(
        "DispatchRequest field",
        &["graph", "task", "labels", "workspace", "cancel"],
    );
    assert_contract_names(
        "reserved label",
        &["run_id", "round", "node", "step", "persona"],
    );
    assert_contract_names(
        "WorkspaceSpec variant",
        &["Path(PathBuf)", "VcsSession(SessionSpec"],
    );

    // The contract's `WorkspaceSpec::VcsSession` means the machine running the
    // dispatch opens the session, so the request carries the *ask*, never an
    // already-opened session.
    match &request.workspace {
        WorkspaceSpec::VcsSession(session) => {
            assert_eq!(session.repo, "nickderobertis/some-service");
        }
        WorkspaceSpec::Path(path) => panic!("built a VcsSession, got a path: {}", path.display()),
    }

    let local = WorkspaceSpec::Path(Path::new("/tmp/work").to_path_buf());
    assert_ne!(local, request.workspace);
}

#[test]
fn the_local_executor_is_the_one_v1_ships_and_takes_both_workspaces() {
    let local = LocalExecutor;
    assert_eq!(local.name(), "local");
    assert_eq!(
        local.capabilities(),
        Capabilities { vcs_sessions: true },
        "the contract says LocalExecutor supports both WorkspaceSpec variants"
    );
    assert!(CONTRACT.contains("v1 ships `LocalExecutor` only (supports both variants)"));
}

#[test]
fn the_local_executors_capacity_claims_nothing_it_has_not_measured() {
    // Interface-only: nothing probes the host yet, so the report must not let a
    // rules file select this executor on numbers nobody measured.
    let report = LocalExecutor.capacity();
    assert_eq!(report, CapacityReport::default());
    assert_eq!(report.slots_free, 0);
    assert_eq!(report.load1, 0.0);
    assert_eq!(report.mem_free_bytes, 0);
    assert_contract_names(
        "CapacityReport field",
        &["slots_free", "load1", "mem_free_bytes"],
    );
}

#[test]
fn dispatching_refuses_rather_than_pretending_to_run() {
    // `Box<dyn DispatchHandle>` is not `Debug`, so the success arm is destructured
    // rather than unwrapped.
    let Err(err) = LocalExecutor.dispatch(DispatchRequest {
        graph: ConfigRef("./graphs/node-scope.yaml".into()),
        task: "anything".into(),
        labels: Labels::default(),
        workspace: WorkspaceSpec::Path(PathBuf::from(".")),
        cancel: CancellationToken,
    }) else {
        panic!("nothing is implemented behind the seam, so dispatch cannot succeed");
    };
    assert!(
        err.to_string().contains("NOT IMPLEMENTED"),
        "the refusal is loud: {err}"
    );
}

#[test]
fn a_dispatch_is_cancelled_the_two_ways_the_contract_names() {
    assert_ne!(CancelMode::Cooperative, CancelMode::Kill);
    assert_contract_names("CancelMode variant", &["Cooperative | Kill"]);
}

#[test]
fn the_contract_declares_the_seams_traits_and_methods() {
    let sketch = fenced_block("rust");
    for item in [
        "pub trait Executor",
        "fn name(",
        "fn capabilities(",
        "fn capacity(",
        "fn dispatch(",
        "pub struct DispatchRequest",
        "pub trait DispatchHandle",
        "fn events(",
        "fn wait(",
        "fn cancel(",
    ] {
        assert!(
            sketch.contains(item),
            "the contract's Rust block no longer declares `{item}`"
        );
    }
}

/// A plan exercising every node shape the contract names.
fn every_node_shape() -> Value {
    json!({
        "schema_version": PLAN_SCHEMA_VERSION,
        "name": "every-shape",
        "concurrency": 3,
        "goal": {"text": "prove the schema"},
        "tasks": [
            {
                "id": "direct",
                "persona": "engineer",
                "task": "## What\nx\n\n## Why\ny\n\n## Acceptance criteria\n- z",
                "done_when": "all task acceptance criteria are met",
                "max_turns": 24,
                "expects_no_diff": true,
                "context": "the earlier round already landed the schema",
                "executor": "local",
                "agent_graph": "./graphs/node-scope.yaml",
                "deps": ["run:other-run#upstream"]
            },
            {
                "id": "approval",
                "kind": "human",
                "task": "Approve the design.",
                "deps": ["direct"]
            },
            {
                "id": "lifecycle",
                "repo": "nickderobertis/some-service",
                "repo_type": "team",
                "workflow": "remote",
                "merge_policy": "change-auto",
                "base_branch": "main",
                "branch": "feat/thing",
                "title": "feat: thing",
                "execution_checkout": "isolated",
                "verify_via_ci": true,
                "parked": true,
                "resume": {"branch": "feat/thing", "checkpoint": "abc1234"},
                "deps": ["approval"],
                "steps": [
                    {
                        "id": "implement",
                        "persona": "engineer",
                        "task": "## What\nx",
                        "done_when": "the gate is green",
                        "max_turns": 32,
                        "expects_no_diff": false,
                        "executor": "local",
                        "agent_graph": "./graphs/node-scope.yaml"
                    },
                    {
                        "id": "sign-off",
                        "kind": "human",
                        "task": "Exercise staging and approve.",
                        "deps": ["implement"]
                    }
                ]
            }
        ]
    })
}

#[test]
fn the_plan_schema_carries_every_node_shape_the_contract_names() {
    let plan: Plan = serde_json::from_value(every_node_shape()).expect("the plan parses");

    assert_eq!(plan.schema_version, PLAN_SCHEMA_VERSION);
    assert_eq!(plan.concurrency, 3);
    assert_eq!(plan.goal.as_ref().expect("a goal").text, "prove the schema");
    assert_eq!(plan.tasks.len(), 3);

    let direct = &plan.tasks[0];
    assert_eq!(direct.kind, NodeKind::Agent, "`agent` is the default kind");
    assert!(direct.expects_no_diff);
    assert_eq!(direct.executor.as_deref(), Some("local"));
    assert_eq!(
        direct.agent_graph,
        Some(ConfigRef("./graphs/node-scope.yaml".into())),
        "`agent_graph` is an oneagentgraph config reference"
    );
    assert_eq!(
        direct.context.as_deref(),
        Some("the earlier round already landed the schema")
    );
    assert_eq!(
        direct.deps,
        vec!["run:other-run#upstream"],
        "a cross-DAG reference is a dependency like any other"
    );

    assert_eq!(plan.tasks[1].kind, NodeKind::Human);

    let lifecycle = &plan.tasks[2];
    assert_eq!(
        lifecycle.repo.as_deref(),
        Some("nickderobertis/some-service")
    );
    assert_eq!(lifecycle.repo_type, Some(RepoType::Team));
    assert_eq!(lifecycle.workflow, Some(Workflow::Remote));
    assert_eq!(lifecycle.merge_policy, Some(MergePolicy::ChangeAuto));
    assert!(lifecycle.parked);
    assert_eq!(
        lifecycle.resume,
        Some(Resume {
            branch: "feat/thing".into(),
            checkpoint: Some("abc1234".into())
        })
    );
    let steps = lifecycle
        .steps
        .as_ref()
        .expect("nested steps on one branch");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].kind, NodeKind::Agent);
    assert_eq!(steps[1].kind, NodeKind::Human);

    assert_contract_names(
        "node shape",
        &[
            "`agent` direct",
            "lifecycle with `repo`",
            "`kind: human`",
            "nested `steps` on one branch",
            "`expects_no_diff`",
            "`context`",
            "cross-DAG `run:<id>#<node>` refs",
            "judge-only `done_when`",
            "`executor: NAME`",
            "`agent_graph: REF`",
        ],
    );
}

#[test]
fn a_plan_round_trips_without_losing_a_field() {
    let plan: Plan = serde_json::from_value(every_node_shape()).expect("parses");
    let again: Plan = serde_json::from_value(serde_json::to_value(&plan).expect("serializes"))
        .expect("re-parses");
    assert_eq!(again, plan);
}

#[test]
fn a_mistyped_node_key_is_refused_at_the_boundary() {
    let err = serde_json::from_value::<Plan>(json!({
        "schema_version": PLAN_SCHEMA_VERSION,
        "tasks": [{"id": "x", "persna": "engineer"}]
    }))
    .expect_err("a mistyped key is rejected, not silently dropped");
    assert!(
        err.to_string().contains("persna"),
        "the error names it: {err}"
    );
}

#[test]
fn a_node_and_a_step_default_to_the_shapes_the_contract_states() {
    let node = Node {
        id: "x".into(),
        ..Node::default()
    };
    assert_eq!(node.kind, NodeKind::Agent);
    assert!(!node.expects_no_diff);
    assert!(!node.parked);
    assert!(node.deps.is_empty());

    let step = Step {
        id: "s".into(),
        ..Step::default()
    };
    assert_eq!(step.kind, NodeKind::Agent);
    assert!(!step.expects_no_diff);

    // An unset optional is omitted, so an old consumer never sees a null it did
    // not have before.
    let rendered = serde_json::to_value(&node).expect("serializes");
    assert_eq!(
        rendered,
        json!({"id": "x"}),
        "the default kind is omitted, so an old consumer sees no field it did not have"
    );
}

#[test]
fn the_shipped_example_plans_parse() {
    for name in ["single-node.plan.json", "mixed-graph.plan.json"] {
        let path = repo_root().join("examples").join(name);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name} ships: {e}"));
        let plan: Plan =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} parses: {e}"));
        assert_eq!(plan.schema_version, PLAN_SCHEMA_VERSION);
        assert!(!plan.tasks.is_empty(), "{name} has nodes");
    }
}

/// The `op` a [`Edit`] serializes as.
///
/// The match has no wildcard, so a tenth variant stops this suite compiling
/// until the contract, [`OPS`], and the round-trip below all name it. That is
/// the half of "exactly the ops this crate accepts" a hand-written list cannot
/// prove.
fn op_of(command: &Edit) -> &'static str {
    match command {
        Edit::Add { .. } => "add",
        Edit::Drop { .. } => "drop",
        Edit::Reparent { .. } => "reparent",
        Edit::Retry { .. } => "retry",
        Edit::Cancel { .. } => "cancel",
        Edit::Requeue { .. } => "requeue",
        Edit::Attest { .. } => "attest",
        Edit::Complete { .. } => "complete",
        Edit::Context { .. } => "context",
    }
}

/// The ops the contract lists, in the order it lists them.
const OPS: &[&str] = &[
    "add", "drop", "reparent", "retry", "cancel", "requeue", "attest", "complete", "context",
];

#[test]
fn the_contract_lists_exactly_the_ops_this_crate_accepts() {
    let listed = "`add | drop | reparent | retry | cancel | requeue | attest | complete | context`";
    assert!(
        CONTRACT.contains(listed),
        "the contract's op list moved; update OPS with it"
    );
    assert_eq!(OPS.len(), 9);

    assert_eq!(
        op_of(&Edit::Cancel { id: "x".into() }),
        "cancel",
        "the exhaustive match above is what proves the variant set, and it runs"
    );
}

#[test]
fn every_op_deserializes_with_the_fields_the_protocol_requires() {
    let envelopes: Vec<(&str, Value)> = vec![
        ("add", json!({"op": "add", "node": {"id": "new"}})),
        (
            "drop",
            json!({"op": "drop", "id": "slow", "dependents": "detach"}),
        ),
        (
            "reparent",
            json!({"op": "reparent", "id": "pending", "deps": ["slow"]}),
        ),
        (
            "retry",
            json!({"op": "retry", "id": "failed", "node": {"id": "retry"}}),
        ),
        ("cancel", json!({"op": "cancel", "id": "sweep"})),
        (
            "requeue",
            json!({"op": "requeue", "id": "sweep", "amend": {"max_turns": 32}}),
        ),
        ("attest", json!({"op": "attest", "ref": "approve"})),
        (
            "complete",
            json!({"op": "complete", "reason": "closeout verified"}),
        ),
        (
            "context",
            json!({"op": "context", "id": "slow", "note": "the fix landed"}),
        ),
    ];
    let seen: Vec<&str> = envelopes.iter().map(|(op, _)| *op).collect();
    assert_eq!(
        seen, OPS,
        "every op the contract lists is exercised, in order"
    );

    for (op, value) in &envelopes {
        let edit: Edit = serde_json::from_value(value.clone())
            .unwrap_or_else(|e| panic!("`{op}` deserializes: {e}"));
        assert_eq!(
            &op_of(&edit),
            op,
            "`{op}` deserialized into another variant"
        );
        let again = serde_json::to_value(&edit).expect("serializes");
        assert_eq!(&again, value, "`{op}` round-trips unchanged");
    }
}

#[test]
fn drop_must_state_the_dependents_fate() {
    let err = serde_json::from_value::<Edit>(json!({"op": "drop", "id": "slow"}))
        .expect_err("`dependents` is required");
    assert!(
        err.to_string().contains("dependents"),
        "the error names it: {err}"
    );

    assert_ne!(Dependents::Drop, Dependents::Detach);
    assert!(CONTRACT.contains("drop"));
}

#[test]
fn an_unknown_op_is_refused_rather_than_ignored() {
    let err = serde_json::from_value::<Edit>(json!({"op": "rewrite", "id": "x"}))
        .expect_err("an op outside the protocol is refused");
    assert!(
        err.to_string().contains("rewrite"),
        "the error names it: {err}"
    );
}

#[test]
fn a_command_only_envelope_and_a_verdict_envelope_are_both_replies() {
    let commands_only: Reply = serde_json::from_value(json!({
        "version": 1,
        "commands": [{"op": "attest", "ref": "approve"}]
    }))
    .expect("a command-only envelope parses");
    assert_eq!(commands_only.version, Some(1));
    assert_eq!(commands_only.commands.len(), 1);
    assert_eq!(commands_only.completion, None);

    let both: Reply = serde_json::from_value(json!({
        "completion": false,
        "message": "apply the replacement and continue",
        "reason": "the failed node is retryable",
        "version": 1,
        "commands": [{"op": "retry", "id": "failed", "node": {"id": "retry", "expects_no_diff": true}}]
    }))
    .expect("commands may accompany a legacy verdict");
    assert_eq!(both.completion, Some(false));
    assert_eq!(both.commands.len(), 1);

    let legacy: Reply = serde_json::from_value(json!({"completion": true, "reason": "done"}))
        .expect("a legacy verdict alone parses");
    assert!(legacy.commands.is_empty());

    assert!(CONTRACT.contains(r#"{"version": 1, "commands": [...]}"#));
}

#[test]
fn the_only_surface_kind_the_contract_names_is_check_in() {
    let kind: SurfaceKind = serde_json::from_value(json!("check-in")).expect("parses");
    assert_eq!(kind, SurfaceKind::CheckIn);
    assert!(CONTRACT.contains("--kind check-in"));
    assert!(
        CONTRACT.contains("oneagentgraph reset-timer RUN check-in"),
        "consuming a surface resets the pacemaker"
    );
}

#[test]
fn the_reply_exit_codes_are_the_ones_the_contract_assigns() {
    assert!(CONTRACT.contains(
        "reply exit 0 = applied, 1 = accepted-not-yet-reconciled, 2 = refused/malformed"
    ));
    assert_eq!(EXIT_SUCCESS, 0);
    assert_eq!(EXIT_QUEUED, 1);
    assert_eq!(EXIT_REFUSED, 2);

    assert!(CONTRACT.contains("exit 3 = nothing is driving the run"));
    assert_eq!(EXIT_NOTHING_DRIVING, 3);

    // The interface-only refusal must not collide with any of them.
    for spent in [
        EXIT_SUCCESS,
        EXIT_QUEUED,
        EXIT_REFUSED,
        EXIT_NOTHING_DRIVING,
    ] {
        assert_ne!(EXIT_NOT_IMPLEMENTED, spent);
    }
}

#[test]
fn an_envelope_round_trips_through_the_merged_streams_shape() {
    let wire = json!({
        "v": ENVELOPE_VERSION,
        "ts": "2026-08-07T12:00:00.000Z",
        "stream": "onepipeline-7f3a",
        "seq": 42,
        "source": "pipeline",
        "kind": "node-settled",
        "labels": {"run_id": "run-1", "round": 2, "node": "service", "attempt": 1},
        "payload": {"status": "done"},
        "artifacts": [{"id": "gate-log", "kind": "log", "bytes": 8192}]
    });

    let envelope: Envelope = serde_json::from_value(wire.clone()).expect("the envelope parses");
    assert_eq!(envelope.source, Source::Pipeline);
    assert_eq!(envelope.kind, EventKind("node-settled".into()));
    assert_eq!(envelope.labels.run_id.as_deref(), Some("run-1"));
    assert_eq!(envelope.labels.round, Some(2));
    assert_eq!(
        envelope.labels.extra.get("attempt"),
        Some(&json!(1)),
        "a label outside the reserved keys rides in `extra`"
    );
    assert_eq!(
        envelope.artifacts,
        vec![ArtifactRef {
            id: ArtifactId("gate-log".into()),
            kind: "log".into(),
            bytes: 8192
        }]
    );

    assert_eq!(serde_json::to_value(&envelope).expect("serializes"), wire);
}

#[test]
fn the_three_merged_streams_are_the_three_libraries_the_contract_composes() {
    assert!(CONTRACT.contains("merges the three event streams"));
    for (library, source) in [
        ("oneagentgraph", Source::Agentgraph),
        ("onevcs", Source::Vcs),
        ("onepipeline", Source::Pipeline),
    ] {
        assert!(
            CONTRACT.contains(library),
            "the contract no longer names `{library}` as a composed library"
        );
        let _ = source;
    }
    assert_eq!(
        serde_json::to_value(Source::Agentgraph).expect("serializes"),
        json!("agentgraph")
    );
    assert_eq!(
        serde_json::to_value(Source::Vcs).expect("serializes"),
        json!("vcs")
    );
    assert_eq!(
        serde_json::to_value(Source::Pipeline).expect("serializes"),
        json!("pipeline")
    );

    // A fourth source is not a stream this crate merges.
    serde_json::from_value::<Source>(json!("harness")).expect_err("an unknown source is refused");
}

#[test]
fn a_relayed_envelope_keeps_its_producers_own_kind() {
    // onepipeline merges rather than rewrites, so an envelope a sibling produced
    // survives the trip through this crate's shape unchanged.
    let wire = json!({
        "v": ENVELOPE_VERSION,
        "ts": "2026-08-07T12:00:01.500Z",
        "stream": "onevcs-1a2b",
        "seq": 3,
        "source": "vcs",
        "kind": "gate-finished",
        "labels": {},
        "payload": {},
        "artifacts": []
    });
    let envelope: Envelope = serde_json::from_value(wire.clone()).expect("parses");
    assert_eq!(envelope.source, Source::Vcs);
    assert_eq!(envelope.kind, EventKind("gate-finished".into()));
    assert_eq!(serde_json::to_value(&envelope).expect("serializes"), wire);
}

#[test]
fn the_driver_contracts_invocation_parses_exactly_as_written() {
    let documented = "onepipeline start plan.json [--attach|--detach] \
                      [--round-budget 14400] [--heartbeat-interval 1800]";
    assert!(CONTRACT.contains(documented), "the driver invocation moved");

    let cli = Cli::try_parse_from([
        "onepipeline",
        "start",
        "plan.json",
        "--detach",
        "--round-budget",
        "14400",
        "--heartbeat-interval",
        "1800",
    ])
    .expect("the documented invocation parses");
    let Command::Start(args) = cli.command else {
        panic!("expected `start`");
    };
    assert_eq!(args.plan, PathBuf::from("plan.json"));
    assert!(args.detach);
    assert!(!args.attach);
    assert_eq!(args.round_budget, 14_400);
    assert_eq!(args.heartbeat_interval, 1_800);

    // The document's numbers are this crate's defaults.
    assert_eq!(DEFAULT_ROUND_BUDGET_SECONDS, 14_400);
    assert_eq!(DEFAULT_HEARTBEAT_INTERVAL_SECONDS, 1_800);
    let defaulted = Cli::try_parse_from(["onepipeline", "start", "plan.json"]).expect("parses");
    let Command::Start(args) = defaulted.command else {
        panic!("expected `start`");
    };
    assert_eq!(args.round_budget, DEFAULT_ROUND_BUDGET_SECONDS);
    assert_eq!(args.heartbeat_interval, DEFAULT_HEARTBEAT_INTERVAL_SECONDS);
}

#[test]
fn attach_and_detach_are_the_alternatives_the_contract_writes_them_as() {
    Cli::try_parse_from(["onepipeline", "start", "p.json", "--attach"]).expect("attach parses");
    Cli::try_parse_from(["onepipeline", "start", "p.json", "--attach", "--detach"])
        .expect_err("`--attach|--detach` are alternatives, not a pair");
}

#[test]
fn every_command_the_contract_names_parses() {
    let invocations: &[(&str, &[&str])] = &[
        ("start", &["start", "plan.json"]),
        ("adopt", &["adopt", "run-1"]),
        ("round run", &["round", "run", "run-1"]),
        ("round next", &["round", "next", "run-1"]),
        ("channel serve", &["channel", "serve", "run-1"]),
        ("next", &["next", "run-1"]),
        ("reply", &["reply", "run-1"]),
        ("reply FILE", &["reply", "run-1", "edits.json"]),
        (
            "surface",
            &[
                "surface",
                "run-1",
                "--kind",
                "check-in",
                "--message",
                "all clear",
            ],
        ),
        ("attest", &["attest", "run-1", "approve"]),
        ("stop", &["stop", "run-1"]),
        ("stop --force", &["stop", "run-1", "--force"]),
        ("runs", &["runs"]),
        ("runs --mine", &["runs", "--mine"]),
        ("status", &["status"]),
        ("host", &["host"]),
        ("monitor", &["monitor", "run-1"]),
        ("results", &["results", "run-1"]),
        ("goals", &["goals"]),
        ("telemetry", &["telemetry"]),
        ("telemetry --breakdown", &["telemetry", "--breakdown"]),
    ];

    for (name, args) in invocations {
        let argv: Vec<&str> = std::iter::once("onepipeline")
            .chain(args.iter().copied())
            .collect();
        Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("`{name}` does not parse: {e}"));
    }
}

#[test]
fn the_contract_names_every_command_and_view_this_crate_offers() {
    assert_contract_names(
        "channel command",
        &[
            "`onepipeline next RUN`",
            "reply RUN [FILE]",
            "surface RUN --kind check-in --message TEXT",
            "attest RUN REF",
            "stop RUN",
        ],
    );
    assert_contract_names(
        "engine verb",
        &[
            "onepipeline round run|next",
            "onepipeline channel serve RUN",
            "onepipeline adopt RUN",
        ],
    );

    // The views, as the contract lists them.
    let tokens = backticked();
    for view in ["runs", "status", "host", "monitor", "results", "goals"] {
        assert!(
            tokens.contains(view),
            "the contract no longer lists the `{view}` view"
        );
    }
    assert!(tokens.contains("telemetry [--breakdown]"));
    assert!(tokens.contains("runs --mine"));
}

#[test]
fn a_command_outside_the_surface_is_refused() {
    Cli::try_parse_from(["onepipeline", "publish", "run-1"])
        .expect_err("the surface is exactly what the contract names");
}

#[test]
fn the_dag_scope_graph_is_an_orchestrator_plus_a_resettable_check_in() {
    assert!(CONTRACT
        .contains("shipped default: `orchestrator` member + resettable-cron `check-in` member"));

    let text = std::fs::read_to_string(repo_root().join("graphs/dag-scope.yaml"))
        .expect("the dag-scope graph ships");
    // It parses as oneagentgraph's own schema — the library that launches it.
    let graph: GraphConfig = serde_norway::from_str(&text).expect("it is a valid graph config");
    assert_eq!(graph.name, "dag-scope");

    let orchestrator = graph
        .members
        .get("orchestrator")
        .expect("an orchestrator member");
    let Member::Onejudge(orchestrator) = orchestrator else {
        panic!("the orchestrator is a two-party member");
    };
    match &orchestrator.judge {
        JudgeSide::Command(judge) => assert_eq!(
            judge.command[..3],
            ["onepipeline", "channel", "serve"],
            "the orchestrator's judge side is this crate's channel server"
        ),
        JudgeSide::Harness(_) => panic!("the contract makes the judge side a command provider"),
    }

    let check_in = graph.members.get("check-in").expect("a check-in member");
    let Member::Oneharness(check_in) = check_in else {
        panic!("the pacemaker is a single-sided member");
    };
    let schedule = check_in.schedule.expect("it is a cron member");
    assert!(
        schedule.resettable,
        "the contract makes the check-in resettable"
    );
    assert_eq!(
        schedule.every, DEFAULT_HEARTBEAT_INTERVAL_SECONDS,
        "its period is the driver's default heartbeat interval"
    );
}

#[test]
fn the_default_node_scope_graph_is_a_worker_and_a_judge() {
    assert!(CONTRACT.contains("a default node-scope config (worker+judge)"));
    let text = std::fs::read_to_string(repo_root().join("graphs/node-scope.yaml"))
        .expect("the node-scope graph ships");
    let graph: GraphConfig = serde_norway::from_str(&text).expect("it is a valid graph config");
    assert_eq!(graph.name, "node-scope");

    let worker = graph.members.get("worker").expect("a worker member");
    let Member::Onejudge(worker) = worker else {
        panic!("worker+judge is a two-party member");
    };
    assert!(
        matches!(worker.judge, JudgeSide::Harness(_)),
        "the node-scope judge is harness-backed"
    );
    assert_eq!(
        graph.members.len(),
        1,
        "worker+judge is one onejudge member"
    );
}

#[test]
fn every_persona_the_contract_ships_is_present_and_has_both_sides() {
    assert!(CONTRACT.contains("personas `orchestrator`, `check-in`, `pr-author`"));
    for name in ["orchestrator", "check-in", "pr-author"] {
        let path = repo_root().join("personas").join(format!("{name}.yaml"));
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name} persona ships: {e}"));
        let persona: Value =
            serde_norway::from_str(&text).unwrap_or_else(|e| panic!("{name} parses: {e}"));
        assert!(
            persona.pointer("/agent/instructions").is_some(),
            "{name} states the agent's role"
        );
        assert!(
            persona.pointer("/user/persona").is_some(),
            "{name} states the supervisor's review bar"
        );
    }
}

#[test]
fn the_pr_author_never_blocks_publication() {
    assert!(
        CONTRACT.contains("drafting failure falls back deterministic and never blocks publication")
    );
    let text = std::fs::read_to_string(repo_root().join("personas/pr-author.yaml"))
        .expect("the pr-author persona ships");
    // The persona is wrapped prose, so match on its words rather than its line
    // breaks.
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flattened.contains("not on the publication path"),
        "the persona itself says the dispatch is not on the publication path"
    );
}

#[test]
fn every_recorded_divergence_names_a_type_the_contract_actually_declares() {
    let divergences = std::fs::read_to_string(repo_root().join("docs/contract-divergences.md"))
        .expect("the divergence record ships");
    for named in [
        "ResolvedGraphRef",
        "SessionSpec",
        "DispatchOutcome",
        "min_free_mem",
        "executor_has_capacity",
    ] {
        assert!(
            CONTRACT.contains(named),
            "the contract no longer names `{named}`; retire its divergence entry"
        );
        assert!(
            divergences.contains(named),
            "`{named}` diverges from the contract but is not recorded"
        );
    }
}

#[test]
fn the_smoke_scripts_command_list_is_the_binarys_whole_surface() {
    // The published-artifact smoke checks `--help` against a hand-written word
    // list. Without this gate a command added to the contract could be missing
    // from every published binary and the smoke would still pass.
    let script = std::fs::read_to_string(repo_root().join("scripts/smoke-published.sh"))
        .expect("the smoke script ships");
    let listed = script
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("for command in ")?
                .strip_suffix("; do")
        })
        .expect("the smoke script iterates a `for command in ...; do` list")
        .split_whitespace()
        .map(str::to_string)
        .collect::<BTreeSet<String>>();

    let surface = Cli::command()
        .get_subcommands()
        .map(|sub| sub.get_name().to_string())
        .collect::<BTreeSet<String>>();

    assert_eq!(
        listed, surface,
        "scripts/smoke-published.sh checks a different command set than the CLI offers"
    );
}

#[test]
fn the_readmes_interface_claims_match_the_code_they_describe() {
    // The README restates numbers that live in the code. Each copy is gated
    // here, so a change to the code fails the suite instead of leaving the
    // README quietly wrong.
    let raw = std::fs::read_to_string(repo_root().join("README.md")).expect("the README ships");
    // Wrapped prose, so match on its words rather than its line breaks.
    let readme = raw.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(
        readme.contains(&format!("exit code `{EXIT_NOT_IMPLEMENTED}`")),
        "the README states a different interface-only exit code than the crate uses"
    );
    assert!(
        readme.contains(&format!(
            "exit `{EXIT_NOTHING_DRIVING}` means nothing is driving"
        )),
        "the README states a different code for an undriven run than the crate uses"
    );
    assert!(
        readme.contains(&format!(
            "exits `{EXIT_SUCCESS}` when the reconciler applied it, `{EXIT_QUEUED}` when it is queued"
        )) && readme.contains(&format!("and `{EXIT_REFUSED}` when")),
        "the README's reply exit-code mapping no longer matches the crate's constants"
    );

    // Every view the README lists is a command the binary actually offers.
    let surface = Cli::command()
        .get_subcommands()
        .map(|sub| sub.get_name().to_string())
        .collect::<BTreeSet<String>>();
    let views = readme
        .split_once("Read-only views")
        .expect("the README has a read-only views paragraph")
        .1
        .split_once("without touching a run")
        .expect("that paragraph ends where the README says it does")
        .0
        .to_string();
    for view in [
        "runs",
        "status",
        "host",
        "monitor",
        "results",
        "goals",
        "telemetry",
    ] {
        assert!(
            views.contains(&format!("`{view}`")),
            "the README's view list omits `{view}`"
        );
        assert!(
            surface.contains(view),
            "`{view}` is not a command the binary offers"
        );
    }
}
