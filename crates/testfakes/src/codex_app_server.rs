//! `codex app-server --listen unix://PATH` for the `fake-codex` double: the
//! shared, multi-client server the linked `oneharness-core` runs every
//! controlled Codex turn on, one per pool key (oneharness-core 0.28).
//!
//! Only as much of Codex's protocol as one turn needs: the WebSocket upgrade,
//! then JSON-RPC — `initialize`, `thread/start` / `thread/resume`, `turn/start`
//! (answered with the turn, then `turn/started`, one `item/agentMessage/delta`
//! carrying the reply and `turn/completed`), `thread/unsubscribe` and
//! `thread/loaded/list`. Like the rest of the doubles it refuses what oneharness
//! would never send — a malformed upgrade, frame, envelope or request, or a
//! method outside that list — by exiting with the reason on stderr, which the
//! engine reports as the server lost under the turn: loud, never a turn that
//! quietly answered something else.
//!
//! Scripted from the double's directory, as every double is:
//!
//! * [`AWAIT_TURNS_FILE`] — a turn count `N`: before completing, a turn waits
//!   until this **process** has started `N` turns, bounded by [`AWAIT_LIMIT`].
//!   Turns served by two different servers can never satisfy it, so turns that
//!   each completed having seen `N` started ran concurrently on one process.
//! * [`TURNS_FILE`] — written, never read: one JSON line per completed turn,
//!   `{"server_pid", "thread", "turns_started"}`, the last being how many turns
//!   this process had started when the turn completed.
//!
//! The server exits on its own once it has served a connection and then held
//! none for [`IDLE_EXIT`], so a suite's servers are gone well before the runner
//! reaps its processes whether or not a sweeper stopped them first.

use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

/// The file in the script directory naming how many turns a turn waits for.
pub const AWAIT_TURNS_FILE: &str = "codex-await-turns";

/// The file in the script directory each completed turn is recorded into.
pub const TURNS_FILE: &str = "codex-turns.jsonl";

/// How long a turn waits for [`AWAIT_TURNS_FILE`]'s count before completing
/// anyway, so a journey whose turns never meet fails on what was recorded
/// rather than hanging.
pub const AWAIT_LIMIT: Duration = Duration::from_secs(60);

/// Long enough that two dispatches started together both connect before the
/// first one's departure stops the server, and short enough that a suite's
/// servers are gone well before the runner reaps its processes.
const IDLE_EXIT: Duration = Duration::from_secs(5);

/// How long the server waits for a first connection before giving up, so a
/// launch nobody dials cannot leak this process onto a runner.
const UNUSED_EXIT: Duration = Duration::from_secs(60);

/// The longest path `sun_path` holds on every unix this suite runs on; a
/// longer address is bound elsewhere and linked, as Codex itself does.
const SUN_PATH_BUDGET: usize = 100;

/// The most one client message may hold. A turn's prompt is the largest thing
/// oneharness sends; a frame past this is refused rather than allocated.
const MESSAGE_LIMIT: u64 = 16 * 1024 * 1024;

/// oneharness's upgrade lines are a few dozen bytes; the bound keeps a client
/// that never sends `\r\n` from growing this process's memory.
const HEADER_LINE_LIMIT: u64 = 8 * 1024;

struct Server {
    dir: PathBuf,
    connections: AtomicUsize,
    served: AtomicUsize,
    turns_started: AtomicUsize,
    next_thread: AtomicUsize,
    /// Every thread a connection opened or rejoined: a turn may only name one.
    threads: Mutex<HashSet<String>>,
}

/// Serve on `listen` (the `unix://PATH` of `--listen`) until idle, recording
/// into `dir`; never returns.
pub fn run(listen: &str, dir: PathBuf) -> ! {
    let path = listen
        .strip_prefix("unix://")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::fail("app-server --listen must name a unix:// path"));
    let listener = bind(&path);
    let server = Arc::new(Server {
        dir,
        connections: AtomicUsize::new(0),
        served: AtomicUsize::new(0),
        turns_started: AtomicUsize::new(0),
        next_thread: AtomicUsize::new(1),
        threads: Mutex::new(HashSet::new()),
    });
    let watcher = Arc::clone(&server);
    std::thread::spawn(move || exit_when_idle(&watcher));
    for stream in listener.incoming().flatten() {
        let server = Arc::clone(&server);
        server.connections.fetch_add(1, Ordering::SeqCst);
        server.served.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            if let Err(why) = serve(&server, stream) {
                crate::fail(&format!("app-server: {why}"));
            }
            server.connections.fetch_sub(1, Ordering::SeqCst);
        });
    }
    std::process::exit(0);
}

fn exit_when_idle(server: &Server) {
    let started = Instant::now();
    let mut idle_since: Option<Instant> = None;
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let served = server.served.load(Ordering::SeqCst) > 0;
        if !served && started.elapsed() >= UNUSED_EXIT {
            std::process::exit(0);
        }
        if !served || server.connections.load(Ordering::SeqCst) > 0 {
            idle_since = None;
            continue;
        }
        let since = *idle_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= IDLE_EXIT {
            std::process::exit(0);
        }
    }
}

/// Bind `path`, or a short stand-in linked from it when `path` is past the
/// `sun_path` budget.
fn bind(path: &Path) -> UnixListener {
    let target = if path.as_os_str().len() >= SUN_PATH_BUDGET {
        let target = PathBuf::from(format!("/tmp/op-codex-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(path);
        if let Err(e) = std::os::unix::fs::symlink(&target, path) {
            crate::fail(&format!("cannot link {}: {e}", path.display()));
        }
        target
    } else {
        path.to_path_buf()
    };
    let _ = std::fs::remove_file(&target);
    UnixListener::bind(&target)
        .unwrap_or_else(|e| crate::fail(&format!("cannot bind {}: {e}", target.display())))
}

type Writer = Arc<Mutex<UnixStream>>;

/// One connection: the upgrade, then every message until the client closes.
/// `Err` is a client that broke the protocol.
fn serve(server: &Server, stream: UnixStream) -> Result<(), String> {
    let writer = stream
        .try_clone()
        .map_err(|e| format!("cannot write to a connection: {e}"))?;
    let writer: Writer = Arc::new(Mutex::new(writer));
    let mut reader = BufReader::new(stream);
    let Some(key) = read_upgrade(&mut reader)? else {
        return Ok(());
    };
    let upgrade = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        oneharness_core::io::websocket::accept_key(&key)
    );
    if lock(&writer).write_all(upgrade.as_bytes()).is_err() {
        return Ok(());
    }
    while let Some(message) = read_message(&mut reader, &writer)? {
        let parsed: Message = serde_json::from_slice(&message).map_err(|e| {
            format!(
                "not a JSON-RPC 2.0 message oneharness sends ({e}): {}",
                String::from_utf8_lossy(&message)
            )
        })?;
        handle(server, &writer, parsed)?;
    }
    Ok(())
}

/// The client's upgrade request, checked as RFC 6455 §4.2.1 asks of a server:
/// a `GET`, `Upgrade: websocket`, a `Connection` naming `Upgrade`, version 13
/// and a 16-byte key. `Ok(None)` when the client left before finishing it.
fn read_upgrade(reader: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        let read = reader
            .by_ref()
            .take(HEADER_LINE_LIMIT)
            .read_line(&mut line)
            .map_err(|e| format!("unreadable upgrade request: {e}"))?;
        if read == 0 {
            return Ok(None);
        }
        if !line.ends_with("\r\n") {
            return Err(format!("upgrade request line is unterminated: {line:?}"));
        }
        if line == "\r\n" {
            break;
        }
        lines.push(line.trim_end().to_string());
    }
    let request = lines.first().map(String::as_str).unwrap_or_default();
    if !(request.starts_with("GET ") && request.ends_with(" HTTP/1.1")) {
        return Err(format!("not a WebSocket upgrade request: {request:?}"));
    }
    let header = |wanted: &str| {
        lines.iter().skip(1).find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case(wanted)
                .then(|| value.trim().to_string())
        })
    };
    let upgrade = header("upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let connection = header("connection").is_some_and(|v| {
        v.split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
    });
    let version = header("sec-websocket-version").as_deref() == Some("13");
    if !(upgrade && connection && version) {
        return Err(format!(
            "the upgrade request is missing a required header: {lines:?}"
        ));
    }
    let key = header("sec-websocket-key")
        .ok_or_else(|| format!("the upgrade request carries no Sec-WebSocket-Key: {lines:?}"))?;
    // RFC 6455's nonce is 16 bytes, so its base64 is 22 characters and `==`.
    let nonce = key.len() == 24
        && key.ends_with("==")
        && key[..22]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/');
    if !nonce {
        return Err(format!(
            "Sec-WebSocket-Key {key:?} is not 16 bytes of base64"
        ));
    }
    Ok(Some(key))
}

fn lock(writer: &Writer) -> std::sync::MutexGuard<'_, UnixStream> {
    writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_frame(writer: &Writer, opcode: u8, payload: &[u8]) {
    let mut bytes = vec![0x80 | opcode];
    match payload.len() {
        len if len < 126 => bytes.push(len as u8),
        len if len <= usize::from(u16::MAX) => {
            bytes.push(126);
            bytes.extend_from_slice(&(len as u16).to_be_bytes());
        }
        len => {
            bytes.push(127);
            bytes.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    bytes.extend_from_slice(payload);
    let _ = lock(writer).write_all(&bytes);
}

fn send(writer: &Writer, frame: &Value) {
    write_frame(writer, 0x1, frame.to_string().as_bytes());
}

/// The next whole text message from the client, unmasked; `Ok(None)` once it
/// is gone or closes. A frame RFC 6455 forbids a client to send — unmasked,
/// reserved bits set, an unknown or binary opcode, a fragmented or oversized
/// control frame, a continuation with nothing to continue — or a message past
/// [`MESSAGE_LIMIT`] is `Err`.
fn read_message(stream: &mut impl Read, writer: &Writer) -> Result<Option<Vec<u8>>, String> {
    let mut message: Option<Vec<u8>> = None;
    loop {
        let mut head = [0u8; 2];
        if stream.read_exact(&mut head).is_err() {
            return Ok(None);
        }
        let (fin, opcode, masked) = (head[0] & 0x80 != 0, head[0] & 0x0f, head[1] & 0x80 != 0);
        if head[0] & 0x70 != 0 {
            return Err(format!("a frame set reserved bits: {:#04x}", head[0]));
        }
        if !masked {
            return Err("a client frame arrived unmasked".to_string());
        }
        let len = match head[1] & 0x7f {
            126 => {
                let mut ext = [0u8; 2];
                if stream.read_exact(&mut ext).is_err() {
                    return Ok(None);
                }
                u64::from(u16::from_be_bytes(ext))
            }
            127 => {
                let mut ext = [0u8; 8];
                if stream.read_exact(&mut ext).is_err() {
                    return Ok(None);
                }
                u64::from_be_bytes(ext)
            }
            short => u64::from(short),
        };
        let control = opcode & 0x08 != 0;
        if control && (!fin || len > 125) {
            return Err(format!(
                "control frame {opcode:#x} is fragmented or past 125 bytes ({len})"
            ));
        }
        let held = message.as_ref().map_or(0, Vec::len) as u64;
        if len.saturating_add(held) > MESSAGE_LIMIT {
            return Err(format!("a message past {MESSAGE_LIMIT} bytes ({len} more)"));
        }
        let mut mask = [0u8; 4];
        if stream.read_exact(&mut mask).is_err() {
            return Ok(None);
        }
        let mut payload = vec![0u8; usize::try_from(len).map_err(|e| e.to_string())?];
        if stream.read_exact(&mut payload).is_err() {
            return Ok(None);
        }
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
        match (opcode, message.as_mut()) {
            (0x8, _) => return Ok(None),
            (0x9, _) => {
                write_frame(writer, 0xA, &payload);
                continue;
            }
            (0xA, _) => continue,
            (0x1, None) => message = Some(payload),
            (0x0, Some(held)) => held.extend_from_slice(&payload),
            (0x0, None) => return Err("a continuation frame continues no message".to_string()),
            (0x1, Some(_)) => {
                return Err("a text frame arrived inside a fragmented message".to_string())
            }
            (other, _) => return Err(format!("opcode {other:#x} is not one oneharness sends")),
        }
        if fin {
            return Ok(message.take());
        }
    }
}

/// One JSON-RPC 2.0 message from the client, typed at the frame: this server
/// sends no requests of its own, so every client message is a request with an
/// `id` or the one notification, and its `params` are the shape that method's
/// sender — oneharness-core's Codex dialogue — builds, field for field.
#[derive(Deserialize)]
#[serde(try_from = "RawEnvelope")]
enum Message {
    Request {
        id: u64,
        call: Call,
    },
    /// `initialized`, which oneharness sends after `initialize` with `{}`.
    Initialized,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnvelope {
    jsonrpc: JsonRpcVersion,
    #[serde(default)]
    id: Option<u64>,
    method: String,
    params: Value,
}

#[derive(Deserialize)]
enum JsonRpcVersion {
    #[serde(rename = "2.0")]
    V2,
}

/// Every request this server answers. A method outside it is refused, so a
/// request oneharness started sending fails a journey rather than being
/// answered.
#[derive(Deserialize)]
#[serde(tag = "method", content = "params")]
enum Call {
    #[serde(rename = "initialize")]
    Initialize(InitializeParams),
    #[serde(rename = "thread/start")]
    ThreadStart(ThreadParams),
    #[serde(rename = "thread/resume")]
    ThreadResume(ResumeParams),
    #[serde(rename = "turn/start")]
    TurnStart(TurnParams),
    #[serde(rename = "thread/unsubscribe")]
    ThreadUnsubscribe(ThreadRef),
    #[serde(rename = "thread/loaded/list")]
    ThreadLoadedList(Empty),
}

impl TryFrom<RawEnvelope> for Message {
    type Error = String;

    fn try_from(raw: RawEnvelope) -> Result<Self, String> {
        let RawEnvelope {
            jsonrpc: JsonRpcVersion::V2,
            id,
            method,
            params,
        } = raw;
        match id {
            None if method == "initialized" => {
                serde_json::from_value::<Empty>(params)
                    .map_err(|e| format!("initialized params: {e}"))?;
                Ok(Message::Initialized)
            }
            None => Err(format!("`{method}` is not a notification oneharness sends")),
            Some(id) => {
                let call = serde_json::from_value(json!({"method": &method, "params": params}))
                    .map_err(|e| format!("`{method}` is not a request oneharness sends: {e}"))?;
                Ok(Message::Request { id, call })
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct InitializeParams {
    client_info: ClientInfo,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientInfo {
    name: String,
    #[serde(rename = "title")]
    _title: String,
    #[serde(rename = "version")]
    _version: String,
}

/// Codex's approval spelling; oneharness runs headless, so only `never`.
#[derive(Deserialize)]
enum ApprovalPolicy {
    #[serde(rename = "never")]
    Never,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ThreadParams {
    #[serde(rename = "cwd")]
    _cwd: String,
    #[serde(rename = "approvalPolicy")]
    _approval_policy: ApprovalPolicy,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, rename = "config")]
    _config: Option<ThreadConfig>,
}

/// The per-thread environment a pooled dispatch carries.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThreadConfig {
    #[serde(rename = "shell_environment_policy")]
    _shell_environment_policy: ShellEnvironmentPolicy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellEnvironmentPolicy {
    #[serde(rename = "set")]
    _set: BTreeMap<String, String>,
    #[serde(rename = "exclude")]
    _exclude: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ResumeParams {
    thread_id: String,
    #[serde(rename = "cwd")]
    _cwd: String,
    #[serde(rename = "approvalPolicy")]
    _approval_policy: ApprovalPolicy,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, rename = "config")]
    _config: Option<ThreadConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct TurnParams {
    thread_id: String,
    #[serde(rename = "cwd")]
    _cwd: String,
    input: Vec<InputItem>,
    #[serde(rename = "approvalPolicy")]
    _approval_policy: ApprovalPolicy,
    #[serde(rename = "sandboxPolicy")]
    _sandbox_policy: SandboxPolicy,
    #[serde(default, rename = "model")]
    _model: Option<String>,
}

/// A turn's input item: oneharness sends its prompt as text and nothing else.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum InputItem {
    Text {
        #[serde(rename = "text")]
        _text: String,
    },
}

/// The sandbox each permission mode maps onto, in Codex's own spelling.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum SandboxPolicy {
    DangerFullAccess,
    ReadOnly,
    #[serde(rename_all = "camelCase")]
    WorkspaceWrite {
        #[serde(rename = "writableRoots")]
        _writable_roots: Vec<String>,
        #[serde(rename = "networkAccess")]
        _network_access: bool,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ThreadRef {
    thread_id: String,
}

fn threads(server: &Server) -> std::sync::MutexGuard<'_, HashSet<String>> {
    server
        .threads
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn handle(server: &Server, writer: &Writer, message: Message) -> Result<(), String> {
    let Message::Request { id, call } = message else {
        return Ok(());
    };
    let reply = |result: Value| {
        send(
            writer,
            &json!({"jsonrpc": "2.0", "id": id, "result": result}),
        );
    };
    match call {
        Call::Initialize(InitializeParams { client_info }) => reply(json!({
            "userAgent": format!("onepipeline-fake-codex-app-server (for {})", client_info.name)
        })),
        Call::ThreadStart(ThreadParams { model, .. }) => {
            let thread = format!(
                "fake-thread-{}-{}",
                std::process::id(),
                server.next_thread.fetch_add(1, Ordering::SeqCst)
            );
            reply(opened(server, thread, model));
        }
        Call::ThreadResume(ResumeParams {
            thread_id, model, ..
        }) => {
            reply(opened(server, thread_id, model));
        }
        Call::TurnStart(params) => turn(server, writer, params, reply)?,
        Call::ThreadUnsubscribe(ThreadRef { thread_id }) => {
            let held = threads(server).remove(&thread_id);
            reply(json!({"status": if held { "unsubscribed" } else { "notLoaded" }}));
        }
        Call::ThreadLoadedList(Empty {}) => {
            let held: Vec<String> = threads(server).iter().cloned().collect();
            reply(json!({"data": held, "nextCursor": null}));
        }
    }
    Ok(())
}

/// Hold `thread` open and answer for it under the model the client asked for.
fn opened(server: &Server, thread: String, model: Option<String>) -> Value {
    threads(server).insert(thread.clone());
    let mut result = json!({"thread": {"id": thread}});
    if let Some(model) = model {
        result["model"] = json!(model);
    }
    result
}

fn awaited(dir: &Path) -> Result<usize, String> {
    match std::fs::read_to_string(dir.join(AWAIT_TURNS_FILE)) {
        Ok(text) => text
            .trim()
            .parse()
            .map_err(|e| format!("{AWAIT_TURNS_FILE} holds no turn count ({text:?}): {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(format!("cannot read {AWAIT_TURNS_FILE}: {e}")),
    }
}

fn turn(
    server: &Server,
    writer: &Writer,
    params: TurnParams,
    reply: impl FnOnce(Value),
) -> Result<(), String> {
    let TurnParams {
        thread_id, input, ..
    } = params;
    if !threads(server).contains(&thread_id) {
        return Err(format!(
            "turn/start names thread {thread_id}, which no client opened"
        ));
    }
    if input.is_empty() {
        return Err(format!("turn/start on {thread_id} carries no input"));
    }
    let started = server.turns_started.fetch_add(1, Ordering::SeqCst) + 1;
    let turn_id = format!("fake-turn-{}-{started}", std::process::id());
    reply(json!({"turn": {"id": turn_id, "status": "inProgress"}}));
    send(
        writer,
        &json!({"jsonrpc": "2.0", "method": "turn/started",
            "params": {"threadId": thread_id, "turn": {"id": turn_id}}}),
    );
    let wanted = awaited(&server.dir)?;
    let until = Instant::now() + AWAIT_LIMIT;
    while server.turns_started.load(Ordering::SeqCst) < wanted && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    let line = json!({
        "server_pid": std::process::id(),
        "thread": thread_id,
        "turns_started": server.turns_started.load(Ordering::SeqCst),
    });
    crate::append(&server.dir.join(TURNS_FILE), &line.to_string());
    send(
        writer,
        &json!({"jsonrpc": "2.0", "method": "item/agentMessage/delta",
            "params": {"threadId": thread_id, "turnId": turn_id, "itemId": "item-0", "delta": "ok"}}),
    );
    send(
        writer,
        &json!({"jsonrpc": "2.0", "method": "turn/completed",
            "params": {"threadId": thread_id, "turn": {"id": turn_id, "status": "completed"}}}),
    );
    Ok(())
}
