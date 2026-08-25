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
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use clap::{CommandFactory, Parser};
use oneagentgraph::config::{ConfigRef, GraphConfig, JudgeSide, Member};
use oneagentgraph::persona::{merge, Persona};
use onepipeline::channel::{allows, Author, Command as Edit, Dependents, Reply, SurfaceKind};
use onepipeline::cli::{Cli, Command, DAG_GRAPH_OFF, DEFAULT_HEARTBEAT_INTERVAL_SECONDS};
use onepipeline::controls::NodeControls;
use onepipeline::error::{EXIT_NOTHING_DRIVING, EXIT_QUEUED, EXIT_REFUSED, EXIT_SUCCESS};
use onepipeline::event::{
    ArtifactId, ArtifactRef, Envelope, EventKind, Labels, Phase, PipelineKind, Source,
    ENVELOPE_VERSION, PIPELINE_KINDS,
};
use onepipeline::executor::{
    CancelMode, CancellationToken, Capabilities, CapacityReport, DispatchRequest, Executor,
    LocalExecutor, WorkspaceSpec,
};
use onepipeline::filter::{
    EventFilter, Filters, LaunchConfig, Matcher, LAUNCH_CONFIG_SCHEMA_VERSION,
    LAUNCH_CONFIG_SCHEMA_VERSIONS_READ,
};
use onepipeline::plan::{
    Node, NodeKind, Plan, Resume, Step, AMENDMENT_HEADING, CROSS_REPO_REFERENCES_HEADING,
    PLANNER_CONTEXT_HEADING, PLAN_SCHEMA_VERSION, PLAN_SCHEMA_VERSIONS_READ,
};
use onepipeline::report::{
    retain, ACCEPTED_REPORT_FILE, MAX_REPORT_BYTES, MEMBER_SETTLED, REPORT_PATH,
};
use onepipeline::rules::{ExecutorKind, ExecutorRules, Predicate};
use onepipeline::views::RunPaths;
use onevcs::registry::{RepoType, Workflow};
use onevcs::{Adoption, MergePolicy, SessionRequest};
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

/// The one fenced block of that language whose body names `needle`.
///
/// The contract carries more than one example in a given language, so a fixture
/// says which of them it is about by naming a key only that one has — rather
/// than by an index, which a block inserted above it would silently shift onto
/// the wrong example.
fn fenced_block_naming(language: &str, needle: &str) -> String {
    let mut matching: Vec<String> = fenced_blocks(language)
        .into_iter()
        .filter(|body| body.contains(needle))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one ```{language} block naming {needle:?} in docs/contract.md, found {}",
        matching.len()
    );
    matching.pop().expect("one block")
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
    let yaml = fenced_block_naming("yaml", "executors:");
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
        serde_norway::from_str(&fenced_block_naming("yaml", "executors:"))
            .expect("the contract's example parses");
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
            max_turns: NonZeroU32::new(24),
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
        NonZeroU32::new(24),
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

/// The `filters:` block in the contract is a block this crate's own types read.
///
/// Driven out of the document, like every other fixture here: the grammar is
/// shared across the stack with no shared crate, so the committed text is the one
/// source and a copy that stopped matching it fails this gate.
#[test]
fn the_contracts_launch_config_example_parses_and_round_trips() {
    let yaml = fenced_block_naming("yaml", "schema_version: 2");
    let config: LaunchConfig = serde_norway::from_str(&yaml).expect("the launch config parses");
    // A version this build **reads**, which is what the contract's committed
    // example is: the document is approved verbatim and is never edited to
    // follow the schema, so a later version having been written since does not
    // stop the example being a config an operator still has on disk.
    assert!(
        LAUNCH_CONFIG_SCHEMA_VERSIONS_READ.contains(&config.schema_version),
        "the contract's example declares a version this build does not read: {}",
        config.schema_version
    );
    assert_eq!(
        config.pr_author_graph.as_deref(),
        Some("./graphs/pr-author.yaml"),
        "the contract's example declares the launch's other decision and this build \
         does not read it"
    );
    let filters = config.filters;

    let agentgraph = filters
        .agentgraph
        .as_ref()
        .expect("it names a source filter");
    assert_eq!(agentgraph.include, Vec::new(), "an absent include is empty");
    assert_eq!(agentgraph.exclude.len(), 1);
    assert_eq!(agentgraph.exclude[0].kind.as_deref(), Some("turn-activity"));
    let vcs = filters.vcs.as_ref().expect("it names a vcs filter");
    assert_eq!(vcs.include.len(), 2);
    assert_eq!(vcs.include[0].kind.as_deref(), Some("gate-*"));

    // The two shipped profiles, exactly as the contract states them: an
    // override that changed either would be a run whose default view is not the
    // documented one.
    assert_eq!(
        filters.profiles["planner"],
        EventFilter {
            include: vec![Matcher {
                source: Some(Source::Pipeline),
                ..Matcher::default()
            }],
            exclude: Vec::new(),
        }
    );
    assert_eq!(filters.profiles["monitor"], EventFilter::default());

    let round_tripped: Filters =
        serde_json::from_str(&serde_json::to_string(&filters).expect("serializes"))
            .expect("re-parses");
    assert_eq!(round_tripped, filters);

    // The checked-in golden **is** the contract's own example. Two documents that
    // both claim to pin the launch config's shape and could disagree would be two
    // sources; this is the one place they are held to being one.
    let golden: LaunchConfig = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join("tests/golden/launch-config-v2.json"))
            .expect("the golden ships"),
    )
    .expect("the golden parses");
    assert_eq!(
        (
            golden.schema_version,
            golden.filters,
            golden.pr_author_graph
        ),
        (config.schema_version, filters, config.pr_author_graph),
        "tests/golden/launch-config-v2.json and the contract's own example are \
         different documents"
    );

    // The version before it is still a document this build reads, and it ships
    // as its own golden: the bump is additive, and that promise is to every
    // config already written beside a plan.
    let earlier: LaunchConfig = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join("tests/golden/launch-config-v1.json"))
            .expect("the earlier golden ships"),
    )
    .expect("the earlier golden parses");
    assert_eq!(earlier.schema_version, 1);
    assert_eq!(
        earlier.pr_author_graph, None,
        "the earlier golden carries a key that version never had"
    );
    assert!(
        CONTRACT.contains("a version-1 config is a complete document this build still reads"),
        "the contract no longer says an earlier launch config still reads"
    );
}

/// A launch config declaring only its version is a launch that says nothing.
///
/// The contract's promise to a document written before the block existed, and to
/// one that never wanted it: the block is optional, an empty one is omitted from
/// what this crate writes, and neither is an error.
#[test]
fn the_contracts_launch_config_omits_an_empty_block_and_still_reads() {
    for version in LAUNCH_CONFIG_SCHEMA_VERSIONS_READ {
        let bare: LaunchConfig = serde_norway::from_str(&format!("schema_version: {version}\n"))
            .expect("a config may declare only a version");
        assert!(bare.filters.is_empty());
        assert_eq!(bare.pr_author_graph, None);
        assert_eq!(bare.node_validator, None);
    }
    // And what this crate *writes* for one is the version alone: every optional
    // key is omitted, so a launch that declared none is a document an earlier
    // reader accepts.
    assert_eq!(
        serde_json::to_string(&LaunchConfig::default()).expect("serializes"),
        format!(r#"{{"schema_version":{LAUNCH_CONFIG_SCHEMA_VERSION}}}"#),
        "an empty filters block, an absent drafting graph, or an absent node \
         validator was written out"
    );
    assert_contract_names(
        "launch config surface",
        &["--launch-config FILE", "schema_version: 2"],
    );
}

/// The shipped defaults are the contract's, and both are overridable by name.
#[test]
fn the_shipped_profiles_are_the_contracts_own_and_are_overridable() {
    let empty = Filters::default();
    assert_eq!(
        empty.profile("planner").expect("planner ships"),
        EventFilter {
            include: vec![Matcher {
                source: Some(Source::Pipeline),
                ..Matcher::default()
            }],
            exclude: Vec::new(),
        }
    );
    assert_eq!(
        empty.profile("monitor").expect("monitor ships"),
        EventFilter::default(),
        "the shipped monitor profile is unfiltered"
    );

    let mine = EventFilter::parse(r#"{"include": [{"kind": "node-*"}]}"#).expect("a filter");
    let overridden = Filters {
        profiles: [
            ("planner".to_string(), mine.clone()),
            ("monitor".to_string(), mine.clone()),
        ]
        .into_iter()
        .collect(),
        ..Filters::default()
    };
    assert_eq!(overridden.profile("planner").expect("overridden"), mine);
    assert_eq!(overridden.profile("monitor").expect("overridden"), mine);

    let unknown = empty
        .profile("planer")
        .expect_err("a profile this run does not have is refused");
    let said = unknown.to_string();
    assert!(said.contains("planer"), "{said}");
    assert!(
        said.contains("planner") && said.contains("monitor"),
        "{said}"
    );
}

/// The grammar's refusal semantics, at the boundary a spec crosses.
#[test]
fn a_filter_spec_is_refused_by_the_shared_grammars_own_rules() {
    let unknown_field = EventFilter::parse(r#"{"include": [{"role": "agent"}]}"#)
        .expect_err("a matcher field the grammar does not have is refused");
    let said = unknown_field.to_string();
    assert!(said.contains("role"), "the refusal names the field: {said}");
    assert!(
        said.contains("include") && said.contains('1'),
        "the refusal says which list and where in it: {said}"
    );

    let stray = EventFilter::parse(r#"{"includes": []}"#)
        .expect_err("a filter names include and exclude and nothing else");
    assert!(stray.to_string().contains("includes"), "{stray}");

    // `round` is a reserved label the approved matcher list does not name, so it
    // is refused here like any other non-field rather than quietly accepted.
    let deprecated = EventFilter::parse(r#"{"include": [{"round": "1"}]}"#)
        .expect_err("`round` is not in the grammar");
    assert!(deprecated.to_string().contains("round"), "{deprecated}");

    // Both refusals are at the one boundary: a spec reaches this crate from a
    // command line, from a file, and from the launch record every later read
    // opens, and a filter checked only where an operator typed it would be a
    // record that could be edited into a matcher this build says it will not
    // honour — and then honoured.
    let empty_matcher = EventFilter::parse(r#"{"exclude": [{}]}"#)
        .expect_err("a matcher naming no field matches everything");
    assert!(
        empty_matcher.to_string().contains("exclude"),
        "{empty_matcher}"
    );

    let empty_field = EventFilter::parse(r#"{"include": [{"kind": ""}]}"#)
        .expect_err("nothing on the stream carries an empty kind");
    assert!(empty_field.to_string().contains("kind"), "{empty_field}");

    // The launch record is that boundary too, and it is the one an operator
    // never typed at: a block edited into a matcher naming nothing is refused
    // where the record is read.
    let record = serde_json::from_str::<Filters>(r#"{"vcs": {"exclude": [{}]}}"#)
        .expect_err("a launch record carrying an unusable filter is refused");
    assert!(record.to_string().contains("exclude"), "{record}");
}

/// `exclude` wins, an absent `include` admits everything, and a glob is `*`.
#[test]
fn the_grammar_matches_the_way_the_contract_says_it_does() {
    let envelope = |source: Source, kind: &str, labels: Labels| Envelope {
        v: ENVELOPE_VERSION,
        ts: "2026-08-15T00:00:00.000Z".into(),
        stream: "s".into(),
        seq: 0,
        source,
        kind: EventKind(kind.into()),
        phase: None,
        labels,
        payload: Default::default(),
        artifacts: Vec::new(),
    };
    let plain = envelope(Source::Agentgraph, "turn-activity", Labels::default());

    assert!(
        EventFilter::default().matches(&plain),
        "an absent include admits everything"
    );
    let excluded = EventFilter::parse(r#"{"exclude": [{"kind": "turn-*"}]}"#).expect("a filter");
    assert!(!excluded.matches(&plain), "a glob matches the wire string");
    let both = EventFilter::parse(
        r#"{"include": [{"source": "agentgraph"}], "exclude": [{"kind": "turn-activity"}]}"#,
    )
    .expect("a filter");
    assert!(!both.matches(&plain), "exclude wins over include");

    // A label the envelope never stamped is not a wildcard.
    let asks_node = EventFilter::parse(r#"{"include": [{"node": "build"}]}"#).expect("a filter");
    assert!(
        !asks_node.matches(&plain),
        "an unstamped label never matches"
    );
    assert!(asks_node.matches(&envelope(
        Source::Pipeline,
        "node-settled",
        Labels {
            node: Some("build".into()),
            ..Labels::default()
        }
    )));

    // `member` has no typed slot on this crate's labels, so it is read out of
    // the extras a relayed envelope stamps it in.
    let asks_member =
        EventFilter::parse(r#"{"include": [{"member": "worker"}]}"#).expect("a filter");
    let mut relayed = plain.clone();
    relayed
        .labels
        .extra
        .insert("member".into(), json!("worker"));
    assert!(asks_member.matches(&relayed));
    assert!(!asks_member.matches(&plain));
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

/// The contract's schema version and this crate's are the same number, and every
/// version the document says this build reads, it reads.
///
/// The plan schema is a serialized contract: a document says which version it
/// was written at, and a reader decides by that. So the number the document
/// states and the number the code writes are gated against each other here — and
/// so is the set below it, because "an earlier plan still runs" is a promise to
/// every plan already written on a host and there is nothing else holding it.
#[test]
fn the_contracts_plan_schema_version_is_the_one_this_crate_writes() {
    assert!(
        CONTRACT.contains(&format!("Plan schema v{PLAN_SCHEMA_VERSION} =")),
        "the contract states a different plan schema version than this crate writes \
         ({PLAN_SCHEMA_VERSION})"
    );
    assert!(
        CONTRACT.contains("this build reads **3, 2, and 1**"),
        "the contract no longer names the versions this build reads"
    );
    assert_eq!(
        PLAN_SCHEMA_VERSIONS_READ,
        [3, 2, 1],
        "this crate reads a different set of versions than the contract states"
    );

    let root = std::env::temp_dir().join(format!("onepipeline-version-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("a scratch root");
    // Every version the contract names, as a document an operator wrote: each
    // one loads, and each keeps the version it declares — a reader decides by
    // that number, so a loader that normalized it would answer for a document
    // nobody wrote. That they *execute* is driven through the binary, in
    // `tests/e2e/plan.rs`, and all the way to a publication in
    // `tests/e2e/lifecycle.rs`, because that is where a planner meets either
    // answer.
    for version in PLAN_SCHEMA_VERSIONS_READ {
        let path = root.join(format!("v{version}.plan.json"));
        std::fs::write(
            &path,
            format!(
                r#"{{"schema_version":{version},
                    "tasks":[{{"id":"a","persona":"engineer","task":"Do it."}}]}}"#
            ),
        )
        .expect("written");
        let plan = Plan::load(&path)
            .unwrap_or_else(|why| panic!("a version {version} plan is a readable document: {why}"));
        assert_eq!(plan.schema_version, version);
    }

    // What this crate *writes* carries the current number, whatever it read.
    let earlier = Plan::load(&root.join("v1.plan.json")).expect("it still loads");
    let current = Plan {
        schema_version: PLAN_SCHEMA_VERSION,
        ..earlier
    };
    let written = serde_json::to_value(&current).expect("it serialises");
    assert_eq!(written["schema_version"], PLAN_SCHEMA_VERSION);
    std::fs::remove_dir_all(&root).ok();
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
    // At the retired version, as every plan carrying this field is: the field is
    // what its author has to move, so the field is what they are told about.
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
    assert!(
        !message.contains("schema_version"),
        "the version refusal displaced the field's: {message}"
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
        format!(
            r#"{{"schema_version":{PLAN_SCHEMA_VERSION},"tasks":[
                {{"id":"contract","persona":"engineer","task":"Do the thing.",
                 "max_turns":45}}]}}"#
        ),
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
        max_turns: NonZeroU32::new(45),
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
            max_turns: NonZeroU32::new(45)
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
        Edit::Amend { .. } => "amend",
        Edit::Finding { .. } => "finding",
    }
}

/// Every surface kind this build carries, in its wire spelling.
///
/// Enumerated rather than listed: `SurfaceKind` is `#[non_exhaustive]`, so no
/// match written out here can be exhaustive, and a hardcoded array would miss
/// exactly the variant somebody added without writing it down. `ValueEnum` is
/// what `--kind` already parses against, so this *is* the set a caller can spell
/// and the set the queue can hold.
fn every_surface_kind() -> BTreeSet<String> {
    <SurfaceKind as clap::ValueEnum>::value_variants()
        .iter()
        .map(|kind| {
            serde_json::to_value(kind)
                .expect("a surface kind serializes")
                .as_str()
                .expect("as a string")
                .to_string()
        })
        .collect()
}

/// The ops the contract lists, in the order it lists them.
const OPS: &[&str] = &[
    "add", "drop", "reparent", "retry", "cancel", "requeue", "attest", "complete", "context",
];

/// The per-author allowlist the contract fixes, both directions.
///
/// A monitor is an observer: it may correct and re-run work, and it may not
/// decide that the run is finished, that a person acted, or that work leaves the
/// graph. Held against every op the protocol has, so an op added later is
/// refused for the monitor until somebody decides otherwise rather than granted
/// by omission.
#[test]
fn the_monitor_may_issue_exactly_the_ops_the_contract_allows_it() {
    assert!(
        CONTRACT.contains(
            "`monitor` may issue `retry | requeue | cancel | context | add` only, and \
             `complete`, `attest`, and `drop` are refused for the monitor with a reason"
        ),
        "the contract's per-author allowlist moved"
    );

    let node = Node {
        id: "fresh".into(),
        persona: Some("engineer".into()),
        task: Some("## What\ndo it".into()),
        ..Node::default()
    };
    let every: Vec<(&str, Edit)> = vec![
        ("add", Edit::Add { node: node.clone() }),
        (
            "drop",
            Edit::Drop {
                id: "x".into(),
                dependents: Dependents::Detach,
            },
        ),
        (
            "reparent",
            Edit::Reparent {
                id: "x".into(),
                deps: Vec::new(),
            },
        ),
        (
            "retry",
            Edit::Retry {
                id: "x".into(),
                node,
            },
        ),
        ("cancel", Edit::Cancel { id: "x".into() }),
        (
            "requeue",
            Edit::Requeue {
                id: "x".into(),
                amend: None,
            },
        ),
        (
            "attest",
            Edit::Attest {
                reference: "x".into(),
            },
        ),
        (
            "complete",
            Edit::Complete {
                reason: "done".into(),
            },
        ),
        (
            "context",
            Edit::Context {
                id: "x".into(),
                note: "look here".into(),
                deliver: onepipeline::channel::Deliver::Auto,
            },
        ),
    ];
    assert_eq!(every.len(), OPS.len(), "an op is missing from this table");

    let allowed = ["retry", "requeue", "cancel", "context", "add"];
    for (op, command) in &every {
        // The planner owns the graph, so nothing is refused for it.
        allows(Author::Planner, command)
            .unwrap_or_else(|e| panic!("the planner was refused `{op}`: {e}"));

        let verdict = allows(Author::Monitor, command);
        if allowed.contains(op) {
            verdict.unwrap_or_else(|e| panic!("the monitor was refused `{op}`: {e}"));
            continue;
        }
        let refusal = verdict
            .expect_err(&format!("the monitor was allowed `{op}`"))
            .to_string();
        assert!(
            refusal.contains(op),
            "the refusal does not name the op: {refusal}"
        );
        // With a reason, not merely a no: the monitor has to know what to do
        // instead, and "surface it" is the whole answer.
        assert!(
            refusal.contains("Surface it to the planner"),
            "the refusal does not say what to do instead: {refusal}"
        );
    }

    // The author rides the envelope, defaults to the planner, and is omitted
    // when it is the default — so a reply written before authors existed is one.
    let plain: Reply = serde_json::from_str(r#"{"completion":true}"#).expect("it parses");
    assert_eq!(plain.author, Author::Planner);
    assert!(
        !serde_json::to_string(&plain)
            .expect("it serializes")
            .contains("author"),
        "the default author is written out"
    );
    let watched: Reply = serde_json::from_str(r#"{"version":1,"author":"monitor","commands":[]}"#)
        .expect("it parses");
    assert_eq!(watched.author, Author::Monitor);
    assert_eq!(Author::Monitor.as_str(), "monitor");
    assert_eq!(Author::Planner.as_str(), "planner");
}

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

/// One divergence entry's fenced JSON block, by the number it opens with.
fn divergence_block(number: &str) -> Value {
    let record = std::fs::read_to_string(repo_root().join("docs/contract-divergences.md"))
        .expect("the divergence record reads");
    let entry = record
        .split("\n## ")
        .find(|entry| entry.starts_with(number))
        .unwrap_or_else(|| panic!("the divergence record still carries entry {number}"));
    let block = entry
        .split("```json")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .unwrap_or_else(|| panic!("entry {number} carries the json block this test drives"));
    serde_json::from_str(block).unwrap_or_else(|e| panic!("entry {number}'s block is JSON: {e}"))
}

/// The ops and surface kinds this build carries **beyond** the contract's own
/// lists are exactly the ones the divergence record proposes.
///
/// The contract is committed as approved and names neither, so the entry that
/// proposes them is the only place they are written down — and a divergence
/// nothing gates quietly stops being true. Both directions: a build that grows
/// an op or a kind the entry does not name fails here as loudly as one that
/// drops one it does.
#[test]
fn what_this_build_carries_beyond_the_contract_is_what_the_divergence_record_names() {
    let block = divergence_block("39.");
    let fixtures: Vec<Value> =
        serde_json::from_value(block["ops"].clone()).expect("entry 39 names the ops it adds");
    let monitor_may: BTreeSet<String> = serde_json::from_value(block["monitor_may_issue"].clone())
        .expect("entry 39 says which of them the monitor may issue");
    let kinds: Vec<String> = serde_json::from_value(block["surface_kinds"].clone())
        .expect("entry 39 names the surface kinds it adds");
    assert!(!fixtures.is_empty() && !kinds.is_empty(), "{block}");

    for fixture in &fixtures {
        let op = fixture["op"].as_str().expect("the fixture names its op");
        assert!(
            !OPS.contains(&op),
            "`{op}` is on the contract's own list, so it is no divergence"
        );
        // Written as the wire carries it: the entry's block is the source, so
        // what parses here is what a planner would type.
        let edit: Edit = serde_json::from_value(fixture.clone())
            .unwrap_or_else(|e| panic!("`{op}` deserializes: {e}"));
        assert_eq!(op_of(&edit), op, "`{op}` deserialized into another variant");
        assert_eq!(
            &serde_json::to_value(&edit).expect("serializes"),
            fixture,
            "`{op}` round-trips unchanged"
        );

        // The planner owns the graph, so nothing is refused for it.
        allows(Author::Planner, &edit)
            .unwrap_or_else(|e| panic!("the planner was refused `{op}`: {e}"));
        let verdict = allows(Author::Monitor, &edit);
        if monitor_may.contains(op) {
            verdict.unwrap_or_else(|e| panic!("the monitor was refused `{op}`: {e}"));
        } else {
            verdict.expect_err(&format!("the monitor was allowed `{op}`"));
        }
    }

    // The kind set is the contract's one plus the entry's, and nothing else —
    // held against the variants themselves, so a kind added without a line in
    // either document fails here rather than shipping unwritten-down.
    for kind in &kinds {
        assert!(
            !CONTRACT.contains(&format!("--kind {kind}")),
            "the contract names `{kind}`, so it is no divergence"
        );
        let parsed: SurfaceKind = serde_json::from_value(json!(kind))
            .unwrap_or_else(|e| panic!("`{kind}` is a kind this build parses: {e}"));
        assert_eq!(
            serde_json::to_value(parsed).expect("serializes"),
            json!(kind),
            "`{kind}` round-trips unchanged"
        );
    }
    let declared: BTreeSet<String> = std::iter::once("check-in".to_string())
        .chain(kinds.iter().cloned())
        .collect();
    assert_eq!(
        every_surface_kind(),
        declared,
        "the surface kinds this build carries are not the contract's plus entry 39's"
    );
}

/// The release-adoption surface this build carries **beyond** the contract is
/// exactly what the divergence record proposes.
///
/// The contract is committed as approved and names none of it, so entry 40 is the
/// only place it is written down — and a divergence nothing gates quietly stops
/// being true. The entry's own block is the source: what parses here is what a
/// planner would write in a plan file.
#[test]
fn the_release_adoption_surface_is_what_the_divergence_record_names() {
    let block = divergence_block("40.");

    // The node shape, exactly as the entry writes it.
    let written = block["node"].clone();
    let node: Node = serde_json::from_value(written.clone()).expect("entry 40's node parses");
    assert_eq!(
        serde_json::to_value(&node).expect("serializes"),
        written,
        "entry 40's node does not round-trip as written"
    );
    assert_eq!(
        node.adoption,
        Some(Adoption::Published),
        "`adoption` is the node rung of the chain"
    );
    assert_eq!(
        node.consumes
            .get("engine")
            .map(std::string::ToString::to_string),
        Some("crate".to_string()),
        "`consumes` is keyed by dependency node id"
    );
    // At schema 3, and optional: a plan naming neither field is the plan it
    // always was, and round-trips without either appearing.
    let plain = json!({"id": "solo", "persona": "engineer", "task": "## What\nx"});
    let bare: Node = serde_json::from_value(plain.clone()).expect("a node naming neither parses");
    assert_eq!(bare.adoption, None);
    assert!(bare.consumes.is_empty());
    assert_eq!(
        serde_json::to_value(&bare).expect("serializes"),
        plain,
        "a node naming neither field gained one on the way out"
    );
    // The event kinds, held against the enum itself and against the contract.
    let kinds: Vec<String> =
        serde_json::from_value(block["event_kinds"].clone()).expect("entry 40 names its kinds");
    assert!(!kinds.is_empty());
    for kind in &kinds {
        assert!(
            PipelineKind::from_wire(&EventKind(kind.clone())).is_some(),
            "`{kind}` is not a kind this crate emits"
        );
    }

    // The heading, which is a published constant so a reader finds the block by
    // name rather than by matching prose.
    assert_eq!(
        block["heading"].as_str(),
        Some(CROSS_REPO_REFERENCES_HEADING),
        "entry 40 names a different heading than this crate publishes"
    );
    assert_ne!(CROSS_REPO_REFERENCES_HEADING, PLANNER_CONTEXT_HEADING);
}

/// The amendment lever and the node-validator hook this build carries **beyond**
/// the contract are exactly what the divergence record proposes.
///
/// The contract is committed as approved and names neither, so entry 41 is the
/// only place they are written down — and a divergence nothing gates quietly
/// stops being true. The entry's own block is the source: what parses here is
/// what a planner would type and what an operator would write in a config.
#[test]
fn the_amendment_and_validator_surface_is_what_the_divergence_record_names() {
    let block = divergence_block("41.");

    // The op, as the wire carries it.
    let fixtures: Vec<Value> =
        serde_json::from_value(block["ops"].clone()).expect("entry 41 names the op it adds");
    let monitor_may: BTreeSet<String> = serde_json::from_value(block["monitor_may_issue"].clone())
        .expect("entry 41 says which of them the monitor may issue");
    assert!(!fixtures.is_empty(), "{block}");
    for fixture in &fixtures {
        let op = fixture["op"].as_str().expect("the fixture names its op");
        assert!(
            !OPS.contains(&op),
            "`{op}` is on the contract's own list, so it is no divergence"
        );
        let edit: Edit = serde_json::from_value(fixture.clone())
            .unwrap_or_else(|e| panic!("`{op}` deserializes: {e}"));
        assert_eq!(op_of(&edit), op, "`{op}` deserialized into another variant");
        assert_eq!(
            &serde_json::to_value(&edit).expect("serializes"),
            fixture,
            "`{op}` round-trips unchanged"
        );
        allows(Author::Planner, &edit)
            .unwrap_or_else(|e| panic!("the planner was refused `{op}`: {e}"));
        let verdict = allows(Author::Monitor, &edit);
        if monitor_may.contains(op) {
            verdict.unwrap_or_else(|e| panic!("the monitor was refused `{op}`: {e}"));
            continue;
        }
        // Refused, and the refusal names the op — an observer told only "no"
        // has nothing to act on.
        let refusal = verdict
            .expect_err(&format!("the monitor was allowed `{op}`"))
            .to_string();
        assert!(
            refusal.contains(op) && refusal.contains("Surface it to the planner"),
            "the refusal does not name `{op}` and what to do instead: {refusal}"
        );
    }

    // The node field, at schema 3 and optional, exactly as the entry writes it.
    let written = block["node"].clone();
    let node: Node = serde_json::from_value(written.clone()).expect("entry 41's node parses");
    assert_eq!(
        serde_json::to_value(&node).expect("serializes"),
        written,
        "entry 41's node does not round-trip as written"
    );
    let text = node.amendment.clone().expect("the node carries one");
    let plain = json!({"id": "solo", "persona": "engineer", "task": "## What\nx"});
    let bare: Node = serde_json::from_value(plain.clone()).expect("a node naming none parses");
    assert_eq!(bare.amendment, None);
    assert_eq!(
        serde_json::to_value(&bare).expect("serializes"),
        plain,
        "a node naming no amendment gained one on the way out"
    );

    // The heading, which is a published constant, and the rendering it opens:
    // the amendment states its own authority over the notes it sits above.
    assert_eq!(
        block["heading"].as_str(),
        Some(AMENDMENT_HEADING),
        "entry 41 names a different heading than this crate publishes"
    );
    assert_ne!(AMENDMENT_HEADING, PLANNER_CONTEXT_HEADING);
    let rendered = Node {
        task: Some("## What\nship it\n\n## Additional info\n\nrun the gate.\n".into()),
        ..node.clone()
    }
    .rendered_task();
    let (heading, notes) = (
        rendered.find(AMENDMENT_HEADING).expect("it is rendered"),
        rendered.find("## Additional info").expect("the notes are"),
    );
    assert!(
        heading < notes,
        "the amendment is below the notes: {rendered}"
    );
    assert!(rendered.contains(&text), "{rendered}");
    assert!(
        rendered.contains("this section wins"),
        "the amendment does not state its authority: {rendered}"
    );

    // The validator, named three ways, with the config key at the version the
    // entry states.
    let validator = &block["validator"];
    assert_eq!(validator["config_key"].as_str(), Some("node_validator"));
    let at = validator["config_schema_version"]
        .as_u64()
        .expect("entry 41 states the version the key arrived at");
    assert_eq!(
        u32::try_from(at).expect("a version fits"),
        LAUNCH_CONFIG_SCHEMA_VERSION
    );
    let named: LaunchConfig = serde_json::from_value(json!({
        "schema_version": at,
        "node_validator": "./scripts/check-node.sh",
    }))
    .expect("a launch config naming a validator parses");
    assert_eq!(
        named.node_validator.as_deref(),
        Some("./scripts/check-node.sh")
    );
    // The flag is the one `start` actually takes, asked of the parser rather
    // than of a list beside it.
    let flag = validator["flag"].as_str().expect("entry 41 names the flag");
    let parsed = Cli::try_parse_from(["onepipeline", "start", "plan.json", flag, "check-node"])
        .expect("the flag entry 41 names is one `start` takes");
    let Command::Start(started) = parsed.command else {
        panic!("that is not a start")
    };
    assert_eq!(started.node_validator.as_deref(), Some("check-node"));
    // And the ops it is offered, which the journeys drive.
    let offered: BTreeSet<String> = serde_json::from_value(validator["ops_offered"].clone())
        .expect("entry 41 names the ops a validator is offered");
    assert_eq!(
        offered,
        ["add", "amend", "requeue", "retry"]
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<String>>()
    );

    // The README is a **second copy** of all of this, in the prose an operator
    // meets it in, and nothing compiles that. So the entry is held against it
    // here: every name and every op above has to appear there, and the two
    // headings the guidance turns on have to be the constants this crate
    // publishes rather than prose that drifted away from them.
    let readme = std::fs::read_to_string(repo_root().join("README.md")).expect("the README ships");
    let prose = readme.split_whitespace().collect::<Vec<_>>().join(" ");
    let precedence: Vec<String> = serde_json::from_value(validator["precedence"].clone())
        .expect("entry 41 states the order it proposes");
    let mut at: Vec<usize> = Vec::new();
    for named in &precedence {
        let spelling = validator[named.as_str()]
            .as_str()
            .expect("entry 41 names it");
        let found = prose.find(spelling).unwrap_or_else(|| {
            panic!("the README does not name the validator's {named}, `{spelling}`")
        });
        at.push(found);
    }
    // And in the order the entry proposes: a README listing them the other way
    // round would read as a different rule while naming the same three things.
    assert!(
        at.windows(2).all(|pair| pair[0] < pair[1]),
        "the README names the three spellings in an order entry 41 does not propose: \
         {precedence:?} at {at:?}"
    );
    for op in offered
        .iter()
        .chain(std::iter::once(&"context".to_string()))
    {
        assert!(
            prose.contains(&format!("`{op}`")),
            "the README's live-edit guidance does not name `{op}`"
        );
    }
    for heading in [AMENDMENT_HEADING, PLANNER_CONTEXT_HEADING] {
        assert!(
            prose.contains(heading),
            "the README does not name `{heading}`, which is where this crate renders one of              the two levers"
        );
    }
    // And the distinction itself, which is the whole reason both exist: the
    // README has to say which one moves the bar and which one only steers.
    assert!(
        prose.contains("adds no acceptance criteria")
            && prose.contains("the worker and the judge reviewing it read the same ruling"),
        "the README no longer states which lever changes what a node is judged against and          which one only steers its worker"
    );
    // The refusal it promises for a rejected edit, and the default it promises
    // for a launch that names none.
    assert!(
        prose.contains("a non-zero exit refuses it with the command's own stderr as the reason")
            && prose.contains("naming none is the default and runs no validator at all"),
        "the README no longer states what a validator's answers mean"
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

/// The halves the contract routes a reply by, as the wire shape declares them.
///
/// A pending surface is answered by a **verdict half** and nothing else, so the
/// three fields it is spelled with are what tells the two readers' envelopes
/// apart — and a commands-only envelope declares none of them. No addressing
/// field was minted for this: the discrimination is already in the wire shape,
/// which is what this asserts, alongside the routing the document states.
#[test]
fn a_reply_declares_the_halves_the_contract_routes_it_by() {
    let read = |value: Value| serde_json::from_value::<Reply>(value).expect("the envelope parses");

    let commands_only = read(json!({
        "version": 1,
        "commands": [{"op": "context", "id": "plan", "note": "the scope changed"}]
    }));
    assert_eq!(commands_only.completion, None);
    assert_eq!(commands_only.message, None);
    assert_eq!(commands_only.reason, None);
    assert_eq!(commands_only.commands.len(), 1);

    for (half, value) in [
        ("completion", json!({"completion": false})),
        ("message", json!({"message": "keep going"})),
        ("reason", json!({"reason": "keep going"})),
    ] {
        let alone = read(value);
        assert!(
            alone.completion.is_some() || alone.message.is_some() || alone.reason.is_some(),
            "`{half}` alone declares no verdict half"
        );
        assert!(
            alone.commands.is_empty(),
            "`{half}` alone declares a commands half"
        );
    }

    let both = read(json!({
        "completion": false,
        "reason": "retry it",
        "version": 1,
        "commands": [{"op": "cancel", "id": "slow"}]
    }));
    assert_eq!(both.reason.as_deref(), Some("retry it"));
    assert_eq!(both.commands.len(), 1);

    assert!(
        CONTRACT.contains(
            "**A reply is routed by the halves it carries, never by which reader reaches the \
             queue first.**"
        ),
        "the contract no longer states that a reply is routed by its halves"
    );
    assert!(
        CONTRACT.contains(
            "It answers a pending surface only when it carries a **verdict half** — \
             `completion`, `message`, or `reason`"
        ),
        "the contract no longer says which half answers a pending surface"
    );
    assert!(
        CONTRACT.contains(
            "belongs to the command path alone: it leaves the pending surface, and any reader \
             waiting there for a verdict, untouched"
        ),
        "the contract no longer says where a commands-only envelope goes"
    );
    assert!(
        CONTRACT.contains("One carrying both is delivered to both"),
        "the contract no longer says what an envelope carrying both halves does"
    );
    assert!(
        CONTRACT.contains("Neither reader advances the other's cursor"),
        "the contract no longer promises the two cursors stay apart"
    );
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
    assert_eq!(PIPELINE_KINDS.len(), 23, "the closed set changed size");
    let listed: BTreeSet<String> = backticked()
        .into_iter()
        .filter(|token| {
            token.chars().all(|c| c.is_ascii_lowercase() || c == '-') && token.contains('-')
        })
        .collect();
    // The kinds the contract does not list are exactly the ones the divergence
    // record proposes, and no others: a kind neither document names fails here.
    let proposed: BTreeSet<String> =
        serde_json::from_value(divergence_block("40.")["event_kinds"].clone())
            .expect("entry 40 names the kinds it adds");
    let undocumented: BTreeSet<String> = PIPELINE_KINDS
        .iter()
        .map(|kind| kind.as_str().to_string())
        .filter(|kind| !listed.contains(kind))
        .collect();
    assert_eq!(
        undocumented, proposed,
        "the kinds this crate emits that docs/contract.md does not list are not entry 40's"
    );

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

/// The envelope's `phase` is the **sibling's** vocabulary, spelled the same way
/// on both sides of the relay, and every one of it.
///
/// The one field on the merged envelope that neither this crate nor
/// `docs/contract.md` owns: `onevcs` classifies what an event of its own belongs
/// to and this crate carries the classification rather than making one. So the
/// gate is exhaustive in both directions and over that library's own list — a
/// phase it adds fails here rather than arriving as a value every reader
/// silently declines, and one this copy grew alone fails here too.
///
/// `docs/contract-divergences.md` entry 40 is where the field is proposed to the
/// planner who owns the contract; this is what holds it to the sibling's while
/// it is a proposal.
#[test]
fn the_envelopes_phase_is_the_siblings_own_vocabulary_and_all_of_it() {
    // Spelled by a match rather than by a list, so a variant added to this
    // copy has to be spelled here as well as there.
    let spelled = |phase: Phase| match phase {
        Phase::Development => "development",
        Phase::Integrate => "integrate",
        Phase::Review => "review",
        Phase::Release => "release",
    };
    let theirs = onevcs::Phase::every();
    assert_eq!(
        theirs.len(),
        4,
        "the sibling's phase vocabulary changed size"
    );
    for phase in theirs {
        let wire = serde_json::to_value(phase).expect("the sibling's phase serializes");
        let mine: Phase = serde_json::from_value(wire.clone())
            .unwrap_or_else(|e| panic!("this copy does not read the sibling's {wire}: {e}"));
        assert_eq!(json!(spelled(mine)), wire, "the two copies spell it apart");
        // And back, so neither side carries a phase the other cannot read.
        let round: onevcs::Phase = serde_json::from_value(json!(spelled(mine)))
            .expect("the sibling reads what this copy writes");
        assert_eq!(round, phase);
    }

    // On the wire it is optional and omitted when absent: a store written before
    // there was a phase round-trips as its writer wrote it, and one written with
    // a phase keeps it.
    let without = json!({
        "v": ENVELOPE_VERSION,
        "ts": "2026-08-07T12:00:01.500Z",
        "stream": "onevcs-1a2b",
        "seq": 3,
        "source": "vcs",
        "kind": "session-opened",
        "labels": {},
        "payload": {},
        "artifacts": []
    });
    let envelope: Envelope = serde_json::from_value(without.clone()).expect("parses");
    assert_eq!(envelope.phase, None);
    assert_eq!(
        serde_json::to_value(&envelope).expect("serializes"),
        without
    );

    let mut with = without.clone();
    with["phase"] = json!("release");
    let envelope: Envelope = serde_json::from_value(with.clone()).expect("parses");
    assert_eq!(envelope.phase, Some(Phase::Release));
    assert_eq!(serde_json::to_value(&envelope).expect("serializes"), with);
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

/// A scratch root for one test, removed when it ends.
///
/// The retention path writes to a real filesystem, so these journeys need
/// somewhere to hold a consumer's own runs root beside a producing process's own
/// scratch — which is the whole distinction the path is about.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "onepipeline-contract-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch root");
    dir
}

/// The `member-settled` a producing process wrote to its own stdout, as the wire
/// line it arrives as.
///
/// Built out of the published constants rather than a second spelling of them:
/// what a consumer has to be able to compose is exactly this envelope, and the
/// crate's own words are what it composes it from.
fn member_settled(stream: &str, seq: u64, named: &Path) -> Envelope {
    let mut wire = json!({
        "v": ENVELOPE_VERSION,
        "ts": "2026-08-18T09:00:00.000Z",
        "stream": stream,
        "seq": seq,
        "source": "agentgraph",
        "kind": MEMBER_SETTLED,
        "labels": {"node": "build", "member": "worker"},
        "payload": {},
        // The artifact names the stream and not the seq, which is why the copy
        // is derivable from this envelope and never from the id alone.
        "artifacts": [{"id": format!("report-{stream}"), "kind": "report", "bytes": 0}]
    });
    wire["payload"][REPORT_PATH] = json!(named.display().to_string());
    serde_json::from_value(wire).expect("the settlement parses")
}

/// The whole published path, driven the way a consumer downstream of this crate
/// drives it: mint the run's paths, ingest the settlement a producing process
/// wrote, and read the report back from the path `report_for` derives.
///
/// The stream carries characters the sanitiser rewrites, so what is proved is
/// the *derived* name rather than a passthrough one — and the consumer obtains
/// it by calling, never by restating the rule behind it.
#[test]
fn a_consumer_retains_a_report_and_reads_it_back_from_the_path_it_derives() {
    let root = scratch("retention-journey");
    // The producing library's own scratch: a directory this crate neither
    // chooses nor can attest.
    let produced = root.join("producer");
    std::fs::create_dir_all(&produced).expect("a producer scratch");
    let named = produced.join(ACCEPTED_REPORT_FILE);
    let body = r#"{"results":[{"harness":"claude-code","text":"Ran the gate."}]}"#;
    std::fs::write(&named, body).expect("the report the producer wrote");

    let runs = root.join("runs");
    let paths = RunPaths::under(&runs, "demo");
    assert_eq!(paths.run, "demo");
    assert_eq!(paths.dir, runs.join("demo"));
    assert!(paths.reports_dir().starts_with(&paths.dir));

    let stream = "node-scope/1786925518098 3163646";
    retain(&paths, &member_settled(stream, 7, &named));

    let kept = paths.report_for(stream, 7);
    assert_eq!(
        std::fs::read_to_string(&kept).expect("this run's own copy of the report"),
        body,
        "the copy at the derived path is not the report the producer wrote"
    );

    // One segment under the run's own storage, whatever the producer's stream id
    // said — the sanitiser is reached only through this call.
    let leaf = kept
        .strip_prefix(paths.reports_dir())
        .expect("the copy is under the run's own reports directory");
    assert_eq!(
        leaf.components().count(),
        1,
        "the derived name is not a single segment: {leaf:?}"
    );

    // Copied, not referenced: the producer's own file going away costs the run
    // nothing, which is what makes the derived path the one a reader opens.
    std::fs::remove_file(&named).expect("the producer's own copy goes away");
    assert!(std::fs::read_to_string(&kept).is_ok());

    std::fs::remove_dir_all(&root).ok();
}

/// A producer's stream id is a string; the path it derives is a name.
///
/// The seq is part of that name because the settlement is what identifies the
/// report — the artifact id names the stream alone — so two settlements of one
/// stream never resolve to one file.
#[test]
fn a_derived_report_path_is_one_segment_under_the_runs_own_storage() {
    let paths = RunPaths::under(Path::new("/nowhere"), "demo");
    for stream in [
        "oneagentgraph-1",
        "../../elsewhere",
        "..",
        "",
        "node scope:1786925518098/3163646",
    ] {
        let kept = paths.report_for(stream, 3);
        let leaf = kept
            .strip_prefix(paths.reports_dir())
            .unwrap_or_else(|_| panic!("'{stream}' derived a path outside the run: {kept:?}"));
        assert_eq!(
            leaf.components().count(),
            1,
            "'{stream}' derived more than one segment: {leaf:?}"
        );
        assert_ne!(
            kept,
            paths.report_for(stream, 4),
            "'{stream}' resolves two settlements to one file"
        );
    }
}

/// Every refusal the writer makes, met the way a consumer meets it: nothing is
/// at the path `report_for` derives, so no reader of this run's store finds one.
#[test]
fn the_published_writer_refuses_anything_that_is_not_the_producers_own_plain_file() {
    let root = scratch("retention-refusals");
    let paths = RunPaths::under(&root.join("runs"), "demo");
    let secret = root.join("secret.json");
    std::fs::write(&secret, r#"{"transcript":{"messages":[]}}"#)
        .expect("a file the producing library never wrote");

    let planted = root.join("planted");
    std::fs::create_dir_all(&planted).expect("somewhere to plant a link");
    let link = planted.join(ACCEPTED_REPORT_FILE);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, &link).expect("a symlink");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&secret, &link).expect("a symlink");

    let directory = root.join("as-a-directory").join(ACCEPTED_REPORT_FILE);
    std::fs::create_dir_all(&directory).expect("a directory wearing the accepted name");

    for (seq, (case, named)) in [
        ("a base name the producing library never writes", secret),
        ("a symlink wearing the accepted name", link),
        ("a directory wearing the accepted name", directory),
        (
            "nothing at all",
            root.join("gone").join(ACCEPTED_REPORT_FILE),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let seq = seq as u64;
        retain(&paths, &member_settled("oneagentgraph-1", seq, &named));
        let kept = paths.report_for("oneagentgraph-1", seq);
        assert!(
            std::fs::symlink_metadata(&kept).is_err(),
            "{case}: '{}' reached the run's own storage at {kept:?}",
            named.display()
        );
    }
    std::fs::remove_dir_all(&root).ok();
}

/// The bound is the writer's, and it is published so a caller knows what will be
/// refused rather than discovering it.
#[test]
fn the_published_writer_refuses_a_report_past_the_bound_it_publishes() {
    let root = scratch("retention-oversize");
    let paths = RunPaths::under(&root.join("runs"), "demo");
    let produced = root.join("producer");
    std::fs::create_dir_all(&produced).expect("a producer scratch");
    let named = produced.join(ACCEPTED_REPORT_FILE);
    // Claimed rather than written: the bound is on the size the filesystem
    // reports, and a real 32MiB fixture would be a slow way to say so.
    let file = std::fs::File::create(&named).expect("the stored report");
    file.set_len(MAX_REPORT_BYTES + 1)
        .expect("a report past the bound");
    drop(file);

    retain(&paths, &member_settled("oneagentgraph-1", 4, &named));
    let kept = paths.report_for("oneagentgraph-1", 4);
    assert!(
        std::fs::symlink_metadata(&kept).is_err(),
        "a report past {MAX_REPORT_BYTES} bytes reached the run's own storage"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// The writer ingests a settlement of the producing library's, and nothing else
/// of the same shape.
#[test]
fn the_published_writer_ingests_only_an_oneagentgraph_settlement() {
    let root = scratch("retention-kinds");
    let paths = RunPaths::under(&root.join("runs"), "demo");
    let produced = root.join("producer");
    std::fs::create_dir_all(&produced).expect("a producer scratch");
    let named = produced.join(ACCEPTED_REPORT_FILE);
    std::fs::write(&named, "{}").expect("the report the producer wrote");

    // This library's own event, of the very same shape.
    let mut ours = member_settled("onepipeline-1", 1, &named);
    ours.source = Source::Pipeline;
    retain(&paths, &ours);

    // And a kind of the producing library's that settles nothing.
    let mut other = member_settled("oneagentgraph-1", 2, &named);
    other.kind = EventKind("turn-completed".into());
    retain(&paths, &other);

    assert!(
        !paths.reports_dir().exists(),
        "an envelope that is not an agentgraph {MEMBER_SETTLED} was ingested as a report"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// The document names every item the retention path is promised through, and the
/// release a consumer links it as.
#[test]
fn the_contract_names_the_retention_path_and_the_release_it_ships_in() {
    // The package's own version, read from the manifest rather than from a
    // second spelling of it: the whole point of publishing it is that a host
    // pinning this engine and a reader of its run store can prove they are one
    // release.
    let manifest =
        std::fs::read_to_string(repo_root().join("Cargo.toml")).expect("the manifest ships");
    let declared = manifest
        .split_once("\n[package]")
        .expect("the manifest declares this package")
        .1
        .lines()
        .find_map(|line| line.trim().strip_prefix("version = \"")?.strip_suffix('"'))
        .expect("the package declares a version");
    assert_eq!(
        onepipeline::VERSION,
        declared,
        "`VERSION` is not this crate's own package version"
    );

    assert_contract_names(
        "published retention path",
        &[
            "views::RunPaths",
            "the run id `run` and the run's own directory `dir`",
            "RunPaths::new",
            "RunPaths::under",
            "reports_dir()",
            "report_for(STREAM, SEQ)",
            "reports/<sanitised stream>-<seq>.json",
            "report::retain(&RunPaths, &Envelope)",
            "report::MEMBER_SETTLED",
            "report::REPORT_PATH",
            "report::ACCEPTED_REPORT_FILE",
            "report::MAX_REPORT_BYTES",
            "onepipeline::VERSION",
        ],
    );
    // The precondition rides with the function: publishing it did not widen when
    // it is safe to call, and a document that dropped this leaves a caller free
    // to hand it a path it has no authority for.
    assert!(
        CONTRACT.contains("the caller holds the producing process's authority for the path"),
        "docs/contract.md no longer states `retain`'s precondition"
    );
    assert!(
        CONTRACT.contains("the sanitiser is not public"),
        "docs/contract.md no longer says the sanitiser is unreachable"
    );
}

#[test]
fn the_driver_contracts_invocation_parses_exactly_as_written() {
    let documented = "onepipeline start plan.json [--attach|--detach] \
                      [--dag-graph off|REF] [--pr-author-graph REF] \
                      [--heartbeat-interval 1800] \
                      [--set PATH=VALUE]... [--node-set PATH=VALUE]... \
                      [--acknowledge-concurrent]";
    assert!(CONTRACT.contains(documented), "the driver invocation moved");

    let cli = Cli::try_parse_from([
        "onepipeline",
        "start",
        "plan.json",
        "--detach",
        "--dag-graph",
        "graphs/dag-scope.yaml",
        "--pr-author-graph",
        "graphs/pr-author.yaml",
        "--heartbeat-interval",
        "1800",
        "--set",
        "members.monitor.agent.model=dag one",
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
    assert_eq!(args.dag_graph, "graphs/dag-scope.yaml");
    assert_eq!(
        args.pr_author_graph.as_deref(),
        Some("graphs/pr-author.yaml")
    );
    assert_eq!(args.heartbeat_interval, 1_800);
    assert!(args.acknowledge_concurrent);
    assert_eq!(
        args.dag_sets,
        [
            "members.monitor.agent.model=dag one",
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

    // The document's numbers and its default are this crate's.
    assert_eq!(DEFAULT_HEARTBEAT_INTERVAL_SECONDS, 1_800);
    assert!(
        CONTRACT.contains("`--dag-graph` defaults to `off`"),
        "the contract no longer states the shipped default"
    );
    let defaulted = Cli::try_parse_from(["onepipeline", "start", "plan.json"]).expect("parses");
    let Command::Start(args) = defaulted.command else {
        panic!("expected `start`");
    };
    assert_eq!(
        args.dag_graph, DAG_GRAPH_OFF,
        "a plan runs with no agent graph unless one is asked for"
    );
    assert_eq!(
        args.pr_author_graph, None,
        "a change request is drafted by no graph unless one is asked for"
    );
    assert_eq!(args.heartbeat_interval, DEFAULT_HEARTBEAT_INTERVAL_SECONDS);
}

/// The verbs an agent used to drive the engine with, which no longer exist.
///
/// Refused rather than merely absent: a caller that still spells one is told, by
/// clap, that there is no such command — and this is what stops them being
/// reintroduced by habit.
#[test]
fn the_round_verbs_are_gone_from_the_command_surface() {
    for retired in [
        vec!["round", "run", "run-1"],
        vec!["round", "next", "run-1"],
    ] {
        Cli::try_parse_from(std::iter::once("onepipeline").chain(retired.iter().copied()))
            .expect_err("a round verb still parses");
    }
    Cli::try_parse_from(["onepipeline", "start", "p.json", "--round-budget", "10"])
        .expect_err("--round-budget still parses");
    assert!(
        !CONTRACT.contains("round run") && !CONTRACT.contains("--round-budget"),
        "the contract still names a retired verb or flag"
    );
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
            "`onepipeline next RUN [--filter NAME|SPEC] [--all]`",
            "reply RUN [FILE]",
            "surface RUN --kind check-in --message TEXT",
            "attest RUN REF",
            "stop RUN",
        ],
    );
    assert_contract_names(
        "driver verb",
        &["onepipeline channel serve RUN", "onepipeline adopt RUN"],
    );

    // The views, as the contract lists them.
    let tokens = backticked();
    for view in ["runs", "status", "host", "results", "goals"] {
        assert!(
            tokens.contains(view),
            "the contract no longer lists the `{view}` view"
        );
    }
    assert!(tokens.contains("monitor RUN [--filter NAME|SPEC] [--all]"));
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
fn the_dag_scope_graph_is_a_monitor_plus_a_resettable_check_in() {
    assert!(CONTRACT.contains("shipped: `monitor` member + resettable-cron `check-in` member"));

    let text = std::fs::read_to_string(repo_root().join("graphs/dag-scope.yaml"))
        .expect("the dag-scope graph ships");
    // It parses as oneagentgraph's own schema — the library that launches it.
    let graph: GraphConfig = serde_norway::from_str(&text).expect("it is a valid graph config");
    assert_eq!(graph.name, "dag-scope");

    let monitor = graph.members.get("monitor").expect("a monitor member");
    let Member::Onejudge(monitor) = monitor else {
        panic!("the monitor is a two-party member");
    };
    match &monitor.judge {
        JudgeSide::Command(judge) => assert_eq!(
            judge.command[..3],
            ["onepipeline", "channel", "serve"],
            "the monitor's judge side is this crate's channel server"
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

/// The shipped persona files, and the role each one carries.
///
/// The monitor's file keeps the `orchestrator.yaml` name it shipped under: the
/// orchestrator persona was **rewritten** into the observer, not replaced by a
/// file beside it, so the path a consumer already names keeps resolving. The
/// role is what changed, and the role is what the contract, the graph member,
/// and the channel's author allowlist all spell `monitor`.
const SHIPPED_PERSONAS: [(&str, &str); 3] = [
    ("orchestrator", "monitor"),
    ("check-in", "check-in"),
    ("pr-author", "pr-author"),
];

#[test]
fn every_persona_the_contract_ships_is_present_and_has_both_sides() {
    assert!(CONTRACT.contains(
        "personas `monitor` (at `personas/orchestrator.yaml`, the shipped file the \
         orchestrator persona was rewritten into), `check-in`, `pr-author`"
    ));
    for (file, role) in SHIPPED_PERSONAS {
        let path = repo_root().join("personas").join(format!("{file}.yaml"));
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{file} persona ships: {e}"));
        // Through the sibling's own reader, which is the trust boundary a member
        // resolving this file crosses: a persona in a shape it refuses ships
        // broken, and asserting on the YAML alone would not notice.
        let persona = Persona::parse(&text, &format!("personas/{file}.yaml"))
            .unwrap_or_else(|e| panic!("{file} is not a persona oneagentgraph loads: {e}"));
        assert_eq!(
            persona.label(),
            Some(role),
            "personas/{file}.yaml carries the {role} role"
        );
        // What it layers onto a base, read off the merge rather than off the
        // file: that is the config the member is actually launched with.
        let effective = merge("{}\n", "an empty base config", &persona)
            .unwrap_or_else(|e| panic!("{file} does not layer onto a base config: {e}"));
        assert!(
            effective.pointer("/system_prompt").is_some(),
            "{file} states the agent's role"
        );
        assert!(
            effective.pointer("/user/persona").is_some(),
            "{file} states the supervisor's review bar"
        );
    }
}

/// The monitor persona's own account of its allowlist is the allowlist.
///
/// The persona is the only thing the member ever reads, so a persona naming an
/// op the channel refuses sends it to be refused every run, and one silent about
/// an op the channel allows leaves that op unreachable however carefully it was
/// wired. Both directions, off the same `allows` the channel enforces with.
#[test]
fn the_monitor_persona_names_exactly_the_ops_the_channel_lets_it_issue() {
    let text = std::fs::read_to_string(repo_root().join("personas/orchestrator.yaml"))
        .expect("the monitor persona ships");
    let prose = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let node = Node {
        id: "fresh".into(),
        persona: Some("engineer".into()),
        task: Some("## What\ndo it".into()),
        ..Node::default()
    };
    // Every op this build has, contract's and divergent alike, each in the shape
    // the wire carries it.
    let every: Vec<Edit> = vec![
        Edit::Add { node: node.clone() },
        Edit::Drop {
            id: "x".into(),
            dependents: Dependents::Detach,
        },
        Edit::Reparent {
            id: "x".into(),
            deps: Vec::new(),
        },
        Edit::Retry {
            id: "x".into(),
            node,
        },
        Edit::Cancel { id: "x".into() },
        Edit::Requeue {
            id: "x".into(),
            amend: None,
        },
        Edit::Attest {
            reference: "x".into(),
        },
        Edit::Complete {
            reason: "done".into(),
        },
        Edit::Context {
            id: "x".into(),
            note: "look here".into(),
            deliver: onepipeline::channel::Deliver::Auto,
        },
        Edit::Finding {
            message: "it drifted".into(),
            blocking: false,
            id: None,
        },
        Edit::Amend {
            id: "x".into(),
            text: "the ruling".into(),
        },
    ];
    assert_eq!(
        every.len(),
        OPS.len() + 2,
        "an op is missing from this table; `op_of` above is what says so"
    );

    // The persona states both lists in one sentence pair, which is what makes
    // this readable to the member: what it may issue, then what is refused.
    let window = prose
        .split_once("what that author may issue:")
        .and_then(|(_, tail)| tail.split_once("are refused for you"))
        .map(|(window, _)| window.to_string())
        .expect("the monitor persona still states its allowlist and its refusals");
    let (may_issue, refused) = window
        .split_once(". ")
        .expect("the persona's allowlist and its refusals are two sentences");

    for command in &every {
        let op = op_of(command);
        let spelt = format!("`{op}`");
        let allowed = allows(Author::Monitor, command).is_ok();
        let (says_it_may, says_it_may_not) = (may_issue.contains(&spelt), refused.contains(&spelt));
        assert_eq!(
            (says_it_may, says_it_may_not),
            (allowed, !allowed),
            "the channel {} the monitor `{op}` and its persona says otherwise: \
             may issue '{may_issue}', refused '{refused}'",
            if allowed { "allows" } else { "refuses" }
        );
    }
}

#[test]
fn the_pr_author_never_blocks_publication() {
    assert!(
        CONTRACT.contains("Drafting is never on the publication path."),
        "the contract no longer keeps the drafting dispatch off the publication path"
    );
    assert!(
        CONTRACT.contains(
            "the change request opens with no body and the node settles on its \
                           publication as before"
        ),
        "the contract no longer says what a drafting dispatch that ended badly costs"
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
    ("23.", "drive GRAPH"),
    ("24.", "NodeControls"),
    ("25.", "drive-run RUN"),
    ("26.", "nothing else able to move"),
    ("27.", "ending that parked driver politely"),
    ("28.", "`attempt`, `attempts`"),
    ("29.", "inherits both"),
    ("30.", "--launch-config FILE"),
    ("31.", "shaped event view beside the surface"),
    ("32.", "any run of characters including none"),
    ("34.", "body-not-drafted"),
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
