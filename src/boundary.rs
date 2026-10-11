//! The repository publication boundary: which nodes are public, which private, and the
//! one way private work may reach a public one.
//!
//! A node's **clearance** is the stricter of its task's classification and the
//! effective visibility `onevcs` holds for every repository it names — its `repo` and
//! its record's `repositories` — so `unknown` and local-only repositories, and ones this
//! host cannot resolve at all, are private. A node that names no repository takes the
//! clearance of what it depends on: it is private where any of its dependencies is, and
//! public otherwise. Nothing downgrades a clearance: a public classification never makes
//! a private repository's node public.
//!
//! A plan is **mixed** when it holds a private node and a public node that names a
//! repository — somewhere public its work could land. Only a mixed plan pays anything
//! here: a single-visibility plan has no boundary context, no extra prompt text, and
//! its graph is judged exactly as it always was.
//!
//! In a mixed plan an ordinary dependency edge from a private node to a public one is
//! refused, at load and at every live edit. The one way across is an **export edge**
//! (`onepipeline.export_edges`): a private `publish: "preserve"` producer generalizes
//! its work into one neutral directory, the engine preserves it and asks `onevcs` to
//! export that directory as one neutral commit into the consumer's repository, and the
//! public consumer's session starts from that commit. Every refusal here carries the
//! same engine-owned [`Guidance`] the host renders for its planner, so the advice an
//! agent reads is written once.
//!
//! The run's **term scope** is the set of private repositories its plan names, as
//! `onevcs` identity keys. It is what every public boundary check the engine asks on the
//! run's behalf derives its terms from, and an empty one derives nothing.

use std::collections::{BTreeMap, BTreeSet};

use onetaskgraph_plugin_api::Classification;
use serde::{Deserialize, Serialize};

use crate::plan::{ExportEdge, Node, Plan, Publish};
use crate::refusal::Refusal;

/// Which side of the boundary a node's work is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Clearance {
    /// Nothing about it is private.
    Public,
    /// Something about it is: its task, or a repository it names.
    Private,
}

/// What a repository spelling resolves to, as far as the boundary is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Repository {
    /// Which side its effective visibility puts it on: only a verified public repository
    /// is public.
    pub(crate) clearance: Clearance,
    /// What a term scope names it by.
    pub(crate) identity: Identity,
}

/// How a term scope names a repository.
// llmlint: ignore-block[invalid_states_unrepresentable] each is the string `onevcs` itself
// takes a scope as — `TermScope::Identities(Vec<String>)` — and the boundary is the
// sibling's: it normalizes a key, refuses one it has not registered as unavailable, and
// names neither, because the name may be private. A newtype here could hold no invariant
// the sibling does not check at the one place the scope is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Identity {
    /// The `onevcs` identity key the host resolved it to.
    Registered(String),
    /// The spelling a node named, which the host could not resolve: a scoped check names
    /// it as it is, and answers it unavailable rather than passing.
    Unresolved(String),
}

// llmlint: ignore-end[invalid_states_unrepresentable]

impl Identity {
    /// The word a term scope carries for it.
    fn into_scope(self) -> String {
        match self {
            Self::Registered(key) | Self::Unresolved(key) => key,
        }
    }
}

/// Where the boundary learns about repositories and terms.
///
/// The engine asks `onevcs` ([`Onevcs`]); a unit test answers from a table.
pub(crate) trait Oracle {
    /// What `repo` is.
    fn repository(&mut self, repo: &str) -> Repository;

    /// Whether `input` may be written somewhere public, as the term check answers.
    fn check(&mut self, input: onevcs::BoundaryInput) -> onevcs::BoundaryVerdict;
}

/// The linked `onevcs`: visibility refreshed at the boundary, identity from the
/// registry, and the one term check every public write is put through.
#[derive(Debug, Default)]
pub(crate) struct Onevcs {
    known: BTreeMap<String, Repository>,
}

impl Oracle for Onevcs {
    fn repository(&mut self, repo: &str) -> Repository {
        if let Some(known) = self.known.get(repo) {
            return known.clone();
        }
        let visibility = onevcs::inspect_repository(&onevcs::boundary::InspectRequest {
            repository: repo.to_owned(),
        })
        .map(|answer| answer.visibility)
        .unwrap_or(onevcs::Visibility::Unknown);
        let identity = onevcs::release_targets(repo).map_or_else(
            |_| Identity::Unresolved(repo.to_owned()),
            |targets| Identity::Registered(targets.identity),
        );
        let answered = Repository {
            clearance: if visibility.effective() == onevcs::Visibility::Public {
                Clearance::Public
            } else {
                Clearance::Private
            },
            identity,
        };
        self.known.insert(repo.to_owned(), answered.clone());
        answered
    }

    fn check(&mut self, input: onevcs::BoundaryInput) -> onevcs::BoundaryVerdict {
        // An error asking is an answer nobody gave, which is never a pass.
        onevcs::check_public_output(input).unwrap_or(onevcs::BoundaryVerdict::Unavailable {
            reason: onevcs::boundary::Unavailability::Registry,
        })
    }
}

/// Every repository a node names: its `repo`, then whatever else its record names.
pub(crate) fn repositories_of(node: &Node) -> Vec<&str> {
    let mut named: Vec<&str> = node.repo.as_deref().into_iter().collect();
    for repository in node
        .task_record
        .iter()
        .flat_map(|record| record.repositories.iter())
    {
        if !named.contains(&repository.as_str()) {
            named.push(repository);
        }
    }
    named
}

/// How the boundary stands for one plan.
// llmlint: ignore-block[invalid_states_unrepresentable] node ids are the plain `String`
// every map of `RunState` keys by — `branches`, `heads`, `superseded` — validated where the
// graph took each node, and `scope` is the `onevcs` identity list `Identity` above says
// why is a string; a newtype on these alone would disagree with every map it is read
// beside and convert at each read, which `src/AGENTS.md` names as drift.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Resolved {
    /// Every node's clearance, by id.
    pub(crate) clearances: BTreeMap<String, Clearance>,
    /// The nodes that name a repository and are public: somewhere public work lands.
    pub(crate) public_destinations: BTreeSet<String>,
    /// The run's term scope: the identity of every private repository a node names.
    pub(crate) scope: Vec<String>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl Resolved {
    /// Whether the plan holds both a private node and a public destination.
    pub(crate) fn mixed(&self) -> bool {
        !self.public_destinations.is_empty()
            && self
                .clearances
                .values()
                .any(|clearance| *clearance == Clearance::Private)
    }

    /// One node's clearance; a node this resolution never saw is private.
    pub(crate) fn clearance(&self, node: &str) -> Clearance {
        self.clearances
            .get(node)
            .copied()
            .unwrap_or(Clearance::Private)
    }

    /// The term scope as `onevcs` takes it.
    pub(crate) fn term_scope(&self) -> onevcs::TermScope {
        onevcs::TermScope::Identities(self.scope.clone())
    }
}

/// Resolve every node's clearance and the run's term scope.
pub(crate) fn resolve(plan: &Plan, oracle: &mut dyn Oracle) -> Resolved {
    let edges: BTreeSet<(&str, &str)> = plan
        .export_edges
        .iter()
        .map(|edge| (edge.from.as_str(), edge.to.as_str()))
        .collect();
    let mut resolved = Resolved::default();
    let mut scope = BTreeSet::new();
    // What each node's own task and repositories say, before anything is inherited.
    let mut own: BTreeMap<&str, Option<Clearance>> = BTreeMap::new();
    for node in &plan.tasks {
        let named = repositories_of(node);
        let mut clearance = (node.classification == Classification::Private)
            .then_some(Clearance::Private)
            .or_else(|| (!named.is_empty()).then_some(Clearance::Public));
        for repo in named {
            let repository = oracle.repository(repo);
            if repository.clearance == Clearance::Private {
                clearance = Some(Clearance::Private);
                scope.insert(repository.identity.into_scope());
            }
        }
        if clearance == Some(Clearance::Public) {
            resolved.public_destinations.insert(node.id.clone());
        }
        own.insert(node.id.as_str(), clearance);
    }
    // A node naming no repository inherits from what it depends on, through any depth
    // of such nodes; an export edge carries no clearance across it, which is its point.
    let deps: BTreeMap<&str, Vec<&str>> = plan
        .tasks
        .iter()
        .map(|node| {
            (
                node.id.as_str(),
                node.deps
                    .iter()
                    .map(String::as_str)
                    .filter(|dep| !edges.contains(&(*dep, node.id.as_str())))
                    .collect(),
            )
        })
        .collect();
    fn inherit<'a>(
        id: &'a str,
        own: &BTreeMap<&'a str, Option<Clearance>>,
        deps: &BTreeMap<&'a str, Vec<&'a str>>,
        done: &mut BTreeMap<&'a str, Clearance>,
        visiting: &mut BTreeSet<&'a str>,
    ) -> Clearance {
        if let Some(settled) = done.get(id) {
            return *settled;
        }
        let clearance = match own.get(id).copied().flatten() {
            Some(clearance) => clearance,
            None if !visiting.insert(id) => Clearance::Public,
            None => {
                let inherited = deps
                    .get(id)
                    .into_iter()
                    .flatten()
                    .filter(|dep| own.contains_key(*dep))
                    .map(|dep| inherit(dep, own, deps, done, visiting))
                    .max()
                    .unwrap_or(Clearance::Public);
                visiting.remove(id);
                inherited
            }
        };
        done.insert(id, clearance);
        clearance
    }
    let mut done = BTreeMap::new();
    for node in &plan.tasks {
        let clearance = inherit(&node.id, &own, &deps, &mut done, &mut BTreeSet::new());
        resolved.clearances.insert(node.id.clone(), clearance);
    }
    resolved.scope = scope.into_iter().collect();
    resolved
}

/// The plan a run stands at now: its live graph, written with the launch plan's fields
/// and its export edges following every retry.
pub(crate) fn plan_now(state: &crate::projection::RunState) -> Plan {
    let launched = state.plan.clone().unwrap_or_else(|| Plan {
        schema_version: crate::plan::PLAN_SCHEMA_VERSION,
        goal: None,
        name: None,
        concurrency: state.graph.concurrency.max(1),
        tasks: Vec::new(),
        export_edges: Vec::new(),
    });
    let mut plan = state.graph.to_plan(&launched);
    plan.export_edges = edges_now(&launched.export_edges, &state.superseded);
    plan
}

/// What a resolution reads of a plan: each node's id, the repositories it names, its
/// classification and its dependencies, and the export edges. Two plans with one shape
/// resolve alike, so a shape already resolved is not asked of `onevcs` again.
type Shape = (
    Vec<(String, Vec<String>, Classification, Vec<String>)>,
    Vec<ExportEdge>,
);

fn shape(plan: &Plan) -> Shape {
    (
        plan.tasks
            .iter()
            .map(|node| {
                (
                    node.id.clone(),
                    repositories_of(node)
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                    node.classification,
                    node.deps.clone(),
                )
            })
            .collect(),
        plan.export_edges.clone(),
    )
}

/// The resolutions this process has made, by shape. A run's shape changes only where an
/// accepted edit changes what a node names or depends on, so this holds one or two
/// entries a run; it is bounded all the same, and the oldest goes first.
static RESOLVED: std::sync::Mutex<Vec<(Shape, Resolved)>> = std::sync::Mutex::new(Vec::new());

/// How many shapes [`RESOLVED`] keeps.
const RESOLVED_KEPT: usize = 8;

/// How the boundary stands for a run's plan, asked of the linked `onevcs` the first time
/// the plan takes this shape — at launch, and again after every accepted edit that changes
/// a repository, a classification, a dependency or an edge.
///
/// What a dispatch and a write-back read, which recomputes the run's scope on exactly what
/// the contract recomputes it on: an accepted edit. A repository whose visibility changes
/// under an unchanged plan is read anew by the next load, live edit or envelope judged,
/// each of which resolves afresh, and by these two at the next edit after it.
// llmlint: ignore[changed_behavior_has_e2e] reuse under an unchanged shape is the absence of
// a host read, which no journey can observe here: every repository this suite registers has
// a visibility its rules file states, and a declared visibility is never asked of the host.
// Recomputation on a changed shape is
// `publication_boundary::a_live_edit_naming_b_puts_b_in_the_run_scope_for_every_check_after_it`.
pub(crate) fn of_run(plan: &Plan) -> Resolved {
    let key = shape(plan);
    if let Ok(cached) = RESOLVED.lock() {
        if let Some((_, resolved)) = cached.iter().find(|(shape, _)| *shape == key) {
            return resolved.clone();
        }
    }
    let resolved = resolve(plan, &mut Onevcs::default());
    if let Ok(mut cached) = RESOLVED.lock() {
        if cached.len() >= RESOLVED_KEPT {
            cached.remove(0);
        }
        cached.push((key, resolved.clone()));
    }
    resolved
}

/// The store's `WritePolicy`, answered by the linked `onevcs` rather than by the two
/// commands a standalone store runs: the same visibility and the same check, with the
/// verdict read through the store's own reading of a check's answer
/// ([`onetaskgraph_core::boundary::check_verdict`]), so a write the engine makes and one
/// the store's command line makes are refused in the same words.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct WritePolicy;

impl WritePolicy {
    /// The verdict `input` earns, as the store reads one.
    fn verdict(
        input: &onetaskgraph_core::boundary::PublicWriteInput,
    ) -> onetaskgraph_core::boundary::WriteVerdict {
        let destination = match input.destination {
            onetaskgraph_core::boundary::RepositoryVisibility::Public => onevcs::Visibility::Public,
            onetaskgraph_core::boundary::RepositoryVisibility::Private => {
                onevcs::Visibility::Private
            }
            onetaskgraph_core::boundary::RepositoryVisibility::Unknown => {
                onevcs::Visibility::Unknown
            }
        };
        let checked = onevcs::check_public_output(onevcs::BoundaryInput {
            destination,
            text: input.text.clone(),
            paths: input.paths.clone(),
            metadata: input.metadata.clone(),
            scope: input
                .scope
                .clone()
                .map_or(onevcs::TermScope::Registry, onevcs::TermScope::Identities),
        });
        // The exit code `onevcs boundary check` gives each verdict, so the one reading of an
        // answer reads this one too.
        let (code, answer) = match checked {
            Ok(verdict @ onevcs::BoundaryVerdict::Pass) => (0, verdict),
            Ok(verdict @ onevcs::BoundaryVerdict::Refuse { .. }) => (1, verdict),
            Ok(verdict) => (2, verdict),
            Err(_) => (
                2,
                onevcs::BoundaryVerdict::Unavailable {
                    reason: onevcs::boundary::Unavailability::Registry,
                },
            ),
        };
        let stdout = serde_json::to_vec(&answer).unwrap_or_default();
        onetaskgraph_core::boundary::check_verdict(Some(code), &stdout)
    }
}

#[async_trait::async_trait]
impl onetaskgraph_core::boundary::WritePolicy for WritePolicy {
    async fn repository_visibility(
        &self,
        repository: &onetaskgraph_plugin_api::Repository,
    ) -> Result<
        onetaskgraph_core::boundary::RepositoryVisibility,
        onetaskgraph_core::boundary::PolicyError,
    > {
        let answered = onevcs::inspect_repository(&onevcs::boundary::InspectRequest {
            repository: repository.as_str().to_owned(),
        })
        .map_err(|error| onetaskgraph_core::boundary::PolicyError {
            message: error.to_string(),
        })?;
        Ok(match answered.visibility {
            onevcs::Visibility::Public => onetaskgraph_core::boundary::RepositoryVisibility::Public,
            onevcs::Visibility::Private => {
                onetaskgraph_core::boundary::RepositoryVisibility::Private
            }
            onevcs::Visibility::Unknown => {
                onetaskgraph_core::boundary::RepositoryVisibility::Unknown
            }
        })
    }

    async fn check_public_write(
        &self,
        input: &onetaskgraph_core::boundary::PublicWriteInput,
    ) -> Result<onetaskgraph_core::boundary::WriteVerdict, onetaskgraph_core::boundary::PolicyError>
    {
        Ok(Self::verdict(input))
    }
}

/// Whether `dep` → `node` is an export edge of the run as it stands now.
pub(crate) fn is_export_edge(state: &crate::projection::RunState, dep: &str, node: &str) -> bool {
    state.plan.as_ref().is_some_and(|plan| {
        edges_now(&plan.export_edges, &state.superseded)
            .iter()
            .any(|edge| edge.from == dep && edge.to == node)
    })
}

/// What one dispatch is told and given because of the boundary.
///
/// Outside a mixed plan it is the run's term scope and nothing else, so a
/// single-visibility plan's dispatch is composed exactly as it always was.
#[derive(Debug, Clone, Default)]
pub(crate) struct Dispatching {
    /// The run's term scope, which every check this dispatch's closeout asks derives
    /// its terms from.
    pub(crate) term_scope: onevcs::TermScope,
    /// Its side of the boundary, in a mixed plan, and nothing in any other.
    side: Option<Side>,
}

/// One dispatch's side of a mixed plan's boundary: each of the three roles a node can
/// have there, carrying exactly what that role is given.
// llmlint: ignore-block[invalid_states_unrepresentable] a branch name here is a
// *proposal* handed to `onevcs` as `SessionRequest::branch_name: Option<String>`, which the
// sibling sanitizes into a name git accepts and refuses by naming it; it is composed only
// in `dispatching` from a fixed shape and an opaque hex id, so no caller can hand one in.
#[derive(Debug, Clone)]
enum Side {
    /// A private node that produces no export.
    Private {
        /// The section appended to its task.
        section: String,
    },
    /// A private node that produces an export, cutting a neutral branch.
    Producer {
        /// The section appended to its task.
        section: String,
        /// The neutral name proposed for the branch its session cuts.
        branch_name: String,
    },
    /// A public node, cutting a neutral branch where it names a repository, and starting
    /// from an export where it consumes one.
    Public {
        /// The section appended to its task.
        section: String,
        /// The neutral name proposed for the branch its session cuts.
        branch_name: Option<String>,
        /// The export its session starts from.
        export: Option<Export>,
    },
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl Dispatching {
    /// The section appended to the task, in a mixed plan.
    pub(crate) fn section(&self) -> Option<&str> {
        self.side.as_ref().map(|side| match side {
            Side::Private { section }
            | Side::Producer { section, .. }
            | Side::Public { section, .. } => section.as_str(),
        })
    }

    /// The neutral branch name proposed for a session this node cuts, where it has one.
    pub(crate) fn branch_name(&self) -> Option<&str> {
        match self.side.as_ref()? {
            Side::Private { .. } => None,
            Side::Producer { branch_name, .. } => Some(branch_name),
            Side::Public { branch_name, .. } => branch_name.as_deref(),
        }
    }

    /// The export this consumer's session starts from.
    pub(crate) fn export(&self) -> Option<&Export> {
        match self.side.as_ref()? {
            Side::Public { export, .. } => export.as_ref(),
            Side::Private { .. } | Side::Producer { .. } => None,
        }
    }

    /// Whether this node is public in a mixed plan, so what is rendered for it names
    /// nothing of the run beyond the node itself.
    pub(crate) fn is_public(&self) -> bool {
        matches!(self.side, Some(Side::Public { .. }))
    }
}

/// The one value a context's `mixed` takes: a single-visibility plan has no context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mixed;

impl Serialize for Mixed {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

/// The template context a mixed plan's dispatch receives.
// llmlint: ignore-block[invalid_states_unrepresentable] both directories are an export
// edge's own, which `check_edges` held to `normalized` — the grammar
// `the_directory_grammar_is_the_one_onevcs_export_holds_a_directory_to` reconciles with
// the sibling's — before any plan carrying them could load or be edited in; this value is
// built from that edge and only serialized, into the contract's JSON shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Context {
    /// Always `true`.
    mixed: Mixed,
    /// The dispatching node's clearance.
    clearance: Clearance,
    /// The directory an export producer writes its generic example under.
    #[serde(skip_serializing_if = "Option::is_none")]
    export_directory: Option<String>,
    /// The directory an export lands in, in the consumer's repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    target_directory: Option<String>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// One export a consumer's session starts from.
// llmlint: ignore-block[invalid_states_unrepresentable] every field but the ordinal is a
// value handed straight to `onevcs::ExportRequest`, whose own fields are the same strings
// for the reason its own suppression states: export is the one boundary that decides
// each — an identity, a branch git accepts, a normalized directory — and refuses one by
// naming the step, never the value, because the value may be the very private name an
// export exists to keep out of public output. A newtype here would refuse on
// construction, and would have to say what it refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Export {
    /// The producer's node id, which only the private record names.
    pub(crate) producer: String,
    /// The producer's repository.
    pub(crate) from: String,
    /// The producer's kept branch.
    pub(crate) branch: String,
    /// The commit that branch was kept at, where the settlement recorded one.
    pub(crate) head: Option<String>,
    /// The directory exported.
    pub(crate) source_directory: String,
    /// The consumer's repository.
    pub(crate) to: String,
    /// Where the directory lands there.
    pub(crate) target_directory: String,
    /// The consumer's position in the plan, from one, which its branch is named by.
    pub(crate) ordinal: usize,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// The heading of the boundary section a mixed plan's task carries.
pub(crate) const SECTION_HEADING: &str = "## Repository boundary";

/// A fresh opaque id: twelve hex characters that say nothing about the run, the plan or
/// the node they name a branch for.
fn opaque() -> String {
    use sha2::Digest;
    static MINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut hasher = sha2::Sha256::new();
    hasher.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(
        MINTED
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_le_bytes(),
    );
    hasher
        .finalize()
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// What one dispatch of `node` is told and given, in a plan resolved as `resolved`.
///
/// Refused — the node settles as an infrastructure failure, before anything is
/// dispatched — where it consumes an export whose producer recorded no kept branch: its
/// session starts from the export or not at all, and never from the default base.
pub(crate) fn dispatching(
    resolved: &Resolved,
    plan: &Plan,
    node: &Node,
    kept: &BTreeMap<String, String>,
    heads: &BTreeMap<String, String>,
) -> Result<Dispatching, String> {
    let term_scope = resolved.term_scope();
    if !resolved.mixed() {
        return Ok(Dispatching {
            term_scope,
            side: None,
        });
    }
    let clearance = resolved.clearance(&node.id);
    let ordinal = plan
        .tasks
        .iter()
        .position(|candidate| candidate.id == node.id)
        .map_or(plan.tasks.len() + 1, |index| index + 1);
    let produced = plan.export_edges.iter().find(|edge| edge.from == node.id);
    let consumed = consumed_by(plan, &node.id);
    let mut context = Context {
        mixed: Mixed,
        clearance,
        export_directory: None,
        target_directory: None,
    };
    let mut branch_name = None;
    let mut export = None;
    let mut lines: Vec<String> = Vec::new();
    match (clearance, produced, consumed) {
        (Clearance::Private, Some(edge), _) => {
            context.export_directory = Some(edge.source_directory.clone());
            context.target_directory = Some(edge.target_directory.clone());
            branch_name = Some(format!("export/{}", opaque()));
            lines.push(format!(
                "This node is an export producer. Write the generic example under `{}`; \
                 after this node's branch is kept, exactly that directory is exported into a \
                 public repository under `{}`.",
                edge.source_directory, edge.target_directory
            ));
            lines.extend(PRODUCER.iter().map(|line| format!("- {line}")));
        }
        (Clearance::Private, None, _) => {
            lines.push(
                "This node is private. Its work reaches a public node only through an export \
                 edge."
                    .to_owned(),
            );
            lines.push(summary());
        }
        (Clearance::Public, _, consumed) => {
            if node.repo.is_some() {
                branch_name = Some(format!("public/{}/task-{ordinal}", opaque()));
            }
            if let Some(edge) = consumed {
                context.target_directory = Some(edge.target_directory.clone());
                let Some(branch) = kept.get(&edge.from) else {
                    return Err(
                        "its export producer settled with no kept branch, so there is nothing \
                         to export and its session does not start from anything else"
                            .to_owned(),
                    );
                };
                let producer = plan
                    .tasks
                    .iter()
                    .find(|candidate| candidate.id == edge.from)
                    .and_then(|producer| producer.repo.clone())
                    .ok_or_else(|| "its export producer names no repository".to_owned())?;
                export = Some(Export {
                    producer: edge.from.clone(),
                    from: producer,
                    branch: branch.clone(),
                    head: heads.get(&edge.from).cloned(),
                    source_directory: edge.source_directory.clone(),
                    to: node.repo.clone().unwrap_or_default(),
                    target_directory: edge.target_directory.clone(),
                    ordinal,
                });
                lines.push(format!(
                    "This node is public, and its branch starts from one neutral commit that \
                     adds generic examples under `{}`; build on them.",
                    edge.target_directory
                ));
            } else {
                lines.push("This node is public.".to_owned());
            }
            lines.push(
                "Everything it writes is public: name no private repository, project, plan, \
                 task, path or identifier in files, commit messages, branch names or \
                 change-request text."
                    .to_owned(),
            );
        }
    }
    let rendered = serde_json::to_string(&serde_json::json!({ "boundary": context }))
        .expect("a context serializes");
    let section = format!(
        "{SECTION_HEADING}\n\nThis plan spans public and private repositories.\n\n```json\n{rendered}\n```\n\n{}",
        lines.join("\n")
    );
    let side = match (clearance, branch_name) {
        (Clearance::Public, branch_name) => Side::Public {
            section,
            branch_name,
            export,
        },
        (Clearance::Private, Some(branch_name)) => Side::Producer {
            section,
            branch_name,
        },
        (Clearance::Private, None) => Side::Private { section },
    };
    Ok(Dispatching {
        term_scope,
        side: Some(side),
    })
}

/// A task with the boundary section appended, where there is one.
pub(crate) fn with_section(task: String, section: Option<&str>) -> String {
    match section {
        Some(section) => format!("{}\n\n{section}", task.trim_end()),
        None => task,
    }
}

/// The host's branch prefix, resolved as `onevcs` resolves it for a branch it cuts: the
/// environment over the host's branches file over none. An export's branch is named by
/// this crate, so the prefix is put in front of it here, and the export screens the
/// whole name — the prefix included — against the run's terms. A branches file that is
/// there and cannot be read is refused rather than read as no prefix, exactly as `onevcs`
/// refuses one; the e2e journeys hold the two to one answer for both layers.
fn host_prefix() -> Result<String, String> {
    if let Ok(prefix) = std::env::var(onevcs::branches::PREFIX_ENV) {
        return Ok(prefix);
    }
    let unreadable = |why: String| {
        eprintln!("onepipeline: the host's branches file could not be read: {why}");
        "the host's branch prefix could not be read, so no export branch could be named; \
         the driver's log on this host says why"
            .to_owned()
    };
    let path = onevcs::branches::default_path().map_err(|error| unreadable(error.to_string()))?;
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_norway::from_str::<onevcs::branches::BranchesFile>(&raw)
            .map(|file| file.prefix)
            .map_err(|error| unreadable(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(unreadable(error.to_string())),
    }
}

/// What the run's private record holds about one export: everything about where it came
/// from, which no public renderer reads.
// llmlint: ignore-block[invalid_states_unrepresentable] the run's own record of what it
// asked `onevcs::export` for and what that answered, field for field — the
// `ExportRequest` strings and the `Exported` branch and head, which are strings in the
// sibling for the reason `Export` above states. It is written only by `ensure_export`
// and read back whole, unknown fields refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recorded {
    producer: String,
    from: String,
    branch: String,
    head: Option<String>,
    source_directory: String,
    to: String,
    target_directory: String,
    exported: String,
    exported_head: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl Recorded {
    /// Whether this record is an export of `export`'s own edge: the same producer, into
    /// the same repository and directory.
    fn of_edge(&self, export: &Export) -> bool {
        self.producer == export.producer
            && self.to == export.to
            && self.target_directory == export.target_directory
    }
}

/// Where the run keeps its exports: a private file beside its journal.
fn exports_path(paths: &crate::ledger::RunPaths) -> std::path::PathBuf {
    paths.dir.join("exports.json")
}

/// The run's record of its exports: none where it has made none, and a refusal where the
/// record is there and cannot be read — never an empty record over evidence that exists.
fn recorded(paths: &crate::ledger::RunPaths) -> Result<Vec<Recorded>, String> {
    let path = exports_path(paths);
    let unreadable = |why: String| {
        eprintln!("onepipeline: {} could not be read: {why}", path.display());
        "the run's record of its exports could not be read, so nothing is exported over it; \
         the driver's log on this host says why"
            .to_owned()
    };
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| unreadable(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(unreadable(error.to_string())),
    }
}

/// Whether `branch` is an export of `export`'s own edge this run recorded — the one
/// branch that edge's consumer may be pinned to.
pub(crate) fn pinned_to_its_export(
    paths: &crate::ledger::RunPaths,
    export: &Export,
    branch: &str,
) -> Result<bool, String> {
    Ok(recorded(paths)?
        .iter()
        .any(|held| held.of_edge(export) && held.exported == branch))
}

/// The public branch `export` starts its consumer from, exporting it the first time it
/// is asked for and answering the recorded one every time after — a re-dispatch, a
/// retry's replacement and a resumed driver all start from the same commit, never from
/// the default base. A producer superseded by a replacement is exported anew.
///
/// The refusal is the export's own neutral sentence; what it found is kept privately by
/// `onevcs`.
pub(crate) fn ensure_export(
    paths: &crate::ledger::RunPaths,
    export: &Export,
    term_scope: &onevcs::TermScope,
) -> Result<String, String> {
    let path = exports_path(paths);
    let mut recorded = recorded(paths)?;
    // The producer's kept branch is the work: a producer kept again is a replacement under
    // a new id with a branch of its own, so it is exported anew, and the head is evidence
    // the record keeps rather than part of what it is looked up by.
    // llmlint: ignore[changed_behavior_has_e2e] the one case beyond the lineage journey's — a
    // producer replaced *after* it was exported — has no input: an exported producer has
    // settled `done`, and `retry` supersedes only a running, failed or cancelled node, so no
    // edit can put a replacement behind an export that already happened. The replacement
    // the record does distinguish, a superseded producer's, is
    // `publication_boundary::the_export_base_holds_through_supersession_retry_and_resume`.
    let same = |held: &Recorded| {
        held.of_edge(export)
            && held.branch == export.branch
            && held.source_directory == export.source_directory
    };
    if let Some(held) = recorded.iter().find(|held| same(held)) {
        return Ok(held.exported.clone());
    }
    // The kept branch was pushed from the session's own clone, so it is made reachable
    // from the producer's registered checkout — which is where an export reads it — first.
    // Ref plumbing only: nothing is checked out and nothing is published.
    // llmlint: ignore[changed_behavior_has_e2e] a kept branch is already in the producer's
    // registered checkout wherever its session's clone was cut from it, which is every
    // journey this suite can drive, so the import is a no-op there and cannot be made to
    // fail without placing a different branch under a name `onevcs` minted at random a
    // moment before. What a failure answers is the neutral refusal below, the detail kept
    // in the driver's own log.
    onevcs::import_branch(&onevcs::ImportRequest {
        repo: std::path::PathBuf::from(&export.from),
        branch: export.branch.clone(),
        from: None,
        under: None,
    })
    .map_err(|refused| {
        // What was refused names the producer's repository and branch, so it goes to the
        // driver's own log — private, on this host — and the consumer is told neutrally.
        eprintln!("onepipeline: an export producer's kept branch could not be imported: {refused}");
        "the export producer's kept branch could not be made reachable for its export; the \
         driver's log on this host says why"
            .to_owned()
    })?;
    let branch_name = format!(
        "{}public/{}/task-{}",
        host_prefix()?,
        opaque(),
        export.ordinal
    );
    let exported = onevcs::export(
        &onevcs::Providers::real(),
        &onevcs::ExportRequest {
            from: export.from.clone(),
            branch: export.branch.clone(),
            directory: export.source_directory.clone(),
            to: export.to.clone(),
            target_directory: export.target_directory.clone(),
            branch_name,
            term_scope: term_scope.clone(),
        },
    )
    .map_err(|refused| refused.to_string())?;
    recorded.push(Recorded {
        producer: export.producer.clone(),
        from: export.from.clone(),
        branch: export.branch.clone(),
        head: export.head.clone(),
        source_directory: export.source_directory.clone(),
        to: export.to.clone(),
        target_directory: export.target_directory.clone(),
        exported: exported.branch.clone(),
        exported_head: exported.head,
    });
    let bytes = serde_json::to_vec_pretty(&recorded).map_err(|error| error.to_string())?;
    // llmlint: ignore[changed_behavior_has_e2e] reached only by a write that fails in the
    // instant between an export `onevcs` made and its record: the record was read a moment
    // before, in the same directory, and every journal write of the run goes there too, so
    // no journey can make this one write fail without failing those first or racing the
    // export. What it answers is the export's own outcome, unrecorded, which the next
    // attempt exports again under a fresh name rather than starting from the default base.
    crate::ledger::write_atomic(&path, &bytes, crate::ledger::Durability::Record)
        .map_err(|error| format!("the export was made but could not be recorded: {error}"))?;
    Ok(exported.branch)
}

/// What the boundary decided about one envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Judged {
    /// Nothing in it reaches a public node carrying a private term.
    Pass,
    /// Something in it would, and it must not be appended or delivered.
    Refuse(String),
    /// The check could not decide, which is never a pass.
    Unavailable(String),
}

impl Judged {
    /// The exit code `onepipeline boundary validate-envelope` answers this with: the bus
    /// validator protocol's `0` for a pass and `1` for a refusal, and anything else —
    /// here `2` — for a check that could not decide, which the bus never sends.
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            Self::Pass => 0,
            Self::Refuse(_) => 1,
            Self::Unavailable(_) => 2,
        }
    }

    /// The neutral sentence it is reported in, where it is not a pass.
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Pass => None,
            Self::Refuse(reason) | Self::Unavailable(reason) => Some(reason),
        }
    }
}

/// Every string inside `value`, keys excepted, in document order.
fn strings_of(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => out.push(text.clone()),
        serde_json::Value::Array(values) => values.iter().for_each(|value| strings_of(value, out)),
        serde_json::Value::Object(fields) => fields
            .iter()
            .filter(|(key, _)| key.as_str() != "op")
            .for_each(|(_, value)| strings_of(value, out)),
        _ => {}
    }
}

/// The commands a document carries — a reply envelope's, a routed record's, or a reply
/// a record wraps — each read as the channel's own [`Command`](crate::channel::Command),
/// which refuses an op it does not know and a field no op takes. `None` for a document
/// one of whose commands is not one.
fn commands_of(document: &serde_json::Value) -> Option<Vec<crate::channel::Command>> {
    let mut commands = Vec::new();
    for found in [
        document.get("commands"),
        document.pointer("/reply/commands"),
    ]
    .into_iter()
    .flatten()
    {
        for listed in found.as_array()? {
            commands.push(serde_json::from_value(listed.clone()).ok()?);
        }
    }
    Some(commands)
}

/// The text a document carries that no command addresses to a node: a verdict's message
/// and reason, and a surface's message — what an observer or the planner says to nobody in
/// particular, which a public node may yet be shown.
fn untargeted_of(document: &serde_json::Value) -> Vec<String> {
    let mut text = Vec::new();
    for pointer in [
        "/message",
        "/reason",
        "/reply/message",
        "/reply/reason",
        "/text",
    ] {
        if let Some(found) = document.pointer(pointer) {
            strings_of(found, &mut text);
        }
    }
    text
}

/// The run as `commands`' graph edits would leave it, so a node is judged by the
/// clearance it will have: every command that changes which nodes there are, what they
/// name or what they depend on — an `add`, a `retry`, a `requeue` and its amendment, a
/// `drop`, a `reparent` — compiled by the engine's own edit validator and folded exactly as
/// the reconciler folds it. The rest change no clearance and are judged against the run as
/// it stands. `Err` names the first edit that would not apply, which is one the engine
/// refuses.
fn with_graph_edits(
    state: &crate::projection::RunState,
    commands: &[crate::channel::Command],
) -> Result<crate::projection::RunState, usize> {
    let mut staged = state.clone();
    for (index, command) in commands.iter().enumerate() {
        if !matches!(
            command,
            crate::channel::Command::Add { .. }
                | crate::channel::Command::Retry { .. }
                | crate::channel::Command::Requeue { .. }
                | crate::channel::Command::Drop { .. }
                | crate::channel::Command::Reparent { .. }
        ) {
            continue;
        }
        let mut candidate = staged.graph.clone();
        let operations = crate::edits::compile(
            &mut candidate,
            &staged.frontier(),
            crate::channel::Author::planner(),
            command,
        )
        .map_err(|_| index)?;
        crate::projection::fold_operations(&mut staged, &operations, None);
    }
    Ok(staged)
}

/// What a command naming a node the run does not hold — or one the engine would not
/// apply — is, to the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unknown {
    /// Unavailable: the bus validator, which nothing judges after, cannot pass a command
    /// it has no clearance to judge by.
    Unavailable,
    /// Left to the engine's own validation, which refuses such a command in words that
    /// say why, before anything is applied.
    Elsewhere,
}

/// Judge one document — a reply envelope, a routed command record, a surface — against
/// the run `state` describes.
///
/// Only a mixed plan is judged: an all-public one derives no term and an all-private one
/// has nowhere public for anything to reach. In one, every command addressed to a public
/// node — by [`crate::channel::target_of`], the engine's own reading of what a command is
/// about — is checked over its whole prospective payload, in the run's own term scope;
/// one addressed to a private node passes; and text addressed to no node is checked as
/// public. A command naming a node the run does not hold, or one the engine would not
/// apply, has no clearance to judge it by, and is what `unknown` says.
pub(crate) fn judge(
    state: &crate::projection::RunState,
    document: &serde_json::Value,
    oracle: &mut dyn Oracle,
    unknown: Unknown,
) -> Judged {
    let unjudgeable = |why: &str| match unknown {
        Unknown::Unavailable => Some(Judged::Unavailable(format!(
            "the publication boundary could not judge {why}"
        ))),
        Unknown::Elsewhere => None,
    };
    let Some(commands) = commands_of(document) else {
        return unjudgeable("what it was handed: a command in it is not one this build reads")
            .unwrap_or(Judged::Pass);
    };
    let staged = match with_graph_edits(state, &commands) {
        Ok(staged) => staged,
        Err(index) => {
            let op = crate::channel::op_of(&commands[index]);
            if let Some(judged) = unjudgeable(&format!("`{op}`: it would not apply to this run")) {
                return judged;
            }
            state.clone()
        }
    };
    // Asked afresh every time, never from memory: a repository whose visibility changed
    // since the last envelope is judged by what it is now.
    let resolved = resolve(&plan_now(&staged), oracle);
    if !resolved.mixed() {
        return Judged::Pass;
    }
    // A node an envelope drops is gone from the graph it leaves, so its own command is
    // judged by the clearance the node had: resolved only where an envelope drops one.
    let before = commands
        .iter()
        .any(|command| matches!(command, crate::channel::Command::Drop { .. }))
        .then(|| resolve(&plan_now(state), oracle));
    let clearance_of = |target: &str| {
        resolved.clearances.get(target).copied().or_else(|| {
            before
                .as_ref()
                .and_then(|before| before.clearances.get(target).copied())
        })
    };
    let mut ask = |text: Vec<String>, about: &str| -> Option<Judged> {
        if text.is_empty() {
            return None;
        }
        let verdict = oracle.check(onevcs::BoundaryInput {
            destination: onevcs::Visibility::Public,
            text,
            paths: Vec::new(),
            metadata: Vec::new(),
            scope: resolved.term_scope(),
        });
        let reason = verdict.reason()?;
        Some(match verdict {
            onevcs::BoundaryVerdict::Refuse { .. } => Judged::Refuse(format!(
                "the publication boundary refused {about}: {reason}"
            )),
            _ => Judged::Unavailable(format!(
                "the publication boundary could not judge {about}: {reason}"
            )),
        })
    };
    for command in &commands {
        let op = crate::channel::op_of(command);
        let mut text = Vec::new();
        strings_of(
            &serde_json::to_value(command).unwrap_or_default(),
            &mut text,
        );
        // What a command is about, as the engine reads it — but a retry is judged as the
        // replacement it puts in, which is the node its payload becomes.
        let target = match command {
            crate::channel::Command::Retry { node, .. } => Some(node.id.clone()),
            _ => crate::channel::target_of(command),
        };
        match target.map(|target| (clearance_of(&target), target)) {
            Some((None, _)) => {
                if let Some(judged) = unjudgeable(&format!(
                    "`{op}`: the node it names has no clearance in this run"
                )) {
                    return judged;
                }
            }
            Some((Some(clearance), target)) => {
                if clearance == Clearance::Public {
                    if let Some(judged) = ask(text, &format!("`{op}` for public node {target}")) {
                        return judged;
                    }
                }
            }
            None => {
                if let Some(judged) = ask(text, &format!("`{op}`, which names no node")) {
                    return judged;
                }
            }
        }
    }
    if let Some(judged) = ask(untargeted_of(document), "text addressed to no node") {
        return judged;
    }
    Judged::Pass
}

/// Refuse what [`judge`] does not pass, for the engine's own submissions and the commands
/// its reconciler takes up — a direct `reply`, `note` or `attest`, a queued envelope, and
/// one a dead driver had claimed — so a caller that never meets the bus meets the same
/// rule. A command naming a node the run does not hold, or one that would not apply, is
/// left to the engine's own refusal of it.
pub(crate) fn guard(
    state: &crate::projection::RunState,
    document: &serde_json::Value,
) -> crate::Result<()> {
    match judge(state, document, &mut Onevcs::default(), Unknown::Elsewhere) {
        Judged::Pass => Ok(()),
        judged => Err(crate::Error::Refused(
            judged.reason().unwrap_or_default().to_owned(),
        )),
    }
}

/// `onepipeline boundary validate-envelope --run RUN`: judge the document on `stdin`
/// against the run's current plan, answering the bus validator protocol's exit code and
/// the neutral reason to print.
pub(crate) fn validate_envelope(
    paths: &crate::ledger::RunPaths,
    stdin: &str,
) -> crate::Result<Judged> {
    let document: serde_json::Value = serde_json::from_str(stdin).map_err(|error| {
        crate::Error::Invalid(format!("the envelope on stdin is not JSON: {error}"))
    })?;
    if let Err(why) = well_formed(&document) {
        return Ok(Judged::Unavailable(format!(
            "the publication boundary could not judge what it was handed: {why}"
        )));
    }
    let view = crate::views::RunView::open(paths)?;
    Ok(judge(
        &view.state,
        &document,
        &mut Onevcs::default(),
        Unknown::Unavailable,
    ))
}

/// Whether `document` has the shape of what a bus hands a validator — a reply envelope, a
/// routed record wrapping one, a command record or a surface: a mapping, whose commands,
/// wherever it carries them, are a list of mappings each naming its `op`. Anything else is
/// judged by nothing, so it is never passed.
fn well_formed(document: &serde_json::Value) -> Result<(), String> {
    let Some(fields) = document.as_object() else {
        return Err("it is not a mapping".to_owned());
    };
    let reply = match fields.get("reply") {
        None => None,
        Some(serde_json::Value::Object(reply)) => Some(reply),
        Some(_) => return Err("its `reply` is not a mapping".to_owned()),
    };
    for commands in [
        fields.get("commands"),
        reply.and_then(|reply| reply.get("commands")),
    ]
    .into_iter()
    .flatten()
    {
        let Some(listed) = commands.as_array() else {
            return Err("its `commands` is not a list".to_owned());
        };
        if !listed
            .iter()
            .all(|command| command.get("op").is_some_and(serde_json::Value::is_string))
        {
            return Err("a command in it names no `op`".to_owned());
        }
    }
    // And it is one of the records the channel itself writes, read as that record: a
    // misspelled field is not quietly read as an envelope carrying nothing to judge.
    // The reply envelope refuses an unknown field itself; the three queue records read the
    // way a durable record must — a newer build's field skipped — so what a validator is
    // handed of them is held to exactly the fields the channel's layout registers for each.
    let only = |known: Vec<String>| fields.keys().all(|key| known.contains(key));
    let reads_as = |document: &serde_json::Value| {
        serde_json::from_value::<crate::channel::Reply>(document.clone()).is_ok()
            || (only(fields_of::<crate::channel::layout::QueuedReply>())
                && serde_json::from_value::<crate::channel::QueuedReply>(document.clone()).is_ok())
            || (only(fields_of::<crate::channel::layout::QueuedCommands>())
                && serde_json::from_value::<crate::channel::QueuedCommands>(document.clone())
                    .is_ok())
            || (only(fields_of::<crate::channel::layout::Surface>())
                && serde_json::from_value::<crate::channel::layout::Surface>(document.clone())
                    .is_ok())
    };
    if reads_as(document) {
        Ok(())
    } else {
        Err(
            "it is none of a reply envelope, a routed reply or command record, or a surface"
                .to_owned(),
        )
    }
}

/// Every field one of the channel layout's registered records carries, read off the
/// schema the layout publishes it under — so the set is that record's own, not a copy.
fn fields_of<T: schemars::JsonSchema>() -> Vec<String> {
    schemars::schema_for!(T)
        .as_value()
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default()
}

/// What a bus pass cache keys a pass by beside the payload's bytes: a digest of the run's
/// term scope, every node's clearance, and the terms each scoped repository contributes
/// now. A changed term, a changed visibility or a changed scope is a different key, so a
/// payload cached as passing is judged again rather than passed on its bytes.
pub(crate) fn fingerprint(paths: &crate::ledger::RunPaths) -> crate::Result<String> {
    use sha2::Digest;
    let view = crate::views::RunView::open(paths)?;
    let plan = plan_now(&view.state);
    let resolved = resolve(&plan, &mut Onevcs::default());
    let terms: Vec<serde_json::Value> = resolved
        .scope
        .iter()
        .map(|identity| match onevcs::repository_boundary(identity) {
            Ok(boundary) => serde_json::json!({"identity": identity, "boundary": boundary}),
            Err(error) => {
                serde_json::json!({"identity": identity, "unreadable": error.to_string()})
            }
        })
        .collect();
    let document = serde_json::json!({
        "scope": resolved.scope,
        "clearances": resolved.clearances,
        "public_destinations": resolved.public_destinations,
        "terms": terms,
    });
    let digest = sha2::Sha256::digest(document.to_string().as_bytes());
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The engine-owned advice for crossing the boundary: one typed value, rendered into
/// every refusal this module makes, printed by `onepipeline boundary guidance`, and
/// read by the host's planner injection and plan check, so no host restates the rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Guidance {
    /// The version of this shape.
    pub(crate) schema_version: u32,
    /// The steps, in order: generalize, preserve, export, consume.
    pub(crate) steps: Vec<GuidanceStep>,
    /// What a generalize producer is held to so its export is not refused.
    pub(crate) producer: Vec<String>,
    /// The sentence a refusal carries, which is the steps in one line.
    pub(crate) summary: String,
}

/// One step of [`Guidance`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GuidanceStep {
    /// Which step it is.
    pub(crate) step: Step,
    /// What it asks.
    pub(crate) text: &'static str,
}

/// The four steps of crossing the boundary, in the order they happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Step {
    /// The private work is written as a generic example in one directory.
    Generalize,
    /// The engine keeps the producer's branch.
    Preserve,
    /// The engine exports that directory as one neutral commit.
    Export,
    /// The public node starts from that commit.
    Consume,
}

impl Step {
    /// The name the step is spelled with.
    fn as_str(self) -> &'static str {
        match self {
            Self::Generalize => "generalize",
            Self::Preserve => "preserve",
            Self::Export => "export",
            Self::Consume => "consume",
        }
    }
}

/// The version [`Guidance`] is written at.
pub(crate) const GUIDANCE_VERSION: u32 = 1;

/// The steps of [`Guidance`], in order.
const STEPS: [(Step, &str); 4] = [
    (
        Step::Generalize,
        "Write the private work as a generic example inside one neutral directory, on a \
         private node that publishes with `publish: \"preserve\"`.",
    ),
    (
        Step::Preserve,
        "The engine keeps that producer's branch on its own origin rather than landing it.",
    ),
    (
        Step::Export,
        "The engine exports only that directory, as one neutral commit on the public \
         repository's base, carrying no private history, name, path or author.",
    ),
    (
        Step::Consume,
        "The public node depends on the producer through an entry of the project's \
         `onepipeline.export_edges` — `{from, to, source_directory, target_directory}` — and \
         its session starts from that commit; its id, title and task name nothing private.",
    ),
];

/// What a producer is held to, in order.
const PRODUCER: [&str; 3] = [
    "Change nothing outside the edge's `source_directory`: a change anywhere else refuses \
     the export.",
    "Keep every commit subject and message neutral — no private repository, project, \
     path or identifier — because the export screens them with the files.",
    "The engine names the producer's branch `<host prefix>export/<opaque id>`; do not \
     rename it.",
];

/// The guidance, as every reader of it gets it.
pub(crate) fn guidance() -> Guidance {
    Guidance {
        schema_version: GUIDANCE_VERSION,
        steps: STEPS
            .iter()
            .map(|(step, text)| GuidanceStep { step: *step, text })
            .collect(),
        producer: PRODUCER.iter().map(|line| (*line).to_owned()).collect(),
        summary: summary(),
    }
}

/// The guidance as `onepipeline boundary guidance` prints it: one JSON object, or the
/// summary a refusal carries followed by what a producer is held to.
pub(crate) fn render_guidance(json: bool) -> crate::Result<String> {
    let guidance = guidance();
    if json {
        return serde_json::to_string_pretty(&guidance)
            .map_err(|error| crate::Error::Invalid(error.to_string()));
    }
    let mut lines = vec![guidance.summary, String::new(), "A producer:".to_owned()];
    lines.extend(guidance.producer.iter().map(|line| format!("- {line}")));
    Ok(lines.join("\n"))
}

/// The steps as one sentence, which is what a refusal carries.
fn summary() -> String {
    format!(
        "Private work reaches a public node only by generalize -> preserve -> export -> \
         consume: {}",
        STEPS
            .iter()
            .map(|(step, text)| format!("{}: {text}", step.as_str()))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

/// A refusal about `node`, carrying the guidance.
fn refused(node: &str, what: impl AsRef<str>) -> Refusal {
    Refusal::node(node, format!("{} {}", what.as_ref(), summary()))
}

/// A refusal about the plan as a whole, carrying the guidance.
fn refused_plan(what: impl AsRef<str>) -> Refusal {
    Refusal::plain(format!("{} {}", what.as_ref(), summary()))
}

/// Whether `directory` is a relative, normalized path: no root, no `.` or `..`, no
/// empty segment, no `.git`, and no backslash — the rule `onevcs export` holds it to.
pub(crate) fn normalized(directory: &str) -> bool {
    !directory.is_empty()
        && !directory.starts_with('/')
        && !directory.contains('\\')
        && directory
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | "..") && segment != ".git")
}

/// The export edges as they stand after the run's supersessions: an edge naming a node
/// a `retry` replaced names its replacement, through any number of retries.
pub(crate) fn edges_now(
    edges: &[ExportEdge],
    superseded: &BTreeMap<String, String>,
) -> Vec<ExportEdge> {
    let latest = |id: &str| {
        let mut id = id.to_owned();
        let mut seen = BTreeSet::new();
        while let Some(next) = superseded.get(&id) {
            if !seen.insert(id.clone()) {
                break;
            }
            id.clone_from(next);
        }
        id
    };
    edges
        .iter()
        .map(|edge| ExportEdge {
            from: latest(&edge.from),
            to: latest(&edge.to),
            ..edge.clone()
        })
        .collect()
}

/// The export edge a node consumes, where it consumes one.
pub(crate) fn consumed_by<'a>(plan: &'a Plan, node: &str) -> Option<&'a ExportEdge> {
    plan.export_edges.iter().find(|edge| edge.to == node)
}

/// Whether `dep` → `node` is an export edge of `plan`.
pub(crate) fn is_export(plan: &Plan, dep: &str, node: &str) -> bool {
    plan.export_edges
        .iter()
        .any(|edge| edge.from == dep && edge.to == node)
}

/// The export edges' own shape, which needs nothing but the plan: every end names a node,
/// no edge is declared twice, no consumer has two, both directories are normalized, and
/// every edge is a dependency its consumer states. Asked by the graph's own rules, before
/// any other dependency rule, so a refusal about an edge is about the edge.
pub(crate) fn check_edges(plan: &Plan) -> Result<(), Refusal> {
    let ids: BTreeSet<&str> = plan.tasks.iter().map(|node| node.id.as_str()).collect();
    let mut seen = BTreeSet::new();
    let mut consumers: BTreeMap<&str, &str> = BTreeMap::new();
    for edge in &plan.export_edges {
        for (end, id) in [("from", &edge.from), ("to", &edge.to)] {
            if !ids.contains(id.as_str()) {
                return Err(refused_plan(format!(
                    "export edge {} -> {} is dangling: its `{end}` names no node of this plan.",
                    edge.from, edge.to
                )));
            }
        }
        if !seen.insert((edge.from.as_str(), edge.to.as_str())) {
            return Err(refused(
                &edge.to,
                format!(
                    "export edge {} -> {} is declared twice.",
                    edge.from, edge.to
                ),
            ));
        }
        if let Some(first) = consumers.insert(edge.to.as_str(), edge.from.as_str()) {
            return Err(refused(
                &edge.to,
                format!(
                    "it is the consumer of two export edges, from {first} and from {}, which \
                     is ambiguous: a session starts from one export.",
                    edge.from
                ),
            ));
        }
        for (which, directory) in [
            ("source_directory", &edge.source_directory),
            ("target_directory", &edge.target_directory),
        ] {
            if !normalized(directory) {
                return Err(refused(
                    &edge.to,
                    format!(
                        "its export edge's `{which}` is not a relative, normalized path (no \
                         root, `.`, `..`, empty segment, `.git` or backslash)."
                    ),
                )
                .field("export_edges"));
            }
        }
    }

    for edge in &plan.export_edges {
        let consumer = plan
            .tasks
            .iter()
            .find(|node| node.id == edge.to)
            .expect("every end was found above");
        if !consumer.deps.contains(&edge.from) {
            return Err(refused(
                &consumer.id,
                format!(
                    "its export edge from {} is not a dependency it states, so the plan graph \
                     and the export edge disagree.",
                    edge.from
                ),
            )
            .field("deps"));
        }
    }
    Ok(())
}

/// Where a plan is judged: as a document is loaded, or as a live edit leaves the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// `start`, `plan check` and `adopt`, over what a planner wrote.
    Load,
    /// Every accepted live edit, over a graph the engine has also pinned branches on.
    Edit,
}

/// Judge `plan` against the boundary's rules, asking `oracle` what its repositories are
/// and whether a consumer's public fields are neutral.
///
/// A single-visibility plan with no export edges is accepted at once.
pub(crate) fn check(
    plan: &Plan,
    stage: Stage,
    oracle: &mut dyn Oracle,
) -> Result<Resolved, Refusal> {
    let resolved = resolve(plan, oracle);
    let ids: BTreeSet<&str> = plan.tasks.iter().map(|node| node.id.as_str()).collect();
    let nodes: BTreeMap<&str, &Node> = plan
        .tasks
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();

    check_edges(plan)?;

    for edge in &plan.export_edges {
        let producer = nodes[edge.from.as_str()];
        let consumer = nodes[edge.to.as_str()];
        if resolved.clearance(&producer.id) != Clearance::Private
            || producer.repo.is_none()
            || producer.publish != Publish::Preserve
        {
            return Err(refused(
                &producer.id,
                "an export producer must be a private lifecycle node that publishes with \
                 `publish: \"preserve\"`.",
            ));
        }
        if resolved.clearance(&consumer.id) != Clearance::Public
            || !resolved.public_destinations.contains(&consumer.id)
        {
            return Err(refused(
                &consumer.id,
                "an export consumer must be a public lifecycle node: its classification and \
                 every repository it names public.",
            ));
        }
        // A `branch` or a `resume` the engine pins on a consumer it cancelled and requeued is
        // the export's own branch, so only a document's — what a plan was loaded with — is
        // judged here; a dispatch refuses any pinned branch that is not a recorded export.
        let authored = matches!(stage, Stage::Load)
            && (consumer.branch.is_some() || consumer.resume.is_some());
        if consumer.base_branch.is_some() || authored {
            return Err(refused(
                &consumer.id,
                "an export consumer's session starts from its export, so it may not name an \
                 alternate private context — a `base_branch`, a `branch` or a `resume`.",
            ));
        }
    }

    // Every dependency from a private node into a public destination is an export edge,
    // and a consumer has no private context beside the one its edge exports.
    for node in &plan.tasks {
        if !resolved.public_destinations.contains(&node.id) {
            continue;
        }
        for dep in node.deps.iter().filter(|dep| ids.contains(dep.as_str())) {
            if resolved.clearance(dep) != Clearance::Private || is_export(plan, dep, &node.id) {
                continue;
            }
            let what = if consumed_by(plan, &node.id).is_some() {
                format!(
                    "it depends on private node {dep} beside its export edge, an alternate \
                     private context its public session may not carry."
                )
            } else {
                format!(
                    "it is public and depends on private node {dep} through an ordinary \
                     dependency, which would carry private work into public output."
                )
            };
            return Err(refused(&node.id, what).field("deps"));
        }
    }

    // A consumer's public fields, and the directories an export writes, name nothing
    // private: asked of the term check, over the run's own scope.
    for edge in &plan.export_edges {
        let consumer = nodes[edge.to.as_str()];
        let input = onevcs::BoundaryInput {
            destination: onevcs::Visibility::Public,
            text: consumer
                .task
                .iter()
                .chain(consumer.body.iter())
                .chain(consumer.amendment.iter())
                .chain(consumer.context.iter())
                .cloned()
                .collect(),
            paths: vec![edge.source_directory.clone(), edge.target_directory.clone()],
            metadata: std::iter::once(consumer.id.clone())
                .chain(consumer.title.iter().cloned())
                .collect(),
            scope: resolved.term_scope(),
        };
        match oracle.check(input) {
            onevcs::BoundaryVerdict::Pass => {}
            verdict => {
                return Err(refused(
                    &consumer.id,
                    format!(
                        "an export consumer's id, title, task and directories must be neutral: {}.",
                        verdict.reason().unwrap_or_default()
                    ),
                ))
            }
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Repositories answered from a table: a name ending `-public` is public, and every
    /// term check refuses text carrying `secret`.
    #[derive(Default)]
    struct Table {
        asked: Vec<onevcs::BoundaryInput>,
    }

    impl Oracle for Table {
        fn repository(&mut self, repo: &str) -> Repository {
            Repository {
                clearance: if repo.ends_with("-public") {
                    Clearance::Public
                } else {
                    Clearance::Private
                },
                identity: Identity::Registered(format!("github.com/owner/{repo}")),
            }
        }

        fn check(&mut self, input: onevcs::BoundaryInput) -> onevcs::BoundaryVerdict {
            let leaks = input
                .text
                .iter()
                .chain(&input.paths)
                .chain(&input.metadata)
                .any(|entry| entry.contains("secret"));
            self.asked.push(input);
            if leaks {
                onevcs::BoundaryVerdict::Refuse {
                    surface: onevcs::boundary::Surface::Text,
                }
            } else {
                onevcs::BoundaryVerdict::Pass
            }
        }
    }

    fn lifecycle(id: &str, repo: &str, deps: &[&str]) -> Node {
        Node {
            id: id.into(),
            repo: Some(repo.into()),
            title: Some(format!("feat: {id}")),
            persona: Some("engineer".into()),
            task: Some(format!("## What\n{id}\n\n## Acceptance criteria\n- done")),
            deps: deps.iter().map(|dep| (*dep).to_owned()).collect(),
            ..Node::default()
        }
    }

    fn plan(tasks: Vec<Node>, export_edges: Vec<ExportEdge>) -> Plan {
        Plan {
            schema_version: crate::plan::PLAN_SCHEMA_VERSION,
            goal: None,
            name: None,
            concurrency: 1,
            tasks,
            export_edges,
        }
    }

    fn edge(from: &str, to: &str) -> ExportEdge {
        ExportEdge {
            from: from.into(),
            to: to.into(),
            source_directory: "examples/generic".into(),
            target_directory: "examples".into(),
        }
    }

    fn producer() -> Node {
        Node {
            publish: Publish::Preserve,
            ..lifecycle("generalize", "inner", &[])
        }
    }

    #[test]
    fn a_single_visibility_plan_is_not_mixed_and_derives_its_scope_from_private_repositories() {
        let mut table = Table::default();
        let all_private = plan(
            vec![
                lifecycle("a", "inner", &[]),
                lifecycle("b", "other", &["a"]),
                Node {
                    id: "review".into(),
                    deps: vec!["b".into()],
                    ..Node::default()
                },
            ],
            vec![],
        );
        let resolved =
            check(&all_private, Stage::Load, &mut table).expect("an all-private plan is fine");
        assert!(!resolved.mixed());
        assert_eq!(resolved.clearance("review"), Clearance::Private);
        assert_eq!(
            resolved.scope,
            vec!["github.com/owner/inner", "github.com/owner/other"]
        );

        let all_public = plan(
            vec![
                lifecycle("a", "lib-public", &[]),
                Node {
                    id: "review".into(),
                    deps: vec!["a".into()],
                    ..Node::default()
                },
            ],
            vec![],
        );
        let resolved =
            check(&all_public, Stage::Load, &mut table).expect("an all-public plan is fine");
        assert!(!resolved.mixed());
        assert!(
            resolved.scope.is_empty(),
            "nothing private, nothing to derive"
        );
        assert!(table.asked.is_empty(), "no term check is asked of either");
    }

    #[test]
    fn an_ordinary_private_to_public_dependency_is_refused_with_the_guidance() {
        let mut table = Table::default();
        let direct = plan(
            vec![
                lifecycle("private", "inner", &[]),
                lifecycle("public", "lib-public", &["private"]),
            ],
            vec![],
        );
        let refusal = check(&direct, Stage::Load, &mut table).expect_err("refused");
        assert!(
            refusal.message.contains("ordinary dependency"),
            "{refusal:?}"
        );
        assert!(refusal.message.contains(&summary()), "{refusal:?}");

        // Through a node naming no repository, which inherits the private side.
        let through = plan(
            vec![
                lifecycle("private", "inner", &[]),
                Node {
                    id: "relay".into(),
                    deps: vec!["private".into()],
                    ..Node::default()
                },
                lifecycle("public", "lib-public", &["relay"]),
            ],
            vec![],
        );
        let refusal = check(&through, Stage::Load, &mut table).expect_err("refused");
        assert!(
            refusal.message.contains("private node relay"),
            "{refusal:?}"
        );

        // A private classification is never downgraded by a public repository.
        let classified = plan(
            vec![
                Node {
                    classification: Classification::Private,
                    ..lifecycle("classified", "lib-public", &[])
                },
                lifecycle("public", "lib-public", &["classified"]),
            ],
            vec![],
        );
        assert!(check(&classified, Stage::Load, &mut table).is_err());
    }

    #[test]
    fn a_valid_export_edge_is_accepted_and_its_consumer_is_checked_for_neutrality() {
        let mut table = Table::default();
        let valid = plan(
            vec![
                producer(),
                lifecycle("examples", "lib-public", &["generalize"]),
            ],
            vec![edge("generalize", "examples")],
        );
        let resolved = check(&valid, Stage::Load, &mut table).expect("a valid edge");
        assert!(resolved.mixed());
        assert_eq!(table.asked.len(), 1);
        assert_eq!(
            table.asked[0].scope,
            onevcs::TermScope::Identities(vec!["github.com/owner/inner".into()])
        );

        let leaky = Plan {
            tasks: vec![
                producer(),
                Node {
                    title: Some("feat: secret examples".into()),
                    ..lifecycle("examples", "lib-public", &["generalize"])
                },
            ],
            ..valid.clone()
        };
        let refusal = check(&leaky, Stage::Load, &mut table).expect_err("refused");
        assert!(refusal.message.contains("must be neutral"), "{refusal:?}");
    }

    #[test]
    fn every_malformed_edge_is_refused_naming_what_is_wrong() {
        let mut table = Table::default();
        let consumer = || lifecycle("examples", "lib-public", &["generalize"]);
        let cases: Vec<(Vec<Node>, Vec<ExportEdge>, &str)> = vec![
            (
                vec![producer(), consumer()],
                vec![edge("generalize", "nowhere")],
                "dangling",
            ),
            (
                vec![producer(), consumer()],
                vec![
                    edge("generalize", "examples"),
                    edge("generalize", "examples"),
                ],
                "declared twice",
            ),
            (
                vec![
                    producer(),
                    Node {
                        id: "second".into(),
                        ..producer()
                    },
                    lifecycle("examples", "lib-public", &["generalize", "second"]),
                ],
                vec![edge("generalize", "examples"), edge("second", "examples")],
                "ambiguous",
            ),
            (
                vec![producer(), lifecycle("examples", "lib-public", &[])],
                vec![edge("generalize", "examples")],
                "disagree",
            ),
            (
                vec![lifecycle("generalize", "inner", &[]), consumer()],
                vec![edge("generalize", "examples")],
                "export producer must be",
            ),
            (
                vec![
                    producer(),
                    lifecycle("other", "inner", &[]),
                    lifecycle("examples", "lib-public", &["generalize", "other"]),
                ],
                vec![edge("generalize", "examples")],
                "alternate private context",
            ),
            (
                vec![
                    producer(),
                    Node {
                        base_branch: Some("private-topic".into()),
                        ..consumer()
                    },
                ],
                vec![edge("generalize", "examples")],
                "alternate private context",
            ),
            (
                vec![producer(), consumer()],
                vec![ExportEdge {
                    source_directory: "../outside".into(),
                    ..edge("generalize", "examples")
                }],
                "normalized",
            ),
        ];
        for (tasks, edges, expected) in cases {
            let refusal = check(&plan(tasks, edges), Stage::Load, &mut table).expect_err(expected);
            assert!(
                refusal.message.contains(expected),
                "{expected}: {refusal:?}"
            );
            assert!(refusal.message.contains(&summary()), "{refusal:?}");
        }
    }

    #[test]
    fn an_envelope_is_judged_by_where_each_command_lands() {
        let mut table = Table::default();
        let state_of = |plan: Plan| crate::projection::RunState {
            graph: crate::graph::Graph::from_plan(&plan),
            plan: Some(plan),
            ..crate::projection::RunState::default()
        };
        let mixed = plan(
            vec![
                producer(),
                lifecycle("examples", "lib-public", &["generalize"]),
            ],
            vec![edge("generalize", "examples")],
        );
        let mixed = state_of(mixed);
        let note = |id: &str, text: &str| {
            serde_json::json!({"commands": [
                {"op": "note", "id": id, "addressee": "worker", "text": text}
            ]})
        };
        // A public node's note is checked, and one carrying a term refused naming only
        // where it was found; a private node's is not.
        assert_eq!(
            judge(
                &mixed,
                &note("examples", "use the plain fixtures"),
                &mut table,
                Unknown::Unavailable
            ),
            Judged::Pass
        );
        let refused = judge(
            &mixed,
            &note("examples", "copy the secret layout"),
            &mut table,
            Unknown::Unavailable,
        );
        assert!(matches!(&refused, Judged::Refuse(reason) if !reason.contains("secret")));
        assert_eq!(refused.exit_code(), 1);
        assert_eq!(
            judge(
                &mixed,
                &note("generalize", "copy the secret layout"),
                &mut table,
                Unknown::Unavailable
            ),
            Judged::Pass
        );
        // A node the plan does not hold has no clearance to judge by.
        let unknown = judge(
            &mixed,
            &note("nowhere", "anything"),
            &mut table,
            Unknown::Unavailable,
        );
        assert!(matches!(unknown, Judged::Unavailable(_)));
        assert_eq!(unknown.exit_code(), 2);
        // Text addressed to no node is public wherever any node is.
        let verdict = serde_json::json!({"completion": false, "message": "a secret aside"});
        assert!(matches!(
            judge(&mixed, &verdict, &mut table, Unknown::Unavailable),
            Judged::Refuse(_)
        ));
        // A node an envelope adds is judged by the clearance it will have.
        let added = serde_json::json!({"commands": [{"op": "add", "node": {
            "id": "more", "repo": "lib-public", "title": "feat: more things",
            "persona": "engineer",
            "task": "## What\nThe secret things.\n\n## Acceptance criteria\n- done",
        }}]});
        let judged_add = judge(&mixed, &added, &mut table, Unknown::Unavailable);
        assert!(matches!(judged_add, Judged::Refuse(_)), "{judged_add:?}");
        // An all-private plan has nothing public to protect.
        let private = state_of(plan(vec![lifecycle("a", "inner", &[])], vec![]));
        assert_eq!(
            judge(
                &private,
                &serde_json::json!({"message": "a secret aside"}),
                &mut table,
                Unknown::Unavailable
            ),
            Judged::Pass
        );
    }

    /// The directory grammar an edge is refused by at load is the one `onevcs export`
    /// refuses a directory by: each spelling is put to the sibling's own export, in a
    /// scratch state root, and is refused there for not being normalized exactly where it
    /// is refused here.
    #[test]
    fn the_directory_grammar_is_the_one_onevcs_export_holds_a_directory_to() {
        let _home = crate::vcs::scratch_home_held();
        let root = std::env::temp_dir().join(format!("onepipeline-grammar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch state root");
        std::env::set_var("ONEVCS_HOME", &root);
        for directory in [
            "examples",
            "examples/generic",
            "a/b/c",
            "",
            "/examples",
            "./examples",
            "examples/./generic",
            "examples/../elsewhere",
            "..",
            "examples//generic",
            "examples/",
            ".git",
            "examples/.git/hooks",
            "examples\\generic",
            "examples/.gitkeep",
        ] {
            let refused = onevcs::export(
                &onevcs::Providers::real(),
                &onevcs::ExportRequest {
                    from: "nowhere".into(),
                    branch: "topic".into(),
                    directory: directory.into(),
                    to: "elsewhere".into(),
                    target_directory: "examples".into(),
                    branch_name: "topic".into(),
                    term_scope: onevcs::TermScope::Identities(Vec::new()),
                },
            )
            .expect_err("nothing here is registered, so every export is refused")
            .to_string();
            let sibling =
                !refused.contains("its source directory is not a relative, normalized path");
            assert_eq!(normalized(directory), sibling, "{directory:?}: {refused}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_edge_follows_its_ends_through_every_retry() {
        let superseded: BTreeMap<String, String> = [
            ("examples".to_owned(), "examples-2".to_owned()),
            ("examples-2".to_owned(), "examples-3".to_owned()),
        ]
        .into_iter()
        .collect();
        let now = edges_now(&[edge("generalize", "examples")], &superseded);
        assert_eq!(now[0].to, "examples-3");
        assert_eq!(now[0].from, "generalize");
    }
}
