//! Cooperative cancellation for in-flight harness subprocesses.
//!
//! A harness launcher is its own process-group leader (see [`crate::io`]'s
//! process module), so killing the *oneharness* process does not take the
//! harness down with it. When oneharness runs as a library that matters: a host
//! that cancels a turn has no other way to stop the tree it asked for. This
//! module is the handle it uses.
//!
//! Two independent triggers, both consulted by [`cancellation_requested`]:
//!
//! - a [`CancelToken`] the caller owns and flips for *its own* run, and
//! - a process-wide flag raised by [`install_signal_cancel`] when the host
//!   receives SIGINT/SIGTERM (Unix) or a console control event (Windows).
//!
//! Cancellation is cooperative by design: the runner polls it on a short slice
//! and then tears the tree down through the ordinary `Finish::Terminate` path,
//! so a cancelled run still reaps its descendants, drains its pipes, and returns
//! a [`crate::domain::report::Capture`] the caller can report on. It is never a
//! hard kill of the host.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// Raised by the signal/console handlers installed by [`install_signal_cancel`].
///
/// A plain `static` (not an `Arc`) because a signal handler may only touch
/// async-signal-safe state: a store to a lock-free atomic is, a
/// reference-counted allocation is not.
static SIGNAL_CANCEL: AtomicBool = AtomicBool::new(false);

/// How many cancellation signals the host has received. The second one is taken
/// as "I meant it", and exits immediately instead of waiting for teardown.
static SIGNAL_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The exit status a second SIGINT/SIGTERM leaves, mirroring a shell's
/// `128 + SIGINT` convention for an interrupted foreground job.
const FORCED_EXIT_CODE: i32 = 130;

/// A shared "stop the run" flag handed to the runner alongside a job.
///
/// Cloning shares the flag, so a caller keeps a handle while the run holds
/// another. Cancelling is one-way: a token never un-cancels, because a run whose
/// tree has already been terminated cannot be resumed.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh, uncancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation of every run holding this token. Idempotent, and
    /// safe to call from any thread — including while the run is in flight,
    /// which is the whole point.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether *this* token has been cancelled. Callers deciding whether to tear
    /// a run down want [`cancellation_requested`], which also honors a host
    /// signal.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Whether the host process has been asked to stop by SIGINT/SIGTERM (Unix) or a
/// console control event (Windows). Always `false` until
/// [`install_signal_cancel`] has been called — an uninstalled handler leaves the
/// platform default, which for a CLI means the process simply dies.
#[must_use]
pub fn signal_cancel_requested() -> bool {
    SIGNAL_CANCEL.load(Ordering::SeqCst)
}

/// Whether an in-flight run should be torn down: either `token` was cancelled by
/// its owner, or the host received a cancellation signal. The runner polls this,
/// so a library consumer's own token and the CLI's Ctrl-C reach the same
/// teardown path.
#[must_use]
pub fn cancellation_requested(token: &CancelToken) -> bool {
    token.is_cancelled() || signal_cancel_requested()
}

/// Install SIGINT/SIGTERM (Unix) or console-control (Windows) handlers that
/// raise the process-wide cancellation flag instead of killing the process, so
/// in-flight harness trees are terminated and the run still reports.
///
/// This deliberately changes the host's signal disposition, so it is opt-in: the
/// `oneharness` CLI calls it for `run`, and a library embedder calls it only if
/// it wants oneharness to own that disposition. An embedder that already has its
/// own signal handling should cancel a [`CancelToken`] instead.
///
/// A **second** signal exits the process immediately with
/// `130`: teardown is bounded but not instant, and an operator pressing Ctrl-C
/// twice is asking to stop waiting for it.
///
/// Idempotent, and errors are the OS's own (`sigaction`/`SetConsoleCtrlHandler`
/// failing), never a panic.
pub fn install_signal_cancel() -> std::io::Result<()> {
    platform::install()
}

/// Record one cancellation signal, exiting hard on the second.
///
/// Async-signal-safe: two lock-free atomic writes and, at most, `_exit`.
fn note_signal() {
    SIGNAL_CANCEL.store(true, Ordering::SeqCst);
    if SIGNAL_COUNT.fetch_add(1, Ordering::SeqCst) >= 1 {
        platform::exit_now(FORCED_EXIT_CODE);
    }
}

#[cfg(unix)]
mod platform {
    use std::io;
    use std::mem;

    pub(super) fn install() -> io::Result<()> {
        for signal in [libc::SIGINT, libc::SIGTERM] {
            install_one(signal)?;
        }
        Ok(())
    }

    fn install_one(signal: libc::c_int) -> io::Result<()> {
        // SAFETY: `action` is zeroed to the documented all-fields-cleared state
        // before its handler/flags are set, matching `sigaction`'s contract.
        let mut action: libc::sigaction = unsafe { mem::zeroed() };
        action.sa_sigaction = handler as usize;
        // SA_RESTART keeps the wait/read syscalls the runner is blocked in from
        // failing with EINTR: cancellation is observed by the runner's own poll
        // slice, so an interrupted syscall would only turn a clean teardown into
        // a spurious wait failure.
        action.sa_flags = libc::SA_RESTART;
        // SAFETY: `sigaction` is called with a fully initialized action for a
        // valid signal number and a null old-action pointer, and `handler` is
        // async-signal-safe.
        if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    extern "C" fn handler(_signal: libc::c_int) {
        super::note_signal();
    }

    pub(super) fn exit_now(code: i32) {
        // SAFETY: `_exit` is async-signal-safe and does not run destructors or
        // atexit handlers, which is exactly what a signal handler may call.
        unsafe { libc::_exit(code) }
    }
}

/// Windows console-control cancellation.
///
/// Unlike the Unix path, this is an *improvement* rather than the load-bearing
/// guarantee: a hard-killed oneharness on Windows already takes its harnesses
/// with it, because each tree is a Job Object with `KILL_ON_JOB_CLOSE` (see
/// `io::process`). What this adds is the same graceful shape Unix gets — the
/// tree is reaped and the run still reports, instead of the console terminator
/// ending the process mid-run.
// llmlint: ignore-block[changed_behavior_has_e2e] Driving a console control event needs the test to own a console process group and call GenerateConsoleCtrlEvent, which the cargo test harness cannot do without also signalling itself; the descendant-termination guarantee this improves on is already covered on Windows by `windows_streaming_stop_terminates_native_descendant` plus the Job Object's KILL_ON_JOB_CLOSE, and the cross-platform teardown/report shape is covered on Unix by `a_host_signal_cancels_the_run_and_terminates_a_silent_harness`.
#[cfg(windows)]
mod platform {
    use std::io;

    use windows_sys::core::BOOL;
    use windows_sys::Win32::System::Console::{
        SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT,
        CTRL_SHUTDOWN_EVENT,
    };

    /// The Win32 `BOOL` values, spelled here because `windows-sys` exposes the
    /// type as a plain `i32` without named constants.
    const TRUE: BOOL = 1;
    const FALSE: BOOL = 0;

    pub(super) fn install() -> io::Result<()> {
        // SAFETY: a valid handler pointer is registered exactly as documented;
        // repeated registration of the same handler is allowed.
        if unsafe { SetConsoleCtrlHandler(Some(handler), TRUE) } == FALSE {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    // SAFETY: the OS calls this on its own thread with an event code; it reads
    // that code and touches only atomics (or, on a second signal, exits the
    // process), so it shares no state that needs synchronizing.
    unsafe extern "system" fn handler(event: u32) -> BOOL {
        match event {
            CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT => {
                super::note_signal();
                // TRUE: handled — do not run the default terminator, so the run
                // can tear its harness tree down and still report.
                TRUE
            }
            _ => FALSE,
        }
    }

    pub(super) fn exit_now(code: i32) {
        std::process::exit(code);
    }
}
// llmlint: ignore-end[changed_behavior_has_e2e]

#[cfg(not(any(unix, windows)))]
mod platform {
    use std::io;

    pub(super) fn install() -> io::Result<()> {
        Ok(())
    }

    pub(super) fn exit_now(code: i32) {
        std::process::exit(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_starts_uncancelled_and_stays_cancelled() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
        // Idempotent: a second request changes nothing.
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn a_clone_shares_the_flag_with_its_origin() {
        // The runner holds one handle while the caller keeps another; cancelling
        // either must be visible to the other, or an in-flight run never sees it.
        let token = CancelToken::default();
        let handle = token.clone();
        assert!(!cancellation_requested(&handle));
        token.cancel();
        assert!(handle.is_cancelled());
        assert!(cancellation_requested(&handle));
    }
}
