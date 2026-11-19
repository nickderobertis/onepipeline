//! Private ordering gates for subprocess regression journeys.
//!
//! A loopback peer announces that the actual process reached a step, then holds
//! that step until the journey releases it. No gate exists unless explicitly
//! supplied in the process environment.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::error::{Error, Result};

/// Return whether this process met a gate. The peer controls ordering, not a delay.
pub(crate) fn meet(variable: &str) -> Result<bool> {
    let Some(address) = std::env::var_os(variable) else {
        return Ok(false);
    };
    let address = address
        .to_str()
        .and_then(|value| value.parse::<SocketAddr>().ok())
        .filter(|address| address.ip().is_loopback())
        .ok_or_else(|| Error::Invalid(format!("{variable} must name a loopback socket")))?;
    exchange(address).map_err(|error| {
        Error::Invalid(format!(
            "ordering gate {variable} at {address} failed: {error}"
        ))
    })?;
    Ok(true)
}

fn exchange(address: SocketAddr) -> std::io::Result<()> {
    let backstop = Duration::from_secs(120);
    let mut stream = TcpStream::connect_timeout(&address, backstop)?;
    stream.set_read_timeout(Some(backstop))?;
    stream.set_write_timeout(Some(backstop))?;
    writeln!(stream, "{}", std::process::id())?;
    let mut released = [0];
    stream.read_exact(&mut released)
}

static EDITS_TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Met once per driver, by the first pass whose edits moved the run, as soon as it has
/// taken them up and before it goes on: a journey holds the loop there, with the edit
/// applied and answered, to order another thread against everything the rest of that pass
/// does.
pub(crate) fn edits_taken() -> Result<()> {
    if !EDITS_TAKEN.swap(true, std::sync::atomic::Ordering::SeqCst) {
        meet("ONEPIPELINE_TEST_EDITS_TAKEN_GATE")?;
    }
    Ok(())
}

static TIMER_WAITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SETTLEMENT_START: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SETTLEMENT_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn timer_wait() -> Result<()> {
    TIMER_WAITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if SETTLEMENT_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        meet("ONEPIPELINE_TEST_POLL_EXPIRED_GATE")?;
    }
    Ok(())
}

fn settlement_target(node: &str) -> bool {
    std::env::var("ONEPIPELINE_TEST_SETTLEMENT_NODE").is_ok_and(|target| target == node)
}

pub(crate) fn settlement_started(node: &str) -> Result<()> {
    if settlement_target(node) {
        SETTLEMENT_START.store(
            TIMER_WAITS.load(std::sync::atomic::Ordering::Relaxed),
            std::sync::atomic::Ordering::Relaxed,
        );
        SETTLEMENT_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
        meet("ONEPIPELINE_TEST_SETTLEMENT_GATE")?;
    }
    Ok(())
}

/// Hold the loop on the target node's relayed push as it is received, before it drains
/// what is queued behind it: released after that node's settlement has been queued, the
/// push and the settlement are applied in one pass, so whatever the push makes owed is
/// claimed only once the settlement is on the record. Answers whether it held.
pub(crate) fn push_received(node: &str) -> Result<bool> {
    if settlement_target(node) {
        return meet("ONEPIPELINE_TEST_PUSH_RECEIVED_GATE");
    }
    Ok(false)
}

pub(crate) fn settlement_before_queue(node: &str) -> Result<()> {
    if settlement_target(node) {
        meet("ONEPIPELINE_TEST_SETTLEMENT_BEFORE_QUEUE_GATE")?;
    }
    Ok(())
}

/// Hold the relay after the loop has received a settlement, so a journey can
/// let the driver close while the relay cannot forward another message.
pub(crate) fn settlement_relayed() -> Result<()> {
    meet("ONEPIPELINE_TEST_SETTLEMENT_RELAYED_GATE")?;
    Ok(())
}

pub(crate) fn settlement_queued(node: &str) -> Result<()> {
    if settlement_target(node) {
        meet("ONEPIPELINE_TEST_SETTLEMENT_QUEUED_GATE")?;
    }
    Ok(())
}

static SETTLEMENT_PROJECTING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Met once, by the write-back, before it projects the first snapshot carrying the settlement
/// of the node `ONEPIPELINE_TEST_PROJECTION_SETTLED_NODE` names: a journey holds that
/// settlement off the store there.
pub(crate) fn settlement_projecting<'a>(
    mut settled: impl Iterator<Item = &'a String>,
) -> Result<()> {
    let Ok(target) = std::env::var("ONEPIPELINE_TEST_PROJECTION_SETTLED_NODE") else {
        return Ok(());
    };
    if settled.any(|node| *node == target)
        && !SETTLEMENT_PROJECTING.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        meet("ONEPIPELINE_TEST_PROJECTION_SETTLED_GATE")?;
    }
    Ok(())
}

static DISPATCH_PROJECTING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Met once, by the write-back, before it projects the first snapshot in which the node
/// `ONEPIPELINE_TEST_PROJECTION_DISPATCHED_NODE` names is running and not yet settled: a
/// journey holds that dispatch's projection off the store there until the node has settled.
pub(crate) fn dispatch_projecting<'a>(mut running: impl Iterator<Item = &'a String>) -> Result<()> {
    let Ok(target) = std::env::var("ONEPIPELINE_TEST_PROJECTION_DISPATCHED_NODE") else {
        return Ok(());
    };
    if running.any(|node| *node == target)
        && !DISPATCH_PROJECTING.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        meet("ONEPIPELINE_TEST_PROJECTION_DISPATCHED_GATE")?;
    }
    Ok(())
}

static JOURNAL_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Met once, inside this process's first append to a run's journal of a record of the kind
/// `ONEPIPELINE_TEST_JOURNAL_HELD_KIND` names, with the journal open and its appender lock
/// held: a journey holds the appender there to write to the journal from another process.
pub(crate) fn journal_held(path: &std::path::Path, line: &str) -> Result<()> {
    let Ok(target) = std::env::var("ONEPIPELINE_TEST_JOURNAL_HELD_KIND") else {
        return Ok(());
    };
    let kind = serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|record| record.get("kind")?.as_str().map(str::to_owned));
    if path.file_name() == Some(std::ffi::OsStr::new("events.jsonl"))
        && kind.as_deref() == Some(target.as_str())
        && !JOURNAL_HELD.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        meet("ONEPIPELINE_TEST_JOURNAL_HELD_GATE")?;
    }
    Ok(())
}

pub(crate) fn settlement_recorded(paths: &crate::ledger::RunPaths, node: &str) -> Result<()> {
    if settlement_target(node) {
        SETTLEMENT_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        let waits = TIMER_WAITS.load(std::sync::atomic::Ordering::Relaxed)
            - SETTLEMENT_START.load(std::sync::atomic::Ordering::Relaxed);
        let path = paths.dir.join("settlement-timer-waits.txt");
        crate::ledger::write_atomic(
            &path,
            waits.to_string().as_bytes(),
            crate::ledger::Durability::Projection,
        )?;
    }
    Ok(())
}

/// The clock a `watch --flow` reads its deadline, its heartbeat and its waits off.
///
/// The host's own wherever [`FLOW_WATCH_CLOCK_ENV`] is unset, which is everywhere
/// but a regression journey: then every reading and every wait is exactly the one
/// the watch made before this seam existed. Set, it names a loopback peer the
/// watch asks for the time and hands each wait to instead of sleeping, so the
/// journey decides when every moment of the watch's wait comes, and sees each
/// one the watch asked about, in order — no reading of it depends on how promptly
/// the host runs either party.
pub(crate) enum WatchClock {
    /// The host's monotonic clock and a real sleep.
    Host,
    /// A journey's: `now` is `origin` plus the milliseconds the peer answers.
    Peer {
        origin: std::time::Instant,
        asks: TcpStream,
        answers: std::io::BufReader<TcpStream>,
    },
}

/// The variable naming the loopback peer a flow watch's [`WatchClock`] asks.
pub(crate) const FLOW_WATCH_CLOCK_ENV: &str = "ONEPIPELINE_TEST_FLOW_WATCH_CLOCK";

impl WatchClock {
    /// The host's clock, or the peer [`FLOW_WATCH_CLOCK_ENV`] names.
    pub(crate) fn from_environment() -> Result<Self> {
        let Some(address) = std::env::var_os(FLOW_WATCH_CLOCK_ENV) else {
            return Ok(Self::Host);
        };
        let address = address
            .to_str()
            .and_then(|value| value.parse::<SocketAddr>().ok())
            .filter(|address| address.ip().is_loopback())
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "{FLOW_WATCH_CLOCK_ENV} must name a loopback socket"
                ))
            })?;
        let connected = || -> std::io::Result<Self> {
            let backstop = Duration::from_secs(120);
            let asks = TcpStream::connect_timeout(&address, backstop)?;
            // Each question is a turn the watch waits on: sent as it is written.
            asks.set_nodelay(true)?;
            asks.set_read_timeout(Some(backstop))?;
            asks.set_write_timeout(Some(backstop))?;
            let answers = std::io::BufReader::new(asks.try_clone()?);
            Ok(Self::Peer {
                origin: std::time::Instant::now(),
                asks,
                answers,
            })
        };
        connected().map_err(|error| clock_failed(&address, &error))
    }

    /// What time it is.
    pub(crate) fn now(&mut self) -> Result<std::time::Instant> {
        match self {
            Self::Host => Ok(std::time::Instant::now()),
            Self::Peer {
                origin,
                asks,
                answers,
            } => {
                let millis = ask(asks, answers, "now")?
                    .parse::<u64>()
                    .map_err(|error| Error::Invalid(format!("{FLOW_WATCH_CLOCK_ENV}: {error}")))?;
                Ok(*origin + Duration::from_millis(millis))
            }
        }
    }

    /// The deadline a wait of `timeout` gives up at, taken now.
    pub(crate) fn deadline(
        &mut self,
        timeout: crate::cli::WatchTimeout,
    ) -> Result<Option<std::time::Instant>> {
        match self {
            Self::Host => crate::watch::deadline(timeout),
            Self::Peer { .. } => match timeout {
                crate::cli::WatchTimeout::Bounded(_) => {
                    let now = self.now()?;
                    crate::watch::deadline_from(timeout, || now)
                }
                crate::cli::WatchTimeout::Unbounded => Ok(None),
            },
        }
    }

    /// Whether `deadline` has come.
    pub(crate) fn due(&mut self, deadline: Option<std::time::Instant>) -> Result<bool> {
        match deadline {
            Some(deadline) => Ok(self.now()? >= deadline),
            None => Ok(false),
        }
    }

    /// How long it has been since `since`.
    pub(crate) fn since(&mut self, since: std::time::Instant) -> Result<Duration> {
        match self {
            Self::Host => Ok(since.elapsed()),
            Self::Peer { .. } => Ok(self.now()?.saturating_duration_since(since)),
        }
    }

    /// Wait as long as [`crate::watch::wake_within`] says, and no longer than `most`.
    pub(crate) fn wait(
        &mut self,
        deadline: Option<std::time::Instant>,
        tick: Duration,
        quiet_since: std::time::Instant,
        most: Duration,
    ) -> Result<()> {
        match self {
            Self::Host => {
                std::thread::sleep(
                    crate::watch::wake_within(deadline, tick, quiet_since).min(most),
                );
                Ok(())
            }
            Self::Peer { .. } => {
                let now = self.now()?;
                let wait =
                    crate::watch::wake_within_at(|| now, deadline, tick, quiet_since).min(most);
                let Self::Peer { asks, answers, .. } = self else {
                    unreachable!("matched as a peer above")
                };
                let millis = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX);
                match ask(asks, answers, &format!("wait {millis}"))?.as_str() {
                    "ok" => Ok(()),
                    other => Err(Error::Invalid(format!(
                        "{FLOW_WATCH_CLOCK_ENV} answered a wait with '{other}'"
                    ))),
                }
            }
        }
    }
}

fn ask(
    asks: &mut TcpStream,
    answers: &mut std::io::BufReader<TcpStream>,
    question: &str,
) -> Result<String> {
    let failed = |error: std::io::Error| {
        Error::Invalid(format!("the flow watch's clock peer failed: {error}"))
    };
    // One write per question, so it leaves whole.
    asks.write_all(format!("{question}\n").as_bytes())
        .map_err(failed)?;
    let mut answer = String::new();
    std::io::BufRead::read_line(answers, &mut answer).map_err(failed)?;
    Ok(answer.trim_end().to_owned())
}

fn clock_failed(address: &SocketAddr, error: &std::io::Error) -> Error {
    Error::Invalid(format!(
        "{FLOW_WATCH_CLOCK_ENV} at {address} failed: {error}"
    ))
}

#[cfg(test)]
mod tests {
    use super::exchange;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn the_process_stays_at_the_gate_until_its_peer_releases_it() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the gate listens");
        let address = listener.local_addr().expect("the gate address");
        let peer = std::thread::spawn(move || exchange(address));
        let (mut stream, _) = listener.accept().expect("the process arrives");
        let mut announcement = String::new();
        BufReader::new(stream.try_clone().expect("the stream is shared"))
            .read_line(&mut announcement)
            .expect("the process announces itself");
        assert_eq!(announcement.trim(), std::process::id().to_string());
        assert!(!peer.is_finished(), "the process passed an unreleased gate");
        stream.write_all(b"go").expect("the peer releases it");
        peer.join()
            .expect("the process finished")
            .expect("the gate answered");
    }

    #[test]
    fn a_peer_that_disconnects_does_not_release_the_gate() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the gate listens");
        let address = listener.local_addr().expect("the gate address");
        let peer = std::thread::spawn(move || exchange(address));
        let (stream, _) = listener.accept().expect("the process arrives");
        let mut announcement = String::new();
        BufReader::new(&stream)
            .read_line(&mut announcement)
            .expect("the announcement");
        drop(stream);
        assert_eq!(
            peer.join()
                .expect("the process finished")
                .expect_err("the gate never answered")
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }
}
