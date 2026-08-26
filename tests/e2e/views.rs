//! The read-only views. They render from the merged three-stream event store,
//! take no lock a writer needs, and never call a node running once the ledger
//! has recorded it settled.
//!
//! Ported from `test_monitor_e2e`, `test_monitor_run_plan_e2e`, `test_goals_e2e`, `test_run_views_by_id_e2e`, `test_live_dispatch_views_e2e`, and `test_telemetry_e2e`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. `onevcs` is not substituted here either: the lifecycle journeys below
// drive the real library against a real git origin. What the `oneagentgraph` double buys
// is a dispatch outcome a real sibling would need paid model turns to produce, and
// `dispatch.rs` is where the real binary is driven instead. `harness.rs` carries the same
// suppression and the full rationale.

use crate::harness::{agent, human, plan_of, reaped_pid, Run, World};

use crate::harness::lifecycle;
use onepipeline::event::{Envelope, Source};

/// The document a double plants where a settlement points but nothing should
/// read, and the words that prove it was read if they ever appear.
const PLANTED_DOCUMENT: &str =
    r#"{"transcript":{"messages":[{"role":"assistant","content":"planted-and-never-read"}]}}"#;

/// The one recognisable string inside it.
const PLANTED_WORDS: &str = "planted-and-never-read";

/// Whether a payload value is a path whose file name is the producing library's
/// own report file.
///
/// Exactly the test the engine makes on the value it copies, and asked of the
/// value rather than of a key this test names: both the key a report path rides
/// under and the file name at the end of it are the producer's, so a settlement
/// is picked out of a journal by asking the producing library rather than by
/// restating its payload shape here.
fn names_a_report_file(value: &serde_json::Value) -> bool {
    value
        .as_str()
        .map(std::path::Path::new)
        .and_then(std::path::Path::file_name)
        == Some(std::ffi::OsStr::new(oneagentgraph::member::REPORT_FILE))
}

/// Drive one run from this test, attached, keeping what the launch said: a
/// refusal made as an envelope is ingested reaches the driver's own stderr,
/// which in a detached run goes to a log no assertion can read.
fn driven(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> (String, Run) {
    let path = world.plan(name, &plan_of(name, nodes));
    let launched = world.run(&["start", &path, "--attach"]);
    (name.to_string(), launched)
}

/// How long a held publication phase is kept open, so its bucket is a real
/// duration on the clock rather than a bucket that merely exists.
const HELD: std::time::Duration = std::time::Duration::from_millis(400);

/// The floor a held stretch must clear once it has been measured. Below the
/// hold, because the two records bracketing it are written either side of the
/// rendezvous rather than exactly on it.
const FLOOR: u64 = 250;

fn settled(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

#[test]
fn monitor_renders_all_three_streams_under_their_own_typed_ids() {
    let world = World::new("views-monitor");
    world.repository("local-direct", &[]);
    world.script("service.work", "the worker wrote this\n");
    let run = settled(&world, "watched", vec![lifecycle("service", &[])]);

    let stream = world.run(&["monitor", &run, "--all"]);
    stream.exited(0);
    // The first line is the contract, not a banner.
    assert!(
        stream.stdout.starts_with("Concise graph events;"),
        "{}",
        stream.stdout
    );
    stream.out_has("graph:service");
    stream.out_has("agent:");
    stream.out_has("vcs:");
    // The run's own state has no node, so it has no graph id: it reaches the
    // reader as a trailer rather than as an event line, naming the run.
    stream.out_has("-- watched  1/1 done");
}
#[test]
fn monitor_writes_nothing_and_consumes_nothing() {
    let world = World::new("views-readonly");
    world.script("build.wait", "hold");
    let path = world.plan("readonly", &plan_of("readonly", vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("a node to be in flight", |world| {
        !world.events_of("readonly", "node-dispatched").is_empty()
    });

    let before = world.journal("readonly").len();
    for view in [
        vec!["monitor", "readonly"],
        vec!["results", "readonly"],
        vec!["status", "readonly"],
        vec!["goals", "readonly"],
        vec!["transcript", "readonly"],
        vec!["telemetry", "readonly"],
        vec!["runs"],
        vec!["host"],
    ] {
        world.run(&view).exited(0);
    }
    assert_eq!(
        world.journal("readonly").len(),
        before,
        "a read-only view wrote to the journal"
    );
    // And none of them took the lock the driving process holds.
    world.release("build.go");
}

#[test]
fn results_reports_each_nodes_own_evidence() {
    let world = World::new("views-results");
    world.script("failing.fail", "1");
    let run = settled(
        &world,
        "evidence",
        vec![
            agent("built", &[]),
            agent("failing", &[]),
            human("approve", &["built"]),
            agent("gated", &["approve"]),
        ],
    );

    let results = world.run(&["results", &run]);
    results
        .exited(0)
        .out_has("built")
        .out_has("done")
        .out_has("failing")
        .out_has("failed")
        .out_has("approve")
        .out_has("waiting")
        .out_has("action:")
        .out_has("unblocks: gated");
}

/// A skip is a node the run **never asked**, and which dependency stopped it
/// being asked is the fact the status word does not carry.
///
/// Two causes on one node and a chain through another, because a reader told
/// about one of two would fix it and watch the node stay skipped.
#[test]
fn results_names_every_skipped_node_and_the_dependency_that_skipped_it() {
    let world = World::new("views-skipped-nodes");
    world.script("build.fail", "1");
    world.script("lint.fail", "1");
    let run = settled(
        &world,
        "unattempted",
        vec![
            agent("build", &[]),
            agent("lint", &[]),
            agent("ship", &["build", "lint"]),
            agent("announce", &["ship"]),
            agent("aside", &[]),
        ],
    );

    let results = world.run(&["results", &run]);
    results.exited(0);
    for (node, cause) in [
        (
            "ship",
            "never attempted; skipped by: build (failed), lint (failed)",
        ),
        ("announce", "never attempted; skipped by: ship (skipped)"),
    ] {
        results.out_has(node).out_has(cause);
    }
    assert!(
        !results.stdout.contains("skipped by: aside")
            && results.stdout.matches("never attempted").count() == 2,
        "{}",
        results.stdout
    );

    // The split is readable without opening `results` at all, which is where a
    // supervisor scanning a host meets a run before they read anything of it.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("1/5 done, 2 never attempted");
    world.run(&["runs"]).exited(0).out_has("2 never attempted");
}

/// An observer this host cannot ask about is reported as watching, never as
/// dead.
///
/// The direction is the safety of the verdict: `OBSERVER DEAD` sends somebody to
/// relaunch a graph, and saying it of a working observer costs a run its watcher
/// for nothing.
#[test]
fn an_observer_this_host_cannot_ask_about_is_never_reported_dead() {
    let world = World::new("views-observer-unprovable");
    world.script("build.wait", "hold");
    let path = world.plan(
        "unprovable",
        &plan_of("unprovable", vec![agent("build", &[])]),
    );
    // The sibling's run store, pointed at this world rather than at whatever the
    // host running these tests keeps in its own: the answer below has to be that
    // *this* run's observer cannot be asked about, not that a stranger's could.
    let read = |world: &World, view: &[&str]| -> String {
        let mut command = world.cmd(view);
        command.env("ONEAGENTGRAPH_STATE_DIR", world.graph_state());
        let rendered = world.run_on(command, "a view over an unprovable observer");
        rendered.exited(0);
        rendered
            .stdout
            .lines()
            .find(|line| line.contains("unprovable"))
            .unwrap_or_else(|| panic!("no line for the run in:\n{}", rendered.stdout))
            .to_string()
    };

    let mut launch = world.cmd(&[
        "start",
        &path,
        "--detach",
        "--dag-graph",
        &world.shipped_dag_graph(),
    ]);
    launch.env("ONEAGENTGRAPH_STATE_DIR", world.graph_state());
    world.run_on(launch, "start unprovable").exited(0);
    world.until("the run to dispatch its node", |world| {
        !world.events_of("unprovable", "node-dispatched").is_empty()
    });

    // Not a vacuous claim: the launch really did attach an observer and really
    // did record the graph run it minted, so there is something to ask about —
    // and the store it would be asked about in holds no record of it.
    let recorded = world.run_json("unprovable", "launch.json");
    let graph_run = recorded["graph_run"].as_str().unwrap_or_default();
    assert!(
        !recorded["graph"].as_str().unwrap_or_default().is_empty() && !graph_run.is_empty(),
        "the launch attached no observer, so nothing below is about one: {recorded}"
    );
    assert!(
        !world.graph_state().join(graph_run).exists(),
        "the sibling holds a record for {graph_run}, so this journey is not about an \
         observer nothing can answer for"
    );

    for view in [vec!["runs"], vec!["status"]] {
        let line = read(&world, &view);
        assert!(
            line.contains("ACTIVE") && !line.contains("OBSERVER"),
            "a run whose observer this host cannot ask about is not reported as \
             watched: {line}"
        );
    }
    world.release("build.go");
}

#[test]
fn goals_says_what_each_run_is_for_and_which_identities_it_holds() {
    let world = World::new("views-goals");
    world.repository("local-direct", &[]);
    let run = settled(&world, "purposeful", vec![lifecycle("service", &[])]);

    let goals = world.run(&["goals"]);
    goals
        .exited(0)
        .out_has("Deliver purposeful")
        .out_has("identities: service");
    world
        .run(&["goals", &run])
        .exited(0)
        .out_has("Deliver purposeful");
}
#[test]
fn every_scoped_view_takes_the_run_id_the_launch_record_advertises() {
    let world = World::new("views-byid");
    let run = settled(&world, "addressed", vec![agent("build", &[])]);

    for view in ["monitor", "results", "status", "goals", "telemetry"] {
        world.run(&[view, &run]).exited(0).out_has(&run);
    }
    // Unscoped, the same views cover every run.
    for view in ["status", "goals", "telemetry"] {
        world.run(&[view]).exited(0).out_has(&run);
    }
}

#[test]
fn status_names_a_live_dispatch_and_flags_one_nothing_is_driving() {
    let world = World::new("views-live");
    world.script("build.wait", "hold");
    let path = world.plan("live", &plan_of("live", vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the dispatch to be in flight", |world| {
        !world.events_of("live", "node-dispatched").is_empty()
    });

    // Before the dispatch has published anything, the ledger says it is running
    // and nothing is driving it — a positive claim, not a guess.
    world.run(&["status", "live"]).exited(0).out_has("UNDRIVEN");
    world.run(&["host"]).exited(0).out_has("build");

    world.release("build.go");
    world.until("the dispatch to publish", |world| {
        world
            .journal("live")
            .iter()
            .any(|event| event["source"] == "agentgraph")
    });
    world.until("the run to settle", |world| {
        world.run_file("live", "result.json").is_file()
    });

    // Once it has settled, no view calls it running whatever else is true.
    let status = world.run(&["status", "live"]);
    status.exited(0);
    assert!(
        !status.stdout.contains("build: running"),
        "a settled node is still reported running:\n{}",
        status.stdout
    );
    world.run(&["host"]).exited(0).out_has("no live dispatches");
}

#[test]
fn status_carries_the_provider_health_block_from_the_sibling() {
    let world = World::new("views-health");
    let run = settled(&world, "healthy", vec![agent("build", &[])]);
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("providers: fake-provider");
}

/// Every measured bucket, summed. An unmeasured one carries no `ms` at all,
/// which is the point: a zero there would read as a measurement.
fn measured(document: &serde_json::Value) -> u64 {
    document["buckets"]
        .as_array()
        .expect("buckets")
        .iter()
        .filter_map(|bucket| bucket.get("ms").and_then(serde_json::Value::as_u64))
        .sum()
}

#[test]
fn telemetry_buckets_sum_exactly_to_the_wall_clock() {
    let world = World::new("views-telemetry");
    let run = settled(
        &world,
        "timed",
        vec![agent("first", &[]), agent("second", &["first"])],
    );

    let telemetry = world.run(&["telemetry", &run]);
    telemetry.exited(0);
    let document = telemetry.json();
    let wall = document["wall_ms"].as_u64().expect("a wall clock");
    assert_eq!(
        measured(&document),
        wall,
        "the buckets do not sum to WALL: {document}"
    );
    assert_eq!(document["schema_version"], 2, "{document}");
    assert_eq!(document["dispatches"], 2);
    assert_eq!(document["settled_done"], 2);

    // Eight buckets, and the three nothing in this stack measures are served
    // absent rather than as a zero that reads as measured. `gate` is one of them
    // now: no library this crate composes runs a verification tier of its own, so
    // there is no such stretch to charge — what verifies a change is the
    // repository's own merge path, and the wall time that costs is the
    // publication's.
    let named: Vec<&str> = document["buckets"]
        .as_array()
        .expect("buckets")
        .iter()
        .map(|bucket| bucket["name"].as_str().expect("a bucket name"))
        .collect();
    assert_eq!(
        named,
        vec![
            "agent",
            "judge",
            "llmlint",
            "gate",
            "publication_wait",
            "lock_wait",
            "setup",
            "scheduling",
        ],
        "{document}"
    );
    for absent in ["judge", "llmlint", "gate"] {
        let bucket = document["buckets"]
            .as_array()
            .expect("buckets")
            .iter()
            .find(|bucket| bucket["name"] == absent)
            .expect("the bucket is still named");
        assert!(
            bucket.get("ms").is_none(),
            "{absent} reported a measured span nothing produces: {bucket}"
        );
    }

    let breakdown = world.run(&["telemetry", &run, "--breakdown"]);
    breakdown
        .exited(0)
        .out_has("WALL")
        .out_has("agent")
        .out_has("scheduling")
        .out_has("not measured");
}

/// The other direction, which never balanced: measured buckets that add up to
/// **more** than the wall clock, with nothing left in `scheduling` to give back.
///
/// A store is several producers' records merged, each stream in its own `seq`
/// and the streams interleaved by `ts`, so a stamp that goes backwards zeroes
/// the span across it and the next one forward re-charges ground already
/// counted — while the wall clock stays first-to-last as read.
///
/// The store is the one this run wrote, down to its records, streams and
/// sequences. What is arranged is the **clock**: a doubled dispatch settles
/// inside a millisecond, so it cannot skew one on its own, and the run's own
/// stamps are re-stated as an opening, a hundred seconds of work whose clock
/// slips back and forward across it, and a settlement a second after that.
#[test]
fn telemetry_balances_a_store_that_overcounts_past_an_empty_scheduling_bucket() {
    /// When the run opened, and what every record before its dispatch carries.
    const OPENED: &str = "2026-08-22T00:00:00.000Z";
    /// A hundred seconds later: the dispatch's own records slip between this and
    /// [`OPENED`], which is the sawtooth a merged store's several clocks make.
    const WORKING: &str = "2026-08-22T00:01:40.000Z";
    /// A second after that, and the last stamp the store carries.
    const SETTLED: &str = "2026-08-22T00:01:41.000Z";

    let world = World::new("views-telemetry-skew");
    let run = settled(&world, "skewed", vec![agent("build", &[])]);

    // llmlint: ignore-block[tests_mirror_real_usage] the records, the streams, the
    // sequences and the reader are all real; only the clock is arranged, because
    // clock skew is the one thing a journey cannot ask a producer for.
    let journal = world.run_file(&run, "events.jsonl");
    let mut records: Vec<Envelope> = std::fs::read_to_string(&journal)
        .expect("the journal reads")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a record reads back as an envelope"))
        .collect();
    let (mut dispatched, mut over, mut ahead) = (false, false, false);
    for record in &mut records {
        over |= record.kind.0 == "node-settled";
        record.ts = if over {
            SETTLED.to_string()
        } else if dispatched && record.source == Source::Agentgraph {
            ahead = !ahead;
            if ahead { WORKING } else { OPENED }.to_string()
        } else {
            OPENED.to_string()
        };
        dispatched |= record.kind.0 == "node-dispatched";
    }
    let skewed: String = records
        .iter()
        .map(|record| {
            format!(
                "{}\n",
                serde_json::to_string(record).expect("the envelope serialises")
            )
        })
        .collect();
    std::fs::write(&journal, skewed).expect("the store is written back");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let document = world.run(&["telemetry", &run]).json();
    let wall = document["wall_ms"].as_u64().expect("a wall clock");
    let scheduling = document["buckets"]
        .as_array()
        .expect("buckets")
        .iter()
        .find(|bucket| bucket["name"] == "scheduling")
        .and_then(|bucket| bucket["ms"].as_u64())
        .expect("a measured scheduling bucket");
    // The premise, checked rather than assumed: this store's dispatch is charged
    // more than the whole run's wall clock, and `scheduling` holds nothing to
    // take the difference out of. That is the document that used to ship with
    // parts longer than its whole.
    assert_eq!(wall, 101_000, "{document}");
    assert_eq!(scheduling, 0, "{document}");
    assert!(
        document["buckets"]
            .as_array()
            .expect("buckets")
            .iter()
            .any(|bucket| bucket["name"] == "agent" && bucket["ms"].as_u64() > Some(0)),
        "the dispatch is charged nothing, so nothing overcounted: {document}"
    );
    assert_eq!(
        measured(&document),
        wall,
        "the buckets do not sum to WALL: {document}"
    );
    // Absent stays absent: an overcount comes off what was measured and is never
    // charged to a bucket nothing measured.
    for absent in ["judge", "llmlint", "gate"] {
        let bucket = document["buckets"]
            .as_array()
            .expect("buckets")
            .iter()
            .find(|bucket| bucket["name"] == absent)
            .expect("the bucket is still named");
        assert!(
            bucket.get("ms").is_none(),
            "{absent} reported a span nothing measured: {bucket}"
        );
    }
}

/// The same direction taken past what one bucket can give back: an overcount
/// **larger than the longest span the run measured**, which has to keep coming
/// off the next-longest until it is gone.
///
/// The run above skews one producer and the residue fits inside the dispatch it
/// skewed. This one skews every stream, on a clock apiece, so the overcount is
/// spread across the dispatch, the publication and the workspace setup and no
/// one of the three can give it back alone.
///
/// What is arranged is the **clock** alone: the records, the streams, the
/// sequences and the reader are the ones this run produced.
#[test]
fn telemetry_drains_an_overcount_past_the_longest_span_onto_the_next() {
    /// The first record, and the low half of every sawtooth after it.
    const OPENED: &str = "2026-08-22T00:00:00.000Z";
    /// The high half: fifty seconds is longer than the whole run's wall clock,
    /// so one step alone already overcounts it.
    const LATER: &str = "2026-08-22T00:00:50.000Z";
    /// The forward half of the publishing session's own sawtooth, on a clock of
    /// its own.
    const MIDWAY: &str = "2026-08-22T00:00:20.000Z";
    /// The last record, one second after the first — the whole this run's parts
    /// have to add back up to.
    const CLOSED: &str = "2026-08-22T00:00:01.000Z";

    let world = World::new("views-telemetry-drain");
    world.repository("local-direct", &[]);
    world.script("service.work", "the worker wrote this\n");
    let run = settled(&world, "drained", vec![lifecycle("service", &[])]);

    // llmlint: ignore-block[tests_mirror_real_usage] the records, the streams, the
    // sequences and the reader are all real; only the clock is arranged, because
    // clock skew is the one thing a journey cannot ask a producer for.
    let journal = world.run_file(&run, "events.jsonl");
    let mut records: Vec<Envelope> = std::fs::read_to_string(&journal)
        .expect("the journal reads")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a record reads back as an envelope"))
        .collect();
    // Every stream ends on the closing stamp, so whichever the merge emits last
    // carries it and the wall clock is the second between the first record and
    // the last. A stream's last record is its highest `seq` — the order the
    // reader merges that stream in — and not the line it happens to sit on.
    let mut tails: std::collections::BTreeMap<String, (u64, usize)> =
        std::collections::BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        let tail = tails
            .entry(record.stream.clone())
            .or_insert((record.seq, index));
        if record.seq >= tail.0 {
            *tail = (record.seq, index);
        }
    }
    let closing: std::collections::BTreeSet<usize> =
        tails.values().map(|(_, index)| *index).collect();
    let mut ahead: std::collections::BTreeMap<String, bool> = std::collections::BTreeMap::new();
    for (index, record) in records.iter_mut().enumerate() {
        record.ts = if index == 0 || closing.contains(&index) {
            if index == 0 { OPENED } else { CLOSED }.to_string()
        } else {
            let ahead = ahead.entry(record.stream.clone()).or_default();
            *ahead = !*ahead;
            if *ahead {
                if record.source == Source::Vcs {
                    MIDWAY
                } else {
                    LATER
                }
            } else {
                OPENED
            }
            .to_string()
        };
    }
    // The arranged clock, kept for the failure messages: what this test asserts
    // is a property of an order the reader derives, so a red run that cannot
    // show the stamps it derived it from says nothing a reader can act on.
    let store: Vec<String> = records
        .iter()
        .map(|record| {
            format!(
                "{} {} {} {}",
                record.stream, record.seq, record.kind.0, record.ts
            )
        })
        .collect();
    let skewed: String = records
        .iter()
        .map(|record| {
            format!(
                "{}\n",
                serde_json::to_string(record).expect("the envelope serialises")
            )
        })
        .collect();
    std::fs::write(&journal, skewed).expect("the store is written back");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let document = world.run(&["telemetry", &run]).json();
    let wall = document["wall_ms"].as_u64().expect("a wall clock");
    assert_eq!(wall, 1_000, "{document}\n{store:#?}");
    let spans: Vec<(&str, u64)> = document["buckets"]
        .as_array()
        .expect("buckets")
        .iter()
        .filter_map(|bucket| {
            Some((
                bucket["name"].as_str()?,
                bucket.get("ms").and_then(serde_json::Value::as_u64)?,
            ))
        })
        .collect();
    // The premise, checked rather than assumed: the run charged three different
    // buckets, so the residue had somewhere further to go than the first one it
    // came off.
    for charged in ["agent", "publication_wait", "setup"] {
        assert!(
            spans.iter().any(|(name, _)| *name == charged),
            "{charged} was never measured, so nothing overcounted through \
             it: {document}\n{store:#?}"
        );
    }
    assert_eq!(
        measured(&document),
        wall,
        "the buckets do not sum to WALL: {document}\n{store:#?}"
    );
    // And it went there: every measured span but one was taken down to nothing.
    // A residue the longest span absorbed alone could not leave this — the other
    // two would still be carrying tens of seconds against a wall clock of one.
    let standing: Vec<&(&str, u64)> = spans.iter().filter(|(_, ms)| *ms > 0).collect();
    assert_eq!(
        standing.len(),
        1,
        "{spans:?} against a wall of {wall}\n{store:#?}"
    );
    assert_eq!(standing[0].1, wall, "{spans:?}");
    // Absent stays absent: an overcount comes off what was measured and is never
    // charged to a bucket nothing measured.
    for absent in ["judge", "llmlint"] {
        let bucket = document["buckets"]
            .as_array()
            .expect("buckets")
            .iter()
            .find(|bucket| bucket["name"] == absent)
            .expect("the bucket is still named");
        assert!(
            bucket.get("ms").is_none(),
            "{absent} reported a span nothing measured: {bucket}"
        );
    }
}

/// The number a run is budgeted against, on a host whose routine failure mode
/// is quota exhaustion across five identities.
#[test]
fn telemetry_reports_what_each_party_spent() {
    let world = World::new("views-usage");
    let run = settled(&world, "spent", vec![agent("build", &[])]);

    let document = world.run(&["telemetry", &run]).json();
    let usage = &document["usage"];
    let total = &usage["total"];
    assert!(
        total["input"].as_u64().is_some_and(|tokens| tokens > 0),
        "the run reported no input tokens: {document}"
    );
    assert!(
        total["output"].as_u64().is_some_and(|tokens| tokens > 0),
        "the run reported no output tokens: {document}"
    );
    assert!(total["cost_usd"].as_f64().is_some(), "{document}");

    // The split between the sides of a two-party member is in the report it
    // settled with, which is where it is read from.
    assert!(
        usage["agent"]["input"].as_u64().is_some_and(|t| t > 0),
        "the agent side reported nothing: {document}"
    );
    assert!(
        usage["judge"]["input"].as_u64().is_some_and(|t| t > 0),
        "the judge side reported nothing: {document}"
    );
    // Nothing in this stack runs an LLM-lint pass, so it is absent rather than
    // present and zero.
    assert!(usage.get("llmlint").is_none(), "{document}");

    world
        .run(&["telemetry", &run, "--breakdown"])
        .exited(0)
        .out_has("usage agent")
        .out_has("usage total");
}

/// A publication and a lock wait are the two stretches an operator most needs
/// answered apart from the agent's — and a lifecycle node spends real time in
/// both.
///
/// The **publication** is held here, at the one point a journey can hold it from
/// outside: git runs the repository's own `pre-push` hook at the publishing push,
/// and this one waits for a file. That is also where the `gate` bucket went. No
/// library this crate composes runs a verification tier of its own any more — the
/// repository's merge path is the verifier — so nothing measures a gate, and the
/// bucket is served absent, which is what the contract says of a bucket nothing in
/// the stack measures. The stretch itself is not lost: it is the publication
/// waiting on its merge path, which is what it now is.
///
/// The lock wait cannot be held, and not for want of trying:
/// the sibling emits `lock-wait` *after* it has waited, carrying the elapsed
/// seconds in its payload, and then emits `lock-acquired` immediately — so the
/// interval this crate measures between the two is the cost of writing two
/// records, however long the wait was. The `onevcs` double emitted the marker
/// and *then* blocked, which is a shape no release of that library has ever
/// produced, and it is what let this bucket read as measured. The bucket is
/// still served, and it is still not the agent's; what it is not is a
/// measurement. Recorded as divergence 16 in `docs/contract-divergences.md`.
#[test]
fn telemetry_separates_publication_and_lock_time_from_agent_time() {
    let world = World::new("views-publicationtime");
    let go = world.fakes.join("push.go");
    // The publishing push is held open for a measurable span, so its bucket is a
    // real duration on the clock rather than a bucket that merely exists. Declared
    // after the world, so its release runs before the world takes the tree away.
    let held = crate::harness::held_publication(&world, &go);
    world.repository("local-direct", &held.argv());
    world.script("service.work", "the worker wrote this\n");
    let path = world.plan("held", &plan_of("held", vec![lifecycle("service", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);

    world.until("the publication to reach its merge path", |world| {
        !world.events_of("held", "merge-queued").is_empty()
    });
    let since = std::time::Instant::now();
    world.until("the merge path to last a measurable stretch", |_| {
        since.elapsed() >= HELD
    });
    held.release();
    world.until("the run to settle", |world| {
        world.run_file("held", "result.json").is_file()
    });

    let document = world.run(&["telemetry", "held"]).json();
    let span = |name: &str| {
        document["buckets"]
            .as_array()
            .expect("buckets")
            .iter()
            .find(|bucket| bucket["name"] == name)
            .unwrap_or_else(|| panic!("a {name} bucket"))
            .get("ms")
            .and_then(serde_json::Value::as_u64)
    };
    // The held stretch is its own bucket, and it is not inside the agent's: the
    // whole point of the eight-way split.
    let publication = span("publication_wait")
        .unwrap_or_else(|| panic!("publication_wait is unmeasured: {document}"));
    assert!(
        publication >= FLOOR,
        "publication_wait measured {publication}ms of a stretch held for {}ms: {document}",
        HELD.as_millis()
    );
    assert!(
        span("agent").is_some(),
        "the agent bucket is unmeasured: {document}"
    );
    // And the bucket that used to hold it is served with no measurement on it:
    // this run published through a real merge path and no gate ran, because
    // nothing in the stack runs one. A number here would be a stretch nothing
    // spent.
    let bucket = document["buckets"]
        .as_array()
        .expect("buckets")
        .iter()
        .find(|bucket| bucket["name"] == "gate")
        .unwrap_or_else(|| panic!("the gate bucket is not served at all: {document}"));
    assert!(
        bucket.get("ms").is_none(),
        "a run whose verification was its repository's own merge path charged a gate: \
         {document}"
    );
    // And the wait the sibling did do is on its own record, as the number a
    // reader would need to charge it: this is the datum the bucket below is
    // missing, and it is deterministic — the payload either carries the elapsed
    // seconds or it does not.
    let waited = &world.events_of("held", "lock-wait")[0];
    assert!(
        waited["payload"]["elapsed"].is_number(),
        "the sibling recorded no elapsed lock wait to charge: {waited}"
    );
    // The bucket is **served** — an unmeasured stretch must not read as absent
    // any more than a measured zero — and it is **not a measurement**: what it
    // spans is the cost of the sibling writing two records back to back, which
    // is below the threshold this journey calls measurable however long the
    // publication actually waited. Bounded rather than fixed at zero: the two
    // timestamps are real and millisecond-precise, so on a slow enough host —
    // under coverage instrumentation, reliably — they differ by one. The
    // `onevcs` double emitted the marker and *then* blocked, which is what made
    // an exact number look like a fact here.
    let lock_wait = span("lock_wait").unwrap_or_else(|| {
        panic!("the lock_wait bucket is absent, not served as unmeasured: {document}")
    });
    assert!(
        lock_wait < FLOOR,
        "lock_wait measured {lock_wait}ms, which is a stretch rather than the cost of two \
         appends — if the wait is genuinely charged now, hold this journey to the wait: \
         {document}"
    );
    assert!(
        span("setup").is_some(),
        "setup is unmeasured for a run that published: {document}"
    );
    assert_eq!(
        measured(&document),
        document["wall_ms"].as_u64().expect("a wall clock"),
        "{document}"
    );
}
#[test]
fn telemetry_counts_a_no_diff_node_without_counting_a_dispatch() {
    let world = World::new("views-nodiff");
    let run = settled(
        &world,
        "counted",
        vec![serde_json::json!({
            "id": "handoff",
            "task": "## What\nNothing changes.",
            "expects_no_diff": true,
        })],
    );

    let document = world.run(&["telemetry", &run]).json();
    assert_eq!(document["no_diff"], 1);
    assert_eq!(document["dispatches"], 0);
    assert_eq!(
        measured(&document),
        document["wall_ms"].as_u64().expect("a wall clock")
    );
    // A run that dispatched nothing spent nothing, and says so by absence.
    assert!(
        document["usage"].as_object().is_some_and(|u| u.is_empty()),
        "{document}"
    );
}

/// A dispatch whose report was never written where its settlement said.
///
/// The settlement still names where it went, so the evidence is missing rather
/// than the result — and every view that would have read it says which of the
/// two it is meeting instead of reporting a dispatch that did nothing.
#[test]
fn evidence_this_run_could_not_keep_is_reported_as_unread_rather_than_as_nothing() {
    let world = World::new("views-unread");
    world.script("report.missing", "");
    let run = settled(&world, "unread", vec![agent("build", &[])]);

    let transcript = world.run(&["transcript", &run]);
    transcript
        .exited(0)
        .out_has("unread  build")
        // The turn's tools are in the merged store and still render.
        .out_has("tool_call bash")
        // The words are not, and the line says so rather than omitting the
        // report it has no copy of.
        .out_has("not retained by this run");

    // Same fact in the telemetry: the member's total is on the wire, and the
    // split between its two sides was only ever in the report.
    let usage = world.run(&["telemetry", &run]).json()["usage"].clone();
    assert!(
        usage["total"]["input"].as_u64().is_some_and(|t| t > 0),
        "{usage}"
    );
    assert!(
        usage.get("agent").is_none() && usage.get("judge").is_none(),
        "an unreadable report was reported as a measured split: {usage}"
    );
}

/// Given no node, the verb covers every node the run dispatched.
///
/// The form an agent reaches for first: it does not yet know which node it is
/// looking for, which is the whole reason to read a transcript.
#[test]
fn transcript_given_no_node_renders_every_dispatch_the_run_recorded() {
    let world = World::new("views-alltranscripts");
    let run = settled(
        &world,
        "everynode",
        vec![agent("first", &[]), agent("second", &["first"])],
    );

    let transcript = world.run(&["transcript", &run]);
    transcript.exited(0);
    for node in ["first", "second"] {
        transcript.out_has(&format!("everynode  {node}"));
    }
    // Each with its own turn, its own tools, and its own retained report. Two
    // tool lines per node, because the verb reads both sources it names: the
    // bounded summary the store carried while the turn ran, and the structured
    // input the report kept once it settled.
    assert_eq!(
        transcript
            .stdout
            .lines()
            .filter(|line| line.contains("tool_call bash"))
            .count(),
        4,
        "a node's tools were missed or doubled:\n{}",
        transcript.stdout
    );
    assert_eq!(
        transcript
            .stdout
            .lines()
            .filter(|line| line.trim_start().starts_with("report "))
            .count(),
        2,
        "a node's retained report was missed or doubled:\n{}",
        transcript.stdout
    );
}

/// A settlement naming a file the producing library never writes.
///
/// Nothing follows it: the run keeps its own copy of the evidence it ingests,
/// and every reader afterwards opens only that. The refusal is said out loud
/// where the ingest happened, and the transcript says the report is not there.
#[test]
fn a_settlement_naming_a_file_the_producer_never_writes_is_refused_out_loud() {
    let world = World::new("views-elsewhere");
    world.script("report.elsewhere", "");
    let run = driven(&world, "elsewhere", vec![agent("build", &[])]);
    // Refused as it was ingested, naming the file and why.
    run.1
        .err_has("not retaining the report at")
        .err_has("notes.json");

    let transcript = world.run(&["transcript", &run.0]);
    transcript
        .exited(0)
        // The turn's own record still renders; only the named file is refused.
        .out_has("tool_call bash")
        .out_has("not retained by this run");
    assert!(
        !transcript.stdout.contains(PLANTED_WORDS),
        "a file the producer never writes was read anyway:\n{}",
        transcript.stdout
    );
}

/// A settlement naming a **symlink** that wears the producer's own file name.
///
/// The one case a name check alone cannot catch: the path says `report.json`
/// and delivers something else. Ingest does not follow it, so there is nothing
/// for a reader to open.
#[test]
fn a_settlement_naming_a_symlink_is_refused_and_never_followed() {
    let world = World::new("views-symlink");
    world.script("report.symlink", "");
    let run = driven(&world, "linked", vec![agent("build", &[])]);
    run.1
        .err_has("not retaining the report at")
        .err_has("symlink");

    let transcript = world.run(&["transcript", &run.0]);
    transcript.exited(0).out_has("not retained by this run");
    assert!(
        !transcript.stdout.contains(PLANTED_WORDS),
        "the symlink was followed to what it pointed at:\n{}",
        transcript.stdout
    );
}

/// A settlement naming something that is not a file, and one naming a file past
/// the bound a copy will take.
///
/// Both are refused as they are ingested and both leave the reader saying the
/// report is not there — the shapes a name check alone would wave through, and
/// the one that would let a producer fill the runs root or stall the writer
/// that is copying it.
#[test]
fn a_settlement_naming_a_directory_or_an_oversize_file_is_refused_at_ingest() {
    // A directory is the one shape the two platforms refuse at a different
    // step, so its *reason* is the one thing written per-platform here. Unix
    // opens a directory, and the metadata taken from that handle says what it
    // is; Windows will not hand out a handle to one at all, so ingest never
    // gets past the open. Refused either way, without reading it either way —
    // which is what the rest of this journey asserts, unchanged on both.
    #[cfg(unix)]
    let a_directory_is = "it is not a file";
    #[cfg(not(unix))]
    let a_directory_is = "it cannot be opened as a plain file";

    for (scripted, why) in [
        ("report.directory", a_directory_is),
        ("report.oversize", "larger than"),
    ] {
        let world = World::new(&format!("views-{}", scripted.replace('.', "-")));
        world.script(scripted, "");
        let run = driven(&world, "refused", vec![agent("build", &[])]);
        run.1.err_has("not retaining the report at").err_has(why);

        let transcript = world.run(&["transcript", &run.0]);
        transcript.exited(0).out_has("not retained by this run");
        assert!(
            !transcript.stdout.contains(PLANTED_WORDS),
            "'{scripted}' was copied and read anyway:\n{}",
            transcript.stdout
        );
    }
}

/// A settlement written into the journal *after* the fact, naming a readable
/// file outside anything this run owns.
///
/// The threat the confinement exists for: a line in the store is not a
/// producer, and a reader that opened what one pointed at would print any JSON
/// document on the host. Only what the run copied at ingest is ever read.
#[test]
fn a_journal_line_naming_a_file_outside_the_run_is_never_read() {
    let world = World::new("views-outofroot");
    let run = settled(&world, "planted", vec![agent("build", &[])]);

    // A readable report, in the producing library's own file name, outside every
    // directory this run owns.
    let outside = world.root.join("outside");
    std::fs::create_dir_all(&outside).expect("a directory outside the run");
    let planted = outside.join(oneagentgraph::member::REPORT_FILE);
    std::fs::write(&planted, PLANTED_DOCUMENT).expect("a planted report");

    // llmlint: ignore-block[tests_mirror_real_usage] the state is **a settlement no
    // producer emitted**, and it has no engine-side constructor: a verb that appended one
    // to a run's journal would be the very defect this journey exists to catch, so
    // appending it here is the only way to reach the condition. The line is not written by
    // hand — it is the settlement this run really recorded, read back as an
    // `event::Envelope` and re-serialised by the implementation the journal writer
    // serialises with, so its kind, the labels the enricher stamped, and the key a report
    // path rides under cannot drift from what a producer's line carries. Everything
    // asserted after it is through the CLI, which is where the claim lives.
    //
    // Four fields are the forgery's: the path, of course; a timestamp after everything the
    // run really wrote; and a stream and sequence no producer used, which matter because
    // the run names its own copy of a report from exactly those two — so this is a
    // settlement whose report was never kept.
    let mut forged = world
        .journal(&run)
        .into_iter()
        .filter_map(|event| serde_json::from_value::<Envelope>(event).ok())
        .find(|event| {
            event.source == Source::Agentgraph && event.payload.values().any(names_a_report_file)
        })
        .expect("the settled run recorded a settlement naming a report");
    forged.ts = "2099-01-01T00:00:00.000Z".to_string();
    forged.stream = "forged".to_string();
    forged.seq = 0;
    let mut repointed = 0;
    for value in forged.payload.values_mut() {
        if names_a_report_file(value) {
            *value = serde_json::json!(planted.display().to_string());
            repointed += 1;
        }
    }
    assert_eq!(
        repointed, 1,
        "the settlement's report path was not the one thing repointed: {forged:?}"
    );
    let journal = world.run_file(&run, "events.jsonl");
    let mut existing = std::fs::read_to_string(&journal).expect("the journal reads");
    existing.push_str(&format!(
        "{}\n",
        serde_json::to_string(&forged).expect("the envelope serialises")
    ));
    std::fs::write(&journal, existing).expect("the journal is appended to");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let transcript = world.run(&["transcript", &run]);
    transcript
        .exited(0)
        .out_has(&planted.display().to_string())
        .out_has("not retained by this run");
    assert!(
        !transcript.stdout.contains(PLANTED_WORDS),
        "a path a journal line named was read:\n{}",
        transcript.stdout
    );
    // And the same line buys nothing in the telemetry, which reads the same copy.
    let usage = world.run(&["telemetry", &run]).json()["usage"].clone();
    assert!(
        usage.get("agent").is_some(),
        "the real report still counts: {usage}"
    );
}

/// A report a harness produced without a transcript. It is a report this build
/// can say nothing further about, which is not a dispatch that did nothing.
#[test]
fn a_retained_report_carrying_no_transcript_says_so() {
    let world = World::new("views-bare");
    world.script("report.bare", "");
    let run = settled(&world, "bare", vec![agent("build", &[])]);

    world
        .run(&["transcript", &run])
        .exited(0)
        .out_has("report ")
        .out_has("it carries no transcript");
}

/// What a tool returned, through the verb, in the three shapes an output
/// reaches a reader in: a structured one, one carrying control characters, and
/// one longer than the line will print.
///
/// The rendering used to be a blank column for all three — a `tool_result`
/// states its text under `output` and states no `detail`, and `detail` was the
/// whole column — so this drives the compiled binary over a store that carries
/// each of them and reads the line back.
///
/// The two sources answer at different times and are bounded differently, so
/// both are here: the run's own journal, whose payload texts this crate cut and
/// marked at ingest, and the retained report, whose outputs are a harness's raw
/// bytes with nothing bounding them at all. It is the report path that proves
/// the view's own ceiling, because it is the only one that can exceed it.
#[test]
fn a_transcript_prints_a_tools_output_structured_stripped_bounded_and_labelled() {
    let world = World::new("views-outputs");
    let run = settled(&world, "outputs", vec![agent("build", &[])]);

    // llmlint: ignore-block[tests_mirror_real_usage] the run, its settlement and the
    // reader are real; what is planted is a producer's *content* — outputs in shapes
    // a scripted double settles too quickly to produce, and which a journey cannot
    // ask a real harness for on demand.
    let settlement = world
        .journal(&run)
        .into_iter()
        .filter_map(|event| serde_json::from_value::<Envelope>(event).ok())
        .find(|event| event.source == Source::Agentgraph && event.kind.0 == "member-settled")
        .expect("the settled run recorded a settlement");

    // The journal half: an output the producer had already cut short, carrying a
    // control character a rendered line must not be able to obey, and one a
    // harness relayed as the structure it really had.
    let relayed = world
        .journal(&run)
        .into_iter()
        .filter_map(|event| serde_json::from_value::<Envelope>(event).ok())
        .find(|event| event.kind.0 == "turn-activity")
        .expect("the dispatch relayed its activity");
    let journal = world.run_file(&run, "events.jsonl");
    let mut store = std::fs::read_to_string(&journal).expect("the journal reads");
    for (seq, payload) in [
        (
            1_000,
            serde_json::json!({
                "kind": "tool_result",
                "output": "cleared\u{1b}[2K the line",
                "output_truncated": true,
                "index": 9,
            }),
        ),
        // A harness that relayed the structure it really had rather than text.
        // The store carries it under the same key, so the verb reads it the one
        // way it reads a retained report's — a reader that took only the string
        // case would drop this back to the blank column the verb was corrected
        // for.
        (
            1_001,
            serde_json::json!({
                "kind": "tool_result",
                "output": {"exit": 0, "stdout": "relayed, not text"},
                "index": 10,
            }),
        ),
    ] {
        let mut record = relayed.clone();
        record.seq += seq;
        record.payload = payload.as_object().expect("a payload").clone();
        store.push_str(&format!(
            "{}\n",
            serde_json::to_string(&record).expect("the envelope serialises")
        ));
    }
    std::fs::write(&journal, store).expect("the store is appended to");

    // The report half: this run's own copy, at the name the reader derives from
    // the settlement rather than the path the producer named.
    let kept = onepipeline::views::RunPaths::under(&world.runs, &run)
        .report_for(&settlement.stream, settlement.seq);
    std::fs::write(
        &kept,
        serde_json::json!({
            "schema_version": 7,
            "transcript": {"messages": [{"role": "assistant", "content": "read it back", "events": [
                // A harness that answers with the structure it really had rather
                // than with text.
                {"kind": "tool_result", "index": 0,
                 "output": {"exit": 0, "stdout": "structured, not text"}},
                // And one past what a line prints, which only this path can be.
                {"kind": "tool_result", "index": 1,
                 "output": format!("{}the tail nobody sees", "y".repeat(5_000))},
                // A producer that stated its truncation flag as something this
                // build cannot read as either answer.
                {"kind": "tool_result", "index": 2, "output": "a flag nobody can read",
                 "output_truncated": "sometimes"},
            ]}]},
        })
        .to_string(),
    )
    .expect("this run's own copy of the report");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let transcript = world.run(&["transcript", &run, "build"]);
    transcript.exited(0);
    // Stripped, and said: the escape is a space on the rendered line, and the
    // output the producer had already cut is not offered as a whole one.
    transcript.out_has("tool_result   cleared [2K the line … [already cut short by the producer]");
    assert!(
        !transcript.stdout.contains('\u{1b}'),
        "a control character in a tool's output reached the rendered line:\n{}",
        transcript.stdout
    );
    transcript.out_has(r#"tool_result   {"exit":0,"stdout":"relayed, not text"}"#);
    transcript.out_has(r#"tool_result   {"exit":0,"stdout":"structured, not text"}"#);
    transcript.out_has("… [4096 of 5020 characters]");
    transcript.out_lacks("the tail nobody sees");
    // And a flag that is neither answer is reported as neither, rather than
    // read as the one that claims the output is whole.
    transcript
        .out_has(r#"a flag nobody can read … [the producer's truncation flag is unreadable]"#);
}

/// A dispatch that has recorded something without naming a tool claims the
/// count and the age, and nothing more.
///
/// The session a lifecycle node opens is recorded before its first turn is, so
/// this is the state every lifecycle node passes through — and it is where a
/// readout that invented a "now" would be inventing it.
#[test]
fn a_dispatch_that_has_named_no_tool_reports_its_count_rather_than_a_guess() {
    let world = World::new("views-nameless");
    world.repository("local-direct", &[]);
    world.script("service.wait", "hold");
    let path = world.plan(
        "nameless",
        &plan_of("nameless", vec![lifecycle("service", &[])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the session to be recorded", |world| {
        !world.events_of("nameless", "session-opened").is_empty()
    });

    let status = world.run(&["status", "nameless"]);
    status
        .exited(0)
        .out_has("service: running")
        .out_has("event(s)");
    assert!(
        !status.stdout.contains("now "),
        "a dispatch that has named no tool was reported doing one:\n{}",
        status.stdout
    );
    world.release("service.go");
}
/// How many seconds ago one node's `status` line says it last did anything.
///
/// Read off the rendered line rather than out of the journal: the claim under
/// test is what an operator sees, and an age taken from anywhere else would pass
/// while the line said something different.
fn seconds_since_activity(status: &str, node: &str) -> u64 {
    let line = status
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("{node}: running")))
        .unwrap_or_else(|| panic!("`status` has no in-flight line for {node}:\n{status}"));
    let at = line
        .find(" ago")
        .unwrap_or_else(|| panic!("`{line}` carries no age"));
    let age: String = line[..at]
        .chars()
        .rev()
        .take_while(|c| *c != ' ')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    // The rendered spelling is `12s` under a minute and `1m30s` above it, and
    // the second is already past anything this journey waits for.
    age.strip_suffix('s')
        .and_then(|seconds| seconds.parse().ok())
        .unwrap_or_else(|| panic!("`{line}` carries no readable age in seconds"))
}

/// A dispatch that is only heartbeating is aged by the work it has done, not by
/// the heartbeat.
///
/// An age over every envelope can never be older than one beat for anything
/// that has not died: measured on a real run, a node reported as possibly
/// wedged said "14s ago", and would have said the same after ten silent
/// minutes. The liveness is reported beside the work rather than as it.
#[test]
fn status_ages_a_dispatch_by_its_work_rather_than_by_its_heartbeat() {
    let world = World::new("views-heartbeat");
    world.script("stuck.turn-open", "");
    world.script("stuck.wait", "hold");
    world.script("stuck.heartbeat", "100");
    // A second dispatch that heartbeats without ever announcing a turn: alive
    // from the first beat and having produced nothing at all, which is what a
    // harness that has started and not begun looks like.
    world.script("mute.wait", "hold");
    world.script("mute.heartbeat", "100");
    let path = world.plan(
        "beating",
        &plan_of("beating", vec![agent("stuck", &[]), agent("mute", &[])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);

    // Long enough that an age reading the heartbeat and one reading the work
    // cannot be confused: the turn was announced once, and the beats have gone
    // on for seconds since.
    world.until("both dispatches to heartbeat for a while", |world| {
        world
            .events_of("beating", "member-heartbeat")
            .iter()
            .filter(|event| event["labels"]["onepipeline.node"] == "stuck")
            .count()
            >= 30
            && world
                .events_of("beating", "member-heartbeat")
                .iter()
                .filter(|event| event["labels"]["onepipeline.node"] == "mute")
                .count()
                >= 30
    });

    let status = world.run(&["status", "beating"]);
    status.exited(0).out_has("stuck: running");
    // The two envelopes the turn's announcement is: a member starting and a turn
    // starting. Everything since has been a heartbeat, and none of it is work.
    status.out_has("2 event(s)");
    assert!(
        seconds_since_activity(&status.stdout, "stuck") >= 2,
        "the age of the work was taken from the heartbeat:\n{}",
        status.stdout
    );
    // And the liveness is still reported, because a dispatch that has gone quiet
    // and one that has died call for opposite actions.
    status.out_has("alive ");

    // The dispatch that has produced nothing says so, rather than claiming an
    // age for work it has not done — and it does not read as a node nothing is
    // driving, which is the opposite mistake.
    let mute = status
        .stdout
        .lines()
        .find(|line| line.trim_start().starts_with("mute: running"))
        .unwrap_or_else(|| panic!("`status` has no line for mute:\n{}", status.stdout));
    assert!(
        mute.contains("nothing recorded yet") && mute.contains("alive "),
        "a dispatch that has only ever heartbeated reads as one that has worked: {mute}"
    );
    assert!(
        !mute.contains("UNDRIVEN"),
        "a dispatch that is heartbeating reads as one nothing is driving: {mute}"
    );

    world.release("stuck.go");
    world.release("mute.go");
}

/// How long ago one node's `status` line says it was last heard from alive.
///
/// Read off the rendered line for the same reason the age of the work is: the
/// claim under test is the one an operator reads, and the line carries two ages
/// — the work's and the liveness — so this one is picked out by the word that
/// introduces it rather than by position.
fn seconds_since_alive(status: &str, node: &str) -> u64 {
    let line = status
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("{node}: running")))
        .unwrap_or_else(|| panic!("`status` has no in-flight line for {node}:\n{status}"));
    let age = line
        .split("alive ")
        .nth(1)
        .and_then(|rest| rest.split(" ago").next())
        .unwrap_or_else(|| panic!("`{line}` reports no liveness"));
    // The rendered spelling is `12s` under a minute and `1m30s` above it, and
    // the second is already past anything this journey waits for.
    age.strip_suffix('s')
        .and_then(|seconds| seconds.parse().ok())
        .unwrap_or_else(|| panic!("`{line}` carries no readable liveness age in seconds"))
}

/// A dispatch whose every envelope so far carries a stamp this build cannot
/// read has recorded nothing it can age, and says so.
///
/// The envelopes arrived — the node is not one nothing is driving — but not one
/// of them can be placed in time, so there is no moment to date the work to. An
/// age invented for it would be the same lie the whole readout exists to stop
/// telling, and "0s ago" is the worst of them: it reads as a dispatch that was
/// working a moment ago.
#[test]
fn status_reports_no_work_for_a_dispatch_whose_envelopes_cannot_be_placed_in_time() {
    let world = World::new("views-unplaceable");
    world.script("blind.turn-open", "");
    world.script("blind.wait", "hold");
    // The sibling announces itself and its turn on a clock this build cannot
    // read, so not one of its envelopes so far can be placed.
    world.script("blind.unplaceable-member-start", "");
    world.script("blind.unplaceable-turn-start", "");
    let path = world.plan(
        "unplaceable",
        &plan_of("unplaceable", vec![agent("blind", &[])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the dispatch to announce its turn", |world| {
        !world.events_of("unplaceable", "turn-started").is_empty()
    });

    let status = world.run(&["status", "unplaceable"]);
    status.exited(0);
    let line = status
        .stdout
        .lines()
        .find(|line| line.trim_start().starts_with("blind: running"))
        .unwrap_or_else(|| panic!("`status` has no line for blind:\n{}", status.stdout))
        .to_string();
    assert!(
        line.contains("nothing recorded yet"),
        "a dispatch whose envelopes cannot be placed in time is reported as having \
         worked: {line}"
    );
    assert!(
        !line.contains("event(s)") && !line.contains(" ago"),
        "an age was claimed for work nothing can date: {line}"
    );
    // And it is not the other mistake either: the envelopes did arrive, so this
    // is a dispatch that is being driven and cannot be aged, not a missing one.
    assert!(
        !line.contains("UNDRIVEN"),
        "a dispatch whose envelopes arrived unplaceable reads as one nothing is \
         driving: {line}"
    );

    world.release("blind.go");
}

/// A dispatch heard from before its stamps could be placed is dated from the
/// first arrival there was a moment for, and counted from there too.
///
/// The count and the age are one record, because a count standing on its own is
/// what a view renders as an age. So the arrivals before the first placeable one
/// are outside both: what is reported is smaller than what arrived, and every
/// bit of it is something this build actually watched happen — which is the
/// trade the whole readout is, an under-count over an invented moment.
#[test]
fn status_dates_and_counts_a_dispatch_from_its_first_placeable_envelope() {
    let world = World::new("views-clockback");
    world.script("late.turn-open", "");
    world.script("late.wait", "hold");
    // The member arrives on a clock this build cannot read, and the turn
    // announced behind it on one it can.
    world.script("late.unplaceable-member-start", "");
    let path = world.plan("clockback", &plan_of("clockback", vec![agent("late", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the dispatch to announce its turn", |world| {
        !world.events_of("clockback", "turn-started").is_empty()
    });

    // Both envelopes reached the store; only one of them is datable.
    let relayed = world
        .journal("clockback")
        .into_iter()
        .filter(|event| event["source"] == "agentgraph" && event["labels"]["node"] == "late")
        .count();
    assert_eq!(relayed, 2, "the dispatch did not announce itself twice");

    let status = world.run(&["status", "clockback"]);
    status.exited(0);
    let line = status
        .stdout
        .lines()
        .find(|line| line.trim_start().starts_with("late: running"))
        .unwrap_or_else(|| panic!("`status` has no line for late:\n{}", status.stdout))
        .to_string();
    assert!(
        line.contains("1 event(s)"),
        "the arrival nothing could place was counted, so the count claims a \
         record that reaches further back than the age beside it: {line}"
    );
    assert!(
        seconds_since_activity(&status.stdout, "late") < 60,
        "the work was dated from an arrival nothing could place: {line}"
    );

    world.release("late.go");
}

/// An arrival this build cannot place still counts, and does not move the age
/// of the work.
///
/// It happened — dropping it would report a dispatch doing less than it is —
/// and the one thing it cannot do is say when. So the record goes on counting
/// and its age stands at the last arrival there was a moment for, rather than
/// jumping to an instant nothing measured.
#[test]
fn status_counts_an_unplaceable_arrival_without_letting_it_move_the_age() {
    let world = World::new("views-midturn");
    world.script("drifting.turn-open", "");
    world.script("drifting.wait", "hold");
    // The member arrives on a clock this build can read, and the turn announced
    // behind it on one it cannot.
    world.script("drifting.unplaceable-turn-start", "");
    let path = world.plan("midturn", &plan_of("midturn", vec![agent("drifting", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the dispatch to announce its turn", |world| {
        !world.events_of("midturn", "turn-started").is_empty()
    });

    let status = world.run(&["status", "midturn"]);
    status.exited(0);
    let line = status
        .stdout
        .lines()
        .find(|line| line.trim_start().starts_with("drifting: running"))
        .unwrap_or_else(|| panic!("`status` has no line for drifting:\n{}", status.stdout))
        .to_string();
    assert!(
        line.contains("2 event(s)"),
        "the arrival nothing could place was dropped from the count, so a working \
         dispatch reads as one doing less than it is: {line}"
    );
    // Seconds, because the run has just started: an age the unplaceable arrival
    // moved would be the distance from an instant nothing measured, which is not
    // an age this run could have.
    assert!(
        seconds_since_activity(&status.stdout, "drifting") < 60,
        "an arrival nothing could place moved the age of the work: {line}"
    );

    world.release("drifting.go");
}

/// A dispatch whose clock stops being readable is still reported alive, by the
/// last beat that could be placed.
///
/// The beats keep coming — the dispatch is as alive as it ever was — so a reader
/// that dropped the liveness the moment it could not place a beat would report a
/// live worker as one nothing had been heard from. What is retained is the last
/// arrival there was a moment for, and it goes on ageing, which is the honest
/// pair: still alive, and heard from that long ago.
#[test]
fn status_keeps_the_last_placeable_beat_when_the_clock_stops_being_readable() {
    let world = World::new("views-lostclock");
    world.script("fading.turn-open", "");
    world.script("fading.wait", "hold");
    world.script("fading.heartbeat", "100");
    // One beat this build can place, and every one after it on a clock it
    // cannot read.
    world.script("fading.unplaceable-beats-after-the-first", "");
    let path = world.plan(
        "lostclock",
        &plan_of("lostclock", vec![agent("fading", &[])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);

    // Long enough that a liveness taken from the newest beat and one taken from
    // the last placeable beat cannot be confused.
    world.until("the clock to have been unreadable for a while", |world| {
        world
            .events_of("lostclock", "member-heartbeat")
            .iter()
            .filter(|event| event["labels"]["onepipeline.node"] == "fading")
            .count()
            >= 30
    });

    let status = world.run(&["status", "lostclock"]);
    status
        .exited(0)
        .out_has("fading: running")
        .out_has("alive ");
    assert!(
        seconds_since_alive(&status.stdout, "fading") >= 2,
        "the liveness was taken from a beat nothing can place, so a dispatch \
         last heard from seconds ago reads as one heard from just now:\n{}",
        status.stdout
    );

    world.release("fading.go");
}

/// A run that has dispatched nothing has no transcript, and says so rather than
/// rendering an empty one.
#[test]
fn a_run_that_has_dispatched_nothing_says_it_has_no_transcript() {
    let world = World::new("views-notranscript");
    // A human action and nothing else: the loop records it as waiting and
    // dispatches nothing at all, which is the state under test.
    let path = world.plan("quiet", &plan_of("quiet", vec![human("approve", &[])]));
    world.run(&["start", &path, "--attach"]).exited(0);

    world
        .run(&["transcript", "quiet"])
        .exited(0)
        .out_has("no dispatch has recorded a transcript");
}

#[test]
fn runs_summarises_every_recorded_run_and_says_whose_it_is() {
    let world = World::new("views-runs");
    settled(&world, "alpha", vec![agent("build", &[])]);
    settled(&world, "beta", vec![agent("build", &[])]);

    let listing = world.run(&["runs"]);
    listing
        .exited(0)
        .out_has("alpha")
        .out_has("beta")
        .out_has("[mine]")
        .out_has("done");
}

/// A directory under the runs root that records no launch is a **rejection**,
/// and every whole-root view names it.
///
/// The reading it replaces: the views listed what they could open and said
/// nothing at all about the rest, so an operator on a host holding thirty run
/// roots read `no runs recorded` and took it for an idle machine.
#[test]
fn a_run_root_the_views_refuse_is_named_with_its_reason() {
    let world = World::new("views-skipped");
    let run = settled(&world, "readable", vec![agent("build", &[])]);
    // llmlint: ignore-block[tests_mirror_real_usage] all three states are the
    // *filesystem's* rather than any command's, which is why the views met them in the
    // first place: **a run root with no launch record** (a crash between the directory and
    // the record, or a directory an operator left beside the runs), **a launch record that
    // is not a launch record** (a document with none of what this build needs to say
    // anything about the run), and **a launch record that is a directory**. None has an
    // engine-side constructor, and a verb that made one would be the defect. The run
    // beside them is launched through the CLI, and every claim is read off the CLI.
    // A run root left half-written: the directory is there and the launch record
    // that says who owns it is not.
    std::fs::create_dir_all(world.runs.join("half-written")).expect("a run root with no launch");
    // And one whose launch record this build genuinely cannot read: `results`
    // words that refusal with the file and what the record was missing.
    std::fs::create_dir_all(world.runs.join("not-a-record")).expect("a run root");
    std::fs::write(
        world.runs.join("not-a-record").join("launch.json"),
        r#"{"oops": true}"#,
    )
    .expect("a launch record this build cannot read");
    // And one whose launch record is there and is not a record at all: absent
    // and "present as something else" are different things to tell a reader.
    std::fs::create_dir_all(world.runs.join("impostor").join("launch.json"))
        .expect("a launch record that is a directory");
    // llmlint: ignore-end[tests_mirror_real_usage]

    for view in [vec!["runs"], vec!["status"], vec!["goals"]] {
        world
            .run(&view)
            .exited(0)
            .out_has(&run)
            .out_has("3 run root(s) skipped")
            .out_has("half-written")
            .out_has("launch.json")
            .out_has("missing field `run_id`")
            .out_has("is not a file");
    }
    // `host` lists dispatches rather than runs, so the run it read is not on it
    // — but a root it could not read is a dispatch it cannot see, and it says so.
    world
        .run(&["host"])
        .exited(0)
        .out_has("3 run root(s) skipped")
        .out_has("half-written");
}

/// A run whose launch record carries a field this build has never had is **read**,
/// and the whole-host view reports the run.
///
/// The defect this replaces: the launch record was deserialized strictly, so a
/// key another build of this crate wrote took the *whole* record with it, and
/// the run vanished from the view an operator opens to see what is running on
/// their machine. The refusal even named the field, which is what makes it the
/// wrong answer: this build knew exactly what it was refusing over.
///
/// The record here is the one this build wrote for a real run driven through the
/// CLI, with the stranger's key added to it — so what is proved is a record that
/// is otherwise entirely ordinary, which is what a record from a neighbouring
/// build is.
#[test]
fn a_run_whose_record_carries_a_field_this_build_never_had_is_still_reported() {
    let world = World::new("views-newer-record");
    let run = settled(&world, "from-a-newer-build", vec![agent("build", &[])]);
    // llmlint: ignore-block[tests_mirror_real_usage] no verb of this build writes a key
    // this build does not have — that is the point of it — so the only way to hold a
    // record another build wrote is to put the key on the record this one wrote. The run
    // is launched and settled through the CLI, and every claim below is read off the CLI.
    let record = world.runs.join(&run).join("launch.json");
    let mut written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&record).expect("the launch record"))
            .expect("a launch record");
    written["channel_id"] = serde_json::json!("a field a later build removed");
    std::fs::write(&record, written.to_string()).expect("a launch record from another build");
    // llmlint: ignore-end[tests_mirror_real_usage]

    for view in [vec!["runs"], vec!["status"], vec!["goals"]] {
        let rendered = world.run(&view);
        rendered.exited(0).out_has(&run);
        rendered.out_lacks("run root(s) skipped");
        rendered.out_lacks("channel_id");
    }
    world.run(&["results", &run]).exited(0).out_has(&run);
}

/// A root whose every run was refused is not a root with nothing in it.
///
/// The two used to render identically, and only one of them means there is
/// nothing running — which is the reading a planner acts on by starting more
/// work on a machine that is already full.
#[test]
fn a_root_whose_every_run_is_refused_does_not_read_as_an_empty_one() {
    let world = World::new("views-allrefused");
    // llmlint: ignore-block[tests_mirror_real_usage] the same filesystem state as the
    // journey above, and for the same reason: no command makes a run root with no launch
    // record. What is asserted is the CLI's answer to it.
    std::fs::create_dir_all(world.runs.join("half-written")).expect("a run root with no launch");
    // llmlint: ignore-end[tests_mirror_real_usage]

    for view in [vec!["runs"], vec!["status"], vec!["goals"]] {
        let rendered = world.run(&view);
        rendered
            .exited(0)
            .out_has("no run under")
            .out_has("1 run root(s) skipped")
            .out_has("half-written");
        assert!(
            !rendered.stdout.contains("no runs recorded"),
            "a rejected run root was reported as an absence:\n{}",
            rendered.stdout
        );
    }
}

/// A run another planner owns is not this one's, and `--mine` filtering it out
/// is not the same fact as a root that could not be read.
///
/// The empty view has to say which of the two it met: `no runs recorded` for a
/// listing nothing matched, and the roots it refused named beside it either way.
#[test]
fn mine_filtering_everything_out_is_not_the_same_as_a_root_that_could_not_be_read() {
    let world = World::new("views-mine-skipped");
    // The run belongs to another planner's session, so `--mine` has nothing to
    // list — while the root beside it is still one this build refused.
    let stranger = world.as_session("session-other");
    settled(&stranger, "theirs", vec![agent("build", &[])]);
    // llmlint: ignore-block[tests_mirror_real_usage] as above: a run root with no launch
    // record is a filesystem state no command produces. The run beside it is another
    // planner's, launched through the CLI as that planner would.
    std::fs::create_dir_all(world.runs.join("half-written")).expect("a run root with no launch");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let rendered = world.run(&["runs", "--mine"]);
    rendered
        .exited(0)
        // A run was read; it was simply not this session's.
        .out_has("no runs recorded")
        .out_has("1 run root(s) skipped")
        .out_has("half-written");
    assert!(
        !rendered.stdout.contains("no run under"),
        "a run that read was reported as one that could not:\n{}",
        rendered.stdout
    );
}

/// A stopped run's dispatches are not live dispatches.
///
/// The one proof that does not depend on a process at all: the run's own ledger
/// records that it was ended, and a row rendered from it afterwards claims a
/// worker the stop was aimed at.
#[test]
fn host_never_renders_a_dispatch_of_a_run_that_was_stopped() {
    let world = World::new("views-stopped");
    world.script("build.wait", "hold");
    let path = world.plan("halted", &plan_of("halted", vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the dispatch to be in flight", |world| {
        !world.events_of("halted", "node-dispatched").is_empty()
    });
    world.run(&["host"]).exited(0).out_has("build");

    world.run(&["stop", "halted"]).exited(0);
    world
        .run(&["host"])
        .exited(0)
        .out_has("no live dispatches")
        .out_has("1 stale registry entry ignored")
        .out_has("halted/build")
        .out_has("the run was stopped");
    world.release("build.go");
}

// llmlint: ignore-block[tests_mirror_real_usage] every state below is one held ownership
// lock a live driver did not release, and no command produces one on demand — a verb that
// could would be a verb that kills a live driver mid-dispatch: **a lock whose pid is a
// reaped process**, **one whose start token is another process's**, **one taken on another
// host**, **one from a build that predates the start token**, **one that is not JSON**, and
// **one that is a directory**. Nothing is assembled by hand: the lock the live driver took
// is read back and each answer changes exactly one fact about it, with the removal
// asserting it removed something. The run is real, its dispatch is genuinely in flight, and
// every claim afterwards is read off the CLI.
/// A `host` row is a claim that a dispatch exists **now**, and it is acted on —
/// an operator leaves the work alone, or ends it. So the row is rendered only
/// while this host can prove the run behind it is still being driven: the
/// ownership lock's pid, and the start token that says the pid is still the
/// process that took it.
#[test]
fn host_never_renders_a_dispatch_whose_driver_this_host_can_prove_is_gone() {
    let world = World::new("views-ghosted");
    world.script("build.wait", "hold");
    let path = world.plan("ghosted", &plan_of("ghosted", vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the dispatch to be in flight", |world| {
        !world.events_of("ghosted", "node-dispatched").is_empty()
    });

    // A driver is holding the run, so the dispatch is exactly what the row says.
    world.run(&["host"]).exited(0).out_has("build");

    // Now the driver dies without releasing what it held, which is the only
    // thing that changes.
    let lock = world.run_file("ghosted", "owner.lock");
    // The lock exactly as the live driver took it, kept so each answer below
    // changes one fact about it and nothing else.
    let held = world.run_json("ghosted", "owner.lock");
    let rewrite = |edit: &dyn Fn(&mut serde_json::Value)| {
        let mut record = held.clone();
        edit(&mut record);
        std::fs::write(&lock, record.to_string()).expect("the lock is rewritten");
    };
    rewrite(&|record| record["pid"] = serde_json::json!(reaped_pid()));

    world
        .run(&["host"])
        .exited(0)
        .out_has("no live dispatches")
        .out_has("1 stale registry entry ignored")
        .out_has("ghosted/build")
        .out_has("is gone")
        // And the scope of the claim, because this scan cannot see a run
        // recorded under another runs root.
        .out_has(&world.runs.display().to_string());

    // A pid the host has since handed to something else: the pid is live and it
    // is not the process that took the lock. Proved stale, for a different
    // reason — and the reason a pid alone was never enough.
    rewrite(&|record| {
        record["started"] = serde_json::json!("the process that took it, which is not this one");
    });
    world
        .run(&["host"])
        .exited(0)
        .out_has("1 stale registry entry ignored")
        .out_has("different process");

    // The three answers this host does not have. None of them renders as a live
    // dispatch, and none is dropped either: a dispatch that may be running is
    // the other error, and the row says outright that nothing backs it.
    let unproven = |why: &str| {
        let rendered = world.run(&["host"]);
        rendered.exited(0).out_has("UNPROVEN").out_has(why);
        assert!(
            !rendered.stdout.contains("stale registry"),
            "an answer this host does not have was reported as a proof:\n{}",
            rendered.stdout
        );
    };
    // A lock taken on another machine, where a pid means nothing.
    rewrite(&|record| record["host"] = serde_json::json!("some-other-host"));
    unproven("some-other-host");
    // A lock from a build that predates the start token. Taking away a field
    // this build always writes would arrange nothing, so the removal has to have
    // removed something.
    rewrite(&|record| {
        assert!(
            record
                .as_object_mut()
                .expect("a lock record")
                .remove("started")
                .is_some(),
            "the lock this build took carries no start token to take away: {record}"
        );
    });
    unproven("no start token");
    // A lock this build cannot read at all. Still a claim — it is what stops a
    // second writer — but it proves nothing about a *dispatch*, and a row is a
    // claim that one exists.
    std::fs::write(&lock, "not json at all").expect("the lock is rewritten");
    unproven("cannot be read");
    // A lock that is there and is not a lock: absent is a proof that nothing
    // drives the run, and this is not absence.
    std::fs::remove_file(&lock).expect("the lock is removed");
    std::fs::create_dir_all(&lock).expect("a lock that is a directory");
    unproven("is not a file");
    std::fs::remove_dir_all(&lock).expect("the lock is removed");

    // And with nothing holding the run at all, nothing is driving it.
    world
        .run(&["host"])
        .exited(0)
        .out_has("no live dispatches")
        .out_has("1 stale registry entry ignored")
        .out_has("nothing holds the run's ownership lock");
    world.release("build.go");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// One zone east of UTC and one west of it, as the environment names them.
///
/// POSIX-form offsets rather than zone names, deliberately: a host with no
/// `tzdata` resolves `America/New_York` to UTC and answers a reader in either
/// "zone" identically, which would leave the journeys below passing while
/// proving nothing. These need nothing installed.
#[cfg(unix)]
const EAST: &str = "XXX-5";
#[cfg(unix)]
const WEST: &str = "YYY9";

/// How this host renders one process's start to a reader standing in `zone`.
///
/// The journeys' own oracle, deliberately not the crate's: what they are about
/// is that a *recorded* start token and a later reading of the same live process
/// agree, and asking the code under test how it reads one would be asking the
/// answer of the thing under test. This asks `ps` the way a person would.
#[cfg(unix)]
fn start_of_as_rendered_in(zone: &str, pid: u32) -> String {
    let listed = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .env("TZ", zone)
        .output()
        .expect("this host says when a process started");
    assert!(
        listed.status.success(),
        "`ps` would not say when pid {pid} started"
    );
    String::from_utf8(listed.stdout)
        .expect("`ps` wrote an answer this host can decode")
        .trim()
        .to_string()
}

/// A live dispatch is still a live dispatch to a reader whose environment is not
/// the driver's.
///
/// The defect this states: a start token is `ps -o lstart=`, which every Unix
/// renders through the **reader's** own time zone — so the driver that recorded
/// the run's lock and the session that later looked at it read one live process
/// as two different ones, and the view whose whole job is to say whether a
/// dispatch is alive answered `no live dispatches` for a run that was working.
/// Nothing about the run changes between the two commands below; only who is
/// asking does.
#[cfg(unix)]
#[test]
fn host_renders_a_live_dispatch_read_from_a_different_zone_than_its_driver_recorded_it_in() {
    let world = World::new("views-zoned");
    world.script("build.wait", "hold");
    let path = world.plan("zoned", &plan_of("zoned", vec![agent("build", &[])]));
    let mut launch = world.cmd(&["start", &path, "--detach"]);
    launch.env("TZ", EAST);
    let started = world.run_on(launch, "a detached launch made in one zone");
    started.exited(0);
    let driver = u32::try_from(
        serde_json::from_str::<serde_json::Value>(started.stdout.trim())
            .expect("a detached launch announces itself")["pid"]
            .as_u64()
            .expect("a driver pid"),
    )
    .expect("a pid");
    world.until("the dispatch to be in flight", |world| {
        !world.events_of("zoned", "node-dispatched").is_empty()
    });

    // The fixture is only a fixture if the two readers really are given
    // different answers about the same live process.
    assert_ne!(
        start_of_as_rendered_in(EAST, driver),
        start_of_as_rendered_in(WEST, driver),
        "this host renders pid {driver}'s start the same way in both zones, so this journey \
         proves nothing"
    );

    let mut read = world.cmd(&["host"]);
    read.env("TZ", WEST);
    let rendered = world.run_on(read, "host, read from the other zone");
    rendered.exited(0).out_has("zoned").out_has("build");
    assert!(
        !rendered.stdout.contains("no live dispatches")
            && !rendered.stdout.contains("stale registry"),
        "a live dispatch was reported dead to a reader standing in another zone:\n{}",
        rendered.stdout
    );
    world.release("build.go");
}

/// And a live dispatch whose ownership lock another build wrote is one too.
///
/// The lock is what proves a *dispatch* is being driven, so a key this build
/// does not know used to answer `the run's ownership lock cannot be read` — the
/// run reads as one nothing can prove is running, on a host where it plainly is.
/// A field it does not know is now ignored, and the row is the row.
#[test]
fn host_renders_a_live_dispatch_whose_lock_another_build_wrote() {
    let world = World::new("views-newer-lock");
    world.script("build.wait", "hold");
    let path = world.plan(
        "newer-lock",
        &plan_of("newer-lock", vec![agent("build", &[])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the dispatch to be in flight", |world| {
        !world.events_of("newer-lock", "node-dispatched").is_empty()
    });

    // llmlint: ignore-block[tests_mirror_real_usage] no verb of this build writes a key
    // this build does not have, so the only way to hold a lock another build took is to
    // put the key on the one this build's own driver took. The run, the dispatch, and the
    // claim below are the real binary end to end.
    let lock = world.run_file("newer-lock", "owner.lock");
    let mut written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&lock).expect("the run's ownership lock"))
            .expect("an ownership lock");
    written["claimed_for"] = serde_json::json!("a build that came later");
    std::fs::write(&lock, written.to_string()).expect("a lock another build took");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let rendered = world.run(&["host"]);
    rendered.exited(0).out_has("newer-lock").out_has("build");
    rendered.out_lacks("ownership lock cannot be read");
    rendered.out_lacks("no live dispatches");
    world.release("build.go");
}

/// And an **adopted** run's live dispatches are live dispatches too.
///
/// The moment the defect was found in, and the worst one to be wrong in: an
/// adoption is exactly when an operator asks whether the takeover worked, and
/// `host` answered `no live dispatches` for a run that was heartbeating. It is
/// also where the two environments come apart by themselves — the driver that
/// takes the lock is a fresh process started by whatever session happened to
/// adopt the run, and it shares a host with the reader and nothing else.
///
/// The dispatch is held across the whole thing and outlives the driver that
/// started it, which is what makes this an adoption of a run *holding* a live
/// dispatch rather than a fresh launch wearing the word.
#[cfg(unix)]
#[test]
fn host_renders_the_live_dispatches_of_a_run_that_was_adopted() {
    let world = World::new("views-adopted");
    world.script("build.wait", "hold");
    let path = world.plan("adopted", &plan_of("adopted", vec![agent("build", &[])]));
    let mut launch = world.cmd(&["start", &path, "--detach"]);
    launch.env("TZ", EAST);
    let started = world.run_on(launch, "a detached launch made in one zone");
    started.exited(0);
    let first = u32::try_from(
        serde_json::from_str::<serde_json::Value>(started.stdout.trim())
            .expect("a detached launch announces itself")["pid"]
            .as_u64()
            .expect("a driver pid"),
    )
    .expect("a pid");
    world.until("the dispatch to be in flight", |world| {
        !world.events_of("adopted", "node-dispatched").is_empty()
    });

    // The driver dies and what it started does not: the state an adoption exists
    // to recover from, and the reason the run is still holding a dispatch when
    // the next driver takes it over.
    crate::harness::end_process(first);
    world.until("the run to read as undriven", |world| {
        world
            .run(&["status", "adopted"])
            .stdout
            .contains("DRIVER DEAD")
    });

    // An adoption attaches, so the adopting driver is left running rather than
    // waited on: it is the process holding the run while the view below is read.
    let mut adopting = world.cmd(&["adopt", "adopted"]);
    adopting.env("TZ", EAST);
    let mut adopting = adopting
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the adopting driver starts");
    // Waited for through the run's own record of the takeover, which is what
    // the adoption announces to every reader of the run: the driver that adopted
    // it says so, and says which process it is.
    world.until("the run to record its adoption", |world| {
        !world.events_of("adopted", "driver-adopted").is_empty()
    });
    let adopter = u32::try_from(
        world.events_of("adopted", "driver-adopted")[0]["payload"]["pid"]
            .as_u64()
            .expect("an adoption names the driver that took the run"),
    )
    .expect("a pid");
    assert_eq!(
        adopter,
        adopting.id(),
        "the run recorded an adoption by a process this journey did not start"
    );
    assert_ne!(
        start_of_as_rendered_in(EAST, adopter),
        start_of_as_rendered_in(WEST, adopter),
        "this host renders pid {adopter}'s start the same way in both zones, so this journey \
         proves nothing"
    );

    let mut read = world.cmd(&["host"]);
    read.env("TZ", WEST);
    let rendered = world.run_on(read, "host, read from the other zone after an adoption");
    rendered.exited(0).out_has("adopted").out_has("build");
    assert!(
        !rendered.stdout.contains("no live dispatches")
            && !rendered.stdout.contains("stale registry"),
        "an adopted run's live dispatch was reported dead:\n{}",
        rendered.stdout
    );

    let _ = adopting.kill();
    let _ = adopting.wait();
    world.release("build.go");
}

/// A chain that fell through and was then served is reported as the recovery it
/// was, naming the identity that actually ran the turn.
///
/// The regression this holds down is a recovered chain rendered as a refusal,
/// which sends every reader at a subscription that never blocked a turn.
#[test]
fn a_chain_that_fell_through_and_was_served_names_the_identity_that_served_it() {
    let world = World::new("views-recovered");
    world.script("build.refused", "agent 1 claude-code:alternate quota\n");
    // The turn the agent side's chain went on to run, under the next candidate.
    world.script("build.served", "agent 1 claude-code:alternate2\n");
    world.script("build.fail", "1");
    let run = settled(&world, "recovered", vec![agent("build", &[])]);

    let results = world.run(&["results", &run]);
    results.exited(0).out_has(
        "fallback: the agent side fell through 'claude-code:alternate' (quota) → served by \
         'claude-code:alternate2'",
    );
    // A recovery is never the reason a node failed, so it never wears the word
    // a chain that ran out does.
    assert!(
        !results.stdout.contains("provider:") && !results.stdout.contains("refused"),
        "a chain that recovered was reported as a refusal:\n{}",
        results.stdout
    );

    let status = world.run(&["status", &run]);
    status
        .exited(0)
        .out_has("build: fallback — the agent side fell through 'claude-code:alternate'")
        .out_has("served by 'claude-code:alternate2'");
    assert!(
        !status.stdout.contains("build: failed — the agent side"),
        "a chain that recovered was reported as the node's failure:\n{}",
        status.stdout
    );
}

/// One chain, three turns, two endings: the turns it recovered on are counted
/// as the one fact they are, and the turn it ran out on is its own.
///
/// The fold keeps a chain's turns apart for exactly this. A record that had
/// collapsed them could only ever be rendered as one of the two endings, and
/// whichever it picked would decide where every reader went next.
#[test]
fn one_chain_that_recovers_and_then_runs_out_reports_both_endings() {
    let world = World::new("views-both-endings");
    world.script(
        "build.refused",
        "agent 1 claude-code quota\nagent 2 claude-code quota\nagent 3 claude-code quota\n",
    );
    // Two of the three turns went on to run under the next candidate; the third
    // reached the end of the chain with nothing left to try.
    world.script(
        "build.served",
        "agent 1 claude-code:alternate\nagent 2 claude-code:alternate\n",
    );
    world.script("build.fail", "1");
    let run = settled(&world, "endings", vec![agent("build", &[])]);

    world
        .run(&["results", &run])
        .exited(0)
        .out_has(
            "fallback: the agent side fell through 'claude-code' (quota) → served by \
             'claude-code:alternate', recorded 2 times",
        )
        .out_has("provider: the agent side: identity 'claude-code' refused (quota)");
}

/// A node that failed because an identity chain ran out says which side asked
/// and which identity refused — and only that chain gets the provider line.
///
/// The side is the point: a two-party member runs one chain per side and they
/// prefer different identities, so a fix aimed at the wrong one changes nothing
/// and the run fails the same way again. Here the agent side recovered and the
/// judge side had no successful candidate, which is exactly the pair a reader
/// has to be able to tell apart.
#[test]
fn a_provider_refusal_names_the_side_and_the_identity_in_results_and_status() {
    let world = World::new("views-refused");
    world.script(
        "build.refused",
        // The judge side's chain refuses twice over, which is one fact recorded
        // twice rather than two facts.
        "agent 1 claude-code quota\njudge 1 codex rate_limit\njudge 1 codex rate_limit\n",
    );
    // Only the agent side ever ran a turn: the judge side's chain reached its
    // end with no successful candidate, which is what a bare "refused" is for.
    world.script("build.served", "agent 1 claude-code:alternate\n");
    world.script("build.fail", "1");
    let run = settled(&world, "refused", vec![agent("build", &[])]);

    let results = world.run(&["results", &run]);
    results
        .exited(0)
        .out_has("failed")
        .out_has(
            "provider: the judge side: identity 'codex' refused (rate_limit), recorded 2 times",
        )
        .out_has(
            "fallback: the agent side fell through 'claude-code' (quota) → served by \
             'claude-code:alternate'",
        );
    assert!(
        !results.stdout.contains("provider: the agent side"),
        "the side that recovered was reported as the one that refused:\n{}",
        results.stdout
    );

    world
        .run(&["status", &run])
        .exited(0)
        .out_has("build: failed —")
        .out_has("the judge side")
        .out_has("codex");
}

/// A node that failed on a judge verdict says **why**, and gets no provider line
/// over a chain that recovered.
///
/// The incident: a node failed on its judge with three provider lines above it
/// pointing somewhere else entirely, and the real reason was reachable only by
/// opening the node's retained report by hand. Both halves are asserted here,
/// because either alone leaves the reader where they were.
#[test]
fn a_node_that_failed_on_a_judge_verdict_says_why_and_names_no_provider() {
    let world = World::new("views-verdict");
    world.script(
        "build.verdict",
        // A verdict that passed, one that failed, one that named neither half,
        // a **numeric** one — which onejudge reports and fails nothing over —
        // and a record that is not one of onejudge's verdicts at all, which is
        // what a producer newer than this build writes.
        "true|the branch is pushed|it is\n\
         false|the change builds|cargo build fails in src/views.rs\n\
         false||\n\
         2.0|how readable it is|it is dense\n\
         ?|the tests pass|the suite is red\n",
    );
    // A chain that fell through and recovered, which is what used to be printed
    // as the reason a node like this failed.
    world.script("build.refused", "agent 1 claude-code quota\n");
    world.script("build.served", "agent 1 claude-code:alternate\n");
    world.script("build.fail", "1");
    let run = settled(&world, "judged", vec![agent("build", &[])]);

    let results = world.run(&["results", &run]);
    results
        .exited(0)
        .out_has("verdict: 'the change builds' failed — cargo build fails in src/views.rs")
        // A judge that named neither the criterion nor a reason still failed
        // this node, which is the fact a provider line above it would otherwise
        // be read as. An empty string is a criterion nobody wrote, not one worth
        // a bare pair of quotes on a line.
        .out_has(
            "verdict: a criterion the record does not name failed — the record carries no reason",
        );
    assert!(
        !results.stdout.contains("provider:"),
        "a node that failed on its judge was given a provider line:\n{}",
        results.stdout
    );
    // A verdict that passed failed nothing; a score gates nothing; and a record
    // this build cannot read is dropped whole rather than mined for the fields
    // it happens to carry. Naming any of the three would put a sentence nobody
    // wrote under a criterion nobody failed the node on.
    for absent in [
        "the branch is pushed",
        "how readable it is",
        "it is dense",
        "the tests pass",
        "the suite is red",
    ] {
        assert!(
            !results.stdout.contains(absent),
            "a verdict that failed nothing, or that this build cannot read, was named as the \
             failure:\n{}",
            results.stdout
        );
    }
}

/// A single-sided member has one side and stamps no role, so the member it ran
/// under is what names the side. It is never given one it did not carry — and
/// with no side and no turn there is nothing to pair its fall-through with, so
/// it is never called a refusal either.
#[test]
fn a_refusal_that_names_no_side_is_attributed_to_its_member_rather_than_invented() {
    let world = World::new("views-refused-side");
    world.script("build.refused", "- - codex auth\n");
    world.script("build.fail", "1");
    let run = settled(&world, "sideless", vec![agent("build", &[])]);

    let results = world.run(&["results", &run]);
    results.exited(0).out_has(
        "fallback: member 'worker' fell through 'codex' (auth); nothing this run recorded \
         names what served that turn",
    );
    for invented in ["the agent side", "the judge side"] {
        assert!(
            !results.stdout.contains(invented),
            "a side the record never carried was invented:\n{}",
            results.stdout
        );
    }
    // A chain this run cannot follow is not a chain that ran out: saying it
    // refused would send a reader at a subscription that was never the problem.
    assert!(
        !results.stdout.contains("refused"),
        "a fall-through nothing can answer for was reported as a refusal:\n{}",
        results.stdout
    );

    // The same on the view a planner reads first, which words a node's chains
    // itself: an unattributed fall-through is evidence beside the failure, never
    // the failure.
    let status = world.run(&["status", &run]);
    status
        .exited(0)
        .out_has("build: fallback — member 'worker' fell through 'codex' (auth)");
    assert!(
        !status.stdout.contains("build: failed — member"),
        "a fall-through nothing can answer for was reported as the node's failure:\n{}",
        status.stdout
    );
}

#[test]
fn a_view_of_a_run_with_no_events_still_renders() {
    let world = World::new("views-empty");
    world.script("driver.wait", "hold");
    let path = world.plan("quiet", &plan_of("quiet", vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);

    for view in ["monitor", "results", "status", "goals", "telemetry"] {
        world.run(&[view, "quiet"]).exited(0);
    }
    world.release("driver.go");
}

/// A node that is ready and *waiting on a workspace* is told apart from one
/// merely queued behind a slot.
///
/// Both read as `ready`, and on a status view that says nothing about either
/// they are the same line of silence — which is how a node sat waiting on the
/// occupancy lease its own repository was under for forty minutes while a
/// supervisor looked for a wedge that did not exist. Two lifecycle nodes on one
/// repository at `concurrency: 1` is that state exactly: the second cannot open
/// a session until the first lets go of the workspace.
#[test]
fn a_ready_node_waiting_on_a_held_workspace_is_told_apart_from_one_merely_queued() {
    let world = World::new("views-ready-held");
    world.repository("local-direct", &[]);
    world.script("service.wait", "hold");
    // One at a time, so the second lifecycle node stays ready while the first
    // holds the repository's workspace open.
    // A second repository, registered and idle: a node on it is repository-backed
    // and waiting on nothing, which is the case a reader must not confuse with
    // either of the other two.
    idle_repository(&world, "other");
    let mut plan = plan_of(
        "readyheld",
        vec![
            lifecycle("service", &[]),
            lifecycle("service-two", &[]),
            lifecycle("other-repo", &[]),
            agent("elsewhere", &[]),
        ],
    );
    plan["concurrency"] = serde_json::json!(1);
    // The second node works on the same repository as the first; the third on
    // the idle one; the fourth has no repository at all.
    plan["tasks"][1]["repo"] = serde_json::json!("service");
    plan["tasks"][2]["repo"] = serde_json::json!("other");
    let path = world.plan("readyheld", &plan);
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the first node's session to be recorded", |world| {
        !world.events_of("readyheld", "session-opened").is_empty()
    });

    let status = world.run(&["status", "readyheld"]);
    status.exited(0);
    assert!(
        status
            .stdout
            .lines()
            .any(|line| line.contains("service-two: ready")
                && line.contains("waiting for the 'service' workspace")
                && line.contains("owner_pid")),
        "a node waiting on a workspace another dispatch holds reads as one merely \
         queued:\n{}",
        status.stdout
    );
    for queued in ["other-repo", "elsewhere"] {
        assert!(
            status
                .stdout
                .lines()
                .any(|line| line.contains(&format!("{queued}: ready — queued for dispatch"))),
            "a node waiting for nothing but a slot does not say so:\n{}",
            status.stdout
        );
    }

    world.release("service.go");
}

/// A registered repository with nothing working in it.
///
/// [`World::repository`] builds the one every lifecycle journey publishes from
/// and rewrites the rules file as it goes; this is the smaller half — an origin,
/// a checkout of it, and a registration — for a journey that needs a *second*
/// repository whose workspace nothing holds.
fn idle_repository(world: &World, alias: &str) {
    let origin = world.root.join(format!("{alias}.git"));
    let checkout = world.root.join(alias);
    std::fs::create_dir_all(&origin).expect("a scratch directory");
    crate::harness::git(world, &origin, &["init", "--bare", "--initial-branch=main"]);
    crate::harness::git(
        world,
        &world.root,
        &["clone", &origin.to_string_lossy(), alias],
    );
    std::fs::write(checkout.join("README.md"), "another repository\n").expect("the seed file");
    crate::harness::git(world, &checkout, &["add", "-A"]);
    crate::harness::git(world, &checkout, &["commit", "-m", "chore: seed"]);
    crate::harness::git(world, &checkout, &["push", "-u", "origin", "main"]);
    world.register(
        &checkout,
        Some(&format!("https://github.com/owner/{alias}.git")),
    );
}

/// A repository this host cannot answer for is said out loud, and the node it
/// belongs to is still rendered.
///
/// The holder enumeration is `onevcs`'s, and it refuses a repository its state
/// root has no record of — which is what a run read from a state root that has
/// moved, or from a machine that never registered the checkout, meets. A view
/// that swallowed the refusal would render the same line for "nothing holds this
/// workspace" and "nobody could be asked", and only one of those is a reason to
/// stop looking for what a node is waiting on.
///
/// The launch itself cannot reach this state: the interlock asks the same
/// sibling about every repository a plan names and refuses a launch it cannot
/// resolve. So the state root moves *after* the run is under way, which is the
/// only way a reader meets it and exactly how one does.
#[test]
fn a_workspace_this_host_cannot_ask_about_is_reported_rather_than_read_as_free() {
    let world = World::new("views-unknown-repo");
    world.repository("local-direct", &[]);
    world.script("service.wait", "hold");
    let mut plan = plan_of(
        "unknownrepo",
        vec![lifecycle("service", &[]), lifecycle("second", &[])],
    );
    // One at a time, so the second node is still ready when `status` renders it.
    plan["concurrency"] = serde_json::json!(1);
    let path = world.plan("unknownrepo", &plan);
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the first node's session to be recorded", |world| {
        !world.events_of("unknownrepo", "session-opened").is_empty()
    });

    // The same run, read against a state root that knows nothing about its
    // repositories.
    let elsewhere = world.root.join("another-state-root");
    std::fs::create_dir_all(&elsewhere).expect("a state root with nothing in it");
    let mut reader = world.cmd(&["status", "unknownrepo"]);
    reader.env("ONEVCS_HOME", &elsewhere);
    let status = world.run_on(reader, "status unknownrepo");
    status.exited(0);
    status.err_has("cannot read the session holders of service");
    assert!(
        status
            .stdout
            .lines()
            .any(|line| line.contains("second: ready")
                && line.contains("this host cannot say whether the 'service' workspace is free")),
        "a workspace nobody could be asked about reads as one nothing holds:\n{}",
        status.stdout
    );

    world.release("service.go");
}

/// A workspace whose only holder has *finished with it* reads as free.
///
/// A session record outlives the session: closing one releases the worktree and
/// the lease and leaves the record behind, because the branch it names is still
/// the only record of the work. So a repository worked in earlier in the run has
/// holders, and none of them holds anything — and a ready node on it is waiting
/// for a slot, not for a lease. Reported as waiting, it would send a supervisor
/// looking for a dispatch that settled hours ago.
#[test]
fn a_ready_node_whose_repositorys_only_session_has_closed_reads_as_queued() {
    let world = World::new("views-ready-closed");
    world.repository("local-direct", &[]);
    world.script("service.work", "the worker wrote this\n");
    // One whole lifecycle first, so the repository has a session record and that
    // session is closed.
    let done = world.plan(
        "worked",
        &plan_of("worked", vec![lifecycle("service", &[])]),
    );
    world.run(&["start", &done, "--attach"]).settled();
    world.until("the first run to settle", |world| {
        world.run_file("worked", "result.json").is_file()
    });
    assert!(
        !world.events_of("worked", "session-closed").is_empty(),
        "the first run's session never closed, so nothing left a spent holder behind:\n{}",
        world.dump()
    );

    // Now a run whose lifecycle node is ready behind a node that is not: the
    // repository's only holder is the closed one above.
    world.script("blocker.wait", "hold");
    let mut plan = plan_of(
        "readyclosed",
        vec![agent("blocker", &[]), lifecycle("service", &[])],
    );
    plan["concurrency"] = serde_json::json!(1);
    let path = world.plan("readyclosed", &plan);
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the blocking node to be dispatched", |world| {
        !world.events_of("readyclosed", "node-dispatched").is_empty()
    });

    let status = world.run(&["status", "readyclosed"]);
    status.exited(0);
    assert!(
        status
            .stdout
            .lines()
            .any(|line| line.contains("service: ready — queued for dispatch")),
        "a workspace whose only session has closed reads as one something is \
         holding:\n{}",
        status.stdout
    );

    world.release("blocker.go");
}

/// Whether one fragment is rendered before another, for a journey whose claim is
/// about the **order** two steps are prescribed in.
///
/// A prescription that names both steps in the wrong order is as expensive as
/// one that names the wrong step: it is the order that makes the second one do
/// anything.
fn named_in_order(rendered: &str, first: &str, then: &str) -> bool {
    match (rendered.find(first), rendered.find(then)) {
        (Some(before), Some(after)) => before < after,
        _ => false,
    }
}

/// A run whose graph has settled is never sent an operator after a fresh driver.
///
/// The two lines came from two readings — the row's word from the graph, the
/// advice under it from the driver alone — so a run that had finished printed
/// `SETTLED` and, directly beneath it, `DRIVER DEAD — attach a fresh driver`.
/// Both endings a settled graph has are driven here, because the prescription
/// was identical for them: a run that converged, and one that failed. The word
/// under each is still the truth about its driver; what is gone is the advice to
/// replace it.
#[test]
fn a_settled_run_is_never_advised_to_attach_a_fresh_driver() {
    let world = World::new("views-settled-advice");
    world.script("failing.fail", "1");
    let converged = settled(&world, "converged", vec![agent("built", &[])]);
    let broke = settled(&world, "brokeoff", vec![agent("failing", &[])]);

    let listing = world.run(&["runs"]);
    listing.exited(0).out_has(&converged).out_has(&broke);
    listing.out_has("SETTLED").out_has("DRIVER DEAD");
    listing.out_lacks("attach a fresh driver");
    listing.out_lacks("onepipeline adopt");

    for run in [&converged, &broke] {
        let status = world.run(&["status", run]);
        status.exited(0);
        status.out_lacks("adopt it or stop it");
        status.out_lacks("onepipeline adopt");
    }
}

/// A run nothing is driving whose unfinished work is parked is told to requeue
/// it **before** a driver is attached.
///
/// The state is reached the way an operator reaches it: a `cancel` idles the
/// node, its dispatch stops, and the driver — with an empty frontier and nothing
/// it may dispatch — closes the run out and goes. A parked node is held out of
/// every later reconcile pass, so the `adopt` this used to prescribe on its own
/// returns at exit 0 having dispatched nothing, which is what it did twice.
#[test]
fn a_run_whose_unfinished_work_is_parked_is_told_to_requeue_before_adopting() {
    let world = World::new("views-parked-advice");
    world.script("slow.turn-open", "");
    world.script("slow.wait", "hold");
    world.script("slow.stops-when-interrupted", "");
    let path = world.plan("parkedrun", &plan_of("parkedrun", vec![agent("slow", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the held node's turn to open", |world| {
        !world.events_of("parkedrun", "turn-started").is_empty()
    });

    world
        .run_with_stdin(
            &["reply", "parkedrun"],
            &serde_json::json!({"version": 1, "commands": [{"op": "cancel", "id": "slow"}]})
                .to_string(),
        )
        .exited(0);
    // The run is only what this journey is about once its driver has gone: what
    // is being read is the advice given to a run nothing is driving.
    world.until("the driver to close the run out", |world| {
        world.run_file("parkedrun", "result.json").is_file()
    });

    let listing = world.run(&["runs"]);
    listing.exited(0).out_has("parkedrun");
    listing.out_has("DRIVER DEAD");
    // The node to requeue is named, so the reply the line asks for is one a
    // reader can write without going looking for what is parked.
    listing.out_has("slow");
    listing.out_has("onepipeline reply parkedrun");
    listing.out_has("onepipeline adopt parkedrun");
    assert!(
        named_in_order(&listing.stdout, "requeue", "onepipeline adopt parkedrun"),
        "`runs` prescribes the adoption before the requeue that gives it something \
         to do:\n{}",
        listing.stdout
    );

    let status = world.run(&["status", "parkedrun"]);
    status.exited(0);
    status.out_has("onepipeline reply parkedrun");
    status.out_has("onepipeline adopt parkedrun");
    assert!(
        named_in_order(&status.stdout, "requeue", "onepipeline adopt parkedrun"),
        "`status` prescribes the adoption before the requeue that gives it something \
         to do:\n{}",
        status.stdout
    );
    // And neither view gives the prescription that did nothing: an adoption on
    // its own.
    for rendered in [&listing.stdout, &status.stdout] {
        assert!(
            !rendered.contains("its ledger is intact; attach a fresh driver"),
            "a run whose only unfinished work is parked was sent straight to an \
             adoption:\n{rendered}"
        );
    }

    world.release("slow.go");
}

/// A run nothing is driving that has work a fresh driver could schedule keeps
/// the advice it has always given.
///
/// The other half of the same reading, and the reason it cannot simply be
/// silenced: this is the run `adopt` exists for — its driver died with a
/// dispatch in flight and a node behind it — and the line that says so is the
/// one an operator needs.
#[cfg(unix)]
#[test]
fn a_run_with_work_a_fresh_driver_could_schedule_is_still_told_to_adopt() {
    let world = World::new("views-undriven-advice");
    world.script("build.wait", "hold");
    let path = world.plan(
        "livework",
        &plan_of(
            "livework",
            vec![agent("build", &[]), agent("later", &["build"])],
        ),
    );
    let started = world.run(&["start", &path.to_string_lossy(), "--detach"]);
    started.exited(0);
    let driver = u32::try_from(
        started.json()["pid"]
            .as_u64()
            .expect("a detached launch names the driver it retained"),
    )
    .expect("a pid");
    world.until("the dispatch to be in flight", |world| {
        !world.events_of("livework", "node-dispatched").is_empty()
    });

    crate::harness::end_process(driver);
    world.until("the run to read as undriven", |world| {
        world
            .run(&["status", "livework"])
            .stdout
            .contains("DRIVER DEAD")
    });

    let listing = world.run(&["runs"]);
    listing.exited(0).out_has(
        "DRIVER DEAD — its ledger is intact; attach a fresh driver with: \
         onepipeline adopt livework",
    );
    listing.out_lacks("requeue");

    let status = world.run(&["status", "livework"]);
    status.exited(0);
    status.out_has("DRIVER DEAD: nothing is driving this run; adopt it or stop it");
    status.out_lacks("requeue");

    world.release("build.go");
}
