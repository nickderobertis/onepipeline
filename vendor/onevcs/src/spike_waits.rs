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

/// The closed vocabulary of this measurement's independently selectable options.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Variant {
    /// Wake on a child exit or finished output capture.
    Exit,
    /// Start file polls short and cap at their original interval.
    Adaptive,
    // llmlint: ignore[code_lands_in_the_domain_that_owns_it] Measurement-only shared switch, never landed (a1): every child inherits one selection so artifacts identify the entire experiment, but only onepipeline's harness reads Relaunch. This vendored helper is not a repository-engine API proposal; the report explicitly requires R to land in onepipeline tests alone with no onevcs release.
    /// Reuse stable CLI readings inside the selected journeys.
    Relaunch,
}

fn invalid_selection(reason: &str) -> ! {
    eprintln!("{ENV}: {reason}; expected a comma-separated list of exit, adaptive, relaunch");
    std::process::exit(2)
}

fn selected() -> &'static [Variant] {
    static SELECTED: OnceLock<Vec<Variant>> = OnceLock::new();
    SELECTED.get_or_init(|| {
        let value = match std::env::var(ENV) {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => String::new(),
            Err(std::env::VarError::NotUnicode(_)) => invalid_selection("not valid Unicode"),
        };
        value
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| match name {
                "exit" => Variant::Exit,
                "adaptive" => Variant::Adaptive,
                "relaunch" => Variant::Relaunch,
                _ => invalid_selection(&format!("unknown prototype {name:?}")),
            })
            .collect()
    })
}

/// Whether the named prototype is in effect in this process.
pub fn is_selected(variant: Variant) -> bool {
    selected().contains(&variant)
}

/// Whether exit notification is in effect.
pub fn exit_notify() -> bool {
    is_selected(Variant::Exit)
}

/// Whether adaptive polls are in effect.
pub fn adaptive() -> bool {
    is_selected(Variant::Adaptive)
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
        // llmlint: ignore[changed_behavior_has_e2e] Measurement-only fallback, never landed (a1): portable hosted journeys cannot safely force OpenProcess access failure without mocking the kernel or changing host privileges. It keeps the old bounded sleep on failure; successful and already-gone child paths are exercised. Production adoption must cover platform permission failures separately.
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
    // llmlint: ignore[changed_behavior_has_e2e] Measurement-only fallback, never landed (a1): real spawned PIDs fit pid_t, so this defensive conversion arm is outside the hosted workload. It retains the caller's bounded sleep; no saving is attributed to it, and testing an invented PID is not a realistic child journey.
    let Ok(raw) = libc::pid_t::try_from(pid) else {
        return std::thread::sleep(upto);
    };
    // SAFETY: `pidfd_open` takes a pid and flags and returns a descriptor this
    // function owns, or -1.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, raw, 0) };
    // llmlint: ignore[changed_behavior_has_e2e] Measurement-only fallback, never landed (a1): the kernel returns an int descriptor, so an out-of-int syscall result is defensive only. Resource exhaustion or syscall interception would change the measured host; this preserves the old bounded sleep and is not priced as an optimization.
    let Ok(descriptor) = libc::c_int::try_from(descriptor) else {
        return std::thread::sleep(upto);
    };
    // llmlint: ignore[changed_behavior_has_e2e] Measurement-only fallback, never landed (a1): safely forcing pidfd_open to be unavailable requires a different kernel/seccomp policy, outside these hosted runners. ESRCH is exercised by the already-gone child test; other errors keep the original bounded sleep. Production adoption must validate unsupported-kernel behavior.
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
    // llmlint: ignore[changed_behavior_has_e2e] Measurement-only fallback, never landed (a1): kqueue allocation failure cannot be forced safely in the hosted workload without exhausting descriptors or mocking the kernel. This retains the caller's bounded sleep; no saving is claimed for it, and production adoption must validate that resource-failure branch.
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

    #[test]
    fn selection_probe() {
        println!("selected={:?}", selected());
    }

    #[test]
    fn the_selector_reads_only_known_variants_and_refuses_a_typo_in_a_fresh_process() {
        let probe = |value: &str| {
            std::process::Command::new(std::env::current_exe().expect("the test binary"))
                .args([
                    "--exact",
                    "spike_waits::tests::selection_probe",
                    "--nocapture",
                ])
                .env(ENV, value)
                .output()
                .expect("the selector probe starts")
        };
        for (value, expected) in [
            ("", "selected=[]"),
            ("exit", "selected=[Exit]"),
            ("adaptive", "selected=[Adaptive]"),
            ("relaunch", "selected=[Relaunch]"),
            (
                "exit, adaptive,relaunch",
                "selected=[Exit, Adaptive, Relaunch]",
            ),
        ] {
            let output = probe(value);
            assert!(output.status.success(), "{output:?}");
            assert!(
                String::from_utf8_lossy(&output.stdout).contains(expected),
                "{output:?}"
            );
        }
        let output = probe("adpative");
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("unknown prototype"));
    }

    #[cfg(unix)]
    #[test]
    fn a_non_unicode_selector_is_refused_in_a_fresh_process() {
        use std::os::unix::ffi::OsStringExt as _;
        let output = std::process::Command::new(std::env::current_exe().expect("the test binary"))
            .args([
                "--exact",
                "spike_waits::tests::selection_probe",
                "--nocapture",
            ])
            .env(ENV, std::ffi::OsString::from_vec(vec![0xff]))
            .output()
            .expect("the selector probe starts");
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("not valid Unicode"));
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
