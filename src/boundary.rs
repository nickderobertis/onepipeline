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

impl Clearance {
    /// The word a template context and a refusal spell it with.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
        }
    }
}

/// What a repository spelling resolves to, as far as the boundary is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Repository {
    /// Its effective visibility: only a verified public repository is public.
    pub(crate) public: bool,
    /// The `onevcs` identity key a term scope names it by, or the spelling itself where
    /// the host cannot resolve one — which a scoped check then answers as unavailable
    /// rather than passing.
    pub(crate) identity: String,
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
        let identity = onevcs::release_targets(repo)
            .map(|targets| targets.identity)
            .unwrap_or_else(|_| repo.to_owned());
        let answered = Repository {
            public: visibility.effective() == onevcs::Visibility::Public,
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Resolved {
    /// Every node's clearance, by id.
    pub(crate) clearances: BTreeMap<String, Clearance>,
    /// The nodes that name a repository and are public: somewhere public work lands.
    pub(crate) public_destinations: BTreeSet<String>,
    /// The run's term scope: the identity of every private repository a node names.
    pub(crate) scope: Vec<String>,
}

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
            if !repository.public {
                clearance = Some(Clearance::Private);
                scope.insert(repository.identity);
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
/// Every field but the term scope is empty outside a mixed plan, so a
/// single-visibility plan's dispatch is composed exactly as it always was.
#[derive(Debug, Clone, Default)]
pub(crate) struct Dispatching {
    /// The run's term scope, which every check this dispatch's closeout asks derives
    /// its terms from.
    pub(crate) term_scope: onevcs::TermScope,
    /// The section appended to the task this dispatch is handed: the template context
    /// and the guidance for its side of the boundary. Mixed plans only.
    pub(crate) section: Option<String>,
    /// The neutral name proposed for a branch this node's session cuts, in place of the
    /// plan-derived one: a public node's and an export producer's. The host's branch
    /// prefix is put in front of it by `onevcs`.
    pub(crate) branch_name: Option<String>,
    /// The export this consumer's session starts from.
    pub(crate) export: Option<Export>,
    /// Whether this node is public in a mixed plan, so what is rendered for it names
    /// nothing of the run beyond the node itself.
    pub(crate) public: bool,
}

/// The template context a mixed plan's dispatch receives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Context {
    /// Always `true`: a single-visibility plan has no context at all.
    pub(crate) mixed: bool,
    /// The dispatching node's clearance.
    pub(crate) clearance: Clearance,
    /// The directory an export producer writes its generic example under.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) export_directory: Option<String>,
    /// The directory an export lands in, in the consumer's repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target_directory: Option<String>,
}

/// One export a consumer's session starts from.
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
    let mut dispatching = Dispatching {
        term_scope: resolved.term_scope(),
        ..Dispatching::default()
    };
    if !resolved.mixed() {
        return Ok(dispatching);
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
        mixed: true,
        clearance,
        export_directory: None,
        target_directory: None,
    };
    let mut lines: Vec<String> = Vec::new();
    match (clearance, produced, consumed) {
        (Clearance::Private, Some(edge), _) => {
            context.export_directory = Some(edge.source_directory.clone());
            context.target_directory = Some(edge.target_directory.clone());
            dispatching.branch_name = Some(format!("export/{}", opaque()));
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
            dispatching.public = true;
            if node.repo.is_some() {
                dispatching.branch_name = Some(format!("public/{}/task-{ordinal}", opaque()));
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
                dispatching.export = Some(Export {
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
    dispatching.section = Some(format!(
        "{SECTION_HEADING}\n\nThis plan spans public and private repositories.\n\n```json\n{rendered}\n```\n\n{}",
        lines.join("\n")
    ));
    Ok(dispatching)
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
/// whole name — the prefix included — against the run's terms.
fn host_prefix() -> String {
    if let Ok(prefix) = std::env::var(onevcs::branches::PREFIX_ENV) {
        return prefix;
    }
    onevcs::branches::default_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_norway::from_str::<onevcs::branches::BranchesFile>(&raw).ok())
        .map(|file| file.prefix)
        .unwrap_or_default()
}

/// What the run's private record holds about one export: everything about where it came
/// from, which no public renderer reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Where the run keeps its exports: a private file beside its journal.
fn exports_path(paths: &crate::ledger::RunPaths) -> std::path::PathBuf {
    paths.dir.join("exports.json")
}

/// The public branch `export` starts its consumer from, exporting it the first time it
/// is asked for and answering the recorded one every time after — a re-dispatch, a
/// retry's replacement and a resumed driver all start from the same commit, never from
/// the default base. A producer kept again at a new commit is exported anew.
///
/// The refusal is the export's own neutral sentence; what it found is kept privately by
/// `onevcs`.
pub(crate) fn ensure_export(
    paths: &crate::ledger::RunPaths,
    export: &Export,
    term_scope: &onevcs::TermScope,
) -> Result<String, String> {
    let path = exports_path(paths);
    let mut recorded: Vec<Recorded> = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let same = |held: &Recorded| {
        held.producer == export.producer
            && held.branch == export.branch
            && held.head == export.head
            && held.source_directory == export.source_directory
            && held.to == export.to
            && held.target_directory == export.target_directory
    };
    if let Some(held) = recorded.iter().find(|held| same(held)) {
        return Ok(held.exported.clone());
    }
    // The kept branch was pushed from the session's own clone, so it is made reachable
    // from the producer's registered checkout — which is where an export reads it — first.
    // Ref plumbing only: nothing is checked out and nothing is published.
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
        host_prefix(),
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
    crate::ledger::write_atomic(&path, &bytes, crate::ledger::Durability::Record)
        .map_err(|error| format!("the export was made but could not be recorded: {error}"))?;
    Ok(exported.branch)
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
    /// The step's name.
    pub(crate) step: &'static str,
    /// What it asks.
    pub(crate) text: &'static str,
}

/// The version [`Guidance`] is written at.
pub(crate) const GUIDANCE_VERSION: u32 = 1;

/// The steps of [`Guidance`], in order.
const STEPS: [(&str, &str); 4] = [
    (
        "generalize",
        "Write the private work as a generic example inside one neutral directory, on a \
         private node that publishes with `publish: \"preserve\"`.",
    ),
    (
        "preserve",
        "The engine keeps that producer's branch on its own origin rather than landing it.",
    ),
    (
        "export",
        "The engine exports only that directory, as one neutral commit on the public \
         repository's base, carrying no private history, name, path or author.",
    ),
    (
        "consume",
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
    "The engine names the producer's branch `<host prefix>/export/<opaque id>`; do not \
     rename it.",
];

/// The guidance, as every reader of it gets it.
pub(crate) fn guidance() -> Guidance {
    Guidance {
        schema_version: GUIDANCE_VERSION,
        steps: STEPS
            .iter()
            .map(|(step, text)| GuidanceStep { step, text })
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
            .map(|(step, text)| format!("{step}: {text}"))
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

/// Judge `plan` against the boundary's rules, asking `oracle` what its repositories are
/// and whether a consumer's public fields are neutral.
///
/// A single-visibility plan with no export edges is accepted at once.
pub(crate) fn check(plan: &Plan, oracle: &mut dyn Oracle) -> Result<Resolved, Refusal> {
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
        if consumer.base_branch.is_some() || consumer.branch.is_some() || consumer.resume.is_some()
        {
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
                public: repo.ends_with("-public"),
                identity: format!("github.com/owner/{repo}"),
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
        let resolved = check(&all_private, &mut table).expect("an all-private plan is fine");
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
        let resolved = check(&all_public, &mut table).expect("an all-public plan is fine");
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
        let refusal = check(&direct, &mut table).expect_err("refused");
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
        let refusal = check(&through, &mut table).expect_err("refused");
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
        assert!(check(&classified, &mut table).is_err());
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
        let resolved = check(&valid, &mut table).expect("a valid edge");
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
        let refusal = check(&leaky, &mut table).expect_err("refused");
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
            let refusal = check(&plan(tasks, edges), &mut table).expect_err(expected);
            assert!(
                refusal.message.contains(expected),
                "{expected}: {refusal:?}"
            );
            assert!(refusal.message.contains(&summary()), "{refusal:?}");
        }
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
