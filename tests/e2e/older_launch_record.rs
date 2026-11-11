//! A settled run whose launch record an older build wrote, read by this one.
//!
//! `onemessagebus` 0.7 made every codec of a bus configuration name its
//! `select` and its `frames`, and a launch record keeps the configuration its
//! run was launched under. So a record written before that carries codecs this
//! build's bus types call incomplete, and every build from 0.37.0 until this one
//! refused the whole record over it: every view of the settled run failed, and its success hook —
//! the automation that launches the follow-up run — never fired (issue #380).
//!
//! The record here is built from real ones rather than by hand: the recorded run
//! root `onemessagebus-repair-2`, with the `bus_config` a run on this host was
//! launched under on 2026-09-17 spliced into it verbatim
//! (`tests/recorded/launch/`, whose README says where it came from), and the run
//! pointed at a real hook. Over it, this build's `status`, `results` and `adopt`
//! read the run, the success hook fires, and the record's bus configuration is
//! left exactly as the older build wrote it.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The hook is the operator's own command, and this suite supplies a real
// one. `harness.rs` carries the same suppression and the full rationale.

use serde_json::{json, Value};

use crate::harness::REFUSED;
use crate::recorded_support::{recorded, recorded_world, seeded_with, RUN};
use crate::run_end_hooks::{hook, invocations, records, HOOK_TIMEOUT, RECORD_ENV};

/// The real launch record whose bus configuration predates `select` and `frames`.
const OLDER: &str = "launch/otg-closed-state-writes-status.json";

/// The session the recorded launch record names as the run's owner, which
/// `adopt` has to be run as.
fn owner() -> String {
    let launch: Value = serde_json::from_str(
        &std::fs::read_to_string(recorded(&format!("run-root/{RUN}/launch.json")))
            .expect("the recorded launch record reads"),
    )
    .expect("the recorded launch record is JSON");
    launch["session"]
        .as_str()
        .expect("the recorded launch record names its session")
        .to_owned()
}

/// The older record's bus configuration, as it was written.
fn older_bus_config() -> Value {
    let older: Value = serde_json::from_str(
        &std::fs::read_to_string(recorded(OLDER)).expect("the older launch record reads"),
    )
    .expect("the older launch record is JSON");
    let config = older["bus_config"].clone();
    let codecs = config["codecs"]
        .as_object()
        .expect("the older record's bus configuration names codecs");
    assert!(
        codecs
            .values()
            .all(|codec| codec.get("select").is_none() && codec.get("frames").is_none()),
        "the recorded bus configuration is not one written before codecs named `select` and \
         `frames`: {config}"
    );
    config
}

#[test]
fn a_settled_run_whose_record_an_older_build_wrote_is_read_adopted_and_fires_its_hook() {
    let world = recorded_world("older-launch-record");
    std::fs::create_dir_all(records(&world)).expect("a record directory");
    let record_dir = records(&world).to_string_lossy().into_owned();
    let world = world.with_env(RECORD_ENV, &record_dir);
    let hook = hook(&world);
    seeded_with(&world, Some(&recorded(&format!("channel/{RUN}"))));

    // The world's copy of the recorded record, carrying the older bus
    // configuration and a real hook, in a directory that exists on this host.
    let launch = world.runs.join(RUN).join("launch.json");
    let mut record: Value =
        serde_json::from_str(&std::fs::read_to_string(&launch).expect("the seeded record"))
            .expect("the seeded record is JSON");
    let fields = record.as_object_mut().expect("a record object");
    fields.insert("dir".into(), json!(world.project));
    fields.insert("success_hook".into(), json!(hook));
    fields.insert("failure_hook".into(), json!(hook));
    fields.insert(
        "hook_timeout".into(),
        json!(HOOK_TIMEOUT.parse::<u64>().expect("a timeout")),
    );
    fields.insert("bus_config".into(), older_bus_config());
    let written = serde_json::to_string_pretty(&record).expect("the record serializes");
    // llmlint: ignore[tests_mirror_real_usage] the state under test is a record an *older* build wrote, which no interface of this build can produce; it is seeded from recorded bytes as `recorded_channel` seeds a recorded channel, and everything after it is the user's own verbs.
    std::fs::write(&launch, &written).expect("the older record is written");

    // Both records carry the retired interval as the previous release wrote it.
    assert_eq!(record["heartbeat_interval"], json!(1_800), "{record}");
    assert!(
        world
            .events_of(RUN, "run-started")
            .iter()
            .all(|event| event["payload"]["heartbeat_interval"].is_u64()),
        "the seeded run-started does not carry the retired interval"
    );

    // Every view reads the run, and none of them rewrites its record.
    world
        .run(&["status", RUN])
        .exited(0)
        .out_has(&format!("{RUN}  SETTLED  1/1 done"));
    world
        .run(&["results", RUN])
        .exited(0)
        .out_has(&format!("{RUN}  complete"));
    assert_eq!(
        std::fs::read_to_string(&launch).expect("the record reads"),
        written,
        "reading the run rewrote its launch record"
    );

    // Adopted by its owner, the settled run is driven to its ending again, and
    // the success hook that ending names fires once and succeeds. The owner's
    // world is held for the rest of the journey, because a world dropped removes
    // the root it shares.
    let owner = world.as_session(&owner());
    owner
        .run(&["adopt", RUN])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");
    let finished = world.events_of(RUN, "run-hook-finished");
    assert_eq!(finished.len(), 1, "{finished:?}");
    assert_eq!(invocations(&world, RUN), ["success"], "{}", world.dump());
    assert_eq!(
        (
            &finished[0]["payload"]["hook"],
            &finished[0]["payload"]["ending"]
        ),
        (&json!("success"), &json!("succeeded"))
    );

    // `adopt` rewrote the record for its own fields, and the bus configuration it
    // carries is still the older build's, with nothing filled in.
    let record = world.run_json(RUN, "launch.json");
    assert_eq!(record["adoptions"], json!(1));
    assert_eq!(record["bus_config"], older_bus_config());
    assert_eq!(
        record["heartbeat_interval"],
        json!(1_800),
        "adoption rewrote the record without the interval it carried"
    );
    world
        .run(&["status", RUN])
        .exited(0)
        .out_has(&format!("{RUN}  SETTLED  1/1 done"));
}

/// The older record's bus configuration exactly as that build wrote it: the
/// bytes of its `bus_config` value, indented where it sits in the record.
fn older_bus_config_text() -> String {
    let older = std::fs::read_to_string(recorded(OLDER)).expect("the older launch record reads");
    bus_config_text(&older)
}

/// The bytes of a launch record's `bus_config` value, as they stand in `record`.
fn bus_config_text(record: &str) -> String {
    let key = "\n  \"bus_config\": ";
    let start = record
        .find(key)
        .expect("the record names a bus configuration")
        + key.len();
    let mut depth = 0_u32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in record[start..].bytes().enumerate() {
        match (in_string, escaped, byte) {
            (true, true, _) => escaped = false,
            (true, false, b'\\') => escaped = true,
            (true, false, b'"') | (false, _, b'"') => in_string = !in_string,
            (false, _, b'{' | b'[') => depth += 1,
            (false, _, b'}' | b']') => {
                depth -= 1;
                if depth == 0 {
                    return record[start..=start + offset].to_owned();
                }
            }
            _ => {}
        }
    }
    panic!("the record's bus configuration never closes: {record}");
}

/// What the channel holds: every queue log, by name.
fn channel_logs(world: &crate::harness::World) -> Vec<(String, String)> {
    let channel = world.runs.join(RUN).join("channel");
    let mut logs: Vec<(String, String)> = std::fs::read_dir(&channel)
        .expect("the run's channel")
        .map(|entry| entry.expect("a channel file").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .map(|path| {
            (
                path.file_name()
                    .expect("a file name")
                    .to_string_lossy()
                    .into_owned(),
                std::fs::read_to_string(&path).expect("a queue log reads"),
            )
        })
        .collect();
    logs.sort();
    logs
}

/// What every refusal of an offer to a run launched with validators says: that
/// the run was launched with them, that this engine no longer runs them, and
/// what to do instead.
const LAUNCHED_WITH_VALIDATORS: [&str; 3] = [
    "launched with onemessagebus validators",
    "this engine no longer runs",
    "let the run settle, or stop it",
];

/// A run an older build launched with `validators:` in its bus configuration —
/// which `onemessagebus` 0.11 removed — stays readable by every view and by
/// `adopt`, which writes its bus configuration back byte for byte; but nothing
/// those validators would have judged is taken: a reply carrying commands, a
/// verdict carrying none, a surface a verb raises and the surfaces the engine
/// raises while `adopt` drives the run are each refused, saying why and what to
/// do, and the run's channel holds nothing new afterwards.
#[test]
fn a_run_launched_with_validators_is_read_and_adopted_and_refuses_every_offer() {
    let world = recorded_world("launched-with-validators");
    seeded_with(&world, Some(&recorded(&format!("channel/{RUN}"))));

    // The world's copy of the recorded record, its bus configuration spliced in
    // as the older build wrote it, byte for byte.
    let launch = world.runs.join(RUN).join("launch.json");
    let mut record: Value =
        serde_json::from_str(&std::fs::read_to_string(&launch).expect("the seeded record"))
            .expect("the seeded record is JSON");
    let fields = record.as_object_mut().expect("a record object");
    fields.insert("dir".into(), json!(world.project));
    fields.remove("bus_config");
    let recorded_config = older_bus_config_text();
    assert!(
        recorded_config.contains("\"validators\": ["),
        "the recorded bus configuration declares no validators: {recorded_config}"
    );
    let unspliced = serde_json::to_string_pretty(&record).expect("the record serializes");
    let written = format!(
        "{},\n  \"bus_config\": {recorded_config}\n}}",
        unspliced
            .strip_suffix("\n}")
            .expect("a pretty record closes on its own line")
    );
    // llmlint: ignore[tests_mirror_real_usage] the state under test is a record an *older* build wrote, with a key this build's bus no longer has, which no interface of this build can produce; it is seeded from recorded bytes as the journey above seeds its record, and everything after it is the user's own verbs.
    std::fs::write(&launch, &written).expect("the older record is written");

    // Every view reads the run, and none of them rewrites its record.
    world
        .run(&["status", RUN])
        .exited(0)
        .out_has(&format!("{RUN}  SETTLED  1/1 done"));
    world
        .run(&["results", RUN])
        .exited(0)
        .out_has(&format!("{RUN}  complete"));
    assert_eq!(
        std::fs::read_to_string(&launch).expect("the record reads"),
        written,
        "reading the run rewrote its launch record"
    );
    let held = channel_logs(&world);
    assert!(
        held.iter().any(|(_, log)| !log.is_empty()),
        "the seeded channel holds nothing to compare against: {held:?}"
    );

    // A reply carrying commands, a verdict carrying none, and a surface a verb
    // raises: each refused, saying why and what to do.
    let edit = json!({"version": 3, "commands": [
        {"op": "finding", "message": "the fixture is slow"}
    ]})
    .to_string();
    let verdict = json!({"completion": false, "reason": "carry on"}).to_string();
    for (args, stdin) in [
        (vec!["reply", RUN], edit.as_str()),
        (vec!["reply", RUN], verdict.as_str()),
        (
            vec!["surface", RUN, "--kind", "finding", "--message", "a report"],
            "",
        ),
    ] {
        let refused = world.run_with_stdin(&args, stdin);
        refused.exited(REFUSED);
        for said in LAUNCHED_WITH_VALIDATORS {
            assert!(
                refused.stderr.contains(said),
                "{args:?} was not refused saying {said:?}: {}",
                refused.stderr
            );
        }
    }
    assert_eq!(
        channel_logs(&world),
        held,
        "a refused offer reached the channel"
    );
    assert!(
        world.events_of(RUN, "planner-surface-dropped").is_empty(),
        "a refused verb was journalled"
    );

    // Adopted by its owner, the run is driven to its ending again; every surface
    // the engine raises on the way is refused for the same reason and journalled,
    // and the run settles regardless.
    let queued = world.events_of(RUN, "planner-surface-queued").len();
    let owner = world.as_session(&owner());
    owner
        .run(&["adopt", RUN])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");
    let dropped = world.events_of(RUN, "planner-surface-dropped");
    assert!(
        !dropped.is_empty(),
        "adopting the run raised no surface to refuse: {:?}",
        world.kinds(RUN)
    );
    for event in &dropped {
        let refused = event["payload"]["refused"].as_str().unwrap_or_default();
        for said in LAUNCHED_WITH_VALIDATORS {
            assert!(
                refused.contains(said),
                "an engine surface was not refused saying {said:?}: {event}"
            );
        }
    }
    assert_eq!(
        world.events_of(RUN, "planner-surface-queued").len(),
        queued,
        "adopting the run queued a surface"
    );
    assert_eq!(
        channel_logs(&world),
        held,
        "an engine surface reached the channel"
    );

    // `adopt` rewrote the record for its own fields, and its bus configuration is
    // still the older build's bytes, validators and all.
    let rewritten = std::fs::read_to_string(&launch).expect("the record reads");
    assert_ne!(rewritten, written, "adopt did not rewrite the record");
    assert_eq!(world.run_json(RUN, "launch.json")["adoptions"], json!(1));
    assert_eq!(bus_config_text(&rewritten), recorded_config);
    world
        .run(&["status", RUN])
        .exited(0)
        .out_has(&format!("{RUN}  SETTLED  1/1 done"));
}
