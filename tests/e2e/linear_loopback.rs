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
    /// `project(id:)`: one project.
    Project,
    /// `project(id:){relations inverseRelations}`: one project's dependency edges.
    ProjectRelations,
    /// `issueUpdate`: one issue written.
    IssueUpdate,
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
        } else if query.starts_with("mutation") {
            Self::Unknown
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

#[derive(Default)]
struct World {
    states: Vec<Named>,
    statuses: Vec<Named>,
    project_name: String,
    project_content: String,
    project_status: String,
    issues: Vec<Issue>,
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
            project_content: description.to_owned(),
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
            description: description.to_owned(),
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
            | Operation::IssueUpdate => variables["id"].as_str().expect("validated request ID"),
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
            Operation::Project => json!({"project": (id == PROJECT).then(|| world.project())}),
            Operation::ProjectRelations => json!({"project": (id == PROJECT).then(|| json!({
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
            Operation::Unknown => return refusal(&format!("the loopback answers no {query}")),
        };
        json!({"data": data})
    }
}

impl World {
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
            issue.description = description;
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
