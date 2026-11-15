//! Which clock this process's sources pace and back off on, and the client half of the
//! simulated one.
//!
//! [`process_clock`] is the one decision: with [`SIMULATED_CLOCK_VARIABLE`] unset or empty it
//! is [`system_clock`], and every source behaves exactly as it does on the runtime's timer.
//! Set, it names a coordinator a test started and which client of it this process is, and the
//! clock answers every [`Clock::now`] and completes every [`Clock::sleep`] in that
//! coordinator's virtual time — so a journey whose plugin waits a minute between pages costs
//! no real minute, and reports the same figures on every run.
//!
//! # The wire between a client and its coordinator
//!
//! One TCP connection to the address the variable names, one line per message, each side
//! flushing after every line:
//!
//! | From | Line | Meaning |
//! | --- | --- | --- |
//! | client | `attach <client>` | This process is client `<client>`, 0-based. Sent once, first. |
//! | coordinator | `attached` | The attach is recorded; the client goes on. |
//! | client | `now <seq>` | What is virtual time now? |
//! | coordinator | `now <seq> <nanos>` | It is `<nanos>` nanoseconds past the origin. |
//! | client | `sleep <seq> <nanos>` | Wake this client's wait `<seq>` once `<nanos>` have passed. |
//! | client | `sleep <seq> <nanos> <cause>` | The same, for a wait the plugin names a cause of — one word, a `WaitCause`, see [`Clock::sleep_because`]. A coordinator keeps it beside the wait. |
//! | coordinator | `wake <seq>` | That wait is over. |
//! | client | `cancel <seq>` | The wait `<seq>` was dropped before it was woken. |
//!
//! The connection closing is the client detaching. The coordinator's own half, and the rule
//! for when virtual time advances, are the test harness's to state, beside `SimulatedClock`.

use std::collections::BTreeMap;
use std::future::Future;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use onetaskgraph_plugin_api::{Clock, SharedClock, WaitCause, system_clock};
use tokio::sync::oneshot;

use crate::Environment;

/// The variable that puts this process on a simulated clock; see
/// [`onetaskgraph_plugin_api::SIMULATED_CLOCK_VARIABLE`].
pub use onetaskgraph_plugin_api::SIMULATED_CLOCK_VARIABLE;

/// Which clock a value of [`SIMULATED_CLOCK_VARIABLE`] asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockChoice {
    /// The real clock.
    System,
    /// A simulated clock attached to the coordinator at `address`, as client `client`.
    Simulated {
        /// The coordinator's address.
        address: SocketAddr,
        /// This process's 0-based client number.
        client: usize,
    },
}

/// The clock `value` — the variable's value, or `None` when it is unset — asks for.
///
/// # Errors
///
/// A message naming the variable when the value is set and is not `<address>/<client>`.
pub fn clock_choice(value: Option<&str>) -> Result<ClockChoice, String> {
    let value = value.unwrap_or_default().trim();
    if value.is_empty() {
        return Ok(ClockChoice::System);
    }
    let refuse = || {
        format!(
            "{SIMULATED_CLOCK_VARIABLE} is {value:?}, which is not `<address>/<client>` — a \
             loopback address and a client number, as a test's simulated clock hands it out; \
             unset it to run on the real clock"
        )
    };
    let (address, client) = value.rsplit_once('/').ok_or_else(refuse)?;
    let address: SocketAddr = address.parse().map_err(|_| refuse())?;
    // A simulated clock is a test's, on the machine the test runs on: an address anywhere else
    // would put this process's every wait in the hands of a host nobody named for that.
    if !address.ip().is_loopback() {
        return Err(format!(
            "{SIMULATED_CLOCK_VARIABLE} names {address}, which is not a loopback address; a \
             simulated clock is a test's own, on this machine — unset it to run on the real \
             clock"
        ));
    }
    Ok(ClockChoice::Simulated {
        address,
        client: client.parse().map_err(|_| refuse())?,
    })
}

/// This process's one clock, as `environment` asks for it.
///
/// # Errors
///
/// A message naming the variable when its value is malformed, or when the coordinator it
/// names cannot be reached.
pub fn process_clock(environment: &Environment) -> Result<SharedClock, String> {
    match clock_choice(environment.get(SIMULATED_CLOCK_VARIABLE))? {
        ClockChoice::System => Ok(system_clock()),
        ClockChoice::Simulated { address, client } => attach(address, client),
    }
}

/// Attach to the coordinator at `address` as client `client`, answering a clock that runs on
/// its virtual time.
///
/// # Errors
///
/// A message naming the variable and the address when the coordinator cannot be reached or
/// does not acknowledge the attach.
pub fn attach(address: SocketAddr, client: usize) -> Result<SharedClock, String> {
    let unreachable = |error: std::io::Error| {
        format!(
            "{SIMULATED_CLOCK_VARIABLE} names a simulated clock at {address} that could not \
             be reached: {error}; unset it to run on the real clock"
        )
    };
    let stream = TcpStream::connect(address).map_err(unreachable)?;
    stream.set_nodelay(true).map_err(unreachable)?;
    let mut writer = stream.try_clone().map_err(unreachable)?;
    let mut reader = BufReader::new(stream);
    writeln!(writer, "attach {client}").map_err(unreachable)?;
    writer.flush().map_err(unreachable)?;
    let mut line = String::new();
    reader.read_line(&mut line).map_err(unreachable)?;
    if line.trim_end() != "attached" {
        return Err(format!(
            "the simulated clock at {address} answered {line:?} to an attach rather than \
             `attached`; unset {SIMULATED_CLOCK_VARIABLE} to run on the real clock"
        ));
    }
    let shared = Arc::new(Shared {
        address,
        writer: Mutex::new(writer),
        pending: Mutex::new(Pending::default()),
        sequence: AtomicU64::new(0),
    });
    let listening = Arc::clone(&shared);
    std::thread::spawn(move || listening.listen(reader));
    Ok(Arc::new(SimulatedClient { shared }))
}

/// A clock whose time is the coordinator's.
struct SimulatedClient {
    shared: Arc<Shared>,
}

/// What a client's calls and its listening thread share.
struct Shared {
    address: SocketAddr,
    writer: Mutex<TcpStream>,
    pending: Mutex<Pending>,
    sequence: AtomicU64,
}

/// The answers a client is waiting for, by sequence number.
#[derive(Default)]
struct Pending {
    nows: BTreeMap<u64, mpsc::Sender<Duration>>,
    wakes: BTreeMap<u64, oneshot::Sender<()>>,
    /// Set once the connection is gone: nothing waits on it any more.
    closed: bool,
    /// The latest time the coordinator answered, which no later answer may precede.
    latest: Duration,
}

impl Shared {
    /// Send one line; a coordinator that has gone answers nothing, which the waiters below
    /// read as the connection having closed.
    fn send(&self, line: &str) {
        let mut writer = self
            .writer
            .lock()
            .expect("the clock's writer is not poisoned");
        let _ = writeln!(writer, "{line}").and_then(|()| writer.flush());
    }

    /// The coordinator is gone, or said something that is not a time: this process's time is
    /// no longer known, and no answer it could give would be true. A simulated clock is only
    /// ever a test's, so the test is stopped saying so rather than run on a time nobody kept.
    fn lost(&self) -> ! {
        // llmlint: ignore[no_panics_on_recoverable_errors] `Clock::now` and `Clock::sleep` are infallible by the contract every plugin waits through, so there is no caller to propagate to; the only alternatives are a fabricated time or a wait that never ends, and both would let a test report figures nobody measured. This client exists only under a coordinator a test started, so its loss is that test's failure, and stopping with the reason is the recoverable path.
        panic!(
            "the simulated clock at {} closed its connection or answered with something that \
             is not a time, so this process's time is unknown; check the test that started it",
            self.address
        )
    }

    fn next(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }

    /// Hand every line the coordinator writes to whoever is waiting for it, until it closes.
    fn listen(&self, reader: BufReader<TcpStream>) {
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let mut words = line.split_whitespace();
            let mut pending = self
                .pending
                .lock()
                .expect("the clock's waiters are not poisoned");
            let verb = words.next();
            let seq = words.next().and_then(|seq| seq.parse::<u64>().ok());
            match (verb, seq, words.next(), words.next()) {
                (Some("now"), Some(seq), Some(nanos), None) => {
                    // A time that is not one ends the connection rather than reading as zero:
                    // every wait after it would be measured from a time nobody said.
                    let Ok(nanos) = nanos.parse::<u64>() else {
                        break;
                    };
                    // So does a time nobody asked for — the caller who did ask would wait on —
                    // and one earlier than a time already answered, which a monotonic clock
                    // cannot report.
                    let now = Duration::from_nanos(nanos);
                    let Some(waiter) = pending.nows.remove(&seq) else {
                        break;
                    };
                    if now < pending.latest {
                        break;
                    }
                    pending.latest = now;
                    let _ = waiter.send(now);
                }
                // A wake for a wait already dropped is the coordinator answering a sleep before
                // it read the cancellation, so it is passed over rather than refused.
                (Some("wake"), Some(seq), None, None) => {
                    if let Some(waiter) = pending.wakes.remove(&seq) {
                        let _ = waiter.send(());
                    }
                }
                // A line that is not one of the coordinator's ends the connection too.
                _ => break,
            }
        }
        let mut pending = self
            .pending
            .lock()
            .expect("the clock's waiters are not poisoned");
        pending.closed = true;
        pending.nows.clear();
        pending.wakes.clear();
    }
}

impl Clock for SimulatedClient {
    fn now(&self) -> Duration {
        let seq = self.shared.next();
        let (sender, receiver) = mpsc::channel();
        {
            let mut pending = self
                .shared
                .pending
                .lock()
                .expect("the clock's waiters are not poisoned");
            if pending.closed {
                self.shared.lost();
            }
            pending.nows.insert(seq, sender);
        }
        self.shared.send(&format!("now {seq}"));
        receiver.recv().unwrap_or_else(|_| self.shared.lost())
    }

    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.wait(duration, None)
    }

    fn sleep_because(
        &self,
        duration: Duration,
        cause: WaitCause,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.wait(duration, Some(cause))
    }
}

impl SimulatedClient {
    /// One wait on the coordinator, naming its cause when it has one.
    fn wait(
        &self,
        duration: Duration,
        cause: Option<WaitCause>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        let seq = self.shared.next();
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self
                .shared
                .pending
                .lock()
                .expect("the clock's waiters are not poisoned");
            if pending.closed {
                self.shared.lost();
            }
            pending.wakes.insert(seq, sender);
        }
        // A cause is one word on the wire, which `WaitCause` holds it to.
        let line = match cause {
            Some(cause) => format!("sleep {seq} {} {cause}", duration.as_nanos()),
            None => format!("sleep {seq} {}", duration.as_nanos()),
        };
        self.shared.send(&line);
        Box::pin(Wait {
            shared: Arc::clone(&self.shared),
            seq,
            receiver,
            woken: false,
        })
    }
}

/// One wait on the coordinator, which tells it when the wait is dropped unwoken so a client
/// that stopped waiting is not counted as waiting.
struct Wait {
    shared: Arc<Shared>,
    seq: u64,
    receiver: oneshot::Receiver<()>,
    woken: bool,
}

impl Future for Wait {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        match Pin::new(&mut self.receiver).poll(context) {
            Poll::Ready(Ok(())) => {
                self.woken = true;
                Poll::Ready(())
            }
            // Nothing will ever wake it, and completing it would claim a time that never came.
            Poll::Ready(Err(_)) => self.shared.lost(),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        if !self.woken {
            self.shared
                .pending
                .lock()
                .expect("the clock's waiters are not poisoned")
                .wakes
                .remove(&self.seq);
            self.shared.send(&format!("cancel {}", self.seq));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variable_unset_or_empty_selects_the_real_clock() {
        assert_eq!(clock_choice(None), Ok(ClockChoice::System));
        assert_eq!(clock_choice(Some("")), Ok(ClockChoice::System));
        assert_eq!(clock_choice(Some("  ")), Ok(ClockChoice::System));
    }

    #[test]
    fn a_value_names_the_coordinator_and_the_client() {
        assert_eq!(
            clock_choice(Some("127.0.0.1:4567/1")),
            Ok(ClockChoice::Simulated {
                address: "127.0.0.1:4567".parse().expect("an address"),
                client: 1,
            })
        );
        for malformed in [
            "127.0.0.1:4567",
            "nowhere/0",
            "127.0.0.1:4567/x",
            "192.0.2.1:4567/0",
        ] {
            let refused = clock_choice(Some(malformed)).expect_err("refused");
            assert!(refused.contains(SIMULATED_CLOCK_VARIABLE), "{refused}");
        }
    }

    #[tokio::test]
    async fn a_named_cause_is_sent_as_its_word_beside_a_plain_sleep() {
        // A stand-in coordinator over a real socket, waking each wait as it arrives and keeping
        // every line the client sent: a wait with a cause carries its word as the line's last,
        // and a plain sleep carries none.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let address = listener.local_addr().expect("its address");
        let coordinator = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("the client connects");
            let mut writer = stream.try_clone().expect("a second handle");
            let mut sent = Vec::new();
            for line in BufReader::new(stream).lines() {
                let line = line.expect("a line");
                let mut words = line.split_whitespace();
                match words.next() {
                    Some("attach") => writeln!(writer, "attached").expect("answered"),
                    Some("sleep") => {
                        let seq = words.next().expect("a sequence number");
                        writeln!(writer, "wake {seq}").expect("woken");
                        sent.push(line.clone());
                        if sent.len() == 3 {
                            return sent;
                        }
                    }
                    _ => {}
                }
            }
            sent
        });
        let clock = attach(address, 0).expect("attached");
        let second = Duration::from_secs(1);
        clock.sleep(second).await;
        clock
            .sleep_because(second, WaitCause::new("retry-after"))
            .await;
        clock
            .sleep_because(second, WaitCause::new("board-filing"))
            .await;
        let nanos = second.as_nanos();
        assert_eq!(
            coordinator.join().expect("the coordinator"),
            [
                format!("sleep 0 {nanos}"),
                format!("sleep 1 {nanos} retry-after"),
                format!("sleep 2 {nanos} board-filing"),
            ]
        );
    }

    #[tokio::test]
    async fn an_unset_variable_builds_the_real_clock_which_sleeps_for_real() {
        let clock = process_clock(&Environment::from_pairs(
            std::iter::empty::<(String, String)>(),
        ))
        .expect("the real clock");
        let before = clock.now();
        clock.sleep(Duration::from_millis(20)).await;
        assert!(clock.now() - before >= Duration::from_millis(20));
    }
}
