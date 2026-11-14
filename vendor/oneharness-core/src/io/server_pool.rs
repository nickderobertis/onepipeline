//! The generic sidecar-server pool.
//!
//! Some control mechanisms reach their turn only through a server that must
//! already be running and outlives any one turn (`opencode serve`,
//! `crush server`). Those are declared per harness in
//! [`crate::domain::control::ServerSpec`] rather than special-cased, so a server
//! costing ~137MB is started once and shared by every dispatch with the same key
//! instead of once per turn.
//!
//! The ACP stdio protocols (`copilot --acp`, `goose acp`) are deliberately NOT
//! pooled: oneharness spawns them as the dispatch's own child, so the interrupt
//! rides the same stdin the turn does and the process dies with the run.
//! `codex app-server` is pooled too, by default: one `--listen unix://` server
//! per configuration, shared by every concurrent dispatch with its key
//! ([`crate::io::app_server`]). Its entries are the ones whose record names a
//! server marker ([`ServerRecord::marker`]), and they are stopped by that marker
//! rather than by pid, so a launcher shim's native child goes with it.
//!
//! **Membership is a lease held by a live process, never a counter.** A counter
//! leaks a permanently-live server the first time a dispatch is `SIGKILL`ed
//! before it can decrement — and dispatch trees are killed routinely. Each
//! dispatch instead writes a lease file naming itself as a [`LeaseHolder`]:
//! its pid *and* that incarnation's kernel birth identity, because a pid alone
//! is recycled and would let an unrelated process inherit the lease. Both sides
//! of an entry — the server it names and the dispatches holding it — are
//! identified that same way. Everything on disk is therefore self-healing: no
//! shutdown hook is load-bearing.
//!
//! Layout under `<state>/oneharness/servers/`:
//!
//! ```text
//! <pool-key>/server.json     the running server: pid + identity, argv, address
//! <pool-key>/leases/<id>     one file per live dispatch: its pid + identity
//! <pool-key>/.lock           whole-entry mutex around start/stop decisions
//! <pool-key>/server.log      a shared Codex app-server's stderr, from its start
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::domain::control::{PoolKey, ServerAddress, ServerSpec};

/// How long an idle server (no live lease) is kept before reclamation, so a
/// burst of short dispatches does not thrash a heavyweight process up and down.
pub const DEFAULT_LINGER: Duration = Duration::from_secs(60);

/// How often a shared Codex app-server's pool-owned sweeper looks for a lease
/// whose holder died, so a dead holder is found within this long of its death
/// even with no other dispatch alive.
pub const DEAD_LEASE_SWEEP_INTERVAL: Duration = Duration::from_secs(2);

/// How long the pool takes, from finding a dead holder, to stop the commands it
/// left running: its turn interrupted, then every process carrying its marker
/// stopped, forcibly at the deadline.
pub const DEAD_LEASE_STOP_DEADLINE: Duration = Duration::from_secs(5);

/// How long stopping a shared Codex app-server lets the processes carrying its
/// marker drain before they are killed. A `--listen` server's own SIGTERM waits
/// for every in-flight command with no deadline of its own.
pub const SERVER_STOP_DEADLINE: Duration = Duration::from_secs(10);

/// How long a `turn/interrupt` sent over a shared connection waits for the
/// server's answer. The server answers an interrupt naming a turn that already
/// ended with nothing at all, so no interrupt is ever sent unbounded.
pub const INTERRUPT_REPLY_DEADLINE: Duration = Duration::from_secs(5);

const LEASE_DIR: &str = "leases";
const SERVER_FILE: &str = "server.json";
const LOCK_FILE: &str = ".lock";
const SERVER_LOG: &str = "server.log";

/// The identity recorded for a server that exited before its own could be
/// read. No kernel reading is ever spelled this way, so it never matches a
/// running process.
const EXITED_UNIDENTIFIED: &str = "exited-before-identified";

/// How long a server just spawned that the kernel will not yet identify, and
/// that has not yet been seen to exit, is watched for either before the launch
/// is refused, and how often it is looked at meanwhile.
const IDENTIFY_GRACE: Duration = Duration::from_secs(2);
const IDENTIFY_POLL: Duration = Duration::from_millis(5);

/// How much of a server's stderr an early-exit diagnosis quotes, from its end.
const STDERR_TAIL_BYTES: u64 = 2048;

/// A process id the OS could actually have handed out.
///
/// `0` is not a smaller pid — on Unix it addresses the caller's whole process
/// group, so a record that lost its pid and kept a `0` would have reclamation
/// signal *this* dispatch's tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Pid(u32);

impl Pid {
    /// Accept `raw` as a pid, or say why it cannot be one.
    pub fn new(raw: u32) -> Result<Self, String> {
        if raw == 0 {
            Err("0 is not a process id".to_string())
        } else {
            Ok(Pid(raw))
        }
    }

    #[must_use]
    pub fn get(self) -> u32 {
        self.0
    }
}

impl<'de> Deserialize<'de> for Pid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        Pid::new(u32::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// A spawn argv that names a program: never empty, so `argv[0]` is always there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct LaunchArgv(Vec<String>);

impl LaunchArgv {
    /// Accept `argv` as a spawn plan, or say why it cannot be one.
    ///
    /// A blank `argv[0]` is refused alongside an empty vector: both fail to name
    /// a program, and the blank one fails later and less legibly — as a spawn
    /// error for `""` rather than as the plan being wrong.
    pub fn new(argv: Vec<String>) -> Result<Self, String> {
        match argv.first() {
            Some(program) if !program.trim().is_empty() => Ok(LaunchArgv(argv)),
            _ => Err("a launch argv must name a program".to_string()),
        }
    }

    /// The program to spawn and the arguments to pass it.
    #[must_use]
    pub fn split(&self) -> (&String, &[String]) {
        self.0
            .split_first()
            .expect("LaunchArgv is never empty by construction")
    }

    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for LaunchArgv {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        LaunchArgv::new(Vec::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

pub use crate::domain::app_server::{DispatchMarker, ServerMarker, ThreadId, TurnId};

impl DispatchMarker {
    /// A marker no other dispatch carries.
    #[must_use]
    pub fn fresh() -> Self {
        DispatchMarker::new(uuid::Uuid::now_v7().hyphenated().to_string())
            .expect("a hyphenated uuid is a usable marker")
    }
}

impl ServerMarker {
    /// A marker no other server carries.
    #[must_use]
    pub fn fresh() -> Self {
        ServerMarker::new(uuid::Uuid::now_v7().hyphenated().to_string())
            .expect("a hyphenated uuid is a usable marker")
    }
}

/// The record describing the server currently backing a pool entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRecord {
    /// The server process's pid.
    pub pid: Pid,
    /// Kernel-assigned identity for this incarnation of `pid`. Pids are
    /// recycled, so liveness alone cannot prove the record still names the
    /// process that the pool started.
    pub identity: ProcessIdentity,
    /// The full argv it was launched with (for diagnostics and drift).
    pub argv: LaunchArgv,
    /// How it is reached — the transport and its coordinates together, so a
    /// reader can never see a port that belongs to a socket.
    pub address: ServerAddress,
    /// Epoch seconds the entry last had no live lease; `None` while leased.
    pub idle_since: Option<u64>,
    /// The same instant in epoch milliseconds, which the linger is measured in
    /// when present: a one-second linger measured in whole seconds can end at
    /// once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_since_ms: Option<u64>,
    /// The value of the server-marker variable the server was launched with
    /// (`ONEHARNESS_APP_SERVER_MARKER`, fresh per server), on a shared Codex
    /// app-server's entry. Such an entry is stopped by that marker — every
    /// process carrying it, the launcher shim and its native child alike —
    /// and its dead leases are retired through the app-server protocol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker: Option<ServerMarker>,
    /// The linger, in milliseconds, the dispatch that started the server asked
    /// for, so the pool-owned sweeper enforces the one this server was started
    /// under. `None` on the entries started before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linger_ms: Option<u64>,
}

impl ServerRecord {
    /// Whether the server this record names is still running — the same pid
    /// *and* the same incarnation of it, which is the rule
    /// [`LeaseHolder::is_live`] states for the holder side of an entry.
    ///
    /// This process's own finished servers are reaped first. A server that
    /// exited a moment ago stays a zombie until its parent waits on it, and a
    /// zombie still answers signal 0 — so without the reap, "did the server we
    /// just launched die?" would answer "no" for the life of the dispatch.
    #[must_use]
    pub fn is_running(&self) -> bool {
        reap_finished();
        pid_alive(self.pid) && process_identity(self.pid).as_ref() == Some(&self.identity)
    }
}

/// An OS-derived process birth identity, compared together with a pid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ProcessIdentity(String);

impl ProcessIdentity {
    #[must_use]
    pub fn new(value: String) -> Option<Self> {
        (!value.is_empty()).then_some(Self(value))
    }
}

impl<'de> Deserialize<'de> for ProcessIdentity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        ProcessIdentity::new(String::deserialize(deserializer)?)
            .ok_or_else(|| D::Error::custom("a process identity must not be empty"))
    }
}

/// The process holding a lease on a pool entry.
///
/// A pid and the birth identity of the incarnation that took the lease, never
/// the pid alone. A lease file outlives its writer only by accident — a holder
/// that was `SIGKILL`ed leaves one behind — and by then the OS may have handed
/// that pid to an unrelated process. Judged on liveness alone, that stranger
/// holds a pooled server for as long as it happens to live: the one leak the
/// reclamation sweep cannot close, because every sweep agrees the holder is
/// alive. Pairing the pid with its incarnation is what makes "still held" a
/// question about the process that actually took the lease.
///
/// This is the same ownership rule [`ServerRecord`] states for the server side
/// of an entry, said once for the holder side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseHolder {
    /// The holding process's pid.
    pub pid: Pid,
    /// Kernel-assigned identity of the incarnation of `pid` that took it.
    pub identity: ProcessIdentity,
}

impl LeaseHolder {
    /// This process, as a lease holder.
    ///
    /// `None` where the platform has no readable birth identity: a lease that
    /// cannot name *which* incarnation holds it is exactly the bare pid this
    /// type exists to rule out, so the pool refuses to take one rather than
    /// write one that a recycled pid could inherit.
    #[must_use]
    pub fn current() -> Option<Self> {
        let pid = Pid::new(std::process::id()).ok()?;
        Some(LeaseHolder {
            pid,
            identity: process_identity(pid)?,
        })
    }

    /// Whether the process that took this lease is still running — the same
    /// pid *and* the same incarnation of it.
    #[must_use]
    pub fn is_live(&self) -> bool {
        pid_alive(self.pid) && process_identity(self.pid).as_ref() == Some(&self.identity)
    }
}

/// What a lease on a shared Codex app-server says beyond its holder: the
/// dispatch's marker, and the thread it holds with the turn running on it —
/// everything the pool needs to stop a dead holder's work without that
/// holder's help. A reader that knows only [`LeaseHolder`] ignores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseNote {
    /// The dispatch marker's value (`ONEHARNESS_DISPATCH_MARKER`).
    pub marker: DispatchMarker,
    /// The app-server thread the dispatch holds, once it holds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<LeasedThread>,
}

/// A thread a lease holds, and the turn running on it while one is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeasedThread {
    pub id: ThreadId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<TurnId>,
}

/// One lease file: the holder, and its note on a shared Codex app-server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseFile {
    #[serde(flatten)]
    pub holder: LeaseHolder,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<LeaseNote>,
}

/// A lease on a pooled server, released when dropped.
///
/// The drop is a courtesy, not the correctness mechanism: reclamation verifies
/// the [`LeaseHolder`] is still running, so a lease whose holder was `SIGKILL`ed
/// is reclaimed by the next pool operation regardless — including when the OS
/// has since handed that pid to something else.
#[derive(Debug)]
pub struct ServerLease {
    entry: PathBuf,
    lease_file: PathBuf,
    holder: LeaseHolder,
    record: ServerRecord,
    linger: Duration,
    started: bool,
}

impl ServerLease {
    /// The server this lease holds.
    #[must_use]
    pub fn record(&self) -> &ServerRecord {
        &self.record
    }

    /// Whether this acquire started the server, rather than finding it running.
    #[must_use]
    pub fn started(&self) -> bool {
        self.started
    }

    /// Rewrite this lease's file with `note` beside its holder — atomically, so
    /// a sweeper never reads half of one.
    pub fn annotate(&self, note: &LeaseNote) -> io::Result<()> {
        let text = serde_json::to_string(&LeaseFile {
            holder: self.holder.clone(),
            note: Some(note.clone()),
        })
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tmp = self.lease_file.with_extension("lease.tmp");
        fs::write(&tmp, text)?;
        fs::rename(tmp, &self.lease_file)
    }

    /// The pool entry directory backing it.
    #[must_use]
    pub fn entry(&self) -> &Path {
        &self.entry
    }
}

impl Drop for ServerLease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lease_file);
        // Mark the entry idle so a later pool operation can time out the linger.
        if let Ok(_guard) = EntryLock::acquire(&self.entry) {
            let _ = reconcile(&self.entry, self.linger);
        }
    }
}

/// The pool root: `<state>/oneharness/servers`, or an explicit override.
#[must_use]
pub fn resolve_root(configured: Option<&str>) -> Option<PathBuf> {
    match configured {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => state_dir().map(|d| d.join("oneharness").join("servers")),
    }
}

/// The per-user state directory, resolved exactly like the history and session
/// stores: `%LOCALAPPDATA%` on Windows; `$XDG_STATE_HOME` (else
/// `~/.local/state`) elsewhere.
pub(crate) fn state_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
    }
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(xdg));
    }
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(|home| PathBuf::from(home).join(".local").join("state"))
}

/// Everything the pool needs to start a server for one key.
///
/// Built only through [`LaunchPlan::new`], so its argv always has a program in
/// it: a plan that cannot name what to spawn is not a smaller plan, it is not a
/// plan, and the pool would otherwise index into an empty vector.
#[derive(Debug)]
pub struct LaunchPlan {
    /// argv[0] is the harness binary; the rest is `ServerSpec::launch` plus any
    /// caller overrides and address flags. Never empty.
    argv: LaunchArgv,
    /// The same argv with the address still spelled `{address}`, kept so a
    /// persisted record can be checked against this plan at the address that
    /// record names. The pool picks a fresh address per acquire, so comparing
    /// the concrete argv would call every reusable server a stranger.
    template: Vec<String>,
    /// Environment applied to the server process.
    env: Vec<(String, String)>,
    /// Whether the server starts from an empty environment holding only
    /// [`Self::env`], rather than inheriting the starting dispatch's.
    env_clear: bool,
    /// The server marker the server is launched with, recorded in its entry.
    marker: Option<ServerMarker>,
    /// The address the server will be reachable at.
    address: ServerAddress,
}

impl LaunchPlan {
    /// The plan for `spec` against `bin`, with `overrides` appended after the
    /// declared launch args. Pure assembly; nothing is spawned.
    ///
    /// Refuses an address that does not speak the transport the spec declares —
    /// a TCP address for a stdio server names something no reader of that pool
    /// entry could dial, and the mismatch would only surface as a connection
    /// failure long after the server was started.
    pub fn new(
        bin: &str,
        spec: &ServerSpec,
        overrides: &[String],
        address: ServerAddress,
        env: Vec<(String, String)>,
    ) -> io::Result<Self> {
        if address.transport() != spec.transport {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "a `{:?}` address cannot back a server declared as `{:?}`",
                    address.transport(),
                    spec.transport
                ),
            ));
        }
        let mut template = vec![bin.to_string()];
        template.extend(spec.launch.iter().map(|a| (*a).to_string()));
        template.extend(spec.address_args.iter().map(|a| (*a).to_string()));
        template.extend(overrides.iter().cloned());
        // The address the pool picked, in the spelling this server's CLI wants.
        // Rendered here rather than by the caller so a pool entry's argv always
        // names the address its record says the server is reachable at.
        let argv = LaunchArgv::new(render(&template, &address))
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
        Ok(LaunchPlan {
            argv,
            template,
            env,
            env_clear: false,
            marker: None,
            address,
        })
    }

    /// The plan for a server whose whole argv the caller assembled, with every
    /// `{address}` placeholder still in it, started from an empty environment
    /// holding only `env` and the server-marker variable set to `marker` —
    /// added here, so the marker the entry records is the one the server
    /// carries. This is the shared Codex app-server's plan: its argv is the
    /// dispatch's own launch plus the listen arguments, not a declared spec.
    pub fn scrubbed(
        template: Vec<String>,
        address: ServerAddress,
        mut env: Vec<(String, String)>,
        marker: ServerMarker,
    ) -> io::Result<Self> {
        let marker_var = crate::domain::app_server::SERVER_MARKER_VAR;
        env.retain(|(name, _)| name != marker_var);
        env.push((marker_var.to_string(), marker.as_str().to_string()));
        let argv = LaunchArgv::new(render(&template, &address))
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
        Ok(LaunchPlan {
            argv,
            template,
            env,
            env_clear: true,
            marker: Some(marker),
            address,
        })
    }

    /// Whether `record` describes the server this plan asks for: the same
    /// program, launch arguments and overrides, at the address the record
    /// itself names.
    ///
    /// The pool entry is the only thing claiming a pid is this key's server,
    /// and it outlives every dispatch — so it can name a pid the OS has since
    /// recycled onto something unrelated, or a launch from before this
    /// harness's argv changed. Liveness alone does not make it ours.
    ///
    /// Checked at the record's own address rather than this plan's: a fresh
    /// address is picked per acquire, so a reusable server is never listening
    /// on the one this dispatch would have used.
    #[must_use]
    pub fn backs(&self, record: &ServerRecord) -> bool {
        record.address.transport() == self.address.transport()
            && render(&self.template, &record.address) == record.argv.as_slice()
    }

    /// The full argv the server is spawned with (argv\[0\] is the program).
    #[must_use]
    pub fn argv(&self) -> &LaunchArgv {
        &self.argv
    }

    /// The address the server will be reachable at.
    #[must_use]
    pub fn address(&self) -> &ServerAddress {
        &self.address
    }
}

/// `template` with every `{address}` placeholder spelled the way this server's
/// CLI takes `address`.
fn render(template: &[String], address: &ServerAddress) -> Vec<String> {
    let rendered = match address {
        ServerAddress::Tcp { port } => port.get().to_string(),
        ServerAddress::UnixSocket { path } => path.to_string(),
        ServerAddress::Stdio => String::new(),
    };
    template
        .iter()
        .map(|arg| arg.replace("{address}", &rendered))
        .collect()
}

/// Take a lease on the server for `key` under `root`, starting one if none is
/// live. Reuse is the normal path: a second dispatch with the same key gets the
/// same record without spawning anything.
pub fn acquire(
    root: &Path,
    key: &PoolKey,
    plan: &LaunchPlan,
    linger: Duration,
) -> io::Result<ServerLease> {
    let entry = root.join(key.as_str());
    fs::create_dir_all(entry.join(LEASE_DIR))?;
    let _guard = EntryLock::acquire(&entry)?;

    // Resolved before anything is started: a dispatch that cannot say who it is
    // cannot take a lease, and starting a server it could not then hold would
    // leave one running that no sweep is allowed to reclaim.
    let holder = LeaseHolder::current().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "this process has no readable identity, so it cannot take a pool lease",
        )
    })?;

    let mut started = true;
    let record = match reconcile(&entry, linger)? {
        Some(live)
            if plan.backs(&live) && process_identity(live.pid).as_ref() == Some(&live.identity) =>
        {
            started = false;
            live
        }
        // A live record this plan does not describe is not this plan's server:
        // the entry outlives every dispatch, so its pid may have been recycled
        // onto something unrelated, or its argv may predate a change to the
        // launch. Reusing it would submit the turn to whatever answers at that
        // address; the record is dropped and a server this dispatch can vouch
        // for is started in its place.
        //
        // The process itself is deliberately NOT signalled: not being able to
        // say what it is, is the whole reason it is not being reused.
        Some(_) => {
            let _ = fs::remove_file(entry.join(SERVER_FILE));
            start(&entry, plan, linger)?
        }
        None => start(&entry, plan, linger)?,
    };

    let lease_file = entry.join(LEASE_DIR).join(lease_id());
    fs::write(
        &lease_file,
        serde_json::to_string(&holder)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
    )?;
    // Now leased again: clear the idle stamp so the linger only ever measures
    // uninterrupted idleness.
    let cleared = ServerRecord {
        idle_since: None,
        idle_since_ms: None,
        ..record.clone()
    };
    write_record(&entry, &cleared)?;

    Ok(ServerLease {
        entry,
        lease_file,
        holder,
        record: cleared,
        linger,
        started,
    })
}

/// Drop dead leases, then decide the entry's fate: keep a live server, or
/// reclaim one whose idle linger has expired. Returns the still-usable server,
/// if any. This is the single reclamation path — every pool operation runs it,
/// which is why a `SIGKILL`ed holder cannot leak a server.
pub(crate) fn reconcile(entry: &Path, linger: Duration) -> io::Result<Option<ServerRecord>> {
    reap_finished();
    let recorded = read_record(entry);
    let leases = entry.join(LEASE_DIR);
    let mut live_leases = 0usize;
    if let Ok(entries) = fs::read_dir(&leases) {
        for lease in entries.flatten() {
            let path = lease.path();
            if path.extension().is_some_and(|ext| ext == "tmp") {
                continue;
            }
            let text = fs::read_to_string(&path).ok();
            let holder = text
                .as_deref()
                .and_then(|text| serde_json::from_str::<LeaseHolder>(text).ok());
            match holder {
                Some(holder) if holder.is_live() => live_leases += 1,
                // No readable holder, or one whose process is gone — including
                // a pid the OS still knows but has since handed to something
                // else, which is a stranger and not this lease's owner. The
                // holder is gone, however it went. On a shared Codex
                // app-server the work it left running is stopped first: the
                // server does not notice a dead client, and keeps its turn and
                // its commands running.
                _ => {
                    if let Some(record) = recorded.as_ref().filter(|r| r.marker.is_some()) {
                        let note = text
                            .as_deref()
                            .and_then(|text| serde_json::from_str::<LeaseFile>(text).ok())
                            .and_then(|file| file.note);
                        if let Some(note) = note {
                            crate::io::app_server::retire_dead_lease(record, &note);
                        }
                    }
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }

    let Some(mut record) = recorded else {
        return Ok(None);
    };
    if !pid_alive(record.pid) {
        // The server died on its own; forget it so the next acquire restarts.
        // A marked server's launcher can die and leave its native child
        // serving, so whatever still carries the marker is stopped too.
        if record.marker.is_some() {
            crate::io::app_server::stop_server(entry, &record);
        }
        let _ = fs::remove_file(entry.join(SERVER_FILE));
        return Ok(None);
    }
    if live_leases > 0 {
        record.idle_since = None;
        record.idle_since_ms = None;
        write_record(entry, &record)?;
        return Ok(Some(record));
    }

    // No live lease: the linger measures how long that has been true. An entry
    // seen idle for the first time is stamped now, so `now - since` is 0 and a
    // zero linger reclaims immediately while a real one keeps the process warm.
    let now = epoch_secs();
    let now_ms = epoch_millis();
    let since_ms = record
        .idle_since_ms
        .or(record.idle_since.map(|secs| secs.saturating_mul(1000)))
        .unwrap_or(now_ms);
    let linger = record.linger_ms.map_or(linger, Duration::from_millis);
    if u128::from(now_ms.saturating_sub(since_ms)) >= linger.as_millis() {
        if record.marker.is_some() {
            crate::io::app_server::stop_server(entry, &record);
            reap_spawned(record.pid.get());
        } else {
            kill(record.pid);
        }
        let _ = fs::remove_file(entry.join(SERVER_FILE));
        return Ok(None);
    }
    if record.idle_since.is_none() {
        record.idle_since = Some(now);
        record.idle_since_ms = Some(now_ms);
        write_record(entry, &record)?;
    }
    Ok(Some(record))
}

/// Reclaim every pool entry whose server has no live lease and whose linger has
/// expired. Lazily driven (any pool operation, or an explicit sweep) rather
/// than by a daemon: there is no oneharness process alive to do it otherwise.
pub fn sweep(root: &Path, linger: Duration) -> io::Result<usize> {
    let mut reclaimed = 0;
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(0);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(_guard) = EntryLock::acquire(&path) else {
            continue;
        };
        let had = read_record(&path).is_some();
        if reconcile(&path, linger)?.is_none() && had {
            reclaimed += 1;
        }
    }
    Ok(reclaimed)
}

/// Spawn the server and record it. The child is detached from this dispatch's
/// process tree on purpose: it must outlive the run that started it, which is
/// the entire point of pooling.
///
/// It is also started in the pool entry's own directory rather than inheriting
/// the starting dispatch's cwd. A pooled server belongs to no one dispatch —
/// every turn carries its own working directory on the wire — so an inherited
/// cwd is an arbitrary project directory that later dispatches never chose, and
/// anything the server resolves relative to it (a tool call that slips the
/// per-turn directory, a log, a scratch file) lands in that project's tree. It
/// did: a live control run from this repository's root left the agent's
/// `step-*.txt` files in the checkout.
///
/// A shared Codex app-server's stderr goes to the entry's log, started afresh
/// with each server, so a server that exits before it serves can be asked why
/// ([`early_exit`]): nobody reads a detached process's stderr, and a pipe
/// would outlive its reader with the dispatch that started it.
fn start(entry: &Path, plan: &LaunchPlan, linger: Duration) -> io::Result<ServerRecord> {
    let (program, args) = plan.argv.split();
    let stderr = if plan.marker.is_some() {
        open_server_log(entry)
            .map_or_else(|_| std::process::Stdio::null(), std::process::Stdio::from)
    } else {
        std::process::Stdio::null()
    };
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .current_dir(entry)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(stderr);
    if plan.env_clear {
        command.env_clear();
    }
    for (key, value) in &plan.env {
        command.env(key, value);
    }
    detach(&mut command);
    let child = command.spawn()?;
    let pid =
        Pid::new(child.id()).map_err(|message| io::Error::new(io::ErrorKind::Other, message))?;
    let identity = identify_spawned(child, pid, process_identity, IDENTIFY_GRACE)?;
    let record = ServerRecord {
        pid,
        identity,
        argv: plan.argv.clone(),
        address: plan.address.clone(),
        idle_since: None,
        idle_since_ms: None,
        marker: plan.marker.clone(),
        linger_ms: plan
            .marker
            .is_some()
            .then(|| u64::try_from(linger.as_millis()).unwrap_or(u64::MAX)),
    };
    write_record(entry, &record)?;
    Ok(record)
}

/// The entry's server log, emptied for the server about to start and readable
/// by its owner alone: what a server says may name its configuration. It is
/// opened without following a link or waiting on a FIFO, and anything there
/// but a regular file is refused rather than written through.
fn open_server_log(entry: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(entry.join(SERVER_LOG))?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "the server log is not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.set_len(0)?;
    Ok(file)
}

/// Why the server `record` names, in the pool entry `entry`, stopped before it
/// served: its exit status when this process started and reaped it, and the
/// tail of what it wrote to stderr. Read once the server is known not to be
/// running.
#[must_use]
pub fn early_exit(entry: &Path, record: &ServerRecord) -> String {
    let status = exit_status(record.pid).map_or_else(
        // llmlint: ignore[changed_behavior_has_e2e] An unknown status needs a server another dispatch started to die after that dispatch connected and before this one's first dial, a window no journey can place a death inside; the silent and long-stderr halves beside it are proven by `a_server_that_exits_before_listening_fails_the_dispatch_with_its_own_reason`.
        || "its exit status is unknown here, since another process started it".to_string(),
        |status| format!("it ended with {status}"),
    );
    match stderr_tail(&entry.join(SERVER_LOG)) {
        Some(tail) => format!("{status}, its stderr ending: {tail}"),
        None => format!("{status}, and wrote nothing to stderr"),
    }
}

/// The last [`STDERR_TAIL_BYTES`] of `log`, trimmed, with any bytes that are
/// not UTF-8 replaced; `None` when it is absent or a link (never followed), is
/// not a regular file (never waited on, as a FIFO would be), or holds only
/// whitespace.
fn stderr_tail(log: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(log).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let len = file.metadata().ok()?.len();
    let from = len.saturating_sub(STDERR_TAIL_BYTES);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim();
    (!text.is_empty()).then(|| {
        if from > 0 {
            format!("…{text}")
        } else {
            text.to_string()
        }
    })
}

/// The identity of the server just spawned as `child` (`pid`), as `read`
/// answers it, keeping the child to be reaped later.
///
/// Gone before it could be identified — macOS answers nothing for a process
/// that has exited, while Linux still reads a zombie's — is a server that
/// exited at start, not a launch that failed: it is identified as
/// [`EXITED_UNIDENTIFIED`], which no running process can match, so it reads as
/// not running and its caller reports the exit (status included) and
/// relaunches, as it would for one that died a moment later.
///
/// macOS stops answering for a process as soon as it starts to exit, before a
/// wait can see it gone, so one that answers neither is given up to `grace` to
/// settle into one or the other before the launch is refused.
fn identify_spawned(
    mut child: std::process::Child,
    pid: Pid,
    read: impl Fn(Pid) -> Option<ProcessIdentity>,
    grace: Duration,
) -> io::Result<ProcessIdentity> {
    let until = Instant::now() + grace;
    loop {
        if let Some(identity) = read(pid) {
            remember_spawned(child);
            return Ok(identity);
        }
        if let Ok(Some(status)) = child.try_wait() {
            remember_exit(pid.get(), status);
            // llmlint: ignore[invalid_states_unrepresentable] The identity is written to server.json, which every oneharness release sharing the pool reads; a value no kernel reading can spell makes each of them, releases before this one included, read the record as a server not running, where a new state would change a record shape they cannot parse.
            return Ok(ProcessIdentity(EXITED_UNIDENTIFIED.to_string()));
        }
        if Instant::now() >= until {
            break;
        }
        std::thread::sleep(IDENTIFY_POLL);
    }
    remember_spawned(child);
    // llmlint: ignore[changed_behavior_has_e2e] A child still running whose start time the kernel will not answer cannot be staged without mocking `/proc` or `proc_pidinfo`, the platform layer this reads; `identify_spawned` takes its reader as a parameter so the unit tests drive this branch, and it is the error the launch returned before this change.
    Err(io::Error::new(
        io::ErrorKind::Other,
        format!(
            "could not read the identity of spawned server pid {}",
            pid.get()
        ),
    ))
}

/// Read a kernel value that changes when a pid is recycled.
#[must_use]
pub fn process_identity(pid: Pid) -> Option<ProcessIdentity> {
    #[cfg(target_os = "linux")]
    {
        let stat = fs::read_to_string(format!("/proc/{}/stat", pid.get())).ok()?;
        // The command name is parenthesized and may contain spaces. Fields
        // after its final ')' begin at field 3; starttime is field 22.
        let tail = stat.rsplit_once(')')?.1;
        let start_ticks = tail.split_whitespace().nth(19)?;
        ProcessIdentity::new(start_ticks.to_string())
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
        use windows_sys::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: every out-pointer is a live local FILETIME, and the handle is
        // checked for null before use and closed exactly once.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid.get());
            if handle.is_null() {
                return None;
            }
            let mut creation = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
            CloseHandle(handle);
            if ok == 0 {
                return None;
            }
            let ticks =
                (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
            ProcessIdentity::new(ticks.to_string())
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::ffi::c_void;

        const PROC_PIDTBSDINFO: libc::c_int = 3;
        #[repr(C)]
        #[derive(Default)]
        struct ProcBsdInfo {
            flags: u32,
            status: u32,
            xstatus: u32,
            pid: u32,
            ppid: u32,
            uid: u32,
            gid: u32,
            ruid: u32,
            rgid: u32,
            svuid: u32,
            svgid: u32,
            reserved: u32,
            comm: [u8; 16],
            name: [u8; 32],
            nfiles: u32,
            pgid: u32,
            pjobc: u32,
            ttydev: u32,
            tpgid: u32,
            nice: i32,
            start_seconds: u64,
            start_microseconds: u64,
        }
        // SAFETY: the signature matches libproc's `proc_pidinfo` declaration in
        // <libproc.h>.
        #[link(name = "proc")]
        unsafe extern "C" {
            fn proc_pidinfo(
                pid: libc::c_int,
                flavor: libc::c_int,
                arg: u64,
                buffer: *mut c_void,
                buffer_size: libc::c_int,
            ) -> libc::c_int;
        }

        let mut info = ProcBsdInfo::default();
        let size = std::mem::size_of::<ProcBsdInfo>();
        // SAFETY: the buffer is a live, writable `ProcBsdInfo` whose size is the
        // length passed; the kernel writes at most that many bytes.
        let read = unsafe {
            proc_pidinfo(
                pid.get() as libc::c_int,
                PROC_PIDTBSDINFO,
                0,
                (&raw mut info).cast(),
                size as libc::c_int,
            )
        };
        if read != size as libc::c_int {
            return None;
        }
        ProcessIdentity::new(format!(
            "{}:{}",
            info.start_seconds, info.start_microseconds
        ))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        // Refusing reuse is safe on platforms where this crate does not yet
        // have a kernel birth-token reader; adopting a recycled pid is not.
        let _ = pid;
        None
    }
}

/// Start `command` detached from the caller: in a session of its own, holding
/// no descriptor but the three standard ones it was given.
///
/// Its own session/process group, so terminating the dispatch's tree does not
/// take the shared process down with it. No other descriptor, because a
/// detached process outlives its caller: anything it inherits stays open as
/// long as it runs. A caller's descriptors are not all close-on-exec — a
/// launcher that passed its own output on at another number, or, on macOS,
/// a pipe or socket another thread had just made (`std` marks those
/// close-on-exec only after creating them) — and a shared server holding one
/// end of a caller's output pipe keeps whoever reads that pipe from ever
/// seeing end-of-file.
#[cfg(unix)]
pub(crate) fn detach(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the hook runs in the forked child before exec and calls only
    // `setsid`, which is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    crate::io::process::close_inherited_on_exec(command);
}

#[cfg(not(unix))]
pub(crate) fn detach(_command: &mut std::process::Command) {}

/// Whether `pid` is still running. This is the pool's whole correctness
/// argument, so it asks the kernel rather than trusting any bookkeeping.
///
/// A zombie is not running. It still answers signal 0 on every Unix, and it
/// stays one for as long as its parent lives without waiting on it — a shared
/// server is the child of whichever dispatch started it, and that dispatch
/// waits on nothing while its turn runs. Counted as alive, a dead server would
/// never be stopped by marker, so a command it started that kept a client's
/// connection open would hold that client waiting for end-of-file forever.
#[must_use]
pub fn pid_alive(pid: Pid) -> bool {
    let pid = pid.get();
    #[cfg(unix)]
    {
        // Signal 0 performs the existence/permission check without delivering.
        // SAFETY: `kill` takes plain integers and signal 0 delivers nothing.
        let known = unsafe { libc::kill(pid as libc::pid_t, 0) == 0 };
        known && !is_zombie(pid)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: the handle is checked for null before use and closed once.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            CloseHandle(handle);
            true
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

/// Whether `pid` has exited and is waiting to be reaped, as the kernel reports
/// its state: `Z` in `/proc/<pid>/stat` on Linux; on macOS, a pid
/// `proc_pidinfo` cannot find. A state that cannot be read otherwise is not a
/// zombie: the caller's signal 0 has already said the pid exists.
#[cfg(unix)]
fn is_zombie(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .is_some_and(|state| matches!(state, "Z" | "X"))
        })
    }
    #[cfg(target_os = "macos")]
    {
        // <sys/proc.h>'s `SZOMB`, the `p_stat` of a process awaiting collection.
        const SZOMB: u32 = 5;
        let Ok(pid) = libc::c_int::try_from(pid) else {
            return false;
        };
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: `info` is writable for `size` bytes, the size of the
        // structure PROC_PIDTBSDINFO fills.
        let filled = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if filled == size {
            // SAFETY: the kernel filled all `size` bytes of the structure.
            return unsafe { info.assume_init() }.pbi_status == SZOMB;
        }
        // With a zero `arg` XNU looks the pid up among processes that have not
        // exited (`proc_find`, which skips one marked dead at exit) and answers
        // ESRCH for any other — while signal 0 still succeeds for an exited,
        // unreaped pid (`kill`'s `pzfind` branch). So ESRCH after signal 0
        // answered is exactly an exited process. Any other failure says
        // nothing about its state.
        filled <= 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        false
    }
}

/// Servers this process itself spawned, retained only so a still-running
/// dispatch can *reap* one it later reclaims. Without this the terminated
/// server lingers as a zombie for the life of the parent — harmless in a short
/// CLI run, but it would make "is the pid gone?" answer wrong for a long-lived
/// supervisor. A server outliving its starter is reparented and reaped by init.
fn spawned() -> &'static std::sync::Mutex<Vec<std::process::Child>> {
    static SPAWNED: std::sync::OnceLock<std::sync::Mutex<Vec<std::process::Child>>> =
        std::sync::OnceLock::new();
    SPAWNED.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// How many reaped servers' exit statuses are kept for [`exit_status`].
const EXITED_KEPT: usize = 32;

/// The exit statuses of the servers this process reaped, newest last: the
/// reap is what makes a dead server read as dead, and it consumes the status.
fn exited() -> &'static std::sync::Mutex<Vec<(u32, std::process::ExitStatus)>> {
    static EXITED: std::sync::OnceLock<std::sync::Mutex<Vec<(u32, std::process::ExitStatus)>>> =
        std::sync::OnceLock::new();
    EXITED.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn remember_exit(pid: u32, status: std::process::ExitStatus) {
    // llmlint: ignore[changed_behavior_has_e2e] Reaching the bound end to end takes one long-lived process starting and reaping more than EXITED_KEPT real servers, and all it changes is whether a stale early-exit diagnostic names a status or says it is unknown; `only_the_newest_reaped_exit_statuses_are_kept` holds the bound and the replacement through this function and `exit_status`, and every journey that reports an exit status reads the same path.
    if let Ok(mut statuses) = exited().lock() {
        statuses.retain(|(known, _)| *known != pid);
        statuses.push((pid, status));
        let excess = statuses.len().saturating_sub(EXITED_KEPT);
        statuses.drain(..excess);
    }
}

/// How the server `pid` ended, when this process started it and has reaped it.
fn exit_status(pid: Pid) -> Option<std::process::ExitStatus> {
    reap_finished();
    let statuses = exited().lock().ok()?;
    statuses
        .iter()
        .rev()
        .find(|(known, _)| *known == pid.get())
        .map(|(_, status)| *status)
}

fn remember_spawned(child: std::process::Child) {
    if let Ok(mut children) = spawned().lock() {
        children.push(child);
    }
}

/// Reap a server this process started, if it is one of ours.
fn reap_spawned(pid: u32) {
    let Ok(mut children) = spawned().lock() else {
        return;
    };
    if let Some(index) = children.iter().position(|child| child.id() == pid) {
        let mut child = children.remove(index);
        if let Ok(status) = child.wait() {
            remember_exit(pid, status);
        }
    }
}

/// Reap any server of ours that has already exited, so a dead pid reads as dead
/// rather than as a zombie that still answers signal 0.
fn reap_finished() {
    let Ok(mut children) = spawned().lock() else {
        return;
    };
    children.retain_mut(|child| match child.try_wait() {
        Ok(Some(status)) => {
            remember_exit(child.id(), status);
            false
        }
        _ => true,
    });
}

fn kill(pid: Pid) {
    let pid = pid.get();
    // SAFETY: `kill` takes plain integers; a pid or group already gone is an
    // ignored error, not undefined behavior.
    #[cfg(unix)]
    unsafe {
        // The server was started in its own session; signal the whole group so
        // a launcher shim's native child goes with it.
        libc::kill(-(pid as libc::pid_t), libc::SIGTERM);
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, TerminateProcess, PROCESS_TERMINATE,
        };
        // SAFETY: the handle is checked for null before use and closed once.
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !handle.is_null() {
                TerminateProcess(handle, 1);
                CloseHandle(handle);
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
    }
    reap_spawned(pid);
}

pub(crate) fn read_record(entry: &Path) -> Option<ServerRecord> {
    let text = fs::read_to_string(entry.join(SERVER_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_record(entry: &Path, record: &ServerRecord) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(record)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    text.push('\n');
    let tmp = entry.join("server.json.tmp");
    fs::write(&tmp, text)?;
    fs::rename(tmp, entry.join(SERVER_FILE))
}

fn lease_id() -> String {
    format!(
        "{}-{}.lease",
        std::process::id(),
        uuid::Uuid::now_v7().simple()
    )
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A whole-entry mutex around start/stop decisions, so two dispatches racing to
/// first-acquire the same key cannot both spawn a server. Uses the same
/// cross-platform advisory locking as the history index.
pub(crate) struct EntryLock {
    _file: fs::File,
}

impl EntryLock {
    pub(crate) fn acquire(entry: &Path) -> io::Result<Self> {
        use fs2::FileExt;
        fs::create_dir_all(entry)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(entry.join(LOCK_FILE))?;
        file.lock_exclusive()?;
        Ok(EntryLock { _file: file })
    }
}

impl Drop for EntryLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self._file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::control::ServerTransport;
    use crate::io::scratch::ScratchDir;

    /// A long-lived supervisor reaps servers without end, so only the newest
    /// [`EXITED_KEPT`] statuses are kept: an older one reads as unknown, the
    /// early-exit report's "another process started it" fallback, and a pid
    /// reaped again keeps only its latest status. The pids are ones no process
    /// on a test host has, so concurrent tests' reaps cannot collide with them.
    #[test]
    fn only_the_newest_reaped_exit_statuses_are_kept() {
        #[cfg(unix)]
        let status = |code: i32| {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code << 8)
        };
        #[cfg(windows)]
        let status = |code: i32| {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code as u32)
        };
        let base = 4_000_000_000_u32;
        let pid = |offset: usize| Pid::new(base + offset as u32).unwrap();
        remember_exit(pid(0).get(), status(9));
        remember_exit(pid(0).get(), status(1));
        assert_eq!(exit_status(pid(0)).and_then(|s| s.code()), Some(1));
        for offset in 1..=EXITED_KEPT {
            remember_exit(pid(offset).get(), status(2));
        }
        assert_eq!(exit_status(pid(0)), None, "the oldest status was kept");
        assert_eq!(
            exit_status(pid(EXITED_KEPT)).and_then(|s| s.code()),
            Some(2)
        );
    }

    #[test]
    fn a_lease_note_names_only_usable_identities_and_refuses_the_rest_when_read() {
        assert_eq!(ThreadId::new("th-1").unwrap().as_str(), "th-1");
        assert_eq!(ThreadId::new(""), None);
        assert_eq!(TurnId::new("tu\n1"), None);
        assert_ne!(DispatchMarker::fresh(), DispatchMarker::fresh());
        let note = LeaseNote {
            marker: DispatchMarker::new("m-1").unwrap(),
            thread: Some(LeasedThread {
                id: ThreadId::new("th-1").unwrap(),
                turn: TurnId::new("tu-1"),
            }),
        };
        let text = serde_json::to_string(&note).unwrap();
        assert_eq!(
            text,
            r#"{"marker":"m-1","thread":{"id":"th-1","turn":"tu-1"}}"#
        );
        assert_eq!(serde_json::from_str::<LeaseNote>(&text).unwrap(), note);
        for bad in [
            r#"{"marker":""}"#,
            r#"{"marker":"m-1","thread":{"id":""}}"#,
            r#"{"marker":"m-1","thread":{"id":"th-1","turn":"\u0007"}}"#,
        ] {
            let refused = serde_json::from_str::<LeaseNote>(bad).unwrap_err();
            assert!(refused.to_string().contains("is not a"), "{bad}: {refused}");
        }
    }

    /// A server gone before its identity could be read — as macOS reports a
    /// process that has exited — is a server that exited at start: it reads as
    /// not running, and how it ended is kept for its caller to report. One the
    /// kernel cannot identify while it still runs is refused.
    #[cfg(unix)]
    #[test]
    fn a_server_gone_before_it_was_identified_reads_as_exited_with_its_status() {
        let child = std::process::Command::new("sh")
            .args(["-c", "exit 5"])
            .spawn()
            .unwrap();
        let pid = Pid::new(child.id()).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(10);
        while pid_alive(pid) && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        let identity = identify_spawned(child, pid, |_| None, Duration::ZERO).unwrap();
        let record = ServerRecord {
            pid,
            identity,
            argv: LaunchArgv::new(vec!["sh".to_string()]).unwrap(),
            address: ServerAddress::Stdio,
            idle_since: None,
            idle_since_ms: None,
            marker: None,
            linger_ms: None,
        };
        assert!(!record.is_running(), "{record:?}");
        assert_eq!(exit_status(pid).and_then(|status| status.code()), Some(5));

        let running = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let running_pid = Pid::new(running.id()).unwrap();
        let refused = identify_spawned(running, running_pid, |_| None, Duration::from_millis(50))
            .unwrap_err();
        kill(running_pid);
        assert!(
            refused
                .to_string()
                .contains("could not read the identity of spawned server pid"),
            "{refused}"
        );
    }

    /// macOS stops answering for a process while it is still exiting, before
    /// a wait can see it gone: a server unidentifiable at once that exits a
    /// moment later is still a server that exited at start, with its status,
    /// not a launch that failed.
    #[cfg(unix)]
    #[test]
    fn a_server_still_exiting_when_it_could_not_be_identified_reads_as_exited() {
        let child = std::process::Command::new("sh")
            .args(["-c", "sleep 0.2; exit 6"])
            .spawn()
            .unwrap();
        let pid = Pid::new(child.id()).unwrap();
        let identity = identify_spawned(child, pid, |_| None, Duration::from_secs(10)).unwrap();
        assert_eq!(identity.0, EXITED_UNIDENTIFIED);
        assert_eq!(exit_status(pid).and_then(|status| status.code()), Some(6));
    }

    /// Unix-gated with the lifecycle tests that use it: every acquire/sweep test
    /// drives a real long-lived child through [`sleeper_plan`], so this key has
    /// no consumer on a platform where that stand-in server does not exist.
    #[cfg(unix)]
    fn key() -> PoolKey {
        crate::domain::control::pool_key("test", &[], &[])
    }

    fn temp_root(tag: &str) -> ScratchDir {
        ScratchDir::new(&format!("pool-{tag}-{}", epoch_secs())).unwrap()
    }

    /// A stand-in server: a real, long-lived child process. The pool only ever
    /// asks the OS about pids, so a `sleep` exercises exactly the same paths a
    /// `codex app-server` would.
    ///
    /// Unix-gated because `sleep` is the stand-in: Windows has no such program
    /// on PATH, so every test that reaches for one is `#[cfg(unix)]` too. That
    /// is a gap in where the pool's lifecycle is *proven*, not in where it
    /// works — `pid_alive`/`process_identity` carry real Windows arms.
    #[cfg(unix)]
    fn sleeper_plan() -> LaunchPlan {
        LaunchPlan::new(
            "sleep",
            &ServerSpec {
                launch: &["120"],
                address_args: &[],
                key_env: &[],
                transport: ServerTransport::Stdio,
            },
            &[],
            ServerAddress::Stdio,
            Vec::new(),
        )
        .expect("a stdio address backs a stdio spec")
    }

    #[test]
    fn a_key_that_could_escape_the_pool_root_cannot_be_built() {
        let root = temp_root("badkey");
        assert!(PoolKey::parse("../escape").is_err());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_launch_argv_must_actually_name_a_program() {
        assert!(LaunchArgv::new(Vec::new()).is_err());
        assert!(LaunchArgv::new(vec![String::new()]).is_err());
        assert!(LaunchArgv::new(vec!["  ".to_string(), "serve".to_string()]).is_err());
        let argv = LaunchArgv::new(vec!["opencode".to_string(), "serve".to_string()]).unwrap();
        assert_eq!(argv.split().0, "opencode");
    }

    #[test]
    fn a_persisted_process_identity_must_not_be_empty() {
        assert!(serde_json::from_str::<ProcessIdentity>(r#"""#).is_err());
        assert_eq!(
            serde_json::from_str::<ProcessIdentity>(r#""12345""#).unwrap(),
            ProcessIdentity::new("12345".to_string()).unwrap()
        );
    }

    #[test]
    fn a_lease_file_naming_only_a_pid_is_not_a_holder() {
        // The shape a bare pid would have. It cannot say WHICH incarnation of
        // that pid took the lease, which is the only question reclamation asks
        // — so it is refused at the boundary rather than read as ownership.
        assert!(serde_json::from_str::<LeaseHolder>("12345").is_err());
        assert!(serde_json::from_str::<LeaseHolder>(r#"{"pid":12345}"#).is_err());
        assert!(serde_json::from_str::<LeaseHolder>(r#"{"pid":0,"identity":"a"}"#).is_err());
        let holder = LeaseHolder::current().expect("this process has a readable identity");
        assert_eq!(holder.pid.get(), std::process::id());
        assert!(holder.is_live());
        assert_eq!(
            serde_json::from_str::<LeaseHolder>(&serde_json::to_string(&holder).unwrap()).unwrap(),
            holder
        );
    }

    #[test]
    fn resolve_root_prefers_the_configured_path() {
        assert_eq!(
            resolve_root(Some("/custom/servers")),
            Some(PathBuf::from("/custom/servers"))
        );
        assert_eq!(resolve_root(Some("")), resolve_root(None));
    }

    #[test]
    fn launch_plan_appends_declared_args_then_overrides() {
        let spec = ServerSpec {
            launch: &["serve"],
            address_args: &["--port", "{address}"],
            key_env: &[],
            transport: ServerTransport::Tcp,
        };
        let address = ServerAddress::Tcp {
            port: crate::domain::control::Port::new(7777).unwrap(),
        };
        let plan = LaunchPlan::new(
            "opencode",
            &spec,
            &["--log-level".to_string(), "ERROR".to_string()],
            address.clone(),
            Vec::new(),
        )
        .unwrap();
        // The address the POOL picked is rendered onto the argv, so an entry's
        // command always names the address its record says it answers on;
        // caller overrides follow it.
        assert_eq!(
            plan.argv().as_slice(),
            [
                "opencode",
                "serve",
                "--port",
                "7777",
                "--log-level",
                "ERROR"
            ]
        );
        assert_eq!(plan.address(), &address);

        // An address that speaks a different transport is refused, not asserted
        // away in debug builds only.
        let mismatched = LaunchPlan::new("opencode", &spec, &[], ServerAddress::Stdio, Vec::new());
        assert_eq!(mismatched.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn one_server_is_reused_across_dispatches_sharing_a_key() {
        let root = temp_root("reuse");
        let first = acquire(&root, &key(), &sleeper_plan(), DEFAULT_LINGER).unwrap();
        let second = acquire(&root, &key(), &sleeper_plan(), DEFAULT_LINGER).unwrap();
        assert_eq!(first.record().pid, second.record().pid);
        assert!(pid_alive(first.record().pid));

        // Two live leases on the one entry.
        let leases = fs::read_dir(root.join(key().as_str()).join(LEASE_DIR))
            .unwrap()
            .count();
        assert_eq!(leases, 2);

        let pid = first.record().pid;
        drop(first);
        drop(second);
        // Still alive: the linger has not expired, which is what keeps bursty
        // rounds from thrashing the process.
        assert!(pid_alive(pid));
        assert_eq!(sweep(&root, DEFAULT_LINGER).unwrap(), 0);
        assert!(pid_alive(pid));

        // Once idle past the linger it is reclaimed.
        assert_eq!(sweep(&root, Duration::from_secs(0)).unwrap(), 1);
        wait_until_dead(pid);
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_sigkilled_lease_holder_does_not_leak_the_server() {
        let root = temp_root("sigkill");
        // A real second process takes the lease and is killed without releasing
        // it — the exact leak a reference counter cannot survive.
        let holder = std::process::Command::new("sleep")
            .arg("120")
            .spawn()
            .unwrap();
        let holder_pid = Pid::new(holder.id()).unwrap();
        let entry = root.join(key().as_str());
        fs::create_dir_all(entry.join(LEASE_DIR)).unwrap();
        let ours = acquire(&root, &key(), &sleeper_plan(), DEFAULT_LINGER).unwrap();
        let server_pid = ours.record().pid;
        fs::write(
            entry.join(LEASE_DIR).join("stray.lease"),
            serde_json::to_string(&LeaseHolder {
                pid: holder_pid,
                identity: process_identity(holder_pid).expect("a live process has an identity"),
            })
            .unwrap(),
        )
        .unwrap();
        drop(ours);

        // While the stray holder lives, the server is retained even past the
        // linger: reclamation is by lease liveness, not by elapsed time.
        assert_eq!(sweep(&root, Duration::from_secs(0)).unwrap(), 0);
        assert!(pid_alive(server_pid));

        let mut holder = holder;
        // SAFETY: `kill` takes plain integers; the holder is this test's child.
        unsafe {
            libc::kill(holder_pid.get() as libc::pid_t, libc::SIGKILL);
        }
        holder.wait().ok();

        // The holder never released anything; the pool notices it is gone.
        assert_eq!(sweep(&root, Duration::from_secs(0)).unwrap(), 1);
        wait_until_dead(server_pid);
        assert!(!entry.join(LEASE_DIR).join("stray.lease").exists());
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_started_server_runs_in_the_pool_entry_not_the_starting_dispatch_cwd() {
        let root = temp_root("cwd");
        // A stand-in server that records where it was started, then stays alive
        // like a real one. The dispatch's own cwd is this crate's source tree,
        // so an inherited cwd would show up as anything but the pool entry.
        let plan = LaunchPlan::new(
            "sh",
            &ServerSpec {
                launch: &["-c"],
                address_args: &[],
                key_env: &[],
                transport: ServerTransport::Stdio,
            },
            &["pwd > started-in.txt; sleep 120".to_string()],
            ServerAddress::Stdio,
            Vec::new(),
        )
        .expect("a stdio address backs a stdio spec");
        let lease = acquire(&root, &key(), &plan, DEFAULT_LINGER).unwrap();
        let entry = lease.entry().to_path_buf();

        let marker = entry.join("started-in.txt");
        let mut started_in = None;
        for _ in 0..200 {
            // A path only once there is one: the redirection creates the file
            // before `pwd` writes into it, so an existing-but-empty marker is a
            // read that arrived mid-write, not a server started in nowhere.
            match fs::read_to_string(&marker) {
                Ok(text) if !text.trim().is_empty() => {
                    started_in = Some(text.trim().to_string());
                    break;
                }
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let started_in = started_in.expect(
            "the server wrote its cwd marker somewhere other than the pool entry, so it \
             inherited the starting dispatch's working directory",
        );
        assert_eq!(
            fs::canonicalize(&started_in).unwrap(),
            fs::canonicalize(&entry).unwrap(),
            "a pooled server must not inherit the starting dispatch's working directory"
        );

        drop(lease);
        sweep(&root, Duration::from_secs(0)).unwrap();
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_reusable_server_is_vouched_for_at_its_own_address_not_this_acquires() {
        // The pool asks the OS for a free port on every acquire, so a reusable
        // server is never listening on the address the next dispatch would have
        // picked. A record checked against this plan's concrete argv would call
        // every one of them a stranger, and each dispatch would start its own
        // copy of a ~137MB process — the cost pooling exists to avoid.
        let root = temp_root("address");
        let spec = ServerSpec {
            launch: &["-c", "sleep 120"],
            address_args: &["--port", "{address}"],
            key_env: &[],
            transport: ServerTransport::Tcp,
        };
        let plan_at = |port: u16| {
            LaunchPlan::new(
                "sh",
                &spec,
                &[],
                ServerAddress::Tcp {
                    port: crate::domain::control::Port::new(port).unwrap(),
                },
                Vec::new(),
            )
            .expect("a tcp address backs a tcp spec")
        };
        let first = acquire(&root, &key(), &plan_at(7777), DEFAULT_LINGER).unwrap();
        let second = acquire(&root, &key(), &plan_at(8888), DEFAULT_LINGER).unwrap();
        assert_eq!(
            first.record().pid,
            second.record().pid,
            "a second dispatch started its own server instead of reusing the pooled one"
        );
        // And it keeps the address it is actually listening on, not the one
        // this acquire offered.
        assert_eq!(second.record().address, *plan_at(7777).address());

        let pid = first.record().pid;
        drop(first);
        drop(second);
        sweep(&root, Duration::from_secs(0)).unwrap();
        wait_until_dead(pid);
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_server_that_died_is_replaced_on_the_next_acquire() {
        let root = temp_root("dead");
        let first = acquire(&root, &key(), &sleeper_plan(), DEFAULT_LINGER).unwrap();
        let dead_pid = first.record().pid;
        // SAFETY: `kill` takes plain integers; the server is this test's own.
        unsafe {
            libc::kill(dead_pid.get() as libc::pid_t, libc::SIGKILL);
        }
        wait_until_dead(dead_pid);

        let second = acquire(&root, &key(), &sleeper_plan(), DEFAULT_LINGER).unwrap();
        assert_ne!(second.record().pid, dead_pid);
        assert!(pid_alive(second.record().pid));
        let pid = second.record().pid;
        drop(second);
        sweep(&root, Duration::from_secs(0)).unwrap();
        wait_until_dead(pid);
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    fn wait_until_dead(pid: Pid) {
        // A terminated child may briefly remain a zombie; give the reap a moment.
        for _ in 0..100 {
            if !pid_alive(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Zombies answer signal 0; the process is no longer running either way,
        // which is what the reclamation contract is about.
    }
}
