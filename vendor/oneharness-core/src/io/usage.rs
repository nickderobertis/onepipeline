// llmlint: ignore-block[comments_earn_their_place] The zero-turn property is a property of the invocations below, so the header states what changing one has to re-establish; cutting it to a pointer at `docs/harness-usage.md` is what `no_redundant_instruction_pointers` forbids.
//! The zero-turn subscription-headroom probes.
//!
//! Every probe here reads a harness's plan headroom **without the harness taking
//! a model turn** — no user message is written and no turn is completed. That is
//! the property that makes `oneharness usage` usable as a pre-flight check
//! rather than a thing that costs what it measures, and it is a property of the
//! *invocations below*: changing one means re-establishing it. Each probe's
//! observed exchange, and the evidence that it takes no turn, is
//! `docs/harness-usage.md` — including the Cursor credential hazard
//! [`CURSOR_LOGIN_ENVS`] guards against.
//!
//! Parsing is pure and lives in [`crate::domain::usage`]; this module only
//! spawns, writes, reads, and hands bytes over. Every failure — a missing
//! binary, an unauthenticated harness, a malformed payload, a timeout — becomes
//! data in the report, never a panic and never a zero.
// llmlint: ignore-end[comments_earn_their_place]

use std::collections::HashMap;
use std::io::Write;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::domain::config::VariantName;
use crate::domain::harness::{
    codex_model_catalog_setting, has_plugins_toggle, CODEX_ANALYTICS_OFF,
};
use crate::domain::usage::{
    claude_control_response, claude_usage_snapshot_missing, parse_claude_get_usage,
    parse_codex_rate_limits, parse_copilot_http, parse_cursor_about, without_control_chars,
    AuthMode, IdentitySelector, ParsedUsage, ResetCredits, ResetCreditsUnknown, UnavailableReason,
    UnknownReason, UsageAvailability, UsageIdentity, UsageProbe, UsageReport, UsageSupport,
    UtcInstant,
};
use crate::errors::OneharnessError;
use crate::io::model_catalog;
use crate::io::process::{resolve_program, Finish, PipeEvent, Process};

/// The environment variable selecting a Claude Code identity. Per-process, and
/// observed returning different reset days and different credit histories for
/// two subscriptions on one host — which is what makes per-identity attribution
/// real rather than nominal.
pub const CLAUDE_IDENTITY_ENV: &str = "CLAUDE_CONFIG_DIR";
/// The environment variable selecting a codex identity, per-process like
/// [`CLAUDE_IDENTITY_ENV`].
pub const CODEX_IDENTITY_ENV: &str = "CODEX_HOME";
/// GitHub bearer tokens in the precedence Copilot itself documents. The first
/// non-empty one wins, and only its *name* ever reaches the report.
pub const COPILOT_TOKEN_ENVS: &[&str] = &["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"];
/// Cursor's API-key variables, which the probe **removes** from its child.
///
/// This is a safety guard, not a preference. `--api-key`/`CURSOR_API_KEY` is not
/// a per-process selector: it performs a token exchange and calls
/// `setAuthentication`, persisting credentials to the shared store — observed
/// creating `~/.config/cursor/auth.json` where none existed and clobbering a
/// real user login. `about` ignores the key today; masking it means a future
/// release that honors it still cannot make this probe authenticate.
pub const CURSOR_LOGIN_ENVS: &[&str] = &["CURSOR_API_KEY"];

/// The GitHub API base for the Copilot probe. Overridable with
/// `ONEHARNESS_COPILOT_API_BASE` for a GitHub Enterprise host (and for the
/// hermetic tests, which point it at a local server). Must be HTTPS unless it
/// names a loopback host — see the private `COPILOT_PLAINTEXT_HOSTS` list.
pub const COPILOT_API_BASE_ENV: &str = "ONEHARNESS_COPILOT_API_BASE";
const COPILOT_API_BASE_DEFAULT: &str = "https://api.github.com";
/// The only hosts a plaintext `http://` base may name. The request carries the
/// GitHub token in an `Authorization` header, so any other plaintext host would
/// put a live credential on the wire in the clear — a misconfigured base is a
/// refused probe, never a leaked token. Loopback stays permitted because that
/// traffic never leaves the machine, which is what the hermetic tests point the
/// probe at.
const COPILOT_PLAINTEXT_HOSTS: &[&str] = &["127.0.0.1", "localhost", "[::1]"];
/// The program that performs the Copilot HTTP GET. curl is used rather than an
/// in-process TLS stack so the engine keeps its lean dependency tree; when it is
/// absent the probe reports that as data, like any other probe failure.
const COPILOT_HTTP_CLIENT: &str = "curl";
/// Marks the status line curl appends after the body, so the two are separable
/// without a second request. Deliberately unlike anything the endpoint returns.
const COPILOT_STATUS_MARKER: &str = "oneharness-http-status:";

/// The single `get_usage` control request's id, echoed back in the response so a
/// concurrent control message can never be mistaken for the answer.
const CLAUDE_REQUEST_ID: &str = "oneharness-usage-1";
/// The JSON-RPC id of codex's `account/rateLimits/read` request.
const CODEX_RATE_LIMITS_ID: i64 = 2;
/// How many times, in all, the claude probe asks for a snapshot the CLI says it
/// could not fetch right now ([`claude_usage_snapshot_missing`]) before
/// settling on that answer. Each attempt is a fresh `claude -p`, since the
/// control request is answered on the way out of the input stream. Bounded
/// because a credential that stays expired answers so indefinitely, and a
/// probe is a pre-flight check rather than a wait.
const CLAUDE_SNAPSHOT_ATTEMPTS: usize = 3;
/// The pause between those attempts. A retry is taken only when this fits in
/// what is left of the caller's timeout; otherwise the probe settles at once.
const CLAUDE_SNAPSHOT_RETRY_PAUSE: Duration = Duration::from_secs(1);
/// How long a child that has already answered gets to exit on its own before its
/// tree is terminated. Bounded so an idling harness cannot hold the probe.
const EXIT_GRACE: Duration = Duration::from_millis(500);
/// The longest a probe may wait, in seconds — the ceiling every caller gets,
/// not just the CLI. A probe is a pre-flight check whose harness answers in
/// single-digit seconds; an hour is already far past "something is wrong".
///
/// The CLI reads this same constant for its `--timeout` range, so the documented
/// maximum has one source. Enforcing it here as well is what makes it a real
/// boundary: [`probe`] is public API, and a library caller handing it a
/// multi-year `Duration` would otherwise hang a process, or — since
/// `Instant + Duration` **panics** on overflow — take one down.
pub const MAX_TIMEOUT_SECS: u64 = 3_600;
/// [`MAX_TIMEOUT_SECS`] as a [`Duration`].
pub const MAX_TIMEOUT: Duration = Duration::from_secs(MAX_TIMEOUT_SECS);

/// One identity to probe: which probe, which binary, and the exact environment
/// the child gets — the same variant environment `run` builds, so a usage
/// identity is selected by the machinery that already selects a run's identity.
pub struct UsageProbeRequest {
    pub probe: UsageProbe,
    /// The resolved harness binary (unused by [`UsageProbe::CopilotUserEndpoint`],
    /// which spawns no harness).
    pub bin: String,
    pub cwd: Option<PathBuf>,
    /// Environment applied to the child, last write winning — a variant's `env`,
    /// `env_file`, and `env_from`, exactly as `run` assembles them.
    pub env: Vec<(String, String)>,
    /// Variables masked from the child. Applied after [`Self::env`], so a
    /// removal wins over a set of the same name — matching the runner.
    pub env_remove: Vec<String>,
    /// How long to wait for this probe's answer. **Clamped to [`MAX_TIMEOUT`]**
    /// — see [`UsageProbeRequest::effective_timeout`], which is what the probe
    /// and its diagnostics actually use.
    pub timeout: Duration,
    /// The resolved `model_catalog_refresh_seconds` when the codex probe reads
    /// the shared model catalog (`shared_model_catalog`); `None` leaves
    /// Codex to fetch its own. Read only by the codex probe, before its
    /// timeout starts.
    pub model_catalog_refresh_seconds: Option<NonZeroU64>,
}

impl UsageProbeRequest {
    /// The timeout this request is actually run under: the requested value, or
    /// [`MAX_TIMEOUT`] when it exceeds the ceiling. Every deadline and every
    /// timeout message reads from here, so a clamped request is *reported* as
    /// the value it was run under rather than the one that was asked for.
    #[must_use]
    pub fn effective_timeout(&self) -> Duration {
        self.timeout.min(MAX_TIMEOUT)
    }

    /// The instant this probe gives up, computed **once** per probe from
    /// [`Self::effective_timeout`]: every exchange the probe makes — a retry
    /// included — reads against the same one, so asking again can never
    /// outlive the timeout the caller set.
    fn deadline(&self) -> Instant {
        // Clamped at the boundary, so this addition cannot overflow whatever a
        // caller asked for.
        Instant::now() + self.effective_timeout()
    }
}

/// One probed identity: how it was selected, and what was learned.
pub struct ProbedIdentity {
    pub selector: IdentitySelector,
    pub parsed: ParsedUsage,
}

/// Probe one identity. Never fails loudly: every outcome is a [`ParsedUsage`].
#[must_use]
pub fn probe(request: &UsageProbeRequest) -> ProbedIdentity {
    match request.probe {
        UsageProbe::ClaudeGetUsage => probe_claude(request),
        UsageProbe::CodexAppServer => probe_codex(request),
        UsageProbe::CopilotUserEndpoint => probe_copilot(request),
        UsageProbe::CursorAbout => probe_cursor(request),
    }
}

/// The identity an unprobed or unavailable harness is attributed to, so its
/// entry still names the selector a probe *would* have used.
#[must_use]
pub fn selector_for(probe: Option<UsageProbe>, request_env: &EnvView<'_>) -> IdentitySelector {
    match probe {
        Some(UsageProbe::ClaudeGetUsage) => env_path_selector(request_env, CLAUDE_IDENTITY_ENV),
        Some(UsageProbe::CodexAppServer) => env_path_selector(request_env, CODEX_IDENTITY_ENV),
        Some(UsageProbe::CopilotUserEndpoint) => match copilot_token_env(request_env) {
            Some((env, _)) => IdentitySelector::EnvSecret {
                env: env.to_string(),
            },
            None => IdentitySelector::Ambient,
        },
        Some(UsageProbe::CursorAbout) | None => IdentitySelector::Ambient,
    }
}

/// The child's effective view of the environment: the request's overrides
/// layered over the ambient process environment, with removals applied last
/// (exactly the order [`crate::io::runner`] applies them to a `Command`).
pub struct EnvView<'a> {
    env: &'a [(String, String)],
    env_remove: &'a [String],
}

impl<'a> EnvView<'a> {
    #[must_use]
    pub fn new(env: &'a [(String, String)], env_remove: &'a [String]) -> Self {
        Self { env, env_remove }
    }

    /// The value the child would see for `name`, treating empty as unset (an
    /// empty `CLAUDE_CONFIG_DIR` selects nothing).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<String> {
        if self.env_remove.iter().any(|key| key == name) {
            return None;
        }
        self.env
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var(name).ok())
            .filter(|value| !value.is_empty())
    }
}

impl UsageProbeRequest {
    fn env_view(&self) -> EnvView<'_> {
        EnvView::new(&self.env, &self.env_remove)
    }
}

fn env_path_selector(env: &EnvView<'_>, name: &str) -> IdentitySelector {
    match env.get(name) {
        Some(path) => IdentitySelector::EnvPath {
            env: name.to_string(),
            path,
        },
        None => IdentitySelector::Ambient,
    }
}

fn copilot_token_env(env: &EnvView<'_>) -> Option<(&'static str, String)> {
    COPILOT_TOKEN_ENVS
        .iter()
        .find_map(|name| env.get(name).map(|value| (*name, value)))
}

struct ProbeCapture {
    /// The answer an `on_line` callback recognized, if any.
    answer: Option<ParsedUsage>,
    stdout: String,
    stderr: String,
    /// Whether the deadline expired before the child answered or exited.
    timed_out: bool,
    /// A spawn failure, which short-circuits everything else.
    spawn_error: Option<String>,
}

impl ProbeCapture {
    /// The failure this capture represents when no answer was recognized:
    /// the spawn error, a timeout, or the child's own stderr.
    fn failure(&self, what: &str, timeout: Duration) -> ParsedUsage {
        if let Some(error) = &self.spawn_error {
            return ParsedUsage::unknown(UnknownReason::ProbeFailed {
                message: error.clone(),
            });
        }
        let message = if self.timed_out {
            format!(
                "{what} did not answer within {}s (raise --timeout if the harness is slow to start)",
                timeout.as_secs()
            )
        } else {
            let detail = first_meaningful_line(&self.stderr)
                .or_else(|| first_meaningful_line(&self.stdout))
                .unwrap_or_else(|| "no output".to_string());
            format!("{what} exited without an answer: {detail}")
        };
        ParsedUsage::unknown(UnknownReason::ProbeFailed { message })
    }
}

/// Bound on a diagnostic excerpt of a harness's own output, in **characters**
/// so a multi-byte line is never split mid-code-point.
const DIAGNOSTIC_CHARS: usize = 300;

fn first_meaningful_line(text: &str) -> Option<String> {
    let line = text.lines().find_map(|line| {
        let flat = without_control_chars(line).trim().to_string();
        (!flat.is_empty()).then_some(flat)
    })?;
    Some(match line.char_indices().nth(DIAGNOSTIC_CHARS) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line,
    })
}

/// When a probe closes the child's stdin, which decides whether an answer that
/// is still in flight ever arrives.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StdinAfterRequests {
    /// Close it as soon as the requests are written: the child treats EOF as
    /// "that was the whole input" and does not start work until it sees one
    /// (`curl --config -`), so holding it open would hang the probe.
    Close,
    /// Hold it open until the answer arrives or the deadline passes, then close.
    /// A long-lived server shuts *down* on EOF, dropping whatever it had not
    /// answered yet: `codex app-server` replies to `initialize` synchronously
    /// but reads rate limits asynchronously, so closing stdin behind the three
    /// requests loses the one answer the probe came for, on an account whose
    /// headroom was readable the whole time.
    HoldUntilAnswered,
}

/// Spawn `argv`, write `stdin_lines` (each newline-terminated), then read stdout
/// line by line until `on_line` recognizes an answer, the child exits, or
/// `deadline` passes. `stdin_after` decides when the child's stdin is closed; it
/// is closed either way before the child is given its moment to exit. The
/// process tree is always terminated on the way out, so a harness that would
/// idle waiting for more input cannot outlive the probe.
///
/// The deadline is the probe's ([`UsageProbeRequest::deadline`]), handed in
/// rather than computed here, so a probe that converses more than once still
/// answers within one timeout.
fn converse(
    request: &UsageProbeRequest,
    deadline: Instant,
    argv: &[String],
    stdin_lines: &[String],
    stdin_after: StdinAfterRequests,
    mut on_line: impl FnMut(&str) -> Option<ParsedUsage>,
) -> ProbeCapture {
    // Resolved exactly as the runner resolves a harness binary: a probe reads the
    // same bare registry name (`codex`), and on Windows that name is an npm
    // `.cmd` shim `CreateProcess` cannot find. Skipping this reported a harness
    // every other command could spawn as `program not found`.
    let mut command = Command::new(resolve_program(&argv[0]));
    command
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &request.env {
        command.env(key, value);
    }
    for key in &request.env_remove {
        command.env_remove(key);
    }

    let mut process = match Process::spawn(command) {
        Ok(process) => process,
        Err(error) => {
            return ProbeCapture {
                answer: None,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
                spawn_error: Some(format!("failed to spawn `{}`: {error}", argv[0])),
            }
        }
    };

    // Written inline rather than on a helper thread: the payload is a handful of
    // short lines built here, orders of magnitude below any pipe buffer, so this
    // write cannot block.
    let mut stdin = process.take_stdin();
    if let Some(handle) = stdin.as_mut() {
        for line in stdin_lines {
            if handle.write_all(line.as_bytes()).is_err() || handle.write_all(b"\n").is_err() {
                break;
            }
        }
        let _ = handle.flush();
    }
    if stdin_after == StdinAfterRequests::Close {
        drop(stdin.take());
    }

    let mut answer = None;
    let mut timed_out = false;
    loop {
        match process.recv_stdout_until(deadline) {
            PipeEvent::Data(chunk) => {
                let line = String::from_utf8_lossy(&chunk);
                if let Some(parsed) = on_line(line.trim()) {
                    answer = Some(parsed);
                    break;
                }
            }
            PipeEvent::Closed => break,
            PipeEvent::Deadline => {
                timed_out = true;
                break;
            }
        }
    }

    // The answer is in hand (or the deadline is up), so close stdin now if it was
    // held open: EOF is how a server-shaped harness is told to shut down, and it
    // has to precede the grace below for that shutdown to be what happens there.
    drop(stdin.take());

    // The probe has what it came for and the child's stdin is now closed, so
    // a well-behaved harness is on its way out: give it a bounded moment to exit
    // by itself rather than signalling a process that is mid-shutdown (which
    // costs it whatever it flushes at exit). A harness that idles anyway — or one
    // that blew the deadline — still gets its whole tree torn down.
    let finish = if timed_out {
        Finish::Terminate
    } else {
        match process.wait_until(Instant::now() + EXIT_GRACE) {
            Ok(Some(_)) => Finish::Exited,
            Ok(None) | Err(_) => Finish::Terminate,
        }
    };
    let finished = process.finish(finish);
    ProbeCapture {
        answer,
        stdout: finished.stdout,
        stderr: finished.stderr,
        timed_out,
        spawn_error: None,
    }
}

/// The setting sources a headroom probe loads. `user` alone, because Claude Code
/// runs the working directory's project `SessionStart` hooks before answering a
/// control request, and the answer must not depend on where the probe was
/// pointed. User settings stay loaded: they are the identity's own and
/// [`CLAUDE_IDENTITY_ENV`] still selects which of them applies, so per-identity
/// attribution is unaffected.
const CLAUDE_SETTING_SOURCES: &str = "user";

/// The exact zero-turn invocation: `-p` with stream-json in and out, an empty
/// tool set, user settings only, and no prompt. The control request rides stdin;
/// no user message is ever sent, so the session completes zero turns.
///
/// `--setting-sources` raises no compatibility floor: every build new enough to
/// take `--tools`, which this invocation already carried, takes it too.
fn claude_argv(bin: &str) -> Vec<String> {
    vec![
        bin.to_string(),
        "-p".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--tools".to_string(),
        String::new(),
        "--setting-sources".to_string(),
        CLAUDE_SETTING_SOURCES.to_string(),
    ]
}

fn claude_request_line() -> String {
    serde_json::json!({
        "type": "control_request",
        "request_id": CLAUDE_REQUEST_ID,
        "request": {"subtype": "get_usage"},
    })
    .to_string()
}

/// One `get_usage` exchange, against the probe's shared deadline. Alongside the
/// capture: whether the recognized answer was the transient no-snapshot one,
/// decided by the domain's predicate rather than by reading its message back.
fn claude_exchange(request: &UsageProbeRequest, deadline: Instant) -> (ProbeCapture, bool) {
    let mut snapshot_missing = false;
    let capture = converse(
        request,
        deadline,
        &claude_argv(&request.bin),
        &[claude_request_line()],
        // `claude -p` answers the queued control request on its way out of the
        // input stream, so EOF is what completes this exchange rather than what
        // cuts it short — verified against the real CLI, which reports headroom
        // under exactly this close.
        StdinAfterRequests::Close,
        |line| {
            let value: Value = serde_json::from_str(line).ok()?;
            if value
                .pointer("/response/request_id")
                .and_then(Value::as_str)
                != Some(CLAUDE_REQUEST_ID)
            {
                return None;
            }
            let payload = claude_control_response(&value)?;
            snapshot_missing = claude_usage_snapshot_missing(payload);
            Some(parse_claude_get_usage(payload))
        },
    );
    (capture, snapshot_missing)
}

/// Why the claude probe stopped asking for a snapshot the CLI said it could
/// not fetch, so the settled reason can say it.
#[derive(Clone, Copy)]
enum SnapshotStop {
    /// The CLI answered so [`CLAUDE_SNAPSHOT_ATTEMPTS`] times.
    Bound,
    /// Another pause would not fit in what was left of the timeout.
    Budget,
    /// A further attempt produced no answer at all.
    Unanswered,
}

/// The transient reason as the probe settles it: how many times in a row the
/// CLI answered so and, when that is fewer than the bound, why it was not
/// asked again.
fn claude_snapshot_missing_settled(
    transient: ParsedUsage,
    answered: usize,
    stop: SnapshotStop,
) -> ParsedUsage {
    let UsageAvailability::Unknown {
        reason: UnknownReason::ProbeFailed { message },
    } = transient.availability
    else {
        return transient;
    };
    let times = match answered {
        1 => "once".to_string(),
        n => format!("{n} times in a row"),
    };
    let stopped = match stop {
        SnapshotStop::Bound => "",
        SnapshotStop::Budget => "; the timeout left no room to ask again",
        SnapshotStop::Unanswered => "; a further attempt produced no answer",
    };
    ParsedUsage::unknown(UnknownReason::ProbeFailed {
        message: format!("{message} (answered so {times}{stopped})"),
    })
}

fn probe_claude(request: &UsageProbeRequest) -> ProbedIdentity {
    let selector = env_path_selector(&request.env_view(), CLAUDE_IDENTITY_ENV);
    let deadline = request.deadline();
    // The transient answer in hand, if the CLI has given one, and how many times.
    let mut transient: Option<(ParsedUsage, usize)> = None;
    let parsed = loop {
        let (mut capture, snapshot_missing) = claude_exchange(request, deadline);
        let Some(answer) = capture.answer.take() else {
            // A retry that produced nothing does not overwrite the answer the CLI
            // already gave: reporting the deadline here would send the reader
            // after a hung harness when the diagnosis was already in hand.
            break match transient {
                Some((answer, answered)) => {
                    claude_snapshot_missing_settled(answer, answered, SnapshotStop::Unanswered)
                }
                None => capture.failure(
                    "claude-code's `get_usage` control request",
                    request.effective_timeout(),
                ),
            };
        };
        if !snapshot_missing {
            // Drift, an API-key or logged-out answer, or headroom: each is the
            // CLI's settled word, so none of them is asked twice.
            break answer;
        }
        let answered = transient.as_ref().map_or(1, |(_, answered)| answered + 1);
        if answered >= CLAUDE_SNAPSHOT_ATTEMPTS {
            break claude_snapshot_missing_settled(answer, answered, SnapshotStop::Bound);
        }
        if deadline.saturating_duration_since(Instant::now()) <= CLAUDE_SNAPSHOT_RETRY_PAUSE {
            break claude_snapshot_missing_settled(answer, answered, SnapshotStop::Budget);
        }
        transient = Some((answer, answered));
        std::thread::sleep(CLAUDE_SNAPSHOT_RETRY_PAUSE);
    };
    ProbedIdentity { selector, parsed }
}

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Real Codex cannot be read in the deterministic gate; the live phase `oh_codex_feature_flag_order` (scripts/e2e-lib.sh) checks `--disable plugins` against it.
/// `codex app-server --disable plugins --disable apps -c analytics.enabled=false
/// [-c model_catalog_json=<catalog>] --stdio`. Plugins, apps and analytics are
/// off whatever the config says: a rate-limits read loads no plugin, calls no
/// connector and reports nothing, and the plugin catalog Codex would otherwise
/// download on start (~29 MB) is most of what a probe costs. The shared model
/// catalog, when the config keeps it on, spares the probe Codex's own catalog
/// fetch.
fn codex_argv(bin: &str, model_catalog: Option<&Path>) -> Vec<String> {
    let mut argv = vec![
        bin.to_string(),
        "app-server".to_string(),
        "--disable".to_string(),
        "plugins".to_string(),
        "--disable".to_string(),
        "apps".to_string(),
        "-c".to_string(),
        CODEX_ANALYTICS_OFF.to_string(),
    ];
    if let Some(path) = model_catalog {
        argv.extend(["-c".to_string(), codex_model_catalog_setting(path)]);
    }
    argv.push("--stdio".to_string());
    argv
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// The JSON-RPC `initialize` oneharness opens every `codex app-server`
/// conversation with: the usage probe's, and the shared model catalog's
/// refresh, which sends it and nothing else.
pub(crate) fn codex_initialize_request() -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": "oneharness",
                "title": null,
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": null,
        },
    })
    .to_string()
}

/// `initialize`, the `initialized` notification, then the rate-limits read. All
/// three are written before reading, matching the driver this was sourced from;
/// the response is matched by JSON-RPC id, so ordering is not assumed.
///
/// Calling convention detail worth preserving: `account/rateLimits/read` takes
/// `params: null` (its sibling `account/read` instead *requires* `params: {}`).
fn codex_request_lines() -> Vec<String> {
    vec![
        codex_initialize_request(),
        serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}).to_string(),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": CODEX_RATE_LIMITS_ID,
            "method": "account/rateLimits/read",
            "params": null,
        })
        .to_string(),
    ]
}

fn probe_codex(request: &UsageProbeRequest) -> ProbedIdentity {
    let selector = env_path_selector(&request.env_view(), CODEX_IDENTITY_ENV);
    let catalog = request
        .model_catalog_refresh_seconds
        .and_then(|refresh_seconds| {
            let choice = model_catalog::resolve(&model_catalog::CatalogLaunch {
                bin: &request.bin,
                cwd: request.cwd.as_deref(),
                env: &request.env,
                env_remove: &request.env_remove,
                refresh_seconds,
            });
            if let Some(warning) = choice.warning {
                eprintln!("oneharness: warning: codex usage probe: {warning}");
            }
            choice.catalog
        });
    let mut capture = converse(
        request,
        request.deadline(),
        &codex_argv(&request.bin, catalog.as_deref()),
        &codex_request_lines(),
        StdinAfterRequests::HoldUntilAnswered,
        {
            |line| {
                let value: Value = serde_json::from_str(line).ok()?;
                if value.get("id").and_then(Value::as_i64) != Some(CODEX_RATE_LIMITS_ID) {
                    return None;
                }
                Some(parse_codex_rate_limits(&value))
            }
        },
    );
    let parsed = capture.answer.take().unwrap_or_else(|| {
        capture.failure(
            "codex's `account/rateLimits/read`",
            request.effective_timeout(),
        )
    });
    ProbedIdentity { selector, parsed }
}

fn cursor_argv(bin: &str) -> Vec<String> {
    vec![
        bin.to_string(),
        "about".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ]
}

fn probe_cursor(request: &UsageProbeRequest) -> ProbedIdentity {
    // `about` prints one pretty-printed document, so it is parsed after exit
    // rather than line by line.
    let guarded = UsageProbeRequest {
        probe: request.probe,
        bin: request.bin.clone(),
        cwd: request.cwd.clone(),
        env: request.env.clone(),
        env_remove: request
            .env_remove
            .iter()
            .cloned()
            .chain(CURSOR_LOGIN_ENVS.iter().map(|name| (*name).to_string()))
            .collect(),
        timeout: request.timeout,
        model_catalog_refresh_seconds: None,
    };
    let capture = converse(
        &guarded,
        guarded.deadline(),
        &cursor_argv(&request.bin),
        &[],
        StdinAfterRequests::Close,
        |_| None,
    );
    let parsed = match serde_json::from_str::<Value>(capture.stdout.trim()) {
        Ok(value) if value.is_object() => parse_cursor_about(&value),
        _ => capture.failure(
            "cursor's `about --format json`",
            request.effective_timeout(),
        ),
    };
    ProbedIdentity {
        selector: IdentitySelector::Ambient,
        parsed,
    }
}

fn probe_copilot(request: &UsageProbeRequest) -> ProbedIdentity {
    let env = request.env_view();
    let token = copilot_token_env(&env);
    let selector = match &token {
        Some((name, _)) => IdentitySelector::EnvSecret {
            env: (*name).to_string(),
        },
        None => IdentitySelector::Ambient,
    };
    let Some((_, token)) = token else {
        return ProbedIdentity {
            selector,
            parsed: ParsedUsage::unknown(UnknownReason::ProbeFailed {
                message: format!(
                    "no GitHub token to read Copilot quota with: set one of {}. \
                     Copilot's own stored OAuth login lives in the OS keyring, which \
                     oneharness cannot read",
                    COPILOT_TOKEN_ENVS.join(", ")
                ),
            }),
        };
    };
    let base = env
        .get(COPILOT_API_BASE_ENV)
        .unwrap_or_else(|| COPILOT_API_BASE_DEFAULT.to_string());
    let parsed = match copilot_config(&base, &token) {
        Ok(config) => copilot_fetch(request, config),
        Err(message) => ParsedUsage::unknown(UnknownReason::ProbeFailed { message }),
    };
    ProbedIdentity { selector, parsed }
}

/// Build the curl config read from stdin. The token never reaches the argv (and
/// so never reaches the process table); both it and the base URL are validated
/// first, because a quote or newline in either would otherwise be injected into
/// curl's own config grammar.
fn copilot_config(base: &str, token: &str) -> Result<String, String> {
    let base = base.trim_end_matches('/');
    if !carries_the_token_safely(base) || !is_config_safe(base) {
        return Err(format!(
            "{COPILOT_API_BASE_ENV} must be an HTTPS URL (plaintext http:// only for a \
             loopback host) with no quotes, backslashes, whitespace, or control \
             characters (got `{base}`)"
        ));
    }
    if !is_config_safe(token) {
        // The value itself is never echoed.
        return Err(
            "the GitHub token contains quotes, backslashes, whitespace, or control characters \
             and cannot be forwarded safely"
                .to_string(),
        );
    }
    Ok(format!(
        "url = \"{base}/copilot_internal/user\"\n\
         header = \"Authorization: Bearer {token}\"\n\
         header = \"Accept: application/json\"\n\
         header = \"User-Agent: oneharness/{}\"\n\
         silent\n\
         show-error\n\
         write-out = \"\\n{COPILOT_STATUS_MARKER}%{{http_code}}\\n\"\n",
        env!("CARGO_PKG_VERSION")
    ))
}

/// Whether this base may carry the bearer token: HTTPS anywhere, plaintext only
/// to a loopback host ([`COPILOT_PLAINTEXT_HOSTS`]). The host is the authority
/// up to the first `/`, minus a numeric port — so `http://127.0.0.1@elsewhere`
/// is the host `elsewhere` names it as, not loopback.
fn carries_the_token_safely(base: &str) -> bool {
    if base.starts_with("https://") {
        return true;
    }
    let Some(authority) = base.strip_prefix("http://") else {
        return false;
    };
    let authority = authority.split('/').next().unwrap_or_default();
    let host = authority
        .rsplit_once(':')
        .filter(|(_, port)| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()))
        .map_or(authority, |(host, _)| host);
    COPILOT_PLAINTEXT_HOSTS.contains(&host)
}

fn is_config_safe(value: &str) -> bool {
    !value.is_empty()
        && !value
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '"' || c == '\\')
}

fn copilot_fetch(request: &UsageProbeRequest, config: String) -> ParsedUsage {
    let argv = vec![
        COPILOT_HTTP_CLIENT.to_string(),
        "--config".to_string(),
        "-".to_string(),
    ];
    // curl reads its config to EOF before it makes the request, so this close is
    // what starts the exchange rather than what ends it.
    let capture = converse(
        request,
        request.deadline(),
        &argv,
        &[config],
        StdinAfterRequests::Close,
        |_| None,
    );
    match split_copilot_response(&capture.stdout) {
        Some((status, body)) => parse_copilot_http(status, &body),
        None => capture.failure(
            &format!("the Copilot quota request via `{COPILOT_HTTP_CLIENT}`"),
            request.effective_timeout(),
        ),
    }
}

/// Split curl's output into the response body and its status code. The marker
/// line is written by curl *after* the body, so the last one wins — a body that
/// happened to contain the marker cannot displace the real status.
fn split_copilot_response(stdout: &str) -> Option<(u16, String)> {
    let (at, line) = stdout
        .lines()
        .enumerate()
        .filter(|(_, line)| line.trim_start().starts_with(COPILOT_STATUS_MARKER))
        .last()?;
    let status: u16 = line
        .trim()
        .trim_start_matches(COPILOT_STATUS_MARKER)
        .parse()
        .ok()?;
    let body = stdout.lines().take(at).collect::<Vec<_>>().join("\n");
    Some((status, body))
}

/// Probes run concurrently: identity selection is per-process for every harness
/// here, so nothing is shared between them. Bounded so a `--all` sweep on a
/// small machine does not start eight subprocesses at once.
const MAX_PARALLEL_PROBES: usize = 4;

/// What a [`report`] sweep probes, as plain data.
///
/// Every field is one thing the CLI resolves from its own flags, so an embedder
/// states them rather than inheriting them from a process it does not own.
#[derive(Debug, Clone, Default)]
pub struct UsageRequest {
    /// Probe every supported harness. Also the default when `harness` is empty.
    pub all: bool,
    /// Harness id(s) to probe, `<id>` or `<id>:<variant>`.
    pub harness: Vec<String>,
    /// Harness id(s) to drop from an all-harness sweep.
    pub exclude: Vec<String>,
    /// `--bin ID=PATH` overrides, in the CLI's own spelling.
    pub bin: Vec<String>,
    /// Working directory for the probes, and where config discovery starts.
    pub cwd: Option<PathBuf>,
    /// Per-probe timeout. `None` takes [`DEFAULT_TIMEOUT`].
    pub timeout: Option<Duration>,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery — the repeatable `--config` flag. Each later file
    /// (its own `extends` chain beneath it) overrides the earlier ones; empty
    /// means discovery. See [`crate::io::config::load_layers`].
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Vec<PathBuf>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. Takes
    /// precedence over `config`: set together, no file is loaded.
    pub no_config: bool,
}

/// The per-probe timeout a request that names none gets. Generous next to the
/// single-digit seconds a probe's session startup takes, because the cost of
/// being too tight is a spurious "unknown" for a caller who does have headroom.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Report every selected identity's remaining subscription headroom, without
/// spending a model turn.
///
/// This is the whole `oneharness usage` verb as a call: it **returns** the
/// [`UsageReport`] the CLI prints, so a Rust consumer checking provider health
/// reads it in process rather than spawning the binary and parsing its stdout.
/// [`probe`] is the single-identity primitive underneath; this is the sweep —
/// selection, variant identities, configured binaries, bounded concurrency, and
/// the one clock read that stamps the report.
///
/// Nothing a harness does can fail this call: a missing binary, an
/// unauthenticated harness, a malformed payload, a timeout, and even a probe
/// that panics are all normalized identities in the report.
///
/// # Errors
///
/// Only a genuine usage error: an unknown harness id, an undeclared
/// `<id>:<variant>`, a malformed `--bin` value, or configuration that cannot be
/// loaded — each raised before any probe runs.
pub fn report(request: &UsageRequest) -> Result<UsageReport, OneharnessError> {
    // Probing defaults to every harness; only naming one narrows the sweep.
    // `exclude` therefore means "the sweep, minus these" rather than an empty
    // selection.
    let all = request.all || request.harness.is_empty();
    let specs = crate::domain::select::select_specs(all, &request.harness, &request.exclude)?;
    let selected_ids = crate::domain::select::dedupe_exact_ids(&request.harness);

    let project_start = request
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let loaded = crate::io::config::load(&request.config, request.no_config, &project_start)?;
    // A variant that was never declared is a usage error, not an identity
    // silently collapsed onto the base harness's credentials.
    for id in request.harness.iter().chain(&request.exclude) {
        if let Some((base, variant)) = id.split_once(':') {
            if loaded.config.variant_for(id).is_none() {
                return Err(OneharnessError::UnknownHarnessVariant {
                    id: id.clone(),
                    base: base.to_string(),
                    variant: variant.to_string(),
                });
            }
        }
    }
    let overrides = crate::io::detect::BinOverrides::parse(&request.bin)?
        .with_config_bins(config_bins(&loaded.config));
    let timeout = request.timeout.unwrap_or(DEFAULT_TIMEOUT);

    // One plan per selected identity, built before any probe runs so a config
    // fault fails loudly up front rather than half way through a sweep.
    let mut plans = Vec::with_capacity(specs.len());
    for (index, spec) in specs.iter().enumerate() {
        let id = selected_ids
            .get(index)
            .map_or_else(|| spec.id.to_string(), Clone::clone);
        let env = crate::io::identity::variant_environment(&loaded.config, &id, &project_start)?;
        let env_remove = loaded
            .config
            .variant_for(&id)
            .map_or_else(Vec::new, |variant| variant.unset_env.clone());
        let resolved = crate::io::detect::resolve_named(spec, &id, &overrides);
        let variant = variant_of(&id)?;
        let model_catalog_refresh_seconds = (has_plugins_toggle(spec.id)
            && loaded.config.shared_model_catalog_for(&id))
        .then(|| loaded.config.model_catalog_refresh_seconds_for(&id));
        plans.push(Plan {
            id,
            variant,
            base: spec.id,
            support: spec.usage,
            bin: resolved.bin,
            available: resolved.available,
            env,
            env_remove,
            cwd: request.cwd.clone(),
            timeout,
            model_catalog_refresh_seconds,
        });
    }

    Ok(UsageReport::new(now_utc(), probe_all(&plans)))
}

/// Everything one identity's probe needs, resolved from config and the request.
struct Plan {
    /// The composed selector (`claude-code:work`), which is how a consumer joins
    /// this entry back to the `run` invocation it describes.
    id: String,
    /// The variant half of [`Plan::id`], parsed once here so the probe path —
    /// which must report an outcome for every identity, including a crashed one
    /// — has no way left to fail. See [`variant_of`].
    variant: Option<VariantName>,
    /// The registry id, which is what the report's `harness` field carries.
    base: &'static str,
    support: UsageSupport,
    bin: String,
    available: bool,
    env: Vec<(String, String)>,
    env_remove: Vec<String>,
    cwd: Option<PathBuf>,
    timeout: Duration,
    /// [`UsageProbeRequest::model_catalog_refresh_seconds`], resolved from
    /// config.
    model_catalog_refresh_seconds: Option<NonZeroU64>,
}

/// The variant half of a composed selector (`claude-code:work`), so two
/// subscriptions of one harness stay separately attributed in the report.
///
/// The name is checked against the declared variants before any plan is built,
/// so this parse restates that check as the type the report's field holds
/// rather than performing a second one. It stays fallible so an id that somehow
/// reached here is refused loudly, rather than attributed to a name the report
/// could never be read back with.
fn variant_of(id: &str) -> Result<Option<VariantName>, OneharnessError> {
    let Some((base, name)) = id.split_once(':') else {
        return Ok(None);
    };
    name.parse::<VariantName>()
        .map(Some)
        .map_err(|_| OneharnessError::UnknownHarnessVariant {
            id: id.to_string(),
            base: base.to_string(),
            variant: name.to_string(),
        })
}

impl Plan {
    /// Probe this identity, or record why no probe ran. Never fails: every
    /// outcome — including a missing binary — is a normalized identity.
    fn probe(&self) -> UsageIdentity {
        fault_inject_probe_panic(&self.id);
        self.probe_inner().with_variant(self.variant.clone())
    }

    fn probe_inner(&self) -> UsageIdentity {
        let env = EnvView::new(&self.env, &self.env_remove);
        let Some(probe) = self.support.probe() else {
            // An affirmative "no headroom to report", stated rather than
            // omitted. `expect` is sound: a support tier is either probed or
            // carries a reason, and this is the not-probed half.
            let reason = self
                .support
                .unprobed_reason()
                .expect("a tier with no probe carries a reason");
            return UsageIdentity::new(self.base, selector_for(None, &env), unavailable(reason));
        };
        if probe.spawns_harness() && !self.available {
            return UsageIdentity::new(
                self.base,
                selector_for(Some(probe), &env),
                ParsedUsage::unknown(UnknownReason::binary_missing(&self.bin)),
            );
        }
        let probed = self::probe(&UsageProbeRequest {
            probe,
            bin: self.bin.clone(),
            cwd: self.cwd.clone(),
            env: self.env.clone(),
            env_remove: self.env_remove.clone(),
            timeout: self.timeout,
            model_catalog_refresh_seconds: self.model_catalog_refresh_seconds,
        });
        UsageIdentity::new(self.base, probed.selector, probed.parsed)
    }

    /// What this identity reports when its probe panicked outright. Nothing was
    /// learned, which is exactly `unknown` — and saying so keeps the rest of the
    /// report readable.
    fn crashed(&self) -> UsageIdentity {
        self.probe_failed("the probe stopped unexpectedly")
    }

    fn worker_start_failed(&self, error: &std::io::Error) -> UsageIdentity {
        self.probe_failed(&format!("could not start probe worker: {error}"))
    }

    fn probe_failed(&self, message: &str) -> UsageIdentity {
        UsageIdentity::new(
            self.base,
            selector_for(
                self.support.probe(),
                &EnvView::new(&self.env, &self.env_remove),
            ),
            ParsedUsage::unknown(UnknownReason::ProbeFailed {
                message: message.to_string(),
            }),
        )
        .with_variant(self.variant.clone())
    }
}

/// Comma-separated harness ids whose probe must panic outright, read by
/// [`fault_inject_probe_panic`].
///
/// A panicking probe is a *bug*, not an input: no payload, timeout, or missing
/// binary produces one, so [`probe_all`]'s containment — itself the fix for a
/// probe that took a whole report down — has nothing that can drive it through
/// the CLI a consumer runs. This injects one, and is compiled only into the
/// `mock-harness` test build, exactly like the mock harness fixture binary, so
/// it cannot exist in a shipped `oneharness`.
#[cfg(feature = "mock-harness")]
const PANIC_PROBE_ENV: &str = "MOCK_PANIC_PROBE";

#[cfg(feature = "mock-harness")]
fn fault_inject_probe_panic(id: &str) {
    let faulted = std::env::var(PANIC_PROBE_ENV).unwrap_or_default();
    assert!(
        !faulted.split(',').any(|name| name == id),
        "fault-injected probe panic for `{id}`"
    );
}

#[cfg(not(feature = "mock-harness"))]
fn fault_inject_probe_panic(_id: &str) {}

#[cfg(feature = "mock-harness")]
fn fault_inject_thread_failure(id: &str) -> std::io::Result<()> {
    let faulted = std::env::var("MOCK_FAIL_PROBE_THREAD").unwrap_or_default();
    if faulted.split(',').any(|name| name == id) {
        return Err(std::io::Error::other("fault-injected resource exhaustion"));
    }
    Ok(())
}

#[cfg(not(feature = "mock-harness"))]
fn fault_inject_thread_failure(_id: &str) -> std::io::Result<()> {
    Ok(())
}

fn unavailable(reason: UnavailableReason) -> ParsedUsage {
    ParsedUsage {
        auth_mode: AuthMode::Unknown,
        plan: None,
        availability: UsageAvailability::Unavailable { reason },
        reset_credits: ResetCredits::Unknown {
            reason: ResetCreditsUnknown::NoReader,
        },
    }
}

/// Run every plan's probe concurrently, preserving selection order.
///
/// Each probe is joined individually, so one that panics becomes *that
/// identity's* `unknown` instead of taking the whole report down with it. A
/// report is the deliverable here; losing seven readings because the eighth
/// harness misbehaved is the failure mode this command exists to avoid.
fn probe_all(plans: &[Plan]) -> Vec<UsageIdentity> {
    let mut identities = Vec::with_capacity(plans.len());
    // Bounded concurrency without a work queue: the selection is at most the
    // registry's size, so a chunk at a time is both simpler and enough.
    for chunk in plans.chunks(MAX_PARALLEL_PROBES.max(1)) {
        std::thread::scope(|scope| {
            let running: Vec<_> = chunk
                .iter()
                .map(|plan| {
                    fault_inject_thread_failure(&plan.id)?;
                    std::thread::Builder::new().spawn_scoped(scope, || plan.probe())
                })
                .collect();
            for (plan, worker) in chunk.iter().zip(running) {
                identities.push(match worker {
                    Ok(handle) => handle.join().unwrap_or_else(|_| plan.crashed()),
                    Err(error) => plan.worker_start_failed(&error),
                });
            }
        });
    }
    identities
}

fn config_bins(config: &crate::domain::config::FileConfig) -> HashMap<String, String> {
    let mut bins: HashMap<String, String> = config
        .harness
        .iter()
        .filter_map(|(id, harness)| harness.bin.clone().map(|bin| (id.clone(), bin)))
        .collect();
    for (base, harness) in &config.harness {
        for name in harness.variant.keys() {
            let id = format!("{base}:{name}");
            if let Some(bin) = config.bin_for(&id) {
                bins.insert(id, bin.to_string());
            }
        }
    }
    bins
}

/// The single clock read for the whole report, so the domain stays pure and
/// every identity is stamped with one observation time. Minted as a
/// [`UtcInstant`], canonical by construction from epoch seconds, so the
/// report's documented UTC contract holds without a re-parse.
fn now_utc() -> UtcInstant {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    UtcInstant::from_epoch(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view<'a>(env: &'a [(String, String)], remove: &'a [String]) -> EnvView<'a> {
        EnvView::new(env, remove)
    }

    fn request(timeout: Duration) -> UsageProbeRequest {
        UsageProbeRequest {
            probe: UsageProbe::CursorAbout,
            bin: "cursor-agent".to_string(),
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
            timeout,
            model_catalog_refresh_seconds: None,
        }
    }

    /// Both docs spell Copilot's token precedence out in prose, which is the
    /// only place a reader learns which variable wins. Adding or reordering one
    /// in [`COPILOT_TOKEN_ENVS`] would leave that prose confidently wrong, so
    /// the ordered list is generated from the constant and searched for.
    #[test]
    fn documented_copilot_token_precedence_tracks_the_constant() {
        // Prose wraps across lines, so compare on collapsed whitespace.
        fn collapsed(text: &str) -> String {
            text.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        fn joined(separator: &str) -> String {
            COPILOT_TOKEN_ENVS
                .iter()
                .map(|env| format!("`{env}`"))
                .collect::<Vec<_>>()
                .join(separator)
        }

        let reference = collapsed(include_str!("../../../../docs/harness-usage.md"));
        let precedence = joined(" > ");
        assert!(
            reference.contains(&precedence),
            "docs/harness-usage.md must state the precedence as `{precedence}`"
        );

        let readme = collapsed(include_str!("../../../../README.md"));
        let prose = joined(", then ");
        assert!(
            readme.contains(&prose),
            "README.md must state the precedence as `{prose}`"
        );
    }

    /// The codex probe's argv is stated in docs/harness-usage.md, where a
    /// reader learns what the probe starts; it is generated from `codex_argv`.
    #[test]
    fn documented_codex_probe_argv_tracks_codex_argv() {
        let reference = include_str!("../../../../docs/harness-usage.md")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let spawned = format!("Spawn `{}`", codex_argv("codex", None).join(" "));
        assert!(
            reference.contains(&spawned),
            "docs/harness-usage.md must state the probe as {spawned}"
        );
    }

    /// The retry policy is spelled out in prose, which is the only place a
    /// reader learns how many times the probe asks and how far apart. Changing
    /// either constant would leave that prose confidently wrong, so the
    /// sentence is generated from the constants and searched for.
    #[test]
    fn documented_claude_snapshot_retry_tracks_the_constants() {
        fn collapsed(text: &str) -> String {
            text.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        fn in_words(n: usize) -> String {
            match n {
                1 => "one".to_string(),
                2 => "two".to_string(),
                3 => "three".to_string(),
                n => n.to_string(),
            }
        }

        let reference = collapsed(include_str!("../../../../docs/harness-usage.md"));
        let policy = format!(
            "{} attempts in all, {} second apart",
            in_words(CLAUDE_SNAPSHOT_ATTEMPTS),
            in_words(
                usize::try_from(CLAUDE_SNAPSHOT_RETRY_PAUSE.as_secs()).expect("a small pause")
            )
        );
        assert!(
            reference.contains(&policy),
            "docs/harness-usage.md must state the retry policy as `{policy}`"
        );
        assert_eq!(
            CLAUDE_SNAPSHOT_RETRY_PAUSE,
            Duration::from_secs(CLAUDE_SNAPSHOT_RETRY_PAUSE.as_secs()),
            "the prose states the pause in whole seconds"
        );
    }

    #[test]
    fn a_library_caller_gets_the_documented_timeout_ceiling_too() {
        // `probe` is public API, so the ceiling has to be enforced here rather
        // than only by the CLI's flag range: a sibling tool depending on the
        // engine would otherwise hang its own process for as long as it asked.
        assert_eq!(
            request(Duration::from_secs(MAX_TIMEOUT_SECS * 24)).effective_timeout(),
            MAX_TIMEOUT,
            "an over-ceiling request runs under the ceiling"
        );
        assert_eq!(
            request(Duration::MAX).effective_timeout(),
            MAX_TIMEOUT,
            "including one that would overflow the deadline arithmetic outright"
        );

        // A deadline built from the clamped value cannot overflow, which is the
        // panic this boundary exists to make unreachable.
        assert!(Instant::now()
            .checked_add(request(Duration::MAX).effective_timeout())
            .is_some());

        // Anything at or under the ceiling is honored exactly — the clamp is a
        // ceiling, not a rewrite of every timeout.
        for secs in [1, 60, MAX_TIMEOUT_SECS] {
            assert_eq!(
                request(Duration::from_secs(secs)).effective_timeout(),
                Duration::from_secs(secs)
            );
        }
    }

    #[test]
    fn a_clamped_probe_reports_the_timeout_it_actually_waited() {
        // The diagnostic must not quote a duration nothing waited for: a caller
        // reading "did not answer within 157680000s" would go looking for an
        // hours-long hang that never happened.
        let capture = ProbeCapture {
            answer: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
            spawn_error: None,
        };
        let over_ceiling = request(Duration::from_secs(MAX_TIMEOUT_SECS * 10));

        let parsed = capture.failure("a probe", over_ceiling.effective_timeout());

        let UnknownReason::ProbeFailed { message } = (match parsed.availability {
            crate::domain::usage::UsageAvailability::Unknown { reason } => reason,
            other => panic!("expected a probe failure, got {other:?}"),
        }) else {
            panic!("expected a probe failure");
        };
        assert!(
            message.contains(&format!("{MAX_TIMEOUT_SECS}s")),
            "the message must quote the enforced timeout: {message}"
        );
    }

    #[test]
    fn a_removed_variable_is_invisible_to_the_child_even_when_also_set() {
        // The runner applies removals last, so a removal wins over a set of the
        // same name; the selector must describe what the child actually sees.
        let env = vec![(CLAUDE_IDENTITY_ENV.to_string(), "/tmp/a".to_string())];
        let remove = vec![CLAUDE_IDENTITY_ENV.to_string()];

        assert_eq!(view(&env, &remove).get(CLAUDE_IDENTITY_ENV), None);
        assert_eq!(
            view(&env, &[]).get(CLAUDE_IDENTITY_ENV),
            Some("/tmp/a".to_string())
        );
    }

    #[test]
    fn the_last_write_of_a_variable_wins_and_an_empty_value_is_unset() {
        let env = vec![
            (CODEX_IDENTITY_ENV.to_string(), "/tmp/one".to_string()),
            (CODEX_IDENTITY_ENV.to_string(), "/tmp/two".to_string()),
        ];
        assert_eq!(
            view(&env, &[]).get(CODEX_IDENTITY_ENV),
            Some("/tmp/two".to_string())
        );

        let empty = vec![(CODEX_IDENTITY_ENV.to_string(), String::new())];
        assert_eq!(view(&empty, &[]).get(CODEX_IDENTITY_ENV), None);
    }

    #[test]
    fn the_identity_selector_names_the_directory_never_a_credential() {
        let env = vec![(
            CLAUDE_IDENTITY_ENV.to_string(),
            "/home/u/.claude".to_string(),
        )];
        assert_eq!(
            selector_for(Some(UsageProbe::ClaudeGetUsage), &view(&env, &[])),
            IdentitySelector::EnvPath {
                env: CLAUDE_IDENTITY_ENV.to_string(),
                path: "/home/u/.claude".to_string(),
            }
        );

        let token = vec![("GH_TOKEN".to_string(), "ghs_secret".to_string())];
        let selector = selector_for(Some(UsageProbe::CopilotUserEndpoint), &view(&token, &[]));
        assert_eq!(
            selector,
            IdentitySelector::EnvSecret {
                env: "GH_TOKEN".to_string()
            }
        );
        assert!(
            !selector.key().contains("ghs_secret"),
            "a credential must never reach the report"
        );
    }

    #[test]
    fn copilot_token_precedence_follows_the_documented_order() {
        let all = vec![
            ("GITHUB_TOKEN".to_string(), "c".to_string()),
            ("GH_TOKEN".to_string(), "b".to_string()),
            ("COPILOT_GITHUB_TOKEN".to_string(), "a".to_string()),
        ];
        assert_eq!(
            copilot_token_env(&view(&all, &[])),
            Some(("COPILOT_GITHUB_TOKEN", "a".to_string()))
        );

        let removed = vec!["COPILOT_GITHUB_TOKEN".to_string()];
        assert_eq!(
            copilot_token_env(&view(&all, &removed)),
            Some(("GH_TOKEN", "b".to_string()))
        );
    }

    #[test]
    fn the_claude_probe_sends_one_control_request_and_no_user_message() {
        let argv = claude_argv("claude");
        assert_eq!(
            argv,
            vec![
                "claude",
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--tools",
                "",
                "--setting-sources",
                "user",
            ],
            "the zero-turn invocation is load-bearing: an empty tool set, no prompt, \
             and user settings only — project settings would make the answer's latency \
             whatever session-start work the probe's working directory registers"
        );

        let line = claude_request_line();
        let value: Value = serde_json::from_str(&line).expect("a JSON line");
        assert_eq!(value["type"], "control_request");
        assert_eq!(value["request"]["subtype"], "get_usage");
        assert!(
            !line.contains("\"user\""),
            "sending a user message would make the probe cost a model turn"
        );
    }

    #[test]
    fn the_codex_probe_initializes_then_reads_rate_limits() {
        let lines = codex_request_lines();
        assert_eq!(lines.len(), 3);
        let parsed: Vec<Value> = lines
            .iter()
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect();
        assert_eq!(parsed[0]["method"], "initialize");
        assert_eq!(parsed[1]["method"], "initialized");
        assert_eq!(parsed[2]["method"], "account/rateLimits/read");
        assert_eq!(parsed[2]["id"], CODEX_RATE_LIMITS_ID);
        assert!(
            parsed[2]["params"].is_null(),
            "account/rateLimits/read takes params: null (account/read instead requires {{}})"
        );
    }

    #[test]
    fn the_copilot_config_keeps_the_token_off_the_argv() {
        let config = copilot_config("https://api.github.com/", "ghs_abc").expect("a safe config");

        assert!(config.contains("url = \"https://api.github.com/copilot_internal/user\""));
        assert!(config.contains("header = \"Authorization: Bearer ghs_abc\""));
        let argv = [
            COPILOT_HTTP_CLIENT.to_string(),
            "--config".to_string(),
            "-".to_string(),
        ];
        assert!(
            !argv.iter().any(|arg| arg.contains("ghs_abc")),
            "the token rides stdin so it never reaches the process table"
        );
    }

    #[test]
    fn an_injectable_token_or_base_is_refused_without_echoing_the_value() {
        let error = copilot_config("https://api.github.com", "gh\"\nheader = \"X: y")
            .expect_err("a token carrying config syntax is refused");
        assert!(!error.contains("header = "), "the value must not be echoed");

        assert!(copilot_config("ftp://elsewhere", "ghs_abc").is_err());
        assert!(copilot_config("https://a b", "ghs_abc").is_err());
    }

    /// The token rides an `Authorization` header, so plaintext reaches only a
    /// host the packets never leave the machine to get to.
    #[test]
    fn only_https_or_a_loopback_base_may_carry_the_bearer_token() {
        for permitted in [
            "https://api.github.com",
            "https://ghe.internal.example",
            "http://127.0.0.1:8080",
            "http://localhost:1/x",
            "http://[::1]:4321",
            "http://[::1]",
        ] {
            assert!(
                copilot_config(permitted, "ghs_abc").is_ok(),
                "`{permitted}` keeps the token off the clear wire"
            );
        }
        for refused in [
            "http://api.github.com",
            "http://ghe.internal.example:8080",
            "http://127.0.0.1@evil.example",
            "http://localhost.evil.example",
            "http://127.0.0.1:notaport",
        ] {
            let error =
                copilot_config(refused, "ghs_abc").expect_err("`{refused}` must not be probed");
            assert!(
                error.contains("HTTPS"),
                "the refusal must say what is required: {error}"
            );
        }
    }

    #[test]
    fn the_status_marker_is_read_from_the_last_line_not_a_body_that_mimics_it() {
        let stdout = format!(
            "{{\"copilot_plan\":\"individual\",\"note\":\"{COPILOT_STATUS_MARKER}999\"}}\n\
             {COPILOT_STATUS_MARKER}200\n"
        );

        let (status, body) = split_copilot_response(&stdout).expect("a status and a body");

        assert_eq!(status, 200);
        assert!(body.contains("copilot_plan"));
        assert_eq!(split_copilot_response("no marker at all"), None);
    }
}
