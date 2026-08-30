//! The task DAG: what a plan must be, and what its nodes may do next.
//!
//! Two questions live here and nowhere else. **Is this graph legal?** — the node
//! shape rules, the reference rules, and acyclicity, checked at the trust
//! boundary so a typo fails loudly before any provider time is spent. And
//! **what may run now?** — each node's derived status against the nodes that
//! have already settled.
//!
//! `blocked` and `skipped` are *derived*, never stored: every committed edit
//! discards them and re-derives them against the new graph, so a node the
//! planner just made eligible is schedulable on the same pass.

use std::collections::{BTreeMap, BTreeSet};

use onevcs::provenance::SUBJECT_LIMIT;
use serde::{Deserialize, Serialize};

use crate::controls::NodeControls;
use crate::error::{Error, Result};
use crate::plan::{Node, NodeKind, Plan, Step};

/// The separator reserved for addressing a step within its node.
pub const STEP_SEPARATOR: char = '/';

/// The identity of a node this graph carries.
///
/// A newtype and never a bare `String`, because a node identity is not free text
/// and the place that says so is here: [`validate`] refuses a plan whose node has
/// a blank id, and this is the type that carries what that refusal established
/// past it. [`NodeRef::of`] is the only constructor and it takes a
/// [`Node`] — so an identity crossing a boundary as a value of this type came
/// from a node in a graph, rather than from a field anybody could put anything
/// in.
///
/// Used where a node identity leaves the reconcile loop's own scope and has to
/// be carried rather than borrowed: a message from a dispatch thread, which is
/// exactly where an unvalidated string would arrive at the single writer with
/// nothing left to check it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodeRef(String);

impl NodeRef {
    /// The identity of `node`, or `None` where it has none.
    ///
    /// Fallible rather than assumed, even though [`validate`] has already
    /// refused a blank id on every path a plan reaches execution by: a
    /// constructor that took the id on trust would leave the empty identity
    /// representable again, one indirection later.
    pub(crate) fn of(node: &Node) -> Option<Self> {
        let id = node.id.trim();
        (!id.is_empty()).then(|| Self(id.to_string()))
    }

    /// The identity as a graph, a journal label, and a surface spell it.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where a node has got to.
///
/// The settled statuses are recorded in the journal; [`Blocked`](Self::Blocked)
/// and [`Skipped`](Self::Skipped) are derived on every read and never written as
/// a node's own settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    /// Not yet eligible, and nothing prevents it becoming so.
    Pending,
    /// Eligible now: every dependency is `done`.
    Ready,
    /// A dispatch is in flight.
    Running,
    /// A ready human action needs a person.
    Waiting,
    /// Transitively gated by a waiting human or a parked dependency.
    Blocked,
    /// A planner `cancel` idled it. No later pass dispatches it until a
    /// `requeue`.
    Parked,
    /// A `drop` or `retry` stopped its dispatch cooperatively. Deliberately not
    /// [`Parked`](Self::Parked): the engine took this stop, so the harness
    /// finishes what it started, while a park is the planner's own idle.
    Cancelled,
    /// It executed and completed.
    Done,
    /// It executed and completed, and the change it published is a **draft**
    /// because a release it adopted early has not happened yet.
    ///
    /// Neither settled nor running: merging now would make the node's temporary
    /// git pin permanent in a base branch, so no dependent may start on it and no
    /// run holding one has settled. What clears it is the release arriving, which
    /// puts a new worker on the branch this node already has.
    CompleteDraft,
    /// It executed and failed.
    Failed,
    /// A failed dependency made execution unsafe.
    Skipped,
}

impl NodeStatus {
    /// The word this status is written and rendered as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
            Self::Parked => "parked",
            Self::Cancelled => "cancelled",
            Self::Done => "done",
            Self::CompleteDraft => "complete-but-draft",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    /// Read a status back from a journal record.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "pending" => Self::Pending,
            "ready" => Self::Ready,
            "running" => Self::Running,
            "waiting" => Self::Waiting,
            "blocked" => Self::Blocked,
            "parked" => Self::Parked,
            "cancelled" => Self::Cancelled,
            "done" => Self::Done,
            "complete-but-draft" => Self::CompleteDraft,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            _ => return None,
        })
    }

    /// Whether the loop is finished with the node.
    ///
    /// [`CompleteDraft`](Self::CompleteDraft) is deliberately not one: the node
    /// has finished its work and the run has not finished with it, because the
    /// release it is waiting on lifts the draft and dispatches it once more. A
    /// run whose nodes are all draft-complete is **waiting**, not settled.
    pub fn is_settled(self) -> bool {
        matches!(
            self,
            Self::Done
                | Self::Failed
                | Self::Skipped
                | Self::Waiting
                | Self::Blocked
                | Self::Parked
                | Self::Cancelled
        )
    }

    /// Whether a later pass may still dispatch the node.
    ///
    /// A `done` node is never rescheduled; a parked one waits for a `requeue`. A
    /// draft-complete one is dispatched again by the release it awaits arriving.
    pub fn is_dispatchable(self) -> bool {
        !matches!(self, Self::Done | Self::Parked)
    }
}

/// Whether a settled node's change reached the branch it was published onto.
///
/// **A qualifier on the terminal status, not a status of its own**, and the
/// choice is the point. A node *settles* when it publishes, and for a `team`
/// identity publishing opens a change request — so a node reports `done` while
/// its work sits in a pull request nobody has merged. "Settled" and "landed" are
/// different facts, and reporting only the first lets a planner close work on a
/// change that never reached anyone.
///
/// A tenth [`NodeStatus`] would have said the same thing in a place where it
/// changes what the run *does*: `done` is what makes a dependent eligible, what
/// stops a node being re-dispatched, what `state_of` reads for `complete`, and
/// what `docs/contract.md` fixes as the only settlement a cross-DAG reference
/// accepts. None of that should move — a node whose change is open really has
/// finished its work — so what is added is the second fact beside the status
/// rather than a new value inside it.
///
/// Recorded **only where a publication answered**, which is what keeps it an
/// observation. `None` is "no change of this node's to land": a direct agent
/// node, a human action, a workstream whose branch its base already carried,
/// a publication that failed and settled `failed` under its own name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Landing {
    /// The change reached its base branch, and this run **saw** it get there:
    /// `onevcs` answered the publication with the commit it landed at.
    Landed,
    /// The node published a change that has not reached its base branch — a
    /// change request open for review, or one the host has queued behind checks.
    ///
    /// A statement about the moment the node settled, and never re-derived
    /// afterwards: a person merging the change later is not something this run
    /// waits for, polls for, or claims to know about. See
    /// [`crate::vcs::landing_of`].
    Unlanded,
}

impl Landing {
    /// The word this landing is written and rendered as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Landed => "landed",
            Self::Unlanded => "unlanded",
        }
    }

    /// Read a landing back from a journal record.
    ///
    /// `None` for a word this build does not know, which is the same reading a
    /// record written before the field existed gets: a build that cannot
    /// interpret a landing has not observed one, and inventing either answer
    /// from an unreadable value is the false report this whole distinction
    /// exists to remove.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "landed" => Some(Self::Landed),
            "unlanded" => Some(Self::Unlanded),
            _ => None,
        }
    }
}

/// How a whole graph settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphState {
    /// Every node is `done`.
    Complete,
    /// Something waits, is blocked, or is parked, and nothing failed.
    Waiting,
    /// A node failed or was skipped.
    Failed,
}

impl GraphState {
    /// The word this state is written and rendered as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Waiting => "waiting",
            Self::Failed => "failed",
        }
    }

    /// The process exit status a settled graph carries: 0 complete, 1 otherwise.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Complete => crate::error::EXIT_SUCCESS,
            _ => crate::error::EXIT_QUEUED,
        }
    }
}

/// The desired graph: the nodes the loop is converging toward.
///
/// Insertion-ordered so a plan, a run's launch record, and the graph a
/// reconcile pass folds all render their nodes in the order the planner wrote
/// them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Graph {
    order: Vec<String>,
    nodes: BTreeMap<String, Node>,
    /// How many nodes may be dispatched at once.
    pub concurrency: u32,
}

impl Graph {
    /// An empty graph that will dispatch at most `concurrency` nodes at once.
    pub fn with_concurrency(concurrency: u32) -> Self {
        Self {
            concurrency,
            ..Self::default()
        }
    }

    /// The graph a plan describes.
    pub fn from_plan(plan: &Plan) -> Self {
        let mut graph = Self::with_concurrency(plan.concurrency);
        for node in &plan.tasks {
            graph.insert(node.clone());
        }
        graph
    }

    /// The plan this graph would be written as.
    pub fn to_plan(&self, source: &Plan) -> Plan {
        Plan {
            schema_version: source.schema_version,
            goal: source.goal.clone(),
            name: source.name.clone(),
            concurrency: self.concurrency,
            tasks: self.iter().cloned().collect(),
        }
    }

    /// Add a node, or replace one with the same id in place.
    pub fn insert(&mut self, node: Node) {
        if !self.nodes.contains_key(&node.id) {
            self.order.push(node.id.clone());
        }
        self.nodes.insert(node.id.clone(), node);
    }

    /// Remove a node, keeping the order of the rest.
    pub fn remove(&mut self, id: &str) -> Option<Node> {
        self.order.retain(|existing| existing != id);
        self.nodes.remove(id)
    }

    /// One node, by id.
    pub fn get(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// One node, by id, to change.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.get_mut(id)
    }

    /// Whether the graph holds a node with this id.
    pub fn contains(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    /// The nodes, in the order the plan wrote them.
    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.order.iter().filter_map(|id| self.nodes.get(id))
    }

    /// The node ids, in the order the plan wrote them.
    pub fn ids(&self) -> impl Iterator<Item = &String> {
        self.order.iter()
    }

    /// How many nodes the graph holds.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether the graph holds no nodes.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The nodes that name `id` as a dependency.
    pub fn dependents_of(&self, id: &str) -> Vec<String> {
        self.iter()
            .filter(|node| node.deps.iter().any(|dep| dep == id))
            .map(|node| node.id.clone())
            .collect()
    }
}

/// Whether a dependency reference names another run's node rather than this
/// graph's.
///
/// A cross-DAG reference names no node of this graph, so nothing here can
/// satisfy it: it is never removed as a satisfied dependency, and it is
/// re-resolved against the referenced run's ledger on every reconcile pass.
pub fn is_cross_dag(reference: &str) -> bool {
    crate::crossdag::is_reference(reference)
}

/// Check that a plan is one this engine may execute.
///
/// External input is validated here, at its trust boundary, so an unsatisfiable
/// graph is refused before any provider time is spent on it.
pub fn validate(plan: &Plan) -> Result<()> {
    if plan.tasks.is_empty() {
        return Err(Error::Invalid("a plan needs at least one node".into()));
    }
    validate_edited(plan)?;
    validate_declared_version(plan)
}

/// The two rules a plan's **own declared version** decides.
///
/// A plan is a document written at a version and read by a build, so what
/// it may say is the version it declares rather than the one this build writes.
/// Version 3 states the change request a lifecycle node publishes: a `title` is
/// required on one, and a `body` may be carried beside it. A plan declaring an
/// earlier version in
/// [`PLAN_SCHEMA_VERSIONS_READ`](crate::plan::PLAN_SCHEMA_VERSIONS_READ) is read
/// as that version — its untitled lifecycle nodes publish under the subject
/// `onevcs` derives from the branch's own conventional commits — and naming a
/// field that version never had is refused **by the field's name**, exactly as
/// an unknown field is.
///
/// Here rather than in [`validate_node`], which is the shape check every
/// *edited* graph is also held to: an edit is compiled against the version this
/// build writes, and a graph folded from a version-2 run carries the untitled
/// lifecycle nodes that run was launched with. Requiring a title there would
/// refuse every later edit to those runs — a `retry` of a node clones it — which
/// is the whole graph held hostage to a field the plan was never written with.
fn validate_declared_version(plan: &Plan) -> Result<()> {
    for node in &plan.tasks {
        let named = |what: String| Error::Invalid(format!("node '{}': {what}", node.id));
        // The version is the whole of what `body` is checked for, and deliberately.
        // It is one of seven publication-only optional fields the common node shape
        // carries — `title`, `branch`, `merge_policy`, `base_branch`, `repo_type` and
        // `workflow` are the others — and what makes a node a lifecycle node is
        // `repo`. None of the other six is refused on a node kind that never
        // publishes, so refusing this one alone would answer a planner differently
        // for the same mistake depending on which field they made it in.
        // llmlint: ignore[boundary_inputs_validated] the paragraph above is the decision:
        // the schema struct's `deny_unknown_fields` is this document's boundary, and what a
        // node kind does with a field it does not use is the plan shape's own convention
        // rather than a validation this field is missing.
        if node.body.is_some() && plan.schema_version < crate::plan::PLAN_SCHEMA_VERSION {
            return Err(named(crate::plan::body_is_newer(plan.schema_version)));
        }
        if plan.schema_version >= crate::plan::PLAN_SCHEMA_VERSION
            && node.repo.is_some()
            && node.title.is_none()
        {
            return Err(named(crate::plan::TITLE_IS_REQUIRED.to_owned()));
        }
    }
    Ok(())
}

/// Check a graph an edit produced.
///
/// Every rule [`validate`] applies except one: an edit may legally empty the
/// graph. A `drop` that removes the last node leaves a run with nothing left to
/// do, which is a settled run rather than a malformed plan — refusing it would
/// make the planner unable to abandon a graph it started.
pub fn validate_edited(plan: &Plan) -> Result<()> {
    // Every version this build reads, because what each newer one added is keyed
    // to the version a document declares: a plan written at an earlier one
    // describes a graph this engine executes exactly as it always did, and its
    // author has nothing to migrate. A number outside the set is one this crate
    // has never written and there is no document to read it as, so it is refused
    // by its number and the ones that are read are named.
    let read = crate::plan::PLAN_SCHEMA_VERSIONS_READ;
    if !read.contains(&plan.schema_version) {
        let known = read
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::Invalid(format!(
            "plan schema_version {} is not one this build reads ({known})",
            plan.schema_version
        )));
    }
    if plan.concurrency == 0 {
        return Err(Error::Invalid("concurrency must be at least 1".into()));
    }
    if let Some(goal) = &plan.goal {
        if goal.text.trim().is_empty() {
            return Err(Error::Invalid("a goal needs non-empty text".into()));
        }
    }

    let mut seen = BTreeSet::new();
    for node in &plan.tasks {
        if node.id.trim().is_empty() {
            return Err(Error::Invalid("every node needs a non-empty id".into()));
        }
        if !seen.insert(node.id.clone()) {
            return Err(Error::Invalid(format!("duplicate node id '{}'", node.id)));
        }
        validate_node(node)?;
    }

    for node in &plan.tasks {
        for dep in &node.deps {
            if dep == &node.id {
                return Err(Error::Invalid(format!(
                    "node '{}' depends on itself",
                    node.id
                )));
            }
            if is_cross_dag(dep) {
                continue;
            }
            // Anything meant as a cross-DAG reference names no node of this
            // graph, so reporting it as a missing dependency would send a
            // planner looking for a node they never wrote.
            if crate::crossdag::is_malformed(dep) {
                return Err(Error::Invalid(format!(
                    "node '{}' depends on '{dep}', which is a malformed cross-DAG \
                     reference; expected '{}'",
                    node.id,
                    crate::crossdag::SYNTAX
                )));
            }
            if !seen.contains(dep) {
                return Err(Error::Invalid(format!(
                    "node '{}' depends on '{dep}', which is not in the plan",
                    node.id
                )));
            }
        }
    }

    if let Some(cycle) = find_cycle(&plan.tasks) {
        return Err(Error::Invalid(format!("dependency cycle: {cycle}")));
    }
    Ok(())
}

/// What a plan naming the reserved drafting persona is told.
///
/// The persona a dispatch runs under is what tells the change request's drafting
/// apart from a node's own work — it is the one fact the executor has when it
/// composes a launch, and the two are composed differently: a node's dispatch
/// carries the run's node-scope overrides and its own persona, and the drafting
/// dispatch carries the graph the operator named and nothing else. A node
/// claiming this name would be composed as the drafting dispatch and lose the
/// overrides it declared, silently, which is the one direction this crate
/// refuses to fail in. So the name is refused where a plan is read.
pub(crate) const RESERVED_PERSONA: &str = "`pr-author` is the persona this crate dispatches a      change request's drafting under, so a node's own worker cannot run as it";

/// Check one node's shape.
pub fn validate_node(node: &Node) -> Result<()> {
    let named = |what: &str| Error::Invalid(format!("node '{}': {what}", node.id));
    if node.persona.as_deref() == Some(crate::lifecycle::PR_AUTHOR_PERSONA) {
        return Err(named(RESERVED_PERSONA));
    }

    // A title is only ever the subject of a publication, and only a node that
    // names a repo publishes — so this holds exactly the nodes whose title
    // `onevcs` will read, and holds them here rather than there: it checks the
    // title after a whole dispatch and its gate, by which point a retry can only
    // recompute the same title from the same plan and be refused identically.
    if let (Some(_), Some(title)) = (&node.repo, &node.title) {
        validate_title(title).map_err(|why| named(&why))?;
    }

    // A blank amendment is a bar nobody can clear. The `amend` op refuses one
    // rather than recording it, and a plan file is the same input by another
    // route — so it is refused here too, at the boundary every plan and every
    // edited graph crosses, rather than accepted and then silently left out of
    // the task it was written to change.
    if node
        .amendment
        .as_ref()
        .is_some_and(|text| text.trim().is_empty())
    {
        return Err(named(
            "`amendment` is present and says nothing — give it the ruling it carries, or leave \
             it out",
        ));
    }

    // `consumes` is keyed by **dependency node id**, so a key that names nothing
    // this node depends on is a plan whose author expected a target to apply and
    // will not find out from the run that it did not. Refused here, at the
    // boundary every plan and every edited graph crosses, rather than silently
    // dropped where the dependency is resolved.
    for consumed in node.consumes.keys() {
        if !node.deps.iter().any(|dep| dep == consumed) {
            return Err(named(&format!(
                "`consumes` names '{consumed}', which is not one of this node's deps"
            )));
        }
    }

    if node.kind == NodeKind::Human {
        if node.id.contains(STEP_SEPARATOR) {
            return Err(named(
                "a human id cannot contain '/', which addresses a step",
            ));
        }
        if node.task.as_ref().is_none_or(|t| t.trim().is_empty()) {
            return Err(named("a human node needs task prose"));
        }
        if node.persona.is_some() || node.max_turns.is_some() {
            return Err(named(
                "a human node has no dispatch, so no persona or turn budget",
            ));
        }
        if node.repo.is_some() || node.steps.is_some() || node.expects_no_diff {
            return Err(named("a human node has no execution fields"));
        }
        if node.context.is_some() {
            return Err(named(
                "a planner note is addressed to a dispatch, and a human node has none",
            ));
        }
        return Ok(());
    }

    if node.expects_no_diff {
        if node.task.as_ref().is_none_or(|t| t.trim().is_empty()) {
            return Err(named("an expects_no_diff node needs task prose"));
        }
        if node.persona.is_some() || node.max_turns.is_some() {
            return Err(named(
                "expects_no_diff settles without a dispatch, so it takes no persona or turn budget",
            ));
        }
        if node.steps.is_some() {
            return Err(named("expects_no_diff and steps cannot both be set"));
        }
        return Ok(());
    }

    // Every control this node declares has to have somewhere to land. One that
    // does not is refused here — at the validation a launch runs, and the one a
    // live edit is checked against — rather than at a dispatch that would have
    // spent its budget under a default nobody asked for.
    NodeControls::of_node(node)
        .and_then(|controls| controls.overrides())
        .map_err(|why| named(&why))?;

    match (&node.repo, &node.steps) {
        (None, Some(_)) => Err(named("steps run on one branch, so they need a repo")),
        (None, None) => {
            if node.persona.is_none() {
                return Err(named("a direct agent node needs a persona"));
            }
            if node.task.as_ref().is_none_or(|t| t.trim().is_empty()) {
                return Err(named("a direct agent node needs task prose"));
            }
            Ok(())
        }
        (Some(_), Some(steps)) => {
            // The turn budget belongs with the persona and the task: a node with
            // steps dispatches none of its own, so one written here would reach
            // no dispatch at all. Refused rather than quietly ignored.
            if node.persona.is_some() || node.task.is_some() || node.max_turns.is_some() {
                return Err(named(
                    "a node with steps takes its persona, task, and turn budget from them",
                ));
            }
            validate_steps(node, steps)
        }
        (Some(_), None) => {
            if node.persona.is_none() {
                return Err(named("a lifecycle node needs a persona or steps"));
            }
            if node.task.as_ref().is_none_or(|t| t.trim().is_empty()) {
                return Err(named("a lifecycle node needs task prose"));
            }
            Ok(())
        }
    }
}

/// Check one explicit change-request title against the rule its publication is
/// held to.
///
/// The bound is [`onevcs::provenance::SUBJECT_LIMIT`] itself, and the length is
/// measured on the trimmed title as the sibling measures it, so nothing is
/// refused here that would have published. Blank is its own refusal because a
/// title that is only spacing publishes a commit with no subject at all, which
/// is the one shape a length check reads as fine.
// llmlint: ignore[invalid_states_unrepresentable] the validated title is `onevcs::Subject`, which the sibling owns and builds at the publication request; this reads the plain `String` a plan document wrote, which serde parses before any type could constrain it.
fn validate_title(title: &str) -> std::result::Result<(), String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("the title is blank, and a publication needs a subject".to_owned());
    }
    if title.len() > SUBJECT_LIMIT {
        return Err(format!(
            "the title is {} characters, over the {SUBJECT_LIMIT}-character limit onevcs \
             holds a publication subject to",
            title.len()
        ));
    }
    Ok(())
}

fn validate_steps(node: &Node, steps: &[Step]) -> Result<()> {
    let named = |what: String| Error::Invalid(format!("node '{}': {what}", node.id));
    if steps.is_empty() {
        return Err(named("a steps list cannot be empty".into()));
    }
    let mut seen = BTreeSet::new();
    for step in steps {
        if step.id.trim().is_empty() {
            return Err(named("every step needs a non-empty id".into()));
        }
        if !seen.insert(step.id.clone()) {
            return Err(named(format!("duplicate step id '{}'", step.id)));
        }
        if step.kind == NodeKind::Human {
            if step.id.contains(STEP_SEPARATOR) {
                return Err(named(format!(
                    "human step '{}' cannot contain '/', which addresses it",
                    step.id
                )));
            }
            if step.persona.is_some() || step.max_turns.is_some() || step.expects_no_diff {
                return Err(named(format!(
                    "human step '{}' has no dispatch, so no persona, turn budget, or \
                     expects_no_diff",
                    step.id
                )));
            }
        } else if step.expects_no_diff {
            if step.persona.is_some() || step.max_turns.is_some() {
                return Err(named(format!(
                    "step '{}': expects_no_diff settles without a dispatch",
                    step.id
                )));
            }
        } else {
            if step.persona.is_none() {
                return Err(named(format!("agent step '{}' needs a persona", step.id)));
            }
            if step.persona.as_deref() == Some(crate::lifecycle::PR_AUTHOR_PERSONA) {
                return Err(named(format!("step '{}': {RESERVED_PERSONA}", step.id)));
            }
            // The same rule the node above is held to: a control this build
            // cannot apply stops the plan here rather than at the step's launch.
            NodeControls::of_step(step)
                .and_then(|controls| controls.overrides())
                .map_err(|why| named(format!("step '{}': {why}", step.id)))?;
        }
        if step.task.as_ref().is_none_or(|t| t.trim().is_empty()) {
            return Err(named(format!("step '{}' needs task prose", step.id)));
        }
    }
    for step in steps {
        for dep in &step.deps {
            if dep == &step.id {
                return Err(named(format!("step '{}' depends on itself", step.id)));
            }
            if !seen.contains(dep) {
                return Err(named(format!(
                    "step '{}' depends on '{dep}', which is not a step of this node",
                    step.id
                )));
            }
        }
    }
    Ok(())
}

/// The first dependency cycle, rendered as the path around it.
fn find_cycle(nodes: &[Node]) -> Option<String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Open,
        Closed,
    }
    let deps: BTreeMap<&str, Vec<&str>> = nodes
        .iter()
        .map(|node| {
            (
                node.id.as_str(),
                node.deps
                    .iter()
                    .filter(|dep| !is_cross_dag(dep))
                    .map(String::as_str)
                    .collect(),
            )
        })
        .collect();
    let mut marks: BTreeMap<&str, Mark> = BTreeMap::new();
    let mut path: Vec<&str> = Vec::new();

    // Iterative depth-first search: a graph deep enough to overflow a recursive
    // walk is a graph a planner can legally write.
    for root in nodes.iter().map(|n| n.id.as_str()) {
        if marks.contains_key(root) {
            continue;
        }
        let mut stack: Vec<(&str, usize)> = vec![(root, 0)];
        marks.insert(root, Mark::Open);
        path.push(root);
        while let Some((node, index)) = stack.pop() {
            let children = deps.get(node).map(Vec::as_slice).unwrap_or_default();
            if index < children.len() {
                stack.push((node, index + 1));
                let child = children[index];
                match marks.get(child) {
                    Some(Mark::Open) => {
                        let start = path.iter().position(|n| *n == child).unwrap_or(0);
                        let mut cycle: Vec<&str> = path[start..].to_vec();
                        cycle.push(child);
                        return Some(cycle.join(" -> "));
                    }
                    Some(Mark::Closed) => {}
                    None => {
                        marks.insert(child, Mark::Open);
                        path.push(child);
                        stack.push((child, 0));
                    }
                }
            } else {
                marks.insert(node, Mark::Closed);
                path.pop();
            }
        }
    }
    None
}

/// Derive every node's status from the graph and what has already settled.
///
/// `recorded` holds only the statuses the journal wrote. Everything else —
/// `ready`, `pending`, and the two derived gates `blocked` and `skipped` — is
/// computed here against the current graph, so an edit that changes eligibility
/// takes effect on the same pass.
pub fn derive(
    graph: &Graph,
    recorded: &BTreeMap<String, NodeStatus>,
    resolved_cross_dag: &dyn Fn(&str) -> Option<NodeStatus>,
) -> BTreeMap<String, NodeStatus> {
    let mut statuses: BTreeMap<String, NodeStatus> = BTreeMap::new();
    for node in graph.iter() {
        if node.parked {
            // A park is the planner's own idle, and it outranks whatever the
            // dispatch it stopped went on to record: `cancel` parks a node *and*
            // stops its work, so a node parked while it was running settles
            // `cancelled` a moment later. Read the settlement instead and the
            // node reports as something a `requeue` is not obviously the way
            // back from — while the flag that actually holds it out of every
            // later dispatch sits unmentioned on its definition.
            statuses.insert(node.id.clone(), NodeStatus::Parked);
            continue;
        }
        if let Some(recorded) = recorded.get(&node.id) {
            // A recorded settlement stands, except for the two derived gates:
            // they are re-derived against the graph as it is now.
            if !matches!(recorded, NodeStatus::Blocked | NodeStatus::Skipped) {
                statuses.insert(node.id.clone(), *recorded);
            }
        }
    }

    // Repeat until nothing changes: a gate propagates along dependency edges,
    // and the plan does not promise a topological node order.
    loop {
        let mut changed = false;
        for node in graph.iter() {
            if statuses.contains_key(&node.id) {
                continue;
            }
            let Some(status) = eligibility(graph, node, &statuses, resolved_cross_dag) else {
                continue;
            };
            statuses.insert(node.id.clone(), status);
            changed = true;
        }
        if !changed {
            break;
        }
    }

    // Anything still unresolved has a dependency that has not settled.
    for node in graph.iter() {
        statuses
            .entry(node.id.clone())
            .or_insert(NodeStatus::Pending);
    }
    statuses
}

fn eligibility(
    graph: &Graph,
    node: &Node,
    statuses: &BTreeMap<String, NodeStatus>,
    resolved_cross_dag: &dyn Fn(&str) -> Option<NodeStatus>,
) -> Option<NodeStatus> {
    let mut all_done = true;
    let mut failed = false;
    let mut gated = false;

    for dep in &node.deps {
        let status = if is_cross_dag(dep) {
            // An unknown or unfinished upstream leaves the consumer blocked
            // rather than failing it: the upstream may still arrive.
            match resolved_cross_dag(dep) {
                Some(status) => status,
                None => {
                    all_done = false;
                    gated = true;
                    continue;
                }
            }
        } else if graph.contains(dep) {
            match statuses.get(dep) {
                Some(status) => *status,
                None => {
                    all_done = false;
                    continue;
                }
            }
        } else {
            // A dependency the graph no longer holds was detached by a `drop`.
            continue;
        };

        match status {
            NodeStatus::Done => {}
            _ if skips_dependents(status) => {
                failed = true;
                all_done = false;
            }
            NodeStatus::Waiting
            | NodeStatus::Blocked
            | NodeStatus::Parked
            | NodeStatus::Cancelled => {
                gated = true;
                all_done = false;
            }
            _ => all_done = false,
        }
    }

    // Failure takes precedence over a simultaneous waiting path: such a
    // descendant is skipped, not blocked.
    if failed {
        return Some(NodeStatus::Skipped);
    }
    if gated {
        return Some(NodeStatus::Blocked);
    }
    if !all_done {
        return None;
    }
    if node.parked {
        return Some(NodeStatus::Parked);
    }
    if node.kind == NodeKind::Human {
        return Some(NodeStatus::Waiting);
    }
    Some(NodeStatus::Ready)
}

/// Whether a dependency in this state makes executing its dependents unsafe.
///
/// One predicate for both halves of a skip — [`eligibility`] derives it,
/// [`skipped_by`] names what answered it — so a run cannot report a skip whose
/// cause it disagrees with.
fn skips_dependents(status: NodeStatus) -> bool {
    matches!(status, NodeStatus::Failed | NodeStatus::Skipped)
}

/// The dependencies whose own failure or skip is why `id` derived
/// [`Skipped`](NodeStatus::Skipped), in the order the node declared them.
///
/// `statuses` must be a [`derive`] fixpoint: the cause is re-derived out of the
/// map the skip itself came from. Empty unless `id` is skipped — a park outranks
/// the gates, so a parked node with a failed dependency is not being held by it.
/// A detached edge and a cross-DAG upstream have no status here and cannot cause
/// a skip either.
pub fn skipped_by(
    graph: &Graph,
    statuses: &BTreeMap<String, NodeStatus>,
    id: &str,
) -> Vec<(String, NodeStatus)> {
    if statuses.get(id) != Some(&NodeStatus::Skipped) {
        return Vec::new();
    }
    let Some(node) = graph.get(id) else {
        return Vec::new();
    };
    node.deps
        .iter()
        .filter_map(|dep| {
            let status = *statuses.get(dep)?;
            skips_dependents(status).then(|| (dep.clone(), status))
        })
        .collect()
}

/// How a graph with these statuses settled.
pub fn state_of(statuses: &BTreeMap<String, NodeStatus>) -> GraphState {
    if statuses
        .values()
        .any(|s| matches!(s, NodeStatus::Failed | NodeStatus::Skipped))
    {
        GraphState::Failed
    } else if statuses.values().any(|s| {
        matches!(
            s,
            NodeStatus::Waiting | NodeStatus::Blocked | NodeStatus::Parked | NodeStatus::Cancelled
        )
    }) {
        GraphState::Waiting
    } else if statuses.values().all(|s| *s == NodeStatus::Done) {
        GraphState::Complete
    } else {
        GraphState::Waiting
    }
}

/// Whether every node has settled, so the loop has nothing left to converge.
pub fn is_terminal(statuses: &BTreeMap<String, NodeStatus>) -> bool {
    statuses.values().all(|s| s.is_settled())
}

/// What each ready human action unblocks, for the view that reports it.
pub fn unblocks(graph: &Graph, id: &str) -> Vec<String> {
    graph.dependents_of(id)
}

#[cfg(test)]
mod tests {
    /// Every status a node can settle or wait at, for the gate below.
    ///
    /// **Walked rather than written out.** A list written out is one a new variant
    /// never has to join: [`NodeStatus::as_str`] and [`NodeStatus::parse`] fail to
    /// compile until the variant is spelled in each, and neither of those edits
    /// touches a list beside them — so a status this build carried could reach the
    /// divergence gate below unnamed, which is the one thing that gate exists to
    /// catch. The walk is an exhaustive `match` over the enum itself, so the
    /// variant has to be named *here* too, and the only answer its arm can give is
    /// which status comes after it.
    ///
    /// One thing exhaustiveness cannot make somebody do is point an *existing* arm
    /// at the new variant, so an arm written `=> None` would end the walk a second
    /// time and leave its own status unreached. Nothing in stable Rust counts an
    /// enum's variants, so that end is closed by the arms reading as an order —
    /// each names the one after it and exactly one names none — and by the
    /// assertion below, which refuses a walk that comes back to a status it has
    /// already reached. A second list written out beside this one would close
    /// nothing: it would be one more thing a new variant does not have to join.
    fn every_status() -> Vec<NodeStatus> {
        fn after(status: NodeStatus) -> Option<NodeStatus> {
            match status {
                NodeStatus::Pending => Some(NodeStatus::Ready),
                NodeStatus::Ready => Some(NodeStatus::Running),
                NodeStatus::Running => Some(NodeStatus::Waiting),
                NodeStatus::Waiting => Some(NodeStatus::Blocked),
                NodeStatus::Blocked => Some(NodeStatus::Parked),
                NodeStatus::Parked => Some(NodeStatus::Cancelled),
                NodeStatus::Cancelled => Some(NodeStatus::Done),
                NodeStatus::Done => Some(NodeStatus::CompleteDraft),
                NodeStatus::CompleteDraft => Some(NodeStatus::Failed),
                NodeStatus::Failed => Some(NodeStatus::Skipped),
                NodeStatus::Skipped => None,
            }
        }
        let mut every = vec![NodeStatus::Pending];
        while let Some(next) = after(*every.last().expect("the walk starts at one status")) {
            assert!(
                !every.contains(&next),
                "the walk over NodeStatus reaches {next:?} twice, so it names no order and \
                 whatever follows it is never reached"
            );
            every.push(next);
        }
        every
    }

    /// The draft settlement vocabulary this build carries is exactly what the
    /// divergence record proposes, and `docs/contract.md` names none of it.
    ///
    /// A node status and a publication outcome, and both are private vocabulary —
    /// `graph` and `vcs` are engine modules, so `tests/contract.rs`, which drives
    /// the published surface, cannot reach either and entry 51 is the only place
    /// they are written down. Held both directions, and against the contract as
    /// well: a word this build grows without a line in that entry fails here, one
    /// the entry names that this build no longer spells fails here, and one the
    /// approved contract has since taken up is no divergence and fails here too.
    #[test]
    fn the_draft_settlement_vocabulary_is_what_the_divergence_record_names() {
        let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
        let record = std::fs::read_to_string(docs.join("contract-divergences.md"))
            .expect("the divergence record ships");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with("51."))
            .expect("the record still carries entry 51");
        let block: serde_json::Value = entry
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .and_then(|block| serde_json::from_str(block).ok())
            .expect("entry 51 carries the json block this test drives");
        let contract =
            std::fs::read_to_string(docs.join("contract.md")).expect("the contract ships");

        let statuses: Vec<String> = serde_json::from_value(block["node_statuses"].clone())
            .expect("entry 51 names the node statuses it adds");
        // Every status this build spells that the contract does not mention **at
        // all** — read off the enum through the same `parse`/`as_str` pair the
        // journal is written and read back with, so a word only one of the two
        // knows fails here. The contract writes most of these in prose rather
        // than in backticks (`a ready human action`, `blocked, never failed`), so
        // the search is for the word and not for a rendering of it: what is being
        // asked is whether the document has ever heard of it.
        let undocumented: Vec<String> = every_status()
            .iter()
            .map(|status| status.as_str().to_string())
            .filter(|word| !contract.contains(word.as_str()))
            .collect();
        assert_eq!(
            undocumented, statuses,
            "the statuses this build carries that docs/contract.md does not name are not entry \
             51's"
        );
        for word in &statuses {
            assert_eq!(
                NodeStatus::parse(word).map(NodeStatus::as_str),
                Some(word.as_str()),
                "`{word}` does not round-trip through the status this build writes"
            );
        }

        let outcomes: Vec<String> = serde_json::from_value(block["outcomes"].clone())
            .expect("entry 51 names the outcome it adds");
        assert_eq!(
            outcomes,
            vec![crate::vcs::DRAFTED.to_string()],
            "entry 51 names a different outcome than a drafted publication settles on"
        );
        for word in &outcomes {
            assert!(
                !contract.contains(word.as_str()),
                "the contract names `{word}`, so it is no divergence"
            );
        }
    }
    use super::*;
    use crate::plan::{Goal, PLAN_SCHEMA_VERSION};

    /// A node identity is what a node has, and nothing else can be made into
    /// one.
    ///
    /// Both directions, because the type is only worth having if the second one
    /// holds: a node the graph would carry yields its identity, and one whose id
    /// `validate` would refuse yields nothing to carry.
    #[test]
    fn a_node_identity_comes_from_a_node_and_a_blank_id_is_not_one() {
        let named = Node {
            id: "  service  ".into(),
            ..Node::default()
        };
        assert_eq!(
            NodeRef::of(&named).as_ref().map(NodeRef::as_str),
            Some("service"),
            "an identity is the node's own id, trimmed"
        );
        for blank in ["", "   ", "\t\n"] {
            let unnamed = Node {
                id: blank.into(),
                ..Node::default()
            };
            assert_eq!(
                NodeRef::of(&unnamed),
                None,
                "a node with no id yielded an identity: {blank:?}"
            );
        }
        // And the same refusal, from the check this type carries past:
        // `validate` is where a blank id is turned away in the first place.
        let plan = Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            name: Some("blank".into()),
            concurrency: 1,
            goal: None,
            tasks: vec![Node {
                id: " ".into(),
                task: Some("## What\nwork".into()),
                persona: Some("engineer".into()),
                ..Node::default()
            }],
        };
        assert!(validate(&plan).is_err(), "a blank node id was accepted");
    }

    /// The journal writes these words through `as_str` and the run's ledger
    /// writes them through serde. Two spellings of one vocabulary is exactly how
    /// a projection quietly stops recognising what the ledger recorded, so the
    /// two are held equal here rather than by eye.
    #[test]
    fn a_status_serialises_as_the_word_it_is_written_and_read_as() {
        for status in [
            NodeStatus::Pending,
            NodeStatus::Ready,
            NodeStatus::Running,
            NodeStatus::Waiting,
            NodeStatus::Blocked,
            NodeStatus::Parked,
            NodeStatus::Cancelled,
            NodeStatus::Done,
            NodeStatus::Failed,
            NodeStatus::Skipped,
        ] {
            let json = serde_json::to_string(&status).expect("a status serialises");
            assert_eq!(json, format!("\"{}\"", status.as_str()));
            assert_eq!(NodeStatus::parse(status.as_str()), Some(status));
            assert_eq!(
                serde_json::from_str::<NodeStatus>(&json).expect("it reads back"),
                status
            );
        }
    }

    #[test]
    fn a_graph_state_serialises_as_the_word_it_is_rendered_as() {
        for state in [
            GraphState::Complete,
            GraphState::Waiting,
            GraphState::Failed,
        ] {
            let json = serde_json::to_string(&state).expect("a state serialises");
            assert_eq!(json, format!("\"{}\"", state.as_str()));
            assert_eq!(
                serde_json::from_str::<GraphState>(&json).expect("it reads back"),
                state
            );
        }
    }

    fn agent(id: &str, deps: &[&str]) -> Node {
        Node {
            id: id.into(),
            persona: Some("engineer".into()),
            task: Some("## What\ndo it".into()),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
            ..Node::default()
        }
    }

    fn human(id: &str, deps: &[&str]) -> Node {
        Node {
            id: id.into(),
            kind: NodeKind::Human,
            task: Some("approve it".into()),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
            ..Node::default()
        }
    }

    fn plan_of(tasks: Vec<Node>) -> Plan {
        Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            goal: None,
            name: Some("test".into()),
            concurrency: 4,
            tasks,
        }
    }

    fn no_cross_dag(_: &str) -> Option<NodeStatus> {
        None
    }

    #[test]
    fn a_legal_mixed_plan_validates() {
        let plan = plan_of(vec![
            agent("build", &[]),
            human("approve", &["build"]),
            Node {
                id: "publish".into(),
                repo: Some("owner/repo".into()),
                persona: Some("engineer".into()),
                task: Some("## What\nship".into()),
                title: Some("feat: ship it".into()),
                deps: vec!["approve".into()],
                ..Node::default()
            },
        ]);
        validate(&plan).expect("the plan is legal");
    }

    #[test]
    fn a_self_edge_and_a_cycle_are_both_refused() {
        let mut plan = plan_of(vec![agent("a", &["a"])]);
        let message = validate(&plan).unwrap_err().to_string();
        assert!(message.contains("depends on itself"), "{message}");

        plan = plan_of(vec![agent("a", &["b"]), agent("b", &["a"])]);
        let message = validate(&plan).unwrap_err().to_string();
        assert!(message.contains("cycle"), "{message}");
    }

    #[test]
    fn a_deep_chain_does_not_overflow_the_cycle_walk() {
        let mut tasks: Vec<Node> = Vec::new();
        for index in 0..20_000u32 {
            let deps: Vec<&str> = Vec::new();
            let mut node = agent(&format!("n{index}"), &deps);
            if index > 0 {
                node.deps = vec![format!("n{}", index - 1)];
            }
            tasks.push(node);
        }
        validate(&plan_of(tasks)).expect("a 20k-node chain is legal");
    }

    #[test]
    fn a_dangling_dependency_is_refused_but_a_cross_dag_one_is_not() {
        let plan = plan_of(vec![agent("a", &["nowhere"])]);
        let message = validate(&plan).unwrap_err().to_string();
        assert!(message.contains("not in the plan"), "{message}");

        let plan = plan_of(vec![agent("a", &["run:other#build"])]);
        validate(&plan).expect("a cross-DAG reference is not a missing node");
    }

    #[test]
    fn every_node_shape_rule_the_contract_states_is_enforced() {
        let cases: &[(Node, &str)] = &[
            (
                Node {
                    id: "no-persona".into(),
                    task: Some("t".into()),
                    ..Node::default()
                },
                "needs a persona",
            ),
            (
                Node {
                    id: "with/slash".into(),
                    kind: NodeKind::Human,
                    task: Some("t".into()),
                    ..Node::default()
                },
                "cannot contain '/'",
            ),
            (
                Node {
                    id: "human-persona".into(),
                    kind: NodeKind::Human,
                    task: Some("t".into()),
                    persona: Some("engineer".into()),
                    ..Node::default()
                },
                "no persona or turn budget",
            ),
            (
                Node {
                    id: "human-context".into(),
                    kind: NodeKind::Human,
                    task: Some("t".into()),
                    context: Some("a note".into()),
                    ..Node::default()
                },
                "has none",
            ),
            (
                Node {
                    id: "nodiff-persona".into(),
                    expects_no_diff: true,
                    task: Some("t".into()),
                    persona: Some("engineer".into()),
                    ..Node::default()
                },
                "takes no persona or turn budget",
            ),
            (
                Node {
                    id: "steps-no-repo".into(),
                    steps: Some(vec![Step {
                        id: "one".into(),
                        persona: Some("engineer".into()),
                        task: Some("t".into()),
                        ..Step::default()
                    }]),
                    ..Node::default()
                },
                "need a repo",
            ),
            (
                Node {
                    id: "steps-and-task".into(),
                    repo: Some("o/r".into()),
                    task: Some("t".into()),
                    steps: Some(vec![Step {
                        id: "one".into(),
                        persona: Some("engineer".into()),
                        task: Some("t".into()),
                        ..Step::default()
                    }]),
                    ..Node::default()
                },
                "takes its persona, task, and turn budget from them",
            ),
            (
                Node {
                    id: "step-no-persona".into(),
                    repo: Some("o/r".into()),
                    steps: Some(vec![Step {
                        id: "one".into(),
                        task: Some("t".into()),
                        ..Step::default()
                    }]),
                    ..Node::default()
                },
                "needs a persona",
            ),
            (
                Node {
                    id: "human-budget".into(),
                    kind: NodeKind::Human,
                    task: Some("t".into()),
                    max_turns: Some(45),
                    ..Node::default()
                },
                "no persona or turn budget",
            ),
            (
                Node {
                    id: "nodiff-budget".into(),
                    expects_no_diff: true,
                    task: Some("t".into()),
                    max_turns: Some(45),
                    ..Node::default()
                },
                "takes no persona or turn budget",
            ),
            (
                Node {
                    id: "steps-and-budget".into(),
                    repo: Some("o/r".into()),
                    max_turns: Some(45),
                    steps: Some(vec![Step {
                        id: "one".into(),
                        persona: Some("engineer".into()),
                        task: Some("t".into()),
                        ..Step::default()
                    }]),
                    ..Node::default()
                },
                "takes its persona, task, and turn budget from them",
            ),
            (
                Node {
                    id: "human-step-budget".into(),
                    repo: Some("o/r".into()),
                    steps: Some(vec![Step {
                        id: "sign-off".into(),
                        kind: NodeKind::Human,
                        task: Some("t".into()),
                        max_turns: Some(45),
                        ..Step::default()
                    }]),
                    ..Node::default()
                },
                "no persona, turn budget, or expects_no_diff",
            ),
            (
                Node {
                    id: "step-cycle".into(),
                    repo: Some("o/r".into()),
                    steps: Some(vec![Step {
                        id: "one".into(),
                        persona: Some("engineer".into()),
                        task: Some("t".into()),
                        deps: vec!["one".into()],
                        ..Step::default()
                    }]),
                    ..Node::default()
                },
                "depends on itself",
            ),
        ];
        for (node, expected) in cases {
            let message = validate_node(node).unwrap_err().to_string();
            assert!(
                message.contains(expected),
                "node '{}': expected {expected:?} in {message:?}",
                node.id
            );
        }
    }

    /// A title `onevcs` will refuse at publication is refused at the plan's own
    /// boundary.
    ///
    /// The lengths are built from [`SUBJECT_LIMIT`], never written out: a number
    /// spelled here would be a second copy of a bound this crate does not own.
    #[test]
    fn a_title_the_publication_would_refuse_is_refused_before_anything_is_dispatched() {
        let titled = |title: &str| Node {
            title: Some(title.to_owned()),
            repo: Some("owner/repo".into()),
            ..agent("publish", &[])
        };

        let over = "t".repeat(SUBJECT_LIMIT + 1);
        let message = validate(&plan_of(vec![titled(&over)]))
            .unwrap_err()
            .to_string();
        assert!(message.contains("node 'publish'"), "{message}");
        assert!(
            message.contains(&format!("{} characters", SUBJECT_LIMIT + 1)),
            "the refusal does not say how long the title is: {message}"
        );
        assert!(
            message.contains(&format!("{SUBJECT_LIMIT}-character limit")),
            "the refusal does not name the limit: {message}"
        );

        validate(&plan_of(vec![titled(&"t".repeat(SUBJECT_LIMIT))]))
            .expect("a title at the limit is publishable, so the plan is legal");

        validate(&plan_of(vec![titled(&format!(
            "  {}  ",
            "t".repeat(SUBJECT_LIMIT)
        ))]))
        .expect("the surrounding spacing was counted against the limit");

        let message = validate(&plan_of(vec![titled("   ")]))
            .unwrap_err()
            .to_string();
        assert!(message.contains("node 'publish'"), "{message}");
        assert!(message.contains("blank"), "{message}");

        // A node that names no repository publishes nothing, so its title is
        // read by no one and is held to no publication's rule.
        validate(&plan_of(vec![Node {
            title: Some(over),
            ..agent("direct", &[])
        }]))
        .expect("a title on a node that never publishes was held to the publication's limit");
    }

    /// An ordinary title takes the path where this check does nothing at all.
    ///
    /// Every length above is built from [`SUBJECT_LIMIT`] and moves with it,
    /// which proves the arithmetic at the edge and only there. This one is
    /// deliberately *not* derived from the bound: it is the length a planner
    /// actually writes, well inside the limit, and the case that would catch a
    /// check that refused far more than the publication does.
    #[test]
    fn a_title_a_planner_would_actually_write_is_left_alone() {
        // 100 characters — a real subject, not a fixture built out of the bound.
        let title = "feat(plan): refuse a node title that the publication would not commit under, \
                     before it is dispatched";
        assert!(
            title.len() < SUBJECT_LIMIT,
            "this is only an ordinary title while it is inside the bound: {} characters",
            title.len()
        );

        validate(&plan_of(vec![Node {
            title: Some(title.to_owned()),
            repo: Some("owner/repo".into()),
            ..agent("publish", &[])
        }]))
        .expect("an ordinary title was refused");
    }

    /// Every version this build reads validates, and a number outside that set
    /// is refused by it.
    ///
    /// A plan written at an earlier version has nothing to migrate: what each
    /// later version added is keyed to the version the document declares, so the
    /// graph it describes is one this engine executes. A number this crate has
    /// never written is the only version refusal there is, and it names the ones
    /// that are read rather than leaving its author to guess.
    #[test]
    fn every_version_this_build_reads_validates_and_no_other_does() {
        for version in crate::plan::PLAN_SCHEMA_VERSIONS_READ {
            let mut plan = plan_of(vec![agent("a", &[])]);
            plan.schema_version = version;
            validate(&plan)
                .unwrap_or_else(|why| panic!("a version {version} plan is a document: {why}"));
        }

        let mut unknown = plan_of(vec![agent("a", &[])]);
        unknown.schema_version = 99;
        let message = validate(&unknown).unwrap_err().to_string();
        assert!(message.contains("schema_version 99"), "{message}");
        for version in crate::plan::PLAN_SCHEMA_VERSIONS_READ {
            assert!(
                message.contains(&version.to_string()),
                "the refusal does not name version {version}, which this build reads: {message}"
            );
        }
    }

    #[test]
    fn the_plan_envelope_itself_is_validated() {
        let mut plan = plan_of(vec![agent("a", &[])]);
        plan.schema_version = 99;
        assert!(validate(&plan)
            .unwrap_err()
            .to_string()
            .contains("schema_version"));

        let mut plan = plan_of(vec![agent("a", &[])]);
        plan.concurrency = 0;
        assert!(validate(&plan)
            .unwrap_err()
            .to_string()
            .contains("concurrency"));

        let mut plan = plan_of(vec![]);
        plan.tasks = vec![];
        assert!(validate(&plan)
            .unwrap_err()
            .to_string()
            .contains("at least one node"));

        let mut plan = plan_of(vec![agent("a", &[]), agent("a", &[])]);
        plan.tasks[1].id = "a".into();
        assert!(validate(&plan)
            .unwrap_err()
            .to_string()
            .contains("duplicate"));

        let mut plan = plan_of(vec![agent("a", &[])]);
        plan.goal = Some(Goal { text: "  ".into() });
        assert!(validate(&plan)
            .unwrap_err()
            .to_string()
            .contains("non-empty text"));
    }

    #[test]
    fn a_ready_frontier_is_everything_whose_dependencies_are_done() {
        let graph = Graph::from_plan(&plan_of(vec![
            agent("a", &[]),
            agent("b", &[]),
            agent("c", &["a", "b"]),
        ]));
        let mut recorded = BTreeMap::new();
        recorded.insert("a".to_string(), NodeStatus::Done);

        let statuses = derive(&graph, &recorded, &no_cross_dag);
        assert_eq!(statuses["a"], NodeStatus::Done);
        assert_eq!(statuses["b"], NodeStatus::Ready);
        assert_eq!(statuses["c"], NodeStatus::Pending);
    }

    #[test]
    fn a_waiting_human_blocks_and_a_failure_skips_the_same_descendant() {
        let graph = Graph::from_plan(&plan_of(vec![
            agent("build", &[]),
            human("approve", &["build"]),
            agent("ship", &["approve"]),
        ]));
        let mut recorded = BTreeMap::new();
        recorded.insert("build".to_string(), NodeStatus::Done);
        let statuses = derive(&graph, &recorded, &no_cross_dag);
        assert_eq!(statuses["approve"], NodeStatus::Waiting);
        assert_eq!(statuses["ship"], NodeStatus::Blocked);
        assert_eq!(state_of(&statuses), GraphState::Waiting);

        // Failure takes precedence over a simultaneous waiting path.
        let graph = Graph::from_plan(&plan_of(vec![
            agent("build", &[]),
            human("approve", &[]),
            agent("ship", &["approve", "build"]),
        ]));
        let mut recorded = BTreeMap::new();
        recorded.insert("build".to_string(), NodeStatus::Failed);
        let statuses = derive(&graph, &recorded, &no_cross_dag);
        assert_eq!(statuses["approve"], NodeStatus::Waiting);
        assert_eq!(statuses["ship"], NodeStatus::Skipped);
        assert_eq!(state_of(&statuses), GraphState::Failed);
    }

    #[test]
    fn a_parked_node_blocks_its_dependents_rather_than_skipping_them() {
        let mut parked = agent("sweep", &[]);
        parked.parked = true;
        let graph = Graph::from_plan(&plan_of(vec![parked, agent("after", &["sweep"])]));
        let statuses = derive(&graph, &BTreeMap::new(), &no_cross_dag);
        assert_eq!(statuses["sweep"], NodeStatus::Parked);
        assert_eq!(statuses["after"], NodeStatus::Blocked);
        assert_eq!(state_of(&statuses), GraphState::Waiting);
        assert!(is_terminal(&statuses));
    }

    #[test]
    fn a_recorded_gate_is_discarded_and_re_derived() {
        // The journal recorded `blocked` while a human waited. The human has
        // since been attested, so the same node must re-derive to ready.
        let graph = Graph::from_plan(&plan_of(vec![
            human("approve", &[]),
            agent("ship", &["approve"]),
        ]));
        let mut recorded = BTreeMap::new();
        recorded.insert("ship".to_string(), NodeStatus::Blocked);
        recorded.insert("approve".to_string(), NodeStatus::Done);
        let statuses = derive(&graph, &recorded, &no_cross_dag);
        assert_eq!(statuses["ship"], NodeStatus::Ready);
    }

    #[test]
    fn an_unresolved_cross_dag_reference_blocks_its_consumer() {
        let graph = Graph::from_plan(&plan_of(vec![agent("consume", &["run:other#build"])]));
        let statuses = derive(&graph, &BTreeMap::new(), &no_cross_dag);
        assert_eq!(statuses["consume"], NodeStatus::Blocked);

        let done = |_: &str| Some(NodeStatus::Done);
        let statuses = derive(&graph, &BTreeMap::new(), &done);
        assert_eq!(statuses["consume"], NodeStatus::Ready);

        let failed = |_: &str| Some(NodeStatus::Failed);
        let statuses = derive(&graph, &BTreeMap::new(), &failed);
        assert_eq!(statuses["consume"], NodeStatus::Skipped);
    }

    /// A derived skip and its named cause are one answer, so they are held to
    /// the same map here: whatever `derive` settled on, `skipped_by` names the
    /// dependencies that made it say so.
    #[test]
    fn every_skipped_node_names_the_dependencies_that_skipped_it() {
        let graph = Graph::from_plan(&plan_of(vec![
            agent("build", &[]),
            agent("lint", &[]),
            agent("ship", &["build", "lint"]),
            agent("announce", &["ship"]),
        ]));
        let mut recorded = BTreeMap::new();
        recorded.insert("build".to_string(), NodeStatus::Failed);
        let statuses = derive(&graph, &recorded, &no_cross_dag);

        assert_eq!(statuses["ship"], NodeStatus::Skipped);
        assert_eq!(statuses["announce"], NodeStatus::Skipped);
        assert_eq!(
            skipped_by(&graph, &statuses, "ship"),
            vec![("build".to_string(), NodeStatus::Failed)]
        );
        assert_eq!(
            skipped_by(&graph, &statuses, "announce"),
            vec![("ship".to_string(), NodeStatus::Skipped)]
        );
        assert!(skipped_by(&graph, &statuses, "lint").is_empty());
        assert!(skipped_by(&graph, &statuses, "nowhere").is_empty());

        // A park outranks the derived gates, so this node is not skipped — and
        // is handed no reason for a skip it is not being held by.
        let mut parked = agent("sweep", &["build"]);
        parked.parked = true;
        let graph = Graph::from_plan(&plan_of(vec![agent("build", &[]), parked]));
        let statuses = derive(&graph, &recorded, &no_cross_dag);
        assert_eq!(statuses["sweep"], NodeStatus::Parked);
        assert!(skipped_by(&graph, &statuses, "sweep").is_empty());
    }

    #[test]
    fn a_dropped_dependency_detaches_rather_than_blocking() {
        let mut graph = Graph::from_plan(&plan_of(vec![agent("a", &[]), agent("b", &["a"])]));
        graph.remove("a");
        let statuses = derive(&graph, &BTreeMap::new(), &no_cross_dag);
        assert_eq!(statuses["b"], NodeStatus::Ready);
    }

    #[test]
    fn a_complete_graph_is_complete_and_exits_zero() {
        let graph = Graph::from_plan(&plan_of(vec![agent("a", &[])]));
        let mut recorded = BTreeMap::new();
        recorded.insert("a".to_string(), NodeStatus::Done);
        let statuses = derive(&graph, &recorded, &no_cross_dag);
        assert_eq!(state_of(&statuses), GraphState::Complete);
        assert_eq!(GraphState::Complete.exit_code(), 0);
        assert_eq!(GraphState::Waiting.exit_code(), 1);
        assert_eq!(GraphState::Failed.exit_code(), 1);
    }

    #[test]
    fn a_graph_still_running_has_not_settled() {
        let graph = Graph::from_plan(&plan_of(vec![agent("a", &[])]));
        let mut recorded = BTreeMap::new();
        recorded.insert("a".to_string(), NodeStatus::Running);
        let statuses = derive(&graph, &recorded, &no_cross_dag);
        assert!(!is_terminal(&statuses));
        assert_eq!(state_of(&statuses), GraphState::Waiting);
    }

    #[test]
    fn statuses_round_trip_through_their_written_word() {
        for status in [
            NodeStatus::Pending,
            NodeStatus::Ready,
            NodeStatus::Running,
            NodeStatus::Waiting,
            NodeStatus::Blocked,
            NodeStatus::Parked,
            NodeStatus::Cancelled,
            NodeStatus::Done,
            NodeStatus::Failed,
            NodeStatus::Skipped,
        ] {
            assert_eq!(NodeStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(NodeStatus::parse("invented"), None);
    }

    /// A landing round-trips through its word, spells it the same way on both
    /// wires, and reads an unknown one as no observation rather than a guess.
    ///
    /// Three halves, and the middle one is a drift gate. A landing reaches a
    /// reader by two routes that are written independently: `serde` puts it in
    /// the run's `result.json`, and [`Landing::as_str`] puts it in the journal
    /// payload the views fold. Nothing but this makes the two spellings agree, and
    /// a run whose result file and whose ledger disagree about a word is a run
    /// nobody can reconcile.
    ///
    /// The third is compatibility: a build meeting a landing it does not know has
    /// to answer "this run observed nothing about where that change got to",
    /// because the other two answers are the two false reports this qualifier was
    /// added to stop.
    #[test]
    fn a_landing_round_trips_through_its_word_and_an_unknown_one_is_no_landing() {
        for landing in [Landing::Landed, Landing::Unlanded] {
            assert_eq!(Landing::parse(landing.as_str()), Some(landing));
            // The serialized spelling *is* the rendered one, both ways.
            assert_eq!(
                serde_json::to_value(landing).expect("a landing serialises"),
                serde_json::Value::String(landing.as_str().to_string()),
                "`serde` and `as_str` disagree about how to spell {landing:?}"
            );
            assert_eq!(
                serde_json::from_value::<Landing>(serde_json::json!(landing.as_str()))
                    .expect("the rendered word reads back"),
                landing
            );
        }
        for unreadable in ["", "invented", "done", "merged", "Landed"] {
            assert_eq!(
                Landing::parse(unreadable),
                None,
                "{unreadable:?} was read as a landing this build understands"
            );
        }
        // And it is not a status: a reader handed one of these words where the
        // other belongs gets nothing, rather than the neighbouring meaning.
        assert_eq!(NodeStatus::parse(Landing::Landed.as_str()), None);
        assert_eq!(Landing::parse(NodeStatus::Done.as_str()), None);
    }

    #[test]
    fn a_graph_keeps_the_order_the_plan_wrote_and_reports_dependents() {
        let mut graph = Graph::from_plan(&plan_of(vec![
            agent("first", &[]),
            agent("second", &["first"]),
        ]));
        graph.insert(agent("third", &["first"]));
        assert_eq!(
            graph.ids().cloned().collect::<Vec<_>>(),
            vec!["first", "second", "third"]
        );
        assert_eq!(unblocks(&graph, "first"), vec!["second", "third"]);
        assert_eq!(graph.len(), 3);
        assert!(!graph.is_empty());

        // Replacing a node in place keeps its position.
        graph.insert(agent("second", &[]));
        assert_eq!(
            graph.ids().cloned().collect::<Vec<_>>(),
            vec!["first", "second", "third"]
        );
        assert!(graph.get_mut("second").is_some());
        assert_eq!(graph.remove("second").map(|n| n.id), Some("second".into()));
        assert!(!graph.contains("second"));
        assert!(Graph::default().is_empty());
    }

    /// `consumes` is keyed by **dependency node id**, so a key naming nothing this
    /// node depends on is refused where the plan is read.
    ///
    /// Silently dropping it is the failure this exists to remove: a planner who
    /// wrote a release target and had it ignored would find out from a node that
    /// launched against the wrong artifact, long after the plan loaded.
    #[test]
    fn consumes_naming_something_this_node_does_not_depend_on_is_refused() {
        let named = |on: &str| {
            let mut node = agent("consumer", &["engine"]);
            node.consumes.insert(
                on.to_string(),
                "crate".parse().expect("a release target name"),
            );
            plan_of(vec![agent("engine", &[]), node])
        };
        let refusal = validate(&named("packager")).unwrap_err().to_string();
        assert!(
            refusal.contains("node 'consumer'")
                && refusal.contains("`consumes` names 'packager'")
                && refusal.contains("not one of this node's deps"),
            "{refusal}"
        );
        // The dependency it does name is legal, and so is a cross-DAG one.
        validate(&named("engine")).expect("a target for a dependency it has");
        let mut across = agent("consumer", &["run:other#upstream"]);
        across.consumes.insert(
            "run:other#upstream".to_string(),
            "crate".parse().expect("a release target name"),
        );
        validate(&plan_of(vec![across])).expect("a target for a cross-DAG dependency");
    }

    #[test]
    fn a_graph_renders_back_as_the_plan_it_came_from() {
        let source = plan_of(vec![agent("a", &[])]);
        let graph = Graph::from_plan(&source);
        let round_trip = graph.to_plan(&source);
        assert_eq!(round_trip, source);
    }
}
