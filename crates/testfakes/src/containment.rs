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
//!   the kernel's parent links at the moment of the teardown. A process whose
//!   parent had already exited was handed to `init` then, and nothing links it
//!   back here. The one thing that would keep it here, Linux's
//!   `PR_SET_CHILD_SUBREAPER`, would make every such orphan a zombie of the test
//!   process once it ended, and a zombie answers the liveness probe the engine
//!   reads its drivers with as alive. macOS has nothing like it.
//!
//! Per process rather than per test, because a test runs in a process of its own
//! under nextest and that process is the unit both platforms can describe. A
//! caller holding several fixtures ends its descendants when the last one goes.

/// How long [`end_descendants`] waits for what it ended to be gone.
///
/// A process ended with `SIGKILL` or `TerminateProcess` cannot refuse it, so
/// anything still running past this is something the teardown did not reach,
/// and the caller is told which pids those are.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(30);

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
/// The pids still running when the wait ran out, or the reason this host would
/// not describe its processes.
pub fn end_descendants() -> Result<(), String> {
    platform::end_descendants()
}

#[cfg(unix)]
mod platform {
    use super::{PATIENCE, POLL};

    pub fn enrol() {}

    pub fn end_descendants() -> Result<(), String> {
        let me = std::process::id();
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            let found = descendants(me)?;
            if found.is_empty() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "pids {found:?} are still running {}s after they were ended",
                    PATIENCE.as_secs()
                ));
            }
            // Stopped first, all of them, and the walk read again until it finds
            // nothing new: a stopped process starts nothing, so the set this
            // ends has stopped growing. Killed before that, a parent hands any
            // child it started in the meantime to `init`, where no walk from
            // here finds it.
            let mut frozen = found;
            for pid in &frozen {
                signal(*pid, libc::SIGSTOP);
            }
            loop {
                let fresh: Vec<u32> = descendants(me)?
                    .into_iter()
                    .filter(|pid| !frozen.contains(pid))
                    .collect();
                // Bounded too: another thread of this process still starting
                // things would otherwise keep this from ever reaching the kill.
                if fresh.is_empty() || std::time::Instant::now() >= deadline {
                    break;
                }
                for pid in &fresh {
                    signal(*pid, libc::SIGSTOP);
                }
                frozen.extend(fresh);
            }
            for pid in &frozen {
                signal(*pid, libc::SIGKILL);
            }
            std::thread::sleep(POLL);
        }
    }

    /// Send `signal` to one process, by a pid the walk found under this one.
    ///
    /// A pid that is no longer there is not an error: it ended between the walk
    /// and the signal, which is what this was going to make it do.
    fn signal(pid: u32, signal: i32) {
        let Ok(raw) = i32::try_from(pid) else {
            return;
        };
        // `kill` reads zero and below as a group or a broadcast, never one
        // process; nothing the walk returns is one, but a signal is not sent
        // on the strength of that.
        if raw <= 0 {
            return;
        }
        // SAFETY: `kill` touches no memory this call owns, and `raw` addresses
        // one process.
        unsafe { libc::kill(raw, signal) };
    }

    /// Every process below `root` that has not yet ended, however deep.
    ///
    /// Through `ps`, which every Unix answers, and with its state so that a
    /// child that has ended and is only waiting for this process to collect it
    /// is not counted: it holds no handle and cannot be ended again.
    ///
    /// The `ps` that answers is itself a child of `root` while it lists, and a
    /// new one each time, so it is left out by its pid: counted, it would be a
    /// descendant every listing found afresh.
    fn descendants(root: u32) -> Result<Vec<u32>, String> {
        let lister = std::process::Command::new("ps")
            .args(["-A", "-o", "pid=,ppid=,stat="])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
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
        let table = parse(&text)?;
        let mut found = Vec::new();
        let mut frontier = vec![root];
        while let Some(parent) = frontier.pop() {
            for (pid, _, _) in table.iter().filter(|(_, ppid, _)| *ppid == parent) {
                if *pid != root && *pid != lister_pid && !found.contains(pid) {
                    found.push(*pid);
                    frontier.push(*pid);
                }
            }
        }
        // Walked through ended processes too, because one that has ended but
        // not been collected still parents what it started, and only then
        // dropped from what is ended.
        Ok(found
            .into_iter()
            .filter(|pid| {
                table
                    .iter()
                    .any(|(listed, _, state)| listed == pid && !state.starts_with('Z'))
            })
            .collect())
    }

    /// `(pid, parent, state)` for every row, or the row that is not one.
    ///
    /// Every row is read or the listing is refused: a row skipped could be the
    /// descendant the teardown exists to reach.
    fn parse(listed: &str) -> Result<Vec<(u32, u32, String)>, String> {
        listed
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                row(line).ok_or_else(|| format!("ps wrote a row that is not a process: {line:?}"))
            })
            .collect()
    }

    /// Exactly the three columns asked for: two ids, and a state whose first
    /// character is the one-letter code every `ps` leads it with — the rest are
    /// flags (`Ss`, `R+`, `S<l`). A pid of `0` is no process this may signal.
    fn row(line: &str) -> Option<(u32, u32, String)> {
        let mut columns = line.split_whitespace();
        let pid = columns.next()?.parse().ok().filter(|pid| *pid != 0)?;
        let parent = columns.next()?.parse().ok()?;
        let state = columns.next()?;
        let well_formed = state
            .chars()
            .next()
            .is_some_and(|code| code.is_ascii_uppercase())
            && state.chars().all(|flag| flag.is_ascii_graphic());
        (well_formed && columns.next().is_none()).then(|| (pid, parent, state.to_owned()))
    }
}

#[cfg(windows)]
mod platform {
    use super::{PATIENCE, POLL};
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

    pub fn end_descendants() -> Result<(), String> {
        let Some(job) = JOB.get().map(|job| *job as HANDLE) else {
            // Nothing was enrolled, so nothing this process started is in a job
            // to end.
            return Ok(());
        };
        let me = std::process::id();
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            let members: Vec<u32> = members(job)?.into_iter().filter(|pid| *pid != me).collect();
            if members.is_empty() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "pids {members:?} are still in this test's job {}s after they were ended",
                    PATIENCE.as_secs()
                ));
            }
            for pid in members {
                end(job, pid);
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
    fn end(job: HANDLE, pid: u32) {
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
            let millis = u32::try_from(PATIENCE.as_millis()).unwrap_or(u32::MAX);
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
