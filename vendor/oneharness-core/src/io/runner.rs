//! Spawning harness subprocesses with timeouts and bounded parallelism.
//!
//! This layer only spawns and captures — it does no parsing, so the spawn path
//! and the extraction path stay independently testable. stdout and stderr are
//! drained on their own threads so a chatty harness can never deadlock the wait.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::domain::report::{Capture, OutputObservation, Status};
use crate::io::cancel::{cancellation_requested, CancelToken};
use crate::io::control::ControlHandle;
use crate::io::process::{resolve_program, Finish, PipeEvent, Process};

/// How long a wait/read blocks before the run re-checks for cancellation.
///
/// Cancellation is only as fast as this slice, and a harness that produces no
/// output at all reaches the check no other way: without it the run stays parked
/// in the pipe read until its own deadline, and a cancelled harness keeps
/// running (and billing). Short enough to feel immediate, long enough that the
/// wakeups cost nothing next to a model call.
pub(crate) const CANCEL_POLL_SLICE: Duration = Duration::from_millis(50);

/// A fully-specified subprocess to run.
#[derive(Clone)]
pub struct Job {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    // llmlint: ignore[invalid_states_unrepresentable] Jobs are internal spawn plans built only after config validation checks every environment name; retaining Command's native string shape here avoids a second conversion/type at the I/O boundary, and real-spawn tests assert masking and parent isolation.
    pub env_remove: Vec<String>,
    pub timeout: Duration,
    /// Bytes to pipe to the child's stdin, when the prompt is delivered that way
    /// instead of on the argv (the large-prompt escape hatch — a prompt too big
    /// for a single argv string trips `E2BIG` at spawn). `None` leaves stdin
    /// closed (`Stdio::null()`), the ordinary case. When `Some`, it is written on
    /// a dedicated thread so a child that fills its stdout can never deadlock the
    /// write.
    pub stdin: Option<String>,
}

/// The next attempt a retry policy asks for: a fresh argv and its stdin (the
/// prompt may ride stdin for a large-prompt run, so a re-prompt has to rewrite
/// both). Reuses the original job's cwd/env/timeout.
pub struct NextRun {
    pub argv: Vec<String>,
    pub stdin: Option<String>,
}

/// The caller's own claim on every harness child a run spawns: the seam for
/// putting each one into the **process group (POSIX) or job object (Windows)
/// the embedder already supervises**.
///
/// A consumer that drove the `oneharness` binary as a subprocess got that
/// grouping from the OS for free — the whole harness tree hung under one child
/// it owned. Calling [`crate::io::run::run`] in-process removes that hop, and
/// with it the two things the group bought: an activity watchdog can no longer
/// see the harness subtree as one unit (a busy member reads as idle and is
/// condemned), and the caller's own kill no longer reaps the tree (a killed run
/// leaves paid harness processes running and billing). These two hooks give
/// both back. They are not general-purpose spawn callbacks: grouping and
/// teardown are what they exist for.
///
/// Both methods default to doing nothing, so an implementation writes only the
/// half it needs, and a run driven through plain [`crate::io::run::run`] — which
/// passes no supervisor — behaves exactly as it did before this existed. The
/// trait is [`Sync`] because
/// a parallel run spawns its harnesses from several threads at once, sharing
/// one supervisor across them.
///
/// # Which side owns teardown
///
/// oneharness contains every child it spawns in a tree of its own and tears
/// that tree down on a timeout, a consumer stop, or a
/// [`crate::io::cancel::CancelToken`]. Setting a hook does not switch that off:
///
/// * **Observing hooks** — recording the pid and its group, adding the child to
///   a Windows job object of your own — leave the child leading the group
///   oneharness created, so oneharness's teardown still ends the whole
///   descendant tree, exactly as with no supervisor at all.
/// * **A `spawning` hook that re-parents the child's process group** (a
///   `pre_exec` `setpgid` into a group of yours) makes that group yours, and
///   with it the descendants that inherit it. oneharness will not signal a
///   process group it did not create — yours can hold your own processes,
///   including the caller itself — so its teardown confines itself to the
///   direct child (TERM, a grace, then KILL) and reaping the rest of that tree
///   is yours to do.
///
///   That second `setpgid` really does take effect, despite oneharness having
///   already made the child a group leader: POSIX refuses the call only for a
///   **session** leader, and a group leader may join another group in its own
///   session. `tests/library.rs` does not take that on trust — it asks the OS
///   for the child's group afterwards, on every Unix platform in the matrix.
///
/// Windows needs no such division: a job assignment nests, so the child stays
/// in oneharness's job as well as yours and either side's teardown ends it.
///
/// Hand one to a run through [`crate::io::run::run_supervised`].
///
/// ```no_run
/// use std::process::Child;
/// use oneharness_core::io::runner::ProcessSupervisor;
///
/// struct Watchdog;
/// impl ProcessSupervisor for Watchdog {
///     fn spawned(&self, child: &Child) {
///         // The harness leads its own process group, so this pid is also the
///         // pgid a watchdog can poll and a kill can reap.
///         eprintln!("harness pid {}", child.id());
///     }
/// }
/// ```
pub trait ProcessSupervisor: Sync {
    /// The last look at the [`Command`] before it is spawned — and it is the
    /// exact one that is spawned, program, argv, working directory and
    /// environment included, so a `pre_exec` (Unix) or `creation_flags`
    /// (Windows) registered here takes effect on the harness process itself.
    ///
    /// oneharness has already prepared its own containment on this `Command`
    /// when the hook runs: a `pre_exec` that makes the child a process-group
    /// leader on Unix, `CREATE_SUSPENDED` on Windows. A `pre_exec` added here
    /// therefore runs *after* oneharness's, which is exactly what lets a
    /// `setpgid` into the caller's own group win — and hands the caller that
    /// tree's teardown, as described above. On Windows, `creation_flags`
    /// *replaces* the flags rather than adding to them, so a hook that sets any
    /// must include `CREATE_SUSPENDED` or the job assignment loses the window
    /// that keeps a fast launcher's descendant from escaping.
    fn spawning(&self, _command: &mut Command) {}

    /// The spawned child, before the run waits on it, reads a byte of its
    /// output, or can otherwise observe it exiting — so what the hook adopts is
    /// a live process rather than a pid that may already have been reused. On
    /// Windows the child is still **suspended** (oneharness resumes it once
    /// this returns), so a job object assigned here cannot miss a descendant;
    /// for that same reason a hook must not block waiting for the child to
    /// produce anything.
    ///
    /// A shared reference on purpose: the run owns the child's lifetime, and
    /// waiting on or killing it here would reap the process out from under the
    /// capture. Read `child.id()` — or its `AsRawHandle`/`AsRawFd` — and
    /// return; tear the tree down through your own group or job object.
    fn spawned(&self, _child: &Child) {}
}

/// The caller-owned side channels of a spawn: the token that tears the harness
/// tree down, and the [`ProcessSupervisor`] that may also own it.
///
/// This is process supervision, not the turn control `run --control` performs —
/// that is [`ControlledInput`], and the two are independent.
///
/// `#[non_exhaustive]` from birth, and built through [`SpawnControls::new`]
/// rather than a literal, so the *next* side channel can be added without
/// breaking a caller — the trap `RunControls` is already in, and the reason
/// [`crate::io::run::run_supervised`] exists as its own entry point.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct SpawnControls<'a> {
    /// Cancels the run: every in-flight tree is terminated through the same
    /// private `Finish::Terminate` path a timeout uses, and queued jobs go
    /// unspawned.
    pub cancel: &'a CancelToken,
    /// The caller's claim on each child, or `None` for the historical behavior:
    /// oneharness alone owns every harness tree it spawns.
    pub supervisor: Option<&'a dyn ProcessSupervisor>,
}

impl<'a> SpawnControls<'a> {
    /// Cancellation only — what the entry points that predate the supervisor
    /// pass, and byte for byte the behavior they have always had.
    pub fn new(cancel: &'a CancelToken) -> Self {
        Self {
            cancel,
            supervisor: None,
        }
    }

    /// Hand each spawned child to `supervisor` as well, so the caller can take
    /// it into the process group / job object it owns.
    #[must_use]
    pub fn with_supervisor(mut self, supervisor: &'a dyn ProcessSupervisor) -> Self {
        self.supervisor = Some(supervisor);
        self
    }
}

/// One job's final result plus how many times it was invoked. `attempts` is 1
/// for a plain run, more when a retry policy re-ran it (structured output), and
/// **0** for a job cancelled while still queued — it has no invocation, which is
/// what stops a caller from reporting bounds for a run that never began.
pub struct Outcome {
    pub capture: Capture,
    pub attempts: u32,
}

/// Run `jobs` concurrently, at most `max_parallel` at a time, preserving order.
pub fn run_jobs(jobs: &[Job], max_parallel: usize) -> Vec<Capture> {
    run_jobs_with(jobs, max_parallel, |_, _, _| None)
        .into_iter()
        .map(|o| o.capture)
        .collect()
}

/// A run cancelled before it ever spawned: the wave was torn down while this job
/// was still queued. Reported rather than dropped, so a caller's results stay
/// one-per-job and a cancelled batch is legible entry by entry.
pub(crate) fn unstarted_capture(job: &Job) -> Capture {
    Capture {
        status: Status::Cancelled,
        exit_code: None,
        duration_ms: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        error: Some(cancelled_error(&job.argv[0])),
        started_at: utc_now(),
        finished_at: Some(utc_now()),
        stdout_observations: Vec::new(),
    }
}

/// The `error` text for a run oneharness terminated on a cancellation request.
/// Distinct from the timeout message on purpose: nothing was exceeded, so
/// pointing at `--timeout` would send the reader down the wrong path.
pub(crate) fn cancelled_error(program: &str) -> String {
    format!(
        "`{program}` was cancelled: oneharness terminated it and its descendants before it finished. Suggestion: re-run when the work should complete"
    )
}

/// Like [`run_jobs`], but after each run consults `retry` to decide whether to
/// re-run the same job with a new argv. `retry(job_index, attempt, &capture)`
/// returns `Some(next_argv)` to run again (e.g. structured-output validation
/// failed, so re-prompt) or `None` to stop. `attempt` is the number of runs
/// completed so far (1 after the first), and the policy is responsible for its
/// own bound — the loop runs until it returns `None`. Only the argv changes
/// across attempts; cwd/env/timeout are reused. Returns each job's final capture
/// and total attempt count, in input order.
///
/// `retry` is `Sync` because it is shared across worker threads; the domain
/// validation it performs is pure, keeping this layer free of parsing logic.
pub fn run_jobs_with<F>(jobs: &[Job], max_parallel: usize, retry: F) -> Vec<Outcome>
where
    F: Fn(usize, u32, &Capture) -> Option<NextRun> + Sync,
{
    run_jobs_with_cancel(jobs, max_parallel, &CancelToken::new(), retry)
}

/// [`run_jobs_with`] under a caller-owned [`CancelToken`].
///
/// Cancelling tears down every in-flight job's process tree through the same
/// private `Finish::Terminate` path a timeout uses, and leaves still-queued
/// jobs unspawned — each reported as [`Status::Cancelled`] so the results stay
/// one-per-job. A host SIGINT/SIGTERM does the same thing to *every* run once
/// [`crate::io::cancel::install_signal_cancel`] is in force, so the plain
/// [`run_jobs_with`] entry point is cancellable too; this overload exists for a
/// library caller that cancels one run without touching the process.
pub fn run_jobs_with_cancel<F>(
    jobs: &[Job],
    max_parallel: usize,
    cancel: &CancelToken,
    retry: F,
) -> Vec<Outcome>
where
    F: Fn(usize, u32, &Capture) -> Option<NextRun> + Sync,
{
    run_jobs_supervised(jobs, max_parallel, SpawnControls::new(cancel), retry)
}

/// [`run_jobs_with_cancel`] with the caller's own claim on each spawned child
/// ([`SpawnControls::supervisor`]) — the entry point an embedder that groups the
/// harness processes itself calls. A `None` supervisor is byte for byte
/// [`run_jobs_with_cancel`].
pub fn run_jobs_supervised<F>(
    jobs: &[Job],
    max_parallel: usize,
    controls: SpawnControls<'_>,
    retry: F,
) -> Vec<Outcome>
where
    F: Fn(usize, u32, &Capture) -> Option<NextRun> + Sync,
{
    let cancel = controls.cancel;
    let n = jobs.len();
    if n == 0 {
        return Vec::new();
    }
    let workers = max_parallel.clamp(1, n);
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Outcome>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let retry = &retry;

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= n {
                    break;
                }
                let outcome = if cancellation_requested(cancel) {
                    Outcome {
                        capture: unstarted_capture(&jobs[i]),
                        attempts: 0,
                    }
                } else {
                    run_job_with_retry(&jobs[i], i, controls, retry)
                };
                *slots[i].lock().expect("slot mutex poisoned") = Some(outcome);
            });
        }
    });

    slots
        .into_iter()
        .map(|m| {
            m.into_inner()
                .expect("slot mutex poisoned")
                .expect("slot unfilled")
        })
        .collect()
}

/// Run one job, then loop while `retry` asks for another attempt with a new argv.
/// A cancellation ends the loop: re-prompting a run the caller has abandoned
/// would spend another turn on an answer nobody is waiting for.
fn run_job_with_retry<F>(job: &Job, index: usize, controls: SpawnControls<'_>, retry: &F) -> Outcome
where
    F: Fn(usize, u32, &Capture) -> Option<NextRun>,
{
    let cancel = controls.cancel;
    let mut capture = run_job_supervised(job, controls);
    let mut attempts = 1u32;
    while !cancellation_requested(cancel) {
        let Some(next) = retry(index, attempts, &capture) else {
            break;
        };
        let next = Job {
            argv: next.argv,
            cwd: job.cwd.clone(),
            env: job.env.clone(),
            env_remove: job.env_remove.clone(),
            timeout: job.timeout,
            stdin: next.stdin,
        };
        capture = run_job_supervised(&next, controls);
        attempts += 1;
    }
    Outcome { capture, attempts }
}

/// Decide the program and argument list to actually spawn for `argv`.
///
/// Normally this is just the [`resolve_program`]'d binary plus `argv[1..]`. On
/// Windows there is one rewrite: since Rust 1.77, `Command` runs a `.cmd`/`.bat`
/// through cmd.exe but refuses (with `InvalidInput`) any argument it cannot
/// escape for cmd.exe — a newline is the trigger. npm installs every JS harness
/// as a `claude.cmd` shim, so a multi-line argument (a rendered `--system`)
/// fails to spawn. When that exact situation is detected — a resolved `.cmd`/
/// `.bat` *and* a multi-line argument — parse the npm shim and invoke its real
/// target directly: a node interpreter plus script, or — as for claude-code,
/// whose bin is `bin/claude.exe` — the wrapped executable itself. That target is
/// a real `.exe`, so std's ordinary argument encoding carries the newline
/// through (only a `.cmd`/`.bat` goes through cmd.exe). Anything else
/// (single-line args, an unparseable shim, a non-shim `.cmd`) falls through
/// unchanged, so the established spawn path — and its error reporting — is byte
/// for byte what it was. The function is platform-shaped on purpose: the rewrite
/// only exists on Windows, and the pure shim parsing it relies on is tested on
/// every platform in `domain::shim`.
fn spawn_target(argv: &[String]) -> (std::ffi::OsString, Vec<String>) {
    let resolved = resolve_program(&argv[0]);
    let rest = argv[1..].to_vec();
    #[cfg(windows)]
    {
        if let Some(plan) = windows_shim_plan(std::path::Path::new(&resolved), &rest) {
            return plan;
        }
    }
    (resolved, rest)
}

/// The Windows shim rewrite for [`spawn_target`]: `Some((interpreter, args))`
/// when `resolved` is a `.cmd`/`.bat`, some argument is multi-line, and the file
/// parses as an npm shim; `None` (fall through to the `.cmd`) otherwise.
#[cfg(windows)]
fn windows_shim_plan(
    resolved: &std::path::Path,
    args: &[String],
) -> Option<(std::ffi::OsString, Vec<String>)> {
    // Only act when std would actually refuse the spawn: a multi-line argument.
    if !args.iter().any(|a| a.contains('\n') || a.contains('\r')) {
        return None;
    }
    let ext = resolved.extension()?.to_str()?.to_ascii_lowercase();
    if ext != "cmd" && ext != "bat" {
        return None;
    }
    let contents = std::fs::read_to_string(resolved).ok()?;
    let dir = resolved.parent()?.to_str()?;
    let target = crate::domain::shim::parse_cmd_shim(&contents, dir)?;
    let mut full = target.prefix_args;
    full.extend_from_slice(args);
    Some((resolve_program(&target.interpreter), full))
}

/// Configure the child's stdin: pipe it when the job carries prompt bytes to
/// deliver that way (the large-prompt escape hatch), else close it. Returns the
/// bytes to write once the child is spawned, if any.
fn stdin_stdio(job: &Job) -> (Stdio, Option<String>) {
    match &job.stdin {
        Some(text) => (Stdio::piped(), Some(text.clone())),
        None => (Stdio::null(), None),
    }
}

/// Write `bytes` to a spawned child's stdin on a dedicated thread, then close it
/// (drop closes the pipe, signalling EOF). Off-thread so a child that fills its
/// stdout mid-write can never deadlock the parent. A write failure (the child
/// exited early / closed stdin) is ignored — its exit/output is the real signal.
fn feed_stdin(process: &mut Process, bytes: String) {
    if let Some(mut stdin) = process.take_stdin() {
        std::thread::spawn(move || {
            let _ = std::io::Write::write_all(&mut stdin, bytes.as_bytes());
            // Dropping `stdin` here closes the pipe → EOF for the child.
        });
    }
}

/// Run a single job, returning its raw capture. Never panics on harness behavior.
pub fn run_job(job: &Job) -> Capture {
    run_job_cancellable(job, &CancelToken::new())
}

/// [`run_job`] under a caller-owned [`CancelToken`]. Cancelling terminates the
/// harness and its descendants through the same private `Finish::Terminate`
/// path a timeout uses, and the run comes back as [`Status::Cancelled`] with
/// whatever output had already been captured.
pub fn run_job_cancellable(job: &Job, cancel: &CancelToken) -> Capture {
    run_job_supervised(job, SpawnControls::new(cancel))
}

/// The one buffered implementation, under both caller-owned side channels: the
/// cancel token and the optional [`ProcessSupervisor`].
fn run_job_supervised(job: &Job, controls: SpawnControls<'_>) -> Capture {
    let cancel = controls.cancel;
    let start = Instant::now();
    let start_epoch_ms = epoch_millis();
    let started_at = crate::domain::history::format_rfc3339_millis(start_epoch_ms);
    let (program, args) = spawn_target(&job.argv);
    let (stdin_cfg, stdin_bytes) = stdin_stdio(job);
    let mut command = Command::new(program);
    command
        .args(&args)
        .stdin(stdin_cfg)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &job.cwd {
        command.current_dir(cwd);
        // Mirror a shell `cd`: keep $PWD consistent with the working directory.
        // `current_dir` only chdir()s the child; the inherited $PWD stays stale.
        // Some tools (e.g. Bun-based CLIs like OpenCode) trust $PWD over getcwd()
        // to locate the project, so a stale $PWD points them at the wrong dir.
        // Use the logical path (no symlink resolution), like `cd` does.
        let pwd = if cwd.is_absolute() {
            cwd.clone()
        } else {
            std::env::current_dir()
                .map(|base| base.join(cwd))
                .unwrap_or_else(|_| cwd.clone())
        };
        command.env("PWD", pwd);
    }
    // Explicit --env entries win over the derived PWD above.
    for (key, value) in &job.env {
        command.env(key, value);
    }
    for key in &job.env_remove {
        command.env_remove(key);
    }

    let mut process = match Process::spawn_supervised(command, controls.supervisor) {
        Ok(process) => process,
        Err(err) => {
            return Capture {
                status: Status::SpawnError,
                exit_code: None,
                duration_ms: Some(start.elapsed().as_millis()),
                stdout: String::new(),
                stderr: String::new(),
                error: Some(format!(
                    "failed to spawn `{}`: {err}. Suggestion: check the binary exists and is executable (try `oneharness detect`)",
                    job.argv[0]
                )),
                started_at,
                finished_at: Some(utc_now()),
                stdout_observations: Vec::new(),
            };
        }
    };

    // Feed the prompt to stdin (off-thread) when the job delivers it that way.
    if let Some(bytes) = stdin_bytes {
        feed_stdin(&mut process, bytes);
    }

    let deadline = (!job.timeout.is_zero()).then(|| start + job.timeout);
    let (status, exit_code, finish) = match wait_or_cancel(&mut process, deadline, cancel) {
        Wait::Exited(exit) => {
            let code = exit.code();
            let status = if code == Some(0) {
                Status::Ok
            } else {
                Status::Nonzero
            };
            (status, code, Finish::Exited)
        }
        Wait::TimedOut => (Status::Timeout, None, Finish::Terminate),
        Wait::Cancelled => (Status::Cancelled, None, Finish::Terminate),
        Wait::Failed => (Status::SpawnError, None, Finish::Terminate),
    };

    let finished = process.finish(finish);
    let duration_ms = Some(start.elapsed().as_millis());

    let error = match status {
        Status::Timeout => Some(timeout_error(job)),
        Status::Cancelled => Some(cancelled_error(&job.argv[0])),
        Status::SpawnError => Some(format!("`{}` could not be waited on", job.argv[0])),
        _ => None,
    };

    let observations = observations_since(start, start_epoch_ms, finished.stdout_observations);
    Capture {
        status,
        exit_code,
        duration_ms,
        stdout: finished.stdout,
        stderr: finished.stderr,
        error,
        started_at,
        finished_at: Some(utc_now()),
        stdout_observations: observations,
    }
}

/// What a streaming line callback asks the run to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStep {
    /// Keep reading the harness's output.
    Continue,
    /// Stop now and tear down the child — the consumer has gone away (e.g. a
    /// broken stdout pipe: it short-circuited on an observed action).
    Stop,
}

/// Run one job, invoking `on_line` for each complete line of stdout **as it
/// arrives**, and return the same [`Capture`] the batch path would (accumulated
/// stdout/stderr, status, timing) so the caller can still emit a final envelope.
///
/// This is the streaming counterpart to [`run_job`]: it exists so a consumer can
/// observe a harness's normalized events incrementally and short-circuit the
/// moment it sees a disallowed action — instead of paying for a whole turn before
/// judging it. The parsing stays out of this layer: `on_line` (a pure
/// domain-driven closure in the command layer) decides what to emit and returns
/// [`StreamStep::Stop`] to end early (the command layer returns `Stop` when its
/// write to the consumer fails, i.e. the consumer closed the stream). On `Stop`
/// or timeout the child is killed; on normal EOF its exit is awaited. Never
/// panics on harness behavior — same contract as [`run_job`].
pub fn run_job_streaming<F>(job: &Job, on_line: F) -> Capture
where
    F: FnMut(&str) -> StreamStep,
{
    run_job_streaming_cancellable(job, &CancelToken::new(), on_line)
}

/// [`run_job_streaming`] under a caller-owned [`CancelToken`].
///
/// The cancellation check is what makes a **silent** harness stoppable. The
/// stream loop is otherwise parked in the stdout pipe read until the deadline,
/// and `on_line` — the only other way out — is never called for a harness that
/// writes nothing. So the read is bounded by the private `CANCEL_POLL_SLICE`
/// slice and the cancellation flag re-checked on each tick, which then tears
/// the whole tree down through private `Finish::Terminate`. Without it a
/// cancelled run leaves a live harness behind, since the harness is its own
/// process-group leader and does not die with the host.
pub fn run_job_streaming_cancellable<F>(job: &Job, cancel: &CancelToken, on_line: F) -> Capture
where
    F: FnMut(&str) -> StreamStep,
{
    stream_job(job, None, SpawnControls::new(cancel), on_line)
}

/// Everything the runner needs to keep a run's stdin open for out-of-band turn
/// control (`run --control`).
///
/// The ordinary paths drop the child's stdin as soon as the prompt is written,
/// which is exactly what makes a turn uninterruptible. Here the handle is
/// *parked* in the shared [`ControlHandle`] instead, so the control socket's
/// listener thread can push an interrupt frame into the same live process — and
/// the runner closes it again the moment the harness emits its end-of-turn
/// document, so a control-enabled run still terminates on its own.
pub struct ControlledInput<'a> {
    /// The shared handle the control socket serves requests through. It also
    /// owns the protocol conversation for a server-backed mechanism, so the
    /// runner only pumps lines through it rather than knowing any protocol.
    pub handle: &'a ControlHandle,
    /// The assembled user prompt. For a mechanism that rides the harness's own
    /// run it becomes the first stdin frame; for a server-backed one the
    /// dialogue sends it after its handshake.
    pub prompt: String,
    /// The shared Codex app-server this turn runs on instead of a child of its
    /// own (`app_server = "shared"`), or `None` to spawn the job as before.
    pub app_server: Option<&'a crate::io::app_server::PooledDispatch>,
}

/// [`run_job_streaming`], with the child's stdin held open for turn control when
/// `control` is `Some`. `None` is byte-for-byte the ordinary streaming path.
///
/// A controlled run stays cancellable: it carries a fresh token, which the
/// process-wide flag a host SIGINT/SIGTERM raises still trips, so an interrupted
/// supervisor tears the harness tree down exactly as an uncontrolled run does.
pub fn run_job_streaming_controlled<F>(
    job: &Job,
    control: Option<&ControlledInput>,
    on_line: F,
) -> Capture
where
    F: FnMut(&str) -> StreamStep,
{
    stream_job(
        job,
        control,
        SpawnControls::new(&CancelToken::new()),
        on_line,
    )
}

/// [`run_job_streaming_controlled`] under a caller-owned [`CancelToken`] — the
/// two side channels combined, for a run that is both interruptible on its
/// stdin and cancellable by whoever asked for it.
///
/// A library caller needs this even when it never passes `control`: the token is
/// its only way to tear the tree down, since the harness leads its own process
/// group and no signal it sends its own group reaches one.
pub fn run_job_streaming_controlled_cancellable<F>(
    job: &Job,
    control: Option<&ControlledInput>,
    cancel: &CancelToken,
    on_line: F,
) -> Capture
where
    F: FnMut(&str) -> StreamStep,
{
    stream_job(job, control, SpawnControls::new(cancel), on_line)
}

/// [`run_job_streaming_controlled_cancellable`] with the caller's own claim on
/// the spawned child ([`SpawnControls::supervisor`]) — the streaming
/// counterpart to [`run_jobs_supervised`]. A `None` supervisor is byte for byte
/// the entry point above.
pub fn run_job_streaming_supervised<F>(
    job: &Job,
    control: Option<&ControlledInput>,
    controls: SpawnControls<'_>,
    on_line: F,
) -> Capture
where
    F: FnMut(&str) -> StreamStep,
{
    stream_job(job, control, controls, on_line)
}

/// The one streaming implementation both public entry points delegate to:
/// optional turn control on the child's stdin, bounded by `cancel`.
fn stream_job<F>(
    job: &Job,
    control: Option<&ControlledInput>,
    controls: SpawnControls<'_>,
    mut on_line: F,
) -> Capture
where
    F: FnMut(&str) -> StreamStep,
{
    let cancel = controls.cancel;
    // A turn on a shared app-server spawns nothing of its own: it leases the
    // pooled server and speaks to it over a socket.
    if let Some((control, pooled)) =
        control.and_then(|control| control.app_server.map(|pooled| (control, pooled)))
    {
        return crate::io::app_server::run_pooled_turn(
            job,
            control.handle,
            &control.prompt,
            pooled,
            cancel,
            &mut on_line,
        );
    }
    let start = Instant::now();
    let start_epoch_ms = epoch_millis();
    let started_at = crate::domain::history::format_rfc3339_millis(start_epoch_ms);
    let (program, args) = spawn_target(&job.argv);
    // A controlled run always pipes stdin — the handle is the control channel,
    // not just a prompt delivery.
    let (stdin_cfg, stdin_bytes) = if control.is_some() {
        (Stdio::piped(), None)
    } else {
        stdin_stdio(job)
    };
    let mut command = Command::new(program);
    command
        .args(&args)
        .stdin(stdin_cfg)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &job.cwd {
        command.current_dir(cwd);
        let pwd = if cwd.is_absolute() {
            cwd.clone()
        } else {
            std::env::current_dir()
                .map(|base| base.join(cwd))
                .unwrap_or_else(|_| cwd.clone())
        };
        command.env("PWD", pwd);
    }
    for (key, value) in &job.env {
        command.env(key, value);
    }
    for key in &job.env_remove {
        command.env_remove(key);
    }

    let mut process = match Process::spawn_supervised(command, controls.supervisor) {
        Ok(process) => process,
        Err(err) => {
            return Capture {
                status: Status::SpawnError,
                exit_code: None,
                duration_ms: Some(start.elapsed().as_millis()),
                stdout: String::new(),
                stderr: String::new(),
                error: Some(format!(
                    "failed to spawn `{}`: {err}. Suggestion: check the binary exists and is executable (try `oneharness detect`)",
                    job.argv[0]
                )),
                started_at,
                finished_at: Some(utc_now()),
                stdout_observations: Vec::new(),
            };
        }
    };

    if let Some(bytes) = stdin_bytes {
        feed_stdin(&mut process, bytes);
    }
    // Park stdin in the control handle and open the turn with the prompt frame.
    // A failed hand-off is not fatal: the harness simply never receives the
    // prompt and reports its own error, which is more useful than a panic.
    if let Some(control) = control {
        if let Some(stdin) = process.take_stdin() {
            control.handle.begin_turn(stdin);
            for frame in control.handle.open_frames(&control.prompt) {
                if let Err(err) = control.handle.write_line(&frame) {
                    eprintln!("oneharness: warning: could not open the controlled turn: {err}");
                    break;
                }
            }
        }
    }

    // Stdin was held open past the prompt, so a controlled harness waits for
    // another message instead of exiting. Closing it on the end-of-turn document
    // is what lets the run finish on its own, interrupted or not.
    //
    // A harness whose turn oneharness *drives* (a JSON-RPC server) has no reason
    // to exit at all once its turn is over — it is a server, waiting for the
    // next one — so there the end of the turn is the end of the run and the
    // child is torn down rather than waited on.
    let mut stopped_after_turn = false;
    let mut on_line = |line: &str| {
        if let Some(control) = control {
            if control.handle.advance(line) {
                control.handle.end_turn();
                if control.handle.drives_turn_over_stdin() {
                    stopped_after_turn = true;
                    on_line(line);
                    return StreamStep::Stop;
                }
            }
        }
        on_line(line)
    };

    let deadline = (!job.timeout.is_zero()).then(|| start + job.timeout);
    let mut pending = Vec::new();
    let end = loop {
        if cancellation_requested(cancel) {
            break StreamEnd::Cancelled;
        }
        let now = Instant::now();
        if deadline.is_some_and(|deadline| now >= deadline) {
            break StreamEnd::TimedOut;
        }
        let poll_until = deadline.map_or(now + CANCEL_POLL_SLICE, |deadline| {
            deadline.min(now + CANCEL_POLL_SLICE)
        });
        match process.recv_stdout_until(poll_until) {
            PipeEvent::Data(chunk) => {
                pending.extend_from_slice(&chunk);
                if deliver_complete_lines(&mut pending, &mut on_line) == StreamStep::Stop {
                    break StreamEnd::Stopped;
                }
            }
            PipeEvent::Closed => {
                if deliver_final_line(&mut pending, &mut on_line) == StreamStep::Stop {
                    break StreamEnd::Stopped;
                }
                break match wait_or_cancel(&mut process, deadline, cancel) {
                    Wait::Exited(exit) => StreamEnd::Exited(exit),
                    Wait::TimedOut => StreamEnd::TimedOut,
                    Wait::Cancelled => StreamEnd::Cancelled,
                    Wait::Failed => StreamEnd::WaitFailed,
                };
            }
            // A poll tick, not the run's deadline: loop back to re-check both
            // cancellation and the real deadline.
            PipeEvent::Deadline => {}
        }
    };

    let (status, exit_code, finish) = match end {
        StreamEnd::Exited(exit) => {
            let code = exit.code();
            let status = if code == Some(0) {
                Status::Ok
            } else {
                Status::Nonzero
            };
            (status, code, Finish::Exited)
        }
        // Consumer-driven stop is not a harness failure. The consumer already
        // received what it needed, so the best-effort envelope remains Ok.
        StreamEnd::Stopped => (Status::Ok, None, Finish::Terminate),
        StreamEnd::TimedOut => (Status::Timeout, None, Finish::Terminate),
        StreamEnd::Cancelled => (Status::Cancelled, None, Finish::Terminate),
        StreamEnd::WaitFailed => (Status::SpawnError, None, Finish::Terminate),
    };
    let finished = process.finish(finish);

    let error = match status {
        Status::Timeout => Some(timeout_error(job)),
        Status::Cancelled => Some(cancelled_error(&job.argv[0])),
        _ => None,
    };

    let observations = observations_since(start, start_epoch_ms, finished.stdout_observations);
    Capture {
        status,
        exit_code,
        duration_ms: Some(start.elapsed().as_millis()),
        stdout: finished.stdout,
        stderr: finished.stderr,
        error,
        started_at,
        finished_at: Some(utc_now()),
        stdout_observations: observations,
    }
}

pub(crate) fn utc_now() -> String {
    crate::domain::history::format_rfc3339_millis(epoch_millis())
}

pub(crate) fn epoch_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) fn observations_since(
    start: Instant,
    start_epoch_ms: u128,
    observations: Vec<(Instant, Vec<u8>)>,
) -> Vec<OutputObservation> {
    observations
        .into_iter()
        .map(|(observed_at, bytes)| OutputObservation {
            offset_ms: observed_at.saturating_duration_since(start).as_millis(),
            observed_at: crate::domain::history::format_rfc3339_millis(
                start_epoch_ms + observed_at.saturating_duration_since(start).as_millis(),
            ),
            bytes,
        })
        .collect()
}

#[derive(Clone, Copy)]
enum StreamEnd {
    Exited(std::process::ExitStatus),
    Stopped,
    TimedOut,
    Cancelled,
    WaitFailed,
}

/// How a bounded wait for the direct child ended.
enum Wait {
    Exited(std::process::ExitStatus),
    TimedOut,
    Cancelled,
    Failed,
}

/// Wait for the child until an optional `deadline`, waking every
/// [`CANCEL_POLL_SLICE`] to re-check cancellation. The slice is the only reason
/// cancellation is observed for an unbounded run or before a distant deadline.
fn wait_or_cancel(process: &mut Process, deadline: Option<Instant>, cancel: &CancelToken) -> Wait {
    loop {
        if cancellation_requested(cancel) {
            return Wait::Cancelled;
        }
        let now = Instant::now();
        if deadline.is_some_and(|deadline| now >= deadline) {
            return Wait::TimedOut;
        }
        let poll_until = deadline.map_or(now + CANCEL_POLL_SLICE, |deadline| {
            deadline.min(now + CANCEL_POLL_SLICE)
        });
        match process.wait_until(poll_until) {
            Ok(Some(exit)) => return Wait::Exited(exit),
            // The slice elapsed; the loop decides whether that was the deadline.
            Ok(None) => {}
            Err(_) => return Wait::Failed,
        }
    }
}

/// The `error` text for a run killed at its own deadline.
pub(crate) fn timeout_error(job: &Job) -> String {
    format!(
        "`{}` exceeded the {}s timeout and was killed. Suggestion: raise --timeout or simplify the prompt",
        job.argv[0],
        job.timeout.as_secs()
    )
}

/// Deliver every newline-terminated line currently buffered, retaining an
/// incomplete tail for the next pipe chunk. Splitting happens on bytes so a
/// multi-byte UTF-8 character divided across OS reads is decoded only when its
/// whole line is present.
fn deliver_complete_lines<F>(pending: &mut Vec<u8>, on_line: &mut F) -> StreamStep
where
    F: FnMut(&str) -> StreamStep,
{
    while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
        let line: Vec<u8> = pending.drain(..=newline).collect();
        let line = String::from_utf8_lossy(&line);
        if on_line(line.trim_end_matches(['\n', '\r'])) == StreamStep::Stop {
            return StreamStep::Stop;
        }
    }
    StreamStep::Continue
}

fn deliver_final_line<F>(pending: &mut Vec<u8>, on_line: &mut F) -> StreamStep
where
    F: FnMut(&str) -> StreamStep,
{
    if pending.is_empty() {
        return StreamStep::Continue;
    }
    let line = String::from_utf8_lossy(pending);
    let step = on_line(line.trim_end_matches(['\n', '\r']));
    pending.clear();
    step
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use crate::io::scratch::ScratchDir;

    fn job(argv: &[&str]) -> Job {
        Job {
            argv: argv.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
            timeout: Duration::from_secs(5),
            stdin: None,
        }
    }

    #[test]
    fn empty_jobs_returns_empty_without_spawning_workers() {
        // The no-work fast path: no jobs means no captures and no threads.
        assert!(run_jobs(&[], 4).is_empty());
    }

    #[test]
    fn spawn_error_is_data_not_a_panic() {
        // A binary that cannot be spawned must surface as a `SpawnError` capture
        // with a helpful message — never a crash. Run it through `run_jobs` so the
        // worker-pool path that fills the result slot is exercised too.
        let jobs = [job(&["/no/such/oneharness-binary-xyz", "arg"])];
        let captures = run_jobs(&jobs, 1);
        assert_eq!(captures.len(), 1);
        let cap = &captures[0];
        assert_eq!(cap.status, Status::SpawnError);
        assert!(cap.exit_code.is_none());
        assert!(cap.stdout.is_empty());
        assert!(cap.duration_ms.is_some());
        let msg = cap.error.as_deref().unwrap_or_default();
        assert!(msg.contains("failed to spawn"), "{msg}");
        assert!(msg.contains("oneharness-binary-xyz"), "{msg}");
    }

    #[test]
    fn run_jobs_with_retries_until_the_policy_stops() {
        // The structured-output loop in disguise: re-run with a fresh argv until
        // the policy returns None. Uses unspawnable binaries so it stays portable
        // and process-free, asserting on the attempt count and that the *final*
        // capture came from the last retry's argv (its error names that binary).
        let jobs = [job(&["/no/such/first"])];
        let outcomes = run_jobs_with(&jobs, 1, |i, attempt, cap| {
            assert_eq!(i, 0);
            assert_eq!(cap.status, Status::SpawnError);
            (attempt < 3).then(|| NextRun {
                argv: vec![format!("/no/such/retry-{attempt}")],
                stdin: None,
            })
        });
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].attempts, 3);
        let err = outcomes[0].capture.error.as_deref().unwrap_or_default();
        assert!(
            err.contains("retry-2"),
            "final capture should be last retry: {err}"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn stdin_is_piped_to_the_child_without_deadlock() {
        // A prompt delivered via stdin must reach the child even when it is large
        // enough to fill the OS pipe buffer while the child echoes it straight
        // back to stdout — the classic writer/reader deadlock the off-thread
        // stdin write (paired with the off-thread stdout drain) avoids. `cat`
        // streams stdin→stdout, so a naive same-thread write would wedge at the
        // ~64 KB pipe buffer; this payload is far larger.
        let payload = "oneharness-stdin-probe\n".repeat(8000); // ~180 KB
        let job = Job {
            argv: vec!["cat".to_string()],
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
            timeout: Duration::from_secs(30),
            stdin: Some(payload.clone()),
        };
        let cap = run_job(&job);
        assert_eq!(cap.status, Status::Ok, "stderr: {}", cap.stderr);
        // `cat` is a byte-faithful echo, so stdout is exactly what we fed to stdin.
        assert_eq!(cap.stdout, payload);
    }

    #[test]
    fn run_jobs_with_a_no_op_policy_runs_once() {
        let jobs = [job(&["/no/such/binary"])];
        let outcomes = run_jobs_with(&jobs, 1, |_, _, _| None);
        assert_eq!(outcomes[0].attempts, 1);
        // Empty input is still the no-work fast path.
        assert!(run_jobs_with(&[], 4, |_, _, _| None).is_empty());
    }

    /// A portable job that prints three lines then exits 0 (`sh -c printf` on
    /// Unix, `cmd /c echo` on Windows), for exercising the streaming reader.
    fn three_line_job() -> Job {
        #[cfg(not(windows))]
        let argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf 'a\\nb\\nc\\n'".to_string(),
        ];
        #[cfg(windows)]
        let argv = vec![
            "cmd".to_string(),
            "/c".to_string(),
            "echo a& echo b& echo c".to_string(),
        ];
        Job {
            argv,
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
            timeout: Duration::from_secs(10),
            stdin: None,
        }
    }

    #[test]
    fn streaming_delivers_each_line_and_accumulates_stdout() {
        // The happy path: every line reaches the callback in order, and the final
        // capture's stdout is the byte-faithful accumulation, status Ok.
        let mut lines = Vec::new();
        let cap = run_job_streaming(&three_line_job(), |line| {
            lines.push(line.to_string());
            StreamStep::Continue
        });
        assert_eq!(cap.status, Status::Ok);
        assert_eq!(cap.exit_code, Some(0));
        assert_eq!(lines.len(), 3, "got {lines:?}");
        // Trim to tolerate any shell quirks; the content is a/b/c in order.
        let trimmed: Vec<_> = lines.iter().map(|l| l.trim()).collect();
        assert_eq!(trimmed, vec!["a", "b", "c"]);
        for token in ["a", "b", "c"] {
            assert!(cap.stdout.contains(token), "stdout: {:?}", cap.stdout);
        }
    }

    #[test]
    fn streaming_stops_and_tears_down_on_callback_stop() {
        // Returning Stop after the first line ends the run immediately (the child
        // is killed); the consumer-driven stop is reported as Ok, not a failure.
        let mut count = 0u32;
        let cap = run_job_streaming(&three_line_job(), |_| {
            count += 1;
            StreamStep::Stop
        });
        assert_eq!(count, 1, "should stop after the first line");
        assert_eq!(cap.status, Status::Ok);
    }

    #[cfg(unix)]
    fn descendant_job(timeout: Duration, tick_file: &std::path::Path) -> Job {
        // The wrapper and its child both ignore TERM. The child inherits stdout
        // and writes a side-channel tick for five seconds, reproducing an npm
        // launcher whose native grandchild survives a direct-child kill and
        // keeps the parent's pipe reader blocked.
        let script = r#"
            trap '' TERM
            sh -c '
                trap "" TERM
                i=0
                while [ "$i" -lt 100 ]; do
                    printf x >> "$TICK_FILE"
                    printf "native tick %s\n" "$i"
                    i=$((i + 1))
                    sleep 0.05
                done
            ' &
            printf 'launcher ready\n'
            wait
        "#;
        Job {
            argv: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
            cwd: None,
            env: vec![("TICK_FILE".to_string(), tick_file.display().to_string())],
            env_remove: Vec::new(),
            timeout,
            stdin: None,
        }
    }

    #[cfg(unix)]
    fn unique_tick_file(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "oneharness-{label}-{}-{:?}.ticks",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[cfg(unix)]
    #[test]
    fn timeout_terminates_descendants_and_bounds_pipe_drain() {
        let ticks = unique_tick_file("timeout-tree");
        let _ = std::fs::remove_file(&ticks);
        let started = Instant::now();
        let cap = run_job(&descendant_job(Duration::from_millis(200), &ticks));

        assert_eq!(cap.status, Status::Timeout);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timeout teardown took {:?}",
            started.elapsed()
        );
        assert!(cap.stdout.contains("launcher ready"), "{}", cap.stdout);

        let after_return = std::fs::metadata(&ticks).map(|m| m.len()).unwrap_or(0);
        std::thread::sleep(Duration::from_millis(250));
        let after_grace = std::fs::metadata(&ticks).map(|m| m.len()).unwrap_or(0);
        assert_eq!(
            after_return, after_grace,
            "a descendant kept ticking after process-tree teardown"
        );
        let _ = std::fs::remove_file(ticks);
    }

    #[cfg(unix)]
    #[test]
    fn streaming_stop_terminates_descendants_and_returns_promptly() {
        let ticks = unique_tick_file("stream-tree");
        let _ = std::fs::remove_file(&ticks);
        let started = Instant::now();
        let mut lines = 0;
        let cap = run_job_streaming(&descendant_job(Duration::from_secs(10), &ticks), |_| {
            lines += 1;
            StreamStep::Stop
        });

        assert_eq!(lines, 1);
        assert_eq!(cap.status, Status::Ok);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "stream teardown took {:?}",
            started.elapsed()
        );
        let after_return = std::fs::metadata(&ticks).map(|m| m.len()).unwrap_or(0);
        std::thread::sleep(Duration::from_millis(250));
        let after_grace = std::fs::metadata(&ticks).map(|m| m.len()).unwrap_or(0);
        assert_eq!(after_return, after_grace);
        let _ = std::fs::remove_file(ticks);
    }

    /// A harness that writes **nothing** while a TERM-ignoring descendant keeps
    /// the inherited pipes open for `20s`. This is the shape the streaming loop
    /// cannot see any other way: `on_line` never fires, so a consumer-driven
    /// `Stop` is unreachable and only the cancellation poll ends the run.
    #[cfg(unix)]
    fn silent_descendant_job(timeout: Duration, tick_file: &std::path::Path) -> Job {
        let script = r#"
            trap '' TERM
            sh -c '
                trap "" TERM
                i=0
                while [ "$i" -lt 400 ]; do
                    printf x >> "$TICK_FILE"
                    i=$((i + 1))
                    sleep 0.05
                done
            ' &
            wait
        "#;
        Job {
            argv: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
            cwd: None,
            env: vec![("TICK_FILE".to_string(), tick_file.display().to_string())],
            env_remove: Vec::new(),
            timeout,
            stdin: None,
        }
    }

    /// Cancel `token` once `tick_file` proves the descendant is really running,
    /// so the test cancels a live tree rather than racing the spawn.
    #[cfg(unix)]
    fn cancel_once_alive(token: CancelToken, tick_file: std::path::PathBuf) {
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if std::fs::metadata(&tick_file).is_ok_and(|meta| meta.len() > 0) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            token.cancel();
        });
    }

    /// The tick file must stop growing once the runner has returned.
    #[cfg(unix)]
    fn assert_descendant_stopped(tick_file: &std::path::Path) {
        let after_return = std::fs::metadata(tick_file).map(|m| m.len()).unwrap_or(0);
        assert!(
            after_return > 0,
            "the fixture never proved its descendant ran, so the teardown assertion is vacuous"
        );
        std::thread::sleep(Duration::from_millis(300));
        let after_grace = std::fs::metadata(tick_file).map(|m| m.len()).unwrap_or(0);
        assert_eq!(
            after_return, after_grace,
            "a descendant kept ticking after the cancelled run returned"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_a_silent_run_terminates_it_and_its_descendants() {
        // The blocking defect: a harness that produces no output never reaches
        // `on_line`, so before the cancellation poll existed this run stayed
        // parked in the pipe read for the whole 20s timeout and its descendants
        // outlived the caller's cancel.
        let ticks = unique_tick_file("cancel-silent-stream");
        let _ = std::fs::remove_file(&ticks);
        let cancel = CancelToken::new();
        cancel_once_alive(cancel.clone(), ticks.clone());

        let started = Instant::now();
        let mut lines = 0u32;
        let cap = run_job_streaming_cancellable(
            &silent_descendant_job(Duration::from_secs(20), &ticks),
            &cancel,
            |_| {
                lines += 1;
                StreamStep::Continue
            },
        );

        assert_eq!(
            lines, 0,
            "the fixture must be silent for this to be the test"
        );
        assert_eq!(cap.status, Status::Cancelled);
        assert!(cap.exit_code.is_none());
        assert!(
            cap.error
                .as_deref()
                .unwrap_or_default()
                .contains("cancelled"),
            "{:?}",
            cap.error
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "cancellation waited for the harness timeout instead of the poll: {:?}",
            started.elapsed()
        );
        assert_descendant_stopped(&ticks);
        let _ = std::fs::remove_file(ticks);
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_a_buffered_run_terminates_it_and_its_descendants() {
        // The same guarantee on the non-streaming path, which a batch run takes.
        let ticks = unique_tick_file("cancel-silent-buffered");
        let _ = std::fs::remove_file(&ticks);
        let cancel = CancelToken::new();
        cancel_once_alive(cancel.clone(), ticks.clone());

        let started = Instant::now();
        let cap = run_job_cancellable(
            &silent_descendant_job(Duration::from_secs(20), &ticks),
            &cancel,
        );

        assert_eq!(cap.status, Status::Cancelled);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "cancellation waited for the harness timeout: {:?}",
            started.elapsed()
        );
        assert_descendant_stopped(&ticks);
        let _ = std::fs::remove_file(ticks);
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_a_wave_leaves_queued_jobs_unspawned_but_reported() {
        // One worker, two jobs: the second is still queued when the first is
        // cancelled. It must never spawn, and must still occupy its own result
        // slot so a caller's results stay one-per-job.
        let ticks = unique_tick_file("cancel-wave");
        let queued_ticks = unique_tick_file("cancel-wave-queued");
        for path in [&ticks, &queued_ticks] {
            let _ = std::fs::remove_file(path);
        }
        let cancel = CancelToken::new();
        cancel_once_alive(cancel.clone(), ticks.clone());

        let jobs = [
            silent_descendant_job(Duration::from_secs(20), &ticks),
            silent_descendant_job(Duration::from_secs(20), &queued_ticks),
        ];
        let outcomes = run_jobs_with_cancel(&jobs, 1, &cancel, |_, _, _| None);

        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].capture.status, Status::Cancelled);
        assert_eq!(outcomes[0].attempts, 1);
        assert_eq!(outcomes[1].capture.status, Status::Cancelled);
        assert_eq!(
            outcomes[1].attempts, 0,
            "a queued job must not be invoked at all"
        );
        assert!(
            !queued_ticks.exists(),
            "the queued job spawned despite the cancellation"
        );
        assert_descendant_stopped(&ticks);
        let _ = std::fs::remove_file(ticks);
    }

    #[cfg(unix)]
    #[test]
    fn a_cancelled_run_does_not_retry() {
        // The structured-output loop must not spend another turn re-prompting a
        // run whose caller has already walked away.
        let cancel = CancelToken::new();
        let trigger = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            trigger.cancel();
        });
        let jobs = [Job {
            argv: vec!["sh".to_string(), "-c".to_string(), "sleep 20".to_string()],
            ..job(&["sh"])
        }];
        let asked = AtomicUsize::new(0);
        let outcomes = run_jobs_with_cancel(&jobs, 1, &cancel, |_, _, _| {
            asked.fetch_add(1, Ordering::SeqCst);
            Some(NextRun {
                argv: vec!["/no/such/again".to_string()],
                stdin: None,
            })
        });
        assert_eq!(
            asked.load(Ordering::SeqCst),
            0,
            "the retry policy must not be consulted after a cancellation"
        );
        assert_eq!(outcomes[0].capture.status, Status::Cancelled);
        assert_eq!(outcomes[0].attempts, 1);
    }

    #[test]
    fn streaming_spawn_error_is_data_not_a_panic() {
        // A missing binary surfaces as a SpawnError capture, same as run_job.
        let job = job(&["/no/such/oneharness-stream-binary"]);
        let cap = run_job_streaming(&job, |_| StreamStep::Continue);
        assert_eq!(cap.status, Status::SpawnError);
        assert!(cap.error.as_deref().unwrap_or_default().contains("spawn"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_shim_plan_rewrites_a_cmd_only_for_multiline_args() {
        use std::io::Write;

        // A minimal npm-style `.cmd` shim written to a real temp file, so the
        // plan reads it the way `run_job` would.
        let dir = ScratchDir::new("shim").unwrap();
        let cmd_path = dir.join("claude.cmd");
        let mut f = std::fs::File::create(&cmd_path).unwrap();
        write!(
            f,
            "SET \"_prog=node\"\r\n\"%_prog%\" \"%dp0%\\cli.js\" %*\r\n"
        )
        .unwrap();
        drop(f);

        let dir_str = dir.to_str().unwrap();
        let script = format!("{dir_str}\\cli.js");

        // A multi-line argument triggers the rewrite: node + script + the args.
        let multiline = vec!["-p".to_string(), "a\nb\nc".to_string()];
        let (prog, args) = windows_shim_plan(&cmd_path, &multiline).expect("multiline → rewrite");
        // `node` resolves to a real path ending in node.exe.
        assert!(
            std::path::Path::new(&prog)
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with("node.exe"),
            "interpreter should resolve to node.exe, got {prog:?}"
        );
        assert_eq!(args, vec![script, "-p".to_string(), "a\nb\nc".to_string()]);

        // A single-line argument must NOT be rewritten — the `.cmd` spawns fine.
        let single = vec!["-p".to_string(), "hello".to_string()];
        assert!(windows_shim_plan(&cmd_path, &single).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn windows_shim_plan_ignores_non_batch_programs() {
        // A real `.exe` (or anything not `.cmd`/`.bat`) is never rewritten, even
        // with a multi-line argument — std spawns it directly without cmd.exe.
        let exe = resolve_program("where");
        let multiline = vec!["x\ny".to_string()];
        assert!(windows_shim_plan(std::path::Path::new(&exe), &multiline).is_none());
    }
}
