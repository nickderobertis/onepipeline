//! Owned subprocess trees and bounded pipe capture.
//!
//! A harness launcher is not necessarily the workload: npm shims commonly
//! start a native child which can outlive the launcher. Every spawned process is
//! therefore contained as one owned tree (a process group on Unix, a kill-on-
//! close Job Object on Windows). Timeout and streaming teardown terminate that
//! whole tree, reap the direct child, and bound pipe draining so an escaped
//! descendant can never hold the caller forever.
//!
//! An embedder can claim the same child through a [`ProcessSupervisor`], which
//! is the one thing that changes what teardown may signal: a caller that
//! re-parents the child's process group owns the tree from then on, so this
//! layer confines itself to the direct child rather than signalling a group it
//! did not create.

use std::io::{self, Read};
use std::process::{Child, ChildStdin, Command, ExitStatus};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

use crate::io::runner::ProcessSupervisor;

const TERM_GRACE: Duration = Duration::from_millis(100);
const PIPE_CLOSE_GRACE: Duration = Duration::from_millis(100);
const PIPE_DRAIN_GRACE: Duration = Duration::from_millis(250);
const PIPE_POLL_SLICE: Duration = Duration::from_millis(5);

/// A stdout read observed by a streaming caller.
pub(crate) enum PipeEvent {
    Data(Vec<u8>),
    Closed,
    Deadline,
}

/// Captured bytes after the direct child and its owned descendants are done.
pub(crate) struct Finished {
    pub stdout: String,
    pub stderr: String,
    pub stdout_observations: Vec<(Instant, Vec<u8>)>,
}

/// Why the runner is finishing a process.
pub(crate) enum Finish {
    /// The direct child exited by itself.
    Exited,
    /// The run timed out, the stream consumer stopped, or waiting failed.
    Terminate,
}

/// Resolve a program name to the binary actually spawned.
///
/// On Windows this is load-bearing: `CreateProcess` only auto-appends `.exe`, so
/// a bare name like `claude` never finds the `claude.cmd` shim that npm (and many
/// other installers) drop on `PATH`. Resolving via `which` is PATHEXT-aware, so it
/// locates the `.cmd`/`.bat` shim — and `std::process::Command`, given a path that
/// ends in `.cmd`/`.bat`, runs it through the command interpreter with the safe
/// argument escaping added in Rust 1.77 (CVE-2024-24576). Detection already uses
/// `which`, so a harness can be reported `available` yet still fail to spawn from a
/// bare name; this closes that gap. If resolution fails, fall back to the original
/// name so the spawn error stays accurate.
///
/// It lives beside [`Process::spawn`] because it is a property of spawning, not
/// of any one caller: the runner and the `usage` probes both take a bare harness
/// name from the registry, and a resolution only the runner performed left the
/// probes reporting `program not found` for an npm-installed harness that every
/// other command could run.
///
/// On non-Windows the name is returned unchanged: PATH lookup already handles
/// extensionless binaries, and we must not alter the established spawn behavior.
pub(crate) fn resolve_program(program: &str) -> std::ffi::OsString {
    #[cfg(windows)]
    {
        if let Ok(path) = which::which(program) {
            return path.into_os_string();
        }
    }
    program.into()
}

/// One direct child, all of its descendants, and both output drains.
pub(crate) struct Process {
    child: Option<Child>,
    tree: platform::Tree,
    stdout: PipeDrain,
    stderr: PipeDrain,
}

impl Process {
    /// Spawn `command` into a platform-owned process tree and start draining its
    /// already-piped stdout/stderr. Windows starts suspended, assigns the process
    /// to its Job Object, and only then resumes it, so a fast launcher cannot
    /// create an unowned descendant in the assignment race.
    pub(crate) fn spawn(command: Command) -> io::Result<Self> {
        Self::spawn_supervised(command, None)
    }

    /// [`Process::spawn`] for a child that must not outlive this process,
    /// however this process ends — SIGKILLed included, when no deadline it
    /// holds will ever fire. On Unix the child joins the process group of a
    /// guard, a fork of this process that watches its own parent pid: once
    /// this process is gone the guard is reparented, and it SIGKILLs its
    /// group, the child and its descendants with it. The guard holds no
    /// descriptor and the child none above standard error, so neither holds
    /// this process's output. On
    /// Windows the Job Object every spawn joins already kills the tree when
    /// its last handle, this process's, closes.
    pub(crate) fn spawn_tethered(mut command: Command) -> io::Result<Self> {
        let tree = platform::Tree::prepare_tethered(&mut command)?;
        Self::spawn_prepared(command, tree, None)
    }

    /// [`Process::spawn`] with the caller's own claim on the child
    /// ([`ProcessSupervisor`]). `None` is byte for byte [`Process::spawn`].
    ///
    /// Where the two hooks sit is the whole of what they promise. `spawning`
    /// runs *after* this layer has prepared its own containment and immediately
    /// before `Command::spawn`, so the `Command` it receives is the one actually
    /// spawned and a `pre_exec` it registers runs after ours (which is what lets
    /// a caller's `setpgid` win). `spawned` runs before the first wait or pipe
    /// read — and on Windows while the child is still suspended — so the caller
    /// adopts a process that is certainly still there, and on Windows one that
    /// cannot yet have created a descendant outside the caller's job.
    pub(crate) fn spawn_supervised(
        mut command: Command,
        supervisor: Option<&dyn ProcessSupervisor>,
    ) -> io::Result<Self> {
        let tree = platform::Tree::prepare(&mut command)?;
        Self::spawn_prepared(command, tree, supervisor)
    }

    fn spawn_prepared(
        mut command: Command,
        mut tree: platform::Tree,
        supervisor: Option<&dyn ProcessSupervisor>,
    ) -> io::Result<Self> {
        if let Some(supervisor) = supervisor {
            supervisor.spawning(&mut command);
        }
        let mut child = command.spawn()?;
        if let Err(error) = tree.attach(&mut child) {
            tree.terminate(&mut child, TERM_GRACE);
            return Err(error);
        }
        if let Some(supervisor) = supervisor {
            supervisor.spawned(&child);
        }
        if let Err(error) = tree.start(&mut child) {
            tree.terminate(&mut child, TERM_GRACE);
            return Err(error);
        }

        let stdout = child.stdout.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "child stdout was not piped")
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "child stderr was not piped")
        })?;

        Ok(Self {
            child: Some(child),
            tree,
            stdout: PipeDrain::spawn(stdout),
            stderr: PipeDrain::spawn(stderr),
        })
    }

    pub(crate) fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.as_mut().and_then(|child| child.stdin.take())
    }

    /// Wait until `deadline` for the direct child. `None` means it did not exit
    /// by the deadline; an error is a wait failure. The deadline check happens
    /// before polling, so output that looks complete never extends the runtime.
    pub(crate) fn wait_until(&mut self, deadline: Instant) -> io::Result<Option<ExitStatus>> {
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        self.child
            .as_mut()
            .expect("live process has a child")
            .wait_timeout(deadline - now)
    }

    /// Receive one stdout chunk for incremental parsing while preserving the
    /// exact bytes for the final capture. A separate stderr reader continuously
    /// moves its pipe into a channel, so it can never backpressure the child.
    pub(crate) fn recv_stdout_until(&mut self, deadline: Instant) -> PipeEvent {
        self.stdout.recv_until(deadline)
    }

    /// Finish the owned tree and return its best-effort output. Pipe drain is
    /// bounded even if a process somehow escaped containment and kept an
    /// inherited handle open.
    pub(crate) fn finish(mut self, reason: Finish) -> Finished {
        match reason {
            Finish::Exited => {
                let close_deadline = Instant::now() + PIPE_CLOSE_GRACE;
                self.drain_pipes_until(close_deadline);
                if !self.pipes_closed() {
                    self.terminate();
                }
            }
            Finish::Terminate => self.terminate(),
        }

        self.drain_pipes_until(Instant::now() + PIPE_DRAIN_GRACE);
        let child = self.child.take();
        drop(child);
        Finished {
            stdout: self.stdout.take_string(),
            stderr: self.stderr.take_string(),
            stdout_observations: self.stdout.take_observations(),
        }
    }

    fn terminate(&mut self) {
        if let Some(child) = self.child.as_mut() {
            self.tree.terminate(child, TERM_GRACE);
        }
    }

    fn drain_pipes_until(&mut self, deadline: Instant) {
        // Alternate in short slices so a continuously-writing escaped stdout
        // cannot starve stderr, and make every queued-byte read obey the same
        // absolute deadline. Reader threads may outlive this bound, but dropping
        // their receivers makes them exit on their next send.
        while Instant::now() < deadline && !self.pipes_closed() {
            self.stdout.drain_one_until(poll_deadline(deadline));
            self.stderr.drain_one_until(poll_deadline(deadline));
        }
    }

    fn pipes_closed(&self) -> bool {
        self.stdout.closed && self.stderr.closed
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Covers an unwind or any future early return between spawn and finish.
        if let Some(mut child) = self.child.take() {
            self.tree.terminate(&mut child, TERM_GRACE);
        }
    }
}

enum PipeMessage {
    Data(Instant, Vec<u8>),
    Closed,
}

struct PipeDrain {
    receiver: Receiver<PipeMessage>,
    bytes: Vec<u8>,
    observations: Vec<(Instant, Vec<u8>)>,
    closed: bool,
}

impl PipeDrain {
    fn spawn<R>(mut reader: R) -> Self
    where
        R: Read + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            let mut pending = Vec::new();
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        pending.extend_from_slice(&chunk[..count]);
                        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                            let record: Vec<u8> = pending.drain(..=newline).collect();
                            if sender
                                .send(PipeMessage::Data(Instant::now(), record))
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            }
            if !pending.is_empty()
                && sender
                    .send(PipeMessage::Data(Instant::now(), pending))
                    .is_err()
            {
                return;
            }
            let _ = sender.send(PipeMessage::Closed);
        });
        Self {
            receiver,
            bytes: Vec::new(),
            observations: Vec::new(),
            closed: false,
        }
    }

    fn recv_until(&mut self, deadline: Instant) -> PipeEvent {
        if self.closed {
            return PipeEvent::Closed;
        }
        let now = Instant::now();
        if now >= deadline {
            return PipeEvent::Deadline;
        }
        match self.receiver.recv_timeout(deadline - now) {
            Ok(PipeMessage::Data(observed_at, chunk)) => {
                self.bytes.extend_from_slice(&chunk);
                self.observations.push((observed_at, chunk.clone()));
                PipeEvent::Data(chunk)
            }
            Ok(PipeMessage::Closed) | Err(RecvTimeoutError::Disconnected) => {
                self.closed = true;
                PipeEvent::Closed
            }
            Err(RecvTimeoutError::Timeout) => PipeEvent::Deadline,
        }
    }

    fn drain_one_until(&mut self, deadline: Instant) {
        if self.closed {
            return;
        }
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        match self.receiver.recv_timeout(deadline - now) {
            Ok(PipeMessage::Data(observed_at, chunk)) => {
                self.bytes.extend_from_slice(&chunk);
                self.observations.push((observed_at, chunk));
            }
            Ok(PipeMessage::Closed) | Err(RecvTimeoutError::Disconnected) => {
                self.closed = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }

    fn take_string(&mut self) -> String {
        String::from_utf8_lossy(&std::mem::take(&mut self.bytes)).into_owned()
    }

    fn take_observations(&mut self) -> Vec<(Instant, Vec<u8>)> {
        std::mem::take(&mut self.observations)
    }
}

fn poll_deadline(deadline: Instant) -> Instant {
    deadline.min(Instant::now() + PIPE_POLL_SLICE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[cfg(unix)]
    use std::path::Path;

    #[test]
    fn coalesced_pipe_read_is_observed_as_complete_records() {
        let mut drain = PipeDrain::spawn(Cursor::new(b"one\ntwo\nthree\n".to_vec()));
        let deadline = Instant::now() + Duration::from_secs(1);
        while !matches!(drain.recv_until(deadline), PipeEvent::Closed) {}
        let observations = drain.take_observations();
        assert_eq!(observations.len(), 3);
        assert_eq!(observations[0].1, b"one\n");
        assert_eq!(observations[2].1, b"three\n");
    }

    #[test]
    fn resolve_program_falls_back_to_the_name_when_unresolvable() {
        // A name that PATH lookup cannot resolve must come back unchanged on every
        // platform, so the spawn attempt — and its error message — stay accurate.
        let name = "oneharness-no-such-binary-zzz";
        assert_eq!(resolve_program(name), std::ffi::OsString::from(name));
    }

    /// Set when this test binary is re-executed as a tethered child's caller:
    /// the directory the caller and the journey share.
    #[cfg(unix)]
    const TETHERED_CALLER: &str = "ONEHARNESS_TETHERED_CALLER_DIR";

    #[cfg(unix)]
    const COMPETING_SPAWN_TEST: &str =
        "io::process::tests::a_tethered_child_ends_with_its_caller_whatever_a_competing_spawn_inherited";

    /// A tethered child ends with its caller even when another child of the
    /// caller holds every descriptor the caller had — as one does that another
    /// thread forked before a descriptor being made could be marked
    /// close-on-exec, which Darwin cannot do atomically for a pipe. This test
    /// binary is re-executed as the caller: it starts a tethered shell that
    /// starts a descendant, then a competing child holding all its descriptors
    /// (its output aside), and waits. The journey holds the caller's output at
    /// descriptors 3 and 4 too, SIGKILLs it, and requires its output to end and
    /// the tethered shell, its descendant and its whole process group to end
    /// within the bound, while the competing child — no tether's — lives on.
    #[cfg(unix)]
    #[test]
    fn a_tethered_child_ends_with_its_caller_whatever_a_competing_spawn_inherited() {
        use crate::io::scratch::ScratchDir;
        use crate::io::server_pool::{pid_alive, Pid};
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;

        /// Pids this journey captured, SIGKILLed on the way out should it fail
        /// with them still running; never any other.
        struct Captured(Vec<u32>);
        impl Drop for Captured {
            fn drop(&mut self) {
                for pid in &self.0 {
                    if alive(*pid) {
                        // SAFETY: plain integers; a pid this journey captured.
                        unsafe { libc::kill(*pid as libc::pid_t, libc::SIGKILL) };
                    }
                }
            }
        }
        fn alive(pid: u32) -> bool {
            Pid::new(pid).is_ok_and(pid_alive)
        }
        fn read_to_end(mut pipe: impl Read + Send + 'static) -> Receiver<Vec<u8>> {
            let (sender, ends) = mpsc::channel();
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = pipe.read_to_end(&mut bytes);
                let _ = sender.send(bytes);
            });
            ends
        }
        fn wait_for(label: &str, limit: Duration, mut done: impl FnMut() -> bool) {
            let deadline = Instant::now() + limit;
            while !done() {
                assert!(
                    Instant::now() < deadline,
                    "timed out after {limit:?} waiting for {label}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        if let Some(dir) = std::env::var_os(TETHERED_CALLER) {
            be_the_tethered_caller(Path::new(&dir));
        }

        let scratch = ScratchDir::new("tether-competing-spawn").unwrap();
        let mut caller = Command::new(std::env::current_exe().unwrap());
        caller
            .args([
                COMPETING_SPAWN_TEST,
                "--exact",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env(TETHERED_CALLER, scratch.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: the hook runs in the forked child before exec, after its
        // standard streams are in place, and calls only async-signal-safe
        // `dup2`; the copies it makes are not close-on-exec.
        unsafe {
            caller.pre_exec(|| {
                if libc::dup2(1, 3) == -1 || libc::dup2(2, 4) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut caller = caller.spawn().unwrap();
        let mut captured = Captured(vec![caller.id()]);
        let stdout = read_to_end(caller.stdout.take().unwrap());
        let stderr = read_to_end(caller.stderr.take().unwrap());
        let pids = scratch.path().join("pids");
        wait_for(
            "the caller to start its children",
            Duration::from_secs(30),
            || pids.exists(),
        );
        let pids: Vec<u32> = std::fs::read_to_string(&pids)
            .unwrap()
            .split_whitespace()
            .map(|pid| pid.parse().unwrap())
            .collect();
        captured.0.extend(&pids);
        let [tethered, descendant, competing] = pids[..] else {
            panic!("the caller wrote {pids:?}, not three pids");
        };
        // SAFETY: `getpgid` only reads the process table.
        let group = unsafe { libc::getpgid(tethered as libc::pid_t) };
        assert!(group > 0, "{}", io::Error::last_os_error());

        // SAFETY: plain integers; the caller is this journey's own child.
        unsafe { libc::kill(caller.id() as libc::pid_t, libc::SIGKILL) };
        caller.wait().unwrap();
        let bound = Duration::from_secs(5);
        stdout
            .recv_timeout(bound)
            .expect("the killed caller's standard output is held open past its end");
        stderr
            .recv_timeout(bound)
            .expect("the killed caller's standard error is held open past its end");
        wait_for("the tethered process group to end", bound, || {
            // SAFETY: signal 0 to a group delivers nothing.
            !alive(tethered) && !alive(descendant) && unsafe { libc::kill(-group, 0) } == -1
        });
        assert!(
            alive(competing),
            "the competing child did not outlive the caller, so nothing was proven"
        );
    }

    /// The caller of [`a_tethered_child_ends_with_its_caller_whatever_a_competing_spawn_inherited`]:
    /// never returns; the journey SIGKILLs it.
    #[cfg(unix)]
    fn be_the_tethered_caller(dir: &Path) -> ! {
        use std::ffi::CString;
        use std::process::Stdio;

        let started = dir.join("tethered");
        let mut tethered = Command::new("/bin/sh");
        tethered
            .args([
                "-c",
                r#"sleep 600 & echo "$$ $!" >"$1.partial" && mv "$1.partial" "$1"; wait"#,
            ])
            .arg("tethered")
            .arg(&started)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let _tethered = Process::spawn_tethered(tethered).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !started.exists() {
            assert!(
                Instant::now() < deadline,
                "the tethered shell never started"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        // The competing child: forked and exec'd by hand, not through `std`,
        // whose exec-status pipe would otherwise lose its close-on-exec flag
        // below. Everything it needs is built before the fork.
        let sleep = CString::new("/bin/sleep").unwrap();
        let seconds = CString::new("600").unwrap();
        let argv = [sleep.as_ptr(), seconds.as_ptr(), std::ptr::null()];
        let envp = [std::ptr::null()];
        let open_max = descriptor_bound();
        let null = std::fs::File::open("/dev/null").unwrap();
        let null = std::os::fd::AsRawFd::as_raw_fd(&null);
        // SAFETY: the child calls only async-signal-safe `dup2`, `close`,
        // `fcntl`, `execve` and `_exit`, on data prepared before the fork.
        let competing = unsafe { libc::fork() };
        if competing == 0 {
            // SAFETY: as above.
            unsafe {
                // None of the caller's output, which the journey reads to its
                // end: that would be the competing child's hold, not a tether's.
                for fd in 0..=2 {
                    libc::dup2(null, fd);
                }
                libc::close(3);
                libc::close(4);
                for fd in 5..open_max {
                    libc::fcntl(fd, libc::F_SETFD, 0);
                }
                libc::execve(sleep.as_ptr(), argv.as_ptr(), envp.as_ptr());
                libc::_exit(127);
            }
        }
        assert!(competing > 0, "{}", io::Error::last_os_error());

        let mut pids = std::fs::read_to_string(&started)
            .unwrap()
            .trim()
            .to_string();
        pids.push_str(&format!(" {competing}"));
        let partial = dir.join("pids.partial");
        std::fs::write(&partial, pids).unwrap();
        // The journey SIGKILLs this process as soon as the pids are in, and a
        // killed process writes no coverage profile: write it while it can.
        write_coverage_profile();
        std::fs::rename(&partial, dir.join("pids")).unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    /// Write this process's coverage counters to its profile now, in an
    /// instrumented build; nothing otherwise.
    // `coverage` is the cfg cargo-llvm-cov sets on instrumented builds alone,
    // which no manifest of this workspace declares.
    #[allow(unexpected_cfgs)]
    #[cfg(unix)]
    fn write_coverage_profile() {
        #[cfg(coverage)]
        {
            extern "C" {
                fn __llvm_profile_write_file() -> libc::c_int;
            }
            // SAFETY: the instrumented build's profiler runtime defines it;
            // it writes this process's counters to its profile file.
            unsafe { __llvm_profile_write_file() };
        }
    }

    #[cfg(windows)]
    #[test]
    fn resolve_program_finds_a_cmd_shim_on_windows() {
        // `where` is a stock Windows console command living at a real path; the
        // bare name must resolve to an absolute, existing file (the gap that left
        // npm `.cmd` shims unspawnable from a bare name).
        let resolved = resolve_program("where");
        let path = std::path::Path::new(&resolved);
        assert!(
            path.is_absolute(),
            "expected an absolute path, got {resolved:?}"
        );
        assert!(path.exists(), "resolved path should exist: {resolved:?}");
    }
}

/// Mark every descriptor above standard error close-on-exec in `command`'s
/// child, so it holds only the three standard ones it was given. A caller's
/// descriptors are not all close-on-exec — a launcher that passed its own
/// output on at another number, or, on macOS, a pipe or socket another thread
/// had just made (`std` marks those close-on-exec only after creating them) —
/// and a child that outlives its caller holding one end of the caller's output
/// pipe keeps whoever reads that pipe from ever seeing end-of-file.
#[cfg(unix)]
pub(crate) fn close_inherited_on_exec(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // Read before the fork: `getrlimit` is not async-signal-safe.
    let open_max = descriptor_bound();
    // SAFETY: the hook runs in the forked child before exec and calls only
    // `syscall(close_range)` and `fcntl`, which are async-signal-safe, and
    // allocates nothing.
    unsafe {
        command.pre_exec(move || {
            close_above_stdio_on_exec(open_max);
            Ok(())
        });
    }
}

/// One past the highest descriptor number this process may hold, capped so a
/// limit reported as unbounded does not cost a spawn millions of calls.
#[cfg(unix)]
fn descriptor_bound() -> libc::c_int {
    const CAP: libc::rlim_t = 1 << 20;
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a live, writable `rlimit` for the call.
    let read = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0;
    let bound = if read && limit.rlim_cur != libc::RLIM_INFINITY {
        limit.rlim_cur.min(CAP)
    } else {
        CAP
    };
    libc::c_int::try_from(bound).unwrap_or(libc::c_int::MAX)
}

/// Mark every descriptor above standard error close-on-exec, in a forked
/// child about to exec. `std`'s own exec-error pipe is close-on-exec already.
#[cfg(unix)]
fn close_above_stdio_on_exec(open_max: libc::c_int) {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: plain integer arguments; a kernel without `close_range`
        // (before 5.11 for this flag) fails it, and the loop below runs.
        let marked = unsafe {
            libc::syscall(
                libc::SYS_close_range,
                3u32,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            )
        };
        if marked == 0 {
            return;
        }
    }
    for fd in 3..open_max {
        // SAFETY: plain integers; a number naming no open descriptor fails
        // with EBADF, which is ignored.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};
    use std::time::Duration;

    use wait_timeout::ChildExt;

    pub(super) struct Tree {
        owned: Option<Owned>,
        /// The guard whose group the child joins, for a tethered spawn.
        tether: Option<Tether>,
    }

    /// The guard of a tethered spawn ([`super::Process::spawn_tethered`]): a
    /// bare fork of this process leading the child's process group, which
    /// SIGKILLs that group as soon as this process is gone.
    ///
    /// It learns that from its own parent pid, which the kernel changes when
    /// this process ends however it ends — the guard is reparented — and
    /// which nothing else can hold unchanged: no descriptor stands for this
    /// process's life, so no child holding one (one another thread forked
    /// before a descriptor could be marked close-on-exec, say) can keep the
    /// guard waiting. Nor can its parent pid come back: it is reparented to a
    /// running process, never to the dead one's pid. While the guard is
    /// unreaped its pid, and so the group's id, cannot be reused, so signalling
    /// the group never reaches a stranger.
    struct Tether {
        guard: libc::pid_t,
    }

    /// How often the guard looks at its parent pid: a probe outlives a killed
    /// caller by at most this long, while a guard, which lives only as long as
    /// a seconds-long probe, wakes a negligible 20 times a second.
    const GUARD_POLL_US: libc::suseconds_t = 50_000;

    impl Tether {
        fn start() -> io::Result<Self> {
            // Read before the fork: nothing in the guard may allocate or take a
            // lock another thread could have held.
            // SAFETY: `getpid` only reads this process's id.
            let caller = unsafe { libc::getpid() };
            let open_max = super::descriptor_bound();
            // SAFETY: the child runs only `guard`, which calls nothing but
            // async-signal-safe functions, as a child of a multithreaded
            // process must until it exits.
            match unsafe { libc::fork() } {
                -1 => Err(io::Error::last_os_error()),
                // SAFETY: as above; `guard` never returns.
                0 => unsafe { guard(caller, open_max) },
                guard => {
                    // The guard leads its group from here, whichever of this
                    // and its own `setpgid` runs first, so a child spawned
                    // next can always join it.
                    // SAFETY: plain integers naming this process's own child.
                    unsafe { libc::setpgid(guard, guard) };
                    Ok(Self { guard })
                }
            }
        }

        fn group(&self) -> libc::pid_t {
            self.guard
        }
    }

    impl Drop for Tether {
        fn drop(&mut self) {
            // The child is done with, or never started: end whatever is left
            // in the group, the guard included, then reap the guard — only now,
            // so until here the group's id stayed reserved.
            signal_group(self.guard, libc::SIGKILL);
            let mut status = 0;
            // SAFETY: `status` is writable; the guard is this process's child.
            unsafe { libc::waitpid(self.guard, &mut status, 0) };
        }
    }

    /// The guard's whole life, in the forked child: lead a group of its own,
    /// hold no descriptor and no inherited signal handling, wait for the
    /// caller to be gone, then SIGKILL the group. Every call is
    /// async-signal-safe — `syscall(close_range)` is the plain system call that
    /// `close` makes, in bulk — and nothing allocates.
    unsafe fn guard(caller: libc::pid_t, open_max: libc::c_int) -> ! {
        libc::setpgid(0, 0);
        #[cfg(target_os = "linux")]
        let closed = libc::syscall(libc::SYS_close_range, 0u32, u32::MAX, 0) == 0;
        #[cfg(not(target_os = "linux"))]
        let closed = false;
        if !closed {
            for fd in 0..open_max {
                libc::close(fd);
            }
        }
        // A handler of the caller's would run here on a stop meant for the
        // group: take the defaults back, and block nothing.
        let mut default: libc::sigaction = std::mem::zeroed();
        default.sa_sigaction = libc::SIG_DFL;
        for signal in 1..32 {
            if signal != libc::SIGKILL && signal != libc::SIGSTOP {
                libc::sigaction(signal, &default, std::ptr::null_mut());
            }
        }
        let mut none: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut none);
        libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
        // `select` with no descriptor and a timeout is the documented
        // portable sleep (select(2) on Linux and Darwin alike).
        while libc::getppid() == caller {
            let mut wait = libc::timeval {
                tv_sec: 0,
                tv_usec: GUARD_POLL_US,
            };
            libc::select(
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut wait,
            );
        }
        libc::kill(0, libc::SIGKILL);
        libc::_exit(0)
    }

    /// What this run may signal when it tears the child down.
    enum Owned {
        /// The child leads the process group `prepare` asked for, so the group
        /// holds exactly this run's harness and its descendants: signal it whole.
        Group(libc::pid_t),
        /// A `spawning` hook moved the child into a process group this run did
        /// not create. That group is the caller's and may hold the caller's own
        /// processes, so signalling it is not ours to do — only the direct child
        /// is. The caller owns the rest of the tree it took responsibility for.
        DirectChild(libc::pid_t),
    }

    impl Tree {
        pub(super) fn prepare(command: &mut Command) -> io::Result<Self> {
            // SAFETY: `pre_exec` runs after fork in the child. The closure calls
            // only async-signal-safe `setpgid`, captures nothing, and reports the
            // OS error directly. Making the child its own group leader before
            // exec closes the descendant-escape race.
            unsafe {
                command.pre_exec(|| {
                    if libc::setpgid(0, 0) == -1 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
            Ok(Self {
                owned: None,
                tether: None,
            })
        }

        /// [`Tree::prepare`], but the child joins a fresh [`Tether`]'s group
        /// rather than leading its own, and inherits no descriptor above
        /// standard error.
        pub(super) fn prepare_tethered(command: &mut Command) -> io::Result<Self> {
            let tether = Tether::start()?;
            let group = tether.group();
            // SAFETY: as in `prepare`; the guard leads `group` and stays
            // alive, polling for its caller's end, until `tether` is dropped.
            unsafe {
                command.pre_exec(move || {
                    if libc::setpgid(0, group) == -1 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
            super::close_inherited_on_exec(command);
            Ok(Self {
                owned: None,
                tether: Some(tether),
            })
        }

        pub(super) fn attach(&mut self, child: &mut Child) -> io::Result<()> {
            let pid: libc::pid_t = child.id().try_into().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "child PID does not fit pid_t")
            })?;
            // Read the group the child is really in rather than assuming the
            // `prepare` above had the last word: a caller's `spawning` hook can
            // register its own `setpgid`, which runs after ours. `spawn` has
            // already returned, so both closures are done and this answer is
            // final. A query failure means a pid that no longer exists, which an
            // unreaped child is not — keep the group we asked for there, since
            // the alternative silently drops descendant teardown.
            // SAFETY: `getpgid` only reads the process table for `pid`.
            let group = unsafe { libc::getpgid(pid) };
            let asked = self.tether.as_ref().map_or(pid, Tether::group);
            self.owned = Some(if group == asked || group == -1 {
                Owned::Group(asked)
            } else {
                Owned::DirectChild(pid)
            });
            Ok(())
        }

        pub(super) fn start(&mut self, _child: &mut Child) -> io::Result<()> {
            // The child was never suspended: `pre_exec` grouping needs no window.
            Ok(())
        }

        pub(super) fn terminate(&mut self, child: &mut Child, grace: Duration) {
            match self.owned {
                Some(Owned::Group(group)) => {
                    signal_group(group, libc::SIGTERM);
                    let _ = child.wait_timeout(grace);
                    // Always follow with KILL: the direct child may have honored
                    // TERM while a descendant ignored it and retained the pipes.
                    signal_group(group, libc::SIGKILL);
                }
                Some(Owned::DirectChild(pid)) => {
                    // SAFETY: a positive PID addresses only that one process.
                    // ESRCH (it already exited) is expected and best-effort.
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                    let _ = child.wait_timeout(grace);
                    let _ = child.kill();
                }
                None => {
                    let _ = child.kill();
                }
            }
            let _ = child.wait();
        }
    }

    fn signal_group(group: libc::pid_t, signal: libc::c_int) {
        // SAFETY: a negative PID addresses the process group created above.
        // ESRCH is expected when every member already exited; all failures are
        // best-effort because reaping/drain bounds still guarantee our return.
        unsafe {
            libc::kill(-group, signal);
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::io;
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};
    use std::ptr;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenThread, ResumeThread, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
    };

    pub(super) struct Tree {
        job: HANDLE,
    }

    impl Tree {
        pub(super) fn prepare(command: &mut Command) -> io::Result<Self> {
            // SAFETY: null security/name pointers request an unnamed Job Object
            // with default security. The returned handle is owned by `Tree`.
            let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `limits` has the exact structure/size required by this
            // information class and remains alive for the duration of the call.
            let configured = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if configured == 0 {
                let error = io::Error::last_os_error();
                // SAFETY: `job` is a live handle owned by this function.
                unsafe { CloseHandle(job) };
                return Err(error);
            }

            // `std::process::Command` ORs these flags into its CreateProcessW
            // call. The primary thread cannot spawn children before assignment.
            command.creation_flags(CREATE_SUSPENDED);
            Ok(Self { job })
        }

        /// Every Windows spawn is already tethered: its Job Object kills the
        /// tree once its last handle — this process's — closes.
        pub(super) fn prepare_tethered(command: &mut Command) -> io::Result<Self> {
            Self::prepare(command)
        }

        pub(super) fn attach(&mut self, child: &mut Child) -> io::Result<()> {
            let process = child.as_raw_handle() as HANDLE;
            // SAFETY: both handles are live; the process is still suspended.
            if unsafe { AssignProcessToJobObject(self.job, process) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        /// Resume the child, once it is in this run's Job Object and any
        /// caller-owned one a `spawned` hook added it to. Splitting this from
        /// [`Tree::attach`] is what gives that hook the suspended window: a job
        /// assigned after the first instruction runs can already have missed a
        /// descendant.
        pub(super) fn start(&mut self, child: &mut Child) -> io::Result<()> {
            resume_primary_thread(child.id())
        }

        pub(super) fn terminate(&mut self, child: &mut Child, _grace: Duration) {
            // Windows has no general graceful signal for an arbitrary console
            // subtree. Terminating the Job atomically covers every descendant.
            // SAFETY: `self.job` remains live until `Drop`.
            unsafe { TerminateJobObject(self.job, 1) };
            let _ = child.wait();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            // KILL_ON_JOB_CLOSE is the final containment backstop. At normal
            // finish the processes are already reaped, so this is a no-op there.
            // SAFETY: `job` is owned by `Tree` and closed exactly once.
            unsafe { CloseHandle(self.job) };
        }
    }

    fn resume_primary_thread(process_id: u32) -> io::Result<()> {
        // A CREATE_SUSPENDED process has only its primary thread. Enumerating it
        // after Job assignment lets us resume without relying on undocumented
        // process APIs; no application code can run during this window.
        // SAFETY: ToolHelp returns an owned snapshot handle.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let result = find_and_resume(snapshot, process_id);
        // SAFETY: `snapshot` is owned here and closed exactly once.
        unsafe { CloseHandle(snapshot) };
        result
    }

    fn find_and_resume(snapshot: HANDLE, process_id: u32) -> io::Result<()> {
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..THREADENTRY32::default()
        };
        // SAFETY: `entry` has the documented size and remains writable.
        let mut present = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        while present {
            if entry.th32OwnerProcessID == process_id {
                // SAFETY: the thread id came from the live snapshot. The handle
                // is opened with only the access needed to resume it.
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if thread.is_null() {
                    return Err(io::Error::last_os_error());
                }
                // SAFETY: `thread` is a live thread handle.
                let resumed = unsafe { ResumeThread(thread) };
                // SAFETY: `thread` is owned here and closed exactly once.
                unsafe { CloseHandle(thread) };
                if resumed == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            // SAFETY: same initialized snapshot/entry contract as Thread32First.
            present = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "suspended child primary thread was not found",
        ))
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use std::io;
    use std::process::{Child, Command};
    use std::time::Duration;

    pub(super) struct Tree;

    impl Tree {
        pub(super) fn prepare(_command: &mut Command) -> io::Result<Self> {
            Ok(Self)
        }

        pub(super) fn prepare_tethered(command: &mut Command) -> io::Result<Self> {
            Self::prepare(command)
        }

        pub(super) fn attach(&mut self, _child: &mut Child) -> io::Result<()> {
            Ok(())
        }

        pub(super) fn start(&mut self, _child: &mut Child) -> io::Result<()> {
            Ok(())
        }

        pub(super) fn terminate(&mut self, child: &mut Child, _grace: Duration) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
