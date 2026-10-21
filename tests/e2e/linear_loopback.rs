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
    /// `issue(id:)`: one issue, read whole — the description a settlement's metadata is merged
    /// into.
    Issue,
    /// `issue(id:){relations inverseRelations}`: one issue's dependency edges.
    IssueRelations,
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
        if query.contains("teams(") && query.contains("projectStatuses(") {
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
        self == Self::Resolution
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
            project_content: stored_form(description),
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
        held.description = stored_form(&edit(&held.description));
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
            description: stored_form(description),
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
            Operation::Resolution => json!({
                "teams": {"nodes": [{"id": "team-1", "states": {
                    "nodes": world.states.iter().map(|named| json!({
                        "id": named.id, "name": named.name, "type": named.kind,
                    })).collect::<Vec<_>>(),
                    "pageInfo": {"hasNextPage": false},
                }}]},
                "projectStatuses": {
                    "nodes": world.statuses.iter().enumerate().map(|(at, named)| json!({
                        "id": named.id, "name": named.name, "type": named.kind,
                        "position": at,
                    })).collect::<Vec<_>>(),
                    "pageInfo": {"hasNextPage": false},
                },
            }),
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
                    Ok(issue) => json!({"issueUpdate": {"success": true, "issue": issue}}),
                    Err(why) => return refusal(&why),
                }
            }
            Operation::ProjectUpdate if variables["input"].get("statusId").is_some() => {
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
                world.project_content = stored_form(content);
                json!({"projectUpdate": {"success": true, "project": {
                    "id": PROJECT, "content": world.project_content,
                }}})
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
        self.project_content = stored_form(content);
        self.project_held = true;
        Ok(json!({"id": PROJECT, "content": self.project_content}))
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
            description: stored_form(description),
            state: state.to_owned(),
            priority: input.get("priority").and_then(Value::as_u64).unwrap_or(0),
        };
        let answer = json!({"id": issue.id, "description": issue.description});
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
            document.content = stored_form(content);
        }
        Ok(json!({"id": document.id, "content": document.content}))
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
            issue.description = stored_form(&description);
        }
        if let Some(title) = text("title") {
            issue.title = title;
        }
        if let Some(priority) = input.get("priority") {
            issue.priority = priority.as_u64().expect("validated priority");
        }
        let issue = issue.clone();
        Ok(self.render(&issue))
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

/// What Linear stores when it is written Markdown, as this endpoint models it: every write of an
/// issue's `description`, a project's `content` and a document's `content` is stored as this
/// answers, and a read hands back what was stored.
///
/// The rules are onetaskgraph-linear's, from the section of its crate documentation headed
/// **"Ruling: what Linear does to Markdown on write, declared"**
/// (`crates/onetaskgraph-linear/src/lib.rs`), which is their one authoritative statement. This
/// is a model of that ruling written here, not a call into the plugin's own rewrite, so a plugin
/// that sends what Linear would rewrite is answered with the rewritten text — and is refused
/// `stored-content-diverged` by the store, or read back with a digest its provenance does not
/// record — exactly as hosted Linear would answer it. Every rule holds outside a fenced block
/// and outside an inline code span opened and closed on one line, which are kept byte for byte,
/// and a Markdown link or image is kept as written. Each rule is listed in [`RULES`] with the
/// ruling's own example, and `the_loopback_stores_every_example_the_ruling_states` holds each:
///
/// - a line opening `- ` is a `* ` bullet;
/// - a line after a list item that opens no block of its own runs on from it, indented under its
///   text;
/// - a table's delimiter row outside a list item is one `--` a cell, alignment colons kept, one
///   space beside every pipe;
/// - an `http://` or `https://` URL is linked to itself, trailing punctuation outside the link;
/// - a domain-like token under one of [`AUTOLINKED`] is linked to `http://` and itself;
/// - a `[` that opens no link, and every `]`, are escaped;
/// - a `~` is escaped;
/// - a single-underscore emphasis is written with asterisks;
/// - the backslash of `\"` is dropped, and a backslash before a letter is doubled;
/// - a line opening `-->` is escaped;
/// - a fenced block and a lone image are set apart by a blank line from the lines beside them;
/// - a run of blank lines is one blank line;
/// - the content ends at its last line holding anything, but for a table's last row, which keeps
///   its line break.
pub fn stored_form(content: &str) -> String {
    let mut stored = String::with_capacity(content.len());
    // The fence a block is open under: its character and how many of it opened it.
    let mut fence: Option<(char, usize)> = None;
    // While the lines that follow a list item continue it, the column its text starts at.
    let mut item: Option<usize> = None;
    // The line before closed a fence or held an image alone, so the next line holding anything
    // is set apart from it.
    let mut set_apart = false;
    // The line before was blank, or there was none.
    let mut after_blank = true;
    // The line before was blank, so a blank line now joins it.
    let mut in_blank_run = false;
    for line in content.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let ending = &line[body.len()..];
        if let Some((mark, opened)) = fence {
            if fence_of(body).is_some_and(|(closing, run, rest)| {
                closing == mark && run >= opened && rest.trim().is_empty()
            }) {
                fence = None;
                set_apart = true;
            }
            stored.push_str(line);
            after_blank = false;
            in_blank_run = false;
            continue;
        }
        if body.trim().is_empty() {
            if !in_blank_run {
                stored.push_str(line);
            }
            item = None;
            set_apart = false;
            after_blank = true;
            in_blank_run = true;
            continue;
        }
        in_blank_run = false;
        let opens_fence = fence_of(body);
        let image = image_alone(body);
        if !after_blank && (set_apart || opens_fence.is_some() || image) {
            stored.push('\n');
        }
        set_apart = image;
        after_blank = false;
        if let Some((mark, run, _)) = opens_fence {
            fence = Some((mark, run));
            item = None;
            stored.push_str(line);
            continue;
        }
        let indent_len = body.len() - body.trim_start_matches([' ', '\t']).len();
        let (indent, text) = body.split_at(indent_len);
        let bullet = text.strip_prefix("- ").or_else(|| text.strip_prefix("* "));
        let runs_on = item.filter(|_| bullet.is_none() && !image && !opens_own_block(text));
        if let Some(column) = runs_on {
            stored.push_str(indent);
            stored.push_str(&" ".repeat(column.saturating_sub(indent.len())));
        } else if let Some(row) = delimiter_row(body).filter(|_| !image) {
            item = None;
            stored.push_str(&row);
            stored.push_str(ending);
            continue;
        } else {
            stored.push_str(indent);
        }
        let text = if let Some(rest) = bullet {
            stored.push_str("* ");
            item = Some(indent.len() + 2);
            rest
        } else {
            if runs_on.is_none() {
                item = None;
            }
            text
        };
        if text.starts_with("-->") {
            stored.push('\\');
        }
        stored.push_str(&prose(text));
        stored.push_str(ending);
    }
    if fence.is_none() {
        // Nothing after the last line holding anything is kept: no line break, and no line
        // holding only whitespace.
        loop {
            if stored.ends_with('\n') {
                stored.pop();
                if stored.ends_with('\r') {
                    stored.pop();
                }
                continue;
            }
            match stored.rfind('\n') {
                Some(at) if stored[at + 1..].trim().is_empty() => stored.truncate(at + 1),
                _ => break,
            }
        }
        let last = stored.rsplit('\n').next().unwrap_or_default().trim();
        if item.is_none() && last.len() > 1 && last.starts_with('|') && last.ends_with('|') {
            stored.push('\n');
        }
    }
    stored
}

/// The top-level domains a bare domain-like token is linked under, as the ruling lists them.
pub const AUTOLINKED: &[&str] = &[
    "ai", "app", "co", "com", "dev", "edu", "gov", "info", "io", "live", "me", "net", "org",
];

/// Each rule of the ruling, by its own words, with the example it gives: what is written and what
/// Linear stores.
pub const RULES: &[(&str, &str, &str)] = &[
    (
        "a line opening `- ` is a `* ` bullet",
        "- one\n  - two",
        "* one\n  * two",
    ),
    (
        "a line after a list item that opens no block of its own runs on from the item",
        "- bullet\n| a | b |\n| --- | --- |",
        "* bullet\n  | a | b |\n  | --- | --- |",
    ),
    (
        "a table's delimiter row outside a list item is one `--` a cell",
        "| --- |:---:|",
        "| -- | :--: |\n",
    ),
    (
        "a URL is linked to itself, trailing punctuation outside the link",
        "see https://example.com/a.",
        "see [https://example.com/a](<https://example.com/a>).",
    ),
    (
        "a domain-like token is linked to `http://` and itself",
        "example.com and \"caller.live\"",
        "[example.com](<http://example.com>) and \"[caller.live](<http://caller.live>)\"",
    ),
    (
        "a `[` that opens no link, and every `]`, are escaped",
        "[x] done",
        "\\[x\\] done",
    ),
    ("a `~` is escaped", "~5 days", "\\~5 days"),
    (
        "a single-underscore emphasis is written with asterisks",
        "_y_ and snake_case",
        "*y* and snake_case",
    ),
    (
        "the backslash of `\\\"` is dropped",
        "say \\\"hi\\\"",
        "say \"hi\"",
    ),
    (
        "a backslash before a letter is doubled",
        "a \\d b",
        "a \\\\d b",
    ),
    ("a line opening `-->` is escaped", "-->", "\\-->"),
    (
        "a fenced block is kept byte for byte, and set apart by a blank line",
        "see\n```\n- [x] _y_ example.com ~\n```\n- z",
        "see\n\n```\n- [x] _y_ example.com ~\n```\n\n* z",
    ),
    (
        "an image alone on a line is set apart by a blank line",
        "![a](https://uploads.linear.app/a)\n![b](https://uploads.linear.app/b)",
        "![a](https://uploads.linear.app/a)\n\n![b](https://uploads.linear.app/b)",
    ),
    (
        "an inline code span is kept byte for byte",
        "a `- [x] _y_ example.com ~` b",
        "a `- [x] _y_ example.com ~` b",
    ),
    (
        "a run of blank lines is one blank line",
        "a\n\n\n\nb",
        "a\n\nb",
    ),
    (
        "the content ends at its last line holding anything",
        "![a](https://uploads.linear.app/a)\n\n",
        "![a](https://uploads.linear.app/a)",
    ),
    (
        "a table ending the content is followed by its last row's line break",
        "| a |\n| -- |\n\n",
        "| a |\n| -- |\n",
    ),
];

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

/// Whether a line holds one image and nothing else.
fn image_alone(line: &str) -> bool {
    let text = line.trim();
    text.starts_with("![") && link_at(text, 1) == Some(text.len())
}

/// Whether a line, unindented, opens a block a list item's text does not run on into: a heading,
/// a quotation, an HTML block, a `+ ` bullet or an ordered list's first item.
fn opens_own_block(text: &str) -> bool {
    text.starts_with(['#', '>', '<'])
        || ["+ ", "1. ", "1) "]
            .iter()
            .any(|opening| text.starts_with(opening))
}

/// A table's delimiter row as Linear stores it, or `None` where the line is not one.
fn delimiter_row(line: &str) -> Option<String> {
    let inner = line.trim().strip_prefix('|')?.strip_suffix('|')?;
    let cells = inner
        .split('|')
        .map(|cell| {
            let cell = cell.trim();
            let dashes = cell.trim_matches(':');
            let colons = cell.len() - dashes.len();
            let is_delimiter =
                !dashes.is_empty() && dashes.bytes().all(|b| b == b'-') && colons <= 2;
            is_delimiter.then(|| {
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
    let indent = &line[..line.len() - line.trim_start().len()];
    Some(format!("{indent}| {} |", cells.join(" | ")))
}

/// One line's prose with every inline rule applied, its code spans and links kept.
fn prose(text: &str) -> String {
    let mut stored = String::with_capacity(text.len());
    let mut before: Option<char> = None;
    let mut at = 0;
    while let Some(next) = text[at..].chars().next() {
        let rest = &text[at..];
        let taken = match next {
            '`' => {
                let run = rest.len() - rest.trim_start_matches('`').len();
                let end = span_end(text, at, run).unwrap_or(at + run);
                stored.push_str(&text[at..end]);
                end - at
            }
            '\\' => match rest[1..].chars().next() {
                Some('"') => {
                    stored.push('"');
                    2
                }
                Some(letter) if letter.is_ascii_alphabetic() => {
                    stored.push_str("\\\\");
                    stored.push(letter);
                    2
                }
                other => {
                    let width = 1 + other.map_or(0, char::len_utf8);
                    stored.push_str(&rest[..width]);
                    width
                }
            },
            '!' if rest[1..].starts_with('[') && link_at(text, at + 1).is_some() => {
                let end = link_at(text, at + 1).expect("just checked");
                stored.push_str(&text[at..end]);
                end - at
            }
            '[' => match link_at(text, at) {
                Some(end) => {
                    stored.push_str(&text[at..end]);
                    end - at
                }
                None => {
                    stored.push_str("\\[");
                    1
                }
            },
            ']' => {
                stored.push_str("\\]");
                1
            }
            '~' => {
                stored.push_str("\\~");
                1
            }
            '_' => match emphasis_end(text, at, before) {
                Some(close) => {
                    stored.push('*');
                    stored.push_str(&text[at + 1..close]);
                    stored.push('*');
                    close + 1 - at
                }
                None => {
                    stored.push('_');
                    1
                }
            },
            _ => match linked(rest, before) {
                Some((token, target)) => {
                    stored.push_str(&format!("[{token}](<{target}>)"));
                    token.len()
                }
                None => {
                    stored.push(next);
                    next.len_utf8()
                }
            },
        };
        before = text[..at + taken].chars().next_back();
        at += taken;
    }
    stored
}

/// Just past the run of exactly `run` backticks that closes the code span opened at `at`.
fn span_end(text: &str, at: usize, run: usize) -> Option<usize> {
    let mut from = at + run;
    while let Some(found) = text[from..].find('`') {
        let start = from + found;
        let length = text[start..].len() - text[start..].trim_start_matches('`').len();
        if length == run {
            return Some(start + length);
        }
        from = start + length;
    }
    None
}

/// Just past the Markdown link `[text](target)` whose `[` is at `at`, its target bare or in
/// angle brackets.
fn link_at(text: &str, at: usize) -> Option<usize> {
    let close = at + 1 + text[at + 1..].find(']')?;
    if text[at + 1..close].contains(['[', '`', '\\']) {
        return None;
    }
    let target = text[close + 1..].strip_prefix('(')?;
    let length = if target.starts_with('<') {
        target.find(">)")? + 2
    } else {
        let end = target.find(')')?;
        if target[..end].contains(char::is_whitespace) {
            return None;
        }
        end + 1
    };
    Some(close + 2 + length)
}

/// The `_` closing a single-underscore emphasis opened at `at`, where it is one: opened after no
/// word character, closed before none, around text that neither starts nor ends with whitespace
/// and holds nothing another rule rewrites.
fn emphasis_end(text: &str, at: usize, before: Option<char>) -> Option<usize> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    if before.is_some_and(word) {
        return None;
    }
    let close = at + 1 + text[at + 1..].find('_')?;
    let inner = &text[at + 1..close];
    let clean = !inner.is_empty()
        && !inner.contains(['`', '[', ']', '~', '*', '\\', '<', '>'])
        && inner.trim() == inner;
    (clean && !text[close + 1..].chars().next().is_some_and(word)).then_some(close)
}

/// The token Linear links at the start of `rest`, and the target it links it to: a URL, or a
/// domain-like token under [`AUTOLINKED`], following nothing that continues a word, a path, an
/// address or an escape.
fn linked(rest: &str, before: Option<char>) -> Option<(&str, String)> {
    if before.is_some_and(|c| c.is_alphanumeric() || "@/.-_:\\#=+%".contains(c)) {
        return None;
    }
    let stops = |c: char| c.is_whitespace() || "<>`[]".contains(c);
    let trailing: &[char] = &['.', ',', ':', ';', '!', '?', '\'', '"', ')', '*', '_', '~'];
    let through_path = || {
        let end = rest.find(stops).unwrap_or(rest.len());
        rest[..end].trim_end_matches(trailing)
    };
    if let Some(scheme) = ["http://", "https://"]
        .into_iter()
        .find(|s| rest.starts_with(s))
    {
        let token = through_path();
        return (token.len() > scheme.len()).then(|| (token, token.to_owned()));
    }
    let host_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
        .unwrap_or(rest.len());
    let host = rest[..host_end].trim_end_matches(['.', '-']);
    let labels: Vec<&str> = host.split('.').collect();
    let top = labels.last()?.to_ascii_lowercase();
    if labels.len() < 2
        || labels.iter().any(|label| label.is_empty())
        || !AUTOLINKED.contains(&top.as_str())
    {
        return None;
    }
    let after = rest[host.len()..].chars().next();
    let token = if host.len() == host_end && after == Some('/') {
        through_path()
    } else if after.is_some_and(|c| c == '@' || c == '_') {
        return None;
    } else {
        host
    };
    Some((token, format!("http://{token}")))
}

/// Every example the ruling gives is what this endpoint stores, and storing what it stored
/// changes nothing more — what the ruling records of Linear rewriting its own output.
#[test]
fn the_loopback_stores_every_example_the_ruling_states() {
    for (rule, written, kept) in RULES {
        assert_eq!(stored_form(written), *kept, "{rule}: {written:?}");
        assert_eq!(stored_form(kept), *kept, "{rule}, stored again: {kept:?}");
    }
    // What no rule names comes back as written: the plugin's own metadata slot among it.
    for kept in [
        "snake_case_name and file_name.md",
        "caller.key, README.md, v0.8.0 and e.g. this",
        "a@example.com",
        "<!-- onetaskgraph.metadata `{\"k\":\"[x] ~ _y_\"}` -->",
        "**bold**, 1. one, # heading, > quoted - dash",
        "[named](https://example.com) and ![shot](./shot.png)",
    ] {
        assert_eq!(stored_form(kept), kept, "{kept:?}");
    }
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
}
