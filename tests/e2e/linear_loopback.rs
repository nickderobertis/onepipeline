//! A loopback Linear: one GraphQL endpoint on `127.0.0.1`, served from this test process, that
//! answers the queries and mutations the linked Linear plugin sends to read a plan and to write a
//! settlement back — and logs every request it was sent, which is what a journey counts a
//! write-back's cost by.
//!
//! It holds one team (`TEAM`) with its workflow states, the workspace's project statuses, one
//! project, its issues and the dependencies between them, with a generic per-kind vocabulary: an issue's states and
//! a project's statuses are two different sets of names, which is what the per-kind
//! `status_mapping` exists for. What it answers is decided by the operation each request names —
//! the plugin's own documents, recognised by their root field — and every request it cannot
//! answer is refused as a GraphQL error naming it, so a journey that reaches one fails on what was
//! asked rather than on a guess.
//!
//! A journey can make the next request of one operation fail the way a hosted Linear fails: a
//! GraphQL error ([`Fault::Refuse`]), a connection closed with no answer ([`Fault::Drop`]), or an
//! answer held back until the journey lets it go ([`Fault::Hold`]).
//!
//! It answers what it does not hold as Linear does, on the terms the linked plugin's own
//! `onetaskgraph_linear::graphql` states and its live journeys check against Linear: a read of an
//! issue, project or document id it does not hold is an errored `Entity not found: <type>`
//! answer under HTTP 400, never a null; and a create naming an id a record holds — or held, until
//! a delete took it back — is refused under HTTP 200 as `conflict on insert of <type>`, because
//! Linear lets one id name one record ever.

// llmlint: ignore-file[e2e_not_mocked] this is the one thing the journeys beside it do not drive
// for real: Linear's hosted API, which an offline check may not reach. It is a stand-in for that
// remote service at the network boundary the linked plugin itself crosses — real HTTP on a real
// socket — and nothing inside this crate or the store it links is substituted.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use onetaskgraph_linear::graphql;
use onetaskgraph_plugin_api::MetadataRecord;
use serde_json::{json, Map, Value};
use yrs::types::xml::{XmlElementPrelim, XmlTextPrelim};
use yrs::{Doc, ReadTxn, Transact, Xml, XmlFragment};

/// The team key a source over this endpoint is configured with.
pub const TEAM: &str = "TEAM";

/// The one project this endpoint holds, by the id Linear would give it.
pub const PROJECT: &str = "project-1";

/// Linear's answer to an issue, a project or a document id it does not hold, which it sends
/// under HTTP 400.
const NOT_FOUND: [&str; 3] = [
    graphql::ISSUE_NOT_FOUND_MESSAGE,
    graphql::PROJECT_NOT_FOUND_MESSAGE,
    graphql::DOCUMENT_NOT_FOUND_MESSAGE,
];

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
    /// `issueRelationCreate`: one dependency between two issues filed.
    IssueRelationCreate,
    /// `issueRelationDelete`: one dependency between two issues taken back.
    IssueRelationDelete,
    /// `issueDelete`: one issue taken back, as a copy's undo takes back what it filed.
    IssueDelete,
    /// `projectDelete`: the project taken back, as a copy's undo takes back what it filed.
    ProjectDelete,
    /// `documentDelete`: one document taken back.
    DocumentDelete,
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
        } else if query.starts_with("mutation") && query.contains("issueRelationCreate(") {
            Self::IssueRelationCreate
        } else if query.starts_with("mutation") && query.contains("issueRelationDelete(") {
            Self::IssueRelationDelete
        } else if query.starts_with("mutation") && query.contains("issueDelete(") {
            Self::IssueDelete
        } else if query.starts_with("mutation") && query.contains("projectDelete(") {
            Self::ProjectDelete
        } else if query.starts_with("mutation") && query.contains("documentDelete(") {
            Self::DocumentDelete
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

    /// Whether this is a write: every mutation this endpoint answers, and any it refuses by name
    /// is [`Operation::Unknown`], which a journey counting writes counts too.
    pub fn writes(self) -> bool {
        matches!(
            self,
            Self::IssueUpdate
                | Self::ProjectUpdate
                | Self::IssueCreate
                | Self::ProjectCreate
                | Self::DocumentCreate
                | Self::DocumentUpdate
                | Self::IssueRelationCreate
                | Self::IssueRelationDelete
                | Self::IssueDelete
                | Self::ProjectDelete
                | Self::DocumentDelete
                | Self::Unknown
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
    /// Read whole and applied, and then the connection closed with no answer, as a network
    /// failure after Linear took the request: a write is held, and its answer never arrives.
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

/// One dependency between two issues: `issue` is the one a write named as its own, and
/// `related` the one it named across the relation, which the plugin reads as `issue` depending
/// on `related`.
#[derive(Clone, Debug)]
struct Relation {
    id: String,
    issue: String,
    related: String,
    kind: String,
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
    /// The project's id: [`PROJECT`] for one [`Linear::serve`] holds from the start, and the id
    /// a create sent for one a copy files, as Linear takes the `id` a create names.
    project_id: String,
    issues: Vec<Issue>,
    documents: Vec<Document>,
    relations: Vec<Relation>,
    /// How many relations were ever filed, so a relation filed after one was taken back never
    /// reuses its id.
    relations_filed: usize,
    /// Every id a record this endpoint deleted held: Linear lets one id name one record ever,
    /// so no create may take one, and a read of one finds nothing.
    burnt: Vec<String>,
    log: Vec<Request>,
    faults: BTreeMap<Operation, VecDeque<Fault>>,
    held: bool,
    /// Whether links and headings are held as real Linear holds them: [`Linear::store_links`].
    links_stored: bool,
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
            project_id: PROJECT.to_owned(),
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

    /// Hold links and headings as real Linear does: content stored by [`linear_stores_links`], and
    /// each record answered with the editor state naming its headings' ids, so a heading can be
    /// linked by the id Linear gave it. Without this a store has no such id to name, which is what
    /// the journeys holding that fallback serve.
    pub fn store_links(&self) {
        self.lock().links_stored = true;
    }

    /// The editor state the issue `id` is answered with.
    pub fn issue_editor_state(&self, id: &str) -> String {
        let world = self.lock();
        let held = world
            .issues
            .iter()
            .find(|held| held.id == id)
            .unwrap_or_else(|| panic!("no issue {id}"));
        editor_state(&held.id, &held.description)
    }

    /// The address the issue `id` is answered with.
    pub fn issue_address(&self, id: &str) -> String {
        let world = self.lock();
        let held = world
            .issues
            .iter()
            .find(|held| held.id == id)
            .unwrap_or_else(|| panic!("no issue {id}"));
        world.render(held)["url"]
            .as_str()
            .expect("an issue answers its address")
            .to_owned()
    }

    /// The project's id: [`PROJECT`] for one served from the start, and for one a copy filed
    /// into [`Linear::serve_empty`], the id that copy's create sent, which only it knows.
    pub fn project_id(&self) -> String {
        self.lock().project_id.clone()
    }

    /// Edit one issue's stored description in place, as a person editing it in Linear does:
    /// `edit` is handed what Linear holds and answers what it holds after.
    pub fn edit_description(&self, issue: &str, edit: impl FnOnce(&str) -> String) {
        let mut world = self.lock();
        let links_stored = world.links_stored;
        let held = world
            .issues
            .iter_mut()
            .find(|held| held.id == issue)
            .unwrap_or_else(|| panic!("no issue {issue}"));
        held.description = stored_as(links_stored, &edit(&held.description));
    }

    /// How many requests of `operation` this endpoint was sent: of a create, how many times it
    /// was asked to file one.
    pub fn creates(&self, operation: Operation) -> usize {
        self.lock()
            .log
            .iter()
            .filter(|request| request.operation == operation)
            .count()
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

    /// Every dependency this endpoint holds between two issues, as `(issue, related, type)`:
    /// `issue` depends on `related`.
    pub fn relations(&self) -> Vec<(String, String, String)> {
        self.lock()
            .relations
            .iter()
            .map(|held| (held.issue.clone(), held.related.clone(), held.kind.clone()))
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

    /// Fail no further request: every fault [`Linear::fail_next`] queued and no request took yet
    /// is dropped.
    pub fn clear_faults(&self) {
        self.lock().faults.clear();
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
                    | Operation::IssueRelationDelete
                    | Operation::IssueDelete
                    | Operation::ProjectDelete
                    | Operation::DocumentDelete
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
                Some(Fault::Drop) => {
                    self.answer(operation, query, &variables);
                    return;
                }
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
            // Linear sends its not-found answer under HTTP 400, and every other answer here
            // under HTTP 200 — an id conflict among them.
            let status = if answer["errors"][0]["message"]
                .as_str()
                .is_some_and(|message| NOT_FOUND.contains(&message))
            {
                "400 Bad Request"
            } else {
                "200 OK"
            };
            let body = answer.to_string();
            let written = write!(
                writer,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
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
            | Operation::DocumentUpdate
            | Operation::IssueRelationDelete
            | Operation::IssueDelete
            | Operation::ProjectDelete
            | Operation::DocumentDelete => variables["id"].as_str().expect("validated request ID"),
            _ => "",
        };
        let data = match operation {
            Operation::Resolution => world.resolution(),
            Operation::IssueResolved => {
                let Some(issue) = world.issue(id) else {
                    return not_found(graphql::ISSUE_NOT_FOUND_MESSAGE);
                };
                let mut data = world.resolution();
                data["issue"] = match world.related(issue, query) {
                    Ok(issue) => issue,
                    Err(why) => return refusal(&why),
                };
                data
            }
            Operation::CounterpartLookups => {
                // The issues whose description holds a phrase the filter names, as Linear narrows
                // a page the plugin sizes to the origins it looks up; and the project, whatever
                // the filter: the plugin confirms each row against the origin it looks up, as it
                // does a listing's.
                let mut data = world.resolution();
                let mut phrases = Vec::new();
                contains_phrases(&variables["issuesFilter"], &mut phrases);
                data["originIssues"] = match world.listed(query) {
                    Ok(mut issues) => {
                        issues.retain(|issue| {
                            let held = issue["description"].as_str().unwrap_or_default();
                            phrases.is_empty() || phrases.iter().any(|at| held.contains(at))
                        });
                        page(issues)
                    }
                    Err(why) => return refusal(&why),
                };
                data["originProjects"] = json!({
                    "nodes": if world.project_held { vec![world.project()] } else { Vec::new() },
                    "pageInfo": {"hasNextPage": false, "endCursor": null},
                });
                data
            }
            Operation::Issue => {
                let Some(issue) = world.issue(id) else {
                    return not_found(graphql::ISSUE_NOT_FOUND_MESSAGE);
                };
                match world.related(issue, query) {
                    Ok(issue) => json!({"issue": issue}),
                    Err(why) => return refusal(&why),
                }
            }
            Operation::IssueRelations => {
                let description = world
                    .issues
                    .iter()
                    .find(|held| held.id == id)
                    .map(|held| held.description.clone());
                match description {
                    None => return not_found(graphql::ISSUE_NOT_FOUND_MESSAGE),
                    Some(description) => {
                        // The relation connections are what this document selects beside the
                        // description, so the issue's own id is not among them.
                        let issue = json!({"id": id, "description": description});
                        match world.related(issue, query) {
                            Ok(mut issue) => {
                                issue
                                    .as_object_mut()
                                    .expect("an issue is an object")
                                    .remove("id");
                                json!({"issue": issue})
                            }
                            Err(why) => return refusal(&why),
                        }
                    }
                }
            }
            Operation::Issues => match world.listed(query) {
                Ok(issues) => json!({"issues": page(issues)}),
                Err(why) => return refusal(&why),
            },
            Operation::Projects => json!({"projects": {
                "nodes": if world.project_held { vec![world.project()] } else { Vec::new() },
                "pageInfo": {"hasNextPage": false, "endCursor": null},
            }}),
            Operation::Project | Operation::ProjectRelations if !world.holds(id) => {
                return not_found(graphql::PROJECT_NOT_FOUND_MESSAGE);
            }
            Operation::Project => json!({"project": world.project()}),
            Operation::ProjectRelations => json!({"project": {
                "content": world.project_content,
                "relations": empty_page(),
                "inverseRelations": empty_page(),
            }}),
            Operation::IssueUpdate => {
                let Some(input) = variables["input"]
                    .as_object()
                    .filter(|input| !input.is_empty())
                else {
                    return refusal("an issueUpdate carries a non-empty `input` object");
                };
                match world.update(id, input) {
                    // A rewrite reads the issue's relations back in its own answer.
                    Ok(issue) => match world.related(issue, query) {
                        Ok(issue) => json!({"issueUpdate": {"success": true, "issue": issue}}),
                        Err(why) => return refusal(&why),
                    },
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
                    return not_found(graphql::PROJECT_NOT_FOUND_MESSAGE);
                }
                world.project_status = status.name.clone();
                json!({"projectUpdate": {"success": true, "project": {
                    "id": world.project_id, "status": {"name": status.name, "type": status.kind},
                }}})
            }
            Operation::ProjectUpdate => {
                let Some(content) = variables["input"]["content"].as_str() else {
                    return refusal("a projectUpdate carries the project's `content`");
                };
                if !world.holds(id) {
                    return not_found(graphql::PROJECT_NOT_FOUND_MESSAGE);
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
                world.project_content = stored_as(world.links_stored, content);
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
                if let Some(conflict) = world.conflict(MetadataRecord::Task, input) {
                    return conflict;
                }
                match world.create_issue(input) {
                    Ok(issue) => json!({"issueCreate": {"success": true, "issue": issue}}),
                    Err(why) => return refusal(&why),
                }
            }
            Operation::ProjectCreate => {
                let Some(input) = variables["input"].as_object() else {
                    return refusal("a projectCreate carries an `input` object");
                };
                if let Some(conflict) = world.conflict(MetadataRecord::Project, input) {
                    return conflict;
                }
                match world.create_project(input) {
                    Ok(project) => json!({"projectCreate": {"success": true, "project": project}}),
                    Err(why) => return refusal(&why),
                }
            }
            Operation::Document => {
                let Some(held) = world.documents.iter().find(|held| held.id == id) else {
                    return not_found(graphql::DOCUMENT_NOT_FOUND_MESSAGE);
                };
                json!({"document": world.render_document(held)})
            }
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
                if target.is_none() {
                    if let Some(conflict) = world.conflict(MetadataRecord::Document, input) {
                        return conflict;
                    }
                }
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
            Operation::IssueRelationCreate => {
                let Some(input) = variables["input"].as_object() else {
                    return refusal("an issueRelationCreate carries an `input` object");
                };
                match world.relate(input) {
                    Ok(relation) => json!({"issueRelationCreate": {
                        "success": true, "issueRelation": {"id": relation},
                    }}),
                    Err(why) => return refusal(&why),
                }
            }
            Operation::IssueRelationDelete => {
                let before = world.relations.len();
                world.relations.retain(|held| held.id != id);
                if world.relations.len() == before {
                    return refusal("Entity not found: IssueRelation");
                }
                json!({"issueRelationDelete": {"success": true}})
            }
            Operation::ProjectDelete => {
                if !world.holds(id) {
                    return not_found(graphql::PROJECT_NOT_FOUND_MESSAGE);
                }
                world.project_held = false;
                world.burnt.push(id.to_owned());
                json!({"projectDelete": {"success": true}})
            }
            Operation::IssueDelete => {
                let before = world.issues.len();
                world.issues.retain(|held| held.id != id);
                if world.issues.len() == before {
                    return not_found(graphql::ISSUE_NOT_FOUND_MESSAGE);
                }
                world
                    .relations
                    .retain(|held| held.issue != id && held.related != id);
                world.burnt.push(id.to_owned());
                json!({"issueDelete": {"success": true}})
            }
            Operation::DocumentDelete => {
                let before = world.documents.len();
                world.documents.retain(|held| held.id != id);
                if world.documents.len() == before {
                    return not_found(graphql::DOCUMENT_NOT_FOUND_MESSAGE);
                }
                world.burnt.push(id.to_owned());
                json!({"documentDelete": {"success": true}})
            }
            Operation::Unknown => return refusal(&format!("the loopback answers no {query}")),
        };
        json!({"data": data})
    }
}

impl World {
    /// Whether `id` names the project, once there is one.
    fn holds(&self, id: &str) -> bool {
        self.project_held && id == self.project_id
    }

    /// File the one project this endpoint holds, its content stored as Linear stores it.
    fn create_project(&mut self, input: &Map<String, Value>) -> Result<Value, String> {
        if self.project_held {
            return Err(format!(
                "the loopback holds one project, and {} is filed",
                self.project_id
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
        self.project_content = stored_as(self.links_stored, content);
        // Linear files a create under the `id` it names, where it names one.
        if let Some(id) = input.get("id").and_then(Value::as_str) {
            id.clone_into(&mut self.project_id);
        }
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
            // Linear files a create under the `id` it names, where it names one.
            id: text("id").map_or_else(|| format!("issue-{n}"), str::to_owned),
            identifier: format!("{TEAM}-{n}"),
            title: title.to_owned(),
            description: stored_as(self.links_stored, description),
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
                // Linear files a create under the `id` it names, where it names one.
                let id = text("id").map_or_else(
                    || format!("document-{}", self.documents.len() + 1),
                    str::to_owned,
                );
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
            document.content = stored_as(self.links_stored, content);
        }
        let document = document.clone();
        Ok(self.render_document(&document))
    }

    fn render_document(&self, document: &Document) -> Value {
        let mut rendered = json!({
            "id": document.id,
            "title": document.title,
            "content": document.content,
            "url": format!("https://linear.app/generic/document/{}", document.id),
            "createdAt": "2026-10-01T00:00:00.000Z",
            "updatedAt": "2026-10-01T00:00:00.000Z",
            "archivedAt": null,
            "project": {"id": self.project_id},
        });
        self.with_editor_state(
            &mut rendered,
            "contentState",
            &document.id,
            &document.content,
        );
        rendered
    }

    fn render(&self, issue: &Issue) -> Value {
        let state = self
            .states
            .iter()
            .find(|named| named.id == issue.state)
            .map(|named| json!({"name": named.name, "type": named.kind}));
        let mut rendered = json!({
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
            "project": {"id": self.project_id},
        });
        self.with_editor_state(
            &mut rendered,
            "descriptionState",
            &issue.id,
            &issue.description,
        );
        rendered
    }

    /// `rendered` with the `field` holding the editor state of the record `id` stored as `content`,
    /// when this endpoint answers with editor states.
    fn with_editor_state(&self, rendered: &mut Value, field: &str, id: &str, content: &str) {
        if self.links_stored {
            rendered[field] = json!(editor_state(id, content));
        }
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

    fn issue(&self, id: &str) -> Option<Value> {
        self.issues
            .iter()
            .find(|held| held.id == id)
            .map(|held| self.render(held))
    }

    /// Linear's refusal of a `record` create whose `input` names an id a record here already
    /// holds or once held — the project, an issue or a document, deleted or not, since Linear
    /// lets one id name one record ever —
    /// as the linked plugin states Linear answers it: under HTTP 200, with no data, its message
    /// `conflict on insert of <type>` and its presentable sentence naming the id.
    fn conflict(&self, record: MetadataRecord, input: &Map<String, Value>) -> Option<Value> {
        let id = input.get("id").and_then(Value::as_str)?;
        let held = self.holds(id)
            || self.burnt.iter().any(|held| held == id)
            || self.issues.iter().any(|held| held.id == id)
            || self.documents.iter().any(|held| held.id == id);
        held.then(|| {
            let root = match record {
                MetadataRecord::Task => "issueCreate",
                MetadataRecord::Project => "projectCreate",
                MetadataRecord::Document => "documentCreate",
            };
            json!({"data": null, "errors": [{
                "message": graphql::id_conflict_message(record),
                "path": [root],
                "extensions": {
                    "code": "INPUT_ERROR",
                    "statusCode": 400,
                    "type": "invalid input",
                    "userError": true,
                    "userPresentableMessage": graphql::id_conflict_presentable(record, id),
                },
            }]})
        })
    }

    fn project(&self) -> Value {
        let status = self
            .statuses
            .iter()
            .find(|named| named.name == self.project_status)
            .map(|named| json!({"name": named.name, "type": named.kind}));
        let mut rendered = json!({
            "id": self.project_id,
            "name": self.project_name,
            "content": self.project_content,
            "url": format!("https://linear.app/generic/project/{}", self.project_id),
            "createdAt": "2026-10-01T00:00:00.000Z",
            "updatedAt": "2026-10-01T00:00:00.000Z",
            "archivedAt": null,
            "status": status,
            "labels": {"nodes": []},
        });
        self.with_editor_state(
            &mut rendered,
            "contentState",
            &self.project_id,
            &self.project_content,
        );
        rendered
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
            issue.description = stored_as(self.links_stored, &description);
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

    /// File one dependency between two issues this endpoint holds, and answer its id.
    fn relate(&mut self, input: &Map<String, Value>) -> Result<String, String> {
        let text = |field: &str| input.get(field).and_then(Value::as_str);
        let (Some(issue), Some(related), Some(kind)) =
            (text("issueId"), text("relatedIssueId"), text("type"))
        else {
            return Err(
                "an issueRelationCreate carries an `issueId`, a `relatedIssueId` and a `type`"
                    .to_owned(),
            );
        };
        if input.len() != 3 {
            return Err(format!(
                "the loopback relates by three fields, not {input:?}"
            ));
        }
        if !matches!(kind, "blocks" | "related") {
            return Err(format!("invalid relation type: {kind}"));
        }
        for end in [issue, related] {
            if !self.issues.iter().any(|held| held.id == end) {
                return Err(format!("Entity not found: Issue {end}"));
            }
        }
        self.relations_filed += 1;
        let id = format!("relation-{}", self.relations_filed);
        self.relations.push(Relation {
            id: id.clone(),
            issue: issue.to_owned(),
            related: related.to_owned(),
            kind: kind.to_owned(),
        });
        Ok(id)
    }

    /// Every issue, each with the relation connections `query` selects.
    fn listed(&self, query: &str) -> Result<Vec<Value>, String> {
        self.issues
            .iter()
            .map(|held| self.related(self.render(held), query))
            .collect()
    }

    /// A rendered issue with each relation connection `query` selects, every node holding
    /// exactly the fields that document names for it: the plugin's own documents are the one
    /// statement of what a relation answers, and a field this endpoint cannot fill is refused by
    /// name rather than left out.
    fn related(&self, mut issue: Value, query: &str) -> Result<Value, String> {
        let Some(id) = issue.get("id").and_then(Value::as_str).map(str::to_owned) else {
            return Ok(issue);
        };
        for connection in [Connection::Relations, Connection::InverseRelations] {
            let Some(selection) = node_selection(query, connection.field()) else {
                continue;
            };
            let nodes = self
                .relations
                .iter()
                .filter(|held| connection.holds(held, &id))
                .map(|held| relation_node(held, selection))
                .collect::<Result<Vec<_>, _>>()?;
            issue[connection.field()] = page(nodes);
        }
        Ok(issue)
    }
}

/// The two connections an issue's relations are read through.
#[derive(Clone, Copy)]
enum Connection {
    /// What the issue depends on: the relations its own writes filed.
    Relations,
    /// What depends on the issue: the relations naming it across them.
    InverseRelations,
}

impl Connection {
    fn field(self) -> &'static str {
        match self {
            Self::Relations => "relations",
            Self::InverseRelations => "inverseRelations",
        }
    }

    fn holds(self, relation: &Relation, issue: &str) -> bool {
        match self {
            Self::Relations => relation.issue == issue,
            Self::InverseRelations => relation.related == issue,
        }
    }
}

/// What `query` selects of each node of its `field` connection, as written between that
/// connection's `nodes{` and the brace closing it, or `None` where it selects no such connection.
fn node_selection<'q>(query: &'q str, field: &str) -> Option<&'q str> {
    let mut from = 0;
    while let Some(at) = query[from..].find(field) {
        let start = from + at;
        from = start + field.len();
        let named = !query[..start]
            .chars()
            .next_back()
            .is_some_and(|before| before.is_ascii_alphanumeric());
        let rest = &query[from..];
        if !named || !(rest.starts_with('(') || rest.starts_with('{')) {
            continue;
        }
        let body = rest.split_once('{')?.1.trim_start();
        let nodes = body.strip_prefix("nodes")?.trim_start().strip_prefix('{')?;
        return Some(&nodes[..closing(nodes)?]);
    }
    None
}

/// Where the brace closing an already opened one sits in `text`.
fn closing(text: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (at, character) in text.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

/// The fields one selection names, in order, each with what it selects of its own value.
fn fields(selection: &str) -> Result<Vec<(&str, Option<&str>)>, String> {
    let mut named = Vec::new();
    let mut rest = selection.trim_start();
    while !rest.is_empty() {
        let end = rest
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .unwrap_or(rest.len());
        if end == 0 {
            return Err(format!("the loopback reads no selection {selection:?}"));
        }
        let (field, after) = rest.split_at(end);
        let after = after.trim_start();
        match after.strip_prefix('{') {
            Some(inner) => {
                let close = closing(inner)
                    .ok_or_else(|| format!("an unclosed selection in {selection:?}"))?;
                named.push((field, Some(&inner[..close])));
                rest = inner[close + 1..].trim_start();
            }
            None => {
                named.push((field, None));
                rest = after;
            }
        }
    }
    Ok(named)
}

/// One relation as a node of the selection the plugin's document names for it.
fn relation_node(relation: &Relation, selection: &str) -> Result<Value, String> {
    let mut node = Map::new();
    for (field, nested) in fields(selection)? {
        let nested = nested.map(fields).transpose()?;
        let value = match (field, nested.as_deref()) {
            ("id", None) => json!(relation.id),
            ("type", None) => json!(relation.kind),
            ("relatedIssue", Some([("id", None)])) => json!({"id": relation.related}),
            ("issue", Some([("id", None)])) => json!({"id": relation.issue}),
            _ => {
                return Err(format!(
                    "the loopback answers no relation field {field} of {selection:?}"
                ))
            }
        };
        node.insert(field.to_owned(), value);
    }
    Ok(Value::Object(node))
}

fn page(nodes: Vec<Value>) -> Value {
    json!({"nodes": nodes, "pageInfo": {"hasNextPage": false, "endCursor": null}})
}

fn empty_page() -> Value {
    page(Vec::new())
}

/// Linear's errored answer to a read or a write of an id it does not hold, `message` the
/// plugin's own `*_NOT_FOUND_MESSAGE` for the kind it addressed.
fn not_found(message: &str) -> Value {
    json!({"errors": [{"message": message}]})
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

/// [`linear_stores`], or [`linear_stores_links`] where `links` are held as Linear holds them.
fn stored_as(links: bool, content: &str) -> String {
    if links {
        linear_stores_links(content)
    } else {
        linear_stores(content)
    }
}

/// What Linear stores of `content` when its links are modelled too: [`linear_stores`], and what
/// writes to `TES2` were observed storing on 2026-11-07, which onetaskgraph's own model of Linear
/// holds as well: bare addresses autolinked, every other link's destination in angle brackets, and
/// every other bracket and tilde escaped, which is why Linear keeps no reference-style link.
pub fn linear_stores_links(content: &str) -> String {
    let stored = linear_stores(content);
    inline_lines(stored.split('\n').map(str::to_owned).collect()).join("\n")
}

/// `lines` with the text of each [`inline_text`] rewrites, which is neither inside a fence, nor a
/// code block's indented line, nor an HTML line such as the metadata slot.
fn inline_lines(lines: Vec<String>) -> Vec<String> {
    let mut fence: Option<(char, usize)> = None;
    let mut after_blank = true;
    lines
        .into_iter()
        .map(|line| {
            if let Some((mark, opened)) = fence {
                if fence_of(&line).is_some_and(|(closing, run, rest)| {
                    closing == mark && run >= opened && rest.trim().is_empty()
                }) {
                    fence = None;
                }
                return line;
            }
            if let Some((mark, run, _)) = fence_of(&line) {
                fence = Some((mark, run));
                return line;
            }
            let code = after_blank && line.starts_with("    ");
            after_blank = code || line.trim().is_empty();
            if code || line.trim_start().starts_with('<') {
                line
            } else {
                inline_text(&line)
            }
        })
        .collect()
}

// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] As with the rest of this model, Linear publishes no statement of what it stores and this reading of its issue address is the model's own on purpose: reusing the store's `section_links` would let the two never disagree, which is the disagreement the journeys through this endpoint exist to find.
/// One line of Markdown's text as Linear stores it. A code span, an image and an escape stay as they
/// are written, and a list item's task box stays one. A link's destination goes in angle brackets,
/// but for a link to a Linear issue under its identifier. A bare URL, or a `.md` file with a fragment
/// read as a domain, is linked under itself, a Linear issue's URL under the identifier. Any other
/// bracket or tilde is escaped. Storing what this stored changes nothing.
fn inline_text(line: &str) -> String {
    let mut stored = String::with_capacity(line.len());
    let box_at = line.len() - line.trim_start_matches([' ', '-', '*']).len();
    let mut at = 0;
    while let Some(character) = line[at..].chars().next() {
        let rest = &line[at..];
        let at_word_start = !line[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || "@/.-_:\\#=+%".contains(c));
        let taken = if character == '`' {
            let run = rest.len() - rest.trim_start_matches('`').len();
            let close = rest[run..]
                .find(&"`".repeat(run))
                .map(|end| run + end + run);
            let span = close.unwrap_or(run);
            stored.push_str(&rest[..span]);
            span
        } else if character == '\\' {
            let escaped = 1 + rest[1..].chars().next().map_or(0, char::len_utf8);
            stored.push_str(&rest[..escaped]);
            escaped
        } else if character == '!' && link_length(&rest[1..]).is_some() {
            let image = 1 + link_length(&rest[1..]).expect("checked just now");
            stored.push_str(&rest[..image]);
            image
        } else if at == box_at
            && at > 0
            && ["[ ] ", "[x] ", "[X] "].iter().any(|b| rest.starts_with(b))
        {
            stored.push_str(&rest[..3]);
            3
        } else if character == '[' {
            match link_length(rest) {
                Some(length) => {
                    stored.push_str(&stored_link(&rest[..length]));
                    length
                }
                None => {
                    stored.push_str("\\[");
                    1
                }
            }
        } else if matches!(character, ']' | '~') {
            stored.push('\\');
            stored.push(character);
            1
        } else if let Some(token) = at_word_start.then(|| autolinked_token(rest)).flatten() {
            match issue_identifier(token) {
                Some(identifier) => stored.push_str(&format!("[{identifier}]({token})")),
                None if token.starts_with("http") => {
                    stored.push_str(&format!("[{token}](<{token}>)"));
                }
                None => stored.push_str(&format!("[{token}](<http://{token}>)")),
            }
            token.len()
        } else {
            stored.push(character);
            character.len_utf8()
        };
        at += taken;
    }
    stored
}

/// The length of the inline link `text` opens with: `[text](target)`, its target bare or in angle
/// brackets. A link's text may hold balanced brackets, as a record's title does.
fn link_length(text: &str) -> Option<usize> {
    let mut depth = 0_usize;
    let close = text.find(|c: char| {
        depth = match c {
            '[' => depth + 1,
            ']' => depth.saturating_sub(1),
            _ => depth,
        };
        depth == 0
    })?;
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

/// The inline link `link` as Linear stores it.
fn stored_link(link: &str) -> String {
    let (text, target) = link
        .split_once("](")
        .expect("a link's text closes before its destination");
    let target = target.strip_suffix(')').expect("a link closes");
    if target.starts_with('<') || issue_identifier(target) == Some(&text[1..]) {
        return link.to_owned();
    }
    format!("{text}](<{target}>)")
}

/// The token at the start of `text` Linear autolinks: a URL, or a file name under `.md` with a
/// fragment. Punctuation trailing it stays outside it.
fn autolinked_token(text: &str) -> Option<&str> {
    let end = text
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '`' | '[' | ']' | '|'))
        .unwrap_or(text.len());
    let token =
        text[..end].trim_end_matches(['.', ',', ':', ';', '!', '?', '\'', '"', ')', '*', '_', '~']);
    let scheme = ["http://", "https://"]
        .iter()
        .any(|scheme| token.len() > scheme.len() && token.starts_with(scheme));
    let file = token.split_once(".md#").is_some_and(|(name, fragment)| {
        !name.is_empty()
            && !fragment.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
    });
    (scheme || file).then_some(token)
}

/// The identifier a Linear issue's address names: `TES2-1` of
/// `https://linear.app/<workspace>/issue/TES2-1#heading-a-12345678`.
fn issue_identifier(address: &str) -> Option<&str> {
    let (_, issue) = address
        .strip_prefix("https://linear.app/")?
        .split_once("/issue/")?;
    issue.split(['/', '#']).next()
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

/// Every rewrite [`linear_stores_links`] adds, by what it rewrites: what was written and what Linear
/// stored.
pub const OBSERVED_LINKS: &[(&str, &str, &str)] = &[
    (
        "reference-link",
        "| [core][1] |\n\n[1]: ../tasks/core.md#budget",
        "| \\[core\\]\\[1\\] |\n\n\\[1\\]: ../tasks/core.md#budget",
    ),
    (
        "link-destination",
        "see [core](https://a.example/b) ~ then",
        "see [core](<https://a.example/b>) \\~ then",
    ),
    (
        "bare-url",
        "| https://a.example/b |",
        "| [https://a.example/b](<https://a.example/b>) |\n",
    ),
    (
        "bare-issue-url",
        "see https://linear.app/w/issue/TES2-1#heading-a-12345678.",
        "see [TES2-1](https://linear.app/w/issue/TES2-1#heading-a-12345678).",
    ),
];
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// Every phrase `filter` asks an issue's description to hold, compared as `contains` compares it.
fn contains_phrases(filter: &Value, phrases: &mut Vec<String>) {
    match filter {
        Value::Object(fields) => {
            for (field, value) in fields {
                match (field.as_str(), value.as_str()) {
                    ("contains", Some(phrase)) => phrases.push(phrase.to_owned()),
                    _ => contains_phrases(value, phrases),
                }
            }
        }
        Value::Array(members) => {
            for member in members {
                contains_phrases(member, phrases);
            }
        }
        _ => {}
    }
}

/// The editor state Linear keeps beside the record `id` stored as `content`: base64 of a Yjs update
/// whose `prosemirror` fragment holds a `heading` node per ATX heading outside a fence. Each id is
/// made from the record, the heading's text and its position among those of that text, so an update
/// leaving a heading's text alone keeps its id, as Linear's does.
fn editor_state(id: &str, content: &str) -> String {
    let doc = Doc::new();
    let fragment = doc.get_or_insert_xml_fragment("prosemirror");
    let mut seen: Vec<&str> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut txn = doc.transact_mut();
    for line in content.lines() {
        if let Some((mark, run, rest)) = fence_of(line) {
            fence = match fence {
                None => Some((mark, run)),
                Some((open, opened)) if open == mark && run >= opened && rest.trim().is_empty() => {
                    None
                }
                held => held,
            };
            continue;
        }
        let marks = line.len() - line.trim_start_matches('#').len();
        let Some(text) = line[marks..]
            .strip_prefix(' ')
            .filter(|_| fence.is_none() && (1..=6).contains(&marks))
        else {
            continue;
        };
        let text = text.trim();
        let ordinal = seen.iter().filter(|earlier| **earlier == text).count();
        seen.push(text);
        let index = u32::try_from(seen.len() - 1).expect("a record's headings fit");
        let heading = fragment.insert(&mut txn, index, XmlElementPrelim::empty("heading"));
        heading.insert_attribute(
            &mut txn,
            "level",
            f64::from(u32::try_from(marks).expect("at most six")),
        );
        heading.insert_attribute(
            &mut txn,
            "id",
            heading_id(&format!("{id}/{text}/{ordinal}")),
        );
        heading.insert(&mut txn, 0, XmlTextPrelim::new(text));
    }
    drop(txn);
    let update = doc
        .transact()
        .encode_state_as_update_v1(&yrs::StateVector::default());
    standard_base64(&update)
}

/// Standard, padded base64 (RFC 4648 §4), the alphabet Linear's `contentState` is written in.
/// Written here rather than linked: a direct dependency named between `async-trait` and `clap`
/// lands inside the context `.github/ci-shape/nx-inputs.test.mjs` reverse-applies the recorded
/// release's `Cargo.lock` edit against, and refuses that patch.
fn standard_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = chunk.iter().enumerate().fold(0u32, |acc, (i, byte)| {
            acc | u32::from(*byte) << (16 - 8 * i)
        });
        for position in 0..4 {
            out.push(if position <= chunk.len() {
                char::from(ALPHABET[(triple >> (18 - 6 * position) & 0x3f) as usize])
            } else {
                '='
            });
        }
    }
    out
}

/// A heading id shaped as Linear's UUIDs are, made from `seed` so a run reproduces.
fn heading_id(seed: &str) -> String {
    let hash = seed.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    let hex = format!("{hash:016x}");
    format!("{}-{}-4000-a000-{}0000", &hex[..8], &hex[8..12], &hex[..8])
}

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

/// The editor state this endpoint keeps decodes through the linked plugin into each heading
/// outside a fence, and a later edit leaving a heading's text alone keeps that heading's id.
#[test]
fn the_editor_state_reads_back_through_the_linked_plugin_with_ids_an_edit_keeps() {
    let content = "## Budget a\n```\n## not a heading\n```\n## Budget a\n\nProse.\n# Other";
    let read = |content: &str| -> Vec<(String, String)> {
        onetaskgraph_linear::headings::decoded(&editor_state("issue-1", content))
            .expect("the plugin decodes the state")
            .into_iter()
            .map(|heading| (heading.text, heading.id))
            .collect()
    };
    let held = read(content);
    let texts: Vec<&str> = held.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(texts, ["Budget a", "Budget a", "Other"]);
    assert_ne!(held[0].1, held[1].1, "two headings of one text: {held:?}");
    assert_eq!(read("## Budget a\n\nAnother body.")[0], held[0]);
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
    for (probe, written, kept) in OBSERVED_LINKS {
        assert_eq!(linear_stores_links(written), *kept, "{probe}: {written:?}");
        assert_eq!(
            linear_stores_links(kept),
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
        assert_eq!(linear_stores_links(kept), kept, "{kept:?}, links modelled");
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

/// The Linear plugin the store links files, reads and replaces a dependency through this endpoint
/// by its own public `TaskSource`, and its GraphQL documents are what fix the relation shapes
/// answered here: each relation document it sends is one this endpoint names, every relation node
/// those documents select is one it fills, and a field one selects that it cannot fill is refused
/// by name. A plugin release that writes or reads relations differently fails here.
#[test]
fn the_linked_linear_plugin_files_reads_and_replaces_a_dependency_through_this_endpoint() {
    use onetaskgraph_linear::graphql;
    use onetaskgraph_plugin_api::{
        DependencyEdge, DependencyEndpoint, DependencyKind, Direction, ItemKind, ItemWrite,
        PageRequest, SourceName, SourcePlugin, Task,
    };

    // The plugin's own documents: each lands on the operation this endpoint answers it as, and
    // each relation node it or a cached page selects is one this endpoint fills.
    for (document, operation) in [
        (
            graphql::ISSUE_RELATION_CREATE,
            Operation::IssueRelationCreate,
        ),
        (
            graphql::ISSUE_RELATION_DELETE,
            Operation::IssueRelationDelete,
        ),
        (graphql::ISSUE_RELATIONS, Operation::IssueRelations),
        (graphql::ISSUE_REWRITE, Operation::IssueUpdate),
    ] {
        assert_eq!(Operation::of(document), operation, "{document}");
    }
    let sample = Relation {
        id: "relation".to_owned(),
        issue: "near".to_owned(),
        related: "far".to_owned(),
        kind: "blocks".to_owned(),
    };
    for (document, connection) in [
        (graphql::ISSUE_RELATIONS, Connection::Relations),
        (graphql::ISSUE_RELATIONS, Connection::InverseRelations),
        (graphql::ISSUE_REWRITE, Connection::Relations),
        (graphql::RELATIONS_PAGE, Connection::Relations),
        (
            graphql::INVERSE_RELATIONS_PAGE,
            Connection::InverseRelations,
        ),
    ] {
        let selection = node_selection(document, connection.field())
            .unwrap_or_else(|| panic!("{document} selects no {} nodes", connection.field()));
        relation_node(&sample, selection).unwrap_or_else(|why| panic!("{why}"));
    }

    let linear = Linear::serve(
        &[("Todo", "unstarted")],
        &[("Planned", "planned")],
        "generic",
        "",
        "Planned",
    );
    let [near, first, second] = ["near", "first", "second"]
        .map(|title| linear.file(title, &format!("Do {title}."), "Todo"));
    let key = "ONEPIPELINE_LOOPBACK_LINEAR_KEY";
    let secrets = onetaskgraph_core::Secrets::load(onetaskgraph_core::Environment::from_pairs(
        vec![(key.to_owned(), "a-loopback-key".to_owned())],
    ))
    .expect("the loopback key resolves");
    let source = onetaskgraph_linear::Plugin
        .build(
            &SourceName::new("tracker").expect("a source name"),
            &json!({
                "api_key_env": key,
                "team": TEAM,
                "endpoint": linear.endpoint(),
                "status_mapping": {"todo": "Todo"},
            }),
            &secrets,
        )
        .unwrap_or_else(|error| panic!("the plugin builds over this endpoint: {error}"));
    let task: Task = serde_json::from_value(json!({
        "id": near, "title": "near", "content": "Do near.",
        "status": {"category": "todo", "name": "Todo"},
        "labels": [], "project": PROJECT, "repositories": [], "metadata": {},
    }))
    .expect("a task");
    let depending_on = |far: &str| ItemWrite {
        target: Some(near.clone().into()),
        item: task.clone(),
        depends_on: vec![DependencyEdge {
            from: DependencyEndpoint::from_native(near.clone().into(), ItemKind::Task),
            to: DependencyEndpoint::from_native(far.to_owned().into(), ItemKind::Task),
            kind: DependencyKind::Blocks,
        }],
    };
    let runtime = || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime for one plugin call")
    };
    let edges = |issue: &str, direction: Direction| -> Vec<(String, String)> {
        let page = runtime().block_on(source.task_dependencies(
            &issue.to_owned().into(),
            direction,
            &PageRequest {
                cursor: None,
                limit: 10,
            },
        ));
        page.unwrap_or_else(|error| panic!("the plugin reads {issue}'s dependencies: {error}"))
            .items
            .into_iter()
            .map(|edge| (edge.from.id().to_owned(), edge.to.id().to_owned()))
            .collect()
    };

    runtime()
        .block_on(source.write_task(&depending_on(&first)))
        .unwrap_or_else(|error| panic!("the plugin files the dependency: {error}"));
    assert_eq!(
        linear.relations(),
        [(near.clone(), first.clone(), "blocks".to_owned())]
    );
    let filed = vec![(near.clone(), first.clone())];
    assert_eq!(edges(&near, Direction::DependsOn), filed);
    assert_eq!(edges(&first, Direction::DependedOnBy), filed);

    // Replaced: the rewrite's own answer names the held relation, which is taken back, and the
    // new one filed in its place.
    runtime()
        .block_on(source.write_task(&depending_on(&second)))
        .unwrap_or_else(|error| panic!("the plugin replaces the dependency: {error}"));
    assert_eq!(
        linear.relations(),
        [(near.clone(), second.clone(), "blocks".to_owned())]
    );
    assert_eq!(
        edges(&near, Direction::DependsOn),
        [(near.clone(), second.clone())]
    );
    assert!(edges(&first, Direction::DependedOnBy).is_empty());
    let sent: Vec<Operation> = linear
        .log()
        .iter()
        .map(|request| request.operation)
        .collect();
    for operation in [
        Operation::IssueRelationCreate,
        Operation::IssueRelationDelete,
    ] {
        assert!(sent.contains(&operation), "{sent:?}");
    }
    assert!(!sent.contains(&Operation::Unknown), "{sent:?}");

    // A relation field no document of the plugin selects is refused by name, not left out.
    let mut stream =
        TcpStream::connect(("127.0.0.1", linear.port)).expect("connect to the loopback");
    let body = json!({
        "query": "query($id:String!){ issue(id:$id){ id relations(first:10){nodes{id createdAt} pageInfo{hasNextPage}} } }",
        "variables": {"id": near},
    })
    .to_string();
    write!(
        stream,
        "POST /graphql HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .expect("the request is sent");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("the request is whole");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("bounded response read");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("the endpoint answers and closes");
    let (_, answered) = response
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("an HTTP response: {response:?}"));
    let answered: Value = serde_json::from_str(answered).expect("a JSON answer");
    assert!(
        answered["errors"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("createdAt")),
        "{answered}"
    );
}

/// What this endpoint answers an id it does not hold, and a create under an id a record holds or
/// held, on the wire: Linear's errored not-found answer under HTTP 400 for an issue, a project
/// and a document — never a null — and its `conflict on insert` refusal under HTTP 200, naming
/// the id, for a `projectCreate`, which files nothing, while a create under a fresh id lands.
#[test]
fn an_absent_id_is_not_found_and_a_held_id_refuses_its_create_as_linear_answers() {
    use onetaskgraph_linear::graphql;
    let linear = Linear::serve(
        &[("Todo", "unstarted")],
        &[("Planned", "planned")],
        "generic",
        "",
        "Planned",
    );
    let issue = linear.file("work", "description", "Todo");
    let post = |query: &str, variables: Value| -> (String, Value) {
        let mut stream =
            TcpStream::connect(("127.0.0.1", linear.port)).expect("connect to the loopback");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("bounded response read");
        let body = json!({"query": query, "variables": variables}).to_string();
        write!(
            stream,
            "POST /graphql HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .expect("the request is sent");
        stream
            .shutdown(std::net::Shutdown::Write)
            .expect("the request is finished");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("the loopback answers");
        let (head, body) = response.split_once("\r\n\r\n").expect("an HTTP response");
        let status = head.lines().next().expect("a status line").to_owned();
        (
            status,
            serde_json::from_str(body).expect("a GraphQL response"),
        )
    };
    for (query, held, missing) in [
        (
            graphql::ISSUE,
            Some(issue.as_str()),
            graphql::ISSUE_NOT_FOUND_MESSAGE,
        ),
        (
            graphql::PROJECT,
            Some(PROJECT),
            graphql::PROJECT_NOT_FOUND_MESSAGE,
        ),
        (graphql::DOCUMENT, None, graphql::DOCUMENT_NOT_FOUND_MESSAGE),
    ] {
        let (status, answer) = post(query, json!({"id": "id-nothing-holds"}));
        assert_eq!(status, "HTTP/1.1 400 Bad Request", "{answer}");
        assert_eq!(
            answer,
            json!({"errors": [{"message": missing}]}),
            "an absent id was not answered {missing:?}"
        );
        // An id this endpoint holds is still answered with its item.
        if let Some(held) = held {
            let (status, answer) = post(query, json!({"id": held}));
            assert_eq!(status, "HTTP/1.1 200 OK", "{answer}");
            assert!(answer.get("errors").is_none(), "{answer}");
        }
    }

    // A project create under the id the held project holds is refused as Linear refuses it, and
    // files nothing; under an issue's id, likewise.
    for taken in [PROJECT, issue.as_str()] {
        let before = linear.project_id();
        let (status, answer) = post(
            graphql::PROJECT_CREATE,
            json!({"input": {"id": taken, "name": "again", "content": "", "statusId": "status-0"}}),
        );
        assert_eq!(status, "HTTP/1.1 200 OK", "{answer}");
        assert_eq!(answer["data"], Value::Null, "{answer}");
        assert_eq!(
            answer["errors"][0]["message"],
            graphql::PROJECT_ID_CONFLICT_MESSAGE,
            "{answer}"
        );
        assert_eq!(
            answer["errors"][0]["extensions"]["userPresentableMessage"],
            graphql::id_conflict_presentable(
                onetaskgraph_plugin_api::MetadataRecord::Project,
                taken
            ),
            "{answer}"
        );
        assert_eq!(linear.project_id(), before);
    }

    // Deleted, the project reads as absent, and its id is still one no create may take.
    let (status, answer) = post(graphql::PROJECT_DELETE, json!({"id": PROJECT}));
    assert_eq!(status, "HTTP/1.1 200 OK", "{answer}");
    assert_eq!(answer["data"]["projectDelete"]["success"], true, "{answer}");
    let (status, answer) = post(graphql::PROJECT, json!({"id": PROJECT}));
    assert_eq!(status, "HTTP/1.1 400 Bad Request", "{answer}");
    assert_eq!(
        answer["errors"][0]["message"],
        graphql::PROJECT_NOT_FOUND_MESSAGE
    );
    let create = |id: &str| {
        post(
            graphql::PROJECT_CREATE,
            json!({"input": {"id": id, "name": "again", "content": "", "statusId": "status-0"}}),
        )
    };
    let (status, answer) = create(PROJECT);
    assert_eq!(status, "HTTP/1.1 200 OK", "{answer}");
    assert_eq!(
        answer["errors"][0]["message"],
        graphql::PROJECT_ID_CONFLICT_MESSAGE,
        "a deleted project's id was taken again: {answer}"
    );
    // Under an id nothing ever held, the create is filed.
    let (status, answer) = create("project-fresh");
    assert_eq!(status, "HTTP/1.1 200 OK", "{answer}");
    assert_eq!(
        answer["data"]["projectCreate"]["project"]["id"], "project-fresh",
        "{answer}"
    );
    assert_eq!(linear.project_id(), "project-fresh");
}

/// The editor state's base64 is the standard padded alphabet: RFC 4648 §10's vectors, plus the
/// two characters only the standard alphabet spells.
#[test]
fn editor_state_base64_matches_the_rfc_vectors() {
    for (input, encoded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(standard_base64(input.as_bytes()), encoded, "{input:?}");
    }
    assert_eq!(standard_base64(&[0xfb, 0xff]), "+/8=");
}
