//! Find and stop every process of the current user that carries a marker
//! environment variable.
//!
//! A process tree is not always something this crate owns. A command a shared
//! server starts on a client's behalf is the server's child, in its own session
//! and process group, with nothing in its ancestry naming the client — so the
//! process-tree containment in `io::process` cannot reach it. What *can* name it
//! is its environment: give the work a variable with a value unique to it, and
//! every process it starts inherits that pair. [`find_marked`] lists the
//! processes carrying a [`ProcessMarker`]; [`stop_marked`] stops them all.
//!
//! How another process's environment is read, per platform:
//!
//! - **Linux** — `/proc/<pid>/environ`, over the `/proc/<pid>` entries the
//!   current effective user owns.
//! - **macOS** — the `KERN_PROCARGS2` sysctl, over the pids `proc_listpids`
//!   reports for the current effective user.
//! - **Windows** — the target's process environment block, found through its
//!   PEB and read with `ReadProcessMemory`, over the processes whose token user
//!   is the current one. The process-query and memory-read rights it needs are
//!   ones a same-user process grants.
//!
//! What is read is the environment the kernel (or the PEB) holds now, which for
//! a process that never rewrote its own environment is the one it started with.
//! A marker is a string inherited at spawn, so that is the one that matters.

use std::io;
use std::time::{Duration, Instant};

/// How long [`stop_marked`] waits between two scans.
const POLL: Duration = Duration::from_millis(50);

/// How long [`stop_marked`] keeps re-scanning after its forced kill. Neither
/// `SIGKILL` nor `TerminateProcess` is synchronous: a killed process can still
/// be listed for a moment after the call returns, so the final scan waits this
/// long for what it killed to be gone before counting it a survivor.
const KILL_SETTLE: Duration = Duration::from_secs(1);

/// An environment variable whose exact value names the processes to find.
///
/// A process matches when its environment holds `name` with exactly `value`:
/// the comparison is byte-for-byte (UTF-16 on Windows), with no case folding of
/// the name, so a marker spelled one way only matches that spelling.
// llmlint: ignore-block[invalid_states_unrepresentable] The two public `String` fields are this module's published interface, which the shared app-server pool is built against; a validating constructor or newtypes would break that interface. An invalid marker never reaches a scan: `Needle::new` validates it at both entry points and returns `InvalidInput`, as the integration test `a_marker_that_cannot_name_a_variable_is_refused` shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessMarker {
    pub name: String,
    pub value: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// What [`stop_marked`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StopReport {
    /// Every pid that was asked to stop — sent `SIGTERM` on Unix, terminated at
    /// once on Windows, which has no such request.
    pub signalled: Vec<u32>,
    /// Every pid that still matched at the deadline (or appeared after it) and
    /// had to be forcibly killed — `SIGKILL` on Unix, `TerminateProcess` on
    /// Windows. A pid can be in both lists.
    pub killed: Vec<u32>,
    /// Every pid that still matched at the end. Empty on success.
    pub survivors: Vec<u32>,
}

/// The pids of the current user's processes whose environment matches
/// `marker`, excluding the calling process.
///
/// A process whose environment cannot be read — another user's, a protected
/// one, one that exited mid-scan — is skipped, never an error.
///
/// # Errors
///
/// - The platform's process listing itself failed.
/// - [`io::ErrorKind::InvalidInput`], before any scan: `marker` cannot name an
///   environment variable at all — an empty name, a name holding `=`, or a NUL
///   in the name or the value. Such a marker could never match, so it is a
///   caller's bug rather than "nothing to find".
pub fn find_marked(marker: &ProcessMarker) -> io::Result<Vec<u32>> {
    scan(&Needle::new(marker)?)
}

/// Stop every process [`find_marked`] reports for `marker`, waiting at most
/// `deadline` for them to go before forcing them.
///
/// It asks every matching process to stop (`SIGTERM` on Unix; on Windows, where
/// there is no such request, it terminates the process at once) and re-scans,
/// asking any newly matching process too, until none matches or `deadline` has
/// passed. It then forcibly kills whatever still matches (`SIGKILL` on Unix,
/// `TerminateProcess` on Windows) and re-scans once more — killing anything new
/// that scan finds, for up to a short settle window while the kills land — so a
/// matching process forked during the stop is caught too. An empty scan only
/// counts once everything the scan before it matched had already stopped
/// matching when it began, so a process that forks and exits *during* a scan
/// cannot leave its child unseen.
///
/// Each process is re-checked against the marker immediately before it is
/// signalled, so a pid that was reused between the scan and the signal is left
/// alone.
///
/// A `deadline` too large to add to the current instant is no deadline at all:
/// the stop then waits until nothing matches.
///
/// # Errors
///
/// - The platform's process listing itself failed.
/// - [`io::ErrorKind::InvalidInput`], before any scan or signal: `marker`
///   cannot name an environment variable at all — an empty name, a name
///   holding `=`, or a NUL in the name or the value.
///
/// A process that cannot be signalled is not an error; it is reported in
/// [`StopReport::survivors`].
pub fn stop_marked(marker: &ProcessMarker, deadline: Duration) -> io::Result<StopReport> {
    let needle = Needle::new(marker)?;
    let give_up = Instant::now().checked_add(deadline);
    let mut report = StopReport::default();
    let mut scans = Scans::new(&needle);

    loop {
        let found = scans.next()?;
        if found.is_empty() && scans.conclusive {
            return Ok(report);
        }
        for pid in found {
            if !report.signalled.contains(&pid) && platform::request_stop(pid, &needle) {
                report.signalled.push(pid);
            }
        }
        let wait = match give_up {
            Some(give_up) => give_up.saturating_duration_since(Instant::now()),
            None => POLL,
        };
        if wait.is_zero() {
            break;
        }
        std::thread::sleep(POLL.min(wait));
    }

    let settled = Instant::now() + KILL_SETTLE;
    loop {
        let found = scans.next()?;
        if found.is_empty() && scans.conclusive {
            return Ok(report);
        }
        // llmlint: ignore[changed_behavior_has_e2e] A process first matching during the settle window must be started after the deadline's first kill scan by a process that outlived every kill — on Windows one refusing PROCESS_TERMINATE, as `a_marked_process_that_refuses_termination_is_reported_as_a_survivor` stages — and nothing a fixture can observe marks the moment the deadline passes, so no test can place that start inside the window. The kill path itself is proven end to end by `a_grandchild_that_ignores_the_stop_request_is_killed_by_the_deadline`, and the survivor path by the refusing test.
        for pid in &found {
            if !report.killed.contains(pid) && platform::kill(*pid, &needle) {
                report.killed.push(*pid);
            }
        }
        let now = Instant::now();
        if now >= settled {
            report.survivors = found;
            return Ok(report);
        }
        std::thread::sleep(POLL.min(settled - now));
    }
}

/// Successive scans for one marker, each knowing whether its answer can be
/// trusted to be empty.
///
/// One scan is not an instant: it lists the process table, then reads each
/// listed process's environment. A marked process listed by a scan can start a
/// marked child the listing already went past and exit before its own
/// environment is read — and that scan matches neither. So an empty scan only
/// ends a stop when it is *conclusive*: every process the previous scan matched
/// had stopped matching before this one began, so whatever it started before
/// stopping was already in the table this scan listed.
struct Scans<'a> {
    needle: &'a Needle,
    previous: Vec<u32>,
    /// Whether the last scan began after everything the one before it found
    /// had stopped matching.
    conclusive: bool,
}

impl<'a> Scans<'a> {
    fn new(needle: &'a Needle) -> Self {
        Self {
            needle,
            previous: Vec::new(),
            conclusive: false,
        }
    }

    fn next(&mut self) -> io::Result<Vec<u32>> {
        // llmlint: ignore[changed_behavior_has_e2e] `a_marked_child_started_while_stopping_is_stopped_too` drives this race end to end, but only probabilistically: the kernel decides where a fork lands relative to a scan's listing and environment reads. Making it deterministic would need a seam inside the platform's process listing, which this module does not expose.
        self.conclusive = self
            .previous
            .iter()
            .all(|&pid| !platform::carries(pid, self.needle));
        let found = scan(self.needle)?;
        self.previous.clone_from(&found);
        Ok(found)
    }
}

/// One scan: the current user's pids, minus this process, whose environment
/// holds the needle.
fn scan(needle: &Needle) -> io::Result<Vec<u32>> {
    let own = std::process::id();
    // llmlint: ignore[changed_behavior_has_e2e] A failure of `/proc`, `proc_listpids` or a Toolhelp snapshot cannot be induced from an unprivileged test without mocking the very platform layer under test; the error is propagated unchanged by `?` through `find_marked` and both stop phases, with no state to clean up on the way.
    let mut found: Vec<u32> = platform::user_pids()?
        .into_iter()
        .filter(|&pid| pid != own && platform::carries(pid, needle))
        .collect();
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

/// A validated marker, as the one environment entry it matches.
struct Needle {
    /// `name=value`, as the bytes a Unix environment holds.
    #[cfg(unix)]
    bytes: Vec<u8>,
    /// The same entry as the UTF-16 units a Windows environment block holds.
    #[cfg(windows)]
    wide: Vec<u16>,
}

impl Needle {
    fn new(marker: &ProcessMarker) -> io::Result<Self> {
        let invalid = |why: &str| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("process marker {:?}: {why}", marker.name),
            )
        };
        if marker.name.is_empty() {
            return Err(invalid("the variable name is empty"));
        }
        if marker.name.contains('=') {
            return Err(invalid("a variable name cannot contain '='"));
        }
        if marker.name.contains('\0') || marker.value.contains('\0') {
            return Err(invalid("an environment entry cannot contain NUL"));
        }
        #[cfg(any(unix, windows))]
        let entry = format!("{}={}", marker.name, marker.value);
        Ok(Self {
            #[cfg(unix)]
            bytes: entry.into_bytes(),
            #[cfg(windows)]
            wide: entry.encode_utf16().collect(),
        })
    }

    /// Whether one of a process's environment entries is this one exactly.
    #[cfg(unix)]
    fn among<'b>(&self, mut entries: impl Iterator<Item = &'b [u8]>) -> bool {
        entries.any(|entry| entry == self.bytes)
    }
}

/// The NUL-terminated entries of an environment block, in order — bytes on
/// Unix, UTF-16 units on Windows. A trailing fragment with no terminator is a
/// block cut short, and is not an entry: matching it could take a prefix of a
/// longer value for the marker's.
// llmlint: ignore[changed_behavior_has_e2e] Every block this reads is produced by the kernel (`/proc`, `KERN_PROCARGS2`) or the target's PEB, so a truncated one cannot be staged by a test through the real interface; the rejection is pinned by the unit test `only_terminated_entries_are_entries`, which runs on every platform.
fn terminated_entries<T: Copy + Default + PartialEq>(block: &[T]) -> impl Iterator<Item = &[T]> {
    let nul = T::default();
    block
        .split_inclusive(move |&unit| unit == nul)
        .filter_map(move |entry| entry.strip_suffix(&[nul]))
}

#[cfg(unix)]
mod platform {
    use super::Needle;

    /// Send `SIGTERM` to `pid` if it still carries the marker.
    pub(super) fn request_stop(pid: u32, needle: &Needle) -> bool {
        signal(pid, needle, libc::SIGTERM)
    }

    /// Send `SIGKILL` to `pid` if it still carries the marker.
    pub(super) fn kill(pid: u32, needle: &Needle) -> bool {
        signal(pid, needle, libc::SIGKILL)
    }

    fn signal(pid: u32, needle: &Needle, signal: libc::c_int) -> bool {
        let Ok(target) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // A non-positive pid would address a process group or every process;
        // a scan never yields one, and this keeps it that way.
        // llmlint: ignore[changed_behavior_has_e2e] The re-check guards pid reuse between a scan and this signal, a window no test can steer a new process into: the kernel picks pids. What it protects is proven from the other side — the decoy test shows a process carrying another value, or none, is never signalled.
        if target <= 0 || !carries(pid, needle) {
            return false;
        }
        // SAFETY: a positive pid addresses exactly that one process, which
        // was re-checked against the marker immediately above.
        // llmlint: ignore[changed_behavior_has_e2e] `kill` fails for a same-user pid only when the process is already gone (ESRCH), and staging EPERM needs a second user, which only root can create; either way the pid is not recorded, and a process still matching is reported in `survivors` by the final scan.
        unsafe { libc::kill(target, signal) == 0 }
    }

    pub(super) use os::{carries, user_pids};

    #[cfg(target_os = "linux")]
    mod os {
        use std::io;
        use std::os::unix::fs::MetadataExt;

        use super::super::Needle;

        /// Every `/proc/<pid>` the current effective user owns.
        pub(in super::super) fn user_pids() -> io::Result<Vec<u32>> {
            // SAFETY: `geteuid` only reads the caller's credentials.
            let euid = unsafe { libc::geteuid() };
            let mut pids = Vec::new();
            // llmlint: ignore-block[changed_behavior_has_e2e] `/proc` failing to list, an entry failing mid-listing, and an entry's metadata failing all need either a broken procfs mount, which an unprivileged test cannot stage, or a process exiting between the listing and the read, a race no test can time; the first is propagated unchanged as the scan's error, the other two skip the entry. Every Linux test drives the listing's success path end to end, and the protected-process test drives the ownership exclusion.
            for entry in std::fs::read_dir("/proc")? {
                let Ok(entry) = entry else { continue };
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<u32>().ok())
                else {
                    continue;
                };
                // A process that exited since `read_dir` listed it is skipped.
                // `a_marked_process_whose_environment_is_unreadable_is_skipped`
                // runs this branch: a non-dumpable process's `/proc` entry is
                // owned by root, so it fails this comparison exactly as another
                // user's process does.
                if entry.metadata().is_ok_and(|meta| meta.uid() == euid) {
                    pids.push(pid);
                }
            }
            // llmlint: ignore-end[changed_behavior_has_e2e]
            Ok(pids)
        }

        /// Whether `/proc/<pid>/environ` holds the entry. A zombie's reads
        /// empty, so a process that has exited never matches.
        ///
        /// A failed read is no match. Every stop test drives that branch: once
        /// the processes a stop ended are reaped, the re-check `Scans::next`
        /// makes of them fails with `ENOENT`, and a same-user process whose
        /// environment the kernel withholds fails with `EACCES` on any scan.
        pub(in super::super) fn carries(pid: u32, needle: &Needle) -> bool {
            std::fs::read(format!("/proc/{pid}/environ"))
                .is_ok_and(|block| needle.among(super::super::terminated_entries(&block)))
        }
    }

    #[cfg(target_os = "macos")]
    mod os {
        use std::io;
        use std::mem::size_of;
        use std::ptr;

        use super::super::Needle;

        /// Every pid `proc_listallpids` reports whose effective uid
        /// (`proc_pidinfo`'s `pbi_uid`) is the caller's.
        pub(in super::super) fn user_pids() -> io::Result<Vec<u32>> {
            // SAFETY: `geteuid` only reads the caller's credentials.
            let euid = unsafe { libc::geteuid() };
            // The table can grow between sizing and filling it, so size with
            // headroom and retry when the answer fills the whole buffer.
            let mut capacity = 1024usize;
            loop {
                let mut pids = vec![0 as libc::pid_t; capacity];
                // llmlint: ignore-block[changed_behavior_has_e2e] A table whose byte size overflows a C int needs hundreds of millions of processes, and `proc_listallpids` fails only on a kernel refusal no unprivileged test can provoke; both are propagated unchanged as the scan's error. Every macOS test drives the listing's success path end to end.
                let bytes = libc::c_int::try_from(pids.len() * size_of::<libc::pid_t>())
                    .map_err(|_| io::Error::other("process table too large to list"))?;
                // SAFETY: `pids` is writable for exactly `bytes` bytes.
                let listed = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
                if listed < 0 {
                    return Err(io::Error::last_os_error());
                }
                // llmlint: ignore-end[changed_behavior_has_e2e]
                let count = usize::try_from(listed).unwrap_or(0);
                if count < capacity {
                    pids.truncate(count);
                    return Ok(pids
                        .into_iter()
                        .filter(|&pid| pid > 0 && owner(pid) == Some(euid))
                        .filter_map(|pid| u32::try_from(pid).ok())
                        .collect());
                }
                // llmlint: ignore[changed_behavior_has_e2e] The retry runs only when the table holds at least as many processes as the buffer at the moment of the call, which a test cannot arrange on a shared runner without starting over a thousand processes; every macOS scan runs the ordinary pass above end to end.
                capacity *= 2;
            }
        }

        // llmlint: ignore-block[changed_behavior_has_e2e] Another user's process needs root to start, which a test on a CI runner does not have, and a query refusal comes only from a process that exited mid-scan; both end in a pid that is not listed. Every macOS test drives the matching side end to end for the caller's own processes.
        /// The effective uid of `pid`, or `None` when it cannot be read.
        fn owner(pid: libc::pid_t) -> Option<libc::uid_t> {
            let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
            let size = libc::c_int::try_from(size_of::<libc::proc_bsdinfo>()).ok()?;
            // SAFETY: `info` is writable for `size` bytes, the size of the
            // structure PROC_PIDTBSDINFO fills.
            let filled = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    info.as_mut_ptr().cast(),
                    size,
                )
            };
            if filled != size {
                return None;
            }
            // SAFETY: the kernel filled all `size` bytes of the structure.
            Some(unsafe { info.assume_init() }.pbi_uid)
        }
        // llmlint: ignore-end[changed_behavior_has_e2e]

        /// Whether the environment `KERN_PROCARGS2` reports for `pid` holds the
        /// entry. The sysctl refuses another user's or a protected process,
        /// which then simply does not match.
        pub(in super::super) fn carries(pid: u32, needle: &Needle) -> bool {
            procargs(pid)
                .is_some_and(|args| environment(&args).is_some_and(|env| needle.among(env)))
        }

        // llmlint: ignore-block[changed_behavior_has_e2e] Both sysctls refuse only a process the caller may not inspect (another user's, a protected one) or one that exited mid-scan, none of which a test on a CI runner can start on demand; a refusal ends in `None`, which skips the process. Every macOS test drives both calls end to end.
        fn procargs(pid: u32) -> Option<Vec<u8>> {
            let pid = libc::c_int::try_from(pid).ok()?;
            let mut argmax: libc::c_int = 0;
            let mut size = size_of::<libc::c_int>();
            let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
            // SAFETY: `argmax` is writable for `size` bytes; no new value is set.
            let read = unsafe {
                libc::sysctl(
                    mib.as_mut_ptr(),
                    2,
                    (&raw mut argmax).cast(),
                    &mut size,
                    ptr::null_mut(),
                    0,
                )
            };
            if read != 0 || argmax <= 0 {
                return None;
            }
            let mut buffer = vec![0u8; usize::try_from(argmax).ok()?];
            let mut size = buffer.len();
            let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
            // SAFETY: `buffer` is writable for `size` bytes; no new value is set.
            let read = unsafe {
                libc::sysctl(
                    mib.as_mut_ptr(),
                    3,
                    buffer.as_mut_ptr().cast(),
                    &mut size,
                    ptr::null_mut(),
                    0,
                )
            };
            if read != 0 {
                return None;
            }
            buffer.truncate(size);
            Some(buffer)
        }
        // llmlint: ignore-end[changed_behavior_has_e2e]

        /// The environment entries of a `KERN_PROCARGS2` buffer: a native
        /// `int` argc, the executable path, NUL padding, `argc` NUL-terminated
        /// arguments, then the NUL-terminated environment up to an empty entry.
        /// `None` for a buffer that does not have that shape — a negative argc,
        /// or fewer arguments than it claims.
        // llmlint: ignore[changed_behavior_has_e2e] The buffer is written by the kernel's `KERN_PROCARGS2` handler, so a malformed one cannot be staged through the real sysctl; the parser's refusals are pinned by `a_malformed_procargs_buffer_has_no_environment`, and its happy path runs end to end in every macOS stop test.
        fn environment(args: &[u8]) -> Option<impl Iterator<Item = &[u8]>> {
            let (head, rest) = args.split_at_checked(size_of::<libc::c_int>())?;
            let argc = libc::c_int::from_ne_bytes(head.try_into().ok()?);
            let argc = usize::try_from(argc).ok()?;
            let mut entries = super::super::terminated_entries(rest);
            // The executable path, then the padding after it.
            entries.next()?;
            let mut entries = entries.skip_while(|entry| entry.is_empty());
            for _ in 0..argc {
                entries.next()?;
            }
            Some(entries.take_while(|entry| !entry.is_empty()))
        }

        #[cfg(test)]
        mod tests {
            use super::environment;

            fn procargs(argc: i32, rest: &[u8]) -> Vec<u8> {
                let mut args = argc.to_ne_bytes().to_vec();
                args.extend_from_slice(rest);
                args
            }

            #[test]
            fn procargs_environment_starts_after_the_arguments() {
                let args = procargs(2, b"/bin/exe\0\0\0\0exe\0--flag\0A=1\0B=2\0\0junk\0");
                let entries: Vec<&[u8]> = environment(&args).unwrap().collect();
                assert_eq!(entries, [&b"A=1"[..], &b"B=2"[..]]);
            }

            #[test]
            fn a_malformed_procargs_buffer_has_no_environment() {
                assert!(environment(&[0, 0]).is_none(), "shorter than argc");
                assert!(environment(&procargs(-1, b"/bin/exe\0A=1\0")).is_none());
                assert!(environment(&procargs(3, b"/bin/exe\0exe\0")).is_none());
                let cut = procargs(1, b"/bin/exe\0exe\0A=1\0B=trunc");
                let entries: Vec<&[u8]> = environment(&cut).unwrap().collect();
                assert_eq!(entries, [&b"A=1"[..]], "an unterminated entry is not one");
            }
        }
    }

    // llmlint: ignore-block[changed_behavior_has_e2e] No CI leg runs a Unix other than Linux or macOS, so this refusal has no platform to run a test on; it exists so the crate still builds there and says loudly that the scan is unsupported instead of reporting an empty table.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    mod os {
        use std::io;

        use super::super::Needle;

        pub(in super::super) fn user_pids() -> io::Result<Vec<u32>> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "listing another process's environment is not implemented on this platform",
            ))
        }

        pub(in super::super) fn carries(_pid: u32, _needle: &Needle) -> bool {
            false
        }
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::io;
    use std::mem::size_of;
    use std::ptr;

    use windows_sys::Wdk::System::Threading::{
        NtQueryInformationProcess, ProcessBasicInformation, ProcessWow64Information,
    };
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, TerminateProcess,
        PROCESS_BASIC_INFORMATION, PROCESS_QUERY_INFORMATION, PROCESS_TERMINATE, PROCESS_VM_READ,
    };

    use super::Needle;

    /// Where one architecture's PEB keeps its `ProcessParameters` pointer, and
    /// where those `RTL_USER_PROCESS_PARAMETERS` keep `Environment` and
    /// `EnvironmentSize`, with the width of every pointer and size in them.
    ///
    /// `windows-sys` exposes only the reserved-field view of both structures,
    /// so the offsets are the ones the full definitions (`ntpebteb.h` and
    /// `ntrtl.h` in the Process Hacker / System Informer headers) give, which
    /// every Windows since Vista shares. They are what the tests read real
    /// processes through: every Windows stop test goes through [`PEB64`], and
    /// `a_marked_32_bit_process_is_found_and_stopped` through [`PEB32`].
    struct Layout {
        word: usize,
        parameters: usize,
        environment: usize,
        size: usize,
    }

    /// A 64-bit process: native x64 or ARM64, or x64 emulated on ARM64.
    const PEB64: Layout = Layout {
        word: 8,
        parameters: 0x20,
        environment: 0x80,
        size: 0x3F0,
    };

    /// A 32-bit x86 process under WOW64, read through its 32-bit PEB: the
    /// 64-bit one WOW64 also keeps is not where its 32-bit code maintains the
    /// environment.
    const PEB32: Layout = Layout {
        word: 4,
        parameters: 0x10,
        environment: 0x48,
        size: 0x290,
    };

    /// Upper bound on an environment block this reads, so a corrupt size in
    /// another process's memory cannot make it allocate without limit.
    const MAX_ENVIRONMENT: usize = 64 << 20;

    /// An owned process or token handle, closed on drop.
    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle is live and owned by this value.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Every process in a Toolhelp snapshot whose token user is the caller's.
    ///
    /// Refused on a 32-bit build: oneharness ships 64-bit Windows binaries only,
    /// and a 32-bit reader cannot address a 64-bit process's memory, so it
    /// would miss every native process while appearing to work.
    pub(super) fn user_pids() -> io::Result<Vec<u32>> {
        // llmlint: ignore[changed_behavior_has_e2e] Only a 32-bit Windows build takes this branch, and the CI gate matrix's one Windows leg (`windows-latest`, x64) never builds one, so no test there can reach it; the refusal is what keeps such a build from silently reporting an empty table. The WOW64 test covers the reachable half of the 32/64-bit split: a 32-bit target read by the shipped 64-bit reader.
        if cfg!(target_pointer_width = "32") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "reading other processes' environments needs a 64-bit build on Windows",
            ));
        }
        // llmlint: ignore-block[changed_behavior_has_e2e] `CreateToolhelp32Snapshot` and `Process32FirstW` fail only on resource exhaustion or a kernel refusal that an unprivileged test cannot provoke without mocking the platform layer under test; both are propagated unchanged as the scan's error. Every Windows test drives the snapshot's success path end to end.
        // SAFETY: a process snapshot takes no pointer arguments.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = Handle(snapshot);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut pids = Vec::new();
        // SAFETY: `entry` is initialized with its required `dwSize`.
        if unsafe { Process32FirstW(snapshot.0, &mut entry) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // llmlint: ignore-end[changed_behavior_has_e2e]
        let own = token_user(current_process_token()?.0)?;
        loop {
            let pid = entry.th32ProcessID;
            if pid != 0 && same_user(pid, &own) {
                pids.push(pid);
            }
            // SAFETY: as for `Process32FirstW`.
            if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
                // Read at once, before any other call can overwrite it.
                let error = io::Error::last_os_error();
                // llmlint: ignore[changed_behavior_has_e2e] `Process32NextW` fails with anything but `ERROR_NO_MORE_FILES` only on a corrupt or exhausted snapshot, which an unprivileged test cannot provoke without mocking the platform layer under test; the error is propagated as the scan's, so a partial table is never reported as complete. Every Windows test drives the exhaustion path, which ends each successful listing.
                if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
                    return Err(error);
                }
                return Ok(pids);
            }
        }
    }

    /// Whether `pid`'s environment block holds the entry.
    pub(super) fn carries(pid: u32, needle: &Needle) -> bool {
        // llmlint: ignore[changed_behavior_has_e2e] A same-user process refuses query and read rights only when it has rewritten its own DACL against its owner, a protection no test fixture should model; the outcome is `false`, which skips the process. Another user's process is excluded earlier, by `same_user`.
        open(pid, 0).is_some_and(|process| holds(&process, needle))
    }

    /// Windows has no request-to-stop for an arbitrary process, so asking is
    /// terminating.
    pub(super) fn request_stop(pid: u32, needle: &Needle) -> bool {
        kill(pid, needle)
    }

    // llmlint: ignore-block[changed_behavior_has_e2e] A refused open is driven end to end on Windows by `a_marked_child_started_while_stopping_is_stopped_too`, whose parent denies PROCESS_TERMINATE on itself until its late child is stopped: the pid is then not recorded and is retried on the next scan. `TerminateProcess` failing on a handle that holds PROCESS_TERMINATE needs a process already exiting at that instant, which no test can time; the pid is then not recorded, and a process still matching is reported in `survivors` by the final scan.
    /// `TerminateProcess` on `pid`, through the same handle that re-checked it
    /// against the marker, so a reused pid is never the one terminated.
    pub(super) fn kill(pid: u32, needle: &Needle) -> bool {
        let Some(process) = open(pid, PROCESS_TERMINATE) else {
            return false;
        };
        // SAFETY: `process` is a live handle opened with PROCESS_TERMINATE.
        holds(&process, needle) && unsafe { TerminateProcess(process.0, 1) } != 0
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]

    fn open(pid: u32, extra: u32) -> Option<Handle> {
        // SAFETY: OpenProcess takes no pointers; a null result is a refusal.
        let process =
            unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | extra, 0, pid) };
        (!process.is_null()).then_some(Handle(process))
    }

    fn holds(process: &Handle, needle: &Needle) -> bool {
        environment(process.0).is_some_and(|block| {
            super::terminated_entries(&block).any(|entry| entry == needle.wide)
        })
    }

    /// The process's environment block, as UTF-16 units, read through the
    /// PEB of the process's own architecture.
    fn environment(process: HANDLE) -> Option<Vec<u16>> {
        let (peb, layout) = match wow64_peb(process)? {
            Some(peb32) => (peb32, &PEB32),
            None => (native_peb(process)?, &PEB64),
        };
        // llmlint: ignore-block[changed_behavior_has_e2e] A corrupt pointer or size and a short read all come from another process's memory, and none can be staged by an unprivileged test on a live process; each ends in `None`, which skips the process, as the integration tests' decoys and protected cases show for the paths that can be staged.
        // Every address below is read out of another process, so none is
        // trusted: an overflow or an odd size is a block this cannot read.
        let parameters = read_word(process, peb.checked_add(layout.parameters)?, layout)?;
        let block = read_word(process, parameters.checked_add(layout.environment)?, layout)?;
        let size = read_word(process, parameters.checked_add(layout.size)?, layout)?;
        if block == 0 || size == 0 || size % 2 != 0 || size > MAX_ENVIRONMENT {
            return None;
        }
        let mut units = vec![0u16; size / 2];
        read(process, block, units.as_mut_ptr().cast(), units.len() * 2)?;
        Some(units)
        // llmlint: ignore-end[changed_behavior_has_e2e]
    }

    /// The 32-bit PEB of a WOW64 process; `Some(None)` for a 64-bit one, and
    /// `None` when the query itself is refused.
    fn wow64_peb(process: HANDLE) -> Option<Option<usize>> {
        let mut peb32 = 0usize;
        let mut written = 0u32;
        // SAFETY: this information class fills one pointer-sized value, which
        // `peb32` is.
        let status = unsafe {
            NtQueryInformationProcess(
                process,
                ProcessWow64Information,
                (&raw mut peb32).cast(),
                size_of::<usize>() as u32,
                &mut written,
            )
        };
        // llmlint: ignore[changed_behavior_has_e2e] The kernel refuses this query only for a handle without query rights, which `open` never returns; the process is then skipped.
        if status < 0 {
            return None;
        }
        Some((peb32 != 0).then_some(peb32))
    }

    /// The native PEB's address.
    fn native_peb(process: HANDLE) -> Option<usize> {
        let mut info = PROCESS_BASIC_INFORMATION::default();
        let mut written = 0u32;
        // SAFETY: `info` is writable for the size passed, which is the size of
        // the structure this information class fills.
        let status = unsafe {
            NtQueryInformationProcess(
                process,
                ProcessBasicInformation,
                (&raw mut info).cast(),
                size_of::<PROCESS_BASIC_INFORMATION>() as u32,
                &mut written,
            )
        };
        // llmlint: ignore[changed_behavior_has_e2e] The kernel refuses this query only for a handle without query rights, which `open` never returns; the process is then skipped.
        if status < 0 || info.PebBaseAddress.is_null() {
            return None;
        }
        Some(info.PebBaseAddress as usize)
    }

    /// One pointer or size of `layout`'s width, read little-endian, as every
    /// Windows architecture stores it.
    fn read_word(process: HANDLE, address: usize, layout: &Layout) -> Option<usize> {
        let mut bytes = [0u8; 8];
        read(process, address, bytes.as_mut_ptr().cast(), layout.word)?;
        usize::try_from(u64::from_le_bytes(bytes)).ok()
    }

    // llmlint: ignore[changed_behavior_has_e2e] A failed or short `ReadProcessMemory` needs a process whose memory changes under the read, which no test can stage on demand; the result is `None`, and the process is skipped.
    fn read(process: HANDLE, address: usize, into: *mut c_void, len: usize) -> Option<()> {
        let mut done = 0usize;
        // SAFETY: `into` is writable for `len` bytes; a failed or short read of
        // the other process's memory is reported, never trusted.
        let ok =
            unsafe { ReadProcessMemory(process, address as *const c_void, into, len, &mut done) };
        (ok != 0 && done == len).then_some(())
    }

    // llmlint: ignore-block[changed_behavior_has_e2e] These refusals are of the calling process's own token, opened with `TOKEN_QUERY` through the current-process pseudo-handle — a right every process holds over itself, so a test could only stage the failure by stripping its own token's privileges or exhausting the handle table, neither of which an unprivileged CI runner can do and recover from. Each failure is propagated as the scan's `io::Error` rather than swallowed, which is the "the listing itself failed" half of the documented contract; every Windows test drives the success path end to end.
    fn current_process_token() -> io::Result<Handle> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: the pseudo-handle is always valid; `token` is writable.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Handle(token))
    }

    /// The `TOKEN_USER` buffer of `token` (its SID lives inside the buffer).
    fn token_user(token: HANDLE) -> io::Result<Vec<u64>> {
        let mut needed = 0u32;
        // SAFETY: a null buffer of length 0 asks only for the size.
        unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }
        // `u64` units keep the buffer aligned for the `TOKEN_USER` it holds.
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        // SAFETY: `buffer` is writable for at least `needed` bytes.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(buffer)
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]

    fn sid_of(user: &[u64]) -> *mut c_void {
        // SAFETY: `user` was filled by GetTokenInformation(TokenUser), so it
        // starts with an aligned TOKEN_USER whose SID points inside it.
        unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    // llmlint: ignore-block[changed_behavior_has_e2e] A process under another account needs that account's credentials or an administrator to start, which a CI runner's test does not have, and the query and token refusals come only from such a process or one that exited mid-scan; every outcome other than a match is `false`, which leaves the pid unlisted. Every Windows test drives the matching path end to end.
    fn same_user(pid: u32, own: &[u64]) -> bool {
        // SAFETY: OpenProcess takes no pointers; a null result is a refusal.
        let process = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid) };
        if process.is_null() {
            return false;
        }
        let process = Handle(process);
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: `process` is live; `token` is writable.
        if unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut token) } == 0 {
            return false;
        }
        let token = Handle(token);
        let Ok(theirs) = token_user(token.0) else {
            return false;
        };
        // SAFETY: both SIDs point into live TOKEN_USER buffers.
        unsafe { EqualSid(sid_of(own), sid_of(&theirs)) != 0 }
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
}

// llmlint: ignore-block[changed_behavior_has_e2e] No CI leg runs a platform that is neither Unix nor Windows, so this refusal has nowhere to be tested; it keeps the crate building there, as `io::process` does, and refuses loudly instead of reporting an empty table.
#[cfg(not(any(unix, windows)))]
mod platform {
    use std::io;

    use super::Needle;

    pub(super) fn user_pids() -> io::Result<Vec<u32>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "listing another process's environment is not implemented on this platform",
        ))
    }

    pub(super) fn carries(_pid: u32, _needle: &Needle) -> bool {
        false
    }

    pub(super) fn request_stop(_pid: u32, _needle: &Needle) -> bool {
        false
    }

    pub(super) fn kill(_pid: u32, _needle: &Needle) -> bool {
        false
    }
}
// llmlint: ignore-end[changed_behavior_has_e2e]

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(name: &str, value: &str) -> ProcessMarker {
        ProcessMarker {
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn only_the_exact_entry_matches_a_block() {
        let needle = Needle::new(&marker("OH_MARK", "abc")).unwrap();
        assert!(needle.among(b"PATH=/bin\0OH_MARK=abc\0".split(|&b| b == 0)));
        assert!(!needle.among(b"OH_MARK=abcd\0".split(|&b| b == 0)));
        assert!(!needle.among(b"OH_MARK=ab\0".split(|&b| b == 0)));
        assert!(!needle.among(b"XOH_MARK=abc\0".split(|&b| b == 0)));
        assert!(!needle.among(b"oh_mark=abc\0".split(|&b| b == 0)));
    }

    #[test]
    fn only_terminated_entries_are_entries() {
        let bytes: Vec<&[u8]> = terminated_entries(b"A=1\0B=2\0C=cut".as_slice()).collect();
        assert_eq!(bytes, [&b"A=1"[..], &b"B=2"[..]]);
        // A Windows block: UTF-16, ending in an empty entry.
        let wide: Vec<u16> = "A=1\0B=2\0\0".encode_utf16().collect();
        let entries: Vec<String> = terminated_entries(&wide)
            .map(String::from_utf16_lossy)
            .collect();
        assert_eq!(entries, ["A=1", "B=2", ""]);
        let cut: Vec<u16> = "A=1\0B=tru".encode_utf16().collect();
        assert_eq!(terminated_entries(&cut).count(), 1);
    }

    #[test]
    fn nothing_matching_stops_nothing() {
        let unused = marker(
            "ONEHARNESS_MARKED_UNIT",
            &format!("unused-{}", std::process::id()),
        );
        assert!(find_marked(&unused).unwrap().is_empty());
        assert_eq!(
            stop_marked(&unused, Duration::from_secs(5)).unwrap(),
            StopReport::default()
        );
    }
}
