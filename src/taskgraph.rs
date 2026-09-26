//! Where a plan comes from: the `onetaskgraph` store, read through its binary.
//!
//! A run is launched by naming a **qualified onetaskgraph project id**, and a
//! plan is one project of that store: the plan-level settings are reserved
//! `onepipeline.<field>` metadata keys on the project, a node is one task in it,
//! and a dependency is a real dependency edge between two of those tasks.
//! `docs/contract.md` fixes that mapping; this module is the only place that
//! performs it.
//!
//! **The binary is driven, not linked.** That is `onetaskgraph`'s own recorded
//! decision for both of its SDKs, and it is what keeps a crates.io release
//! ordering out of every `onepipeline` release. The executable is resolved from
//! [`BINARY_ENV`] when that names one and from [`DEFAULT_BINARY`] on the `PATH`
//! otherwise, and its version is checked **before anything is dispatched** — an
//! absent binary, an unusable one, or one below [`CHECKED_MINIMUM`] refuses the
//! launch, naming the path it resolved, the version it found, the minimum it
//! needs, and how to install one.
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
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::plan::{Node, Plan};
use crate::refusal::Refusal;

/// The environment variable naming the `onetaskgraph` executable.
///
/// Taken out of the child's own environment before it is spawned. That product
/// reads its whole configuration from `ONETASKGRAPH_`-prefixed variables — the
/// suffix is a dotted setting path — so a binary told where it is would be a
/// binary told to configure a setting called `bin`, and would refuse the read.
/// `docs/contract-divergences.md` records the collision.
pub const BINARY_ENV: &str = "ONETASKGRAPH_BIN";

/// The executable's name when the environment names none.
pub const DEFAULT_BINARY: &str = "onetaskgraph";

/// The version floor a launch **checks**: the oldest `onetaskgraph` this build
/// will read a plan through.
///
/// What the mapping needs is the reserved metadata map, which landed after that
/// product's 0.1.0 release and ships in every release from 0.2.0 — so this floor
/// separates an install carrying the map from one that does not, exactly, and a
/// host with the released 0.1.0 is refused by version rather than left to work out
/// why every task of its project reads as unidentified.
///
/// `justfile`'s `onetaskgraph-version` is the release the checks install, and
/// [`the_release_the_checks_install_meets_the_floor_and_is_named_once`](tests::the_release_the_checks_install_meets_the_floor_and_is_named_once)
/// fails if that release falls below this floor.
const CHECKED_MINIMUM: Version = Version {
    major: 0,
    minor: 2,
    patch: 0,
    release: Release::Released,
};

/// How a host that has no `onetaskgraph` gets one.
///
/// Named in every refusal this module makes about the binary, because "not
/// found" without it leaves the one actionable thing unsaid.
pub const INSTALL: &str = "install it with `cargo install onetaskgraph`, \
     `uv tool install onetaskgraph-cli`, or `npm install -g onetaskgraph-cli`, \
     or set ONETASKGRAPH_BIN to an executable one";

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

/// The `onetaskgraph` binary this process reads its plans through.
///
/// Construction performs the version check, which makes it a **launch-time**
/// fact rather than a per-command one: nothing downstream can reach the binary
/// through this value having skipped it.
#[derive(Debug, Clone)]
pub struct Store {
    binary: PathBuf,
    /// The version token that check read, as the binary printed it.
    // llmlint: ignore[invalid_states_unrepresentable] kept as the token the binary printed
    // because it is written verbatim into the write-back's store record for a reader; the
    // check above has already parsed it as a `Version`, and `at_least` re-parses it for the
    // one comparison anything makes of it.
    version: String,
}

impl Store {
    /// The checked executable, for the best-effort write-back worker.
    pub(crate) fn binary(&self) -> PathBuf {
        self.binary.clone()
    }

    /// The version the check read, for the write-back worker to decide what the store offers.
    ///
    /// Read off the `--version` the launch check already asked, so deciding it spends no
    /// store command of its own.
    pub(crate) fn reported_version(&self) -> &str {
        &self.version
    }
    /// Resolve the binary and check what it reports before anything is
    /// dispatched.
    ///
    /// Every ending here is a refusal naming the path resolved, and — where
    /// there was one to read — the version found, the minimum needed, and how
    /// to install one.
    pub fn resolve() -> Result<Self> {
        let binary = resolved_binary();
        let named = |what: String| Error::Sibling {
            tool: DEFAULT_BINARY,
            message: format!(
                "{} ({what}); onepipeline reads a plan out of a onetaskgraph project and \
                 needs {DEFAULT_BINARY} {CHECKED_MINIMUM} or newer, the first release \
                 carrying the reserved metadata this mapping reads — {INSTALL}",
                binary.display()
            ),
        };
        let reported = Command::new(&binary)
            .arg("--version")
            .env_remove(BINARY_ENV)
            .output()
            .map_err(|error| named(format!("cannot be run: {error}")))?;
        if !reported.status.success() {
            return Err(named(format!(
                "refused `--version`: {}",
                first_line(&reported.stderr)
                    .or_else(|| first_line(&reported.stdout))
                    .unwrap_or_else(|| format!("exit {}", code_of(&reported.status)))
            )));
        }
        let printed = std::str::from_utf8(&reported.stdout)
            .map_err(|error| named(format!("reported a version that is not UTF-8: {error}")))?;
        let token = version_token(printed).unwrap_or_default();
        let version = Version::parse(token).ok_or_else(|| {
            named(format!(
                "reported no version this build can read: {:?}",
                printed.trim()
            ))
        })?;
        if version < CHECKED_MINIMUM {
            return Err(named(format!("is version {token}, below the minimum")));
        }
        Ok(Self {
            binary,
            version: token.to_owned(),
        })
    }

    /// Read one qualified project id as the plan it holds.
    ///
    /// The project is external input, so every refusal it earns is made here,
    /// before a run is minted: a reserved key of the wrong JSON type, a key no
    /// plan field answers to, a task carrying no node id, and a dependency edge
    /// whose far end this plan cannot name.
    pub fn plan(&self, project: &QualifiedId) -> Result<Plan> {
        self.load(project)
            .map(|read| read.plan)
            .map_err(Error::from)
    }

    /// The same read, keeping both halves a checker needs.
    ///
    /// The loaded plan, and each task's own metadata map **verbatim** — including
    /// the keys outside this consumer's reserved namespace, which the mapping
    /// above drops because no plan field answers to them. A consumer's check
    /// reads keys this engine does not, so what it is handed is the engine's
    /// resolved node beside the store's own record of the task it came from.
    pub(crate) fn read_plan(&self, project: &QualifiedId) -> std::result::Result<Read, Load> {
        self.load(project)
    }

    fn load(&self, project: &QualifiedId) -> std::result::Result<Read, Load> {
        let held = self.project(project)?;
        let tasks = self.tasks(project)?;

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
            let id = node_id(&task.item)
                .map_err(|why| refused(task.id.as_str(), format!("it {why}")).field("id"))?;
            if let Some(first) = claimed.insert(id.clone(), task.id.to_string()) {
                return Err(refused(
                    task.id.as_str(),
                    format!("`{ID_KEY}` '{id}' is already the id of another task, '{first}'"),
                )
                .field("id")
                .into());
            }
            ids.insert(task.id.as_str().to_owned(), id);
        }

        let mut nodes = Vec::with_capacity(tasks.len());
        for task in &tasks {
            nodes.push(self.node(task, &ids)?);
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
            if let Some(version) = document["schema_version"]
                .as_u64()
                .filter(|v| *v < u64::from(crate::plan::PLAN_SCHEMA_VERSION))
            {
                if node.get("sets").is_some() {
                    return Err(refused(
                        task.id.as_str(),
                        crate::plan::node_field_is_newer("sets", version as u32),
                    )
                    .field("sets")
                    .into());
                }
            }
            read::<Node>(node.clone()).map_err(|error| {
                refused(
                    task.id.as_str(),
                    format!(
                        "{error} — a node's fields are the reserved `{RESERVED}<field>` \
                         metadata keys on its task"
                    ),
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
            .zip(tasks.iter().map(|task| task.item.metadata.clone()))
            .collect();
        Ok(Read { plan, metadata })
    }

    /// One node, assembled out of its task.
    fn node(&self, task: &Qualified<TaskItem>, ids: &Ids) -> std::result::Result<Value, Load> {
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
                    task.id.as_str(),
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
        if let Some(content) = task.item.content.as_ref().filter(|c| !c.trim().is_empty()) {
            node.insert("task".to_owned(), Value::String(content.clone()));
        }
        // The store prints every entry qualified; one that arrives bare names this task's
        // own source, which is the store's rule for a bare id. Anything still not a
        // qualified id is refused by the graph's node check, naming the node and the entry.
        if !task.item.delivers.is_empty() {
            let qualified = task
                .item
                .delivers
                .iter()
                .map(|entry| match entry.contains(':') {
                    true => Value::String(entry.clone()),
                    false => Value::String(format!("{}:{entry}", task.id.source())),
                })
                .collect();
            node.insert("delivers".to_owned(), Value::Array(qualified));
        }
        // The task's human-facing record, which is what a branch this node's session
        // cuts is named from. A blank key is the store's way of carrying none, as a
        // blank title is.
        let mut record = Map::new();
        record.insert("id".to_owned(), Value::String(task.id.native().to_owned()));
        if let Some(key) = task.item.key.as_ref().filter(|key| !key.trim().is_empty()) {
            record.insert("key".to_owned(), Value::String(key.clone()));
        }
        record.insert("title".to_owned(), Value::String(task.item.title.clone()));
        node.insert("task_record".to_owned(), Value::Object(record));
        match (task.item.repositories.first(), node.get("repo")) {
            (Some(_), Some(_)) => {
                return Err(refused(
                    task.id.as_str(),
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
                refused(task.id.as_str(), format!("`{RESERVED}deps` {error}")).field("deps")
            })?;
            for reference in listed {
                // Anything spelled as a cross-DAG reference is carried, well
                // formed or not: a malformed one is answered by the graph
                // module, which says what the shape has to be. What is refused
                // here is a dependency that never meant to leave this run.
                if !reference.starts_with(crate::crossdag::PREFIX) {
                    return Err(refused(
                        task.id.as_str(),
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
    fn deps(
        &self,
        task: &Qualified<TaskItem>,
        ids: &Ids,
    ) -> std::result::Result<Vec<String>, Load> {
        let mut deps = Vec::new();
        for edge in self.edges(&task.id)? {
            if edge.from.kind != ItemKind::Task {
                return Err(Error::Sibling {
                    tool: DEFAULT_BINARY,
                    message: format!(
                        "asked for the dependencies of task '{}' and answered with an edge from \
                         a {}",
                        task.id, edge.from.kind
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
                    tool: DEFAULT_BINARY,
                    message: format!(
                        "asked for the dependencies of '{}' and answered with an edge from \
                         '{}'",
                        task.id, edge.from.id
                    ),
                }
                .into());
            }
            if edge.kind != DependencyKind::Blocks {
                continue;
            }
            let far = &edge.to;
            let both = format!("'{}' depends on '{}'", task.id, far.id);
            if far.kind != ItemKind::Task {
                return Err(refused(
                    task.id.as_str(),
                    format!("{both}, which is a {} and not a node of a plan", far.kind),
                )
                .field("deps")
                .into());
            }
            let resolved = match ids.get(far.id.as_str()) {
                Some(id) => id.clone(),
                // A far end outside this project is still resolved through its
                // own `onepipeline.id`, because that is what a node id is: the
                // one name that survives a copy between two sources. What it
                // resolves to is then this plan's business, and a node id no
                // node of this plan carries is a dangling dependency.
                None => {
                    let far_task = self.show(&far.id).map_err(|error| {
                        refused(
                            task.id.as_str(),
                            format!("{both}, which could not be read: {error}"),
                        )
                        .field("deps")
                    })?;
                    node_id(&far_task.item).map_err(|why| {
                        refused(task.id.as_str(), format!("{both}, and the far task {why}"))
                            .field("deps")
                    })?
                }
            };
            deps.push(resolved);
        }
        Ok(deps)
    }

    fn project(&self, project: &QualifiedId) -> Result<Qualified<ProjectItem>> {
        let read = self.read::<Qualified<ProjectItem>>(&["project", "show", project.as_str()])?;
        one(project, read)
    }

    fn show(&self, task: &QualifiedId) -> Result<Qualified<TaskItem>> {
        let read = self.read::<Qualified<TaskItem>>(&["task", "show", task.as_str()])?;
        one(task, read)
    }

    /// Every task of one project, each one that project's own.
    ///
    /// Checked rather than assumed: `--project` is a filter this build asked a
    /// **third party** to apply, and a plan assembled out of an item from
    /// somewhere else would carry a node nobody put in this project. Both halves
    /// of "somewhere else" are refused — another source, and another project of
    /// this one — because the two are different mistakes and neither is one a
    /// launch could report afterwards. Refused here, where the project asked for
    /// and the item answered with can both still be named.
    fn tasks(&self, project: &QualifiedId) -> Result<Vec<Qualified<TaskItem>>> {
        let tasks: Vec<Qualified<TaskItem>> =
            self.paged(&["task", "list", "--project", project.as_str()])?;
        for task in &tasks {
            let elsewhere = match &task.item.project {
                _ if task.id.source() != project.source() => Some("an item of another source"),
                Some(named) if named != project.native() => Some("a task of another project"),
                None => Some("a task of no project at all"),
                Some(_) => None,
            };
            if let Some(elsewhere) = elsewhere {
                return Err(Error::Sibling {
                    tool: DEFAULT_BINARY,
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

    fn edges(&self, task: &QualifiedId) -> Result<Vec<Edge>> {
        self.paged(&["task", "deps", task.as_str(), "--direction", "depends-on"])
    }

    /// Every page of one query, walked to its end.
    ///
    /// A store pages, and a plan is the whole graph or it is not a plan: a
    /// launch that read the first page alone would execute a prefix of the
    /// project and never say which nodes it left out.
    fn paged<T: serde::de::DeserializeOwned>(&self, args: &[&str]) -> Result<Vec<T>> {
        let mut all = Vec::new();
        let mut page: Option<String> = None;
        let mut walked: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        loop {
            let mut args = args.to_vec();
            if let Some(token) = page.as_deref() {
                args.extend_from_slice(&["--page", token]);
            }
            let response = self.read::<T>(&args)?;
            all.extend(response.items);
            let Some(token) = response.next else {
                return Ok(all);
            };
            // A walk ends because a page says it is the last one. A token that
            // is empty, or that this walk has already been handed, ends nothing:
            // it revisits a page, for ever, and a launch that hung there would
            // never say what it was waiting for. Every token this walk has seen
            // rather than only the last, because a response cycling through two
            // of them repeats just as endlessly and looks like progress.
            if token.is_empty() || !walked.insert(token.clone()) {
                return Err(Error::Sibling {
                    tool: DEFAULT_BINARY,
                    message: format!(
                        "`{DEFAULT_BINARY} {}` answered with a continuation token that does \
                         not advance the walk, so the whole project can never be read",
                        args.join(" ")
                    ),
                });
            }
            page = Some(token);
        }
    }

    /// One `--json` query, refused where the store refuses it.
    ///
    /// `--allow-partial` is deliberately not passed: a source that could not
    /// answer is a plan this process cannot read, and a launch that proceeded
    /// on the sources that did answer would execute a graph missing whatever
    /// the absent one held.
    fn read<T: serde::de::DeserializeOwned>(&self, args: &[&str]) -> Result<Response<T>> {
        let output = Command::new(&self.binary)
            .args(args)
            .arg("--json")
            .env_remove(BINARY_ENV)
            .output()
            .map_err(|error| Error::Sibling {
                tool: DEFAULT_BINARY,
                message: format!("{} cannot be run: {error}", self.binary.display()),
            })?;
        if !output.status.success() {
            return Err(Error::Sibling {
                tool: DEFAULT_BINARY,
                message: format!(
                    "`{DEFAULT_BINARY} {}` exited {}: {}",
                    args.join(" "),
                    code_of(&output.status),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        serde_json::from_slice(&output.stdout).map_err(|error| Error::Sibling {
            tool: DEFAULT_BINARY,
            message: format!(
                "`{DEFAULT_BINARY} {}` answered with something this build cannot read: {error}",
                args.join(" ")
            ),
        })
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
fn node_id(task: &TaskItem) -> std::result::Result<String, String> {
    match task.metadata.get(ID_KEY) {
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
}

/// Why a project did not become a plan.
///
/// The two are different answers and are kept apart: the schema **refused** what
/// the project says, or the project could not be **read** at all — an absent
/// binary, a store that answered badly, a project that names nothing. A caller
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
/// rather than carried inwards for some later layer to notice. That boundary is
/// both directions: the id a person types on the command line, and every id the
/// store answers with, which is a third party's output and is read through the
/// same type.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
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
}

impl TryFrom<String> for QualifiedId {
    type Error = String;

    fn try_from(whole: String) -> std::result::Result<Self, String> {
        match whole.find(':') {
            Some(colon) if valid_source_name(&whole[..colon]) && colon + 1 < whole.len() => {
                Ok(Self { whole, colon })
            }
            _ => Err(format!(
                "'{whole}' is not a qualified onetaskgraph id; write it as <source>:<native>, \
                 for example plan-store:ship-the-widget; source names use lower-case letters, \
                 digits and hyphens, starting with a letter or digit"
            )),
        }
    }
}

/// The source-name language onetaskgraph publishes for qualified ids.
///
/// Native ids stay opaque (and may contain colons or whitespace) because they
/// belong to the upstream system; the configured source name is the component
/// onetaskgraph itself validates.
fn valid_source_name(source: &str) -> bool {
    let mut chars = source.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
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
fn one<T>(id: &QualifiedId, response: Response<Qualified<T>>) -> Result<Qualified<T>> {
    if response.next.is_some() {
        return Err(Error::Sibling {
            tool: DEFAULT_BINARY,
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
            tool: DEFAULT_BINARY,
            message: format!("asked for '{id}' and answered with more than one item"),
        });
    }
    if found.id != *id {
        return Err(Error::Sibling {
            tool: DEFAULT_BINARY,
            message: format!("asked for '{id}' and answered with '{}'", found.id),
        });
    }
    Ok(found)
}

fn first_line(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

fn code_of(status: &std::process::ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "on a signal".to_owned(), |code| code.to_string())
}

fn resolved_binary() -> PathBuf {
    match std::env::var_os(BINARY_ENV) {
        Some(named) if named.to_str().is_some_and(|value| value.trim().is_empty()) => {
            PathBuf::from(DEFAULT_BINARY)
        }
        Some(named) if !named.is_empty() => PathBuf::from(named),
        _ => PathBuf::from(DEFAULT_BINARY),
    }
}

/// A version, ordered the way semantic versioning orders one.
///
/// The three numbers, and then whether the version is a release at all: a
/// pre-release sorts **below** the release it precedes, so `0.2.0-rc.1` does not
/// satisfy a floor of `0.2.0`. That is the direction that cannot go wrong — a
/// release candidate is by definition a build of something not yet released, and
/// a floor is a statement about what has shipped.
///
/// The field order is the comparison order, which is what `derive(Ord)` gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u32,
    minor: u32,
    patch: u32,
    release: Release,
}

/// Whether a version names a release or something before one.
///
/// Declared in comparison order: everything before a release is below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Release {
    /// A pre-release of the version beside it — `-rc.1`, `-alpha`.
    Prerelease,
    /// The release itself.
    Released,
}

impl Version {
    /// One `MAJOR[.MINOR[.PATCH]][-PRERELEASE][+BUILD]` token, or `None`.
    ///
    /// The grammar, rather than a prefix of it: `vv1.2.3`, `1.2.3-` and `1.2.3+`
    /// are not versions, and reading them as `1.2.3` would let a binary printing
    /// something malformed decide a floor.
    fn parse(token: &str) -> Option<Self> {
        // At most one `v`, and only leading.
        let token = token.strip_prefix('v').unwrap_or(token);
        // The build comes **after** the pre-release — `1.2.3-rc.1+build.7` — so
        // it is split off first; the other order puts the build inside the
        // pre-release and refuses a version that is perfectly well formed.
        let (rest, build) = match token.split_once('+') {
            Some((rest, build)) => (rest, Some(build)),
            None => (token, None),
        };
        let (numbers, prerelease) = match rest.split_once('-') {
            Some((numbers, prerelease)) => (numbers, Some(prerelease)),
            None => (rest, None),
        };
        // A pre-release and a build are dot-separated identifiers of ASCII
        // alphanumerics and hyphens. Anything else is not a version, and reading
        // it as one would let malformed output decide the floor.
        if prerelease.is_some_and(|value| !identifiers(value, true))
            || build.is_some_and(|value| !identifiers(value, false))
        {
            return None;
        }
        let number = |component: &str| {
            (!(component.len() > 1 && component.starts_with('0')))
                .then(|| component.parse::<u32>().ok())
                .flatten()
        };
        let mut parts = numbers.split('.');
        let major = number(parts.next()?)?;
        let minor = number(parts.next().unwrap_or("0"))?;
        let patch = number(parts.next().unwrap_or("0"))?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            release: match prerelease {
                None => Release::Released,
                Some(_) => Release::Prerelease,
            },
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)?;
        match self.release {
            Release::Prerelease => formatter.write_str("-<pre-release>"),
            Release::Released => Ok(()),
        }
    }
}

/// Whether `text` is the dot-separated identifiers a pre-release or a build is.
fn identifiers(text: &str, prerelease: bool) -> bool {
    !text.is_empty()
        && text.split('.').all(|part| {
            !part.is_empty()
                && !(prerelease
                    && part.len() > 1
                    && part.starts_with('0')
                    && part.chars().all(|character| character.is_ascii_digit()))
                && part
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

/// The version token in what a `--version` printed, or `None`.
///
/// The **first** line, and either its only token or the second of exactly two —
/// which is what every `--version` in this stack prints, `NAME VERSION`. Read
/// any looser (the last token of the output, say) a binary printing a banner, a
/// path, or a build hash would have some fragment of it parsed as a version, and
/// the floor would be decided by whatever happened to sit at the end.
fn version_token(printed: &str) -> Option<&str> {
    let line = printed.lines().find(|line| !line.trim().is_empty())?;
    let mut tokens = line.split_whitespace();
    let first = tokens.next()?;
    let token = tokens.next().unwrap_or(first);
    tokens.next().is_none().then_some(token)
}

/// Whether a version token `--version` printed names `release` or a later one.
///
/// Either side failing to read as a version is *not* at least, so a store whose version
/// cannot be read is taken to offer only what every release does.
pub(crate) fn at_least(reported: &str, release: &str) -> bool {
    match (Version::parse(reported), Version::parse(release)) {
        (Some(reported), Some(release)) => reported >= release,
        _ => false,
    }
}

/// What every `--json` query answers with, in the shape the store writes it.
///
/// Read leniently, as every sibling's output is: a field this build does not
/// name is that product's to add, and a query that grew one is not a query this
/// process should refuse.
#[derive(Debug, Deserialize)]
struct Response<T> {
    items: Vec<T>,
    #[serde(default)]
    next: Option<String>,
}

/// One item and the qualified id it was read under.
#[derive(Debug, Deserialize)]
struct Qualified<T> {
    id: QualifiedId,
    item: T,
}

#[derive(Debug, Deserialize)]
struct ProjectItem {
    title: String,
    #[serde(default)]
    metadata: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct TaskItem {
    /// The short handle the task's source shows people, from `onetaskgraph` 0.2.44 on;
    /// absent, or null, where the source has none.
    #[serde(default)]
    key: Option<String>,
    title: String,
    /// The project this task belongs to, as the store says it does.
    ///
    /// `None` is a first-class case in that product — an orphan task — and it is
    /// simply not a task of any project this build could have asked for.
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    metadata: Map<String, Value>,
    #[serde(default)]
    repositories: Vec<Repository>,
    /// The tasks this one delivers, as the store holds the relation.
    // llmlint: ignore[invalid_states_unrepresentable] each entry is checked as a qualified id
    // by `graph::check_node`, which names the node and the entry; narrowing it here would
    // refuse a bare entry the store's own rule reads as naming this task's source.
    #[serde(default)]
    delivers: Vec<String>,
}

/// A repository identity in the normalized form onetaskgraph emits.
#[derive(Debug, Deserialize)]
#[serde(try_from = "String")]
struct Repository(String);

impl Repository {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Repository {
    type Error = String;

    fn try_from(origin: String) -> std::result::Result<Self, Self::Error> {
        let valid = !origin.is_empty()
            && !origin.contains("://")
            && !origin.ends_with(".git")
            && !origin.chars().any(char::is_whitespace)
            && origin.split('/').count() >= 3
            && origin
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..");
        valid.then_some(Self(origin.clone())).ok_or_else(|| {
            format!(
                "{origin:?} is not a normalized repository origin; use host/owner/name without \
                 a scheme or .git suffix"
            )
        })
    }
}

#[derive(Debug, Deserialize)]
struct Edge {
    /// The item that depends — the one whose dependencies were asked for.
    from: Endpoint,
    to: Endpoint,
    kind: DependencyKind,
}

#[derive(Debug, Deserialize)]
struct Endpoint {
    id: QualifiedId,
    kind: ItemKind,
}

/// What a dependency edge means.
///
/// A plan's `deps` are the blocking ones; a `related` edge is a link the store
/// draws and not an ordering, so it is passed over rather than refused.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum DependencyKind {
    /// The near item depends on the far one.
    Blocks,
    /// A link without an ordering.
    Related,
}

/// What one end of a dependency edge names.
///
/// A plan node is a task, so a `project` end — which the store's edges may carry,
/// at either level — is refused rather than read as a node.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "String")]
enum ItemKind {
    /// One task.
    Task,
    /// One project.
    Project,
    /// A kind that product added after this build, kept as it arrived so a
    /// refusal can name what it actually said.
    Unknown(String),
}

impl From<String> for ItemKind {
    fn from(wire: String) -> Self {
        match wire.as_str() {
            "task" => Self::Task,
            "project" => Self::Project,
            _ => Self::Unknown(wire),
        }
    }
}

impl std::fmt::Display for ItemKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Task => formatter.write_str("task"),
            Self::Project => formatter.write_str("project"),
            Self::Unknown(wire) => write!(formatter, "'{wire}'"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn version(printed: &str) -> Option<Version> {
        Version::parse(version_token(printed)?)
    }

    #[test]
    fn a_version_is_read_off_the_first_line_the_binary_prints() {
        assert_eq!(version("onetaskgraph 0.1.0\n"), Version::parse("0.1.0"));
        assert_eq!(version("onetaskgraph v1.2.3"), Version::parse("1.2.3"));
        assert_eq!(version("2"), Version::parse("2.0.0"));
        // A second line is not where a version is, and neither is a third token:
        // a banner or a build hash beside the number would otherwise decide the
        // floor.
        assert_eq!(version("onetaskgraph 0.1.0 (abc1234)"), None);
        assert_eq!(version("\nonetaskgraph 0.1.0"), Version::parse("0.1.0"));
        assert_eq!(version(""), None);
        assert_eq!(version("onetaskgraph what"), None);
        assert_eq!(version("onetaskgraph 1.2.3.4"), None);
        // The grammar rather than a prefix of it: a separator with nothing after
        // it, and a second `v`, are malformed rather than `1.2.3`.
        assert_eq!(Version::parse("vv1.2.3"), None);
        assert_eq!(Version::parse("1.2.3-"), None);
        assert_eq!(Version::parse("1.2.3+"), None);
        // A pre-release and a build are dot-separated identifiers, so an empty
        // one and a character that is not in the grammar are not versions.
        assert_eq!(Version::parse("1.2.3-rc..1"), None);
        assert_eq!(Version::parse("1.2.3-rc 1"), None);
        assert_eq!(Version::parse("1.2.3+build_7"), None);
        assert_eq!(Version::parse("01.2.3"), None);
        assert_eq!(Version::parse("1.02.3"), None);
        assert_eq!(Version::parse("1.2.3-01"), None);
        assert!(Version::parse("1.2.3-rc.01").is_none());
        assert!(Version::parse("1.2.3+01").is_some());
        assert!(Version::parse("1.2.3-rc.1+build-7").is_some());
    }

    /// A pre-release sorts below the release it precedes, which is the direction
    /// that cannot go wrong: a floor is a statement about what has shipped, and
    /// a release candidate is by construction a build of something that has not.
    #[test]
    fn versions_order_by_each_number_in_turn_and_a_prerelease_below_its_release() {
        let read = |token: &str| Version::parse(token).expect("a version");
        // The released 0.1.0 predates the metadata map, and is below the floor.
        assert!(read("0.1.0") < CHECKED_MINIMUM);
        assert!(read("0.1.9") < CHECKED_MINIMUM);
        assert!(read("0.2.0") >= CHECKED_MINIMUM);
        assert!(read("1.0.0") > CHECKED_MINIMUM);
        assert!(read("0.2.0-rc.1") < CHECKED_MINIMUM);
        assert!(read("0.2.1-rc.1") > CHECKED_MINIMUM);
        // A build suffix is not a pre-release: it names the same release.
        assert_eq!(read("0.2.0+build.7"), CHECKED_MINIMUM);
    }

    #[test]
    fn a_qualified_id_is_split_on_its_first_colon_and_a_bare_one_is_refused() {
        let id: QualifiedId = "plan-store:ship".parse().expect("a qualified id");
        assert_eq!(id.source(), "plan-store");
        assert_eq!(id.native(), "ship");
        assert_eq!(id.as_str(), "plan-store:ship");
        // A native id may contain colons freely; the split is on the first.
        let nested: QualifiedId = "plan-store:a:b".parse().expect("a qualified id");
        assert_eq!(nested.native(), "a:b");

        let message = "ship".parse::<QualifiedId>().unwrap_err().to_string();
        assert!(message.contains("<source>:<native>"), "{message}");
        assert!(":ship".parse::<QualifiedId>().is_err());
        assert!("plan-store:".parse::<QualifiedId>().is_err());
        assert!("Plan Store:ship".parse::<QualifiedId>().is_err());
        assert!("plan_store:ship".parse::<QualifiedId>().is_err());
        // A native id is the upstream system's opaque value.
        assert!("plan-store:ship it".parse::<QualifiedId>().is_ok());
        // And an id the *store* answers with crosses the same boundary: it is a
        // third party's output, read through the one type.
        assert!(serde_json::from_str::<QualifiedId>("\"bare\"").is_err());
    }

    #[test]
    fn repository_origins_are_validated_when_the_store_response_is_read() {
        assert!(serde_json::from_str::<Repository>("\"github.com/acme/widget\"").is_ok());
        for invalid in [
            "https://github.com/acme/widget",
            "github.com/acme/widget.git",
            "github.com/acme",
            "github.com/acme/white space",
            "github.com/acme/../widget",
        ] {
            let message = serde_json::from_str::<Repository>(&format!("{invalid:?}"))
                .unwrap_err()
                .to_string();
            assert!(
                message.contains("normalized repository origin"),
                "{message}"
            );
        }
    }

    #[test]
    fn the_binary_is_the_environments_when_it_names_one_and_the_name_on_the_path_otherwise() {
        // Each test runs in its own process, so this environment is this test's.
        let named = std::path::Path::new("/opt/onetaskgraph/bin/onetaskgraph");
        std::env::set_var(BINARY_ENV, named);
        assert_eq!(resolved_binary(), named);

        // A variable that is set to nothing names nothing.
        std::env::set_var(BINARY_ENV, "   ");
        assert_eq!(resolved_binary(), PathBuf::from(DEFAULT_BINARY));

        std::env::remove_var(BINARY_ENV);
        assert_eq!(resolved_binary(), PathBuf::from(DEFAULT_BINARY));
    }

    /// The release the checks install meets the floor this file declares, and the
    /// recipe that installs it reads it from the one place it is named.
    ///
    /// The justfile's `onetaskgraph-version` is that place. A release below the
    /// floor would be checks exercising an install every launch refuses, and a
    /// second copy of the number — or an install line that stopped reading the
    /// variable — would be a pin that could move away from it without anything
    /// saying so.
    #[test]
    fn the_release_the_checks_install_meets_the_floor_and_is_named_once() {
        let justfile = include_str!("../justfile");
        let declared = justfile
            .lines()
            .find_map(|line| line.strip_prefix("onetaskgraph-version := \""))
            .and_then(|rest| rest.strip_suffix('"'))
            .expect("the justfile names the onetaskgraph release its checks install");
        let installed = Version::parse(declared).expect("the named release is a version");
        assert!(
            installed >= CHECKED_MINIMUM,
            "the checks install onetaskgraph {declared}, below the {CHECKED_MINIMUM} \
             every launch requires"
        );
        assert_eq!(
            justfile.matches(declared).count(),
            1,
            "the justfile names onetaskgraph {declared} more than once"
        );

        let recipe: Vec<&str> = justfile
            .lines()
            .skip_while(|line| !line.starts_with("_ensure-onetaskgraph:"))
            .skip(1)
            .take_while(|line| line.starts_with(' '))
            .collect();
        let installs: Vec<&&str> = recipe
            .iter()
            .filter(|line| line.contains("cargo install onetaskgraph"))
            .collect();
        assert!(
            !installs.is_empty(),
            "`_ensure-onetaskgraph` no longer installs onetaskgraph"
        );
        for install in installs {
            assert!(
                install.contains("--version {{onetaskgraph-version}}"),
                "`_ensure-onetaskgraph` installs without reading `onetaskgraph-version`: \
                 {install}"
            );
            assert!(
                !install.contains("--git") && !install.contains("--rev"),
                "`_ensure-onetaskgraph` installs an unreleased onetaskgraph: {install}"
            );
        }
    }

    /// Every `onetaskgraph` package a lock carries, as the name and version each
    /// one resolved at.
    ///
    /// A set of pairs rather than a map from name to version, because a lock may
    /// carry one crate twice and that is the state worth catching: keyed by name
    /// alone, the second copy would overwrite the first and the split would read as
    /// a single clean resolution.
    fn onetaskgraph_packages(lock: &toml::Value) -> BTreeSet<(&str, &str)> {
        lock["package"]
            .as_array()
            .expect("a lock is a list of packages")
            .iter()
            .filter_map(|package| {
                let name = package.get("name")?.as_str()?;
                name.starts_with("onetaskgraph-")
                    .then(|| Some((name, package.get("version")?.as_str()?)))?
            })
            .collect()
    }

    /// A lock carrying one of these crates twice reports both copies, so the
    /// version check above sees the one that is wrong instead of only the one that
    /// happened to be listed last.
    #[test]
    fn a_lock_that_resolved_one_onetaskgraph_crate_twice_reports_both_copies() {
        let lock: toml::Value = toml::from_str(
            "[[package]]\nname = \"onetaskgraph-core\"\nversion = \"0.2.30\"\n\n\
             [[package]]\nname = \"onetaskgraph-core\"\nversion = \"0.2.32\"\n\n\
             [[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n",
        )
        .expect("the fixture lock is TOML");
        assert_eq!(
            onetaskgraph_packages(&lock),
            BTreeSet::from([
                ("onetaskgraph-core", "0.2.30"),
                ("onetaskgraph-core", "0.2.32"),
            ]),
            "a crate resolved twice was collapsed, or a crate of another family was taken"
        );
    }

    /// Every `onetaskgraph` crate this build **links** resolves to the release the
    /// checks **install**, so the plugin `label-strict-source` hosts in process and
    /// the binary every plan is read through are one store rather than two.
    ///
    /// The lock rather than the manifest, because four of the six are transitive:
    /// `[workspace.dependencies]` binds `onetaskgraph-core` and
    /// `onetaskgraph-local-md` alone, and the family is lock-step across patch
    /// releases despite the carets in its own manifests — 0.2.30's `local-md` does
    /// not compile against 0.2.32's plugin API, so a lock that split the family
    /// would not build and one that moved it whole would build against a release
    /// nothing here installs.
    ///
    /// The two direct crates are asserted present as well as pinned: a link quietly
    /// dropped would leave this test passing over an empty set, which is the one
    /// answer it must not give.
    #[test]
    fn every_onetaskgraph_crate_in_the_lock_is_the_release_the_checks_install() {
        let justfile = include_str!("../justfile");
        let declared = justfile
            .lines()
            .find_map(|line| line.strip_prefix("onetaskgraph-version := \""))
            .and_then(|rest| rest.strip_suffix('"'))
            .expect("the justfile names the onetaskgraph release its checks install");

        let lock: toml::Value =
            toml::from_str(include_str!("../Cargo.lock")).expect("the lock is TOML");
        let linked = onetaskgraph_packages(&lock);

        for crate_name in ["onetaskgraph-core", "onetaskgraph-local-md"] {
            assert!(
                linked.iter().any(|(name, _)| *name == crate_name),
                "the lock no longer carries {crate_name}, which `crates/testfakes` links \
                 to host the real local-md plugin in process"
            );
        }
        let elsewhere: BTreeSet<(&str, &str)> = linked
            .iter()
            .copied()
            .filter(|(_, version)| *version != declared)
            .collect();
        assert!(
            elsewhere.is_empty(),
            "the checks install onetaskgraph {declared} and the lock links {elsewhere:?}"
        );

        let manifest: toml::Value =
            toml::from_str(include_str!("../Cargo.toml")).expect("this manifest is TOML");
        let required = &manifest["workspace"]["dependencies"];
        for crate_name in [
            "onetaskgraph-core",
            "onetaskgraph-local-md",
            "onetaskgraph-plugin-api",
        ] {
            assert_eq!(
                required[crate_name].as_str(),
                Some(format!("={declared}").as_str()),
                "{crate_name} is not required at exactly the release the checks install"
            );
        }
    }

    /// What this module reads off a task is what the plugin contract declares a task
    /// to be: a task serialized by the contract's own type, with a key and without one,
    /// reads back as the key it carries. A rename on that side fails here rather than
    /// reading every task as keyless.
    #[test]
    fn a_task_the_plugin_contract_serializes_reads_back_with_its_key() {
        use onetaskgraph_plugin_api::{NativeId, Status, StatusCategory, Task};
        let task = |key: Option<&str>| Task {
            id: NativeId::from("tasks/build.md"),
            key: key.map(str::to_owned),
            title: "Build it".into(),
            content: None,
            status: Status {
                category: StatusCategory::Todo,
                name: "todo".into(),
            },
            labels: Vec::new(),
            project: None,
            url: None,
            location: None,
            created_at: None,
            updated_at: None,
            metadata: BTreeMap::new(),
            repositories: Vec::new(),
            delivers: Vec::new(),
            delivered_by: Vec::new(),
        };
        for key in [Some("ENG-123"), None] {
            let written = serde_json::to_value(task(key)).expect("a task serializes");
            let read: TaskItem = serde_json::from_value(written).expect("the item reads");
            assert_eq!(read.key.as_deref(), key);
            assert_eq!(read.title, "Build it");
        }
    }

    #[test]
    fn a_task_carrying_no_node_id_is_refused_by_the_key_it_is_missing() {
        let bare = TaskItem {
            key: None,
            title: "Build it".into(),
            project: None,
            content: None,
            metadata: Map::new(),
            repositories: Vec::new(),
            delivers: Vec::new(),
        };
        let message = node_id(&bare).unwrap_err();
        assert!(message.contains(ID_KEY), "{message}");
        assert!(message.starts_with("carries no"), "{message}");

        let mut typed = bare;
        typed.metadata.insert(ID_KEY.to_owned(), Value::from(7));
        let message = node_id(&typed).unwrap_err();
        assert!(message.contains("a node id is a string"), "{message}");
    }
}
