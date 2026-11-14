//! The shared Codex app-server: one `codex app-server --listen unix://PATH`
//! per pool key, shared by every concurrent `--control` dispatch with that key
//! and managed by the generic sidecar pool ([`crate::io::server_pool`]).
//!
//! **Why each piece is here.** A shared server outlives every dispatch, does
//! not notice a client that died, and keeps a finished thread loaded — so the
//! pool, not the server, has to own each dispatch's footprint on it:
//!
//! - The server is launched with a scrubbed environment
//!   ([`crate::domain::app_server::server_env`]) plus a fresh server marker, and
//!   each dispatch's thread carries the rest of its own environment and a fresh
//!   dispatch marker in `shell_environment_policy`. The markers are how the
//!   pool finds what to stop ([`crate::io::marked_processes`]): a thread's
//!   commands are the server's children with nothing in their ancestry naming
//!   the thread.
//! - Each dispatch holds a lease whose file notes its marker, thread and live
//!   turn ([`LeaseNote`]). Whenever a lease ends — the dispatch finishing, or
//!   its holder found dead — the pool stops its marked commands, then releases
//!   its thread with [`RELEASE_THREAD_METHOD`].
//! - A pool-owned sweeper process beside the server ([`SWEEPER_ENV`]) finds a
//!   dead holder within [`DEAD_LEASE_SWEEP_INTERVAL`] even when no other
//!   dispatch is alive, and stops the server once the last lease has ended and
//!   the linger has passed.
//! - A dispatch reads only frames of its own thread off its connection
//!   ([`crate::domain::app_server::is_own_frame`]): the server broadcasts
//!   `thread/status/changed` for every thread to every connection.

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::domain::app_server::{
    self as rules, EnvPlatform, ShellEnvironmentPolicy, RELEASE_THREAD_METHOD, SERVER_MARKER_VAR,
};
use crate::domain::control::{AbsolutePath, AppServerReport, PoolKey, ServerAddress};
use crate::domain::report::{Capture, Status};
use crate::io::cancel::{cancellation_requested, CancelToken};
use crate::io::control::{ControlHandle, ReplyBoard};
use crate::io::marked_processes::{stop_marked, ProcessMarker};
use crate::io::process::{Finish, Process};
use crate::io::runner::{Job, StreamStep};
use crate::io::server_pool::{
    self, DispatchMarker, EntryLock, LaunchPlan, LeaseHolder, LeaseNote, LeasedThread, ServerLease,
    ServerMarker, ServerRecord, ThreadId, TurnId, DEAD_LEASE_STOP_DEADLINE,
    DEAD_LEASE_SWEEP_INTERVAL, DEFAULT_LINGER, SERVER_STOP_DEADLINE,
};
use crate::io::websocket::{handshake, Incoming, LocalStream, WsReader, WsWriter};

/// The variable that turns a process started from this binary into a pool
/// entry's sweeper; its value is the entry directory. Read by
/// [`serve_sweeper_if_requested`], which a binary driving pooled dispatches
/// calls first thing in `main`.
pub const SWEEPER_ENV: &str = "ONEHARNESS_INTERNAL_APP_SERVER_SWEEPER";

/// A linger, in milliseconds, replacing [`DEFAULT_LINGER`] for the servers this
/// process starts. Internal: it exists so the hermetic journeys can watch a
/// server stop without waiting a minute.
pub const LINGER_ENV: &str = "ONEHARNESS_INTERNAL_APP_SERVER_LINGER_MS";

/// A directory replacing Codex's own socket rendezvous directory
/// (`/tmp/codex-daemon-<euid>`) as the one a launch prepares. Internal: it
/// exists so a hermetic journey can prepare a directory of its own rather than
/// the host's, which every Codex on it shares.
pub const DAEMON_DIR_ENV: &str = "ONEHARNESS_INTERNAL_CODEX_DAEMON_DIR";

/// How long a dispatch waits for a server it started (or found) to accept a
/// connection, unless its own timeout is shorter.
const READY_WINDOW: Duration = Duration::from_secs(60);

/// How long `--version` may take before the executable is called unusable.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// How long releasing a thread waits for the server's answer.
const RELEASE_REPLY_DEADLINE: Duration = Duration::from_secs(5);

/// The JSON-RPC ids this module sends on a dispatch's connection outside its
/// dialogue start here, far above any id the dialogue mints.
const OWN_REQUEST_IDS: u64 = 1 << 40;

/// The longest a frame naming some thread is held before its dispatch knows
/// its own thread. Bounded so a server broadcasting other threads' frames
/// before answering `thread/start` cannot grow this process.
pub const PRE_THREAD_BUFFER: usize = 4096;

const SOCKET_DIR: &str = "sock";
const SOCKET_FILE: &str = "app-server.sock";
const SWEEPER_FILE: &str = "sweeper.json";
const SWEEPER_LOG: &str = "sweeper.log";

static SWEEPER_HOSTED: AtomicBool = AtomicBool::new(false);

/// Run as a pool entry's sweeper when this process was started as one, and
/// otherwise record that this binary can host one.
///
/// A binary that drives pooled Codex dispatches calls this first thing in
/// `main`. The pool starts the sweeper by re-running the current executable
/// with [`SWEEPER_ENV`] set, so a binary that never calls this would run its
/// own `main` instead; the pool therefore starts a sweeper only from a process
/// that has called it. Without one, a dead holder is still found — by the next
/// pool operation of any dispatch with the same key — and an idle server is
/// still stopped by the next one after its linger.
pub fn serve_sweeper_if_requested() {
    SWEEPER_HOSTED.store(true, Ordering::SeqCst);
    if let Some(entry) = std::env::var_os(SWEEPER_ENV) {
        let entry = Path::new(&entry);
        if let Err(why) = sweepable_entry(entry) {
            eprintln!(
                "oneharness: {SWEEPER_ENV}={} is not a shared app-server pool entry: {why}",
                entry.display()
            );
            std::process::exit(2);
        }
        run_sweeper(entry);
        std::process::exit(0);
    }
}

/// Whether `entry` is a pool entry this process's pool keeps: an existing
/// directory named by a pool key, directly under the pool's root. A sweeper
/// reconciles and stops servers, so it is never pointed anywhere else.
fn sweepable_entry(entry: &Path) -> Result<(), String> {
    let root = server_pool::resolve_root(None).ok_or("no state directory holds a pool")?;
    let named = entry
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| PoolKey::parse(name).is_ok());
    if !named {
        return Err("its name is no pool key".to_string());
    }
    let parent = entry
        .parent()
        .and_then(|parent| std::fs::canonicalize(parent).ok());
    let root = std::fs::canonicalize(&root).map_err(|err| format!("{}: {err}", root.display()))?;
    // The entry itself, not what it points at: a symlink named like an entry
    // would lead the sweeper out of the pool.
    let real_dir = std::fs::symlink_metadata(entry).is_ok_and(|meta| meta.file_type().is_dir());
    if parent.as_deref() != Some(root.as_path()) || !real_dir {
        return Err(format!("it is not a directory under {}", root.display()));
    }
    Ok(())
}

/// One dispatch's plan for a turn on a shared server, decided before anything
/// starts: what launches the server and keys it, and what its own thread
/// carries.
#[derive(Debug, Clone)]
pub struct PooledDispatch {
    /// The configured Codex executable (`argv[0]`).
    executable: String,
    /// Every process-level argument after `app-server`.
    process_args: Vec<String>,
    plugins: bool,
    /// The environment the server is launched with, before its marker.
    server_env: Vec<(String, String)>,
    /// The dispatch marker's value.
    marker: DispatchMarker,
    policy: ShellEnvironmentPolicy,
}

impl PooledDispatch {
    /// The plan for a dispatch whose own launch would be `argv` (`<codex>
    /// app-server <args…>`) under the ambient environment with `env` applied
    /// and `env_remove` removed, working in `cwd`. `None` when `argv` is not an
    /// app-server launch, or when an `env`/`env_remove` entry is one no process
    /// environment can hold (an empty name, `=` or NUL in a name, NUL in a
    /// value) — the dispatch then runs its own launch, which reports it.
    #[must_use]
    pub fn plan(
        argv: &[String],
        env: &[(String, String)],
        env_remove: &[String],
        cwd: &Path,
        plugins: bool,
    ) -> Option<Self> {
        let holdable = |name: &str| !name.is_empty() && !name.contains(['=', '\0']);
        if !env
            .iter()
            .all(|(name, value)| holdable(name) && !value.contains('\0'))
            || !env_remove.iter().all(|name| holdable(name))
        {
            return None;
        }
        let (executable, rest) = argv.split_first()?;
        let (subcommand, process_args) = rest.split_first()?;
        if subcommand != "app-server" {
            return None;
        }
        let platform = EnvPlatform::current();
        let dispatch_env = dispatch_environment(env, env_remove, cwd);
        let server_env = rules::server_env(&dispatch_env, platform);
        let marker = DispatchMarker::fresh();
        let mut with_marker = server_env.clone();
        with_marker.push((SERVER_MARKER_VAR.to_string(), String::new()));
        let policy =
            ShellEnvironmentPolicy::for_dispatch(&dispatch_env, &with_marker, &marker, platform);
        Some(PooledDispatch {
            executable: executable.clone(),
            process_args: process_args.to_vec(),
            plugins,
            server_env,
            marker,
            policy,
        })
    }

    /// The `config` map the dispatch's `thread/start` / `thread/resume` carry.
    #[must_use]
    pub fn thread_config(&self) -> Value {
        self.policy.thread_config()
    }

    /// The dispatch's own `shell_environment_policy`, typed.
    #[must_use]
    pub fn policy(&self) -> &ShellEnvironmentPolicy {
        &self.policy
    }

    /// The dispatch marker's value.
    #[must_use]
    pub fn marker(&self) -> &str {
        self.marker.as_str()
    }

    fn process_marker(&self) -> ProcessMarker {
        ProcessMarker {
            name: rules::DISPATCH_MARKER_VAR.to_string(),
            value: self.marker.as_str().to_string(),
        }
    }
}

/// The dispatch's environment as its own child would have had it: the ambient
/// one, `env` applied, `env_remove` removed, `PWD` its working directory.
/// Variables that are not UTF-8 cannot travel in a JSON-RPC frame and are left
/// out.
fn dispatch_environment(
    env: &[(String, String)],
    env_remove: &[String],
    cwd: &Path,
) -> Vec<(String, String)> {
    let platform = EnvPlatform::current();
    let same = |a: &str, b: &str| match platform {
        EnvPlatform::Unix => a == b,
        EnvPlatform::Windows => a.eq_ignore_ascii_case(b),
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, value) in std::env::vars_os() {
        match (name.into_string(), value.into_string()) {
            (Ok(name), Ok(value)) => out.push((name, value)),
            (name, _) => eprintln!(
                "oneharness: warning: the variable {} is not UTF-8, so it cannot travel to a shared codex app-server's thread; that thread's commands run without it",
                match &name {
                    Ok(name) => name.clone(),
                    Err(raw) => raw.to_string_lossy().into_owned(),
                }
            ),
        }
    }
    let mut set = |name: &str, value: &str| {
        out.retain(|(existing, _)| !same(existing, name));
        out.push((name.to_string(), value.to_string()));
    };
    if let Some(pwd) = cwd.to_str() {
        set("PWD", pwd);
    }
    for (name, value) in env {
        set(name, value);
    }
    out.retain(|(name, _)| !env_remove.iter().any(|removed| same(removed, name)));
    out.sort();
    out
}

/// A lease on a running shared server, with the socket it answers on.
pub(crate) struct AppServerLease {
    lease: ServerLease,
    key: PoolKey,
}

impl AppServerLease {
    /// The server this lease holds.
    #[must_use]
    pub(crate) fn record(&self) -> &ServerRecord {
        self.lease.record()
    }

    /// The pool entry directory.
    #[must_use]
    pub(crate) fn entry(&self) -> &Path {
        self.lease.entry()
    }

    /// The server as the run's report names it.
    #[must_use]
    pub(crate) fn report(&self) -> AppServerReport {
        AppServerReport {
            pid: std::num::NonZeroU32::new(self.record().pid.get())
                .expect("a recorded pid is never 0 by construction"),
            pool_key: self.key.clone(),
        }
    }

    fn annotate(&self, note: &LeaseNote) {
        if let Err(err) = self.lease.annotate(note) {
            eprintln!("oneharness: warning: could not note the app-server lease: {err}");
        }
    }
}

/// The linger this process's servers are started under.
fn linger() -> Duration {
    let Ok(raw) = std::env::var(LINGER_ENV) else {
        return DEFAULT_LINGER;
    };
    raw.parse::<u64>().map_or_else(
        |_| {
            eprintln!(
                "oneharness: warning: ignoring {LINGER_ENV}={raw:?}: not whole milliseconds; the server lingers the default {}s",
                DEFAULT_LINGER.as_secs()
            );
            DEFAULT_LINGER
        },
        Duration::from_millis,
    )
}

/// Lease the shared server `dispatch` keys to under `root`, starting one if
/// none is running, and connect to it. A server that exits before accepting a
/// connection is started once more; one that is merely slow is waited for up
/// to `ready_within`.
fn lease_server(
    root: &Path,
    dispatch: &PooledDispatch,
    ready_within: Duration,
) -> Result<(AppServerLease, LocalStream), String> {
    let executable = resolve_executable(&dispatch.executable)?;
    std::fs::create_dir_all(root).map_err(|err| {
        format!(
            "could not prepare the app-server pool at {}: {err}",
            root.display()
        )
    })?;
    let version = probe_version(&executable, &dispatch.server_env, root)?;
    let key = rules::pool_key(
        &executable,
        &version,
        &dispatch.server_env,
        &dispatch.process_args,
        dispatch.plugins,
    );
    let entry = root.join(key.as_str());
    let socket_dir = entry.join(SOCKET_DIR);
    private_dir(&socket_dir)
        .map_err(|err| format!("could not prepare the app-server socket directory: {err}"))?;
    let socket = socket_dir.join(SOCKET_FILE);
    let address = AbsolutePath::new(&socket)
        .map(|path| ServerAddress::UnixSocket { path })
        .map_err(|err| format!("the app-server socket path is unusable: {err}"))?;
    let mut template = vec![executable.clone(), "app-server".to_string()];
    template.extend(dispatch.process_args.iter().cloned());
    template.extend(rules::LISTEN_ARGS.iter().map(|arg| (*arg).to_string()));

    prepare_daemon_dir();

    let mut attempts = 2;
    loop {
        attempts -= 1;
        let marker = ServerMarker::fresh();
        let plan = LaunchPlan::scrubbed(
            template.clone(),
            address.clone(),
            dispatch.server_env.clone(),
            marker,
        )
        .map_err(|err| format!("could not plan the app-server launch: {err}"))?;
        let lease = server_pool::acquire(root, &key, &plan, linger())
            .map_err(|err| format!("could not start the shared codex app-server: {err}"))?;
        ensure_sweeper(lease.entry());
        let record = lease.record().clone();
        let until = Instant::now() + ready_within;
        let connected = loop {
            match LocalStream::connect(&socket) {
                Ok(stream) => break Ok(stream),
                Err(err) => {
                    if !record.is_running() {
                        break Err(format!(
                            "the shared codex app-server (pid {}) exited before accepting a connection on {}: {err}; {}",
                            record.pid.get(),
                            socket.display(),
                            server_pool::early_exit(lease.entry(), &record)
                        ));
                    }
                    if Instant::now() >= until {
                        return Err(format!(
                            "the shared codex app-server (pid {}) did not accept a connection on {} within {}s: {err}",
                            record.pid.get(),
                            socket.display(),
                            ready_within.as_secs()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        };
        match connected {
            Ok(stream) => {
                return Ok((
                    AppServerLease {
                        lease,
                        key: key.clone(),
                    },
                    stream,
                ))
            }
            Err(message) => {
                drop(lease);
                if attempts == 0 {
                    return Err(message);
                }
            }
        }
    }
}

/// Leave Codex's socket rendezvous directory ([`rules::codex_daemon_dir`],
/// for this euid) in the one shape Codex accepts, when it is ours and only its
/// mode is wrong: one another tool created first at its own umask's mode
/// outlives that tool, and every shared server refuses to start meanwhile.
///
/// Only that case is repaired, and only by tightening: a directory owned by
/// someone else, or anything that is not a directory, is left for Codex to
/// refuse with the reason the caller is then shown. An absent directory is
/// left for Codex to create.
fn prepare_daemon_dir() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        let dir = match std::env::var_os(DAEMON_DIR_ENV).filter(|dir| !dir.is_empty()) {
            Some(dir) => std::path::PathBuf::from(dir),
            None => {
                let Ok(tmp) = std::fs::canonicalize("/tmp") else {
                    return;
                };
                // SAFETY: `geteuid` takes nothing and cannot fail.
                rules::codex_daemon_dir(&tmp, unsafe { libc::geteuid() })
            }
        };
        // Checked and tightened through one handle, opened without following a
        // link: nothing can swap the path between the check and the chmod.
        let Ok(handle) = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(&dir)
        else {
            return;
        };
        let Ok(meta) = handle.metadata() else {
            return;
        };
        // SAFETY: as above.
        let ours = meta.uid() == unsafe { libc::geteuid() };
        let mode = rules::CODEX_DAEMON_DIR_MODE;
        // llmlint: ignore[changed_behavior_has_e2e] A directory another user owns, or a chmod refused on one this user owns, needs a second account or root to stage; unprivileged journeys prove the owned-and-loose repair and the not-a-directory refusal (`a_shared_server_starts_once_its_loose_socket_directory_is_made_private`), and either case left alone reaches the caller as Codex's own refusal through the early-exit report.
        if ours && meta.mode() & 0o777 != mode {
            if let Err(err) = handle.set_permissions(std::fs::Permissions::from_mode(mode)) {
                eprintln!(
                    "oneharness: warning: could not make codex's socket directory {} private (mode {mode:04o}), so a shared codex app-server will refuse it: {err}",
                    dir.display()
                );
            }
        }
    }
}

/// `dir`, created if absent, reachable by its owner alone.
fn private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// The configured executable's resolved path: found on `PATH` when it is a bare
/// name, and with every symlink followed, so two spellings of one install key
/// one server.
fn resolve_executable(configured: &str) -> Result<String, String> {
    let found = which::which(configured)
        .map_err(|err| format!("cannot find the codex executable `{configured}`: {err}"))?;
    let resolved = std::fs::canonicalize(&found).unwrap_or(found);
    resolved.into_os_string().into_string().map_err(|_| {
        format!("the codex executable `{configured}` resolves to a path that is not UTF-8")
    })
}

/// `<executable> --version`'s output, asked under the server's own scrubbed
/// environment, and in the pool's `root` rather than the dispatch's directory,
/// so it answers for the release the server will be: two dispatches in two
/// directories key one server.
fn probe_version(
    executable: &str,
    server_env: &[(String, String)],
    root: &Path,
) -> Result<String, String> {
    let mut command = std::process::Command::new(executable);
    command
        .arg("--version")
        .current_dir(root)
        .env_clear()
        .envs(server_env.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Tethered: a caller killed mid-probe leaves no deadline to end a probe
    // that never answers, nor anything to stop it holding the caller's output.
    let mut process = Process::spawn_tethered(command)
        .map_err(|err| format!("cannot run `{executable} --version`: {err}"))?;
    let status = process
        .wait_until(Instant::now() + VERSION_PROBE_TIMEOUT)
        .ok()
        .flatten();
    let finished = process.finish(if status.is_some() {
        Finish::Exited
    } else {
        Finish::Terminate
    });
    match status {
        Some(status) if status.success() => Ok(finished.stdout),
        Some(status) => Err(format!(
            "`{executable} --version` failed ({status}): {}",
            finished.stderr.trim()
        )),
        None => Err(format!(
            "`{executable} --version` did not answer within {}s",
            VERSION_PROBE_TIMEOUT.as_secs()
        )),
    }
}

/// A JSON-RPC connection to a shared server.
pub(crate) struct Connection {
    reader: WsReader,
    writer: Arc<Mutex<WsWriter>>,
    next_id: u64,
    /// Notifications read while the last request awaited its reply — at most
    /// [`SEEN_WHILE_AWAITING`] — so one that arrives before the reply it
    /// follows (a shell turn's `turn/completed`) is not lost.
    seen: std::collections::VecDeque<Value>,
}

/// The most notifications a connection keeps from one request's wait.
const SEEN_WHILE_AWAITING: usize = 1024;

impl Connection {
    fn open(stream: LocalStream) -> io::Result<Self> {
        let (reader, writer) = handshake(stream)?;
        Ok(Connection {
            reader,
            writer,
            next_id: OWN_REQUEST_IDS,
            seen: std::collections::VecDeque::new(),
        })
    }

    fn send(&self, frame: &Value) -> io::Result<()> {
        self.writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .send_text(&frame.to_string())
    }

    /// Send `method` and wait up to `deadline` for its answer, skipping every
    /// other frame. `Ok(None)` when none came in time; an error once the
    /// connection is gone.
    pub(crate) fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> io::Result<Option<Value>> {
        let id = self.next_id;
        self.next_id += 1;
        self.seen.clear();
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        self.await_reply(id, deadline)
    }

    fn await_reply(&mut self, id: u64, deadline: Duration) -> io::Result<Option<Value>> {
        let until = Instant::now() + deadline;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.reader
                .set_read_timeout(Some(left.min(Duration::from_millis(100))))?;
            match self.reader.receive()? {
                Incoming::Text(text) => {
                    let Ok(frame) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    if frame.get("method").is_none()
                        && frame.get("id").and_then(Value::as_u64) == Some(id)
                    {
                        return Ok(Some(frame));
                    }
                    if frame.get("method").is_some() && self.seen.len() < SEEN_WHILE_AWAITING {
                        self.seen.push_back(frame);
                    }
                }
                Incoming::Idle => {}
                Incoming::Closed => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the shared codex app-server closed the connection",
                    ))
                }
            }
        }
    }

    /// Wait up to `deadline` for a notification of `method` naming `thread` —
    /// one already read while the last request awaited its reply included —
    /// skipping every other frame.
    fn await_notification(
        &mut self,
        method: &str,
        thread: &str,
        deadline: Duration,
    ) -> io::Result<Option<Value>> {
        let matches =
            |frame: &Value| frame["method"] == method && rules::frame_thread(frame) == Some(thread);
        if let Some(at) = self.seen.iter().position(matches) {
            return Ok(self.seen.remove(at));
        }
        let until = Instant::now() + deadline;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.reader
                .set_read_timeout(Some(left.min(Duration::from_millis(100))))?;
            match self.reader.receive()? {
                Incoming::Text(text) => {
                    let Ok(frame) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    if matches(&frame) {
                        return Ok(Some(frame));
                    }
                }
                Incoming::Idle => {}
                Incoming::Closed => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the shared codex app-server closed the connection",
                    ))
                }
            }
        }
    }

    fn close(&self) {
        self.writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .close();
    }
}

/// The `initialize` a client sends before anything else on a connection.
fn initialize_params() -> Value {
    json!({"clientInfo": {"name": "oneharness", "title": "oneharness", "version": env!("CARGO_PKG_VERSION")}})
}

/// End a lease's footprint on its server: stop every process carrying the
/// dispatch's marker, then release its thread so the server holds nothing for
/// it. `connection` is one already open and initialized.
fn release_dispatch(connection: &mut Connection, marker: &ProcessMarker, thread: Option<&str>) {
    stop_dispatch_commands(marker, DEAD_LEASE_STOP_DEADLINE);
    if let Some(thread) = thread {
        release_thread(connection, thread);
    }
}

/// Ask the server to release `thread`, saying so when it does not answer or
/// refuses: the thread then stays loaded until the server stops.
fn release_thread(connection: &mut Connection, thread: &str) {
    let warn = |why: String| {
        eprintln!(
            "oneharness: warning: the shared codex app-server did not release thread {thread}: {why}"
        );
    };
    match connection.request(
        RELEASE_THREAD_METHOD,
        json!({"threadId": thread}),
        RELEASE_REPLY_DEADLINE,
    ) {
        Ok(Some(response)) => match rules::reply_result(RELEASE_THREAD_METHOD, &response) {
            Err(why) => warn(why),
            // Released, or already gone from the server.
            Ok(result)
                if matches!(
                    result["status"].as_str(),
                    Some("unsubscribed" | "notLoaded")
                ) => {}
            Ok(result) => warn(format!(
                "it answered {RELEASE_THREAD_METHOD} with status {}, not unsubscribed",
                result["status"]
            )),
        },
        Ok(None) => warn(format!(
            "no answer to {RELEASE_THREAD_METHOD} within {}s",
            RELEASE_REPLY_DEADLINE.as_secs()
        )),
        Err(err) => warn(err.to_string()),
    }
}

fn stop_dispatch_commands(marker: &ProcessMarker, deadline: Duration) {
    match stop_marked(marker, deadline) {
        Ok(report) if !report.survivors.is_empty() => eprintln!(
            "oneharness: warning: {} process(es) carrying {} survived the stop: {:?}",
            report.survivors.len(),
            marker.name,
            report.survivors
        ),
        Ok(_) => {}
        Err(err) => eprintln!("oneharness: warning: could not stop the dispatch's commands: {err}"),
    }
}

/// Retire the lease of a holder found dead: interrupt its live turn, stop every
/// process carrying its marker within [`DEAD_LEASE_STOP_DEADLINE`], then
/// release its thread. Called by the pool's reconciliation under the entry's
/// lock, so it runs once per dead lease whichever process finds it.
pub(crate) fn retire_dead_lease(record: &ServerRecord, note: &LeaseNote) {
    let started = Instant::now();
    let marker = ProcessMarker {
        name: rules::DISPATCH_MARKER_VAR.to_string(),
        value: note.marker.as_str().to_string(),
    };
    let mut connection = connect_to(record, Duration::from_secs(2));
    let thread = note.thread.as_ref().map(|thread| thread.id.as_str());
    let turn = note
        .thread
        .as_ref()
        .and_then(|thread| thread.turn.as_ref())
        .map(TurnId::as_str);
    let interrupt = |connection: &mut Connection, thread: &str, turn: &str, within: Duration| {
        connection
            .request(
                "turn/interrupt",
                json!({"threadId": thread, "turnId": turn}),
                within,
            )
            .ok()
            .flatten()
            .is_some_and(|reply| {
                matches!(
                    rules::InterruptReply::of(&reply),
                    rules::InterruptReply::Accepted | rules::InterruptReply::ThreadGone(_)
                )
            })
    };
    // First within the stop deadline's budget, so the turn starts no more
    // commands while they are stopped …
    let mut interrupted = match (connection.as_mut(), thread, turn) {
        (Some(connection), Some(thread), Some(turn)) => {
            interrupt(connection, thread, turn, Duration::from_millis(1500))
        }
        _ => turn.is_none(),
    };
    // What is left of the deadline, short of the settle window the forced kill
    // needs, so the commands are gone within the deadline from the finding.
    let left = DEAD_LEASE_STOP_DEADLINE
        .saturating_sub(started.elapsed())
        .saturating_sub(Duration::from_millis(1200))
        .max(Duration::from_millis(300));
    stop_dispatch_commands(&marker, left);
    // … then, if that did not land (a server slow to answer), once more with
    // the release's own bound, so the turn is not left running.
    if !interrupted {
        if connection.is_none() {
            connection = connect_to(record, RELEASE_REPLY_DEADLINE);
        }
        if let (Some(connection), Some(thread), Some(turn)) = (connection.as_mut(), thread, turn) {
            interrupted = interrupt(connection, thread, turn, RELEASE_REPLY_DEADLINE);
        }
        if !interrupted {
            eprintln!(
                "oneharness: warning: could not interrupt the dead dispatch's turn on the shared codex app-server (pid {})",
                record.pid.get()
            );
        }
    }
    if let (Some(connection), Some(thread)) = (connection.as_mut(), thread) {
        // Subscribe to the thread first: an unsubscribe from a connection that
        // never subscribed leaves it loaded. Codex refuses to resume a thread
        // that never had a turn, and releases a closed connection's threads by
        // itself, so a refused resume is no failure of the release.
        let _ = connection.request(
            "thread/resume",
            json!({"threadId": thread}),
            RELEASE_REPLY_DEADLINE,
        );
        release_thread(connection, thread);
    }
    if let Some(connection) = connection {
        connection.close();
    }
}

/// A fresh, initialized connection to the server `record` names, or `None` if
/// it cannot be reached.
fn connect_to(record: &ServerRecord, initialize_within: Duration) -> Option<Connection> {
    let ServerAddress::UnixSocket { path } = &record.address else {
        return None;
    };
    if !record.is_running() {
        return None;
    }
    let stream = LocalStream::connect(Path::new(&path.to_string())).ok()?;
    let mut connection = Connection::open(stream).ok()?;
    let initialized = connection
        .request("initialize", initialize_params(), initialize_within)
        .ok()??;
    rules::reply_result("initialize", &initialized).ok()?;
    connection
        .send(&json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}))
        .ok()?;
    Some(connection)
}

/// Stop a shared server: reap every lease's marked commands first, then stop
/// every process carrying the server's marker within [`SERVER_STOP_DEADLINE`] —
/// a drain request, and a kill for whatever is left, the launcher shim and its
/// native child alike.
pub(crate) fn stop_server(entry: &Path, record: &ServerRecord) {
    if let Ok(leases) = std::fs::read_dir(entry.join("leases")) {
        for lease in leases.flatten() {
            let note = std::fs::read_to_string(lease.path())
                .ok()
                .and_then(|text| serde_json::from_str::<server_pool::LeaseFile>(&text).ok())
                .and_then(|file| file.note);
            if let Some(value) = note.map(|note| String::from(note.marker)) {
                stop_dispatch_commands(
                    &ProcessMarker {
                        name: rules::DISPATCH_MARKER_VAR.to_string(),
                        value,
                    },
                    DEAD_LEASE_STOP_DEADLINE,
                );
            }
        }
    }
    if let Some(marker) = &record.marker {
        stop_by_marker(marker);
    }
    // A stopped server's listener can outlive the environment the marker scan
    // reads — the kernel drops an exiting process's memory before its sockets —
    // so the address is unlinked too: a successor's first dial then waits for
    // its own bind instead of reaching the closing listener and being reset.
    let _ = std::fs::remove_file(entry.join(SOCKET_DIR).join(SOCKET_FILE));
}

/// Stop every process carrying the server marker `value`, within
/// [`SERVER_STOP_DEADLINE`].
fn stop_by_marker(server: &ServerMarker) {
    let marker = ProcessMarker {
        name: SERVER_MARKER_VAR.to_string(),
        value: server.as_str().to_string(),
    };
    match stop_marked(&marker, SERVER_STOP_DEADLINE) {
        Ok(report) if !report.survivors.is_empty() => eprintln!(
            "oneharness: warning: the shared codex app-server left {:?} running",
            report.survivors
        ),
        Ok(_) => {}
        Err(err) => {
            eprintln!("oneharness: warning: could not stop the shared codex app-server: {err}")
        }
    }
}

/// Start the entry's sweeper unless a live one is already watching it.
fn ensure_sweeper(entry: &Path) {
    if !SWEEPER_HOSTED.load(Ordering::SeqCst) {
        return;
    }
    let Ok(_guard) = EntryLock::acquire(entry) else {
        return;
    };
    if live_sweeper(entry) {
        return;
    }
    // On Linux the running image itself, which `/proc/self/exe` opens even
    // after its path was replaced (a rebuild of the same target under a running
    // dispatch): the path `current_exe` reads then ends in ` (deleted)` and
    // names no file, so no sweeper would start and a dead holder's commands
    // would run on with no other dispatch alive to find them.
    #[cfg(target_os = "linux")]
    let exe = std::path::PathBuf::from("/proc/self/exe");
    #[cfg(not(target_os = "linux"))]
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // Its warnings — a stop that left survivors, a release that failed — go
    // to a log in the entry: nobody is reading a detached process's stderr.
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(entry.join(SWEEPER_LOG))
        .map_or_else(|_| std::process::Stdio::null(), std::process::Stdio::from);
    let ambient: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    let mut command = std::process::Command::new(exe);
    command
        .env_clear()
        .envs(rules::sweeper_env(&ambient, EnvPlatform::current()))
        .env(SWEEPER_ENV, entry)
        .current_dir(entry)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log);
    server_pool::detach(&mut command);
    match command.spawn() {
        Ok(child) => {
            // Recorded by the starter as well as by the sweeper itself, so a
            // second dispatch arriving before the sweeper is up does not start
            // another one.
            if let Some(holder) = server_pool::Pid::new(child.id()).ok().and_then(|pid| {
                Some(LeaseHolder {
                    pid,
                    identity: server_pool::process_identity(pid)?,
                })
            }) {
                let _ = std::fs::write(
                    entry.join(SWEEPER_FILE),
                    serde_json::to_string(&holder).unwrap_or_default(),
                );
            }
            // Reaped on its own thread: a sweeper outlives the dispatch that
            // started it whenever that dispatch finishes first, and is
            // reparented then.
            std::thread::spawn(move || {
                let mut child = child;
                let _ = child.wait();
            });
        }
        Err(err) => eprintln!("oneharness: warning: could not start the app-server sweeper: {err}"),
    }
}

fn live_sweeper(entry: &Path) -> bool {
    std::fs::read_to_string(entry.join(SWEEPER_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<LeaseHolder>(&text).ok())
        .is_some_and(|holder| holder.is_live())
}

/// The sweeper's whole life: every [`DEAD_LEASE_SWEEP_INTERVAL`], reconcile the
/// entry — retiring the leases of dead holders, and stopping the server once no
/// lease is live and its linger has passed — until the entry names no server.
fn run_sweeper(entry: &Path) {
    let me = LeaseHolder::current();
    let mut watching = {
        let Ok(_guard) = EntryLock::acquire(entry) else {
            return;
        };
        let current = std::fs::read_to_string(entry.join(SWEEPER_FILE))
            .ok()
            .and_then(|text| serde_json::from_str::<LeaseHolder>(&text).ok());
        if current
            .as_ref()
            .is_some_and(|holder| holder.is_live() && Some(holder) != me.as_ref())
        {
            return;
        }
        if let Some(me) = &me {
            let _ = std::fs::write(
                entry.join(SWEEPER_FILE),
                serde_json::to_string(me).unwrap_or_default(),
            );
        }
        match server_pool::read_record(entry).and_then(|record| record.marker) {
            Some(marker) => marker,
            None => {
                let _ = std::fs::remove_file(entry.join(SWEEPER_FILE));
                return;
            }
        }
    };
    loop {
        std::thread::sleep(DEAD_LEASE_SWEEP_INTERVAL);
        // An entry removed from under a live server leaves nothing to
        // reconcile, and nobody else who knows the server: stop it by the
        // marker this sweeper last saw.
        if !entry.join("leases").is_dir() {
            stop_by_marker(&watching);
            return;
        }
        let Ok(_guard) = EntryLock::acquire(entry) else {
            return;
        };
        // One sweeper per entry, for whatever server it holds: a dispatch that
        // replaced the server this one was started beside finds this sweeper
        // live and starts none of its own, so this one adopts the new server.
        let _ = server_pool::reconcile(entry, DEFAULT_LINGER);
        match server_pool::read_record(entry).and_then(|record| record.marker) {
            Some(marker) => watching = marker,
            None => {
                // Gone under the lock, so no dispatch can find this sweeper
                // live after the entry stopped needing it.
                let _ = std::fs::remove_file(entry.join(SWEEPER_FILE));
                return;
            }
        }
    }
}

/// One message per written line, for the control handle's line sink: a frame
/// is written whole and then flushed, and the flush is the send.
struct FrameSink {
    writer: Arc<Mutex<WsWriter>>,
    pending: Vec<u8>,
}

impl Write for FrameSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&self.pending)
            .trim_end()
            .to_string();
        self.pending.clear();
        self.writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .send_text(&text)
    }
}

/// How a pooled turn ended.
enum End {
    /// The turn ended.
    Done,
    /// The consumer stopped reading while the turn may still be live.
    Stopped,
    TimedOut,
    Cancelled,
    /// The server went away under the live turn.
    Lost(String),
    /// The connection ended, or failed, before the turn opened.
    Failed(String),
}

/// Run one controlled turn on the shared server `pooled` keys to, driven by
/// the dialogue `handle` has bound, and capture it as the dispatch's own child
/// would have been: every frame of its own thread a line of `stdout`.
pub(crate) fn run_pooled_turn(
    job: &Job,
    handle: &ControlHandle,
    prompt: &str,
    pooled: &PooledDispatch,
    cancel: &CancelToken,
    on_line: &mut dyn FnMut(&str) -> StreamStep,
) -> Capture {
    let start = Instant::now();
    let start_epoch_ms = crate::io::runner::epoch_millis();
    let started_at = crate::domain::history::format_rfc3339_millis(start_epoch_ms);
    let failed = |status: Status, error: String| Capture {
        status,
        exit_code: None,
        duration_ms: Some(start.elapsed().as_millis()),
        stdout: String::new(),
        stderr: String::new(),
        error: Some(error),
        started_at: started_at.clone(),
        finished_at: Some(crate::io::runner::utc_now()),
        stdout_observations: Vec::new(),
    };
    let Some(root) = server_pool::resolve_root(None) else {
        return failed(
            Status::SpawnError,
            "no state directory to keep the shared codex app-server pool in".to_string(),
        );
    };
    let ready_within = if job.timeout.is_zero() {
        READY_WINDOW
    } else {
        READY_WINDOW.min(job.timeout)
    };
    let (lease, stream) = match lease_server(&root, pooled, ready_within) {
        Ok(leased) => leased,
        Err(error) => return failed(Status::SpawnError, error),
    };
    let marker = pooled.process_marker();
    let mut note = LeaseNote {
        marker: pooled.marker.clone(),
        thread: None,
    };
    lease.annotate(&note);
    let mut connection = match Connection::open(stream) {
        Ok(connection) => connection,
        Err(err) => {
            return failed(
                Status::SpawnError,
                format!("could not open a WebSocket to the shared codex app-server: {err}"),
            )
        }
    };
    let board = Arc::new(ReplyBoard::default());
    handle.begin_pooled_turn(
        Box::new(FrameSink {
            writer: Arc::clone(&connection.writer),
            pending: Vec::new(),
        }),
        Arc::clone(&board),
        lease.report(),
        lease.record().argv.as_slice().to_vec(),
    );

    let mut lines = Captured::default();
    let mut held: Vec<(Instant, String, Value)> = Vec::new();
    let mut end = None;
    for frame in handle.open_frames(prompt) {
        if let Err(err) = handle.write_line(&frame) {
            end = Some(End::Failed(format!(
                "could not open the controlled turn on the shared codex app-server: {err}"
            )));
            break;
        }
    }
    let deadline = (!job.timeout.is_zero()).then(|| start + job.timeout);
    let end = match end {
        Some(end) => end,
        None => loop {
            if cancellation_requested(cancel) {
                break End::Cancelled;
            }
            let now = Instant::now();
            if deadline.is_some_and(|deadline| now >= deadline) {
                break End::TimedOut;
            }
            let slice = deadline.map_or(crate::io::runner::CANCEL_POLL_SLICE, |deadline| {
                (deadline - now).min(crate::io::runner::CANCEL_POLL_SLICE)
            });
            if let Err(err) = connection
                .reader
                .set_read_timeout(Some(slice.max(Duration::from_millis(1))))
            {
                break lost_or_failed(handle, format!("{err}"));
            }
            let text = match connection.reader.receive() {
                Ok(Incoming::Text(text)) => text,
                // A server that died is not always a connection that closed:
                // a process it started can hold the server's end open. On
                // macOS one does whenever the server started it while another
                // of its threads was accepting a connection — Darwin has no
                // atomic close-on-exec `accept` — and nothing then ever reads
                // end-of-file. So a quiet connection asks after the server.
                Ok(Incoming::Idle) if !lease.record().is_running() => {
                    break lost_or_failed(
                        handle,
                        format!(
                            "the shared codex app-server (pid {}) exited",
                            lease.record().pid.get()
                        ),
                    )
                }
                Ok(Incoming::Idle) => continue,
                Ok(Incoming::Closed) => {
                    break lost_or_failed(
                        handle,
                        "the shared codex app-server closed the connection".to_string(),
                    )
                }
                Err(err) => break lost_or_failed(handle, format!("{err}")),
            };
            let Ok(frame) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            // An answer to a request the control thread sent: hand it over.
            if frame.get("method").is_none() {
                if let Some(id) = frame.get("id").and_then(Value::as_u64) {
                    if board.deliver(id, frame.clone()) {
                        lines.push(&text, Instant::now());
                        continue;
                    }
                }
            }
            let own_thread = handle.session_id();
            if !rules::is_own_frame(&frame, own_thread.as_deref()) {
                if own_thread.is_none() && held.len() < PRE_THREAD_BUFFER {
                    held.push((Instant::now(), text, frame));
                }
                continue;
            }
            if let Some(end) = deliver(handle, &text, Instant::now(), &mut lines, on_line) {
                break end;
            }
            // The dialogue may just have learned its thread: replay what was
            // held for it, and forget the rest.
            if own_thread.is_none() {
                if let Some(thread) = handle.session_id() {
                    let mut replayed = None;
                    for (arrived, text, frame) in std::mem::take(&mut held) {
                        if rules::frame_thread(&frame) == Some(thread.as_str())
                            && replayed.is_none()
                        {
                            replayed = deliver(handle, &text, arrived, &mut lines, on_line);
                        }
                    }
                    if let Some(end) = replayed {
                        break end;
                    }
                }
            }
            let thread = handle
                .session_id()
                .and_then(ThreadId::new)
                .map(|id| LeasedThread {
                    id,
                    turn: handle.active_turn().and_then(|(_, turn)| TurnId::new(turn)),
                });
            if note.thread != thread {
                note.thread = thread;
                lease.annotate(&note);
            }
        },
    };

    // A turn still live past the run's own end is interrupted rather than left
    // billing on a server nobody is reading.
    if matches!(end, End::TimedOut | End::Cancelled | End::Stopped) {
        if let Some((thread, turn)) = handle.active_turn() {
            let _ = connection.request(
                "turn/interrupt",
                json!({"threadId": thread, "turnId": turn}),
                Duration::from_millis(1500),
            );
        }
    }
    let thread = handle.session_id();
    handle.end_turn();
    if !matches!(end, End::Lost(_)) {
        release_dispatch(&mut connection, &marker, thread.as_deref());
    } else {
        stop_dispatch_commands(&marker, DEAD_LEASE_STOP_DEADLINE);
    }
    connection.close();
    drop(lease);

    let (status, error) = match end {
        // A consumer-driven stop is not a harness failure, as for a child.
        End::Done | End::Stopped => (Status::Ok, None),
        End::TimedOut => (Status::Timeout, Some(crate::io::runner::timeout_error(job))),
        End::Cancelled => (
            Status::Cancelled,
            Some(crate::io::runner::cancelled_error(&job.argv[0])),
        ),
        End::Lost(error) => {
            handle.mark_app_server_lost();
            (Status::Nonzero, Some(error))
        }
        End::Failed(error) => (Status::Nonzero, Some(error)),
    };
    let mut stdout = lines.lines.join("\n");
    if !stdout.is_empty() {
        stdout.push('\n');
    }
    let stdout_observations =
        crate::io::runner::observations_since(start, start_epoch_ms, lines.observed);
    Capture {
        status,
        exit_code: None,
        duration_ms: Some(start.elapsed().as_millis()),
        stdout,
        stderr: String::new(),
        error,
        started_at,
        finished_at: Some(crate::io::runner::utc_now()),
        stdout_observations,
    }
}

/// The dispatch's own frames as its child's stdout would have carried them:
/// each a line, and when it arrived — the observations a tool event's
/// observed timing is read from.
#[derive(Default)]
struct Captured {
    lines: Vec<String>,
    observed: Vec<(Instant, Vec<u8>)>,
}

impl Captured {
    fn push(&mut self, text: &str, arrived: Instant) {
        self.lines.push(text.to_string());
        self.observed
            .push((arrived, format!("{text}\n").into_bytes()));
    }
}

/// Feed one of the dispatch's own frames through: record it (as arriving at
/// `arrived`), publish it, and advance the dialogue. `Some` when that ends the
/// turn.
fn deliver(
    handle: &ControlHandle,
    text: &str,
    arrived: Instant,
    lines: &mut Captured,
    on_line: &mut dyn FnMut(&str) -> StreamStep,
) -> Option<End> {
    lines.push(text, arrived);
    if on_line(text) == StreamStep::Stop {
        return Some(End::Stopped);
    }
    if handle.advance(text) {
        // A dialogue that still holds its turn ended because a write failed:
        // the connection under it is gone.
        if handle.in_turn() {
            return Some(End::Lost(
                "the shared codex app-server refused a write while the turn was running"
                    .to_string(),
            ));
        }
        return Some(End::Done);
    }
    None
}

fn lost_or_failed(handle: &ControlHandle, cause: String) -> End {
    if handle.in_turn() {
        End::Lost(format!(
            "the shared codex app-server went away while the turn was running ({cause})"
        ))
    } else {
        End::Failed(format!(
            "the shared codex app-server connection ended before the turn opened ({cause})"
        ))
    }
}

/// A dispatch's session on a shared server with no turn of its own driver: the
/// same lease, connection and release a `--control` dispatch takes, for a
/// caller that drives the protocol itself (the sustained-reuse workload).
pub struct PooledSession {
    lease: AppServerLease,
    connection: Connection,
    dispatch: PooledDispatch,
    thread: Option<ThreadId>,
}

impl PooledSession {
    /// Lease the shared server `dispatch` keys to — starting it if none is
    /// running — and open an initialized connection to it.
    pub fn open(dispatch: PooledDispatch) -> Result<Self, String> {
        let root = server_pool::resolve_root(None)
            .ok_or_else(|| "no state directory to keep the app-server pool in".to_string())?;
        let (lease, stream) = lease_server(&root, &dispatch, READY_WINDOW)?;
        lease.annotate(&LeaseNote {
            marker: dispatch.marker.clone(),
            thread: None,
        });
        let mut connection = Connection::open(stream).map_err(|err| format!("{err}"))?;
        let initialized = connection
            .request("initialize", initialize_params(), Duration::from_secs(10))
            .map_err(|err| format!("initialize: {err}"))?
            .ok_or("the app-server did not answer initialize")?;
        rules::reply_result("initialize", &initialized)
            .map_err(|why| format!("the app-server's initialize failed: {why}"))?;
        connection
            .send(&json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}))
            .map_err(|err| format!("initialized: {err}"))?;
        Ok(PooledSession {
            lease,
            connection,
            dispatch,
            thread: None,
        })
    }

    /// The server this session's lease holds.
    #[must_use]
    pub fn record(&self) -> &ServerRecord {
        self.lease.record()
    }

    /// The pool entry directory.
    #[must_use]
    pub fn entry(&self) -> &Path {
        self.lease.entry()
    }

    /// Open (`thread/start`) or reopen (`thread/resume`, naming `resume`) the
    /// session's thread in `cwd`, carrying the dispatch's
    /// `shell_environment_policy`, and answer the server's response.
    pub fn open_thread(&mut self, cwd: &Path, resume: Option<&ThreadId>) -> Result<Value, String> {
        let mut params = json!({
            "cwd": cwd.to_string_lossy(),
            "approvalPolicy": "never",
            "config": self.dispatch.thread_config(),
        });
        let method = match resume {
            Some(thread) => {
                params["threadId"] = json!(thread.as_str());
                "thread/resume"
            }
            None => "thread/start",
        };
        let response = self
            .connection
            .request(method, params, Duration::from_secs(30))
            .map_err(|err| format!("{method}: {err}"))?
            .ok_or_else(|| format!("the app-server did not answer {method}"))?;
        if let Some(error) = response.get("error") {
            return Err(format!("{method} was refused: {error}"));
        }
        let thread = response
            .pointer("/result/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{method} named no thread: {response}"))?
            .to_string();
        let id = ThreadId::new(thread.as_str())
            .ok_or_else(|| format!("{method} named an unusable thread id {thread:?}"))?;
        self.thread = Some(id.clone());
        self.lease.annotate(&LeaseNote {
            marker: self.dispatch.marker.clone(),
            thread: Some(LeasedThread { id, turn: None }),
        });
        Ok(response)
    }

    // llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Real Codex cannot be read in the deterministic gate; the live phases scripts/e2e-control.sh runs on every live-control leg drive these requests against it — pooled-reuse sends thread/start, thread/shellCommand, thread/resume, thread/unsubscribe and thread/loaded/list and fails naming a released thread still loaded, and pooled-env and pooled-kill drive the thread lifecycle and its environment — which is this contract's drift alarm.
    /// Run `command` as a shell turn on the session's thread (`thread/shellCommand`,
    /// which makes no model call) and wait for that turn to complete.
    ///
    /// The sustained-reuse workload needs it: codex-cli 0.159.2 writes a
    /// thread's rollout only after the thread has had a turn, and refuses to
    /// resume a thread with none (`no rollout found for thread id …`), so a
    /// no-model shell turn is what makes its thread resumable. Never used on a
    /// dispatch's own thread, where a shell turn could be left as the thread's
    /// active one.
    pub fn shell_turn(&mut self, command: &str) -> Result<(), String> {
        let thread = self
            .thread
            .clone()
            .ok_or("the session has no thread to run a shell turn on")?;
        let response = self
            .connection
            .request(
                "thread/shellCommand",
                json!({"threadId": thread.as_str(), "command": command, "timeoutMs": 30_000}),
                Duration::from_secs(30),
            )
            .map_err(|err| format!("thread/shellCommand: {err}"))?
            .ok_or("the app-server did not answer thread/shellCommand")?;
        rules::reply_result("thread/shellCommand", &response)
            .map_err(|why| format!("the app-server's thread/shellCommand failed: {why}"))?;
        match self.connection.await_notification(
            "turn/completed",
            thread.as_str(),
            Duration::from_secs(30),
        ) {
            Ok(Some(frame)) => match frame.pointer("/params/turn/status").and_then(Value::as_str) {
                Some("completed") => Ok(()),
                status => Err(format!(
                    "the shell turn ended {}, not completed: {frame}",
                    status.unwrap_or("with no status")
                )),
            },
            Ok(None) => Err("the shell turn did not complete within 30s".to_string()),
            Err(err) => Err(format!("the shell turn: {err}")),
        }
    }

    /// The threads the server holds loaded now (`thread/loaded/list`), every
    /// page of them, read before `until`: a listing still paging then is an
    /// error, so the caller's bound holds across a whole listing.
    pub fn loaded_threads(&mut self, until: Instant) -> Result<Vec<ThreadId>, String> {
        let mut threads = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!(
                    "thread/loaded/list was still paging at the deadline, {} thread(s) in",
                    threads.len()
                ));
            }
            let params = match &cursor {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            // Each page within what is left of the whole listing's deadline.
            let response = self
                .connection
                .request(
                    "thread/loaded/list",
                    params,
                    left.min(Duration::from_secs(10)),
                )
                .map_err(|err| format!("thread/loaded/list: {err}"))?
                .ok_or_else(|| {
                    if Instant::now() >= until {
                        format!(
                            "thread/loaded/list was still paging at the deadline, {} thread(s) in",
                            threads.len()
                        )
                    } else {
                        "the app-server did not answer thread/loaded/list".to_string()
                    }
                })?;
            if let Some(error) = response.get("error") {
                return Err(format!("thread/loaded/list was refused: {error}"));
            }
            let page = response
                .pointer("/result/data")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("thread/loaded/list answered no data: {response}"))?;
            for id in page {
                let thread = id.as_str().and_then(ThreadId::new).ok_or_else(|| {
                    format!("thread/loaded/list named a thread {id} that is no id")
                })?;
                threads.push(thread);
            }
            cursor = match response.pointer("/result/nextCursor") {
                None | Some(Value::Null) => None,
                Some(Value::String(next)) => Some(next.clone()),
                Some(other) => {
                    return Err(format!(
                        "thread/loaded/list answered a cursor {other} that is no string"
                    ))
                }
            };
            if Instant::now() > until {
                return Err(format!(
                    "thread/loaded/list answered past the deadline, {} thread(s) in",
                    threads.len()
                ));
            }
            match &cursor {
                None => return Ok(threads),
                // A cursor answered twice would page forever: refused, so the
                // caller's own bound still holds.
                Some(next) if !seen.insert(next.clone()) => {
                    return Err(format!(
                        "thread/loaded/list answered the cursor {next:?} a second time"
                    ))
                }
                Some(_) => {}
            }
        }
    }

    // llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

    /// End the session's lease: stop its marked commands, release its thread,
    /// close the connection and give the lease back.
    pub fn release(mut self) {
        let marker = self.dispatch.process_marker();
        release_dispatch(
            &mut self.connection,
            &marker,
            self.thread.as_ref().map(ThreadId::as_str),
        );
        self.connection.close();
        drop(self.lease);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_environment_no_process_can_hold_is_not_pooled() {
        let argv = vec!["codex".to_string(), "app-server".to_string()];
        let cwd = std::env::temp_dir();
        let pair = |name: &str, value: &str| vec![(name.to_string(), value.to_string())];
        assert!(PooledDispatch::plan(&argv, &pair("GOOD", "v"), &[], &cwd, false).is_some());
        for env in [
            pair("", "v"),
            pair("A=B", "v"),
            pair("A\0B", "v"),
            pair("A", "v\0w"),
        ] {
            assert!(
                PooledDispatch::plan(&argv, &env, &[], &cwd, false).is_none(),
                "{env:?}"
            );
        }
        for removed in ["", "A=B", "A\0B"] {
            assert!(
                PooledDispatch::plan(&argv, &[], &[removed.to_string()], &cwd, false).is_none(),
                "{removed:?}"
            );
        }
    }

    use super::*;

    /// The live drift gate for Codex's rendezvous (`oh_codex_rendezvous_check`
    /// in scripts/e2e-codex.sh) expects exactly the directory and mode the
    /// launch prepares, so a change to either cannot leave it checking real
    /// Codex against the old one.
    #[test]
    fn the_live_rendezvous_check_expects_the_rendezvous_the_launch_prepares() {
        let lane = include_str!("../../../../scripts/e2e-codex.sh").replace("\r\n", "\n");
        let named = rules::codex_daemon_dir(Path::new(""), 4242);
        let prefix = named
            .to_str()
            .and_then(|name| name.strip_suffix("4242"))
            .expect("the rendezvous directory ends in the euid");
        let values = format!(
            "  local rendezvous_prefix={prefix} rendezvous_mode={:o}\n",
            rules::CODEX_DAEMON_DIR_MODE
        );
        assert!(
            lane.contains(&values),
            "scripts/e2e-codex.sh lacks `{values}`"
        );
        for derived in [
            r#"want="$tmp/$rendezvous_prefix$euid""#,
            r#"[ "$mode" = "$rendezvous_mode" ]"#,
        ] {
            assert!(
                lane.contains(derived),
                "scripts/e2e-codex.sh lacks `{derived}`"
            );
        }
    }

    #[test]
    fn a_dispatch_plans_a_pooled_turn_only_for_an_app_server_launch() {
        let argv = |words: &[&str]| words.iter().map(|w| (*w).to_string()).collect::<Vec<_>>();
        let cwd = std::env::temp_dir();
        assert!(
            PooledDispatch::plan(&argv(&["codex", "exec", "hi"]), &[], &[], &cwd, false).is_none()
        );
        assert!(PooledDispatch::plan(&argv(&["codex"]), &[], &[], &cwd, false).is_none());
        let plan = PooledDispatch::plan(
            &argv(&["codex", "app-server", "-c", "features.plugins=false"]),
            &[
                (
                    "ONEHARNESS_POOL_PLAN_ONLY".to_string(),
                    "dispatch".to_string(),
                ),
                ("CODEX_HOME".to_string(), "/codex-home".to_string()),
            ],
            &["ONEHARNESS_POOL_PLAN_REMOVED".to_string()],
            &cwd,
            false,
        )
        .expect("an app-server launch is poolable");
        assert_eq!(plan.process_args, ["-c", "features.plugins=false"]);
        assert!(plan
            .server_env
            .contains(&("CODEX_HOME".to_string(), "/codex-home".to_string())));
        assert!(!plan
            .server_env
            .iter()
            .any(|(k, _)| k == "ONEHARNESS_POOL_PLAN_ONLY"));
        let config = plan.thread_config();
        let set = &config["shell_environment_policy"]["set"];
        assert_eq!(set["ONEHARNESS_POOL_PLAN_ONLY"], "dispatch");
        assert_eq!(set[rules::DISPATCH_MARKER_VAR], plan.marker());
        assert!(config["shell_environment_policy"]["exclude"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == SERVER_MARKER_VAR));
    }
}
