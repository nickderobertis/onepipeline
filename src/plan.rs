//! The plan schema.
//!
//! A plan is one JSON document: a task DAG whose node shapes are
//! `ai-orchestrator`'s tracked-graph schema v7, unchanged, plus the three things
//! `docs/contract.md` adds — `repo` resolved through `onevcs`, an optional
//! per-node `executor`, and an optional per-node `agent_graph` overriding the
//! default node-scope graph config.
//!
//! This module is the schema and how a plan file is *read*. Whether the graph it
//! describes is legal — its shape rules, its references, and its acyclicity —
//! belongs to the graph module, which validates every plan this loader returns.

// llmlint: ignore-file[invalid_states_unrepresentable] a [`Node`] is one flat mapping
// with optional fields rather than an enum over direct/lifecycle/human, because
// `docs/contract.md` fixes the node shapes as schema v7's and that is the shape a v7
// plan file is written in. Splitting them into variants here would reject plans the
// contract says this schema accepts, and choosing the discriminants would invent an
// interface the contract does not name. The `kind`/`repo`/`steps` combination rules are
// enforced instead by `graph::validate_node`, at the trust boundary every plan crosses.

use std::path::Path;

use oneagentgraph::config::ConfigRef;
use onevcs::registry::{RepoType, Workflow};
use onevcs::MergePolicy;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The plan schema version this crate reads and writes.
pub const PLAN_SCHEMA_VERSION: u32 = 1;

/// The heading a carried planner note is rendered under.
pub const PLANNER_CONTEXT_HEADING: &str = "## Planner context";

/// One plan: the task DAG a run executes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    /// Schema version; [`PLAN_SCHEMA_VERSION`] for anything this crate writes.
    pub schema_version: u32,
    /// What the run is for, in the planner's own words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<Goal>,
    /// The plan's name, used to label the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// How many nodes may be dispatched at once.
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,
    /// The nodes, in no particular order; `deps` is what orders them.
    pub tasks: Vec<Node>,
}

/// The default when a plan states no `concurrency`.
fn default_concurrency() -> u32 {
    4
}

impl Plan {
    /// Read a plan file.
    ///
    /// Each format is read with **its own** escape semantics: a `.json` file is
    /// parsed as JSON, so the surrogate pair a JSON writer emits for one emoji
    /// reaches the dispatched agent as the character it encodes. Reading it as
    /// YAML instead yields two unpaired halves, which no UTF-8 encoder accepts,
    /// and the node fails on its own task prose. A JSON document that JSON
    /// itself cannot parse falls back to the YAML reading, so nothing that
    /// loaded before stops loading.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::Ledger {
            path: path.to_path_buf(),
            source: e,
        })?;
        let is_json = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
        let named = |e: String| Error::Invalid(format!("{}: {e}", path.display()));

        if is_json {
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(serde_json::Value::Object(_)) => {
                    return serde_json::from_str(&text).map_err(|e| named(e.to_string()));
                }
                Ok(other) => {
                    let kind = match other {
                        serde_json::Value::Array(_) => "list",
                        serde_json::Value::String(_) => "string",
                        serde_json::Value::Null => "null",
                        _ => "scalar",
                    };
                    return Err(named(format!("must be a JSON mapping, got {kind}")));
                }
                // Not parseable as JSON at all: fall through to the YAML
                // reading rather than refusing a file that used to load.
                Err(_) => {}
            }
        }
        serde_norway::from_str(&text).map_err(|e| named(e.to_string()))
    }
}

impl Node {
    /// The task prose this node's dispatch receives.
    ///
    /// A carried planner note is rendered as a trailing `## Planner context`
    /// section stating that it reports observed state and adds no acceptance
    /// criteria — so a worker cannot read one as a new bar to clear.
    pub fn rendered_task(&self) -> String {
        render_task(
            self.task.as_deref().unwrap_or_default(),
            self.context.as_deref(),
        )
    }
}

impl Step {
    /// The task prose this step's dispatch receives.
    ///
    /// The note is about the node the steps share, so a workstream renders it
    /// into every agent step and leaves human steps as written.
    pub fn rendered_task(&self, node_context: Option<&str>) -> String {
        render_task(self.task.as_deref().unwrap_or_default(), node_context)
    }
}

fn render_task(task: &str, context: Option<&str>) -> String {
    match context.map(str::trim).filter(|note| !note.is_empty()) {
        None => task.to_string(),
        Some(note) => format!(
            "{}\n\n{PLANNER_CONTEXT_HEADING}\n\
             This reports observed state and adds no acceptance criteria.\n\n{note}\n",
            task.trim_end()
        ),
    }
}

/// What the run is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Goal {
    /// The goal in prose.
    pub text: String,
}

/// Whether a node is work the harness runs or an action only a person can take.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    /// Work a dispatch performs.
    #[default]
    Agent,
    /// An action only an external person or outside system can perform. The
    /// harness never infers or executes it.
    Human,
}

impl NodeKind {
    /// Whether this is the default kind, so serialization can omit it.
    fn is_agent(&self) -> bool {
        matches!(self, Self::Agent)
    }
}

/// One node of the DAG.
///
/// The same mapping carries a direct agent node, a lifecycle node (one that
/// names a `repo` and may run several `steps` on one branch), and a
/// `kind: human` action. Which fields a given node may set is the loader's rule,
/// not this schema's.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    /// Unique within the plan.
    pub id: String,
    /// Defaults to [`NodeKind::Agent`], and is omitted when it is that, so a
    /// node round-trips as the plan file wrote it.
    #[serde(default, skip_serializing_if = "NodeKind::is_agent")]
    pub kind: NodeKind,
    /// The task prose: `## What`, `## Why`, `## Acceptance criteria`, then
    /// `## Additional info` only when it is nonempty. A `kind: human` node
    /// carries the action prose alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The persona the dispatch runs under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    /// Prerequisite node ids, or cross-DAG `run:<id>#<node>` references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    /// The judge-only completion bar. Always requires that every acceptance
    /// criterion in `task` is met, and may add broader quality measures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_when: Option<String>,
    /// The dispatch's turn budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    /// The node is expected to leave the repository unchanged, so an empty diff
    /// is a completed node rather than a failed one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub expects_no_diff: bool,
    /// One planner note carried to the node's next dispatch, and only that one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Held out of every later round until a `requeue`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub parked: bool,
    /// Which executor dispatches this node. Omitted, the executor rules decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    /// A node-scope agent-graph config overriding the default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_graph: Option<ConfigRef>,
    /// The repository this node's work lands in, resolved through `onevcs`. Its
    /// presence is what makes the node a lifecycle node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Overrides the identity's stored type for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_type: Option<RepoType>,
    /// Overrides the identity's stored workflow for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<Workflow>,
    /// How the finished branch is published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_policy: Option<MergePolicy>,
    /// The branch this node's work is cut from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    /// Pin the work to a named branch instead of generating one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The change request's title, when the planner sets it rather than leaving
    /// it to the `pr-author` dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The registered checkout the per-run clone is cut from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_checkout: Option<String>,
    /// Treat the remote host's own checks as the merge-path verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_via_ci: Option<bool>,
    /// Several agent and human steps run in sequence on one branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<Step>>,
    /// Continue preserved work rather than cutting a fresh branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<Resume>,
}

/// One step of a lifecycle node, sharing that node's branch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Unique within the node.
    pub id: String,
    /// Defaults to [`NodeKind::Agent`], and is omitted when it is that, so a
    /// node round-trips as the plan file wrote it.
    #[serde(default, skip_serializing_if = "NodeKind::is_agent")]
    pub kind: NodeKind,
    /// The step's task prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The persona this step runs under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    /// Prerequisite step ids within the same node.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    /// The judge-only completion bar for this step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_when: Option<String>,
    /// The step's turn budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    /// The step is expected to leave the repository unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub expects_no_diff: bool,
    /// Which executor dispatches this step. Omitted, the node's choice applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    /// A node-scope agent-graph config overriding this step's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_graph: Option<ConfigRef>,
}

/// Where a node picks preserved work back up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resume {
    /// The preserved branch to continue on.
    pub branch: String,
    /// The commit on it the continuation starts from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("onepipeline-plan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch root");
        dir
    }

    #[test]
    fn a_json_plan_is_read_with_json_escape_semantics() {
        let root = scratch("json");
        let path = root.join("emoji.plan.json");
        // What `json.dump` writes for one emoji: a surrogate pair. Read as
        // YAML it is two unpaired halves and the node fails on its own prose.
        std::fs::write(
            &path,
            r#"{"schema_version":1,"tasks":[{"id":"a","persona":"engineer","task":"😀 ship it"}]}"#,
        )
        .expect("written");
        let plan = Plan::load(&path).expect("a JSON plan loads");
        assert!(
            plan.tasks[0]
                .task
                .as_deref()
                .expect("task")
                .starts_with('\u{1f600}'),
            "the surrogate pair did not survive as one character"
        );
        assert_eq!(
            plan.concurrency, 4,
            "the default concurrency was not applied"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_json_document_that_is_not_a_mapping_is_refused_by_name() {
        let root = scratch("notmapping");
        let path = root.join("list.plan.json");
        std::fs::write(&path, "[1, 2, 3]").expect("written");
        let message = Plan::load(&path).unwrap_err().to_string();
        assert!(
            message.contains("must be a JSON mapping, got list"),
            "{message}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_json_named_file_that_json_cannot_parse_falls_back_to_yaml() {
        let root = scratch("yamlfallback");
        let path = root.join("actually.plan.json");
        std::fs::write(
            &path,
            "schema_version: 1\ntasks:\n  - id: a\n    persona: engineer\n    task: do it\n",
        )
        .expect("written");
        let plan = Plan::load(&path).expect("the YAML reading is the fallback");
        assert_eq!(plan.tasks[0].id, "a");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_plan_with_an_unknown_field_is_refused_at_its_trust_boundary() {
        let root = scratch("unknown");
        let path = root.join("typo.plan.json");
        std::fs::write(
            &path,
            r#"{"schema_version":1,"concurency":2,"tasks":[{"id":"a","persona":"e","task":"t"}]}"#,
        )
        .expect("written");
        let message = Plan::load(&path).unwrap_err().to_string();
        assert!(message.contains("concurency"), "{message}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_plan_file_names_the_path_it_could_not_read() {
        let message = Plan::load(std::path::Path::new("no/such/plan.json"))
            .unwrap_err()
            .to_string();
        assert!(message.contains("no/such/plan.json"), "{message}");
    }

    #[test]
    fn both_shipped_examples_load_and_keep_what_they_declare() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
        let single = Plan::load(&root.join("single-node.plan.json")).expect("single-node loads");
        assert_eq!(single.tasks.len(), 1);
        assert!(single.goal.is_some());

        let mixed = Plan::load(&root.join("mixed-graph.plan.json")).expect("mixed-graph loads");
        assert_eq!(mixed.concurrency, 3);
        let docs = mixed
            .tasks
            .iter()
            .find(|n| n.id == "docs")
            .expect("the docs node");
        assert_eq!(
            docs.agent_graph.as_ref().map(|r| r.0.as_str()),
            Some("./graphs/node-scope.yaml"),
            "the example does not reference the shipped node-scope config"
        );
        let service = mixed
            .tasks
            .iter()
            .find(|n| n.id == "service")
            .expect("the service node");
        assert_eq!(service.executor.as_deref(), Some("local"));
        assert_eq!(service.steps.as_ref().map(Vec::len), Some(2));
    }

    #[test]
    fn a_planner_note_renders_as_its_own_section_and_disclaims_itself() {
        let node = Node {
            id: "build".into(),
            persona: Some("engineer".into()),
            task: Some("## What\nship it".into()),
            context: Some("the fixture moved to tests/data".into()),
            ..Node::default()
        };
        let rendered = node.rendered_task();
        assert!(rendered.starts_with("## What\nship it"), "{rendered}");
        assert!(rendered.contains(PLANNER_CONTEXT_HEADING), "{rendered}");
        assert!(
            rendered.contains("adds no acceptance criteria"),
            "{rendered}"
        );
        assert!(
            rendered.contains("the fixture moved to tests/data"),
            "{rendered}"
        );
    }

    #[test]
    fn a_node_with_no_note_renders_its_task_unchanged() {
        let node = Node {
            id: "build".into(),
            task: Some("## What\nship it".into()),
            ..Node::default()
        };
        assert_eq!(node.rendered_task(), "## What\nship it");

        let blank = Node {
            context: Some("   ".into()),
            ..node
        };
        assert_eq!(blank.rendered_task(), "## What\nship it");
    }

    #[test]
    fn a_workstreams_note_reaches_every_agent_step() {
        let step = Step {
            id: "implement".into(),
            persona: Some("engineer".into()),
            task: Some("## What\nimplement".into()),
            ..Step::default()
        };
        let rendered = step.rendered_task(Some("the API moved"));
        assert!(rendered.contains(PLANNER_CONTEXT_HEADING), "{rendered}");
        assert_eq!(step.rendered_task(None), "## What\nimplement");
    }

    #[test]
    fn a_plan_round_trips_without_growing_the_fields_it_omitted() {
        let source = r#"{"schema_version":1,"tasks":[{"id":"a","persona":"e","task":"t"}]}"#;
        let plan: Plan = serde_json::from_str(source).expect("it parses");
        let written = serde_json::to_string(&plan).expect("it serialises");
        assert!(
            !written.contains("\"kind\""),
            "{written} grew a default kind"
        );
        assert!(
            !written.contains("\"deps\""),
            "{written} grew an empty deps"
        );
        assert!(
            !written.contains("\"parked\""),
            "{written} grew a false parked"
        );
    }
}
