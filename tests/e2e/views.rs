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

use crate::harness::{agent, gate_script, human, plan_of, Run, World};

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
    let launched = world.run(&["start", &path.to_string_lossy(), "--attach"]);
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
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .settled();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

#[test]
fn monitor_renders_all_three_streams_under_their_own_typed_ids() {
    let world = World::new("views-monitor");
    world.repository("local-direct", &["true"]);
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
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
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

#[test]
fn goals_says_what_each_run_is_for_and_which_identities_it_holds() {
    let world = World::new("views-goals");
    world.repository("local-direct", &["true"]);
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
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
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

    // Eight buckets, and the two nothing in this stack measures are served
    // absent rather than as a zero that reads as measured.
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
    for absent in ["judge", "llmlint"] {
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

/// A gate run and a lock wait are the two stretches an operator most needs
/// answered apart from the agent's — and a lifecycle node spends real time in
/// both.
///
/// The **gate** is held here, because it is the one a journey can hold from
/// outside the publication: `onevcs` runs the repository's own command, and this
/// one waits for a file. The lock wait cannot be, and not for want of a hold:
/// the sibling emits `lock-wait` *after* it has waited, carrying the elapsed
/// seconds in its payload, and then emits `lock-acquired` immediately — so the
/// interval this crate measures between the two is the cost of writing two
/// records, however long the wait was. The `onevcs` double emitted the marker
/// and *then* blocked, which is a shape no release of that library has ever
/// produced, and it is what let this bucket read as measured. The bucket is
/// still served, and it is still not the agent's; what it is not is a
/// measurement. Recorded as divergence 16 in `docs/contract-divergences.md`.
#[test]
fn telemetry_separates_gate_and_lock_time_from_agent_time() {
    let world = World::new("views-gatetime");
    let go = world.fakes.join("gate.go");
    // The gate is held open for a measurable span, so its bucket is a real
    // duration on the clock rather than a bucket that merely exists.
    let gate = gate_script(&world, &["wait-for", &go.to_string_lossy()]);
    world.repository(
        "local-direct",
        &gate.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    world.script("service.work", "the worker wrote this\n");
    let path = world.plan("gated", &plan_of("gated", vec![lifecycle("service", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

    world.until("the gate to start", |world| {
        !world.events_of("gated", "gate-started").is_empty()
    });
    let since = std::time::Instant::now();
    world.until("the gate to last a measurable stretch", |_| {
        since.elapsed() >= HELD
    });
    world.release("gate.go");
    world.until("the run to settle", |world| {
        world.run_file("gated", "result.json").is_file()
    });

    let document = world.run(&["telemetry", "gated"]).json();
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
    let gate = span("gate").unwrap_or_else(|| panic!("gate is unmeasured: {document}"));
    assert!(
        gate >= FLOOR,
        "gate measured {gate}ms of a stretch held for {}ms: {document}",
        HELD.as_millis()
    );
    assert!(
        span("agent").is_some(),
        "the agent bucket is unmeasured: {document}"
    );
    // And the wait the sibling did do is on its own record, as the number a
    // reader would need to charge it: this is the datum the bucket below is
    // missing, and it is deterministic — the payload either carries the elapsed
    // seconds or it does not.
    let waited = &world.events_of("gated", "lock-wait")[0];
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
    for name in ["publication_wait", "setup"] {
        assert!(
            span(name).is_some(),
            "{name} is unmeasured for a run that published: {document}"
        );
    }
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

/// A dispatch that has recorded something without naming a tool claims the
/// count and the age, and nothing more.
///
/// The session a lifecycle node opens is recorded before its first turn is, so
/// this is the state every lifecycle node passes through — and it is where a
/// readout that invented a "now" would be inventing it.
#[test]
fn a_dispatch_that_has_named_no_tool_reports_its_count_rather_than_a_guess() {
    let world = World::new("views-nameless");
    world.repository("local-direct", &["true"]);
    world.script("service.wait", "hold");
    let path = world.plan(
        "nameless",
        &plan_of("nameless", vec![lifecycle("service", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
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
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

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

/// A dispatch this build cannot place in time is reported as having recorded
/// nothing, rather than as having worked a moment ago.
///
/// The stamp on a relayed envelope is a producer's, and a producer this build is
/// newer or older than can spell it in a way nothing here parses. Counted anyway,
/// such an envelope would give the readout an age it never measured — and the age
/// is the whole thing an operator acts on, so a dispatch nobody can place would
/// read as the freshest one on the run. It says what it knows instead: something
/// arrived, and there is nothing here to age it by.
#[test]
fn status_reports_a_dispatch_it_cannot_place_in_time_as_having_recorded_nothing() {
    let world = World::new("views-unplaceable");
    // Announced, beating, and stamped by a clock this build cannot read.
    world.script("blind.turn-open", "");
    world.script("blind.wait", "hold");
    world.script("blind.heartbeat", "100");
    world.script("blind.clock-unreadable", "");
    // The control: the same dispatch, said in a spelling this build reads. It is
    // what makes the assertion about the stamp rather than about the scripting.
    world.script("timed.turn-open", "");
    world.script("timed.wait", "hold");
    world.script("timed.heartbeat", "100");
    let path = world.plan(
        "unplaceable",
        &plan_of(
            "unplaceable",
            vec![agent("blind", &[]), agent("timed", &[])],
        ),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

    // Both have announced their turn and beaten for a while, so the difference
    // below is the stamp and nothing else.
    let beats = |world: &World, node: &str| {
        world
            .events_of("unplaceable", "member-heartbeat")
            .iter()
            .filter(|event| event["labels"]["onepipeline.node"] == node)
            .count()
    };
    world.until("both dispatches to heartbeat for a while", |world| {
        beats(world, "blind") >= 20 && beats(world, "timed") >= 20
    });

    let status = world.run(&["status", "unplaceable"]);
    status.exited(0);
    let line = |node: &str| {
        status
            .stdout
            .lines()
            .find(|line| line.trim_start().starts_with(&format!("{node}: running")))
            .unwrap_or_else(|| panic!("`status` has no line for {node}:\n{}", status.stdout))
            .to_string()
    };

    // The control, so the two announcing envelopes and the beats are known to
    // have arrived and to be readable when they can be read.
    let timed = line("timed");
    assert!(
        timed.contains("2 event(s)") && timed.contains("alive "),
        "the control dispatch did not record the turn it announced: {timed}"
    );

    let blind = line("blind");
    assert!(
        blind.contains("nothing recorded yet"),
        "a dispatch whose every envelope is unplaceable was aged by one of them \
         anyway: {blind}"
    );
    assert!(
        !blind.contains("event(s)") && !blind.contains("alive "),
        "an unplaceable stamp was counted as work or as liveness: {blind}"
    );
    // And it is still a dispatch that is running, which is the opposite mistake:
    // unplaceable is not the same as absent.
    assert!(
        !blind.contains("UNDRIVEN"),
        "a dispatch that is saying things this build cannot place reads as one \
         nothing is driving: {blind}"
    );

    world.release("blind.go");
    world.release("timed.go");
}

/// Once one envelope can be placed, the readout ages the dispatch by that one —
/// and claims nothing for the arrivals it never could place.
///
/// The transition, and the half the wholly-unplaceable journey above cannot
/// show: a producer whose opening envelope carries a spelling this build does not
/// read, and whose next one it does. What must not happen is the age sliding back
/// onto the arrival nothing could place, which would date the dispatch to a
/// moment no clock here ever read. So the count is of what could be placed, and
/// the age is of the last of those.
#[test]
fn status_ages_a_dispatch_from_the_first_envelope_it_can_place() {
    let world = World::new("views-dawning");
    world.script("dawning.turn-open", "");
    world.script("dawning.wait", "hold");
    world.script("dawning.heartbeat", "100");
    // The announcing envelope is unplaceable and the turn's own is not, so the
    // dispatch crosses from having nothing to age by to having one thing.
    world.script("dawning.clock-unreadable-first", "");
    let path = world.plan("dawning", &plan_of("dawning", vec![agent("dawning", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

    // Beating for a while, so an age taken from the placeable envelope and one
    // taken from the latest arrival cannot be confused.
    world.until("the dispatch to heartbeat for a while", |world| {
        world.events_of("dawning", "member-heartbeat").len() >= 20
    });

    let status = world.run(&["status", "dawning"]);
    status.exited(0);
    let line = status
        .stdout
        .lines()
        .find(|line| line.trim_start().starts_with("dawning: running"))
        .unwrap_or_else(|| panic!("`status` has no line for dawning:\n{}", status.stdout))
        .to_string();
    assert!(
        line.contains("1 event(s)"),
        "the arrival this build could not place was counted beside the one it \
         could: {line}"
    );
    assert!(
        seconds_since_activity(&status.stdout, "dawning") >= 2,
        "the dispatch was aged by its heartbeats rather than by the one envelope \
         it could be placed by: {line}"
    );

    world.release("dawning.go");
}

/// A run that has dispatched nothing has no transcript, and says so rather than
/// rendering an empty one.
#[test]
fn a_run_that_has_dispatched_nothing_says_it_has_no_transcript() {
    let world = World::new("views-notranscript");
    // A human action and nothing else: the loop records it as waiting and
    // dispatches nothing at all, which is the state under test.
    let path = world.plan("quiet", &plan_of("quiet", vec![human("approve", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0);

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
    // the record, or a directory an operator left beside the runs), **a launch record this
    // build refuses** (nothing here writes an unknown field — that is the point of it), and
    // **a launch record that is a directory**. None has an engine-side constructor, and a
    // verb that made one would be the defect. The run beside them is launched through the
    // CLI, and every claim is read off the CLI.
    // A run root left half-written: the directory is there and the launch record
    // that says who owns it is not.
    std::fs::create_dir_all(world.runs.join("half-written")).expect("a run root with no launch");
    // And one whose launch record carries a field this build does not accept,
    // which is the refusal `results` already words: the file, the offending
    // field, and what was expected.
    std::fs::create_dir_all(world.runs.join("typo")).expect("a run root");
    std::fs::write(
        world.runs.join("typo").join("launch.json"),
        r#"{"oops": true}"#,
    )
    .expect("a launch record this build refuses");
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
            .out_has("unknown field `oops`")
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
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
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
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
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

/// A pid this host can prove is gone: a real process, started and reaped.
///
/// Picked out of the air it would not be one — the kernel may have handed it to
/// something else — and the whole journey above turns on the difference.
fn reaped_pid() -> u32 {
    let mut child = std::process::Command::new(crate::harness::binary())
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the binary starts");
    let pid = child.id();
    child.wait().expect("it exits");
    pid
}

/// A node that failed because its identity chains ran out says which side asked
/// and which identity refused.
///
/// Both sides, because that is the fact: a two-party member runs one chain per
/// side and they prefer different identities, so a fix aimed at the wrong one
/// changes nothing and the run fails the same way again.
#[test]
fn a_provider_refusal_names_the_side_and_the_identity_in_results_and_status() {
    let world = World::new("views-refused");
    world.script(
        "build.refused",
        // The judge side's chain refuses twice over, which is one fact recorded
        // twice rather than two facts.
        "agent claude-code quota\njudge codex rate_limit\njudge codex rate_limit\n",
    );
    world.script("build.fail", "1");
    let run = settled(&world, "refused", vec![agent("build", &[])]);

    world
        .run(&["results", &run])
        .exited(0)
        .out_has("failed")
        .out_has("provider: the agent side: identity 'claude-code' refused (quota)")
        .out_has(
            "provider: the judge side: identity 'codex' refused (rate_limit), recorded 2 times",
        );

    world
        .run(&["status", &run])
        .exited(0)
        .out_has("build: failed —")
        .out_has("the judge side")
        .out_has("codex");
}

/// A single-sided member has one side and stamps no role, so the member it ran
/// under is what names the side. It is never given one it did not carry.
#[test]
fn a_refusal_that_names_no_side_is_attributed_to_its_member_rather_than_invented() {
    let world = World::new("views-refused-side");
    world.script("build.refused", "- codex auth\n");
    world.script("build.fail", "1");
    let run = settled(&world, "sideless", vec![agent("build", &[])]);

    let results = world.run(&["results", &run]);
    results
        .exited(0)
        .out_has("provider: member 'worker': identity 'codex' refused (auth)");
    for invented in ["the agent side", "the judge side"] {
        assert!(
            !results.stdout.contains(invented),
            "a side the record never carried was invented:\n{}",
            results.stdout
        );
    }
}

#[test]
fn a_view_of_a_run_with_no_events_still_renders() {
    let world = World::new("views-empty");
    world.script("driver.wait", "hold");
    let path = world.plan("quiet", &plan_of("quiet", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

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
    world.repository("local-direct", &["true"]);
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
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
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
    world.repository("local-direct", &["true"]);
    world.script("service.wait", "hold");
    let mut plan = plan_of(
        "unknownrepo",
        vec![lifecycle("service", &[]), lifecycle("second", &[])],
    );
    // One at a time, so the second node is still ready when `status` renders it.
    plan["concurrency"] = serde_json::json!(1);
    let path = world.plan("unknownrepo", &plan);
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
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
    world.repository("local-direct", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    // One whole lifecycle first, so the repository has a session record and that
    // session is closed.
    let done = world.plan(
        "worked",
        &plan_of("worked", vec![lifecycle("service", &[])]),
    );
    world
        .run(&["start", &done.to_string_lossy(), "--attach"])
        .settled();
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
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
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
