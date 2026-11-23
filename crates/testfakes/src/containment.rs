//! Keeping every process a test starts from outliving it.
//!
//! A process a test starts inherits the test's standard handles, and one of them
//! is the pipe `cargo nextest` reads that test's output from. A descendant still
//! running when the test ends holds that pipe open: nextest reports the test
//! `LEAK`, and a CI step that tees the run's output waits for an EOF that comes
//! only when that descendant ends. A `fake-oneharness run` orphaned on the
//! Windows leg held the step that way for thirteen minutes after the last test
//! had passed, until the step's own timeout cancelled the job.
//!
//! [`enrol`] is called before a test starts anything and [`end_descendants`]
//! once it has finished with what it started. What the pair reaches is the
//! platform's to say.
//!
//! - **Windows**: everything. [`enrol`] puts the calling process in a job object
//!   of its own, every process it starts afterwards joins that job, and so does
//!   every process *those* start, whether or not its parent is still running.
//!   The job is a kernel record of descent that an exit does not break, so an
//!   orphan is reached as surely as a child. Nothing this suite runs asks to
//!   leave a job, and nothing could: the job does not allow breakaway.
//! - **Unix**: every process still descended from the calling one, walked through
//!   the kernel's parent links at the moment of the teardown, and waited for by
//!   its own identity once found — killing a parent hands a child still on its
//!   way out to `init`, out of the walk's reach but not out of the teardown's.
//!   A process whose parent had already exited *before* the teardown was handed
//!   to `init` then, and no parent link leads back here; the register of
//!   process groups below is what does. The one thing that would keep it here, Linux's
//!   `PR_SET_CHILD_SUBREAPER`, would make every such orphan a zombie of the test
//!   process once it ended, and a zombie answers the liveness probe the engine
//!   reads its drivers with as alive. macOS has nothing like it.
//!
//! Per process rather than per test, because a test runs in a process of its own
//! under nextest and that process is the unit both platforms can describe. A
//! caller holding several fixtures ends its descendants when the last one goes.
//!
//! What the walk cannot reach on Unix, a **register of process groups** does.
//! Group membership survives reparenting, and a line in a file outlives the
//! process that wrote it, where the parent links the walk reads do not. So the
//! process that makes a group — the test, a double, or a double's descendant —
//! or, for a group something outside this suite made, a double running in it,
//! appends the group's id to the file [`GROUPS_VARIABLE`] names through
//! [`own_group`], straight after `spawn` returns and before it can exit, and the
//! teardown ends every group the register names with [`end_owned_groups`],
//! orphaned members included. Each id is its leader's pid; a group still holding any
//! member keeps that id from being given to another process, so what is ended
//! is the group that was registered. The one id the teardown cannot tell apart
//! is one whose group had already emptied *and* whose pid the host has since
//! given to a new group leader, which needs the host's whole pid space to wrap
//! within one test.

/// How long [`end_descendants`] waits for what it ended to be gone, unless
/// [`PATIENCE_VARIABLE`] says otherwise.
///
/// A process ended with `SIGKILL` or `TerminateProcess` cannot refuse it, so
/// anything still running past this is something the teardown did not reach,
/// and the caller is told which pids those are.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(30);

/// The environment variable that replaces [`PATIENCE`], in whole milliseconds.
///
/// So that a test can hold the teardown to what it reports when the wait runs
/// out without waiting thirty seconds for it: at `0` the teardown gives up at
/// its first look, before it has ended anything.
pub const PATIENCE_VARIABLE: &str = "ONEPIPELINE_TESTFAKES_TEARDOWN_PATIENCE_MS";

/// The environment variable naming a test's register of process groups: an
/// append-only file, one whole-line decimal group id per line.
///
/// A test's fixture sets it on every command it builds, so every process that
/// command starts, and every one those start, registers into the same file.
pub const GROUPS_VARIABLE: &str = "ONEPIPELINE_TESTFAKES_GROUPS";

/// The longest [`PATIENCE_VARIABLE`] may ask for: a bound a teardown still
/// keeps, where a larger one would read on Windows as a wait without end.
const LONGEST_PATIENCE: std::time::Duration = std::time::Duration::from_secs(600);

fn patience() -> Result<std::time::Duration, String> {
    let Some(set) = std::env::var_os(PATIENCE_VARIABLE) else {
        return Ok(PATIENCE);
    };
    let patience = set
        .to_str()
        .and_then(|millis| millis.parse().ok())
        .map(std::time::Duration::from_millis)
        .ok_or_else(|| format!("{PATIENCE_VARIABLE}={set:?} is not a number of milliseconds"))?;
    if patience > LONGEST_PATIENCE {
        return Err(format!(
            "{PATIENCE_VARIABLE}={set:?} is longer than the {LONGEST_PATIENCE:?} a teardown may \
             wait"
        ));
    }
    Ok(patience)
}

/// How long the teardown sleeps between ending what it found and looking again.
///
/// Every teardown that ends anything pays at least one, so it is short; on Unix
/// each look runs `ps`, so it is not shorter.
const POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// Make every process the calling one starts from now on one that
/// [`end_descendants`] can reach.
///
/// Idempotent, so every fixture calls it rather than one remembering to. On
/// Unix there is nothing to set up: the walk reads the kernel's parent links at
/// teardown.
///
/// # Panics
///
/// On Windows, if the job cannot be made or the process cannot be put in it.
/// A test that went on would be one whose descendants nothing can end.
pub fn enrol() {
    platform::enrol();
}

/// End every process the calling one started that is still running, and wait
/// until each has gone.
///
/// Forcefully, because the polite ask has already been made by then: a fixture
/// stops its runs through the binary's own `stop` first, and what is left is
/// what that did not reach.
///
/// # Errors
///
/// The pids still running when the wait ran out, the reason this host would
/// not describe its processes, or a [`PATIENCE_VARIABLE`] that is no number.
pub fn end_descendants() -> Result<(), String> {
    platform::end_descendants(patience()?)
}

/// Register `pgid`, a process group the calling process has just made, in the
/// register [`GROUPS_VARIABLE`] names, so the test's teardown ends it whatever
/// becomes of the calling process.
///
/// Call it straight after the `spawn` that made the group returns — `spawn`
/// returns once the child has exec'd, so the group exists — and before the
/// caller can exit, so no group outlives its maker unregistered. Nothing to do
/// when the variable is unset, which is every process no test started, and on
/// Windows, whose job object already reaches orphans.
///
/// # Errors
///
/// The register could not be written, which leaves the group out of reach of
/// the teardown: the caller fails rather than going on with it.
pub fn own_group(pgid: u32) -> Result<(), String> {
    match std::env::var_os(GROUPS_VARIABLE) {
        Some(register) if cfg!(unix) => own_group_in(std::path::Path::new(&register), pgid),
        _ => Ok(()),
    }
}

/// [`own_group`], for a process whose own environment does not name the
/// register: the test process itself, which knows its world's register by path.
///
/// # Errors
///
/// As [`own_group`].
pub fn own_group_in(register: &std::path::Path, pgid: u32) -> Result<(), String> {
    use std::io::Write;
    if !cfg!(unix) {
        return Ok(());
    }
    // One `write` of one whole line to a file opened for appending, so lines
    // several processes append at once never interleave.
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(register)
        .and_then(|mut file| file.write_all(format!("{pgid}\n").as_bytes()))
        .map_err(|error| {
            format!(
                "process group {pgid} could not be registered in {}: {error}",
                register.display()
            )
        })
}

/// End every process group the register at `register` names, where one is
/// given, and every group in `more`, and wait until none of them holds a member
/// that has not ended.
///
/// `SIGKILL` to each whole group, for the reason [`end_descendants`] gives, and
/// a member that has ended but not been collected counts as ended, as
/// [`process_ended`] says. A register that does not exist names nothing: no
/// process made a group. Never the caller's own group, nor one that is no group
/// at all (`0` and `1` read as broadcasts and `init`). Nothing on Windows, where
/// [`own_group`] registers nothing and the job reaches every member.
///
/// # Errors
///
/// A line of the register that is no group id, the pids still running when the
/// wait ran out, the reason this host would not describe its processes, or a
/// [`PATIENCE_VARIABLE`] that is no number. Every group the register does name
/// is ended before a bad line is reported.
pub fn end_owned_groups(register: Option<&std::path::Path>, more: &[u32]) -> Result<(), String> {
    let read = register.map_or(Ok(String::new()), std::fs::read_to_string);
    let text = match read {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(format!(
                "the register of process groups at {register:?} could not be read: {error}"
            ))
        }
    };
    let mut groups: Vec<u32> = more.to_vec();
    let mut malformed = Vec::new();
    for line in text.lines() {
        match line.parse::<u32>() {
            Ok(pgid) => groups.push(pgid),
            Err(_) => malformed.push(line.to_owned()),
        }
    }
    groups.sort_unstable();
    groups.dedup();
    platform::end_groups(&groups, patience()?)?;
    if malformed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the register of process groups at {register:?} holds lines that are no group id: \
             {malformed:?}"
        ))
    }
}

/// Whether this host can show that `pid` has ended.
///
/// A process that has ended but not yet been reaped counts as ended, because the
/// question is whether it can still write or hold a handle, and a zombie cannot.
/// Every other answer this host cannot give reads as still running, so a wait on
/// this outlives the process rather than stopping short of it.
pub fn process_ended(pid: u32) -> bool {
    platform::process_ended(pid)
}

#[cfg(unix)]
mod platform {
    use super::POLL;

    pub fn enrol() {}

    /// The state is asked of `ps` rather than read from `/proc`, which macOS
    /// does not have: there, a double a teardown killed and nothing reaped read
    /// as still running, and every teardown journey failed on the macOS leg
    /// alone. `ps` leads a zombie's state with `Z` on every Unix, so Linux runs
    /// this path too.
    ///
    /// A zombie its parent reaps after the signal-0 probe and before `ps` reads
    /// the table is listed as nothing, which is not `Z`; read as still running,
    /// that turned an ended driver into a live one on a loaded runner. So a
    /// listing that is not a zombie's asks the probe again, and a pid that has
    /// gone by then has ended.
    pub fn process_ended(pid: u32) -> bool {
        let Ok(raw) = libc::pid_t::try_from(pid) else {
            return false;
        };
        let gone = || {
            // SAFETY: signal 0 delivers nothing and touches no memory this call owns.
            let refused = unsafe { libc::kill(raw, 0) } != 0;
            refused && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        };
        if gone() {
            return true;
        }
        let zombie = crate::profile::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .is_ok_and(|listed| {
                String::from_utf8_lossy(&listed.stdout)
                    .trim_start()
                    .starts_with('Z')
            });
        zombie || gone()
    }

    /// The id of exactly one process: positive, and within what `kill` takes,
    /// which reads zero and below as a group or a broadcast.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Pid(libc::pid_t);

    impl Pid {
        fn new(pid: u32) -> Option<Self> {
            libc::pid_t::try_from(pid)
                .ok()
                .filter(|pid| *pid > 0)
                .map(Self)
        }

        fn get(self) -> u32 {
            self.0.unsigned_abs()
        }
    }

    /// One process this teardown found, told apart from a later process that
    /// is given the same pid by when it started.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Found {
        pid: Pid,
        started: String,
    }

    /// One row of the process table: the process, its parent, and whether it
    /// has ended.
    struct Row {
        process: Found,
        parent: u32,
        state: State,
    }

    /// All a teardown asks of a process's state.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        /// Not yet ended, whatever it is doing: running, sleeping, stopped,
        /// traced, or on its way out.
        Live,
        /// Ended and waiting only to be collected (`Z`): it holds nothing any
        /// more and cannot be ended again.
        Zombie,
    }

    pub fn end_descendants(patience: std::time::Duration) -> Result<(), String> {
        let me = std::process::id();
        let deadline = std::time::Instant::now() + patience;
        // Everything any walk below has found, kept after it stops being found.
        // Killing a parent hands a child still on its way out to `init`, so the
        // next walk from here no longer reaches it; it was a descendant at the
        // teardown all the same, and the teardown is not over until it has gone.
        let mut ended: Vec<Found> = Vec::new();
        loop {
            let table = list()?;
            remember(&mut ended, descendants(me, &table));
            let running: Vec<&Found> = ended
                .iter()
                .filter(|found| still_running(found, &table))
                .collect();
            if running.is_empty() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                let pids: Vec<u32> = running.iter().map(|found| found.pid.get()).collect();
                return Err(format!(
                    "pids {pids:?} are still running {patience:?} after they were ended"
                ));
            }
            // Stopped first, all of them, and the walk read again until it finds
            // nothing new: a stopped process starts nothing, so the set this
            // ends has stopped growing.
            let mut frozen: Vec<Found> = running.into_iter().cloned().collect();
            for found in &frozen {
                signal(found.pid, libc::SIGSTOP);
            }
            // llmlint: ignore-block[changed_behavior_has_e2e] what this loop exists for is a
            // process started in the microseconds between a walk and its stop, which no test
            // can make happen on demand; every journey that ends a tree runs through it, and
            // what each asserts — nothing it started is left — is what it would break.
            loop {
                let table = list()?;
                let fresh: Vec<Found> = descendants(me, &table)
                    .into_iter()
                    .filter(|found| !frozen.contains(found))
                    .collect();
                // Bounded too: another thread of this process still starting
                // things would otherwise keep this from ever reaching the kill.
                if fresh.is_empty() || std::time::Instant::now() >= deadline {
                    break;
                }
                for found in &fresh {
                    signal(found.pid, libc::SIGSTOP);
                }
                remember(&mut ended, fresh.iter().cloned());
                frozen.extend(fresh);
            }
            // llmlint: ignore-end[changed_behavior_has_e2e]
            for found in &frozen {
                signal(found.pid, libc::SIGKILL);
            }
            crate::profile::thread::sleep(POLL);
        }
    }

    pub fn end_groups(groups: &[u32], patience: std::time::Duration) -> Result<(), String> {
        // SAFETY: `getpgrp` takes nothing and cannot fail.
        let mine = unsafe { libc::getpgrp() };
        let groups: Vec<libc::pid_t> = groups
            .iter()
            .filter_map(|pgid| libc::pid_t::try_from(*pgid).ok())
            .filter(|pgid| *pgid > 1 && *pgid != mine)
            .collect();
        if groups.is_empty() {
            return Ok(());
        }
        let deadline = std::time::Instant::now() + patience;
        loop {
            let live = members(&groups)?;
            if live.is_empty() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "pids {live:?} of process groups {groups:?} are still running {patience:?} \
                     after their groups were ended"
                ));
            }
            for pgid in &groups {
                // SAFETY: `killpg` touches no memory this call owns, and each id
                // is a group above `init` that is not this process's own. A group
                // already empty answers `ESRCH`, which is what this was for.
                unsafe { libc::killpg(*pgid, libc::SIGKILL) };
            }
            crate::profile::thread::sleep(POLL);
        }
    }

    /// The pids in any of `groups` that have not ended, through `ps`, which
    /// every Unix answers with each process's group.
    fn members(groups: &[libc::pid_t]) -> Result<Vec<u32>, String> {
        let listed = crate::profile::process::Command::new("ps")
            .args(["-A", "-o", "pid=,pgid=,stat="])
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .map_err(|error| format!("ps did not run: {error}"))?;
        if !listed.status.success() {
            return Err(format!("ps exited {}", listed.status));
        }
        let text = String::from_utf8(listed.stdout)
            .map_err(|error| format!("ps wrote something other than text: {error}"))?;
        let mut live = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let mut columns = line.split_whitespace();
            let (Some(pid), Some(pgid), Some(state)) =
                (columns.next(), columns.next(), columns.next())
            else {
                return Err(format!("ps wrote a row that is not a process: {line:?}"));
            };
            // Wider than a group id, because Linux lists a process already past
            // the point of having one with the group `-1`, unsigned: no group of
            // ours, and no reason to refuse the listing. Untested on purpose: no
            // journey can hold a process at that instant on demand. The one that
            // met it failed its teardown on the refusal:
            // `driver::a_stop_whose_host_never_shows_its_frozen_tree_stopped_still_ends_it`.
            let (Ok(pid), Ok(pgid)) = (pid.parse::<u32>(), pgid.parse::<i64>()) else {
                return Err(format!("ps wrote a row that is not a process: {line:?}"));
            };
            let ours = groups.iter().any(|group| i64::from(*group) == pgid);
            if ours && !state.starts_with('Z') {
                live.push(pid);
            }
        }
        Ok(live)
    }

    fn remember(ended: &mut Vec<Found>, found: impl IntoIterator<Item = Found>) {
        for found in found {
            if !ended.contains(&found) {
                ended.push(found);
            }
        }
    }

    /// Whether `found` is still running: listed under its own pid with the
    /// start it was found with, wherever it is parented now, and not a zombie.
    ///
    /// A zombie has released everything it held and only waits to be
    /// collected, and a pid listed with another start is a different process
    /// that has been given it since.
    fn still_running(found: &Found, table: &[Row]) -> bool {
        // llmlint: ignore[changed_behavior_has_e2e] the start half of the identity only
        // matters once the kernel has given a found pid to another process, which a test
        // cannot arrange without cycling the host's whole pid space; the pid half is what
        // `a_worlds_teardown_waits_for_a_descendant_still_exiting_after_its_double_is_gone`
        // proves.
        table
            .iter()
            .any(|row| row.process == *found && row.state == State::Live)
    }

    /// Send `signal` to one process, by a pid the walk found under this one.
    ///
    /// A pid that is no longer there is not an error: it ended between the walk
    /// and the signal, which is what this was going to make it do.
    fn signal(pid: Pid, signal: i32) {
        // SAFETY: `kill` touches no memory this call owns, and a `Pid`
        // addresses one process.
        unsafe { libc::kill(pid.0, signal) };
    }

    /// Every process on the host, through `ps`, which every Unix answers.
    ///
    /// The `ps` that answers is itself a child of this process while it lists,
    /// and a new one each time, so it is left out by its pid: counted, it would
    /// be a descendant every listing found afresh.
    fn list() -> Result<Vec<Row>, String> {
        let lister = crate::profile::process::Command::new("ps")
            .args(["-A", "-o", "pid=,ppid=,stat=,lstart="])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            // `lstart`'s words are the C locale's in every `ps` that has it.
            .env("LC_ALL", "C")
            .spawn()
            .map_err(|error| format!("ps did not run: {error}"))?;
        let lister_pid = lister.id();
        let listed = lister
            .wait_with_output()
            .map_err(|error| format!("ps did not finish: {error}"))?;
        if !listed.status.success() {
            return Err(format!("ps exited {}", listed.status));
        }
        let text = String::from_utf8(listed.stdout)
            .map_err(|error| format!("ps wrote something other than text: {error}"))?;
        Ok(parse(&text)?
            .into_iter()
            .filter(|row| row.process.pid.get() != lister_pid)
            .collect())
    }

    /// Every process below `root` in `table` that has not yet ended, however
    /// deep.
    ///
    /// Walked through ended processes too, because one that has ended but not
    /// been collected still parents what it started, and only then dropped from
    /// what is ended.
    fn descendants(root: u32, table: &[Row]) -> Vec<Found> {
        let mut found: Vec<&Row> = Vec::new();
        let mut frontier = vec![root];
        while let Some(parent) = frontier.pop() {
            for row in table.iter().filter(|row| row.parent == parent) {
                let pid = row.process.pid;
                if pid.get() != root && !found.iter().any(|seen| seen.process.pid == pid) {
                    found.push(row);
                    frontier.push(pid.get());
                }
            }
        }
        found
            .into_iter()
            .filter(|row| row.state == State::Live)
            .map(|row| row.process.clone())
            .collect()
    }

    /// Every row, or the row that is not one.
    ///
    /// Every row is read or the listing is refused: a row skipped could be the
    /// descendant the teardown exists to reach.
    fn parse(listed: &str) -> Result<Vec<Row>, String> {
        listed
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                row(line).ok_or_else(|| format!("ps wrote a row that is not a process: {line:?}"))
            })
            .collect()
    }

    /// Exactly the columns asked for: two ids; a state whose first character
    /// is the one-letter code every `ps` leads it with, in either case — Linux
    /// writes a traced process `t` — or the `?` macOS writes for a process it
    /// cannot classify, such as one mid-exit, and the rest flags (`Ss`, `R+`,
    /// `S<l`, `?<`); and the start, which is several words and the rest of the
    /// row. A pid that is no [`Pid`] is no row.
    ///
    /// A `?` is read as live: nothing says it has ended, so it is ended and
    /// waited on until it is listed as a zombie or not at all.
    fn row(line: &str) -> Option<Row> {
        let mut columns = line.split_whitespace();
        let pid = Pid::new(columns.next()?.parse().ok()?)?;
        let parent = columns.next()?.parse().ok()?;
        let state = columns.next()?;
        let well_formed = state
            .chars()
            .next()
            .is_some_and(|code| code.is_ascii_alphabetic() || code == '?')
            && state.chars().all(|flag| flag.is_ascii_graphic());
        let started = columns.collect::<Vec<_>>().join(" ");
        (well_formed && !started.is_empty()).then(|| Row {
            process: Found { pid, started },
            parent,
            state: if state.starts_with('Z') {
                State::Zombie
            } else {
                State::Live
            },
        })
    }
}

#[cfg(windows)]
mod platform {
    use super::POLL;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JobObjectBasicProcessIdList,
        QueryInformationJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, TerminateProcess, WaitForSingleObject,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };

    /// The job this process put itself in, held for the life of the process.
    ///
    /// Kept as an address because a `HANDLE` is a raw pointer and a static has
    /// to be shareable between threads; it is only ever read back as the handle
    /// it was.
    static JOB: OnceLock<usize> = OnceLock::new();

    /// A process handle is signalled once — and only once — the process has
    /// terminated.
    pub fn process_ended(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
        // SAFETY: `OpenProcess` returns null on failure and otherwise a handle
        // closed below; no borrowed memory crosses the boundary.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            // No such process any more: its pid names nothing this host can open.
            return std::io::Error::last_os_error().raw_os_error()
                == Some(ERROR_INVALID_PARAMETER as i32);
        }
        // SAFETY: `handle` is live, and a zero timeout returns at once.
        let waited = unsafe { WaitForSingleObject(handle, 0) };
        // SAFETY: the handle came from `OpenProcess` above and is closed once.
        unsafe { CloseHandle(handle) };
        waited == WAIT_OBJECT_0
    }

    pub fn enrol() {
        JOB.get_or_init(|| {
            // SAFETY: null attributes and a null name make an unnamed job
            // whose handle no child inherits; the result is checked below.
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            // llmlint: ignore[no_panics_on_recoverable_errors] This is test-fixture setup:
            // without containment the test cannot run safely, so fail the test here.
            assert!(
                !job.is_null(),
                "no job object for this test's processes: {}",
                std::io::Error::last_os_error()
            );
            // SAFETY: both handles are live; the pseudo-handle for this process
            // needs no closing.
            let assigned = unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) };
            // llmlint: ignore[no_panics_on_recoverable_errors] Failure must fail fixture
            // setup before any test process can start outside its required containment;
            // nextest then exits this test's process, releasing its job handle too.
            assert!(
                assigned != 0,
                "this test's process could not join its own job: {}",
                std::io::Error::last_os_error()
            );
            job as usize
        });
    }

    /// The job already reaches every member of every group, and [`own_group`]
    /// registers nothing here.
    ///
    /// [`own_group`]: super::own_group
    pub fn end_groups(_groups: &[u32], _patience: std::time::Duration) -> Result<(), String> {
        Ok(())
    }

    pub fn end_descendants(patience: std::time::Duration) -> Result<(), String> {
        let Some(job) = JOB.get().map(|job| *job as HANDLE) else {
            // Nothing was enrolled, so nothing this process started is in a job
            // to end.
            return Ok(());
        };
        let me = std::process::id();
        let deadline = std::time::Instant::now() + patience;
        loop {
            // llmlint: ignore[changed_behavior_has_e2e] the job's handle is this module's
            // own and is never closed, so no test can make the kernel refuse to list it;
            // the error is carried rather than unwrapped for a refusal nothing here causes.
            let members: Vec<u32> = members(job)?.into_iter().filter(|pid| *pid != me).collect();
            if members.is_empty() {
                return Ok(());
            }
            // llmlint: ignore[changed_behavior_has_e2e] on Windows only a member that
            // `TerminateProcess` cannot end in time gets here with anything to wait for, and a
            // test cannot make one without a driver; the Unix journey
            // `a_worlds_teardown_names_a_killed_descendant_it_could_not_wait_out` proves the
            // same bound, and the zero-patience journeys prove this report on both platforms.
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "pids {members:?} are still in this test's job {patience:?} after they were \
                     ended"
                ));
            }
            for pid in members {
                end(job, pid, deadline);
            }
            crate::profile::thread::sleep(POLL);
        }
    }

    /// End one member of the job and wait, inside the teardown's bound, for it
    /// to have gone.
    ///
    /// The pid came from the job's own list, and the process it opens is asked
    /// again whether it is in the job before it is ended: a pid is only a
    /// number, and the handle is what cannot have been reissued to something
    /// else in between.
    fn end(job: HANDLE, pid: u32, deadline: std::time::Instant) {
        // SAFETY: `OpenProcess` returns null on failure and otherwise a handle
        // closed below.
        let process = unsafe {
            OpenProcess(
                PROCESS_TERMINATE | PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if process.is_null() {
            // Gone since the list was read.
            return;
        }
        let mut inside = 0;
        // SAFETY: both handles are live and `inside` outlives the call.
        let asked = unsafe { IsProcessInJob(process, job, &raw mut inside) };
        if asked != 0 && inside != 0 {
            // SAFETY: `process` is live and was opened with the right to end it.
            unsafe { TerminateProcess(process, 1) };
            // Only what is left of the teardown's bound, so that ending one
            // member after another cannot wait past it; and never `INFINITE`,
            // which is `u32::MAX`.
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let millis = u32::try_from(left.as_millis()).unwrap_or(u32::MAX - 1);
            // Whatever the wait answers, the caller lists the job again: a member
            // still there is ended again, and one that outlasts the bound is
            // named.
            //
            // SAFETY: `process` is live; the wait is bounded.
            unsafe { WaitForSingleObject(process, millis) };
        }
        // SAFETY: opened above and closed once.
        unsafe { CloseHandle(process) };
    }

    /// The pids the job holds now, this process among them.
    ///
    /// Asked with room for the whole list, grown and asked again while the job
    /// says it holds more than it returned.
    fn members(job: HANDLE) -> Result<Vec<u32>, String> {
        let mut room: usize = 64;
        loop {
            let header = std::mem::size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
            let bytes = header + room * std::mem::size_of::<usize>();
            // `usize` elements, so the buffer is aligned for the structure's
            // own `ULONG_PTR` list.
            let mut buffer = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
            let length = u32::try_from(buffer.len() * std::mem::size_of::<usize>())
                .map_err(|_| "the job's process list outgrew a query".to_owned())?;
            // SAFETY: `job` is live, and the pointer and length describe
            // `buffer`, which outlives the call.
            let read = unsafe {
                QueryInformationJobObject(
                    job,
                    JobObjectBasicProcessIdList,
                    buffer.as_mut_ptr().cast(),
                    length,
                    std::ptr::null_mut(),
                )
            };
            // SAFETY: the buffer is aligned for, and at least as large as, the
            // structure's header, which the call has filled.
            let list = unsafe { &*buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() };
            let assigned = list.NumberOfAssignedProcesses as usize;
            let returned = list.NumberOfProcessIdsInList as usize;
            if read == 0 && assigned <= room {
                return Err(format!(
                    "this test's job would not list its processes: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if assigned > returned {
                room = assigned + 16;
                continue;
            }
            // SAFETY: the kernel wrote `returned` entries starting at the list's
            // first element, all inside `buffer`.
            let ids = unsafe { std::slice::from_raw_parts(list.ProcessIdList.as_ptr(), returned) };
            return Ok(ids
                .iter()
                .filter_map(|pid| u32::try_from(*pid).ok())
                .collect());
        }
    }
}
