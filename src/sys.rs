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

/// What a teardown established about the processes it was aimed at.
///
/// Three outcomes because they call for three different things from the caller,
/// and collapsing any two of them is how a stop reports a completion nobody
/// achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Teardown {
    /// Every process in the tree was reached — or there was no process to aim
    /// at.
    ///
    /// Signalled, not *proved gone*: `kill` reports that the signal was
    /// delivered, and a process may take a moment over it. What this rules out
    /// is the failure that matters — a process nobody aimed at. The caller's
    /// next liveness probe confirms the rest.
    Signalled,
    /// The teardown never began — this host gave no listing the tree could be
    /// read from, or the program that ends it could not be run — so **nothing**
    /// was signalled and the run is exactly as it was.
    ///
    /// Deliberately not a half-teardown: a descendant is reparented the moment
    /// its parent dies, so signalling the root alone would put everything under
    /// it permanently beyond descent — the only handle a later stop has on them.
    /// Untouched, the same ask works once the host answers.
    NotAttempted,
    /// The teardown began and reached part of the tree; at least one process in
    /// it was not reached.
    ///
    /// The run is *not* untouched and retrying will not necessarily help: what
    /// could not be signalled is a process this user may not touch, and it is
    /// still running. The caller has to say so rather than report either of the
    /// other two.
    PartlySignalled,
}

/// How firmly a process is asked to stop.
///
/// Both reach the whole descendant tree; the difference is only how firmly each
/// asks — and only where the host has two ways of asking. Unix does: `SIGTERM`
/// and `SIGKILL`. Windows does not, for the processes a run is made of, so the
/// Windows arm reads this as one mode; the `platform_stop` there records what it
/// checked before deciding that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Ask it to stop and let it record its own abandonment first.
    Politely,
    /// Take it down.
    Now,
}

/// Ask a process on **this** host, and everything it started, to stop.
///
/// One place for both callers, so a platform is not fixed in only one of them;
/// the signal is the caller's decision and the only difference.
///
/// The boundary is **descent** — every process this one started, however deep,
/// and nothing else. Not the process group: the paid agent puts itself in one of
/// its own, so a group teardown sweeps the middle of the tree and leaves the leaf
/// orphaned and still writing. Not one pid, for the same reason. And nothing
/// wider: a process that is legitimately a child of something else is not a
/// descendant, and ending it would be ending work this run does not own.
///
/// Best-effort about *individual* processes: one already gone, or one this user
/// may not signal, is not an error here — the caller's next liveness probe
/// decides whether the stop landed. It is **not** best-effort about the tree: a
/// host that gives no trustworthy listing gets [`Teardown::NotAttempted`] and no
/// signals at all, because a teardown that cannot see what it must end has to
/// say so rather than end half of it.
///
/// The table is read a moment before the signals go out, so a child started
/// inside that moment is missed; signalling the root first is what closes that
/// in practice.
pub fn stop(pid: u32, how: Stop) -> Teardown {
    // `0` is no process, and this process is one a teardown must never turn on
    // itself: `stop` is called from the command doing the stopping. Neither
    // leaves a tree unreached, which is what the answer is about.
    if pid == 0 || pid == self::pid() {
        return Teardown::Signalled;
    }
    platform_stop(pid, how)
}

#[cfg(unix)]
fn platform_stop(pid: u32, how: Stop) -> Teardown {
    let signal = match how {
        Stop::Politely => libc::SIGTERM,
        Stop::Now => libc::SIGKILL,
    };
    // The tree is read **before** anything is signalled: a process whose parent
    // has died is reparented at once, so a table read after the root is gone no
    // longer descends to any of them.
    let Some(tree) = descendants(pid) else {
        return Teardown::NotAttempted;
    };
    // The root first, so what is left has stopped growing while its members are
    // taken down. Every answer is kept: one process this user may not signal is
    // one still running, and a teardown that reported the tree as reached
    // anyway would be the same false completion in a smaller place.
    let mut reached = signal_one(pid, signal);
    for descendant in tree {
        reached = signal_one(descendant, signal) && reached;
    }
    if reached {
        Teardown::Signalled
    } else {
        Teardown::PartlySignalled
    }
}

/// Signal one process, and refuse every id that is not one. `true` when this
/// user's signal reached it, or when there was no longer anything to reach.
///
/// `kill` reads a non-positive pid as a **broadcast**: `0` is the caller's whole
/// process group, `-1` is every process it may signal, and a negative id is
/// another group. None of those is a process this walk found, and any of them
/// would take down far more than the run — the launcher's own group includes
/// whatever started it. The walk's ids come from parsing a `ps` listing, which
/// is external input, so the guard is here at the one place a signal is sent
/// rather than only where the ids are read.
///
/// The answer is what makes the teardown's own answer honest. `ESRCH` — no such
/// process — is the outcome that was wanted: it exited between the listing and
/// the signal. Anything else, `EPERM` above all, is a process still running that
/// this user may not touch, and a teardown reporting that as reached would be
/// claiming a completion it was refused.
#[cfg(unix)]
fn signal_one(pid: u32, signal: i32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    if raw <= 0 {
        return false;
    }
    // SAFETY: `kill` takes a pid and a signal number and touches no memory this
    // call owns. `raw` is positive, so this addresses one process.
    if unsafe { libc::kill(raw, signal) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

/// Every process descended from `pid`, however deep, or `None` when this host
/// would not say.
///
/// The two are not the same answer and the caller must not read them as one: an
/// empty set means the process started nothing, while `None` means the tree is
/// unknown and anything under it is about to be orphaned.
///
/// Walked over a single snapshot, in whatever order the frontier gives — the
/// caller needs the *set*. A pid already found is never queued again, which is
/// what makes a table reporting a cycle terminate rather than hang a teardown.
#[cfg(unix)]
fn descendants(pid: u32) -> Option<Vec<u32>> {
    let table = process_table()?;
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
    Some(found)
}

/// This host's `(pid, parent pid)` pairs, or `None` when it gave no listing this
/// can be trusted to have read.
///
/// Through `ps`, the one answer every Unix gives — Linux has `/proc` and macOS
/// does not, and a second implementation is a platform fixed in only one of them.
///
/// External input deciding who gets signalled, so nothing about it is read
/// leniently. A `ps` that cannot run, that exits non-zero, that writes bytes
/// this cannot decode, or that writes a row this cannot read is not a listing:
/// the answer is `None`, and the caller is told the tree is unknown rather than
/// handed part of one. A dropped row could be the descendant that matters, and
/// the whole point of the walk is that the process it misses is the expensive
/// one.
#[cfg(unix)]
fn process_table() -> Option<Vec<(u32, u32)>> {
    let listed = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !listed.status.success() {
        return None;
    }
    parse_table(&String::from_utf8(listed.stdout).ok()?)
}

/// The `(pid, parent pid)` pairs a listing holds, or `None` if any line of it is
/// not one.
///
/// Separate from running `ps` so the rows can be read without a process and
/// without a `PATH`: what a listing may contain is a question about text, and
/// answering it by rewriting this process's environment would race every other
/// test that spawns something.
///
/// `pid=,ppid=` suppresses the headers, so every non-blank line is meant to be
/// exactly two ids and anything else means this is not the listing that was
/// asked for. A row claiming pid `0` counts as unreadable too: no process a
/// teardown may signal has that id — to `kill` it means the caller's whole
/// process group — so a listing offering one is not describing this host.
#[cfg(unix)]
fn parse_table(listed: &str) -> Option<Vec<(u32, u32)>> {
    listed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut columns = line.split_whitespace();
            let pid: u32 = columns.next()?.parse().ok()?;
            let parent: u32 = columns.next()?.parse().ok()?;
            (columns.next().is_none() && pid != 0).then_some((pid, parent))
        })
        .collect()
}

/// `how` is deliberately unread here; see the note on `/F` below.
#[cfg(windows)]
fn platform_stop(pid: u32, _how: Stop) -> Teardown {
    // `/T` for the tree — the same boundary the Unix arm walks the process table
    // for, which this platform offers outright.
    //
    // `/F` in **both** modes, which is not the distinction the other platform
    // draws, and the reason is a property of this one. Without `/F` `taskkill`
    // asks by posting `WM_CLOSE` to the target's top-level windows, and nothing
    // a run is made of has one: the driver, the graph it starts, and the harness
    // under that are console processes. Asked that way they answer `This process
    // can only be terminated forcefully (with /F option)` and every one of them
    // keeps running — which leaves a stop on this platform with no outcome that
    // is not a false report. Calling that a reached tree is the completion
    // nobody achieved that this seam exists to remove; calling it an unreached
    // one refuses every stop the platform can make. The test named
    // `a_polite_taskkill_cannot_end_a_console_process` holds that fact against
    // the real program.
    //
    // Nothing is given up by asking forcefully. The polite mode's promise is
    // that a process may record its own abandonment first, and it is `SIGTERM`
    // that carries it — a signal whose default action is to terminate, which
    // nothing in this crate installs a handler for. So the grace this drops is
    // grace no process here was taking.
    let ran = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    taskkill_established(ran, || platform_process_may_be_live(pid))
}

/// What a run of `taskkill /T /F` established about the tree it was aimed at.
///
/// Separate from running it so all four answers can be proved without a process:
/// what a `taskkill` that ran and *failed* means is a question about this
/// mapping alone, and one of its cases is not one a suite can produce on demand
/// — it needs a process this user may not end, which is not a thing to go and
/// make.
///
/// The failure is read from `still_live` rather than from the exit status, and
/// that is the whole point of this mapping. `taskkill` answers **the same**
/// non-zero status for a process it was refused and for one that was not there
/// to end — `128` for both on the host
/// `a_taskkill_failure_does_not_say_which_failure_it_was` checks it on. So the
/// status can say *that* the teardown did not complete cleanly and can never say
/// which of the two it was, and the two call for opposite answers: a stop that
/// raced its own tree to exit reached everything there was to reach, while one
/// that was refused left a process running. Asking the platform afterwards
/// answers the question the variant actually poses, where reading the status
/// only guesses at it.
///
/// This platform reaches [`Teardown::PartlySignalled`] deliberately, rather than
/// leaving it a variant only the Unix arm constructs: `taskkill /T` ends the
/// tree as it walks it, so a run of it that was refused has in general ended
/// part of that tree, which is the one thing [`Teardown::NotAttempted`] promises
/// is not so. The only Windows outcome that establishes "nothing was touched" is
/// a `taskkill` that never ran at all.
///
/// The liveness asked about is the **root's**, which is the pid the ledger holds
/// and the only handle a later stop has. A root that is gone took its tree's
/// reachability with it — a descendant that outlived it is beyond descent on
/// either platform — so what this cannot separate is a refusal deeper in a tree
/// whose root died anyway, and no answer available here would give an operator a
/// different next step for it.
#[cfg(windows)]
fn taskkill_established(
    ran: std::io::Result<std::process::ExitStatus>,
    still_live: impl FnOnce() -> bool,
) -> Teardown {
    match ran {
        // `taskkill` walks the tree itself, so a run of it that succeeded
        // reached the same boundary the Unix arm enumerates.
        Ok(status) if status.success() => Teardown::Signalled,
        // It never ran, so nothing in the tree was touched — the same answer,
        // and for the same reason, as a `ps` that will not answer.
        Err(_) => Teardown::NotAttempted,
        // It ran and did not complete. Still there is a process this teardown
        // was refused; gone is the race every teardown runs against its own
        // tree, and the rule the Unix arm applies to `ESRCH`: nothing left to
        // reach is reached.
        Ok(_) if still_live() => Teardown::PartlySignalled,
        Ok(_) => Teardown::Signalled,
    }
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

/// When a process started, as an opaque token this host can compare a later
/// reading against.
///
/// The half of a liveness proof a pid alone cannot give. A pid is reused, so a
/// record naming one is evidence about the process that took it only while that
/// process is still the one holding it — and a record that has been sitting on
/// disk for two days is exactly where that stops being true. Recorded beside the
/// pid and compared for equality afterwards: the same host, asking the same way,
/// gets the same answer for the same process and a different one for its
/// successor.
///
/// Opaque on purpose — never parsed, never ordered, never rendered as a time.
/// What makes it a proof is that two readings agree, not what either one says.
///
/// `None` is "this host would not say", which is **neither** verdict: a caller
/// has an unproven row rather than a live one or a dead one, and reporting it as
/// either is the misreading this exists to stop.
pub fn process_start_token(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    platform_process_start_token(pid)
}

/// Through `ps`, for the same reason [`process_table`] is: Linux has `/proc` and
/// macOS does not, and a second implementation is a platform fixed in only one
/// of them. `lstart` is the process's own start time, which the kernel fixes
/// when the process is created and nothing afterwards changes.
///
/// Read strictly. A `ps` that cannot run, exits non-zero, or writes bytes this
/// cannot decode is not an answer, and neither is an empty line — a token
/// nothing produced would compare equal to another one nothing produced, which
/// would make two different processes prove each other.
#[cfg(unix)]
fn platform_process_start_token(pid: u32) -> Option<String> {
    let listed = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !listed.status.success() {
        return None;
    }
    let token = String::from_utf8(listed.stdout).ok()?.trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// The creation time this platform keeps on the process itself, which is the
/// same fact `lstart` reports on the other one.
#[cfg(windows)]
fn platform_process_start_token(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: `OpenProcess` returns a null handle on failure and a handle this
    // function closes on success; no borrowed memory crosses the boundary.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    // SAFETY: `handle` is a live handle and every out-parameter is a `FILETIME`
    // this frame owns for the duration of the call.
    let read = unsafe {
        GetProcessTimes(
            handle,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    };
    // SAFETY: the handle came from `OpenProcess` above and is closed once.
    unsafe { CloseHandle(handle) };
    (read != 0).then(|| format!("{}:{}", created.dwHighDateTime, created.dwLowDateTime))
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

    /// The two things a start token has to do to be a proof: give one process
    /// the same answer twice, and give no answer at all for a pid nothing holds.
    ///
    /// The second is what stops a record two days old from proving a live
    /// dispatch: a pid the kernel has handed to something else answers with that
    /// process's start, which is not the one the record was written against.
    #[test]
    fn a_start_token_is_stable_for_one_process_and_absent_for_a_pid_nothing_holds() {
        let mine = process_start_token(pid()).expect("this host says when a process started");
        assert!(!mine.is_empty());
        assert_eq!(
            process_start_token(pid()).as_deref(),
            Some(mine.as_str()),
            "one process gave two different start tokens"
        );
        let dead = reaped_pid();
        assert!(
            process_start_token(dead).is_none(),
            "pid {dead} was reaped and still answered with a start"
        );
        assert!(process_start_token(0).is_none());
    }

    /// A stop reaches the leaf, not just the process whose pid it was given.
    ///
    /// The tree here is the shape a run actually makes — a driver, the graph it
    /// starts, and the paid agent that graph's harness starts. Every process is
    /// real and every pid is read off the kernel rather than assumed.
    ///
    /// The boundary is pinned from both sides by where the two bystanders sit.
    /// The tree is given a **process group of its own**, led by its root, and one
    /// bystander is started inside that group without being descended from it:
    /// a teardown that swept the group — the defect this exists to fix, which
    /// reached the group and left the leaf running — takes that bystander and is
    /// caught by it. The other bystander is outside the group as well as outside
    /// the tree, so a teardown that widened past the group is caught too. What
    /// gets through both is descent, which is the boundary [`stop`] promises.
    ///
    /// The group is made here rather than by the leaf detaching itself, which is
    /// what the real paid process does. That reads better and cost this suite
    /// the platform it most needed: detaching means `setsid`, `setsid(1)` is
    /// util-linux's, and macOS does not ship one — so the third level never
    /// started there, silently, and the whole tree this journey is about went
    /// unproven on the platform whose consumers were still being orphaned.
    /// `setpgid` through [`std::os::unix::process::CommandExt::process_group`]
    /// is POSIX, needs no program on the host, and leaves `stop` answering the
    /// same question.
    ///
    /// Unix-only because the tree is built with `sh` and read back through the
    /// process table. The Windows arm hands the same boundary to `taskkill /T`,
    /// which has always walked it.
    ///
    /// Driven for **each** mode by the two tests below rather than for one of
    /// them, because the mode decides the signal and a signal decides whether a
    /// process that is ignoring `SIGTERM` actually goes: asserting the tree only
    /// under `SIGKILL` would leave the polite walk — the one an operator's `stop`
    /// takes — proved nowhere.
    #[cfg(unix)]
    fn a_stop_reaches_the_whole_descendant_tree_and_not_the_process_beside_it(how: Stop) {
        use std::os::unix::process::CommandExt;

        // Each level prints its pid and then waits, so the test learns the tree
        // from the kernel. `exec 2>&1` folds the tree's own diagnostics into the
        // stream those pids arrive on: a level that cannot start is then a line
        // this fails on and quotes, rather than a level that never appears and
        // says nothing about why.
        let mut tree = std::process::Command::new("sh")
            .args([
                "-c",
                "exec 2>&1; echo $$; sh -c 'echo $$; sh -c \"echo \\$\\$; sleep 120\" & \
                 sleep 120' & sleep 120",
            ])
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("a process tree");
        let group = i32::try_from(tree.id()).expect("a pid is a process group id");
        // A process started beside the tree rather than under it, and outside
        // its process group as well. A teardown that widened past the group
        // would take this too.
        let mut beside = std::process::Command::new("sh")
            .args(["-c", "sleep 120"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("a process beside the tree");
        // And one in the tree's own process group that the tree did not start.
        // This is the one a group teardown cannot avoid: it is what the group
        // holds beyond the run, and ending it would be ending work this run does
        // not own.
        let mut beside_in_group = std::process::Command::new("sh")
            .args(["-c", "sleep 120"])
            .process_group(group)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("a process in the tree's process group");

        let mut pids = Vec::new();
        {
            use std::io::BufRead;
            let out = std::io::BufReader::new(tree.stdout.take().expect("the tree reports itself"));
            for line in out.lines().take(3) {
                let line = line.expect("a reported pid");
                pids.push(
                    line.trim()
                        .parse::<u32>()
                        .unwrap_or_else(|_| panic!("the tree said {line:?} where a pid was due")),
                );
            }
        }
        assert_eq!(pids.len(), 3, "the tree did not report three levels");
        assert!(
            pids.iter().all(|pid| process_may_be_live(*pid)),
            "the tree was not running before it was stopped: {pids:?}"
        );

        stop(pids[0], how);
        // Reaped rather than waited on: a signalled child nobody has collected
        // is a zombie and a zombie answers a liveness probe as alive, but a
        // *blocking* wait on a root the stop missed would return only when the
        // fixture finished on its own — turning this journey's own failure into
        // a slow pass. The two below it are this process's grandchildren, so
        // nothing here can collect those; init does.
        let patience = std::time::Duration::from_secs(10);
        let reaped = ended_within(&mut tree, patience);
        let deadline = std::time::Instant::now() + patience;
        while std::time::Instant::now() < deadline
            && pids.iter().any(|pid| process_may_be_live(*pid))
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            reaped,
            "the stop never reached the root of the tree {pids:?}"
        );

        let surviving: Vec<u32> = pids
            .iter()
            .copied()
            .filter(|pid| process_may_be_live(*pid))
            .collect();
        assert!(
            surviving.is_empty(),
            "a stop left {surviving:?} of the tree {pids:?} running — the leaf is the paid one"
        );
        // Asked of the child handle rather than of [`process_may_be_live`], and
        // that is what makes these two assertions able to fail at all: these
        // bystanders are this process's own children, so one a stop signalled is
        // a zombie nobody has collected, and a zombie answers a liveness probe as
        // alive. Read that way, a teardown that took them both would still be
        // reported here as having left them alone.
        assert!(
            still_running(&mut beside),
            "a stop took a process that was beside the tree rather than under it"
        );
        assert!(
            still_running(&mut beside_in_group),
            "a stop took a process that shared the tree's process group without being descended \
             from it — the boundary a teardown ends is descent, not the group"
        );
        for bystander in [&mut beside, &mut beside_in_group] {
            let _ = bystander.kill();
            let _ = bystander.wait();
        }
    }

    /// The mode an operator's `stop` takes.
    #[cfg(unix)]
    #[test]
    fn a_polite_stop_reaches_the_whole_descendant_tree() {
        a_stop_reaches_the_whole_descendant_tree_and_not_the_process_beside_it(Stop::Politely);
    }

    /// The mode a cancelled dispatch takes, where the leaf is the paid process.
    #[cfg(unix)]
    #[test]
    fn a_forceful_stop_reaches_the_whole_descendant_tree() {
        a_stop_reaches_the_whole_descendant_tree_and_not_the_process_beside_it(Stop::Now);
    }

    /// Whether a child of this process has not ended, without waiting for it to.
    ///
    /// The bystanders' oracle. `try_wait` reaps, so it separates a process that
    /// is still running from one that was signalled and is lying about it as a
    /// zombie — which [`process_may_be_live`] cannot do, because a zombie is
    /// exactly a pid `kill(pid, 0)` still succeeds on.
    #[cfg(unix)]
    fn still_running(child: &mut std::process::Child) -> bool {
        matches!(child.try_wait(), Ok(None))
    }

    /// Whether `child` ends inside `patience`, reaping it if it does.
    ///
    /// Polled rather than waited on, and that is the point: a blocking `wait`
    /// on a process the stop failed to signal returns when the process finishes
    /// *on its own*, so the assertion after it passes — late, and for a reason
    /// that has nothing to do with the stop. Every one of these fixtures sleeps
    /// far longer than any teardown should take, so ending inside `patience` is
    /// only ever the signal landing.
    #[cfg(unix)]
    fn ended_within(child: &mut std::process::Child, patience: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + patience;
        while std::time::Instant::now() < deadline {
            // Reaped here, because a signalled child nobody has waited on is a
            // zombie and a zombie answers a liveness probe as alive.
            if matches!(child.try_wait(), Ok(Some(_))) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    /// A process that is already gone counts as reached; an id that is not a
    /// process never does.
    ///
    /// The distinction the teardown's own answer rests on. `ESRCH` means the
    /// process exited between the listing and the signal, which is the outcome
    /// that was wanted — treating it as a failure would make every ordinary race
    /// report an incomplete stop. A non-positive id is not a process at all: to
    /// `kill` it is a broadcast, so it is refused rather than sent, and refusing
    /// to send is not reaching anything.
    #[cfg(unix)]
    #[test]
    fn a_signal_reports_a_process_already_gone_as_reached_and_a_broadcast_as_not() {
        assert!(
            signal_one(reaped_pid(), libc::SIGTERM),
            "a process that had already exited was reported as unreached"
        );
        assert!(
            !signal_one(0, libc::SIGTERM),
            "pid 0 was reported as reached, and to `kill` it is a whole process group"
        );
    }

    /// A console process tree, and the pids of both its levels.
    ///
    /// `cmd` runs `ping`, so the root has a real descendant to be reached
    /// through — the shape a run makes, and the shape a teardown that stopped at
    /// the root would leave half of. Both levels are console processes with no
    /// window between them, which is the property every Windows fact below turns
    /// on.
    ///
    /// The child's pid is read through `Win32_Process`, which is this test's own
    /// oracle and deliberately not the crate's: `platform_stop` hands the tree to
    /// `taskkill /T` and never enumerates one, so a fixture that asked the crate
    /// where the leaf was would be asking the code under test to grade itself.
    #[cfg(windows)]
    fn console_tree() -> (std::process::Child, u32) {
        let mut root = std::process::Command::new("cmd")
            .args(["/C", "ping -n 120 127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("a console process tree");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut leaf = None;
        while leaf.is_none() && std::time::Instant::now() < deadline {
            leaf = child_of(root.id());
            if leaf.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        match leaf {
            Some(leaf) => (root, leaf),
            None => {
                let pid = root.id();
                let _ = root.kill();
                let _ = root.wait();
                panic!("the tree under {pid} never started its leaf");
            }
        }
    }

    /// The pid of one child of `parent`, or `None` while it has none.
    #[cfg(windows)]
    fn child_of(parent: u32) -> Option<u32> {
        let listed = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "(Get-CimInstance Win32_Process -Filter 'ParentProcessId={parent}').ProcessId"
                ),
            ])
            .output()
            .expect("this host lists its processes");
        String::from_utf8_lossy(&listed.stdout)
            .lines()
            .find_map(|line| line.trim().parse::<u32>().ok())
    }

    /// Whether every pid in `tree` is gone inside `patience`.
    #[cfg(windows)]
    fn all_ended_within(tree: &[u32], patience: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + patience;
        while std::time::Instant::now() < deadline {
            if tree.iter().all(|pid| !platform_process_may_be_live(*pid)) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }

    /// `taskkill` without `/F` cannot end a console process, and says so.
    ///
    /// The fact `platform_stop` asks forcefully in **both** modes on the
    /// strength of. The polite ask is `WM_CLOSE` to a top-level window, and a
    /// run is made entirely of processes that have none — so asking that way
    /// ends nothing at all, rather than ending it less abruptly. Held against
    /// the real program because it is the whole reason this platform does not
    /// draw the distinction the other one does, and a reasoned answer to it was
    /// wrong once already.
    #[cfg(windows)]
    #[test]
    fn a_polite_taskkill_cannot_end_a_console_process() {
        let (mut root, leaf) = console_tree();
        let pid = root.id();

        let asked = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T"])
            .output()
            .expect("taskkill runs");
        let said = String::from_utf8_lossy(&asked.stderr).into_owned();
        assert!(
            !asked.status.success(),
            "a polite taskkill reported that it ended a console tree: {said}"
        );
        assert!(
            platform_process_may_be_live(pid) && platform_process_may_be_live(leaf),
            "the polite ask ended part of the console tree {pid}/{leaf} after all, so the \
             forceful ask below is not the only one that reaches it: {said}"
        );

        assert_eq!(
            stop(pid, Stop::Now),
            Teardown::Signalled,
            "the forceful ask did not reach the tree the polite one could not"
        );
        assert!(all_ended_within(
            &[pid, leaf],
            std::time::Duration::from_secs(10)
        ));
        let _ = root.wait();
    }

    /// A failed `taskkill` does not say *which* failure it was, so the answer
    /// cannot be read from its status.
    ///
    /// The reason [`taskkill_established`] asks the platform what is still
    /// running instead. These two outcomes call for opposite answers — a tree
    /// that had already exited was wholly reached, a process this teardown was
    /// refused was not — and this holds them side by side to show the exit
    /// status is the same for both. Asserted as an equality rather than against
    /// the literal `128`, because what makes the status unusable is that it does
    /// not separate them, not the particular number it collapses them onto.
    #[cfg(windows)]
    #[test]
    fn a_taskkill_failure_does_not_say_which_failure_it_was() {
        let (mut root, leaf) = console_tree();
        let pid = root.id();

        // A process still running that this ask cannot end.
        let refused = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T"])
            .output()
            .expect("taskkill runs");
        assert!(
            platform_process_may_be_live(pid),
            "the tree ended by itself"
        );

        // And one this host has proved is gone. Proved *before* the ask, not
        // after: a pid the host had already reissued would make this fixture
        // send a forceful teardown at whatever now holds it.
        let dead = reaped_pid();
        assert!(
            !platform_process_may_be_live(dead),
            "the reaped pid {dead} was still live"
        );
        let absent = std::process::Command::new("taskkill")
            .args(["/PID", &dead.to_string(), "/T", "/F"])
            .output()
            .expect("taskkill runs");

        assert!(
            !refused.status.success() && !absent.status.success(),
            "a taskkill reported success for a tree it did not end"
        );
        assert_eq!(
            refused.status.code(),
            absent.status.code(),
            "a taskkill that was refused a running process and one that found nothing to end \
             report different statuses, so the teardown could read the difference off the \
             status after all"
        );

        stop(pid, Stop::Now);
        assert!(all_ended_within(
            &[pid, leaf],
            std::time::Duration::from_secs(10)
        ));
        let _ = root.wait();
    }

    /// A stop reaches the leaf, not just the process whose pid it was given —
    /// on this platform too.
    ///
    /// The Windows half of the journey the Unix arm proves by walking a process
    /// table, and the one this fix is for: until it, a stop here ended nothing,
    /// because the only ask it made was one no console process could receive.
    /// Driven for **each** mode, because the mode used to decide whether the ask
    /// was deliverable at all, and the polite one — the mode an operator's `stop`
    /// takes — is the one that reached nothing.
    ///
    /// There is no bystander here as there is on Unix. `taskkill /T` is handed
    /// descent rather than given a set this walked, so what pins the boundary on
    /// this platform is the program's own contract, and the tree is what the
    /// teardown has to be shown to reach.
    #[cfg(windows)]
    fn a_stop_reaches_the_whole_console_tree(how: Stop) {
        let (mut root, leaf) = console_tree();
        let pid = root.id();
        assert!(
            platform_process_may_be_live(pid) && platform_process_may_be_live(leaf),
            "the tree {pid}/{leaf} was not running before it was stopped"
        );

        assert_eq!(
            stop(pid, how),
            Teardown::Signalled,
            "a stop that reached the tree {pid}/{leaf} did not report reaching it"
        );
        assert!(
            all_ended_within(&[pid, leaf], std::time::Duration::from_secs(10)),
            "a stop left part of the tree {pid}/{leaf} running — the leaf is the paid one"
        );
        let _ = root.wait();
    }

    /// The mode an operator's `stop` takes.
    #[cfg(windows)]
    #[test]
    fn a_polite_stop_reaches_the_whole_console_tree() {
        a_stop_reaches_the_whole_console_tree(Stop::Politely);
    }

    /// The mode a cancelled dispatch takes, where the leaf is the paid process.
    #[cfg(windows)]
    #[test]
    fn a_forceful_stop_reaches_the_whole_console_tree() {
        a_stop_reaches_the_whole_console_tree(Stop::Now);
    }

    /// A teardown aimed at a tree that has already gone reports a complete one.
    ///
    /// The ordinary race, driven through the real program rather than the
    /// mapping: `taskkill` fails because there is nothing left to end, and the
    /// stop that raced its own tree to exit reached everything there was to
    /// reach. Reporting it as partial would send an operator to hunt a process
    /// that does not exist.
    #[cfg(windows)]
    #[test]
    fn a_stop_aimed_at_a_tree_that_has_already_gone_is_a_complete_teardown() {
        let dead = reaped_pid();
        assert!(
            !platform_process_may_be_live(dead),
            "the reaped pid {dead} was still live, so this is not the race under test"
        );
        assert_eq!(
            stop(dead, Stop::Politely),
            Teardown::Signalled,
            "a stop that raced its own tree to exit was reported as having left one running"
        );
    }

    /// The four answers a `taskkill` can establish, including both directions of
    /// the one its exit status cannot tell apart.
    ///
    /// The seam is a mapping so the case that matters is provable at all: a
    /// teardown genuinely refused a process needs one this user may not end, and
    /// going and making one would be a worse thing than the bug being checked
    /// for. Driven here instead, from the same failed status the ordinary race
    /// produces, so the two are separated by the one thing that does separate
    /// them — whether the process is still there.
    #[cfg(windows)]
    #[test]
    fn a_failed_taskkill_is_read_from_what_is_still_running_not_from_its_status() {
        use std::os::windows::process::ExitStatusExt;

        let exited = |code: u32| Ok(std::process::ExitStatus::from_raw(code));
        let never_asked = || panic!("liveness was asked about a teardown that settled without it");
        assert_eq!(
            taskkill_established(exited(0), never_asked),
            Teardown::Signalled,
            "a taskkill that walked the tree was not reported as having reached it"
        );
        assert_eq!(
            taskkill_established(
                Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
                never_asked
            ),
            Teardown::NotAttempted,
            "a taskkill that never ran was reported as having touched the tree"
        );
        // The same status, and the opposite answer, on the strength of what the
        // platform says afterwards.
        assert_eq!(
            taskkill_established(exited(128), || true),
            Teardown::PartlySignalled,
            "a teardown that left a process running was reported as a clean stop"
        );
        assert_eq!(
            taskkill_established(exited(128), || false),
            Teardown::Signalled,
            "a tree that was already gone was reported as a process still to be found"
        );
    }

    /// A listing this cannot read in full is not a listing it may act on.
    ///
    /// `pid=,ppid=` suppresses the headers, so every non-blank line is meant to
    /// be exactly two ids. Anything else means the answer is not the one that
    /// was asked for, and the caller is told the tree is unknown rather than
    /// handed part of one — a row that was dropped could be the descendant that
    /// matters, and the process a teardown misses is the expensive one.
    #[cfg(unix)]
    #[test]
    fn a_listing_with_a_row_it_cannot_read_is_no_listing_at_all() {
        assert_eq!(
            parse_table("11 10\n13 11\n"),
            Some(vec![(11, 10), (13, 11)]),
            "a listing every line of which is two ids was not read"
        );
        for unreadable in [
            "11 10\nnot-a-pid also-not\n13 11\n",
            "11 10\n14\n",
            "  PID  PPID\n11 10\n",
            "11 10 and-a-third\n",
        ] {
            assert_eq!(
                parse_table(unreadable),
                None,
                "a listing holding {unreadable:?} was read as a tree anyway"
            );
        }
    }

    /// Blank lines are not rows and cost nothing.
    #[cfg(unix)]
    #[test]
    fn a_blank_line_is_not_a_row_it_failed_to_read() {
        assert_eq!(
            parse_table("11 10\n\n13 11\n   \n"),
            Some(vec![(11, 10), (13, 11)])
        );
    }

    /// A listing that claims pid `0` is not describing this host.
    ///
    /// `kill(0, ...)` is not "no process" — it is the caller's **entire process
    /// group**, which here means the launcher and whatever it was started
    /// beside. The ids come from parsing external input, so a row claiming `0`
    /// is a row that would turn a teardown of one tree into a broadcast, and a
    /// listing offering one is not one this may act on.
    #[cfg(unix)]
    #[test]
    fn a_listing_that_claims_pid_zero_is_not_acted_on() {
        assert_eq!(
            parse_table("0 7\n7 1\n"),
            None,
            "a listing claiming pid 0 was read as a tree"
        );
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
