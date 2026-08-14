//! The host facts the engine reads: the clock, process liveness, and who is
//! asking.
//!
//! Everything here is deliberately small and total. A run's ledger records
//! timestamps and pids, and every view that reports `DRIVER DEAD` or `PARKED`
//! resolves them through this module, so an unanswerable question resolves
//! toward "still working" here rather than in each caller.

use std::time::{SystemTime, UNIX_EPOCH};

/// The environment variable naming the launching session, when the harness
/// exports one.
pub const LAUNCHER_SESSION_ENV: &str = "ONEPIPELINE_LAUNCHER_SESSION";

/// The environment variable naming the launcher itself.
pub const LAUNCHER_ENV: &str = "ONEPIPELINE_LAUNCHER";

/// What a run's owner is recorded as when nothing identifies the launcher.
pub const UNKNOWN_LAUNCHER: &str = "unknown";

/// Milliseconds since the Unix epoch.
///
/// A clock before the epoch reads as `0` rather than panicking: a run whose host
/// clock is wrong should still be observable.
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Now, as the envelope's RFC 3339 millisecond-precision UTC timestamp.
pub fn now_rfc3339() -> String {
    rfc3339_from_millis(now_millis())
}

/// Render epoch milliseconds as RFC 3339, millisecond precision, UTC.
///
/// Written out rather than taken from a date crate because this is the only
/// calendar arithmetic the crate does, and the envelope fixes the one format it
/// has to produce.
pub fn rfc3339_from_millis(millis: u64) -> String {
    let secs = millis / 1_000;
    let ms = millis % 1_000;
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let sod = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (sod / 3_600, (sod % 3_600) / 60, sod % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z")
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to a Gregorian
/// date.
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    // Both are in range by construction: `m` is 1..=12 and `d` is 1..=31.
    (year, m as u64, d as u64)
}

/// This process's id.
pub fn pid() -> u32 {
    std::process::id()
}

/// This host's name, as the ledger records it.
///
/// A pid means nothing across machines, so every ownership and liveness verdict
/// is qualified by the host that recorded it.
pub fn hostname() -> String {
    for key in ["HOSTNAME", "COMPUTERNAME"] {
        if let Ok(value) = std::env::var(key) {
            if !value.is_empty() {
                return value;
            }
        }
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// How firmly a process is asked to stop.
///
/// Both reach the whole descendant tree; the difference is only how firmly each
/// asks. See [`stop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Ask it to stop and let it record its own abandonment first.
    Politely,
    /// Take it down.
    Now,
}

/// Ask a process on **this** host, and everything it started, to stop.
///
/// One place, because both callers need the same two `cfg` blocks and a second
/// copy is a platform that gets fixed in one of them: a driver being asked to
/// stand down, and a dispatch being killed. Which signal each sends is the
/// caller's decision and the only thing that differs.
///
/// # The tree, and only the tree
///
/// A run's expensive process is never the one whose pid anything recorded. The
/// driver spawns a graph, the graph spawns a harness, and the harness spawns the
/// paid agent — and that agent puts itself in a process group of its own, so a
/// teardown aimed at the *group* sweeps the middle of the tree and leaves the
/// leaf running, reparented to init, still writing into a real checkout with
/// nobody reading its output. That costs quota, it costs correctness, and it
/// costs diagnosis: an orphan carries no run attribution and is invisible to
/// every view that reports what is live.
///
/// So the boundary here is **descent**, which is neither the group's nor one
/// pid's: every process this one started, however deep, and nothing else. A
/// round that is legitimately a *child of something else* is not a descendant of
/// this pid and is not touched — a teardown that reached one would be ending
/// work it does not own, which is the other half of the same mistake.
///
/// Best-effort by construction. A process that has already exited, or that this
/// user may not signal, is not an error to report — the caller's next liveness
/// probe is what decides whether the stop landed. The process table is read a
/// moment before the signals go out, so a child started inside that moment is
/// missed; the root is signalled first, which is what closes it in practice.
pub fn stop(pid: u32, how: Stop) {
    if pid == 0 || pid == self::pid() {
        return;
    }
    platform_stop(pid, how);
}

#[cfg(unix)]
fn platform_stop(pid: u32, how: Stop) {
    let signal = match how {
        Stop::Politely => libc::SIGTERM,
        Stop::Now => libc::SIGKILL,
    };
    // The tree is read **before** anything is signalled, and that ordering is
    // the whole of it: a process whose parent has died is reparented to init at
    // once, so a table read after the root is gone no longer descends to any of
    // them. The root is then signalled first, so what is left is a tree that has
    // stopped growing while its own members are taken down.
    let tree = descendants(pid);
    signal_one(pid, signal);
    for descendant in tree {
        signal_one(descendant, signal);
    }
}

/// Send one signal to one process, best-effort.
#[cfg(unix)]
fn signal_one(pid: u32, signal: i32) {
    let Ok(raw) = i32::try_from(pid) else {
        return;
    };
    // SAFETY: `kill` takes a pid and a signal number and touches no memory this
    // call owns.
    unsafe { libc::kill(raw, signal) };
}

/// Every process descended from `pid`, however deep.
///
/// Breadth-first over a single snapshot of the process table, so a tree that
/// changed shape while it was being walked cannot be walked twice or leave this
/// looping: a pid already found is never queued again, which is also what makes
/// a table reporting a cycle — which the kernel does not produce, but a parse of
/// one might — terminate rather than hang a teardown.
#[cfg(unix)]
fn descendants(pid: u32) -> Vec<u32> {
    let table = process_table();
    let mut found: Vec<u32> = Vec::new();
    let mut frontier = vec![pid];
    while let Some(parent) = frontier.pop() {
        for (child, _) in table.iter().filter(|(_, ppid)| *ppid == parent) {
            if *child != pid && !found.contains(child) {
                found.push(*child);
                frontier.push(*child);
            }
        }
    }
    found
}

/// This host's `(pid, parent pid)` pairs.
///
/// Read through `ps`, which is the one answer every Unix gives: Linux has
/// `/proc` and macOS does not, and a second implementation is a platform that
/// gets fixed in one of them — the thing this module exists to avoid. A `ps`
/// that cannot be run leaves the table empty, which degrades a teardown to the
/// single process it was already reaching rather than failing it.
#[cfg(unix)]
fn process_table() -> Vec<(u32, u32)> {
    let Ok(listed) = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            Some((columns.next()?.parse().ok()?, columns.next()?.parse().ok()?))
        })
        .collect()
}

#[cfg(windows)]
fn platform_stop(pid: u32, how: Stop) {
    // `/T` for the tree in both cases — the same boundary the Unix arm walks the
    // process table for, which this platform offers outright. `/F` is what makes
    // the difference between the two modes: without it `taskkill` asks, and a
    // process that ignores the ask keeps running.
    let mut command = std::process::Command::new("taskkill");
    command.args(["/PID", &pid.to_string(), "/T"]);
    if how == Stop::Now {
        command.arg("/F");
    }
    let _ = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Whether a process *may* be live on this host.
///
/// Deliberately asymmetric: `false` is a proof that the process is gone, and
/// every other answer — including one this host cannot take, such as a pid
/// recorded on another machine — is `true`. A view that sends a planner to tear
/// down live work is the worse error, so an unknown resolves toward "still
/// working".
pub fn process_may_be_live(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    platform_process_may_be_live(pid)
}

#[cfg(unix)]
fn platform_process_may_be_live(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return true;
    };
    // SAFETY: `kill` with signal 0 performs the permission and existence checks
    // without delivering anything. It touches no memory this call owns.
    let rc = unsafe { libc::kill(raw, 0) };
    if rc == 0 {
        return true;
    }
    // ESRCH is the only proof of absence. EPERM means it exists and is
    // someone else's, and anything else is a question this host cannot answer.
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(windows)]
fn platform_process_may_be_live(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    // SAFETY: `OpenProcess` returns a null handle on failure and a handle this
    // function closes on success; no borrowed memory crosses the boundary.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        // A pid that never existed is rejected as an invalid parameter; every
        // other failure (a permission refusal, most of all) leaves the question
        // open, so it resolves toward live.
        return std::io::Error::last_os_error().raw_os_error()
            != Some(ERROR_INVALID_PARAMETER as i32);
    }
    // A process handle becomes signalled when — and only when — the process has
    // terminated, so a zero-millisecond wait is the whole question. Asked this
    // way rather than through `GetExitCodeProcess`, whose "still running" answer
    // is the sentinel `STILL_ACTIVE`, which is also the exit code `259` of a
    // process that has genuinely exited.
    //
    // SAFETY: `handle` is a live handle and a zero timeout returns immediately.
    let waited = unsafe { WaitForSingleObject(handle, 0) };
    // SAFETY: the handle came from `OpenProcess` above and is closed once.
    unsafe { CloseHandle(handle) };
    // `WAIT_OBJECT_0` is the one proof of absence: `WAIT_TIMEOUT` is a process
    // still running, and `WAIT_FAILED` is a question this host cannot answer.
    waited != WAIT_OBJECT_0
}

/// Stop the processes this one starts from inheriting *its own* standard
/// handles.
///
/// Windows creates a process with `bInheritHandles`, which hands over every
/// inheritable handle the parent holds — not only the three the child is being
/// given. So a launcher whose own stdout is a pipe passes that pipe's write end
/// to the driver it starts, and to everything the driver starts in turn;
/// whoever is reading the pipe then waits not for the launcher but for the whole
/// tree. `start --detach` would return to its caller only once the run it
/// detached from had finished, which is the one thing detaching promises not to
/// do. Unix hands over exactly the descriptors it is told to, so there is
/// nothing to disown there.
///
/// Call this only where nothing further will be started that is meant to
/// inherit them: it is this process's whole answer, not one spawn's.
pub fn disown_standard_handles() {
    platform_disown_standard_handles();
}

#[cfg(unix)]
fn platform_disown_standard_handles() {}

#[cfg(windows)]
fn platform_disown_standard_handles() {
    use windows_sys::Win32::Foundation::{
        SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: `GetStdHandle` returns a handle this process already owns, or
        // a null/invalid one it does not; neither borrows memory.
        let handle = unsafe { GetStdHandle(which) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            // A process started without that stream has nothing to disown.
            continue;
        }
        // A handle whose flags cannot be changed is one no child could have
        // inherited anyway, so the failure is not worth a diagnostic — least of
        // all on the stream the diagnostic would go to.
        //
        // SAFETY: `handle` is a live handle this process owns, and the call
        // clears one flag on it.
        unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
    }
}

/// The session that launched a run, as the harness's environment reports it.
///
/// Detected from the exported environment and never from process ancestry, and
/// never attributed to the reader: a launch nothing identifies is
/// [`UNKNOWN_LAUNCHER`], and an unknown run is nobody's.
pub fn launching_session() -> String {
    std::env::var(LAUNCHER_SESSION_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| UNKNOWN_LAUNCHER.to_string())
}

/// The launcher that owns this session.
pub fn launcher() -> String {
    std::env::var(LAUNCHER_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| UNKNOWN_LAUNCHER.to_string())
}

/// How another planner's session is labelled in a view.
///
/// The session id may be sensitive, so a foreign owner is named by a stable
/// digest of it rather than by the id itself. `[mine]` and `[unknown]` are the
/// two labels this never produces.
pub fn session_digest(session: &str) -> String {
    // FNV-1a: stable across processes and platforms, which is all a display
    // label needs. Nothing authenticates on this value.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in session.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{:08x}", (hash >> 32) as u32)
}

/// A pid this host can prove is gone.
///
/// Every `DRIVER DEAD` test needs one, and a pid picked out of the air is not
/// one: the kernel may have reused it. This spawns a real process and reaps it,
/// so the absence is proved rather than assumed. The child is this test binary
/// asked only to list its tests, which is portable and returns immediately.
#[cfg(test)]
pub(crate) fn reaped_pid() -> u32 {
    let mut child = std::process::Command::new(
        std::env::current_exe().expect("the test binary knows its own path"),
    )
    .args(["--list", "--format", "terse"])
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .spawn()
    .expect("the test binary starts");
    let pid = child.id();
    child.wait().expect("it exits");
    pid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_renders_as_rfc3339_millis() {
        assert_eq!(rfc3339_from_millis(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn a_known_instant_renders_with_its_milliseconds() {
        // 2026-08-08T13:29:45.678Z
        assert_eq!(
            rfc3339_from_millis(1_786_195_785_678),
            "2026-08-08T13:29:45.678Z"
        );
    }

    #[test]
    fn a_leap_day_is_not_skipped() {
        // 2024-02-29T00:00:00Z
        assert_eq!(
            rfc3339_from_millis(1_709_164_800_000),
            "2024-02-29T00:00:00.000Z"
        );
    }

    #[test]
    fn now_is_rendered_in_the_envelope_shape() {
        let now = now_rfc3339();
        assert_eq!(now.len(), 24, "{now} is not RFC 3339 millisecond UTC");
        assert!(now.ends_with('Z'), "{now} is not UTC");
    }

    #[test]
    fn this_process_is_live_and_pid_zero_is_not() {
        assert!(process_may_be_live(pid()));
        assert!(!process_may_be_live(0));
    }

    #[test]
    fn a_reaped_process_is_proved_gone() {
        let dead = reaped_pid();
        assert!(!process_may_be_live(dead), "pid {dead} was reaped");
    }

    /// A stop reaches the leaf, not just the process whose pid it was given.
    ///
    /// The tree here is the shape a run actually makes — a driver, the graph it
    /// starts, and the paid agent that graph's harness starts — and the leaf
    /// puts itself in a **process group of its own**, which is what a real one
    /// does and what made a teardown aimed at the group miss it. Every process
    /// is real and every pid is read off the kernel rather than assumed.
    ///
    /// Unix-only because the tree is built with `sh` and read back through the
    /// process table. The Windows arm hands the same boundary to `taskkill /T`,
    /// which has always walked it.
    #[cfg(unix)]
    #[test]
    fn a_stop_reaches_the_whole_descendant_tree_and_not_the_process_beside_it() {
        // `setsid` on the leaf is the point: a new session is a new process
        // group, so the leaf is outside the group of everything above it. Each
        // level prints its pid and then waits, so the test learns the tree from
        // the kernel.
        let mut tree = std::process::Command::new("sh")
            .args([
                "-c",
                "echo $$; sh -c 'echo $$; setsid sh -c \"echo \\$\\$; sleep 120\" & \
                 sleep 120' & sleep 120",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("a process tree");
        // A process started beside the tree rather than under it. A teardown
        // that widened until the leaf was included would take this too.
        let mut beside = std::process::Command::new("sh")
            .args(["-c", "sleep 120"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("a process beside the tree");

        let mut pids = Vec::new();
        {
            use std::io::BufRead;
            let out = std::io::BufReader::new(tree.stdout.take().expect("the tree reports itself"));
            for line in out.lines().take(3) {
                pids.push(
                    line.expect("a reported pid")
                        .trim()
                        .parse::<u32>()
                        .expect("a pid"),
                );
            }
        }
        assert_eq!(pids.len(), 3, "the tree did not report three levels");
        assert!(
            pids.iter().all(|pid| process_may_be_live(*pid)),
            "the tree was not running before it was stopped: {pids:?}"
        );

        stop(pids[0], Stop::Now);
        // Collected here rather than at the end, because a signalled child
        // nobody has waited on is a zombie, and a zombie answers a liveness
        // probe as alive. The two below it are this process's grandchildren, so
        // nothing here can wait on them — init collects those.
        let _ = tree.wait();
        // Signalled is not reaped: the kernel takes a moment.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline
            && pids.iter().any(|pid| process_may_be_live(*pid))
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let surviving: Vec<u32> = pids
            .iter()
            .copied()
            .filter(|pid| process_may_be_live(*pid))
            .collect();
        assert!(
            surviving.is_empty(),
            "a stop left {surviving:?} of the tree {pids:?} running — the leaf is the paid one"
        );
        assert!(
            process_may_be_live(beside.id()),
            "a stop took a process that was beside the tree rather than under it"
        );
        let _ = beside.kill();
        let _ = beside.wait();
    }

    #[test]
    fn a_foreign_session_is_labelled_by_a_stable_digest() {
        let first = session_digest("claude-code:3f9a1c2e");
        assert_eq!(first, session_digest("claude-code:3f9a1c2e"));
        assert_ne!(first, session_digest("claude-code:other"));
        assert_eq!(first.len(), 8);
    }

    #[test]
    fn the_host_always_names_itself() {
        assert!(!hostname().is_empty());
    }
}
