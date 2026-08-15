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

/// The document a double plants where a settlement points but nothing should
/// read, and the words that prove it was read if they ever appear.
const PLANTED_DOCUMENT: &str =
    r#"{"transcript":{"messages":[{"role":"assistant","content":"planted-and-never-read"}]}}"#;

/// The one recognisable string inside it.
const PLANTED_WORDS: &str = "planted-and-never-read";

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

    let stream = world.run(&["monitor", &run]);
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

// llmlint: ignore-block[tests_mirror_real_usage] the *arrangement* below writes a line
// into the run's store on purpose, because that is the threat: a settlement no producer
// emitted. No command forges one — a user interface that could would be the defect — so
// there is nothing else to reach this condition with. Everything asserted afterwards is
// through the CLI, which is where the claim lives: `transcript` does not open what the
// line named, and says so.
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
    let planted = outside.join("report.json");
    std::fs::write(&planted, PLANTED_DOCUMENT).expect("a planted report");

    let forged = serde_json::json!({
        "v": 1,
        "ts": "2099-01-01T00:00:00.000Z",
        "stream": "forged",
        "seq": 0,
        "source": "agentgraph",
        "kind": "member-settled",
        "labels": {"node": "build", "onepipeline.node": "build"},
        "payload": {"report_path": planted.display().to_string()},
    });
    let journal = world.run_file(&run, "events.jsonl");
    let mut existing = std::fs::read_to_string(&journal).expect("the journal reads");
    existing.push_str(&format!("{forged}\n"));
    std::fs::write(&journal, existing).expect("the journal is appended to");

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
// llmlint: ignore-end[tests_mirror_real_usage]

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
