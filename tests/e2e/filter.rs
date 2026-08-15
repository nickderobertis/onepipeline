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
            .run(&["monitor", name, "--all"])
            .stdout
            .contains("agent:")
    });
    name.to_string()
}

/// Raise a blocking question about the run, and wait until it is queued.
///
/// Through the channel server, because that is the only author of a blocking
/// surface: `surface --kind check-in` is a report and never holds anything back.
fn raise_blocker(world: &World, run: &str) -> std::process::Child {
    let waiting = |world: &World| {
        world
            .run(&["status", run])
            .stdout
            .contains("planner update(s) waiting")
    };
    // Read through `status`, which is where a planner sees that something is
    // waiting for them. Every caller consumes what it raises before raising the
    // next, so "nothing waiting" is the state this starts from — asserted rather
    // than assumed, because a wait that was already satisfied would return before
    // this question was queued and leave `next` with nothing to claim.
    assert!(
        !waiting(world),
        "a surface was already waiting, so this journey cannot tell its own from it"
    );
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
    world.until("the question to reach the planner", waiting);
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

    // The same spec, named as a file — which is how one long enough to be worth
    // writing down is kept, and is read as the YAML the grammar is written in
    // rather than as the JSON one line of argv carries.
    let spec = world.root.join("only-dispatches.yaml");
    std::fs::write(&spec, "include:\n  - kind: node-dispatched\n").expect("the spec is written");
    let from_file = world.run(&["monitor", &run, "--filter", &spec.to_string_lossy()]);
    from_file.exited(0).out_has("node-dispatched");
    assert_eq!(
        from_file.stdout, narrowed.stdout,
        "the same filter read from a file shaped a different view"
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

    // The store as a reader sees the whole of it: `monitor --all` is the
    // unfiltered view, one line per event, so "the store did not change" is
    // exactly "this rendering did not change". Read through the product surface
    // rather than off the file, because what a profile must not disturb is what
    // the *next reader* gets.
    let store = || {
        world
            .run(&["monitor", &run, "--all"])
            .stdout
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
    // `monitor` ends with a trailer line about the run rather than an event, so
    // the events are everything before it — and the one line that appeared is the
    // consumption.
    let consumed = store();
    let events = |lines: &[String]| lines[..lines.len() - 1].to_vec();
    let (was, now) = (events(&before), events(&consumed));
    assert_eq!(
        now[..was.len()],
        was[..],
        "reading a run through a profile rewrote what its store already held"
    );
    let written = &now[was.len()..];
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

/// Every way of naming a profile shapes `next`'s event view, and none of them
/// touches the blocking surface it delivers.
///
/// The guarantee that makes a profile safe to narrow: which surfaces exist
/// belongs to the channel, so a view that admits no event at all still delivers
/// the question the run is waiting on. Asserted for each spelling of `--filter`
/// there is, beside the view that spelling produced — the two claims are about
/// one command, so they are read off one invocation of it.
#[test]
fn every_spelling_of_a_profile_shapes_the_view_and_still_delivers_a_blocking_surface() {
    let world = World::new("filter-blocking");
    let run = settled(&world, "blocked", &[]);

    let spec = world.root.join("pipeline-only.yaml");
    std::fs::write(&spec, "include:\n  - source: pipeline\n").expect("the spec is written");
    let from_file = spec.to_string_lossy().to_string();

    /// What sources a view is expected to carry: none at all, this crate's own,
    /// or every one the store holds.
    #[derive(Debug, PartialEq, Eq)]
    enum View {
        Nothing,
        PipelineOnly,
        Everything,
    }

    for (filter, expected) in [
        // Named, inline, from a file, bypassed, and defaulted.
        (vec!["--filter", "monitor"], View::Everything),
        (vec!["--filter", "planner"], View::PipelineOnly),
        (
            vec![
                "--filter",
                r#"{"include": [{"kind": "nothing-emits-this"}]}"#,
            ],
            View::Nothing,
        ),
        (vec!["--filter", &from_file], View::PipelineOnly),
        (vec!["--all"], View::Everything),
        (vec![], View::PipelineOnly),
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

        let events = read.json()["events"]
            .as_array()
            .expect("`next` reports its event view")
            .clone();
        let sources: std::collections::BTreeSet<String> = events
            .iter()
            .filter_map(|event| event["source"].as_str().map(str::to_string))
            .collect();
        let saw = match (events.is_empty(), sources.len()) {
            (true, _) => View::Nothing,
            (false, 1) if sources.contains("pipeline") => View::PipelineOnly,
            _ => View::Everything,
        };
        assert_eq!(
            saw, expected,
            "`next {filter:?}` shaped its view as {saw:?}, over sources {sources:?}"
        );

        drop(serving.stdin.take());
        let _ = serving.wait();
    }
}

/// `next` shapes its view the same way when there is no surface to deliver.
///
/// The answer a polling planner gets most of the time. A view that only appeared
/// beside a surface would make "nothing to answer" also mean "nothing to see",
/// which is exactly backwards: a run moving along with nobody to ask is the case
/// where the events *are* the whole report.
#[test]
fn next_shapes_its_view_when_there_is_no_surface_to_deliver() {
    let world = World::new("filter-no-surface");
    let run = settled(&world, "quiet", &[]);

    let read = world.run(&["next", &run]);
    read.exited(0);
    assert_eq!(read.json()["surface"], serde_json::Value::Null);
    assert_eq!(read.json()["status"], "finished");
    let events = read.json()["events"]
        .as_array()
        .expect("`next` reports its event view with no surface to deliver")
        .clone();
    assert!(
        !events.is_empty() && events.iter().all(|event| event["source"] == "pipeline"),
        "the no-surface answer was not shaped by the default profile: {events:?}"
    );

    let all = world.run(&["next", &run, "--all"]);
    all.exited(0);
    assert!(
        all.json()["events"]
            .as_array()
            .expect("an event view")
            .iter()
            .any(|event| event["source"] == "agentgraph"),
        "`--all` did not bypass the profile on the no-surface answer:\n{}",
        all.stdout
    );
}

/// `--filter` and `--all` are two answers to one question, so naming both is
/// refused rather than silently resolved.
#[test]
fn naming_both_a_filter_and_all_is_refused() {
    let world = World::new("filter-conflict");
    let run = settled(&world, "conflicted", &[]);

    for verb in ["next", "monitor"] {
        let refused = world.run(&[verb, &run, "--filter", "monitor", "--all"]);
        refused.exited(REFUSED);
        assert!(
            refused.stderr.contains("--all") && refused.stderr.contains("--filter"),
            "`{verb}` did not say which two flags conflict:\n{}",
            refused.stderr
        );
    }
}

/// A launch refuses a `filters:` block it could not honour, before it mints a
/// run for it.
///
/// The whole point of checking at launch: a spec a source will not take is an
/// exit 2 an operator sees on the command line they typed it on, rather than a
/// run that has already cut sessions and dispatched work before a graph refuses
/// the filter it was handed.
#[test]
fn a_launch_refuses_a_filter_block_it_could_not_honour() {
    let world = World::new("filter-launch-refusal");
    let plan = world
        .plan("refused", &plan_of("refused", vec![agent("build", &[])]))
        .to_string_lossy()
        .to_string();

    // Each refusal names the thing its author has to fix: the declaration that
    // is not `NAME=SPEC`, the field that is not a matcher field, the matcher
    // that could match nothing, or the file that is not there.
    for (flags, named) in [
        (vec!["--filter-profile", "planner"], "NAME=SPEC"),
        (vec!["--filter-profile", "={\"include\": []}"], "empty name"),
        (
            vec!["--filter-profile", r#"mine={"include": [{"role": "a"}]}"#],
            "role",
        ),
        (
            vec!["--filter-agentgraph", r#"{"include": [{"role": "a"}]}"#],
            "role",
        ),
        (vec!["--filter-vcs", r#"{"exclude": [{}]}"#], "exclude"),
        (vec!["--filter-vcs", "no/such/filter.yaml"], "no/such"),
    ] {
        let mut args = vec!["start", &plan, "--attach"];
        args.extend_from_slice(&flags);
        let refused = world.run(&args);
        refused.exited(REFUSED);
        assert!(
            refused.stderr.contains(named),
            "the refusal for {flags:?} does not name {named:?}:\n{}",
            refused.stderr
        );
    }

    // And none of them left a run behind: every refusal happened before the run
    // was minted, so `runs` — the view that lists what this host holds — has
    // nothing to show and there is nothing to adopt or clean up.
    let listed = world.run(&["runs"]);
    listed.exited(0);
    assert!(
        !listed.stdout.contains("refused"),
        "a refused launch minted a run anyway:\n{}",
        listed.stdout
    );
}

/// A launch that composes an **installed** `oneagentgraph` spells the filter onto
/// its command line.
///
/// The third way the same filter travels, and the one no other journey reaches.
/// This build normally runs the sibling as a library, or self-execs its own
/// `drive` for a launch it must not stay to read; `dispatch.rs` proves both
/// against the real binary. The remaining path is the one an operator takes with
/// `ONEPIPELINE_ONEAGENTGRAPH_BIN` — the all-or-nothing override that composes
/// whatever the host installed — where the filter has to become the
/// `--event-filter` argument that sibling's own CLI takes.
///
/// What reaches the argv is the **rendered value**, never the spec that was
/// typed: the filter here is named as a file, which would mean nothing to a
/// sibling running somewhere else, and a launch that passed the path on would
/// also be re-reading a file that may have changed since the launch that
/// validated it.
#[test]
fn a_launch_composing_an_installed_sibling_spells_the_filter_onto_its_argv() {
    let world = World::new("filter-argv");
    let spec = world.root.join("no-turns.yaml");
    std::fs::write(&spec, "exclude:\n  - kind: \"turn-*\"\n").expect("the spec is written");

    let run = settled(
        &world,
        "spelled",
        &["--filter-agentgraph", &spec.to_string_lossy()],
    );
    let _ = run;

    // llmlint: ignore-block[tests_mirror_real_usage] an overridden sibling is a foreign
    // binary this world stands in for, so the command line it was handed is the only
    // place this claim exists: no product surface of *this* crate renders the argv of a
    // process it started, and the events that came back would be the installed
    // sibling's answer rather than the ask. `channel.rs` asserts the pacemaker's
    // addressing the same way and for the same reason.
    let spelled = world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneagentgraph")
        .filter_map(|call| {
            let args = call["args"].as_array()?.clone();
            let at = args.iter().position(|arg| arg == "--event-filter")?;
            args.get(at + 1)?.as_str().map(str::to_string)
        })
        .collect::<Vec<String>>();
    assert!(
        !spelled.is_empty(),
        "no launch spelled `--event-filter` onto the sibling it composed: {:?}",
        world.invocations()
    );
    for value in &spelled {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(value).expect("the argument is a document"),
            serde_json::json!({"exclude": [{"kind": "turn-*"}]}),
            "the launch passed something other than the filter it validated"
        );
    }
    // llmlint: ignore-end[tests_mirror_real_usage]
}

/// A `next` that refuses its filter leaves the surface unclaimed.
///
/// The order matters and nothing else states it: `next` consumes a surface, and
/// consumption is not undoable — so a spec this build will not honour has to be
/// refused *before* the claim, or a planner who mistyped a matcher would spend
/// the question they were about to answer on an exit 2.
#[test]
fn a_next_that_refuses_its_filter_leaves_the_surface_unclaimed() {
    let world = World::new("filter-claim-order");
    let run = settled(&world, "unclaimed", &[]);
    let mut serving = raise_blocker(&world, &run);

    let refused = world.run(&["next", &run, "--filter", r#"{"include": [{"role": "a"}]}"#]);
    refused.exited(REFUSED);
    assert!(refused.stderr.contains("role"), "{}", refused.stderr);

    // Still waiting, and still the planner's to read.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("planner update(s) waiting");
    let read = world.run(&["next", &run]);
    read.exited(0);
    assert_eq!(
        read.json()["surface"]["blocking"],
        serde_json::json!(true),
        "a refused read spent the surface it refused to render:\n{}",
        read.stdout
    );

    drop(serving.stdin.take());
    let _ = serving.wait();
}
