//! A loopback Linear: one GraphQL endpoint on `127.0.0.1`, served from this test process, that
//! answers the queries and mutations the linked Linear plugin sends to read a plan and to write a
//! settlement back — and logs every request it was sent, which is what a journey counts a
//! write-back's cost by.
//!
//! It holds one team (`TEAM`) with its workflow states, the workspace's project statuses, one
//! project and its issues, with a generic per-kind vocabulary: an issue's states and
//! a project's statuses are two different sets of names, which is what the per-kind
//! `status_mapping` exists for. What it answers is decided by the operation each request names —
//! the plugin's own documents, recognised by their root field — and every request it cannot
//! answer is refused as a GraphQL error naming it, so a journey that reaches one fails on what was
//! asked rather than on a guess.
//!
//! A journey can make the next request of one operation fail the way a hosted Linear fails: a
//! GraphQL error ([`Fault::Refuse`]), a connection closed with no answer ([`Fault::Drop`]), or an
//! answer held back until the journey lets it go ([`Fault::Hold`]).

// llmlint: ignore-file[e2e_not_mocked] this is the one thing the journeys beside it do not drive
// for real: Linear's hosted API, which an offline check may not reach. It is a stand-in for that
// remote service at the network boundary the linked plugin itself crosses — real HTTP on a real
// socket — and nothing inside this crate or the store it links is substituted.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};

/// The team key a source over this endpoint is configured with.
pub const TEAM: &str = "TEAM";

/// The one project this endpoint holds, by the id Linear would give it.
pub const PROJECT: &str = "project-1";

/// The operation one request named, by the root field of the document the plugin sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Operation {
    /// `teams{…states} projectStatuses`: everything a status write resolves, in one request.
    Resolution,
    /// `issue(id:)` beside `teams{…states} projectStatuses`: the issue an update naming a status
    /// reads, with everything that status write resolves riding on the same request — what the
    /// plugin sends while it holds no resolution yet.
    IssueResolved,
    /// `issue(id:)`: one issue, read whole — the description a settlement's metadata is merged
    /// into.
    Issue,
    /// `issue(id:){relations inverseRelations}`: one issue's dependency edges.
    IssueRelations,
    /// `originIssues: issues originProjects: projects organization` beside `teams{…states}
    /// projectStatuses`: every candidate counterpart a copy looks up by origin, with everything a
    /// status write resolves, in one request ahead of the copy's first write.
    CounterpartLookups,
    /// `issues(first:,filter:)`: a page of issues.
    Issues,
    /// `projects(first:,filter:)`: a page of projects — the one this endpoint holds, if it holds
    /// one, whatever the filter, which the plugin confirms each row against itself.
    Projects,
    /// `project(id:)`: one project.
    Project,
    /// `project(id:){relations inverseRelations}`: one project's dependency edges.
    ProjectRelations,
    /// `issueUpdate`: one issue written.
    IssueUpdate,
    /// `projectUpdate`: the project's content written — where Linear keeps a project's
    /// metadata, the driver claim among it — or its status alone.
    ProjectUpdate,
    /// `issueCreate`: one issue filed, as a copy files a task.
    IssueCreate,
    /// `projectCreate`: the project filed, as a copy files a plan.
    ProjectCreate,
    /// `document(id:)`: one document.
    Document,
    /// `documents(first:,filter:)`: a page of documents — every one this endpoint holds,
    /// whatever the filter, which the plugin confirms each row against itself.
    Documents,
    /// `documentCreate`: one document filed under the project.
    DocumentCreate,
    /// `documentUpdate`: one document's title or content written.
    DocumentUpdate,
    /// Anything else, which this endpoint refuses by name.
    Unknown,
}

impl Operation {
    fn of(query: &str) -> Self {
        let query = query.trim_start();
        if query.contains("originIssues:") {
            Self::CounterpartLookups
        } else if query.contains("issue(id:") && query.contains("teams(") {
            Self::IssueResolved
        } else if query.contains("teams(") && query.contains("projectStatuses(") {
            Self::Resolution
        } else if query.starts_with("mutation") && query.contains("issueUpdate(") {
            Self::IssueUpdate
        } else if query.starts_with("mutation") && query.contains("projectUpdate(") {
            Self::ProjectUpdate
        } else if query.starts_with("mutation") && query.contains("issueCreate(") {
            Self::IssueCreate
        } else if query.starts_with("mutation") && query.contains("projectCreate(") {
            Self::ProjectCreate
        } else if query.starts_with("mutation") && query.contains("documentCreate(") {
            Self::DocumentCreate
        } else if query.starts_with("mutation") && query.contains("documentUpdate(") {
            Self::DocumentUpdate
        } else if query.starts_with("mutation") {
            Self::Unknown
        } else if query.contains("projects(") {
            Self::Projects
        } else if query.contains("documents(") {
            Self::Documents
        } else if query.contains("document(id:") {
            Self::Document
        } else if query.contains("issues(") {
            Self::Issues
        } else if query.contains("issue(id:") && query.contains("relations(") {
            Self::IssueRelations
        } else if query.contains("issue(id:") {
            Self::Issue
        } else if query.contains("project(id:") && query.contains("relations(") {
            Self::ProjectRelations
        } else if query.contains("project(id:") {
            Self::Project
        } else {
            Self::Unknown
        }
    }

    /// Whether this is one of the reads a status write resolves its names through.
    pub fn resolves(self) -> bool {
        matches!(
            self,
            Self::Resolution | Self::IssueResolved | Self::CounterpartLookups
        )
    }
}

/// One request this endpoint was sent, as it arrived.
#[derive(Clone, Debug)]
pub struct Request {
    pub operation: Operation,
    pub variables: Value,
}

/// How the next request of one operation is made to fail.
#[derive(Clone, Copy, Debug)]
pub enum Fault {
    /// Answered with a GraphQL error, HTTP 200 — what Linear answers an input it will not take.
    Refuse,
    /// Read whole and then the connection closed with no answer, as a network failure mid-call.
    Drop,
    /// Not answered until [`Linear::release`], then answered as usual.
    Hold,
}

#[derive(Clone, Debug)]
struct Named {
    id: String,
    name: String,
    kind: String,
}

#[derive(Clone, Debug)]
struct Issue {
    id: String,
    identifier: String,
    title: String,
    description: String,
    state: String,
    priority: u64,
}

#[derive(Clone, Debug)]
struct Document {
    id: String,
    title: String,
    content: String,
}

#[derive(Default)]
struct World {
    states: Vec<Named>,
    statuses: Vec<Named>,
    project_name: String,
    project_content: String,
    project_status: String,
    /// Whether the project exists yet: [`Linear::serve_empty`] serves a workspace a copy files
    /// it into.
    project_held: bool,
    issues: Vec<Issue>,
    documents: Vec<Document>,
    log: Vec<Request>,
    faults: BTreeMap<Operation, VecDeque<Fault>>,
    held: bool,
}

/// The loopback endpoint. Its listener serves until the test process ends, so a driver a journey
/// left behind still finds it there.
#[derive(Clone)]
pub struct Linear {
    world: Arc<(Mutex<World>, Condvar)>,
    port: u16,
}

impl Linear {
    /// Serve a team holding `states` (`(name, type)`, Linear's `WorkflowState.type`) and a
    /// workspace holding `statuses` (`(name, type)`, its `ProjectStatus.type`), with one project,
    /// named `name`, at the project status `project_status`, whose description is `description`.
    pub fn serve(
        states: &[(&str, &str)],
        statuses: &[(&str, &str)],
        name: &str,
        description: &str,
        project_status: &str,
    ) -> Self {
        let named = |prefix: &str, all: &[(&str, &str)]| {
            all.iter()
                .enumerate()
                .map(|(at, (name, kind))| Named {
                    id: format!("{prefix}-{at}"),
                    name: (*name).to_owned(),
                    kind: (*kind).to_owned(),
                })
                .collect::<Vec<_>>()
        };
        let world = World {
            states: named("state", states),
            statuses: named("status", statuses),
            project_name: name.to_owned(),
            project_content: linear_stores(description),
            project_held: true,
            project_status: project_status.to_owned(),
            ..World::default()
        };
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("a bound address").port();
        let linear = Self {
            world: Arc::new((Mutex::new(world), Condvar::new())),
            port,
        };
        let serving = linear.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let serving = serving.clone();
                std::thread::spawn(move || serving.connection(stream));
            }
        });
        linear
    }

    /// Serve a team holding `states` and a workspace holding `statuses`, as [`Linear::serve`]
    /// does, with no project yet: the one a copy files is the one it holds from then on.
    pub fn serve_empty(states: &[(&str, &str)], statuses: &[(&str, &str)]) -> Self {
        let linear = Self::serve(states, statuses, "", "", "");
        linear.lock().project_held = false;
        linear
    }

    /// Edit one issue's stored description in place, as a person editing it in Linear does:
    /// `edit` is handed what Linear holds and answers what it holds after.
    pub fn edit_description(&self, issue: &str, edit: impl FnOnce(&str) -> String) {
        let mut world = self.lock();
        let held = world
            .issues
            .iter_mut()
            .find(|held| held.id == issue)
            .unwrap_or_else(|| panic!("no issue {issue}"));
        held.description = linear_stores(&edit(&held.description));
    }

    /// Every document this endpoint holds, by id, with its content as stored.
    pub fn documents(&self) -> Vec<(String, String)> {
        self.lock()
            .documents
            .iter()
            .map(|held| (held.id.clone(), held.content.clone()))
            .collect()
    }

    /// Every issue this endpoint holds, by id, with its title and its description as stored.
    pub fn issues(&self) -> Vec<(String, String, String)> {
        self.lock()
            .issues
            .iter()
            .map(|held| {
                (
                    held.id.clone(),
                    held.title.clone(),
                    held.description.clone(),
                )
            })
            .collect()
    }

    /// The URL a `linear` source's `endpoint` names to reach this endpoint.
    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/graphql", self.port)
    }

    /// File one issue of the project, at the workflow state named `state`, and answer its id.
    pub fn file(&self, title: &str, description: &str, state: &str) -> String {
        let mut world = self.lock();
        let n = world.issues.len() + 1;
        let state = world
            .states
            .iter()
            .find(|named| named.name == state)
            .unwrap_or_else(|| panic!("the team has no state {state:?}"))
            .id
            .clone();
        let id = format!("issue-{n}");
        world.issues.push(Issue {
            id: id.clone(),
            identifier: format!("{TEAM}-{n}"),
            title: title.to_owned(),
            description: linear_stores(description),
            state,
            priority: 0,
        });
        id
    }

    /// Add a workflow state to the team, as a person adding one in Linear's settings does.
    pub fn add_state(&self, name: &str, kind: &str) {
        let mut world = self.lock();
        let id = format!("state-{}", world.states.len());
        world.states.push(Named {
            id,
            name: name.to_owned(),
            kind: kind.to_owned(),
        });
    }

    /// Make the next request of `operation` fail as `fault` says.
    pub fn fail_next(&self, operation: Operation, fault: Fault) {
        self.lock()
            .faults
            .entry(operation)
            .or_default()
            .push_back(fault);
    }

    /// Let every held request go.
    pub fn release(&self) {
        let (lock, changed) = &*self.world;
        lock.lock().expect("the loopback's state").held = false;
        changed.notify_all();
    }

    /// Every request this endpoint has been sent, in the order it arrived.
    pub fn log(&self) -> Vec<Request> {
        self.lock().log.clone()
    }

    /// The name of the workflow state one issue is at.
    pub fn state_of(&self, issue: &str) -> String {
        let world = self.lock();
        let held = world
            .issues
            .iter()
            .find(|held| held.id == issue)
            .unwrap_or_else(|| panic!("no issue {issue}"));
        world
            .states
            .iter()
            .find(|named| named.id == held.state)
            .map(|named| named.name.clone())
            .unwrap_or_default()
    }

    /// The project content after Linear has stored every write.
    pub fn project_content(&self) -> String {
        self.lock().project_content.clone()
    }

    /// The name of the project status the project is at.
    pub fn project_status(&self) -> String {
        self.lock().project_status.clone()
    }

    /// The id of the project status named `name`.
    pub fn status_id(&self, name: &str) -> String {
        self.lock()
            .statuses
            .iter()
            .find(|named| named.name == name)
            .map(|named| named.id.clone())
            .unwrap_or_else(|| panic!("the workspace has no project status {name:?}"))
    }

    /// The name of the workflow state `id` names, where the team has one.
    pub fn state_named(&self, id: &str) -> Option<String> {
        self.lock()
            .states
            .iter()
            .find(|named| named.id == id)
            .map(|named| named.name.clone())
    }

    /// One issue's description, as it now stands.
    pub fn description_of(&self, issue: &str) -> String {
        self.lock()
            .issues
            .iter()
            .find(|held| held.id == issue)
            .map(|held| held.description.clone())
            .unwrap_or_else(|| panic!("no issue {issue}"))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.0.lock().expect("the loopback's state")
    }

    /// Serve one connection's requests, one after another, for as long as it stays open.
    fn connection(&self, stream: TcpStream) {
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(stream);
        loop {
            let Some(body) = read_request(&mut reader) else {
                return;
            };
            let Ok(request) = serde_json::from_slice::<Value>(&body) else {
                return;
            };
            // A GraphQL request is a document and an object of variables, and nothing else is
            // one this endpoint answers.
            let (Some(query), Some(variables)) = (
                request["query"].as_str(),
                request["variables"]
                    .as_object()
                    .map(|v| Value::Object(v.clone())),
            ) else {
                return;
            };
            let envelope = request
                .as_object()
                .expect("the named fields belong to an object");
            if envelope
                .keys()
                .any(|key| !matches!(key.as_str(), "query" | "variables" | "operationName"))
                || envelope
                    .get("operationName")
                    .is_some_and(|name| !name.is_null() && !name.is_string())
                || query.trim().is_empty()
            {
                return;
            }
            let operation = Operation::of(query);
            if matches!(
                operation,
                Operation::Issue
                    | Operation::IssueResolved
                    | Operation::IssueRelations
                    | Operation::Project
                    | Operation::ProjectRelations
                    | Operation::IssueUpdate
                    | Operation::ProjectUpdate
                    | Operation::Document
                    | Operation::DocumentUpdate
            ) && !variables["id"]
                .as_str()
                .is_some_and(|id| !id.trim().is_empty())
            {
                return;
            }
            let fault = {
                let mut world = self.lock();
                world.log.push(Request {
                    operation,
                    variables: variables.clone(),
                });
                let fault = world
                    .faults
                    .get_mut(&operation)
                    .and_then(VecDeque::pop_front);
                if matches!(fault, Some(Fault::Hold)) {
                    world.held = true;
                }
                fault
            };
            let answer = match fault {
                Some(Fault::Drop) => return,
                Some(Fault::Refuse) => json!({"errors": [{
                    "message": format!("the loopback refuses this {operation:?}"),
                    "extensions": {"code": "INVALID_INPUT"}
                }]}),
                Some(Fault::Hold) => {
                    let (lock, changed) = &*self.world;
                    let mut world = lock.lock().expect("the loopback's state");
                    while world.held {
                        world = changed.wait(world).expect("the loopback's state");
                    }
                    drop(world);
                    self.answer(operation, query, &variables)
                }
                None => self.answer(operation, query, &variables),
            };
            let body = answer.to_string();
            let written = write!(
                writer,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            if written.and_then(|()| writer.flush()).is_err() {
                return;
            }
        }
    }

    fn answer(&self, operation: Operation, query: &str, variables: &Value) -> Value {
        let mut world = self.lock();
        let id = match operation {
            Operation::Issue
            | Operation::IssueResolved
            | Operation::IssueRelations
            | Operation::Project
            | Operation::ProjectRelations
            | Operation::IssueUpdate
            | Operation::ProjectUpdate
            | Operation::Document
            | Operation::DocumentUpdate => variables["id"].as_str().expect("validated request ID"),
            _ => "",
        };
        let data = match operation {
            Operation::Resolution => world.resolution(),
            Operation::IssueResolved => {
                let mut data = world.resolution();
                data["issue"] = world.issue(id);
                data
            }
            Operation::CounterpartLookups => {
                // Every issue and the project, whatever the filters: the plugin confirms each
                // row against the origin it looks up, as it does a listing's.
                let mut data = world.resolution();
                data["originIssues"] = json!({
                    "nodes": world.issues.iter().map(|held| world.render(held)).collect::<Vec<_>>(),
                    "pageInfo": {"hasNextPage": false, "endCursor": null},
                });
                data["originProjects"] = json!({
                    "nodes": if world.project_held { vec![world.project()] } else { Vec::new() },
                    "pageInfo": {"hasNextPage": false, "endCursor": null},
                });
                data
            }
            Operation::Issue => json!({"issue": world.issue(id)}),
            Operation::IssueRelations => {
                let description = world
                    .issues
                    .iter()
                    .find(|held| held.id == id)
                    .map(|held| held.description.clone());
                json!({"issue": description.map(|description| json!({
                    "description": description,
                    "relations": empty_page(),
                    "inverseRelations": empty_page(),
                }))})
            }
            Operation::Issues => json!({"issues": {
                "nodes": world.issues.iter().map(|held| world.render(held)).collect::<Vec<_>>(),
                "pageInfo": {"hasNextPage": false, "endCursor": null},
            }}),
            Operation::Projects => json!({"projects": {
                "nodes": if world.project_held { vec![world.project()] } else { Vec::new() },
                "pageInfo": {"hasNextPage": false, "endCursor": null},
            }}),
            Operation::Project => json!({"project": world.holds(id).then(|| world.project())}),
            Operation::ProjectRelations => json!({"project": world.holds(id).then(|| json!({
                "content": world.project_content,
                "relations": empty_page(),
                "inverseRelations": empty_page(),
            }))}),
            Operation::IssueUpdate => {
                let Some(input) = variables["input"]
                    .as_object()
                    .filter(|input| !input.is_empty())
                else {
                    return refusal("an issueUpdate carries a non-empty `input` object");
                };
                match world.update(id, input) {
                    Ok(mut issue) => {
                        // A rewrite reads the issue's relations back in its own answer; this
                        // endpoint relates nothing.
                        if query.contains("relations(") {
                            issue["relations"] = empty_page();
                        }
                        json!({"issueUpdate": {"success": true, "issue": issue}})
                    }
                    Err(why) => return refusal(&why),
                }
            }
            Operation::ProjectUpdate
                if variables["input"]
                    .as_object()
                    .is_some_and(|input| input.len() == 1 && input.contains_key("statusId")) =>
            {
                // A status written alone, as `set_project_status` writes one: the input holds
                // nothing else, and names a status the workspace has.
                let status = variables["input"]
                    .as_object()
                    .filter(|input| input.len() == 1)
                    .and_then(|input| input["statusId"].as_str())
                    .and_then(|id| world.statuses.iter().find(|named| named.id == id))
                    .cloned();
                let Some(status) = status else {
                    return refusal(
                        "a status projectUpdate carries one `statusId` the workspace has",
                    );
                };
                if !world.holds(id) {
                    return refusal(&format!("no project {id}"));
                }
                world.project_status = status.name.clone();
                json!({"projectUpdate": {"success": true, "project": {
                    "id": PROJECT, "status": {"name": status.name, "type": status.kind},
                }}})
            }
            Operation::ProjectUpdate => {
                let Some(content) = variables["input"]["content"].as_str() else {
                    return refusal("a projectUpdate carries the project's `content`");
                };
                if !world.holds(id) {
                    return refusal(&format!("no project {id}"));
                }
                // A rewrite sends the project whole: its name, its status and its labels beside
                // its content, of which this endpoint holds none.
                let input = &variables["input"];
                if input
                    .get("labelIds")
                    .is_some_and(|labels| labels != &json!([]))
                {
                    return refusal("the loopback holds no labels");
                }
                let name = match input.get("name") {
                    None => None,
                    Some(Value::String(name)) => Some(name.clone()),
                    Some(name) => return refusal(&format!("a project's name is a string: {name}")),
                };
                if let Some(status) = input.get("statusId") {
                    let Some(status) = world
                        .statuses
                        .iter()
                        .find(|named| status.as_str() == Some(named.id.as_str()))
                        .cloned()
                    else {
                        return refusal(&format!("no project status {status}"));
                    };
                    world.project_status = status.name;
                }
                if let Some(name) = name {
                    world.project_name = name;
                }
                world.project_content = linear_stores(content);
                // The project as stored, which the plugin selects in its answer — the content
                // Linear stored among it — as Linear answers it.
                let mut project = world.project();
                if query.contains("relations(") {
                    project["relations"] = empty_page();
                }
                json!({"projectUpdate": {"success": true, "project": project}})
            }
            Operation::IssueCreate => {
                let Some(input) = variables["input"].as_object() else {
                    return refusal("an issueCreate carries an `input` object");
                };
                match world.create_issue(input) {
                    Ok(issue) => json!({"issueCreate": {"success": true, "issue": issue}}),
                    Err(why) => return refusal(&why),
                }
            }
            Operation::ProjectCreate => {
                let Some(input) = variables["input"].as_object() else {
                    return refusal("a projectCreate carries an `input` object");
                };
                match world.create_project(input) {
                    Ok(project) => json!({"projectCreate": {"success": true, "project": project}}),
                    Err(why) => return refusal(&why),
                }
            }
            Operation::Document => json!({"document": world
                .documents
                .iter()
                .find(|held| held.id == id)
                .map_or(Value::Null, |held| world.render_document(held))}),
            Operation::Documents => json!({"documents": {
                "nodes": world
                    .documents
                    .iter()
                    .map(|held| world.render_document(held))
                    .collect::<Vec<_>>(),
                "pageInfo": {"hasNextPage": false, "endCursor": null},
            }}),
            Operation::DocumentCreate | Operation::DocumentUpdate => {
                let Some(input) = variables["input"].as_object() else {
                    return refusal("a document write carries an `input` object");
                };
                let target = (operation == Operation::DocumentUpdate).then_some(id);
                match world.write_document(target, input) {
                    Ok(document) => {
                        let root = if target.is_some() {
                            "documentUpdate"
                        } else {
                            "documentCreate"
                        };
                        json!({root: {"success": true, "document": document}})
                    }
                    Err(why) => return refusal(&why),
                }
            }
            Operation::Unknown => return refusal(&format!("the loopback answers no {query}")),
        };
        json!({"data": data})
    }
}

impl World {
    /// Whether `id` names the project, once there is one.
    fn holds(&self, id: &str) -> bool {
        self.project_held && id == PROJECT
    }

    /// File the one project this endpoint holds, its content stored as Linear stores it.
    fn create_project(&mut self, input: &Map<String, Value>) -> Result<Value, String> {
        if self.project_held {
            return Err(format!(
                "the loopback holds one project, and {PROJECT} is filed"
            ));
        }
        let (Some(name), Some(content), Some(status)) = (
            input.get("name").and_then(Value::as_str),
            input.get("content").and_then(Value::as_str),
            input.get("statusId").and_then(Value::as_str),
        ) else {
            return Err(
                "a projectCreate carries a `name`, a `content` and a `statusId`".to_owned(),
            );
        };
        let Some(status) = self.statuses.iter().find(|named| named.id == status) else {
            return Err(format!("no project status {status}"));
        };
        self.project_status = status.name.clone();
        self.project_name = name.to_owned();
        self.project_content = linear_stores(content);
        self.project_held = true;
        Ok(self.project())
    }

    /// File one issue of the project, its description stored as Linear stores it.
    fn create_issue(&mut self, input: &Map<String, Value>) -> Result<Value, String> {
        let text = |field: &str| input.get(field).and_then(Value::as_str);
        let (Some(title), Some(description), Some(state)) =
            (text("title"), text("description"), text("stateId"))
        else {
            return Err(
                "an issueCreate carries a `title`, a `description` and a `stateId`".to_owned(),
            );
        };
        if !self.states.iter().any(|named| named.id == state) {
            return Err(format!("no workflow state {state}"));
        }
        if let Some(project) = text("projectId").filter(|project| !self.holds(project)) {
            return Err(format!("Entity not found: Project {project}"));
        }
        let n = self.issues.len() + 1;
        let issue = Issue {
            id: format!("issue-{n}"),
            identifier: format!("{TEAM}-{n}"),
            title: title.to_owned(),
            description: linear_stores(description),
            state: state.to_owned(),
            priority: input.get("priority").and_then(Value::as_u64).unwrap_or(0),
        };
        let answer = self.render(&issue);
        self.issues.push(issue);
        Ok(answer)
    }

    /// File a document under the project, or write the one `target` names, its content stored
    /// as Linear stores it.
    fn write_document(
        &mut self,
        target: Option<&str>,
        input: &Map<String, Value>,
    ) -> Result<Value, String> {
        let text = |field: &str| input.get(field).and_then(Value::as_str);
        if let Some(project) = text("projectId").filter(|project| !self.holds(project)) {
            return Err(format!("Entity not found: Project {project}"));
        }
        let document = match target {
            Some(id) => self
                .documents
                .iter_mut()
                .find(|held| held.id == id)
                .ok_or_else(|| "Entity not found: Document".to_owned())?,
            None => {
                let (Some(title), Some(_)) = (text("title"), text("content")) else {
                    return Err("a documentCreate carries a `title` and a `content`".to_owned());
                };
                let id = format!("document-{}", self.documents.len() + 1);
                self.documents.push(Document {
                    id,
                    title: title.to_owned(),
                    content: String::new(),
                });
                self.documents.last_mut().expect("just filed")
            }
        };
        if let Some(title) = text("title") {
            title.clone_into(&mut document.title);
        }
        if let Some(content) = text("content") {
            document.content = linear_stores(content);
        }
        let document = document.clone();
        Ok(self.render_document(&document))
    }

    fn render_document(&self, document: &Document) -> Value {
        json!({
            "id": document.id,
            "title": document.title,
            "content": document.content,
            "url": format!("https://linear.app/generic/document/{}", document.id),
            "createdAt": "2026-10-01T00:00:00.000Z",
            "updatedAt": "2026-10-01T00:00:00.000Z",
            "archivedAt": null,
            "project": {"id": PROJECT},
        })
    }

    fn render(&self, issue: &Issue) -> Value {
        let state = self
            .states
            .iter()
            .find(|named| named.id == issue.state)
            .map(|named| json!({"name": named.name, "type": named.kind}));
        json!({
            "id": issue.id,
            "identifier": issue.identifier,
            "title": issue.title,
            "description": issue.description,
            "url": format!("https://linear.app/generic/issue/{}", issue.identifier),
            "createdAt": "2026-10-01T00:00:00.000Z",
            "updatedAt": "2026-10-01T00:00:00.000Z",
            "archivedAt": null,
            "state": state,
            "priority": issue.priority,
            "labels": {"nodes": []},
            "project": {"id": PROJECT},
        })
    }

    /// Everything a status write resolves: the team's workflow states and the workspace's
    /// project statuses, and the workspace used to verify a private source.
    fn resolution(&self) -> Value {
        json!({
            "organization": {"id": "organization-1"},
            "teams": {"nodes": [{"id": "team-1", "states": {
                "nodes": self.states.iter().map(|named| json!({
                    "id": named.id, "name": named.name, "type": named.kind,
                })).collect::<Vec<_>>(),
                "pageInfo": {"hasNextPage": false},
            }}]},
            "projectStatuses": {
                "nodes": self.statuses.iter().enumerate().map(|(at, named)| json!({
                    "id": named.id, "name": named.name, "type": named.kind,
                    "position": at,
                })).collect::<Vec<_>>(),
                "pageInfo": {"hasNextPage": false},
            },
        })
    }

    fn issue(&self, id: &str) -> Value {
        self.issues
            .iter()
            .find(|held| held.id == id)
            .map_or(Value::Null, |held| self.render(held))
    }

    fn project(&self) -> Value {
        let status = self
            .statuses
            .iter()
            .find(|named| named.name == self.project_status)
            .map(|named| json!({"name": named.name, "type": named.kind}));
        json!({
            "id": PROJECT,
            "name": self.project_name,
            "content": self.project_content,
            "url": "https://linear.app/generic/project/project-1",
            "createdAt": "2026-10-01T00:00:00.000Z",
            "updatedAt": "2026-10-01T00:00:00.000Z",
            "archivedAt": null,
            "status": status,
            "labels": {"nodes": []},
        })
    }

    fn update(&mut self, id: &str, input: &Map<String, Value>) -> Result<Value, String> {
        if let Some(state) = input.get("stateId").and_then(Value::as_str) {
            if !self.states.iter().any(|named| named.id == state) {
                return Err(format!("no workflow state {state}"));
            }
        }
        // Every field judged before any is written, so a refused input changes nothing.
        for (field, value) in input {
            let valid = match (field.as_str(), value) {
                ("stateId" | "description" | "title", Value::String(_)) => true,
                // A rewrite sends the issue whole: its project, which has to be the one held,
                // and its labels, of which this endpoint holds none.
                ("projectId", Value::String(project)) => self.holds(project),
                ("labelIds", Value::Array(labels)) => labels.is_empty(),
                // Linear's own scale: 0 for none, then 1 (urgent) to 4 (low).
                ("priority", Value::Number(priority)) => {
                    priority.as_u64().is_some_and(|priority| priority <= 4)
                }
                _ => false,
            };
            if !valid {
                return Err(format!("the loopback writes no {field} of {value}"));
            }
        }
        let Some(issue) = self.issues.iter_mut().find(|held| held.id == id) else {
            return Err("Entity not found: Issue".to_owned());
        };
        let text = |field: &str| input.get(field).and_then(Value::as_str).map(str::to_owned);
        if let Some(state) = text("stateId") {
            issue.state = state;
        }
        if let Some(description) = text("description") {
            issue.description = linear_stores(&description);
        }
        if let Some(title) = text("title") {
            issue.title = title;
        }
        if let Some(priority) = input.get("priority") {
            issue.priority = priority.as_u64().expect("validated priority");
        }
        let issue = issue.clone();
        let mut answer = self.render(&issue);
        answer["relations"] = empty_page();
        Ok(answer)
    }
}

fn empty_page() -> Value {
    json!({"nodes": [], "pageInfo": {"hasNextPage": false, "endCursor": null}})
}

fn refusal(message: &str) -> Value {
    json!({"errors": [{"message": message, "extensions": {"code": "INVALID_INPUT"}}]})
}

/// The largest request body this endpoint reads: far past any document the plugin sends, and
/// a bound on what a malformed `Content-Length` can make it allocate.
const MAX_BODY: usize = 1 << 20;

/// One HTTP/1.1 request's body, or `None` once the connection has closed or sent a request
/// this endpoint will not read.
fn read_request(reader: &mut BufReader<TcpStream>) -> Option<Vec<u8>> {
    let line = |reader: &mut BufReader<TcpStream>| -> Option<String> {
        let mut line = String::new();
        (&mut *reader).take(8193).read_line(&mut line).ok()?;
        if line.len() > 8192 || !line.ends_with("\r\n") {
            return None;
        }
        Some(line)
    };
    if line(reader)?.trim_end() != "POST /graphql HTTP/1.1" {
        return None;
    }
    let mut length = None;
    let mut headers = 0usize;
    loop {
        let header = line(reader)?;
        headers += header.len();
        if headers > 65536 {
            return None;
        }
        if header == "\r\n" {
            break;
        }
        let (name, value) = header.trim_end().split_once(':')?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || name.eq_ignore_ascii_case("transfer-encoding")
        {
            return None;
        }
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() || !value.trim().bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|length| *length > 0 && *length <= MAX_BODY)?,
            );
        }
    }
    let length = length?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(body)
}

/// The description Linear holds for an item whose visible content is `content` and whose
/// metadata is `metadata`, in the slot the linked plugin writes: one line, the canonical JSON in
/// a code span.
pub fn described(content: &str, metadata: &Value) -> String {
    format!("{content}\n\n<!-- onetaskgraph.metadata `{metadata}` -->")
}

/// The metadata a description's slot holds, or an empty map where it holds none.
pub fn metadata_of(description: &str) -> Map<String, Value> {
    description
        .rsplit_once("<!-- onetaskgraph.metadata `")
        .and_then(|(_, slot)| slot.rsplit_once("` -->"))
        .and_then(|(json, _)| serde_json::from_str(json).ok())
        .unwrap_or_default()
}

/// How long a journey waits for this endpoint before it calls a wait lost.
pub const PATIENCE: Duration = Duration::from_secs(120);

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] this models a remote service's behaviour, not a contract with a source to reconcile against: hosted Linear publishes no statement of what it stores, the spike's measured stored forms exist only in its report, and the one declaration a gate once compared this with — the Linear plugin's `stored_form` — is what onetaskgraph 0.12.4 retires, because a table of Linear's rewrites never converges. Nothing in the engine or the store reads this model: the launch journey needs only rewrites the store does not predict, stable on their own output, which `the_loopback_stores_every_rewrite_the_spike_measured` holds; drift against hosted Linear is what onetaskgraph-linear's credentialled live lane measures.
/// What Linear stores when it is written Markdown, as this endpoint models it: every write of an
/// issue's `description`, a project's `content` and a document's `content` is stored as this
/// answers, a read hands back what was stored, and a write's own answer selects it too.
///
/// It models what spike `spike-linear-stored-content` measured real Linear storing, which
/// onetaskgraph-linear's ruling **"what Linear stores is read off each write's own answer"**
/// records (`crates/onetaskgraph-linear/src/lib.rs`): Linear re-serializes the whole document,
/// so the store predicts none of it and takes what each write's answer reports. Each rewrite here
/// is one the spike measured — [`MEASURED`] lists them by probe — and five of them are ones no
/// declared table of Linear's rules ever predicted: a blank line set before a list that follows a
/// heading or a paragraph line, an ordered item on a list item's lazy continuation line escaped,
/// an inline code span wrapped across two lines joined, `**bold**` spanning a line break split
/// into one run a line, and an indented code block fenced. Content ending in a table row keeps
/// that row's line break, so the one blank line a metadata slot is set off by is stored as one
/// line break after it. A fenced block, a code span on one line and an HTML comment line — the
/// metadata slot among them — are kept byte for byte, and storing what it stored changes nothing
/// more, as the spike measured of every follow-up write.
pub fn linear_stores(content: &str) -> String {
    let lines = split_bold(&join_code_spans(content.split('\n').collect()));
    let mut stored: Vec<String> = Vec::with_capacity(lines.len());
    let mut fence: Option<(char, usize)> = None;
    // The column a list item's text starts at, while the lines after it continue it.
    let mut item: Option<usize> = None;
    // Whether that item is a bullet, after whose text an ordered item cannot open a list.
    let mut bullet = false;
    let mut before = Before::Nothing;
    let mut at = 0;
    while at < lines.len() {
        let line = &lines[at];
        at += 1;
        if let Some((mark, opened)) = fence {
            if fence_of(line).is_some_and(|(closing, run, rest)| {
                closing == mark && run >= opened && rest.trim().is_empty()
            }) {
                fence = None;
                before = Before::Fence;
            }
            stored.push(line.clone());
            continue;
        }
        if line.trim().is_empty() {
            if !matches!(before, Before::Nothing | Before::Blank) {
                stored.push(String::new());
            }
            before = Before::Blank;
            continue;
        }
        if matches!(before, Before::Fence) {
            // A closed fence is set apart from whatever follows it.
            stored.push(String::new());
            before = Before::Blank;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        let text = &line[indent..];
        let continues = item.filter(|column| {
            matches!(before, Before::Item) || (matches!(before, Before::Blank) && indent >= *column)
        });
        if continues.is_none() {
            item = None;
        }
        let set_apart = |stored: &mut Vec<String>, before: Before| {
            if !matches!(before, Before::Nothing | Before::Blank) {
                stored.push(String::new());
            }
        };
        if let Some((mark, run, _)) = fence_of(line) {
            set_apart(&mut stored, before);
            fence = Some((mark, run));
            item = None;
            stored.push(line.clone());
            before = Before::Block;
        } else if indent >= 4 && item.is_none() && matches!(before, Before::Nothing | Before::Blank)
        {
            // An indented code block, through its last indented line, is stored fenced.
            let mut end = at;
            while end < lines.len()
                && (lines[end].starts_with("    ") || lines[end].trim().is_empty())
            {
                end += 1;
            }
            while end > at && lines[end - 1].trim().is_empty() {
                end -= 1;
            }
            stored.push("```".to_owned());
            for code in std::iter::once(line).chain(&lines[at..end]) {
                stored.push(code.get(4..).unwrap_or_default().to_owned());
            }
            stored.push("```".to_owned());
            at = end;
            before = Before::Fence;
        } else if let Some(rest) = text.strip_prefix("- ").or_else(|| text.strip_prefix("* ")) {
            if matches!(before, Before::Line | Before::Heading) {
                stored.push(String::new());
            }
            stored.push(format!("{}* {rest}", " ".repeat(indent)));
            item = Some(indent + 2);
            bullet = true;
            before = Before::Item;
        } else if let Some((number, rest)) = ordered(text) {
            if let Some(column) = continues.filter(|_| bullet && number != "1") {
                // Not a list: an ordered item other than the first cannot interrupt the item's
                // text, so the line runs on from it and its `.` is escaped.
                stored.push(format!(
                    "{}{number}\\. {rest}",
                    " ".repeat(indent.max(column))
                ));
            } else {
                if matches!(before, Before::Line | Before::Heading) {
                    stored.push(String::new());
                }
                stored.push(line.clone());
                item = Some(indent + number.len() + 2);
                bullet = false;
            }
            before = Before::Item;
        } else if let Some(column) = continues.filter(|_| !text.starts_with(['#', '>', '<', '|'])) {
            stored.push(format!("{}{text}", " ".repeat(indent.max(column))));
            before = Before::Item;
        } else if text.starts_with('#') {
            item = None;
            stored.push(line.clone());
            before = Before::Heading;
        } else if image_alone(text) {
            item = None;
            if matches!(before, Before::Image) {
                stored.push(String::new());
            }
            stored.push(line.clone());
            before = Before::Image;
        } else if let Some(row) = delimiter_row(text) {
            item = None;
            stored.push(row);
            before = Before::Block;
        } else if table_row(text) || text.starts_with('<') {
            item = None;
            stored.push(line.clone());
            before = Before::Block;
        } else {
            item = None;
            stored.push(line.clone());
            before = Before::Line;
        }
    }
    while stored.last().is_some_and(|line| line.trim().is_empty()) {
        stored.pop();
    }
    let ends_in_table = fence.is_none()
        && item.is_none()
        && stored.last().is_some_and(|line| table_row(line.trim()));
    let mut stored = stored.join("\n");
    if ends_in_table {
        stored.push('\n');
    }
    stored
}

/// What the line before the one being stored was, as [`linear_stores`] sets lines apart by it.
#[derive(Clone, Copy)]
enum Before {
    Nothing,
    Blank,
    /// A paragraph's line.
    Line,
    Heading,
    /// A list item's line, or a line running on from one.
    Item,
    Image,
    /// A fence's closing line.
    Fence,
    /// A fence's opening line, a table row or an HTML line.
    Block,
}

/// Every rewrite [`linear_stores`] models, by the spike probe that measured it: what was written
/// and what Linear stored.
pub const MEASURED: &[(&str, &str, &str)] = &[
    (
        "bullet-list",
        "- first item\n- second item\n  - nested item",
        "* first item\n* second item\n  * nested item",
    ),
    (
        "pipe-table",
        "| A | B |\n| --- |:---:|\n| one | two |",
        "| A | B |\n| -- | :--: |\n| one | two |\n",
    ),
    (
        "list-after-heading",
        "## A heading\n- first item",
        "## A heading\n\n* first item",
    ),
    (
        "list-after-paragraph",
        "A paragraph line.\n- first item",
        "A paragraph line.\n\n* first item",
    ),
    (
        "lazy-ordered-item",
        "- a list item whose text wraps onto\n3. a lazy line\n- the next item",
        "* a list item whose text wraps onto\n  3\\. a lazy line\n* the next item",
    ),
    (
        "code-span-wrap",
        "a `code span that\nwraps across a line break` in prose",
        "a `code span that wraps across a line break` in prose",
    ),
    (
        "bold-wrap",
        "has **bold text that spans\na line break** in it",
        "has **bold text that spans**\n**a line break** in it",
    ),
    (
        "indented-code",
        "Before.\n\n    line one\n    line two\n\nAfter.",
        "Before.\n\n```\nline one\nline two\n```\n\nAfter.",
    ),
    (
        "adjacent-images",
        "![a](https://uploads.linear.app/a)\n![b](https://uploads.linear.app/b)",
        "![a](https://uploads.linear.app/a)\n\n![b](https://uploads.linear.app/b)",
    ),
    (
        "fenced-block",
        "Before.\n```\n- **x\n```\nAfter.",
        "Before.\n\n```\n- **x\n```\n\nAfter.",
    ),
    (
        "table-before-slot",
        "| a |\n| -- |\n\n\n<!-- onetaskgraph.metadata `{\"k\":\"**\"}` -->",
        "| a |\n| -- |\n\n<!-- onetaskgraph.metadata `{\"k\":\"**\"}` -->",
    ),
];
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// The content a description holding a metadata slot reports, as a read of it does: the text
/// above the slot less the one blank line that sets it off — of which only a line break is left
/// after a table row, which keeps its own.
pub fn content_of(description: &str) -> &str {
    let above = description
        .rfind("<!-- onetaskgraph.metadata `")
        .map_or(description, |slot| &description[..slot]);
    match above.strip_suffix("\n\n") {
        Some(visible) if table_row(visible.rsplit('\n').next().unwrap_or_default().trim()) => {
            &above[..above.len() - 1]
        }
        Some(visible) => visible,
        None => above.strip_suffix('\n').unwrap_or(above),
    }
}

/// The lines of `lines`, each inline code span wrapped across a line break joined onto one line.
fn join_code_spans(lines: Vec<&str>) -> Vec<String> {
    let mut joined: Vec<String> = Vec::with_capacity(lines.len());
    let mut fence: Option<(char, usize)> = None;
    let mut open = false;
    for line in lines {
        if open && !line.trim().is_empty() {
            let last = joined.last_mut().expect("an open span follows a line");
            last.push(' ');
            last.push_str(line.trim_start());
            open = last.matches('`').count() % 2 == 1;
            continue;
        }
        open = false;
        match fence {
            Some((mark, opened)) => {
                if fence_of(line).is_some_and(|(closing, run, rest)| {
                    closing == mark && run >= opened && rest.trim().is_empty()
                }) {
                    fence = None;
                }
            }
            None => match fence_of(line) {
                Some((mark, run, _)) => fence = Some((mark, run)),
                None => open = line.matches('`').count() % 2 == 1,
            },
        }
        joined.push(line.to_owned());
    }
    joined
}

/// The lines of `lines`, each `**` run left open at a line's end closed there and opened again on
/// the next line.
fn split_bold(lines: &[String]) -> Vec<String> {
    let mut split: Vec<String> = Vec::with_capacity(lines.len());
    let mut fence: Option<(char, usize)> = None;
    let mut open = false;
    for line in lines {
        let mut line = line.clone();
        if let Some((mark, opened)) = fence {
            if fence_of(&line).is_some_and(|(closing, run, rest)| {
                closing == mark && run >= opened && rest.trim().is_empty()
            }) {
                fence = None;
            }
            split.push(line);
            continue;
        }
        if let Some((mark, run, _)) = fence_of(&line) {
            fence = Some((mark, run));
            open = false;
            split.push(line);
            continue;
        }
        if line.trim().is_empty() {
            open = false;
            split.push(line);
            continue;
        }
        if open {
            let indent = line.len() - line.trim_start().len();
            line.insert_str(indent, "**");
            split
                .last_mut()
                .expect("an open run follows a line")
                .push_str("**");
        }
        open = outside_code_spans(&line).matches("**").count() % 2 == 1;
        split.push(line);
    }
    split
}

/// `line` with every inline code span taken out.
fn outside_code_spans(line: &str) -> String {
    line.split('`').step_by(2).collect()
}

/// The fence a line opens or closes — its character, how many, and what follows — where it is
/// indented at most three spaces and opens with three or more backticks or tildes.
fn fence_of(line: &str) -> Option<(char, usize, &str)> {
    let text = line.trim_start_matches(' ');
    if line.len() - text.len() > 3 {
        return None;
    }
    let mark = text.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let run = text.len() - text.trim_start_matches(mark).len();
    (run >= 3).then(|| (mark, run, &text[run..]))
}

/// The number and the text of an ordered list item a line opens, where it opens one.
fn ordered(text: &str) -> Option<(&str, &str)> {
    let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let rest = text[digits..].strip_prefix(". ")?;
    (digits > 0).then(|| (&text[..digits], rest))
}

/// Whether a line, unindented, holds one image and nothing else.
fn image_alone(text: &str) -> bool {
    let Some((alt, target)) = text
        .trim_end()
        .strip_prefix("![")
        .and_then(|rest| rest.split_once("]("))
    else {
        return false;
    };
    // The image's target closes the line: nothing follows its one closing parenthesis.
    !alt.contains(']')
        && target
            .strip_suffix(')')
            .is_some_and(|target| !target.contains(['(', ')']))
}

/// Whether a line, trimmed, is a table row.
fn table_row(text: &str) -> bool {
    text.len() > 1 && text.starts_with('|') && text.ends_with('|')
}

/// A table's delimiter row as Linear stores it — one `--` a cell, its alignment kept — or `None`
/// where the line is not one.
fn delimiter_row(text: &str) -> Option<String> {
    let inner = text.trim_end().strip_prefix('|')?.strip_suffix('|')?;
    let cells = inner
        .split('|')
        .map(|cell| {
            let cell = cell.trim();
            let dashes = cell.trim_matches(':');
            (!dashes.is_empty() && dashes.bytes().all(|b| b == b'-')).then(|| {
                let left = if cell.starts_with(':') { ":" } else { "" };
                let right = if cell.len() > 1 && cell.ends_with(':') {
                    ":"
                } else {
                    ""
                };
                format!("{left}--{right}")
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(format!("| {} |", cells.join(" | ")))
}

/// Refused HTTP frames and GraphQL envelopes never reach the endpoint's operation log;
/// the same endpoint still answers a valid request afterwards.
#[test]
fn malformed_requests_are_refused_before_a_valid_graphql_read() {
    let linear = Linear::serve(
        &[("Todo", "unstarted")],
        &[("Planned", "planned")],
        "generic",
        "",
        "Planned",
    );
    let send = |request_line: &str, body: &Value| {
        let mut stream =
            TcpStream::connect(("127.0.0.1", linear.port)).expect("connect to the loopback");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("bounded response read");
        let body = body.to_string();
        // The endpoint may refuse a request on its first line and close the connection while
        // the rest is still being sent, so the send and its shutdown can find the connection
        // gone: what is asserted is that nothing was answered.
        let _ = write!(
            stream,
            "{request_line}\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.shutdown(std::net::Shutdown::Write);
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        response
    };
    let valid = json!({"query":"query($id:ID!){project(id:$id){id}}", "variables":{"id":PROJECT}});
    for first in [
        "GET /graphql HTTP/1.1",
        "POST /elsewhere HTTP/1.1",
        "POST /graphql malformed",
    ] {
        assert!(send(first, &valid).is_empty());
    }
    for body in [
        json!([]),
        json!({"query":1, "variables":{}}),
        json!({"query":"query {}", "variables":[]}),
        json!({"query":"query {}", "variables":{}, "extra":true}),
        json!({"query":"query {}", "variables":{}, "operationName":4}),
    ] {
        assert!(send("POST /graphql HTTP/1.1", &body).is_empty());
    }
    for query in [
        "query($id:ID!){issue(id:$id){id}}",
        "query($id:ID!){issue(id:$id){relations(first:10){nodes{id}}}}",
        "query($id:ID!){project(id:$id){id}}",
        "query($id:ID!){project(id:$id){relations(first:10){nodes{id}}}}",
    ] {
        for variables in [
            json!({}),
            json!({"id":null}),
            json!({"id":4}),
            json!({"id":""}),
        ] {
            assert!(
                send(
                    "POST /graphql HTTP/1.1",
                    &json!({"query":query,"variables":variables})
                )
                .is_empty(),
                "a read with an invalid ID was accepted"
            );
        }
    }
    assert!(
        linear.log().is_empty(),
        "invalid requests reached operation handling"
    );
    let issue = linear.file("work", "description", "Todo");
    let update = json!({
        "query":"mutation($id:ID!,$input:IssueUpdateInput!){issueUpdate(id:$id,input:$input){success}}",
        "variables":{"id":issue,"input":{"priority":3}}
    });
    let updated = send("POST /graphql HTTP/1.1", &update);
    let updated: Value =
        serde_json::from_str(updated.split_once("\r\n\r\n").expect("mutation response").1)
            .expect("mutation JSON");
    assert_eq!(updated["data"]["issueUpdate"]["issue"]["priority"], 3);
    let read = send(
        "POST /graphql HTTP/1.1",
        &json!({
            "query":"query($id:ID!){issue(id:$id){priority}}", "variables":{"id":issue}
        }),
    );
    let read: Value = serde_json::from_str(read.split_once("\r\n\r\n").expect("issue response").1)
        .expect("issue JSON");
    assert_eq!(read["data"]["issue"]["priority"], 3);
    let response = send("POST /graphql HTTP/1.1", &valid);
    let body = response.split_once("\r\n\r\n").expect("an HTTP response").1;
    let answer: Value = serde_json::from_str(body).expect("the GraphQL response");
    assert_eq!(answer["data"]["project"]["id"], PROJECT);
    assert_eq!(linear.log().len(), 3);
    // A rewrite naming a project whose name is not a string is refused and changes nothing,
    // its status among it.
    let rename = json!({
        "query":"mutation($id:ID!,$input:ProjectUpdateInput!){projectUpdate(id:$id,input:$input){success}}",
        "variables":{"id":PROJECT,"input":{
            "content":"rewritten", "name":7, "statusId":linear.status_id("Planned"),
        }}
    });
    let refused = send("POST /graphql HTTP/1.1", &rename);
    let refused: Value =
        serde_json::from_str(refused.split_once("\r\n\r\n").expect("mutation response").1)
            .expect("mutation JSON");
    assert_eq!(
        refused["errors"][0]["message"],
        "a project's name is a string: 7"
    );
    let read = send(
        "POST /graphql HTTP/1.1",
        &json!({
            "query":"query($id:ID!){project(id:$id){name content}}", "variables":{"id":PROJECT}
        }),
    );
    let read: Value = serde_json::from_str(read.split_once("\r\n\r\n").expect("project read").1)
        .expect("project JSON");
    assert_eq!(read["data"]["project"]["content"], "");
}

/// Every rewrite the spike measured is what this endpoint stores, and storing what it stored
/// changes nothing more — what the spike measured of every follow-up write's answer.
#[test]
fn the_loopback_stores_every_rewrite_the_spike_measured() {
    for (probe, written, kept) in MEASURED {
        assert_eq!(linear_stores(written), *kept, "{probe}: {written:?}");
        assert_eq!(
            linear_stores(kept),
            *kept,
            "{probe}, stored again: {kept:?}"
        );
    }
    // What no measured rewrite names comes back as written: the plugin's own metadata slot, a
    // code span on one line and a numbered list among it.
    for kept in [
        "snake_case_name and file_name.md",
        "<!-- onetaskgraph.metadata `{\"k\":\"[x] ~ _y_ **\"}` -->",
        "a `- [x] **b` c",
        "1. one\n2. two",
        "**bold**, # heading, > quoted - dash",
        "* item\n\n<!-- onetaskgraph.metadata `{}` -->",
        // An image with prose after it is not an image alone, so no blank line parts it from
        // the image below.
        "![a](https://uploads.linear.app/a) and (b)\n![b](https://uploads.linear.app/b)",
    ] {
        assert_eq!(linear_stores(kept), kept, "{kept:?}");
    }
    // A read's content is the text above the slot less its separator, and a table's own line
    // break survives it.
    for (stored, content) in [
        ("* a\n\n<!-- onetaskgraph.metadata `{}` -->", "* a"),
        (
            "| a |\n| -- |\n\n<!-- onetaskgraph.metadata `{}` -->",
            "| a |\n| -- |\n",
        ),
        ("no slot", "no slot"),
    ] {
        assert_eq!(content_of(stored), content, "{stored:?}");
    }
}
