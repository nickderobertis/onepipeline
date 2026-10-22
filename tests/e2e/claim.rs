//! A driver whose claim on its project no read could confirm keeps every dispatch it has in
//! flight, starts nothing, says so everywhere a view looks, and carries on at the first read
//! that confirms it — while a read that finds another driver's claim still ends its work, and
//! an adopter starts on evidence rather than on a timer. Entry 137 of
//! `docs/contract-divergences.md`.
//!
//! Each journey drives the compiled binary over a real `local-md` store — the scripted one
//! where a journey makes the store refuse the claim's read — the real linked `onevcs` over a
//! real origin on disk, and GitHub at `onevcs`'s own `ONEVCS_GH` seam, with the claim
//! interval shortened through the engine's own setting. Every journey that drives an
//! unconfirmed claim also runs a separate process reading `<run>/claim.json` in a tight loop
//! through its declared type while the driver rewrites it, which must never read a torn file.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary over a real `local-md` store, and `onevcs` is the real library over real git and a
// real origin on disk. `oneagentgraph` is substituted at its subprocess boundary so a
// journey states a dispatch outcome rather than paying for a model turn, and GitHub at
// `onevcs`'s own `ONEVCS_GH` override so a change request can be read back offline.
// `harness.rs` carries the same suppression and the full rationale.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{agent, lifecycle, plan_of, Rendezvous, World};
use onepipeline::views::ClaimFile;

/// The setting naming the claim interval, in whole seconds.
const CLAIM_INTERVAL_ENV: &str = "ONEPIPELINE_CLAIM_INTERVAL_SECONDS";

/// The failpoint met before each read of the claim.
const READ_GATE: &str = "ONEPIPELINE_TEST_CLAIM_READ_GATE";

/// The failpoint met inside each read of the claim, after it began.
const READ_LATE_GATE: &str = "ONEPIPELINE_TEST_CLAIM_READ_LATE_GATE";

/// The failpoint met before a dispatch of the node [`BEGIN_NODE`] names would start.
const BEGIN_GATE: &str = "ONEPIPELINE_TEST_CLAIM_BEGIN_GATE";
const BEGIN_NODE: &str = "ONEPIPELINE_TEST_CLAIM_BEGIN_NODE";

/// The claim interval every journey here runs on: the lease window.
const INTERVAL: u64 = 5;

/// What the claim's read's deadline is: past it, a read is late.
const READ_DEADLINE: Duration = Duration::from_secs(20);

/// The variables the reader process is started with.
const READER_FILE_ENV: &str = "ONEPIPELINE_TEST_CLAIM_READER_FILE";
const READER_STOP_ENV: &str = "ONEPIPELINE_TEST_CLAIM_READER_STOP";

/// A separate process reading a run's `claim.json` in a tight loop through its declared
/// type, while the driver rewrites it: this test binary again, at
/// [`a_claim_file_read_while_it_is_rewritten_is_always_whole`].
struct ClaimReader {
    child: std::process::Child,
    stop: PathBuf,
}

impl ClaimReader {
    fn start(world: &World, run: &str) -> Self {
        let file = world.run_file(run, "claim.json");
        let stop = world.run_file(run, "claim-reader.stop");
        let child = std::process::Command::new(std::env::current_exe().expect("this binary"))
            .args([
                "--exact",
                "claim::a_claim_file_read_while_it_is_rewritten_is_always_whole",
                "--nocapture",
            ])
            .env(READER_FILE_ENV, &file)
            .env(READER_STOP_ENV, &stop)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the reader starts");
        Self { child, stop }
    }

    /// Stop it, and hold that every document it read was whole: answers how many it read.
    fn finished(self) -> u64 {
        std::fs::write(&self.stop, "").expect("the reader is told to stop");
        let out = self.child.wait_with_output().expect("the reader ends");
        let said = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "the reader read a torn claim.json:\n{said}"
        );
        let read = said
            .lines()
            .find_map(|line| line.strip_prefix("claim.json read whole: "))
            .and_then(|count| count.trim().parse::<u64>().ok())
            .unwrap_or_else(|| panic!("the reader said nothing of what it read:\n{said}"));
        assert!(
            read > 0,
            "the reader never found a claim.json to read:\n{said}"
        );
        read
    }
}

/// The reader process, when this binary is started as one: reads the file it is named in a
/// tight loop, and fails on the first read that is not a whole document of its declared
/// type. Run as the suite runs it, it holds what that loop relies on instead — that the
/// declared type refuses an empty or truncated document, so a reader that never failed
/// never read one.
#[test]
fn a_claim_file_read_while_it_is_rewritten_is_always_whole() {
    let Some(file) = std::env::var_os(READER_FILE_ENV) else {
        let whole = r#"{"schema_version":1,"state":"unconfirmed","last_confirmed_at":null,"unconfirmed_since":"2026-10-21T00:00:00.000Z","reason":"late","detail":"no answer","lease_interval_s":5}"#;
        assert!(ClaimFile::parse(whole).is_ok());
        assert!(ClaimFile::parse("").is_err());
        for cut in 1..whole.len() {
            assert!(ClaimFile::parse(&whole[..cut]).is_err());
        }
        return;
    };
    let stop = PathBuf::from(std::env::var_os(READER_STOP_ENV).expect("a stop file"));
    let file = PathBuf::from(file);
    let mut read = 0_u64;
    while !stop.exists() {
        match std::fs::read_to_string(&file) {
            Ok(text) => {
                if let Err(why) = ClaimFile::parse(&text) {
                    panic!("read a claim.json that is not whole ({why}): {text:?}");
                }
                read += 1;
            }
            Err(why) if why.kind() == std::io::ErrorKind::NotFound => {}
            Err(why) => panic!("could not read {}: {why}", file.display()),
        }
    }
    println!("claim.json read whole: {read}");
}

/// The run's `claim.json`, read through its declared type.
fn claim_file(world: &World, run: &str) -> Option<ClaimFile> {
    std::fs::read_to_string(world.run_file(run, "claim.json"))
        .ok()
        .map(|text| ClaimFile::parse(&text).expect("claim.json is whole"))
}

/// The `claim` object `status --json` reads.
fn claim_read(world: &World, run: &str) -> Value {
    let status = world.run(&["status", run, "--json"]);
    status.exited(0);
    status.json()["claim"].clone()
}

/// The planner surfaces of kind `claim-unconfirmed` the run raised.
fn unconfirmed_surfaces(world: &World, run: &str) -> Vec<Value> {
    world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter(|event| event["payload"]["kind"] == "claim-unconfirmed")
        .collect()
}

/// The records of `kind` naming `node` in the run's journal.
fn records_of(world: &World, run: &str, kind: &str, node: &str) -> Vec<Value> {
    world
        .journal(run)
        .into_iter()
        .filter(|event| event["kind"] == kind && event["labels"]["node"] == node)
        .collect()
}

/// The settlement `node` journalled in `run`, once it has.
fn settled(world: &World, run: &str, node: &str) -> Value {
    world.until(&format!("{node} to settle"), |world| {
        !records_of(world, run, "node-settled", node).is_empty()
    });
    records_of(world, run, "node-settled", node)[0]["payload"].clone()
}

/// A team world whose hosts read the claim every [`INTERVAL`] seconds and give a cancelled
/// dispatch one second, through the scripted store.
fn claim_world(name: &str) -> World {
    let world = World::new(name).with_env("ONEVCS_BRANCH_PREFIX", "me/");
    crate::draft_lifecycle::team_repository(&world);
    world.script("gh.checks", crate::draft_lifecycle::GREEN);
    world
        .with_env(CLAIM_INTERVAL_ENV, &INTERVAL.to_string())
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1")
        .through_scripted_source()
}

/// A store failure that is not the rate limit.
fn board_down() -> Value {
    json!({"kind": "unavailable", "message": "the board is down"})
}

/// The store's rate limit, naming no wait.
fn rate_limited() -> Value {
    json!({"kind": "rate-limited", "retry_after_seconds": null,
           "message": "API rate limit exceeded for installation"})
}

/// A rendezvous a background thread lets every arrival through, holding each for `hold`
/// while `holding` is set.
fn let_through(gate: Rendezvous, holding: Arc<AtomicBool>, hold: Duration) {
    std::thread::spawn(move || loop {
        let arrival = gate.arrived();
        if holding.load(Ordering::SeqCst) {
            std::thread::sleep(hold);
        }
        arrival.release();
    });
}

/// A short `watch` of `run` — waiting for it to settle, timing out after three seconds with a
/// heartbeat each — and the heartbeat and ending lines it wrote.
fn watch_lines(world: &World, run: &str) -> Vec<String> {
    let watched = world.run(&[
        "watch",
        run,
        "--until",
        "settled",
        "--timeout",
        "3",
        "--tick-interval",
        "1",
    ]);
    watched
        .stderr
        .lines()
        .filter(|line| line.starts_with("-- watch"))
        .map(str::to_owned)
        .collect()
}

/// The age a watch line's claim segment states — `30 s`, `12 min` — or nothing.
fn age_of(line: &str) -> String {
    line.split_once("CLAIM UNCONFIRMED ")
        .and_then(|(_, rest)| {
            let mut words = rest.split(' ');
            let count = words.next()?.parse::<u64>().ok()?;
            let unit = words
                .next()
                .filter(|unit| ["s", "min", "h", "d"].contains(unit))?;
            Some(format!("{count} {unit}"))
        })
        .unwrap_or_default()
}

/// How the store is kept from confirming the claim, and let go again.
enum Unconfirming {
    Failed,
    RateLimited,
    Late(Arc<AtomicBool>),
    TimedOut(Arc<AtomicBool>),
}

impl Unconfirming {
    fn reason(&self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::RateLimited => "rate-limited",
            Self::Late(_) | Self::TimedOut(_) => "late",
        }
    }

    fn phrase(&self) -> &'static str {
        match self {
            Self::Failed => "the claim read failed",
            Self::RateLimited => "the claim read was declined for the store's rate limit",
            Self::Late(_) | Self::TimedOut(_) => "the claim read was late",
        }
    }

    fn begin(&self, world: &World) {
        match self {
            Self::Failed => world.store_refuses("get_project", &board_down()),
            Self::RateLimited => world.store_refuses("get_project", &rate_limited()),
            Self::Late(holding) | Self::TimedOut(holding) => holding.store(true, Ordering::SeqCst),
        }
    }

    fn end(&self, world: &World) {
        match self {
            Self::Failed | Self::RateLimited => world.store_stops_refusing("get_project"),
            Self::Late(holding) | Self::TimedOut(holding) => holding.store(false, Ordering::SeqCst),
        }
    }
}

/// Wait until the run's `claim.json` reads `state`.
fn until_claim(world: &World, run: &str, state: &str) -> ClaimFile {
    world.until(&format!("the claim to read {state}"), |world| {
        claim_file(world, run).is_some_and(|file| {
            serde_json::to_value(file.reading.state).is_ok_and(|read| read == state)
        })
    });
    claim_file(world, run).expect("claim.json")
}

/// One driver with `x` in flight, `y` ready behind `w`, and lifecycle node `z`'s work
/// finished and its publication asked for, while `unconfirming` keeps every read of its
/// claim from confirming it for several lease windows; then the store answers again.
///
/// `run` is the run's id, kept short — two characters — so the watch lines it is read on
/// are measured against the screenshots' columns with room for the run id they carry twice.
fn an_unconfirmed_claim_keeps_its_work_and_starts_nothing(
    name: &str,
    run: &str,
    unconfirming: Unconfirming,
) {
    let base = claim_world(name);
    let world = match &unconfirming {
        Unconfirming::Late(holding) => {
            // llmlint: ignore-block[tests_mirror_real_usage] a read the store answers past its
            // deadline is a host's slowness no offline store produces on demand; the failpoint
            // only holds the real read of the real binary, which still answers.
            let late = Rendezvous::failpoint();
            let world = base.with_env(READ_LATE_GATE, &late.address());
            let_through(
                late,
                Arc::clone(holding),
                READ_DEADLINE + Duration::from_secs(1),
            );
            // llmlint: ignore-end[tests_mirror_real_usage]
            world
        }
        Unconfirming::TimedOut(holding) => {
            let request = base.rendezvous(&format!("{}.get_project", crate::harness::SCRIPTED_KEY));
            let_through(
                request,
                Arc::clone(holding),
                READ_DEADLINE + Duration::from_secs(1),
            );
            base
        }
        _ => base,
    };
    world.script("z.work", "z's change\n");
    let x = world.rendezvous("x");
    let z = world.rendezvous("z");
    let w = world.rendezvous("w");
    let mut plan = plan_of(
        run,
        vec![
            agent("x", &[]),
            lifecycle("z", &[]),
            agent("w", &[]),
            agent("y", &["w"]),
        ],
    );
    plan["concurrency"] = json!(4);
    let project = world.plan(run, &plan);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let in_flight = x.arrived();
    let finishing = z.arrived();
    let readying = w.arrived();
    let first = until_claim(&world, run, "confirmed");
    let reader = ClaimReader::start(&world, run);

    unconfirming.begin(&world);
    if matches!(unconfirming, Unconfirming::TimedOut(_)) {
        // Distinguish the store call's own timeout result from the lease watchdog's
        // observation that the read is still outstanding.
        world.until("the store's timeout result to reach claim.json", |world| {
            claim_file(world, run).is_some_and(|file| {
                file.reading.detail.as_deref().is_some_and(|detail| {
                    detail.contains("project show of")
                        && detail.contains(&format!(
                            "did not answer within {} seconds",
                            READ_DEADLINE.as_secs()
                        ))
                })
            })
        });
    }
    let unconfirmed = until_claim(&world, run, "unconfirmed");
    assert_eq!(
        unconfirmed.reading.reason.map(|reason| reason.as_str()),
        Some(unconfirming.reason()),
        "{unconfirmed:?}"
    );
    assert!(unconfirmed.reading.last_confirmed_at >= first.reading.last_confirmed_at);
    assert_eq!(unconfirmed.lease_interval_s, INTERVAL);
    // `z`'s work finishes and asks for its publication; `w` settles, so `y` is ready.
    finishing.release();
    readying.release();
    assert_eq!(settled(&world, run, "w")["status"], "done");

    // One surface for the episode, once it has outlasted the lease window.
    world.until("the claim-unconfirmed surface", |world| {
        !unconfirmed_surfaces(world, run).is_empty()
    });
    let raised = &unconfirmed_surfaces(&world, run)[0];
    let message = raised["payload"]["message"].as_str().expect("a message");
    assert!(message.contains(unconfirming.phrase()), "{message}");
    assert!(
        message.contains("no read has confirmed this driver's claim on its project for "),
        "{message}"
    );
    let since = claim_file(&world, run)
        .and_then(|file| file.reading.unconfirmed_since)
        .expect("an episode under way");
    let raised_at = crate::harness::epoch_millis(raised["ts"].as_str().expect("a time"));
    assert!(
        raised_at >= since + INTERVAL * 1_000,
        "the surface was raised inside the lease window"
    );

    // The episode goes on across more lease windows: still one surface, nothing started.
    let held_until = Instant::now() + Duration::from_secs(3 * INTERVAL);
    while Instant::now() < held_until {
        let reading = claim_read(&world, run);
        assert_eq!(reading["state"], "unconfirmed", "{reading}");
        std::thread::sleep(Duration::from_millis(500));
    }
    assert_eq!(unconfirmed_surfaces(&world, run).len(), 1);
    for kind in ["node-dispatched", "session-opened"] {
        assert!(
            records_of(&world, run, kind, "y").is_empty(),
            "y was started on an unconfirmed claim"
        );
    }
    assert!(
        records_of(&world, run, "change-opened", "z").is_empty(),
        "z's publication opened on an unconfirmed claim"
    );
    assert!(
        records_of(&world, run, "node-settled", "x").is_empty()
            && records_of(&world, run, "dispatch-cancelling", "x").is_empty(),
        "x was ended on an unconfirmed claim: {:#?}",
        world.journal(run)
    );

    // Every view says so, from the same file.
    let file = claim_file(&world, run).expect("claim.json");
    let reading = claim_read(&world, run);
    assert_eq!(reading["state"], "unconfirmed");
    assert_eq!(reading["reason"], unconfirming.reason());
    for (field, value) in [
        ("unconfirmed_since", file.reading.unconfirmed_since),
        ("last_confirmed_at", file.reading.last_confirmed_at),
    ] {
        assert_eq!(
            reading[field].as_str().map(crate::harness::epoch_millis),
            value,
            "{field}: {reading}"
        );
    }
    assert_eq!(
        reading["detail"].as_str(),
        file.reading.detail.as_deref(),
        "{reading}"
    );
    let status = world.run(&["status", run]);
    status.exited(0);
    assert!(
        status
            .stdout
            .contains("claim UNCONFIRMED: no read has confirmed this driver's claim for ")
            && status.stdout.contains(&format!("(lease {INTERVAL} s)"))
            && status.stdout.contains(unconfirming.phrase()),
        "{}",
        status.stdout
    );
    let watched = watch_lines(&world, run);
    let lines: Vec<&str> = watched.iter().map(String::as_str).collect();
    assert!(
        lines.iter().any(|line| line.starts_with("-- watching")),
        "{watched:?}"
    );
    assert!(
        lines.iter().any(|line| !line.starts_with("-- watching")),
        "{watched:?}"
    );
    for line in &lines {
        // The state and its age, and nothing more: the next field follows at once.
        assert!(
            !age_of(line).is_empty()
                && line.contains(&format!("  CLAIM UNCONFIRMED {}  ", age_of(line))),
            "a watch line without the claim's state and age alone: {line:?}"
        );
        // The segment adds at most 26 columns; under a short run id the heartbeat and the
        // ending line both keep within the columns the screenshots render at, segment and all.
        assert!(
            format!("  CLAIM UNCONFIRMED {}", age_of(line))
                .chars()
                .count()
                <= 26,
            "{line:?}"
        );
        assert!(line.chars().count() <= 118, "{line:?}");
    }

    // The store answers again: the next read confirms the claim, and the driver carries on.
    unconfirming.end(&world);
    let again = until_claim(&world, run, "confirmed");
    // Each line is otherwise the one the same watch prints for a confirmed claim: the heartbeat
    // and the ending line, read with the segment taken out and the cursor, which moves on,
    // set aside.
    let confirmed = watch_lines(&world, run);
    for kind in ["-- watching ", "-- watch "] {
        let pick = |of: &[String], strip: bool| -> Option<String> {
            of.iter()
                .find(|line| {
                    line.starts_with(kind)
                        && (kind == "-- watching " || !line.starts_with("-- watching"))
                })
                .map(|line| {
                    let line = if strip {
                        line.replace(&format!("  CLAIM UNCONFIRMED {}", age_of(line)), "")
                    } else {
                        line.clone()
                    };
                    line.split("  cursor ")
                        .next()
                        .unwrap_or_default()
                        .to_owned()
                })
        };
        let (was, now) = (pick(&watched, true), pick(&confirmed, false));
        assert!(was.is_some(), "no {kind:?} line: {watched:?}");
        assert_eq!(was, now, "{watched:?}\n{confirmed:?}");
        assert!(
            !now.unwrap_or_default().contains("CLAIM"),
            "a confirmed claim's line said something of it: {confirmed:?}"
        );
    }
    assert!(again.reading.last_confirmed_at > file.reading.last_confirmed_at);
    assert_eq!(claim_read(&world, run)["state"], "confirmed");
    world.until("y to be dispatched", |world| {
        !records_of(world, run, "node-dispatched", "y").is_empty()
    });
    world.until("z's publication to open", |world| {
        !records_of(world, run, "change-opened", "z").is_empty()
    });
    in_flight.release();
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(settled(&world, run, "x")["status"], "done");
    assert_eq!(settled(&world, run, "y")["status"], "done");
    assert_eq!(settled(&world, run, "z")["outcome"], "change-review-draft");
    assert_eq!(unconfirmed_surfaces(&world, run).len(), 1);
    reader.finished();
}

/// Every read of the claim the store fails, for several lease windows.
#[test]
fn a_claim_whose_reads_fail_keeps_its_dispatch_and_starts_nothing_until_a_read_confirms_it() {
    an_unconfirmed_claim_keeps_its_work_and_starts_nothing(
        "claim-failed",
        "cf",
        Unconfirming::Failed,
    );
}

/// Every read of the claim the store declines for its rate limit, for several lease windows.
#[test]
fn a_claim_whose_reads_are_rate_limited_keeps_its_dispatch_and_starts_nothing() {
    an_unconfirmed_claim_keeps_its_work_and_starts_nothing(
        "claim-limited",
        "cr",
        Unconfirming::RateLimited,
    );
}

/// Every read of the claim answering past its deadline, for several lease windows.
#[test]
fn a_claim_whose_reads_answer_late_keeps_its_dispatch_and_starts_nothing() {
    an_unconfirmed_claim_keeps_its_work_and_starts_nothing(
        "claim-late",
        "cl",
        Unconfirming::Late(Arc::new(AtomicBool::new(false))),
    );
}

/// A real store request times out, is classified late, and recovers when the store answers.
#[test]
fn a_claim_whose_store_requests_time_out_keeps_its_work_and_recovers() {
    an_unconfirmed_claim_keeps_its_work_and_starts_nothing(
        "claim-timeout",
        "ct",
        Unconfirming::TimedOut(Arc::new(AtomicBool::new(false))),
    );
}

/// One episode across three lease windows, a read that confirms the claim, then a second
/// episode outlasting one: two surfaces in all, one per episode.
#[test]
fn each_uninterrupted_unconfirmed_episode_raises_one_surface() {
    let world = claim_world("claim-episodes");
    let x = world.rendezvous("x");
    let run = "claim-episodes";
    let project = world.plan(run, &plan_of(run, vec![agent("x", &[])]));
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let in_flight = x.arrived();
    until_claim(&world, run, "confirmed");
    let reader = ClaimReader::start(&world, run);

    world.store_refuses("get_project", &board_down());
    let first = until_claim(&world, run, "unconfirmed");
    let since = first.reading.unconfirmed_since.expect("an episode");
    world.until("the first episode's surface", |world| {
        !unconfirmed_surfaces(world, run).is_empty()
    });
    // Three lease windows into the episode, and it is still the one surface.
    world.until("three lease windows", |world| {
        claim_file(world, run)
            .and_then(|file| file.reading.unconfirmed_for(crate::harness::now_millis()))
            .is_some_and(|age| age >= 3 * INTERVAL)
    });
    assert_eq!(
        claim_file(&world, run).and_then(|file| file.reading.unconfirmed_since),
        Some(since),
        "one uninterrupted episode began again"
    );
    assert_eq!(unconfirmed_surfaces(&world, run).len(), 1);

    world.store_stops_refusing("get_project");
    until_claim(&world, run, "confirmed");
    world.store_refuses("get_project", &board_down());
    let second = until_claim(&world, run, "unconfirmed");
    assert!(second.reading.unconfirmed_since > Some(since));
    world.until("the second episode's surface", |world| {
        unconfirmed_surfaces(world, run).len() == 2
    });
    world.store_stops_refusing("get_project");
    until_claim(&world, run, "confirmed");
    in_flight.release();
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(unconfirmed_surfaces(&world, run).len(), 2);
    assert_eq!(settled(&world, run, "x")["status"], "done");
    reader.finished();
    // The driver has gone: what its sidecar last said is no claim of a live driver.
    world.until("the driver to exit", |world| {
        !world.events_of(run, "driver-exited").is_empty()
    });
    assert!(
        claim_file(&world, run).is_some(),
        "the sidecar was taken away"
    );
    let status = world.run(&["status", run, "--json"]);
    status.exited(0);
    assert!(status.json().get("claim").is_none(), "{}", status.stdout);
}

/// A claim this driver's own had landed as, which a read then finds gone — somebody cleared
/// it — confirms nothing: the claim is unconfirmed, `failed`, the dispatch in flight runs on,
/// the node made ready starts nothing; and once the claim is back, a read confirms it and the
/// node starts.
#[test]
fn a_landed_claim_found_gone_is_unconfirmed_until_a_read_finds_it_again() {
    let world = claim_world("claim-gone");
    let x = world.rendezvous("x");
    let w = world.rendezvous("w");
    let run = "claim-gone";
    let mut plan = plan_of(
        run,
        vec![agent("x", &[]), agent("w", &[]), agent("y", &["w"])],
    );
    plan["concurrency"] = json!(3);
    let project = world.plan(run, &plan);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let in_flight = x.arrived();
    let readying = w.arrived();
    world.until("the claim to land", |world| {
        stored_claim(world, &project)["run"] == run
    });
    let landed = stored_claim(&world, &project);
    until_claim(&world, run, "confirmed");
    let reader = ClaimReader::start(&world, run);
    // llmlint: ignore-block[tests_mirror_real_usage] a claim cleared by somebody else is a
    // write outside this binary — a person at the store's own tools — so the journey makes it
    // through the store's own engine, as `continuation.rs`'s cleared-claim journeys do.
    let set_claim = |value: Value| {
        let key = onetaskgraph_plugin_api::MetadataKey::new("onepipeline.driver".to_owned())
            .expect("a key");
        let id: onetaskgraph_core::GlobalId = project.parse().expect("a qualified project");
        world
            .store_call(
                |engine| async move { engine.set_project_metadata(&id, &key, &value).await },
            )
            .unwrap_or_else(|why| panic!("the store refused the write: {why}"));
    };
    set_claim(Value::Null);
    // llmlint: ignore-end[tests_mirror_real_usage]
    let gone = until_claim(&world, run, "unconfirmed");
    assert_eq!(
        gone.reading.reason,
        Some(onepipeline::views::UnconfirmedReason::Failed)
    );
    assert!(
        gone.reading
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("found no claim on")),
        "{gone:?}"
    );
    readying.release();
    assert_eq!(settled(&world, run, "w")["status"], "done");
    std::thread::sleep(Duration::from_secs(2 * INTERVAL));
    assert!(records_of(&world, run, "node-dispatched", "y").is_empty());
    assert!(records_of(&world, run, "node-settled", "x").is_empty());

    set_claim(landed);
    until_claim(&world, run, "confirmed");
    world.until("y to be dispatched", |world| {
        !records_of(world, run, "node-dispatched", "y").is_empty()
    });
    in_flight.release();
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(settled(&world, run, "x")["status"], "done");
    reader.finished();
}

/// A reconcile pass held — at the start of another node — for several lease windows with a
/// dispatch in flight: the claim thread goes on reading once per interval, every read
/// confirms the claim, the dispatch runs on untouched, `status --json` reads the claim
/// confirmed throughout, and the journal gains nothing for any of those reads.
#[test]
fn a_held_pass_delays_no_claim_read_and_a_confirming_read_journals_nothing() {
    // llmlint: ignore-block[tests_mirror_real_usage] the property is the claim thread reading
    // while the reconcile loop cannot, and no user-facing input holds that loop — or marks a
    // read's start — at a chosen instant. The failpoints only order the real binary over the
    // real store and agent doubles, and every outcome is asserted on the binary's own records.
    let begins = Rendezvous::failpoint();
    let reads = Rendezvous::failpoint();
    let world = claim_world("claim-held-pass")
        .with_env(BEGIN_GATE, &begins.address())
        .with_env(BEGIN_NODE, "y")
        .with_env(READ_GATE, &reads.address());
    // llmlint: ignore-end[tests_mirror_real_usage]
    let x = world.rendezvous("x");
    let run = "claim-held-pass";
    // `y` waits on `z`, so its start — held — comes on a later pass, with `x` in flight.
    let mut plan = plan_of(
        run,
        vec![agent("x", &[]), agent("z", &[]), agent("y", &["z"])],
    );
    plan["concurrency"] = json!(3);
    let project = world.plan(run, &plan);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let in_flight = x.arrived();
    let loop_held = begins.arrived();
    let reader = ClaimReader::start(&world, run);
    let journal = world.journal(run).len();
    let asked = world.store_asked("get_project");

    // Several lease windows of reads while the pass is held, each counted at its failpoint.
    let windows: u64 = 4;
    let mut arrivals = Vec::new();
    let from = Instant::now();
    while from.elapsed() < Duration::from_secs(windows * INTERVAL) {
        let read = reads.arrived();
        arrivals.push(Instant::now());
        let previous = claim_file(&world, run)
            .expect("the preceding confirmation")
            .reading
            .last_confirmed_at;
        read.release();
        world.until("the read to be written down", |world| {
            claim_file(world, run).is_some_and(|file| file.reading.last_confirmed_at > previous)
        });
        let file = claim_file(&world, run).expect("claim.json");
        assert_eq!(
            serde_json::to_value(file.reading.state).expect("a state"),
            "confirmed",
            "a read inside a held pass did not confirm the claim: {file:?}"
        );
        assert_eq!(claim_read(&world, run)["state"], "confirmed");
    }
    assert!(arrivals.len() as u64 >= windows - 1, "{arrivals:?}");
    for pair in arrivals.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap >= Duration::from_secs(INTERVAL) && gap < Duration::from_secs(2 * INTERVAL),
            "not one read per interval: reads {gap:?} apart"
        );
    }
    assert_eq!(
        world.store_asked("get_project") - asked,
        arrivals.len(),
        "the store was asked for the claim other than once per read"
    );
    assert_eq!(
        world.journal(run).len(),
        journal,
        "a confirming read wrote to the journal"
    );
    assert!(records_of(&world, run, "node-settled", "x").is_empty());
    reader.finished();

    // An unreadable sidecar says nothing is known. Hold the next read so it cannot
    // repair the file while a separate status process exercises each refusal.
    let read = reads.arrived();
    let valid =
        serde_json::to_value(claim_file(&world, run).expect("the confirming read")).unwrap();
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("detail");
    let mut unknown = valid.clone();
    unknown["typo"] = json!(true);
    let mut interval = valid.clone();
    interval["lease_interval_s"] = json!(0);
    let mut version = valid.clone();
    version["schema_version"] = json!(2);
    let mut inconsistent = valid.clone();
    inconsistent["reason"] = json!("failed");
    for (case, document) in [
        ("truncated", r#"{"schema_version":1,"state":"#.to_owned()),
        ("missing key", missing.to_string()),
        ("unknown key", unknown.to_string()),
        ("zero interval", interval.to_string()),
        ("future version", version.to_string()),
        ("inconsistent state", inconsistent.to_string()),
    ] {
        // llmlint: ignore-block[tests_mirror_real_usage] the driver atomically writes a valid
        // sidecar, so malformed documents require disk damage or another build. Write those
        // external inputs directly, then ask the real status process how it reads them.
        std::fs::write(world.run_file(run, "claim.json"), document).expect("the sidecar");
        // llmlint: ignore-end[tests_mirror_real_usage]
        let status = world.run(&["status", run, "--json"]);
        status.exited(0);
        assert!(
            status.json().get("claim").is_none(),
            "{case}: {}",
            status.stdout
        );
        let status = world.run(&["status", run]);
        status.exited(0);
        assert!(
            !status.stdout.contains("claim UNCONFIRMED"),
            "{case}: {}",
            status.stdout
        );
    }
    read.release();
    world.until("the next read to write the sidecar whole", |world| {
        std::fs::read_to_string(world.run_file(run, "claim.json"))
            .ok()
            .and_then(|text| ClaimFile::parse(&text).ok())
            .is_some()
    });

    loop_held.release();
    std::thread::spawn(move || loop {
        begins.arrived().release();
    });
    std::thread::spawn(move || loop {
        reads.arrived().release();
    });
    in_flight.release();
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(settled(&world, run, "x")["status"], "done");
    assert_eq!(settled(&world, run, "y")["status"], "done");
    assert!(unconfirmed_surfaces(&world, run).is_empty());
}

/// A read that finds another driver's claim — a host's `start --take-over` — ends this
/// driver's dispatch in flight: asked to stop, settled `cancelled` with its branch kept. It
/// supersedes, acknowledges, and the adopter dispatches the node again.
#[test]
fn a_read_finding_another_drivers_claim_ends_the_dispatch_in_flight_and_supersedes() {
    let base = claim_world("claim-superseded");
    let successor = base.on_host("host-b");
    let world = base;
    let x = world.rendezvous("x");
    let run = "claim-superseded";
    let project = world.plan(run, &plan_of(run, vec![agent("x", &[])]));
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    until_claim(&world, run, "confirmed");
    let taking = take_over(&successor, &project);
    world.until("the takeover's claim to land", |world| {
        stored_claim(world, &project)["host"] == "host-b"
    });
    world.until("the driver to end", |world| {
        world.run_file(run, "result.json").is_file()
    });
    drop(mine);
    assert_eq!(settled(&world, run, "x")["status"], "cancelled");
    let superseded = world.events_of(run, "driver-superseded");
    assert_eq!(superseded.len(), 1, "{superseded:?}");
    assert_eq!(superseded[0]["payload"]["by"]["host"], "host-b");
    assert_eq!(
        claim_file(&world, run).map(|file| file.reading.state),
        Some(onepipeline::views::ClaimState::Superseded)
    );
    world.until_run_file_holds(run, "driver.log", "ends its dispatches in progress — 'x'");
    let said = taken_over(taking);
    assert!(said.contains("acknowledged the takeover"), "{said}");
    x.arrived().release();
    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(settled(&successor, run, "x")["status"], "done");
}

/// `host` taking `project` over, in the background.
fn take_over(host: &World, project: &str) -> std::process::Child {
    let mut command = host.cmd(&["start", project, "--detach", "--take-over"]);
    command
        .current_dir(&host.project)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command.spawn().expect("the takeover starts")
}

/// A background takeover's answer: it launched, and what it said on its way.
fn taken_over(taking: std::process::Child) -> String {
    let answered = taking.wait_with_output().expect("the takeover answers");
    let said = String::from_utf8_lossy(&answered.stderr).into_owned();
    assert_eq!(
        answered.status.code(),
        Some(0),
        "the takeover failed:\n{said}"
    );
    said
}

/// A superseded driver whose acknowledgement and fence clear the board both refuses stays,
/// owing them, and tries both again on the fence's retry — it neither ends nor leaves its
/// adopter on a fence nothing will clear — and lets go once one lands, which the adopter
/// starts on.
#[test]
fn a_superseded_driver_whose_handover_writes_fail_stays_and_retries_until_one_lands() {
    let base = claim_world("claim-handover-refused");
    let successor = base.on_host("host-b");
    let world = base;
    let x = world.rendezvous("x");
    let run = "claim-handover-refused";
    let project = world.plan(run, &plan_of(run, vec![agent("x", &[])]));
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    until_claim(&world, run, "confirmed");
    let taking = take_over(&successor, &project);
    world.until("the takeover's claim to land", |world| {
        stored_claim(world, &project)["host"] == "host-b"
    });
    world.store_refuses("set_project_metadata", &board_down());
    world.until("the displaced driver to retry its handover", |world| {
        std::fs::read_to_string(world.run_file(run, "driver.log")).is_ok_and(|log| {
            log.matches("could not acknowledge run").count() >= 2
                && log.contains("could not write its dispatches and publications under way")
        })
    });
    drop(mine);
    assert_eq!(settled(&world, run, "x")["status"], "cancelled");
    assert!(
        !world.run_file(run, "result.json").is_file(),
        "the displaced driver let go owing its adopter both writes"
    );
    world.store_stops_refusing("set_project_metadata");
    let said = taken_over(taking);
    assert!(said.contains("acknowledged the takeover"), "{said}");
    world.until("the displaced driver to let go", |world| {
        world.run_file(run, "result.json").is_file()
    });
    x.arrived().release();
    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(settled(&successor, run, "x")["status"], "done");
}

/// This host's name, as the binary reads it when nothing names it.
#[cfg(unix)]
fn this_host() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| "localhost".to_owned())
}

/// A takeover on the driver's own host, from a runs root that does not hold its run, once the
/// claim's pid has died there, waits on nothing — no claim written first, no wait said — and
/// dispatches the node that was in flight onto the branch its record names.
///
/// The same takeover over a pid still live is the wait every other takeover journey drives
/// across hosts: a live driver under another runs root of this host would mint the same run
/// id here and read this takeover's claim as its own, so it is not arranged on one host.
#[cfg(unix)]
#[test]
fn a_takeover_on_the_drivers_host_once_its_pid_has_died_waits_on_nothing() {
    let run = "claim-host-dead";
    let world = claim_world(run);
    world.script("x.work", "x's change\n");
    let elsewhere = world.on_host(&this_host());
    let x = world.rendezvous("x");
    let project = world.plan(run, &plan_of(run, vec![lifecycle("x", &[])]));
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    let branch = recorded_branch(&world, &project, "x");
    assert_eq!(stored_claim(&world, &project)["host"], this_host());
    let pid = world.run_json(run, "launch.json")["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .expect("the driver's pid");
    crate::harness::end_process(pid);
    let said = taken_over(take_over(&elsewhere, &project));
    drop(mine);
    assert!(!said.contains("--take-over claimed"), "{said}");
    assert!(!said.contains("has not acknowledged"), "{said}");
    x.arrived().release();
    elsewhere.until("the takeover to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(sessions_opened_after(&elsewhere, run, 0), vec![branch]);
    assert!(records_of(&elsewhere, run, "node-held", "x").is_empty());
}

/// The node ids `run` dispatched, each with when.
fn dispatches(world: &World, run: &str) -> Vec<(String, String)> {
    world
        .events_of(run, "node-dispatched")
        .into_iter()
        .filter_map(|event| {
            Some((
                event["labels"]["node"].as_str()?.to_owned(),
                event["ts"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// The project's `onepipeline.driver`, as the store holds it.
fn stored_claim(world: &World, project: &str) -> Value {
    world.store_project(project)["items"][0]["item"]["metadata"]
        .get("onepipeline.driver")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The metadata the board's item for `node` carries.
fn item_metadata(world: &World, project: &str, node: &str) -> Value {
    world
        .store_tasks(project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
        .map(|task| task["item"]["metadata"].clone())
        .unwrap_or_else(|| panic!("the board holds no item for {node}"))
}

/// The branch the board's record of `node`'s dispatch names, once it names one.
fn recorded_branch(world: &World, project: &str, node: &str) -> String {
    world.until(&format!("{node}'s dispatch to reach the board"), |world| {
        item_metadata(world, project, node)
            .get("onepipeline.dispatch")
            .is_some()
    });
    item_metadata(world, project, node)["onepipeline.dispatch"]["branch"]
        .as_str()
        .expect("a recorded branch")
        .to_owned()
}

/// The branches `run`'s dispatches opened sessions on after `after`, the journal index from
/// which to read.
fn sessions_opened_after(world: &World, run: &str, after: usize) -> Vec<String> {
    world
        .journal(run)
        .into_iter()
        .skip(after)
        .filter(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
        .filter_map(|event| event["payload"]["branch"].as_str().map(str::to_owned))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Two drivers on one project: the first alive with `x` in flight but unable to read its
/// claim — every read held — when the second takes the project over. The second starts
/// nothing while the first's fence names `x`, however long past its wait; once the first
/// can read again it finds the takeover, ends `x`, and its settlement reaches the store; only
/// then does the second dispatch `x`, onto the branch the first left it on.
#[test]
fn an_adopter_dispatches_no_node_a_driver_unable_to_read_its_claim_still_runs() {
    let base = claim_world("claim-two-drivers");
    base.script("x.work", "x's change\n");
    let successor = base.on_host("host-b");
    // llmlint: ignore-block[tests_mirror_real_usage] a driver alive but unable to read its claim
    // is a store that answers one host and not another, which one offline store cannot be; the
    // failpoint holds the first driver's real reads, and the second driver reads freely.
    let reads = Rendezvous::failpoint();
    let world = base.with_env(READ_GATE, &reads.address());
    // llmlint: ignore-end[tests_mirror_real_usage]
    let x = world.rendezvous("x");
    let run = "claim-two-drivers";
    let project = world.plan(run, &plan_of(run, vec![lifecycle("x", &[])]));
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let first = x.arrived();
    world.until("the first driver's claim to land", |world| {
        stored_claim(world, &project)["run"] == run
    });
    let branch = recorded_branch(&world, &project, "x");
    // Its next read is held: it cannot read its claim, so it is unconfirmed — late.
    let held = reads.arrived();
    let unconfirmed = until_claim(&world, run, "unconfirmed");
    assert_eq!(
        unconfirmed.reading.reason,
        Some(onepipeline::views::UnconfirmedReason::Late)
    );
    let reader = ClaimReader::start(&world, run);

    let mut taking = successor.cmd(&["start", &project, "--detach", "--take-over"]);
    taking
        .current_dir(&successor.project)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut taking = taking.spawn().expect("the takeover starts");
    world.until("the takeover's claim to land", |world| {
        stored_claim(world, &project)["host"] == "host-b"
    });
    // Past the whole wait a claim's terms give — two intervals, a read's deadline, the
    // cancel grace and the slack — the first driver's fence still names `x`, so the second
    // has started nothing, and `x` runs on in the first.
    std::thread::sleep(Duration::from_secs(
        2 * INTERVAL + 20 + 1 + 10 + 2 * INTERVAL,
    ));
    assert!(
        taking.try_wait().expect("the takeover is asked").is_none(),
        "the takeover started while the displaced driver's dispatch could still be running"
    );
    assert!(records_of(&world, run, "node-settled", "x").is_empty());
    assert!(
        successor.events_of(run, "node-dispatched").is_empty(),
        "the second driver dispatched beside the first"
    );

    // The first reads again, finds the takeover, and ends `x`; its settlement is on the store
    // before the second starts.
    held.release();
    std::thread::spawn(move || loop {
        reads.arrived().release();
    });
    let answered = taking.wait_with_output().expect("the takeover answers");
    let said = String::from_utf8_lossy(&answered.stderr).into_owned();
    assert_eq!(answered.status.code(), Some(0), "{said}");
    assert!(said.contains("acknowledged the takeover"), "{said}");
    assert_eq!(settled(&world, run, "x")["status"], "cancelled");
    assert_eq!(world.events_of(run, "driver-superseded").len(), 1);
    drop(first);
    let theirs = x.arrived();
    theirs.release();
    successor.until("the second driver to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(sessions_opened_after(&successor, run, 0), vec![branch]);
    let ended = records_of(&world, run, "node-settled", "x")[0]["ts"]
        .as_str()
        .expect("a time")
        .to_owned();
    let mut both = dispatches(&world, run);
    both.extend(dispatches(&successor, run));
    for (node, at) in dispatches(&successor, run) {
        assert!(
            at >= ended,
            "the second driver dispatched {node} at {at}, before the first's ended at {ended}"
        );
    }
    assert_eq!(
        both.iter().filter(|(node, _)| node == "x").count(),
        2,
        "{both:?}"
    );
    reader.finished();
}

/// A run adopted after the driver it had was put down — `put_down` — waits on nothing: the
/// adopting driver journals no hold on the one it replaces, and dispatches the node that
/// was in flight onto the branch its record names.
fn an_adoption_after_its_driver_is_gone_waits_on_nothing(
    name: &str,
    put_down: fn(&World, &str, &str),
) {
    let world = claim_world(name);
    world.script("x.work", "x's change\n");
    let x = world.rendezvous("x");
    let run = name;
    let project = world.plan(run, &plan_of(run, vec![lifecycle("x", &[])]));
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let first = x.arrived();
    let branch = recorded_branch(&world, &project, "x");
    put_down(&world, run, &project);
    drop(first);
    let before = world.journal(run).len();
    let adopted = world.run(&["adopt", run, "--detach"]);
    adopted.exited(0);
    let again = x.arrived();
    again.release();
    world.until("the adopted run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(sessions_opened_after(&world, run, before), vec![branch]);
    assert!(
        records_of(&world, run, "node-held", "x").is_empty(),
        "the adoption held x on the driver it replaced"
    );
    let log = std::fs::read_to_string(world.run_file(run, "driver.log")).unwrap_or_default();
    for waiting in [
        "starts once",
        "has not acknowledged",
        "could not confirm its claim",
    ] {
        assert!(!log.contains(waiting), "{log}");
    }
    assert_eq!(settled(&world, run, "x")["outcome"], "change-review-draft");
    assert_eq!(
        claim_file(&world, run).map(|file| file.reading.state),
        Some(onepipeline::views::ClaimState::Confirmed)
    );
}

/// A host shutdown put the driver down; `adopt` picks the run up.
#[test]
fn a_run_adopted_after_a_host_shutdown_waits_on_nothing_and_resumes_on_the_recorded_branch() {
    an_adoption_after_its_driver_is_gone_waits_on_nothing(
        "claim-shutdown",
        |world, run, project| {
            // A board can still fence a node from an earlier plan whose settlement
            // has not reached it. Shutdown may release only the dispatch it stops.
            // llmlint: ignore-block[tests_mirror_real_usage] the boundary under test is
            // shutdown preserving an outstanding fence from an older plan. The current
            // driver can fence only its own plan's nodes, so it cannot produce this
            // foreign entry through dispatch. Seed it through the real store API;
            // the real shutdown, store readback and adoption below prove its handling.
            let mut before = world.store_project(project)["items"][0]["item"]["metadata"]
                ["onepipeline.publishing"]
                .clone();
            assert_eq!(before["nodes"], json!(["x"]));
            before["nodes"] = json!(["x", "earlier-plan-node"]);
            let key =
                onetaskgraph_plugin_api::MetadataKey::new("onepipeline.publishing".to_owned())
                    .expect("a key");
            let id: onetaskgraph_core::GlobalId = project.parse().expect("a qualified project");
            world
                .store_call(|engine| async move {
                    engine.set_project_metadata(&id, &key, &before).await
                })
                .expect("the board's outstanding fence is recorded");
            // llmlint: ignore-end[tests_mirror_real_usage]
            let shutdown = world.run(&["shutdown", run, "--grace", "1"]);
            assert!([0, 2].contains(&shutdown.code), "{}", shutdown.stderr);
            assert!(!world.events_of(run, "host-shutdown").is_empty());
            // The dispatch it stopped is recorded stopped on the store: the fence names it no more.
            let fence = world.store_project(project)["items"][0]["item"]["metadata"]
                .get("onepipeline.publishing")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                fence["nodes"],
                json!(["earlier-plan-node"]),
                "shutdown must release x while retaining the board's other fenced node: {fence}"
            );
        },
    );
}

/// The driver's process died on this host; `adopt` picks the run up.
#[cfg(unix)]
#[test]
fn a_run_adopted_after_its_drivers_pid_died_waits_on_nothing_and_resumes_on_the_recorded_branch() {
    an_adoption_after_its_driver_is_gone_waits_on_nothing("claim-pid-died", |world, run, _| {
        let pid = world.run_json(run, "launch.json")["pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .expect("the driver's pid");
        crate::harness::end_process(pid);
        assert!(world.events_of(run, "host-shutdown").is_empty());
    });
}

/// Epoch milliseconds as the RFC 3339 instant the binary stamps.
fn rfc3339(millis: u64) -> String {
    let (secs, ms) = (millis / 1_000, millis % 1_000);
    let days = i64::try_from(secs / 86_400).expect("a day count");
    let of_day = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        of_day / 3_600,
        of_day % 3_600 / 60,
        of_day % 60
    )
}

/// A watch of a live driver whose claim has been unconfirmed for two hours and longer still
/// prints the segment as the state and its age alone, adding at most 26 columns to each
/// heartbeat and ending line, which are otherwise the lines it prints for a confirmed claim.
///
/// Nobody waits two hours in a journey: the driver reads its claim once a minute here, so
/// between reads the journey writes the sidecar it would have written that far into an
/// episode, and the real `watch` reads it back.
#[test]
fn a_watch_over_an_hours_long_unconfirmed_claim_keeps_the_segment_within_its_columns() {
    let run = "ch";
    let world = World::new("claim-hours")
        .through_scripted_source()
        .with_env(CLAIM_INTERVAL_ENV, "60");
    let x = world.rendezvous("x");
    let project = world.plan(run, &plan_of(run, vec![agent("x", &[])]));
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let in_flight = x.arrived();
    let launched = until_claim(&world, run, "confirmed");
    assert_eq!(launched.lease_interval_s, 60);
    let confirmed_at = launched
        .reading
        .last_confirmed_at
        .expect("a confirming read");
    let confirmed = watch_lines(&world, run);
    let minute = 60_000;
    let hour = 60 * minute;
    for age in [
        2 * hour,
        2 * hour + 59 * minute,
        25 * hour,
        100 * hour,
        400 * 24 * hour,
    ] {
        let last = confirmed_at - age;
        // llmlint: ignore-block[tests_mirror_real_usage] an episode hours or days long is
        // what the driver writes only that far into one, which no journey can wait out; the
        // sidecar it would hold then is written between its reads — once a minute here — in
        // its declared shape, and read back by the real `watch`.
        let sidecar = json!({
            "schema_version": 1,
            "state": "unconfirmed",
            "last_confirmed_at": rfc3339(last),
            "unconfirmed_since": rfc3339(last + 60_000),
            "reason": "failed",
            "detail": "the board is down",
            "lease_interval_s": 60,
        })
        .to_string();
        ClaimFile::parse(&sidecar).expect("the sidecar is the declared shape");
        std::fs::write(world.run_file(run, "claim.json"), &sidecar).expect("the sidecar");
        // llmlint: ignore-end[tests_mirror_real_usage]
        let lines = watch_lines(&world, run);
        assert_eq!(
            std::fs::read_to_string(world.run_file(run, "claim.json")).expect("claim.json"),
            sidecar,
            "the driver rewrote its claim while the watch read it"
        );
        let status = world.run(&["status", run]);
        status.exited(0);
        let hours = age / hour;
        assert!(
            status.stdout.contains(&format!("for {hours} h ")),
            "status no longer says the age in full: {}",
            status.stdout
        );
        assert!(
            lines.iter().any(|line| line.starts_with("-- watching "))
                && lines.iter().any(|line| !line.starts_with("-- watching ")),
            "{lines:?}"
        );
        for line in &lines {
            let age_said = age_of(line);
            let segment = format!("  CLAIM UNCONFIRMED {age_said}");
            assert!(
                !age_said.is_empty() && line.contains(&format!("{segment}  ")),
                "{age}ms: {line:?}"
            );
            assert!(
                segment.chars().count() <= 26,
                "{age}ms: the segment is {} columns: {line:?}",
                segment.chars().count()
            );
        }
        for kind in ["-- watching ", "-- watch "] {
            let pick = |of: &[String], strip: bool| -> Option<String> {
                of.iter()
                    .find(|line| {
                        line.starts_with(kind)
                            && (kind == "-- watching " || !line.starts_with("-- watching"))
                    })
                    .map(|line| {
                        let line = if strip {
                            line.replace(&format!("  CLAIM UNCONFIRMED {}", age_of(line)), "")
                        } else {
                            line.clone()
                        };
                        line.split("  cursor ")
                            .next()
                            .unwrap_or_default()
                            .to_owned()
                    })
            };
            assert_eq!(pick(&lines, true), pick(&confirmed, false), "{lines:?}");
        }
    }
    in_flight.release();
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
}
