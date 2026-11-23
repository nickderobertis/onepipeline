//! Where each test's wall clock goes.
//!
//! Every test process records what it spent its time on, periodically replaces
//! a provisional JSON record, and finalizes it when the test ends. Each run
//! leaves a per-test breakdown, including any unfinished test's last heartbeat.
//! `scripts/test-profile-report.mjs` folds the records into the per-test,
//! per-module and per-platform tables.
//!
//! **Per process, because a test is one.** nextest runs every test in a process
//! of its own and names it in `NEXTEST_TEST_NAME`; a test binary run by hand with
//! `--exact <name>` is the same shape. Under any other shape — several tests in
//! one process — nothing is written, because no single test owns the totals.
//!
//! **Six categories, and each moment of the test thread is in at most one.** A
//! process this one starts is `git` when the program is git, `build` when it is
//! cargo, and `spawn` otherwise; `fixture` is the harness building a test's
//! world; `wait` is sleeping and polling; `fs` is file and directory work. They
//! nest — a fixture runs git and writes files, a poll reads files — and the
//! moment is the innermost one's: a fixture's own time is what it spent outside
//! the git, process and file work inside it, which are counted where they
//! happened. So the categories' times sum to no more than the test's wall clock,
//! and what is left is `unattributed_s`: the test's own computation and anything
//! it did directly through `std` rather than through this module.
//!
//! Each category also carries `inclusive_s`, the time spent inside its
//! outermost frames with what nested in them, so a fixture's whole cost reads
//! off as one figure; those overlap across categories by construction and are
//! never summed.
//!
//! **The test thread's time, and every thread's counts.** Only the thread
//! nextest runs the test on is the test's own clock; work on a thread the test
//! started overlaps it, so it is counted, and its time is kept apart as
//! `background_s` rather than added to the sum.
//!
//! The replacements below — [`process::Command`], [`fs`], [`thread::sleep`] —
//! are drop-in for the `std` items of the same names, and that is the whole of
//! how a site is recorded: a journey written against them asserts exactly what it
//! would against `std`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// The environment variable that replaces the directory records are written
/// under, which is otherwise the one the test binary's harness names at
/// [`start`].
pub const DIR_ENV: &str = "ONEPIPELINE_TEST_PROFILE_DIR";

/// Where, under the workspace root, the e2e harness has records written: one
/// directory per test binary beneath it. CI keeps this directory from every leg
/// that runs the tests, which `tests/ci_workflows.rs` holds it to.
pub const RECORD_DIR: &str = "target/test-profile";

/// The switch that has every `onepipeline` process a test starts write its span
/// record — `ONEPIPELINE_PROFILE_SPANS`, the binary's own — into that test's own
/// record directory, `<binary>/<test>.spans/` beside its record, so each span
/// record is the test's whose spawn produced it. Unset or empty, which is every
/// tier of the gate, no process is asked to.
pub const SPANS_ENV: &str = "ONEPIPELINE_TEST_PROFILE_SPANS";

/// The binary's own variable, which [`SPANS_ENV`] sets on each `onepipeline` a
/// test starts — unless the test set or removed it itself.
const BINARY_SPANS_ENV: &str = "ONEPIPELINE_PROFILE_SPANS";

/// The version of the record's shape, written into every record as `schema`.
pub const SCHEMA: u32 = 3;

/// What a moment of a test's wall clock was spent on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// A process this one started, other than git and cargo.
    Spawn,
    /// A git command, as a process or through a linked library that runs one.
    Git,
    /// A cargo build of a binary the test runs.
    Build,
    /// The harness building the world a test runs in.
    Fixture,
    /// Sleeping, and polling for a condition.
    Wait,
    /// File and directory work.
    Fs,
}

impl Category {
    /// Every category, in the order a record lists them.
    pub const ALL: [Self; 6] = [
        Self::Spawn,
        Self::Git,
        Self::Build,
        Self::Fixture,
        Self::Wait,
        Self::Fs,
    ];

    /// The key a record writes this category under.
    pub fn name(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Git => "git",
            Self::Build => "build",
            Self::Fixture => "fixture",
            Self::Wait => "wait",
            Self::Fs => "fs",
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    /// The category a process running `program` is recorded under.
    fn of_program(program: &str) -> Self {
        match program {
            "git" => Self::Git,
            "cargo" => Self::Build,
            _ => Self::Spawn,
        }
    }
}

/// One category's totals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// How many times it happened on the test thread. For [`Category::Wait`] this
    /// is sleeps; polls are [`Snapshot::polls`].
    pub count: u64,
    /// The test thread's time in it and nothing nested inside it.
    pub time: Duration,
    /// The test thread's time inside its outermost frames, nesting included.
    pub inclusive: Duration,
    /// How many times it happened on threads the test started.
    pub background_count: u64,
    /// The time those threads spent in it, which overlaps the test thread's.
    pub background: Duration,
}

/// What has been recorded so far in this process.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Each category's totals, in [`Category::ALL`]'s order.
    pub tallies: [Tally; 6],
    /// How many polling waits the test thread ran.
    pub polls: u64,
    /// Per program a process ran, how many the test thread started, how long it
    /// was held by them, and the CPU (user and system) of the children reaped
    /// while it was — `None` where the platform has no `getrusage`.
    pub programs: BTreeMap<String, (u64, Duration, Option<Duration>)>,
}

impl Snapshot {
    /// One category's totals.
    pub fn of(&self, category: Category) -> Tally {
        self.tallies[category.index()]
    }
}

struct Recorder {
    started: Instant,
    started_at: SystemTime,
    /// The host's 1-minute load average when the clock started.
    load_at_start: Option<f64>,
    /// Whether this test's span directory has been emptied of an earlier attempt's.
    spans_cleared: bool,
    /// The test binary, and the directory its records go under, from [`start`].
    binary: Option<(&'static str, PathBuf)>,
    written: bool,
    snapshot: Snapshot,
}

static RECORDER: Mutex<Option<Recorder>> = Mutex::new(None);

fn with_recorder<T>(f: impl FnOnce(&mut Recorder) -> T) -> T {
    let mut held = RECORDER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let recorder = held.get_or_insert_with(|| Recorder {
        started: Instant::now(),
        started_at: SystemTime::now(),
        load_at_start: load_1m(),
        spans_cleared: false,
        binary: None,
        written: false,
        snapshot: Snapshot::default(),
    });
    f(recorder)
}

/// Start this process's clock and say where its record goes.
///
/// Called by the harness before `main`, so the wall clock is the process's, and
/// so a test that never touches the harness still leaves a record: the write is
/// registered with the C runtime's exit as well as with the end of the test
/// thread, and whichever comes first writes it.
pub fn start(binary: &'static str, dir: PathBuf) {
    with_recorder(|recorder| recorder.binary = Some((binary, dir)));
    // SAFETY: `at_exit` is an `extern "C"` function that takes nothing, which is
    // the whole of what the C runtime asks of a handler it calls at exit.
    unsafe { atexit(at_exit) };
    // A test the runner ends — nextest's `terminate-after`, a cancelled leg — runs
    // neither the end of its thread nor the C runtime's exit. So until the final
    // record is written, a provisional one marked `unfinished` is kept current,
    // and the test leaves what it had spent when it was ended.
    let _ = std::thread::Builder::new()
        .name("test-profile-heartbeat".to_owned())
        .spawn(|| loop {
            write(Ended::Unfinished);
            std::thread::sleep(HEARTBEAT);
        });
}

/// How often the provisional record of a test still running is rewritten.
///
/// A heartbeat sees what has closed: time inside a frame still open when the
/// test was ended — the process it was waiting on when nextest ended it — is in
/// the record's wall clock and its `unattributed_s`, not in a category.
pub const HEARTBEAT: Duration = Duration::from_secs(5);

extern "C" {
    fn atexit(handler: extern "C" fn()) -> std::ffi::c_int;
}

extern "C" fn at_exit() {
    write(Ended::ProcessExit);
}

/// Everything recorded so far: for a test asserting what its own work recorded.
pub fn snapshot() -> Snapshot {
    with_recorder(|recorder| recorder.snapshot.clone())
}

/// The test this process runs, when it runs exactly one.
fn test_name() -> Option<String> {
    if let Some(name) = std::env::var_os("NEXTEST_TEST_NAME") {
        return name.into_string().ok();
    }
    // A test binary run by hand: `--exact` and one name. Every argument that is not
    // a flag counts as a name, so one given a flag's value (`--test-threads 1`)
    // reads as two and writes nothing, rather than this keeping a copy of which of
    // libtest's flags take one.
    let exact = std::env::args().any(|arg| arg == "--exact");
    let names: Vec<String> = std::env::args()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    match (exact, names.as_slice()) {
        (true, [name]) => Some(name.clone()),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Unknown,
    Test,
    Background,
}

/// Writes the record when the test thread ends — its last act, after every
/// fixture the test held has been dropped.
struct TestThreadEnds;

impl Drop for TestThreadEnds {
    fn drop(&mut self) {
        write(Ended::TestThread);
    }
}

thread_local! {
    static ROLE: Cell<Role> = const { Cell::new(Role::Unknown) };
    static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
    static ENDS: RefCell<Option<TestThreadEnds>> = const { RefCell::new(None) };
}

fn role() -> Role {
    let known = ROLE.with(Cell::get);
    if known != Role::Unknown {
        return known;
    }
    let ours = test_name().is_some_and(|name| std::thread::current().name() == Some(&name));
    let role = if ours { Role::Test } else { Role::Background };
    ROLE.with(|cell| cell.set(role));
    if ours {
        ENDS.with(|ends| *ends.borrow_mut() = Some(TestThreadEnds));
    }
    role
}

/// What a frame adds to its category's count when it closes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Counts {
    /// One more, always: a process started, a sleep, a file operation.
    Leaf,
    /// One more unless a frame of its category encloses it: a fixture built out
    /// of other fixtures is one fixture.
    Span,
    /// One more poll, unless another poll encloses it.
    Poll,
    /// Nothing: a wait on a process already counted when it started.
    Nothing,
}

struct Frame {
    category: Category,
    counts: Counts,
    start: Instant,
    /// The reaped children's CPU when a frame running a program opened.
    children_cpu: Option<Duration>,
    nested: Duration,
    /// Whether no frame of the same category encloses this one.
    outermost: bool,
    program: Option<String>,
}

/// A recorded stretch of the calling thread, closed when it is dropped.
///
/// Held for the extent of the work it measures; frames close in the reverse of
/// the order they opened, so one is never moved off the stack it opened on.
#[must_use = "a span measures until it is dropped"]
pub struct Span {
    _not_send: std::marker::PhantomData<*const ()>,
}

fn open(category: Category, counts: Counts, program: Option<String>) -> Span {
    FRAMES.with(|frames| {
        let mut frames = frames.borrow_mut();
        let outermost = frames.iter().all(|frame| frame.category != category);
        let children_cpu = program.as_ref().and_then(|_| children_cpu());
        frames.push(Frame {
            category,
            counts,
            start: Instant::now(),
            children_cpu,
            nested: Duration::ZERO,
            outermost,
            program,
        });
    });
    Span {
        _not_send: std::marker::PhantomData,
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let now = Instant::now();
        let Some((frame, enclosed)) = FRAMES.with(|frames| {
            let mut frames = frames.borrow_mut();
            let frame = frames.pop()?;
            let elapsed = now.saturating_duration_since(frame.start);
            if let Some(parent) = frames.last_mut() {
                parent.nested += elapsed;
            }
            Some((frame, !frames.is_empty()))
        }) else {
            return;
        };
        let elapsed = now.saturating_duration_since(frame.start);
        let role = role();
        with_recorder(|recorder| {
            let snapshot = &mut recorder.snapshot;
            let tally = &mut snapshot.tallies[frame.category.index()];
            let counted = match frame.counts {
                Counts::Leaf => true,
                Counts::Span => frame.outermost,
                Counts::Poll | Counts::Nothing => false,
            };
            if role == Role::Test {
                tally.time += elapsed.saturating_sub(frame.nested);
                if frame.outermost {
                    tally.inclusive += elapsed;
                }
                if counted {
                    tally.count += 1;
                }
                if frame.counts == Counts::Poll && frame.outermost {
                    snapshot.polls += 1;
                }
                if let Some(program) = frame.program {
                    let cpu = children_cpu()
                        .zip(frame.children_cpu)
                        .map(|(now, then)| now.saturating_sub(then));
                    let first = !snapshot.programs.contains_key(&program);
                    let entry = snapshot.programs.entry(program).or_default();
                    entry.0 += u64::from(counted);
                    entry.1 += elapsed;
                    // `None` once any of the program's frames had no CPU to read.
                    entry.2 = if first {
                        cpu
                    } else {
                        entry.2.zip(cpu).map(|(held, more)| held + more)
                    };
                }
            } else {
                if counted {
                    tally.background_count += 1;
                }
                if !enclosed {
                    tally.background += elapsed;
                }
            }
        });
    }
}

/// Record `category` for as long as the returned span lives.
pub fn span(category: Category) -> Span {
    open(category, Counts::Span, None)
}

/// One operation of `category` that nothing else is recorded inside.
fn leaf<T>(category: Category, work: impl FnOnce() -> T) -> T {
    let _span = open(category, Counts::Leaf, None);
    work()
}

/// Run `work` as one polling wait: what the predicate does inside it is recorded
/// where it happens, and the rest — the sleeps between looks — is the wait's.
pub fn polled<T>(work: impl FnOnce() -> T) -> T {
    let _span = open(Category::Wait, Counts::Poll, None);
    work()
}

/// What wrote a record: the end of the test's own thread; the process's exit,
/// for a test that never reached anything recorded on it; or the heartbeat, for
/// a test still running — which is what a test the runner ended leaves behind.
#[derive(Clone, Copy)]
enum Ended {
    TestThread,
    ProcessExit,
    /// Provisional: the test was still running when this was written.
    Unfinished,
}

impl Ended {
    fn name(self) -> &'static str {
        match self {
            Self::TestThread => "test-thread",
            Self::ProcessExit => "process-exit",
            Self::Unfinished => "unfinished",
        }
    }
}

/// The file name a test's record is written under, or `None` for a name that
/// could reach outside the record directory. A test's path in its binary is
/// identifiers joined by `::`, so anything else is not one.
fn record_file(name: &str) -> Option<String> {
    let identifiers = name.split("::");
    let valid = identifiers.clone().all(|identifier| {
        !identifier.is_empty()
            && identifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
    });
    valid.then(|| format!("{}.json", identifiers.collect::<Vec<_>>().join(".")))
}

/// Replace the provisional record until this process's record is finalized once.
fn write(ended: Ended) {
    let Some(name) = test_name() else {
        return;
    };
    let Some(file_name) = record_file(&name) else {
        eprintln!("no test profile is written for {name:?}: it is not a test's path");
        return;
    };
    // The file is written under the recorder's lock, so a provisional record
    // can never land after the final one it would replace.
    with_recorder(|recorder| {
        let Some((binary, dir)) = recorder.binary.clone() else {
            return;
        };
        if recorder.written {
            return;
        }
        if !matches!(ended, Ended::Unfinished) {
            recorder.written = true;
        }
        let record = record(recorder, binary, &name, ended);
        let dir = std::env::var_os(DIR_ENV)
            .map_or(dir, PathBuf::from)
            .join(binary);
        let file = dir.join(file_name);
        // Through a name of this process's own and a rename, so a reader never
        // sees half a record; and never a panic, because a record is evidence
        // about the run rather than part of it.
        let staged = dir.join(format!(
            ".{}.{}",
            std::process::id(),
            file.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
        let written = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&staged, format!("{record:#}\n")))
            .and_then(|()| std::fs::rename(&staged, &file));
        if let Err(error) = written {
            eprintln!(
                "the test profile for {name} was not written to {}: {error}",
                file.display()
            );
        }
    });
}

fn record(recorder: &Recorder, binary: &str, name: &str, ended: Ended) -> serde_json::Value {
    let wall = recorder.started.elapsed();
    let snapshot = &recorder.snapshot;
    let mut categories = serde_json::Map::new();
    let mut attributed = Duration::ZERO;
    for category in Category::ALL {
        let tally = snapshot.of(category);
        attributed += tally.time;
        let mut entry = serde_json::json!({
            "count": tally.count,
            "s": tally.time.as_secs_f64(),
            "inclusive_s": tally.inclusive.as_secs_f64(),
            "background_count": tally.background_count,
            "background_s": tally.background.as_secs_f64(),
        });
        if category == Category::Wait {
            entry["polls"] = snapshot.polls.into();
        }
        categories.insert(category.name().to_owned(), entry);
    }
    let programs: serde_json::Map<_, _> = snapshot
        .programs
        .iter()
        .map(|(program, (count, time, cpu))| {
            (
                program.clone(),
                serde_json::json!({
                    "count": count,
                    "s": time.as_secs_f64(),
                    "cpu_s": cpu.map(|cpu| cpu.as_secs_f64()),
                }),
            )
        })
        .collect();
    let env = |key: &str| std::env::var(key).ok();
    serde_json::json!({
        "schema": SCHEMA,
        "binary": binary,
        "test": name,
        "run_id": env("NEXTEST_RUN_ID"),
        "attempt": env("NEXTEST_ATTEMPT").and_then(|attempt| attempt.parse::<u64>().ok()),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "started_at": recorder
            .started_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64()),
        "wall_s": wall.as_secs_f64(),
        "ended": ended.name(),
        "categories": categories,
        "unattributed_s": wall.saturating_sub(attributed).as_secs_f64(),
        "programs": programs,
        "nproc": std::thread::available_parallelism().ok().map(std::num::NonZeroUsize::get),
        "load_1m_start": recorder.load_at_start,
        "load_1m_end": load_1m(),
    })
}

/// The CPU, user and system, of every child this process has reaped, and theirs
/// in turn: one `getrusage` read. A frame reads it as it opens and closes, so
/// what a program's frames add is the children reaped while they were open — on
/// the test thread, the program's own, unless a thread the test started reaped
/// one of its own at the same moment.
#[cfg(unix)]
fn children_cpu() -> Option<Duration> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: `getrusage` fills the one `rusage` the pointer names, which lives for
    // the call; it is read only when the call says it filled it.
    let usage = unsafe {
        if libc::getrusage(libc::RUSAGE_CHILDREN, usage.as_mut_ptr()) != 0 {
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
    Some(time(usage.ru_utime) + time(usage.ru_stime))
}

#[cfg(not(unix))]
fn children_cpu() -> Option<Duration> {
    None
}

/// The host's 1-minute load average, where the platform keeps one.
#[cfg(unix)]
fn load_1m() -> Option<f64> {
    let mut load = [0.0_f64; 1];
    // SAFETY: `getloadavg` writes at most the one sample it is asked for into the
    // one-element array the pointer names.
    let read = unsafe { libc::getloadavg(load.as_mut_ptr(), 1) };
    (read == 1).then_some(load[0])
}

#[cfg(not(unix))]
fn load_1m() -> Option<f64> {
    None
}

/// Where the `onepipeline` processes this test starts write their span records,
/// when [`SPANS_ENV`] asks for them: the test's own directory beside its record,
/// emptied of an earlier attempt's at the first spawn that asks.
fn spans_dir() -> Option<PathBuf> {
    std::env::var_os(SPANS_ENV).filter(|value| !value.is_empty())?;
    let name = test_name()?;
    let file = record_file(&name)?;
    with_recorder(|recorder| {
        let (binary, dir) = recorder.binary.clone()?;
        let spans = std::env::var_os(DIR_ENV)
            .map_or(dir, PathBuf::from)
            .join(binary)
            .join(format!("{}.spans", file.trim_end_matches(".json")));
        if !recorder.spans_cleared {
            recorder.spans_cleared = true;
            let _ = std::fs::remove_dir_all(&spans);
        }
        Some(spans)
    })
}

/// The name a program is recorded under: its file stem, so `git` and
/// `C:\...\git.exe` are one program.
fn program_name(program: &std::ffi::OsStr) -> String {
    std::path::Path::new(program)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// DEMONSTRATION ONLY: the variable naming one harness spawn, which the binary copies
/// into its span record, and the file each spawn's harness-side timing is appended to.
const DEMO_SPAWN_ID_ENV: &str = "ONEPIPELINE_DEMO_SPAWN_ID";
const DEMO_SPAWN_LOG_DIR_ENV: &str = "ONEPIPELINE_DEMO_SPAWN_LOG_DIR";

/// DEMONSTRATION ONLY: a fresh spawn id, unique across this test process.
fn demo_spawn_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{n}", std::process::id())
}

/// DEMONSTRATION ONLY: append one line of the harness's timing of spawn `id`.
fn demo_log(
    dir: &std::path::Path,
    id: &str,
    kind: &str,
    began: std::time::SystemTime,
    took: Duration,
) {
    use std::io::Write as _;
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let t0 = began
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let role = match role() {
        Role::Test => "test",
        _ => "background",
    };
    let line = format!(
        "{{\"id\":\"{id}\",\"kind\":\"{kind}\",\"role\":\"{role}\",\"t0\":{t0:.6},\"s\":{:.6}}}\n",
        took.as_secs_f64()
    );
    // Outside every span directory, which some journeys read file by file: the
    // directory the job names, one file per test's span directory.
    let Some(logs) = std::env::var_os(DEMO_SPAWN_LOG_DIR_ENV).map(std::path::PathBuf::from) else {
        return;
    };
    let Some(name) = dir.file_name() else { return };
    let mut file_name = name.to_os_string();
    file_name.push(".jsonl");
    let _held = LOCK.lock();
    let _ = std::fs::create_dir_all(&logs);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs.join(file_name))
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// [`std::process`], with its `Command` and `Child` recorded.
pub mod process {
    use std::ffi::OsStr;
    use std::path::Path;

    pub use std::process::*;

    use super::{demo_log, demo_spawn_id, open, program_name, Category, Counts, DEMO_SPAWN_ID_ENV};

    /// DEMONSTRATION ONLY: a spawn's id and the directory its timing is logged in.
    type Demo = Option<(String, std::path::PathBuf)>;

    /// [`std::process::Command`], recording each process it starts and every
    /// wait on one as the program's category.
    ///
    /// The builder methods are its own, so a chain that ends in
    /// [`spawn`](Self::spawn), [`output`](Self::output) or
    /// [`status`](Self::status) reaches the recorded ones; everything that only
    /// reads the command is `std`'s, through `Deref`.
    #[derive(Debug)]
    pub struct Command {
        inner: std::process::Command,
    }

    impl From<std::process::Command> for Command {
        fn from(inner: std::process::Command) -> Self {
            Self { inner }
        }
    }

    impl std::ops::Deref for Command {
        type Target = std::process::Command;

        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    impl std::ops::DerefMut for Command {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.inner
        }
    }

    impl Command {
        /// [`std::process::Command::new`].
        pub fn new<S: AsRef<OsStr>>(program: S) -> Self {
            Self {
                inner: std::process::Command::new(program),
            }
        }

        /// [`std::process::Command::arg`].
        pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
            self.inner.arg(arg);
            self
        }

        /// [`std::process::Command::args`].
        pub fn args<I, S>(&mut self, args: I) -> &mut Self
        where
            I: IntoIterator<Item = S>,
            S: AsRef<OsStr>,
        {
            self.inner.args(args);
            self
        }

        /// [`std::process::Command::env`].
        pub fn env<K: AsRef<OsStr>, V: AsRef<OsStr>>(&mut self, key: K, value: V) -> &mut Self {
            self.inner.env(key, value);
            self
        }

        /// [`std::process::Command::envs`].
        pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
        where
            I: IntoIterator<Item = (K, V)>,
            K: AsRef<OsStr>,
            V: AsRef<OsStr>,
        {
            self.inner.envs(vars);
            self
        }

        /// [`std::process::Command::env_remove`].
        pub fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Self {
            self.inner.env_remove(key);
            self
        }

        /// [`std::process::Command::env_clear`].
        pub fn env_clear(&mut self) -> &mut Self {
            self.inner.env_clear();
            self
        }

        /// [`std::process::Command::current_dir`].
        pub fn current_dir<P: AsRef<Path>>(&mut self, dir: P) -> &mut Self {
            self.inner.current_dir(dir);
            self
        }

        /// [`std::process::Command::stdin`].
        pub fn stdin<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
            self.inner.stdin(cfg);
            self
        }

        /// [`std::process::Command::stdout`].
        pub fn stdout<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
            self.inner.stdout(cfg);
            self
        }

        /// [`std::process::Command::stderr`].
        pub fn stderr<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
            self.inner.stderr(cfg);
            self
        }

        /// The program's category and name — and, for an `onepipeline` under
        /// [`SPANS_ENV`](super::SPANS_ENV), where it writes its span record, unless
        /// the test itself set or removed that.
        fn program(&mut self) -> (Category, String, Demo) {
            let program = program_name(self.inner.get_program());
            let mut demo = None;
            if program == "onepipeline"
                && !self
                    .inner
                    .get_envs()
                    .any(|(key, _)| key == super::BINARY_SPANS_ENV)
            {
                if let Some(spans) = super::spans_dir() {
                    self.inner.env(super::BINARY_SPANS_ENV, &spans);
                    let id = demo_spawn_id();
                    self.inner.env(DEMO_SPAWN_ID_ENV, &id);
                    demo = Some((id, spans));
                }
            }
            (Category::of_program(&program), program, demo)
        }

        /// [`std::process::Command::spawn`]: one process, and the time it took
        /// to start.
        ///
        /// # Errors
        ///
        /// As `std`'s.
        pub fn spawn(&mut self) -> std::io::Result<Child> {
            let (category, program, demo) = self.program();
            let (began, clock) = (std::time::SystemTime::now(), std::time::Instant::now());
            let started = {
                let _span = open(category, Counts::Leaf, Some(program.clone()));
                self.inner.spawn()
            };
            if let Some((id, dir)) = &demo {
                demo_log(dir, id, "spawn", began, clock.elapsed());
            }
            started.map(|inner| Child {
                inner,
                category,
                program,
                demo,
            })
        }

        /// [`std::process::Command::output`]: one process, and all the time it
        /// ran.
        ///
        /// # Errors
        ///
        /// As `std`'s.
        pub fn output(&mut self) -> std::io::Result<Output> {
            let (category, program, demo) = self.program();
            let (began, clock) = (std::time::SystemTime::now(), std::time::Instant::now());
            let output = {
                let _span = open(category, Counts::Leaf, Some(program));
                self.inner.output()
            };
            if let Some((id, dir)) = &demo {
                demo_log(dir, id, "output", began, clock.elapsed());
            }
            output
        }

        /// [`std::process::Command::status`]: one process, and all the time it
        /// ran.
        ///
        /// # Errors
        ///
        /// As `std`'s.
        pub fn status(&mut self) -> std::io::Result<ExitStatus> {
            let (category, program, demo) = self.program();
            let (began, clock) = (std::time::SystemTime::now(), std::time::Instant::now());
            let status = {
                let _span = open(category, Counts::Leaf, Some(program));
                self.inner.status()
            };
            if let Some((id, dir)) = &demo {
                demo_log(dir, id, "status", began, clock.elapsed());
            }
            status
        }
    }

    /// [`std::process::Child`], recording a wait on it as its program's
    /// category. Its fields and every other method are `std`'s, through `Deref`.
    #[derive(Debug)]
    pub struct Child {
        inner: std::process::Child,
        category: Category,
        program: String,
        demo: Demo,
    }

    impl std::ops::Deref for Child {
        type Target = std::process::Child;

        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    impl std::ops::DerefMut for Child {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.inner
        }
    }

    impl Child {
        /// The `std` child, for an interface that takes one.
        pub fn into_inner(self) -> std::process::Child {
            self.inner
        }

        /// [`std::process::Child::wait`].
        ///
        /// # Errors
        ///
        /// As `std`'s.
        pub fn wait(&mut self) -> std::io::Result<ExitStatus> {
            let (began, clock) = (std::time::SystemTime::now(), std::time::Instant::now());
            let status = {
                let _span = open(self.category, Counts::Nothing, Some(self.program.clone()));
                self.inner.wait()
            };
            if let Some((id, dir)) = &self.demo {
                demo_log(dir, id, "wait", began, clock.elapsed());
            }
            status
        }

        /// [`std::process::Child::wait_with_output`].
        ///
        /// # Errors
        ///
        /// As `std`'s.
        pub fn wait_with_output(self) -> std::io::Result<Output> {
            let (began, clock) = (std::time::SystemTime::now(), std::time::Instant::now());
            let output = {
                let _span = open(self.category, Counts::Nothing, Some(self.program));
                self.inner.wait_with_output()
            };
            if let Some((id, dir)) = &self.demo {
                demo_log(dir, id, "wait", began, clock.elapsed());
            }
            output
        }
    }
}

/// [`std::thread`], with `sleep` recorded as a wait.
pub mod thread {
    pub use std::thread::*;

    /// [`std::thread::sleep`].
    pub fn sleep(duration: std::time::Duration) {
        super::leaf(super::Category::Wait, || std::thread::sleep(duration));
    }
}

/// [`std::fs`], with the operations the suite spends time in recorded as
/// filesystem work. A directory listing records its opening; reading its
/// entries is the caller's.
pub mod fs {
    use std::path::{Path, PathBuf};

    pub use std::fs::*;

    use super::{leaf, Category};

    /// [`std::fs::write`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn write<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, contents: C) -> std::io::Result<()> {
        leaf(Category::Fs, || std::fs::write(path, contents))
    }

    /// [`std::fs::read`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn read<P: AsRef<Path>>(path: P) -> std::io::Result<Vec<u8>> {
        leaf(Category::Fs, || std::fs::read(path))
    }

    /// [`std::fs::read_to_string`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn read_to_string<P: AsRef<Path>>(path: P) -> std::io::Result<String> {
        leaf(Category::Fs, || std::fs::read_to_string(path))
    }

    /// [`std::fs::read_dir`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn read_dir<P: AsRef<Path>>(path: P) -> std::io::Result<ReadDir> {
        leaf(Category::Fs, || std::fs::read_dir(path))
    }

    /// [`std::fs::create_dir`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn create_dir<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
        leaf(Category::Fs, || std::fs::create_dir(path))
    }

    /// [`std::fs::create_dir_all`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn create_dir_all<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
        leaf(Category::Fs, || std::fs::create_dir_all(path))
    }

    /// [`std::fs::remove_file`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn remove_file<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
        leaf(Category::Fs, || std::fs::remove_file(path))
    }

    /// [`std::fs::remove_dir`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn remove_dir<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
        leaf(Category::Fs, || std::fs::remove_dir(path))
    }

    /// [`std::fs::remove_dir_all`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn remove_dir_all<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
        leaf(Category::Fs, || std::fs::remove_dir_all(path))
    }

    /// [`std::fs::copy`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn copy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> std::io::Result<u64> {
        leaf(Category::Fs, || std::fs::copy(from, to))
    }

    /// [`std::fs::rename`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> std::io::Result<()> {
        leaf(Category::Fs, || std::fs::rename(from, to))
    }

    /// [`std::fs::hard_link`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn hard_link<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> std::io::Result<()> {
        leaf(Category::Fs, || std::fs::hard_link(original, link))
    }

    /// [`std::fs::metadata`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn metadata<P: AsRef<Path>>(path: P) -> std::io::Result<Metadata> {
        leaf(Category::Fs, || std::fs::metadata(path))
    }

    /// [`std::fs::canonicalize`].
    ///
    /// # Errors
    ///
    /// As `std`'s.
    pub fn canonicalize<P: AsRef<Path>>(path: P) -> std::io::Result<PathBuf> {
        leaf(Category::Fs, || std::fs::canonicalize(path))
    }
}
