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
//!
//! The one thing the harness substitutes throughout is `oneagentgraph`, a sibling
//! behind its own subprocess boundary: the layer under test is this crate's
//! reconcile loop, driven as the compiled binary over a real run store, and the
//! counts asserted on are that real loop's own. A journey has to hold a node in
//! flight for a whole minute to reach the converged state, and a model turn is
//! not what any of these are about. Each journey says below what it holds open.

use std::time::{Duration, Instant};

use crate::harness::{
    agent, counts, epoch_millis, human, plan_of, reporting, restored, unreachable, Counts, World,
    LOOP_STATS_ENV, RENDEZVOUS_SECONDS_ENV,
};
use serde_json::{json, Value};

/// The interval every claim here is measured over.
///
/// A minute, because that is what the bounds are stated over: sixty seconds in
/// which an idle run records nothing, and sixty in which a paced read happens on
/// its own interval rather than on the loop's. The tree before this change
/// performed about 2,400 passes in it.
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] this is the interval the
// three minute-long journeys below sleep, each of which carries this same suppression
// with its reason: the minute is the bound's own interval rather than a knob, and what
// it measures is the whole crate's reconcile loop, which any change under `src/` can
// put the sink back into — so no project edged narrower than the crate could honestly
// run them, and a constant they share cannot be edged narrower than they are.
const WINDOW: Duration = Duration::from_secs(60);

/// How long the scale journey waits for its large run's held node to dispatch.
///
/// That node is queued behind the run's ninety-nine others at a concurrency of
/// four, so the wait is those hundred dispatches rather than one step of the
/// loop, and it is sized to them: three seconds each, where the slowest
/// cross-platform leg has taken about one and a half — a debug build reading its
/// journal through the bus reader, whose decode is recorded in
/// `docs/contract-divergences.md` entry 76. It is the harness's backstop and not
/// what the journey proves: every bound the journey asserts is a count of work,
/// and none of them moved.
const QUEUED_DISPATCHES: Duration = Duration::from_secs(3 * 100);

/// A world whose driver counts its own work, and whose held dispatches outlast
/// the journey holding them.
///
/// The harness's default hold patience is set above one `until` deadline, which
/// is what every other journey holds a dispatch across. The journeys here hold
/// one across several — at the longest, two `until` deadlines, then
/// [`QUEUED_DISPATCHES`], then the whole of [`WINDOW`], six hundred seconds —
/// and a hold that expires inside the window is not reported as the expiry it
/// is: the double exits, the engine dispatches the node again, and that
/// re-dispatch's reads land in the minute that was supposed to record nothing.
/// So the patience is that sum with room, and a hold nobody releases still fails
/// first, as an `until` timeout.
fn measured(name: &str) -> World {
    World::new(name)
        .with_env(LOOP_STATS_ENV, "1")
        .with_env(RENDEZVOUS_SECONDS_ENV, "900")
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

fn recorded(world: &World, run: &str, kind: &str, node: &str) -> bool {
    world
        .events_of(run, kind)
        .iter()
        .any(|event| event["labels"]["node"] == node)
}

/// When one record was written, in milliseconds since the epoch.
///
/// The envelope's own timestamp, which is millisecond-precision UTC — so a
/// latency between two records the loop wrote is measured off what the run
/// recorded rather than off what this test process happened to observe. On the
/// epoch's scale so it compares with the one instant the run keeps as a file's
/// write time rather than as a record: [`accepted`].
fn at(event: &Value) -> u64 {
    epoch_millis(
        event["ts"]
            .as_str()
            .unwrap_or_else(|| panic!("no ts: {event}")),
    )
}

// llmlint: ignore-block[tests_mirror_real_usage] the instant a run's channel accepted an
// edit is recorded nowhere a user can read it — the queue's records carry no timestamp
// and no verb reports one — so its write time is the only account of that instant there
// is. What a user-facing reading has instead is the clock around the verb's process,
// which is what this replaced: on a loaded Windows runner it measured the process
// starting and folding the journal, and failed correct work at 1.4s. The verb itself is
// still driven as a user runs it, and its exit is still asserted.
/// When the run's channel accepted the edit it was handed, in milliseconds since
/// the epoch.
///
/// The command queue's records carry no timestamp, so the instant an edit was
/// accepted is the queue's own write time: nothing but a submitted envelope
/// appends to it, and a run handed one edit wrote it once. That is where the
/// loop's part of an edit begins — what the verb spends before it, starting a
/// process and folding the journal to validate the edit, is the caller's host's.
fn accepted(world: &World, run: &str) -> u64 {
    let queue = world.run_file(run, "channel/commands.jsonl");
    let written = std::fs::metadata(&queue)
        .and_then(|metadata| metadata.modified())
        .unwrap_or_else(|e| panic!("{} has no write time: {e}", queue.display()));
    let since = written
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a write time after the epoch");
    u64::try_from(since.as_millis()).expect("milliseconds since the epoch fit")
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// How long a driver's counts must hold still before a window over it opens.
const QUIET: Duration = Duration::from_secs(5);

/// The runs' counts, read once each run's driver has stopped working.
///
/// A run's last record is not its driver's last pass: the pass that follows it
/// re-derives the graph and publishes the board, and on a loaded Windows runner a
/// large run's took long enough to land inside the window after a fixed two
/// seconds — one refold of a 4,187-record journal, read as an idle pass growing
/// with the run. So the window opens once every count has held still for
/// [`QUIET`]. The wait is bounded by one [`WINDOW`] and never fails on its own: a
/// driver that does not go quiet is the defect these journeys catch, and it is
/// left to their assertions to name.
fn quiet_counts(world: &World, runs: &[&str]) -> Vec<Counts> {
    let read = || -> Vec<Counts> { runs.iter().map(|run| counts(world, run)).collect() };
    let deadline = Instant::now() + WINDOW;
    let mut last = read();
    let mut since = Instant::now();
    while since.elapsed() < QUIET && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        let now = read();
        if now != last {
            last = now;
            since = Instant::now();
        }
    }
    last
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

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] this journey sleeps
// the whole of WINDOW, and the minute is not a knob: it *is* the interval the bound is
// stated over, so a shorter one would assert a different claim. What it measures is the
// whole crate's reconcile loop, which any change under `src/` can put the sink back into,
// so a project edged narrower than the crate could not honestly run it. The three
// minute-long journeys in this file run beside each other under nextest; every other
// journey here is seconds.
/// A converged run with one node in flight does no scheduling work at all while
/// it records nothing.
///
/// Zero derivations of the graph's statuses, zero write-back publications, zero
/// reads out of the run store, zero asks about a release and zero reads of
/// another run's ledger — over a minute in which the run wrote not one record.
/// The tree before this change performed roughly 2,400 passes in that minute,
/// each folding the journal four times over.
// llmlint: ignore-block[tests_mirror_real_usage] the claim is that a converged driver does no
// scheduling work, and there is no user-facing representation of work that did not happen: a
// loop that publishes nothing and folds nothing writes no record, so a journey reading the CLI
// alone cannot tell it from the 40-passes-a-second loop this replaced. Everything a user does
// is real here — the shipped binary, a real plan store, a real dispatch, the shipped intervals
// — and the counters are the driver's own account of the one thing left, which the host
// otherwise only feels as CPU a loaded machine hands out as it likes.
// llmlint: ignore-block[e2e_not_mocked] the converged state every bound here is about is
// one node held in flight for a whole minute — see the module note above.
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
    std::thread::sleep(WINDOW);
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
        did.passes <= WINDOW.as_secs(),
        "a converged driver ran more than one scheduling pass a second: {did:?}"
    );

    world.release("hold.go");
    world.until("the run to settle", |world| {
        world.run_file("idle", "result.json").is_file()
    });
}
// llmlint: ignore-end[e2e_not_mocked]
// llmlint: ignore-end[tests_mirror_real_usage]
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] this journey sleeps
// the whole of WINDOW, and the minute is not a knob: it *is* the interval the bound is
// stated over, so a shorter one would assert a different claim. What it measures is the
// whole crate's reconcile loop, which any change under `src/` can put the sink back into,
// so a project edged narrower than the crate could not honestly run it. The three
// minute-long journeys in this file run beside each other under nextest; every other
// journey here is seconds.
/// What a converged idle pass costs does not grow with the run.
///
/// Two converged runs two orders of magnitude apart in nodes, and more than one
/// in journal length, read the same amount out of the store over the same
/// interval — because neither reads anything at all. The tree before this change
/// refolded the whole journal on every pass, so the larger run's driver read
/// hundreds of times what the smaller one's did.
// llmlint: ignore-block[tests_mirror_real_usage] what this compares is how much two real drivers
// read out of two real run stores, which no CLI output reports: the defect it holds off — a
// pass that refolds the journal — is invisible to every user-facing surface and shows only as
// a run that costs more the longer it has been running. The runs, the plans, the dispatches
// and the binary are the real ones; the byte count is the only observation of the property.
// llmlint: ignore-block[e2e_not_mocked] the converged state every bound here is about is
// one node held in flight for a whole minute — see the module note above.
#[test]
fn an_idle_pass_does_not_grow_with_the_run_it_is_idling_on() {
    let world = measured("loopcost-scale");
    world.script("hold.wait", "hold");
    // Each live run projects into its source root. Separate roots prevent one run's
    // board from changing the other's workload while they share this engine process.
    let small_store = world.store_apart("small");
    let large_store = world.store_apart("large");
    let small = world.plan_in(
        &small_store,
        "small",
        &plan_of("small", vec![agent("hold", &[])]),
    );

    let mut many: Vec<Value> = (0..99).map(|n| agent(&format!("n{n}"), &[])).collect();
    many.push(agent("hold", &[]));
    let large = world.plan_in(&large_store, "large", &plan_of("large", many));

    world
        .run_in(&small_store, &["start", &small, "--detach"])
        .exited(0);
    world
        .run_in(&large_store, &["start", &large, "--detach"])
        .exited(0);
    for run in ["small", "large"] {
        world.until_within(QUEUED_DISPATCHES, "both dispatches to start", |world| {
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
    let before = quiet_counts(&world, &["small", "large"]);

    let sizes: Vec<usize> = ["small", "large"]
        .iter()
        .map(|run| world.journal(run).len())
        .collect();
    assert!(
        sizes[1] > sizes[0] * 10,
        "the two runs are not orders of magnitude apart: {sizes:?}"
    );

    std::thread::sleep(WINDOW);
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
// llmlint: ignore-end[e2e_not_mocked]
// llmlint: ignore-end[tests_mirror_real_usage]
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

/// The two things a pass does about whole state are paid once per recorded state
/// change, not once per pass.
// llmlint: ignore-block[tests_mirror_real_usage] "at most one publication and one status
// derivation per recorded state change" is a ratio between what the run journalled, which is
// read the user's way, and what the loop did to produce it, which nothing outside the process
// reports. The journey drives the real CLI end to end and reads the real journal for one half
// of the ratio; the counters are the only account of the other half.
// llmlint: ignore-block[e2e_not_mocked] the converged state every bound here is about is
// one node held in flight for a whole minute — see the module note above.
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
// llmlint: ignore-end[e2e_not_mocked]
// llmlint: ignore-end[tests_mirror_real_usage]

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] this journey sleeps
// the whole of WINDOW, and the minute is not a knob: it *is* the interval the bound is
// stated over, so a shorter one would assert a different claim. What it measures is the
// whole crate's reconcile loop, which any change under `src/` can put the sink back into,
// so a project edged narrower than the crate could not honestly run it. The three
// minute-long journeys in this file run beside each other under nextest; every other
// journey here is seconds.
/// Another run's ledger is read on the interval this loop states, whatever rate
/// its own passes are running at.
///
/// Two consumers of the same upstream, one woken five times a second by a
/// narrating dispatch and one every two seconds. The chatty one runs passes at
/// more than twice the quiet one's rate, and what they read out of the upstream
/// is the same.
///
/// Five a second rather than twenty: every beat a dispatch sends is a record the
/// loop relays, and a host that relays fewer of them a second than it is sent
/// never finishes a pass — each one drains a backlog that grew while it ran. A
/// loaded Windows runner relayed under twenty a second, so the chatty loop ran
/// eighteen passes in the window, and the premise below failed with every claim
/// under it intact (run 36666603034).
///
/// Measured over [`WINDOW`], the same minute every other bound here is stated
/// over: a paced read is a **rate**, and a window of a few seconds bounds it at a
/// number a burst either side of the interval can reach without the rate having
/// moved at all.
// llmlint: ignore-block[tests_mirror_real_usage] the property is that another run's ledger is read
// on a stated interval rather than on the loop's pass rate — a statement about how often a
// real driver reads a real upstream store, which produces no record either way. Both runs,
// both stores and both drivers are real and the shipped intervals are unchanged; the counters
// are what makes "how often" observable at all.
// llmlint: ignore-block[e2e_not_mocked] the converged state every bound here is about is
// one node held in flight for a whole minute — see the module note above.
#[test]
fn another_runs_ledger_is_read_on_its_own_interval_and_not_on_the_loops() {
    let world = measured("loopcost-paced");
    // An upstream that is still going, so there is something to re-read.
    let upstream = world.plan(
        "moving",
        &plan_of("moving", vec![agent("build", &[]), human("approve", &[])]),
    );
    world.run(&["start", &upstream, "--attach"]).settled();

    for (run, every) in [("chatty", "200"), ("quiet", "2000")] {
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

    let before: Vec<Counts> = ["chatty", "quiet"]
        .iter()
        .map(|run| counts(&world, run))
        .collect();
    std::thread::sleep(WINDOW);
    let did: Vec<Counts> = ["chatty", "quiet"]
        .iter()
        .enumerate()
        .map(|(nth, run)| counts(&world, run).since(before[nth]))
        .collect();

    // Two reads answer one edge — has the node settled, and how far has that run
    // got — so twice a second is four reads a second and no more.
    let ceiling = 4 * WINDOW.as_secs() + 4;
    // The chatty loop ran more passes than the ceiling allows reads, which is
    // what makes the claims below mean anything: a loop that read on every pass
    // would have read at least that often and broken the ceiling. Stated against
    // the ceiling rather than against the quiet loop's passes, because the quiet
    // loop is paced by deadlines and the chatty one by how fast this host runs a
    // pass — on a loaded Windows runner a third of the idle rate, which put a
    // ratio between the two below any fixed multiple while every claim here held.
    assert!(
        did[0].passes > ceiling,
        "the chatty loop ran too few passes for its reads to say anything: {did:?}"
    );
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
// llmlint: ignore-end[e2e_not_mocked]
// llmlint: ignore-end[tests_mirror_real_usage]
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

/// The loop still answers inside the bounds a caller observes it by.
///
/// Four of the six, all measured off what the run recorded rather than off which
/// pass the work happened on. The other two are the ones that turn on state this
/// run does not write, and live beside the fixtures that produce them —
/// `loopcost::a_consumer_proceeds_within_a_second_of_its_upstream_settling` below,
/// and `adoption::a_published_node_is_held_until_the_release_answers_and_by_nothing_else`.
// llmlint: ignore-block[e2e_not_mocked] the converged state every bound here is about is
// one node held in flight for a whole minute — see the module note above.
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

    // A settlement is readable in the journal after the dispatch reports it:
    // from the dispatch's own report of its member settling, relayed onto the
    // run's stream with the stamp the dispatch gave it, to the run's settlement.
    // Not from the test's release of the hold, which also spends the dispatch
    // noticing the release and saying so — the double's work, and on a loaded
    // host the larger part of what a clock around it measured.
    world.release("build.go");
    world.until("the settlement to be readable", |world| {
        recorded(world, "prompt", "node-settled", "build")
    });
    let readable = at(&one(&world, "prompt", "node-settled", "build"))
        - at(&one(&world, "prompt", "member-settled", "build"));
    assert!(
        readable < 1_000,
        "a settlement took {readable}ms to become readable after its dispatch reported it"
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

    // An edit accepted on the channel has taken effect: from the channel taking
    // the edit to the reconciler committing it. The verb waits for that answer,
    // so its exit says the edit was taken; what it spends around it, starting a
    // process and validating the edit against the journal before queueing it,
    // is the caller's host's and not the loop's.
    world.run(&["attest", "prompt", "approve"]).exited(0);
    let committed: Vec<Value> = world
        .events_of("prompt", "edit-committed")
        .into_iter()
        .filter(|event| event["payload"]["command"]["ref"] == "approve")
        .collect();
    assert_eq!(
        committed.len(),
        1,
        "prompt committed the attest: {committed:?}"
    );
    // llmlint: ignore-block[tests_mirror_real_usage] measured from the channel queue's own
    // write time, for the reason `accepted` gives: no user-facing surface records when an
    // edit was accepted, and the clock around the verb's process is the race this replaced.
    let answered = at(&committed[0]).saturating_sub(accepted(&world, "prompt"));
    assert!(
        answered < 1_000,
        "an edit took {answered}ms to be answered after the channel accepted it"
    );
    // llmlint: ignore-end[tests_mirror_real_usage]

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
} // llmlint: ignore-end[e2e_not_mocked]

/// A node whose cross-DAG dependency settles in another run proceeds within a
/// second of that settlement.
///
/// The one bound that turns on state this run does not write: nothing tells this
/// driver the upstream moved, so what it costs is the interval the loop looks on.
// llmlint: ignore-block[e2e_not_mocked] the converged state every bound here is about is
// one node held in flight for a whole minute — see the module note above.
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
    // The hold names the reference whole. A cross-run dependency is the one id a
    // reader cannot shorten and still act on — half of it names no node of any
    // graph — so what the record carries is what the plan wrote.
    let holds = world.events_of("watcher", "node-held");
    let ship = holds
        .iter()
        .find(|event| event["labels"]["node"] == "ship")
        .expect("the consumer is held");
    assert!(
        ship["payload"]["reasons"]
            .as_array()
            .expect("a hold carries reasons")
            .iter()
            .any(|reason| reason["kind"] == "dependencies"
                && reason["blocking"] == json!(["run:moving#build"])),
        "the hold on a cross-run dependency does not name the run it waits on: {ship}"
    );

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
} // llmlint: ignore-end[e2e_not_mocked]

/// A projection that fails while the run is recording nothing reaches the
/// planner all the same.
///
/// The write-back worker runs on a thread of its own, so it fails without
/// anything about the run changing — and a loop that only woke for its own state
/// would leave the board reported behind until something else happened to the
/// run. Here nothing else does: one node is held open, nobody edits anything, and
/// the surface has to arrive on a wake the worker caused.
// llmlint: ignore-block[e2e_not_mocked] the converged state every bound here is about is
// one node held in flight for a whole minute — see the module note above.
#[test]
fn a_projection_that_fails_while_the_run_records_nothing_still_reaches_the_planner() {
    let world = World::new("loopcost-unprojected");
    world.script("hold.wait", "hold");
    world.script("first.wait", "hold");
    let project = world.plan(
        "unprojected",
        &plan_of("unprojected", vec![agent("hold", &[]), agent("first", &[])]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to reach the store", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "first"
                && task["item"]["status"]["category"] == "in-progress"
        })
    });

    // The store goes away, one node settles, and then the run records nothing
    // at all: the settlement's own pass publishes the snapshot, and the worker
    // meets the outage well after that pass has finished asking. Taken away
    // rather than moved, because the copy that put `first` on the board can
    // still be writing when it goes — see `unreachable` for what a move lets it do.
    let unavailable = world.root.join("plan-store-unavailable");
    unreachable(
        &world.store(),
        &unavailable,
        "the store becomes unreachable",
    );
    world.release("first.go");
    world.until("the node to settle", |world| {
        recorded(world, "unprojected", "node-settled", "first")
    });
    // How soon is the engine's to bound, not this wait's: the sum
    // `src/writeback.rs` states where the failure is recorded, of
    // `FIRST_RETRY_AFTER` and `CHANNEL_POLL`. The deadline is the backstop for a
    // host that has not scheduled the worker.
    world.until("the failed projection to reach the planner", |world| {
        world
            .events_of("unprojected", "planner-surface-queued")
            .iter()
            .any(|event| {
                event["payload"]["message"]
                    .as_str()
                    .is_some_and(|said| said.contains("did not take this run's projection"))
            })
    });

    restored(&unavailable, &world.store(), "the store returns");
    world.release("hold.go");
    world.until("the run to settle", |world| {
        world.run_file("unprojected", "result.json").is_file()
    });
} // llmlint: ignore-end[e2e_not_mocked]

/// A driver asked for the counts and unable to write them says so, naming the
/// file, instead of going on as though it had written them.
///
/// The failure path of the measurement every other journey here reads. The counts
/// exist because a host asked this driver for them, so a write it cannot do is
/// that host's answer going missing: swallowing it leaves a caller reading a file
/// frozen at an earlier pass with nothing anywhere saying why. Driven through the
/// real CLI over a real run store, and read where a detached driver's failures
/// are read — the run's own driver log.
// llmlint: ignore-block[e2e_not_mocked] the converged state every bound here is about is
// one node held in flight for a whole minute — see the module note above.
#[test]
fn a_driver_that_cannot_write_the_counts_it_was_asked_for_says_so() {
    let world = measured("loopcost-unwritable");
    world.script("hold.wait", "hold");
    let plan = world.plan(
        "unwritable",
        &plan_of("unwritable", vec![agent("hold", &[])]),
    );
    world.run(&["start", &plan, "--detach"]).exited(0);
    // Waited for, so what is obstructed below is the path the driver really writes.
    reporting(&world, "unwritable");

    // A non-empty directory where the counts go, so both halves of the atomic
    // write refuse: the temporary lands beside it, and the rename onto it cannot
    // happen. This is what a host that had mounted something there, or left a
    // directory of that name behind, does to the next write.
    //
    // Placed until it stands rather than once: the driver writes the counts on
    // every wait, so the file can land again between its removal and the
    // directory — a Windows leg met exactly that, refused `AlreadyExists` for
    // the directory over a file the driver had just renamed in — and the same
    // leg can refuse the removal itself while that rename still holds the name.
    // A rename onto a directory is what neither platform allows, so once the
    // directory is there the driver's next write is the refusal under test.
    let obstruction = world.run_file("unwritable", "loop-stats.json");
    let placing = Instant::now();
    let placed = loop {
        let attempt = match std::fs::remove_file(&obstruction) {
            Err(why) if why.kind() != std::io::ErrorKind::NotFound => Err(why),
            _ => std::fs::create_dir(&obstruction),
        };
        match attempt {
            Ok(()) => break Ok(()),
            Err(_) if placing.elapsed() < Duration::from_secs(30) => {}
            Err(why) => break Err(why),
        }
    };
    placed.expect("the obstruction is placed");
    std::fs::write(obstruction.join("held"), "not the counts").expect("the obstruction holds");

    world.until("the driver to report what it could not write", |world| {
        std::fs::read_to_string(world.run_file("unwritable", "driver.log"))
            .unwrap_or_default()
            .contains("loop-stats.json")
    });
    // And it stopped rather than carrying on with the host's question
    // unanswered: the node it was holding open is still unsettled.
    assert!(
        !recorded(&world, "unwritable", "node-settled", "hold"),
        "the driver went on running after refusing"
    );
    assert!(
        obstruction.is_dir() && obstruction.join("held").is_file(),
        "the run wrote over the obstruction it refused on"
    );

    world.release("hold.go");
} // llmlint: ignore-end[e2e_not_mocked]
