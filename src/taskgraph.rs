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
//! What this module produces is a [`Plan`] value, which the graph module then
//! validates exactly as it validated one read out of a file: the shape rules,
//! the reference rules, acyclicity, the required title on a lifecycle node, and
//! the named refusal for each retired field all apply at the point a project is
//! read.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::plan::{Node, Plan};

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

/// The version floor a launch **checks**, which is not the whole requirement.
///
/// Named for what it does rather than for what would be useful: it is the oldest
/// version this build will accept a `--version` from, and no version can say
/// more than that here. What the mapping actually needs is the reserved metadata
/// map, which landed *within* this version — see [`FIRST_REVISION`], which is the
/// rest of the requirement and the half a number cannot express.
const CHECKED_MINIMUM: Version = Version {
    major: 0,
    minor: 1,
    patch: 0,
    release: Release::Released,
};

/// The `onetaskgraph` revision that first carried the surface this mapping
/// reads, which [`CHECKED_MINIMUM`] cannot express.
///
/// The reserved metadata map landed **after** that product's 0.1.0 release and
/// before its next one, so the released 0.1.0 and this revision report the same
/// number and answer differently. A version floor cannot separate them, so the
/// refusal names the revision instead of leaving a host with the released 0.1.0
/// to work out why every task of its project reads as unidentified.
///
/// **This is the only place the revision is written.** `justfile`'s
/// `_ensure-onetaskgraph` reads it out of this file rather than keeping a second
/// copy, and
/// [`the_revision_the_checks_install_is_read_out_of_this_file`](tests::the_revision_the_checks_install_is_read_out_of_this_file)
/// fails if a copy appears. `docs/contract-divergences.md` entry 42 is the
/// proposal to retire it for a version once one carries the surface.
pub const FIRST_REVISION: &str = "dc0180cf1f5754c23aae065aae6531f858ca4d1f";

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

/// A reserved key this mapping fills from the task itself, and where it comes
/// from — so a project stating it is told which end to edit rather than having
/// its value silently lose to the task's own.
const FILLED_FROM_THE_TASK: &[(&str, &str)] = &[
    ("title", "the task's own `title`"),
    ("task", "the task's own `content`"),
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
/// Holding the resolved path and the version it reported together is what makes
/// the version check a **launch-time** fact rather than a per-command one: the
/// value cannot be constructed without one, so nothing downstream can reach the
/// binary having skipped it.
#[derive(Debug, Clone)]
pub struct Store {
    binary: PathBuf,
}

impl Store {
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
                 needs {DEFAULT_BINARY} {CHECKED_MINIMUM} or newer — {INSTALL}. The reserved \
                 metadata this mapping reads landed at revision {FIRST_REVISION}, after \
                 that product's 0.1.0 release, so an install older than that revision \
                 reports a version this check accepts and then answers with tasks \
                 carrying no metadata at all",
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
        let printed = String::from_utf8_lossy(&reported.stdout);
        let token = version_token(&printed).unwrap_or_default();
        let version = Version::parse(token).ok_or_else(|| {
            named(format!(
                "reported no version this build can read: {:?}",
                printed.trim()
            ))
        })?;
        if version < CHECKED_MINIMUM {
            return Err(named(format!("is version {token}, below the minimum")));
        }
        Ok(Self { binary })
    }

    /// Read one qualified project id as the plan it holds.
    ///
    /// The project is external input, so every refusal it earns is made here,
    /// before a run is minted: a reserved key of the wrong JSON type, a key no
    /// plan field answers to, a task carrying no node id, and a dependency edge
    /// whose far end this plan cannot name.
    pub fn plan(&self, project: &QualifiedId) -> Result<Plan> {
        let held = self.project(project)?;
        let tasks = self.tasks(project)?;

        let mut document = Map::new();
        for (key, value) in &held.item.metadata {
            let Some(field) = key.strip_prefix(RESERVED) else {
                continue;
            };
            if field == "tasks" {
                return Err(refused(project.as_str(), TASKS_ARE_THE_PROJECTS.to_owned()));
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
                .map_err(|why| refused(task.id.as_str(), format!("it {why}")))?;
            if let Some(first) = claimed.insert(id.clone(), task.id.to_string()) {
                return Err(refused(
                    task.id.as_str(),
                    format!("`{ID_KEY}` '{id}' is already the id of another task, '{first}'"),
                ));
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
            return Err(refused(project.as_str(), retired));
        }
        // Each node on its own first, so a refusal names the task it is about;
        // the whole document afterwards, which is what refuses a plan-level key
        // no field answers to.
        for (task, node) in tasks.iter().zip(
            document["tasks"]
                .as_array()
                .expect("the nodes just written"),
        ) {
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
        read::<Plan>(document).map_err(|error| {
            refused(
                project.as_str(),
                format!(
                    "{error} — a plan's settings are the reserved `{RESERVED}<field>` \
                     metadata keys on its project"
                ),
            )
        })
    }

    /// One node, assembled out of its task.
    fn node(&self, task: &Qualified<TaskItem>, ids: &Ids) -> Result<Value> {
        let mut node = Map::new();
        for (key, value) in &task.item.metadata {
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
                ));
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
        match (task.item.repositories.first(), node.get("repo")) {
            (Some(_), Some(_)) => {
                return Err(refused(
                    task.id.as_str(),
                    format!(
                        "it names a repository in both `repositories` and `{REPO_KEY}`; a node \
                         lands in one repository, and `{REPO_KEY}` is only for an identity a \
                         normalized origin cannot hold"
                    ),
                ))
            }
            (Some(repository), None) => {
                node.insert("repo".to_owned(), Value::String(repository.clone()));
            }
            (None, _) => {}
        }

        let mut deps = self.deps(task, ids)?;
        // A cross-DAG reference names another run's DAG, which is an item of no
        // source at all, so it is the one dependency that cannot be an edge.
        if let Some(carried) = node.remove("deps") {
            let listed: Vec<String> = serde_json::from_value(carried)
                .map_err(|error| refused(task.id.as_str(), format!("`{RESERVED}deps` {error}")))?;
            for reference in listed {
                // Anything spelled as a cross-DAG reference is carried, well
                // formed or not: a malformed one is answered by the graph
                // module, which says what the shape has to be. What is refused
                // here is a dependency that never meant to leave this run.
                if !reference.starts_with(crate::crossdag::PREFIX) {
                    return Err(refused(
                        task.id.as_str(),
                        format!("`{RESERVED}deps` names '{reference}': {DEPS_ARE_EDGES}"),
                    ));
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
    fn deps(&self, task: &Qualified<TaskItem>, ids: &Ids) -> Result<Vec<String>> {
        let mut deps = Vec::new();
        for edge in self.edges(&task.id)? {
            if edge.kind != DependencyKind::Blocks {
                continue;
            }
            let far = &edge.to;
            let both = format!("'{}' depends on '{}'", task.id, far.id);
            if far.kind != ItemKind::Task {
                return Err(refused(
                    task.id.as_str(),
                    format!("{both}, which is a {} and not a node of a plan", far.kind),
                ));
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
                    })?;
                    node_id(&far_task.item).map_err(|why| {
                        refused(task.id.as_str(), format!("{both}, and the far task {why}"))
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

    /// Every task of one project, each one an item of that project's own source.
    ///
    /// The source check is not a restatement of the query: `--project` is a
    /// filter this build asked a third party to apply, and a plan assembled out
    /// of an item from somewhere else would carry a node whose id collides with
    /// this project's by coincidence. Refused here, where both the project asked
    /// for and the item answered with can still be named.
    fn tasks(&self, project: &QualifiedId) -> Result<Vec<Qualified<TaskItem>>> {
        let tasks: Vec<Qualified<TaskItem>> =
            self.paged(&["task", "list", "--project", project.as_str()])?;
        for task in &tasks {
            if task.id.source() != project.source() {
                return Err(Error::Sibling {
                    tool: DEFAULT_BINARY,
                    message: format!(
                        "asked for the tasks of '{project}' and answered with '{}', which is \
                         an item of another source",
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

fn refused(id: &str, what: String) -> Error {
    Error::Invalid(format!("{id}: {what}"))
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
    /// The source half.
    pub fn source(&self) -> &str {
        &self.whole[..self.colon]
    }

    /// The native half. It may contain colons freely: the split is on the first.
    pub fn native(&self) -> &str {
        &self.whole[self.colon + 1..]
    }

    /// The whole id, as it is written.
    pub fn as_str(&self) -> &str {
        &self.whole
    }
}

impl TryFrom<String> for QualifiedId {
    type Error = String;

    fn try_from(whole: String) -> std::result::Result<Self, String> {
        match whole.find(':') {
            Some(colon) if colon > 0 && colon + 1 < whole.len() => Ok(Self { whole, colon }),
            _ => Err(format!(
                "'{whole}' is not a qualified onetaskgraph id; write it as <source>:<native>, \
                 for example plan-store:ship-the-widget"
            )),
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
fn one<T>(id: &QualifiedId, response: Response<Qualified<T>>) -> Result<Qualified<T>> {
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
    match std::env::var(BINARY_ENV) {
        Ok(named) if !named.trim().is_empty() => PathBuf::from(named),
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
        let (numbers, prerelease) = match token.split_once('-') {
            Some((numbers, prerelease)) => (numbers, Some(prerelease)),
            None => (token, None),
        };
        // A separator with nothing after it names no pre-release and no build.
        let (numbers, build) = match numbers.split_once('+') {
            Some((numbers, build)) => (numbers, Some(build)),
            None => (numbers, None),
        };
        if prerelease.is_some_and(str::is_empty) || build.is_some_and(str::is_empty) {
            return None;
        }
        let mut parts = numbers.split('.').map(str::parse::<u32>);
        let major = parts.next()?.ok()?;
        let minor = parts.next().unwrap_or(Ok(0)).ok()?;
        let patch = parts.next().unwrap_or(Ok(0)).ok()?;
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
    title: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    metadata: Map<String, Value>,
    #[serde(default)]
    repositories: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Edge {
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
#[serde(from = "String")]
enum DependencyKind {
    /// The near item depends on the far one.
    Blocks,
    /// A link without an ordering.
    Related,
    /// A kind that product added after this build. Read leniently, as every
    /// sibling's own vocabulary is: it is not this build's set, and an edge is
    /// passed over rather than refused for being of a kind it does not know.
    Unknown(String),
}

impl From<String> for DependencyKind {
    fn from(wire: String) -> Self {
        match wire.as_str() {
            "blocks" => Self::Blocks,
            "related" => Self::Related,
            _ => Self::Unknown(wire),
        }
    }
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
    }

    /// A pre-release sorts below the release it precedes, which is the direction
    /// that cannot go wrong: a floor is a statement about what has shipped, and
    /// a release candidate is by construction a build of something that has not.
    #[test]
    fn versions_order_by_each_number_in_turn_and_a_prerelease_below_its_release() {
        let read = |token: &str| Version::parse(token).expect("a version");
        assert!(read("0.0.9") < CHECKED_MINIMUM);
        assert!(read("0.1.0") >= CHECKED_MINIMUM);
        assert!(read("1.0.0") > CHECKED_MINIMUM);
        assert!(read("0.1.0-rc.1") < CHECKED_MINIMUM);
        assert!(read("0.1.1-rc.1") > CHECKED_MINIMUM);
        // A build suffix is not a pre-release: it names the same release.
        assert_eq!(read("0.1.0+build.7"), CHECKED_MINIMUM);
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
        // And an id the *store* answers with crosses the same boundary: it is a
        // third party's output, read through the one type.
        assert!(serde_json::from_str::<QualifiedId>("\"bare\"").is_err());
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

    /// The revision the checks install is the one this file declares.
    ///
    /// A version floor cannot separate the released 0.1.0 from the revision that
    /// first carried the metadata surface — they report the same number — so the
    /// revision is written down, once, and everything that needs it derives it
    /// from here. A second copy in the justfile would be a pin that could go
    /// stale against the floor beside it without anything saying so.
    #[test]
    fn the_revision_the_checks_install_is_read_out_of_this_file() {
        let justfile = include_str!("../justfile");
        assert!(
            justfile.contains("FIRST_REVISION"),
            "the justfile no longer derives the revision it installs from this file"
        );
        assert!(
            !justfile.contains(FIRST_REVISION),
            "the justfile carries its own copy of the revision, which can go stale \
             against the floor declared beside it here"
        );
    }

    #[test]
    fn a_task_carrying_no_node_id_is_refused_by_the_key_it_is_missing() {
        let bare = TaskItem {
            title: "Build it".into(),
            content: None,
            metadata: Map::new(),
            repositories: Vec::new(),
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
