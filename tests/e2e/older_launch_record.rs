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

use crate::recorded_channel::{recorded, recorded_world, seeded_with, RUN};
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
    world
        .run(&["status", RUN])
        .exited(0)
        .out_has(&format!("{RUN}  SETTLED  1/1 done"));
}
