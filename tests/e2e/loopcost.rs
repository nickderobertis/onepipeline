//! What the reconcile loop costs, and how fast it still answers.
//!
//! A live driver was measured at 01:39:24 of CPU over 9,007 seconds on a run
//! with **one** node in flight: forty passes a second, each re-reading another
//! run's ledger, refolding this run's journal and handing the board two identical
//! snapshots. None of that is visible in a journal — the whole point is that
//! nothing was happening — and none of it is measurable from outside the process,
//! because what it costs a host is CPU and a loaded machine hands that out as it
//! likes. So the driver counts its own work when it is asked to, and these
//! journeys read the counts off real run stores.
//!
//! Every bound here is stated as work done rather than as time taken, so a
//! loaded host cannot fail correct work — and every one of them is a bound the
//! tree before this change would not have met.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is the compiled binary,
// driven as a subprocess over real run stores, and the counts are the real loop's own.
// Only `oneagentgraph` is substituted, at its own subprocess boundary, so a journey can
// hold a dispatch open without paying for a model turn — the same seam and rationale
// `harness.rs` carries.

use std::time::{Duration, Instant};

use crate::harness::{agent, human, plan_of, World};
use serde_json::{json, Value};

/// The interval every idle claim here is measured over.
///
/// A minute, because that is what the bound is stated over: sixty seconds in
/// which the run records nothing. The tree before this change performed about
/// 2,400 passes in it.
const IDLE: Duration = Duration::from_secs(60);

/// What one driver's reconcile loop did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts {
    passes: u64,
    statuses: u64,
    publications: u64,
    upstream_reads: u64,
    release_asks: u64,
    store_bytes: u64,
}

impl Counts {
    /// What it did between two readings.
    fn since(self, before: Self) -> Self {
        Self {
            passes: self.passes - before.passes,
            statuses: self.statuses - before.statuses,
            publications: self.publications - before.publications,
            upstream_reads: self.upstream_reads - before.upstream_reads,
            release_asks: self.release_asks - before.release_asks,
            store_bytes: self.store_bytes - before.store_bytes,
        }
    }
}

/// A world whose drivers report what their loops did.
fn measured(name: &str) -> World {
    World::new(name).with_env("ONEPIPELINE_LOOP_STATS", "1")
}

/// One driver's counts, read out of its own run directory.
fn counts(world: &World, run: &str) -> Counts {
    let path = world.run_file(run, "loop-stats.json");
    let read =
        || -> Option<Value> { serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok() };
    // Written atomically, so a partial read is not a thing that happens — an
    // absent one is, on the first look before the driver's first wait.
    let value = read().unwrap_or_else(|| {
        panic!(
            "{run} reported no loop counts at {}\n{}",
            path.display(),
            world.dump()
        )
    });
    let count = |name: &str| {
        value[name]
            .as_u64()
            .unwrap_or_else(|| panic!("{run}'s counts carry no {name}: {value}"))
    };
    Counts {
        passes: count("passes"),
        statuses: count("statuses"),
        publications: count("publications"),
        upstream_reads: count("upstream_reads"),
        release_asks: count("release_asks"),
        store_bytes: count("store_bytes"),
    }
}

/// Wait until a driver has reported anything at all, which it does on its first
/// wait.
fn reporting(world: &World, run: &str) {
    world.until("the driver to report its loop counts", |world| {
        world.run_file(run, "loop-stats.json").is_file()
    });
}

/// The records a run wrote that change what the graph is: what "one per recorded
/// state change" is counted against.
fn state_changes(world: &World, run: &str) -> usize {
    world
        .journal(run)
        .into_iter()
        .filter(|event| event["source"] == "pipeline")
        .filter(|event| {
            matches!(
                event["kind"].as_str().unwrap_or_default(),
                "node-settled" | "node-dispatched" | "edit-committed" | "release-adopted"
            )
        })
        .count()
}

/// Whether a run has dispatched, or settled, one node.
fn recorded(world: &World, run: &str, kind: &str, node: &str) -> bool {
    world
        .events_of(run, kind)
        .iter()
        .any(|event| event["labels"]["node"] == node)
}

/// When one record was written, in milliseconds.
///
/// The envelope's own timestamp, which is millisecond-precision UTC — so a
/// latency between two records the loop wrote is measured off what the run
/// recorded rather than off what this test process happened to observe.
fn at(event: &Value) -> u64 {
    let ts = event["ts"]
        .as_str()
        .unwrap_or_else(|| panic!("no ts: {event}"));
    let (date, time) = ts
        .trim_end_matches('Z')
        .split_once('T')
        .unwrap_or_else(|| panic!("not an RFC 3339 timestamp: {ts}"));
    let number = |text: &str| -> u64 {
        text.parse()
            .unwrap_or_else(|e| panic!("{text} of {ts} is not a number: {e}"))
    };
    let day: Vec<&str> = date.split('-').collect();
    let clock: Vec<&str> = time.split(':').collect();
    let (second, millis) = clock[2].split_once('.').unwrap_or((clock[2], "0"));
    // Days since an arbitrary fixed point, which is all a difference needs.
    let days = number(day[0]) * 372 + number(day[1]) * 31 + number(day[2]);
    ((days * 24 + number(clock[0])) * 60 + number(clock[1])) * 60_000
        + number(second) * 1_000
        + number(millis)
}

/// The one record of this kind for this node, for a latency measured off two.
fn one(world: &World, run: &str, kind: &str, node: &str) -> Value {
    let found: Vec<Value> = world
        .events_of(run, kind)
        .into_iter()
        .filter(|event| event["labels"]["node"] == node)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "{run} recorded {kind} for {node}: {found:?}"
    );
    found.into_iter().next().expect("one record")
}

/// A converged run with one node in flight does no scheduling work at all while
/// it records nothing.
///
/// Zero derivations of the graph's statuses, zero write-back publications, zero
/// reads out of the run store, zero asks about a release and zero reads of
/// another run's ledger — over a minute in which the run wrote not one record.
/// The tree before this change performed roughly 2,400 passes in that minute,
/// each folding the journal four times over.
#[test]
fn a_converged_run_does_no_scheduling_work_while_it_records_nothing() {
    let world = measured("loopcost-idle");
    world.script("hold.wait", "hold");
    let plan = world.plan("idle", &plan_of("idle", vec![agent("hold", &[])]));
    world.run(&["start", &plan, "--detach"]).exited(0);
    world.until("the dispatch to start", |world| {
        recorded(world, "idle", "node-dispatched", "hold")
    });
    reporting(&world, "idle");
    // The launch's own records are behind us before the window opens.
    std::thread::sleep(Duration::from_secs(2));

    let wrote = world.journal("idle").len();
    let before = counts(&world, "idle");
    std::thread::sleep(IDLE);
    let did = counts(&world, "idle").since(before);

    assert_eq!(
        world.journal("idle").len(),
        wrote,
        "the run recorded something inside the window this claim is about"
    );
    assert_eq!(
        did.statuses, 0,
        "the graph's statuses were re-derived: {did:?}"
    );
    assert_eq!(did.publications, 0, "the board was re-published: {did:?}");
    assert_eq!(did.store_bytes, 0, "the run store was read: {did:?}");
    assert_eq!(
        did.upstream_reads, 0,
        "a run with no cross-DAG dependency read another run's ledger: {did:?}"
    );
    assert_eq!(
        did.release_asks, 0,
        "a run with nothing awaiting a release asked about one: {did:?}"
    );
    // And the ceiling on how often a pass can happen at all, whatever it costs.
    assert!(
        did.passes <= IDLE.as_secs(),
        "a converged driver ran more than one scheduling pass a second: {did:?}"
    );

    world.release("hold.go");
    world.until("the run to settle", |world| {
        world.run_file("idle", "result.json").is_file()
    });
}

/// What a converged idle pass costs does not grow with the run.
///
/// Two converged runs two orders of magnitude apart in nodes, and more than one
/// in journal length, read the same amount out of the store over the same
/// interval — because neither reads anything at all. The tree before this change
/// refolded the whole journal on every pass, so the larger run's driver read
/// hundreds of times what the smaller one's did.
#[test]
fn an_idle_pass_does_not_grow_with_the_run_it_is_idling_on() {
    let world = measured("loopcost-scale");
    world.script("hold.wait", "hold");
    let small = world.plan("small", &plan_of("small", vec![agent("hold", &[])]));

    let mut many: Vec<Value> = (0..99).map(|n| agent(&format!("n{n}"), &[])).collect();
    many.push(agent("hold", &[]));
    let large = world.plan("large", &plan_of("large", many));

    world.run(&["start", &small, "--detach"]).exited(0);
    world.run(&["start", &large, "--detach"]).exited(0);
    for run in ["small", "large"] {
        world.until("both dispatches to start", |world| {
            recorded(world, run, "node-dispatched", "hold")
        });
        reporting(&world, run);
    }
    world.until("the large run's other nodes to settle", |world| {
        world
            .events_of("large", "node-settled")
            .iter()
            .filter(|event| event["labels"]["node"] != "hold")
            .count()
            == 99
    });
    std::thread::sleep(Duration::from_secs(2));

    let sizes: Vec<usize> = ["small", "large"]
        .iter()
        .map(|run| world.journal(run).len())
        .collect();
    assert!(
        sizes[1] > sizes[0] * 10,
        "the two runs are not orders of magnitude apart: {sizes:?}"
    );

    let before: Vec<Counts> = ["small", "large"]
        .iter()
        .map(|run| counts(&world, run))
        .collect();
    std::thread::sleep(IDLE);
    let did: Vec<Counts> = ["small", "large"]
        .iter()
        .enumerate()
        .map(|(nth, run)| counts(&world, run).since(before[nth]))
        .collect();

    // Within a constant factor of one another, rather than in proportion to the
    // runs. Both are nought, which is inside any factor there is.
    assert!(
        did[1].store_bytes <= 8 * (did[0].store_bytes + 1),
        "what an idle pass reads grew with the run: {did:?}"
    );
    for (nth, run) in ["small", "large"].iter().enumerate() {
        assert_eq!(
            did[nth].store_bytes, 0,
            "{run} read its store idle: {did:?}"
        );
        assert_eq!(
            did[nth].statuses, 0,
            "{run} re-derived its statuses: {did:?}"
        );
    }

    world.release("hold.go");
    for run in ["small", "large"] {
        world.until("the run to settle", |world| {
            world.run_file(run, "result.json").is_file()
        });
    }
}

/// The two things a pass does about whole state are paid once per recorded state
/// change, not once per pass.
#[test]
fn the_board_and_the_frontier_are_recomputed_once_per_recorded_state_change() {
    let world = measured("loopcost-changes");
    for node in ["hold", "a", "b", "c"] {
        world.script(&format!("{node}.wait"), "hold");
    }
    let plan = world.plan(
        "changes",
        &plan_of(
            "changes",
            vec![
                agent("hold", &[]),
                agent("a", &[]),
                agent("b", &["a"]),
                agent("c", &["b"]),
            ],
        ),
    );
    world.run(&["start", &plan, "--detach"]).exited(0);
    world.until("the chain to start", |world| {
        recorded(world, "changes", "node-dispatched", "a")
    });
    reporting(&world, "changes");
    std::thread::sleep(Duration::from_secs(1));

    let before = counts(&world, "changes");
    let changed_before = state_changes(&world, "changes");
    for node in ["a", "b", "c"] {
        world.release(&format!("{node}.go"));
        world.until("the chain to advance", |world| {
            recorded(world, "changes", "node-settled", node)
        });
    }
    std::thread::sleep(Duration::from_secs(1));
    let did = counts(&world, "changes").since(before);
    let changes = (state_changes(&world, "changes") - changed_before) as u64;

    assert!(
        changes >= 5,
        "the window recorded too little to judge: {changes}"
    );
    assert!(
        did.publications <= changes,
        "the board was published more often than the run changed: {did:?} over {changes} changes"
    );
    assert!(
        did.statuses <= changes,
        "the frontier was derived more often than the run changed: {did:?} over {changes} changes"
    );

    world.release("hold.go");
    world.until("the run to settle", |world| {
        world.run_file("changes", "result.json").is_file()
    });
}

/// Another run's ledger is read on the interval this loop states, whatever rate
/// its own passes are running at.
///
/// Two consumers of the same upstream, one woken twenty times a second by a
/// narrating dispatch and one twice a second. Their pass counts are an order of
/// magnitude apart and what they read out of the upstream is not.
#[test]
fn another_runs_ledger_is_read_on_its_own_interval_and_not_on_the_loops() {
    let world = measured("loopcost-paced");
    // An upstream that is still going, so there is something to re-read.
    let upstream = world.plan(
        "moving",
        &plan_of("moving", vec![agent("build", &[]), human("approve", &[])]),
    );
    world.run(&["start", &upstream, "--attach"]).settled();

    for (run, every) in [("chatty", "50"), ("quiet", "500")] {
        world.script(&format!("{run}-hold.wait"), "hold");
        world.script(&format!("{run}-hold.heartbeat"), every);
        let mut consumer = agent("ship", &[]);
        consumer["deps"] = json!(["run:moving#build"]);
        let plan = world.plan(
            run,
            &plan_of(run, vec![agent(&format!("{run}-hold"), &[]), consumer]),
        );
        world.run(&["start", &plan, "--detach"]).exited(0);
        reporting(&world, run);
    }
    std::thread::sleep(Duration::from_secs(1));

    let window = Duration::from_secs(10);
    let before: Vec<Counts> = ["chatty", "quiet"]
        .iter()
        .map(|run| counts(&world, run))
        .collect();
    std::thread::sleep(window);
    let did: Vec<Counts> = ["chatty", "quiet"]
        .iter()
        .enumerate()
        .map(|(nth, run)| counts(&world, run).since(before[nth]))
        .collect();

    // The pass rates really are far apart, which is what makes the next claim
    // mean anything.
    assert!(
        did[0].passes > did[1].passes * 4,
        "the two loops did not run at different rates: {did:?}"
    );
    // Two reads answer one edge — has the node settled, and how far has that run
    // got — so twice a second is four reads a second and no more.
    let ceiling = 4 * window.as_secs() + 4;
    for (nth, run) in ["chatty", "quiet"].iter().enumerate() {
        assert!(
            did[nth].upstream_reads <= ceiling,
            "{run} read the upstream more often than the interval allows: {did:?}"
        );
    }
    assert!(
        did[0].upstream_reads <= 2 * did[1].upstream_reads + 4,
        "reading the upstream tracked the loop's pass rate: {did:?}"
    );

    for run in ["chatty", "quiet"] {
        world.release(&format!("{run}-hold.go"));
    }
}

/// The loop still answers inside the bounds a caller observes it by.
///
/// Four of the six, all measured off what the run recorded rather than off which
/// pass the work happened on. The other two are the ones that turn on state this
/// run does not write, and live beside the fixtures that produce them —
/// `loopcost::a_consumer_proceeds_within_a_second_of_its_upstream_settling` below,
/// and `adoption::a_published_node_is_held_until_the_release_answers_and_by_nothing_else`.
#[test]
fn every_answer_the_loop_owes_arrives_inside_a_second() {
    let world = World::new("loopcost-latency");
    world.script("build.wait", "hold");
    // A node held open throughout, so the driver is still there to be measured:
    // a graph whose every other node has settled is terminal, and the loop that
    // answers these is the loop that has ended.
    world.script("hold.wait", "hold");
    let plan = world.plan(
        "prompt",
        &plan_of(
            "prompt",
            vec![
                agent("hold", &[]),
                agent("build", &[]),
                agent("ship", &["build"]),
                human("approve", &[]),
                agent("after", &["approve"]),
            ],
        ),
    );
    world.run(&["start", &plan, "--detach"]).exited(0);
    world.until("the first dispatch to start", |world| {
        recorded(world, "prompt", "node-dispatched", "build")
    });

    // A settlement is readable in the journal after the dispatch reports it.
    let released = Instant::now();
    world.release("build.go");
    world.until("the settlement to be readable", |world| {
        recorded(world, "prompt", "node-settled", "build")
    });
    let readable = released.elapsed();
    assert!(
        readable < Duration::from_secs(1),
        "a settlement took {readable:?} to become readable"
    );

    // A node whose last dependency settles is dispatched.
    world.until("the dependent to start", |world| {
        recorded(world, "prompt", "node-dispatched", "ship")
    });
    let waited = at(&one(&world, "prompt", "node-dispatched", "ship"))
        - at(&one(&world, "prompt", "node-settled", "build"));
    assert!(
        waited < 1_000,
        "a node waited {waited}ms after its last dependency settled"
    );

    // An edit accepted on the channel has taken effect. The verb waits for the
    // reconciler's own answer, so what it costs a caller *is* the latency.
    let asked = Instant::now();
    world.run(&["attest", "prompt", "approve"]).exited(0);
    let answered = asked.elapsed();
    assert!(
        answered < Duration::from_secs(1),
        "an edit took {answered:?} to be answered"
    );

    // And the subtree that decision was holding proceeds.
    world.until("the held subtree to start", |world| {
        recorded(world, "prompt", "node-dispatched", "after")
    });
    let resumed = at(&one(&world, "prompt", "node-dispatched", "after"))
        - at(&one(&world, "prompt", "human-attested", "approve"));
    assert!(
        resumed < 1_000,
        "a subtree waited {resumed}ms after its decision cleared"
    );

    world.release("hold.go");
    world.until("the run to settle", |world| {
        world.run_file("prompt", "result.json").is_file()
    });
}

/// A node whose cross-DAG dependency settles in another run proceeds within a
/// second of that settlement.
///
/// The one bound that turns on state this run does not write: nothing tells this
/// driver the upstream moved, so what it costs is the interval the loop looks on.
#[test]
fn a_consumer_proceeds_within_a_second_of_its_upstream_settling() {
    let world = World::new("loopcost-upstream");
    world.script("late.wait", "hold");
    let mut consumer = agent("ship", &[]);
    consumer["deps"] = json!(["run:moving#build"]);
    let plan = world.plan(
        "watcher",
        &plan_of("watcher", vec![agent("late", &[]), consumer]),
    );
    world.run(&["start", &plan, "--detach"]).exited(0);
    world.until("the consumer to be held", |world| {
        !world
            .events_of("watcher", "node-held")
            .iter()
            .filter(|event| event["labels"]["node"] == "ship")
            .count()
            .eq(&0)
    });

    // Only now does the upstream exist at all.
    let upstream = world.plan("moving", &plan_of("moving", vec![agent("build", &[])]));
    world.run(&["start", &upstream, "--attach"]).exited(0);
    world.until("the consumer to proceed", |world| {
        recorded(world, "watcher", "node-dispatched", "ship")
    });

    let waited = at(&one(&world, "watcher", "node-dispatched", "ship"))
        - at(&one(&world, "moving", "node-settled", "build"));
    assert!(
        waited < 1_000,
        "a consumer waited {waited}ms after its upstream settled in another run"
    );

    world.release("late.go");
    world.until("the run to settle", |world| {
        world.run_file("watcher", "result.json").is_file()
    });
}
