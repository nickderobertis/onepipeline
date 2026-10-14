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
//!   to `init` then, and nothing links it back here. The one thing that would keep it here, Linux's
//!   `PR_SET_CHILD_SUBREAPER`, would make every such orphan a zombie of the test
//!   process once it ended, and a zombie answers the liveness probe the engine
//!   reads its drivers with as alive. macOS has nothing like it.
//!
//! Per process rather than per test, because a test runs in a process of its own
//! under nextest and that process is the unit both platforms can describe. A
//! caller holding several fixtures ends its descendants when the last one goes.

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

/// The longest [`PATIENCE_VARIABLE`] may ask for: a bound a teardown still
/// keeps, where a larger one would read on Windows as a wait without end.
const LONGEST_PATIENCE: std::time::Duration = std::time::Duration::from_secs(600);

/// [`PATIENCE`], or what [`PATIENCE_VARIABLE`] replaces it with.
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

/// How often the teardown looks again while it waits.
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

#[cfg(unix)]
mod platform {
    use super::POLL;

    pub fn enrol() {}

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
            std::thread::sleep(POLL);
        }
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
        let lister = std::process::Command::new("ps")
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
    /// writes a traced process `t` — and the rest flags (`Ss`, `R+`, `S<l`); and
    /// the start, which is several words and the rest of the row. A pid that
    /// is no [`Pid`] is no row.
    fn row(line: &str) -> Option<Row> {
        let mut columns = line.split_whitespace();
        let pid = Pid::new(columns.next()?.parse().ok()?)?;
        let parent = columns.next()?.parse().ok()?;
        let state = columns.next()?;
        let well_formed = state
            .chars()
            .next()
            .is_some_and(|code| code.is_ascii_alphabetic())
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

    pub fn enrol() {
        JOB.get_or_init(|| {
            // SAFETY: null attributes and a null name make an unnamed job
            // whose handle no child inherits; the result is checked below.
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            assert!(
                !job.is_null(),
                "no job object for this test's processes: {}",
                std::io::Error::last_os_error()
            );
            // SAFETY: both handles are live; the pseudo-handle for this process
            // needs no closing.
            let assigned = unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) };
            assert!(
                assigned != 0,
                "this test's process could not join its own job: {}",
                std::io::Error::last_os_error()
            );
            job as usize
        });
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
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "pids {members:?} are still in this test's job {patience:?} after they were \
                     ended"
                ));
            }
            for pid in members {
                end(job, pid, patience);
            }
            std::thread::sleep(POLL);
        }
    }

    /// End one member of the job and wait, inside the teardown's bound, for it
    /// to have gone.
    ///
    /// The pid came from the job's own list, and the process it opens is asked
    /// again whether it is in the job before it is ended: a pid is only a
    /// number, and the handle is what cannot have been reissued to something
    /// else in between.
    fn end(job: HANDLE, pid: u32, patience: std::time::Duration) {
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
            // Never `INFINITE`, which is `u32::MAX`: `patience` is bounded well
            // below it, and a value past it would still be a bounded wait.
            let millis = u32::try_from(patience.as_millis()).unwrap_or(u32::MAX - 1);
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
