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

pub(crate) fn settlement_queued(node: &str) -> Result<()> {
    if settlement_target(node) {
        meet("ONEPIPELINE_TEST_SETTLEMENT_QUEUED_GATE")?;
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
