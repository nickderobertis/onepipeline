//! The host facts the engine reads: the clock, process liveness, and who is
//! asking.
//!
//! Everything here is deliberately small and total. A run's ledger records
//! timestamps and pids, and every view that reports `DRIVER DEAD` or `PARKED`
//! resolves them through this module, so an unanswerable question resolves
//! toward "still working" here rather than in each caller.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
/// Four outcomes because they call for four different things from the caller,
/// and collapsing any two of them is how a stop reports a completion nobody
/// achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Teardown {
    /// A tree was there and every process in it was reached.
    ///
    /// Signalled, not *proved gone*: `kill` reports that the signal was
    /// delivered, and a process may take a moment over it. What this rules out
    /// is the failure that matters — a process nobody aimed at.
    /// [`stop_and_confirm`] is the liveness probe that settles the rest, and a
    /// caller that reports a stop to a person has to run it: a teardown that
    /// signalled a tree still standing is a run the operator has been told is
    /// over.
    Signalled,
    /// There was **nothing to aim at**: every process the walk named was
    /// already gone, so no signal reached anything.
    ///
    /// Deliberately not [`Signalled`](Self::Signalled), which is what it was
    /// once folded into. The two are opposite answers to the question a caller
    /// is actually asking — *did this end the run's work?* — and reading "there
    /// was nothing running" as "the tree was reached" is how a `stop` reports
    /// having ended a dispatch it never found. Neither is a failure: a run whose
    /// work is already over has nothing left to end.
    NothingToStop,
    /// The teardown never began — this host gave no listing the tree could be
    /// read from, or the program that ends it could not be run — so **nothing**
    /// was signalled and the run is exactly as it was.
    ///
    /// Deliberately not a half-teardown: a descendant is reparented the moment
    /// its parent dies, so signalling the root alone would put everything under
    /// it permanently beyond descent — the only handle a later stop has on them.
    /// Untouched, the same ask works once the host answers.
    NotAttempted,
    /// The teardown began and at least one process in the tree is still
    /// running: one it could not signal, or — where the caller asked for the
    /// probe — one it signalled that has not gone.
    ///
    /// The run is *not* untouched and retrying will not necessarily help: what
    /// is left is a process this user may not touch, or one that took the ask
    /// and stayed, and either way it is still running. The caller has to say so
    /// rather than report any of the other three.
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
    platform_stop(&[pid], how).0
}

/// Ask several trees to stop **together**, and then watch until they are gone.
///
/// Two things [`stop`] leaves to its caller. *Together*, because one listing
/// decides every tree: a root signalled before the next is walked has already
/// reparented its children beyond descent. *Watched*, because
/// [`Teardown::Signalled`] is a delivered signal rather than a process that has
/// exited — and what is watched is the set that was aimed at, read before
/// anything was signalled, since afterwards there is no tree left to descend. A
/// tree still standing when `patience` runs out is
/// [`Teardown::PartlySignalled`].
pub fn stop_and_confirm(pids: &[u32], how: Stop, patience: Duration) -> Teardown {
    let (established, aimed) = platform_stop(pids, how);
    if established != Teardown::Signalled {
        return established;
    }
    if gone_within(&aimed, patience) {
        Teardown::Signalled
    } else {
        Teardown::PartlySignalled
    }
}

/// Whether every process in `aimed` is gone before `patience` runs out.
///
/// Polled rather than waited on: these are not this process's children, so there
/// is nothing to wait for — the only question a host answers about somebody
/// else's process is whether it is still there.
fn gone_within(aimed: &[u32], patience: Duration) -> bool {
    // Saturating rather than added: `patience` is a caller's value, and an
    // instant that cannot be represented is a wait this platform cannot end
    // early rather than a reason to take the process down.
    let deadline = Instant::now().checked_add(patience);
    loop {
        if !aimed.iter().any(|pid| process_may_be_live(*pid)) {
            return true;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return false;
        }
        std::thread::sleep(PROBE_POLL);
    }
}

/// How often the liveness probe asks again.
///
/// Short enough that an ordinary teardown returns as soon as its tree is gone
/// rather than at some interval's convenience, and long enough that a stop
/// waiting out a wedged process is not a busy loop against `kill`.
const PROBE_POLL: Duration = Duration::from_millis(20);

/// The roots a teardown may aim at, in the order they are signalled.
///
/// `0` is no process, and this process is one a teardown must never turn on
/// itself: a stop is called from the command doing the stopping. Neither is a
/// tree left unreached, which is what the answer is about — a set that empties
/// to nothing here is [`Teardown::NothingToStop`].
fn aimable(roots: &[u32]) -> Vec<u32> {
    let mut aimed: Vec<u32> = Vec::new();
    for root in roots
        .iter()
        .copied()
        .filter(|pid| *pid != 0 && *pid != self::pid())
    {
        if !aimed.contains(&root) {
            aimed.push(root);
        }
    }
    aimed
}

#[cfg(unix)]
fn platform_stop(roots: &[u32], how: Stop) -> (Teardown, Vec<u32>) {
    let signal = match how {
        Stop::Politely => libc::SIGTERM,
        Stop::Now => libc::SIGKILL,
    };
    let mut aimed = aimable(roots);
    if aimed.is_empty() {
        return (Teardown::NothingToStop, aimed);
    }
    // The table is read **before** anything is signalled: a process whose parent
    // has died is reparented at once, so a table read after any root is gone no
    // longer descends to what was under it.
    let Some(table) = process_table() else {
        return (Teardown::NotAttempted, Vec::new());
    };
    // The roots first, so what is left has stopped growing while its members are
    // taken down.
    for root in aimed.clone() {
        for descendant in descended_from(&table, root) {
            if !aimed.contains(&descendant) {
                aimed.push(descendant);
            }
        }
    }
    // Every answer is kept: one process this user may not signal is one still
    // running, and a teardown that reported the tree as reached anyway would be
    // the same false completion in a smaller place. And a walk that found
    // nothing but processes already gone reached no tree at all, which is the
    // answer a caller reports as such rather than as a stop it made.
    let mut refused = false;
    let mut delivered = false;
    for pid in &aimed {
        match signal_one(*pid, signal) {
            Reached::Delivered => delivered = true,
            Reached::Absent => {}
            Reached::Refused => refused = true,
        }
    }
    let established = match (refused, delivered) {
        (true, _) => Teardown::PartlySignalled,
        (false, true) => Teardown::Signalled,
        (false, false) => Teardown::NothingToStop,
    };
    (established, aimed)
}

/// What one process did with the signal it was sent.
///
/// Three answers rather than a `bool`, because the teardown's own answer is
/// built from them and two of them used to be one: a process that took the
/// signal and one that was no longer there both count as *reached*, and only the
/// first is a tree this stop ended. Folding them together is what let a teardown
/// that found nothing report the same outcome as one that ended a run.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reached {
    /// This user's signal was delivered to a process that was there.
    Delivered,
    /// There was no such process: it exited between the listing and the signal,
    /// or the walk named an id nothing holds.
    Absent,
    /// A process still running that this teardown may not signal.
    Refused,
}

/// Signal one process, and refuse every id that is not one.
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
/// process — is not a failure: it exited between the listing and the signal, and
/// there is nothing left in it to end. It is not a stop this teardown made
/// either, which is why it is [`Reached::Absent`] and not
/// [`Reached::Delivered`]. Anything else, `EPERM` above all, is a process still
/// running that this user may not touch, and a teardown reporting that as
/// reached would be claiming a completion it was refused.
#[cfg(unix)]
fn signal_one(pid: u32, signal: i32) -> Reached {
    let Ok(raw) = i32::try_from(pid) else {
        return Reached::Refused;
    };
    if raw <= 0 {
        return Reached::Refused;
    }
    // SAFETY: `kill` takes a pid and a signal number and touches no memory this
    // call owns. `raw` is positive, so this addresses one process.
    if unsafe { libc::kill(raw, signal) } == 0 {
        return Reached::Delivered;
    }
    if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        return Reached::Absent;
    }
    Reached::Refused
}

/// Every process descended from `pid` in one listing, however deep.
///
/// Takes the table rather than reading one, because a teardown aimed at more
/// than one root must walk them all over the **same** snapshot: a table read
/// again after another root has been signalled describes a host where that
/// root's children have already been reparented away.
///
/// An empty set means the process started nothing. That a host would not say at
/// all is the caller's question — [`process_table`] answers `None` for it — and
/// the two must never be read as one, because anything under an unknown tree is
/// about to be orphaned.
///
/// Walked in whatever order the frontier gives — the caller needs the *set*. A
/// pid already found is never queued again, which is what makes a table
/// reporting a cycle terminate rather than hang a teardown.
#[cfg(unix)]
fn descended_from(table: &[(u32, u32)], pid: u32) -> Vec<u32> {
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
///
/// The roots are filtered to the ones this host says are still there **before**
/// any of them is asked to end, which is this platform's answer to the question
/// the Unix arm reads off `ESRCH`: a `taskkill` aimed at a tree that is already
/// gone reached nothing, and reporting that as a stop is the false completion
/// this seam exists to remove.
// llmlint: ignore-block[changed_behavior_has_e2e] this arm is `#[cfg(windows)]`, so the only
// build that compiles it is the one that runs the suite's journeys on Windows — and there they
// are its e2e: `the_owner_stops_its_own_run_without_force` and
// `stopping_a_run_whose_work_is_over_says_there_was_nothing_to_stop` are deliberately not
// `#[cfg(unix)]`, and each drives a real `stop` of a run holding a driver and its registered
// dispatches, which is this fold over several roots. What is `#[cfg(unix)]` in
// `tests/e2e/driver.rs` is every journey that asserts against a **process table**, because that
// oracle is the platform's, not the teardown's. A journey here would be a Windows journey run on
// a host that is not one.
#[cfg(windows)]
fn platform_stop(roots: &[u32], _how: Stop) -> (Teardown, Vec<u32>) {
    let aimed: Vec<u32> = aimable(roots)
        .into_iter()
        .filter(|pid| platform_process_may_be_live(*pid))
        .collect();
    if aimed.is_empty() {
        return (Teardown::NothingToStop, aimed);
    }
    // Every tree is asked separately, because `taskkill` takes one root, and the
    // answers are folded the way a teardown of several trees has to be: one tree
    // untouched beside one that was signalled is a run that is neither intact nor
    // ended, which is what [`Teardown::PartlySignalled`] says.
    let mut walked = true;
    let mut attempted = false;
    for pid in &aimed {
        match taskkill_established(taskkill(*pid), || platform_process_may_be_live(*pid)) {
            Teardown::Signalled => attempted = true,
            Teardown::PartlySignalled => {
                attempted = true;
                walked = false;
            }
            Teardown::NotAttempted => walked = false,
            // `taskkill_established` never answers it: a root this teardown
            // aimed at was live when it was filtered above.
            Teardown::NothingToStop => {}
        }
    }
    let established = match (walked, attempted) {
        (true, _) => Teardown::Signalled,
        (false, true) => Teardown::PartlySignalled,
        (false, false) => Teardown::NotAttempted,
    };
    (established, aimed)
}
// llmlint: ignore-end[changed_behavior_has_e2e]

/// Ask this platform to end one tree.
///
/// Split out of [`platform_stop`] so that the fold over several roots above
/// reads as the fold it is: this is the one ask, and everything about *how* it
/// asks — `/T`, and `/F` in both modes — is the note inside it.
#[cfg(windows)]
fn taskkill(pid: u32) -> std::io::Result<std::process::ExitStatus> {
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
    std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
}

/// What a run of `taskkill /T /F` established about the tree it was aimed at.
///
/// Separate from running it so all three answers can be proved without a process:
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

/// What a host says about when one process started, kept as the opaque thing it
/// is.
///
/// A newtype rather than a `String` because there is exactly one operation on
/// it — asking whether a *recorded* token is this same process's — and every
/// other thing a string invites is a bug: it is not a time to parse, not an
/// order to sort by, and not text to render. The one comparison also carries the
/// rule that makes it a proof, which a bare `==` between two strings does not:
/// an **empty** recorded token never matches. Empty is what a lock written
/// before this field existed carries, and what a host that would not answer
/// leaves behind, and reading either as agreement would let two absences prove
/// each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartToken(String);

impl StartToken {
    /// The token as a record on disk carries it.
    pub fn recorded(&self) -> &str {
        &self.0
    }

    /// Whether a token recorded earlier is this same process's.
    ///
    /// The empty one is never anybody's, for the reason on the type.
    pub fn matches(&self, recorded: &str) -> bool {
        !recorded.is_empty() && self.0 == recorded
    }
}

/// When a process started, as an opaque token this host can compare a later
/// reading against.
///
/// The half of a liveness proof a pid alone cannot give. A pid is reused, so a
/// record naming one is evidence about the process that took it only while that
/// process is still the one holding it — and a record that has been sitting on
/// disk for two days is exactly where that stops being true. Recorded beside the
/// pid and compared for equality afterwards: the same host gets the same answer
/// for the same process and a different one for its successor.
///
/// The reading is a function of the **process**, never of who is reading it. One
/// process is written down by the driver that took the run's lock and read back
/// by whatever session goes looking, and those two share a host and nothing else
/// — not a working directory, not a login, not a `TZ`. A token that moved with
/// the reader's environment would make one live process disagree with itself,
/// which reads as a pid that has been handed to something else; see
/// [`platform_process_start_token`] for what that cost and how it is pinned.
///
/// Opaque on purpose — never parsed, never ordered, never rendered as a time.
/// What makes it a proof is that two readings agree, not what either one says.
///
/// `None` is "this host would not say", which is **neither** verdict: a caller
/// has an unproven row rather than a live one or a dead one, and reporting it as
/// either is the misreading this exists to stop.
pub fn process_start_token(pid: u32) -> Option<StartToken> {
    if pid == 0 {
        return None;
    }
    platform_process_start_token(pid).map(StartToken)
}

/// Through `ps`, for the same reason [`process_table`] is: Linux has `/proc` and
/// macOS does not, and a second implementation is a platform fixed in only one
/// of them. `lstart` is the process's own start time, which the kernel fixes
/// when the process is created and nothing afterwards changes.
///
/// **Asked in a fixed environment**, and that is what makes two readings
/// comparable at all. `lstart` is not a fact `ps` copies out; it is that fact
/// *rendered*, and every Unix renders it through the reader's own environment —
/// `localtime` for the zone, and on the BSDs `strftime("%c")` for the words. So
/// the same live process answers `Mon Aug 17 11:22:34 2026` to one reader and
/// `Mon Aug 17 07:22:34 2026` to another standing in a different `TZ`, and two
/// readings that disagree are what a caller comparing them reads as *a different
/// process*. That is not a hypothetical: a run adopted from one session and
/// looked at from another is two processes with two environments, and the view
/// that exists to say whether its dispatches are alive reported them dead.
/// Pinning the zone and the locale on the child leaves the reading a function of
/// the process alone, which is the property the token is for; the alternative —
/// a source rendered by nobody, `/proc/<pid>/stat` — exists on one of the two
/// platforms this crate supports, and a platform fixed in only one of them is
/// what having a single implementation here is worth avoiding.
///
/// Read strictly. A `ps` that cannot run, exits non-zero, or writes bytes this
/// cannot decode is not an answer, and neither is an empty line — a token
/// nothing produced would compare equal to another one nothing produced, which
/// would make two different processes prove each other. Neither is an answer of
/// **more than one line**: one process was asked about, and a host that wrote
/// anything beside its answer is one whose reading cannot be compared against a
/// reading taken when it was well. Folding that into one string would make a
/// live process disagree with its own recorded stamp, which a caller reads as a
/// pid handed to somebody else — the one verdict that must never come from the
/// host misbehaving rather than from the process ending.
#[cfg(unix)]
fn platform_process_start_token(pid: u32) -> Option<String> {
    let listed = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        // `LC_ALL` rather than `LC_TIME`, because it is the one that overrides
        // whatever else the reader's environment sets.
        .env("TZ", "UTC")
        .env("LC_ALL", "C")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !listed.status.success() {
        return None;
    }
    let answer = String::from_utf8(listed.stdout).ok()?;
    let mut lines = answer
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let token = lines.next()?.to_string();
    lines.next().is_none().then_some(token)
}

/// The creation time this platform keeps on the process itself, which is the
/// same fact `lstart` reports on the other one — asked together with whether the
/// process has actually exited, because here the creation time alone is not a
/// liveness proof.
///
/// A Windows process *object* outlives the process: while any handle to it is
/// still open, the pid keeps resolving and `GetProcessTimes` keeps answering
/// with the creation time of the run that has already ended. So the creation
/// time on its own says "a process by this pid was created then", not "a process
/// by this pid is running" — and a caller comparing it against a recorded token
/// would get a match for a dispatch that died two days ago. That is the exact
/// misreading this token exists to stop, so the exit is checked here rather than
/// left to the caller: a signalled handle means the process has terminated, and
/// a terminated process has no start to give.
///
/// The two rights are asked for together and a refusal of either is no answer at
/// all. Downgrading to whichever right was granted would hand back a creation
/// time this cannot pair with an exit check, which is the unproven half on its
/// own — and `None` is already the honest way to say a host will not answer.
#[cfg(windows)]
fn platform_process_start_token(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE,
    };

    // SAFETY: `OpenProcess` returns a null handle on failure and a handle this
    // function closes on success; no borrowed memory crosses the boundary.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
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
    // Asked after the times and not before, so the last thing this knows about
    // the process is that it had not exited. Asked the way `process_may_be_live`
    // asks it, and for that function's reason: a process handle becomes signalled
    // when and only when the process has terminated, which `GetExitCodeProcess`
    // cannot say as cleanly because its "still running" sentinel `STILL_ACTIVE`
    // is also the genuine exit code 259.
    //
    // SAFETY: `handle` is a live handle and a zero timeout returns immediately.
    let waited = unsafe { WaitForSingleObject(handle, 0) };
    // SAFETY: the handle came from `OpenProcess` above and is closed once.
    unsafe { CloseHandle(handle) };
    // `WAIT_TIMEOUT` — still running — is the only answer that leaves a start to
    // report. `WAIT_OBJECT_0` is a process that has exited, and `WAIT_FAILED` is
    // a question this host would not take; neither is a proof of a live process,
    // and both resolve to "this host will not say".
    (read != 0 && waited == WAIT_TIMEOUT)
        .then(|| format!("{}:{}", created.dwHighDateTime, created.dwLowDateTime))
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
        assert!(!mine.recorded().is_empty());
        assert_eq!(
            process_start_token(pid()),
            Some(mine.clone()),
            "one process gave two different start tokens"
        );
        assert!(mine.matches(mine.recorded()));
        // The two absences that must never prove each other.
        assert!(!mine.matches(""));
        assert!(!mine.matches("some other process's start"));
        let dead = reaped_pid();
        assert!(
            process_start_token(dead).is_none(),
            "pid {dead} was reaped and still answered with a start"
        );
        assert!(process_start_token(0).is_none());
    }

    /// The same absence, held under the one condition that makes a pid keep
    /// answering after its process is gone: a handle to the exited process still
    /// open.
    ///
    /// This is where the creation time stops being a liveness proof. Windows
    /// keeps the process *object* alive for as long as any handle to it is, so
    /// the pid still resolves and still reports the creation time of the run that
    /// already ended — a start token that matches the one recorded at launch, for
    /// a dispatch that is dead. [`reaped_pid`] does not pin that on its own: it
    /// drops its handle, and whether the pid is still answerable afterwards is
    /// the operating system's business rather than the test's. Here the handle is
    /// deliberately held for the whole assertion, so the exited process is
    /// guaranteed to be openable and the answer has to come from the exit check
    /// rather than from the pid having gone away.
    ///
    /// Not `#[cfg(windows)]`: the contract is the same everywhere — a process
    /// that has exited has no start to give — and a Unix host that started
    /// keeping something around after `wait` should fail here too.
    #[test]
    fn an_exited_process_gives_no_start_even_while_a_handle_to_it_is_held() {
        let mut child = std::process::Command::new(
            std::env::current_exe().expect("the test binary knows its own path"),
        )
        .args(["--list", "--format", "terse"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the test binary starts");
        let dead = child.id();
        child.wait().expect("it exits");
        // `child` is *not* dropped before the assertions: on Windows dropping it
        // closes the last handle, which is exactly the crutch this test refuses.
        assert!(
            process_start_token(dead).is_none(),
            "pid {dead} has exited and still answered with a start"
        );
        assert!(
            !process_may_be_live(dead),
            "pid {dead} has exited and still read as live"
        );
        drop(child);
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

    /// A live process, a process already gone, and an id that is not a process
    /// each answer differently.
    ///
    /// The three answers the teardown's own answer is built from, and the reason
    /// they are three. `ESRCH` means the process exited between the listing and
    /// the signal, which is not a failure — treating it as one would make every
    /// ordinary race report an incomplete stop — and it is not a process this
    /// teardown ended either, which is the distinction that used to be missing. A
    /// non-positive id is not a process at all: to `kill` it is a broadcast, so
    /// it is refused rather than sent, and refusing to send is not reaching
    /// anything.
    #[cfg(unix)]
    #[test]
    fn a_signal_separates_a_process_it_reached_from_one_already_gone_and_from_a_broadcast() {
        // Signal `0` is the existence check, so this reaches a live process
        // without ending the suite that is running in it.
        assert_eq!(
            signal_one(pid(), 0),
            Reached::Delivered,
            "a live process was not reported as reached"
        );
        assert_eq!(
            signal_one(reaped_pid(), libc::SIGTERM),
            Reached::Absent,
            "a process that had already exited was reported as one this teardown ended"
        );
        assert_eq!(
            signal_one(0, libc::SIGTERM),
            Reached::Refused,
            "pid 0 was signalled, and to `kill` it is a whole process group"
        );
    }

    /// A teardown aimed at a tree that has already gone says there was nothing
    /// to stop, not that it stopped something.
    ///
    /// The whole of the second defect in one assertion: `stop` used to answer
    /// the same value here as it does for a run it actually ended, so `onepipeline
    /// stop` reported a clean teardown of a driver that had died hours earlier —
    /// and the dispatch tree that driver had orphaned kept running.
    ///
    /// Not `#[cfg(unix)]`: the contract is the same on both platforms.
    #[test]
    fn a_teardown_aimed_at_a_tree_that_has_already_gone_says_there_was_nothing_to_stop() {
        let dead = reaped_pid();
        assert!(
            !process_may_be_live(dead),
            "the reaped pid {dead} was still live, so this is not the case under test"
        );
        assert_eq!(
            stop(dead, Stop::Politely),
            Teardown::NothingToStop,
            "a stop that found nothing to aim at reported having reached a tree"
        );
        // The two ids a teardown never aims at, for the same reason.
        assert_eq!(stop(0, Stop::Politely), Teardown::NothingToStop);
        assert_eq!(stop(pid(), Stop::Politely), Teardown::NothingToStop);
    }

    /// A fixture tree this process does **not** own, and the pids it reports.
    ///
    /// Orphaned deliberately, and every probing test below needs it to be: a
    /// fixture left as this process's own child is reaped by nobody while a
    /// probe is watching it, and a signalled child nobody has collected is a
    /// zombie — which answers a liveness probe as alive. A teardown that ended
    /// such a tree would read as one that left it running, so the fixture would
    /// fail the test for a reason that is entirely the fixture's. `init` reaps
    /// what `init` adopts, so an orphan that is gone reads as gone. The
    /// intermediate shell exits at once and is collected here, which is what
    /// hands the tree over.
    ///
    /// `script` is the tree, each of whose levels echoes its own pid; `levels`
    /// is how many of those to wait for.
    #[cfg(unix)]
    fn orphaned(script: &str, levels: usize) -> Vec<u32> {
        use std::io::BufRead;

        let mut spawner = std::process::Command::new("sh")
            .args(["-c", &format!("{script} &")])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("a fixture tree");
        let reported = spawner.stdout.take().expect("the tree reports itself");
        let pids: Vec<u32> = std::io::BufReader::new(reported)
            .lines()
            .take(levels)
            .map(|line| {
                let line = line.expect("a reported pid");
                line.trim()
                    .parse()
                    .unwrap_or_else(|_| panic!("the tree said {line:?} where a pid was due"))
            })
            .collect();
        spawner.wait().expect("the shell that detached it exits");
        assert_eq!(
            pids.len(),
            levels,
            "the fixture reported {pids:?} where {levels} level(s) were due"
        );
        pids
    }

    /// A stop that signalled a tree waits to see it go, and says so when it
    /// does not.
    ///
    /// The probe [`Teardown::Signalled`] used to defer to a caller that never
    /// performed it. The process here takes the polite ask and stays — a real
    /// one with `SIGTERM` ignored, which is what a wedged worker looks like from
    /// outside — so a teardown reporting on the signal alone calls this a clean
    /// stop while the process is still burning a CPU. Asked again forcefully,
    /// the same process goes, and the answer changes with it.
    #[cfg(unix)]
    #[test]
    fn a_stop_that_watches_reports_a_tree_that_took_the_ask_and_stayed() {
        let deaf = orphaned("sh -c 'trap \"\" TERM; echo $$; sleep 120'", 1)[0];

        assert_eq!(
            stop_and_confirm(
                &[deaf],
                Stop::Politely,
                std::time::Duration::from_millis(300)
            ),
            Teardown::PartlySignalled,
            "a stop watched pid {deaf} never go and still called it a clean stop"
        );
        assert!(
            process_may_be_live(deaf),
            "pid {deaf} ended on the polite ask, so the answer above proves nothing"
        );

        assert_eq!(
            stop_and_confirm(&[deaf], Stop::Now, std::time::Duration::from_secs(10)),
            Teardown::Signalled,
            "a tree that went was not reported as reached"
        );
        assert!(
            !process_may_be_live(deaf),
            "the forceful ask left pid {deaf} running"
        );
    }

    /// Several trees are read over one listing and ended together.
    ///
    /// What a `stop` aims at is every process this run's records name, and they
    /// are not one: a driver the launch record names and a driver the ownership
    /// lock names are two roots whenever the first has died and been taken over.
    /// Read one at a time, the second walk would happen with the first tree
    /// already dying — and a child whose parent has gone is reparented at once,
    /// beyond descent for ever.
    #[cfg(unix)]
    #[test]
    fn a_stop_aimed_at_several_roots_ends_every_tree_and_leaves_the_one_beside_them() {
        let trees: Vec<Vec<u32>> = (0..2)
            .map(|_| {
                orphaned(
                    "sh -c 'echo $$; sh -c \"echo \\$\\$; sleep 120\" & sleep 120'",
                    2,
                )
            })
            .collect();
        let beside = orphaned("sh -c 'echo $$; sleep 120'", 1)[0];
        let roots: Vec<u32> = trees.iter().map(|tree| tree[0]).collect();
        let every: Vec<u32> = trees.concat();
        assert!(
            every.iter().all(|pid| process_may_be_live(*pid)),
            "the trees {every:?} were not running before they were stopped"
        );

        assert_eq!(
            stop_and_confirm(&roots, Stop::Now, std::time::Duration::from_secs(10)),
            Teardown::Signalled,
            "a stop that ended {every:?} did not report reaching them"
        );
        let surviving: Vec<u32> = every
            .iter()
            .copied()
            .filter(|pid| process_may_be_live(*pid))
            .collect();
        assert!(
            surviving.is_empty(),
            "a stop of several trees left {surviving:?} of {every:?} running"
        );
        assert!(
            process_may_be_live(beside),
            "a stop of several trees took pid {beside}, which was under none of them"
        );
        stop(beside, Stop::Now);
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

    /// Several trees are ended together on this platform too.
    ///
    /// What a `stop` aims at is every process the run's records name, and they
    /// are not one: a driver a record names, a driver the lock stamps, and each
    /// dispatch the registry says the work is running in. This platform hands
    /// each tree to `taskkill /T` separately, so what has to hold here is the
    /// **fold** — every ask made, every tree gone, and one answer over the lot of
    /// them. The Unix arm walks one process table for all of them instead, and
    /// `a_stop_aimed_at_several_roots_ends_every_tree_and_leaves_the_one_beside_them`
    /// is where that half is held.
    #[cfg(windows)]
    #[test]
    fn a_stop_aimed_at_several_console_trees_ends_every_one_of_them() {
        let (mut first, first_leaf) = console_tree();
        let (mut second, second_leaf) = console_tree();
        let roots = [first.id(), second.id()];
        let every = [first.id(), first_leaf, second.id(), second_leaf];
        assert!(
            every.iter().all(|pid| platform_process_may_be_live(*pid)),
            "the trees {every:?} were not running before they were stopped"
        );

        assert_eq!(
            stop_and_confirm(&roots, Stop::Now, std::time::Duration::from_secs(10)),
            Teardown::Signalled,
            "a stop that ended the trees {every:?} did not report reaching them"
        );
        assert!(
            all_ended_within(&every, std::time::Duration::from_secs(10)),
            "a stop of several trees left part of {every:?} running"
        );
        let _ = first.wait();
        let _ = second.wait();
    }

    /// The three answers a `taskkill` can establish, including both directions of
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
