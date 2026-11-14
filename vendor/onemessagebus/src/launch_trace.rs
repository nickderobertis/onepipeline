//! Measurement only (spike-launch-inventory, never lands): a record of every
//! process this process starts, and of every wait whose only purpose is to watch
//! one, written when `ONEPIPELINE_LAUNCH_TRACE` names a directory and inert
//! otherwise.
//!
//! Lives in the vendored `onevcs` so that the engine, which links it, and this
//! library write one shape through one writer; the other vendored libraries
//! (`oneharness-core`, `onejudge`, `onemessagebus`) carry a byte-identical copy,
//! each its own writer into the same per-test file. Each record is one JSON line
//! appended to `<dir>/<test>.jsonl`, where `<test>` is nextest's
//! `NEXTEST_TEST_NAME` with `::` spelled `__` (`_untested` outside nextest). One
//! `write` per line on a file opened for append, so the processes of one test
//! share the file without interleaving inside a line.
//!
//! The record shape, `v: 1`, which `scripts/launch-inventory.mjs` folds:
//!
//! - `proc`: this process — `pid`, `exe` (file stem), `verb` (its first argument
//!   not starting with `-`), `test`, `t` (unix ns when first traced).
//! - `start`: one process started — `pid` (the starter), `seq` (its order in
//!   this process's writer), `child` (its pid, or
//!   null when the spawn failed), `program` (file stem), `sub` (git's
//!   subcommand, past any `-c k=v`/`-C dir`/`--git-dir d` options), `argv`, `cwd`, `env`
//!   (the id of an `env` record of this pid: every `GIT_*` the child gets,
//!   inherited or set), `site`/`function`, `t`
//!   (unix ns before the spawn), `spawn_ns` (spawn to spawn-return, null when the
//!   site's `std` call does not return between the two), `exit_ns` (spawn to the
//!   exit being collected, null when the site never collects it), `status`,
//!   `trace_ns` (this record's own cost before its write) and `write_ns_before`
//!   (this process's total write cost before this record).
//! - `env`: one `GIT_*` set a start of this process gave its child — `pid`, `id`,
//!   `env`.
//! - `end`: this process reaching the C runtime's exit — `pid`, `t`.
//! - `exit`: the exit of a start written at its spawn — `pid`, `seq` (the
//!   start's), `child`, `t`, `exit_ns`, `status`, and any folded wait; or, with
//!   `site`/`function` and no `seq`, a child of a detached start collected
//!   elsewhere — `pid`, `child`, `t`, `status`.
//! - `sleeps`: at this process's exit, per site of [`sleep`] — `site`/`function`,
//!   `count`, `slept_ns`, `interval_ns` (the last duration asked).
//!   A start whose wait was folded into it ([`Wait::into_launch`]) also carries
//!   `wait_site`, `wait_ns`, `polls` and `interval_ns`.
//! - `wait`: one wait on a child — `site`/`function`, `child`, `interval_ns`
//!   (the poll interval, null for a blocking wait), `polls`, `t`, `wait_ns`.

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The switch: a directory to write records under.
pub const ENV: &str = "ONEPIPELINE_LAUNCH_TRACE";

/// Where in the source a process is started or watched.
#[derive(Clone, Copy, Debug)]
pub struct Site {
    /// `file!()` at the site.
    pub file: &'static str,
    /// `line!()` at the site.
    pub line: u32,
    /// The enclosing function's path, closures dropped.
    pub function: &'static str,
}

/// The [`Site`] this macro is written at, with its enclosing function.
#[macro_export]
macro_rules! launch_site {
    () => {{
        fn here() {}
        fn name_of<T>(_: T) -> &'static str {
            ::std::any::type_name::<T>()
        }
        $crate::launch_trace::Site {
            file: file!(),
            line: line!(),
            function: $crate::launch_trace::function_of(name_of(here)),
        }
    }};
}

/// [`launch_site!`]'s helper: the enclosing function of its marker.
#[doc(hidden)]
pub fn function_of(here: &'static str) -> &'static str {
    let mut name = here.strip_suffix("::here").unwrap_or(here);
    while let Some(outer) = name.strip_suffix("::{{closure}}") {
        name = outer;
    }
    name
}

struct Tracer {
    file: Option<std::sync::Mutex<std::fs::File>>,
    pid: u32,
    verb: String,
    test: String,
    writes: AtomicU64,
    inherited_git: std::collections::BTreeMap<String, Option<String>>,
    envs: std::sync::Mutex<std::collections::HashMap<String, String>>,
    seq: AtomicU64,
}

fn tracer() -> Option<&'static Tracer> {
    static TRACER: OnceLock<Option<Tracer>> = OnceLock::new();
    TRACER
        .get_or_init(|| {
            let dir = std::env::var_os(ENV).filter(|dir| !dir.is_empty())?;
            let test = std::env::var("NEXTEST_TEST_NAME").unwrap_or_default();
            let name = if test.is_empty() {
                "_untested".to_owned()
            } else {
                test.replace("::", "__")
            };
            let dir = PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&dir);
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(format!("{name}.jsonl")))
                .ok()
                .map(std::sync::Mutex::new);
            let args: Vec<String> = std::env::args().collect();
            let exe = std::env::current_exe()
                .ok()
                .as_deref()
                .and_then(Path::file_stem)
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let verb = args
                .iter()
                .skip(1)
                .find(|arg| !arg.starts_with('-'))
                .cloned()
                .unwrap_or_default();
            let tracer = Tracer {
                file,
                pid: std::process::id(),
                verb,
                test,
                writes: AtomicU64::new(0),
                inherited_git: std::env::vars_os()
                    .filter_map(|(key, value)| {
                        let key = key.to_string_lossy().into_owned();
                        key.starts_with("GIT_")
                            .then(|| (key, Some(value.to_string_lossy().into_owned())))
                    })
                    .collect(),
                envs: std::sync::Mutex::default(),
                seq: AtomicU64::new(0),
            };
            tracer.write(serde_json::json!({
                "v": 1,
                "kind": "proc",
                "pid": tracer.pid,
                "exe": exe,
                "verb": tracer.verb,
                "argv": args.iter().skip(1).take(8).collect::<Vec<_>>(),
                "test": tracer.test,
                "t": unix_ns(SystemTime::now()),
            }));
            // This crate forbids `unsafe`, so this copy registers no exit hook:
            // the engine's own writer (the vendored onevcs) records the process's
            // end, and nothing here is tallied for it to flush.
            Some(tracer)
        })
        .as_ref()
}

type Sleeps = std::collections::BTreeMap<(&'static str, u32), (&'static str, u64, u64, u64)>;

fn sleeps() -> &'static std::sync::Mutex<Sleeps> {
    static SLEEPS: OnceLock<std::sync::Mutex<Sleeps>> = OnceLock::new();
    SLEEPS.get_or_init(Default::default)
}

/// A blocking wait on a child that took `took`, bounded by `interval`, counted
/// per `site` with the sleeps and written once at exit.
pub fn waited(site: Site, took: Duration, interval: Option<Duration>) {
    if tracer().is_none() {
        return;
    }
    if let Ok(mut sleeps) = sleeps().lock() {
        let entry = sleeps
            .entry((site.file, site.line))
            .or_insert((site.function, 0, 0, 0));
        entry.1 += 1;
        entry.2 += nanos(took);
        entry.3 = interval.map_or(0, nanos);
    }
}

/// [`std::thread::sleep`], counted per `site` and written once at exit.
pub fn sleep(site: Site, duration: Duration) {
    if tracer().is_none() {
        return std::thread::sleep(duration);
    }
    let began = Instant::now();
    std::thread::sleep(duration);
    let slept = nanos(began.elapsed());
    if let Ok(mut sleeps) = sleeps().lock() {
        let entry = sleeps
            .entry((site.file, site.line))
            .or_insert((site.function, 0, 0, 0));
        entry.1 += 1;
        entry.2 += slept;
        entry.3 = nanos(duration);
    }
}

/// A child started at a [`spawn`] whose [`Launch`] was [`Launch::detached`] was
/// collected at `site`, with its code when it has one.
pub fn exit(site: Site, child: u32, status: Option<i32>) {
    if let Some(tracer) = tracer() {
        tracer.write(serde_json::json!({
            "v": 1,
            "kind": "exit",
            "pid": tracer.pid,
            "child": child,
            "site": format!("{}:{}", site.file, site.line),
            "function": site.function,
            "t": unix_ns(SystemTime::now()),
            "status": status,
        }));
    }
}

impl Tracer {
    /// The id this process gave one `GIT_*` set, written as an `env` record the
    /// first time it is seen.
    ///
    /// Named by this crate, because each vendored library carries its own copy of
    /// this module and so its own writer into the same file.
    fn env_id(&self, env: &std::collections::BTreeMap<String, Option<String>>) -> String {
        let text = serde_json::to_string(env).unwrap_or_default();
        let Ok(mut envs) = self.envs.lock() else {
            return String::new();
        };
        if let Some(id) = envs.get(&text) {
            return id.clone();
        }
        let id = format!("{}:{}", env!("CARGO_PKG_NAME"), envs.len());
        envs.insert(text, id.clone());
        drop(envs);
        self.write(serde_json::json!({
            "v": 1,
            "kind": "env",
            "pid": self.pid,
            "id": id,
            "env": env,
        }));
        id
    }

    fn write(&self, mut record: serde_json::Value) {
        let Some(file) = &self.file else { return };
        record["write_ns_before"] = self.writes.load(Ordering::Relaxed).into();
        let mut line = record.to_string();
        line.push('\n');
        let began = Instant::now();
        if let Ok(mut file) = file.lock() {
            let _ = file.write_all(line.as_bytes());
        }
        self.writes
            .fetch_add(nanos(began.elapsed()), Ordering::Relaxed);
    }
}

fn unix_ns(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).map(nanos).unwrap_or(0)
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn stem(program: &OsStr) -> String {
    Path::new(program)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// git's subcommand: the first argument past its global options.
fn subcommand(args: &[String]) -> Option<String> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "-C" | "--git-dir" | "--work-tree" | "--namespace" => {
                args.next();
            }
            option if option.starts_with('-') => {}
            subcommand => return Some(subcommand.to_owned()),
        }
    }
    None
}

/// One process started. A start whose spawn returns ([`Launch::spawned`]) is
/// written then, and its exit as an `exit` record of the same `seq`, so a process
/// ended while it waits still leaves the start; a start through `output` or
/// `status` is written whole once it returns.
pub struct Launch {
    record: Option<(serde_json::Value, Instant, Duration)>,
    written: Option<serde_json::Value>,
}

/// Record the start of `command` at `site`; call this immediately before its
/// `spawn`, `output` or `status`.
pub fn launch(site: Site, command: &Command) -> Launch {
    let Some(tracer) = tracer() else {
        return Launch {
            record: None,
            written: None,
        };
    };
    let began = Instant::now();
    let t = unix_ns(SystemTime::now());
    let program = stem(command.get_program());
    let args: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let sub = (program == "git").then(|| subcommand(&args)).flatten();
    let mut env = tracer.inherited_git.clone();
    for (key, value) in command.get_envs() {
        let key = key.to_string_lossy().into_owned();
        if key.starts_with("GIT_") {
            env.insert(key, value.map(|value| value.to_string_lossy().into_owned()));
        }
    }
    let cwd = command
        .get_current_dir()
        .map(|dir| dir.to_string_lossy().into_owned())
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|dir| dir.to_string_lossy().into_owned())
        });
    let record = serde_json::json!({
        "v": 1,
        "kind": "start",
        "pid": tracer.pid,
        "seq": format!("{}:{}", env!("CARGO_PKG_NAME"), tracer.seq.fetch_add(1, Ordering::Relaxed)),
        "site": format!("{}:{}", site.file, site.line),
        "function": site.function,
        "program": program,
        "sub": sub,
        "argv": args,
        "cwd": cwd,
        "env": tracer.env_id(&env),
        "t": t,
        "child": null,
        "spawn_ns": null,
        "exit_ns": null,
        "status": null,
    });
    let cost = began.elapsed();
    Launch {
        record: Some((record, Instant::now(), cost)),
        written: None,
    }
}

impl Launch {
    /// The spawn returned, with the child's pid when it started: the start is
    /// written now.
    pub fn spawned(&mut self, child: Option<u32>) {
        let Some(tracer) = tracer() else { return };
        if let Some((record, began, cost)) = &mut self.record {
            let stamped = Instant::now();
            record["spawn_ns"] = nanos(began.elapsed()).into();
            record["child"] = child.into();
            record["trace_ns"] = nanos(*cost + stamped.elapsed()).into();
            tracer.write(record.clone());
            self.written = Some(serde_json::json!({
                "v": 1,
                "kind": "exit",
                "pid": tracer.pid,
                "seq": record["seq"].clone(),
                "child": child,
            }));
        }
    }

    /// The child's exit was collected, with its code when it has one.
    pub fn exited(mut self, status: Option<i32>) {
        let Some(elapsed) = self
            .record
            .as_ref()
            .map(|(_, began, _)| nanos(began.elapsed()))
        else {
            return;
        };
        if let Some(exit) = &mut self.written {
            exit["exit_ns"] = elapsed.into();
            exit["status"] = status.into();
            exit["t"] = unix_ns(SystemTime::now()).into();
            if let Some(tracer) = tracer() {
                tracer.write(exit.clone());
            }
            self.record = None;
        } else if let Some((record, _, _)) = &mut self.record {
            record["exit_ns"] = elapsed.into();
            record["status"] = status.into();
        }
    }

    /// Note that this start is not collected here: it was written at its spawn.
    pub fn detached(mut self) {
        if self.written.is_some() {
            self.record = None;
        }
    }
}

impl Drop for Launch {
    fn drop(&mut self) {
        let Some((mut record, _, cost)) = self.record.take() else {
            return;
        };
        if self.written.is_some() {
            return;
        }
        let Some(tracer) = tracer() else { return };
        let began = Instant::now();
        record["trace_ns"] = nanos(cost + began.elapsed()).into();
        tracer.write(record);
    }
}

/// [`Command::output`], recorded at `site`. Its spawn and its exit are one `std`
/// call, so `spawn_ns` is null and `child` unknown.
pub fn output(site: Site, command: &mut Command) -> std::io::Result<Output> {
    let launch = launch(site, command);
    let output = command.output();
    launch.exited(output.as_ref().ok().and_then(|output| output.status.code()));
    output
}

/// [`Command::status`], recorded at `site`, as [`output`] is.
pub fn status(site: Site, command: &mut Command) -> std::io::Result<ExitStatus> {
    let launch = launch(site, command);
    let status = command.status();
    launch.exited(status.as_ref().ok().and_then(ExitStatus::code));
    status
}

/// [`Command::spawn`], recorded at `site`, for a child this process never
/// collects where the record can follow it.
pub fn spawn_detached(site: Site, command: &mut Command) -> std::io::Result<Child> {
    let (child, launch) = spawn(site, command);
    launch.detached();
    child
}

/// [`Command::spawn`], recorded at `site`, returning the [`Launch`] whose
/// [`Launch::exited`] the caller calls where it collects the child.
pub fn spawn(site: Site, command: &mut Command) -> (std::io::Result<Child>, Launch) {
    let mut launch = launch(site, command);
    let child = command.spawn();
    launch.spawned(child.as_ref().ok().map(Child::id));
    (child, launch)
}

/// One wait whose only purpose is to watch a child, recorded when dropped.
pub struct Wait {
    record: Option<(serde_json::Value, Instant)>,
    polls: u64,
}

/// Start recording a wait at `site` on `child`, polling every `interval` (none
/// for a blocking wait).
pub fn wait(site: Site, child: Option<u32>, interval: Option<Duration>) -> Wait {
    let Some(tracer) = tracer() else {
        return Wait {
            record: None,
            polls: 0,
        };
    };
    Wait {
        record: Some((
            serde_json::json!({
                "v": 1,
                "kind": "wait",
                "pid": tracer.pid,
                "site": format!("{}:{}", site.file, site.line),
                "function": site.function,
                "child": child,
                "interval_ns": interval.map(nanos),
                "t": unix_ns(SystemTime::now()),
            }),
            Instant::now(),
        )),
        polls: 0,
    }
}

impl Wait {
    /// One more poll of the child.
    pub fn poll(&mut self) {
        self.polls += 1;
    }

    /// Write this wait into the start record of the child it watched, as
    /// `wait_site`, `wait_ns`, `polls` and `interval_ns`, rather than as a record of
    /// its own.
    pub fn into_launch(mut self, launch: &mut Launch) {
        let target = match (&mut launch.written, launch.record.as_mut()) {
            (Some(exit), _) => Some(exit),
            (None, Some((record, _, _))) => Some(record),
            (None, None) => None,
        };
        if let (Some((wait, began)), Some(record)) = (self.record.take(), target) {
            record["wait_site"] = wait["site"].clone();
            record["wait_ns"] = nanos(began.elapsed()).into();
            record["polls"] = self.polls.into();
            record["interval_ns"] = wait["interval_ns"].clone();
        }
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        let Some((mut record, began)) = self.record.take() else {
            return;
        };
        let Some(tracer) = tracer() else { return };
        record["wait_ns"] = nanos(began.elapsed()).into();
        record["polls"] = self.polls.into();
        tracer.write(record);
    }
}

/// Whether the trace is on in this process.
pub fn is_enabled() -> bool {
    tracer().is_some()
}
