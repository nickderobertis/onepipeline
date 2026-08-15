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
use onepipeline::controls::NodeControls;
use onepipeline::error::{EXIT_NOTHING_DRIVING, EXIT_QUEUED, EXIT_REFUSED, EXIT_SUCCESS};
use onepipeline::event::{
    ArtifactId, ArtifactRef, Envelope, EventKind, Labels, PipelineKind, Source, ENVELOPE_VERSION,
    PIPELINE_KINDS,
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
            executor_has_capacity: Some("local".into()),
            ..Predicate::default()
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
fn the_contract_states_both_predicate_families_and_what_each_matches_on() {
    let prose = CONTRACT.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        prose.contains("`executor_has_capacity: NAME` matches on **capacity**"),
        "the contract no longer says what the capacity family matches on"
    );
    assert!(
        prose.contains("`node_label: {KEY: VALUE, ...}` matches on the **node's labels**"),
        "the contract no longer says what the label family matches on"
    );
    assert!(
        prose.contains("Several conditions in one `when` conjoin"),
        "the contract no longer says how two conditions in one `when` combine"
    );

    // The keys the contract lists are the keys the grammar accepts. Both halves
    // of that sentence are gated: a key the code accepts and the contract does
    // not name is undocumented surface, and the other way round is a promise
    // nothing keeps.
    for key in onepipeline::rules::SELECTABLE_LABELS {
        assert!(
            prose.contains(&format!("`{key}`")),
            "the contract does not name the selectable label `{key}`"
        );
    }
    let rules: ExecutorRules = serde_norway::from_str(
        "executors: [{name: local, type: local}]\n\
         rules: [{when: {node_label: {step: implement}}, use: local}]\n",
    )
    .expect("it parses");
    let err = rules
        .validate()
        .expect_err("`step` is not a key the contract lists");
    assert!(err.to_string().contains("step"), "{err}");
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
        controls: NodeControls {
            max_turns: Some(24),
        },
        workspace: WorkspaceSpec::VcsSession(SessionRequest {
            repo: "nickderobertis/some-service".into(),
            branch: None,
            base: None,
            execution_checkout: None,
        }),
        cancel: CancellationToken::new(),
    };

    assert_contract_names(
        "DispatchRequest field",
        &["graph", "task", "labels", "controls", "workspace", "cancel"],
    );
    assert_eq!(
        request.controls.max_turns,
        Some(24),
        "the request carries the node's own controls, not only its labels"
    );
    assert_contract_names(
        "reserved label",
        &["run_id", "round", "node", "step", "persona"],
    );
    assert_contract_names(
        "WorkspaceSpec variant",
        &["Path(PathBuf)", "VcsSession(SessionRequest"],
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
fn the_local_executors_capacity_reports_the_three_numbers_the_contract_names() {
    // A rules file selects on these, so each has to be a number a predicate can
    // compare. Every unreadable input resolves toward "has capacity" rather than
    // toward a zero that would stall a healthy host.
    let report = LocalExecutor.capacity();
    assert!(
        report.load1.is_finite() && report.load1 >= 0.0,
        "{report:?}"
    );
    assert!(report.mem_free_bytes > 0, "{report:?}");
    assert_ne!(report, CapacityReport::default(), "nothing was probed");
    assert_contract_names(
        "CapacityReport field",
        &["slots_free", "load1", "mem_free_bytes"],
    );
}

#[test]
fn dispatching_goes_through_the_oneagentgraph_seam_and_says_so_when_it_cannot() {
    // The seam is a subprocess boundary: this crate composes `oneagentgraph`
    // rather than reimplementing it. Pointed at an executable that does not
    // exist, the failure names that sibling instead of reading as a node the
    // agent failed.
    //
    // The seam is *named* rather than left to `PATH`: `oneagentgraph` is a
    // published CLI, so a host that has it installed would otherwise make this
    // assertion depend on whose machine it ran on. nextest runs each test in its
    // own process, so the variable this sets reaches nothing else.
    std::env::set_var(
        "ONEPIPELINE_ONEAGENTGRAPH_BIN",
        "oneagentgraph-that-is-not-installed",
    );
    // `Box<dyn DispatchHandle>` is not `Debug`, so the success arm is destructured
    // rather than unwrapped.
    let Err(err) = LocalExecutor.dispatch(DispatchRequest {
        graph: ConfigRef("./graphs/node-scope.yaml".into()),
        task: "anything".into(),
        labels: Labels::default(),
        controls: NodeControls::default(),
        workspace: WorkspaceSpec::Path(PathBuf::from(".")),
        cancel: CancellationToken::new(),
    }) else {
        panic!("no `oneagentgraph` is installed here, so the dispatch cannot start");
    };
    let message = err.to_string();
    assert!(
        message.contains("oneagentgraph"),
        "the seam is unnamed: {message}"
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
                "resume": {
                    "branch": "feat/thing",
                    "checkpoint": "abc1234",
                    "completed_steps": ["implement"]
                },
                "deps": ["approval"],
                "steps": [
                    {
                        "id": "implement",
                        "persona": "engineer",
                        "task": "## What\nx",
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
    assert_eq!(
        direct.max_turns,
        Some(24),
        "a turn budget is a node-level control the schema keeps"
    );
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
            checkpoint: Some("abc1234".into()),
            completed_steps: vec!["implement".into()],
        })
    );
    let steps = lifecycle
        .steps
        .as_ref()
        .expect("nested steps on one branch");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].kind, NodeKind::Agent);
    assert_eq!(steps[1].kind, NodeKind::Human);
    assert_eq!(
        steps[0].max_turns,
        Some(32),
        "a step carries its own turn budget"
    );

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
            "per-node `max_turns`",
            "`executor: NAME`",
            "`agent_graph: REF`",
        ],
    );
}

/// The retired field, refused **by name** at every boundary a plan crosses.
///
/// `deny_unknown_fields` would answer a plan still carrying it with a bare
/// `unknown field`, which tells a planner that a field does not exist and not
/// where the review bar they wrote belongs. Every plan written before this schema
/// change carries one, so the refusal has to say where the bar goes instead.
#[test]
fn a_plan_still_carrying_done_when_is_refused_by_name_and_told_where_the_bar_goes() {
    assert!(
        CONTRACT.contains("A plan still carrying `done_when` is refused **by name**"),
        "the contract no longer states the refusal"
    );
    let root = std::env::temp_dir().join(format!("onepipeline-donewhen-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("a scratch root");
    let path = root.join("retired.plan.json");
    std::fs::write(
        &path,
        r#"{"schema_version":1,"tasks":[{"id":"contract","persona":"engineer",
            "task":"Do the thing.","done_when":"the gate is green"}]}"#,
    )
    .expect("written");

    let message = Plan::load(&path).unwrap_err().to_string();
    assert!(
        message.contains("'contract':"),
        "the refusal does not name the node that carries it: {message}"
    );
    assert!(
        message.contains("`done_when` is no longer a plan field"),
        "the refusal does not name the field: {message}"
    );
    assert!(
        message.contains("`## Acceptance criteria` section of its own task"),
        "the refusal does not say where a per-node bar goes: {message}"
    );
    assert!(
        message.contains("onejudge base config") && message.contains("user.done_when"),
        "the refusal does not say where a broader bar goes: {message}"
    );
    assert!(
        !message.contains("unknown field"),
        "the schema's bare refusal reached the planner instead: {message}"
    );

    // A step carries the same field and gets the same answer, named by the step.
    std::fs::write(
        &path,
        r#"{"schema_version":1,"tasks":[{"id":"service","repo":"o/r","steps":[
            {"id":"implement","persona":"engineer","task":"Do the thing.",
             "done_when":"the gate is green"}]}]}"#,
    )
    .expect("written");
    let message = Plan::load(&path).unwrap_err().to_string();
    assert!(
        message.contains("'implement':") && message.contains("no longer a plan field"),
        "a step's retired field is not named: {message}"
    );

    // And a plan that carries none still loads: the second, lenient reading only
    // ever runs on a document the schema already refused.
    std::fs::write(
        &path,
        r#"{"schema_version":1,"tasks":[{"id":"contract","persona":"engineer",
            "task":"Do the thing.","max_turns":45}]}"#,
    )
    .expect("written");
    assert_eq!(
        Plan::load(&path)
            .expect("a plan without the retired field loads")
            .tasks[0]
            .max_turns,
        Some(45)
    );
    std::fs::remove_dir_all(&root).ok();
}

/// A dispatch an external caller builds carries its node's controls into the
/// launch, and a control the graph has no field for refuses that launch.
///
/// Through the public seam, with no `run_id`: nothing here reads a launch record,
/// so what reaches `oneagentgraph` is the request's own `controls` or nothing at
/// all. The sibling checks an override against the same schema it reads the graph
/// with, so a `max_turns` addressed to a single-sided member — which has no such
/// field — is refused by name. That refusal is only reachable if the control was
/// transmitted; a dispatch that dropped it would launch the graph happily.
#[test]
fn a_dispatch_built_outside_a_run_still_carries_its_controls_into_the_launch() {
    let root = std::env::temp_dir().join(format!("onepipeline-seam-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("a scratch root");
    std::env::set_var("ONEAGENTGRAPH_STATE_DIR", root.join("state"));
    let graph = root.join("single-sided.yaml");
    std::fs::write(
        &graph,
        "version: 1\nname: single-sided\nmembers:\n  worker:\n    kind: oneharness\n    \
         oneharness_config: ./nothing.toml\n",
    )
    .expect("the graph is written");

    let request = |controls| DispatchRequest {
        graph: ConfigRef(graph.display().to_string()),
        task: "## What\nDo the thing.".into(),
        labels: Labels::default(),
        controls,
        workspace: WorkspaceSpec::Path(root.clone()),
        cancel: CancellationToken::new(),
    };

    let Err(refused) = LocalExecutor.dispatch(request(NodeControls {
        max_turns: Some(45),
    })) else {
        panic!("a single-sided member has no `max_turns`, so the launch cannot start");
    };
    let refused = refused.to_string();
    assert!(
        refused.contains("max_turns"),
        "the control never reached the launch: {refused}"
    );

    // Without one, nothing addresses that field at all, and the launch fails
    // further in — on the config this graph names and this test never wrote.
    let Err(other) = LocalExecutor.dispatch(request(NodeControls::default())) else {
        panic!("the graph names a config that does not exist");
    };
    assert!(
        !other.to_string().contains("max_turns"),
        "a control nobody declared was sent anyway: {other}"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// A node's turn budget reaches the graph that runs it, read off the effective
/// configuration rather than off the code path that composed it.
///
/// The overrides this crate renders are applied to the **shipped** node-scope
/// graph by `oneagentgraph`'s own applier — the same call its `run` makes before
/// it builds a member — and the worker is read out of the result. A budget the
/// overrides never carried cannot survive that, and neither can one addressed to
/// a member or a field the sibling does not have.
#[test]
fn a_declared_turn_budget_reaches_the_effective_configuration_of_the_worker() {
    assert!(
        CONTRACT.contains("`max_turns` is the worker member's own turn ceiling"),
        "the contract no longer says where a turn budget lands"
    );
    let text = std::fs::read_to_string(repo_root().join("graphs/node-scope.yaml"))
        .expect("the node-scope graph ships");

    let effective = |controls: NodeControls| -> GraphConfig {
        let overrides: Vec<_> = controls
            .overrides()
            .expect("a declared budget is appliable")
            .iter()
            .map(|set| oneagentgraph::run::parse_set(set).expect("the sibling parses the override"))
            .collect();
        let mut document: Value = serde_norway::from_str(&text).expect("the graph parses");
        oneagentgraph::run::apply_overrides(&mut document, &overrides)
            .expect("the sibling applies the override");
        serde_norway::from_value(serde_norway::to_value(&document).expect("a value"))
            .expect("the overridden graph is still a valid graph config")
    };

    let turns_of = |graph: &GraphConfig| match graph.members.get("worker") {
        Some(Member::Onejudge(worker)) => worker.max_turns,
        other => panic!("the node-scope worker is a two-party member: {other:?}"),
    };

    assert_eq!(
        turns_of(&effective(NodeControls::default())),
        None,
        "the shipped graph must state no budget, or this proves nothing"
    );
    assert_eq!(
        turns_of(&effective(NodeControls {
            max_turns: Some(45)
        })),
        Some(45),
        "the node's turn budget did not reach the member that runs its work"
    );
}

#[test]
fn resume_carries_what_the_contract_says_a_preserved_branch_needs() {
    let prose = CONTRACT.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        prose.contains("`{branch, checkpoint?, completed_steps?}`"),
        "the contract no longer states the `resume` shape"
    );
    assert!(
        prose.contains("`completed_steps` names the steps that branch already carries"),
        "the contract no longer says what `completed_steps` means"
    );
    assert!(
        prose.contains("`checkpoint` must be a commit reachable on the remote"),
        "the contract no longer says what a checkpoint is"
    );

    // The shape the contract states is the shape the schema reads, including a
    // continuation that names no steps at all.
    let full: Resume = serde_json::from_value(json!({
        "branch": "feat/thing",
        "checkpoint": "abc1234",
        "completed_steps": ["implement", "review"]
    }))
    .expect("the stated shape parses");
    assert_eq!(full.completed_steps, ["implement", "review"]);

    let minimal: Resume =
        serde_json::from_value(json!({"branch": "feat/thing"})).expect("branch alone is a resume");
    assert!(
        minimal.completed_steps.is_empty(),
        "an absent list re-runs the whole workstream"
    );
    // And an empty list is omitted again, so an old consumer sees no new field.
    assert_eq!(
        serde_json::to_value(&minimal).expect("serializes"),
        json!({"branch": "feat/thing"})
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
fn context_carries_the_three_delivery_modes_and_defaults_to_auto() {
    let prose = CONTRACT.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        prose.contains("`deliver: auto|live|next`, defaulting to `auto`"),
        "the contract no longer states the delivery modes or which one is the default"
    );
    assert!(
        prose.contains("`edit-committed` records which happened as `delivery: live | deferred`"),
        "the contract no longer says where the delivery that happened is recorded"
    );
    assert!(
        prose.contains("`oneagentgraph interrupt RUN MEMBER --input`"),
        "the contract no longer names the verb live delivery goes through"
    );

    // Every mode the contract lists is one the wire accepts, and each is a
    // different command than the others.
    let of = |value: Value| serde_json::from_value::<Edit>(value).expect("the mode parses");
    let bare = of(json!({"op": "context", "id": "slow", "note": "the fix landed"}));
    let auto =
        of(json!({"op": "context", "id": "slow", "note": "the fix landed", "deliver": "auto"}));
    let live =
        of(json!({"op": "context", "id": "slow", "note": "the fix landed", "deliver": "live"}));
    let next =
        of(json!({"op": "context", "id": "slow", "note": "the fix landed", "deliver": "next"}));
    assert_eq!(
        bare, auto,
        "a `context` edit that says nothing about delivery is not `auto`"
    );
    assert_ne!(auto, live);
    assert_ne!(live, next);
    assert_ne!(auto, next);

    // The default is omitted again, so an old consumer reading a re-serialized
    // envelope sees no field it did not have before — which is the same reason
    // every `context` edit already written keeps working.
    assert_eq!(
        serde_json::to_value(&bare).expect("serializes"),
        json!({"op": "context", "id": "slow", "note": "the fix landed"})
    );
    assert_eq!(
        serde_json::to_value(&live).expect("serializes"),
        json!({"op": "context", "id": "slow", "note": "the fix landed", "deliver": "live"})
    );

    // A fourth mode is not one the protocol has, and the refusal names what it
    // read rather than dropping the field.
    let err = serde_json::from_value::<Edit>(
        json!({"op": "context", "id": "slow", "note": "n", "deliver": "eventually"}),
    )
    .expect_err("a mode outside the three is refused");
    assert!(
        err.to_string().contains("eventually"),
        "the error names it: {err}"
    );
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

    // Each code means one thing: a caller that reads the status must not have
    // to guess which of two verdicts it got.
    let spent = [
        EXIT_SUCCESS,
        EXIT_QUEUED,
        EXIT_REFUSED,
        EXIT_NOTHING_DRIVING,
    ];
    let mut unique = spent.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), spent.len(), "two verdicts share an exit code");
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
fn the_contract_enumerates_exactly_this_librarys_own_event_kinds() {
    // Both directions. A kind the crate emits and the contract does not list is
    // undocumented wire; a kind the contract lists and the enum does not carry is
    // a promise nothing keeps. `PIPELINE_KINDS` is what `Journal::emit` accepts,
    // so this is the emitted set and not a second copy of it.
    assert_eq!(PIPELINE_KINDS.len(), 20, "the closed set changed size");
    let listed: BTreeSet<String> = backticked()
        .into_iter()
        .filter(|token| {
            token.chars().all(|c| c.is_ascii_lowercase() || c == '-') && token.contains('-')
        })
        .collect();
    for kind in PIPELINE_KINDS {
        assert!(
            listed.contains(kind.as_str()),
            "docs/contract.md does not list the `{kind}` kind this crate emits"
        );
    }

    // The wire spelling is the enum's, not a string beside it.
    assert_eq!(PipelineKind::RunStarted.as_str(), "run-started");
    assert_eq!(
        PipelineKind::from_wire(&EventKind("node-settled".into())),
        Some(PipelineKind::NodeSettled)
    );
    // A sibling's kind stays a wire string: the enum declines it rather than
    // rejecting the envelope.
    assert_eq!(
        PipelineKind::from_wire(&EventKind("gate-finished".into())),
        None
    );
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
                      [--round-budget 14400] [--heartbeat-interval 1800] \
                      [--set PATH=VALUE]... [--node-set PATH=VALUE]... \
                      [--acknowledge-concurrent]";
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
        "--set",
        "members.orchestrator.agent.model=dag one",
        "--set=members.check-in.model=dag=two",
        "--node-set",
        "members.worker.agent.model=node one",
        "--node-set=members.worker.judge.model=node=two",
        "--acknowledge-concurrent",
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
    assert!(args.acknowledge_concurrent);
    assert_eq!(
        args.dag_sets,
        [
            "members.orchestrator.agent.model=dag one",
            "members.check-in.model=dag=two"
        ]
    );
    assert_eq!(
        args.node_sets,
        [
            "members.worker.agent.model=node one",
            "members.worker.judge.model=node=two"
        ]
    );

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
        ("transcript", &["transcript", "run-1"]),
        ("transcript NODE", &["transcript", "run-1", "build"]),
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
    assert!(tokens.contains("transcript RUN [NODE]"));
    assert!(tokens.contains("runs --mine"));
}

/// The words the telemetry document writes, gated against the contract that
/// names them.
///
/// Read out of the source rather than through the types: `telemetry` is behind
/// the contract's surface — the document reaches a consumer through the CLI —
/// so this suite cannot build a `BucketName` to ask it what it spells.
#[test]
fn the_contract_names_every_bucket_and_every_usage_party_the_document_writes() {
    let source = std::fs::read_to_string(repo_root().join("src/telemetry.rs"))
        .expect("the telemetry view ships");
    let tokens = backticked();

    for (what, list) in [
        ("bucket", "pub const ALL: [Self; 8]"),
        ("party", "pub const ALL: [Self; 4]"),
    ] {
        let declared: Vec<String> = source
            .split_once(list)
            .unwrap_or_else(|| panic!("telemetry declares its {what} list"))
            .1
            .split_once("];")
            .expect("the list is closed")
            .0
            .split(',')
            .filter_map(|entry| entry.trim().strip_prefix("Self::"))
            .map(wire_word)
            .collect();
        assert!(!declared.is_empty(), "the {what} list is empty");
        for name in declared {
            assert!(
                tokens.contains(&name),
                "the contract does not name the `{name}` {what}"
            );
        }
    }

    // And the fields a party's usage carries, each one a number an operator
    // budgets against.
    for field in ["input", "output", "cache_read", "cache_write", "cost_usd"] {
        assert!(
            tokens.contains(field),
            "the contract does not name the `{field}` usage field"
        );
    }
}

/// One `CamelCase` variant as the wire spells it: `snake_case`.
fn wire_word(variant: &str) -> String {
    let mut out = String::new();
    for (at, letter) in variant.char_indices() {
        if letter.is_uppercase() && at > 0 {
            out.push('_');
        }
        out.extend(letter.to_lowercase());
    }
    out
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

/// Every divergence the record raises, and the name its ruling adopted.
///
/// A divergence is closed by the contract *saying* the thing, so each entry
/// gates both halves: the record marks the item resolved, and the contract names
/// what was ruled. An entry that quietly loses its ruling, or a contract that
/// stops naming it, fails here.
const RULINGS: &[(&str, &str)] = &[
    ("1.", "ConfigRef"),
    ("2.", "SessionRequest"),
    ("3.", "DispatchOutcome"),
    ("4.", "node_label"),
    ("5.", "min_free_mem"),
    ("6.", "PipelineKind"),
    ("7.", "completed_steps"),
    ("8.", "cross-dag-satisfied"),
    ("9.", "publication_wait"),
    ("24.", "NodeControls"),
];

#[test]
fn every_recorded_divergence_is_ruled_on_or_states_the_proposal_it_waits_on() {
    let divergences = std::fs::read_to_string(repo_root().join("docs/contract-divergences.md"))
        .expect("the divergence record ships");

    let sections: Vec<&str> = divergences.split("\n## ").skip(1).collect();
    assert!(sections.len() >= RULINGS.len(), "{sections:?}");
    let section_of = |number: &str| {
        sections
            .iter()
            .find(|section| section.starts_with(number))
            .unwrap_or_else(|| panic!("the record has no divergence {number}"))
    };

    // Every entry a ruling has not closed is a proposal the planner who owns the
    // contract has not answered. It is recorded and marked, never resolved from
    // this repository — which is the whole point of the file. Matched by the
    // entry's own number rather than by its position, so a later ruling does not
    // have to be renumbered into the leading block to be recognised.
    for section in &sections {
        let heading = section.lines().next().expect("a heading");
        if RULINGS
            .iter()
            .any(|(number, _)| heading.starts_with(number))
        {
            continue;
        }
        assert!(
            heading.ends_with("— OPEN"),
            "an unruled divergence is not marked open: {heading}"
        );
        assert!(
            section.contains("**Proposal"),
            "divergence `{heading}` is open and states no proposal"
        );
    }

    for (number, named) in RULINGS {
        let section = section_of(number);
        let heading = section.lines().next().expect("a heading");
        assert!(
            heading.ends_with("— RESOLVED"),
            "divergence {number} is not marked resolved: {heading}"
        );
        assert!(
            section.contains("**Ruling:"),
            "divergence {number} is marked resolved but records no ruling"
        );
        // `executor_has_capacity` is the one name the record and the contract
        // shared before any ruling; every other is what a ruling adopted.
        assert!(
            CONTRACT.contains(named),
            "the contract does not name `{named}`, which divergence {number} was ruled onto it"
        );
    }
    assert!(CONTRACT.contains("executor_has_capacity"));
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

    let (documented, hidden): (BTreeSet<String>, BTreeSet<String>) = Cli::command()
        .get_subcommands()
        .map(|sub| (sub.get_name().to_string(), sub.is_hide_set()))
        .fold(
            Default::default(),
            |(mut shown, mut hidden), (name, hide)| {
                if hide {
                    hidden.insert(name);
                } else {
                    shown.insert(name);
                }
                (shown, hidden)
            },
        );

    assert_eq!(
        listed, documented,
        "scripts/smoke-published.sh checks a different command set than the CLI offers"
    );

    // A hidden verb is not on `--help`, so the loop above has nothing to find it
    // in — and it is exactly the kind a published artifact could lack without
    // anything noticing, because no user types it. `drive` is what `start
    // --detach` spawns of *itself*, so a build without it cannot launch a
    // detached run at all. The script reaches each one directly instead.
    for command in &hidden {
        assert!(
            script.contains(&format!("onepipeline {command} ")),
            "scripts/smoke-published.sh never runs the hidden `{command}` command, which \
             `--help` does not list for it to check"
        );
    }
}

/// The `name: Type` pairs a struct in `src/executor.rs` declares.
///
/// Read out of the source rather than reflected off the type, because a
/// `#[non_exhaustive]` struct cannot be built field-by-field from outside the
/// crate — which is exactly the property that would otherwise catch the drift.
fn declared_fields(struct_name: &str) -> Vec<String> {
    let source = std::fs::read_to_string(repo_root().join("src/executor.rs"))
        .expect("the executor seam ships");
    let body = source
        .split_once(&format!("pub struct {struct_name} {{"))
        .expect("the struct is declared")
        .1
        .split_once("\n}")
        .expect("the struct is closed")
        .0;
    body.lines()
        .map(str::trim)
        .filter(|line| line.starts_with("pub ") && line.ends_with(','))
        .map(|line| {
            line.trim_start_matches("pub ")
                .trim_end_matches(',')
                .to_string()
        })
        .collect()
}

/// The divergences document restates two things that live in the code. Each copy
/// is gated here, so a change to the code fails this suite instead of leaving
/// the document quietly wrong.
#[test]
fn the_divergence_record_matches_the_code_it_describes() {
    let raw = std::fs::read_to_string(repo_root().join("docs/contract-divergences.md"))
        .expect("the divergence record ships");
    let doc = raw.split_whitespace().collect::<Vec<_>>().join(" ");

    // Divergence 3's ruling put `DispatchOutcome`'s fields in the contract and
    // kept this gate on the prose. The type is `#[non_exhaustive]`, so a struct
    // literal here cannot be the gate; its declaration is read instead, and
    // every field it declares must appear in both documents.
    let declared = declared_fields("DispatchOutcome");
    assert!(!declared.is_empty(), "DispatchOutcome declares no fields");
    let contract = CONTRACT.split_whitespace().collect::<Vec<_>>().join(" ");
    for field in &declared {
        assert!(
            doc.contains(field.as_str()),
            "the divergence record does not spell `{field}`, which DispatchOutcome declares"
        );
        assert!(
            contract.contains(field.as_str()),
            "the contract does not spell `{field}`, which DispatchOutcome declares"
        );
    }

    // Divergence 5 names the units the rules parser accepts.
    for unit in ["KiB", "MiB", "GiB", "TiB"] {
        assert!(
            onepipeline::rules::bytes_of(&format!("1{unit}")).is_some(),
            "the rules parser does not accept {unit}, which the record says it does"
        );
        assert!(
            doc.contains(unit),
            "the divergence record does not name the {unit} unit the parser accepts"
        );
    }
    assert!(
        onepipeline::rules::bytes_of("2GB").is_none(),
        "the record says `2GB` is treated as no limit; the parser accepted it"
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
        "transcript",
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
