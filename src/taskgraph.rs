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
//! absent binary, an unusable one, or one below [`MINIMUM_VERSION`] refuses the
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

/// The oldest `onetaskgraph` this build can read a project out of.
///
/// The reserved metadata map every field of this mapping rides on is that
/// product's own published surface, so the floor is a *requirement* rather than
/// a preference: below it a project carries no metadata at all and every plan
/// would read as a graph of untyped, unidentified nodes.
pub const MINIMUM_VERSION: &str = "0.1.0";

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

/// The dependency kind a plan edge is.
const BLOCKS: &str = "blocks";

/// The endpoint kind a plan node is.
const TASK_KIND: &str = "task";

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
                     needs {DEFAULT_BINARY} {MINIMUM_VERSION} or newer — {INSTALL}",
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
        let version = Version::parse(&printed).ok_or_else(|| {
            named(format!(
                "reported no version this build can read: {:?}",
                printed.trim()
            ))
        })?;
        let minimum = Version::parse(MINIMUM_VERSION).expect("the declared minimum is a version");
        if version < minimum {
            return Err(named(format!("is version {version}, below the minimum")));
        }
        Ok(Self { binary })
    }

    /// Read one qualified project id as the plan it holds.
    ///
    /// The project is external input, so every refusal it earns is made here,
    /// before a run is minted: a reserved key of the wrong JSON type, a key no
    /// plan field answers to, a task carrying no node id, and a dependency edge
    /// whose far end this plan cannot name.
    pub fn plan(&self, project: &str) -> Result<Plan> {
        let source = source_of(project)?;
        let held = self.project(project)?;
        let tasks = self.tasks(project)?;

        let mut document = Map::new();
        for (key, value) in &held.item.metadata {
            let Some(field) = key.strip_prefix(RESERVED) else {
                continue;
            };
            if field == "tasks" {
                return Err(refused(project, TASKS_ARE_THE_PROJECTS.to_owned()));
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
            let id = node_id(&task.item).map_err(|why| refused(&task.id, format!("it {why}")))?;
            if let Some(first) = claimed.insert(id.clone(), task.id.clone()) {
                return Err(refused(
                    &task.id,
                    format!("`{ID_KEY}` '{id}' is already the id of another task, '{first}'"),
                ));
            }
            ids.insert(native_of(&task.id).to_owned(), id);
        }

        let mut nodes = Vec::with_capacity(tasks.len());
        for task in &tasks {
            nodes.push(self.node(&source, task, &ids)?);
        }
        document.insert("tasks".to_owned(), Value::Array(nodes));

        let document = Value::Object(document);
        if let Some(retired) = crate::plan::retired_field_refusal(&document) {
            return Err(refused(project, retired));
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
                    &task.id,
                    format!(
                        "{error} — a node's fields are the reserved `{RESERVED}<field>` \
                         metadata keys on its task"
                    ),
                )
            })?;
        }
        read::<Plan>(document).map_err(|error| {
            refused(
                project,
                format!(
                    "{error} — a plan's settings are the reserved `{RESERVED}<field>` \
                     metadata keys on its project"
                ),
            )
        })
    }

    /// One node, assembled out of its task.
    fn node(&self, source: &str, task: &Qualified<TaskItem>, ids: &Ids) -> Result<Value> {
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
                    &task.id,
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
                    &task.id,
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

        let mut deps = self.deps(source, task, ids)?;
        // A cross-DAG reference names another run's DAG, which is an item of no
        // source at all, so it is the one dependency that cannot be an edge.
        if let Some(carried) = node.remove("deps") {
            let listed: Vec<String> = serde_json::from_value(carried)
                .map_err(|error| refused(&task.id, format!("`{RESERVED}deps` {error}")))?;
            for reference in listed {
                // Anything spelled as a cross-DAG reference is carried, well
                // formed or not: a malformed one is answered by the graph
                // module, which says what the shape has to be. What is refused
                // here is a dependency that never meant to leave this run.
                if !reference.starts_with(crate::crossdag::PREFIX) {
                    return Err(refused(
                        &task.id,
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
    fn deps(&self, source: &str, task: &Qualified<TaskItem>, ids: &Ids) -> Result<Vec<String>> {
        let mut deps = Vec::new();
        for edge in self.edges(&task.id)? {
            if edge.kind != BLOCKS {
                continue;
            }
            let far = &edge.to;
            let both = format!("'{}' depends on '{}'", task.id, far.id);
            if far.kind != TASK_KIND {
                return Err(refused(
                    &task.id,
                    format!("{both}, which is a {} and not a node of a plan", far.kind),
                ));
            }
            let here = far
                .id
                .split_once(':')
                .is_none_or(|(named, _)| named == source);
            let resolved = match ids.get(native_of(&far.id)).filter(|_| here) {
                Some(id) => id.clone(),
                // A far end outside this project is still resolved through its
                // own `onepipeline.id`, because that is what a node id is: the
                // one name that survives a copy between two sources. What it
                // resolves to is then this plan's business, and a node id no
                // node of this plan carries is a dangling dependency.
                None => {
                    let far_task = self.show(&far.id).map_err(|error| {
                        refused(
                            &task.id,
                            format!("{both}, which could not be read: {error}"),
                        )
                    })?;
                    node_id(&far_task.item).map_err(|why| {
                        refused(&task.id, format!("{both}, and the far task {why}"))
                    })?
                }
            };
            deps.push(resolved);
        }
        Ok(deps)
    }

    fn project(&self, project: &str) -> Result<Qualified<ProjectItem>> {
        one(
            project,
            self.read::<Qualified<ProjectItem>>(&["project", "show", project])?,
        )
    }

    fn show(&self, task: &str) -> Result<Qualified<TaskItem>> {
        one(
            task,
            self.read::<Qualified<TaskItem>>(&["task", "show", task])?,
        )
    }

    fn tasks(&self, project: &str) -> Result<Vec<Qualified<TaskItem>>> {
        self.paged(&["task", "list", "--project", project])
    }

    fn edges(&self, task: &str) -> Result<Vec<Edge>> {
        self.paged(&["task", "deps", task, "--direction", "depends-on"])
    }

    /// Every page of one query, walked to its end.
    ///
    /// A store pages, and a plan is the whole graph or it is not a plan: a
    /// launch that read the first page alone would execute a prefix of the
    /// project and never say which nodes it left out.
    fn paged<T: serde::de::DeserializeOwned>(&self, args: &[&str]) -> Result<Vec<T>> {
        let mut all = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut args = args.to_vec();
            if let Some(token) = page.as_deref() {
                args.extend_from_slice(&["--page", token]);
            }
            let response = self.read::<T>(&args)?;
            all.extend(response.items);
            match response.next {
                Some(token) => page = Some(token),
                None => return Ok(all),
            }
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

/// The node ids of one project, by the native id of the task carrying each.
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

/// The refusal one item of the store earns, named by the id it was read under.
fn refused(id: &str, what: String) -> Error {
    Error::Invalid(format!("{id}: {what}"))
}

/// The source half of a qualified `<source>:<native>` id.
fn source_of(id: &str) -> Result<String> {
    id.split_once(':')
        .filter(|(source, native)| !source.is_empty() && !native.is_empty())
        .map(|(source, _)| source.to_owned())
        .ok_or_else(|| {
            Error::Invalid(format!(
                "'{id}' is not a qualified onetaskgraph id; write it as <source>:<native>, \
                 for example plan-store:ship-the-widget"
            ))
        })
}

/// The native half, or the whole id when it carries no source.
fn native_of(id: &str) -> &str {
    id.split_once(':').map_or(id, |(_, native)| native)
}

/// The one item a `show` answered with.
fn one<T>(id: &str, response: Response<T>) -> Result<T> {
    response
        .items
        .into_iter()
        .next()
        .ok_or_else(|| Error::Invalid(format!("'{id}' names nothing in the configured sources")))
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

/// The executable this process reads its plans through.
fn resolved_binary() -> PathBuf {
    match std::env::var(BINARY_ENV) {
        Ok(named) if !named.trim().is_empty() => PathBuf::from(named),
        _ => PathBuf::from(DEFAULT_BINARY),
    }
}

/// A released version, as far as an ordering needs it.
///
/// Three numbers and nothing else: a pre-release or build suffix is dropped,
/// because what this comparison decides is whether an installed binary carries
/// the surface this build reads, and a `-rc.1` of the release that carries it
/// does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u32, u32, u32);

impl Version {
    /// The version in a line like `onetaskgraph 0.1.0`, or `None`.
    fn parse(printed: &str) -> Option<Self> {
        let token = printed.split_whitespace().last()?;
        let token = token.trim_start_matches('v');
        let token = token.split(['-', '+']).next()?;
        let mut parts = token.split('.').map(str::parse::<u32>);
        let major = parts.next()?.ok()?;
        let minor = parts.next().unwrap_or(Ok(0)).ok()?;
        let patch = parts.next().unwrap_or(Ok(0)).ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self(major, minor, patch))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.0, self.1, self.2)
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
    id: String,
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
    kind: String,
}

#[derive(Debug, Deserialize)]
struct Endpoint {
    id: String,
    kind: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_read_off_the_line_the_binary_prints() {
        assert_eq!(
            Version::parse("onetaskgraph 0.1.0\n"),
            Some(Version(0, 1, 0))
        );
        assert_eq!(
            Version::parse("onetaskgraph v1.2.3"),
            Some(Version(1, 2, 3))
        );
        // A pre-release of the release that carries the surface carries it.
        assert_eq!(
            Version::parse("onetaskgraph 0.2.0-rc.1"),
            Some(Version(0, 2, 0))
        );
        assert_eq!(Version::parse("onetaskgraph 2"), Some(Version(2, 0, 0)));
        assert_eq!(Version::parse(""), None);
        assert_eq!(Version::parse("onetaskgraph what"), None);
        assert_eq!(Version::parse("onetaskgraph 1.2.3.4"), None);
    }

    #[test]
    fn versions_order_by_each_number_in_turn() {
        let minimum = Version::parse(MINIMUM_VERSION).expect("the declared minimum parses");
        assert!(Version::parse("0.0.9").expect("a version") < minimum);
        assert!(Version::parse("0.1.0").expect("a version") >= minimum);
        assert!(Version::parse("1.0.0").expect("a version") > minimum);
    }

    #[test]
    fn a_qualified_id_is_split_on_its_first_colon_and_a_bare_one_is_refused() {
        assert_eq!(
            source_of("plan-store:ship").as_deref().ok(),
            Some("plan-store")
        );
        // A native id may contain colons freely; the split is on the first.
        assert_eq!(native_of("plan-store:a:b"), "a:b");
        let message = source_of("ship").unwrap_err().to_string();
        assert!(message.contains("<source>:<native>"), "{message}");
        assert!(source_of(":ship").is_err());
        assert!(source_of("plan-store:").is_err());
    }

    #[test]
    fn the_binary_is_the_environments_when_it_names_one() {
        // The default is the executable name, looked up on the PATH.
        assert_eq!(resolved_binary(), PathBuf::from(DEFAULT_BINARY));
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
