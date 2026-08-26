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

use std::collections::BTreeMap;
use std::path::Path;

use oneagentgraph::config::ConfigRef;
use onevcs::registry::{RepoType, Workflow};
use onevcs::releases::TargetName;
use onevcs::{Adoption, MergePolicy};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The plan schema version this crate writes.
///
/// **3** since a lifecycle node states the change request it publishes: a
/// [`title`](Node::title) is required on one, and a [`body`](Node::body) may be
/// carried beside it.
pub const PLAN_SCHEMA_VERSION: u32 = 3;

/// Every version this build **reads**, newest first.
///
/// A plan is a document written at a version and read by a build, so an earlier
/// one is a legal document rather than one being tolerated: what version 3 adds
/// is keyed to the version the document itself declares, and a plan written
/// before it means exactly what it meant. Its untitled lifecycle nodes publish
/// with no subject of their own, which is `onevcs` deriving one from the
/// branch's own conventional commits, and a field a version never had is refused
/// by that field's name whatever number the document carries.
///
/// A number that is not here is one this crate has never written, and there is
/// no document to read it as.
pub const PLAN_SCHEMA_VERSIONS_READ: [u32; 3] = [PLAN_SCHEMA_VERSION, 2, 1];

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

/// The heading a node's binding amendment is rendered under.
///
/// Published beside [`PLANNER_CONTEXT_HEADING`] for the same reason, and to be
/// read *against* it: an amendment changes what the node is judged against and a
/// carried note does not. A reader of the task — or of the stream — finds each
/// block by a name this crate publishes rather than by matching prose.
pub const AMENDMENT_HEADING: &str = "## Amendment";

/// What an amendment tells its reader about its own authority.
///
/// The opposite of [`CROSS_REPO_REFERENCES_PREAMBLE`] and of the sentence a
/// carried note is rendered under, and deliberately so. A note reports observed
/// state and adds no acceptance criteria; an amendment **is** part of the bar,
/// read by the worker and by the judge that reviews it, so where it and the
/// task's own operational notes disagree it is the one that holds. The authority
/// is the section's first sentence because an instruction whose authority is
/// unstated is one a reader has to guess at.
const AMENDMENT_PRECEDENCE: &str =
    "Where this section and the operational notes below disagree, this section wins.";

/// The task section an amendment is rendered immediately above, when the task
/// has one.
///
/// A node's operational notes live here — how this host runs its gate, what it
/// must not do — and an amendment placed under them would read as one more note
/// among them rather than as the ruling that overrides them. Above them, opening
/// with [`AMENDMENT_PRECEDENCE`], is the convention that resolved this in
/// practice.
const ADDITIONAL_INFO_HEADING: &str = "## Additional info";

/// The heading the out-of-repository dependencies of a fast-adoption node are
/// rendered under.
///
/// Beside [`PLANNER_CONTEXT_HEADING`] because it is the same mechanism: one
/// trailing section the dispatch's own rendering appends, declared here so a
/// reader of the stream — or of the task — can find the block by a name this
/// crate publishes rather than by matching prose.
pub const CROSS_REPO_REFERENCES_HEADING: &str = "## Cross-repository references";

/// What the reference block tells the worker it is looking at.
///
/// Framed as observed state, exactly as a carried planner note is: it reports
/// where the work this node depends on currently is and adds no acceptance
/// criteria, so no worker can read it as a new bar to clear.
const CROSS_REPO_REFERENCES_PREAMBLE: &str =
    "This node launched under fast adoption: the work it depends on is finished but has no\n\
     release yet. Pin against the git references below rather than against a version. Do\n\
     not change a shared interface unilaterally — propose it and keep building against the\n\
     agreed surface. When these releases arrive you will be sent a note naming the\n\
     versions; move the pin then.";

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

/// The second retired field, and where what it asked for is said now.
///
/// `verify_via_ci` was accepted by this schema and read by nothing: no dispatch,
/// no publication, and no view ever consulted it, so a plan that set it got
/// exactly the run a plan that omitted it got. It is retired rather than given a
/// meaning because the thing it asked for is now **observed** rather than
/// declared — a `change-auto` publication watches the host's own required checks
/// to their conclusion, and which of them failed is what settles the node
/// `checks-failed` or `checks-unsettled`. A flag saying "use CI as the
/// verification" beside a policy that already does would be a second way to ask
/// for one behaviour, and the one nothing honoured.
pub(crate) const VERIFY_VIA_CI_RETIRED: &str =
    "`verify_via_ci` is no longer a plan field, and nothing ever read it. The \
     host's own required checks are the merge-path verification of a node whose \
     `merge_policy` is `change-auto`, which watches them to their conclusion; a \
     check that concludes red settles the node `checks-failed` and a bound that \
     elapses with one still pending settles it `checks-unsettled`";

/// The retired fields, each with the refusal a document still carrying it earns.
///
/// A table rather than a branch per field: every one of them reaches this crate
/// by the same three routes — a plan file, a reply envelope's `add`, and a
/// `requeue`'s amendment — and one walk that knows them all is what keeps the
/// three answering alike.
const RETIRED_FIELDS: &[(&str, &str)] = &[
    (DONE_WHEN, DONE_WHEN_RETIRED),
    (VERIFY_VIA_CI, VERIFY_VIA_CI_RETIRED),
];

const DONE_WHEN: &str = "done_when";

const VERIFY_VIA_CI: &str = "verify_via_ci";

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
            if let Some((_, retired)) = RETIRED_FIELDS
                .iter()
                .find(|(field, _)| map.contains_key(*field))
            {
                let whose = map
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(|id| format!("'{id}': "))
                    .unwrap_or_default();
                return Some(format!("{whose}{retired}"));
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
    /// criteria — so a worker cannot read one as a new bar to clear. A carried
    /// **amendment** is rendered under [`AMENDMENT_HEADING`] and says the
    /// opposite about itself, because it is part of the bar the judge reads.
    pub fn rendered_task(&self) -> String {
        self.rendered_task_with(&[])
    }

    /// The same, for a node launched under fast adoption against dependencies
    /// outside its own repository.
    ///
    /// The references are appended by this rendering rather than beside it, so
    /// the block a worker meets sits under the planner note in one document. An
    /// empty slice renders exactly what [`rendered_task`](Self::rendered_task)
    /// renders — which is every node that has no such dependency, and therefore
    /// every plan written before this field existed.
    pub fn rendered_task_with(&self, references: &[CrossRepoReference]) -> String {
        render_task(
            self.task.as_deref().unwrap_or_default(),
            self.amendment.as_deref(),
            self.context.as_deref(),
            references,
        )
    }
}

impl Step {
    /// The task prose this step's dispatch receives.
    ///
    /// The note is about the node the steps share, so a workstream renders it
    /// into every agent step and leaves human steps as written.
    ///
    /// It carries no amendment: an amendment belongs to a node, and a caller
    /// that has that node renders through [`rendered_task_for`](Self::rendered_task_for),
    /// which is what a dispatch uses. This pair is what a caller written before
    /// amendments existed asked for, and answers exactly as it did then.
    pub fn rendered_task(&self, node_context: Option<&str>) -> String {
        self.rendered_task_with(node_context, &[])
    }

    /// The same, carrying the node's out-of-repository dependencies.
    ///
    /// They are the *node's*, like the note beside them: every step of one
    /// workstream is building against the same dependencies on the same branch.
    pub fn rendered_task_with(
        &self,
        node_context: Option<&str>,
        references: &[CrossRepoReference],
    ) -> String {
        render_task(
            self.task.as_deref().unwrap_or_default(),
            None,
            node_context,
            references,
        )
    }

    /// The task prose this step's dispatch receives, as the node it belongs to
    /// composes it.
    ///
    /// Every part of that composition beyond the step's own prose is the
    /// **node's** — its amendment, its carried note, and the out-of-repository
    /// dependencies its branch is building against — because every step of one
    /// workstream shares one branch and one bar. So the node is what this takes,
    /// rather than three values a caller has to remember to pass.
    pub fn rendered_task_for(&self, node: &Node, references: &[CrossRepoReference]) -> String {
        render_task(
            self.task.as_deref().unwrap_or_default(),
            node.amendment.as_deref(),
            node.context.as_deref(),
            references,
        )
    }
}

fn render_task(
    task: &str,
    amendment: Option<&str>,
    context: Option<&str>,
    references: &[CrossRepoReference],
) -> String {
    let task = match amendment.map(str::trim).filter(|text| !text.is_empty()) {
        None => task.to_string(),
        Some(text) => amended(task, text),
    };
    let task = task.as_str();
    let mut rendered = match context.map(str::trim).filter(|note| !note.is_empty()) {
        None => task.to_string(),
        Some(note) => format!(
            "{}\n\n{PLANNER_CONTEXT_HEADING}\n\
             This reports observed state and adds no acceptance criteria.\n\n{note}\n",
            task.trim_end()
        ),
    };
    if references.is_empty() {
        return rendered;
    }
    rendered = format!(
        "{}\n\n{CROSS_REPO_REFERENCES_HEADING}\n\n{CROSS_REPO_REFERENCES_PREAMBLE}\n\n\
         | dependency | repository | branch | commit | release target |\n\
         | --- | --- | --- | --- | --- |\n",
        rendered.trim_end()
    );
    for reference in references {
        rendered.push_str(&reference.row());
        rendered.push('\n');
    }
    rendered
}

/// One task with its node's amendment rendered into it.
///
/// **Above the operational notes, never after them.** A task that states
/// [`ADDITIONAL_INFO_HEADING`] gets the block immediately before that heading,
/// and one that states none gets it at the end — which is the same placement
/// read the same way, since a task with no notes has nothing for the amendment
/// to sit above.
fn amended(task: &str, amendment: &str) -> String {
    let block = format!("{AMENDMENT_HEADING}\n{AMENDMENT_PRECEDENCE}\n\n{amendment}\n");
    match additional_info_at(task) {
        Some(at) => format!("{}\n\n{block}\n{}", task[..at].trim_end(), &task[at..]),
        None => format!("{}\n\n{block}", task.trim_end()),
    }
}

/// Where the task's operational notes begin, as a byte offset of the line the
/// heading is on.
///
/// Matched as a **whole line**, so prose that mentions the heading — a task
/// telling a worker where to put something — is not mistaken for the section
/// itself. A task that spells its notes some other way has none this can find,
/// and its amendment goes at the end, which is what a task with no notes gets.
fn additional_info_at(task: &str) -> Option<usize> {
    let mut at = 0;
    for line in task.split_inclusive('\n') {
        if line.trim_end() == ADDITIONAL_INFO_HEADING {
            return Some(at);
        }
        at += line.len();
    }
    None
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
    /// The manager's binding ruling on this node, part of its effective task
    /// until something replaces it. Present and blank is refused where every
    /// plan and every edited graph is validated, the way `amend` refuses a blank
    /// ruling: a bar nobody can clear is not one.
    ///
    /// The other half of the pair [`context`](Self::context) is one of, and the
    /// distinction is the point of both: a note steers the worker for one
    /// dispatch and says it adds no acceptance criteria, while this is rendered
    /// into the task the worker **and its judge** are handed, on the dispatch
    /// that follows it and on every later one, until an `amend` replaces it. A
    /// turn already running is not reached: its task was composed before the
    /// ruling existed. Replace rather than
    /// append, because a bar that can only grow cannot be corrected: a ruling
    /// thought better of would otherwise go on binding the judge beside its own
    /// correction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amendment: Option<String>,
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
    /// The **integration target**: the branch this node's work is cut from,
    /// kept in sync with, and — at publication — compared against. Absent, the
    /// repository identity's own default base applies.
    ///
    /// It is not a second spelling of [`branch`](Self::branch), and setting it
    /// equal to one is **not a supported way to continue an existing branch**.
    /// A node written that way would be asked at publication what its branch
    /// adds to itself, which is nothing by construction, so `onevcs` **refuses**
    /// it when the session opens rather than at the end of a dispatch nobody can
    /// use: the node settles `infrastructure-failure` carrying the sibling's own
    /// sentence, which names the spelling below.
    ///
    /// To continue work a previous attempt preserved, pin [`branch`](Self::branch)
    /// to it and leave this naming the branch the work is going to land on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    /// Pin the work to a named branch instead of generating one.
    ///
    /// **Where the work goes**, and never what it is measured against: one
    /// branch for the node, shared by every one of its [`steps`](Self::steps).
    /// Absent, `onevcs` names the branch when it opens the node's session.
    ///
    /// A name that already exists is **continued** from its own tip, so this is
    /// the whole of what a node pinned at work a previous attempt preserved has
    /// to say — whether that work is on a session still open, a session its
    /// owner closed, or a branch somebody landed by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The change request's title. Required on a lifecycle node from
    /// [`PLAN_SCHEMA_VERSION`] on; absent, the publication takes the subject
    /// `onevcs` derives from the branch's own conventional commits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The change request's body, when the planner writes it rather than
    /// leaving it to the `pr-author` graph a launch names.
    ///
    /// Publication-only, like the six fields around it: a node that carries no
    /// `repo` never publishes, so it never reads one. Accepted there rather than
    /// refused, because that is what every one of those six already does and one
    /// field answering differently is the surprise.
    // llmlint: ignore[changed_behavior_has_e2e] the journeys that matter are the ones
    // that publish, and both are driven end to end: a node stating its own body and a
    // `pr-author` dispatch drafting one. A node kind that never publishes ignoring this
    // is the plan shape's standing convention rather than behaviour this field changed,
    // so an e2e for it would pin a promise the other six do not make.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// The registered checkout the per-run clone is cut from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_checkout: Option<String>,
    /// Several agent and human steps run in sequence on one branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<Step>>,
    /// Continue preserved work rather than cutting a fresh branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<Resume>,
    /// Whether this node waits for the **work** of its dependencies or for the
    /// **releases** that carry it.
    ///
    /// The first rung of a four-rung chain, and the only one a plan states: the
    /// repository rung and the global rung are `onevcs`'s, and `fast` is the
    /// floor beneath both. Absent — which is every plan written before this
    /// field existed — the node resolves through the three rungs below it, and a
    /// host that has configured nothing resolves to `fast`, which is the
    /// behaviour it always had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adoption: Option<Adoption>,
    /// The release target each dependency is consumed at, by **dependency node
    /// id**.
    ///
    /// Keyed by node rather than by repository because the dependency is a node:
    /// two nodes in one repository can legitimately want different targets — a
    /// crate and a wheel cut from the same tree — and a repository key could not
    /// tell them apart. A dependency this names none for takes the target its
    /// own repository declaration marks as the default, so a plan states a
    /// target only where the default is not what it wants.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub consumes: BTreeMap<String, TargetName>,
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

/// One out-of-repository dependency, as the run can name it at dispatch time.
///
/// Every cell is what this run **observed**, and a cell it could not observe is
/// empty rather than absent: a worker needs to see that a dependency exists even
/// where the run cannot yet say which branch or commit it is on, and a row
/// dropped for a missing cell would hide the dependency itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrossRepoReference {
    /// The dependency, as the plan names it — a node id, or a cross-DAG
    /// `run:<id>#<node>` reference.
    pub dependency: String,
    /// The repository identity its work lands in.
    pub repository: String,
    /// The branch the work is on.
    pub branch: String,
    /// The commit that work reached its base at.
    pub commit: String,
    /// The release target this node consumes that repository at.
    pub release_target: String,
}

impl CrossRepoReference {
    /// The row this reference renders as, cells in the header's order.
    fn row(&self) -> String {
        format!(
            "| {} | {} | {} | {} | {} |",
            cell(&self.dependency),
            cell(&self.repository),
            cell(&self.branch),
            cell(&self.commit),
            cell(&self.release_target),
        )
    }
}

/// One cell of the reference table, held to what a table row may carry.
///
/// Every cell's text came from somewhere else: a repository identity and a
/// release target out of this host's own configuration, a branch and a commit out
/// of git, and either of the last two out of *another run's* ledger. A `|` in any
/// of them ends the cell early and a newline ends the row, so what a worker reads
/// would be a row about a dependency nobody has — which is exactly the misreading
/// the block exists to prevent. Escaped rather than refused, because the point of
/// a row is that the dependency is visible.
fn cell(value: &str) -> String {
    let mut rendered = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '|' => rendered.push_str("\\|"),
            _ if character.is_control() || character.is_whitespace() => rendered.push(' '),
            _ => rendered.push(character),
        }
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This schema's own source, which is where its documentation lives.
    ///
    /// `docs/contract.md` names the node shapes and not what each field of one
    /// means, so the doc comments here are the whole of the plan schema's
    /// documentation — and a statement nothing checks is one that goes stale the
    /// first time somebody trims it.
    const SCHEMA: &str = include_str!("plan.rs");

    /// The doc comment a field carries, as this file writes it.
    ///
    /// The contiguous `///` lines immediately above the declaration, joined into
    /// one string. Empty when the field is not declared at all, which fails the
    /// assertion that reads it rather than passing an empty search.
    fn documentation_of(field: &str) -> String {
        let declaration = format!("pub {field}:");
        let mut lines: Vec<&str> = Vec::new();
        for line in SCHEMA.lines() {
            let line = line.trim();
            if line.starts_with("///") {
                lines.push(line.trim_start_matches("///").trim());
                continue;
            }
            if line.starts_with(&declaration) {
                return lines.join(" ");
            }
            // Anything else — an attribute is the ordinary case — leaves the
            // comment standing, and a blank line or another item ends it.
            if !line.starts_with('#') {
                lines.clear();
            }
        }
        String::new()
    }

    /// The two fields that decide where a lifecycle node's work goes and what it
    /// is measured against say which is which.
    ///
    /// A drift test rather than a restatement: it holds the *claims* and not the
    /// wording, so the sentences can be rewritten and only removing what they
    /// say fails. The other half — that the consequence it names is still what
    /// happens — is
    /// `a_node_whose_base_branch_is_its_branch_is_refused_and_told_what_continues_a_branch`
    /// in `tests/e2e/lifecycle.rs`, which drives a plan written that way through
    /// the real repository side, beside `session_reuse.rs` for the spelling it
    /// points a planner at. Between them the statement cannot go stale in either
    /// direction: one fails if the documentation stops saying it, the others if
    /// the behaviour stops doing it.
    #[test]
    fn the_schema_says_what_branch_and_base_branch_mean_for_a_lifecycle_node() {
        let branch = documentation_of("branch");
        for claim in ["Where the work goes", "continued"] {
            assert!(
                branch.contains(claim),
                "`branch` no longer documents '{claim}', which is what tells a planner \
                 that pinning it is the whole of how work already on a branch is \
                 continued: {branch}"
            );
        }
        let base = documentation_of("base_branch");
        for claim in [
            "integration target",
            "compared against",
            "not a supported way to continue an existing branch",
            "refuses",
        ] {
            assert!(
                base.contains(claim),
                "`base_branch`'s documentation no longer states '{claim}', which is what \
                 stops a planner writing `base_branch` equal to `branch` and reading the \
                 refusal it earns as a verdict about the work: {base}"
            );
        }
    }

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

    /// An amendment is the opposite of the note beside it, and both halves are
    /// asserted: it opens by claiming authority over the notes below it, it
    /// carries no disclaimer of its own, and it sits **above** the operational
    /// notes rather than among them.
    #[test]
    fn an_amendment_renders_above_the_operational_notes_and_states_its_authority() {
        let node = Node {
            id: "build".into(),
            persona: Some("engineer".into()),
            task: Some(
                "## What\nship it\n\n## Acceptance criteria\n\n- it ships\n\n\
                 ## Additional info\n\nRun the gate once, over the finished tree.\n"
                    .into(),
            ),
            amendment: Some("The four comment lines are out of scope: leave them.".into()),
            ..Node::default()
        };
        let rendered = node.rendered_task();
        let at = |needle: &str| {
            rendered
                .find(needle)
                .unwrap_or_else(|| panic!("{needle} is not in:\n{rendered}"))
        };
        assert!(
            at("## Acceptance criteria") < at(AMENDMENT_HEADING)
                && at(AMENDMENT_HEADING) < at("## Additional info"),
            "the amendment is not immediately above the operational notes:\n{rendered}"
        );
        assert!(
            rendered.contains(
                "Where this section and the operational notes below disagree, this section wins."
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("The four comment lines are out of scope: leave them."),
            "{rendered}"
        );
        // The note's disclaimer is the note's. An amendment that carried one
        // would be the very thing this lever exists because `context` is.
        assert!(
            !rendered.contains("adds no acceptance criteria"),
            "the amendment disclaimed itself:\n{rendered}"
        );
        // And the notes it sits above are still there, whole.
        assert!(
            rendered.contains("Run the gate once, over the finished tree."),
            "{rendered}"
        );
    }

    /// A task with no operational notes takes its amendment at the end, and a
    /// node with no amendment renders exactly what it always rendered.
    #[test]
    fn an_amendment_lands_at_the_end_of_a_task_that_states_no_operational_notes() {
        let mut node = Node {
            id: "build".into(),
            persona: Some("engineer".into()),
            task: Some("## What\nship it".into()),
            ..Node::default()
        };
        assert_eq!(node.rendered_task(), "## What\nship it");

        node.amendment = Some("Leave the comments.".into());
        assert_eq!(
            node.rendered_task(),
            "## What\nship it\n\n\
             ## Amendment\n\
             Where this section and the operational notes below disagree, this section wins.\n\n\
             Leave the comments.\n"
        );

        // Blank is nothing, exactly as a blank note is: whitespace does not
        // become a section of its own.
        node.amendment = Some("   \n".into());
        assert_eq!(node.rendered_task(), "## What\nship it");

        // Prose that *mentions* the heading is not the section, so an amendment
        // is not hidden inside a paragraph telling a worker where notes go.
        node.amendment = Some("Leave the comments.".into());
        node.task = Some("## What\nput it under ## Additional info when you write one".into());
        let rendered = node.rendered_task();
        assert!(
            rendered.trim_end().ends_with("Leave the comments."),
            "prose naming the heading was read as the section:\n{rendered}"
        );
    }

    /// Both levers on one node, and each says what it is.
    ///
    /// The amendment is part of the task the judge reads; the note follows it,
    /// under its own heading, still disclaiming itself. A run that carried one
    /// and lost the other would be the failure this pair exists to end.
    #[test]
    fn a_node_carrying_both_levers_renders_each_under_its_own_heading() {
        let node = Node {
            id: "build".into(),
            persona: Some("engineer".into()),
            task: Some("## What\nship it\n\n## Additional info\n\nrun the gate.\n".into()),
            amendment: Some("Leave the comments.".into()),
            context: Some("the fixture moved to tests/data".into()),
            ..Node::default()
        };
        let rendered = node.rendered_task();
        let at = |needle: &str| rendered.find(needle).expect("it is rendered");
        assert!(
            at(AMENDMENT_HEADING) < at("## Additional info")
                && at("## Additional info") < at(PLANNER_CONTEXT_HEADING),
            "{rendered}"
        );
        assert!(
            rendered.contains("adds no acceptance criteria"),
            "{rendered}"
        );
        assert!(rendered.contains("this section wins"), "{rendered}");
    }

    /// Every step of a workstream is judged against the node's amendment,
    /// because every step of one workstream shares one branch and one bar.
    #[test]
    fn a_step_renders_the_amendment_of_the_node_it_belongs_to() {
        let step = Step {
            id: "implement".into(),
            task: Some("## What\nimplement\n\n## Additional info\n\nnotes.\n".into()),
            ..Step::default()
        };
        let node = Node {
            id: "service".into(),
            amendment: Some("Leave the comments.".into()),
            context: Some("the API moved".into()),
            ..Node::default()
        };
        let rendered = step.rendered_task_for(&node, &[]);
        assert!(rendered.contains("Leave the comments."), "{rendered}");
        assert!(rendered.contains("this section wins"), "{rendered}");
        assert!(rendered.contains("the API moved"), "{rendered}");
        assert!(
            rendered.find(AMENDMENT_HEADING) < rendered.find("## Additional info"),
            "{rendered}"
        );
        // The pair written before amendments existed answers exactly as it did
        // then: the node's note, and nothing about its bar.
        let older = step.rendered_task_with(node.context.as_deref(), &[]);
        assert!(!older.contains(AMENDMENT_HEADING), "{older}");
        assert_eq!(older, step.rendered_task(node.context.as_deref()));
    }

    /// The reference block is the shape the divergence record states, appended by
    /// the same rendering the planner note is — and a node with none renders
    /// exactly what it always rendered.
    #[test]
    fn out_of_repository_dependencies_render_as_a_table_under_their_own_heading() {
        let node = Node {
            id: "consumer".into(),
            persona: Some("engineer".into()),
            task: Some("## What\nship it".into()),
            ..Node::default()
        };
        let references = vec![
            CrossRepoReference {
                dependency: "onevcs-release-targets".into(),
                repository: "github.com/nickderobertis/onevcs".into(),
                branch: "onevcs-release-targets".into(),
                commit: "9f3c1ab".into(),
                release_target: "crate".into(),
            },
            // A dependency the run cannot fully name: the cells it cannot fill
            // are empty and the row is still there, because a worker needs to see
            // that the dependency exists.
            CrossRepoReference {
                dependency: "packager".into(),
                repository: "github.com/nickderobertis/other".into(),
                ..CrossRepoReference::default()
            },
        ];
        assert_eq!(
            node.rendered_task_with(&references),
            "## What\nship it\n\n\
             ## Cross-repository references\n\n\
             This node launched under fast adoption: the work it depends on is finished but has \
             no\nrelease yet. Pin against the git references below rather than against a version. \
             Do\nnot change a shared interface unilaterally — propose it and keep building \
             against the\nagreed surface. When these releases arrive you will be sent a note \
             naming the\nversions; move the pin then.\n\n\
             | dependency | repository | branch | commit | release target |\n\
             | --- | --- | --- | --- | --- |\n\
             | onevcs-release-targets | github.com/nickderobertis/onevcs | \
             onevcs-release-targets | 9f3c1ab | crate |\n\
             | packager | github.com/nickderobertis/other |  |  |  |\n"
        );
        assert_eq!(
            node.rendered_task_with(&[]),
            node.rendered_task(),
            "a node with no out-of-repository dependency did not render what it always rendered"
        );

        // A cell whose text would end the cell or the row is escaped, so the
        // table stays a table and the row stays about the dependency it names.
        let forged = CrossRepoReference {
            dependency: "dep".into(),
            repository: "github.com/owner/a|b".into(),
            branch: "topic\n| forged | row | here | now |".into(),
            ..CrossRepoReference::default()
        };
        let rendered = node.rendered_task_with(&[forged]);
        let rows: Vec<&str> = rendered
            .lines()
            .filter(|line| line.starts_with("| dep |"))
            .collect();
        assert_eq!(rows.len(), 1, "a cell forged a second row:\n{rendered}");
        assert_eq!(
            rows[0],
            "| dep | github.com/owner/a\\|b | topic \\| forged \\| row \\| here \\| now \\| |  |  |",
            "a cell was not escaped"
        );
        assert_eq!(
            rendered
                .lines()
                .filter(|line| line.starts_with('|'))
                .count(),
            3,
            "the table is not a header, a separator, and one row:\n{rendered}"
        );

        // It sits under the planner note rather than beside it: one document,
        // and the note is still the note.
        let noted = Node {
            context: Some("the earlier round already landed the schema".into()),
            ..node
        };
        let rendered = noted.rendered_task_with(&references);
        assert!(
            rendered.find(PLANNER_CONTEXT_HEADING) < rendered.find(CROSS_REPO_REFERENCES_HEADING),
            "{rendered}"
        );
        assert!(
            rendered.contains("adds no acceptance criteria"),
            "{rendered}"
        );
        // And every step of one workstream is handed the same block.
        let step = Step {
            id: "implement".into(),
            task: Some("## What\nimplement".into()),
            ..Step::default()
        };
        assert!(step
            .rendered_task_with(None, &references)
            .contains(CROSS_REPO_REFERENCES_HEADING));
        assert_eq!(step.rendered_task_with(None, &[]), step.rendered_task(None));
    }

    /// Both new fields are optional, load at schema 3, and are omitted from what
    /// this crate writes when a plan named neither.
    #[test]
    fn the_adoption_fields_round_trip_and_never_appear_in_a_plan_that_omitted_them() {
        let written = serde_json::json!({
            "id": "consumer",
            "deps": ["engine"],
            "adoption": "published",
            "consumes": {"engine": "crate"}
        });
        let node: Node = serde_json::from_value(written.clone()).expect("both fields load");
        assert_eq!(node.adoption, Some(Adoption::Published));
        assert_eq!(
            node.consumes.get("engine").map(ToString::to_string),
            Some("crate".to_string())
        );
        assert_eq!(serde_json::to_value(&node).expect("serializes"), written);

        let bare = serde_json::json!({"id": "solo"});
        let node: Node = serde_json::from_value(bare.clone()).expect("a node naming neither loads");
        assert_eq!(node.adoption, None);
        assert!(node.consumes.is_empty());
        assert_eq!(serde_json::to_value(&node).expect("serializes"), bare);

        // External input, refused at the boundary: an adoption mode this build
        // does not know, and a target name that could not be one.
        serde_json::from_value::<Node>(serde_json::json!({"id": "x", "adoption": "eventually"}))
            .expect_err("an undeclared adoption mode is refused");
        serde_json::from_value::<Node>(
            serde_json::json!({"id": "x", "consumes": {"engine": "not a target name"}}),
        )
        .expect_err("a target name the sibling would refuse is refused here");
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

    /// A plan carrying the retired field is answered about the *field*, at every
    /// version a document can declare it at.
    ///
    /// The bar its author wrote has to go somewhere, and only the field's own
    /// refusal says where. It is a **parse** refusal, so it comes ahead of
    /// anything the version decides — which is what stops a planner being told to
    /// move a number when what they have to move is a review bar.
    #[test]
    fn a_plan_carrying_done_when_is_answered_about_the_field_at_every_version() {
        let root = scratch("donewhen");
        for version in PLAN_SCHEMA_VERSIONS_READ {
            let path = root.join(format!("v{version}.plan.json"));
            std::fs::write(
                &path,
                format!(
                    r#"{{"schema_version":{version},"tasks":[
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
                "a version refusal displaced the field's: {message}"
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// The second retired field is answered the same way, and about itself.
    ///
    /// A plan that set `verify_via_ci` asked for the host's checks to be the
    /// merge-path verification, and got a run in which nothing read the field at
    /// all. `unknown field` would tell its author the field does not exist and
    /// leave them to guess what asks for it now; this names the policy that does.
    #[test]
    fn a_plan_carrying_verify_via_ci_is_answered_about_the_field_at_every_version() {
        let root = scratch("verifyviaci");
        for version in PLAN_SCHEMA_VERSIONS_READ {
            let path = root.join(format!("v{version}.plan.json"));
            std::fs::write(
                &path,
                format!(
                    r#"{{"schema_version":{version},"tasks":[
                        {{"id":"service","repo":"owner/service","title":"feat: thing",
                         "persona":"e","task":"t","verify_via_ci":true}}]}}"#
                ),
            )
            .expect("written");
            let message = Plan::load(&path).unwrap_err().to_string();
            assert!(message.contains("'service':"), "{message}");
            assert!(message.contains(VERIFY_VIA_CI_RETIRED), "{message}");
            assert!(
                message.contains("change-auto"),
                "the refusal does not say what asks for the host's checks now: {message}"
            );
            assert!(
                !message.contains("schema_version"),
                "a version refusal displaced the field's: {message}"
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
