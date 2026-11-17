//! `project graph`: one project's tasks and the dependency edges between them, laid out
//! once and handed back in the two forms the verb prints.
//!
//! The layout is the contract, and it is stated whole in the verb's own `--help`, which the
//! README carries verbatim: the order the nodes come in, how they are numbered, the order of
//! the edges, which way `auto` lays the graph out, how a group is read off a task's metadata
//! and how a label is escaped. Everything below is a function of what the project's source
//! reports and of the request, so two calls over an unchanged project agree byte for byte.
//!
//! Nothing is written down: the tasks are read through the source's own project listing and
//! each task's forward edges through its own dependency read and what a task of a source
//! declared private records of it, exactly as `task list --project` and `task deps` read
//! them, and the whole graph lives for one call.

use std::collections::{BTreeSet, HashMap, HashSet};

use onetaskgraph_plugin_api::{ItemKind, Task};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::copy::{Level, forward_edges};
use super::dependents::Completeness;
use super::{Engine, EngineError, Qualified, qualify_endpoint};
use crate::GlobalId;
use crate::plan::Unanswered;

/// The version of the document [`ProjectGraph`] serialises to, which its consumers read
/// before anything else in it.
pub const PROJECT_GRAPH_SCHEMA_VERSION: u32 = 1;

/// What a walk of a project's members says it was for, when a source refuses it.
const FOR_A_GRAPH: &str = "for a graph";

/// Which way a graph is laid out: top to bottom, or left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum GraphDirection {
    /// Top to bottom: Mermaid's `flowchart TD`.
    Td,
    /// Left to right: Mermaid's `flowchart LR`.
    Lr,
}

impl GraphDirection {
    /// The word Mermaid's `flowchart` header takes for this direction.
    #[must_use]
    pub const fn mermaid(self) -> &'static str {
        match self {
            Self::Td => "TD",
            Self::Lr => "LR",
        }
    }

    /// The direction `auto` resolves to for a graph of `ranks` ranks whose widest holds
    /// `widest` tasks: left to right exactly when it is wider than it is deep.
    ///
    /// A graph much wider than it is deep is unreadable once a renderer fits it to a page
    /// top-down, and readable laid out the other way: on an 880-pixel page the smallest label
    /// of a 48-task plan over ranks 1/28/15/4 measured 1.6 pixels top-down and 11.5 left to
    /// right, while a 10-task plan over ranks 1/4/2/1/2 measured 11.5 top-down and 9.1 left to
    /// right.
    #[must_use]
    pub const fn auto(ranks: usize, widest: usize) -> Self {
        if widest > ranks { Self::Lr } else { Self::Td }
    }
}

/// The task metadata key `project graph --group-by` groups by: any key a record may hold,
/// and never the empty string, which names none.
///
/// Not a [`MetadataKey`](onetaskgraph_plugin_api::MetadataKey): that is what a *write* may
/// name, which keeps out this product's own namespace and a key with no dot. A read groups
/// by whatever a task holds, so a folder of Markdown's `unit:` is as good a key as
/// `orchestrator.unit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct GroupKey(#[schemars(length(min = 1))] String);

impl GroupKey {
    /// `key`, refused when it is empty.
    ///
    /// # Errors
    ///
    /// The refusal, worded for the command line, when `key` is empty.
    pub fn new(key: impl Into<String>) -> Result<Self, String> {
        let key = key.into();
        if key.is_empty() {
            return Err(
                "a metadata key is not empty; name the key whose value groups a task, \
                        for example `orchestrator.unit`"
                    .to_owned(),
            );
        }
        Ok(Self(key))
    }

    /// The key as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for GroupKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<GroupKey> for String {
    fn from(key: GroupKey) -> Self {
        key.0
    }
}

/// What `project graph` is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectGraphRequest {
    /// The project whose tasks are drawn.
    pub project: GlobalId,
    /// The direction to lay it out in, or `None` for `auto`.
    pub direction: Option<GraphDirection>,
    /// The key whose non-empty string value groups the project's tasks, or `None` for none.
    pub group_by: Option<GroupKey>,
    /// What the graph owes when a source declared private could not be asked what it records
    /// of the project's tasks: every other edge, with that source named in
    /// [`ProjectGraph::unanswered`], or a refusal.
    pub completeness: Completeness,
}

/// One project's dependency graph, laid out: the document `project graph --format json`
/// prints, and everything its Mermaid form is drawn from.
///
/// Built by [`Engine::project_graph`] alone, so every edge names a node it holds, an `n`
/// node is never external and an `x` node never has a group: nothing outside this module can
/// put one together any other way.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ProjectGraph {
    /// The version of this document's shape; `1`.
    schema_version: u32,
    /// The project whose tasks these are.
    project: GlobalId,
    /// The direction the graph is laid out in — `auto` already resolved.
    direction: GraphDirection,
    /// The `--group-by` key, or `null` when none was given.
    group_by: Option<GroupKey>,
    /// Every task of the project in topological order, then every task outside it that one
    /// of them depends on, ordered by title and then qualified id.
    nodes: Vec<GraphNode>,
    /// Every dependency, ordered by its prerequisite's position and then its dependent's.
    edges: Vec<GraphEdge>,
    /// Each source declared private that could not be asked what it records of the project's
    /// tasks, and why — so `edges` lacks whatever it records. Absent when every source
    /// answered, which is the one case the graph is complete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(!skip_serializing_if)]
    unanswered: Vec<Unanswered>,
    /// Each edge as the positions in `nodes` of its prerequisite and its dependent, in the
    /// order of `edges` — what the Mermaid form names each end by.
    #[serde(skip)]
    #[schemars(skip)]
    links: Vec<(usize, usize)>,
}

/// One node of a [`ProjectGraph`]: one task, of the project or outside it.
///
/// Its JSON is [`GraphNodeWire`]'s — `key`, `id`, `title`, `external`, `group` — derived from
/// where it sits, so a node outside the project can have no group and every key is a number
/// of its kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphNode {
    /// The task's qualified id.
    id: GlobalId,
    /// The task's title, exactly as its source reports it.
    title: String,
    /// Where the task sits, and what that gives it.
    place: Place,
}

/// Where a [`GraphNode`]'s task sits.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Place {
    /// A task of the project: the `n<number>` node, with its group, if any.
    Own {
        /// Its position among the project's tasks, from 1.
        number: usize,
        /// Its group under `group_by`.
        group: Option<String>,
    },
    /// A task outside the project: the `x<number>` node, which never has a group.
    External {
        /// Its position among the tasks outside the project, from 1.
        number: usize,
    },
}

impl GraphNode {
    /// The Mermaid form's node id.
    fn key(&self) -> String {
        match &self.place {
            Place::Own { number, .. } => format!("n{number}"),
            Place::External { number } => format!("x{number}"),
        }
    }

    const fn external(&self) -> bool {
        matches!(self.place, Place::External { .. })
    }

    /// The task's group, which only a task of the project can have.
    fn group(&self) -> Option<&str> {
        match &self.place {
            Place::Own { group, .. } => group.as_deref(),
            Place::External { .. } => None,
        }
    }
}

impl Serialize for GraphNode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        GraphNodeWire {
            key: self.key(),
            id: self.id.clone(),
            title: self.title.clone(),
            external: self.external(),
            group: self.group().map(str::to_owned),
        }
        .serialize(serializer)
    }
}

impl JsonSchema for GraphNode {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "GraphNode".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        GraphNodeWire::json_schema(generator)
    }
}

/// One node of a [`ProjectGraph`]: one task.
#[derive(Serialize, JsonSchema)]
struct GraphNodeWire {
    /// The Mermaid form's node id: `n<k>` for the project's own tasks and `x<k>` for a task
    /// outside it, each counted from 1.
    key: String,
    /// The task's qualified id.
    id: GlobalId,
    /// The task's title, exactly as its source reports it — unescaped and unaltered.
    title: String,
    /// Whether the task is outside the project: `true` exactly for the `x<k>` nodes.
    external: bool,
    /// The task's group under `group_by`, or `null` — for a task with none, for every
    /// external task, and for every task when no key was given.
    group: Option<String>,
}

/// One edge of a [`ProjectGraph`], pointing from the task depended on to the task that
/// depends on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct GraphEdge {
    /// The prerequisite: the task that has to finish first.
    from: GlobalId,
    /// The dependent: the project task that waits on it.
    to: GlobalId,
}

impl ProjectGraph {
    /// Each source declared private that could not be asked what it records of the project's
    /// tasks, and why; empty exactly when the graph is complete.
    #[must_use]
    pub fn unanswered(&self) -> &[Unanswered] {
        &self.unanswered
    }

    /// Whether every source that may hold an edge of the project's tasks answered.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.unanswered.is_empty()
    }

    /// The graph as Mermaid flowchart text, every line ending in `\n`.
    ///
    /// Without a `group_by` key: the header, the project's node lines, the external node
    /// lines, the edge lines, and the `classDef` line when an external node was printed.
    /// With one, each group's members and the edges between two of them sit inside that
    /// group's `subgraph` block, ahead of everything else — mermaid 11's layout crashed on a
    /// 100-task plan grouped into 57 subgraphs whose edges were all declared after the
    /// blocks, and rendered the same graph with each block's own edges declared inside it.
    #[must_use]
    pub fn mermaid(&self) -> String {
        let node_line = |node: &GraphNode, indent: &str| {
            if node.external() {
                format!(
                    "{indent}{}[\"{} ({})\"]:::external\n",
                    node.key(),
                    label(&node.title),
                    // A native id is whatever its source issued, so it is escaped as a
                    // title is: a quote or a line break in it would otherwise end the label.
                    label(&node.id.to_string())
                )
            } else {
                format!("{indent}{}[\"{}\"]\n", node.key(), label(&node.title))
            }
        };
        let edge_line = |&(from, to): &(usize, usize), indent: &str| {
            format!(
                "{indent}{} --> {}\n",
                self.nodes[from].key(),
                self.nodes[to].key()
            )
        };
        // The group an edge sits inside: its two ends' group, when they share one.
        let inside = |&(from, to): &(usize, usize)| {
            self.nodes[from]
                .group()
                .filter(|group| self.nodes[to].group() == Some(*group))
        };
        let mut out = format!("flowchart {}\n", self.direction.mermaid());
        // Groups in the order their first member is drawn in, which is the order `nodes`
        // holds the project's tasks in.
        let mut groups: Vec<&str> = Vec::new();
        for group in self.nodes.iter().filter_map(GraphNode::group) {
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
        for (number, group) in groups.iter().enumerate() {
            out.push_str(&format!(
                "  subgraph g{}[\"{}\"]\n",
                number + 1,
                label(group)
            ));
            for node in self.nodes.iter().filter(|node| node.group() == Some(group)) {
                out.push_str(&node_line(node, "    "));
            }
            for link in self.links.iter().filter(|link| inside(link) == Some(group)) {
                out.push_str(&edge_line(link, "    "));
            }
            out.push_str("  end\n");
        }
        for node in self.nodes.iter().filter(|node| node.group().is_none()) {
            out.push_str(&node_line(node, "  "));
        }
        for link in self.links.iter().filter(|link| inside(link).is_none()) {
            out.push_str(&edge_line(link, "  "));
        }
        if self.nodes.iter().any(GraphNode::external) {
            out.push_str("  classDef external stroke-dasharray: 5 5\n");
        }
        out
    }
}

/// A title or a group value as a Mermaid label: each line break (`\r\n`, `\n` or `\r`) one
/// space, then `#`, `"`, `<` and `>` as the entity codes Mermaid reads, in that order — `#`
/// first, so the `#` each later code opens with is not escaped again. Nothing else changes.
#[must_use]
pub fn label(text: &str) -> String {
    text.replace("\r\n", " ")
        .replace(['\n', '\r'], " ")
        .replace('#', "#35;")
        .replace('"', "#quot;")
        .replace('<', "#lt;")
        .replace('>', "#gt;")
}

/// A task's group under `key`: its metadata value there when that is a non-empty string,
/// none when it is absent, `null` or `""`, and a refusal naming the task and the key for any
/// other JSON value.
fn group_of(task: &Qualified<Task>, key: &GroupKey) -> Result<Option<String>, EngineError> {
    match task.item.metadata.get(key.as_str()) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.is_empty() => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(other) => Err(EngineError::GraphGroupNotText {
            task: task.id.clone(),
            key: key.clone(),
            value: Box::new(other.clone()),
        }),
    }
}

/// The order a title tie is broken in: the title, then the qualified id, each by its bytes.
fn tie(title: &str, id: &GlobalId) -> (String, String) {
    (title.to_owned(), id.to_string())
}

/// A source's refusal as the failure of a read, which is what a graph's reads are: the
/// shared walks name it as a copy's, whose next action is to copy again.
fn failed_read(error: EngineError) -> EngineError {
    match error {
        EngineError::SourceRefused { name, error } => EngineError::SourceFailed { name, error },
        other => other,
    }
}

impl Engine {
    /// One project's tasks and their dependency edges, laid out as `project graph` prints
    /// them.
    ///
    /// Every page of the project's tasks is read, then each task's forward edges, then the
    /// title of each task outside the project one of them depends on. Any read failing fails
    /// the whole graph rather than answering part of it — but for a source declared private
    /// that could not be asked what it records of a task, which the graph names in
    /// [`ProjectGraph::unanswered`] and draws every other edge without, unless the request
    /// asks for [`Completeness::Required`].
    ///
    /// # Errors
    ///
    /// [`EngineError::NoSuchProject`] for an id naming no project;
    /// [`EngineError::NoSuchTask`] for a dependency naming a task its source does not hold;
    /// [`EngineError::GraphGroupNotText`] for a group value that is not a string;
    /// [`EngineError::DependencyCycle`] for tasks of the project that depend on each other
    /// in a cycle, which no order can put each after what it depends on; and the engine's
    /// own errors for a source that is not configured, could not be built or failed a read.
    pub async fn project_graph(
        &self,
        request: &ProjectGraphRequest,
    ) -> Result<ProjectGraph, EngineError> {
        let source = self.built(&request.project.source)?;
        let name = source.name().clone();
        let held = source
            .source()
            .get_project(&request.project.native)
            .await
            .map_err(|error| EngineError::SourceFailed {
                name: name.to_string(),
                error,
            })?;
        if held.is_none() {
            return Err(EngineError::NoSuchProject {
                id: request.project.to_string(),
            });
        }
        let tasks = self
            .project_member_tasks(&request.project, FOR_A_GRAPH)
            .await
            .map_err(failed_read)?;
        let groups = match &request.group_by {
            Some(key) => tasks
                .iter()
                .map(|task| group_of(task, key))
                .collect::<Result<Vec<_>, _>>()?,
            None => vec![None; tasks.len()],
        };

        // Every dependency, as (prerequisite, dependent). A set, because a source reporting
        // one relationship twice — as two kinds of edge between the same two tasks, say —
        // still has one edge to draw.
        let mut edges: HashSet<(GlobalId, GlobalId)> = HashSet::new();
        let mut unanswered = Vec::new();
        let mut resolving = super::designators::Resolving::default();
        for task in &tasks {
            let read = forward_edges(source, &task.id.native, Level::Task)
                .await
                .map_err(failed_read)?;
            for edge in read {
                // An opaque far end is drawn as the source its designator is, or named.
                let Some(far) = self.resolve_endpoint(edge.to, &mut resolving).await else {
                    continue;
                };
                let to = qualify_endpoint(&name, far);
                // A task that depends on a whole project is not drawn: the graph is of tasks.
                if to.kind == ItemKind::Task {
                    edges.insert((to.id, task.id.clone()));
                }
            }
            // And what a task of a source declared private records of this one, which this
            // task's own source cannot hold without naming it — asked only of a task a copy
            // may have left such an edge of.
            if !self.reads_recorded_for(&name, &task.item) {
                continue;
            }
            for prerequisite in self
                .recorded_prerequisites_answered(&task.id, request.completeness, &mut unanswered)
                .await?
            {
                edges.insert((prerequisite, task.id.clone()));
            }
        }
        // A far end that could not be confirmed fails the graph as any failed read does, and so,
        // when every edge is asked for, does one that could not be asked.
        if request.completeness == Completeness::Required {
            let mut failed = Vec::new();
            resolving.required(&request.project.to_string(), &mut failed)?;
            resolving = super::designators::Resolving::default();
            if let Some(failure) = failed.into_iter().next() {
                return Err(EngineError::SourceFailed {
                    name: failure.source.to_string(),
                    error: failure.error,
                });
            }
        }
        if let Some(failure) = std::mem::take(&mut resolving.failed).into_iter().next() {
            return Err(EngineError::SourceFailed {
                name: failure.source.to_string(),
                error: failure.error,
            });
        }
        let unanswered: Vec<Unanswered> = unanswered
            .into_iter()
            .map(Unanswered::Source)
            .chain(resolving.unanswered)
            .collect();

        let own: HashMap<&GlobalId, usize> = tasks
            .iter()
            .enumerate()
            .map(|(index, task)| (&task.id, index))
            .collect();
        let order = topological(&tasks, &edges, &own)?;

        // The rank of each project task, in `order`: one more than the highest rank among
        // its in-project prerequisites, and 0 when it has none.
        let mut rank = vec![0_usize; tasks.len()];
        for &index in &order {
            let id = &tasks[index].id;
            rank[index] = edges
                .iter()
                .filter(|(_, dependent)| dependent == id)
                .filter_map(|(prerequisite, _)| own.get(prerequisite))
                .map(|&before| rank[before] + 1)
                .max()
                .unwrap_or(0);
        }
        let ranks = rank.iter().max().map_or(0, |highest| highest + 1);
        let mut widths = vec![0_usize; ranks];
        for &at in &rank {
            widths[at] += 1;
        }
        let widest = widths.iter().copied().max().unwrap_or(0);
        let direction = request
            .direction
            .unwrap_or_else(|| GraphDirection::auto(ranks, widest));

        let mut externals: Vec<(String, GlobalId)> = Vec::new();
        let outside: HashSet<&GlobalId> = edges
            .iter()
            .map(|(prerequisite, _)| prerequisite)
            .filter(|prerequisite| !own.contains_key(prerequisite))
            .collect();
        // A far end whose source could not be asked is already named in `unanswered`: it is
        // drawn under its qualified id rather than asked again, so the graph answers as a
        // dependency read does.
        let unasked: HashSet<&str> = unanswered
            .iter()
            .filter_map(|entry| match entry {
                Unanswered::Source(failure) => Some(failure.source.as_str()),
                Unanswered::Designator(_) => None,
            })
            .collect();
        for id in outside {
            let title = if unasked.contains(id.source.as_str()) {
                id.to_string()
            } else {
                self.graph_title(id).await?
            };
            externals.push((title, id.clone()));
        }
        externals.sort_by_key(|(title, id)| tie(title, id));

        let mut nodes: Vec<GraphNode> = order
            .iter()
            .enumerate()
            .map(|(position, &index)| GraphNode {
                id: tasks[index].id.clone(),
                title: tasks[index].item.title.clone(),
                place: Place::Own {
                    number: position + 1,
                    group: groups[index].clone(),
                },
            })
            .collect();
        nodes.extend(
            externals
                .into_iter()
                .enumerate()
                .map(|(position, (title, id))| GraphNode {
                    id,
                    title,
                    place: Place::External {
                        number: position + 1,
                    },
                }),
        );
        let position: HashMap<&GlobalId, usize> = nodes
            .iter()
            .enumerate()
            .map(|(at, node)| (&node.id, at))
            .collect();
        // Every end of every edge is a node: a project task, or an external one read above.
        let mut links: Vec<(usize, usize)> = edges
            .iter()
            .filter_map(|(prerequisite, dependent)| {
                Some((*position.get(prerequisite)?, *position.get(dependent)?))
            })
            .collect();
        links.sort_unstable();
        let edges = links
            .iter()
            .map(|&(from, to)| GraphEdge {
                from: nodes[from].id.clone(),
                to: nodes[to].id.clone(),
            })
            .collect();

        Ok(ProjectGraph {
            schema_version: PROJECT_GRAPH_SCHEMA_VERSION,
            project: request.project.clone(),
            direction,
            group_by: request.group_by.clone(),
            nodes,
            edges,
            unanswered,
            links,
        })
    }

    /// The title of one task outside the project, read from its own source.
    async fn graph_title(&self, id: &GlobalId) -> Result<String, EngineError> {
        let response = self.task(id).await?;
        if let Some(failure) = response.errors.into_iter().next() {
            return Err(EngineError::SourceFailed {
                name: failure.source.to_string(),
                error: failure.error,
            });
        }
        response
            .items
            .into_iter()
            .next()
            .map(|task| task.item.title)
            .ok_or_else(|| EngineError::NoSuchTask { id: id.to_string() })
    }
}

/// The project's tasks, as indices into `tasks`, in topological order: each after every task
/// of the project it depends on, ties broken by title and then qualified id.
fn topological(
    tasks: &[Qualified<Task>],
    edges: &HashSet<(GlobalId, GlobalId)>,
    own: &HashMap<&GlobalId, usize>,
) -> Result<Vec<usize>, EngineError> {
    let inside: Vec<(usize, usize)> = edges
        .iter()
        .filter_map(|(prerequisite, dependent)| {
            Some((*own.get(prerequisite)?, *own.get(dependent)?))
        })
        .collect();
    let mut waiting = vec![0_usize; tasks.len()];
    for &(_, dependent) in &inside {
        waiting[dependent] += 1;
    }
    let key = |index: usize| (tie(&tasks[index].item.title, &tasks[index].id), index);
    let mut ready: BTreeSet<((String, String), usize)> = (0..tasks.len())
        .filter(|&index| waiting[index] == 0)
        .map(key)
        .collect();
    let mut order = Vec::with_capacity(tasks.len());
    while let Some(next) = ready.pop_first() {
        let index = next.1;
        order.push(index);
        for &(_, dependent) in inside.iter().filter(|(before, _)| *before == index) {
            waiting[dependent] -= 1;
            if waiting[dependent] == 0 {
                ready.insert(key(dependent));
            }
        }
    }
    if order.len() < tasks.len() {
        let mut stuck: Vec<GlobalId> = (0..tasks.len())
            .filter(|index| !order.contains(index))
            .map(|index| tasks[index].id.clone())
            .collect();
        stuck.sort_by_key(ToString::to_string);
        return Err(EngineError::DependencyCycle { tasks: stuck });
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::{GroupKey, label};

    /// Every line break is one space, whichever of the three spellings it is, and the four
    /// escaped characters are escaped once each — `#` first, so no entity is escaped twice.
    #[test]
    fn a_label_writes_each_line_break_as_one_space_and_escapes_four_characters_once() {
        assert_eq!(label("a\r\nb\nc\rd"), "a b c d");
        assert_eq!(label("\r\n\r\n"), "  ");
        assert_eq!(
            label("#\"<>&;"),
            "#35;#quot;#lt;#gt;&;",
            "nothing but the four is altered"
        );
        assert_eq!(label("plain ünïcode"), "plain ünïcode");
    }

    /// A group key is any key but the empty one.
    #[test]
    fn a_group_key_is_refused_only_when_it_is_empty() {
        assert!(GroupKey::new("").is_err());
        for key in ["unit", "orchestrator.unit", "onetaskgraph.origin"] {
            assert_eq!(
                GroupKey::new(key).map(|key| key.as_str().to_owned()),
                Ok(key.to_owned())
            );
        }
    }
}
