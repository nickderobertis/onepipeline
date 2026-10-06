//! Where a plan comes from: the `onetaskgraph` store, **linked** and called in process.
//!
//! A run is launched by naming a **qualified onetaskgraph project id**, and a
//! plan is one project of that store: the plan-level settings are reserved
//! `onepipeline.<field>` metadata keys on the project, a node is one task in it,
//! and a dependency is a real dependency edge between two of those tasks.
//! `docs/contract.md` fixes that mapping; this module is the only place that
//! performs it.
//!
//! **The store is linked, not driven.** Every read goes through `onetaskgraph-core`'s
//! own [`Engine`], built afresh from configuration for each logical command, and every
//! answer arrives as that library's own values — a [`Qualified`] [`Task`], a
//! [`QualifiedEdge`], a [`SourceFailure`] — so a change in the store's shape is a compile
//! error here rather than a misread at run time. The release is whatever `Cargo.lock`
//! resolves, which the engine's bill of materials records like every other linked
//! library; there is no second artifact on the host whose version could drift from it.
//!
//! What the store's own CLI used to decide for a caller is decided here instead, the
//! same way: the configuration is discovered from a working directory, read with the
//! `ONETASKGRAPH_*` settings and `secrets.env` of this process's own environment (less
//! [`RETIRED_BINARY_ENV`], which names nothing now); a `show` answering nothing and
//! reporting no failure is an item that is not there; and a project selector is
//! qualified only when its source is configured. See [`Store`].
//!
//! What this module produces is a [`Plan`] value, held to the graph module's own
//! rules exactly as one read out of a file was: the shape rules, the reference
//! rules, acyclicity, the required title on a lifecycle node, and the named
//! refusal for each retired field all apply at the point a project is read.
//!
//! Two of the refusals cannot be decided from the document alone, because what
//! decides them belongs to the repository a lifecycle node publishes into — so
//! [`crate::destination`] asks it, here, after the graph's own rules and before
//! anything is dispatched.

use std::collections::BTreeMap;
use std::future::Future;
use std::num::NonZeroU32;
use std::path::PathBuf;

use onetaskgraph_core::config::{self, Layer};
use onetaskgraph_core::{
    ConfigError, DependencyRequest, Engine, EngineError, Environment, Filters, GlobalId, Loaded,
    PageToken, Paging, ProjectSelector, Qualified, QualifiedEdge, QueryResponse, SourceFailure,
    TaskRequest,
};
use onetaskgraph_plugin_api::{
    DependencyKind, Direction, ItemKind, MetadataKey, NativeId, Project, SourceName, Task,
};
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::plan::{Node, Plan};
use crate::refusal::Refusal;

/// The environment variable that named the `onetaskgraph` executable when the engine
/// drove one.
///
/// It names nothing now — the store is linked — but a host that still sets it must not
/// have it reach the store's configuration: that product reads its whole configuration
/// from `ONETASKGRAPH_`-prefixed variables, the suffix a dotted setting path, so it would
/// read this one as a setting called `bin` and refuse every command. [`environment`]
/// leaves it out of what the store is handed, exactly as the engine once took it out of
/// the child's environment. `docs/contract-divergences.md` entry 43 records the collision.
pub(crate) const RETIRED_BINARY_ENV: &str = "ONETASKGRAPH_BIN";

/// The store's name, as every refusal about something it answered names it.
const STORE: &str = "onetaskgraph";

/// The metadata prefix reserved to this consumer.
///
/// `onetaskgraph` reserves it to `onepipeline` by name, which is what lets a
/// node's persona, its turn budget, and its publication policy ride on a task
/// without becoming vocabulary of a general task framework.
const RESERVED: &str = "onepipeline.";

/// The reserved key carrying a node's id.
const ID_KEY: &str = "onepipeline.id";

/// Projection-only metadata carrying the last node settlement.
///
/// It shares the consumer's reserved namespace but is not a plan field: relaunching a
/// project that a previous run updated must read the same node definition it launched.
pub(crate) const SETTLEMENT_KEY: &str = "onepipeline.settlement";

/// Projection-only metadata naming the lineage head a destination item carries — the
/// attempt whose fields it renders — beside [`SUPERSEDES_KEY`]; entry 80 of
/// `docs/contract-divergences.md` names both.
pub(crate) const NODE_KEY: &str = "onepipeline.node";

/// Projection-only metadata listing the ids a lineage's head superseded, root first,
/// written only where the head is not the root.
pub(crate) const SUPERSEDES_KEY: &str = "onepipeline.supersedes";

/// The reserved keys the write-back owns and no node field answers to.
///
/// A project a run projected onto — its own, where the plan's store is the destination —
/// carries them on every task, so a relaunch reads past them the way it reads past a
/// settlement: what it launches is the lineage head's definition under the root's id,
/// which is what the item says. Reading them as node fields refused the relaunch of every
/// project a run had written to, naming `node` as a field nobody authored.
const PROJECTION_ONLY: &[&str] = &[SETTLEMENT_KEY, NODE_KEY, SUPERSEDES_KEY];

/// A reserved key this mapping fills from the task itself, and where it comes
/// from — so a project stating it is told which end to edit rather than having
/// its value silently lose to the task's own.
const FILLED_FROM_THE_TASK: &[(&str, &str)] = &[
    ("title", "the task's own `title`"),
    ("task", "the task's own `content`"),
    ("delivers", "the task's own `delivers`"),
    (
        "task_record",
        "the task itself — its native id, its `key` and its `title`",
    ),
];

/// The reserved key a repository identity the store cannot hold is carried
/// under.
///
/// A node's `repo` is the first entry of its task's `repositories`, and that is
/// the spelling every hosted identity uses. `onetaskgraph`'s own repository type
/// is a **normalized origin** — `host/owner/name`, no scheme, no `.git` — so the
/// other kind of `onevcs` identity, a local checkout named by its absolute path,
/// is not a value that list can hold at all. This key is where that identity
/// goes, and a task stating both is refused rather than one of them quietly
/// losing. `docs/contract-divergences.md` records it as a proposal.
const REPO_KEY: &str = "onepipeline.repo";

/// What a project stating `onepipeline.tasks` is told.
const TASKS_ARE_THE_PROJECTS: &str =
    "`onepipeline.tasks` is not a project field: the plan's nodes are the project's own tasks";

/// What a node naming a dependency that is not a cross-DAG reference under
/// `onepipeline.deps` is told.
///
/// The key carries **only** the references that leave this store — another run's
/// DAG is not an item of any source, so no dependency edge can name it. A
/// dependency on a node of this same plan is a real edge between two tasks, and
/// recording it here instead would leave the backend drawing a graph missing
/// that arrow.
const DEPS_ARE_EDGES: &str =
    "`onepipeline.deps` carries cross-DAG `run:<id>#<node>` references only; a dependency \
     on another node of this plan is a dependency edge between the two tasks";

/// The `onetaskgraph` store one configuration describes: a working directory its
/// `onetaskgraph.yaml` is discovered from, and the environment its settings and
/// credentials are read out of.
///
/// Holding no engine is the point. A `github-projects` source keeps its whole-board
/// read for as long as it lives, so an engine that outlived one logical command would
/// answer the next from the board as it was — a later write-back attempt reading the
/// snapshot an earlier one took. Each read therefore builds its [`Engine`] from
/// configuration afresh, through [`Store::engine`], and drops it when it is done.
#[derive(Debug, Clone)]
pub struct Store {
    /// Where `onetaskgraph.yaml` is discovered from, exactly as the store's own CLI
    /// discovered it from the directory it was started in.
    dir: PathBuf,
    /// This process's environment, less [`RETIRED_BINARY_ENV`].
    environment: Environment,
}

impl Store {
    /// The store this process's working directory configures, which is where a plan
    /// is read from.
    pub fn discovered() -> Result<Self> {
        let dir = std::env::current_dir().map_err(|error| Error::Sibling {
            tool: STORE,
            message: format!(
                "the working directory the store's configuration is discovered from cannot \
                 be read: {error}"
            ),
        })?;
        Ok(Self::at(dir))
    }

    /// The store a configuration discovered from `dir` describes: the write-back's, which
    /// is the launch record's directory rather than whichever one the driver is in.
    pub(crate) fn at(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            environment: environment(),
        }
    }

    /// Load this store's configuration, `flags` layered last, and build the engine it
    /// describes — afresh, every time.
    ///
    /// `flags` is what a command line's `--set` was to the store's CLI: the write-back
    /// declares its shadow source there, beside whatever the operator configured.
    pub(crate) fn engine(&self, flags: &Layer) -> std::result::Result<Built, ConfigError> {
        let Loaded {
            config, secrets, ..
        } = config::load(&self.dir, &self.environment, flags)?;
        Ok(Built {
            engine: Engine::build(&config, &secrets),
            page: config.page_size(),
        })
    }

    /// Read one qualified project id as the plan it holds, keeping what a checker and the
    /// write-back need beside it.
    ///
    /// The project is external input, so every refusal it earns is made here,
    /// before a run is minted: a reserved key of the wrong JSON type, a key no
    /// plan field answers to, a task carrying no node id, and a dependency edge
    /// whose far end this plan cannot name.
    ///
    /// Beside the loaded plan, each task's own metadata map **verbatim** — including
    /// the keys outside this consumer's reserved namespace, which the mapping
    /// above drops because no plan field answers to them — and each task exactly as
    /// the store answered it. A consumer's check reads keys this engine does not, and
    /// the launch seeds the write-back's landed baseline from the tasks.
    pub(crate) fn read_plan(&self, project: &QualifiedId) -> std::result::Result<Read, Load> {
        self.load(project)
    }

    /// One task or one document, read by its qualified id.
    ///
    /// # Errors
    ///
    /// [`Error::Sibling`] for a configuration that does not load or a store that
    /// cannot answer, and for an id naming nothing.
    pub(crate) fn item(&self, id: &QualifiedId, kind: StoredKind) -> Result<StoredItem> {
        let built = self
            .engine(&Layer::default())
            .map_err(|error| Error::Sibling {
                tool: STORE,
                message: format!(
                    "the configuration discovered from {} cannot be read: {error}",
                    self.dir.display()
                ),
            })?;
        let reader = Reader::over(built)?;
        let global = id.global();
        if kind == StoredKind::Document {
            let read = reader
                .call(reader.engine.document(&global))
                .map_err(|error| failed("document show", id.as_str(), &error))?;
            let found = one(&global, "document show", read)?;
            return Ok(StoredItem {
                content: found.item.content.unwrap_or_default(),
                metadata: found.item.metadata.into_iter().collect(),
            });
        }
        let found = reader.show(&global)?;
        Ok(StoredItem {
            content: found.item.content.unwrap_or_default(),
            metadata: found.item.metadata.into_iter().collect(),
        })
    }

    fn load(&self, project: &QualifiedId) -> std::result::Result<Read, Load> {
        let built = self
            .engine(&Layer::default())
            .map_err(|error| Error::Sibling {
                tool: STORE,
                message: format!(
                    "the configuration discovered from {} cannot be read: {error}",
                    self.dir.display()
                ),
            })?;
        Reader::over(built)?.load(project)
    }
}

/// One engine, built from configuration for one logical command, and the page size that
/// configuration reads its pages at.
pub(crate) struct Built {
    pub engine: Engine,
    pub page: NonZeroU32,
}

/// The environment the store is handed: this process's own, less [`RETIRED_BINARY_ENV`].
///
/// Read whole rather than filtered to the store's prefix, because it is also where a
/// source's credential comes from — a `github-projects` source's token is read from the
/// variable its configuration names — and where `secrets.env` is found.
pub(crate) fn environment() -> Environment {
    Environment::from_os_pairs(
        std::env::vars_os().filter(|(name, _)| name.as_os_str() != RETIRED_BINARY_ENV),
    )
}

/// Run one logical command's store calls to completion on a runtime of its own.
///
/// The store's plugin traits are async, so each command that calls it — a plan read, a
/// write-back attempt — builds a current-thread runtime and drops it when it is done.
/// Nothing outlives that: the engine holds no task of its own, so dropping the runtime
/// leaves nothing of the command running.
pub(crate) fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

/// How a project selector names a project: qualified only when its source is one this
/// configuration has, and a native id every selected source is asked about otherwise.
///
/// The store's CLI decided this for its `--project` argument, and it is the caller's to
/// decide now: `urn:project:1` is a qualified id only on a host with a source called
/// `urn`, and a native id full of colons on one without.
fn selector(engine: &Engine, project: &QualifiedId) -> ProjectSelector {
    let id = project.global();
    if engine.has(&id.source) {
        ProjectSelector::Qualified(id)
    } else {
        ProjectSelector::Native(NativeId::from(project.as_str()))
    }
}

/// One plan read's engine, and the runtime its calls are driven on.
struct Reader {
    runtime: tokio::runtime::Runtime,
    engine: Engine,
    page: NonZeroU32,
}

impl Reader {
    fn over(built: Built) -> Result<Self> {
        Ok(Self {
            runtime: runtime().map_err(|error| Error::Sibling {
                tool: STORE,
                message: format!("the store cannot be called: {error}"),
            })?,
            engine: built.engine,
            page: built.page,
        })
    }

    /// Drive one of the engine's futures to its end on this read's runtime.
    fn call<T>(&self, future: impl Future<Output = T>) -> T {
        self.runtime.block_on(future)
    }

    fn load(&self, project: &QualifiedId) -> std::result::Result<Read, Load> {
        let held = self.project(project)?;
        let members = members_of(&held.id, &held.item.metadata.clone().into_iter().collect())
            .map_err(|why| Error::Sibling {
                tool: STORE,
                message: why,
            })?;
        let tasks = self.tasks(project, &members)?;

        let mut document = Map::new();
        for (key, value) in &held.item.metadata {
            let Some(field) = key.strip_prefix(RESERVED) else {
                continue;
            };
            if field == "tasks" {
                return Err(
                    refused_plan(project.as_str(), TASKS_ARE_THE_PROJECTS.to_owned())
                        .field("tasks")
                        .into(),
                );
            }
            document.insert(field.to_owned(), value.clone());
        }
        // The project's own title is the plan's name where the reserved key
        // states none: a store's projects are named in the store, and restating
        // that name in metadata to launch one would be the same string twice.
        document
            .entry("name".to_owned())
            .or_insert_with(|| Value::String(held.item.title.clone()));

        // Every node id first, so a dependency edge is resolved against the
        // whole plan rather than against the part of it read so far. A node id
        // is what a dependency names a node by, so two tasks carrying one is a
        // collision the store is where to catch: both ends of it are the
        // author's to fix, and only here are both ends still nameable.
        let mut ids: Ids = BTreeMap::new();
        let mut claimed: BTreeMap<String, String> = BTreeMap::new();
        for task in &tasks {
            let whole = task.id.to_string();
            let id = node_id(&task.item.metadata)
                .map_err(|why| refused(&whole, format!("it {why}")).field("id"))?;
            if let Some(first) = claimed.insert(id.clone(), whole.clone()) {
                return Err(refused(
                    &whole,
                    format!("`{ID_KEY}` '{id}' is already the id of another task, '{first}'"),
                )
                .field("id")
                .into());
            }
            ids.insert(whole, id);
        }

        let mut nodes = Vec::with_capacity(tasks.len());
        for task in &tasks {
            nodes.push(self.node(task, project, &ids)?);
        }
        document.insert("tasks".to_owned(), Value::Array(nodes));

        let document = Value::Object(document);
        if let Some(retired) = crate::plan::retired_field_refusal(&document) {
            return Err(refused_plan(project.as_str(), retired).into());
        }
        // Each node on its own first, so a refusal names the task it is about;
        // the whole document afterwards, which is what refuses a plan-level key
        // no field answers to.
        for (task, node) in tasks.iter().zip(
            document["tasks"]
                .as_array()
                .expect("the nodes just written"),
        ) {
            let whole = task.id.to_string();
            // The node as its plan names it, beside the task it was read out of: a
            // task id is the store's, and only the node id is what the planner wrote.
            let named = |what: String| match node.get("id").and_then(Value::as_str) {
                Some(id) => format!("node '{id}': {what}"),
                None => what,
            };
            if let Some(version) = document["schema_version"]
                .as_u64()
                .filter(|v| *v < u64::from(crate::plan::PLAN_SCHEMA_VERSION))
            {
                // By the key and whatever it carries: a schema-2 document has no
                // `publish` to state, so even the default spelled out is a field that
                // version never had.
                for field in ["sets", "publish"] {
                    if node.get(field).is_some() {
                        return Err(refused(
                            &whole,
                            named(crate::plan::node_field_is_newer(field, version as u32)),
                        )
                        .field(field)
                        .into());
                    }
                }
            }
            read::<Node>(node.clone()).map_err(|error| {
                refused(
                    &whole,
                    named(format!(
                        "{error} — a node's fields are the reserved `{RESERVED}<field>` \
                         metadata keys on its task"
                    )),
                )
            })?;
        }
        let plan: Plan = read::<Plan>(document).map_err(|error| {
            refused_plan(
                project.as_str(),
                format!(
                    "{error} — a plan's settings are the reserved `{RESERVED}<field>` \
                     metadata keys on its project"
                ),
            )
        })?;
        // The graph's own rules, and then what each lifecycle node's destination
        // repository says about it — in that order, and the order is the point.
        // A node whose shape is wrong is refused for being wrong rather than for
        // what a repository would have said about a node no dispatch could have
        // run, and `consumes` naming something this node does not depend on is
        // still answered by the sentence that names the key. Both are the same
        // refusals `onepipeline start` and `onepipeline plan check` already make
        // here; running them at the read is what puts the second set of them
        // before anything is dispatched.
        crate::graph::check(&plan)?;
        crate::destination::check(&plan)?;
        // Keyed by node id, which the walk above has already established is
        // unique within the project: a plan that got this far has one task per
        // node and one node per task.
        let metadata = plan
            .tasks
            .iter()
            .map(|node| node.id.clone())
            .zip(
                tasks
                    .iter()
                    .map(|task| task.item.metadata.clone().into_iter().collect()),
            )
            .collect();
        let stored = plan
            .tasks
            .iter()
            .map(|node| node.id.clone())
            .zip(tasks.iter().map(|task| StoredTask {
                qualified: QualifiedId::from(&task.id),
                content: task.item.content.clone().unwrap_or_default(),
            }))
            .collect();
        let tasks = plan
            .tasks
            .iter()
            .map(|node| node.id.clone())
            .zip(tasks)
            .collect();
        let project_metadata = held.item.metadata.clone().into_iter().collect();
        Ok(Read {
            plan,
            metadata,
            stored,
            tasks,
            project_metadata,
        })
    }

    /// One node, assembled out of its task — a task of the home project `home`, or of one of
    /// its members.
    fn node(
        &self,
        task: &Qualified<Task>,
        home: &QualifiedId,
        ids: &Ids,
    ) -> std::result::Result<Value, Load> {
        let whole = task.id.to_string();
        let mut node = Map::new();
        for (key, value) in &task.item.metadata {
            if PROJECTION_ONLY.contains(&key.as_str()) {
                continue;
            }
            let Some(field) = key.strip_prefix(RESERVED) else {
                continue;
            };
            if let Some((_, whence)) = FILLED_FROM_THE_TASK
                .iter()
                .find(|(filled, _)| *filled == field)
            {
                return Err(refused(
                    &whole,
                    format!(
                        "`{RESERVED}{field}` is not a node field: a node's `{field}` is {whence}"
                    ),
                )
                .field(field)
                .into());
            }
            node.insert(field.to_owned(), value.clone());
        }
        // The task's own fields, which the mapping takes from the item rather
        // than from its metadata: a store shows a person a title, a body, and
        // the repositories the work concerns, and a plan reads those.
        if !task.item.title.trim().is_empty() {
            node.insert("title".to_owned(), Value::String(task.item.title.clone()));
        }
        // Trimmed here because `local-md` stopped trimming a body at 0.2.45: whitespace a
        // file ends its body with is the file's layout, not part of what a step is handed.
        if let Some(content) = task
            .item
            .content
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            node.insert("task".to_owned(), Value::String(content.to_owned()));
        }
        // An entry that arrives bare names this task's own source, which is the store's
        // own rule for a bare id, applied by the store's own type. Anything still not a
        // qualified id is refused by the graph's node check, naming the node and the entry.
        if !task.item.delivers.is_empty() {
            let qualified = task
                .item
                .delivers
                .iter()
                .map(|entry| Value::String(entry.in_source(&task.id.source).to_string()))
                .collect();
            node.insert("delivers".to_owned(), Value::Array(qualified));
        }
        // The task's human-facing record, which is what a branch this node's session
        // cuts is named from. A blank key is the store's way of carrying none, as a
        // blank title is. A member's task names its own source, which is what the
        // write-back finds its item by; a home task's is the launched project's, and
        // is left unsaid so a plan read from one source records what it always did.
        let mut record = Map::new();
        record.insert(
            "id".to_owned(),
            Value::String(task.id.native.as_str().to_owned()),
        );
        if task.id.source.as_str() != home.source() {
            record.insert(
                "source".to_owned(),
                Value::String(task.id.source.as_str().to_owned()),
            );
        }
        if let Some(key) = task.item.key.as_ref().filter(|key| !key.trim().is_empty()) {
            record.insert("key".to_owned(), Value::String(key.clone()));
        }
        record.insert("title".to_owned(), Value::String(task.item.title.clone()));
        node.insert("task_record".to_owned(), Value::Object(record));
        match (task.item.repositories.first(), node.get("repo")) {
            (Some(_), Some(_)) => {
                return Err(refused(
                    &whole,
                    format!(
                        "it names a repository in both `repositories` and `{REPO_KEY}`; a node \
                         lands in one repository, and `{REPO_KEY}` is only for an identity a \
                         normalized origin cannot hold"
                    ),
                )
                .field("repo")
                .into())
            }
            (Some(repository), None) => {
                node.insert(
                    "repo".to_owned(),
                    Value::String(repository.as_str().to_owned()),
                );
            }
            (None, _) => {}
        }

        let mut deps = self.deps(task, ids)?;
        // A cross-DAG reference names another run's DAG, which is an item of no
        // source at all, so it is the one dependency that cannot be an edge.
        if let Some(carried) = node.remove("deps") {
            let listed: Vec<String> = serde_json::from_value(carried).map_err(|error| {
                refused(&whole, format!("`{RESERVED}deps` {error}")).field("deps")
            })?;
            for reference in listed {
                // Anything spelled as a cross-DAG reference is carried, well
                // formed or not: a malformed one is answered by the graph
                // module, which says what the shape has to be. What is refused
                // here is a dependency that never meant to leave this run.
                if !reference.starts_with(crate::crossdag::PREFIX) {
                    return Err(refused(
                        &whole,
                        format!("`{RESERVED}deps` names '{reference}': {DEPS_ARE_EDGES}"),
                    )
                    .field("deps")
                    .into());
                }
                deps.push(reference);
            }
        }
        if !deps.is_empty() {
            node.insert(
                "deps".to_owned(),
                Value::Array(deps.into_iter().map(Value::String).collect()),
            );
        }
        Ok(Value::Object(node))
    }

    /// The node ids this task's own dependency edges point at.
    fn deps(&self, task: &Qualified<Task>, ids: &Ids) -> std::result::Result<Vec<String>, Load> {
        let whole = task.id.to_string();
        let mut deps = Vec::new();
        for edge in self.edges(&task.id)? {
            if edge.from.kind != ItemKind::Task {
                return Err(Error::Sibling {
                    tool: STORE,
                    message: format!(
                        "asked for the dependencies of task '{}' and answered with an edge from \
                         a {}",
                        task.id,
                        kind(edge.from.kind)
                    ),
                }
                .into());
            }
            // The near end is the task whose dependencies were asked for. An
            // edge answering about a different one would put another task's
            // prerequisites on this node, which is a graph nobody wrote and a
            // run nothing could explain afterwards.
            if edge.from.id != task.id {
                return Err(Error::Sibling {
                    tool: STORE,
                    message: format!(
                        "asked for the dependencies of '{}' and answered with an edge from \
                         '{}'",
                        task.id, edge.from.id
                    ),
                }
                .into());
            }
            // A plan's `deps` are the blocking ones; a `related` edge is a link the store
            // draws and not an ordering, so it is passed over rather than refused.
            if edge.kind != DependencyKind::Blocks {
                continue;
            }
            let far = &edge.to;
            let both = format!("'{}' depends on '{}'", task.id, far.id);
            // A plan node is a task, so a `project` end — which the store's edges may
            // carry, at either level — is refused rather than read as a node.
            if far.kind != ItemKind::Task {
                return Err(refused(
                    &whole,
                    format!(
                        "{both}, which is a {} and not a node of a plan",
                        kind(far.kind)
                    ),
                )
                .field("deps")
                .into());
            }
            let resolved = match ids.get(&far.id.to_string()) {
                Some(id) => id.clone(),
                // A far end outside this project is still resolved through its
                // own `onepipeline.id`, because that is what a node id is: the
                // one name that survives a copy between two sources. What it
                // resolves to is then this plan's business, and a node id no
                // node of this plan carries is a dangling dependency.
                None => {
                    let far_task = self.show(&far.id).map_err(|error| {
                        refused(&whole, format!("{both}, which could not be read: {error}"))
                            .field("deps")
                    })?;
                    node_id(&far_task.item.metadata).map_err(|why| {
                        refused(&whole, format!("{both}, and the far task {why}")).field("deps")
                    })?
                }
            };
            deps.push(resolved);
        }
        Ok(deps)
    }

    fn project(&self, project: &QualifiedId) -> Result<Qualified<Project>> {
        let id = project.global();
        let read = self
            .call(self.engine.project(&id))
            .map_err(|error| failed("project show", project.as_str(), &error))?;
        one(&id, "project show", read)
    }

    fn show(&self, task: &GlobalId) -> Result<Qualified<Task>> {
        let read = self
            .call(self.engine.task(task))
            .map_err(|error| failed("task show", &task.to_string(), &error))?;
        one(task, "task show", read)
    }

    /// Every task of one home project and of the member projects it names, each one
    /// that home's own or one of those members' own.
    ///
    /// A plan is a home project plus the member projects its `onetaskgraph.members`
    /// names, each in a source of its own, so the read asks the store for the home
    /// with its members and every task comes back qualified by its own source.
    ///
    /// Checked rather than assumed: the project selector is a filter this build asked a
    /// **third party**'s sources to apply, and a plan assembled out of an item from
    /// somewhere else would carry a node nobody put in this plan. Both halves
    /// of "somewhere else" are refused — a source holding neither the home nor a
    /// member, and another project of a source that does — because the two are
    /// different mistakes and neither is one a launch could report afterwards.
    /// Refused here, where the project asked for and the item answered with can both
    /// still be named.
    fn tasks(&self, project: &QualifiedId, members: &[GlobalId]) -> Result<Vec<Qualified<Task>>> {
        let selector = selector(&self.engine, project);
        let tasks = self.paged("task list", project.as_str(), |token| {
            let request = TaskRequest {
                sources: Vec::new(),
                filters: Filters::default(),
                priorities: Vec::new(),
                metadata: Vec::new(),
                origin: None,
                project: selector.clone(),
                commented_since: None,
                // Asked only of a home that names members: the store learns them by reading the
                // home again, and a plan of one source needs no second read of it.
                include_members: !members.is_empty(),
                paging: Paging {
                    limit: self.page,
                    token,
                },
            };
            async move { self.engine.tasks(&request).await }
        })?;
        for task in &tasks {
            if let Some(elsewhere) = stranger(task, project, members) {
                return Err(Error::Sibling {
                    tool: STORE,
                    message: format!(
                        "asked for the tasks of '{project}' and answered with '{}', which is \
                         {elsewhere}",
                        task.id
                    ),
                });
            }
        }
        Ok(tasks)
    }

    fn edges(&self, task: &GlobalId) -> Result<Vec<QualifiedEdge>> {
        self.paged("task deps", &task.to_string(), |token| {
            let request = DependencyRequest {
                id: task.clone(),
                direction: Direction::DependsOn,
                paging: Paging {
                    limit: self.page,
                    token,
                },
            };
            async move { self.engine.task_dependencies(&request).await }
        })
    }

    /// Every page of one query, walked to its end.
    ///
    /// A store pages, and a plan is the whole graph or it is not a plan: a
    /// launch that read the first page alone would execute a prefix of the
    /// project and never say which nodes it left out.
    ///
    /// A partial answer is not accepted: a source that could not answer is a plan this
    /// process cannot read, and a launch that proceeded on the sources that did answer
    /// would execute a graph missing whatever the absent one held.
    fn paged<T, F, Fut>(&self, query: &str, about: &str, page: F) -> Result<Vec<T>>
    where
        F: Fn(Option<PageToken>) -> Fut,
        Fut: Future<Output = std::result::Result<QueryResponse<T>, EngineError>>,
    {
        let mut all = Vec::new();
        let mut token: Option<PageToken> = None;
        let mut walked: std::collections::HashSet<PageToken> = std::collections::HashSet::new();
        loop {
            let response = self
                .call(page(token.clone()))
                .map_err(|error| failed(query, about, &error))?;
            partial(query, about, &response.errors)?;
            all.extend(response.items);
            let Some(next) = response.next else {
                return Ok(all);
            };
            // A walk ends because a page says it is the last one. A token this walk
            // has already been handed ends nothing: it revisits a page, for ever, and a
            // launch that hung there would never say what it was waiting for. Every
            // token this walk has seen rather than only the last, because a response
            // cycling through two of them repeats just as endlessly and looks like
            // progress.
            if !walked.insert(next.clone()) {
                return Err(Error::Sibling {
                    tool: STORE,
                    message: format!(
                        "`{query}` of '{about}' answered with a continuation token that does \
                         not advance the walk, so the whole project can never be read"
                    ),
                });
            }
            token = Some(next);
        }
    }
}

/// An engine's refusal to run one query, as the refusal a launch reports.
fn failed(query: &str, about: &str, error: &EngineError) -> Error {
    Error::Sibling {
        tool: STORE,
        message: format!("`{query}` of '{about}' failed: {error}"),
    }
}

/// A partial answer, refused naming each source that could not contribute.
fn partial(query: &str, about: &str, errors: &[SourceFailure]) -> Result<()> {
    if errors.is_empty() {
        return Ok(());
    }
    let named: Vec<String> = errors
        .iter()
        .map(|failure| {
            format!(
                "source {} could not answer: {}",
                failure.source, failure.error
            )
        })
        .collect();
    Err(Error::Sibling {
        tool: STORE,
        message: format!(
            "`{query}` of '{about}' answered in part: {}",
            named.join("; ")
        ),
    })
}

/// Why `task` is no task of the plan whose home is `home` and whose members are `members`, or
/// `None` where it is one: a task of the home's project in the home's source, or of a member's
/// project in that member's source.
fn stranger(
    task: &Qualified<Task>,
    home: &QualifiedId,
    members: &[GlobalId],
) -> Option<&'static str> {
    let expected = if task.id.source.as_str() == home.source() {
        Some(home.native())
    } else {
        members
            .iter()
            .find(|member| member.source == task.id.source)
            .map(|member| member.native.as_str())
    };
    match (&task.item.project, expected) {
        // llmlint: ignore[changed_behavior_has_e2e] the linked store answers a members read with the home's own project in its source and each member's in theirs, by construction, so no configuration a journey can write reaches this arm through the CLI; it defends a third party's source, and `taskgraph::tests::a_task_of_neither_the_home_nor_a_member_is_named_for_what_it_is` holds every arm.
        (_, None) => Some("an item of a source holding neither the home nor a member"),
        (Some(named), Some(expected)) if named.as_str() != expected => {
            Some("a task of another project")
        }
        (None, Some(_)) => Some("a task of no project at all"),
        (Some(_), Some(_)) => None,
    }
}

/// The member projects the home `home` names at `onetaskgraph.members`: none where it names
/// no such key.
///
/// External input, held to the store's own rule for the list rather than trimmed to what
/// parses: a list of qualified project ids, none in the home's source and at most one per
/// source. A list that breaks it is refused naming the home and why, because a plan read as
/// though it had fewer members than it does would leave a member's tasks out silently.
pub(crate) fn members_of(
    home: &GlobalId,
    metadata: &BTreeMap<String, Value>,
) -> std::result::Result<Vec<GlobalId>, String> {
    let key = MetadataKey::MEMBERS_KEY;
    let Some(value) = metadata.get(key) else {
        return Ok(Vec::new());
    };
    let refuse = |why: String| {
        format!(
            "{home} records {key} as {value}, which {why}; a home's member list is a list of \
             qualified project ids, at most one per source and none in the home's own"
        )
    };
    let Value::Array(entries) = value else {
        return Err(refuse("is not a list".to_owned()));
    };
    let mut members: Vec<GlobalId> = Vec::new();
    for entry in entries {
        let member = entry
            .as_str()
            .and_then(|entry| entry.parse::<GlobalId>().ok())
            .ok_or_else(|| refuse(format!("holds {entry}, which is not a qualified id")))?;
        if member.source == home.source || members.iter().any(|kept| kept.source == member.source) {
            return Err(refuse(format!(
                "names {member}, a second project in {}",
                member.source
            )));
        }
        members.push(member);
    }
    Ok(members)
}

/// What one end of a dependency edge names, as a refusal says it.
fn kind(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Task => "task",
        ItemKind::Project => "project",
    }
}

/// The node ids of one project, by the **whole** qualified id of the task
/// carrying each.
///
/// The whole id and not its native half: a lookup that matched on the half would
/// have to establish separately that the source agreed, and the pair of checks
/// is one thing said twice — with only one of them in the type.
type Ids = BTreeMap<String, String>;

/// The reserved id one task carries, or why it has none.
///
/// The reason alone, phrased to follow a subject, because both callers name a
/// different one: the task itself where a plan is being assembled, and the far
/// end of a dependency edge where one is being resolved.
fn node_id(metadata: &BTreeMap<String, Value>) -> std::result::Result<String, String> {
    match metadata.get(ID_KEY) {
        Some(Value::String(id)) if !id.trim().is_empty() => Ok(id.clone()),
        Some(Value::String(_)) | None => Err(format!(
            "carries no `{ID_KEY}`, which is the node id this plan's dependencies name it by"
        )),
        Some(other) => Err(format!(
            "carries `{ID_KEY}` as {other}, and a node id is a string"
        )),
    }
}

/// Read one assembled document through the schema, naming the field that
/// refused it.
///
/// `serde_json`'s own error says the type it wanted and not where it wanted it,
/// and a project's fields are metadata keys a person wrote by hand — so the path
/// is the whole of what makes the refusal actionable, and a nested one (a step's
/// own budget, say) is unreachable without it.
fn read<T: serde::de::DeserializeOwned>(document: Value) -> std::result::Result<T, String> {
    serde_path_to_error::deserialize(document).map_err(|error| match error.path().to_string() {
        path if path == "." => error.into_inner().to_string(),
        path => format!("{path}: {}", error.into_inner()),
    })
}

/// A refusal about one **task** of the project, named by the store id it has.
///
/// The store id and not the node id: a task carrying no `onepipeline.id` has no
/// node id to be named by, and the two halves of that mistake are both in the
/// store.
fn refused(id: &str, what: String) -> Refusal {
    Refusal::about(id, format!("{id}: {what}"))
}

/// A refusal about the **project**, which is no node of the plan it holds.
fn refused_plan(id: &str, what: String) -> Refusal {
    Refusal::plain(format!("{id}: {what}"))
}

/// One project, read.
pub(crate) struct Read {
    /// The plan the engine loaded, every default resolved.
    pub plan: Plan,
    /// Each task's own metadata map, verbatim, by node id.
    pub metadata: BTreeMap<String, Map<String, Value>>,
    /// Each task exactly as the store answered it, by node id: what the write-back's landed
    /// baseline is seeded from, so its first projection re-reads nothing the launch just read.
    pub tasks: BTreeMap<String, Qualified<Task>>,
    /// The project's own metadata, verbatim.
    pub project_metadata: BTreeMap<String, Value>,
    /// Each task's qualified id and its content exactly as stored, by node id: what a
    /// rendered-only check digests, before the mapping trims it into a node's `task`.
    pub stored: BTreeMap<String, StoredTask>,
}

pub(crate) struct StoredTask {
    pub qualified: QualifiedId,
    /// Byte for byte; empty where the task has none.
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoredKind {
    Task,
    Document,
}

pub(crate) struct StoredItem {
    pub content: String,
    pub metadata: BTreeMap<String, Value>,
}

/// Why a project did not become a plan.
///
/// The two are different answers and are kept apart: the schema **refused** what
/// the project says, or the project could not be **read** at all — a
/// configuration that does not load, a source that could not answer, a project
/// that names nothing. A caller
/// that collapsed them would report a store outage as a plan its author has to
/// fix.
pub(crate) enum Load {
    /// The schema refused it, and this is what about.
    Refused(Refusal),
    /// It could not be read.
    Unreadable(Error),
}

impl From<Refusal> for Load {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<Error> for Load {
    fn from(error: Error) -> Self {
        Self::Unreadable(error)
    }
}

impl From<Load> for Error {
    fn from(load: Load) -> Self {
        match load {
            Load::Refused(refusal) => refusal.into(),
            Load::Unreadable(error) => error,
        }
    }
}

/// Every id in this module: a `<source>:<native>` pair, parsed once where it
/// arrives and never re-split afterwards.
///
/// A bare id names nothing — a store may hold several sources and a native id is
/// only unique within one — so an unqualified one is refused at the boundary
/// rather than carried inwards for some later layer to notice. The boundary is the
/// id a person types and the ids a run records; every id the store answers with
/// arrives as the store's own [`GlobalId`], which that library has already
/// qualified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedId {
    whole: String,
    /// Where the colon is, so both halves are slices of `whole`.
    colon: usize,
}

impl QualifiedId {
    pub fn source(&self) -> &str {
        &self.whole[..self.colon]
    }

    /// May contain colons freely: the split is on the first.
    pub fn native(&self) -> &str {
        &self.whole[self.colon + 1..]
    }

    pub fn as_str(&self) -> &str {
        &self.whole
    }

    /// The same id as the store's own type, which is what every call into it names.
    ///
    /// Infallible, because the two agree on what a qualified id is: a source name the
    /// store's own [`SourceName`] parser accepted where this id was parsed, and a native id
    /// that is not empty.
    pub(crate) fn global(&self) -> GlobalId {
        GlobalId::new(
            SourceName::new(self.source())
                .expect("a qualified id's source is a source name the store accepts"),
            NativeId::from(self.native()),
        )
    }
}

impl From<&GlobalId> for QualifiedId {
    /// An id the store answered with, which that library has already qualified: its
    /// source name holds no colon, so the first one is where the two halves meet.
    fn from(id: &GlobalId) -> Self {
        let whole = id.to_string();
        let colon = whole
            .find(':')
            .expect("a store id is written `<source>:<native>`");
        Self { whole, colon }
    }
}

impl TryFrom<String> for QualifiedId {
    type Error = String;

    /// The source half is held to the store's own [`SourceName`] parser, whose words say
    /// what a source name is when one is not; the native half is the upstream system's
    /// opaque value, and only has to be there.
    fn try_from(whole: String) -> std::result::Result<Self, String> {
        let refused = |why: &str| {
            format!(
                "'{whole}' is not a qualified onetaskgraph id{why}; write it as \
                 <source>:<native>, for example plan-store:ship-the-widget"
            )
        };
        let Some(colon) = whole.find(':').filter(|colon| colon + 1 < whole.len()) else {
            return Err(refused(""));
        };
        match SourceName::new(&whole[..colon]) {
            Ok(_) => Ok(Self { whole, colon }),
            Err(error) => Err(refused(&format!(": {error}"))),
        }
    }
}

impl std::str::FromStr for QualifiedId {
    type Err = Error;

    fn from_str(id: &str) -> Result<Self> {
        Self::try_from(id.to_owned()).map_err(Error::Invalid)
    }
}

impl std::fmt::Display for QualifiedId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.whole.fmt(formatter)
    }
}

/// The one item a `show` of one id answered with.
///
/// Exactly one, and that one's own id: a `show` addresses a single item, so a
/// response carrying several — or carrying one that is not the item asked for —
/// is a store this build cannot read a plan out of rather than a set to pick the
/// first of. Taking the first would mean a plan assembled out of items nobody
/// named, which is the one failure a launch cannot report afterwards.
///
/// An answer holding nothing and reporting no failure is an item that is **not
/// there**, which is the store's own reading of an empty `show` — its CLI decided
/// it, and a caller of the library decides it the same way. An answer holding
/// nothing because a source failed is a store that could not be read, which is a
/// different thing to tell a person.
fn one<T>(
    id: &GlobalId,
    query: &str,
    response: QueryResponse<Qualified<T>>,
) -> Result<Qualified<T>> {
    partial(query, &id.to_string(), &response.errors)?;
    if response.next.is_some() {
        return Err(Error::Sibling {
            tool: STORE,
            message: format!("asked to show '{id}' and the answer claims another page"),
        });
    }
    let mut items = response.items.into_iter();
    let Some(found) = items.next() else {
        return Err(Error::Invalid(format!(
            "'{id}' names nothing in the configured sources"
        )));
    };
    if items.next().is_some() {
        return Err(Error::Sibling {
            tool: STORE,
            message: format!("asked for '{id}' and answered with more than one item"),
        });
    }
    if found.id != *id {
        return Err(Error::Sibling {
            tool: STORE,
            message: format!("asked for '{id}' and answered with '{}'", found.id),
        });
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use onetaskgraph_core::Config;
    use serde_json::json;

    use super::*;

    #[test]
    fn a_qualified_id_is_split_on_its_first_colon_and_a_bare_one_is_refused() {
        let id: QualifiedId = "plan-store:ship".parse().expect("a qualified id");
        assert_eq!(id.source(), "plan-store");
        assert_eq!(id.native(), "ship");
        assert_eq!(id.as_str(), "plan-store:ship");
        // A native id may contain colons freely; the split is on the first.
        let nested: QualifiedId = "plan-store:a:b".parse().expect("a qualified id");
        assert_eq!(nested.native(), "a:b");
        // And it names the same item as the store's own id type.
        assert_eq!(nested.global().to_string(), "plan-store:a:b");
        assert_eq!(nested.global().native.as_str(), "a:b");

        let message = "ship".parse::<QualifiedId>().unwrap_err().to_string();
        assert!(message.contains("<source>:<native>"), "{message}");
        assert!(":ship".parse::<QualifiedId>().is_err());
        assert!("plan-store:".parse::<QualifiedId>().is_err());
        assert!("Plan Store:ship".parse::<QualifiedId>().is_err());
        assert!("plan_store:ship".parse::<QualifiedId>().is_err());
        // A native id is the upstream system's opaque value.
        assert!("plan-store:ship it".parse::<QualifiedId>().is_ok());
    }

    #[test]
    fn a_task_carrying_no_node_id_is_refused_by_the_key_it_is_missing() {
        let mut metadata = BTreeMap::new();
        let message = node_id(&metadata).unwrap_err();
        assert!(message.contains(ID_KEY), "{message}");
        assert!(message.starts_with("carries no"), "{message}");

        metadata.insert(ID_KEY.to_owned(), Value::from(7));
        let message = node_id(&metadata).unwrap_err();
        assert!(message.contains("a node id is a string"), "{message}");
    }

    // What follows reads plans through the **linked** store: `onetaskgraph-core`'s own
    // engine over its own `in-memory` plugin, configured the way an operator's
    // `onetaskgraph.yaml` configures one. Nothing here stands in for the store; the
    // fixture is the store's data.

    /// A status every fixture task carries: the store requires one on every item.
    fn todo() -> Value {
        json!({"category": "todo", "name": "Todo"})
    }

    /// One task of the `ship` project, carrying `metadata` and whatever `rest` adds.
    fn task(id: &str, metadata: Value, rest: Value) -> Value {
        let mut task = json!({
            "id": id,
            "title": format!("Do {id}"),
            "content": format!("## What\nDo {id}.\n\n## Why\nSo it is done.\n\n## Acceptance criteria\n- {id} is done."),
            "status": todo(),
            "labels": [],
            "project": "ship",
            "metadata": metadata,
        });
        if let (Value::Object(task), Value::Object(rest)) = (&mut task, rest) {
            task.extend(rest);
        }
        task
    }

    /// A node's reserved metadata: its id and a persona.
    fn node(id: &str) -> Value {
        json!({"onepipeline.id": id, "onepipeline.persona": "engineer"})
    }

    /// A store of one `in-memory` source called `plans`, holding the `ship` project with
    /// `project_metadata`, these tasks, and these task-level edges.
    fn store(project_metadata: Value, tasks: Vec<Value>, edges: Vec<Value>) -> Built {
        store_paged(project_metadata, tasks, edges, 50)
    }

    fn store_paged(
        project_metadata: Value,
        tasks: Vec<Value>,
        edges: Vec<Value>,
        page: u32,
    ) -> Built {
        let mut project_metadata = project_metadata;
        project_metadata
            .as_object_mut()
            .expect("project metadata is a mapping")
            .entry("onepipeline.schema_version")
            .or_insert(json!(crate::plan::PLAN_SCHEMA_VERSION));
        let config = Config::from_document(json!({
            "page_size": page,
            "sources": {"plans": {"plugin": "in-memory", "config": {
                "projects": [{
                    "id": "ship",
                    "title": "Ship the widget",
                    "status": todo(),
                    "labels": [],
                    "metadata": project_metadata,
                }],
                "tasks": tasks,
                "task_dependencies": edges,
            }}},
        }))
        .expect("the fixture configuration is one the store accepts");
        Built {
            engine: Engine::build(
                &config,
                &onetaskgraph_core::Secrets::load(Environment::default()).expect("no secrets"),
            ),
            page: config.page_size(),
        }
    }

    fn load(built: Built) -> std::result::Result<Read, Load> {
        Reader::over(built)
            .expect("a runtime")
            .load(&"plans:ship".parse().expect("a qualified id"))
    }

    fn refusal(load: std::result::Result<Read, Load>) -> String {
        match load {
            Err(Load::Refused(refusal)) => Error::from(refusal).to_string(),
            Err(Load::Unreadable(error)) => panic!("the project was unreadable: {error}"),
            Ok(read) => panic!("the project was read: {:?}", read.plan.tasks),
        }
    }

    /// Project = plan, task = node, dependency edge = dependency: the mapping read off the
    /// store's own values, reserved metadata included and everything else carried beside it.
    #[test]
    fn a_project_of_the_linked_store_reads_as_the_plan_it_holds() {
        let read = load(store(
            json!({"onepipeline.concurrency": 2, "onepipeline.goal": {"text": "Ship it"}}),
            vec![
                task(
                    "t-build",
                    node("build"),
                    json!({"repositories": ["github.com/acme/widget"]}),
                ),
                task(
                    "t-test",
                    json!({
                        "onepipeline.id": "test",
                        "onepipeline.persona": "engineer",
                        "onepipeline.repo": "/srv/checkouts/widget",
                        "team.owner": "qa",
                    }),
                    json!({}),
                ),
                task(
                    "t-ship",
                    node("ship"),
                    json!({"delivers": ["t-ticket", "tickets:T-9"]}),
                ),
            ],
            vec![
                json!({"from": "t-test", "to": "t-build", "kind": "blocks"}),
                json!({"from": "t-ship", "to": "t-test", "kind": "blocks"}),
                // A link the store draws and not an ordering.
                json!({"from": "t-ship", "to": "t-build", "kind": "related"}),
            ],
        ))
        .unwrap_or_else(|_| panic!("the project reads as a plan"));
        let plan = &read.plan;
        assert_eq!(
            plan.name.as_deref(),
            Some("Ship the widget"),
            "the title names the plan"
        );
        assert_eq!(plan.concurrency, 2);
        let nodes: BTreeMap<&str, &Node> = plan
            .tasks
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes["build"].title.as_deref(), Some("Do t-build"));
        assert!(nodes["build"]
            .task
            .as_deref()
            .is_some_and(|task| task.contains("Do t-build.")));
        assert_eq!(
            nodes["build"].repo.as_deref(),
            Some("github.com/acme/widget")
        );
        assert_eq!(nodes["test"].repo.as_deref(), Some("/srv/checkouts/widget"));
        assert_eq!(nodes["test"].deps, vec!["build".to_owned()]);
        assert_eq!(
            nodes["ship"].deps,
            vec!["test".to_owned()],
            "a related link is no dependency"
        );
        assert_eq!(
            nodes["ship"].delivers,
            vec!["plans:t-ticket".to_owned(), "tickets:T-9".to_owned()],
            "a bare entry names the task's own source"
        );
        // Each task's own metadata, verbatim, keys this engine does not read included.
        assert_eq!(read.metadata["test"]["team.owner"], json!("qa"));
    }

    /// A body's surrounding whitespace is the store's layout, so a step is handed the body
    /// without it, and a body that is only whitespace is no task prose at all.
    #[test]
    fn a_task_body_is_handed_over_without_the_whitespace_around_it() {
        let read = load(store(
            json!({}),
            vec![task(
                "t-padded",
                node("padded"),
                json!({"content": "\n## What\nDo padded.\n\n## Acceptance criteria\n- padded is done.\n\n"}),
            )],
            vec![],
        ))
        .unwrap_or_else(|_| panic!("the project reads as a plan"));
        assert_eq!(
            read.plan.tasks[0].task.as_deref(),
            Some("## What\nDo padded.\n\n## Acceptance criteria\n- padded is done.")
        );

        let message = refusal(load(store(
            json!({}),
            vec![task("t-blank", node("blank"), json!({"content": " \n\n"}))],
            vec![],
        )));
        assert!(
            message.contains("'blank'") && message.contains("task prose"),
            "{message}"
        );
    }

    /// A store too large for one page is read to its end, so a plan is never a prefix.
    #[test]
    fn a_project_larger_than_one_page_is_read_to_its_end() {
        let tasks = (0..7)
            .map(|n| task(&format!("t-{n}"), node(&format!("n{n}")), json!({})))
            .collect();
        let edges = (1..7)
            .map(|n| json!({"from": format!("t-{n}"), "to": format!("t-{}", n - 1), "kind": "blocks"}))
            .collect();
        let read = load(store_paged(json!({}), tasks, edges, 2))
            .unwrap_or_else(|_| panic!("the project reads as a plan"));
        assert_eq!(read.plan.tasks.len(), 7);
        let last = read
            .plan
            .tasks
            .iter()
            .find(|node| node.id == "n6")
            .expect("the last node");
        assert_eq!(last.deps, vec!["n5".to_owned()]);
    }

    /// Every input the contract refuses is refused before anything is dispatched, naming
    /// the field or node and what to change.
    #[test]
    fn every_input_the_contract_refuses_is_refused_naming_what_to_change() {
        let one = |metadata: Value| vec![task("t-a", metadata, json!({}))];
        let cases: Vec<(&str, Built, &[&str])> = vec![
            (
                "the project's tasks",
                store(json!({"onepipeline.tasks": []}), one(node("a")), vec![]),
                &["onepipeline.tasks", "the project's own tasks"],
            ),
            (
                "a non-cross-DAG dep",
                store(
                    json!({}),
                    one(
                        json!({"onepipeline.id": "a", "onepipeline.persona": "engineer", "onepipeline.deps": ["b"]}),
                    ),
                    vec![],
                ),
                &["plans:t-a", "onepipeline.deps", "dependency edge"],
            ),
            (
                "a reserved key filled from the task",
                store(
                    json!({}),
                    one(
                        json!({"onepipeline.id": "a", "onepipeline.persona": "engineer", "onepipeline.title": "x"}),
                    ),
                    vec![],
                ),
                &["plans:t-a", "onepipeline.title", "the task's own `title`"],
            ),
            (
                "a repo named both ways",
                store(
                    json!({}),
                    vec![task(
                        "t-a",
                        json!({"onepipeline.id": "a", "onepipeline.persona": "engineer", "onepipeline.repo": "/srv/a"}),
                        json!({"repositories": ["github.com/acme/a"]}),
                    )],
                    vec![],
                ),
                &["plans:t-a", "both `repositories` and `onepipeline.repo`"],
            ),
            (
                "a task with no node id",
                store(
                    json!({}),
                    one(json!({"onepipeline.persona": "engineer"})),
                    vec![],
                ),
                &["plans:t-a", "carries no `onepipeline.id`"],
            ),
            (
                "a retired field",
                store(
                    json!({"onepipeline.max_parallel": 2}),
                    one(node("a")),
                    vec![],
                ),
                &["max_parallel"],
            ),
            (
                "a cycle",
                store(
                    json!({}),
                    vec![
                        task("t-a", node("a"), json!({})),
                        task("t-b", node("b"), json!({})),
                    ],
                    vec![
                        json!({"from": "t-a", "to": "t-b", "kind": "blocks"}),
                        json!({"from": "t-b", "to": "t-a", "kind": "blocks"}),
                    ],
                ),
                &["cycle"],
            ),
        ];
        for (case, built, said) in cases {
            let message = refusal(load(built));
            for words in said {
                assert!(
                    message.contains(words),
                    "{case}: {message:?} does not say {words:?}"
                );
            }
        }
    }

    /// A store of three `local-md` sources under a scratch directory of its own: `plans`,
    /// holding the home `ship`, whose `onetaskgraph.members` names `linear:ship-linear`;
    /// `linear`, holding that member; and `other`, holding a project that is no part of the
    /// plan. `tasks` are `(source, project, file, node id, edges)`, each edge a qualified task.
    fn spanning(name: &str, tasks: &[(&str, &str, &str, &str, &[&str])]) -> Built {
        let dir = std::env::temp_dir().join(format!(
            "onepipeline-spanning-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |path: PathBuf, front: Value| {
            std::fs::create_dir_all(path.parent().expect("a folder")).expect("a store folder");
            let yaml = serde_norway::to_string(&front).expect("front matter");
            std::fs::write(
                path,
                format!(
                    "---\n{yaml}---\n## What\nDo it.\n\n## Acceptance criteria\n- It is done.\n"
                ),
            )
            .expect("a document");
        };
        for (source, project, metadata) in [
            (
                "plans",
                "ship",
                json!({"onepipeline.schema_version": crate::plan::PLAN_SCHEMA_VERSION,
                       "onetaskgraph.members": ["linear:ship-linear"]}),
            ),
            (
                "linear",
                "ship-linear",
                json!({"onetaskgraph.member_of": "plans:ship"}),
            ),
            ("other", "elsewhere", json!({})),
        ] {
            write(
                dir.join(source)
                    .join("projects")
                    .join(format!("{project}.md")),
                json!({"title": project, "metadata": metadata}),
            );
        }
        for (source, project, file, id, edges) in tasks {
            let depends_on: Vec<Value> = edges
                .iter()
                .map(|to| json!({"id": to, "kind": "blocks", "item": "task"}))
                .collect();
            write(
                dir.join(source)
                    .join("tasks")
                    .join(project)
                    .join(format!("{file}.md")),
                json!({"title": format!("Do {id}"), "project": project, "depends_on": depends_on,
                       "metadata": node(id)}),
            );
        }
        let source = |name: &str| json!({"plugin": "local-md", "config": {"root": dir.join(name).to_string_lossy()}});
        let config = Config::from_document(json!({
            "sources": {"plans": source("plans"), "linear": source("linear"), "other": source("other")},
        }))
        .expect("the fixture configuration is one the store accepts");
        Built {
            engine: Engine::build(
                &config,
                &onetaskgraph_core::Secrets::load(Environment::default()).expect("no secrets"),
            ),
            page: config.page_size(),
        }
    }

    /// A plan is a home project plus the member projects it names: one read answers the
    /// tasks of both sources, an edge between two of them resolves to a dependency in either
    /// direction, and a member's task records the source its item is in.
    #[test]
    fn a_home_and_its_member_read_as_one_plan_with_edges_both_ways() {
        let read = load(spanning(
            "both-ways",
            &[
                ("plans", "ship", "core", "core", &[]),
                (
                    "linear",
                    "ship-linear",
                    "adopt",
                    "adopt",
                    &["plans:ship/core"],
                ),
                (
                    "plans",
                    "ship",
                    "ship",
                    "ship",
                    &["linear:ship-linear/adopt"],
                ),
            ],
        ))
        .unwrap_or_else(|load| panic!("the plan reads: {}", Error::from(load)));
        let nodes: BTreeMap<&str, &Node> = read
            .plan
            .tasks
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        assert_eq!(nodes.len(), 3, "{:?}", read.plan.tasks);
        assert_eq!(nodes["adopt"].deps, vec!["core".to_owned()]);
        assert_eq!(nodes["ship"].deps, vec!["adopt".to_owned()]);
        let record = |id: &str| nodes[id].task_record.clone().expect("a task record");
        assert_eq!(
            record("adopt").source.as_ref().map(SourceName::as_str),
            Some("linear")
        );
        assert_eq!(record("adopt").id, "ship-linear/adopt");
        assert_eq!(
            record("core").source,
            None,
            "a home task's record names no source"
        );
        assert_eq!(
            read.stored["adopt"].qualified.as_str(),
            "linear:ship-linear/adopt"
        );
    }

    /// An edge onto a task of a source and project that is neither the home nor a member is
    /// outside the plan, and is refused as it always was, naming the task it leaves.
    #[test]
    fn an_edge_onto_a_task_outside_the_home_and_its_members_is_refused() {
        let refused = refusal(load(spanning(
            "outside",
            &[
                ("plans", "ship", "core", "core", &["other:elsewhere/stray"]),
                ("other", "elsewhere", "stray", "stray", &[]),
            ],
        )));
        assert!(
            refused.contains("node 'core' depends on 'stray', which is not in the plan"),
            "{refused}"
        );
    }

    /// A home's member list is held to the store's own rule: a list of qualified ids, none in
    /// the home's source and at most one per source. Anything else is refused naming the home,
    /// and a plan read over it is unreadable rather than a plan with fewer members.
    #[test]
    fn a_member_list_breaking_the_stores_rule_is_refused() {
        let home: GlobalId = "plans:ship".parse().expect("an id");
        let listing = |value: Value| BTreeMap::from([(MetadataKey::MEMBERS_KEY.to_owned(), value)]);
        assert_eq!(members_of(&home, &BTreeMap::new()), Ok(Vec::new()));
        assert_eq!(
            members_of(&home, &listing(json!(["linear:ship-linear"]))),
            Ok(vec!["linear:ship-linear"
                .parse::<GlobalId>()
                .expect("an id")])
        );
        for (value, why) in [
            (json!("linear:ship-linear"), "is not a list"),
            (json!(["bare"]), "which is not a qualified id"),
            (json!([7]), "which is not a qualified id"),
            (json!(["plans:other"]), "a second project in plans"),
            (
                json!(["linear:a", "linear:b"]),
                "a second project in linear",
            ),
        ] {
            let refused = members_of(&home, &listing(value.clone()))
                .expect_err(&format!("{value} was read as a member list"));
            assert!(
                refused.contains(why) && refused.contains("plans:ship"),
                "{refused}"
            );
        }

        let unreadable = load(store(
            json!({MetadataKey::MEMBERS_KEY: "linear:ship-linear"}),
            vec![task("t-a", node("a"), json!({}))],
            vec![],
        ));
        match unreadable {
            Err(Load::Unreadable(error)) => {
                assert!(error.to_string().contains("is not a list"), "{error}");
            }
            Err(Load::Refused(refusal)) => panic!("refused: {}", Error::from(refusal)),
            Ok(_) => panic!("a plan was read over a member list the store's rule refuses"),
        }
    }

    /// A task the store answers with is the plan's only where it is a task of the home's
    /// project in the home's source or of a member's project in that member's source; any
    /// other is named for what it is.
    #[test]
    fn a_task_of_neither_the_home_nor_a_member_is_named_for_what_it_is() {
        let home: QualifiedId = "plans:ship".parse().expect("a qualified id");
        let members = ["linear:ship-linear".parse::<GlobalId>().expect("an id")];
        let answered = |id: &str, project: Option<&str>| {
            let mut task: Task =
                serde_json::from_value(task("t", node("n"), json!({"project": project})))
                    .expect("a task");
            task.project = project.map(NativeId::from);
            Qualified {
                id: id.parse::<GlobalId>().expect("an id"),
                item: task,
            }
        };
        assert_eq!(
            stranger(&answered("plans:t", Some("ship")), &home, &members),
            None
        );
        assert_eq!(
            stranger(&answered("linear:t", Some("ship-linear")), &home, &members),
            None
        );
        assert_eq!(
            stranger(&answered("other:t", Some("ship")), &home, &members),
            Some("an item of a source holding neither the home nor a member")
        );
        assert_eq!(
            stranger(&answered("linear:t", Some("ship")), &home, &members),
            Some("a task of another project")
        );
        assert_eq!(
            stranger(&answered("plans:t", Some("ship-linear")), &home, &members),
            Some("a task of another project")
        );
        assert_eq!(
            stranger(&answered("plans:t", None), &home, &members),
            Some("a task of no project at all")
        );
    }

    /// A store that cannot be read is reported as unreadable — a store outage — and never
    /// as a plan its author has to fix.
    #[test]
    fn a_store_that_cannot_be_read_is_unreadable_rather_than_refused() {
        let gone =
            std::env::temp_dir().join(format!("onepipeline-no-store-{}", std::process::id()));
        let config = Config::from_document(json!({
            "sources": {"plans": {"plugin": "local-md", "config": {"root": gone}}},
        }))
        .expect("a configuration the store accepts");
        let built = Built {
            engine: Engine::build(
                &config,
                &onetaskgraph_core::Secrets::load(Environment::default()).expect("no secrets"),
            ),
            page: config.page_size(),
        };
        match load(built) {
            Err(Load::Unreadable(error)) => {
                let message = error.to_string();
                assert!(message.contains("plans:ship"), "{message}");
            }
            Err(Load::Refused(refusal)) => {
                panic!("a store outage read as a refusal: {}", refusal.message)
            }
            Ok(_) => panic!("a store that is not there was read"),
        }
        // And a project the store answers nothing for is one that is not there.
        let missing = Reader::over(store(json!({}), vec![], vec![]))
            .expect("a runtime")
            .load(&"plans:elsewhere".parse().expect("a qualified id"));
        match missing {
            Err(Load::Unreadable(Error::Invalid(message))) => {
                assert!(message.contains("names nothing"), "{message}");
            }
            Err(Load::Unreadable(other)) => panic!("an absent project read as {other}"),
            Err(Load::Refused(refusal)) => {
                panic!("an absent project was refused: {}", refusal.message)
            }
            Ok(_) => panic!("an absent project was read"),
        }
    }

    /// The retired `ONETASKGRAPH_BIN` never reaches the store's configuration: it names
    /// nothing now, and a host that still sets it reads its plans exactly as one that does
    /// not.
    #[test]
    fn the_retired_binary_variable_never_reaches_the_stores_configuration() {
        // Each test binary's environment is its own process's; this test owns this key.
        std::env::set_var(RETIRED_BINARY_ENV, "/nowhere/onetaskgraph");
        let handed = environment();
        std::env::remove_var(RETIRED_BINARY_ENV);
        assert_eq!(handed.get(RETIRED_BINARY_ENV), None);
        let dir = std::env::temp_dir().join(format!("onepipeline-bin-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        std::fs::write(
            dir.join("onetaskgraph.yaml"),
            "sources:\n  plans:\n    plugin: in-memory\n",
        )
        .expect("a store configuration");
        let loaded = Store {
            dir: dir.clone(),
            environment: handed,
        }
        .engine(&Layer::default());
        let _ = std::fs::remove_dir_all(&dir);
        let built = loaded.unwrap_or_else(|error| panic!("the store was not configured: {error}"));
        assert!(
            built
                .engine
                .has(&SourceName::new("plans").expect("a source name")),
            "the store discovered from the directory does not hold its source"
        );
    }

    /// A scratch directory of this test's own, holding `onetaskgraph.yaml` as given.
    fn configured(name: &str, document: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("onepipeline-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        std::fs::write(dir.join("onetaskgraph.yaml"), document).expect("a store configuration");
        dir
    }

    /// Whether the store the configuration describes built its `board` source, or why not.
    fn board(store: &Store) -> std::result::Result<(), String> {
        let built = store
            .engine(&Layer::default())
            .unwrap_or_else(|error| panic!("the configuration loads: {error}"));
        let listed = built
            .engine
            .listing()
            .into_iter()
            .find(|listing| listing.source.as_str() == "board")
            .expect("the configuration's board source is listed");
        match listed.state {
            onetaskgraph_core::SourceState::Available { .. } => Ok(()),
            onetaskgraph_core::SourceState::Unavailable { error } => Err(error.to_string()),
        }
    }

    /// A source's token is read from the variable its own configuration names — out of the
    /// engine's own environment, or out of the `secrets.env` that environment names — and an
    /// `ONETASKGRAPH_*` setting in that environment is layered over the discovered document,
    /// exactly as the store's own CLI read each of them.
    #[test]
    fn a_sources_token_and_settings_come_from_the_engines_own_environment() {
        const TOKEN: &str = "ONEPIPELINE_TASKGRAPH_TEST_BOARD_TOKEN";
        let dir = configured(
            "credentials",
            &format!(
                "sources:\n  board:\n    plugin: github-projects\n    config:\n      owner: acme\n      \
                 project_number: 1\n      token_env: {TOKEN}\n      endpoint: http://127.0.0.1:9/graphql\n"
            ),
        );
        let bare = Store {
            dir: dir.clone(),
            environment: Environment::default(),
        };
        let refused = board(&bare).expect_err("a source with no token is not built");
        assert!(
            refused.contains(TOKEN),
            "the refusal does not name the variable: {refused}"
        );

        // The engine's own environment, read through the constructor the write-back uses.
        std::env::set_var(TOKEN, "a-token");
        let exported = Store::at(&dir);
        std::env::remove_var(TOKEN);
        assert_eq!(board(&exported), Ok(()), "the exported token was not read");

        // The `secrets.env` the environment names.
        let secrets = dir.join("secrets.env");
        std::fs::write(&secrets, format!("{TOKEN}=a-token\n")).expect("a secrets file");
        let filed = Store {
            dir: dir.clone(),
            environment: Environment::from_pairs([(
                "ONETASKGRAPH_SECRETS_FILE",
                secrets.to_string_lossy().into_owned(),
            )]),
        };
        assert_eq!(
            board(&filed),
            Ok(()),
            "the token in secrets.env was not read"
        );

        // And a setting in the environment, layered over the document it discovered.
        let layered = Store {
            dir: dir.clone(),
            environment: Environment::from_pairs([(
                "ONETASKGRAPH_SOURCES__PLANS__PLUGIN",
                "in-memory",
            )]),
        };
        let built = layered
            .engine(&Layer::default())
            .expect("the layered configuration loads");
        assert!(
            built
                .engine
                .has(&SourceName::new("plans").expect("a source name")),
            "a source declared in the environment was not layered over the document"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The `status_mapping` an ai-orchestrator launch's tracked `onetaskgraph.yaml` carries for
    /// its Linear source, indented as it sits under that source's `config`: a name per kind
    /// where a Linear workspace's project statuses are a different vocabulary from its team's
    /// issue states, and one bare name where both say the same.
    const PER_KIND_MAPPING: &str = "      status_mapping:
        backlog:     { task: Proposed,        project: Proposal }
        draft:       { task: Backlog,         project: Idea }
        todo:        { task: Todo,            project: Planned }
        queued:      { task: Queued,          project: Accepted }
        in-progress: In Progress
        unknown:     { task: Needs Attention, project: Blocked }
        done:        { task: Done,            project: Completed }
        cancelled:   Canceled
";

    /// Each named source the configuration discovered from `dir` describes, built or refused —
    /// or, where the configuration itself does not load, why not. Read through [`Store::engine`],
    /// which is how a launch's plan read and every write-back attempt build the store.
    fn sources(
        dir: &std::path::Path,
        environment: Environment,
    ) -> std::result::Result<BTreeMap<String, std::result::Result<(), String>>, String> {
        let store = Store {
            dir: dir.to_path_buf(),
            environment,
        };
        let built = store
            .engine(&Layer::default())
            .map_err(|error| error.to_string())?;
        Ok(built
            .engine
            .listing()
            .into_iter()
            .map(|listing| {
                let state = match listing.state {
                    onetaskgraph_core::SourceState::Available { .. } => Ok(()),
                    onetaskgraph_core::SourceState::Unavailable { error } => Err(error.to_string()),
                };
                (listing.source.as_str().to_owned(), state)
            })
            .collect())
    }

    /// One `status_mapping` grammar across both hosted plugins this crate links: a `linear`
    /// source mapping each category to a name per item kind, or to one name for both, and a
    /// `github-projects` source mapping each to one bare name, both build from the
    /// `onetaskgraph.yaml` a launch discovers. A per-kind object naming a kind the grammar does
    /// not have refuses the configuration, naming the source it was written under.
    #[test]
    fn a_status_mapping_scoped_by_item_kind_builds_and_an_unknown_kind_is_refused_by_source() {
        const LINEAR_KEY: &str = "ONEPIPELINE_TASKGRAPH_TEST_LINEAR_KEY";
        const BOARD_TOKEN: &str = "ONEPIPELINE_TASKGRAPH_TEST_BOARD_TOKEN";
        let environment =
            || Environment::from_pairs([(LINEAR_KEY, "a-linear-key"), (BOARD_TOKEN, "a-token")]);
        let document = |linear_mapping: &str| {
            format!(
                "sources:\n  hellopatient:\n    plugin: linear\n    config:\n      \
                 api_key_env: {LINEAR_KEY}\n      team: HP\n      \
                 endpoint: http://127.0.0.1:9/graphql\n{linear_mapping}  board:\n    \
                 plugin: github-projects\n    config:\n      owner: acme\n      \
                 project_number: 1\n      token_env: {BOARD_TOKEN}\n      \
                 endpoint: http://127.0.0.1:9/graphql\n      status_mapping:\n        \
                 todo: Todo\n        queued: Queued\n        in-progress: In Progress\n        \
                 unknown: Needs Attention\n        done: Done\n        cancelled: Canceled\n"
            )
        };
        let mapped = configured("status-by-kind", &document(PER_KIND_MAPPING));
        let built = sources(&mapped, environment()).expect("the configuration loads");
        assert_eq!(
            built.get("hellopatient"),
            Some(&Ok(())),
            "the per-kind Linear mapping did not build: {built:?}"
        );
        assert_eq!(
            built.get("board"),
            Some(&Ok(())),
            "the bare-name GitHub Projects mapping did not build: {built:?}"
        );

        let misspelt = PER_KIND_MAPPING.replace(
            "{ task: Todo,            project: Planned }",
            "{ task: Todo,            projects: Planned }",
        );
        assert_ne!(
            misspelt, PER_KIND_MAPPING,
            "the fixture names the kind it misspells"
        );
        let refused = configured("status-by-kind-refused", &document(&misspelt));
        // Refused as the configuration is read, before any source is built: the whole store is
        // one an operator has to correct, and the message says where.
        let why = sources(&refused, environment())
            .expect_err("a per-kind object naming no item kind was accepted");
        assert!(
            why.contains("sources.hellopatient") && why.contains("\"projects\""),
            "the refusal names neither the source nor the key it refused: {why}"
        );
        let _ = std::fs::remove_dir_all(&mapped);
        let _ = std::fs::remove_dir_all(&refused);
    }

    /// Each plan read builds its engine from the configuration as it stands, so a store
    /// reconfigured between two reads is read as it now is rather than as the first read
    /// found it.
    #[test]
    fn each_read_builds_its_store_from_the_configuration_afresh() {
        let dir = configured("afresh", "sources:\n  plans:\n    plugin: in-memory\n");
        let store = Store {
            dir: dir.clone(),
            environment: Environment::default(),
        };
        let first = store.engine(&Layer::default()).expect("a configuration");
        assert!(first
            .engine
            .has(&SourceName::new("plans").expect("a source name")));
        std::fs::write(
            dir.join("onetaskgraph.yaml"),
            "sources:\n  work:\n    plugin: in-memory\n",
        )
        .expect("the configuration changes");
        let second = store.engine(&Layer::default()).expect("a configuration");
        assert!(
            second
                .engine
                .has(&SourceName::new("work").expect("a source name"))
                && !second
                    .engine
                    .has(&SourceName::new("plans").expect("a source name")),
            "a later read answered from the store an earlier read built"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
