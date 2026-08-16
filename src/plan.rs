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
///
/// **3** since a lifecycle node states the change request it publishes: a
/// [`title`](Node::title) is required on one, and a [`body`](Node::body) may be
/// carried beside it. Unlike the version before it this one is **additive**, so
/// [`PLAN_SCHEMA_VERSION_LEGACY`] still loads and still runs: a node written
/// there states no title, and its publication takes the subject `onevcs` derives
/// from the branch's own conventional commits.
pub const PLAN_SCHEMA_VERSION: u32 = 3;

/// The version before it, which this build still reads.
///
/// A plan written at it is a legal document rather than one being tolerated: the
/// two rules version 3 adds are both keyed to the version the document itself
/// declares, so a version-2 plan means exactly what it meant.
pub const PLAN_SCHEMA_VERSION_LEGACY: u32 = 2;

/// The version this schema retired, which a plan on disk may still declare.
pub const PLAN_SCHEMA_VERSION_RETIRED: u32 = 1;

/// What a plan still declaring [`PLAN_SCHEMA_VERSION_RETIRED`] is told.
///
/// A bare "schema_version 1 is not read" would leave a planner to guess what
/// changed. What changed is one field removed and one field made to work, so the
/// refusal says both and where the removed one's content goes.
pub(crate) const RETIRED_VERSION_MIGRATION: &str =
    "version 2 retired the judge-only `done_when` and forwards a node's `max_turns` to \
     its dispatch, which version 1 never did. Move any `done_when` into that node's \
     `## Acceptance criteria` — or, for a bar broader than one node, into the onejudge \
     base config the node-scope graph's worker points at — and set `schema_version: 2`";

/// What a lifecycle node at [`PLAN_SCHEMA_VERSION`] stating no `title` is told.
///
/// The node and the field, because both are what its author has to act on: a
/// plan may carry many lifecycle nodes and only one of them be missing its
/// subject.
pub(crate) const TITLE_IS_REQUIRED: &str = "a lifecycle node states the title its change request \
     opens under, and this one names no `title`";

/// What a plan below [`PLAN_SCHEMA_VERSION`] naming `body` is told.
///
/// The same rule this schema already applies to a field it never had — refused
/// by the field's own name — rather than a value silently dropped: a planner who
/// wrote a change request body and had it ignored would find that out from the
/// published change request.
pub(crate) fn body_is_newer(declared: u32) -> String {
    format!(
        "`body` is a schema {PLAN_SCHEMA_VERSION} field and this plan declares schema_version \
         {declared} — set `schema_version: {PLAN_SCHEMA_VERSION}`"
    )
}

/// The heading a carried planner note is rendered under.
pub const PLANNER_CONTEXT_HEADING: &str = "## Planner context";

/// A field the plan schema used to carry, and where its content goes now.
///
/// `deny_unknown_fields` refuses a plan still carrying it either way, with
/// `unknown field `done_when``. That tells a planner a field does not exist; it
/// does not tell them where the review bar they wrote belongs, and every plan
/// written before this schema change carries one. So the refusal names the field
/// and says where the bar goes instead.
pub(crate) const DONE_WHEN_RETIRED: &str =
    "`done_when` is no longer a plan field. A node's review bar is the \
     `## Acceptance criteria` section of its own task, which the judge is handed \
     verbatim; a bar broader than one node belongs in the onejudge base config the \
     node-scope graph's worker already points at, under `user.done_when`";

/// The name of that field, as a submitted document still spells it.
const DONE_WHEN: &str = "done_when";

/// What stands in for a [`Goal`] a plan states none of.
///
/// One spelling for both readers of it — the `goals` view a planner reads, and
/// the task the dag-scope graph is launched with — because a run with no goal
/// reads the same either way, and two spellings of it would drift apart.
/// Crate-private: it is a rendering, not part of the published surface.
pub(crate) const NO_GOAL: &str = "(no goal stated)";

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

        // A document this schema refuses is read a second time, leniently, to
        // see whether a retired field is why. Only on the failing path: the
        // reading that decides whether a plan loads stays exactly the one it was.
        let refused = |e: String| named(retired_field_refusal_in(&text).unwrap_or(e));

        if is_json {
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(serde_json::Value::Object(_)) => {
                    return serde_json::from_str(&text).map_err(|e| refused(e.to_string()));
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
        serde_norway::from_str(&text).map_err(|e| refused(e.to_string()))
    }
}

/// The refusal a submitted document still carrying the retired field earns,
/// named with where in the document it was found, or `None` if it carries none.
///
/// A whole-document walk rather than a walk of the plan's own shape: the same
/// field reaches this crate inside a plan file, inside a reply envelope's `add`,
/// and inside a `requeue`'s amendment, and one refusal for all three is one
/// answer a planner can act on. Only mapping *keys* are read, so prose that
/// discusses the field is not mistaken for a document that declares it.
pub(crate) fn retired_field_refusal(document: &serde_json::Value) -> Option<String> {
    match document {
        serde_json::Value::Object(map) => {
            if map.contains_key(DONE_WHEN) {
                let whose = map
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(|id| format!("'{id}': "))
                    .unwrap_or_default();
                return Some(format!("{whose}{DONE_WHEN_RETIRED}"));
            }
            map.values().find_map(retired_field_refusal)
        }
        serde_json::Value::Array(items) => items.iter().find_map(retired_field_refusal),
        _ => None,
    }
}

/// The same, for a document that has not been parsed yet.
///
/// Read leniently — as YAML, which also reads the JSON a plan file is usually
/// written in — because the text reaching here is one the strict schema already
/// refused, and a second refusal to parse it is simply "no retired field".
fn retired_field_refusal_in(text: &str) -> Option<String> {
    retired_field_refusal(&serde_norway::from_str::<serde_json::Value>(text).ok()?)
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
    /// Held out of every later reconcile pass until a `requeue`.
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
    /// The change request's title. Required on a lifecycle node from
    /// [`PLAN_SCHEMA_VERSION`] on; absent, the publication takes the subject
    /// `onevcs` derives from the branch's own conventional commits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The change request's body, when the planner writes it rather than
    /// leaving it to the `pr-author` graph a launch names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
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
    ///
    /// A commit **reachable on the remote**: the machine that continues a node
    /// is not the machine that made it, so a local-only revision names nothing
    /// the continuation can fetch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    /// The steps that branch already carries, which the continuation skips.
    ///
    /// Empty — or absent — re-runs the whole workstream. That is the safe
    /// direction: work is repeated, never skipped or lost, and only a step this
    /// crate watched finish is ever named here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_steps: Vec<String>,
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
            r#"{"schema_version":2,"tasks":[{"id":"a","persona":"engineer","task":"😀 ship it"}]}"#,
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
            "schema_version: 2\ntasks:\n  - id: a\n    persona: engineer\n    task: do it\n",
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
            r#"{"schema_version":2,"concurency":2,"tasks":[{"id":"a","persona":"e","task":"t"}]}"#,
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
        let source = r#"{"schema_version":2,"tasks":[{"id":"a","persona":"e","task":"t"}]}"#;
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

    /// A plan at the current version round-trips byte-for-byte through this
    /// crate, and a node that declares no turn budget writes none.
    ///
    /// The version is on the wire, so it is asserted on the wire: a plan this
    /// crate writes says `"schema_version":2`, and a reader on the previous
    /// version is meant to see that and refuse rather than to read the document
    /// as one of its own. `max_turns` is optional, so a node without one carries
    /// no key at all — a `"max_turns":null` would be this crate inventing a
    /// declaration the planner never wrote.
    #[test]
    fn a_current_version_plan_round_trips_and_omits_the_budget_it_does_not_declare() {
        let source = format!(
            r#"{{"schema_version":{PLAN_SCHEMA_VERSION},"name":"round-trip","tasks":[
                {{"id":"budgeted","persona":"e","task":"t","max_turns":45}},
                {{"id":"plain","persona":"e","task":"t"}}]}}"#
        );
        let plan: Plan = serde_json::from_str(&source).expect("it parses");
        assert_eq!(plan.schema_version, PLAN_SCHEMA_VERSION);
        assert_eq!(plan.tasks[0].max_turns, Some(45));
        assert_eq!(plan.tasks[1].max_turns, None);

        let written = serde_json::to_string(&plan).expect("it serialises");
        assert!(
            written.contains(&format!("\"schema_version\":{PLAN_SCHEMA_VERSION}")),
            "the version a reader decides by is not on the wire: {written}"
        );
        assert_eq!(
            written.matches("\"max_turns\"").count(),
            1,
            "a node that declared no turn budget was written one: {written}"
        );
        assert!(written.contains("\"max_turns\":45"), "{written}");
        assert_eq!(
            serde_json::from_str::<Plan>(&written).expect("it re-parses"),
            plan,
            "the plan did not survive a round trip through this crate"
        );
    }

    /// A legacy plan carrying the retired field is answered about the *field*.
    ///
    /// Both things are wrong with it — the version it declares and the field it
    /// carries — and only one of the two refusals says what to do with the review
    /// bar its author wrote. The document never deserialises, so the version is
    /// never reached, and that ordering is the point rather than an accident.
    #[test]
    fn a_legacy_plan_carrying_done_when_is_answered_about_the_field_not_the_version() {
        let root = scratch("legacy");
        let path = root.join("legacy.plan.json");
        std::fs::write(
            &path,
            format!(
                r#"{{"schema_version":{PLAN_SCHEMA_VERSION_RETIRED},"tasks":[
                    {{"id":"contract","persona":"e","task":"t",
                     "done_when":"the gate is green"}}]}}"#
            ),
        )
        .expect("written");
        let message = Plan::load(&path).unwrap_err().to_string();
        assert!(message.contains("'contract':"), "{message}");
        assert!(message.contains(DONE_WHEN_RETIRED), "{message}");
        assert!(
            !message.contains("schema_version"),
            "the version refusal displaced the field's: {message}"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
