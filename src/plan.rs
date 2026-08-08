//! The plan schema.
//!
//! A plan is one JSON document: a task DAG whose node shapes are
//! `ai-orchestrator`'s tracked-graph schema v7, unchanged, plus the three things
//! `docs/contract.md` adds — `repo` resolved through `onevcs`, an optional
//! per-node `executor`, and an optional per-node `agent_graph` overriding the
//! default node-scope graph config.
//!
//! These structs are the schema and nothing else: no dependency is resolved, no
//! cycle is detected, no `repo` is looked up, no cross-DAG reference is followed,
//! and no node is scheduled.

// llmlint: ignore-file[invalid_states_unrepresentable] a [`Node`] is one flat mapping
// with optional fields rather than an enum over direct/lifecycle/human, because
// `docs/contract.md` fixes the node shapes as schema v7's and that is the shape a v7
// plan file is written in. Splitting them into variants here would reject plans the
// contract says this schema accepts, and choosing the discriminants would be interface
// invention the interface-only stage forbids (see AGENTS.md). The `kind`/`repo`/`steps`
// combination rules are the loader's, and the loader is what does not exist yet.

use oneagentgraph::config::ConfigRef;
use onevcs::registry::{RepoType, Workflow};
use onevcs::MergePolicy;
use serde::{Deserialize, Serialize};

/// The plan schema version this crate reads and writes.
pub const PLAN_SCHEMA_VERSION: u32 = 1;

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
