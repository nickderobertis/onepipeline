//! Read-time filter profiles: what one reader of a run is shown.
//!
//! The other half of the filter contract — the source filters a launch passes
//! through to `oneagentgraph` and `onevcs` are proven against the real siblings,
//! in `dispatch.rs` and `real_vcs.rs`, because what they claim is that the
//! *source* did not relay. What a profile claims is about this crate alone: it
//! shapes an event view and touches nothing else, so two readers of one run see
//! it differently and neither loses anything the other keeps.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the `oneagentgraph`
// *sibling* at its subprocess boundary and nothing inside the crate under test, which is
// driven as a real compiled binary. What these journeys need from that sibling is a
// stream with detailed activity on it — a real one produces that only from a paid model
// turn. `harness.rs` carries the same suppression and the full rationale.

use std::io::Write;

use crate::harness::{agent, plan_of, World, REFUSED};

/// Run one node to settlement, and answer with the run's id.
///
/// Settled rather than held mid-dispatch, because what every profile below is
/// about is the difference between this crate's own events and the sibling's
/// detailed activity — and the sibling relays its turns as the dispatch ends, so
/// a run held at its first node has none of them in the store yet.
fn settled(world: &World, name: &str, flags: &[&str]) -> String {
    let path = world.plan(name, &plan_of(name, vec![agent("build", &[])]));
    let path = path.to_string_lossy().to_string();
    let mut args = vec!["start", &path, "--attach"];
    args.extend_from_slice(flags);
    world.run(&args).exited(0).settled();
    world.until("the sibling's own activity to reach the store", |world| {
        world
            .journal(name)
            .iter()
            .any(|event| event["source"] == "agentgraph")
    });
    name.to_string()
}

/// Raise a blocking question about the run, and wait until it is queued.
///
/// Through the channel server, because that is the only author of a blocking
/// surface: `surface --kind check-in` is a report and never holds anything back.
fn raise_blocker(world: &World, run: &str) -> std::process::Child {
    let mut serving = world
        .cmd(&["channel", "serve", run])
        // Nobody answers this one, and the server's own wait is not what is under
        // test.
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"the plan looks wrong; what now?"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    world.until("the question to reach the planner", |world| {
        !world.events_of(run, "planner-surface-queued").is_empty()
    });
    serving
}

/// The shipped `planner` profile is what `monitor` reads through by default, and
/// `--all` and the shipped `monitor` profile are the whole store.
#[test]
fn monitor_defaults_to_the_planner_profile_and_all_bypasses_it() {
    let world = World::new("filter-monitor-default");
    let run = settled(&world, "profiled", &[]);

    // The default: every pipeline-level event, and none of the detailed activity
    // behind them. `monitor` renders a `graph:` id for this crate's own events
    // and `agent:` / `vcs:` for a sibling's, so the ids are what say which
    // source a line came from.
    let planner = world.run(&["monitor", &run]);
    planner.exited(0).out_has("node-dispatched");
    assert!(
        !planner.stdout.contains("agent:"),
        "the default profile showed the detailed agentgraph activity:\n{}",
        planner.stdout
    );

    for bypass in [
        vec!["monitor", &run, "--all"],
        vec!["monitor", &run, "--filter", "monitor"],
    ] {
        let read = world.run(&bypass);
        read.exited(0).out_has("agent:").out_has("node-dispatched");
    }
}

/// `--filter` takes a spec as well as a name, and a spec is read the way the
/// grammar is written.
#[test]
fn a_filter_flag_takes_an_inline_spec_as_well_as_a_profile_name() {
    let world = World::new("filter-inline");
    let run = settled(&world, "inline", &[]);

    let narrowed = world.run(&[
        "monitor",
        &run,
        "--filter",
        r#"{"include": [{"kind": "node-dispatched"}]}"#,
    ]);
    narrowed.exited(0).out_has("node-dispatched");
    assert!(
        !narrowed.stdout.contains("node-ready"),
        "the inline spec admitted a kind it did not name:\n{}",
        narrowed.stdout
    );

    // Refused by the shared grammar's own rules, naming the field that is not
    // one — a planner who mistyped a matcher is told which word was wrong.
    let refused = world.run(&[
        "monitor",
        &run,
        "--filter",
        r#"{"include": [{"role": "agent"}]}"#,
    ]);
    refused.exited(REFUSED);
    assert!(
        refused.stderr.contains("role"),
        "the refusal does not name the offending field:\n{}",
        refused.stderr
    );

    // And a name that is neither a profile this run has nor a readable file is
    // answered with the profiles it does have.
    let unknown = world.run(&["monitor", &run, "--filter", "planer"]);
    unknown.exited(REFUSED);
    assert!(
        unknown.stderr.contains("planner") && unknown.stderr.contains("monitor"),
        "the refusal does not name the profiles this run has:\n{}",
        unknown.stderr
    );
}

/// A launch overrides either shipped profile by declaring one of that name.
#[test]
fn a_launch_overrides_the_shipped_profiles_by_name() {
    let world = World::new("filter-override");
    let run = settled(
        &world,
        "overridden",
        &[
            "--filter-profile",
            r#"planner={"include": [{"kind": "node-dispatched"}]}"#,
            "--filter-profile",
            r#"monitor={"include": [{"kind": "run-started"}]}"#,
        ],
    );

    // The default view is now the launch's own planner profile.
    let planner = world.run(&["monitor", &run]);
    planner.exited(0).out_has("node-dispatched");
    assert!(
        !planner.stdout.contains("run-started"),
        "the shipped planner profile was read instead of the launch's own:\n{}",
        planner.stdout
    );

    // And `monitor` is no longer unfiltered, because this run said otherwise.
    let monitor = world.run(&["monitor", &run, "--filter", "monitor"]);
    monitor.exited(0).out_has("run-started");
    assert!(
        !monitor.stdout.contains("node-dispatched"),
        "the shipped monitor profile was read instead of the launch's own:\n{}",
        monitor.stdout
    );

    // `--all` is not a profile, so nothing a launch declares reaches it.
    world
        .run(&["monitor", &run, "--all"])
        .exited(0)
        .out_has("run-started")
        .out_has("node-dispatched");
}

/// A profile shapes the view and **nothing else**: not the store, not which
/// surfaces exist, and not the unread accounting over them.
#[test]
fn a_profile_shapes_the_view_without_touching_the_store_or_the_channel() {
    let world = World::new("filter-readonly");
    let run = settled(&world, "shaped", &[]);
    let mut serving = raise_blocker(&world, &run);

    let store = || {
        std::fs::read_to_string(world.run_file(&run, "events.jsonl"))
            .expect("the store is read")
            .lines()
            .map(str::to_string)
            .collect::<Vec<String>>()
    };
    let before = store();

    // `next` shapes its event view the same way `monitor` does, and defaults the
    // same way: the pipeline spine, without the detailed activity behind it.
    let read = world.run(&["next", &run]);
    read.exited(0);
    let events = read.json()["events"]
        .as_array()
        .expect("`next` reports its event view")
        .clone();
    assert!(
        events.iter().all(|event| event["source"] == "pipeline"),
        "`next` defaulted to something other than the planner profile: {events:?}"
    );
    assert!(
        !events.is_empty(),
        "`next` reported an empty view of a run with events in it"
    );

    // The blocking surface was delivered, and the accounting moved exactly as it
    // does unfiltered: one surface consumed, none left waiting.
    assert_eq!(read.json()["surface"]["blocking"], serde_json::json!(true));
    let after = world.run(&["runs"]);
    assert!(
        !after.stdout.contains("planner update(s) waiting"),
        "a consumed surface is still reported unread:\n{}",
        after.stdout
    );

    // The read above wrote exactly one record, and it is the `planner-surfaced`
    // the **channel** records for the consumption — nothing the profile shaped
    // was written back, and nothing it left out was removed.
    let consumed = store();
    assert_eq!(
        consumed[..before.len()],
        before[..],
        "reading a run through a profile rewrote what its store already held"
    );
    let written: Vec<&String> = consumed[before.len()..].iter().collect();
    assert_eq!(written.len(), 1, "{written:?}");
    assert!(
        written[0].contains("planner-surfaced"),
        "a read wrote something other than the consumption it records: {}",
        written[0]
    );
    world
        .run(&["monitor", &run, "--filter", "monitor"])
        .exited(0);
    world
        .run(&[
            "next",
            &run,
            "--filter",
            r#"{"include": [{"kind": "nothing-emits-this"}]}"#,
        ])
        .exited(0);
    assert_eq!(
        consumed,
        store(),
        "reading a run through a profile changed its merged store"
    );

    drop(serving.stdin.take());
    let _ = serving.wait();
}

/// A blocking surface reaches its planner under the narrowest profile there is.
///
/// The guarantee that makes a profile safe to narrow: which surfaces exist
/// belongs to the channel, so a view that admits no event at all still delivers
/// the question the run is waiting on.
#[test]
fn a_blocking_surface_is_delivered_under_every_profile() {
    let world = World::new("filter-blocking");
    let run = settled(&world, "blocked", &[]);

    for filter in [
        vec![
            "--filter",
            r#"{"include": [{"kind": "nothing-emits-this"}]}"#,
        ],
        vec!["--filter", "planner"],
        vec!["--filter", "monitor"],
        vec!["--all"],
        vec![],
    ] {
        let mut serving = raise_blocker(&world, &run);
        let mut args = vec!["next", &run];
        args.extend_from_slice(&filter);
        let read = world.run(&args);
        read.exited(0);
        assert_eq!(
            read.json()["surface"]["blocking"],
            serde_json::json!(true),
            "the blocking surface was withheld under {filter:?}: {}",
            read.stdout
        );
        drop(serving.stdin.take());
        let _ = serving.wait();
    }
}
