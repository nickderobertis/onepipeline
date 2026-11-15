//! One spawned plugin, and the one exchange at a time the engine has with it.
//!
//! The transport is deliberately hand-rolled, for the reason `engine/join.rs` gives for
//! its own combinator: this crate is written against `std::future` alone and runs on
//! whatever runtime its caller brings, so it may not reach for a runtime's process or
//! channel types. What it must not do instead is block: a blocking read inside an `async
//! fn` would stall every *other* source's future on the same task, and asking every
//! source at once is the property the engine is built on. So the blocking half runs on an
//! ordinary thread and the async half waits on [`Answer`], a one-shot that parks the
//! caller's waker until that thread has a line.
//!
//! Requests on one connection are serialized, which the protocol permits and §1.1 names
//! as the simpler correct choice. The concurrency that matters here is *across* sources,
//! and that is the caller's: the engine drives one future per source.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use onetaskgraph_plugin_api::SourceError;
use serde_json::Value;

use super::wire::{Request, Response};

/// How much of an offending line §6.3 quotes back. Long enough to recognise the message,
/// short enough that a plugin echoing a whole page of tasks cannot fill a terminal.
const QUOTED: usize = 200;

/// The most a peer may put on one line before this side stops reading it.
///
/// A line is read into memory before anything can be said about it, so a peer that never
/// writes a newline is a peer that decides how much memory this process uses. Sixteen
/// mebibytes is far above any real page — a source declaring a page size of ten thousand
/// and a kibibyte of prose per task is a tenth of it — and far below anything that
/// threatens a host, which is the whole of what a bound like this is for.
pub const MAX_LINE: u64 = 16 * 1024 * 1024;

/// How much of a plugin's standard error is kept for diagnostics. Bounded because a
/// plugin that logs in a loop must not be able to grow this engine's memory without end —
/// the invariant this product is built on is that the engine holds work data transiently,
/// and a diagnostics buffer with no ceiling is a way to hold it for ever by accident.
const KEPT_DIAGNOSTICS: usize = 4096;

/// What reading one line from a peer produced.
pub(crate) enum Line {
    /// A line, with its terminator still on it.
    Read(String),
    /// The peer closed the stream with nothing more to say.
    Ended,
    /// The peer wrote [`MAX_LINE`] bytes without ending the line.
    TooLong,
    /// The stream itself failed.
    Failed(std::io::Error),
}

/// Read one line, refusing a peer that never ends one.
///
/// The bound is applied *before* the allocation rather than after it, which is the whole
/// point: checking the length of a line already in memory is a check made too late.
pub(crate) fn read_line(reader: &mut (impl BufRead + ?Sized)) -> Line {
    let mut line = String::new();
    match reader.take(MAX_LINE).read_line(&mut line) {
        Err(error) => Line::Failed(error),
        Ok(0) => Line::Ended,
        Ok(_) if !line.ends_with('\n') => Line::TooLong,
        Ok(_) => Line::Read(line),
    }
}

/// A live plugin process, with a thread doing its blocking input and output.
pub(crate) struct Connection {
    /// Where a request line goes. `None` once the worker has given up, so that a caller
    /// after a fatal error gets that error rather than a wait nothing will end.
    jobs: Mutex<Option<Sender<Job>>>,
    /// Whatever the plugin has written to standard error, bounded and shared.
    diagnostics: Arc<Mutex<String>>,
    /// The next request id. Opaque to the plugin, which echoes it byte for byte (§1.1).
    next_id: AtomicU64,
    /// Held so the child is reaped, and killed, when this connection is dropped.
    ///
    /// Absent when the peer is not a process this engine owns — [`Peer::over`] connects to
    /// one that is already running, and killing something it did not start is not this
    /// type's to do.
    child: Arc<Mutex<Option<Child>>>,
    deadline: Duration,
}

/// One request line and the slot its answer belongs in.
struct Job {
    /// The serialized request, without its terminating line feed.
    line: String,
    /// Where the worker puts the answer.
    slot: Arc<Slot>,
}

impl Connection {
    /// Adopt a plugin whose handshake has already been answered.
    ///
    /// Taking the streams *after* the handshake is what lets the handshake itself be an
    /// ordinary blocking exchange: it happens while the source is being built, where the
    /// contract's `build` is synchronous anyway, so no runtime is involved and no other
    /// source's future exists yet to be stalled.
    pub(crate) fn adopt(peer: Peer) -> Self {
        let Peer {
            child,
            mut writer,
            mut reader,
            stderr,
            request_deadline,
            handshake_deadline: _,
        } = peer;
        let diagnostics = Arc::new(Mutex::new(String::new()));
        if let Some(stderr) = stderr {
            drain(stderr, Arc::clone(&diagnostics));
        }
        let (sender, receiver) = channel::<Job>();
        std::thread::spawn(move || {
            for job in &receiver {
                let answer = exchange(&mut writer, &mut reader, &job.line);
                let fatal = answer.is_err();
                job.slot.fill(answer);
                if fatal {
                    break;
                }
            }
            // Dropping the receiver is what turns a later `send` into an error instead of
            // a wait nothing would end. Anything already queued is failed by name first.
            for job in receiver.try_iter() {
                job.slot.fill(Err(SourceError::Unavailable {
                    message: "the plugin connection closed before this request was sent".to_owned(),
                }));
            }
        });
        Self {
            jobs: Mutex::new(Some(sender)),
            diagnostics,
            next_id: AtomicU64::new(1),
            child,
            deadline: request_deadline,
        }
    }

    /// Send one method call and wait for the line that answers it.
    ///
    /// # Errors
    ///
    /// Returns the plugin's own [`SourceError`] when it answered with one, and
    /// [`SourceError::Unavailable`] or [`SourceError::Malformed`] when the connection
    /// failed or the answer was not one this protocol allows.
    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value, SourceError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let request = Request {
            id: id.clone(),
            method: method.to_owned(),
            params,
        };
        // A request is built from contract types that all serialize, so this cannot fail
        // for a reason a user could act on.
        let line = serde_json::to_string(&request).expect("a request is plain data");
        let slot = Arc::new(Slot::empty());
        self.dispatch(Job {
            line,
            slot: Arc::clone(&slot),
        })?;
        let timed = Arc::clone(&slot);
        let child = Arc::clone(&self.child);
        let deadline = self.deadline;
        let timed_method = method.to_owned();
        std::thread::spawn(move || {
            std::thread::sleep(deadline);
            let expired = timed.fill_if_empty(Err(SourceError::Unavailable {
                message: format!(
                    "the plugin did not answer {timed_method:?} within {} milliseconds",
                    deadline.as_millis()
                ),
            }));
            if expired
                && let Ok(mut child) = child.lock()
                && let Some(child) = child.as_mut()
            {
                let _ = child.kill();
            }
        });
        let answer = Answer { slot }.await?;
        self.interpret(&id, &answer)
    }

    /// Hand one job to the worker, or say plainly that there is no longer a worker.
    fn dispatch(&self, job: Job) -> Result<(), SourceError> {
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(sender) = jobs.as_ref() else {
            return Err(self.closed());
        };
        if sender.send(job).is_err() {
            *jobs = None;
            return Err(self.closed());
        }
        Ok(())
    }

    /// The one line a caller reads when the plugin is gone, with its own last words.
    fn closed(&self) -> SourceError {
        SourceError::Unavailable {
            message: format!("the plugin stopped answering{}", self.said()),
        }
    }

    /// Whatever the plugin wrote to standard error, as a clause to append to a message.
    fn said(&self) -> String {
        let diagnostics = self
            .diagnostics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let said = diagnostics.trim();
        if said.is_empty() {
            String::new()
        } else {
            format!("; it wrote: {said}")
        }
    }

    /// Turn one answer line into the result or failure it carries.
    fn interpret(&self, id: &str, line: &str) -> Result<Value, SourceError> {
        let response: Response = serde_json::from_str(line).map_err(|error| {
            self.violation(
                format!("the plugin answered with a line that is not a response envelope: {error}"),
                line,
            )
        })?;
        if response.id != id {
            return Err(self.violation(
                format!(
                    "the plugin answered request {id:?} with an envelope addressed to {:?}",
                    response.id
                ),
                line,
            ));
        }
        match response.outcome() {
            Some(outcome) => outcome,
            None => Err(self.violation(
                "the plugin answered with an envelope carrying both a result and an error, \
                 or neither"
                    .to_owned(),
                line,
            )),
        }
    }

    /// A §6.3 protocol violation, quoting the offending line at a readable length.
    fn violation(&self, problem: String, line: &str) -> SourceError {
        SourceError::Malformed {
            message: format!("{problem}: {}{}", quoted(line), self.said()),
        }
    }
}

impl Drop for Connection {
    /// Close standard input, then make sure the child is gone.
    ///
    /// §1.2 step 4 is the polite half: dropping the sender drops the worker's `ChildStdin`
    /// and a well-behaved plugin sees end-of-file and exits `0`. The kill is for the other
    /// kind — a plugin that ignores end-of-file would otherwise outlive the run that
    /// spawned it, and a stranded child holding a user's credentials is the one leak this
    /// process must not walk away from.
    fn drop(&mut self) {
        if let Ok(mut jobs) = self.jobs.lock() {
            *jobs = None;
        }
        if let Ok(mut child) = self.child.lock()
            && let Some(child) = child.as_mut()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A plugin at the other end of a pair of streams, before a worker owns them.
///
/// The two constructors are the whole of what distinguishes a plugin this engine started
/// from one it merely talks to. Everything after the handshake — framing, ids, violations,
/// diagnostics — is the same either way, which is what lets the protocol's two halves be
/// driven against each other over an ordinary pipe rather than only through a process
/// this test suite would then be unable to misbehave on purpose.
pub(crate) struct Peer {
    /// The process, when this engine started one.
    pub(crate) child: Arc<Mutex<Option<Child>>>,
    /// Where requests go.
    pub(crate) writer: Box<dyn Write + Send>,
    /// Where responses come from.
    pub(crate) reader: Box<dyn BufRead + Send>,
    /// Diagnostics only; never parsed (§1).
    pub(crate) stderr: Option<ChildStderr>,
    pub(crate) request_deadline: Duration,
    /// Present only when this engine owns a child it can interrupt during initialization.
    pub(crate) handshake_deadline: Option<Duration>,
}

/// The state the operating system itself needs a spawned plugin to keep, and nothing else.
///
/// `env_clear` below is about *credentials*: §3.1 gives a plugin the variables its own
/// configuration names and nothing more, so a host's unrelated tokens do not cross the
/// pipe. On Windows that clearing takes something with it that is not the user's at all.
/// Winsock's provider catalog records its DLLs as `%SystemRoot%\system32\...`, and it is
/// the *child's* environment that expands them, so a child spawned without that variable
/// cannot open a socket at all: every networked plugin reports its backend as unreachable,
/// on that platform alone. A GitHub Projects board a process away answered on Linux and
/// macOS and was unreachable on Windows for exactly this reason.
///
/// `SystemRoot` is the operating system's own and holds nothing of a user's work and no
/// credential, so §3.1's boundary is where it was: a plugin still reads its configuration
/// and its secrets from the `initialize` request rather than from any of this. Anything
/// else a platform turns out to need is named here beside it, one variable at a time, with
/// the reason it is not the user's.
#[cfg(windows)]
fn keep_platform_state(command: &mut Command) {
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
}

/// Nothing to keep: a Unix child needs no variable of its own in order to reach a socket.
#[cfg(not(windows))]
fn keep_platform_state(_command: &mut Command) {}

impl Peer {
    /// Spawn `program` and take its three streams.
    ///
    /// `handshake` bounds the initializing [`exchange`](Self::exchange), which carries the
    /// child's own start-up inside it; `requests` bounds each exchange after that one.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::Unavailable`] when the command cannot be spawned, naming
    /// the program, because that is nearly always a path that is wrong or not executable
    /// and the message a caller sees is their only clue which.
    pub(crate) fn spawn(
        program: &str,
        args: &[String],
        handshake: Duration,
        requests: Duration,
    ) -> Result<Self, SourceError> {
        let mut command = Command::new(program);
        command
            .args(args)
            // Credentials cross this boundary only in the initialize request (§3.1).
            // Inheriting the engine's environment would also hand the plugin every
            // unrelated token held by its host process.
            .env_clear();
        keep_platform_state(&mut command);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| SourceError::Unavailable {
                message: format!("could not run the plugin program {program:?}: {error}"),
            })?;
        // Every stream was asked for as a pipe immediately above, so none of them can be
        // absent; `expect` here rather than a branch a reader has to weigh.
        let writer = Box::new(child.stdin.take().expect("stdin was piped"));
        let reader = Box::new(BufReader::new(
            child.stdout.take().expect("stdout was piped"),
        ));
        let stderr = child.stderr.take().expect("stderr was piped");
        Ok(Self {
            child: Arc::new(Mutex::new(Some(child))),
            writer,
            reader,
            stderr: Some(stderr),
            request_deadline: requests,
            handshake_deadline: Some(handshake),
        })
    }

    /// Talk to a plugin that is already running, over streams somebody else owns.
    pub(crate) fn over(
        writer: impl Write + Send + 'static,
        reader: impl Read + Send + 'static,
        deadline: Duration,
    ) -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            writer: Box::new(writer),
            reader: Box::new(BufReader::new(reader)),
            stderr: None,
            request_deadline: deadline,
            handshake_deadline: None,
        }
    }

    /// One blocking request and response, for the handshake.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::Unavailable`] when the plugin cannot be written to or has
    /// nothing to say.
    pub(crate) fn exchange(&mut self, line: &str) -> Result<String, SourceError> {
        let Some(deadline) = self.handshake_deadline else {
            return exchange(&mut self.writer, &mut self.reader, line);
        };
        let finished = Arc::new(AtomicBool::new(false));
        let timed_out = Arc::new(AtomicBool::new(false));
        let watched = Arc::clone(&self.child);
        let done = Arc::clone(&finished);
        let expired = Arc::clone(&timed_out);
        std::thread::spawn(move || {
            std::thread::sleep(deadline);
            if !done.load(Ordering::Acquire) {
                expired.store(true, Ordering::Release);
                if let Ok(mut child) = watched.lock()
                    && let Some(child) = child.as_mut()
                {
                    let _ = child.kill();
                }
            }
        });
        let answer = exchange(&mut self.writer, &mut self.reader, line);
        finished.store(true, Ordering::Release);
        if timed_out.load(Ordering::Acquire) {
            Err(SourceError::Unavailable {
                message: format!(
                    "the plugin did not answer the initialize request within {} milliseconds",
                    deadline.as_millis()
                ),
            })
        } else {
            answer
        }
    }

    /// Whatever the plugin wrote to standard error, once it can no longer write more.
    ///
    /// Only ever reached on a handshake that failed, where this peer is being discarded
    /// and the one remaining question is what it said. So the child is ended first, on
    /// purpose: reading a live plugin's standard error means waiting for a line it may
    /// never write, which would turn a diagnostic into a hang — and a plugin whose
    /// handshake failed has no connection left to keep open. With the writing end gone the
    /// read reaches end-of-file, so this returns rather than waits.
    pub(crate) fn said(&mut self) -> String {
        if let Ok(mut child) = self.child.lock()
            && let Some(child) = child.as_mut()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        let Some(stderr) = self.stderr.as_mut() else {
            return String::new();
        };
        let mut said = String::new();
        let mut reader = BufReader::new(stderr);
        while said.len() < KEPT_DIAGNOSTICS {
            let mut line = String::new();
            // Bounded by what is still wanted rather than by the line: a plugin whose
            // diagnostic is one enormous line must not decide how much of this process's
            // memory it takes, and the outer condition cannot say that on its own — it is
            // only consulted between lines.
            let room = (KEPT_DIAGNOSTICS - said.len()) as u64;
            match (&mut reader).take(room).read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => said.push_str(&line),
            }
        }
        said.trim().to_owned()
    }
}

/// Write one line, flush it, and read the one that answers.
fn exchange(
    writer: &mut (impl Write + ?Sized),
    reader: &mut (impl BufRead + ?Sized),
    line: &str,
) -> Result<String, SourceError> {
    writeln!(writer, "{line}")
        .and_then(|()| writer.flush())
        .map_err(|error| SourceError::Unavailable {
            message: format!("could not send a request to the plugin: {error}"),
        })?;
    match read_line(reader) {
        Line::Read(answer) => Ok(answer),
        Line::Ended => Err(SourceError::Unavailable {
            message: "the plugin closed its output without answering".to_owned(),
        }),
        Line::TooLong => Err(SourceError::Malformed {
            message: format!(
                "the plugin wrote more than {MAX_LINE} bytes without ending the line; a \
                 response is one line and this engine will not hold an unbounded one"
            ),
        }),
        Line::Failed(error) => Err(SourceError::Unavailable {
            message: format!("could not read the plugin's answer: {error}"),
        }),
    }
}

/// Keep the plugin's standard error, bounded, on a thread of its own.
///
/// A thread rather than a read at failure time because a plugin that fills the pipe's
/// buffer and blocks writing to it would never answer the request the engine is waiting
/// on — a deadlock whose symptom is a hang rather than a diagnostic.
fn drain(stderr: ChildStderr, into: Arc<Mutex<String>>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        loop {
            let mut line = String::new();
            // One line at a time, each bounded on its own, because the cap has to hold
            // against a plugin that logs one line and never ends it as well as against one
            // that logs for ever. Reading past the cap and dropping the excess keeps the
            // pipe drained — a plugin blocked writing to a full stderr never answers the
            // request the engine is waiting on.
            match (&mut reader).take(MAX_LINE).read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let mut kept = into.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let room = KEPT_DIAGNOSTICS.saturating_sub(kept.len());
            if room > 0 {
                let end = line
                    .char_indices()
                    .nth(room)
                    .map_or(line.len(), |(at, _)| at);
                kept.push_str(&line[..end]);
            }
        }
    });
}

/// One line, cut to a length a person can read, saying so when it was cut.
fn quoted(line: &str) -> String {
    let line = line.trim();
    match line.char_indices().nth(QUOTED) {
        None => format!("{line:?}"),
        Some((at, _)) => format!("{:?} (truncated)", &line[..at]),
    }
}

/// Where a worker thread leaves an answer for the future that is waiting on it.
struct Slot {
    /// The answer and the waker, together, so filling one and waking the other cannot
    /// interleave with a poll that reads them in the other order.
    state: Mutex<SlotState>,
}

/// What a slot holds between the request going out and the caller reading it.
#[derive(Default)]
struct SlotState {
    /// The answer line, or the failure that ended the connection.
    answer: Option<Result<String, SourceError>>,
    /// Set permanently by the first answer, even after the future consumes that answer.
    completed: bool,
    /// The waiting task, if it has polled at least once.
    waker: Option<Waker>,
}

impl Slot {
    /// A slot with nothing in it yet.
    fn empty() -> Self {
        Self {
            state: Mutex::new(SlotState::default()),
        }
    }

    /// Leave an answer and wake whoever is waiting for it.
    fn fill(&self, answer: Result<String, SourceError>) {
        let _ = self.fill_if_empty(answer);
    }

    /// Leave the first answer only; a deadline racing a real response must not replace it.
    fn fill_if_empty(&self, answer: Result<String, SourceError>) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.completed {
            return false;
        }
        state.completed = true;
        state.answer = Some(answer);
        let waker = state.waker.take();
        drop(state);
        if let Some(waker) = waker {
            waker.wake();
        }
        true
    }
}

/// The future half of one exchange: pending until the worker fills the slot.
struct Answer {
    /// Shared with the worker thread that will fill it.
    slot: Arc<Slot>,
}

impl Future for Answer {
    type Output = Result<String, SourceError>;

    fn poll(self: std::pin::Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self
            .slot
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state.answer.take() {
            Some(answer) => Poll::Ready(answer),
            None => {
                // Replaced rather than kept: a future polled on a second task must be
                // woken through that task's waker, not the one that first polled it.
                state.waker = Some(context.waker().clone());
                Poll::Pending
            }
        }
    }
}
