//! The pure half of the HTTP-driven turn: response framing and each
//! mechanism's protocol.
//!
//! Two control mechanisms reach their turn only over HTTP against a sidecar
//! server (`opencode serve`, `crush server`), and interrupting a CLI-driven run
//! was live-REFUTED for both — so oneharness submits the turn to that server
//! itself. Everything decidable without a socket lives here: the response
//! framing a reader must get right, the exact route and body of each request,
//! and what one line of an event stream means. The socket work is
//! the crate-private `io::http` module.
//!
//! Three facts here cost real time to find and are what the unit tests pin:
//!
//! * Both servers answer `Transfer-Encoding: chunked`, so a reader that hands
//!   the raw framing to a JSON parser reports a decode error where the harness
//!   in fact answered correctly.
//! * Both block on a permission decision the client must answer — crush emits a
//!   `permission_request` event and waits, opencode a `permission.*` event —
//!   exactly like ACP's `session/request_permission`. Without an answer the
//!   turn never does any work and every downstream assertion is vacuous.
//! * Crush's `client_id` is a self-assigned UUID that travels in the **body**
//!   when creating a workspace but as a **query parameter** on every other
//!   route; a mismatch answers a bare `{"message":"invalid client_id"}`.

use serde_json::{json, Value};

use crate::domain::control::{AbsolutePath, ControlShape};
use crate::domain::harness::ModeSpec;

/// The HTTP methods these control protocols use. A closed set rather than a
/// string: a typo'd verb is a route that silently does not exist, which reads
/// as a harness that ignored the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    /// The token as it goes on the request line.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// One HTTP request to a control server: everything but the socket.
///
/// Constructible only by this module's route functions, so a path is always
/// assembled from a constant plus already-validated ids — there is no way for a
/// caller to put a space or a CRLF on the request line, which would split it
/// into a second request the server would answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    method: Method,
    path: String,
    body: Option<String>,
}

impl HttpRequest {
    fn new(method: Method, path: String, body: Option<Value>) -> Self {
        debug_assert!(
            is_request_target(&path),
            "a route function built an unusable request path: {path}"
        );
        HttpRequest {
            method,
            path,
            body: body.map(|value| value.to_string()),
        }
    }

    /// The method to put on the request line.
    #[must_use]
    pub fn method(&self) -> Method {
        self.method
    }

    /// The origin-form target to put on the request line.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The JSON body, already serialized. `None` sends no body at all, which is
    /// not the same as `{}` for a route that validates its shape.
    #[must_use]
    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }

    /// A request built directly, so the socket layer can be exercised against a
    /// server double. Test-only: in a real run the route functions above are
    /// the only way a request exists, which is what keeps a raw path off the
    /// request line.
    #[cfg(test)]
    #[must_use]
    pub fn for_test(method: Method, path: &str, body: Option<&str>) -> Self {
        HttpRequest {
            method,
            path: path.to_string(),
            body: body.map(str::to_string),
        }
    }
}

/// Whether `path` is an origin-form target that cannot corrupt a request line.
fn is_request_target(path: &str) -> bool {
    path.starts_with('/')
        && !path.is_empty()
        && path
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && c.is_ascii())
}

/// What one line of a server's event stream tells the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEvent {
    /// The server is blocked on a permission decision and will not proceed
    /// until this exact request is answered.
    PermissionRequest(PermissionAsk),
    /// The server admitted the prompt: the turn is now genuinely in flight.
    ///
    /// Load-bearing for opencode, which announces idleness whenever the session
    /// is idle — including in the moment between the stream opening and the
    /// prompt being admitted. Without this the first idle ends the run in under
    /// a second, before the agent has done anything.
    Started,
    /// Assistant text to accumulate.
    Text(String),
    /// The turn ended, however it ended (completed, failed, or cancelled).
    Finished,
    /// A line that means nothing to the driver.
    Ignored,
}

/// A permission decision the server is waiting on, addressed the way that
/// server addresses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionAsk {
    /// The request's own id, as it appears in the answer's route or body.
    /// Validated on recognition, because answering is a request whose PATH it
    /// becomes: an ask carrying an unusable id is ignored rather than turned
    /// into a request at some other route.
    pub id: ResourceId,
    /// The session the request belongs to, when the event names one. A server
    /// whose event stream is shared across dispatches (opencode's is) can ask
    /// about a session this turn does not own, and that ask is not this run's
    /// to answer.
    pub session: Option<ResourceId>,
    /// The whole request payload, for a server (crush) that wants it echoed
    /// back rather than referenced by id.
    pub payload: Value,
}

/// The subset of [`ControlShape`] whose turn is submitted over HTTP.
///
/// A separate type rather than a `ControlShape` plus a runtime check: every
/// function here needs a route table, and the shapes that have none (the stdio
/// protocols, Claude's own stdin) would otherwise have to be handled — and
/// mishandled — at each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpShape {
    /// `opencode serve` on loopback TCP.
    Opencode,
    /// `crush server` on a unix socket.
    Crush,
}

impl HttpShape {
    /// The HTTP shape `shape` names, or `None` when its turn is not submitted
    /// over HTTP.
    #[must_use]
    pub fn of(shape: ControlShape) -> Option<Self> {
        match shape {
            ControlShape::OpencodeHttp => Some(HttpShape::Opencode),
            ControlShape::CrushHttp => Some(HttpShape::Crush),
            _ => None,
        }
    }

    /// The mechanism this shape is declared as, for the report's `mechanism`.
    #[must_use]
    pub fn shape(self) -> ControlShape {
        match self {
            HttpShape::Opencode => ControlShape::OpencodeHttp,
            HttpShape::Crush => ControlShape::CrushHttp,
        }
    }

    /// Whether an aborted turn ends **silently** on this server's event stream,
    /// so a served interrupt is the only signal that it is over.
    ///
    /// Measured live, not guessed. Opencode emits nothing after an abort: the
    /// stream stops mid-turn with no `session.idle`, and a run waiting for one
    /// waits out its entire timeout — so a redirection held for an end-of-turn
    /// event would never be delivered. Its interrupt route is synchronous, and
    /// that 2xx is the ending. Crush instead emits `run_complete` however the
    /// run ended, cancellation included, so its redirection rides that and is
    /// never submitted into a turn still winding down.
    #[must_use]
    pub fn abort_ends_turn_silently(self) -> bool {
        matches!(self, HttpShape::Opencode)
    }
}

/// A harness-supplied id, checked before it becomes part of a request path.
///
/// The ids come from another program's JSON, so they are external input: one
/// carrying `/`, `?` or `..` would silently retarget a later request at a route
/// oneharness never meant to call — including the interrupt, whose whole job is
/// to reach one specific turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceId(String);

impl ResourceId {
    /// Accept `raw` as an id usable in a path, or say why it cannot be one.
    pub fn new(raw: &str) -> Result<Self, String> {
        if raw.is_empty() || raw.len() > 128 {
            return Err(format!(
                "`{raw}` is not a usable id (empty or over 128 characters)"
            ));
        }
        if !raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!(
                "`{raw}` carries characters no id may put in a path"
            ));
        }
        // All dots (`.`, `..`) clears the character check yet still resolves as
        // a traversal — the retargeting this type exists to refuse.
        if raw.chars().all(|c| c == '.') {
            return Err(format!("`{raw}` traverses rather than names a resource"));
        }
        Ok(ResourceId(raw.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A client identity safe to interpolate into a query string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientId(String);

impl ClientId {
    /// Accept `raw` as a client id, or say why it cannot be one. Hyphenated hex
    /// is the shape crush hands out and accepts; anything that could add a
    /// query parameter is refused rather than escaped.
    pub fn new(raw: &str) -> Result<Self, String> {
        if raw.is_empty() || raw.len() > 128 {
            return Err(format!("`{raw}` is not a usable client id"));
        }
        if !raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!(
                "`{raw}` carries characters no query parameter may hold"
            ));
        }
        Ok(ClientId(raw.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The ids one HTTP turn is addressed by, as that server addresses it.
///
/// One variant per protocol rather than one struct of optional coordinates:
/// crush addresses everything through a workspace and names its client on every
/// route, opencode has neither. Independent `Option`s would let a caller build
/// an opencode address carrying crush coordinates, or a crush address missing
/// the ones its routes cannot be written without — and each route function
/// would then have to guess what to do about it. Here every route reads exactly
/// the coordinates its own protocol has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnAddress {
    /// A session on `opencode serve`, which has no further coordinates.
    Opencode { session: ResourceId },
    /// A session under a crush workspace, addressed as one client identity.
    /// That identity is validated because it is interpolated into a query
    /// string: a value carrying `&` or `#` would silently become other
    /// parameters.
    Crush {
        workspace: ResourceId,
        session: ResourceId,
        client: ClientId,
    },
}

/// The coordinates one protocol's *open* request is written from, before there
/// is a [`TurnAddress`] to address anything by.
///
/// One variant per protocol for the reason [`TurnAddress`] has them: crush's
/// workspace cannot be written without the client identity every later route
/// carries, and it declares this run's permission posture at creation, while
/// opencode's session-create has neither. A shape plus an optional client would
/// let a caller ask for a crush workspace with no identity — a request that
/// reaches the wire and answers a bare `{"message":"invalid client_id"}`, which
/// reads as a server that is broken rather than a call that was never valid.
///
/// The working directory is an [`AbsolutePath`] on both, because the server is
/// shared: a relative one would be resolved by the server against wherever it
/// was started, which is another dispatch's project as often as this one's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOpening<'a> {
    /// A session on `opencode serve`, which has no client identity and answers
    /// each permission ask as it arrives rather than declaring a posture. The
    /// model is named here or not at all — see [`OpencodeModel`].
    Opencode {
        cwd: &'a AbsolutePath,
        model: Option<&'a OpencodeModel>,
    },
    /// The workspace every other crush route hangs off, opened as one client
    /// identity and in one permission posture.
    Crush {
        cwd: &'a AbsolutePath,
        client: &'a ClientId,
        decision: PermissionDecision,
    },
}

impl TurnAddress {
    /// The protocol these coordinates belong to. Derived rather than carried
    /// alongside, so a shape and an address can never disagree.
    #[must_use]
    pub fn shape(&self) -> HttpShape {
        match self {
            TurnAddress::Opencode { .. } => HttpShape::Opencode,
            TurnAddress::Crush { .. } => HttpShape::Crush,
        }
    }

    /// The harness's own session id for this turn.
    #[must_use]
    pub fn session(&self) -> &ResourceId {
        match self {
            TurnAddress::Opencode { session } | TurnAddress::Crush { session, .. } => session,
        }
    }
}

/// A crush route: the workspace prefix, `tail`, and the client identity every
/// one of its routes carries as a query parameter (see the module note).
fn crush_path(workspace: &ResourceId, client: &ClientId, tail: &str) -> String {
    format!(
        "/v1/workspaces/{}{tail}?client_id={}",
        workspace.as_str(),
        client.as_str()
    )
}

/// What this run answers when a server asks whether an action may proceed.
///
/// A two-variant type rather than a `bool`, because it is threaded through
/// every route that decides what an agent may do — the workspace's posture, the
/// blanket skip, and each individual reply. At a call site `true`/`false` says
/// nothing about which way round it is, and an inverted one does not fail: it
/// quietly grants a run authority it was not given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Act without asking.
    Allow,
    /// Refuse the action.
    Deny,
}

impl PermissionDecision {
    /// This decision as the boolean a wire field wants. The one place the
    /// mapping is written, so no route has to restate it.
    #[must_use]
    pub fn allows(self) -> bool {
        matches!(self, PermissionDecision::Allow)
    }
}

/// The decision this run's posture implies — the same rule the stdio dialogue
/// applies, so an HTTP turn and a protocol turn answer a permission request the
/// same way.
///
/// Read off the harness's OWN declaration for the mode ([`ModeSpec::posture`])
/// rather than off the normalized spectrum,
/// because the posture a controlled turn must take is the one that mode gives
/// *without* `--control`. Usually the two agree; where the CLI cannot honor the
/// spectrum — `crush run` auto-approves whatever it is asked — the harness's own
/// answer is the one that keeps the two paths under one policy.
#[must_use]
pub fn permits_action(mode: &ModeSpec) -> PermissionDecision {
    if mode.posture.is_unattended() {
        PermissionDecision::Allow
    } else {
        // Deny is the safe default, so a mode added later refuses until someone
        // decides otherwise rather than granting on arrival.
        PermissionDecision::Deny
    }
}

/// The model an opencode turn runs on, split the way its session route names
/// one: a provider and an id, both required.
///
/// A controlled opencode turn is a session on a pooled server, and the model is
/// a property of that SESSION — not of the server, and not of the prompt. It was
/// simply dropped: `--model` reaches `opencode run` on the argv and reached
/// nothing at all under `--control`, so a controlled turn ran on whatever the
/// server picked by itself. That is not a theoretical default — live it was
/// `wandb/nvidia/NVIDIA-Nemotron-3.5-Lightning-30B-A3B` on one host and
/// `ling-3.0-tiny-free` in CI, both answering `Provider request failed with
/// HTTP 401`, which is what made the live control suite's opencode phase a coin
/// flip. Naming it in the server's own config does NOT fix that: `opencode
/// serve` loads `OPENCODE_CONFIG_CONTENT`'s `model` and echoes it back on
/// `/config` while the session it creates still runs on its own choice
/// (measured against opencode 1.18.5).
///
/// The field names are the server's own (`{"providerID": …, "id": …}`, both
/// required by `/doc`'s schema for `POST /api/session`), and a real session
/// created that way answers with the model echoed back and runs its step on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpencodeModel {
    provider: String,
    id: String,
}

impl OpencodeModel {
    /// Split a fully-qualified `<provider>/<model>` id — the form `opencode run
    /// --model` documents and this route requires.
    ///
    /// `None` when there is no provider to name, because there is nothing to
    /// guess: opencode's own ids carry slashes inside them
    /// (`wandb/nvidia/NVIDIA-Nemotron-…`), so only the FIRST segment is the
    /// provider and a bare id names none. A caller that gets `None` must refuse
    /// rather than open a session without one — which is exactly the silent
    /// drop this type exists to end.
    #[must_use]
    pub fn parse(model: &str) -> Option<Self> {
        let (provider, id) = model.split_once('/')?;
        if provider.is_empty() || id.is_empty() {
            return None;
        }
        Some(Self {
            provider: provider.to_string(),
            id: id.to_string(),
        })
    }
}

/// The request that creates the turn's own session (opencode), or the workspace
/// everything else hangs off (crush).
#[must_use]
pub fn open_request(opening: &TurnOpening) -> HttpRequest {
    match opening {
        // The session names its own working directory, so the server can be
        // shared by dispatches in different projects — the cwd stays a per-turn
        // setting and never widens the pool key. The model is per turn for the
        // same reason, and it has to be named here: see [`OpencodeModel`].
        TurnOpening::Opencode { cwd, model } => {
            let mut body = json!({"location": {"directory": cwd.to_string()}});
            if let Some(model) = model {
                body["model"] = json!({"providerID": model.provider, "id": model.id});
            }
            HttpRequest::new(Method::Post, "/api/session".to_string(), Some(body))
        }
        // The workspace is where crush's `client_id` travels in the BODY — and
        // where its permission posture is declared: `yolo` is the same "act
        // without asking" the `--yolo` flag sets, applied at creation so the
        // workspace never exists in a posture this run did not choose. Sent
        // either way, because `false` is a decision too and leaving it to the
        // server's default would make a restrictive run's posture implicit.
        TurnOpening::Crush {
            cwd,
            client,
            decision,
        } => HttpRequest::new(
            Method::Post,
            "/v1/workspaces".to_string(),
            Some(json!({
                "client_id": client.as_str(),
                "path": cwd.to_string(),
                "yolo": decision.allows(),
            })),
        ),
    }
}

/// Crush's session-create request, which hangs off the workspace the open
/// request produced. Crush-only: opencode's open request already made its
/// session, so there is no shape here to get wrong.
#[must_use]
pub fn session_request(workspace: &ResourceId, client: &ClientId) -> HttpRequest {
    // Sessions are created on the WORKSPACE; `/agent/sessions` is where an
    // EXISTING one is addressed, and creating there answers `404 page not
    // found`.
    HttpRequest::new(
        Method::Post,
        crush_path(workspace, client, "/sessions"),
        Some(json!({"title": "oneharness"})),
    )
}

/// The id in a create response, wherever that server puts it. Never guessed: an
/// unrecognized shape yields `None` so the run fails loudly rather than
/// addressing an id nobody returned.
#[must_use]
pub fn parse_id(body: &str) -> Option<ResourceId> {
    let value: Value = serde_json::from_str(body).ok()?;
    // Opencode wraps every answer in a `{"data": …}` envelope; crush does not.
    let object = value.get("data").unwrap_or(&value);
    let raw = object.get("id").and_then(Value::as_str)?;
    ResourceId::new(raw).ok()
}

/// The request that submits the prompt and starts the turn.
#[must_use]
pub fn prompt_request(address: &TurnAddress, prompt: &str) -> HttpRequest {
    match address {
        TurnAddress::Opencode { session } => HttpRequest::new(
            Method::Post,
            format!("/api/session/{}/prompt", session.as_str()),
            Some(json!({"prompt": {"text": prompt}})),
        ),
        // The prompt goes to the workspace's agent with the session in the
        // BODY: `/agent/sessions/{sid}` is a GET-only resource and answers
        // `405 Method Not Allowed`. It returns 202 with the turn running in the
        // background, so completion is read off the event stream.
        TurnAddress::Crush {
            workspace,
            session,
            client,
        } => HttpRequest::new(
            Method::Post,
            crush_path(workspace, client, "/agent"),
            Some(json!({"prompt": prompt, "session_id": session.as_str()})),
        ),
    }
}

/// The request that aborts the in-flight turn — the whole point of the
/// mechanism.
#[must_use]
pub fn interrupt_request(address: &TurnAddress) -> HttpRequest {
    match address {
        TurnAddress::Opencode { session } => HttpRequest::new(
            Method::Post,
            format!("/api/session/{}/interrupt", session.as_str()),
            Some(json!({})),
        ),
        TurnAddress::Crush {
            workspace,
            session,
            client,
        } => HttpRequest::new(
            Method::Post,
            crush_path(
                workspace,
                client,
                &format!("/agent/sessions/{}/cancel", session.as_str()),
            ),
            Some(json!({})),
        ),
    }
}

/// The blanket "stop asking" request a permissive run makes once, where the
/// server has one. Crush's `permissions/skip` is its `--yolo`; opencode has no
/// equivalent route, so it answers each request as it arrives.
#[must_use]
pub fn skip_permissions_request(
    address: &TurnAddress,
    decision: PermissionDecision,
) -> Option<HttpRequest> {
    match address {
        TurnAddress::Opencode { .. } => None,
        TurnAddress::Crush {
            workspace, client, ..
        } => decision.allows().then(|| {
            HttpRequest::new(
                Method::Post,
                crush_path(workspace, client, "/permissions/skip"),
                Some(json!({"skip": true})),
            )
        }),
    }
}

/// The request that answers one permission ask, or `None` when the ask is not
/// this turn's to answer.
///
/// Opencode's event stream is server-wide — a pooled server serves every
/// dispatch that shares its key — so an ask can name a session belonging to
/// another run entirely. Answering that one would spend this run's posture on a
/// turn it does not own, and on a route it was never given, so an ask naming
/// any other session is left for its owner. The route is always built from this
/// turn's own validated session, never from the event's.
#[must_use]
pub fn permission_reply_request(
    address: &TurnAddress,
    ask: &PermissionAsk,
    decision: PermissionDecision,
) -> Option<HttpRequest> {
    match address {
        TurnAddress::Opencode { session } => {
            if ask.session.as_ref().is_some_and(|asked| asked != session) {
                return None;
            }
            Some(HttpRequest::new(
                Method::Post,
                format!(
                    "/api/session/{}/permission/{}/reply",
                    session.as_str(),
                    ask.id.as_str()
                ),
                // `once` rather than `always`: a grant this run makes must not
                // outlive it into the next one.
                Some(json!({"reply": match decision {
                    PermissionDecision::Allow => "once",
                    PermissionDecision::Deny => "reject",
                }})),
            ))
        }
        // Crush's answer names no session at all: it goes to the workspace this
        // run created, carrying the ask's own payload back. There is nothing an
        // event could retarget, so there is nothing to check.
        // llmlint: ignore[boundary_inputs_validated] `permission` is crush's own
        // request echoed verbatim, which is how its grant route identifies what
        // is being answered — a shape oneharness deliberately does not model, so
        // there is nothing here to validate against. It reaches the wire as a
        // serialized JSON value, never as request-line or header text, and the
        // route it goes to is built entirely from this run's validated ids.
        TurnAddress::Crush {
            workspace, client, ..
        } => Some(HttpRequest::new(
            Method::Post,
            crush_path(workspace, client, "/permissions/grant"),
            Some(json!({
                "action": match decision {
                    PermissionDecision::Allow => "allow",
                    PermissionDecision::Deny => "deny",
                },
                "permission": ask.payload,
            })),
        )),
    }
}

/// A cheap request that answers as soon as the server is up, for the readiness
/// wait after a launch. Any HTTP answer at all proves it is listening — the
/// status does not matter, only that something spoke HTTP back.
#[must_use]
pub fn readiness_request(shape: HttpShape) -> HttpRequest {
    match shape {
        HttpShape::Opencode => HttpRequest::new(Method::Get, "/api/app".to_string(), None),
        HttpShape::Crush => HttpRequest::new(Method::Get, "/v1/health".to_string(), None),
    }
}

/// The event stream to follow for this turn.
#[must_use]
pub fn event_stream_request(address: &TurnAddress) -> HttpRequest {
    match address {
        TurnAddress::Opencode { .. } => {
            HttpRequest::new(Method::Get, "/api/event".to_string(), None)
        }
        TurnAddress::Crush {
            workspace, client, ..
        } => HttpRequest::new(Method::Get, crush_path(workspace, client, "/events"), None),
    }
}

/// What one `data:` payload from the event stream means.
#[must_use]
pub fn classify_event(shape: HttpShape, payload: &str) -> TurnEvent {
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return TurnEvent::Ignored;
    };
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match shape {
        HttpShape::Opencode => classify_opencode(kind, &value),
        HttpShape::Crush => classify_crush(kind, &value),
    }
}

fn classify_opencode(kind: &str, value: &Value) -> TurnEvent {
    let data = value.get("data").unwrap_or(&Value::Null).clone();
    match kind {
        // A permission ask carries its own id and the session to answer on.
        k if k.starts_with("permission.") || k == "permission" => {
            let id = data
                .get("id")
                .or_else(|| data.get("requestID"))
                .and_then(Value::as_str);
            let session = match data.get("sessionID") {
                Some(Value::String(session)) => match ResourceId::new(session) {
                    Ok(session) => Some(session),
                    Err(_) => return TurnEvent::Ignored,
                },
                Some(_) => return TurnEvent::Ignored,
                None => None,
            };
            match id.and_then(|id| ResourceId::new(id).ok()) {
                Some(id) => TurnEvent::PermissionRequest(PermissionAsk {
                    id,
                    session,
                    payload: data,
                }),
                None => TurnEvent::Ignored,
            }
        }
        // Only the end of a turn that STARTED: opencode announces idleness
        // whenever the session is idle, including before the prompt is
        // admitted, and reading that as terminal ended every run in under a
        // second. The driver holds this until it has seen `Started`.
        "session.idle" => TurnEvent::Finished,
        "session.next.prompt.admitted" => TurnEvent::Started,
        "session.next.text.ended" => data
            .get("text")
            .and_then(Value::as_str)
            .map_or(TurnEvent::Ignored, |text| TurnEvent::Text(text.to_string())),
        _ => TurnEvent::Ignored,
    }
}

fn classify_crush(kind: &str, value: &Value) -> TurnEvent {
    // Crush nests the interesting object two payloads deep:
    // `{"type":K,"payload":{"type":"created","payload":{…}}}`.
    let inner = value
        .get("payload")
        .and_then(|p| p.get("payload"))
        .cloned()
        .unwrap_or(Value::Null);
    match kind {
        "permission_request" => match inner
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| ResourceId::new(id).ok())
        {
            Some(id) => TurnEvent::PermissionRequest(PermissionAsk {
                id,
                session: inner
                    .get("session_id")
                    .and_then(Value::as_str)
                    .and_then(|session| ResourceId::new(session).ok()),
                payload: inner,
            }),
            None => TurnEvent::Ignored,
        },
        // Emitted however the run ended — completed, errored, or cancelled.
        "run_complete" => TurnEvent::Finished,
        "message" => TurnEvent::Started,
        _ => TurnEvent::Ignored,
    }
}

/// The most of one answer's body — or of one event — a reader will hold.
///
/// Every route here answers a small JSON document, and the bytes arrive from
/// another program's socket. Without a ceiling the *peer* decides how much
/// memory a run spends: a declared length nobody could satisfy, framing that
/// never terminates, or a line with no newline in it are all the same spend.
/// Orders of magnitude above anything these servers actually send.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The most of one response head a reader will hold before refusing it.
///
/// A head is a status line and a handful of headers; past this there is no
/// blank line coming, and every byte read is memory a peer chose to spend.
pub const MAX_HEAD_BYTES: usize = 64 * 1024;

/// A status code a response can actually carry.
///
/// A newtype for the same reason [`crate::domain::control::Port`] is one: the
/// range check belongs to the boundary that reads the status line, and every
/// reader after it should be handed a value that has already passed. Both the
/// success test and the refusal test branch on this, so a `0` or a `9999`
/// reaching them is a decision taken about a number no server sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusCode(u16);

impl StatusCode {
    /// Accept `raw` as a status a response can carry, or say why it cannot be
    /// one. The range is the grammar's: `100` through `599`.
    pub fn new(raw: u16) -> Result<Self, String> {
        if (100..600).contains(&raw) {
            Ok(StatusCode(raw))
        } else {
            Err(format!("{raw} is not an HTTP status code"))
        }
    }

    #[must_use]
    pub fn get(self) -> u16 {
        self.0
    }

    /// Whether the server accepted the request. A control route that answers
    /// `4xx`/`5xx` has not done what was asked, however readable its body.
    ///
    /// A `3xx` has not either: this client follows no `Location`, so a redirect
    /// is an answer saying where to ask, not the asking. Counted as success, a
    /// redirected interrupt would be reported `served` while the turn ran on —
    /// worse than any refusal, because a refusal is something a supervisor can
    /// act on.
    ///
    /// Defined here rather than at each caller so the answered-or-not question
    /// has one answer: a response's `ok` and a stream's refusal cannot drift
    /// into disagreeing about the same code.
    #[must_use]
    pub fn is_success(self) -> bool {
        (200..300).contains(&self.0)
    }
}

impl std::fmt::Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// How a response says where its body ends.
///
/// One value rather than a `chunked` flag beside a `content_length`, because
/// HTTP/1.1's framing mechanisms are a *choice* and not two independent
/// switches. Declaring both is the ambiguity RFC 9112 refuses, and as two
/// fields that refusal is only a rule [`parse_head`] remembers to enforce —
/// every reader downstream still has to defend against the combination, and
/// the guard that does it (`if !head.chunked` beside a length check) reads
/// like an ordinary condition rather than the invariant it is. Choosing here
/// means no reader can be handed the ambiguous state at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyFraming {
    /// `Transfer-Encoding: chunked`: the body ends at its terminating chunk.
    /// Both servers use it, and a reader that skips de-chunking reports a JSON
    /// decode error where the harness in fact answered correctly.
    Chunked,
    /// `Content-Length`: the body is exactly this many bytes.
    ///
    /// This is how a reader knows the answer is whole *without* the server
    /// closing the connection. Opencode keeps a `Connection: close` request's
    /// socket open on some routes, so a reader that waits for EOF reports a
    /// complete answer as cut short.
    Length(usize),
    /// Neither was declared: the body ends when the connection does.
    UntilClose,
}

impl BodyFraming {
    /// Whether the body arrives in chunk framing, and so must be de-chunked.
    #[must_use]
    pub fn is_chunked(self) -> bool {
        matches!(self, BodyFraming::Chunked)
    }

    /// Whether the head declares where the body ends, so a close arriving
    /// first is an answer cut short rather than an answer.
    #[must_use]
    pub fn ends_before_close(self) -> bool {
        !matches!(self, BodyFraming::UntilClose)
    }
}

/// A response's head, as far as a reader must understand it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: StatusCode,
    /// Where the body starts in the buffer the head was parsed from.
    pub body_at: usize,
    /// Where this response's body ends, as the server framed it.
    pub framing: BodyFraming,
}

/// What the front of a response turned out to be.
///
/// Three answers rather than an `Option`, because "not all here yet" and "here
/// and unreadable" call for opposite actions: the first is read on, the second
/// is stop. Collapsing them makes a malformed head an arrival that never comes,
/// which a reader spends its whole timeout waiting for and then reports as a
/// server that went quiet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadScan {
    /// The blank line that ends the head has not arrived yet.
    Incomplete,
    /// A whole head, with everything a reader needs to frame the body.
    Whole(ResponseHead),
    /// A whole head oneharness will not frame a body from, and why.
    Unreadable(HeadUnreadable),
}

/// Why a whole head has no body a reader may frame from it.
///
/// A closed set rather than a message, so every reader reports the same answers
/// in the same words, and a shape nobody has decided about has to be added here
/// before it can be reported as anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadUnreadable {
    /// The first line is not an HTTP status line, so this is not a response at
    /// all — and nothing after it is a head to read headers out of.
    NotAStatusLine,
    /// A `Content-Length` that names no number of bytes, carrying the value as
    /// declared: what a proxy or server actually sent is the whole diagnostic.
    UnreadableLength(String),
    /// Two `Content-Length` declarations that frame different bodies.
    ConflictingLengths(usize, usize),
    /// Both mutually exclusive HTTP/1.1 body-framing mechanisms were declared.
    AmbiguousFraming,
    /// A `Content-Length` past what a reader will hold. Refused where it is
    /// declared rather than where the bytes run out: the declaration is what a
    /// reader would otherwise reserve and wait for.
    OversizedLength(usize),
    /// No blank line ended the head within [`MAX_HEAD_BYTES`].
    OversizedHead(usize),
}

impl std::fmt::Display for HeadUnreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeadUnreadable::NotAStatusLine => {
                write!(f, "its first line is not an HTTP status line")
            }
            HeadUnreadable::UnreadableLength(declared) => write!(
                f,
                "it declared a Content-Length no reader can frame a body from (`{declared}`)"
            ),
            HeadUnreadable::ConflictingLengths(first, second) => write!(
                f,
                "it declared two different Content-Length values ({first} and {second})"
            ),
            HeadUnreadable::AmbiguousFraming => write!(
                f,
                "it declared both Transfer-Encoding: chunked and Content-Length"
            ),
            HeadUnreadable::OversizedLength(declared) => write!(
                f,
                "it declared a {declared}-byte body, past the {MAX_BODY_BYTES} bytes a reader will hold"
            ),
            HeadUnreadable::OversizedHead(bound) => {
                write!(f, "no blank line ended its head within {bound} bytes")
            }
        }
    }
}

/// Scan the head at the front of `raw`.
///
/// The head is external input from another program: its framing decides how
/// many bytes of the connection belong to this answer, so a declaration that
/// cannot be read is refused rather than dropped. Dropping it leaves the answer
/// looking unframed, which ends it at the close of the connection — so a body
/// the server framed as 12 bytes would be handed back with whatever followed it
/// appended, and a JSON document that parses is a wrong answer where the
/// refusal is a loud one.
#[must_use]
pub fn parse_head(raw: &[u8]) -> HeadScan {
    let Some(end) = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|at| at + 4)
    else {
        // "Not all here yet" only holds while a blank line is still plausible.
        // Past the bound it is not: the reader would go on holding whatever the
        // peer keeps sending, waiting for an ending nobody is going to send.
        if raw.len() > MAX_HEAD_BYTES {
            return HeadScan::Unreadable(HeadUnreadable::OversizedHead(MAX_HEAD_BYTES));
        }
        return HeadScan::Incomplete;
    };
    let head = String::from_utf8_lossy(&raw[..end]);
    let Some(status) = parse_status_line(head.lines().next().unwrap_or_default()) else {
        return HeadScan::Unreadable(HeadUnreadable::NotAStatusLine);
    };
    let lowered = head.to_ascii_lowercase();
    // Read off the header itself, never searched for in the head at large: a
    // head that merely carries the words (an upstream note, a header whose name
    // ends in the same one) would otherwise frame a body the server never
    // declared, and the reader would de-chunk an answer that was never chunked.
    // `chunked` is the last coding of a possibly-longer list, per the grammar.
    let chunked = lowered
        .lines()
        .filter_map(|line| line.strip_prefix("transfer-encoding:"))
        .any(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|coding| !coding.is_empty())
                .next_back()
                == Some("chunked")
        });
    let mut content_length = None;
    for declared in lowered
        .lines()
        .filter_map(|line| line.strip_prefix("content-length:"))
        .map(str::trim)
    {
        let Ok(length) = declared.parse::<usize>() else {
            return HeadScan::Unreadable(HeadUnreadable::UnreadableLength(declared.to_string()));
        };
        // Two disagreeing declarations frame two different bodies, and whichever
        // one a reader picks, the remainder is read as something it is not.
        if let Some(first) = content_length.filter(|first| *first != length) {
            return HeadScan::Unreadable(HeadUnreadable::ConflictingLengths(first, length));
        }
        // A number a reader would reserve for and then wait on. Refused here so
        // no caller has to decide what to do half-way through spending it.
        if length > MAX_BODY_BYTES {
            return HeadScan::Unreadable(HeadUnreadable::OversizedLength(length));
        }
        content_length = Some(length);
    }
    // The one place a response's framing is chosen. RFC 9112 calls declaring
    // both mechanisms ambiguous framing: picking either would let a peer make
    // oneharness and an intermediary disagree about where this response ends,
    // so it is refused here rather than travelling on as a state every reader
    // has to keep refusing for itself.
    let framing = match (chunked, content_length) {
        (true, Some(_)) => return HeadScan::Unreadable(HeadUnreadable::AmbiguousFraming),
        (true, None) => BodyFraming::Chunked,
        (false, Some(length)) => BodyFraming::Length(length),
        (false, None) => BodyFraming::UntilClose,
    };
    HeadScan::Whole(ResponseHead {
        status,
        body_at: end,
        framing,
    })
}

/// The status `line` states, or `None` when it is not a status line.
///
/// Checked in full rather than "whatever second token parses as a number": a
/// line like `NOT-HTTP 200 fine` would otherwise be read as a `200`, which
/// promotes something that is not a response at all into a head whose headers
/// then decide how the rest of the connection is read.
fn parse_status_line(line: &str) -> Option<StatusCode> {
    let mut tokens = line.split_whitespace();
    if !tokens.next()?.starts_with("HTTP/") {
        return None;
    }
    // Exactly three digits, per the grammar: a shorter or longer run of digits
    // is a different field that happens to be numeric.
    let code = tokens.next()?;
    if code.len() != 3 || !code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    StatusCode::new(code.parse::<u16>().ok()?).ok()
}

/// Reassembles a `Transfer-Encoding: chunked` body as its bytes arrive.
#[derive(Debug, Default)]
pub struct ChunkedDecoder {
    pending: Vec<u8>,
    /// Whether a whole size-header line arrived that is not a chunk size.
    ///
    /// The peer said `Transfer-Encoding: chunked`; this is the peer then
    /// contradicting itself, at a trust boundary. Sticky, because nothing
    /// arriving later re-frames bytes already read at the wrong offset.
    malformed: bool,
    /// Whether the terminating zero-length chunk has arrived.
    complete: bool,
    /// Whether framing that never completed grew past [`MAX_BODY_BYTES`].
    /// Retained rather than held: the bytes are dropped, and the reader is told
    /// so, because a body that silently lost its middle is a wrong answer.
    overflowed: bool,
    /// How far into `pending` the search for a size header's `\r\n` has already
    /// looked and found none.
    ///
    /// Without it every arrival re-searches the whole buffer, so a peer sending
    /// a size header it never terminates costs quadratic time in what it sends
    /// — which is a second thing it gets to spend on this reader's behalf, and
    /// a cheaper one to spend than memory.
    scanned: usize,
}

impl ChunkedDecoder {
    /// Whether the body is whole — the terminating chunk has been fed.
    ///
    /// A reader waiting on a socket needs this to stop *without* an EOF: a
    /// server that keeps a connection open after a complete chunked answer
    /// would otherwise be indistinguishable from one still writing it.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    /// Whether framing that never completed grew past what this will hold, so
    /// a reader can say the answer is unreadable rather than hand back a body
    /// with a hole in it.
    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }
    /// Whether the peer framed its answer as chunked and then sent something
    /// that is not chunk framing, so there is no body here to hand back — only
    /// bytes whose shape nobody can vouch for.
    #[must_use]
    pub fn malformed(&self) -> bool {
        self.malformed
    }

    /// Feed `bytes`, returning whatever is now completely decoded. Incomplete
    /// framing is retained for the next call rather than guessed at — but only
    /// up to [`MAX_BODY_BYTES`], because "retained" is memory a peer that never
    /// finishes a chunk gets to keep asking for.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        if self.overflowed || self.malformed {
            return Vec::new();
        }
        self.pending.extend_from_slice(bytes);
        if self.pending.len() > MAX_BODY_BYTES {
            self.pending = Vec::new();
            self.overflowed = true;
            return Vec::new();
        }
        let mut out = Vec::new();
        loop {
            // A chunk's terminating CRLF can arrive in a later read than its
            // body, so it is skipped here rather than after the body: leaving
            // it in front of the next size header parses as an empty size and
            // ends the stream early.
            let strip = self
                .pending
                .iter()
                .position(|b| !matches!(b, b'\r' | b'\n'))
                .unwrap_or(self.pending.len());
            if strip > 0 {
                self.pending.drain(..strip);
                self.scanned = 0;
            }
            // Searched from where the last search left off: a size header that
            // never terminates would otherwise be re-scanned in full on every
            // arrival. The overlap of one byte is what keeps a `\r\n` split
            // across two arrivals from being missed.
            let Some(line_end) = self.pending[self.scanned.min(self.pending.len())..]
                .windows(2)
                .position(|w| w == b"\r\n")
                .map(|at| at + self.scanned + 2)
            else {
                self.scanned = self.pending.len().saturating_sub(1);
                return out;
            };
            self.scanned = 0;
            let header = String::from_utf8_lossy(&self.pending[..line_end - 2]).to_string();
            let size =
                usize::from_str_radix(header.split(';').next().unwrap_or_default().trim(), 16);
            let Ok(size) = size else {
                // A whole line arrived where a chunk size belongs and it is not
                // one. This decoder is fed only when the head SAID `chunked`, so
                // that is the peer contradicting its own framing — and framing is
                // what says where the body starts and stops. Handing the bytes
                // back as an unframed body is the reader picking a framing on the
                // peer's behalf: it splices size lines and terminators into the
                // caller's data, and lets a server declare one shape and deliver
                // another. Refused instead, and stickily: bytes already read at
                // the wrong offset are not re-framed by anything arriving later.
                self.pending.clear();
                self.malformed = true;
                return Vec::new();
            };
            if size == 0 {
                self.pending.clear();
                self.complete = true;
                return out;
            }
            if self.pending.len() < line_end + size {
                return out;
            }
            out.extend_from_slice(&self.pending[line_end..line_end + size]);
            self.pending.drain(..line_end + size);
        }
    }
}

/// Splits a server-sent-events body into whole `data:` payloads as they arrive.
#[derive(Debug, Default)]
pub struct SseAccumulator {
    pending: String,
    /// Whether one payload grew past [`MAX_BODY_BYTES`] without a newline.
    overflowed: bool,
}

impl SseAccumulator {
    /// Whether one payload grew past what this will hold before ending. A
    /// stream is long-lived by design, so an event with no newline in it is the
    /// one arrival that never stops costing memory.
    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Feed decoded body bytes, returning every complete `data:` payload in
    /// them. A partial line is held back, never emitted half-parsed — bounded,
    /// because the newline that ends it is the peer's to send.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        if self.overflowed {
            return Vec::new();
        }
        self.pending.push_str(&String::from_utf8_lossy(bytes));
        if self.pending.len() > MAX_BODY_BYTES {
            self.pending = String::new();
            self.overflowed = true;
            return Vec::new();
        }
        let mut out = Vec::new();
        while let Some(at) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=at).collect();
            if let Some(data) = line.trim().strip_prefix("data:") {
                let data = data.trim();
                if !data.is_empty() {
                    out.push(data.to_string());
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::control::{absolute_for_test, absolute_text_for_test};
    use crate::domain::mode::PermissionMode;

    fn crush_address() -> TurnAddress {
        TurnAddress::Crush {
            workspace: ResourceId::new("ws-1").unwrap(),
            session: ResourceId::new("ses-1").unwrap(),
            client: ClientId::new("client-9").unwrap(),
        }
    }

    fn opencode_address() -> TurnAddress {
        TurnAddress::Opencode {
            session: ResourceId::new("ses_abc").unwrap(),
        }
    }

    #[test]
    fn an_invalid_supplied_permission_session_is_rejected() {
        assert_eq!(
            classify_event(
                HttpShape::Opencode,
                r#"{"type":"permission.requested","data":{"id":"per_1","sessionID":"../other"}}"#,
            ),
            TurnEvent::Ignored
        );
    }

    /// A crush opening in `decision`'s posture. The client identity is not
    /// optional here — the type has no variant without one — so every test
    /// builds the request the way the wire requires it.
    fn crush_opening<'a>(
        cwd: &'a AbsolutePath,
        client: &'a ClientId,
        decision: PermissionDecision,
    ) -> TurnOpening<'a> {
        TurnOpening::Crush {
            cwd,
            client,
            decision,
        }
    }

    /// `path` as a working directory the host counts as absolute.
    fn cwd(path: &str) -> AbsolutePath {
        absolute_for_test(path)
    }

    #[test]
    fn an_id_that_could_retarget_a_request_is_refused() {
        // Every id here comes out of another program's JSON and goes straight
        // into a path — including the interrupt's.
        for bad in ["", "..", ".", "a/b", "a?b", "a b", &"x".repeat(129)] {
            assert!(ResourceId::new(bad).is_err(), "accepted `{bad}`");
        }
        assert_eq!(
            ResourceId::new("ses_01d2.a-b").unwrap().as_str(),
            "ses_01d2.a-b"
        );
    }

    #[test]
    fn a_turn_names_its_own_working_directory_on_the_shared_server() {
        // Both servers are shared across dispatches, so a turn that did not say
        // where it runs would do its work in whatever directory the server
        // happened to start in — and an absolute one is the only kind that
        // names the same place from the server's process as from this one.
        let here = cwd("/work/here");
        let opencode: Value = serde_json::from_str(
            open_request(&TurnOpening::Opencode {
                cwd: &here,
                model: None,
            })
            .body()
            .unwrap(),
        )
        .unwrap();
        assert_eq!(opencode["location"]["directory"], here.to_string());
        let client = ClientId::new("c").unwrap();
        let crush: Value = serde_json::from_str(
            open_request(&crush_opening(&here, &client, PermissionDecision::Allow))
                .body()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(crush["path"], here.to_string());
    }

    #[test]
    fn an_opencode_turn_names_the_model_it_runs_on_to_the_session() {
        // The model is per turn like the cwd above, and the session is the only
        // place opencode takes one: its server loads a `model` from
        // `OPENCODE_CONFIG_CONTENT` and creates sessions on a different one
        // anyway (measured against 1.18.5), so a session opened without it runs
        // on whatever the server picked — live, a free model answering 401.
        let here = cwd("/work/here");
        let model = OpencodeModel::parse("anthropic/claude-haiku-4-5").expect("qualified");
        let body: Value = serde_json::from_str(
            open_request(&TurnOpening::Opencode {
                cwd: &here,
                model: Some(&model),
            })
            .body()
            .unwrap(),
        )
        .unwrap();
        // The field names are the server's own, from `/doc`'s schema for
        // `POST /api/session`, where both are required.
        assert_eq!(body["model"]["providerID"], "anthropic");
        assert_eq!(body["model"]["id"], "claude-haiku-4-5");
        // A run with no model asks for none rather than sending an empty one,
        // which the same schema would refuse.
        let bare: Value = serde_json::from_str(
            open_request(&TurnOpening::Opencode {
                cwd: &here,
                model: None,
            })
            .body()
            .unwrap(),
        )
        .unwrap();
        assert!(bare.get("model").is_none(), "{bare}");
    }

    #[test]
    fn a_model_naming_no_provider_is_refused_rather_than_guessed_at() {
        // Only the FIRST segment is the provider: opencode's own ids carry
        // slashes inside them, so a split anywhere else invents a provider.
        let nested = OpencodeModel::parse("wandb/nvidia/NVIDIA-Nemotron-3.5").expect("qualified");
        assert_eq!(nested.provider, "wandb");
        assert_eq!(nested.id, "nvidia/NVIDIA-Nemotron-3.5");
        // And a bare id names no provider, so there is nothing to send. The
        // caller refuses; it must never open a session without one, because
        // that is the silent drop that ran a controlled turn on a free model.
        for bare in ["haiku", "", "/claude-haiku-4-5", "anthropic/"] {
            assert!(OpencodeModel::parse(bare).is_none(), "accepted `{bare}`");
        }
    }

    #[test]
    fn crush_carries_its_client_id_in_the_body_then_the_query() {
        // The one detail that answers a bare `{"message":"invalid client_id"}`
        // when it is got wrong.
        let client = ClientId::new("client-9").unwrap();
        let open = open_request(&crush_opening(
            &cwd("/work"),
            &client,
            PermissionDecision::Allow,
        ));
        assert_eq!(open.path(), "/v1/workspaces");
        let body: Value = serde_json::from_str(open.body().unwrap()).unwrap();
        assert_eq!(body["client_id"], "client-9");
        assert_eq!(body["path"], absolute_text_for_test("/work"));

        let address = crush_address();
        for request in [
            prompt_request(&address, "hi"),
            interrupt_request(&address),
            event_stream_request(&address),
            skip_permissions_request(&address, PermissionDecision::Allow).unwrap(),
        ] {
            assert!(
                request.path().contains("client_id=client-9"),
                "{} lost the client id",
                request.path()
            );
        }
    }

    #[test]
    fn a_crush_workspace_declares_the_runs_permission_posture_when_it_is_created() {
        // Crush's server blocks on a permission decision, so the posture is what
        // decides whether the agent ever runs a tool. `yolo` carries it on the
        // workspace itself — the same thing `--yolo` sets — so the workspace is
        // never briefly in a posture this run did not choose.
        let client = ClientId::new("client-9").unwrap();
        let work = cwd("/work");
        let permissive: Value = serde_json::from_str(
            open_request(&crush_opening(&work, &client, PermissionDecision::Allow))
                .body()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(permissive["yolo"], true);

        // And a restrictive run says so rather than leaving it to the server's
        // default: an absent field is not a decision.
        let restrictive: Value = serde_json::from_str(
            open_request(&crush_opening(&work, &client, PermissionDecision::Deny))
                .body()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(restrictive["yolo"], false);

        // Opencode has no such field — nor a posture, nor a client identity, so
        // its opening carries none of them and inventing one would be a key its
        // session-create route does not read.
        let opencode: Value = serde_json::from_str(
            open_request(&TurnOpening::Opencode {
                cwd: &work,
                model: None,
            })
            .body()
            .unwrap(),
        )
        .unwrap();
        assert!(opencode.get("yolo").is_none(), "{opencode}");
        assert!(opencode.get("client_id").is_none(), "{opencode}");
    }

    #[test]
    fn only_opencode_ends_an_aborted_turn_without_saying_so() {
        // Measured live, and the difference decides when a redirection can be
        // handed over: opencode's stream simply stops after an abort — no
        // `session.idle` ever arrives — so a run holding the message for an
        // end-of-turn event waits out its whole timeout and delivers nothing.
        // Crush emits `run_complete` however the run ended, so its redirection
        // rides that rather than a cancel whose turn may still be winding down.
        assert!(HttpShape::Opencode.abort_ends_turn_silently());
        assert!(!HttpShape::Crush.abort_ends_turn_silently());
    }

    #[test]
    fn each_shape_addresses_its_own_routes() {
        let crush = crush_address();
        assert_eq!(crush.shape(), HttpShape::Crush);
        assert_eq!(
            prompt_request(&crush, "hi").path(),
            "/v1/workspaces/ws-1/agent?client_id=client-9"
        );
        assert_eq!(
            interrupt_request(&crush).path(),
            "/v1/workspaces/ws-1/agent/sessions/ses-1/cancel?client_id=client-9"
        );
        // The session is created on the workspace, not under `/agent`.
        assert_eq!(
            session_request(
                &ResourceId::new("ws-1").unwrap(),
                &ClientId::new("client-9").unwrap(),
            )
            .path(),
            "/v1/workspaces/ws-1/sessions?client_id=client-9"
        );

        let opencode = opencode_address();
        assert_eq!(opencode.shape(), HttpShape::Opencode);
        assert_eq!(
            prompt_request(&opencode, "hi").path(),
            "/api/session/ses_abc/prompt"
        );
        assert_eq!(
            interrupt_request(&opencode).path(),
            "/api/session/ses_abc/interrupt"
        );
        // Each address names the session the report echoes and a later run
        // resumes from, whichever protocol it belongs to.
        assert_eq!(crush.session().as_str(), "ses-1");
        assert_eq!(opencode.session().as_str(), "ses_abc");
    }

    #[test]
    fn the_prompt_rides_each_servers_own_field() {
        let opencode: Value =
            serde_json::from_str(prompt_request(&opencode_address(), "do it").body().unwrap())
                .unwrap();
        assert_eq!(opencode["prompt"]["text"], "do it");

        let crush: Value =
            serde_json::from_str(prompt_request(&crush_address(), "do it").body().unwrap())
                .unwrap();
        // A missing `prompt` or `session_id` is a 500 naming the field.
        assert_eq!(crush["prompt"], "do it");
        assert_eq!(crush["session_id"], "ses-1");
    }

    #[test]
    fn a_create_response_yields_its_id_through_either_envelope() {
        assert_eq!(
            parse_id(r#"{"data":{"id":"ses_01"}}"#).unwrap().as_str(),
            "ses_01"
        );
        assert_eq!(parse_id(r#"{"id":"ws-2"}"#).unwrap().as_str(), "ws-2");
        // Never fabricated, and never an id that could retarget a path.
        assert!(parse_id(r#"{"data":{}}"#).is_none());
        assert!(parse_id("not json").is_none());
        assert!(parse_id(r#"{"id":"../other"}"#).is_none());
    }

    #[test]
    fn a_permission_ask_is_recognized_and_answered_on_this_turns_session() {
        // Without this the turn never starts work at all, so every downstream
        // assertion would pass vacuously.
        let event = classify_event(
            HttpShape::Opencode,
            r#"{"type":"permission.requested","data":{"id":"per_1","sessionID":"ses_abc"}}"#,
        );
        let TurnEvent::PermissionRequest(ask) = event else {
            panic!("opencode permission ask not recognized");
        };
        let reply =
            permission_reply_request(&opencode_address(), &ask, PermissionDecision::Allow).unwrap();
        assert_eq!(reply.path(), "/api/session/ses_abc/permission/per_1/reply");
        let body: Value = serde_json::from_str(reply.body().unwrap()).unwrap();
        assert_eq!(body["reply"], "once");
        let denied =
            permission_reply_request(&opencode_address(), &ask, PermissionDecision::Deny).unwrap();
        let body: Value = serde_json::from_str(denied.body().unwrap()).unwrap();
        assert_eq!(body["reply"], "reject");

        let event = classify_event(
            HttpShape::Crush,
            r#"{"type":"permission_request","payload":{"type":"created","payload":{"id":"p9","session_id":"s1","tool_name":"bash"}}}"#,
        );
        let TurnEvent::PermissionRequest(ask) = event else {
            panic!("crush permission ask not recognized");
        };
        let reply =
            permission_reply_request(&crush_address(), &ask, PermissionDecision::Allow).unwrap();
        assert_eq!(
            reply.path(),
            "/v1/workspaces/ws-1/permissions/grant?client_id=client-9"
        );
        let body: Value = serde_json::from_str(reply.body().unwrap()).unwrap();
        assert_eq!(body["action"], "allow");
        // Crush wants the request echoed back, not referenced by id — the
        // answer names no session, so nothing about it can be retargeted.
        assert_eq!(body["permission"]["tool_name"], "bash");
    }

    #[test]
    fn a_permission_ask_for_another_session_is_left_for_the_run_that_owns_it() {
        // Opencode's event stream is server-wide and the server is pooled, so
        // an ask for a concurrent dispatch's session arrives on this turn's
        // stream. Answering it would spend this run's posture on a turn it does
        // not own, at a route it was never given.
        let event = classify_event(
            HttpShape::Opencode,
            r#"{"type":"permission.requested","data":{"id":"per_1","sessionID":"ses_other"}}"#,
        );
        let TurnEvent::PermissionRequest(ask) = event else {
            panic!("opencode permission ask not recognized");
        };
        assert_eq!(ask.session.as_ref().unwrap().as_str(), "ses_other");
        assert!(
            permission_reply_request(&opencode_address(), &ask, PermissionDecision::Allow)
                .is_none()
        );
        assert!(
            permission_reply_request(&opencode_address(), &ask, PermissionDecision::Deny).is_none()
        );

        // An event that names no session at all is this turn's own: the only
        // stream it could have arrived on is the one this turn opened, and the
        // route is built from this turn's session either way.
        let unnamed = PermissionAsk {
            id: ResourceId::new("per_2").unwrap(),
            session: None,
            payload: Value::Null,
        };
        assert_eq!(
            permission_reply_request(&opencode_address(), &unnamed, PermissionDecision::Allow)
                .unwrap()
                .path(),
            "/api/session/ses_abc/permission/per_2/reply"
        );
    }

    #[test]
    fn the_end_of_a_turn_is_read_from_each_streams_own_document() {
        // Opencode's idle event ends a turn only once one has STARTED: it fires
        // before the prompt is admitted too, and a driver that acted on the
        // first one ended every run in under a second.
        assert_eq!(
            classify_event(HttpShape::Opencode, r#"{"type":"session.idle","data":{}}"#),
            TurnEvent::Finished
        );
        // ...and only after the prompt was admitted, which is its own event.
        assert_eq!(
            classify_event(
                HttpShape::Opencode,
                r#"{"type":"session.next.prompt.admitted","data":{}}"#
            ),
            TurnEvent::Started
        );
        assert_eq!(
            classify_event(
                HttpShape::Crush,
                r#"{"type":"run_complete","payload":{"type":"updated","payload":{"cancelled":true}}}"#
            ),
            TurnEvent::Finished
        );
        // Everything else is noise, including a line that is not JSON at all.
        for noise in [
            r#"{"type":"plugin.added","data":{}}"#,
            r#"{"type":"session","payload":{}}"#,
            "not json",
        ] {
            assert!(matches!(
                classify_event(HttpShape::Opencode, noise),
                TurnEvent::Ignored | TurnEvent::Text(_)
            ));
        }
    }

    /// The decision each harness's own mapping for `mode` implies.
    fn decision_for(id: &str, mode: PermissionMode) -> PermissionDecision {
        permits_action(
            crate::domain::harness::by_id(id)
                .expect("a registered harness")
                .mode(mode)
                .expect("a mode the harness declares"),
        )
    }

    #[test]
    fn asking_is_skipped_only_where_the_harness_declares_it_acts_unattended() {
        assert_eq!(
            decision_for("opencode", PermissionMode::Bypass),
            PermissionDecision::Allow
        );
        assert_eq!(
            decision_for("opencode", PermissionMode::Default),
            PermissionDecision::Deny
        );
        // A mode nobody declared as unattended denies rather than grants, so
        // adding one cannot hand an agent authority by omission.
        assert_eq!(
            decision_for("opencode", PermissionMode::ReadOnly),
            PermissionDecision::Deny
        );
        // `edit` carries its policy in the harness's own config, which a turn
        // submitted to a pooled server cannot deliver — so the command layer
        // refuses that combination outright. Denying is the safe answer if one
        // ever reaches the wire anyway: a permission ask carries no sourced way
        // to tell an edit from a command.
        assert_eq!(
            decision_for("opencode", PermissionMode::Edit),
            PermissionDecision::Deny
        );
        // And the posture is the HARNESS's, not the spectrum's: `crush run`
        // cannot gate at all, so its `default` acts without asking — a
        // controlled turn that denied it would be stricter than the CLI can be.
        assert_eq!(
            decision_for("crush", PermissionMode::Default),
            PermissionDecision::Allow
        );
        // Crush can be told once; opencode has no such route and answers each.
        assert!(skip_permissions_request(&crush_address(), PermissionDecision::Allow).is_some());
        assert!(skip_permissions_request(&crush_address(), PermissionDecision::Deny).is_none());
        assert!(skip_permissions_request(&opencode_address(), PermissionDecision::Allow).is_none());
    }

    #[test]
    fn a_chunked_body_is_reassembled_across_arbitrary_arrival_boundaries() {
        // The framing both servers answer with. A reader that skipped this
        // reported a JSON decode error where the harness answered correctly.
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        let HeadScan::Whole(head) = parse_head(raw) else {
            panic!("a chunked head was not read as one");
        };
        assert_eq!(head.status.get(), 200);
        assert_eq!(head.framing, BodyFraming::Chunked);

        let wire = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let mut whole = ChunkedDecoder::default();
        assert_eq!(whole.feed(wire), b"hello world".to_vec());

        // Split at every offset: the decoder must never emit a partial chunk.
        for split in 1..wire.len() {
            let mut decoder = ChunkedDecoder::default();
            let mut out = decoder.feed(&wire[..split]);
            out.extend(decoder.feed(&wire[split..]));
            assert_eq!(out, b"hello world".to_vec(), "split at {split}");
        }
    }

    #[test]
    fn a_body_declared_chunked_that_is_not_chunk_framed_is_refused() {
        // This decoder is fed only when the head said `Transfer-Encoding:
        // chunked`, so a first line that is not a chunk size is the peer
        // contradicting its own framing. Reading it as an unframed body would
        // let a server declare one shape and deliver another.
        let mut decoder = ChunkedDecoder::default();
        assert!(decoder.feed(b"{\"id\":\"x\"}\r\n").is_empty());
        assert!(decoder.malformed());
        assert!(
            !decoder.is_complete(),
            "unreadable framing is not an ending"
        );
        // Sticky: a size header that arrives after the framing broke does not
        // make the bytes before it readable.
        assert!(decoder.feed(b"4\r\nabcd\r\n0\r\n\r\n").is_empty());
        assert!(decoder.malformed());
        assert!(!decoder.is_complete());
    }

    #[test]
    fn framing_that_breaks_after_a_decoded_chunk_is_refused_rather_than_awaited() {
        // The same contradiction, mid-body. Waiting on it instead spends the
        // reader's whole timeout on bytes that will never frame, and reports a
        // peer's malformed answer as a slow one.
        let mut decoder = ChunkedDecoder::default();
        assert_eq!(decoder.feed(b"4\r\nabcd\r\n"), b"abcd".to_vec());
        assert!(!decoder.malformed());
        assert!(decoder.feed(b"not-a-size\r\nmore\r\n").is_empty());
        assert!(decoder.malformed());
    }

    #[test]
    fn a_head_is_only_parsed_once_it_is_whole() {
        assert_eq!(parse_head(b"HTTP/1.1 200 OK\r\n"), HeadScan::Incomplete);
        let HeadScan::Whole(head) = parse_head(b"HTTP/1.1 204 No Content\r\n\r\n") else {
            panic!("a whole head was not read as one");
        };
        assert_eq!(head.status.get(), 204);
        assert_eq!(head.framing, BodyFraming::UntilClose);
        assert_eq!(head.body_at, 27);
        // A whole head that is not a response is not an arrival to wait on: a
        // reader told to wait would spend its whole timeout on bytes that are
        // all already here.
        assert_eq!(
            parse_head(b"garbage\r\n\r\n"),
            HeadScan::Unreadable(HeadUnreadable::NotAStatusLine)
        );
        // Nor is a line that merely CARRIES a number where a status goes: read
        // as a `200`, something that is not a response at all would decide how
        // the rest of the connection is framed.
        for impostor in [
            &b"NOT-HTTP 200 fine\r\n\r\n"[..],
            b"HTTP/1.1 20 OK\r\n\r\n",
            b"HTTP/1.1 0200 OK\r\n\r\n",
            b"HTTP/1.1 999 OK\r\n\r\n",
            b"HTTP/1.1\r\n\r\n",
        ] {
            assert_eq!(
                parse_head(impostor),
                HeadScan::Unreadable(HeadUnreadable::NotAStatusLine),
                "accepted `{}`",
                String::from_utf8_lossy(impostor)
            );
        }
    }

    #[test]
    fn a_status_code_carries_its_range_and_answers_whether_it_succeeded() {
        // The range belongs to the type, so a reader downstream of the status
        // line never branches on a number no server could have sent.
        for outside in [0, 99, 600, 9999] {
            assert!(
                StatusCode::new(outside).is_err(),
                "accepted {outside} as a status"
            );
        }
        for edge in [100, 599] {
            assert_eq!(StatusCode::new(edge).unwrap().get(), edge);
        }
        // And success is decided in one place. A `3xx` is not it: this client
        // follows no `Location`, so a redirected interrupt that counted as
        // success would be reported `served` while the turn ran on.
        for accepted in [200, 204, 299] {
            assert!(StatusCode::new(accepted).unwrap().is_success());
        }
        for refused in [100, 300, 302, 404, 500] {
            assert!(
                !StatusCode::new(refused).unwrap().is_success(),
                "{refused} counted as the server having done what was asked"
            );
        }
    }

    #[test]
    fn a_length_declaration_no_reader_can_frame_a_body_from_is_refused() {
        // `Content-Length` is external input that decides how many bytes of the
        // connection are this answer. Dropping an unreadable one leaves the
        // answer looking unframed, which ends it at the close — so the body
        // would be handed back with whatever followed it appended.
        for head in [
            &b"HTTP/1.1 200 OK\r\nContent-Length: banana\r\n\r\n"[..],
            b"HTTP/1.1 200 OK\r\nContent-Length: \r\n\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Length: 7, 7\r\n\r\n",
        ] {
            assert!(
                matches!(
                    parse_head(head),
                    HeadScan::Unreadable(HeadUnreadable::UnreadableLength(_))
                ),
                "accepted `{}`",
                String::from_utf8_lossy(head)
            );
        }
        // Two declarations that disagree frame two different bodies, and
        // whichever one a reader picks the remainder is read as something it is
        // not. Repeating the SAME length says one thing, so it still reads.
        assert_eq!(
            parse_head(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nContent-Length: 9\r\n\r\n"),
            HeadScan::Unreadable(HeadUnreadable::ConflictingLengths(7, 9))
        );
        let HeadScan::Whole(head) =
            parse_head(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nContent-Length: 7\r\n\r\n")
        else {
            panic!("two agreeing declarations are one length");
        };
        assert_eq!(head.framing, BodyFraming::Length(7));
        // A header that merely ENDS in the name is a different header.
        let HeadScan::Whole(head) =
            parse_head(b"HTTP/1.1 200 OK\r\nX-Original-Content-Length: nope\r\n\r\n")
        else {
            panic!("an unrelated header was read as a length declaration");
        };
        assert_eq!(head.framing, BodyFraming::UntilClose);
    }

    #[test]
    fn chunked_framing_is_read_from_the_transfer_encoding_header_itself() {
        // The declaration decides how the rest of the connection is framed, so
        // it is read as a header rather than searched for anywhere in the head.
        // A head merely CARRYING the words frames a body it never declared: the
        // reader then de-chunks an unchunked answer, and hands back whatever
        // survives that as the server's reply.
        for head in [
            &b"HTTP/1.1 200 OK\r\nX-Upstream-Note: transfer-encoding: chunked\r\n\r\n"[..],
            b"HTTP/1.1 200 OK\r\nX-Transfer-Encoding: chunked\r\n\r\n",
            b"HTTP/1.1 200 transfer-encoding: chunked\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n",
        ] {
            let HeadScan::Whole(parsed) = parse_head(head) else {
                panic!("a whole head was not read as one");
            };
            assert!(
                !parsed.framing.is_chunked(),
                "framed a body as chunked from `{}`",
                String::from_utf8_lossy(head)
            );
        }
        // And every spelling of the real declaration still frames one: the
        // header is case-insensitive, its value may be unspaced, and `chunked`
        // is the last coding of a list rather than the whole value.
        for head in [
            &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"[..],
            b"HTTP/1.1 200 OK\r\ntransfer-encoding:chunked\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTRANSFER-ENCODING: gzip, chunked\r\n\r\n",
        ] {
            let HeadScan::Whole(parsed) = parse_head(head) else {
                panic!("a whole head was not read as one");
            };
            assert!(
                parsed.framing.is_chunked(),
                "missed the framing in `{}`",
                String::from_utf8_lossy(head)
            );
        }
    }

    #[test]
    fn framing_that_never_ends_is_bounded_rather_than_held() {
        // Everything here arrives from another program's socket, and each of
        // these shapes has no ending in it: a declared length nobody could
        // satisfy, chunk framing whose size header never arrives, and an event
        // line with no newline. Unbounded, the peer decides how much memory the
        // run spends — so each is refused at its bound instead of accumulated.
        let declared = MAX_BODY_BYTES + 1;
        assert_eq!(
            parse_head(format!("HTTP/1.1 200 OK\r\nContent-Length: {declared}\r\n\r\n").as_bytes()),
            HeadScan::Unreadable(HeadUnreadable::OversizedLength(declared))
        );
        // A head that never reaches its blank line stops being "not here yet".
        let endless = format!(
            "HTTP/1.1 200 OK\r\nX-Pad: {}\r\n",
            "p".repeat(MAX_HEAD_BYTES)
        );
        assert_eq!(
            parse_head(endless.as_bytes()),
            HeadScan::Unreadable(HeadUnreadable::OversizedHead(MAX_HEAD_BYTES))
        );
        // And one that is merely still arriving is still just that.
        assert_eq!(parse_head(b"HTTP/1.1 200 OK\r\n"), HeadScan::Incomplete);

        // Chunk framing: a size header that never arrives behind a growing body.
        let mut chunks = ChunkedDecoder::default();
        assert!(chunks.feed(b"4\r\nabcd\r\n").len() == 4);
        assert!(!chunks.overflowed());
        assert!(chunks.feed(&vec![b'x'; MAX_BODY_BYTES + 1]).is_empty());
        assert!(chunks.overflowed(), "unterminated framing was held");
        // Once overflowed it stays that way: the bytes it dropped are gone, so
        // decoding on would splice a hole into the middle of a body.
        assert!(chunks.feed(b"0\r\n\r\n").is_empty());
        assert!(!chunks.is_complete());

        // An SSE payload with no newline in it, on a stream that is long-lived
        // by design — the one arrival that never stops costing memory.
        let mut sse = SseAccumulator::default();
        assert_eq!(sse.feed(b"data: one\n"), vec!["one".to_string()]);
        assert!(sse
            .feed(format!("data: {}", "y".repeat(MAX_BODY_BYTES + 1)).as_bytes())
            .is_empty());
        assert!(sse.overflowed(), "an endless event line was held");
        assert!(sse.feed(b"\n").is_empty());
    }

    #[test]
    fn sse_payloads_are_emitted_only_once_their_line_is_complete() {
        let mut sse = SseAccumulator::default();
        assert!(sse.feed(b"data: {\"type\":\"a\"}").is_empty());
        assert_eq!(sse.feed(b"}\n"), vec!["{\"type\":\"a\"}}".to_string()]);
        assert_eq!(
            sse.feed(b": comment\n\ndata: one\ndata: two\n"),
            vec!["one".to_string(), "two".to_string()]
        );
    }

    #[test]
    fn every_route_this_module_builds_is_safe_on_a_request_line() {
        // A path is only ever a constant plus already-validated ids, and this is
        // the property that follows: no space or control character can split the
        // request line into a second request the server would also answer.
        let crush = crush_address();
        let opencode = opencode_address();
        let ask = PermissionAsk {
            id: ResourceId::new("per_1").unwrap(),
            session: None,
            payload: Value::Null,
        };
        let client = ClientId::new("c").unwrap();
        // A working directory whose own text would corrupt a request line if it
        // ever reached one; it rides the BODY, so no route may carry it.
        let awkward = cwd("/a b/../c");
        let mut routes = vec![
            open_request(&crush_opening(&awkward, &client, PermissionDecision::Allow)),
            open_request(&TurnOpening::Opencode {
                cwd: &awkward,
                model: None,
            }),
            readiness_request(HttpShape::Crush),
            readiness_request(HttpShape::Opencode),
        ];
        for address in [&crush, &opencode] {
            routes.push(prompt_request(
                address,
                "a prompt with spaces\nand a newline",
            ));
            routes.push(interrupt_request(address));
            routes.push(event_stream_request(address));
            routes.extend(skip_permissions_request(address, PermissionDecision::Allow));
            routes.extend(permission_reply_request(
                address,
                &ask,
                PermissionDecision::Allow,
            ));
        }
        routes.push(session_request(
            &ResourceId::new("ws-1").unwrap(),
            &ClientId::new("c").unwrap(),
        ));
        for route in routes {
            assert!(
                is_request_target(route.path()),
                "`{}` is not usable on a request line",
                route.path()
            );
        }
        // The property itself, so the check above cannot pass vacuously.
        assert!(!is_request_target("/has space"));
        assert!(!is_request_target("/has\r\nCRLF"));
        assert!(!is_request_target("relative"));
    }

    #[test]
    fn only_the_http_mechanisms_have_an_http_shape() {
        assert_eq!(
            HttpShape::of(ControlShape::OpencodeHttp),
            Some(HttpShape::Opencode)
        );
        assert_eq!(
            HttpShape::of(ControlShape::CrushHttp),
            Some(HttpShape::Crush)
        );
        assert_eq!(HttpShape::of(ControlShape::AcpCancel), None);
        assert_eq!(HttpShape::of(ControlShape::ClaudeControlRequest), None);
        assert_eq!(HttpShape::Crush.shape(), ControlShape::CrushHttp);
        assert_eq!(HttpShape::Opencode.shape(), ControlShape::OpencodeHttp);
    }
}
