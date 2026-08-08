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
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, STILL_ACTIVE,
    };

    // SAFETY: `OpenProcess` returns a null handle on failure and a handle this
    // function closes on success; no borrowed memory crosses the boundary.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // A pid that never existed is rejected as an invalid parameter; every
        // other failure (a permission refusal, most of all) leaves the question
        // open, so it resolves toward live.
        return std::io::Error::last_os_error().raw_os_error()
            != Some(ERROR_INVALID_PARAMETER as i32);
    }
    let mut code: u32 = 0;
    // SAFETY: `handle` is a live handle and `code` is a `u32` this frame owns.
    let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
    // SAFETY: the handle came from `OpenProcess` above and is closed once.
    unsafe { CloseHandle(handle) };
    ok == 0 || code == STILL_ACTIVE as u32
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
