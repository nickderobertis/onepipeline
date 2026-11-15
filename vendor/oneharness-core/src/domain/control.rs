//! Out-of-band turn control: the pure half.
//!
//! A dispatched turn runs for many minutes, and until now the only lever a
//! supervisor had over one that went the wrong way was to kill the dispatch —
//! losing the whole turn and its session. `oneharness run --control` opens a
//! unix socket for the run's lifetime; a *separate* `oneharness interrupt
//! --session <NAME>` process resolves that socket and asks the run to abort the
//! current turn while keeping the session alive.
//!
//! An interrupt may also carry a **redirection** ([`RedirectInput`]): the user
//! message that says what to do instead. Stopping alone only costs the turn —
//! the supervisor still has to start a fresh dispatch to say anything — so the
//! two travel as one request.
//!
//! *Atomic* here means **committed with the abort, delivered at the turn
//! boundary**, and that is a deliberate design rather than a convenience. Every
//! one of these protocols drops a message sent into a turn already in flight:
//! Claude Code silently discards a `user` frame mid-turn (verified live), codex
//! and ACP refuse a second turn on a thread that has one, and both HTTP servers
//! queue against a session they are still running. So a supervisor doing this by
//! hand *has* to stop, wait, and then send — and every one of those waits is a
//! window where the turn is dead and the message is nobody's. Instead the run
//! takes ownership of the message in the same operation that aborts the turn:
//! it is parked before the abort is delivered, released again if the abort
//! fails, and otherwise written by the run itself the moment the turn ends. The
//! answer waits for that write: `redirected: true` means the message IS the
//! next turn's prompt, and a turn that ends — or a run that exits — before it
//! could be written answers a refusal saying it was not delivered, after which
//! the run never writes it. A supervisor that reads `ok` is holding a
//! guarantee, not a race it won.
//!
//! This module holds everything with no I/O in it: the wire frames, the
//! per-harness capability shapes, the sidecar-server declaration and its pool
//! key, the harness-specific stdin frames, and the report block. The socket,
//! the process lifetimes, and the pool's disk state live in
//! [`crate::io::control`] and [`crate::io::server_pool`].

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use crate::domain::history::sanitize_name;
use crate::domain::report::OutputFormat;
use crate::domain::usage::UtcInstant;

/// The control protocol version carried in every frame's `v` field.
///
/// It exists so a later verb can be added without breaking a supervisor pinned
/// to today's shape: `interrupt` is the only verb now, but some harnesses can
/// also *steer* a turn without ending it (codex `turn/steer`, opencode
/// `delivery:"steer"`), which is deliberately out of scope here.
///
/// **v2** added the interrupt's optional `input` — the redirection delivered
/// with the abort — and the answer's `redirected` flag. Both directions are
/// version-checked on the way in, so a v1 supervisor talking to a v2 run (or the
/// reverse) is told which side speaks what rather than having a field it does
/// not understand silently ignored.
pub const PROTOCOL_VERSION: u32 = 2;

/// The directory (under the session store) holding one socket per named run.
pub const CONTROL_DIR: &str = "control";

/// A filesystem path that is known to be absolute.
///
/// Control addresses are handed to a *different process* — the socket path in
/// the report is how a supervisor finds the run — so a relative path is not a
/// smaller version of the right answer, it is a different file depending on who
/// reads it. The type is the difference between "we documented it as absolute"
/// and "it is".
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] The generated SDK
// contract is a plain string on purpose: this appears only in *output* (the
// report's `control.socket`), which oneharness produces and the SDKs validate
// leniently so a reader never rejects a report the CLI just emitted. There is
// also no portable JSON Schema for "absolute" — the rule differs between
// `/x` and `C:\x` — so a pattern would encode one platform's answer as the
// contract's.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct AbsolutePath(PathBuf);

impl AbsolutePath {
    /// Wrap `path`, or say why it cannot be an address.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        if path.is_absolute() {
            Ok(AbsolutePath(path))
        } else {
            Err(format!("`{}` is not an absolute path", path.display()))
        }
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// The text a unix-shaped fixture path takes to be absolute on this host.
///
/// Windows needs a prefix before a path is absolute, so a bare `/work` is not
/// portable: [`AbsolutePath::new`] refuses it there and every fixture built on
/// it panics. Tests assert against this rather than the literal so the wire
/// text they check is the one this host would really send.
#[cfg(test)]
pub(crate) fn absolute_text_for_test(path: &str) -> String {
    if cfg!(windows) {
        format!("C:{path}")
    } else {
        path.to_string()
    }
}

/// [`absolute_text_for_test`] as the wrapped type.
#[cfg(test)]
pub(crate) fn absolute_for_test(path: &str) -> AbsolutePath {
    AbsolutePath::new(absolute_text_for_test(path)).expect("fixture path is absolute on this host")
}

impl std::fmt::Display for AbsolutePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl<'de> Deserialize<'de> for AbsolutePath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        AbsolutePath::new(PathBuf::from(String::deserialize(deserializer)?))
            .map_err(D::Error::custom)
    }
}

/// How a harness accepts an out-of-band interrupt for an in-flight turn.
///
/// Registry data on [`crate::domain::harness::HarnessSpec::control`], sourced
/// from a live interrupt against the real CLI — never guessed. `None` there
/// means `oneharness interrupt` is a loud usage error for the harness, never a
/// silent no-op: a supervisor that is told "ok" while the turn keeps running is
/// worse off than one told the lever does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ControlShape {
    /// Claude Code: the control frame rides the run process's **own stdin**
    /// (`-p --input-format stream-json`), so no sidecar server is needed. A
    /// `{"type":"control_request",…,"request":{"subtype":"interrupt"}}` line
    /// aborts the turn with a `result` document; the session survives and a
    /// later turn continues it with full context. Verified live: a plain user
    /// message written mid-turn is *silently dropped*, so the control frame is
    /// the only mechanism that works.
    ClaudeControlRequest,
    /// Codex: `turn/interrupt {threadId,turnId}` over the `codex app-server`
    /// JSON-RPC stdio protocol, which oneharness spawns as the run's own child
    /// and drives the thread/turn lifecycle on — so the interrupt rides the same
    /// open stdin and nothing is shared or pooled.
    CodexAppServer,
    /// OpenCode: `POST /api/session/{id}/interrupt` against `opencode serve`.
    OpencodeHttp,
    /// Goose and Copilot: the ACP `session/cancel` **notification** (no `id`)
    /// over their JSON-RPC stdio servers.
    AcpCancel,
    /// Crush: `POST /v1/workspaces/{id}/agent/sessions/{sid}/cancel` against
    /// `crush server`.
    CrushHttp,
}

impl ControlShape {
    /// Every declared mechanism, so a grid over the set cannot quietly omit one
    /// a later release adds.
    ///
    /// Hand-written, and therefore drift-gated against the enum's OWN generated
    /// schema (`every_declared_mechanism_appears_in_all`) — schemars derives
    /// that variant list from the enum rather than from this array, so a variant
    /// added without being listed here fails a test instead of dropping out of
    /// every grid that iterates it.
    pub const ALL: [ControlShape; 5] = [
        ControlShape::ClaudeControlRequest,
        ControlShape::CodexAppServer,
        ControlShape::OpencodeHttp,
        ControlShape::AcpCancel,
        ControlShape::CrushHttp,
    ];

    /// The stable wire spelling reported as a served interrupt's `mechanism`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ControlShape::ClaudeControlRequest => "claude-control-request",
            ControlShape::CodexAppServer => "codex-app-server",
            ControlShape::OpencodeHttp => "opencode-http",
            ControlShape::AcpCancel => "acp-cancel",
            ControlShape::CrushHttp => "crush-http",
        }
    }

    /// The shape a wire `mechanism` names, or `None` for one this build does not
    /// know — a supervisor reading an unrecognized mechanism gets a loud parse
    /// failure rather than a silently-degraded frame.
    #[must_use]
    pub fn from_wire(mechanism: &str) -> Option<ControlShape> {
        ControlShape::ALL
            .into_iter()
            .find(|shape| shape.as_str() == mechanism)
    }

    /// Whether oneharness *drives the turn itself* over this mechanism's own
    /// protocol, rather than riding the harness's ordinary headless run.
    ///
    /// True for everything except Claude Code, whose control frame rides the
    /// same `-p` run a plain dispatch uses. A driven turn negotiates its model,
    /// working directory and approvals on the wire, so none of them appear on
    /// the argv and the harness's stdout format has no bearing on its session id.
    #[must_use]
    pub fn drives_turn(self) -> bool {
        !matches!(self, ControlShape::ClaudeControlRequest)
    }

    /// The client request this mechanism opens a turn on an **existing**
    /// conversation with, or `None` when its protocol has none oneharness
    /// implements.
    ///
    /// Registry-grade protocol data, read out of the CLI's own generated schema
    /// rather than guessed: codex's `ClientRequest` declares `thread/resume`,
    /// whose `ThreadResumeParams` requires `threadId` and accepts the same
    /// `cwd`/`model`/`approvalPolicy` `thread/start` does, and whose response
    /// carries the same required `thread` field — so one arm reads the opened
    /// conversation's id either way.
    ///
    /// `None` is what makes a caller-owned `--session` handle **inexpressible**
    /// on a mechanism that drives its own turn ([`Self::carries_session`]): the
    /// turn is negotiated on the wire, so the only way to continue one
    /// conversation is to ask the protocol for it, and a mechanism that cannot
    /// be asked would open a new conversation every turn while the store looked
    /// healthy.
    #[must_use]
    pub fn resume_request(self) -> Option<&'static str> {
        match self {
            ControlShape::CodexAppServer => Some("thread/resume"),
            ControlShape::ClaudeControlRequest
            | ControlShape::OpencodeHttp
            | ControlShape::AcpCancel
            | ControlShape::CrushHttp => None,
        }
    }

    /// Whether a named `--session` handle can be **continued** over this
    /// mechanism.
    ///
    /// True for a mechanism that does not drive the turn at all (Claude Code's
    /// control frame rides the ordinary `-p` run, whose `--resume` argv carries
    /// the handle exactly as it does without `--control`), and for a driven turn
    /// whose protocol has a [`Self::resume_request`]. False otherwise — and
    /// there a continue is a loud usage error rather than a silent fresh start.
    #[must_use]
    pub fn carries_session(self) -> bool {
        !self.drives_turn() || self.resume_request().is_some()
    }

    /// Whether this mechanism needs a **shared, long-lived** server process,
    /// declared as a [`ServerSpec`] and managed by the pool.
    ///
    /// The stdio protocols do not: oneharness spawns the server as the run's own
    /// child, so the interrupt rides the same stdin the turn does and the
    /// process dies with the dispatch. Only the HTTP mechanisms need a server
    /// that outlives one turn and is worth sharing across dispatches.
    #[must_use]
    pub fn needs_pooled_server(self) -> bool {
        matches!(self, ControlShape::OpencodeHttp | ControlShape::CrushHttp)
    }

    /// The output format a control-enabled run must use, when the mechanism
    /// pins one. Claude Code refuses the combination outright —
    /// "`--input-format=stream-json` requires output-format=stream-json" — so
    /// oneharness selects it, and turns a conflicting explicit `--output-format`
    /// into a usage error rather than a spawn failure the caller has to decode.
    /// `None` for a server-backed mechanism, whose turn does not go through the
    /// harness CLI's own output at all.
    #[must_use]
    pub fn required_format(self) -> Option<OutputFormat> {
        match self {
            ControlShape::ClaudeControlRequest => Some(OutputFormat::StreamJson),
            _ => None,
        }
    }
}

/// How a sidecar server is reached once launched — the *declaration*, on
/// [`ServerSpec`]. The running server's actual address is [`ServerAddress`],
/// which carries the transport and its coordinates together so the two can
/// never disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ServerTransport {
    /// JSON-RPC over the server process's stdin/stdout (codex app-server, ACP).
    Stdio,
    /// HTTP over a unix domain socket the server binds (crush).
    UnixSocket,
    /// HTTP over a loopback TCP port the server binds (opencode).
    Tcp,
}

/// A TCP port something can actually be dialed on.
///
/// `0` is the kernel's "pick one for me" sentinel, never an address: a record
/// that lost its port and kept a `0` would send every later interrupt to
/// whatever happens to answer, which is the one outcome worse than sending none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Port(u16);

impl Port {
    /// Accept `raw` as a dialable port, or say why it cannot be one.
    pub fn new(raw: u16) -> Result<Self, String> {
        if raw == 0 {
            Err("0 is not a dialable port".to_string())
        } else {
            Ok(Port(raw))
        }
    }

    #[must_use]
    pub fn get(self) -> u16 {
        self.0
    }
}

impl<'de> Deserialize<'de> for Port {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        Port::new(u16::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Where a *running* sidecar server actually is.
///
/// One value rather than a transport tag beside a loose address string: a
/// stdio server has no address at all, and a TCP server's port is meaningless
/// to a unix-socket reader, so pairing them separately would make
/// "`Stdio` with a port" and "`Tcp` with a socket path" representable states a
/// reader would have to defend against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "kebab-case")]
pub enum ServerAddress {
    /// The transport is the process's own pipes; there is nothing to dial.
    Stdio,
    /// A unix domain socket the server bound.
    UnixSocket { path: AbsolutePath },
    /// A loopback TCP port the server bound. Loopback by construction: a
    /// control server is a private lever over a running agent, so an address
    /// reachable from anywhere else is not a variant worth being able to spell.
    Tcp { port: Port },
}

impl ServerAddress {
    /// The transport this address speaks.
    #[must_use]
    pub fn transport(&self) -> ServerTransport {
        match self {
            ServerAddress::Stdio => ServerTransport::Stdio,
            ServerAddress::UnixSocket { .. } => ServerTransport::UnixSocket,
            ServerAddress::Tcp { .. } => ServerTransport::Tcp,
        }
    }
}

/// A [`ServerAddress`] a socket can actually be opened to.
///
/// The dialable subset, as its own type, because [`ServerAddress::Stdio`] names
/// a server reached over pipes its parent already holds — there is no address
/// there for anyone to connect to. A client built from the general type has to
/// carry that impossibility to dial time and fail there, which is a socket error
/// raised on a value that was never dialable in the first place; taking this
/// type instead means such a client cannot be constructed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialAddress {
    /// A unix domain socket the server bound.
    UnixSocket { path: AbsolutePath },
    /// A loopback TCP port the server bound.
    Tcp { port: Port },
}

impl DialAddress {
    /// The transport this address speaks — never [`ServerTransport::Stdio`],
    /// which is the whole point of the type.
    #[must_use]
    pub fn transport(&self) -> ServerTransport {
        match self {
            DialAddress::UnixSocket { .. } => ServerTransport::UnixSocket,
            DialAddress::Tcp { .. } => ServerTransport::Tcp,
        }
    }
}

/// Why an address cannot be dialed.
///
/// Its own error rather than a bare `()`, so the one caller that turns it into
/// an `io::Error` says the same thing everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotDialable;

impl std::fmt::Display for NotDialable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stdio server has no address to dial")
    }
}

impl std::error::Error for NotDialable {}

impl TryFrom<ServerAddress> for DialAddress {
    type Error = NotDialable;

    fn try_from(address: ServerAddress) -> Result<Self, NotDialable> {
        match address {
            ServerAddress::UnixSocket { path } => Ok(DialAddress::UnixSocket { path }),
            ServerAddress::Tcp { port } => Ok(DialAddress::Tcp { port }),
            ServerAddress::Stdio => Err(NotDialable),
        }
    }
}

impl From<DialAddress> for ServerAddress {
    fn from(address: DialAddress) -> Self {
        match address {
            DialAddress::UnixSocket { path } => ServerAddress::UnixSocket { path },
            DialAddress::Tcp { port } => ServerAddress::Tcp { port },
        }
    }
}

/// A harness's sidecar server: how to launch it, what makes two launches
/// interchangeable, and how it is reached.
///
/// Declared per harness rather than special-cased. Only the two HTTP mechanisms
/// (opencode, crush) need one: their turn is submitted to a server that outlives
/// the dispatch, so it is pooled. The stdio protocols spawn their server as the
/// run's OWN child and Claude Code needs none at all, so both leave this `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerSpec {
    /// The argv appended to the harness binary to start the server (e.g.
    /// `["serve"]`, `["app-server"]`, `["acp"]`). Never includes the binary.
    pub launch: &'static [&'static str],
    /// How the chosen address reaches the server's argv, with `{address}`
    /// standing for the port or socket path (`["--port", "{address}"]`,
    /// `["-H", "unix://{address}"]`). Separate from `launch` because the
    /// address is picked per pool entry, not declared: two dispatches sharing a
    /// key must reach the SAME running server, so the address can never be part
    /// of what makes their keys differ.
    pub address_args: &'static [&'static str],
    /// The environment variables whose *values* make two servers different
    /// state (codex's `CODEX_HOME`). Per-turn and per-thread settings — model,
    /// effort, cwd, sandbox/approval policy, instructions — are deliberately
    /// absent: they are negotiated per thread or per turn, so widening the key
    /// with them would start a fresh ~137MB process per dispatch.
    pub key_env: &'static [&'static str],
    pub transport: ServerTransport,
}

/// The identity two dispatches must share to reuse one sidecar server.
///
/// A validated value rather than a loose string: the pool joins it straight
/// into a filesystem path, so a key carrying a separator, `..`, or `.` would
/// name a directory other than an entry under the pool root. Constructed by
/// [`pool_key`], or parsed from text with [`PoolKey::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PoolKey(String);

impl PoolKey {
    /// Accept `text` as a key, or say why it cannot be one.
    pub fn parse(text: &str) -> Result<Self, String> {
        if is_pool_key(text) {
            Ok(PoolKey(text.to_string()))
        } else {
            Err(format!(
                "`{text}` is not a valid pool key (it must be a single path segment of \
                 alphanumerics, `-`, or `_`, at most 128 characters)"
            ))
        }
    }

    /// The key's text, which is also its directory name under the pool root.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PoolKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl JsonSchema for PoolKey {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("PoolKey")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // Held to `is_pool_key` by `the_pool_key_schema_accepts_exactly_what_parse_does`.
        schemars::json_schema!({
            "type": "string",
            "minLength": 1,
            "maxLength": POOL_KEY_MAX_LEN,
            "pattern": "^[A-Za-z0-9_-]+$",
        })
    }
}

impl<'de> Deserialize<'de> for PoolKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        PoolKey::parse(&String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// The key for `harness_id` under the given resolved `key_env` values and
/// launch overrides.
///
/// Pure string arithmetic — so the same key is computed identically by
/// independent processes, which is what makes the on-disk pool work at all.
/// Infallible: the digest covers every input including the id, so a caller
/// passing something unusable as a directory name gets a different key rather
/// than an error it has nothing useful to do with.
#[must_use]
pub fn pool_key(
    harness_id: &str,
    key_env: &[(String, Option<String>)],
    launch_overrides: &[String],
) -> PoolKey {
    let mut material = String::from(harness_id);
    // Sort so two dispatches that resolved the same variables in a different
    // order still land on one server.
    let mut env: Vec<&(String, Option<String>)> = key_env.iter().collect();
    env.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, value) in env {
        material.push('\n');
        material.push_str(name);
        material.push('=');
        material.push_str(value.as_deref().unwrap_or(""));
    }
    for arg in launch_overrides {
        material.push('\n');
        material.push_str(arg);
    }
    // The prefix is only there to make a pool directory readable; the digest is
    // what makes the key unique, and it already covers the untouched id. So the
    // prefix is reduced to what a directory name can hold rather than refused.
    let prefix: String = harness_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    PoolKey(format!("{prefix}-{:016x}", fnv1a64(material.as_bytes())))
}

/// The longest pool key, in its schema and at runtime alike.
const POOL_KEY_MAX_LEN: usize = 128;

/// Whether `key` is shaped like a [`pool_key`] result — the single rule behind
/// [`PoolKey`].
///
/// `.` is excluded rather than merely guarded against `..`: [`pool_key`] never
/// emits one (its prefix filter keeps alphanumerics, `-`, and `_`, and the
/// digest is hex), so accepting one could only admit a segment that names a
/// directory other than an entry — `.` is the pool root itself, which a
/// dispatch would then lease and reclaim wholesale. Excluding the character
/// makes both `.` and `..` unrepresentable, so no separate traversal check is
/// needed.
fn is_pool_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= POOL_KEY_MAX_LEN
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// FNV-1a, 64-bit. A tiny, dependency-free, *stable* digest — `DefaultHasher`
/// is explicitly not guaranteed stable across Rust releases, and a pool key
/// that changes under a toolchain upgrade would orphan every live server.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// The control verb a supervisor sends. Only `interrupt` exists today; the
/// frame's `v` is what leaves room for `steer` later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ControlVerb {
    Interrupt,
}

impl ControlVerb {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ControlVerb::Interrupt => "interrupt",
        }
    }
}

/// The most characters a redirection may carry.
///
/// Code points, like every other bound in this codebase that a JSON Schema also
/// has to express. It is a ceiling on a *message a person wrote*, not on a
/// document: generous for a redirection, and small enough that the encoded frame
/// stays well inside the 64 KiB bound both ends of the socket read against, so a
/// redirect that was accepted here can never be one the peer refuses unread.
pub const MAX_REDIRECT_INPUT_CHARS: usize = 8 * 1024;

/// The user message an interrupt delivers with the abort.
///
/// A validated type rather than a `String` because the text is spliced into
/// *another program's* protocol frame — Claude Code's stdin message stream, a
/// JSON-RPC `turn/start`, an HTTP prompt body. The three rules below are what
/// separate "a redirection" from "bytes that reached a harness": it must say
/// something, it must fit the frame the peer will read, and it must not carry
/// characters that are not message text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct RedirectInput(String);

impl RedirectInput {
    /// Accept `raw` as a redirection, or say why it cannot be one.
    pub fn new(raw: impl Into<String>) -> Result<Self, String> {
        let raw = raw.into();
        // Blank is not a smaller redirection: it would abort the turn and hand
        // the agent nothing, which is the plain interrupt wearing a flag.
        if raw.trim().is_empty() {
            return Err(
                "the interrupt's `input` is blank, so it redirects the turn at nothing \
                        (send the interrupt without `--input` to just stop it)"
                    .to_string(),
            );
        }
        let chars = raw.chars().count();
        if chars > MAX_REDIRECT_INPUT_CHARS {
            return Err(format!(
                "the interrupt's `input` is {chars} characters, past the \
                 {MAX_REDIRECT_INPUT_CHARS} a redirection may carry"
            ));
        }
        // Newline, tab and carriage return are message text — a person pasting a
        // paragraph sends them. Every other control character (C0, DEL, C1) is
        // not: it reaches a harness inside a protocol frame, where a NUL or an
        // escape sequence is something other than what was typed.
        if let Some(bad) = raw
            .chars()
            .find(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err(format!(
                "the interrupt's `input` carries the control character U+{:04X}, which is not \
                 message text",
                bad as u32
            ));
        }
        Ok(RedirectInput(raw))
    }

    /// The message, exactly as it was written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RedirectInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RedirectInput {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        RedirectInput::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// One newline-terminated request frame: `{"v":2,"verb":"interrupt"}`, or
/// `{"v":2,"verb":"interrupt","input":"…"}` to redirect rather than only stop.
///
/// The version is not a field a caller sets: a `ControlRequest` that exists is
/// one this build speaks, so an unsupported `v` is rejected while parsing
/// rather than travelling as a value every reader must re-check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ControlRequest {
    v: u32,
    verb: ControlVerb,
    /// The redirection to deliver with the abort. Absent is a plain stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input: Option<RedirectInput>,
}

impl ControlRequest {
    /// Stop the turn and hand the agent nothing further.
    #[must_use]
    pub fn interrupt() -> Self {
        ControlRequest {
            v: PROTOCOL_VERSION,
            verb: ControlVerb::Interrupt,
            input: None,
        }
    }

    /// Stop the turn and deliver `input` in the same operation.
    #[must_use]
    pub fn redirect(input: RedirectInput) -> Self {
        ControlRequest {
            v: PROTOCOL_VERSION,
            verb: ControlVerb::Interrupt,
            input: Some(input),
        }
    }

    /// The verb requested.
    #[must_use]
    pub fn verb(&self) -> ControlVerb {
        self.verb
    }

    /// The redirection this request carries, or `None` for a plain stop.
    #[must_use]
    pub fn input(&self) -> Option<&RedirectInput> {
        self.input.as_ref()
    }
}

impl<'de> Deserialize<'de> for ControlRequest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        // llmlint: ignore[invalid_states_unrepresentable] This is the literal
        // untrusted wire frame, whose arbitrary `v` is exactly what a peer can
        // send; it exists for one step so the version can be refused before a
        // `ControlRequest` — which cannot hold an unsupported version — is built.
        #[derive(Deserialize)]
        struct RequestWire {
            v: u32,
            verb: ControlVerb,
            #[serde(default)]
            input: Option<RedirectInput>,
        }
        let wire = RequestWire::deserialize(deserializer)?;
        if wire.v != PROTOCOL_VERSION {
            return Err(D::Error::custom(format!(
                "unsupported control protocol version {} (this oneharness speaks v{PROTOCOL_VERSION})",
                wire.v
            )));
        }
        Ok(ControlRequest {
            v: wire.v,
            verb: wire.verb,
            input: wire.input,
        })
    }
}

/// Why a control request could not be served. Distinct reasons because a
/// supervisor reacts differently to each: `unsupported` is permanent for the
/// harness, `not_running` means the dispatch is gone, `no_active_turn` means
/// the run is alive but between turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlReason {
    /// The harness declares no [`ControlShape`] — the lever does not exist.
    Unsupported,
    /// The run is alive but no turn is in flight right now.
    NoActiveTurn,
    /// No run is listening on the socket (never started, already exited, or a
    /// stale socket file left by an abnormally-terminated dispatch).
    NotRunning,
}

impl ControlReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ControlReason::Unsupported => "unsupported",
            ControlReason::NoActiveTurn => "no_active_turn",
            ControlReason::NotRunning => "not_running",
        }
    }
}

/// One newline-terminated response frame, the whole answer to one connection.
///
/// A sum type, not an `ok` flag with optional companions: a frame is either a
/// success carrying the mechanism that served it, or a refusal carrying an
/// error *and* a reason. "Succeeded, and here is why it was refused" is a state
/// a supervisor would have to defend against, so it is not representable. The
/// serialized shape is the fixed wire contract either way —
/// `{"v":2,"ok":true,"mechanism":…}` / `{"v":2,"ok":false,"error":…,"reason":…}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlResponse {
    // llmlint: ignore[invalid_states_unrepresentable] `redirected` is a bool
    // because BOTH of its values are valid here and neither contradicts
    // anything else in the variant: a served interrupt either carried a message
    // or did not, and a supervisor branches on exactly that. Splitting `Served`
    // into two variants would not remove a representable-but-invalid state —
    // there is none — while duplicating `mechanism` and every match arm that
    // reads it. The contradiction this type does exist to forbid, "succeeded,
    // and here is why it was refused", is the `Served`/`Refused` split above,
    // and a refusal claiming a redirection is rejected while parsing.
    Served {
        mechanism: ControlShape,
        /// Whether the request's redirection was delivered: written as the
        /// prompt of the turn that replaces the aborted one. A redirect is
        /// answered only once that is so — a turn that ends, or a run that
        /// exits, before the write answers `Refused` instead (`no_active_turn`
        /// / `not_running`), and the run then never writes the message. So a
        /// supervisor that sent `input` reads back whether it landed, exactly
        /// once, rather than inferring it from `ok`.
        redirected: bool,
    },
    Refused {
        error: String,
        reason: ControlReason,
    },
}

/// The serialized form of [`ControlResponse`]. Private: it exists only so serde
/// derives the exact wire frame, and its "any combination" shape is never
/// reachable from outside this module.
// llmlint: ignore[invalid_states_unrepresentable] This is the literal external
// wire frame, whose `ok` + optional-companion shape is fixed by the published
// protocol and is exactly what an untrusted peer can send. Its whole purpose is
// to hold that unvalidated form for one step so `Deserialize` can reject every
// contradictory combination; the public type it produces cannot represent one.
#[derive(Serialize, Deserialize)]
struct ResponseWire {
    v: u32,
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mechanism: Option<String>,
    /// Omitted rather than sent as `false`, so a plain interrupt's answer gains
    /// no field a supervisor did not ask for — only `v` distinguishes it from
    /// the frame the previous version emitted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    redirected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<ControlReason>,
}

impl Serialize for ControlResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let wire = match self {
            ControlResponse::Served {
                mechanism,
                redirected,
            } => ResponseWire {
                v: PROTOCOL_VERSION,
                ok: true,
                mechanism: Some(mechanism.as_str().to_string()),
                redirected: *redirected,
                error: None,
                reason: None,
            },
            ControlResponse::Refused { error, reason } => ResponseWire {
                v: PROTOCOL_VERSION,
                ok: false,
                mechanism: None,
                redirected: false,
                error: Some(error.clone()),
                reason: Some(*reason),
            },
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ControlResponse {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let wire = ResponseWire::deserialize(deserializer)?;
        if wire.v != PROTOCOL_VERSION {
            return Err(D::Error::custom(format!(
                "unsupported control protocol version {} (this oneharness speaks v{PROTOCOL_VERSION})",
                wire.v
            )));
        }
        if wire.ok {
            if wire.error.is_some() || wire.reason.is_some() {
                return Err(D::Error::custom(
                    "a successful control frame must not carry an error or a refusal reason",
                ));
            }
            let mechanism = wire.mechanism.ok_or_else(|| {
                D::Error::custom("a successful control frame must carry a mechanism")
            })?;
            return ControlShape::from_wire(&mechanism)
                .map(|mechanism| ControlResponse::Served {
                    mechanism,
                    redirected: wire.redirected,
                })
                .ok_or_else(|| {
                    D::Error::custom(format!("unknown control mechanism `{mechanism}`"))
                });
        }
        if wire.mechanism.is_some() {
            return Err(D::Error::custom(
                "a refused control frame must not carry a mechanism",
            ));
        }
        // Nothing was delivered, so a refusal claiming a redirection is a frame
        // that contradicts itself — refused here rather than decoded into a
        // supervisor's belief that its message reached the agent.
        if wire.redirected {
            return Err(D::Error::custom(
                "a refused control frame must not claim a redirection was delivered",
            ));
        }
        Ok(ControlResponse::Refused {
            error: wire
                .error
                .ok_or_else(|| D::Error::custom("a refused control frame must carry an error"))?,
            reason: wire
                .reason
                .ok_or_else(|| D::Error::custom("a refused control frame must carry a reason"))?,
        })
    }
}

/// The schema is written by hand for the same reason [`Serialize`] and
/// [`Deserialize`] are: the wire frame is an `ok` flag with companions, and the
/// Rust type is the sum that flag stands for, so no derive can bridge them. It
/// is a `oneOf` of the two frames rather than the permissive union of their
/// fields, because that is what `Deserialize` accepts — an SDK validator built
/// from this schema must refuse exactly the contradictory frames the Rust
/// reader refuses, or a consumer validates a frame oneharness itself would not.
impl JsonSchema for ControlResponse {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("ControlResponse")
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mechanism = generator.subschema_for::<ControlShape>();
        let reason = generator.subschema_for::<ControlReason>();
        schemars::json_schema!({
            "description": "The answer to one `oneharness interrupt`: either the abort was served, or it was refused with a reason.",
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "v": { "type": "integer", "const": PROTOCOL_VERSION },
                        "ok": { "type": "boolean", "const": true },
                        "mechanism": mechanism,
                        "redirected": {
                            "type": "boolean",
                            "description": "Whether the request's redirection was delivered: written as the prompt of the turn that replaces the aborted one. A redirect is answered only once that is so; one whose turn ended or whose run exited first is refused instead. Omitted rather than sent as `false`, so a plain interrupt's answer gains no field the supervisor did not ask for.",
                        },
                        // The two fields a served frame must not carry, spelled
                        // per-property rather than as `additionalProperties:
                        // false`: the reader accepts a field a newer oneharness
                        // adds, so the schema must too, and only the
                        // contradictions are excluded.
                        "error": false,
                        "reason": false,
                    },
                    "required": ["v", "ok", "mechanism"],
                },
                {
                    "type": "object",
                    "properties": {
                        "v": { "type": "integer", "const": PROTOCOL_VERSION },
                        "ok": { "type": "boolean", "const": false },
                        "error": { "type": "string" },
                        "reason": reason,
                        // Nothing was delivered, so a mechanism is a
                        // contradiction and a claimed redirection is a lie a
                        // supervisor would act on.
                        "mechanism": false,
                        "redirected": { "const": false },
                    },
                    "required": ["v", "ok", "error", "reason"],
                },
            ],
        })
    }
}

impl ControlResponse {
    /// The documented success frame, carrying the mechanism that served it.
    #[must_use]
    pub fn served(shape: ControlShape) -> Self {
        ControlResponse::Served {
            mechanism: shape,
            redirected: false,
        }
    }

    /// The success frame for an interrupt whose redirection was delivered as
    /// the prompt of the turn that replaced the aborted one.
    #[must_use]
    pub fn redirected(shape: ControlShape) -> Self {
        ControlResponse::Served {
            mechanism: shape,
            redirected: true,
        }
    }

    /// Whether the redirection was delivered as the next turn's prompt.
    #[must_use]
    pub fn is_redirected(&self) -> bool {
        matches!(self, ControlResponse::Served { redirected, .. } if *redirected)
    }

    /// The documented failure frame.
    #[must_use]
    pub fn refused(error: impl Into<String>, reason: ControlReason) -> Self {
        ControlResponse::Refused {
            error: error.into(),
            reason,
        }
    }

    /// Whether the request was served.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        matches!(self, ControlResponse::Served { .. })
    }

    /// The mechanism that served it, or `None` on a refusal.
    #[must_use]
    pub fn mechanism(&self) -> Option<ControlShape> {
        match self {
            ControlResponse::Served { mechanism, .. } => Some(*mechanism),
            ControlResponse::Refused { .. } => None,
        }
    }

    /// Why it was refused, or `None` when it was served.
    #[must_use]
    pub fn reason(&self) -> Option<ControlReason> {
        match self {
            ControlResponse::Served { .. } => None,
            ControlResponse::Refused { reason, .. } => Some(*reason),
        }
    }

    /// The run-report record for having handled `verb` at `at` with this answer.
    #[must_use]
    pub fn record(&self, verb: ControlVerb, at: UtcInstant) -> ControlEvent {
        match self {
            ControlResponse::Served { redirected, .. } => ControlEvent::Served {
                verb,
                at,
                redirected: *redirected,
            },
            ControlResponse::Refused { reason, .. } => ControlEvent::Refused {
                verb,
                at,
                reason: *reason,
            },
        }
    }

    /// The frame as one wire line (newline-terminated), never failing: the
    /// shape is closed and always serializable.
    #[must_use]
    pub fn to_line(&self) -> String {
        let mut line = serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                "{{\"v\":{PROTOCOL_VERSION},\"ok\":false,\"error\":\"response could not be encoded\",\"reason\":\"not_running\"}}"
            )
        });
        line.push('\n');
        line
    }
}

/// Parse one request line, or say exactly why it is not a request this version
/// serves. Every rejection is loud — an unparseable frame is never treated as
/// an interrupt, and an interrupt is never treated as noise.
pub fn parse_request(line: &str) -> Result<ControlRequest, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err("empty control request".to_string());
    }
    // Deserialization refuses an unsupported version, so a `ControlRequest`
    // that exists here is one this build speaks.
    serde_json::from_str(trimmed).map_err(|err| {
        let message = err.to_string();
        if message.contains("protocol version") {
            message
        } else {
            format!("malformed control request: {message}")
        }
    })
}

/// The most bytes a unix-domain socket **address** may occupy, the terminating
/// NUL included: `sockaddr_un.sun_path` is 104 bytes on the BSD/macOS lineage
/// and 108 on Linux, and `bind` past it fails with `ENAMETOOLONG`.
///
/// It is a property of the address, not of the filesystem: a path the OS opens
/// happily as a file is unbindable as a socket at 108 bytes, which is how a
/// session name one byte too long turned every controlled dispatch on a host
/// into an unreachable run. Named here so [`socket_path`] can keep the address
/// inside it by construction and say so loudly when it cannot.
///
/// `usize::MAX` off unix, where there is no `sun_path` to overflow — control is
/// refused on those platforms before a socket is ever addressed, and imposing a
/// unix budget there would refuse an ordinary deep path for a reason that does
/// not exist on it.
pub const SOCKET_ADDRESS_MAX: usize = if !cfg!(unix) {
    usize::MAX
} else if cfg!(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)) {
    104
} else {
    108
};

/// The digest, in hex characters, that stands in for a session name too long to
/// spell in a socket address. 64 bits of FNV-1a truncated to 48: the names it
/// separates all live in one run's `control/` directory, and a collision there
/// would have to be two live sessions whose sanitized names agree.
const DIGEST_CHARS: usize = 12;

/// The shortest file name [`socket_path`] will build: digest plus suffix, with
/// nothing of the session name left. Below this the address cannot be made to
/// fit by shortening at all, and the store directory is what has to change.
///
/// Crate-visible so the socket tests can build a store sitting exactly at the
/// budget rather than restating the arithmetic they are checking.
pub(crate) const MIN_FILE_NAME: usize = DIGEST_CHARS + SOCKET_SUFFIX.len();

const SOCKET_SUFFIX: &str = ".sock";

/// A socket address that cannot be bound on this platform, with the numbers a
/// reader needs to act: which address, how long it came out, and the budget it
/// broke.
///
/// The length is **derived** from the path rather than stored beside it, and the
/// limit is this platform's, so an error whose measurements contradict its own
/// address cannot be built — the number a caller acts on is always the number of
/// the address it is looking at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketAddressTooLong {
    path: PathBuf,
}

impl SocketAddressTooLong {
    fn new(path: PathBuf) -> Self {
        SocketAddressTooLong { path }
    }

    /// The address that could not be bound — the shortest one that could be
    /// built for this store and session.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Its length in bytes, terminating NUL included — what is compared against
    /// [`Self::limit`].
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.path.as_os_str().len() + 1
    }

    /// This platform's `sun_path` budget ([`SOCKET_ADDRESS_MAX`]).
    #[must_use]
    pub fn limit(&self) -> usize {
        SOCKET_ADDRESS_MAX
    }
}

impl std::fmt::Display for SocketAddressTooLong {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the control socket address `{}` is {} bytes, past this platform's {}-byte unix-socket \
             address limit (`sun_path`, terminating NUL included)",
            self.path.display(),
            self.bytes(),
            self.limit()
        )
    }
}

impl std::error::Error for SocketAddressTooLong {}

/// The socket file name backing session `name` (`<sanitized name>.sock`), or a
/// digest-suffixed abbreviation of it when the full name would not fit in
/// `budget` bytes. `None` when even the shortest abbreviation (the private
/// `MIN_FILE_NAME`) exceeds the budget.
///
/// The abbreviation keeps as much of the readable name as fits and ends in a
/// digest of the whole sanitized name, so two long names that share a prefix
/// still address different sockets. It is a pure function of the name and the
/// budget, which is what lets `oneharness interrupt` — a different process —
/// derive the same address without being told it.
#[must_use]
pub fn socket_file_name(name: &str, budget: usize) -> Option<String> {
    let sanitized = sanitize_name(name);
    let full = format!("{sanitized}{SOCKET_SUFFIX}");
    if full.len() <= budget {
        return Some(full);
    }
    if budget < MIN_FILE_NAME {
        return None;
    }
    let digest = digest(&sanitized);
    // One byte of the budget goes to the `-` joining name to digest, so a budget
    // of exactly `MIN_FILE_NAME` keeps nothing of the name.
    let kept: String = sanitized
        .chars()
        .take(budget.saturating_sub(MIN_FILE_NAME + 1))
        .collect::<String>()
        .trim_end_matches('-')
        .to_string();
    Some(if kept.is_empty() {
        format!("{digest}{SOCKET_SUFFIX}")
    } else {
        format!("{kept}-{digest}{SOCKET_SUFFIX}")
    })
}

/// FNV-1a over `text`, rendered as [`DIGEST_CHARS`] hex characters.
///
/// Spelled out rather than taken from `DefaultHasher`, whose output std
/// explicitly does not promise to be stable across releases: this digest is
/// half of an *address* two processes resolve independently, so a run and the
/// `oneharness interrupt` that reaches it must agree on it forever, not merely
/// within one toolchain.
fn digest(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")[..DIGEST_CHARS].to_string()
}

/// The socket backing session `name` under the session store `dir`:
/// `<dir>/control/<name>.sock`. Pure path arithmetic; touches no disk.
///
/// The address is kept inside [`SOCKET_ADDRESS_MAX`] **by construction** — a
/// session name that would not fit is abbreviated to a digest-suffixed one (see
/// [`socket_file_name`]), deterministically, so the separate `oneharness
/// interrupt` process resolves the same address from the same `(dir, name)`.
/// Names of the length a graph mints (`node-scope-<millis>-<pid>-worker-skill`)
/// therefore bind with room to spare instead of failing at the last byte.
///
/// When even the shortest file name does not fit, the store directory itself is
/// past the budget and nothing here can rescue it: that is a
/// [`SocketAddressTooLong`] the caller reports before anything spawns, rather
/// than a control channel that silently is not there. Two callers, one rule —
/// the run that binds and the `interrupt` that connects.
///
/// A caller that later *canonicalizes* the address can still lengthen it past
/// the budget (a symlinked store, macOS's `/tmp` → `/private/tmp`);
/// [`socket_address_within_limit`] is that check, applied where the resolved
/// address is known.
pub fn socket_path(dir: &Path, name: &str) -> Result<PathBuf, SocketAddressTooLong> {
    let control_dir = dir.join(CONTROL_DIR);
    // The prefix the file name is measured against, taken from a real join so a
    // separator this platform adds (or does not, at a root) is counted rather
    // than assumed.
    let prefix = control_dir.join("x").as_os_str().len() - 1;
    let budget = (SOCKET_ADDRESS_MAX - 1).saturating_sub(prefix);
    match socket_file_name(name, budget) {
        Some(file) => Ok(control_dir.join(file)),
        None => Err(SocketAddressTooLong::new(
            control_dir.join(format!("{}{SOCKET_SUFFIX}", digest(&sanitize_name(name)))),
        )),
    }
}

/// Whether `path` can be bound as a unix-socket address on this platform — the
/// same budget [`socket_path`] builds against, checked against an address that
/// has already been resolved (canonicalized) rather than one being built.
pub fn socket_address_within_limit(path: &Path) -> Result<(), SocketAddressTooLong> {
    // Measured through the error type, which derives both numbers from the path
    // itself — so the check and the diagnostic can never disagree about what
    // this address costs.
    let measured = SocketAddressTooLong::new(path.to_path_buf());
    if measured.bytes() <= measured.limit() {
        return Ok(());
    }
    Err(measured)
}

/// The stdin line that delivers the user prompt for a control-enabled run, for
/// a harness whose control rides its own stdin. `None` for a server-backed
/// mechanism, whose prompt goes over the server protocol instead.
///
/// Claude Code's `-p --input-format stream-json` reads one JSON message per
/// line; the prompt is a `user` message rather than an argv positional, which
/// is what lets the stdin handle stay open for the control frame afterwards.
///
/// The same frame opens a redirected turn: an interrupt's `input` is written
/// here once the aborted turn's `result` arrives, which is the only moment
/// Claude Code does not drop it (a `user` message sent mid-turn is silently
/// discarded — verified live).
#[must_use]
pub fn prompt_frame(shape: ControlShape, prompt: &str) -> Option<String> {
    match shape {
        ControlShape::ClaudeControlRequest => {
            let value = serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [{"type": "text", "text": prompt}],
                },
            });
            Some(format!("{value}\n"))
        }
        _ => None,
    }
}

/// The stdin line that aborts the in-flight turn, for a stdin-borne mechanism.
/// `None` for a server-backed mechanism.
#[must_use]
pub fn interrupt_frame(shape: ControlShape, request_id: &str) -> Option<String> {
    match shape {
        ControlShape::ClaudeControlRequest => {
            let value = serde_json::json!({
                "type": "control_request",
                "request_id": request_id,
                "request": {"subtype": "interrupt"},
            });
            Some(format!("{value}\n"))
        }
        _ => None,
    }
}

/// Whether `line` is the harness's end-of-turn document, for a stdin-borne
/// mechanism whose process stays alive waiting for more input.
///
/// This is load-bearing: with stdin held open (the very thing that makes
/// control possible) Claude Code does **not** exit when the turn ends — it
/// waits for the next message. Recognizing the terminal document is how the
/// run knows to close stdin and let the process finish, so a control-enabled
/// run still terminates on its own like an ordinary one.
#[must_use]
pub fn is_turn_terminal(shape: ControlShape, line: &str) -> bool {
    match shape {
        ControlShape::ClaudeControlRequest => {
            let trimmed = line.trim();
            if !trimmed.starts_with('{') {
                return false;
            }
            serde_json::from_str::<serde_json::Value>(trimmed)
                .ok()
                .and_then(|value| {
                    value
                        .get("type")
                        .and_then(|t| t.as_str())
                        .map(|t| t == "result")
                })
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// One control request the run handled, recorded in the report so a consumer
/// can tell an interrupted turn from one that simply ended.
///
/// A sum type discriminated by `outcome`, so a record can never say "served,
/// and here is the refusal reason": the reason exists exactly when there was a
/// refusal. Serialized flat — `{"outcome":"served","verb":…,"at":…}` /
/// `{"outcome":"refused","verb":…,"at":…,"reason":…}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ControlEvent {
    // llmlint: ignore[invalid_states_unrepresentable] The same reasoning as
    // `ControlResponse::Served`: both values of `redirected` are valid records
    // of something that really happened, so there is no invalid state to make
    // unrepresentable. It is also a published report field — a second `outcome`
    // value would be a schema change for consumers matching on that tag, which
    // is a cost with nothing bought.
    /// The mechanism accepted the request.
    Served {
        /// The verb requested.
        verb: ControlVerb,
        /// When the request was handled.
        at: UtcInstant,
        /// Whether a redirection was delivered as the prompt of the turn that
        /// replaced the aborted one — the same outcome the interrupt was
        /// answered with; one that was not delivered is a `Refused` record.
        /// Omitted when it was a plain stop, so an older consumer reads the
        /// same record it always did.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        redirected: bool,
    },
    /// The request was refused, with why.
    Refused {
        verb: ControlVerb,
        at: UtcInstant,
        reason: ControlReason,
    },
}

impl ControlEvent {
    /// The verb this record is about.
    #[must_use]
    pub fn verb(&self) -> ControlVerb {
        match self {
            ControlEvent::Served { verb, .. } | ControlEvent::Refused { verb, .. } => *verb,
        }
    }

    /// When it was handled.
    #[must_use]
    pub fn at(&self) -> &UtcInstant {
        match self {
            ControlEvent::Served { at, .. } | ControlEvent::Refused { at, .. } => at,
        }
    }

    /// Whether the mechanism accepted it.
    #[must_use]
    pub fn is_served(&self) -> bool {
        matches!(self, ControlEvent::Served { .. })
    }

    /// Whether this request also delivered a redirection as the next turn's
    /// prompt.
    #[must_use]
    pub fn is_redirected(&self) -> bool {
        matches!(self, ControlEvent::Served { redirected, .. } if *redirected)
    }

    /// Why it was refused, or `None` when it was served.
    #[must_use]
    pub fn reason(&self) -> Option<ControlReason> {
        match self {
            ControlEvent::Served { .. } => None,
            ControlEvent::Refused { reason, .. } => Some(*reason),
        }
    }
}

/// The run report's `control` block: where the socket lived, which mechanism
/// backed it, and every request served over the run's lifetime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ControlReport {
    /// Absolute path of the socket this run listened on.
    pub socket: AbsolutePath,
    /// The harness mechanism backing it.
    pub mechanism: ControlShape,
    /// Every control request served, in order.
    pub interrupts: Vec<ControlEvent>,
    /// The shared Codex app-server the run's turn ran on, when it ran on one
    /// (`app_server = "shared"`). Absent for every other mechanism and for a
    /// dispatch's own `codex app-server` child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_server: Option<AppServerReport>,
}

/// Which shared Codex app-server a pooled dispatch's turn ran on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppServerReport {
    /// The pid the pool recorded for the server (its `server.json`): the
    /// process it launched, which is the launcher shim when Codex is one.
    pub pid: std::num::NonZeroU32,
    /// The pool entry the server belongs to — equal for every dispatch that
    /// shared it.
    pub pool_key: PoolKey,
}

#[cfg(test)]
mod control_mode_parity {
    //! Every control-capable harness × every [`PermissionMode`]: what a
    //! controlled run is allowed to do must be what the same mode allows
    //! without `--control`.
    //!
    //! The one confirmed bug in this feature was a cell of this grid. Codex's
    //! `--control --mode bypass` asked the app-server for `workspaceWrite`
    //! where `codex exec` under the same mode asks for no sandbox at all, so
    //! the controlled run was strictly MORE restricted than the uncontrolled
    //! one — and on a host without unprivileged user namespaces every shell
    //! call failed before running, so the turn did no work whatsoever. It
    //! survived because the live suite drives control under `--mode bypass`
    //! only: the one position that was broken was the only one tested.
    //!
    //! So this is a unit assertion over the whole grid rather than another live
    //! phase. It reads both postures out of the REAL code — the registry's own
    //! `build_argv` on the uncontrolled side, the protocol client on the
    //! controlled one — and is driven off the registry, so a harness or a mode
    //! added later arrives here unasserted and fails.
    //!
    //! Equality alone is a floorless property: two paths that both permit a
    //! write agree just as well as two that both refuse one, so a `read-only`
    //! that stopped blocking anything satisfies every cell above. The companion
    //! `a_no_mutation_mode_withholds_the_capability_to_write` is that floor —
    //! per harness, in that CLI's own vocabulary — and it covers the whole
    //! registry rather than the control-capable part of it, because a mode that
    //! fails open fails open with or without `--control`.

    use serde_json::Value;

    use super::{absolute_for_test, ControlShape};
    use crate::domain::dialogue::{Dialogue, DialogueConfig, DialogueStep};
    use crate::domain::harness::{self, BuildCtx, HarnessSpec, PromptDelivery};
    use crate::domain::http::permits_action;
    use crate::domain::mode::{ApprovalPosture, PermissionMode};

    /// The working directory both sides are asked about. Absolute on every
    /// platform, since a `SandboxPolicy` names writable roots by path.
    const WORK: &str = "/work";

    /// The argv the registry really builds for `mode`, differing from an
    /// ordinary run in nothing but how the prompt is delivered.
    fn argv_for(
        spec: &'static HarnessSpec,
        mode: PermissionMode,
        delivery: PromptDelivery,
    ) -> Vec<String> {
        (spec.build_argv)(&BuildCtx {
            bin: spec.default_bin,
            prompt: "hi",
            model: None,
            system: None,
            resume: None,
            fork: false,
            mode,
            output_format: spec.output_format,
            schema: None,
            system_file: None,
            delivery,
            plugins: false,
            apps: false,
            analytics: false,
            model_catalog: None,
        })
    }

    /// The sandbox `codex exec` asks for under `mode`, read off its real argv
    /// and spelled in the app-server's own `SandboxPolicy` vocabulary so the
    /// two sides are comparable at all.
    fn codex_exec_sandbox(argv: &[String]) -> &'static str {
        if argv
            .iter()
            .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox")
        {
            return "dangerFullAccess";
        }
        match argv
            .windows(2)
            .find(|pair| pair[0] == "--sandbox")
            .map(|pair| pair[1].as_str())
        {
            Some("read-only") => "readOnly",
            Some("workspace-write") => "workspaceWrite",
            Some(other) => panic!("`codex exec` asks for an unmapped sandbox `{other}`"),
            // `exec`'s own default, stated at the registry entry: no flag is
            // the read-only sandbox, not an absent one.
            None => "readOnly",
        }
    }

    /// The sandbox a controlled codex turn asks its app-server for.
    fn codex_control_sandbox(mode: PermissionMode) -> String {
        Dialogue::new(
            ControlShape::CodexAppServer,
            DialogueConfig {
                prompt: "hi".to_string(),
                cwd: absolute_for_test(WORK),
                model: None,
                mode,
                resume: None,
                posture: posture_of(harness::by_id("codex").unwrap(), mode),
                shell_environment_policy: None,
            },
        )
        .expect("codex drives its turn over the app-server")
        .codex_sandbox_policy()["type"]
            .as_str()
            .expect("a sandbox names its type")
            .to_string()
    }

    /// The harness's own declaration for `mode`.
    fn posture_of(spec: &'static HarnessSpec, mode: PermissionMode) -> ApprovalPosture {
        spec.mode(mode)
            .expect("the caller checked the mode is supported")
            .posture
    }

    /// Whether a driven turn acts without asking under `mode` — answered by the
    /// real client, through the shape it would really be asked in.
    fn unattended_under_control(
        spec: &'static HarnessSpec,
        shape: ControlShape,
        mode: PermissionMode,
    ) -> bool {
        match shape {
            // ACP offers options and the client picks one (or cancels), so the
            // answer is read off an actual reply rather than a predicate.
            ControlShape::AcpCancel => {
                let mut dialogue = Dialogue::new(
                    shape,
                    DialogueConfig {
                        prompt: "hi".to_string(),
                        cwd: absolute_for_test(WORK),
                        model: None,
                        mode,
                        resume: None,
                        posture: posture_of(spec, mode),
                        shell_environment_policy: None,
                    },
                )
                .expect("ACP drives its turn");
                let step = dialogue.on_line(
                    r#"{"jsonrpc":"2.0","id":1,"method":"session/request_permission","params":{"options":[{"optionId":"a","kind":"allow_once"}]}}"#,
                );
                let DialogueStep::Send(frames) = step else {
                    panic!("a permission request is always answered");
                };
                let reply: Value = serde_json::from_str(&frames[0]).expect("a JSON reply");
                reply["result"]["outcome"]["outcome"] == "selected"
            }
            // Both HTTP servers carry the posture as one decision: opencode
            // answers each ask with it, crush declares it on the workspace.
            _ => permits_action(
                spec.mode(mode)
                    .expect("the caller checked the mode is supported"),
            )
            .allows(),
        }
    }

    /// Whether the harness's OWN uncontrolled run acts without asking under
    /// `mode`, read off the argv and environment its registry entry really
    /// produces. The token per harness is that CLI's documented don't-ask
    /// switch — the same source `build_argv` maps the mode from. This is what
    /// keeps `ModeSpec::posture` honest: it is compared against the mapping
    /// rather than trusted.
    fn unattended_without_control(spec: &'static HarnessSpec, mode: PermissionMode) -> bool {
        let argv = argv_for(spec, mode, PromptDelivery::Argv);
        let env = spec.mode(mode).map_or(&[][..], |declared| declared.env);
        match spec.id {
            "opencode" => argv
                .iter()
                .any(|arg| arg == "--dangerously-skip-permissions"),
            // `GOOSE_MODE`: `approve` gates each call, `smart_approve` and
            // `auto` both act on their own.
            "goose" => env
                .iter()
                .any(|(_, value)| *value == "auto" || *value == "smart_approve"),
            // `crush run -q` auto-approves the whole session and carries no
            // mode on its argv at all, so it is unattended whatever was asked
            // for — the limitation declared at its registry entry.
            "crush" => true,
            other => panic!("`{other}` drives its turn but has no uncontrolled posture reader"),
        }
    }

    #[test]
    fn a_controlled_run_allows_exactly_what_the_same_mode_allows_without_control() {
        let mut grid: Vec<String> = Vec::new();
        for spec in harness::all() {
            let Some(shape) = spec.control else { continue };
            for mode in PermissionMode::ALL {
                let verdict = cell(spec, shape, mode);
                grid.push(format!("{} {} {verdict}", spec.id, mode.as_str()));
            }
        }
        // The whole grid, spelled out: a harness or a mode added later lands
        // here as a line nobody wrote down, which is the point. Every cell a
        // harness supports is an EQUALITY, because a controlled run that
        // reshapes the policy is the bug this grid exists to catch — with one
        // exception, spelled `known-gap:…`, which is a cell nobody has made
        // equal yet. A gap is NAMED here rather than dropped from the grid: a
        // missing line reads as coverage, and this feature has already lost a
        // night to a cell that was silently absent.
        assert_eq!(
            grid,
            [
                "claude-code read-only same-argv",
                "claude-code plan same-argv",
                "claude-code default same-argv",
                "claude-code edit same-argv",
                "claude-code auto same-argv",
                "claude-code bypass same-argv",
                "codex read-only same-sandbox:readOnly",
                "codex plan same-sandbox:readOnly",
                "codex default same-sandbox:readOnly",
                "codex edit mode-unsupported",
                "codex auto same-sandbox:workspaceWrite",
                "codex bypass same-sandbox:dangerFullAccess",
                "opencode read-only same-posture:gated",
                "opencode plan same-posture:gated",
                "opencode default same-posture:gated",
                // The one cell that is NOT an equality, and says so. `edit` is
                // opencode's `OPENCODE_CONFIG_CONTENT`, which a turn submitted
                // to a pooled server has no way to carry — so `--control --mode
                // edit` is a usage error rather than a turn under a policy
                // nobody asked for.
                "opencode edit known-gap:mode-env-not-delivered-to-a-pooled-server",
                "opencode auto mode-unsupported",
                "opencode bypass same-posture:unattended",
                "goose read-only mode-unsupported",
                "goose plan mode-unsupported",
                "goose default same-posture:gated",
                "goose edit mode-unsupported",
                "goose auto same-posture:unattended",
                "goose bypass same-posture:unattended",
                // `crush run` cannot gate, so its `default` is unattended
                // however it is asked — and a controlled turn says the same.
                "crush read-only mode-unsupported",
                "crush plan mode-unsupported",
                "crush default same-posture:unattended",
                "crush edit mode-unsupported",
                "crush auto mode-unsupported",
                "crush bypass same-posture:unattended",
                // Copilot's permission flags ride the `--acp` argv beside it, so
                // its controlled launch carries the mode's own mapping whole.
                "copilot read-only same-argv",
                "copilot plan same-argv",
                "copilot default same-argv",
                "copilot edit same-argv",
                "copilot auto mode-unsupported",
                "copilot bypass same-argv",
            ]
        );
    }

    /// The argv with this delivery's own prefix removed, so what is left is the
    /// policy the mode put on it.
    ///
    /// The prefixes are asserted rather than assumed: they are the ONLY thing
    /// the two launches are allowed to differ in, so a change to one has to
    /// come through here.
    fn policy_tail(
        spec: &'static HarnessSpec,
        mode: PermissionMode,
        delivery: PromptDelivery,
    ) -> Vec<String> {
        let argv = argv_for(spec, mode, delivery);
        let prefix: &[&str] = match (spec.id, delivery) {
            // The prompt is a positional; under control it is the first frame
            // on a stdin message stream instead.
            ("claude-code", PromptDelivery::Argv) => &[spec.default_bin, "-p", "hi"],
            ("claude-code", PromptDelivery::ControlStream) => {
                &[spec.default_bin, "-p", "--input-format", "stream-json"]
            }
            // The prompt (and the session, and the model) are negotiated on the
            // ACP wire, so the control launch is the server switch alone.
            ("copilot", PromptDelivery::Argv) => &[spec.default_bin, "-p", "hi"],
            ("copilot", PromptDelivery::ControlStream) => &[spec.default_bin, "--acp"],
            (id, _) => panic!("`{id}` has no declared control-launch prefix"),
        };
        assert_eq!(
            &argv[..prefix.len()],
            prefix,
            "`{}` {mode:?} {delivery:?} launch prefix",
            spec.id
        );
        argv[prefix.len()..].to_vec()
    }

    /// One cell of the grid: assert the two paths send the same policy, and
    /// name how it got there. Every supported mode is an equality, except the
    /// one delivery nobody has made equal — which answers `known-gap:…` rather
    /// than disappearing from the grid.
    fn cell(spec: &'static HarnessSpec, shape: ControlShape, mode: PermissionMode) -> String {
        let Some(declared) = spec.mode(mode) else {
            // The harness cannot express this mode at all, so the command layer
            // refuses the run before anything spawns — with `--control` or
            // without it. There is no pair of policies to compare.
            return "mode-unsupported".to_string();
        };
        // 1. A KNOWN GAP, kept in the grid rather than dropped from it. A mode
        //    the harness delivers through its OWN environment cannot reach a
        //    turn submitted to a pooled server: the environment belongs to the
        //    server process, and handing it there — which is what shipped, and
        //    what is now reverted — made the approval mode a component of the
        //    pool key and left a controlled `--mode default` opencode turn
        //    ending in `status=timeout` across four CI cycles. So there is no
        //    equality to assert here, and the honest cell says which one is
        //    missing: the command layer refuses the mode before anything spawns
        //    (`OneharnessError::ControlModeUnsupported`) rather than run the
        //    turn under the server's own policy.
        if !declared.env.is_empty() && shape.needs_pooled_server() {
            return "known-gap:mode-env-not-delivered-to-a-pooled-server".to_string();
        }
        match shape {
            // 2. The one policy the control path recomputes in its own
            //    vocabulary, and where the bug lived.
            ControlShape::CodexAppServer => {
                let ordinary = codex_exec_sandbox(&argv_for(spec, mode, PromptDelivery::Argv));
                assert_eq!(
                    codex_control_sandbox(mode),
                    ordinary,
                    "`{}` asks for a different sandbox under --control for {mode:?}",
                    spec.id
                );
                format!("same-sandbox:{ordinary}")
            }
            // 3. Harnesses whose controlled launch IS the ordinary argv:
            //    Claude Code's control frame rides its own `-p` run, and
            //    copilot's permission flags sit beside `--acp` (verified
            //    against `copilot --help` and by handshaking a real one). The
            //    mode's arguments must match byte for byte.
            _ if MODE_RIDES_CONTROL_ARGV.contains(&spec.id) => {
                assert_eq!(
                    policy_tail(spec, mode, PromptDelivery::Argv),
                    policy_tail(spec, mode, PromptDelivery::ControlStream),
                    "`{}` sends different mode arguments under --control for {mode:?}",
                    spec.id
                );
                "same-argv".to_string()
            }
            // 4. Everything else answers the server's permission requests, and
            //    the answer must be the posture the harness's own run takes.
            //    A mode delivered by environment and driven over stdio (goose's
            //    `GOOSE_MODE`) is already equal by construction — the control
            //    child IS an ordinary job, so it is spawned with the same job
            //    environment an uncontrolled run gets — and this pins the wire
            //    answer layered on top of it.
            shape => {
                let controlled = unattended_under_control(spec, shape, mode);
                let ordinary = unattended_without_control(spec, mode);
                assert_eq!(
                    controlled, ordinary,
                    "`{}` allows {controlled} under --control and {ordinary} without it for {mode:?}",
                    spec.id
                );
                format!(
                    "same-posture:{}",
                    if controlled { "unattended" } else { "gated" }
                )
            }
        }
    }

    /// The harnesses whose control launch carries the mode's own argument list.
    /// Stated rather than detected, so a launch that silently stopped carrying
    /// it fails here instead of quietly falling through to a coarser check.
    const MODE_RIDES_CONTROL_ARGV: [&str; 2] = ["claude-code", "copilot"];

    /// The tools a `read-only` Claude Code run is left holding. Restated here
    /// rather than read from the registry: this is the assertion, so it has to
    /// fail when the registry's list moves rather than move with it.
    const CLAUDE_READ_ONLY_TOOLS: [&str; 5] = ["Read", "Grep", "Glob", "WebFetch", "WebSearch"];

    /// How a no-mutation `mode` takes the ability to write away from the agent,
    /// read off the argv the registry really builds and asserted present.
    ///
    /// Each harness answers in its own vocabulary because each CLI has its own
    /// mechanism, and the mechanisms are not equally strong. A sandbox or a
    /// native plan mode is enforcement the CLI owns, so a tool it gains later
    /// arrives already inside it. An enumeration of tool names is only as good
    /// as which side it enumerates — which is why Claude's is asserted to name
    /// what the run MAY use. It named what the run may not (`--disallowedTools
    /// Bash Edit Write NotebookEdit`) until claude 2.1.220 put `Task` in the
    /// built-in set: the agent handed the shell call to a subagent the deny
    /// rules did not reach, and the live Windows leg watched the file appear.
    fn withholding(spec: &'static HarnessSpec, mode: PermissionMode) -> String {
        let argv = argv_for(spec, mode, PromptDelivery::Argv);
        let carries = |want: &[&str]| {
            assert!(
                argv.windows(want.len()).any(|window| window == want),
                "`{}` {mode:?} must carry {want:?} to withhold a write; got {argv:?}",
                spec.id
            );
            format!("carries:{}", want.join(" "))
        };
        match (spec.id, mode) {
            ("claude-code", PermissionMode::ReadOnly) => {
                let permitted: Vec<&str> = argv
                    .iter()
                    .skip_while(|arg| *arg != "--tools")
                    .skip(1)
                    .take_while(|arg| !arg.starts_with("--"))
                    .map(String::as_str)
                    .collect();
                assert_eq!(
                    permitted, CLAUDE_READ_ONLY_TOOLS,
                    "`claude-code` read-only must permit exactly the read-only tools; got {argv:?}"
                );
                assert!(
                    !argv.iter().any(|arg| arg == "--disallowedTools"),
                    "`claude-code` read-only must not go back to naming what it forbids, \
                     which leaves every tool it forgot — and every tool the CLI adds — \
                     reachable; got {argv:?}"
                );
                format!("permits-only:{}", permitted.join(","))
            }
            ("claude-code", _) => carries(&["--permission-mode", "plan"]),
            ("codex", _) => carries(&["--sandbox", "read-only"]),
            ("opencode", _) => carries(&["--agent", "plan"]),
            ("qwen", _) => carries(&["--approval-mode", "plan"]),
            ("cursor", PermissionMode::ReadOnly) => carries(&["--mode", "ask"]),
            // Copilot enumerates too, but over its permission vocabulary's own
            // three categories rather than per-tool names, so denying the two
            // that act leaves only the one that reads.
            ("copilot", PermissionMode::ReadOnly) => {
                carries(&["--deny-tool", "shell"]);
                carries(&["--deny-tool", "write"])
            }
            ("cursor" | "copilot", _) => carries(&["--mode", "plan"]),
            (other, _) => {
                panic!("`{other}` supports {mode:?} but nothing here says what it takes away")
            }
        }
    }

    #[test]
    fn a_no_mutation_mode_withholds_the_capability_to_write() {
        let mut grid: Vec<String> = Vec::new();
        for spec in harness::all() {
            for mode in [PermissionMode::ReadOnly, PermissionMode::Plan] {
                if spec.mode(mode).is_none() {
                    continue;
                }
                grid.push(format!(
                    "{} {} {}",
                    spec.id,
                    mode.as_str(),
                    withholding(spec, mode)
                ));
            }
        }
        // Spelled out for the same reason as the grid above: a harness that
        // gains `read-only` or `plan` later lands here as a line nobody wrote.
        assert_eq!(
            grid,
            [
                "claude-code read-only permits-only:Read,Grep,Glob,WebFetch,WebSearch",
                "claude-code plan carries:--permission-mode plan",
                "codex read-only carries:--sandbox read-only",
                "codex plan carries:--sandbox read-only",
                "opencode read-only carries:--agent plan",
                "opencode plan carries:--agent plan",
                "qwen read-only carries:--approval-mode plan",
                "qwen plan carries:--approval-mode plan",
                "copilot read-only carries:--deny-tool write",
                "copilot plan carries:--mode plan",
                "cursor read-only carries:--mode ask",
                "cursor plan carries:--mode plan",
            ]
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_pool_key_schema_accepts_exactly_what_parse_does() {
        let schema = serde_json::to_value(schemars::schema_for!(PoolKey)).unwrap();
        let pattern = regex::Regex::new(schema["pattern"].as_str().unwrap()).unwrap();
        let min = schema["minLength"].as_u64().unwrap() as usize;
        let max = schema["maxLength"].as_u64().unwrap() as usize;
        let mut corpus: Vec<String> = (0u8..=127).map(|b| (b as char).to_string()).collect();
        corpus.extend([0, 1, 127, 128, 129, 200].iter().map(|n| "k".repeat(*n)));
        corpus.extend([
            "é".to_string(),
            "a b".to_string(),
            "codex-app-server-1f".to_string(),
        ]);
        for key in corpus {
            let by_schema = pattern.is_match(&key) && (min..=max).contains(&key.chars().count());
            assert_eq!(
                PoolKey::parse(&key).is_ok(),
                by_schema,
                "{key:?}: the runtime and the schema disagree"
            );
        }
    }

    use super::*;

    #[test]
    fn shape_wire_names_are_stable() {
        assert_eq!(
            ControlShape::ClaudeControlRequest.as_str(),
            "claude-control-request"
        );
        assert_eq!(ControlShape::CodexAppServer.as_str(), "codex-app-server");
        assert_eq!(ControlShape::OpencodeHttp.as_str(), "opencode-http");
        assert_eq!(ControlShape::AcpCancel.as_str(), "acp-cancel");
        assert_eq!(ControlShape::CrushHttp.as_str(), "crush-http");
    }

    #[test]
    fn only_claude_control_rides_the_harnesss_ordinary_run() {
        assert!(!ControlShape::ClaudeControlRequest.drives_turn());
        for shape in [
            ControlShape::CodexAppServer,
            ControlShape::OpencodeHttp,
            ControlShape::AcpCancel,
            ControlShape::CrushHttp,
        ] {
            assert!(shape.drives_turn(), "{shape:?} should drive its own turn");
        }
    }

    /// The drift gate on [`ControlShape::ALL`]: a hand-written list, checked
    /// against one nothing in this module writes.
    ///
    /// Every grid below iterates `ALL`, so a variant missing from it would be
    /// missing from all of them at once — tautologically green. schemars derives
    /// the enum's variant list from the enum itself, which is the independent
    /// source that check needs.
    #[test]
    fn every_declared_mechanism_appears_in_all() {
        let schema = crate::domain::sdk::schema_for_serialize::<ControlShape>();
        let declared: Vec<&str> = schema
            .get("oneOf")
            .and_then(serde_json::Value::as_array)
            .expect("the documented enum generates one schema per variant")
            .iter()
            .map(|variant| {
                variant["const"]
                    .as_str()
                    .expect("each variant is a string constant")
            })
            .collect();
        let listed: Vec<&str> = ControlShape::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            listed, declared,
            "ControlShape::ALL must list every declared mechanism, in the enum's own order"
        );
    }

    /// Which mechanisms can continue a named conversation, over the whole set.
    ///
    /// The grid, not a spot check: `--session` is accepted under `--control` for
    /// every control-capable harness, so a mechanism that silently could not
    /// continue one opened a new conversation every turn while the store, the
    /// report and the flag all read as healthy. Adding a `ControlShape` means
    /// adding its cell here and sourcing its resume request (or declaring it has
    /// none) from the CLI's own protocol.
    #[test]
    fn every_mechanism_says_whether_it_can_continue_a_session() {
        assert_eq!(
            ControlShape::CodexAppServer.resume_request(),
            Some("thread/resume"),
            "codex's ClientRequest declares thread/resume, and its response carries \
             the same `thread` field thread/start does"
        );
        for shape in [
            ControlShape::ClaudeControlRequest,
            ControlShape::OpencodeHttp,
            ControlShape::AcpCancel,
            ControlShape::CrushHttp,
        ] {
            assert_eq!(shape.resume_request(), None, "{shape:?}");
        }
        // Claude Code needs no resume request at all: it rides the harness's
        // ordinary `-p` run, whose verified `--resume` argv carries the handle
        // under `--control` exactly as it does without it.
        assert!(ControlShape::ClaudeControlRequest.carries_session());
        assert!(ControlShape::CodexAppServer.carries_session());
        for shape in [
            ControlShape::OpencodeHttp,
            ControlShape::AcpCancel,
            ControlShape::CrushHttp,
        ] {
            assert!(
                !shape.carries_session(),
                "{shape:?} drives its own turn with no resume request, so a continue \
                 over it must be refused rather than silently started over"
            );
        }
        // Nothing outside the two answers is possible: a mechanism carries a
        // session exactly when it does not drive the turn, or asks to resume.
        for shape in ControlShape::ALL {
            assert_eq!(
                shape.carries_session(),
                !shape.drives_turn() || shape.resume_request().is_some(),
                "{shape:?}"
            );
        }
    }

    #[test]
    fn only_the_http_mechanisms_need_a_pooled_server() {
        // The stdio protocols run as the dispatch's own child, so there is
        // nothing to share and nothing to leak; only a server that outlives one
        // turn is worth pooling.
        for shape in [
            ControlShape::ClaudeControlRequest,
            ControlShape::CodexAppServer,
            ControlShape::AcpCancel,
        ] {
            assert!(!shape.needs_pooled_server(), "{shape:?}");
        }
        assert!(ControlShape::OpencodeHttp.needs_pooled_server());
        assert!(ControlShape::CrushHttp.needs_pooled_server());
    }

    #[test]
    fn an_address_must_be_absolute_to_exist() {
        assert!(AbsolutePath::new("relative/x.sock").is_err());
        let text = absolute_text_for_test("/tmp/x.sock");
        let ok = absolute_for_test("/tmp/x.sock");
        assert_eq!(ok.to_string(), text);
        assert_eq!(serde_json::to_value(&ok).unwrap(), serde_json::json!(text));
        assert!(serde_json::from_value::<AbsolutePath>(serde_json::json!("rel")).is_err());
    }

    #[test]
    fn a_server_address_carries_its_own_transport() {
        assert_eq!(ServerAddress::Stdio.transport(), ServerTransport::Stdio);
        assert_eq!(
            ServerAddress::UnixSocket {
                path: absolute_for_test("/tmp/x.sock")
            }
            .transport(),
            ServerTransport::UnixSocket
        );
        let tcp = ServerAddress::Tcp {
            port: Port::new(7777).unwrap(),
        };
        assert!(Port::new(0).is_err(), "0 is the kernel's sentinel");
        assert_eq!(tcp.transport(), ServerTransport::Tcp);
        // Round-trips with the transport as its discriminator, so a reader can
        // never see coordinates that belong to a different transport.
        let value = serde_json::to_value(&tcp).unwrap();
        assert_eq!(value["transport"], "tcp");
        assert_eq!(value["port"], 7777);
        assert_eq!(serde_json::from_value::<ServerAddress>(value).unwrap(), tcp);
    }

    #[test]
    fn only_an_address_with_something_to_connect_to_narrows_to_a_dialable_one() {
        // A stdio server is reached over pipes its parent already holds, so
        // there is no address for a socket client to open. Refusing here is
        // what keeps an HTTP client for one from being constructible at all,
        // rather than failing later on the first dial.
        assert_eq!(
            DialAddress::try_from(ServerAddress::Stdio),
            Err(NotDialable)
        );
        assert_eq!(
            NotDialable.to_string(),
            "a stdio server has no address to dial"
        );

        // Both dialable transports narrow, and narrowing loses nothing: each
        // one widens back to the address it came from.
        for address in [
            ServerAddress::UnixSocket {
                path: absolute_for_test("/tmp/x.sock"),
            },
            ServerAddress::Tcp {
                port: Port::new(7777).unwrap(),
            },
        ] {
            let dialable = DialAddress::try_from(address.clone()).unwrap();
            assert_eq!(dialable.transport(), address.transport());
            assert_eq!(ServerAddress::from(dialable), address);
        }
    }

    #[test]
    fn only_the_stdin_borne_mechanism_pins_an_output_format() {
        assert_eq!(
            ControlShape::ClaudeControlRequest.required_format(),
            Some(OutputFormat::StreamJson)
        );
        assert_eq!(ControlShape::CodexAppServer.required_format(), None);
    }

    #[test]
    fn request_round_trips_on_the_wire() {
        let line = serde_json::to_string(&ControlRequest::interrupt()).unwrap();
        assert_eq!(line, r#"{"v":2,"verb":"interrupt"}"#);
        assert_eq!(parse_request(&line).unwrap(), ControlRequest::interrupt());
        assert_eq!(ControlRequest::interrupt().input(), None);
    }

    #[test]
    fn a_redirecting_request_carries_its_message_and_round_trips() {
        let request = ControlRequest::redirect(RedirectInput::new("do X instead").unwrap());
        let line = serde_json::to_string(&request).unwrap();
        assert_eq!(line, r#"{"v":2,"verb":"interrupt","input":"do X instead"}"#);
        let parsed = parse_request(&line).unwrap();
        assert_eq!(parsed, request);
        assert_eq!(
            parsed.input().map(RedirectInput::as_str),
            Some("do X instead")
        );
    }

    #[test]
    fn a_redirection_must_be_a_message_a_harness_can_be_handed() {
        // The text is spliced into another program's protocol frame, so the
        // boundary is where it has to be a message rather than bytes.
        for (bad, why) in [
            ("", "blank"),
            ("   \n\t ", "whitespace only"),
            ("stop\u{0}now", "a NUL"),
            ("stop\u{1b}[2Jnow", "an escape sequence"),
            ("c1\u{85}break", "a C1 control"),
        ] {
            assert!(
                RedirectInput::new(bad).is_err(),
                "`{bad}` ({why}) should not be a redirection"
            );
        }
        let too_long = "x".repeat(MAX_REDIRECT_INPUT_CHARS + 1);
        assert!(RedirectInput::new(too_long).is_err());
        // Counted in characters, not bytes: a multi-byte message right at the
        // bound is a message, not an overflow.
        let wide = "é".repeat(MAX_REDIRECT_INPUT_CHARS);
        assert!(RedirectInput::new(wide).is_ok());
        // A paragraph is exactly what a redirection usually is.
        let paragraph = RedirectInput::new("stop that.\n\tdo this instead\r\n").unwrap();
        assert_eq!(paragraph.as_str(), "stop that.\n\tdo this instead\r\n");
        assert_eq!(paragraph.to_string(), paragraph.as_str());
        // And the same rules hold when it arrives from the wire rather than a flag.
        assert!(serde_json::from_value::<RedirectInput>(serde_json::json!("  ")).is_err());
    }

    #[test]
    fn parse_rejects_empty_malformed_and_other_versions() {
        assert!(parse_request("   ").unwrap_err().contains("empty"));
        assert!(parse_request("{ nope").unwrap_err().contains("malformed"));
        assert!(parse_request(r#"{"v":2,"verb":"steer"}"#)
            .unwrap_err()
            .contains("malformed"));
        // A v1 supervisor and a v2 run disagree about what a frame can carry, so
        // each is told which version the other speaks rather than having its
        // `input` silently dropped.
        let old = parse_request(r#"{"v":1,"verb":"interrupt"}"#).unwrap_err();
        assert!(old.contains("version 1"), "{old}");
        let future = parse_request(r#"{"v":3,"verb":"interrupt"}"#).unwrap_err();
        assert!(future.contains("version 3"), "{future}");
        // A redirection that is not a message is refused while parsing, so no
        // run is ever asked to hand one to a harness.
        assert!(parse_request(r#"{"v":2,"verb":"interrupt","input":""}"#).is_err());
        // The version cannot be carried past parsing: there is no way to build
        // a request that claims one this build does not speak.
        assert_eq!(ControlRequest::interrupt().verb(), ControlVerb::Interrupt);
    }

    #[test]
    fn success_frame_carries_the_mechanism_and_omits_error_fields() {
        let line = ControlResponse::served(ControlShape::ClaudeControlRequest).to_line();
        assert_eq!(
            line,
            "{\"v\":2,\"ok\":true,\"mechanism\":\"claude-control-request\"}\n"
        );
        // A redirection says so, so a supervisor reads back that the run took
        // its message rather than inferring it from `ok`.
        let redirected = ControlResponse::redirected(ControlShape::ClaudeControlRequest);
        assert_eq!(
            redirected.to_line(),
            "{\"v\":2,\"ok\":true,\"mechanism\":\"claude-control-request\",\"redirected\":true}\n"
        );
        assert!(redirected.is_redirected());
        assert!(!ControlResponse::served(ControlShape::ClaudeControlRequest).is_redirected());
    }

    #[test]
    fn failure_frame_carries_the_reason_and_omits_the_mechanism() {
        let line = ControlResponse::refused("no run is listening", ControlReason::NotRunning);
        assert_eq!(
            line.to_line(),
            "{\"v\":2,\"ok\":false,\"error\":\"no run is listening\",\"reason\":\"not_running\"}\n"
        );
        assert!(!line.is_redirected());
        assert_eq!(ControlReason::NoActiveTurn.as_str(), "no_active_turn");
        assert_eq!(ControlReason::Unsupported.as_str(), "unsupported");
    }

    #[test]
    fn a_frame_that_contradicts_itself_is_refused_at_parse_time() {
        // The type cannot represent "ok, and here is the refusal reason", so a
        // frame claiming success without a mechanism (or a refusal without one)
        // fails loudly instead of decoding into a half-state.
        for bad in [
            r#"{"v":2,"ok":true}"#,
            r#"{"v":2,"ok":true,"mechanism":"made-up"}"#,
            r#"{"v":2,"ok":true,"mechanism":"acp-cancel","error":"nope"}"#,
            r#"{"v":2,"ok":true,"mechanism":"acp-cancel","reason":"not_running"}"#,
            r#"{"v":2,"ok":false,"error":"nope","reason":"not_running","mechanism":"acp-cancel"}"#,
            r#"{"v":2,"ok":false,"reason":"not_running"}"#,
            r#"{"v":2,"ok":false,"error":"nope"}"#,
            // Nothing was stopped, so nothing was redirected either: a refusal
            // claiming otherwise would tell a supervisor its message landed.
            r#"{"v":2,"ok":false,"error":"nope","reason":"not_running","redirected":true}"#,
            r#"{"v":3,"ok":true,"mechanism":"claude-control-request"}"#,
        ] {
            assert!(
                serde_json::from_str::<ControlResponse>(bad).is_err(),
                "should have refused {bad}"
            );
        }
        let served: ControlResponse =
            serde_json::from_str(r#"{"v":2,"ok":true,"mechanism":"acp-cancel"}"#).unwrap();
        assert_eq!(served.mechanism(), Some(ControlShape::AcpCancel));
        assert!(served.is_ok());
        assert!(!served.is_redirected());
        let redirected: ControlResponse =
            serde_json::from_str(r#"{"v":2,"ok":true,"mechanism":"acp-cancel","redirected":true}"#)
                .unwrap();
        assert!(redirected.is_redirected());
    }

    #[test]
    fn the_generated_schema_accepts_exactly_the_frames_the_reader_accepts() {
        // The hand-written `JsonSchema` is what the SDK validators are built
        // from, and it is a second statement of the rules `Deserialize` already
        // enforces — so the two can disagree, and a consumer would then accept
        // a frame oneharness itself refuses. This drives both against the same
        // frames and requires the same answer.
        //
        // Unknown fields are deliberately NOT part of the disagreement: the
        // reader takes a field a newer oneharness adds, so the schema does too.
        let schema = serde_json::to_value(schemars::schema_for!(ControlResponse))
            .expect("the generated schema is JSON");
        let validator = jsonschema::validator_for(&schema).expect("it is a valid JSON Schema");

        for frame in [
            r#"{"v":2,"ok":true,"mechanism":"acp-cancel"}"#,
            r#"{"v":2,"ok":true,"mechanism":"acp-cancel","redirected":true}"#,
            r#"{"v":2,"ok":false,"error":"nope","reason":"not_running"}"#,
            r#"{"v":2,"ok":false,"error":"nope","reason":"not_running","redirected":false}"#,
            r#"{"v":2,"ok":true,"mechanism":"acp-cancel","future_field":7}"#,
            // Every contradiction the reader refuses.
            r#"{"v":2,"ok":true}"#,
            r#"{"v":2,"ok":true,"mechanism":"made-up"}"#,
            r#"{"v":2,"ok":true,"mechanism":"acp-cancel","error":"nope"}"#,
            r#"{"v":2,"ok":true,"mechanism":"acp-cancel","reason":"not_running"}"#,
            r#"{"v":2,"ok":false,"error":"nope","reason":"not_running","mechanism":"acp-cancel"}"#,
            r#"{"v":2,"ok":false,"reason":"not_running"}"#,
            r#"{"v":2,"ok":false,"error":"nope"}"#,
            r#"{"v":2,"ok":false,"error":"nope","reason":"not_running","redirected":true}"#,
            r#"{"v":3,"ok":true,"mechanism":"claude-control-request"}"#,
        ] {
            let value: serde_json::Value = serde_json::from_str(frame).expect("a JSON frame");
            assert_eq!(
                validator.is_valid(&value),
                serde_json::from_str::<ControlResponse>(frame).is_ok(),
                "the schema and the reader disagree about {frame}"
            );
        }
    }

    #[test]
    fn a_recorded_event_carries_its_reason_only_when_refused() {
        let at = UtcInstant::from_epoch(1_786_190_000);
        let served = ControlResponse::served(ControlShape::ClaudeControlRequest)
            .record(ControlVerb::Interrupt, at.clone());
        assert!(served.is_served());
        assert_eq!(served.verb(), ControlVerb::Interrupt);
        assert_eq!(served.at(), &at);
        assert_eq!(served.reason(), None);
        assert!(!served.is_redirected());
        let value = serde_json::to_value(&served).unwrap();
        assert_eq!(value["outcome"], "served");
        assert!(value.get("reason").is_none());
        // Omitted for a plain stop, so a consumer written against v1 reads the
        // record it always did.
        assert!(value.get("redirected").is_none(), "{value}");

        let redirected = ControlResponse::redirected(ControlShape::ClaudeControlRequest)
            .record(ControlVerb::Interrupt, at.clone());
        assert!(redirected.is_redirected());
        assert_eq!(
            serde_json::to_value(&redirected).unwrap()["redirected"],
            serde_json::json!(true)
        );

        let refused = ControlResponse::refused("nope", ControlReason::NoActiveTurn)
            .record(ControlVerb::Interrupt, at);
        assert_eq!(refused.reason(), Some(ControlReason::NoActiveTurn));
        assert!(!refused.is_redirected());
        let value = serde_json::to_value(&refused).unwrap();
        assert_eq!(value["outcome"], "refused");
        assert_eq!(value["reason"], "no_active_turn");
    }

    #[test]
    fn every_shape_round_trips_through_its_wire_name() {
        for shape in [
            ControlShape::ClaudeControlRequest,
            ControlShape::CodexAppServer,
            ControlShape::OpencodeHttp,
            ControlShape::AcpCancel,
            ControlShape::CrushHttp,
        ] {
            assert_eq!(ControlShape::from_wire(shape.as_str()), Some(shape));
        }
        assert_eq!(ControlShape::from_wire("nope"), None);
    }

    #[test]
    fn socket_path_sanitizes_the_name_under_the_control_dir() {
        assert_eq!(
            socket_path(Path::new("/store"), "Greet Flow").expect("a short name fits any budget"),
            PathBuf::from("/store/control/greet-flow.sock")
        );
    }

    /// The shape a graph mints for a worker's session — the name that put the
    /// address one byte past `sun_path` on a real host, with every dispatch
    /// reporting an unreachable control channel.
    fn graph_session_name() -> String {
        "node-scope-1786808430370-3422037-worker-skill".to_string()
    }

    #[test]
    fn a_graph_minted_name_addresses_within_the_budget_under_a_default_length_store() {
        // The store this failed under: `$HOME/.local/state/oneharness/sessions`.
        let store = PathBuf::from("/home/nick.guest/.local/state/oneharness/sessions");
        let path = socket_path(&store, &graph_session_name())
            .expect("a default-length store must leave room for a session name");
        assert_eq!(
            socket_address_within_limit(&path),
            Ok(()),
            "address `{}` must be bindable on this platform",
            path.display()
        );
        assert!(path.starts_with(store.join(CONTROL_DIR)));
    }

    #[test]
    fn a_name_too_long_to_spell_is_abbreviated_stably_and_distinctly() {
        // Long enough that the full name cannot fit under any store: the
        // abbreviation is what has to stay addressable.
        let store = PathBuf::from("/home/agent/.local/state/oneharness/sessions");
        let long = format!("{}-alpha", "x".repeat(120));
        let sibling = format!("{}-beta", "x".repeat(120));
        let path = socket_path(&store, &long).expect("shortening keeps this store addressable");
        assert_eq!(socket_address_within_limit(&path), Ok(()));
        // Two processes derive the address independently, so it must be a pure
        // function of `(dir, name)` — and two sessions must never collide on it.
        assert_eq!(
            path,
            socket_path(&store, &long).expect("shortening is deterministic")
        );
        assert_ne!(
            path,
            socket_path(&store, &sibling).expect("a sibling name is addressable too"),
            "names sharing a 120-character prefix must not share a socket"
        );
        let file = path.file_name().and_then(|f| f.to_str()).expect("utf-8");
        assert!(file.starts_with("xxx") && file.ends_with(".sock"), "{file}");
    }

    // Unix-gated with the budget it exercises: off unix there is no `sun_path`
    // to overrun, and `SOCKET_ADDRESS_MAX` says so by being unbounded.
    #[cfg(unix)]
    #[test]
    fn a_multibyte_session_name_is_still_measured_in_bytes() {
        // The budget is BYTES and the abbreviation counts characters, so the two
        // agree only because `sanitize_name` yields ASCII. That coupling is
        // invisible at the call site, and a name of multibyte characters is
        // where it would break: 120 of them are 360 bytes.
        let store = PathBuf::from("/home/agent/.local/state/oneharness/sessions");
        for name in ["日本語のセッション".repeat(20), "é".repeat(120)] {
            let path = socket_path(&store, &name).expect("a multibyte name is addressable");
            assert_eq!(
                socket_address_within_limit(&path),
                Ok(()),
                "address `{}` must be within the byte budget",
                path.display()
            );
            let file = path.file_name().and_then(|f| f.to_str()).expect("utf-8");
            assert!(
                file.is_ascii(),
                "the socket file name must be ASCII: {file}"
            );
        }
    }

    // Unix-gated with the budget it exercises: off unix there is no `sun_path`
    // to overrun, and `SOCKET_ADDRESS_MAX` says so by being unbounded — so no
    // store is ever past it and this refusal has nothing to refuse.
    #[cfg(unix)]
    #[test]
    fn a_store_past_the_budget_is_a_loud_error_naming_the_path_length_and_limit() {
        // A realistic deep store: a session dir inside a versioned worktree,
        // which no shortening of the session name can rescue.
        let store = PathBuf::from(
            "/home/nick.guest/.onevcs/workspaces/github.com-nickderobertis-oneharness-e6efbed8d311/runs/s-3b0f64620cdc/worktree/.oneharness/sessions",
        );
        let error = socket_path(&store, &graph_session_name())
            .expect_err("a store this deep cannot host a socket address");
        assert_eq!(error.limit(), SOCKET_ADDRESS_MAX);
        assert_eq!(error.bytes(), error.path().as_os_str().len() + 1);
        assert!(error.bytes() > error.limit());
        let said = error.to_string();
        assert!(said.contains(&error.path().display().to_string()), "{said}");
        assert!(said.contains(&error.bytes().to_string()), "{said}");
        assert!(said.contains(&error.limit().to_string()), "{said}");
    }

    /// The other side of that gate. A refusal borrowed from a platform whose
    /// limit does not apply is its own silent breakage — a control channel
    /// declined for a `sun_path` the host has no notion of — so the store the
    /// unix test cannot address must still be addressable here.
    #[cfg(not(unix))]
    #[test]
    fn a_deep_store_is_addressable_where_there_is_no_sun_path_to_overrun() {
        let store = PathBuf::from(
            "/home/nick.guest/.onevcs/workspaces/github.com-nickderobertis-oneharness-e6efbed8d311/runs/s-3b0f64620cdc/worktree/.oneharness/sessions",
        );
        let path = socket_path(&store, &graph_session_name())
            .expect("no budget off unix means no store is past it");
        assert_eq!(socket_address_within_limit(&path), Ok(()));
        assert!(path.starts_with(store.join(CONTROL_DIR)));
    }

    // Unix-gated with the budget it exercises: off unix there is no `sun_path`
    // to overrun, and `SOCKET_ADDRESS_MAX` says so by being unbounded.
    #[cfg(unix)]
    #[test]
    fn an_already_resolved_address_is_checked_against_the_same_budget() {
        let short = PathBuf::from("/tmp/store/control/flow.sock");
        assert_eq!(socket_address_within_limit(&short), Ok(()));
        let long = PathBuf::from(format!("/tmp/{}/control/flow.sock", "d".repeat(200)));
        let error = socket_address_within_limit(&long).expect_err("200 bytes of directory");
        assert_eq!(error.bytes(), long.as_os_str().len() + 1);
        assert_eq!(error.limit(), SOCKET_ADDRESS_MAX);
    }

    #[test]
    fn pool_key_ignores_env_order_but_not_values() {
        let a = pool_key(
            "codex",
            &[
                ("CODEX_HOME".into(), Some("/a".into())),
                ("OTHER".into(), None),
            ],
            &[],
        );
        let reordered = pool_key(
            "codex",
            &[
                ("OTHER".into(), None),
                ("CODEX_HOME".into(), Some("/a".into())),
            ],
            &[],
        );
        assert_eq!(a, reordered);

        let different = pool_key("codex", &[("CODEX_HOME".into(), Some("/b".into()))], &[]);
        assert_ne!(a, different);
        assert!(a.as_str().starts_with("codex-"), "{a}");
    }

    #[test]
    fn a_pool_key_is_always_a_safe_segment_whatever_the_caller_passed() {
        // The digest already covers the id, so an id that cannot be a directory
        // name yields a different key rather than a refusal (or a panic) the
        // caller could do nothing with.
        for id in ["codex", "../escape", "a/b", "", "wild name!"] {
            let key = pool_key(id, &[], &[]);
            assert!(
                PoolKey::parse(key.as_str()).is_ok(),
                "`{id}` produced an unusable key `{key}`"
            );
        }
        assert_ne!(pool_key("a/b", &[], &[]), pool_key("ab", &[], &[]));
    }

    #[test]
    fn a_pool_key_that_could_escape_the_pool_root_is_refused() {
        assert!(PoolKey::parse(pool_key("codex", &[], &[]).as_str()).is_ok());
        for bad in [
            "",
            "../escape",
            "a/b",
            "a\\b",
            // `.` names the pool root rather than an entry under it, so a
            // dispatch parsing it would lease and reclaim the whole pool.
            ".",
            "..",
            "x..y",
            &"a".repeat(129),
        ] {
            assert!(
                PoolKey::parse(bad).is_err(),
                "`{bad}` should not be a pool key"
            );
        }
        assert!(serde_json::from_value::<PoolKey>(serde_json::json!("../escape")).is_err());
    }

    #[test]
    fn pool_key_separates_launch_overrides() {
        let plain = pool_key("opencode", &[], &[]);
        let overridden = pool_key("opencode", &[], &["--hostname".into(), "::1".into()]);
        assert_ne!(plain, overridden);
    }

    #[test]
    fn claude_prompt_and_interrupt_frames_match_the_live_protocol() {
        let prompt = prompt_frame(ControlShape::ClaudeControlRequest, "hi").unwrap();
        assert!(prompt.ends_with('\n'), "frames are newline-terminated");
        let prompt: serde_json::Value = serde_json::from_str(&prompt).unwrap();
        assert_eq!(prompt["type"], "user");
        assert_eq!(prompt["message"]["role"], "user");
        assert_eq!(prompt["message"]["content"][0]["type"], "text");
        assert_eq!(prompt["message"]["content"][0]["text"], "hi");

        let interrupt = interrupt_frame(ControlShape::ClaudeControlRequest, "req-1").unwrap();
        assert!(interrupt.ends_with('\n'));
        let interrupt: serde_json::Value = serde_json::from_str(&interrupt).unwrap();
        assert_eq!(interrupt["type"], "control_request");
        assert_eq!(interrupt["request_id"], "req-1");
        assert_eq!(interrupt["request"]["subtype"], "interrupt");
    }

    #[test]
    fn server_backed_shapes_have_no_stdin_frames() {
        for shape in [
            ControlShape::CodexAppServer,
            ControlShape::OpencodeHttp,
            ControlShape::AcpCancel,
            ControlShape::CrushHttp,
        ] {
            assert!(prompt_frame(shape, "hi").is_none());
            assert!(interrupt_frame(shape, "r").is_none());
            assert!(!is_turn_terminal(shape, r#"{"type":"result"}"#));
        }
    }

    #[test]
    fn turn_terminal_recognizes_only_the_result_document() {
        let shape = ControlShape::ClaudeControlRequest;
        assert!(is_turn_terminal(
            shape,
            r#"{"type":"result","subtype":"error_during_execution"}"#
        ));
        assert!(!is_turn_terminal(shape, r#"{"type":"assistant"}"#));
        assert!(!is_turn_terminal(shape, "not json"));
        assert!(!is_turn_terminal(shape, ""));
        // A `result` string appearing inside another document is not terminal.
        assert!(!is_turn_terminal(
            shape,
            r#"{"type":"user","message":{"content":"result"}}"#
        ));
    }
}
