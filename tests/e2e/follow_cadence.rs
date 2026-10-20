//! How often a live follow asks after the session it follows.
//!
//! A follow reads its session's stream every twentieth of a second and asks whether the
//! session has been released far less often: the read is one file, and the ask is a library
//! call that shells out to `git` in the session's clone about a dozen times. Asked at the
//! stream's cadence, a follow spawned those processes back to back for the whole of a
//! publication — on Windows, where a process is the expensive thing, that was most of what a
//! busy review round cost. What is observed is the ask itself: each one reads the session's
//! record, so the kernel's record of the driver opening that file is the count, taken
//! without any knowledge of how the linked library asks.
// llmlint: ignore-file[e2e_not_mocked] `oneagentgraph` is the established double at its
// subprocess boundary, because a real agent turn is a paid one and this journey is about the
// follow of a publication: the session, its clone, its stream and the push are the real linked
// `onevcs` over a real origin on disk, and the driver is the compiled binary.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::harness::{held_publication, lifecycle, plan_of, World};

/// The least time between two asks the follow makes, a little under the five seconds it waits:
/// the tracer stamps each open when the call enters, and the follow's own clock starts
/// before its call does.
const LEAST_GAP_SECONDS: f64 = 4.9;

/// How many asks the journey waits to see: enough gaps that one long one — a loaded host
/// slowing a single ask — cannot stand in for the cadence.
const ASKS: usize = 4;

/// Each open of `record` the trace holds, as the thread that made it and when, in the order
/// the tracer wrote them.
fn opens_of(trace: &Path, record: &str) -> Vec<(String, f64)> {
    std::fs::read_to_string(trace)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(&format!("\"{record}\"")) && !line.contains("ENOENT"))
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let thread = fields.next()?.to_owned();
            let at = fields.next()?.parse().ok()?;
            Some((thread, at))
        })
        .collect()
}

/// A live follow asks after its session at most once every five seconds, while the stream it relays is
/// read as it grows and the follow still ends the moment its publication does.
///
/// The publication is held at the repository's merge path, so for as long as this journey
/// watches, the follow is live and nothing else in the driver has reason to read the
/// session's record. Linux-only, as every journey that reads the kernel's account of the
/// binary is: the tracer is Linux's, and the cadence it measures is the same code on every
/// platform.
#[test]
fn a_live_follow_asks_after_its_session_at_most_every_five_seconds() {
    let world = World::new("follow-cadence");
    let go = world.fakes.join("push.go");
    let held = held_publication(&world, &go);
    world.repository("local-direct", &held.argv());
    world.script("build.work", "the follow saw this\n");
    let path = world.plan(
        "followed",
        &plan_of("followed", vec![lifecycle("build", &[])]),
    );

    // The command `World` composes, with the tracer wrapped around it, so what is observed is
    // the launch a user makes. Refused rather than passed where the tracer will not run.
    let trace = world.root.join("follow.strace");
    let inner = world.cmd(&["start", &path, "--attach"]);
    let mut traced = Command::new("strace");
    traced
        .args([
            "-f",
            "-qq",
            "-ttt",
            "-e",
            "trace=openat",
            "-e",
            "signal=none",
            "-o",
        ])
        .arg(&trace)
        .arg(inner.get_program())
        .args(inner.get_args())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in inner.get_envs() {
        match value {
            Some(value) => traced.env(key, value),
            None => traced.env_remove(key),
        };
    }
    let driver = traced.spawn().unwrap_or_else(|error| {
        panic!(
            "this journey's claim is what the driver opened, and the tracer would not run: \
             strace: {error}. Install strace, or run the suite where ptrace is permitted."
        )
    });

    // A record the follow relayed while the push is held: the stream is being read as it
    // grows, and the publication it follows is still running.
    world.until(
        "the follow to relay the publication it is following",
        |world| !world.events_of("followed", "merge-queued").is_empty(),
    );
    let token = world.events_of("followed", "session-opened")[0]["payload"]["token"]
        .as_str()
        .expect("the session names itself")
        .to_owned();
    let record = world
        .onevcs_home()
        .join("sessions")
        .join(format!("{token}.json"))
        .to_string_lossy()
        .into_owned();
    let from = opens_of(&trace, &record).len();
    world.until(
        "the follow to ask after its session again and again",
        |_| opens_of(&trace, &record).len() >= from + ASKS,
    );
    let asked = opens_of(&trace, &record);
    held.release();
    crate::harness::ended(driver);
    assert!(
        world.run_file("followed", "result.json").is_file(),
        "the run did not settle once its publication went through"
    );

    // One thread asks — the follow's — and every ask it made after the first is five seconds or
    // more after the one before.
    let threads: std::collections::BTreeSet<&str> = asked[from..]
        .iter()
        .map(|(thread, _)| thread.as_str())
        .collect();
    assert_eq!(
        threads.len(),
        1,
        "more than one thread read the session's record while the push was held: {asked:?}"
    );
    let gaps: Vec<f64> = asked[from..]
        .windows(2)
        .map(|pair| pair[1].1 - pair[0].1)
        .collect();
    assert!(
        gaps.iter().all(|gap| *gap >= LEAST_GAP_SECONDS),
        "the follow asked after its session less than five seconds after it last asked: gaps {gaps:?}"
    );
}
