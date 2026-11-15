//! Measurement only (spike-child-waits, never lands): the child-wait prototypes this
//! branch's demonstrations select, each on its own.
//!
//! [`ENV`] holds a comma-separated list of the prototypes in effect for this process
//! and every process it starts:
//!
//! - `exit` — a wait that polls only to learn that a child exited is woken by the
//!   platform's own notification of that exit (a pidfd on Linux, a kqueue
//!   `EVFILT_PROC` on macOS, the process handle on Windows) instead of sleeping out
//!   its interval;
//! - `adaptive` — a poll on something no notification covers (a file a child or
//!   another thread writes) starts short and backs off to its old interval;
//! - `relaunch` — read by the e2e harness alone: the journeys it names launch the
//!   CLI fewer times.
//!
//! Unset or empty, every site behaves exactly as the release this copy vendors.
//! The e2e harness sets it from its own compiled-in selection, so a demonstration
//! commit changes one constant and nothing in any workflow.

use std::process::Child;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The switch.
pub const ENV: &str = "ONEPIPELINE_SPIKE_CHILD_WAITS";

fn selected() -> &'static [String] {
    static SELECTED: OnceLock<Vec<String>> = OnceLock::new();
    SELECTED.get_or_init(|| {
        std::env::var(ENV)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect()
    })
}

/// Whether the named prototype is in effect in this process.
pub fn is_selected(name: &str) -> bool {
    selected().iter().any(|chosen| chosen == name)
}

/// Whether exit notification is in effect.
pub fn exit_notify() -> bool {
    is_selected("exit")
}

/// Whether adaptive polls are in effect.
pub fn adaptive() -> bool {
    is_selected("adaptive")
}

/// Wait at most `upto` for `child` to exit, returning as soon as it does.
///
/// The child is never reaped here — its caller's `try_wait` still collects it — so
/// this only ever shortens the caller's wait to the instant of the exit. Where the
/// platform's notification cannot be armed, it sleeps `upto` as the caller did.
/// Not recorded on its own: every caller is inside a traced wait that counts it.
pub fn await_child_exit(child: &Child, upto: Duration) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle as _;
        wait_handle(child.as_raw_handle(), upto);
    }
    #[cfg(unix)]
    {
        await_pid(child.id(), upto);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = child;
        std::thread::sleep(upto);
    }
}

/// Wait at most `upto` for the process `pid` — a child or not — to exit, returning
/// as soon as it does. Recorded at `site` as a blocking wait bounded by `upto`.
pub fn await_pid_exit(site: crate::launch_trace::Site, pid: u32, upto: Duration) {
    let began = Instant::now();
    #[cfg(unix)]
    await_pid(pid, upto);
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
        // SAFETY: opens a handle this function owns and closes below; a null
        // answer (a process already gone, or one this user cannot open) is
        // waited out as a plain sleep would have been.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            // ERROR_INVALID_PARAMETER: no such process, so it is already gone.
            if std::io::Error::last_os_error().raw_os_error() != Some(87) {
                std::thread::sleep(upto);
            }
        } else {
            wait_handle(handle, upto);
            // SAFETY: the handle was opened above and is closed once.
            unsafe {
                CloseHandle(handle);
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        std::thread::sleep(upto);
    }
    crate::launch_trace::waited(site, began.elapsed(), Some(upto));
}

#[cfg(windows)]
fn wait_handle(handle: std::os::windows::raw::HANDLE, upto: Duration) {
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    let millis = u32::try_from(upto.as_millis()).unwrap_or(u32::MAX - 1);
    // SAFETY: the handle is live for the call; the wait only reads its state.
    unsafe {
        WaitForSingleObject(handle as _, millis);
    }
}

#[cfg(target_os = "linux")]
fn await_pid(pid: u32, upto: Duration) {
    let Ok(raw) = libc::pid_t::try_from(pid) else {
        return std::thread::sleep(upto);
    };
    // SAFETY: `pidfd_open` takes a pid and flags and returns a descriptor this
    // function owns, or -1.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, raw, 0) };
    let Ok(descriptor) = libc::c_int::try_from(descriptor) else {
        return std::thread::sleep(upto);
    };
    if descriptor < 0 {
        // ESRCH: the process is already gone, and that is the answer.
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return;
        }
        return std::thread::sleep(upto);
    }
    let mut watched = libc::pollfd {
        fd: descriptor,
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = libc::c_int::try_from(upto.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: one descriptor owned here, live for the call; closed once after.
    unsafe {
        libc::poll(&raw mut watched, 1, millis);
        libc::close(descriptor);
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn await_pid(pid: u32, upto: Duration) {
    // SAFETY: a fresh kqueue this function owns and closes below.
    let queue = unsafe { libc::kqueue() };
    if queue < 0 {
        return std::thread::sleep(upto);
    }
    let change = libc::kevent {
        ident: pid as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let mut fired = change;
    let timeout = libc::timespec {
        tv_sec: libc::time_t::try_from(upto.as_secs()).unwrap_or(libc::time_t::MAX),
        tv_nsec: libc::c_long::from(upto.subsec_nanos().min(999_999_999) as i32),
    };
    // SAFETY: one change and one event slot, both live for the call; the queue
    // is closed once after. A registration refused with ESRCH is a process
    // already exited, which returns at once, as it should.
    unsafe {
        libc::kevent(
            queue,
            &raw const change,
            1,
            &raw mut fired,
            1,
            &raw const timeout,
        );
        libc::close(queue);
    }
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "macos", target_os = "ios"))
))]
fn await_pid(_pid: u32, upto: Duration) {
    std::thread::sleep(upto);
}

/// An adaptive poll's interval: the `step`-th look waits `first` doubled `step`
/// times, never more than `ceiling` — the interval the site used before.
pub fn backoff(step: u32, first: Duration, ceiling: Duration) -> Duration {
    first
        .checked_mul(1_u32.checked_shl(step.min(16)).unwrap_or(u32::MAX))
        .map_or(ceiling, |interval| interval.min(ceiling))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backoff_doubles_from_its_first_interval_up_to_its_ceiling() {
        let first = Duration::from_millis(1);
        let ceiling = Duration::from_millis(20);
        let steps: Vec<_> = (0..7).map(|step| backoff(step, first, ceiling)).collect();
        assert_eq!(
            steps,
            [1, 2, 4, 8, 16, 20, 20].map(Duration::from_millis).to_vec()
        );
        assert_eq!(backoff(u32::MAX, first, ceiling), ceiling);
    }

    #[test]
    fn a_wait_on_a_child_returns_at_its_exit_rather_than_at_its_bound() {
        let mut child = quick_child();
        let began = Instant::now();
        let bound = Duration::from_secs(20);
        let mut exited = None;
        while began.elapsed() < bound {
            if let Some(status) = child.try_wait().expect("the child can be asked about") {
                exited = Some(status);
                break;
            }
            await_child_exit(&child, bound);
        }
        if exited.is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
        assert!(exited.is_some(), "the child never exited");
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "the wait ran to its bound: {:?}",
            began.elapsed()
        );
    }

    #[test]
    fn a_wait_on_a_pid_already_gone_returns_without_waiting_its_bound() {
        let mut child = quick_child();
        let pid = child.id();
        child.wait().expect("the child is reaped");
        let began = Instant::now();
        await_pid_exit(crate::launch_site!(), pid, Duration::from_secs(20));
        assert!(began.elapsed() < Duration::from_secs(10));
    }

    fn quick_child() -> Child {
        #[cfg(windows)]
        let mut command = std::process::Command::new("cmd");
        #[cfg(windows)]
        command.args(["/C", "exit 0"]);
        #[cfg(not(windows))]
        let mut command = std::process::Command::new("sh");
        #[cfg(not(windows))]
        command.args(["-c", "exit 0"]);
        command.spawn().expect("a shell starts")
    }
}
