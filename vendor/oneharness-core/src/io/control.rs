//! Out-of-band turn control: the I/O half.
//!
//! Two sides of one unix socket:
//!
//! * [`ControlListener`] — opened by `oneharness run --control` for the run's
//!   lifetime. It owns the socket file (0600, removed on drop, including on a
//!   panic) and serves one newline-terminated request per connection against a
//!   [`ControlHandle`], which knows how to reach the harness mechanism.
//! * [`send`] — used by the separate `oneharness interrupt` process. It is a
//!   different process from the run; that is the whole reason this is a socket
//!   and not a flag.
//!
//! Unix only. `std` has no unix-socket type on Windows, so the command layer
//! refuses `--control` there loudly rather than opening nothing and reporting
//! success.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::domain::app_server::InterruptReply;
use crate::domain::control::{
    interrupt_frame, prompt_frame, AppServerReport, ControlEvent, ControlReason, ControlRequest,
    ControlResponse, ControlShape, ControlVerb, RedirectInput,
};
use crate::domain::dialogue::Dialogue;
use crate::domain::http::HttpShape;
use crate::domain::usage::UtcInstant;
use crate::io::http_turn::HttpTurn;

/// How long a client waits for a run to answer before giving up. Generous
/// enough for a busy run, short enough that a wedged peer is not a hang.
///
/// Unix-gated with the socket code it bounds: on Windows there is no socket to
/// wait on, and [`imp::send`] refuses before it would ever have one.
#[cfg(unix)]
const CLIENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a redirecting interrupt waits for the run to write its message as
/// the next turn's prompt before taking the message back.
///
/// The answer to a redirect is given on delivery, and delivery waits for the
/// aborted turn to end — which is the harness's to decide. Unbounded, a turn
/// that never ends would hold the supervisor's process forever; so past this
/// the run withdraws the message (nothing will write it afterwards) and refuses,
/// which keeps "the caller always knows" true however long the harness takes.
const REDIRECT_DELIVERY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// A delivery deadline, in milliseconds, replacing
/// [`REDIRECT_DELIVERY_DEADLINE`] for the run this process serves. Internal: it
/// exists so a hermetic journey can watch a redirect be taken back without
/// waiting a minute.
pub(crate) const REDIRECT_DEADLINE_ENV: &str = "ONEHARNESS_INTERNAL_REDIRECT_DELIVERY_MS";

/// A file the run appends `REDIRECT_WRITTEN` to the moment a redirection's
/// replacement prompt has been written to the harness — after the write, before
/// the interrupt is answered. Internal: a hermetic journey points it at its
/// harness double's own log, so the write and the answer land in one ordered
/// stream it can read, on mechanisms whose harness sends no acknowledgement.
pub(crate) const REDIRECT_TRACE_ENV: &str = "ONEHARNESS_INTERNAL_REDIRECT_TRACE";

/// Append `REDIRECT_WRITTEN` to the trace file, when one is named. Best
/// effort: a trace that cannot be written never touches the run.
fn trace_redirect_written() {
    let Some(path) = std::env::var_os(REDIRECT_TRACE_ENV) else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write as _;
        // One append includes the newline: the harness can append its answer
        // concurrently, so separate formatting writes can merge two records.
        let _ = file.write_all(b"REDIRECT_WRITTEN\n");
    }
}

/// The delivery deadline this process's runs wait out.
fn redirect_delivery_deadline() -> std::time::Duration {
    let Ok(raw) = std::env::var(REDIRECT_DEADLINE_ENV) else {
        return REDIRECT_DELIVERY_DEADLINE;
    };
    raw.parse::<u64>().map_or_else(
        |_| {
            eprintln!(
                "oneharness: warning: ignoring {REDIRECT_DEADLINE_ENV}={raw:?}: not whole \
                 milliseconds; a redirect waits the default {}s",
                REDIRECT_DELIVERY_DEADLINE.as_secs()
            );
            REDIRECT_DELIVERY_DEADLINE
        },
        std::time::Duration::from_millis,
    )
}

/// The most one frame may be — in either direction — before it is refused
/// unread.
///
/// A control frame is a single small JSON object, so this is orders of
/// magnitude of headroom. It exists because both readers are waiting for a
/// newline the *peer* controls: without a bound, a peer that writes without
/// ever terminating its line grows this process's buffer for as long as it
/// keeps writing. It binds the answer as well as the request — the client is
/// the supervisor's process, and whatever is listening on that socket path is
/// no more trusted by it than a caller is by the run.
///
/// Unix-gated with the two readers it bounds, for the same reason as
/// [`CLIENT_TIMEOUT`]: neither reader is compiled on a platform with no unix
/// socket to read from.
#[cfg(unix)]
const MAX_FRAME_BYTES: u64 = 64 * 1024;

/// The single clock read this module makes, kept here so
/// [`crate::domain::control`] stays pure. A pre-1970 clock falls back to the
/// epoch rather than panicking — control is a side channel and never takes a
/// run down.
fn now() -> UtcInstant {
    UtcInstant::from_epoch(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    )
}

/// The live end of a stdin-borne control mechanism.
///
/// A control-enabled run parks the child's stdin here instead of dropping it
/// after the prompt write, so the listener thread can push an interrupt frame
/// into the *same* process the turn is running in. The state machine is what
/// makes `no_active_turn` honest: before the prompt is written and after the
/// turn's terminal document arrives, there is nothing to interrupt.
enum TurnState {
    /// No turn in flight (before the prompt, or after the turn ended).
    Idle,
    /// A turn is running; the line sink frames go to is held open for control
    /// frames — the child's stdin, or a shared app-server connection.
    Active(Box<dyn io::Write + Send>),
}

/// The answers a pooled turn's connection delivers for requests the control
/// thread sent, keyed by JSON-RPC id.
///
/// On a dispatch's own child an interrupt is a write and nothing more. On a
/// shared app-server the interrupt waits for the server's answer, bounded by
/// [`crate::io::server_pool::INTERRUPT_REPLY_DEADLINE`]: the reader that sees
/// the answer is the turn's own loop, on another thread, so it hands the answer
/// over here.
#[derive(Default)]
pub(crate) struct ReplyBoard {
    pending: Mutex<std::collections::HashMap<u64, Option<serde_json::Value>>>,
    arrived: std::sync::Condvar,
}

impl ReplyBoard {
    /// Expect an answer to `id`. Called before the request is written, so an
    /// answer arriving at once is never missed.
    pub(crate) fn expect(&self, id: u64) {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, None);
    }

    /// Hand over a response; `true` when someone was waiting for it.
    pub(crate) fn deliver(&self, id: u64, response: serde_json::Value) -> bool {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match pending.get_mut(&id) {
            Some(slot) => {
                *slot = Some(response);
                self.arrived.notify_all();
                true
            }
            None => false,
        }
    }

    /// Wait for the answer to `id` until `deadline`; `None` if none came.
    pub(crate) fn wait(&self, id: u64, deadline: std::time::Duration) -> Option<serde_json::Value> {
        let until = std::time::Instant::now() + deadline;
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if let Some(Some(_)) = pending.get(&id) {
                return pending.remove(&id).flatten();
            }
            let now = std::time::Instant::now();
            if now >= until {
                pending.remove(&id);
                return None;
            }
            pending = self
                .arrived
                .wait_timeout(pending, until - now)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }
}

/// What became of one redirection, from the moment an interrupt committed it
/// to the moment the run wrote it — or could not.
///
/// The interrupt that committed it waits on this before answering, so
/// `redirected: true` is said only of a message that is already the next turn's
/// prompt. Whoever settles it is the only party that could have delivered it:
/// the thread that writes the replacement turn, or the path that ends the turn
/// or the run without one. `Writing` is what makes that exclusive — a writer
/// claims the message before it writes, and a withdrawal only takes back one
/// still `Pending`, so the text is written at most once and never after the
/// caller was told it was not.
struct RedirectDelivery {
    state: Mutex<DeliveryState>,
    settled: std::sync::Condvar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeliveryState {
    /// Committed; nothing has tried to write it yet.
    Pending,
    /// A writer has claimed it and is writing it now.
    Writing,
    /// Written as the prompt of the turn that replaces the aborted one — the
    /// write itself on Claude Code's stdin and ACP, and where the harness says
    /// whether it took the turn, that answer: codex's acknowledgement, an HTTP
    /// server's admission.
    Delivered,
    /// It will never be written: why, and the refusal reason that says so.
    Undelivered(ControlReason, String),
}

impl RedirectDelivery {
    fn new() -> Arc<Self> {
        Arc::new(RedirectDelivery {
            state: Mutex::new(DeliveryState::Pending),
            settled: std::sync::Condvar::new(),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, DeliveryState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Claim the message for writing: `true` only for one still `Pending`, so a
    /// message already withdrawn — or already written — is never written again.
    fn claim(&self) -> bool {
        let mut state = self.state();
        if *state == DeliveryState::Pending {
            *state = DeliveryState::Writing;
            true
        } else {
            false
        }
    }

    /// Record the final outcome. A settled delivery stays settled: the first
    /// outcome is the one the waiting interrupt answers with.
    fn settle(&self, outcome: DeliveryState) {
        let mut state = self.state();
        if matches!(*state, DeliveryState::Pending | DeliveryState::Writing) {
            *state = outcome;
            self.settled.notify_all();
        }
    }

    /// Refuse a message nothing has claimed yet; `true` when this withdrew it.
    fn withdraw(&self, reason: ControlReason, why: String) -> bool {
        let mut state = self.state();
        if *state == DeliveryState::Pending {
            *state = DeliveryState::Undelivered(reason, why);
            self.settled.notify_all();
            true
        } else {
            false
        }
    }

    /// Wait for the outcome. A message still unclaimed at `deadline` is
    /// withdrawn; one being written is waited out, because its write is
    /// already happening and only its result says whether it landed.
    fn outcome(&self, deadline: std::time::Duration) -> DeliveryState {
        let until = std::time::Instant::now() + deadline;
        let mut state = self.state();
        loop {
            match &*state {
                DeliveryState::Delivered | DeliveryState::Undelivered(..) => return state.clone(),
                DeliveryState::Writing => {
                    state = self
                        .settled
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                DeliveryState::Pending => {
                    let now = std::time::Instant::now();
                    if now >= until {
                        *state = DeliveryState::Undelivered(
                            ControlReason::NotRunning,
                            format!(
                                "the interrupted turn did not end within {deadline:?}, so the \
                                 redirection was not delivered and was taken back"
                            ),
                        );
                        return state.clone();
                    }
                    state = self
                        .settled
                        .wait_timeout(state, until - now)
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .0;
                }
            }
        }
    }
}

/// Why a redirection whose aborted turn ended without it was not delivered.
fn turn_ended_undelivered(cause: Option<&dyn std::fmt::Display>) -> DeliveryState {
    DeliveryState::Undelivered(
        ControlReason::NoActiveTurn,
        match cause {
            Some(cause) => format!(
                "the interrupted turn ended but its replacement could not be opened ({cause}); \
                 the redirection was not delivered"
            ),
            None => "the interrupted turn ended before its redirection could be written; the \
                     redirection was not delivered"
                .to_string(),
        },
    )
}

/// Why a redirection whose run ended first was not delivered.
fn run_ended_undelivered() -> DeliveryState {
    DeliveryState::Undelivered(
        ControlReason::NotRunning,
        "the run's harness exited before the redirection could be written; the redirection \
         was not delivered"
            .to_string(),
    )
}

/// Where the serving candidate's turn on a shared app-server is, and what it
/// leaves for the candidate's result. `command` is the server's launch argv,
/// which the result records as what oneharness ran.
#[derive(Default)]
enum PooledState {
    /// No turn on a shared app-server: the candidate drives its own child, or
    /// has not opened its turn yet.
    #[default]
    None,
    /// The turn is live; an interrupt's answer arrives on `board`.
    Live {
        board: std::sync::Arc<ReplyBoard>,
        command: Vec<String>,
    },
    /// The turn is over, and how it ended.
    Ended {
        command: Vec<String>,
        end: PooledEnd,
    },
}

/// How a turn on a shared app-server ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PooledEnd {
    /// The turn reached its own end, whatever that end said.
    Finished,
    /// The server went away under the live turn.
    ServerLost,
}

/// What drives — and therefore what interrupts — the turn a candidate is
/// serving.
///
/// Each variant carries what its own shape is knowable from, so
/// [`Backend::shape`] is derived. A shape stored beside it could be set to one
/// the backend does not speak — a channel naming a mechanism it is not on,
/// which is the one thing a supervisor must never be able to address.
enum Backend {
    /// Control rides the harness's ordinary run: an interrupt is a frame on the
    /// child's own stdin. Claude Code's is the only mechanism that neither
    /// drives a turn nor needs a server, so the variant needs no payload.
    Stdin,
    /// A JSON-RPC conversation drives the turn and is the only thing that knows
    /// the ids an interrupt has to address. Boxed, as [`Binding::Dialogue`]
    /// already hands it over: a conversation carries its config, its text and
    /// what the server said, several times the size of the other variants.
    Dialogue(Box<Dialogue>),
    /// Submitted to a control server, so an interrupt is one more request
    /// against the same session. The turn is `Some` only between its opening
    /// and its end — the window in which there is something to abort.
    Http(HttpShape, Option<HttpTurn>),
}

impl Backend {
    fn shape(&self) -> ControlShape {
        match self {
            Backend::Stdin => ControlShape::ClaudeControlRequest,
            Backend::Dialogue(dialogue) => dialogue.shape(),
            Backend::Http(http, _) => http.shape(),
        }
    }

    fn of(binding: Binding) -> Self {
        match binding {
            Binding::Stdin => Backend::Stdin,
            Binding::Dialogue(dialogue) => Backend::Dialogue(dialogue),
            Binding::PooledServer(http) => Backend::Http(http, None),
        }
    }
}

/// What a candidate binds when it takes the turn: [`ControlHandle::bind`]'s
/// whole argument.
///
/// An enum rather than a `(ControlShape, Option<Dialogue>)` pair, so a shape
/// and a conversation that does not speak it cannot be handed over together.
/// Crate-internal, like [`ControlHandle::bind`]: which candidate holds the
/// channel is the runner's to decide, and a consumer rebinding it out of band
/// would move the socket off the turn its own run is serving.
pub(crate) enum Binding {
    /// Claude Code's control frame on the child's own stdin.
    Stdin,
    /// A JSON-RPC conversation, which reports the protocol it was built for.
    /// Boxed because it dwarfs the other two variants.
    Dialogue(Box<Dialogue>),
    /// A turn submitted to a pooled control server of this shape.
    PooledServer(HttpShape),
}

/// Shared, thread-safe access to a run's control mechanism plus the audit trail
/// of everything it served (which the report echoes).
///
/// The mechanism is **late-bound**: a fallback chain reaches candidate N+1 only
/// because candidate N has finished, so exactly one candidate is ever serving —
/// but *which* one, and therefore which mechanism, is not knowable before the
/// chain runs. So the handle is created unbound, each candidate binds its own
/// mechanism as it takes the turn (the crate-private `ControlHandle::bind`) and
/// releases it when that turn ends (`ControlHandle::release`), and an interrupt
/// is served by whatever is bound at the moment it arrives. Between candidates
/// nothing is, which is why a fall-through answers `no_active_turn` rather than
/// reaching a mechanism that is not the serving one.
///
/// Every lock here recovers from poisoning (`into_inner`) rather than
/// propagating a panic: control is a side channel, and a worker that panicked
/// mid-turn must not also take down the run whose results are the deliverable.
/// The guarded state is a stdin handle and an append-only log, neither of which
/// a panic can leave half-updated in a way a later write would misread.
pub struct ControlHandle {
    /// The mechanism the report names: the last one bound, or — before any
    /// candidate has taken the turn — the one the chain would start on.
    reported: Mutex<ControlShape>,
    state: Mutex<TurnState>,
    events: Mutex<Vec<ControlEvent>>,
    requests: Mutex<u64>,
    /// The serving candidate's mechanism, or `None` between candidates. Locked
    /// *before* `state` everywhere, so the two paths that touch both (serving an
    /// interrupt, advancing on a line) can never deadlock against each other.
    mechanism: Mutex<Option<Backend>>,
    /// A redirection an interrupt committed. Only the stdin and HTTP backends
    /// use it — a dialogue holds its own, because opening the replacement turn
    /// is a protocol decision rather than a write.
    redirect: Mutex<Redirect>,
    /// What became of the redirection an interrupt committed, which that
    /// interrupt waits on before answering. One at a time: the listener serves
    /// one request at a time, and a redirect's request is answered only once
    /// this is settled.
    delivery: Mutex<Option<Arc<RedirectDelivery>>>,
    /// How long a redirect waits to be written before it is taken back.
    delivery_deadline: Mutex<std::time::Duration>,
    /// The serving candidate's shared app-server state, when its turn runs on
    /// one.
    pooled: Mutex<PooledState>,
    /// The last shared app-server any candidate ran on, for the report.
    app_server: Mutex<Option<AppServerReport>>,
}

/// Where a redirection is between being accepted and becoming the next turn.
///
/// Three states rather than an `Option` plus a flag, because the middle one is
/// what makes the operation atomic: the run owns the message from the moment it
/// accepts the interrupt — so it can never be lost — but must not hand it to the
/// harness until the abort it rides has actually landed, or the prompt joins the
/// turn being cancelled. "Committed but not yet deliverable" is exactly that
/// window, and two independent fields would let a reader see a message as
/// deliverable while its abort was still in flight.
enum Redirect {
    /// Nothing was committed.
    None,
    /// Accepted from the supervisor; the abort has not landed yet.
    Committed(RedirectInput),
    /// The abort landed, so the turn is over and this is the next one.
    Deliverable(RedirectInput),
}

impl ControlHandle {
    /// An unbound handle whose report names `starts_on` until a candidate binds
    /// its own mechanism.
    ///
    /// Unbound rather than serving `starts_on` straight away: the run has not
    /// chosen a candidate yet, and an interrupt arriving now must be
    /// `no_active_turn` rather than a frame written at a mechanism nobody is on.
    #[must_use]
    pub fn new(starts_on: ControlShape) -> Self {
        ControlHandle {
            reported: Mutex::new(starts_on),
            state: Mutex::new(TurnState::Idle),
            events: Mutex::new(Vec::new()),
            requests: Mutex::new(0),
            mechanism: Mutex::new(None),
            redirect: Mutex::new(Redirect::None),
            delivery: Mutex::new(None),
            delivery_deadline: Mutex::new(redirect_delivery_deadline()),
            pooled: Mutex::new(PooledState::default()),
            app_server: Mutex::new(None),
        }
    }

    /// Bind the channel to the mechanism of the candidate that is about to
    /// serve.
    ///
    /// This is the whole of late binding: the socket a supervisor addresses is
    /// the run's for its whole lifetime, and what sits behind it is whichever
    /// candidate is serving. Called before the candidate's turn opens; every
    /// interrupt served from here until [`Self::release`] reaches that turn.
    pub(crate) fn bind(&self, binding: Binding) {
        let backend = Backend::of(binding);
        // Reported from the backend, which is the only thing that knows what it
        // speaks — so the shape a supervisor is told is the one answering them.
        let bound = backend.shape();
        *self.mechanism() = Some(backend);
        *self.pooled() = PooledState::default();
        *self
            .reported
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = bound;
    }

    /// Release the serving candidate's mechanism: its turn is over, so an
    /// interrupt arriving before the next candidate binds is `no_active_turn`
    /// rather than a request against a turn that has finished.
    ///
    /// A redirection still held here belonged to the turn that just ended, so it
    /// is reported and dropped rather than carried across a fall-through: a
    /// message a supervisor aimed at one candidate's turn must never be
    /// delivered into a different candidate's fresh one, which nobody saw start.
    /// Both stores are asked, because which one holds it depends on the
    /// mechanism: a dialogue keeps its own, and dropping the backend without
    /// asking would lose a codex or ACP redirection in silence — the one
    /// outcome this warning exists to prevent.
    ///
    /// The backend is taken and inspected under ONE acquisition of its lock:
    /// reading it, letting go, then clearing it would leave a window in which an
    /// interrupt could commit a redirection into a conversation that is about to
    /// be dropped — accepted, then discarded in the silence this reports.
    ///
    /// Each field is still taken in its own statement so no two of these locks
    /// are ever held at once — the ordering the rest of this type relies on
    /// (`mechanism` before `redirect` before `state`) is a nesting rule, and the
    /// cheapest way to obey it here is to nest nothing.
    ///
    /// The interrupt waiting on that redirection is answered here too — with
    /// `not_running`, since the candidate it was aimed at is gone — so the
    /// supervisor learns the message is still theirs rather than being told it
    /// was delivered.
    pub(crate) fn release(&self) {
        let undelivered = matches!(
            self.mechanism().take(),
            Some(Backend::Dialogue(dialogue)) if dialogue.has_pending_redirect()
        );
        let held = std::mem::replace(&mut *self.redirect(), Redirect::None);
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = TurnState::Idle;
        if undelivered || !matches!(held, Redirect::None) {
            eprintln!(
                "oneharness: warning: the interrupted turn ended before its redirection could be \
                 delivered; the message was not carried to another candidate"
            );
        }
        self.settle_delivery(run_ended_undelivered());
    }

    /// Bind the live HTTP turn this handle interrupts. Called once the session
    /// exists on the control server — before that there is no turn to address,
    /// and `interrupt` correctly answers `no_active_turn`.
    pub fn begin_http_turn(&self, turn: HttpTurn) {
        if let Some(Backend::Http(_, live)) = self.mechanism().as_mut() {
            *live = Some(turn);
        }
    }

    /// Release the HTTP turn: it is over, so a later interrupt is
    /// `no_active_turn` rather than a request against a finished session.
    pub fn end_http_turn(&self) {
        if let Some(Backend::Http(_, live)) = self.mechanism().as_mut() {
            *live = None;
        }
    }

    /// The frames that open the turn: a dialogue's handshake, or the prompt
    /// frame for a mechanism that rides the harness's ordinary run. Empty when
    /// nothing is bound — there is no turn to open.
    pub fn open_frames(&self, prompt: &str) -> Vec<String> {
        let mut backend = self.mechanism();
        let Some(backend) = backend.as_mut() else {
            return Vec::new();
        };
        let shape = backend.shape();
        match backend {
            Backend::Dialogue(dialogue) => dialogue.open(),
            Backend::Stdin | Backend::Http(..) => {
                crate::domain::control::prompt_frame(shape, prompt)
                    .into_iter()
                    .collect()
            }
        }
    }

    /// Feed one stdout line to the dialogue, writing whatever it answers with.
    /// Returns `true` when the turn is over.
    ///
    /// A write failure is not fatal here: the server has gone, which its own
    /// exit and output will report far more usefully than a panic would.
    pub fn advance(&self, line: &str) -> bool {
        let (step, opened_redirect, awaits_answer, answered) = {
            let mut backend = self.mechanism();
            // Nothing is bound, so there is no turn this line can be part of and
            // none for the caller to keep open.
            let Some(backend) = backend.as_mut() else {
                return true;
            };
            let shape = backend.shape();
            match backend {
                // A redirection the dialogue held before this line and not after
                // it was taken by the turn's end: the step is either the frames
                // opening its replacement turn or the end of a conversation that
                // could not open one.
                //
                // Codex answers the replacement turn's request at once, and that
                // answer — not the write — is what says the turn exists.
                Backend::Dialogue(dialogue) => {
                    let held = dialogue.has_pending_redirect();
                    let step = dialogue.on_line(line);
                    (
                        step,
                        held && !dialogue.has_pending_redirect(),
                        dialogue.awaits_redirect_answer(),
                        dialogue.take_redirect_answer(),
                    )
                }
                // No conversation to advance: the harness's own end-of-turn
                // document is the only signal, and there is nothing to write
                // back.
                Backend::Stdin | Backend::Http(..) => {
                    if !crate::domain::control::is_turn_terminal(shape, line) {
                        return false;
                    }
                    // The aborted turn just ended, which is the first moment
                    // this harness will accept the message an interrupt
                    // committed: written mid-turn it is silently dropped, and
                    // written after `end_turn` closes stdin there is nothing to
                    // write to. So the turn is kept open for one more round.
                    return !self.open_redirected_turn(shape);
                }
            }
        };
        match answered {
            Some(Ok(())) => self.redirect_delivered(),
            Some(Err(why)) => self.redirect_refused(&why),
            None => {}
        }
        // Claimed before the replacement turn is written, so a redirection the
        // waiting interrupt has already taken back is never written after it.
        let delivery = match (&step, opened_redirect) {
            (_, false) => None,
            (crate::domain::dialogue::DialogueStep::Send(_), true) => {
                match self.claim_delivery() {
                    Some(delivery) => Some(delivery),
                    // Withdrawn: the aborted turn is over and nothing replaces
                    // it, so the conversation ends here.
                    None => return true,
                }
            }
            (_, true) => {
                self.settle_delivery(turn_ended_undelivered(None));
                None
            }
        };
        if let crate::domain::dialogue::DialogueStep::Send(frames) = &step {
            for frame in frames {
                if let Err(err) = self.write_line(frame) {
                    eprintln!("oneharness: warning: could not write a control frame: {err}");
                    if let Some(delivery) = &delivery {
                        delivery.settle(turn_ended_undelivered(Some(&err)));
                    }
                    return true;
                }
            }
        }
        // Written. That IS delivery, and from here the message is never handed
        // back — unless the server is still to say whether it took the turn
        // (codex), when its answer, a line from now, settles it.
        if let Some(delivery) = delivery {
            trace_redirect_written();
            if !awaits_answer {
                delivery.settle(DeliveryState::Delivered);
            }
        }
        step == crate::domain::dialogue::DialogueStep::Terminal
    }

    /// The harness's native session id, when the dialogue captured one.
    #[must_use]
    pub fn session_id(&self) -> Option<String> {
        match self.mechanism().as_ref() {
            Some(Backend::Dialogue(dialogue)) => dialogue.session_id().map(str::to_string),
            Some(Backend::Stdin | Backend::Http(..)) | None => None,
        }
    }

    /// The model the server itself said the conversation would run under, when
    /// the dialogue read one off its open response. `None` for every other
    /// mechanism — never inferred.
    #[must_use]
    pub fn observed_model(&self) -> Option<String> {
        match self.mechanism().as_ref() {
            Some(Backend::Dialogue(dialogue)) => dialogue.observed_model().map(str::to_string),
            Some(Backend::Stdin | Backend::Http(..)) | None => None,
        }
    }

    /// Why the dialogue ended without opening a turn, when it did — with the
    /// source the reading came from, for the result's `failure_kind_source`.
    #[must_use]
    pub fn refusal(&self) -> Option<(crate::domain::dialogue::DialogueRefusal, &'static str)> {
        match self.mechanism().as_ref() {
            Some(Backend::Dialogue(dialogue)) => dialogue
                .refusal()
                .map(|refusal| (refusal.clone(), dialogue.text_source())),
            Some(Backend::Stdin | Backend::Http(..)) | None => None,
        }
    }

    /// The assistant text the dialogue accumulated, with how it was obtained.
    /// `None` when the turn produced none — never fabricated.
    #[must_use]
    pub fn text(&self) -> Option<(String, &'static str)> {
        match self.mechanism().as_ref() {
            Some(Backend::Dialogue(dialogue)) => {
                dialogue.text().map(|text| (text, dialogue.text_source()))
            }
            Some(Backend::Stdin | Backend::Http(..)) | None => None,
        }
    }

    /// The token usage the server reported for the dialogue's turns, with how
    /// it was obtained. `None` when it reported none — never fabricated.
    #[must_use]
    pub fn usage(&self) -> Option<(crate::domain::signals::Usage, &'static str)> {
        match self.mechanism().as_ref() {
            Some(Backend::Dialogue(dialogue)) => dialogue
                .usage()
                .map(|usage| (usage, dialogue.text_source())),
            Some(Backend::Stdin | Backend::Http(..)) | None => None,
        }
    }

    /// Whether this handle drives its turn over a JSON-RPC dialogue on the
    /// child's stdin, in which case the runner tears the server down once the
    /// turn ends instead of waiting for it to exit on its own.
    ///
    /// Narrower than [`ControlShape::drives_turn`] on purpose: the HTTP
    /// mechanisms drive their turns too, but not through any child of this
    /// process — nothing about a spawned run applies to them.
    #[must_use]
    pub fn drives_turn_over_stdin(&self) -> bool {
        matches!(self.mechanism().as_ref(), Some(Backend::Dialogue(_)))
    }

    fn mechanism(&self) -> std::sync::MutexGuard<'_, Option<Backend>> {
        self.mechanism
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn redirect(&self) -> std::sync::MutexGuard<'_, Redirect> {
        self.redirect
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Take the redirection an interrupt committed, once its abort has landed.
    ///
    /// Taken rather than read: it is delivered exactly once, so a turn that ends
    /// again — or ends badly — costs one attempt instead of looping. `None`
    /// while the abort is still in flight, so a driver polling for work to do
    /// cannot submit the next turn into the one being cancelled.
    ///
    /// Taking it claims its delivery: the interrupt that committed it is
    /// answered once the run's own driver says whether it landed, or — if
    /// nothing ever says — with `not_running` when the candidate is released.
    /// `None` too for a message that interrupt already took back.
    #[must_use]
    pub fn take_redirect(&self) -> Option<RedirectInput> {
        let input = {
            let mut slot = self.redirect();
            match std::mem::replace(&mut *slot, Redirect::None) {
                Redirect::Deliverable(input) => input,
                // Put back: it is still the run's, just not yet the harness's.
                other => {
                    *slot = other;
                    return None;
                }
            }
        };
        self.claim_delivery().map(|_| input)
    }

    /// The redirection [`Self::take_redirect`] handed over is now the prompt of
    /// the turn that replaces the aborted one.
    pub(crate) fn redirect_delivered(&self) {
        self.settle_delivery(DeliveryState::Delivered);
    }

    /// The redirection [`Self::take_redirect`] handed over could not become a
    /// turn, for the reason `why` names. Nothing will try it again.
    pub(crate) fn redirect_refused(&self, why: &str) {
        self.settle_delivery(turn_ended_undelivered(Some(&why)));
    }

    /// The redirection [`Self::take_redirect`] handed over never reached the
    /// harness, which is gone: `why` says how that was learned.
    pub(crate) fn redirect_lost(&self, why: &str) {
        self.settle_delivery(DeliveryState::Undelivered(
            ControlReason::NotRunning,
            format!(
                "the run's harness could not be reached to open the redirected turn ({why}); \
                 the redirection was not delivered"
            ),
        ));
    }

    /// The aborted turn has ended and nothing will open a turn after it: a
    /// redirection whose abort had not landed yet was not delivered, and is
    /// dropped and refused here rather than carried into a turn nobody opened.
    pub(crate) fn redirect_turn_ended(&self) {
        let held = std::mem::replace(&mut *self.redirect(), Redirect::None);
        match held {
            Redirect::Committed(_) => self.settle_delivery(turn_ended_undelivered(None)),
            // Still the run's to deliver: whoever ends the run answers for it.
            other => *self.redirect() = other,
        }
    }

    fn delivery(&self) -> std::sync::MutexGuard<'_, Option<Arc<RedirectDelivery>>> {
        self.delivery
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Start tracking a redirection an interrupt is committing now.
    fn open_delivery(&self) -> Arc<RedirectDelivery> {
        let delivery = RedirectDelivery::new();
        *self.delivery() = Some(Arc::clone(&delivery));
        delivery
    }

    /// Claim the committed redirection for a write; `None` when it was taken
    /// back (or never tracked), in which case it must not be written.
    fn claim_delivery(&self) -> Option<Arc<RedirectDelivery>> {
        let delivery = self.delivery().clone()?;
        delivery.claim().then_some(delivery)
    }

    /// Settle the committed redirection, if one is still unsettled.
    fn settle_delivery(&self, outcome: DeliveryState) {
        if let Some(delivery) = self.delivery().take() {
            delivery.settle(outcome);
        }
    }

    /// Whether a committed redirection's abort has landed, so a test can
    /// end the aborted turn only once the message may become the next one.
    #[cfg(all(test, unix))]
    pub(crate) fn redirect_is_deliverable(&self) -> bool {
        matches!(&*self.redirect(), Redirect::Deliverable(_))
    }

    /// Shorten how long a redirection may wait to be written, so a test can
    /// reach the withdrawal without waiting out the real deadline.
    #[cfg(all(test, unix))]
    fn set_delivery_deadline(&self, deadline: std::time::Duration) {
        *self
            .delivery_deadline
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = deadline;
    }

    /// The abort landed, so a committed redirection is now the next turn.
    fn release_redirect(&self) {
        let mut slot = self.redirect();
        if let Redirect::Committed(input) = std::mem::replace(&mut *slot, Redirect::None) {
            *slot = Redirect::Deliverable(input);
        }
    }

    /// Whether a redirection is committed and not yet delivered.
    #[must_use]
    pub fn has_pending_redirect(&self) -> bool {
        match self.mechanism().as_ref() {
            Some(Backend::Dialogue(dialogue)) => dialogue.has_pending_redirect(),
            Some(Backend::Stdin | Backend::Http(..)) => {
                !matches!(&*self.redirect(), Redirect::None)
            }
            // Nothing is serving, so nothing is owed: `release` reports and
            // drops anything an ended turn was still holding.
            None => false,
        }
    }

    /// Deliver a committed redirection as the next turn's prompt frame, for a
    /// mechanism whose turn rides the harness's own stdin.
    ///
    /// `true` when one was written, which is the run's signal to keep the turn
    /// (and therefore stdin) open for the answer. A write that fails is reported
    /// and the turn ends, because the message has nowhere left to go and a run
    /// that waited on it would hang until its timeout.
    ///
    /// `shape` is passed in rather than read back off the handle: the caller is
    /// already holding the `mechanism` lock it would have to take.
    ///
    /// A redirection whose abort had not landed when this turn ended belonged
    /// to a turn that finished on its own: it is refused as undelivered, never
    /// carried into a turn of its own.
    fn open_redirected_turn(&self, shape: ControlShape) -> bool {
        let Some(input) = self.take_redirect() else {
            self.redirect_turn_ended();
            return false;
        };
        let Some(frame) = prompt_frame(shape, input.as_str()) else {
            self.redirect_refused("this mechanism has no frame to open a turn with");
            return false;
        };
        match self.write_line(&frame) {
            // Written as the replacement turn's prompt: delivered, and never
            // handed back, whatever the harness does next.
            Ok(()) => {
                trace_redirect_written();
                self.redirect_delivered();
                true
            }
            Err(err) => {
                eprintln!(
                    "oneharness: warning: the turn was interrupted but its redirection could not \
                     be delivered: {err}"
                );
                self.redirect_refused(&err.to_string());
                false
            }
        }
    }

    /// The mechanism the run's `control` block names: the one the serving
    /// candidate bound, or — if the chain never got as far as opening a turn —
    /// the one it would have started on.
    #[must_use]
    pub fn shape(&self) -> ControlShape {
        *self
            .reported
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Park the child's stdin and mark a turn in flight.
    pub fn begin_turn(&self, stdin: std::process::ChildStdin) {
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = TurnState::Active(Box::new(stdin));
    }

    /// Mark a turn in flight on a shared app-server connection: `sink` sends
    /// each written line as one message, and `board` is where the turn's own
    /// reader hands over the answers to requests this handle sends.
    pub(crate) fn begin_pooled_turn(
        &self,
        sink: Box<dyn io::Write + Send>,
        board: std::sync::Arc<ReplyBoard>,
        server: AppServerReport,
        command: Vec<String>,
    ) {
        *self.pooled() = PooledState::Live { board, command };
        *self
            .app_server
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(server);
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = TurnState::Active(sink);
    }

    /// The shared app-server went away under the live turn.
    pub(crate) fn mark_app_server_lost(&self) {
        let mut pooled = self.pooled();
        let command = match std::mem::take(&mut *pooled) {
            PooledState::Live { command, .. } | PooledState::Ended { command, .. } => command,
            PooledState::None => return,
        };
        *pooled = PooledState::Ended {
            command,
            end: PooledEnd::ServerLost,
        };
    }

    /// Whether the serving candidate's shared app-server went away under its
    /// live turn.
    #[must_use]
    pub(crate) fn app_server_lost(&self) -> bool {
        matches!(
            *self.pooled(),
            PooledState::Ended {
                end: PooledEnd::ServerLost,
                ..
            }
        )
    }

    /// The serving candidate's shared app-server launch argv, when its turn
    /// ran on one.
    #[must_use]
    pub(crate) fn app_server_command(&self) -> Option<Vec<String>> {
        match &*self.pooled() {
            PooledState::Live { command, .. } | PooledState::Ended { command, .. } => {
                Some(command.clone())
            }
            PooledState::None => None,
        }
    }

    /// The last shared app-server a candidate of this run ran on.
    #[must_use]
    pub(crate) fn app_server(&self) -> Option<AppServerReport> {
        self.app_server
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The thread and turn the dialogue would address an interrupt to now.
    #[must_use]
    pub(crate) fn active_turn(&self) -> Option<(String, String)> {
        match self.mechanism().as_ref() {
            Some(Backend::Dialogue(dialogue)) => dialogue
                .active_turn()
                .map(|(thread, turn)| (thread.to_string(), turn.to_string())),
            _ => None,
        }
    }

    /// Whether the dialogue's prompt is out and its turn has not ended.
    #[must_use]
    pub(crate) fn in_turn(&self) -> bool {
        matches!(self.mechanism().as_ref(), Some(Backend::Dialogue(dialogue)) if dialogue.in_turn())
    }

    fn pooled(&self) -> std::sync::MutexGuard<'_, PooledState> {
        self.pooled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// End the turn and close the child's stdin (dropping it signals EOF, which
    /// is how a control-enabled child — whose stdin was deliberately held open
    /// — learns it may exit).
    pub fn end_turn(&self) {
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = TurnState::Idle;
        let mut pooled = self.pooled();
        if let PooledState::Live { command, .. } = std::mem::take(&mut *pooled) {
            *pooled = PooledState::Ended {
                command,
                end: PooledEnd::Finished,
            };
        }
    }

    /// Write one raw line into the live turn's stdin, e.g. the prompt frame.
    /// Errors when no turn is active or the child closed its end.
    pub fn write_line(&self, line: &str) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &mut *state {
            TurnState::Active(sink) => {
                sink.write_all(line.as_bytes())?;
                sink.flush()
            }
            TurnState::Idle => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "no active turn to write to",
            )),
        }
    }

    /// Serve one request, recording it for the report either way.
    pub fn serve(&self, request: &ControlRequest) -> ControlResponse {
        let response = match request.verb() {
            ControlVerb::Interrupt => self.interrupt(request.input()),
        };
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(response.record(request.verb(), now()));
        response
    }

    /// Abort the in-flight turn, committing `redirect` to the next one in the
    /// same operation.
    ///
    /// The message is taken over *before* the abort is delivered and given back
    /// on every failure path, so the two outcomes a supervisor can read are
    /// "stopped, and your message is the run's to deliver" and "nothing
    /// happened, and your message is still yours". There is no third state where
    /// the turn is dead and the redirection was dropped.
    ///
    /// Which mechanism that is, is read from the binding at the moment the
    /// request arrives — so on a fallback chain the abort reaches whichever
    /// candidate is serving, and in the window where none is (before the first
    /// turn, across a fall-through, after the last) it is `no_active_turn`. The
    /// binding is taken under the same lock a candidate binds and releases it
    /// under, so there is no instant at which an interrupt could reach a
    /// mechanism that is not the serving one.
    fn interrupt(&self, redirect: Option<&RedirectInput>) -> ControlResponse {
        /// How the live backend delivers an abort, decided under the mechanism
        /// lock and carried out after it is released: an HTTP abort is a
        /// network request and a frame is a write to the child, and neither
        /// should be held against the thread advancing the turn.
        enum Delivery {
            /// One more request against the same session — there is no stdin in
            /// this path at all; the server is not even this process's child.
            Request(HttpTurn),
            /// Frames onto the live turn's stdin.
            Frames(Vec<String>),
        }
        let no_active_turn = || {
            ControlResponse::refused(
                "the run is alive but no turn is in flight",
                ControlReason::NoActiveTurn,
            )
        };
        /// A stdin-borne shape this build has no interrupt frame for. Named
        /// rather than inlined because the arm it answers sits four levels into
        /// the mechanism match, where the message would be unreadable.
        fn not_wired_for_stdin(shape: ControlShape) -> ControlResponse {
            ControlResponse::refused(
                format!(
                    "the `{}` control mechanism is not wired for stdin-borne interrupts",
                    shape.as_str()
                ),
                ControlReason::Unsupported,
            )
        }
        let commit = |input: Option<&RedirectInput>| match input {
            Some(input) => Redirect::Committed(input.clone()),
            None => Redirect::None,
        };
        // Tracked from the moment the message is committed, so whichever path
        // ends up writing it — or ending the turn without it — has something to
        // answer this request through.
        let ticket: Option<Arc<RedirectDelivery>>;
        // The serving candidate's mechanism, read under the lock it is bound
        // and released under. Nothing bound is a run between turns — before the
        // first candidate opens one, across a chain's fall-through, or after the
        // last one ended — which is exactly `no_active_turn`.
        let (shape, delivery) = {
            let mut backend = self.mechanism();
            let Some(backend) = backend.as_mut() else {
                return no_active_turn();
            };
            let shape = backend.shape();
            let delivery = match backend {
                Backend::Http(_, turn) => match turn.clone() {
                    Some(turn) => {
                        // Committed before the request goes out: the run's own
                        // driver reads it when the aborted turn ends, and the
                        // server holds the session either way, so it cannot be
                        // lost in the gap the abort opens.
                        ticket = redirect.map(|_| self.open_delivery());
                        *self.redirect() = commit(redirect);
                        Delivery::Request(turn)
                    }
                    None => return no_active_turn(),
                },
                // A server-backed mechanism addresses the live thread/session by
                // the ids the dialogue captured, so it alone knows whether there
                // is a turn to abort at all — and it takes the redirection over
                // itself, because opening the replacement turn is a protocol
                // decision.
                // Tracked under the same lock the dialogue took the message
                // under, so the line that ends the turn cannot reach it first.
                Backend::Dialogue(dialogue) => match dialogue.interrupt(redirect) {
                    Some(frames) => {
                        ticket = redirect.map(|_| self.open_delivery());
                        Delivery::Frames(frames)
                    }
                    None => return no_active_turn(),
                },
                Backend::Stdin => match interrupt_frame(shape, &self.next_request_id()) {
                    Some(frame) => {
                        ticket = redirect.map(|_| self.open_delivery());
                        *self.redirect() = commit(redirect);
                        Delivery::Frames(vec![frame])
                    }
                    None => return not_wired_for_stdin(shape),
                },
            };
            (shape, delivery)
        };
        // The answer for a delivered abort, naming the mechanism that actually
        // took it — the serving candidate's, never the chain's first. With a
        // redirection it is given only once that message is written as the
        // next turn's prompt, or known never to be.
        let served = || match &ticket {
            Some(ticket) => self.delivery_answer(shape, ticket),
            None => ControlResponse::served(shape),
        };
        // The answer for an abort that did not land. The message is taken back
        // unless something already claimed it — the turn can end on its own
        // while the abort is in flight — in which case what became of it is
        // the answer, so a message that was written is never called undelivered.
        let refused = |error: String, reason: ControlReason| match &ticket {
            Some(ticket) if !ticket.withdraw(reason, error.clone()) => {
                self.delivery_answer(shape, ticket)
            }
            _ => {
                self.abandon_redirect();
                ControlResponse::refused(error, reason)
            }
        };
        let frames = match delivery {
            Delivery::Request(turn) => {
                return match turn.interrupt() {
                    Ok(()) => {
                        // The abort landed, so the message may now be handed on.
                        // For a server whose aborted turn ends silently this is
                        // the ONLY signal the run will get that it is over.
                        self.release_redirect();
                        served()
                    }
                    Err(err) => refused(
                        format!("could not deliver the interrupt to the control server: {err}"),
                        ControlReason::NotRunning,
                    ),
                };
            }
            Delivery::Frames(frames) => frames,
        };
        // On a shared app-server the answer is awaited, bounded: the server
        // answers an interrupt for a turn that already ended with nothing at
        // all, and its answer is what says whether the thread still exists.
        let board = match &*self.pooled() {
            PooledState::Live { board, .. } => Some(std::sync::Arc::clone(board)),
            PooledState::None | PooledState::Ended { .. } => None,
        };
        let awaited: Option<u64> = board.as_ref().and_then(|board| {
            let id = frames
                .first()
                .and_then(|frame| serde_json::from_str::<serde_json::Value>(frame.trim()).ok())
                .and_then(|value| value.get("id").and_then(serde_json::Value::as_u64))?;
            board.expect(id);
            Some(id)
        });
        for frame in &frames {
            match self.write_line(frame) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotConnected => {
                    return refused(
                        "the run is alive but no turn is in flight".to_string(),
                        ControlReason::NoActiveTurn,
                    );
                }
                Err(err) => {
                    return refused(
                        format!("could not deliver the interrupt to the harness: {err}"),
                        ControlReason::NotRunning,
                    );
                }
            }
        }
        if let (Some(board), Some(id)) = (board, awaited) {
            let deadline = crate::io::server_pool::INTERRUPT_REPLY_DEADLINE;
            match board.wait(id, deadline).as_ref().map(InterruptReply::of) {
                Some(InterruptReply::Accepted) => {}
                Some(InterruptReply::ThreadGone(message) | InterruptReply::Refused(message)) => {
                    return refused(
                        format!("the shared codex app-server refused the interrupt: {message}"),
                        ControlReason::NotRunning,
                    );
                }
                None => {
                    return refused(
                        format!(
                            "the shared codex app-server did not answer the interrupt within {}s",
                            deadline.as_secs()
                        ),
                        ControlReason::NotRunning,
                    );
                }
            }
        }
        // Every abort frame is on the child's stdin, so the message may be
        // handed on once the harness says the turn it aborted has ended.
        self.release_redirect();
        served()
    }

    /// The answer to a redirecting interrupt whose abort landed: given once the
    /// message is the next turn's prompt, or known never to be. A message still
    /// unwritten at the deadline is taken back from wherever it is held, so
    /// nothing writes it after the supervisor was told it was not delivered.
    fn delivery_answer(&self, shape: ControlShape, ticket: &RedirectDelivery) -> ControlResponse {
        let deadline = *self
            .delivery_deadline
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match ticket.outcome(deadline) {
            DeliveryState::Delivered => ControlResponse::redirected(shape),
            DeliveryState::Undelivered(reason, why) => {
                self.abandon_redirect();
                ControlResponse::refused(why, reason)
            }
            // `outcome` returns only a settled state.
            DeliveryState::Pending | DeliveryState::Writing => ControlResponse::refused(
                "the redirection's delivery was never settled; the redirection was not delivered",
                ControlReason::NotRunning,
            ),
        }
    }

    /// Give back a redirection whose abort could not be delivered, wherever it
    /// was committed. Both stores are cleared because only one of them ever
    /// holds anything: the backend that took it is the one that has it.
    fn abandon_redirect(&self) {
        *self.redirect() = Redirect::None;
        if let Some(Backend::Dialogue(dialogue)) = self.mechanism().as_mut() {
            dialogue.abandon_redirect();
        }
    }

    fn next_request_id(&self) -> String {
        let mut counter = self
            .requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *counter += 1;
        format!("oh-control-{counter}")
    }

    /// Every request served so far, for the run report's `control` block.
    #[must_use]
    pub fn events(&self) -> Vec<ControlEvent> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// A bound control socket, serving requests on a background thread until it is
/// dropped.
///
/// Dropping it removes the socket file — the run's contract that the address
/// disappears when the run does. A dispatch killed with `SIGKILL` cannot run
/// `Drop`, so a stale socket can survive; a client then gets `ECONNREFUSED` and
/// reports `not_running`, which is the same answer as a missing file.
pub struct ControlListener {
    path: PathBuf,
    handle: Arc<ControlHandle>,
    #[cfg(unix)]
    shutdown: Option<std::sync::mpsc::Sender<()>>,
    #[cfg(unix)]
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ControlListener {
    /// The socket this run is listening on.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The mechanism handle the run writes prompts and turn boundaries through.
    #[must_use]
    pub fn handle(&self) -> Arc<ControlHandle> {
        Arc::clone(&self.handle)
    }

    /// The same handle borrowed, for a caller that outlives the listener.
    #[must_use]
    pub fn handle_ref(&self) -> &ControlHandle {
        &self.handle
    }

    /// Stop serving and return every request this run served, in order.
    ///
    /// The report's `control.interrupts` is read off this, never off the live
    /// [`ControlHandle::events`]: a request is recorded once its delivery has
    /// returned, and the turn an abort ended can finish — and the run assemble
    /// its report — in the window between the frame landing on the harness and
    /// that record. Joining the accept loop first completes the request it was
    /// in the middle of (an in-flight connection is served before the loop
    /// notices it was asked to stop), so an interrupt the supervisor was told
    /// was `served` is never missing from the report that says so. A request
    /// arriving after this is refused as `not_running`, which by then it is.
    /// The socket file stays until the listener is dropped.
    ///
    /// A redirecting interrupt still waiting for its message to be written is
    /// answered `not_running` first: the run is over, so nothing will write it,
    /// and the accept loop being joined is the thread that is waiting.
    pub fn finish(&mut self) -> Vec<ControlEvent> {
        self.handle.settle_delivery(run_ended_undelivered());
        imp::stop_serving(self);
        self.handle.events()
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};

    /// Bind the control socket at `path` (creating its directory) and start
    /// serving. A pre-existing file is removed first only when nothing is
    /// listening on it, so two concurrent runs of the same session name cannot
    /// silently steal each other's address.
    pub fn bind(path: &Path, starts_on: ControlShape) -> io::Result<ControlListener> {
        // Canonicalize the directory before binding: the socket path is an
        // address handed to a *different* process, and a relative one resolves
        // differently depending on where that process runs.
        let path = &match path.parent() {
            Some(parent) => {
                std::fs::create_dir_all(parent)?;
                std::fs::canonicalize(parent)?.join(path.file_name().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "the control socket path names no file",
                    )
                })?)
            }
            None => path.to_path_buf(),
        };
        // Canonicalizing can *lengthen* the address the caller checked (a
        // symlinked store, macOS's `/tmp` -> `/private/tmp`), so the budget is
        // re-checked on what will actually be bound. Said in the same words as
        // the pre-spawn refusal rather than as the kernel's bare
        // `ENAMETOOLONG`, which names no limit and suggests no remedy.
        crate::domain::control::socket_address_within_limit(path).map_err(|too_long| {
            // The caller's diagnostic already names the path, so this states only
            // what resolving it changed: the budget, and the length that broke it.
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "resolving it produced an address of {} bytes, past this platform's {}-byte \
                     unix-socket address limit (`sun_path`, terminating NUL included)",
                    too_long.bytes(),
                    too_long.limit()
                ),
            )
        })?;
        if let Some(parent) = path.parent() {
            // The directory holds addressable levers over running agents, so
            // owner-only is a requirement, not a preference: failing to narrow it
            // must abort the run rather than leave a wider default in place.
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        if path.exists() {
            if UnixStream::connect(path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another run is already listening on this session's control socket",
                ));
            }
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        // 0600: the socket is a lever over a running agent, so only its owner
        // may address it.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;

        let handle = Arc::new(ControlHandle::new(starts_on));
        let worker_handle = Arc::clone(&handle);
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let worker = std::thread::spawn(move || serve_loop(listener, worker_handle, &rx));

        Ok(ControlListener {
            path: path.to_path_buf(),
            handle,
            shutdown: Some(tx),
            worker: Some(worker),
        })
    }

    /// Accept connections until asked to stop. Non-blocking accept plus a short
    /// sleep keeps the thread responsive to shutdown without a second socket.
    ///
    /// Asked to stop, it first answers every connection already waiting: a
    /// client whose `connect` succeeded was accepted by the kernel into the
    /// backlog, and the loop learns of the stop while it is *between* accepts —
    /// returning there would hang up on a request the run's own address took,
    /// and the run reports what it served ([`ControlListener::finish`]) from
    /// exactly this point.
    fn serve_loop(
        listener: UnixListener,
        handle: Arc<ControlHandle>,
        shutdown: &std::sync::mpsc::Receiver<()>,
    ) {
        loop {
            match listener.accept() {
                Ok((stream, _)) => serve_connection(stream, &handle),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    match shutdown.recv_timeout(std::time::Duration::from_millis(25)) {
                        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            while let Ok((stream, _)) = listener.accept() {
                                serve_connection(stream, &handle);
                            }
                            return;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
                // A failed accept is one client's problem, never the run's:
                // control is a side channel and must not take results down.
                Err(_) => return,
            }
        }
    }

    /// One request, one response, connection closed — the whole protocol.
    ///
    /// `pub(super)` so the tests can drive it over a connection they put in the
    /// state a BSD accept leaves one in (see the blocking reset below), which is
    /// otherwise unreachable from Linux.
    pub(super) fn serve_connection(stream: UnixStream, handle: &ControlHandle) {
        use std::io::{BufRead, BufReader, Read, Write};
        // Explicitly blocking, because the accept that produced this stream is
        // not portable about it: the listener is non-blocking (so the accept
        // loop can notice a shutdown), and a BSD accept — macOS's — hands back a
        // socket that INHERITED that flag, while Linux's does not. Left
        // inherited, a read that has to wait for the rest of a frame answers
        // `WouldBlock` instead of waiting, which the reader below reports as a
        // run that could not be read from — an interrupt silently lost on one
        // platform. The timeouts are what bound the wait; this is what makes
        // there be one.
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(CLIENT_TIMEOUT));
        let _ = stream.set_write_timeout(Some(CLIENT_TIMEOUT));
        // Bounded before it is accumulated: the newline that ends a request is
        // the peer's to send, so an unbounded read is the peer deciding how
        // much of the run's memory this connection costs. One byte past the
        // bound is enough to know the frame is not one this version parses.
        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(clone) => clone,
            Err(_) => return,
        })
        .take(MAX_FRAME_BYTES + 1);
        let mut line = String::new();
        let response = match reader.read_line(&mut line) {
            Ok(read) if read as u64 > MAX_FRAME_BYTES => ControlResponse::refused(
                format!("the control request is longer than {MAX_FRAME_BYTES} bytes"),
                ControlReason::Unsupported,
            ),
            Ok(_) => match crate::domain::control::parse_request(&line) {
                Ok(request) => handle.serve(&request),
                // A frame this version cannot parse is refused loudly rather
                // than being mistaken for an interrupt.
                Err(err) => ControlResponse::refused(err, ControlReason::Unsupported),
            },
            Err(err) => ControlResponse::refused(
                format!("could not read the control request: {err}"),
                ControlReason::NotRunning,
            ),
        };
        let mut stream = stream;
        let _ = stream.write_all(response.to_line().as_bytes());
        let _ = stream.flush();
    }

    /// Send one request to a run listening at `path` and read its answer.
    /// A missing socket, or one nothing is listening on, is `not_running` —
    /// exactly what a supervisor needs to distinguish from a refusal.
    pub fn send(path: &Path, request: &ControlRequest) -> ControlResponse {
        use std::io::{BufRead, BufReader, Read, Write};
        let mut stream = match UnixStream::connect(path) {
            Ok(stream) => stream,
            Err(err) => {
                return ControlResponse::refused(
                    format!(
                        "no run is listening on `{}` ({err}). Suggestion: start the run with `--control --session <NAME>`",
                        path.display()
                    ),
                    ControlReason::NotRunning,
                )
            }
        };
        // A redirect is answered on delivery, and once its replacement turn is
        // written only the harness's answer settles it — however long that
        // takes. A client that gave up first would report a failure for a
        // message that may still land, so it waits for the run's definitive
        // answer: the run bounds a message nothing has claimed with its own
        // deadline, and a run that dies closes the socket, which ends the read.
        let read_timeout = match request.input() {
            Some(_) => None,
            None => Some(CLIENT_TIMEOUT),
        };
        let _ = stream.set_read_timeout(read_timeout);
        let _ = stream.set_write_timeout(Some(CLIENT_TIMEOUT));
        let mut line = match serde_json::to_string(request) {
            Ok(line) => line,
            Err(err) => {
                return ControlResponse::refused(
                    format!("could not encode the control request: {err}"),
                    ControlReason::NotRunning,
                )
            }
        };
        line.push('\n');
        if let Err(err) = stream
            .write_all(line.as_bytes())
            .and_then(|()| stream.flush())
        {
            return ControlResponse::refused(
                format!("could not send the control request: {err}"),
                ControlReason::NotRunning,
            );
        }
        let mut reply = String::new();
        // Bounded exactly like the request the run reads: the answer arrives
        // from whatever is listening on that path, and the newline ending it is
        // that peer's to withhold.
        match BufReader::new(&stream)
            .take(MAX_FRAME_BYTES + 1)
            .read_line(&mut reply)
        {
            Ok(0) => ControlResponse::refused(
                "the run closed the control connection without answering",
                ControlReason::NotRunning,
            ),
            Ok(read) if read as u64 > MAX_FRAME_BYTES => ControlResponse::refused(
                format!("the control answer is longer than {MAX_FRAME_BYTES} bytes"),
                ControlReason::NotRunning,
            ),
            Ok(_) => serde_json::from_str(reply.trim()).unwrap_or_else(|err| {
                ControlResponse::refused(
                    format!("the run sent an unreadable control response: {err}"),
                    ControlReason::NotRunning,
                )
            }),
            Err(err) => ControlResponse::refused(
                format!("could not read the control response: {err}"),
                ControlReason::NotRunning,
            ),
        }
    }

    /// Ask the accept loop to stop and wait for it: idempotent, and by the
    /// time it returns every connection accepted so far has been answered.
    pub fn stop_serving(listener: &mut ControlListener) {
        if let Some(tx) = listener.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(worker) = listener.worker.take() {
            let _ = worker.join();
        }
    }

    pub fn shutdown(listener: &mut ControlListener) {
        stop_serving(listener);
        let _ = std::fs::remove_file(&listener.path);
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;

    pub fn bind(_path: &Path, _starts_on: ControlShape) -> io::Result<ControlListener> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unix domain sockets are unavailable on this platform",
        ))
    }

    pub fn send(path: &Path, request: &ControlRequest) -> ControlResponse {
        // No run here can bind a control socket, so a redirection reaches no
        // turn: say so, so the caller keeps ownership of its message.
        let undelivered = if request.input().is_some() {
            "; the redirection was not delivered"
        } else {
            ""
        };
        ControlResponse::refused(
            format!(
                "control sockets are unavailable on this platform (`{}`){undelivered}",
                path.display()
            ),
            ControlReason::NotRunning,
        )
    }

    pub fn stop_serving(_listener: &mut ControlListener) {}

    pub fn shutdown(_listener: &mut ControlListener) {}
}

/// Bind the run's control socket at `path`.
///
/// The address is the run's for its whole lifetime and says nothing about which
/// harness is behind it: the mechanism is bound per candidate, as each takes
/// the turn (the crate-private `ControlHandle::bind`). `starts_on` is only what
/// the report names until the first candidate binds its own — an address a
/// supervisor can already reach, on a run that has not opened a turn yet, has
/// no mechanism to name.
pub fn bind(path: &Path, starts_on: ControlShape) -> io::Result<ControlListener> {
    imp::bind(path, starts_on)
}

/// Send one control request to the run listening at `path`.
#[must_use]
pub fn send(path: &Path, request: &ControlRequest) -> ControlResponse {
    imp::send(path, request)
}

/// Whether this platform can open a control socket at all.
#[must_use]
pub fn supported() -> bool {
    cfg!(unix)
}

impl Drop for ControlListener {
    fn drop(&mut self) {
        imp::shutdown(self);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::domain::control::socket_path;
    use crate::io::scratch::ScratchDir;

    /// A private, self-removing directory for one test's socket.
    ///
    /// Rooted at `/tmp` rather than `std::env::temp_dir()` because this path
    /// becomes a socket *address*, not just a place to put files:
    /// `sockaddr_un.sun_path` holds 104 bytes on macOS against Linux's 108, and
    /// macOS's per-user `$TMPDIR` is a `/var/folders/…` path that [`bind`]
    /// canonicalizes to `/private/var/folders/…` before binding — spending over
    /// half the budget on the root alone, so the address overruns `sun_path`
    /// there while fitting comfortably on Linux.
    /// Resolved, because a bound socket path IS resolved: [`bind`] canonicalizes
    /// the directory before binding (the address is handed to another process),
    /// so on macOS — where `/tmp` is a symlink to `/private/tmp` — a listener
    /// asked for `/tmp/…` reports `/private/tmp/…`. Rooting the tests at the
    /// unresolved path still binds and still interrupts, but no longer matches
    /// the address the listener names.
    fn temp_dir(tag: &str) -> ScratchDir {
        let root = std::fs::canonicalize("/tmp").expect("/tmp must exist to root a control socket");
        ScratchDir::under(
            &root,
            &format!("control-{tag}-{:?}", std::thread::current().id()),
        )
        .unwrap()
    }

    /// Send a redirect from another thread, as a separate `oneharness
    /// interrupt` process would: its answer is given on delivery, so the test
    /// must be free to drive the turn while the request is waiting.
    fn send_in_background(path: &Path, text: &str) -> std::thread::JoinHandle<ControlResponse> {
        let path = path.to_path_buf();
        let request = ControlRequest::redirect(
            crate::domain::control::RedirectInput::new(text).expect("a fixture message"),
        );
        std::thread::spawn(move || send(&path, &request))
    }

    /// Wait (bounded) until `ready` holds.
    fn wait_for(what: &str, ready: impl Fn() -> bool) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !ready() {
            assert!(
                std::time::Instant::now() < until,
                "timed out waiting for {what}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn socket_is_created_owner_only_and_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("lifecycle");
        let path = socket_path(&dir, "flow")
            .expect("a short fixture name fits every platform's socket-address budget");
        {
            let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
            assert!(path.exists(), "socket should exist while the run is alive");
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "socket must be owner-only");
            assert_eq!(listener.path(), path);
        }
        assert!(!path.exists(), "socket must be removed when the run exits");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A graph-minted worker session name, the shape whose address overran
    /// `sun_path` on a real host and left every dispatch uninterruptible.
    const GRAPH_SESSION: &str = "node-scope-1786808430370-3422037-worker-skill";

    #[test]
    fn a_graph_minted_session_name_binds_under_a_default_length_store() {
        // The real default store root — `$HOME/.local/state/oneharness/sessions`
        // — is where this failed, so it is what the address is measured against.
        // Measured, not bound: a test must not open sockets in the developer's
        // own state directory. The bind below is done at a store DEEPER than any
        // default root, so it proves the harder case.
        let default_root =
            crate::io::session::resolve_dir(None).expect("a platform state dir to root the store");
        socket_path(&default_root, GRAPH_SESSION)
            .expect("the default store root must leave room for a session name");

        // The tightest store that can still be addressed at all: one whose own
        // path spends every byte the shortest possible socket file name does not
        // need. Binding here proves the abbreviation is not merely short enough
        // on paper but is the address a listener actually takes.
        let root = std::fs::canonicalize("/tmp").expect("/tmp must exist to root a control socket");
        let unique = format!("oh-tight-{}", std::process::id());
        let used = root.join(&unique).join("control").as_os_str().len() + 1;
        let slack = crate::domain::control::SOCKET_ADDRESS_MAX
            - 1
            - used
            - (crate::domain::control::MIN_FILE_NAME);
        let dir = root.join(format!("{unique}{}", "p".repeat(slack)));
        std::fs::create_dir_all(&dir).unwrap();

        let path =
            socket_path(&dir, GRAPH_SESSION).expect("the tightest store is still addressable");
        assert_eq!(
            path.as_os_str().len() + 1,
            crate::domain::control::SOCKET_ADDRESS_MAX,
            "the fixture must sit exactly at the budget, or it proves a looser case"
        );
        let _listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        assert!(path.exists(), "the abbreviated address must be bindable");
        // What a separate `oneharness interrupt` process does: derive the same
        // address from the same `(dir, name)` and reach the run on it.
        let dialed = socket_path(&dir, GRAPH_SESSION).expect("addressable from the other side too");
        let response = send(&dialed, &ControlRequest::interrupt());
        assert_eq!(
            response.reason(),
            Some(ControlReason::NoActiveTurn),
            "the client reached the listener (an unbound run answers `not_running`)"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn interrupt_without_an_active_turn_is_refused_with_no_active_turn() {
        let dir = temp_dir("noturn");
        let path = socket_path(&dir, "idle")
            .expect("a short fixture name fits every platform's socket-address budget");
        let _listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let response = send(&path, &ControlRequest::interrupt());
        assert!(!response.is_ok());
        assert_eq!(response.reason(), Some(ControlReason::NoActiveTurn));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn interrupt_against_a_missing_socket_is_not_running() {
        let dir = temp_dir("missing");
        let path = socket_path(&dir, "gone")
            .expect("a short fixture name fits every platform's socket-address budget");
        let response = send(&path, &ControlRequest::interrupt());
        assert!(!response.is_ok());
        assert_eq!(response.reason(), Some(ControlReason::NotRunning));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_stale_socket_file_is_reclaimed_by_the_next_run() {
        let dir = temp_dir("stale");
        let path = socket_path(&dir, "stale")
            .expect("a short fixture name fits every platform's socket-address budget");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"").unwrap();
        let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        assert_eq!(listener.path(), path);
        // And a live listener refuses to be displaced.
        let conflict = bind(&path, ControlShape::ClaudeControlRequest);
        assert!(conflict.is_err(), "a live socket must not be stolen");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_served_interrupt_is_recorded_and_reaches_the_child_stdin() {
        use std::io::Read;
        let dir = temp_dir("served");
        let path = socket_path(&dir, "live")
            .expect("a short fixture name fits every platform's socket-address budget");
        let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let handle = listener.handle();

        // A real child process standing in for the harness: it echoes whatever
        // reaches its stdin, so the assertion is on delivered bytes, not on an
        // internal flag.
        let mut child = std::process::Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // Bind the mechanism the candidate serving this turn speaks, then park
        // its stdin: the channel is unbound until a candidate takes the turn.
        handle.bind(Binding::Stdin);
        handle.begin_turn(child.stdin.take().unwrap());

        let response = send(&path, &ControlRequest::interrupt());
        assert!(response.is_ok(), "{response:?}");
        assert_eq!(
            response.mechanism(),
            Some(ControlShape::ClaudeControlRequest)
        );

        handle.end_turn();
        let mut delivered = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut delivered)
            .unwrap();
        child.wait().ok();
        let frame: serde_json::Value = serde_json::from_str(delivered.trim()).unwrap();
        assert_eq!(frame["type"], "control_request");
        assert_eq!(frame["request"]["subtype"], "interrupt");

        let events = handle.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].verb(), ControlVerb::Interrupt);
        assert!(events[0].is_served());
        assert!(!events[0].at().as_str().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_request_in_flight_when_the_run_reports_is_in_what_it_reports() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::os::unix::net::UnixStream;
        // The turn an interrupt aborts ends as soon as the abort lands, and the
        // run assembles its report then — while the request that ended it can
        // still be between delivering the frame and recording itself. Read off
        // the live handle, that report said `interrupts: []` about an interrupt
        // its own client was told was served. `finish` closes that window by
        // waiting for the request in flight, so what it returns is what the
        // socket answered, whichever thread got there first.
        let dir = temp_dir("in-flight");
        let path = socket_path(&dir, "live")
            .expect("a short fixture name fits every platform's socket-address budget");
        let mut listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let handle = listener.handle();
        let mut child = std::process::Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        handle.bind(Binding::Stdin);
        handle.begin_turn(child.stdin.take().unwrap());

        // A request the run has accepted but cannot answer yet: its second half
        // is deliberately late, so the run reports while it is in flight.
        let (connected, connected_rx) = std::sync::mpsc::channel();
        let client_path = path.clone();
        let client = std::thread::spawn(move || -> String {
            let mut stream = UnixStream::connect(&client_path).expect("the run is listening");
            let mut frame = serde_json::to_string(&ControlRequest::interrupt()).unwrap();
            frame.push('\n');
            let (head, tail) = frame.split_at(frame.len() / 2);
            stream.write_all(head.as_bytes()).unwrap();
            stream.flush().unwrap();
            connected.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(200));
            stream.write_all(tail.as_bytes()).unwrap();
            stream.flush().unwrap();
            let mut reply = String::new();
            // Zero bytes is the run hanging up on a request its address took.
            BufReader::new(&stream)
                .read_line(&mut reply)
                .expect("a request in flight when the run stopped serving was answered");
            reply
        });
        connected_rx
            .recv()
            .expect("the client connected before the run reported");

        // The run reporting: what it publishes must already hold the request
        // the socket is answering.
        let reported = listener.finish();
        let reply = client.join().unwrap();
        assert!(
            !reply.is_empty(),
            "the run hung up on a request in flight when it stopped serving"
        );
        let response: ControlResponse = serde_json::from_str(reply.trim()).unwrap();
        assert!(response.is_ok(), "{reply}");
        assert_eq!(reported.len(), 1, "{reported:?}");
        assert!(reported[0].is_served());
        assert_eq!(reported[0].verb(), ControlVerb::Interrupt);

        // Served means delivered: the frame is on the child's stdin.
        handle.end_turn();
        let mut delivered = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut delivered)
            .unwrap();
        child.wait().ok();
        let frame: serde_json::Value = serde_json::from_str(delivered.trim()).unwrap();
        assert_eq!(frame["request"]["subtype"], "interrupt");
        // A request after the run stopped serving is refused as not running,
        // never answered by a run that has already reported.
        let late = send(&path, &ControlRequest::interrupt());
        assert_eq!(late.reason(), Some(ControlReason::NotRunning), "{late:?}");
        drop(listener);
        assert!(!path.exists(), "dropping the listener removes the socket");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_request_waiting_in_the_backlog_when_the_run_stops_serving_is_answered() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::os::unix::net::UnixStream;
        // The other place a request the run's address took can be when the run
        // stops serving: not yet accepted at all. A client's `connect` succeeds
        // once the kernel has queued it in the backlog, before the accept loop
        // has seen it — and the loop learns of the stop *between* accepts, so
        // returning there would hang up on a request that was already the
        // run's to answer.
        let dir = temp_dir("backlog");
        let path = socket_path(&dir, "queued")
            .expect("a short fixture name fits every platform's socket-address budget");
        let mut listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let handle = listener.handle();
        let mut child = std::process::Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        handle.bind(Binding::Stdin);
        handle.begin_turn(child.stdin.take().unwrap());
        let mut frame = serde_json::to_string(&ControlRequest::interrupt()).unwrap();
        frame.push('\n');

        // The loop idles in 25 ms sleeps between accepts, at a phase this test
        // cannot see — unless it sets it. A first request held half-sent parks
        // the loop in that connection; its reply marks the instant the loop
        // finds nothing more to accept and begins a fresh sleep, so a request
        // queued a few milliseconds later, with the stop asked for at once, is
        // in the backlog when the loop wakes to the stop.
        let mut first = UnixStream::connect(&path).expect("the run is listening");
        let (head, tail) = frame.split_at(frame.len() / 2);
        first.write_all(head.as_bytes()).unwrap();
        first.flush().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        first.write_all(tail.as_bytes()).unwrap();
        first.flush().unwrap();
        let mut first_reply = String::new();
        BufReader::new(&first)
            .read_line(&mut first_reply)
            .expect("the first request was answered");
        std::thread::sleep(std::time::Duration::from_millis(5));

        let mut queued = UnixStream::connect(&path).expect("the run is listening");
        queued.write_all(frame.as_bytes()).unwrap();
        queued.flush().unwrap();
        // Stop serving with the whole request queued and unaccepted. What the
        // run reports must already hold it, and its client must be answered.
        let reported = listener.finish();

        let mut reply = String::new();
        BufReader::new(&queued)
            .read_line(&mut reply)
            .expect("a request queued when the run stopped serving was answered");
        assert!(
            !reply.is_empty(),
            "the run hung up on a request waiting in its backlog"
        );
        let response: ControlResponse = serde_json::from_str(reply.trim()).unwrap();
        assert!(response.is_ok(), "{reply}");
        assert_eq!(reported.len(), 2, "{reported:?}");
        assert!(reported.iter().all(ControlEvent::is_served), "{reported:?}");

        // Served means delivered: both frames are on the child's stdin.
        handle.end_turn();
        let mut delivered = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut delivered)
            .unwrap();
        child.wait().ok();
        let frames: Vec<serde_json::Value> = delivered
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(frames.len(), 2, "{delivered}");
        assert!(frames
            .iter()
            .all(|frame| frame["request"]["subtype"] == "interrupt"));
        drop(listener);
        assert!(!path.exists(), "dropping the listener removes the socket");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_interrupt_between_two_candidates_reaches_neither_of_them() {
        use std::io::Read;
        // The fall-through window, held open deliberately: candidate A's turn
        // has ended and candidate B has not opened one. It is the one state a
        // running chain passes through that nothing outside the process can time
        // reliably — precisely because nothing happens in it — so it is driven
        // here, against the same socket `oneharness interrupt` dials.
        //
        // The answer is `no_active_turn`: there IS no live turn, and the honest
        // refusal is the only one that cannot mislead. Queuing the abort for
        // whichever candidate binds next would land a supervisor's stop on a
        // turn they never saw start — the silent reshaping `--control` exists to
        // avoid — and reaching back to the candidate that just finished would
        // write at a mechanism nobody is on.
        let dir = temp_dir("fall-through");
        let path = socket_path(&dir, "chain")
            .expect("a short fixture name fits every platform's socket-address budget");
        let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let handle = listener.handle();

        let spawn_candidate = || {
            std::process::Command::new("cat")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        };

        // Candidate A serves its turn.
        let mut first = spawn_candidate();
        handle.bind(Binding::Stdin);
        handle.begin_turn(first.stdin.take().unwrap());
        // The abort is served, and names the mechanism that took it.
        let served = send(&path, &ControlRequest::interrupt());
        assert!(served.is_ok(), "{served:?}");
        let claude = Some(ControlShape::ClaudeControlRequest);
        assert_eq!(served.mechanism(), claude, "{served:?}");

        // A's turn ends and the chain falls through: nothing is bound.
        handle.release();
        // An interrupt arriving across a fall-through reaches nothing at all.
        let between = send(&path, &ControlRequest::interrupt());
        let no_turn = Some(ControlReason::NoActiveTurn);
        assert_eq!(between.reason(), no_turn, "{}", between.to_line());

        // The sharp half. Candidate B is spawned and its stdin is parked — the
        // driver does that for every controlled candidate — but B has not bound
        // its mechanism yet. A live pipe is NOT a live turn: an abort written
        // here would reach B's process addressed in the mechanism A was on, and
        // B never asked for it. So the answer is still `no_active_turn`, and B's
        // stdin stays untouched.
        let mut second = spawn_candidate();
        handle.begin_turn(second.stdin.take().unwrap());
        let parked = send(&path, &ControlRequest::interrupt());
        assert_eq!(parked.reason(), no_turn, "{}", parked.to_line());

        // Now B binds, on the same socket the supervisor has held throughout.
        // The address never changed; what is behind it did.
        handle.bind(Binding::Stdin);
        // A released channel re-arms for the next candidate.
        let again = send(&path, &ControlRequest::interrupt());
        assert!(again.is_ok(), "{again:?}");
        handle.release();

        // Delivered where it was aimed, and only there: A got the first abort, B
        // the second, and neither got the one sent in between.
        let delivered = |child: &mut std::process::Child| {
            let mut text = String::new();
            child
                .stdout
                .take()
                .unwrap()
                .read_to_string(&mut text)
                .unwrap();
            child.wait().ok();
            text.lines().filter(|line| !line.trim().is_empty()).count()
        };
        assert_eq!(delivered(&mut first), 1, "candidate A took only its own");
        assert_eq!(delivered(&mut second), 1, "candidate B took only its own");

        // Four requests reached the run; the report records the two refusals
        // beside the two that were served rather than hiding them.
        let events = handle.events();
        assert_eq!(events.len(), 4, "{events:?}");
        assert!(events[0].is_served());
        assert!(!events[1].is_served());
        assert!(!events[2].is_served());
        assert!(events[3].is_served());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unbound_channel_opens_nothing_and_drives_nothing() {
        // The state a controlled run is in before its first candidate takes the
        // turn, and again between a chain's candidates. Every accessor has to
        // answer for "nobody is serving" rather than for the last one that was —
        // a handle that still offered the previous candidate's frames, session
        // id or answer would attribute one candidate's turn to another.
        let dir = temp_dir("unbound");
        let path = socket_path(&dir, "idle")
            .expect("a short fixture name fits every platform's socket-address budget");
        let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let handle = listener.handle_ref();
        // There is no turn to open, and a line belongs to no turn — so the
        // caller is not held open waiting for one to end.
        assert!(handle.open_frames("keep working").is_empty());
        assert!(handle.advance(r#"{"type":"result","subtype":"success"}"#));
        assert!(handle.session_id().is_none());
        assert!(handle.text().is_none());
        assert!(!handle.drives_turn_over_stdin());
        assert!(!handle.has_pending_redirect());
        assert!(handle.take_redirect().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_redirection_its_own_turn_never_delivered_is_not_carried_to_the_next_candidate() {
        // A supervisor aimed this message at one candidate's turn. If that turn
        // ends without ever delivering it — the harness died, the chain fell
        // through — the message is reported and dropped, never handed to the
        // next candidate's fresh turn. Delivering it there would run work the
        // supervisor asked of a turn that no longer exists, on a harness they
        // never saw start.
        let dir = temp_dir("redirect-dropped");
        let path = socket_path(&dir, "live")
            .expect("a short fixture name fits every platform's socket-address budget");
        let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let handle = listener.handle();
        let mut child = std::process::Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        handle.bind(Binding::Stdin);
        handle.begin_turn(child.stdin.take().unwrap());

        let pending = send_in_background(&path, "do X instead");
        // The run owns the message from the interrupt onwards, and has not
        // answered: nothing has written it yet.
        wait_for("the redirection to be committed", || {
            handle.has_pending_redirect()
        });
        assert!(!pending.is_finished(), "answered before delivery");

        // The turn ends without the run ever writing it (no terminal document
        // reached `advance`), and the candidate is released.
        // A released channel owes the next candidate nothing.
        handle.release();
        assert!(!handle.has_pending_redirect());
        assert!(handle.take_redirect().is_none());
        // And the supervisor is told so: the message is still theirs.
        let response = pending.join().unwrap();
        assert_eq!(
            response.reason(),
            Some(ControlReason::NotRunning),
            "{response:?}"
        );
        assert!(!response.is_redirected(), "{response:?}");
        assert!(
            response
                .to_line()
                .contains("the redirection was not delivered"),
            "{response:?}"
        );
        let events = handle.events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].reason(), Some(ControlReason::NotRunning));

        // The next candidate binds and opens its turn with its OWN prompt only.
        handle.bind(Binding::Stdin);
        let frames = handle.open_frames("the next candidate's prompt");
        // The dropped redirection must not open another candidate's turn.
        assert_eq!(frames.len(), 1, "{frames:?}");
        assert!(!frames[0].contains("do X instead"), "{frames:?}");
        child.kill().ok();
        child.wait().ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_reported_mechanism_follows_the_candidate_that_bound_it() {
        use crate::domain::control::AbsolutePath;
        use crate::domain::dialogue::DialogueConfig;
        use crate::domain::mode::{ApprovalPosture, PermissionMode};
        // A chain's `control` block must name the mechanism that ended up
        // serving, not the one it was planned to start on — otherwise a
        // consumer reading the report concludes something different from the
        // supervisor who held the socket.
        let dir = temp_dir("reported");
        let path = socket_path(&dir, "moves")
            .expect("a short fixture name fits every platform's socket-address budget");
        let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let handle = listener.handle();
        // Before any candidate binds, the report names the one the chain would
        // start on.
        assert_eq!(handle.shape(), ControlShape::ClaudeControlRequest);

        let dialogue = Dialogue::new(
            ControlShape::CodexAppServer,
            DialogueConfig {
                prompt: "keep working".to_string(),
                cwd: AbsolutePath::new(dir.path()).unwrap(),
                model: None,
                mode: PermissionMode::Bypass,
                resume: None,
                posture: ApprovalPosture::Unattended,
                shell_environment_policy: None,
            },
        );
        assert!(dialogue.is_some(), "codex control is a dialogue shape");
        // The serving candidate's mechanism is the one reported.
        handle.bind(Binding::Dialogue(Box::new(dialogue.unwrap())));
        assert_eq!(handle.shape(), ControlShape::CodexAppServer);

        // And releasing does not rewrite history: the mechanism that served is
        // still what the finished run reports.
        handle.release();
        assert_eq!(handle.shape(), ControlShape::CodexAppServer);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_pooled_binding_reports_the_server_it_names_rather_than_the_chain_head() {
        // `Binding` makes the mismatch this used to have to assert against
        // unspellable: a conversation carries its own protocol and a pooled
        // binding its own server, so no shape can be handed over beside one that
        // contradicts it. What is still worth pinning is that the report follows
        // the binding — a channel still naming the chain's first mechanism would
        // hand a supervisor an address that answers for a different candidate.
        let dir = temp_dir("pooled-binding");
        let path = socket_path(&dir, "named")
            .expect("a short fixture name fits every platform's socket-address budget");
        let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let handle = listener.handle();
        assert_eq!(handle.shape(), ControlShape::ClaudeControlRequest);

        let http = HttpShape::of(ControlShape::OpencodeHttp).expect("opencode submits to a server");
        handle.bind(Binding::PooledServer(http));
        assert_eq!(handle.shape(), ControlShape::OpencodeHttp);
        // Not a stdin-driven turn, so nothing about the chain's head survived
        // the rebinding.
        assert!(!handle.drives_turn_over_stdin());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_redirection_reaches_the_child_only_once_the_aborted_turn_has_ended() {
        use std::io::Read;
        // The ordering IS the feature. A `user` frame written into a live turn
        // is silently dropped by Claude Code, so a redirect delivered alongside
        // the interrupt would be lost — while one delivered by the supervisor
        // after the turn ends needs a second round trip the turn might not
        // survive. So the run takes the message at interrupt time and writes it
        // itself, at the one moment the harness will accept it.
        let dir = temp_dir("redirect");
        let path = socket_path(&dir, "live")
            .expect("a short fixture name fits every platform's socket-address budget");
        let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let handle = listener.handle();

        // A real child again: the assertion is on bytes that reached a process,
        // in the order they reached it.
        let mut child = std::process::Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // Bind the mechanism the candidate serving this turn speaks, then park
        // its stdin: the channel is unbound until a candidate takes the turn.
        handle.bind(Binding::Stdin);
        handle.begin_turn(child.stdin.take().unwrap());

        let pending = send_in_background(&path, "do X instead");
        wait_for("the redirection to be committed", || {
            handle.has_pending_redirect()
        });
        // The abort is out, the message is the run's — and the supervisor has
        // not been answered, because nothing has written it yet.
        assert!(
            !pending.is_finished(),
            "a redirect must not be answered before it is delivered"
        );

        // The turn's own end is the moment the redirection becomes a turn — and
        // the run says so by keeping the turn open rather than closing stdin.
        assert!(
            !handle.advance(r#"{"type":"result","subtype":"error_during_execution"}"#),
            "the run must not end while it still owes a redirection"
        );
        assert!(!handle.has_pending_redirect(), "delivered exactly once");
        // Written: delivered, whatever the harness does next.
        let response = pending.join().unwrap();
        assert!(response.is_ok(), "{response:?}");
        assert!(
            response.is_redirected(),
            "the answer must say the message was delivered: {response:?}"
        );
        // The next end really is the end.
        assert!(handle.advance(r#"{"type":"result","subtype":"success"}"#));

        handle.end_turn();
        let mut delivered = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut delivered)
            .unwrap();
        child.wait().ok();
        let frames: Vec<serde_json::Value> = delivered
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("every frame is JSON"))
            .collect();
        assert_eq!(frames.len(), 2, "{delivered}");
        assert_eq!(frames[0]["type"], "control_request");
        assert_eq!(frames[0]["request"]["subtype"], "interrupt");
        assert_eq!(frames[1]["type"], "user");
        assert_eq!(frames[1]["message"]["content"][0]["text"], "do X instead");

        let events = handle.events();
        assert_eq!(events.len(), 1);
        assert!(events[0].is_served());
        assert!(
            events[0].is_redirected(),
            "the report must distinguish a redirect from a plain stop"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_redirection_nothing_writes_by_its_deadline_is_taken_back_and_never_written() {
        use std::io::Read;
        // A harness whose aborted turn never ends would otherwise hold the
        // supervisor forever. Past the deadline the run takes the message back
        // and says so — and from then on nothing writes it, however the turn
        // ends, because the supervisor now believes it is still theirs.
        let dir = temp_dir("redirect-deadline");
        let path = socket_path(&dir, "live")
            .expect("a short fixture name fits every platform's socket-address budget");
        let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let handle = listener.handle();
        handle.set_delivery_deadline(std::time::Duration::from_millis(200));
        let mut child = std::process::Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        handle.bind(Binding::Stdin);
        handle.begin_turn(child.stdin.take().unwrap());

        let response = send_in_background(&path, "do X instead").join().unwrap();
        assert_eq!(
            response.reason(),
            Some(ControlReason::NotRunning),
            "{response:?}"
        );
        let said = response.to_line();
        assert!(said.contains("the redirection was not delivered"), "{said}");
        assert!(said.contains("taken back"), "{said}");
        assert!(!handle.has_pending_redirect());

        // The aborted turn ends afterwards: nothing is written for it, and the
        // run does not hold the turn open for a message it no longer owns.
        assert!(handle.advance(r#"{"type":"result","subtype":"error_during_execution"}"#));
        handle.end_turn();
        let mut delivered = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut delivered)
            .unwrap();
        child.wait().ok();
        assert_eq!(delivered.lines().count(), 1, "{delivered}");
        assert!(delivered.contains("control_request"), "{delivered}");
        assert!(!delivered.contains("do X instead"), "{delivered}");
        let events = handle.events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].reason(), Some(ControlReason::NotRunning));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_turn_that_ends_before_its_abort_lands_refuses_the_redirection_it_was_given() {
        // The window the old answer lied about: the message is committed, the
        // abort is still being written, and the turn ends on its own. That turn
        // was never redirected, so its message is refused `no_active_turn` —
        // and not delivered into whatever comes next.
        struct Gated {
            release: std::sync::mpsc::Receiver<()>,
            written: Arc<Mutex<Vec<u8>>>,
        }
        impl io::Write for Gated {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                let _ = self.release.recv();
                self.written
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let dir = temp_dir("redirect-early-end");
        let path = socket_path(&dir, "live")
            .expect("a short fixture name fits every platform's socket-address budget");
        let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let handle = listener.handle();
        let (release, gate) = std::sync::mpsc::channel();
        let written = Arc::new(Mutex::new(Vec::new()));
        handle.bind(Binding::Stdin);
        *handle
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            TurnState::Active(Box::new(Gated {
                release: gate,
                written: Arc::clone(&written),
            }));

        let pending = send_in_background(&path, "do X instead");
        wait_for("the redirection to be committed", || {
            handle.has_pending_redirect()
        });
        // The turn's own end arrives while the abort is still in flight: there
        // is nothing to keep the turn open for.
        let ended = std::thread::scope(|scope| {
            let advancing =
                scope.spawn(|| handle.advance(r#"{"type":"result","subtype":"success"}"#));
            // The abort's write is held until the turn has ended.
            wait_for("the turn's end to refuse the redirection", || {
                !handle.has_pending_redirect()
            });
            release.send(()).unwrap();
            advancing.join().unwrap()
        });
        assert!(ended, "a turn that ended on its own is over");
        let response = pending.join().unwrap();
        assert_eq!(
            response.reason(),
            Some(ControlReason::NoActiveTurn),
            "{response:?}"
        );
        assert!(
            response
                .to_line()
                .contains("the redirection was not delivered"),
            "{response:?}"
        );
        let written = String::from_utf8(written.lock().unwrap().clone()).unwrap();
        assert!(written.contains("control_request"), "{written}");
        assert!(!written.contains("do X instead"), "{written}");
        assert_eq!(
            handle.events()[0].reason(),
            Some(ControlReason::NoActiveTurn)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_redirection_whose_interrupt_was_refused_is_not_taken_over() {
        use crate::domain::control::RedirectInput;
        // Nothing was stopped, so nothing may be queued behind it: a supervisor
        // reading a refusal still owns its message and can send it elsewhere.
        let dir = temp_dir("redirect-refused");
        let path = socket_path(&dir, "idle")
            .expect("a short fixture name fits every platform's socket-address budget");
        let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let response = send(
            &path,
            &ControlRequest::redirect(RedirectInput::new("do X instead").unwrap()),
        );
        assert!(!response.is_ok());
        assert!(!response.is_redirected());
        assert_eq!(response.reason(), Some(ControlReason::NoActiveTurn));
        assert!(!listener.handle_ref().has_pending_redirect());
        assert!(listener.handle_ref().take_redirect().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_frame_that_never_ends_is_refused_at_its_bound_rather_than_accumulated() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;
        // A request is one small JSON object. A peer that writes without ever
        // sending the newline is not sending one, so the reader stops at the
        // bound instead of growing a buffer for as long as the peer keeps
        // typing — and answers, rather than holding the connection until its
        // read timeout expires.
        let dir = temp_dir("unbounded");
        let path = socket_path(&dir, "flood")
            .expect("a short fixture name fits every platform's socket-address budget");
        let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let mut stream = UnixStream::connect(&path).unwrap();
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let started = std::time::Instant::now();
        // Past the bound, with no newline anywhere in it. The write may be cut
        // short by the refusal that closes the connection, which is the point.
        let _ = stream.write_all(&vec![b'x'; 4 * MAX_FRAME_BYTES as usize]);
        let mut reply = String::new();
        BufReader::new(&stream).read_line(&mut reply).unwrap();
        let response: ControlResponse = serde_json::from_str(reply.trim()).unwrap();
        assert!(!response.is_ok(), "{response:?}");
        assert_eq!(response.reason(), Some(ControlReason::Unsupported));
        assert!(
            started.elapsed() < CLIENT_TIMEOUT,
            "the reader waited out its timeout instead of stopping at the bound"
        );
        assert!(listener.handle().events().is_empty());

        // And the listener is still serving: one flooding client must not cost
        // the run its control lever.
        drop(stream);
        let response = send(&path, &ControlRequest::interrupt());
        assert_eq!(response.reason(), Some(ControlReason::NoActiveTurn));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_answer_that_never_ends_is_refused_at_its_bound_too() {
        use std::io::Write;
        use std::os::unix::net::UnixListener;
        // The other direction: `oneharness interrupt` is a separate process
        // reading whatever is listening on that path. It waits for a newline
        // that peer controls, so the answer is bounded exactly like the request
        // — otherwise a wedged or hostile listener decides how much memory the
        // supervisor's process spends before anything is parsed.
        let dir = temp_dir("flood-reply");
        let path = socket_path(&dir, "loud")
            .expect("a short fixture name fits every platform's socket-address budget");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&path).unwrap();
        let written = Arc::new(Mutex::new(0usize));
        let counted = Arc::clone(&written);
        let flooder = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            // Never a newline, and it keeps going until the reader hangs up:
            // the client has to stop on its own terms. Capped so a regression
            // is a failed assertion rather than a test that runs out of memory.
            let blob = vec![b'y'; 8 * 1024];
            for _ in 0..4096 {
                if stream.write_all(&blob).is_err() {
                    return;
                }
                *counted.lock().unwrap_or_else(|e| e.into_inner()) += blob.len();
            }
        });

        let started = std::time::Instant::now();
        let response = send(&path, &ControlRequest::interrupt());
        assert!(!response.is_ok(), "{response:?}");
        assert_eq!(response.reason(), Some(ControlReason::NotRunning));
        // Refused *as oversized* rather than parsed and found unreadable: the
        // second answer is what a reader that swallowed the whole flood gives.
        assert!(
            response.to_line().contains("longer than"),
            "the answer was accumulated instead of bounded: {}",
            response.to_line()
        );
        assert!(
            started.elapsed() < CLIENT_TIMEOUT,
            "the client waited out its timeout instead of stopping at the bound"
        );
        let _ = flooder.join();
        // The peer was cut off near the bound rather than drained to its cap.
        let written = *written.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            (written as u64) < 8 * MAX_FRAME_BYTES,
            "the client read {written} bytes of an unterminated answer"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_malformed_frame_is_refused_without_interrupting() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;
        let dir = temp_dir("malformed");
        let path = socket_path(&dir, "bad")
            .expect("a short fixture name fits every platform's socket-address budget");
        let listener = bind(&path, ControlShape::ClaudeControlRequest).unwrap();
        let mut stream = UnixStream::connect(&path).unwrap();
        stream
            .write_all(b"{\"v\":9,\"verb\":\"interrupt\"}\n")
            .unwrap();
        let mut reply = String::new();
        BufReader::new(&stream).read_line(&mut reply).unwrap();
        let response: ControlResponse = serde_json::from_str(reply.trim()).unwrap();
        assert!(!response.is_ok());
        assert_eq!(response.reason(), Some(ControlReason::Unsupported));
        assert!(listener.handle().events().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_request_that_arrives_in_pieces_is_waited_for_on_a_non_blocking_connection() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::{UnixListener, UnixStream};
        // A supervisor's frame is one write, but a socket does not promise it
        // arrives as one, so the run has to wait for the rest of a request it
        // has only part of. The accept it comes from does not promise a
        // connection that CAN wait: the listener is non-blocking, and a BSD
        // accept — macOS's — returns a socket that inherited that flag while
        // Linux's returns one that did not. So the state is set here
        // explicitly; it is the only way this platform can drive the shape
        // macOS is always in, and without it the run answers "could not read"
        // and the interrupt is lost.
        let dir = temp_dir("pieces");
        let path = socket_path(&dir, "split")
            .expect("a short fixture name fits every platform's socket-address budget");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&path).unwrap();

        let client_path = path.clone();
        // `None` for an exchange the run broke off — the supervisor's side of a
        // reader that gave up is a hang-up, not an answer, so it is reported as
        // that rather than as a panicked thread.
        let client = std::thread::spawn(move || -> Option<String> {
            let mut stream = UnixStream::connect(&client_path).ok()?;
            let mut frame = serde_json::to_string(&ControlRequest::interrupt()).ok()?;
            frame.push('\n');
            let (head, tail) = frame.split_at(frame.len() / 2);
            stream.write_all(head.as_bytes()).ok()?;
            stream.flush().ok()?;
            // Long enough that the reader cannot have the rest yet, so a
            // connection that will not wait has to answer without it.
            std::thread::sleep(std::time::Duration::from_millis(200));
            stream.write_all(tail.as_bytes()).ok()?;
            stream.flush().ok()?;
            let mut reply = String::new();
            BufReader::new(&stream).read_line(&mut reply).ok()?;
            Some(reply)
        });

        let (accepted, _) = listener.accept().unwrap();
        accepted.set_nonblocking(true).unwrap();
        let handle = ControlHandle::new(ControlShape::ClaudeControlRequest);
        imp::serve_connection(accepted, &handle);

        let reply = client.join().unwrap().expect(
            "the run broke off the exchange instead of waiting for the rest of the request",
        );
        let response: ControlResponse = serde_json::from_str(reply.trim()).unwrap();
        // The run's own verdict on a whole request — there is no turn in flight
        // — rather than the refusal a reader that gave up mid-frame reports.
        assert_eq!(
            response.reason(),
            Some(ControlReason::NoActiveTurn),
            "{}",
            response.to_line()
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(all(test, not(unix)))]
mod unsupported_tests {
    use super::*;
    use crate::domain::control::RedirectInput;

    #[test]
    fn a_redirect_without_a_control_socket_is_refused_as_undelivered() {
        let path = Path::new("no-socket-here");
        let response = send(
            path,
            &ControlRequest::redirect(RedirectInput::new("do X instead").unwrap()),
        );
        assert_eq!(response.reason(), Some(ControlReason::NotRunning));
        assert!(
            response
                .to_line()
                .contains("the redirection was not delivered"),
            "{}",
            response.to_line()
        );

        let bare = send(path, &ControlRequest::interrupt());
        assert_eq!(bare.reason(), Some(ControlReason::NotRunning));
        assert!(
            !bare.to_line().contains("redirection"),
            "{}",
            bare.to_line()
        );
    }
}
