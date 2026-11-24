//! What one `onepipeline` process spent its wall clock and CPU on, span by span.
//!
//! Opt-in, under [`PROFILE_SPANS_ENV`], which names a directory: the process
//! writes one JSON [`Record`] there as it ends, under a name of its own pid and
//! start time so concurrent processes never collide. Unset or empty, nothing is
//! timed, sampled or written — the variable is read once per process, before
//! `main`, and every span below is then a check of a thread-local flag.
//!
//! **One thread's time.** Spans are timed on the thread that runs the verb, which
//! is the thread [`run`](crate::run) is called on; a span opened on any other
//! thread is not recorded, so the spans' own (self) times are exclusive by
//! construction and sum to no more than the process's wall clock. What they leave
//! is the process's work outside every span.
//!
//! **Waits served elsewhere.** The one exception is [`waiting_elsewhere`]: a wait a
//! thread the verb started sits out on the verb's behalf — a cancelled dispatch's
//! grace, a write-back's retry interval — recorded under its `wait:*` name from
//! whichever thread waited. None of it is the verb thread's own time, so its
//! `self_wall_ms` and `self_cpu_ms` are zero and its `wall_ms` is every such wait's
//! wall summed, which may overlap the verb thread's spans and each other.
//!
//! **Nesting.** A span's `wall_ms` is its inclusive time, counted once per
//! outermost frame of its name; its `self_wall_ms` and `self_cpu_ms` are what it
//! spent outside the spans nested in it, which are counted where they happened.
//! CPU is the verb thread's own (`CLOCK_THREAD_CPUTIME_ID`); a child process's
//! CPU is never in it, and is the record's `children_*_cpu_ms` instead, which is
//! what `getrusage(RUSAGE_CHILDREN)` says of the children this process waited for.
//!
//! The span names are this binary's choice and a reader treats them as opaque:
//! `startup` (process start, argument parsing and the preamble before the verb's
//! dispatch), `dispatch` (the verb itself), `configuration:*` (resolving the
//! runs root, the run a verb names, and the store's configuration), `journal:read`
//! and `ledger:read` (reading a run's journal and its other ledger files),
//! `<library>:<operation>` for a call into a linked library — `onetaskgraph:tasks`,
//! `onevcs:session` — and `child:<program>` for a child process this binary
//! starts and waits for.
//!
//! `ONEPIPELINE_HOST_PHASES` ([`crate::hostphases`]) is a separate line for a
//! separate reader, and stays exactly as it is whether or not this is on.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;

/// The environment variable asking a process to record its spans: the directory
/// its record is written into.
pub(crate) const PROFILE_SPANS_ENV: &str = "ONEPIPELINE_PROFILE_SPANS";

/// The version of [`Record`]'s shape, written into every record as `schema`.
/// `tests/golden/span-profile-record-v1.json` holds the shape, and both the
/// binary's journey and the report's tests read it.
pub(crate) const SCHEMA: u32 = 1;

/// This process, as the profile saw it start: where its record goes, and when.
struct Process {
    dir: PathBuf,
    began: Instant,
    began_at: SystemTime,
    began_cpu: Option<Duration>,
}

/// The one read of [`PROFILE_SPANS_ENV`], and the process's start when it is set.
fn process() -> Option<&'static Process> {
    static PROCESS: OnceLock<Option<Process>> = OnceLock::new();
    PROCESS
        .get_or_init(|| {
            let dir = std::env::var_os(PROFILE_SPANS_ENV).filter(|value| !value.is_empty())?;
            Some(Process {
                dir: PathBuf::from(dir),
                began: Instant::now(),
                began_at: SystemTime::now(),
                began_cpu: thread_cpu(),
            })
        })
        .as_ref()
}

/// Read the variable before `main`, so `startup` starts with the process rather
/// than with [`run`](crate::run): argument parsing happens in between. Each
/// platform's loader runs what its own section lists before `main` —
/// `.init_array` on Linux, `__mod_init_func` on macOS, the C runtime's `.CRT$XCU`
/// on Windows — and where none did, the first span reads it instead.
#[used]
#[cfg_attr(target_os = "linux", link_section = ".init_array")]
#[cfg_attr(target_vendor = "apple", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(windows, link_section = ".CRT$XCU")]
static EARLY: extern "C" fn() = early;

extern "C" fn early() {
    let _ = process();
}

/// The calling thread's CPU time so far, where the platform keeps one.
#[cfg(unix)]
fn thread_cpu() -> Option<Duration> {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `clock_gettime` writes one `timespec` through the pointer it is
    // handed, which points at a live, writable one.
    let read = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut now) };
    let secs = u64::try_from(now.tv_sec).ok()?;
    let nanos = u32::try_from(now.tv_nsec).ok()?;
    (read == 0).then(|| Duration::new(secs, nanos))
}

#[cfg(not(unix))]
fn thread_cpu() -> Option<Duration> {
    None
}

struct Frame {
    name: Cow<'static, str>,
    began: Instant,
    began_cpu: Option<Duration>,
    nested_wall: Duration,
    nested_cpu: Duration,
    outermost: bool,
}

#[derive(Default)]
struct Totals {
    count: u64,
    self_wall: Duration,
    wall: Duration,
    self_cpu: Option<Duration>,
}

thread_local! {
    /// Whether this thread is the one whose spans are recorded.
    static RECORDING: Cell<bool> = const { Cell::new(false) };
    static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
    static TOTALS: RefCell<BTreeMap<Cow<'static, str>, Totals>> =
        const { RefCell::new(BTreeMap::new()) };
}

/// One span, timed until it is dropped; nothing at all when this thread records
/// none. Frames close in the reverse of the order they opened, so a span is never
/// moved off the thread it opened on.
#[must_use = "a span measures until it is dropped"]
pub(crate) struct Span {
    open: bool,
    _not_send: std::marker::PhantomData<*const ()>,
}

fn push(name: Cow<'static, str>, began: Instant, began_cpu: Option<Duration>) -> Span {
    FRAMES.with_borrow_mut(|frames| {
        let outermost = frames.iter().all(|frame| frame.name != name);
        frames.push(Frame {
            name,
            began,
            began_cpu,
            nested_wall: Duration::ZERO,
            nested_cpu: Duration::ZERO,
            outermost,
        });
    });
    Span {
        open: true,
        _not_send: std::marker::PhantomData,
    }
}

const CLOSED: Span = Span {
    open: false,
    _not_send: std::marker::PhantomData,
};

fn recording() -> bool {
    RECORDING.with(Cell::get)
}

/// Time `name` for as long as the returned span lives.
pub(crate) fn span(name: &'static str) -> Span {
    if !recording() {
        return CLOSED;
    }
    push(Cow::Borrowed(name), Instant::now(), thread_cpu())
}

/// Run `work` as one span of `name`.
pub(crate) fn within<T>(name: &'static str, work: impl FnOnce() -> T) -> T {
    let _span = span(name);
    work()
}

/// Every wait [`waiting_elsewhere`] timed, by name: how many and their wall summed.
static ELSEWHERE: Mutex<BTreeMap<&'static str, (u64, Duration)>> = Mutex::new(BTreeMap::new());

/// One wait served on a thread other than the verb's, timed until it is dropped;
/// nothing at all when this process records no spans.
#[must_use = "a wait measures until it is dropped"]
pub(crate) struct Elsewhere {
    name: &'static str,
    began: Option<Instant>,
}

/// Time `name`, a wait sat out on whichever thread calls this, for as long as the
/// returned guard lives. Unprofiled, it is a read of the one-time answer to
/// whether this process records.
pub(crate) fn waiting_elsewhere(name: &'static str) -> Elsewhere {
    Elsewhere {
        name,
        began: process().map(|_| Instant::now()),
    }
}

impl Drop for Elsewhere {
    fn drop(&mut self) {
        let Some(began) = self.began else { return };
        let waited = began.elapsed();
        if let Ok(mut waits) = ELSEWHERE.lock() {
            let entry = waits.entry(self.name).or_default();
            entry.0 += 1;
            entry.1 += waited;
        }
    }
}

/// The span of one read of a run's ledger: `journal:read` for its journal,
/// `events.jsonl`, and `ledger:read` for every other file of it.
pub(crate) fn reading(path: &std::path::Path) -> Span {
    if !recording() {
        return CLOSED;
    }
    let journal = path.file_name().and_then(|name| name.to_str()) == Some("events.jsonl");
    span(if journal {
        "journal:read"
    } else {
        "ledger:read"
    })
}

/// The span of one call into a linked library: `<library>:<operation>`, such as
/// `onetaskgraph:tasks`. The name is only built where it is recorded.
pub(crate) fn library(library: &str, operation: &str) -> Span {
    if !recording() {
        return CLOSED;
    }
    push(
        Cow::Owned(format!("{library}:{operation}")),
        Instant::now(),
        thread_cpu(),
    )
}

/// The span of a child process this binary starts and waits for: `child:` and the
/// program's file stem, so `git` and `C:\...\git.exe` are one program.
pub(crate) fn child(program: &std::ffi::OsStr) -> Span {
    if !recording() {
        return CLOSED;
    }
    let stem = std::path::Path::new(program)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    push(
        Cow::Owned(format!("child:{stem}")),
        Instant::now(),
        thread_cpu(),
    )
}

impl Drop for Span {
    fn drop(&mut self) {
        if !self.open {
            return;
        }
        let now = Instant::now();
        let cpu = thread_cpu();
        let Some(frame) = FRAMES.with_borrow_mut(|frames| {
            let frame = frames.pop()?;
            let wall = now.saturating_duration_since(frame.began);
            let spent = cpu
                .zip(frame.began_cpu)
                .map(|(now, began)| now.saturating_sub(began));
            if let Some(parent) = frames.last_mut() {
                parent.nested_wall += wall;
                parent.nested_cpu += spent.unwrap_or_default();
            }
            Some(frame)
        }) else {
            return;
        };
        let wall = now.saturating_duration_since(frame.began);
        let self_cpu = cpu
            .zip(frame.began_cpu)
            .map(|(now, began)| now.saturating_sub(began).saturating_sub(frame.nested_cpu));
        TOTALS.with_borrow_mut(|totals| {
            let entry = totals.entry(frame.name).or_default();
            entry.count += 1;
            entry.self_wall += wall.saturating_sub(frame.nested_wall);
            if frame.outermost {
                entry.wall += wall;
            }
            // `null` once any entry of the name had no CPU clock to read.
            entry.self_cpu = if entry.count == 1 {
                self_cpu
            } else {
                entry.self_cpu.zip(self_cpu).map(|(held, more)| held + more)
            };
        });
    }
}

/// Start recording on the calling thread, if this process was asked to, and open
/// `startup` back at the process's own start. Closing the returned span is the
/// verb's dispatch beginning.
pub(crate) fn begin() -> Span {
    // Named so the object holding the constructor is linked into every program that
    // records, whatever the linker would otherwise drop as unreferenced.
    std::hint::black_box(&EARLY);
    let Some(process) = process() else {
        return CLOSED;
    };
    RECORDING.with(|recording| recording.set(true));
    push(Cow::Borrowed("startup"), process.began, process.began_cpu)
}

/// One span as a record carries it.
#[derive(Debug, Serialize)]
pub(crate) struct SpanRecord {
    /// The span's name: opaque to a reader.
    name: String,
    /// How many times it was entered.
    count: u64,
    /// Its time outside the spans nested in it.
    self_wall_ms: f64,
    /// Its time with what nested in it, counted once per outermost frame.
    wall_ms: f64,
    /// The verb thread's CPU in it, outside the spans nested in it; `null` where
    /// the platform keeps no per-thread CPU clock.
    self_cpu_ms: Option<f64>,
}

/// What one process writes as it ends: the shape `tests/golden/
/// span-profile-record-v1.json` holds.
#[derive(Debug, Serialize)]
pub(crate) struct Record {
    /// [`SCHEMA`].
    schema: u32,
    /// The subcommand path, words joined by one space: `status`, `channel next`.
    verb: String,
    /// This process's id.
    pid: u32,
    /// When it started, in seconds since the Unix epoch.
    started_at: f64,
    /// Its wall clock, start to the record.
    wall_ms: f64,
    /// `getrusage(RUSAGE_SELF)`'s user and system CPU, every thread's; `null`
    /// where the platform has no `getrusage`.
    user_cpu_ms: Option<f64>,
    /// See [`Record::user_cpu_ms`].
    system_cpu_ms: Option<f64>,
    /// `getrusage(RUSAGE_CHILDREN)`'s: the CPU of the children it waited for, and
    /// theirs in turn, which is never in the process's own.
    children_user_cpu_ms: Option<f64>,
    /// See [`Record::children_user_cpu_ms`].
    children_system_cpu_ms: Option<f64>,
    /// Its peak resident set, in KiB.
    peak_rss_kb: Option<u64>,
    /// The status the process exits with.
    exit_status: i32,
    /// Every span it entered, by name.
    spans: Vec<SpanRecord>,
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

/// User CPU, system CPU and peak RSS in KiB, as `getrusage` reports them.
type Usage = (Duration, Duration, u64);

/// `getrusage`'s user and system CPU and peak RSS for `who`.
#[cfg(unix)]
fn rusage(who: libc::c_int) -> Option<Usage> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: `getrusage` fills the one `rusage` the pointer names, which lives for
    // the call; it is read only when the call says it filled it.
    let usage = unsafe {
        if libc::getrusage(who, usage.as_mut_ptr()) != 0 {
            return None;
        }
        usage.assume_init()
    };
    let time = |value: libc::timeval| {
        Duration::new(
            u64::try_from(value.tv_sec).unwrap_or(0),
            u32::try_from(value.tv_usec).unwrap_or(0) * 1000,
        )
    };
    // Linux counts `ru_maxrss` in KiB and macOS in bytes.
    let rss = u64::try_from(usage.ru_maxrss).unwrap_or(0);
    let rss_kb = if cfg!(target_vendor = "apple") {
        rss / 1024
    } else {
        rss
    };
    Some((time(usage.ru_utime), time(usage.ru_stime), rss_kb))
}

/// The subcommand path this process was started with, asked of the parser
/// again: only a recording process pays for it, after its figures are taken.
fn verb() -> String {
    use clap::CommandFactory as _;
    let Ok(matches) = crate::cli::Cli::command().try_get_matches_from(std::env::args_os()) else {
        return String::new();
    };
    let mut words = Vec::new();
    let mut at = &matches;
    while let Some((name, nested)) = at.subcommand() {
        words.push(name.to_owned());
        at = nested;
    }
    words.join(" ")
}

/// Close every span still open and write the record, if one was asked for.
/// Best-effort: a record that cannot be written costs the measurement and is said
/// on stderr, never the verb's own answer.
pub(crate) fn finish(exit_status: i32) {
    let Some(process) = process().filter(|_| recording()) else {
        return;
    };
    while FRAMES.with_borrow(|frames| !frames.is_empty()) {
        drop(Span {
            open: true,
            _not_send: std::marker::PhantomData,
        });
    }
    let wall = process.began.elapsed();
    #[cfg(unix)]
    let (own, children) = (rusage(libc::RUSAGE_SELF), rusage(libc::RUSAGE_CHILDREN));
    #[cfg(not(unix))]
    let (own, children): (Option<Usage>, Option<Usage>) = (None, None);
    let mut spans: Vec<SpanRecord> = TOTALS.with_borrow(|totals| {
        totals
            .iter()
            .map(|(name, totals)| SpanRecord {
                name: name.to_string(),
                count: totals.count,
                self_wall_ms: ms(totals.self_wall),
                wall_ms: ms(totals.wall),
                self_cpu_ms: totals.self_cpu.map(ms),
            })
            .collect()
    });
    spans.extend(elsewhere());
    RECORDING.with(|recording| recording.set(false));
    let started_at = process
        .began_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let record = Record {
        schema: SCHEMA,
        verb: verb(),
        pid: std::process::id(),
        started_at: started_at.as_secs_f64(),
        wall_ms: ms(wall),
        user_cpu_ms: own.map(|(user, _, _)| ms(user)),
        system_cpu_ms: own.map(|(_, system, _)| ms(system)),
        children_user_cpu_ms: children.map(|(user, _, _)| ms(user)),
        children_system_cpu_ms: children.map(|(_, system, _)| ms(system)),
        peak_rss_kb: own.map(|(_, _, rss)| rss),
        exit_status,
        spans,
    };
    write(&process.dir, &record, started_at);
}

/// The waits [`waiting_elsewhere`] timed, as spans none of whose time is the verb
/// thread's own: a wait still being sat out as the process ends is not among them.
fn elsewhere() -> Vec<SpanRecord> {
    let Ok(waits) = ELSEWHERE.lock() else {
        return Vec::new();
    };
    waits
        .iter()
        .map(|(name, (count, wall))| SpanRecord {
            name: (*name).to_owned(),
            count: *count,
            self_wall_ms: 0.0,
            wall_ms: ms(*wall),
            self_cpu_ms: thread_cpu().map(|_| 0.0),
        })
        .collect()
}

/// Through a dot-named staging file and a rename, so a reader folding the
/// directory never reads half a record and passes over one being written.
fn write(dir: &std::path::Path, record: &Record, started_at: Duration) {
    let name = format!("onepipeline-{}-{}.json", record.pid, started_at.as_nanos());
    let file = dir.join(&name);
    let staged = dir.join(format!(".{name}"));
    let written = serde_json::to_vec(record)
        .map_err(std::io::Error::other)
        .and_then(|bytes| {
            std::fs::create_dir_all(dir)?;
            std::fs::write(&staged, bytes)?;
            std::fs::rename(&staged, &file)
        });
    if let Err(error) = written {
        eprintln!(
            "onepipeline: the span profile was not written to {}: {error}",
            file.display()
        );
    }
}

/// [`std::process::Command::output`] as one [`child`] span.
pub(crate) trait Timed {
    /// [`std::process::Command::output`], timed.
    fn timed_output(&mut self) -> std::io::Result<std::process::Output>;
}

impl Timed for std::process::Command {
    fn timed_output(&mut self) -> std::io::Result<std::process::Output> {
        let _span = child(self.get_program());
        let output = self.output();
        // SPIKE (spike-git-call-sites, never landed): how an engine git ended.
        crate::spike_git::finished(self, &output);
        output
    }
}

// The frame arithmetic is held here rather than by a journey because no verb arranges
// either case on demand: none nests a span name inside itself, and the threads a verb
// starts run in an order nothing outside the process controls. `tests/e2e/span_profile.rs`
// drives the records real verbs write.
#[cfg(test)]
mod tests {
    use super::*;

    fn recorded() -> BTreeMap<String, (u64, Duration, Duration)> {
        TOTALS.with_borrow(|totals| {
            totals
                .iter()
                .map(|(name, totals)| {
                    (
                        name.to_string(),
                        (totals.count, totals.self_wall, totals.wall),
                    )
                })
                .collect()
        })
    }

    #[test]
    fn a_span_inside_one_of_its_own_name_is_counted_twice_and_timed_once() {
        RECORDING.with(|recording| recording.set(true));
        {
            let _outer = span("ledger:read");
            std::thread::sleep(Duration::from_millis(2));
            let _inner = span("ledger:read");
            std::thread::sleep(Duration::from_millis(2));
        }
        let (count, own, whole) = recorded()["ledger:read"];
        assert_eq!(count, 2);
        // The outer frame's own time and the inner's together are the outer's whole,
        // so the inclusive figure is that whole once rather than with the inner again.
        assert_eq!(own, whole);
    }

    #[test]
    fn a_span_on_a_thread_that_is_not_recording_is_not_recorded() {
        RECORDING.with(|recording| recording.set(true));
        std::thread::spawn(|| {
            let _elsewhere = span("onevcs:elsewhere");
            let _child = child(std::ffi::OsStr::new("git"));
        })
        .join()
        .expect("the thread ends");
        assert!(recorded().is_empty(), "{:?}", recorded());
    }
}
